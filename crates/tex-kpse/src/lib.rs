//! Kpathsea-compatible TDS file resolution for the Rust TeX engine.
//!
//! Searches the standard TeX Live directory roots using their `ls-R`
//! filename databases, plus the current working directory for user files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// File format categories, each with its own TDS search subdirectories.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Format {
    Tex,     // .tex .ltx .cls .sty .clo .fd .dfu .cfg .def .ldf .dfu
    Tfm,     // .tfm
    Ofm,
    Type1,   // .pfb .pfa
    Truetype,
    Enc,     // .enc (glyph encoding files)
    Map,     // .map (pdftex map files)
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

    /// TDS subtrees searched for this format, most specific first.
    fn subdirs(&self) -> &'static [&'static str] {
        match self {
            Format::Tex => &["tex"],
            Format::Tfm => &["fonts/tfm"],
            Format::Ofm => &["fonts/ofm"],
            Format::Type1 => &["fonts/type1"],
            Format::Truetype => &["fonts/truetype"],
            Format::Otf => &["fonts/opentype"],
            Format::Enc => &["fonts/enc"],
            Format::Map => &["fonts/map"],
            Format::Bst => &["bibtex/bst"],
            Format::Bib => &["bibtex/bib"],
            Format::Cmap => &["fonts/cmap"],
            Format::Sfd => &["fonts/sfd"],
            Format::Fontmap => &["fonts/map"],
        }
    }
}

struct LsR {
    /// filename (lowercase-insensitive keyed as stored) -> list of directories (absolute or root-relative)
    db: HashMap<String, Vec<PathBuf>>,
}

pub struct Kpse {
    roots: Vec<PathBuf>,
    dbs: Vec<LsR>,
    /// extra paths from environment (e.g. TEXINPUTS), ':'-separated, may contain '//' suffix
    extra_paths: HashMap<Format, Vec<PathBuf>>,
}

impl Kpse {
    pub fn new() -> Self {
        let mut roots = Vec::new();
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
        let dbs = roots.iter().map(|r| Self::load_lsr(r)).collect();
        let extra_paths = Self::parse_extra_paths();
        Kpse { roots, dbs, extra_paths }
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

    fn load_lsr(root: &Path) -> LsR {
        let mut db: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for lsr_name in ["ls-R", "ls-R.lua"] {
            let path = root.join("ls-R");
            let path = if path.exists() { path } else { root.join(lsr_name) };
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
                    db.entry(fname.to_string()).or_insert_with(Vec::new).push(curdir.join(fname));
                }
            }
            break;
        }
        LsR { db }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Find a file of the given format. `name` may already carry an extension.
    pub fn find(&self, name: &str, fmt: Format) -> Option<PathBuf> {
        // 1. absolute path
        let p = Path::new(name);
        if p.is_absolute() {
            return if p.exists() { Some(p.to_path_buf()) } else { None };
        }
        // 2. cwd (and relative paths with directories)
        if p.exists() {
            return Some(PathBuf::from(name));
        }
        if name.contains('/') {
            return None;
        }
        // 3. extra env paths
        if let Some(paths) = self.extra_paths.get(&fmt) {
            for base in paths {
                if base.as_os_str() == "." && p.exists() {
                    return Some(PathBuf::from(name));
                }
                if let Some(hit) = try_ext(base.join(name), fmt) {
                    return Some(hit);
                }
            }
        }
        // 4. ls-R databases: exact name, then with each extension appended
        let mut candidates: Vec<String> = vec![name.to_string()];
        if !fmt.extensions().iter().any(|e| name.ends_with(e)) {
            for e in fmt.extensions() {
                candidates.push(format!("{}{}", name, e));
            }
        }
        for (root, db) in self.roots.iter().zip(&self.dbs) {
            for cand in &candidates {
                if let Some(dirs) = db.db.get(cand.as_str()) {
                    for rel in dirs {
                        let full = root.join(rel);
                        if full.exists() {
                            return Some(full);
                        }
                    }
                }
                // direct subtree probe (ls-R should cover, but be safe)
                for sub in fmt.subdirs() {
                    if let Some(hit) = try_ext(root.join(sub).join(cand), fmt) {
                        return Some(hit);
                    }
                }
            }
        }
        None
    }

    /// kpsewhich-style: find any file by name across all databases.
    pub fn find_any(&self, name: &str) -> Option<PathBuf> {
        if Path::new(name).is_absolute() {
            return if Path::new(name).exists() { Some(PathBuf::from(name)) } else { None };
        }
        if Path::new(name).exists() {
            return Some(PathBuf::from(name));
        }
        for (root, db) in self.roots.iter().zip(&self.dbs) {
            if let Some(dirs) = db.db.get(name) {
                for rel in dirs {
                    let full = root.join(rel);
                    if full.exists() {
                        return Some(full);
                    }
                }
            }
        }
        None
    }

    pub fn read(&self, name: &str, fmt: Format) -> Option<Vec<u8>> {
        self.find(name, fmt).and_then(|p| std::fs::read(p).ok())
    }
}

fn try_ext(path: PathBuf, _fmt: Format) -> Option<PathBuf> {
    if path.exists() {
        Some(path)
    } else {
        None
    }
}
