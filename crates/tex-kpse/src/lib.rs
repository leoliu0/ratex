//! Kpathsea-compatible TDS file resolution for the Rust TeX engine.
//!
//! Searches the standard TeX Live directory roots using their `ls-R`
//! filename databases, plus the current working directory for user files.
//! Lookup order mirrors kpathsea: the local directory is searched first with
//! every format extension, then extra environment paths (`TEXINPUTS` & co),
//! then each TDS root via its `ls-R` database (exact, then
//! case-insensitively) and finally by recursively walking the format's TDS
//! subtrees (`tex/latex//`, `fonts/tfm//`, ...).

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// Upper bound on entries inspected by a single recursive directory walk.
const WALK_BUDGET: usize = 100_000;

/// File format categories, each with its own TDS search subdirectories.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Format {
    Tex,   // .tex .ltx .cls .sty .clo .fd .dfu .cfg .def .ldf
    Tfm,   // .tfm
    Ofm,
    Type1, // .pfb .pfa
    Truetype,
    Enc,   // .enc (glyph encoding files)
    Map,   // .map (pdftex map files)
    Bst,
    Bib,
    Cmap,
    Sfd,
    Otf,
    Fontmap,
}

impl Format {
    pub fn extensions(&self) -> &'static [&'static str] {
        match self {
            Format::Tex => &[".tex", ".ltx", ".sty", ".cls", ".clo", ".fd", ".dfu", ".cfg", ".def", ".ldf", ".texi"],
            Format::Tfm => &[".tfm"],
            Format::Ofm => &[".ofm"],
            Format::Type1 => &[".pfb", ".pfa"],
            Format::Truetype => &[".ttf", ".ttc", ".otf"],
            Format::Enc => &[".enc"],
            Format::Map => &[".map"],
            Format::Bst => &[".bst"],
            Format::Bib => &[".bib"],
            Format::Cmap => &[".cmap"],
            Format::Sfd => &[".sfd"],
            Format::Otf => &[".otf"],
            Format::Fontmap => &[".map"],
        }
    }

    /// TDS search specs for this format in kpathsea `texmf.cnf` style: a
    /// trailing `//` marks the subtree as searched recursively
    /// (`tex/latex//` = every directory below `tex/latex`).
    /// Most specific first.
    fn tds_paths(&self) -> &'static [&'static str] {
        match self {
            Format::Tex => &["tex/latex//", "tex/generic//", "tex/plain//", "tex//"],
            Format::Tfm => &["fonts/tfm//"],
            Format::Ofm => &["fonts/ofm//"],
            Format::Type1 => &["fonts/type1//"],
            Format::Truetype => &["fonts/truetype//"],
            Format::Otf => &["fonts/opentype//"],
            Format::Enc => &["fonts/enc//"],
            Format::Map | Format::Fontmap => &["fonts/map//"],
            Format::Bst => &["bibtex/bst//"],
            Format::Bib => &["bibtex/bib//"],
            Format::Cmap => &["fonts/cmap//"],
            Format::Sfd => &["fonts/sfd//"],
        }
    }
}

struct LsR {
    /// filename (as stored) -> root-relative paths
    db: HashMap<String, Vec<PathBuf>>,
    /// lowercased filename -> root-relative paths (case-insensitive fallback)
    lower: HashMap<String, Vec<PathBuf>>,
}

impl LsR {
    fn new() -> Self {
        LsR { db: HashMap::new(), lower: HashMap::new() }
    }

    fn insert(&mut self, fname: &str, rel: PathBuf) {
        self.db.entry(fname.to_string()).or_default().push(rel.clone());
        self.lower.entry(fname.to_lowercase()).or_default().push(rel);
    }

    /// Exact match first, then the case-insensitive fallback.
    fn get(&self, name: &str) -> Option<&Vec<PathBuf>> {
        self.db.get(name).or_else(|| self.lower.get(&name.to_lowercase()))
    }
}

pub struct Kpse {
    roots: Vec<PathBuf>,
    dbs: Vec<RefCell<Option<LsR>>>,
    /// extra paths from environment (e.g. TEXINPUTS), ':'-separated, may contain '//' suffix
    extra_paths: HashMap<Format, Vec<PathBuf>>,
    /// Directory with the highest lookup precedence (normally the process cwd).
    cwd: PathBuf,
    /// Results of recursive subtree walks, cached per (root, format, name).
    walk_cache: RefCell<HashMap<(usize, Format, String), Option<PathBuf>>>,
}

impl Default for Kpse {
    fn default() -> Self {
        Self::new()
    }
}

impl Kpse {
    /// Build a resolver rooted at the process working directory.
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::with_roots(&cwd, &[])
    }

    /// Build a resolver with an explicit root list (no system defaults).
    pub fn explicit(cwd: &Path, roots: Vec<PathBuf>) -> Self {
        let dbs = roots.iter().map(|_| RefCell::new(None)).collect();
        Kpse {
            roots,
            dbs,
            extra_paths: HashMap::new(),
            cwd: cwd.to_path_buf(),
            walk_cache: RefCell::new(HashMap::new()),
        }
    }

    /// Build a resolver with an explicit local directory. `extra_roots` are
    /// TDS trees searched before the default roots (TEXMFLOCAL-style
    /// precedence); an empty slice keeps the defaults.
    pub fn with_roots(cwd: &Path, extra_roots: &[&Path]) -> Self {
        let mut roots: Vec<PathBuf> = extra_roots.iter().map(|p| p.to_path_buf()).collect();
        if let Ok(home) = std::env::var("TEXMFHOME") {
            roots.push(PathBuf::from(home));
        } else if let Ok(home) = std::env::var("HOME") {
            roots.push(PathBuf::from(home).join("texmf"));
        }
        for env in ["TEXMFLOCAL", "TEXMFDIST"] {
            if let Ok(v) = std::env::var(env) {
                for p in v.split(':') {
                    if !p.is_empty() && !roots.iter().any(|r| r == &PathBuf::from(p)) {
                        roots.push(PathBuf::from(p));
                    }
                }
            }
        }
        for p in [
            "/var/lib/texmf",
            "/usr/share/texmf-dist",
            "/usr/share/texmf",
            "/usr/local/share/texmf",
            "/usr/local/texlive",
        ] {
            if Path::new(p).exists() && !roots.iter().any(|r| r.as_os_str() == p) {
                roots.push(PathBuf::from(p));
            }
        }
        let dbs = roots.iter().map(|_| RefCell::new(None)).collect();
        let extra_paths = Self::parse_extra_paths();
        Kpse {
            roots,
            dbs,
            extra_paths,
            cwd: cwd.to_path_buf(),
            walk_cache: RefCell::new(HashMap::new()),
        }
    }

    fn parse_extra_paths() -> HashMap<Format, Vec<PathBuf>> {
        let mut m = HashMap::new();
        let add = |m: &mut HashMap<Format, Vec<PathBuf>>, fmts: &[Format], var: &str| {
            if let Ok(v) = std::env::var(var) {
                for p in v.split(':') {
                    if p.is_empty() {
                        continue;
                    }
                    for f in fmts {
                        m.entry(*f).or_insert_with(Vec::new).push(PathBuf::from(p));
                    }
                }
            }
        };
        add(&mut m, &[Format::Tex], "TEXINPUTS");
        add(&mut m, &[Format::Tfm], "TFMFONTS");
        add(&mut m, &[Format::Type1, Format::Truetype, Format::Otf], "TTFONTS");
        add(&mut m, &[Format::Type1], "T1FONTS");
        add(&mut m, &[Format::Type1, Format::Truetype, Format::Otf], "OPENTYPEFONTS");
        add(&mut m, &[Format::Enc], "ENCFONTS");
        add(&mut m, &[Format::Map, Format::Fontmap], "TEXFONTMAPS");
        add(&mut m, &[Format::Bst], "BSTINPUTS");
        add(&mut m, &[Format::Bib], "BIBINPUTS");
        m
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Find a file of the given format. `name` may already carry an extension.
    pub fn find(&self, name: &str, fmt: Format) -> Option<PathBuf> {
        // 0. absolute path
        let p = Path::new(name);
        if p.is_absolute() {
            return if p.exists() { Some(p.to_path_buf()) } else { None };
        }
        // Candidate names: as given, plus with each format extension appended
        // when the bare name carries none of the format's extensions.
        let mut candidates: Vec<String> = vec![name.to_string()];
        if !fmt.extensions().iter().any(|e| name.ends_with(e)) {
            candidates.extend(fmt.extensions().iter().map(|e| format!("{name}{e}")));
        }
        // 1. the local directory has the highest precedence
        for cand in &candidates {
            let p = self.cwd.join(cand);
            if p.exists() {
                return Some(p);
            }
        }
        // 2. a name with a directory part is only probed against the roots
        if name.contains('/') {
            for root in &self.roots {
                let full = root.join(name);
                if full.exists() {
                    return Some(clean(full));
                }
            }
            return None;
        }
        // 3. extra env paths (TEXINPUTS & co)
        if let Some(paths) = self.extra_paths.get(&fmt) {
            for base in paths {
                for cand in &candidates {
                    let p = base.join(cand);
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }
        // 4. TDS roots: ls-R database (exact, then case-insensitive), then a
        //    recursive walk of the format's subtrees (`tex/latex//` style)
        for i in 0..self.roots.len() {
            let db = self.db_of(i);
            for cand in &candidates {
                if let Some(dirs) = db.get(cand) {
                    for rel in dirs {
                        let full = self.roots[i].join(rel);
                        if full.exists() {
                            return Some(clean(full));
                        }
                    }
                }
            }
            for cand in &candidates {
                if let Some(hit) = self.walk_cached(i, fmt, cand) {
                    return Some(hit);
                }
            }
        }
        None
    }

    /// The root's `ls-R` database, parsed on first lookup that consults it
    /// (keeps resolver construction O(#roots) instead of O(#entries)).
    fn db_of(&self, i: usize) -> Ref<'_, LsR> {
        if self.dbs[i].borrow().is_some() {
            return Ref::map(self.dbs[i].borrow(), |o| o.as_ref().unwrap());
        }
        let parsed = load_lsr(&self.roots[i]);
        let mut slot = self.dbs[i].borrow_mut();
        if slot.is_none() {
            *slot = Some(parsed);
        }
        drop(slot);
        Ref::map(self.dbs[i].borrow(), |o| o.as_ref().unwrap())
    }

    /// kpsewhich-style: find any file by name across all databases.
    pub fn find_any(&self, name: &str) -> Option<PathBuf> {
        if Path::new(name).is_absolute() {
            return if Path::new(name).exists() { Some(PathBuf::from(name)) } else { None };
        }
        let local = self.cwd.join(name);
        if local.exists() {
            return Some(local);
        }
        for i in 0..self.roots.len() {
            let db = self.db_of(i);
            if let Some(dirs) = db.get(name) {
                for rel in dirs {
                    let full = self.roots[i].join(rel);
                    if full.exists() {
                        return Some(clean(full));
                    }
                }
            }
        }
        None
    }

    /// Resolve a font definition file, e.g. `find_fd("t1", "cmr")` for
    /// `t1cmr.fd`. Case-insensitive on the stored filename.
    pub fn find_fd(&self, encoding: &str, family: &str) -> Option<PathBuf> {
        self.find(&format!("{encoding}{family}.fd"), Format::Tex)
    }

    pub fn read(&self, name: &str, fmt: Format) -> Option<Vec<u8>> {
        self.find(name, fmt).and_then(|p| std::fs::read(p).ok())
    }

    fn walk_cached(&self, root_idx: usize, fmt: Format, cand: &str) -> Option<PathBuf> {
        let key = (root_idx, fmt, cand.to_string());
        if let Some(hit) = self.walk_cache.borrow().get(&key) {
            return hit.clone();
        }
        let hit = walk_find(&self.roots[root_idx], fmt, cand);
        self.walk_cache.borrow_mut().insert(key, hit.clone());
        hit
    }
}

/// Recursive fallback search for `cand` under `root`'s TDS subtrees for
/// `fmt`, honoring the `//` recursive markers of [`Format::tds_paths`].
/// Prefers an exact filename match; otherwise returns the first
/// case-insensitive match.
fn walk_find(root: &Path, fmt: Format, cand: &str) -> Option<PathBuf> {
    let cand_lower = cand.to_lowercase();
    let mut ci_hit: Option<PathBuf> = None;
    let mut budget = WALK_BUDGET;
    let mut visited: HashSet<PathBuf> = HashSet::new();
    for spec in fmt.tds_paths() {
        let (sub, recursive) = match spec.strip_suffix("//") {
            Some(sub) => (sub, true),
            None => (*spec, false),
        };
        let start = root.join(sub);
        if !start.is_dir() {
            continue;
        }
        if !recursive {
            let hit = start.join(cand);
            if hit.is_file() {
                return Some(hit);
            }
            continue;
        }
        let mut stack = vec![start];
        while let Some(dir) = stack.pop() {
            if budget == 0 {
                return ci_hit;
            }
            budget -= 1;
            // Canonicalize to break symlink cycles.
            let Ok(canon) = dir.canonicalize() else { continue };
            if !visited.insert(canon) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(ft) = entry.file_type() else { continue };
                let path = entry.path();
                if ft.is_dir() {
                    stack.push(path);
                    continue;
                }
                if name == cand {
                    return Some(path);
                }
                if ci_hit.is_none() && name.to_lowercase() == cand_lower {
                    ci_hit = Some(path);
                }
            }
        }
    }
    ci_hit
}

/// Parse a root's `ls-R` filename database.
fn load_lsr(root: &Path) -> LsR {
    let mut lsr = LsR::new();
    for name in ["ls-R", "ls-R.lua"] {
        let path = root.join(name);
        if !path.exists() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut curdir = PathBuf::new();
            for line in text.lines() {
                let line = line.trim_end();
                if line.is_empty() || line.starts_with('%') {
                    continue;
                }
                if line.ends_with(':') && !line.starts_with(char::is_whitespace) {
                    curdir = PathBuf::from(&line[..line.len() - 1]);
                    continue;
                }
                let fname = line.trim();
                if fname.is_empty() {
                    continue;
                }
                lsr.insert(fname, curdir.join(fname));
            }
        }
        break; // first database found wins
    }
    lsr
}

/// Drop redundant `.` components from a joined path.
fn clean(p: PathBuf) -> PathBuf {
    p.components()
        .filter(|c| *c != Component::CurDir)
        .fold(PathBuf::new(), |mut out, c| {
            out.push(c.as_os_str());
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh unique directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            for i in 0..1000 {
                let p = std::env::temp_dir()
                    .join(format!("tex-kpse-{tag}-{}-{i}", std::process::id()));
                match std::fs::create_dir(&p) {
                    Ok(()) => return TempDir(p),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("create_dir {p:?}: {e}"),
                }
            }
            panic!("could not create temp dir for {tag}");
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, rel: &str, contents: &str) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, contents).unwrap();
            p
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dist_root() -> Option<&'static Path> {
        let p = Path::new("/usr/share/texmf-dist");
        p.is_dir().then_some(p)
    }

    fn project_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../trust_own")
    }

    #[test]
    fn local_directory_has_highest_precedence() {
        let tmp = TempDir::new("local");
        for (name, marker) in [
            ("article.cls", "% local class"),
            ("main.tex", "% local tex"),
            ("mypkg.sty", "% local sty"),
            ("refs.bib", "% local bib"),
            ("rfs.bst", "% local bst"),
        ] {
            tmp.write(name, marker);
        }
        let kpse = Kpse::with_roots(tmp.path(), &[]);
        assert_eq!(kpse.find("article.cls", Format::Tex), Some(tmp.path().join("article.cls")));
        assert_eq!(kpse.find("main.tex", Format::Tex), Some(tmp.path().join("main.tex")));
        assert_eq!(kpse.find("mypkg.sty", Format::Tex), Some(tmp.path().join("mypkg.sty")));
        assert_eq!(kpse.find("refs.bib", Format::Bib), Some(tmp.path().join("refs.bib")));
        assert_eq!(kpse.find("rfs.bst", Format::Bst), Some(tmp.path().join("rfs.bst")));
        assert_eq!(kpse.read("rfs.bst", Format::Bst).unwrap(), b"% local bst");
    }

    #[test]
    fn resolves_system_class_and_package() {
        let Some(root) = dist_root() else { return };
        let kpse = Kpse::new();
        let cls = kpse.find("article.cls", Format::Tex).expect("article.cls");
        assert!(cls.is_file());
        assert!(cls.starts_with(root), "{cls:?}");
        assert!(cls.ends_with("tex/latex/base/article.cls"), "{cls:?}");
        let sty = kpse.find("newtx.sty", Format::Tex).expect("newtx.sty");
        assert!(sty.is_file());
        assert!(sty.ends_with("tex/latex/newtx/newtx.sty"), "{sty:?}");
    }

    #[test]
    fn resolves_font_definitions() {
        let Some(root) = dist_root() else { return };
        let kpse = Kpse::new();
        let cmr = kpse.find("t1cmr.fd", Format::Tex).expect("t1cmr.fd");
        assert!(cmr.is_file());
        assert!(cmr.starts_with(root));
        // Case-insensitive fallback on a lowercased request.
        let up = kpse.find("T1CMR.FD", Format::Tex).expect("T1CMR.FD fallback");
        assert!(up.is_file());
        // newtx math metrics fd resolves as-is.
        let mi = kpse.find("omlntxmi.fd", Format::Tex).expect("omlntxmi.fd");
        assert!(mi.is_file());
        // find_fd convenience for encoding + family pairs.
        let tlf = kpse.find_fd("t1", "ntxtlf").expect("t1ntxtlf.fd via find_fd");
        assert!(tlf.is_file());
    }

    #[test]
    fn fd_walk_fallback_without_ls_r() {
        let tmp = TempDir::new("fdwalk");
        let want = tmp.write("tex/latex/oldnewtx/t1ntxtext.fd", "% dummy fd");
        let kpse = Kpse::explicit(tmp.path(), vec![tmp.path().to_path_buf()]);
        assert_eq!(kpse.find("t1ntxtext.fd", Format::Tex), Some(want));
    }

    #[test]
    fn resolves_tfm_and_walk_fallback() {
        let kpse = Kpse::new();
        let tfm = kpse.find("txr.tfm", Format::Tfm).expect("txr.tfm");
        assert!(tfm.is_file());
        assert!(tfm.ends_with("fonts/tfm/public/txfonts/txr.tfm"), "{tfm:?}");

        // A tree without ls-R is still searched via the fonts/tfm// walk.
        let tmp = TempDir::new("tfm");
        let want = tmp.write("fonts/tfm/public/txfonts/txr.tfm", "fake tfm");
        let kpse2 = Kpse::explicit(tmp.path(), vec![tmp.path().to_path_buf()]);
        assert_eq!(kpse2.find("txr.tfm", Format::Tfm), Some(want));
    }

    #[test]
    fn case_insensitive_ls_r_fallback() {
        let tmp = TempDir::new("ci");
        let want = tmp.write("fonts/tfm/misc/TxR.TFM", "fake");
        std::fs::write(
            tmp.path().join("ls-R"),
            "./:\nfonts\n./fonts/tfm/misc:\nTxR.TFM\n",
        )
        .unwrap();
        let kpse = Kpse::explicit(tmp.path(), vec![tmp.path().to_path_buf()]);
        assert_eq!(kpse.find("txr.tfm", Format::Tfm), Some(want));
    }

    #[test]
    fn resolves_bst_and_bib() {
        if dist_root().is_some() {
            let kpse = Kpse::new();
            let plain = kpse.find("plain.bst", Format::Bst).expect("plain.bst");
            assert!(plain.is_file());
            assert!(plain.ends_with("bibtex/bst/base/plain.bst"), "{plain:?}");
        }
        // Project-local bibliography style and database win via cwd.
        let proj = project_dir();
        if !proj.is_dir() {
            return;
        }
        let kpse = Kpse::with_roots(&proj, &[]);
        let rfs = kpse.find("rfs.bst", Format::Bst).expect("local rfs.bst");
        assert_eq!(rfs, proj.join("rfs.bst"));
        let bib = kpse.find("references", Format::Bib).expect("references.bib");
        assert_eq!(bib, proj.join("references.bib"));
    }

    #[test]
    fn bst_walk_fallback_without_ls_r() {
        let tmp = TempDir::new("bstwalk");
        let want = tmp.write("bibtex/bst/misc/rfs.bst", "% dummy bst");
        let kpse = Kpse::explicit(tmp.path(), vec![tmp.path().to_path_buf()]);
        assert_eq!(kpse.find("rfs.bst", Format::Bst), Some(want));
    }

    #[test]
    fn resolves_main_tex_requirements() {
        let Some(root) = dist_root() else { return };
        let proj = project_dir();
        if !proj.is_dir() {
            return;
        }
        let kpse = Kpse::with_roots(&proj, &[]);
        // Local files: bibliography style + database.
        let rfs = kpse.find("rfs.bst", Format::Bst).expect("rfs.bst");
        assert!(rfs.starts_with(&proj));
        let bib = kpse.find("references.bib", Format::Bib).expect("references.bib");
        assert!(bib.starts_with(&proj));
        // Distribution files: every \\usepackage target plus the font
        // definition and metric files selected by the T1/newtx setup.
        let dist: &[(&str, Format)] = &[
            ("article.cls", Format::Tex),
            ("geometry.sty", Format::Tex),
            ("inputenc.sty", Format::Tex),
            ("fontenc.sty", Format::Tex),
            ("booktabs.sty", Format::Tex),
            ("setspace.sty", Format::Tex),
            ("caption.sty", Format::Tex),
            ("hyperref.sty", Format::Tex),
            ("natbib.sty", Format::Tex),
            ("graphicx.sty", Format::Tex),
            ("amsmath.sty", Format::Tex),
            ("amsthm.sty", Format::Tex),
            ("xcolor.sty", Format::Tex),
            ("longtable.sty", Format::Tex),
            ("array.sty", Format::Tex),
            ("newtx.sty", Format::Tex),
            ("t1cmr.fd", Format::Tex),
            ("t1ntxtlf.fd", Format::Tex),
            ("ts1ntxtlf.fd", Format::Tex),
            ("omlntxmi.fd", Format::Tex),
            ("txr.tfm", Format::Tfm),
        ];
        for (name, fmt) in dist {
            let p = kpse
                .find(name, *fmt)
                .unwrap_or_else(|| panic!("{name} did not resolve"));
            assert!(p.is_file(), "{name} -> {p:?} is not a file");
            assert!(p.starts_with(root) || p.starts_with(&proj), "{p:?} outside search roots");
        }
    }

    #[test]
    fn find_any_still_works() {
        let kpse = Kpse::new();
        if dist_root().is_some() {
            let p = kpse.find_any("article.cls").expect("find_any article.cls");
            assert!(p.is_file());
        }
        // find_any also honors the local directory.
        let tmp = TempDir::new("any");
        let want = tmp.write("notes.tex", "% notes");
        let kpse2 = Kpse::with_roots(tmp.path(), &[]);
        assert_eq!(kpse2.find_any("notes.tex"), Some(want));
    }

}
