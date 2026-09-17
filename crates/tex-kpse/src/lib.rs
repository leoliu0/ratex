//! Kpathsea-compatible TDS file resolution for the Rust TeX engine.
//!
//! Searches the standard TeX Live directory roots using their `ls-R`
//! filename databases, plus the current working directory for user files.

// Independently compressed chunks and a sorted member index are generated
// once at build time. Runtime lookup inflates only the containing chunk, while
// related small files still share enough context for effective compression.
static PACKAGES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/packages.bin"));
include!(concat!(env!("OUT_DIR"), "/packages_index.rs"));

const CHUNK_CACHE_CAPACITY: usize = 4;
type CachedChunk = (usize, std::sync::Arc<[u8]>);
static CHUNK_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<CachedChunk>>> =
    std::sync::OnceLock::new();

fn package_entry(filename: &str) -> Option<usize> {
    let name = std::path::Path::new(filename)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    PACKAGE_INDEX
        .binary_search_by(|entry| package_name(entry.0, entry.1).cmp(name.as_bytes()))
        .ok()
        .or_else(|| {
            PACKAGE_FOLDED
                .binary_search_by(|&index| {
                    let entry = PACKAGE_INDEX[index as usize];
                    ascii_folded_cmp(package_name(entry.0, entry.1), name.as_bytes())
                })
                .ok()
                .map(|index| PACKAGE_FOLDED[index] as usize)
        })
}

#[inline]
fn package_name(offset: u32, length: u32) -> &'static [u8] {
    let start = offset as usize;
    &PACKAGE_NAMES[start..start + length as usize]
}

#[inline]
fn ascii_folded_cmp(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    left.iter()
        .map(u8::to_ascii_lowercase)
        .cmp(right.iter().map(u8::to_ascii_lowercase))
}

pub fn has_embedded_package(filename: &str) -> bool {
    package_entry(filename).is_some()
}

pub fn get_embedded_package(filename: &str) -> Option<Vec<u8>> {
    let index = package_entry(filename)?;
    let (_, _, chunk_index, member_offset, member_length) = PACKAGE_INDEX[index];
    let chunk_index = chunk_index as usize;
    let member_offset = member_offset as usize;
    let member_length = member_length as usize;
    let cache = CHUNK_CACHE.get_or_init(Default::default);
    let chunk = {
        let mut cache = cache.lock().ok()?;
        let position = cache.iter().position(|(index, _)| *index == chunk_index);
        position.and_then(|position| {
            let entry = cache.remove(position)?;
            let bytes = entry.1.clone();
            cache.push_back(entry);
            Some(bytes)
        })
    };
    let chunk = match chunk {
        Some(bytes) => bytes,
        None => {
            let (offset, length) = *PACKAGE_CHUNKS.get(chunk_index)?;
            let offset = offset as usize;
            let length = length as usize;
            let end = offset.checked_add(length)?;
            let compressed = PACKAGES.get(offset..end)?;
            let bytes: std::sync::Arc<[u8]> = zstd::decode_all(compressed).ok()?.into();
            let mut cache = cache.lock().ok()?;
            if let Some(position) = cache.iter().position(|(index, _)| *index == chunk_index) {
                cache.remove(position);
            }
            while cache.len() >= CHUNK_CACHE_CAPACITY {
                cache.pop_front();
            }
            cache.push_back((chunk_index, bytes.clone()));
            bytes
        }
    };
    let end = member_offset.checked_add(member_length)?;
    Some(chunk.get(member_offset..end)?.to_vec())
}

/// Resolve an input from the embedded TeX tree using TeX's default-extension
/// rule. An extensionless `\\input foo` must prefer `foo.tex`; otherwise an
/// unrelated `foo.sty` can shadow the generic implementation that a package
/// intended to load.
pub fn get_embedded_tex_input(name: &str) -> Option<(String, Vec<u8>)> {
    let clean = std::path::Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(name);
    let candidates = if clean.ends_with(".tex") || clean.ends_with(".ltx") {
        vec![clean.to_string()]
    } else {
        vec![
            format!("{clean}.tex"),
            format!("{clean}.ltx"),
            clean.to_string(),
        ]
    };
    candidates
        .into_iter()
        .find_map(|candidate| get_embedded_package(&candidate).map(|data| (candidate, data)))
}
use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

/// Upper bound on entries inspected by a single recursive directory walk.
const WALK_BUDGET: usize = 100_000;

/// File format categories, each with its own TDS search subdirectories.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Format {
    Tex, // .tex .ltx .cls .sty .clo .fd .dfu .cfg .def .ldf
    Tfm, // .tfm
    Vf,  // .vf (virtual font: char packets mapping into base fonts)
    Ofm,
    Type1, // .pfb .pfa
    Truetype,
    Enc, // .enc (glyph encoding files)
    Map, // .map (pdftex map files)
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
            Format::Tex => &[
                ".tex", ".ltx", ".sty", ".cls", ".clo", ".fd", ".dfu", ".cfg", ".def", ".ldf",
                ".texi",
            ],
            Format::Tfm => &[".tfm"],
            Format::Vf => &[".vf"],
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
            Format::Vf => &["fonts/vf//"],
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

mod filename_index;
pub use filename_index::build_filename_index;

struct LsR {
    packed: Option<filename_index::Packed>,
    text: String,
    /// Folded filename fingerprint -> entry chain. Names remain in `text`;
    /// full comparisons below resolve fingerprint collisions.
    db: HashMap<u64, usize>,
    entries: Vec<LsREntry>,
    /// Exact source bytes used to build this lookup table: path, length, hash.
    source_dependency: Option<(PathBuf, u64, u64)>,
}

struct LsREntry {
    name: std::ops::Range<usize>,
    directory: std::ops::Range<usize>,
    next: usize,
}

impl LsR {
    fn fingerprint(name: &str) -> u64 {
        let lower;
        let bytes = if name.is_ascii() {
            name.as_bytes()
        } else {
            lower = name.to_lowercase();
            lower.as_bytes()
        };
        bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(byte.to_ascii_lowercase())).wrapping_mul(0x100000001b3)
        })
    }

    fn new() -> Self {
        LsR {
            packed: None,
            text: String::new(),
            db: HashMap::new(),
            entries: Vec::new(),
            source_dependency: None,
        }
    }

    fn parse(text: String) -> Self {
        let mut db = HashMap::new();
        let mut entries = Vec::new();
        let mut directory = 0..0;
        let mut offset = 0;
        for raw in text.split_inclusive('\n') {
            let start = offset;
            offset += raw.len();
            let line = raw.trim_end();
            if line.is_empty() || line.starts_with('%') {
                continue;
            }
            if line.ends_with(':') && !line.starts_with(char::is_whitespace) {
                directory = start..start + line.len() - 1;
                continue;
            }
            let name = line.trim();
            if name.is_empty() {
                continue;
            }
            let begin = start + line.len() - line.trim_start().len();
            let head = db.entry(Self::fingerprint(name)).or_insert(0);
            entries.push(LsREntry {
                name: begin..begin + name.len(),
                directory: directory.clone(),
                next: *head,
            });
            *head = entries.len();
        }
        Self {
            packed: None,
            text,
            db,
            entries,
            source_dependency: None,
        }
    }

    /// Exact match first, then the case-insensitive fallback.
    fn get(&self, name: &str) -> Option<Vec<PathBuf>> {
        if let Some(packed) = &self.packed {
            return packed.get(name);
        }
        let mut head = *self.db.get(&Self::fingerprint(name))?;
        let mut matches = Vec::new();
        while head != 0 {
            let entry = &self.entries[head - 1];
            let stored = &self.text[entry.name.clone()];
            let equal = if stored.is_ascii() && name.is_ascii() {
                stored.eq_ignore_ascii_case(name)
            } else {
                stored.to_lowercase() == name.to_lowercase()
            };
            if equal {
                matches.push(entry);
            }
            head = entry.next;
        }
        if matches.is_empty() {
            return None;
        }
        let exact = matches
            .iter()
            .any(|entry| &self.text[entry.name.clone()] == name);
        // Paths are materialized only for requested filenames. Keep database
        // order and exact-case precedence, including duplicate basenames.
        Some(
            matches
                .into_iter()
                .rev()
                .filter_map(|entry| {
                    let stored = &self.text[entry.name.clone()];
                    if exact && stored != name {
                        return None;
                    }
                    Some(Path::new(&self.text[entry.directory.clone()]).join(stored))
                })
                .collect(),
        )
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
    /// High-level find cache: (name, format) -> Option<PathBuf>.
    find_cache: RefCell<HashMap<(String, Format), Option<PathBuf>>>,
    /// Stable listings shared by local case-insensitive resolution and its
    /// dependency trace. Exact candidate probes always bypass this cache.
    directory_cache: RefCell<HashMap<PathBuf, Rc<DirectorySnapshot>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DirectoryGeneration {
    len: u64,
    modified: Option<std::time::SystemTime>,
    readonly: bool,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    links: u64,
    #[cfg(unix)]
    mtime_sec: i64,
    #[cfg(unix)]
    mtime_nsec: i64,
    #[cfg(unix)]
    ctime_sec: i64,
    #[cfg(unix)]
    ctime_nsec: i64,
}

#[derive(Clone, Copy)]
struct DirectoryEntryKind {
    is_file: bool,
    is_dir: bool,
    is_symlink: bool,
}

#[derive(Clone)]
struct DirectoryEntrySnapshot {
    name: OsString,
    kind: Option<DirectoryEntryKind>,
}

struct DirectorySnapshot {
    generation: Option<DirectoryGeneration>,
    entries: Vec<DirectoryEntrySnapshot>,
    fingerprint: Option<u64>,
    complete: bool,
}

fn directory_generation(path: &Path) -> Option<DirectoryGeneration> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(DirectoryGeneration {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            readonly: metadata.permissions().readonly(),
            dev: metadata.dev(),
            ino: metadata.ino(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            mtime_sec: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
            ctime_sec: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
        })
    }
    #[cfg(not(unix))]
    {
        Some(DirectoryGeneration {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            readonly: metadata.permissions().readonly(),
        })
    }
}

fn snapshot_entries_fingerprint<'a>(
    entries: impl IntoIterator<Item = &'a DirectoryEntrySnapshot>,
) -> Option<u64> {
    use std::hash::{Hash, Hasher};

    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for entry in entries {
        let kind = entry.kind?;
        entry.name.hash(&mut hash);
        kind.is_file.hash(&mut hash);
        kind.is_dir.hash(&mut hash);
        kind.is_symlink.hash(&mut hash);
    }
    Some(hash.finish())
}

/// Read one coherent directory generation. Entries from an unstable or
/// partially unreadable scan remain usable for best-effort resolution, but
/// carry no fingerprint and are never retained for dependency tracking.
fn scan_directory(path: &Path) -> Option<DirectorySnapshot> {
    let before = directory_generation(path);
    let read_dir = std::fs::read_dir(path).ok()?;
    let mut complete = before.is_some();
    let mut entries = Vec::new();
    for entry in read_dir {
        match entry {
            Ok(entry) => {
                let kind = match entry.file_type() {
                    Ok(file_type) => Some(DirectoryEntryKind {
                        is_file: file_type.is_file(),
                        is_dir: file_type.is_dir(),
                        is_symlink: file_type.is_symlink(),
                    }),
                    Err(_) => {
                        complete = false;
                        None
                    }
                };
                entries.push(DirectoryEntrySnapshot {
                    name: entry.file_name(),
                    kind,
                });
            }
            Err(_) => complete = false,
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    let after = directory_generation(path);
    complete &= before.is_some() && before == after;
    let fingerprint = complete
        .then(|| snapshot_entries_fingerprint(entries.iter()))
        .flatten();
    complete &= fingerprint.is_some();
    Some(DirectorySnapshot {
        generation: after.or(before),
        entries,
        fingerprint,
        complete,
    })
}

/// Filesystem state that determines the result of one Kpathsea-style lookup.
pub type LookupDependencies = (
    Vec<PathBuf>,
    Vec<(PathBuf, u64)>,
    Vec<(PathBuf, u64, u64)>,
    Vec<PathBuf>,
    Vec<PathBuf>,
    bool,
);

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
            find_cache: RefCell::new(HashMap::new()),
            directory_cache: RefCell::new(HashMap::new()),
        }
    }

    /// Build a resolver with an explicit local directory. `extra_roots` are
    /// TDS trees searched before the default roots (TEXMFLOCAL-style
    /// precedence). Setting `TEX_RS_HERMETIC=1` limits resolution to the
    /// working directory, explicit roots, and resources embedded in the
    /// executable. `TEX_RS_ALLOW_SYSTEM_TEXMF=1` explicitly restores the
    /// environment and system search paths.
    pub fn with_roots(cwd: &Path, extra_roots: &[&Path]) -> Self {
        let mut roots: Vec<PathBuf> = extra_roots.iter().map(|p| p.to_path_buf()).collect();
        let hermetic = Self::environment_flag("TEX_RS_HERMETIC")
            && !Self::environment_flag("TEX_RS_ALLOW_SYSTEM_TEXMF");
        if !hermetic {
            for env in ["TEXMFHOME", "TEXMFVAR", "TEXMFCONFIG", "TEXMFLOCAL", "TEXMFDIST"] {
                if let Ok(v) = std::env::var(env) {
                    for p in std::env::split_paths(&v) {
                        if !p.as_os_str().is_empty() && !roots.iter().any(|r| r == &p) {
                            roots.push(p);
                        }
                    }
                }
            }
            if let Ok(home) = std::env::var("HOME") {
                for cand in [
                    PathBuf::from(&home).join("texmf"),
                    PathBuf::from(&home).join(".texlive/texmf-var"),
                ] {
                    if cand.exists() && !roots.iter().any(|r| r == &cand) {
                        roots.push(cand);
                    }
                }
            }
            if let Ok(data_dir) = std::env::var("TEX_SUITE_DATA") {
                let p = PathBuf::from(data_dir).join("texmf");
                if p.exists() && !roots.iter().any(|r| r == &p) {
                    roots.push(p);
                }
            }
            if let Ok(exe) = std::env::current_exe() {
                if let Some(bin_dir) = exe.parent() {
                    for cand in [
                        bin_dir.join("texmf"),
                        bin_dir.join("../share/tex-suite/texmf"),
                        bin_dir.join("../share/texmf"),
                        bin_dir.join("../../texmf"),
                    ] {
                        if cand.exists() && !roots.iter().any(|r| r == &cand) {
                            roots.push(cand);
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
        }
        let dbs = roots.iter().map(|_| RefCell::new(None)).collect();
        let extra_paths = if hermetic {
            HashMap::new()
        } else {
            Self::parse_extra_paths()
        };
        Kpse {
            roots,
            dbs,
            extra_paths,
            cwd: cwd.to_path_buf(),
            walk_cache: RefCell::new(HashMap::new()),
            find_cache: RefCell::new(HashMap::new()),
            directory_cache: RefCell::new(HashMap::new()),
        }
    }

    fn environment_flag(name: &str) -> bool {
        std::env::var_os(name).is_some_and(|value| {
            let value = value.to_string_lossy();
            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
        })
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
                        m.entry(*f).or_default().push(PathBuf::from(p));
                    }
                }
            }
        };
        add(&mut m, &[Format::Tex], "TEXINPUTS");
        add(&mut m, &[Format::Tfm], "TFMFONTS");
        add(&mut m, &[Format::Vf], "VFFONTS");
        add(
            &mut m,
            &[Format::Type1, Format::Truetype, Format::Otf],
            "TTFONTS",
        );
        add(&mut m, &[Format::Type1], "T1FONTS");
        add(
            &mut m,
            &[Format::Type1, Format::Truetype, Format::Otf],
            "OPENTYPEFONTS",
        );
        add(&mut m, &[Format::Enc], "ENCFONTS");
        add(&mut m, &[Format::Map, Format::Fontmap], "TEXFONTMAPS");
        add(&mut m, &[Format::Bst], "BSTINPUTS");
        add(&mut m, &[Format::Bib], "BIBINPUTS");
        m
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Concrete files and missing paths which governed the result of a lookup.
    /// Search stops at `selected`, just as [`Self::find`] does, so lower-priority
    /// unindexed trees do not unnecessarily disable persistent caching.
    /// Unindexed trees are represented by snapshots of every searched
    /// directory. `complete` is false only when an I/O error prevents a stable
    /// snapshot of the lookup.
    pub fn lookup_dependencies(
        &self,
        name: &str,
        fmt: Format,
        selected: Option<&Path>,
    ) -> LookupDependencies {
        let mut present = Vec::new();
        let mut present_directories = Vec::new();
        let mut present_content = Vec::new();
        let mut missing_files = Vec::new();
        let mut missing_directories = Vec::new();
        let candidates = Self::candidates(name, fmt);

        fn record_file(
            path: PathBuf,
            present: &mut Vec<PathBuf>,
            missing: &mut Vec<PathBuf>,
        ) -> bool {
            let is_file = path.is_file();
            if is_file {
                present.push(path);
            } else {
                missing.push(path);
            }
            is_file
        }

        fn same_path(left: &Path, right: &Path) -> bool {
            if clean(left.to_path_buf()) == clean(right.to_path_buf()) {
                return true;
            }
            std::fs::canonicalize(left)
                .ok()
                .zip(std::fs::canonicalize(right).ok())
                .is_some_and(|(left, right)| left == right)
        }

        fn finish(
            mut present: Vec<PathBuf>,
            mut present_directories: Vec<(PathBuf, u64)>,
            mut present_content: Vec<(PathBuf, u64, u64)>,
            mut missing_files: Vec<PathBuf>,
            mut missing_directories: Vec<PathBuf>,
            complete: bool,
        ) -> LookupDependencies {
            present.sort();
            present.dedup();
            present_directories.sort();
            present_directories.dedup();
            present_content.sort();
            present_content.dedup();
            missing_files.sort();
            missing_files.dedup();
            missing_directories.sort();
            missing_directories.dedup();
            (
                present,
                present_directories,
                present_content,
                missing_files,
                missing_directories,
                complete,
            )
        }

        let selected_matches = |path: &Path| selected.is_some_and(|want| same_path(path, want));

        let requested = Path::new(name);
        if requested.is_absolute() {
            let observed = record_file(requested.to_path_buf(), &mut present, &mut missing_files);
            let complete = if observed {
                selected_matches(requested)
            } else {
                selected.is_none()
            };
            return finish(
                present,
                present_directories,
                present_content,
                missing_files,
                missing_directories,
                complete,
            );
        }

        // 1. The invocation directory, including its case-insensitive fallback.
        for candidate in &candidates {
            let exact = self.cwd.join(candidate);
            let (hit, directories, trace_complete) = self.find_local_traced(candidate);
            present_directories.extend(directories);
            if !trace_complete {
                return finish(
                    present,
                    present_directories,
                    present_content,
                    missing_files,
                    missing_directories,
                    false,
                );
            }
            if let Some(hit) = hit {
                if !same_path(&exact, &hit) {
                    record_file(exact, &mut present, &mut missing_files);
                }
                if !record_file(hit.clone(), &mut present, &mut missing_files) {
                    return finish(
                        present,
                        present_directories,
                        present_content,
                        missing_files,
                        missing_directories,
                        false,
                    );
                }
                let complete = selected_matches(&hit);
                return finish(
                    present,
                    present_directories,
                    present_content,
                    missing_files,
                    missing_directories,
                    complete,
                );
            }
            record_file(exact, &mut present, &mut missing_files);
        }

        // 2. Names containing a directory component are probed directly
        // beneath each root; extension candidates and TDS walks do not apply.
        if name.contains('/') {
            for root in &self.roots {
                let path = root.join(name);
                if record_file(path.clone(), &mut present, &mut missing_files) {
                    let complete = selected_matches(&path);
                    return finish(
                        present,
                        present_directories,
                        present_content,
                        missing_files,
                        missing_directories,
                        complete,
                    );
                }
            }
            return finish(
                present,
                present_directories,
                present_content,
                missing_files,
                missing_directories,
                selected.is_none(),
            );
        }

        // 3. Explicit format paths precede all TDS roots.
        if let Some(paths) = self.extra_paths.get(&fmt) {
            for base in paths {
                for candidate in &candidates {
                    let path = base.join(candidate);
                    if record_file(path.clone(), &mut present, &mut missing_files) {
                        let complete = selected_matches(&path);
                        return finish(
                            present,
                            present_directories,
                            present_content,
                            missing_files,
                            missing_directories,
                            complete,
                        );
                    }
                }
            }
        }

        // 4. Each TDS root uses the first existing ls-R/ls-R.lua database.
        // A usable database makes future additions observable through that
        // database dependency. Missing paths named by the database must also
        // be recorded because they may appear without the index changing.
        for i in 0..self.roots.len() {
            let root = &self.roots[i];
            if !root.is_dir() {
                missing_directories.push(root.clone());
                continue;
            }

            let lsr = root.join("ls-R");
            let lsr_lua = root.join("ls-R.lua");
            let index_path = if lsr.is_file() {
                Some(lsr)
            } else {
                record_file(lsr, &mut present, &mut missing_files);
                if lsr_lua.is_file() {
                    Some(lsr_lua)
                } else {
                    record_file(lsr_lua, &mut present, &mut missing_files);
                    None
                }
            };

            let (database_empty, database_paths, source_dependency) = {
                let db = self.db_of(i);
                let empty = db
                    .packed
                    .as_ref()
                    .map_or_else(|| db.db.is_empty(), |packed| packed.is_empty());
                let paths = if empty {
                    Vec::new()
                } else {
                    candidates
                        .iter()
                        .flat_map(|candidate| db.get(candidate).unwrap_or_default())
                        .map(|relative| root.join(relative))
                        .collect()
                };
                (empty, paths, db.source_dependency.clone())
            };
            match (index_path, source_dependency) {
                (Some(index), Some(dependency)) if same_path(&index, &dependency.0) => {
                    present_content.push(dependency);
                }
                (None, None) => {}
                _ => {
                    return finish(
                        present,
                        present_directories,
                        present_content,
                        missing_files,
                        missing_directories,
                        false,
                    );
                }
            }

            for path in database_paths {
                if record_file(path.clone(), &mut present, &mut missing_files) {
                    let complete = selected_matches(&path);
                    return finish(
                        present,
                        present_directories,
                        present_content,
                        missing_files,
                        missing_directories,
                        complete,
                    );
                }
            }
            if !database_empty {
                continue;
            }

            // Match walk_find's candidate-major order. Directory snapshots
            // make an unindexed recursive walk finite and cacheable: adding a
            // candidate file or subtree changes one of the visited parents.
            for candidate in &candidates {
                for spec in fmt.tds_paths() {
                    let subtree = spec.strip_suffix("//").unwrap_or(spec);
                    let start = root.join(subtree);
                    if !start.is_dir() {
                        missing_directories.push(start);
                    }
                }
                let (hit, directories, walk_complete) = walk_find_traced(root, fmt, candidate);
                present_directories.extend(directories);
                if !walk_complete {
                    return finish(
                        present,
                        present_directories,
                        present_content,
                        missing_files,
                        missing_directories,
                        false,
                    );
                }
                if let Some(path) = hit {
                    if !record_file(path.clone(), &mut present, &mut missing_files) {
                        return finish(
                            present,
                            present_directories,
                            present_content,
                            missing_files,
                            missing_directories,
                            false,
                        );
                    }
                    let complete = selected_matches(&path);
                    return finish(
                        present,
                        present_directories,
                        present_content,
                        missing_files,
                        missing_directories,
                        complete,
                    );
                }
            }
        }

        finish(
            present,
            present_directories,
            present_content,
            missing_files,
            missing_directories,
            selected.is_none(),
        )
    }

    /// Dependencies for a failed external lookup before an embedded fallback.
    pub fn negative_lookup_dependencies(&self, name: &str, fmt: Format) -> LookupDependencies {
        self.lookup_dependencies(name, fmt, None)
    }

    fn local_directory_snapshot(&self, path: &Path) -> Option<Rc<DirectorySnapshot>> {
        let generation = directory_generation(path);
        if let Some(generation) = generation.as_ref() {
            let cached = self
                .directory_cache
                .borrow()
                .get(path)
                .filter(|snapshot| snapshot.generation.as_ref() == Some(generation))
                .cloned();
            if cached.is_some() {
                return cached;
            }
        }

        // Do not let a failed metadata/read attempt leave an older generation
        // reachable. A later lookup will retry the live filesystem.
        self.directory_cache.borrow_mut().remove(path);
        let snapshot = Rc::new(scan_directory(path)?);
        if snapshot.complete {
            self.directory_cache
                .borrow_mut()
                .insert(path.to_path_buf(), snapshot.clone());
        }
        Some(snapshot)
    }

    fn find_local(&self, name: &str) -> Option<PathBuf> {
        let exact = self.cwd.join(name);
        if exact.is_file() {
            return Some(exact);
        }
        let mut path = self.cwd.clone();
        for component in Path::new(name).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => path.push(".."),
                Component::Normal(wanted) => {
                    let exact = path.join(wanted);
                    if exact.exists() {
                        path = exact;
                        continue;
                    }
                    let wanted = wanted.to_string_lossy();
                    let snapshot = self.local_directory_snapshot(&path)?;
                    let entry = snapshot
                        .entries
                        .iter()
                        .find(|entry| entry.name.to_string_lossy().eq_ignore_ascii_case(&wanted))?;
                    path.push(&entry.name);
                }
                Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        path.is_file().then_some(path)
    }

    fn find_local_traced(&self, name: &str) -> (Option<PathBuf>, Vec<(PathBuf, u64)>, bool) {
        let exact = self.cwd.join(name);
        if exact.is_file() {
            return (Some(exact), Vec::new(), true);
        }
        let mut path = self.cwd.clone();
        let mut directories = Vec::new();
        let mut complete = true;
        let components: Vec<_> = Path::new(name).components().collect();
        let component_count = components.len();
        for (component_index, component) in components.into_iter().enumerate() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => path.push(".."),
                Component::Normal(wanted) => {
                    let exact = path.join(wanted);
                    if exact.exists() {
                        // At the last component an exact directory or special
                        // file suppresses the case-insensitive fallback. Track
                        // its parent membership so replacing it with a
                        // differently-cased regular file cannot hide behind a
                        // cached lower-priority result.
                        if component_index + 1 == component_count && !exact.is_file() {
                            let Some(snapshot) = self.local_directory_snapshot(&path) else {
                                return (None, directories, false);
                            };
                            let Some(fingerprint) = snapshot.fingerprint else {
                                return (None, directories, false);
                            };
                            directories.push((path.clone(), fingerprint));
                            if !exact.exists() || exact.is_file() {
                                return (None, directories, false);
                            }
                        }
                        path = exact;
                        continue;
                    }
                    let wanted = wanted.to_string_lossy();
                    let Some(snapshot) = self.local_directory_snapshot(&path) else {
                        return (None, directories, false);
                    };
                    if let Some(fingerprint) = snapshot.fingerprint {
                        directories.push((path.clone(), fingerprint));
                    } else {
                        complete = false;
                    }
                    complete &= snapshot.complete;
                    let Some(entry) = snapshot
                        .entries
                        .iter()
                        .find(|entry| entry.name.to_string_lossy().eq_ignore_ascii_case(&wanted))
                    else {
                        return (None, directories, complete);
                    };
                    path.push(&entry.name);
                }
                Component::RootDir | Component::Prefix(_) => {
                    return (None, directories, false);
                }
            }
        }
        (path.is_file().then_some(path), directories, complete)
    }

    fn candidates(name: &str, fmt: Format) -> Vec<String> {
        // Candidate names: when the name carries none of the format's
        // extensions, the extension-appended spellings are tried before the
        // bare name so that unrelated files sharing the bare name (e.g.
        // tex4ht alias scripts next to font names) cannot shadow the real
        // format file.
        let mut candidates: Vec<String>;
        if name.ends_with(".tex") || name.ends_with(".ltx") {
            candidates = vec![name.to_string()];
        } else {
            let default_exts = match fmt {
                Format::Tex => &[".tex", ".ltx"][..],
                _ => fmt.extensions(),
            };
            candidates = default_exts.iter().map(|e| format!("{name}{e}")).collect();
            candidates.push(name.to_string());
        }
        candidates
    }

    /// Find a file of the given format. `name` may already carry an extension.
    pub fn find(&self, name: &str, fmt: Format) -> Option<PathBuf> {
        for candidate in Self::candidates(name, fmt) {
            if let Some(local) = self.find_local(&candidate) {
                return Some(local);
            }
        }
        let key = (name.to_string(), fmt);
        if let Some(Some(hit)) = self.find_cache.borrow().get(&key) {
            if hit.is_file() {
                return Some(hit.clone());
            }
        }
        let hit = self.find_uncached(name, fmt);
        if hit.is_some() {
            self.find_cache.borrow_mut().insert(key, hit.clone());
        }
        hit
    }

    fn find_uncached(&self, name: &str, fmt: Format) -> Option<PathBuf> {
        let p = Path::new(name);
        if p.is_absolute() {
            return if p.is_file() {
                Some(p.to_path_buf())
            } else {
                None
            };
        }
        let candidates = Self::candidates(name, fmt);
        // 2. a name with a directory part is only probed against the roots
        if name.contains('/') {
            for root in &self.roots {
                let full = root.join(name);
                if full.is_file() {
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
                    if p.is_file() {
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
                        if full.is_file() {
                            return Some(clean(full));
                        }
                    }
                }
            }
            // If this root has an ls-R database (db is non-empty), it is fully
            // indexed; skip the expensive recursive disk walk.
            if db
                .packed
                .as_ref()
                .map_or_else(|| db.db.is_empty(), |p| p.is_empty())
            {
                for cand in &candidates {
                    if let Some(hit) = self.walk_cached(i, fmt, cand) {
                        return Some(hit);
                    }
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
            return if Path::new(name).is_file() {
                Some(PathBuf::from(name))
            } else {
                None
            };
        }
        if let Some(local) = self.find_local(name) {
            return Some(local);
        }
        for i in 0..self.roots.len() {
            let db = self.db_of(i);
            if let Some(dirs) = db.get(name) {
                for rel in dirs {
                    let full = self.roots[i].join(rel);
                    if full.is_file() {
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
        if let Some(p) = self.find(name, fmt) {
            if let Ok(d) = std::fs::read(p) {
                return Some(d);
            }
        }
        let clean = Path::new(name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(name);
        if let Some(d) = get_embedded_package(clean) {
            return Some(d);
        }
        let exts: &[&str] = match fmt {
            Format::Tfm => &[".tfm"],
            Format::Type1 => &[".pfb", ".pfa"],
            Format::Enc => &[".enc"],
            Format::Map => &[".map"],
            Format::Tex => &[".tex", ".sty", ".cls", ".clo", ".ltx", ".def", ".fd"],
            Format::Bst => &[".bst"],
            Format::Bib => &[".bib"],
            _ => &[],
        };
        for ext in exts {
            let with_ext = format!("{clean}{ext}");
            if let Some(d) = get_embedded_package(&with_ext) {
                return Some(d);
            }
        }
        None
    }

    fn walk_cached(&self, root_idx: usize, fmt: Format, cand: &str) -> Option<PathBuf> {
        let key = (root_idx, fmt, cand.to_string());
        if let Some(Some(hit)) = self.walk_cache.borrow().get(&key) {
            if hit.is_file() {
                return Some(hit.clone());
            }
        }
        let hit = walk_find(&self.roots[root_idx], fmt, cand);
        if hit.is_some() {
            self.walk_cache.borrow_mut().insert(key, hit.clone());
        }
        hit
    }
}

/// Recursive fallback search for `cand` under `root`'s TDS subtrees for
/// `fmt`, honoring the `//` recursive markers of [`Format::tds_paths`].
/// Prefers an exact filename match; otherwise returns the first
/// case-insensitive match.
fn walk_find(root: &Path, fmt: Format, cand: &str) -> Option<PathBuf> {
    walk_find_impl(root, fmt, cand, |_, _| {}).0
}

fn walk_find_traced(
    root: &Path,
    fmt: Format,
    cand: &str,
) -> (Option<PathBuf>, Vec<(PathBuf, u64)>, bool) {
    let mut directories = Vec::new();
    let mut trace_complete = true;
    let (hit, complete) = walk_find_impl(root, fmt, cand, |path, entries| {
        let fingerprint = entries
            .map(directory_entries_fingerprint)
            .or_else(|| directory_fingerprint(path));
        if let Some(fingerprint) = fingerprint {
            directories.push((path.to_path_buf(), fingerprint));
        } else {
            trace_complete = false;
        }
    });
    (hit, directories, complete && trace_complete)
}

fn walk_find_impl(
    root: &Path,
    fmt: Format,
    cand: &str,
    mut visited_directory: impl FnMut(&Path, Option<&[(std::fs::DirEntry, std::fs::FileType)]>),
) -> (Option<PathBuf>, bool) {
    let cand_lower = cand.to_lowercase();
    let mut ci_hit: Option<PathBuf> = None;
    let mut budget = WALK_BUDGET;
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut complete = true;
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
            visited_directory(&start, None);
            let hit = start.join(cand);
            if hit.is_file() {
                return (Some(hit), complete);
            }
            continue;
        }
        let mut stack = vec![start];
        while let Some(dir) = stack.pop() {
            if budget == 0 {
                return (ci_hit, complete);
            }
            budget -= 1;
            // Canonicalize to break symlink cycles.
            let Ok(canon) = dir.canonicalize() else {
                complete = false;
                continue;
            };
            if !visited.insert(canon) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                complete = false;
                continue;
            };
            let mut listed = Vec::new();
            for entry in entries {
                match entry {
                    Ok(entry) => listed.push(entry),
                    Err(_) => complete = false,
                }
            }
            let mut entries = listed;
            entries.sort_by_key(|e| e.file_name());
            let mut typed_entries = Vec::with_capacity(entries.len());
            for entry in entries {
                match entry.file_type() {
                    Ok(file_type) => typed_entries.push((entry, file_type)),
                    Err(_) => complete = false,
                }
            }
            visited_directory(&dir, Some(&typed_entries));
            for (entry, ft) in typed_entries {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') {
                    continue;
                }
                let path = entry.path();
                if ft.is_dir() {
                    stack.push(path);
                    continue;
                }
                if name == cand {
                    return (Some(path), complete);
                }
                if ci_hit.is_none() && name.to_lowercase() == cand_lower {
                    ci_hit = Some(path);
                }
            }
        }
    }
    (ci_hit, complete)
}

fn directory_entries_fingerprint(entries: &[(std::fs::DirEntry, std::fs::FileType)]) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for (entry, file_type) in entries {
        entry.file_name().hash(&mut hash);
        file_type.is_file().hash(&mut hash);
        file_type.is_dir().hash(&mut hash);
        file_type.is_symlink().hash(&mut hash);
    }
    hash.finish()
}

/// Deterministic identity of the names and entry types in one directory.
/// The executable identity invalidates records if Rust's hasher changes.
pub fn directory_fingerprint(path: &Path) -> Option<u64> {
    let snapshot = scan_directory(path)?;
    snapshot.complete.then_some(snapshot.fingerprint).flatten()
}

/// Directory identity after omitting named direct children. This is used when
/// a caller has created known output files after an earlier lookup snapshot
/// and needs to prove that those outputs are the only membership change.
pub fn directory_fingerprint_excluding(
    path: &Path,
    excluded_names: &[&std::ffi::OsStr],
) -> Option<u64> {
    let snapshot = scan_directory(path)?;
    if !snapshot.complete {
        return None;
    }
    snapshot_entries_fingerprint(snapshot.entries.iter().filter(|entry| {
        !excluded_names
            .iter()
            .any(|excluded| entry.name == *excluded)
    }))
}

fn dependency_content_identity(bytes: &[u8]) -> (u64, u64) {
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x9e37_79b9_7f4a_7c15;
    for (index, byte) in bytes.iter().enumerate() {
        h1 ^= u64::from(*byte);
        h1 = h1.wrapping_mul(0x1000_0000_01b3);
        h2 = (h2 + u64::from(*byte) + index as u64).wrapping_mul(0x1000_0000_01b3);
    }
    (bytes.len() as u64, h1 ^ h2)
}

/// Parse a root's `ls-R` filename database.
fn load_lsr(root: &Path) -> LsR {
    let timing = std::env::var_os("PHASE_TIMING").map(|_| std::time::Instant::now());
    let mut lsr = LsR::new();
    for name in ["ls-R", "ls-R.lua"] {
        let path = root.join(name);
        if !path.is_file() {
            continue;
        }
        if let Some(packed) = filename_index::Packed::load(&path) {
            let (size, hash) = dependency_content_identity(packed.source_bytes());
            lsr.source_dependency = Some((path, size, hash));
            lsr.packed = Some(packed);
        } else if let Ok(bytes) = std::fs::read(&path) {
            let (size, hash) = dependency_content_identity(&bytes);
            if let Ok(text) = String::from_utf8(bytes) {
                lsr = LsR::parse(text);
            }
            lsr.source_dependency = Some((path, size, hash));
        }
        break; // first database found wins
    }
    if let Some(start) = timing {
        eprintln!(
            "PHASE_TIMING ls-R {} {:.3} ms",
            root.display(),
            start.elapsed().as_secs_f64() * 1000.0
        );
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

    #[test]
    fn compact_lsr_preserves_case_precedence_and_database_order() {
        let db = LsR::parse("% header\r\n./first:\r\nMix.sty\r\n\n./second:\nMIX.sty\nMix.sty\n./third:\n  École.sty  ".to_owned());
        assert_eq!(
            db.get("Mix.sty").unwrap(),
            vec![
                PathBuf::from("./first/Mix.sty"),
                PathBuf::from("./second/Mix.sty")
            ]
        );
        assert_eq!(
            db.get("MIX.sty").unwrap(),
            vec![PathBuf::from("./second/MIX.sty")]
        );
        assert_eq!(
            db.get("mix.sty").unwrap(),
            vec![
                PathBuf::from("./first/Mix.sty"),
                PathBuf::from("./second/MIX.sty"),
                PathBuf::from("./second/Mix.sty")
            ]
        );
        assert_eq!(
            db.get("école.sty").unwrap(),
            vec![PathBuf::from("./third/École.sty")]
        );
        assert!(db.get("absent.sty").is_none());
    }


    #[test]
    fn texmfvar_is_registered_as_search_root() {
        let fake_var = std::env::temp_dir().join(format!("fake-texmfvar-{}", std::process::id()));
        std::fs::create_dir_all(&fake_var).unwrap();
        std::env::set_var("TEXMFVAR", &fake_var);
        let kpse = Kpse::new();
        assert!(kpse.roots.iter().any(|r| r == &fake_var));
        let _ = std::fs::remove_dir_all(&fake_var);
        std::env::remove_var("TEXMFVAR");
    }
    #[test]
    fn lsr_fingerprint_collisions_do_not_create_false_matches() {
        let mut db = LsR::parse("./fonts:\nMatch.sty\nDifferent.sty\nKelvin.sty\n".to_owned());
        // Force unrelated names into one fingerprint bucket.
        db.entries[1].next = 1;
        db.db.insert(LsR::fingerprint("match.sty"), 2);
        assert_eq!(
            db.get("match.sty").unwrap(),
            vec![PathBuf::from("./fonts/Match.sty")]
        );
        db.db.insert(LsR::fingerprint("absent.sty"), 2);
        assert!(db.get("absent.sty").is_none());
        assert_eq!(
            db.get("KELVIN.STY").unwrap(),
            vec![PathBuf::from("./fonts/Kelvin.sty")]
        );
    }

    #[test]
    fn indexed_packages_match_archive_bytes() {
        use std::io::Read;
        let compressed = include_bytes!("../assets/packages.tar.zst");
        let mut archive = tar::Archive::new(zstd::Decoder::new(&compressed[..]).unwrap());
        let mut seen = HashSet::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry.path().unwrap().into_owned();
            let name = path.file_name().unwrap().to_str().unwrap();
            if !seen.insert(name.to_owned()) {
                continue;
            }
            let actual = get_embedded_package(name).expect("indexed package");
            let mut expected = Vec::new();
            entry.read_to_end(&mut expected).unwrap();
            assert_eq!(actual, expected, "{name}");
        }
        assert_eq!(seen.len(), PACKAGE_INDEX.len());
        assert_eq!(
            get_embedded_package("ARTICLE.CLS"),
            get_embedded_package("article.cls")
        );
    }

    #[test]
    fn embedded_tex_lookup_prefers_the_default_tex_extension() {
        assert!(has_embedded_package("xkeyval.sty"));
        assert!(has_embedded_package("xkeyval.tex"));
        let (name, data) = get_embedded_tex_input("xkeyval").expect("embedded xkeyval input");
        assert_eq!(name, "xkeyval.tex");
        assert!(data.starts_with(b"%%"));

        let (name, data) = get_embedded_tex_input("binhex").expect("embedded binhex input");
        assert_eq!(name, "binhex.tex");
        assert!(!data.is_empty());
    }

    #[test]
    fn packed_package_index_is_sorted_and_self_consistent() {
        assert!(PACKAGE_INDEX.windows(2).all(|entries| {
            package_name(entries[0].0, entries[0].1) < package_name(entries[1].0, entries[1].1)
        }));
        assert!(PACKAGE_FOLDED.windows(2).all(|positions| {
            let left = PACKAGE_INDEX[positions[0] as usize];
            let right = PACKAGE_INDEX[positions[1] as usize];
            ascii_folded_cmp(package_name(left.0, left.1), package_name(right.0, right.1))
                == std::cmp::Ordering::Less
        }));
        let mut expected = std::collections::BTreeMap::new();
        for (position, package) in PACKAGE_INDEX.iter().enumerate() {
            let name = std::str::from_utf8(package_name(package.0, package.1))
                .expect("archive filenames are UTF-8");
            expected
                .entry(name.to_ascii_lowercase())
                .or_insert(position as u32);
        }
        assert_eq!(
            PACKAGE_FOLDED,
            expected.values().copied().collect::<Vec<_>>()
        );
        for &package_index in PACKAGE_FOLDED {
            PACKAGE_INDEX
                .get(package_index as usize)
                .expect("folded entry points into the exact index");
        }
    }

    /// Fresh unique directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            for i in 0..1000 {
                let p =
                    std::env::temp_dir().join(format!("tex-kpse-{tag}-{}-{i}", std::process::id()));
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
        assert_eq!(
            kpse.find("article.cls", Format::Tex),
            Some(tmp.path().join("article.cls"))
        );
        assert_eq!(
            kpse.find("main.tex", Format::Tex),
            Some(tmp.path().join("main.tex"))
        );
        assert_eq!(
            kpse.find("mypkg.sty", Format::Tex),
            Some(tmp.path().join("mypkg.sty"))
        );
        assert_eq!(
            kpse.find("refs.bib", Format::Bib),
            Some(tmp.path().join("refs.bib"))
        );
        assert_eq!(
            kpse.find("rfs.bst", Format::Bst),
            Some(tmp.path().join("rfs.bst"))
        );
        assert_eq!(kpse.read("rfs.bst", Format::Bst).unwrap(), b"% local bst");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn local_subpaths_fall_back_case_insensitively() {
        let tmp = TempDir::new("local-casefold");
        let want = tmp.write("Pics/Fund12A.PDF", "pdf");
        let kpse = Kpse::with_roots(tmp.path(), &[]);

        assert_eq!(kpse.find_any("pics/fund12a.pdf"), Some(want));
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
        let up = kpse
            .find("T1CMR.FD", Format::Tex)
            .expect("T1CMR.FD fallback");
        assert!(up.is_file());
        // newtx math metrics fd resolves as-is.
        let mi = kpse.find("omlntxmi.fd", Format::Tex).expect("omlntxmi.fd");
        assert!(mi.is_file());
        // find_fd convenience for encoding + family pairs.
        let tlf = kpse
            .find_fd("t1", "ntxtlf")
            .expect("t1ntxtlf.fd via find_fd");
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
        if let Some(_root) = dist_root() {
            let kpse = Kpse::new();
            if let Some(tfm) = kpse.find("txr.tfm", Format::Tfm) {
                assert!(tfm.is_file());
                assert!(tfm.ends_with("fonts/tfm/public/txfonts/txr.tfm"), "{tfm:?}");
            }
        }
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
        // Local bibliography style and database win via cwd.
        let tmp = TempDir::new("bibproj");
        tmp.write("rfs.bst", "% local rfs");
        tmp.write("references.bib", "% local bib");
        let kpse = Kpse::with_roots(tmp.path(), &[]);
        let rfs = kpse.find("rfs.bst", Format::Bst).expect("local rfs.bst");
        assert_eq!(rfs, tmp.path().join("rfs.bst"));
        let bib = kpse
            .find("references", Format::Bib)
            .expect("references.bib");
        assert_eq!(bib, tmp.path().join("references.bib"));
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
        let tmp = TempDir::new("mainproj");
        tmp.write("rfs.bst", "% local rfs");
        tmp.write("references.bib", "% local bib");
        let kpse = Kpse::with_roots(tmp.path(), &[]);
        // Local files: bibliography style + database.
        let rfs = kpse.find("rfs.bst", Format::Bst).expect("rfs.bst");
        assert!(rfs.starts_with(tmp.path()));
        let bib = kpse
            .find("references.bib", Format::Bib)
            .expect("references.bib");
        assert!(bib.starts_with(tmp.path()));
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
            assert!(
                p.starts_with(root) || p.starts_with(tmp.path()),
                "{p:?} outside search roots"
            );
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

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn local_lookup_falls_back_to_case_insensitive_basename() {
        let tmp = TempDir::new("local-casefold");
        let want = tmp.write("VCH-logo.png", "png");
        let kpse = Kpse::with_roots(tmp.path(), &[]);
        assert_eq!(kpse.find_any("vch-logo.png"), Some(want));
    }

    #[test]
    fn lookup_observes_files_created_after_a_miss_and_local_overrides() {
        let tmp = TempDir::new("live-lookup");
        let cwd = tmp.path().join("project");
        let root = tmp.path().join("tree");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(root.join("tex/latex")).unwrap();
        std::fs::write(root.join("tex/latex/shared.tex"), b"distribution").unwrap();
        let kpse = Kpse::with_roots(&cwd, &[root.as_path()]);
        assert!(kpse.find("new.tex", Format::Tex).is_none());
        std::fs::write(cwd.join("new.tex"), b"created").unwrap();
        assert_eq!(kpse.find("new.tex", Format::Tex), Some(cwd.join("new.tex")));
        assert!(!kpse
            .find("shared.tex", Format::Tex)
            .unwrap()
            .starts_with(&cwd));
        std::fs::write(cwd.join("shared.tex"), b"override").unwrap();
        assert_eq!(
            kpse.find("shared.tex", Format::Tex),
            Some(cwd.join("shared.tex"))
        );
    }

    #[test]
    fn lookup_dependencies_stop_at_an_explicit_path_hit() {
        let tmp = TempDir::new("lookup-deps-extra");
        let cwd = tmp.path().join("project");
        let extra = tmp.path().join("extra");
        let lower = tmp.path().join("lower");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::create_dir_all(lower.join("tex/latex/unindexed")).unwrap();
        let selected = tmp.write("extra/article.cls", "% explicit class");
        let mut kpse = Kpse::explicit(&cwd, vec![lower.clone()]);
        kpse.extra_paths.insert(Format::Tex, vec![extra]);

        assert_eq!(
            kpse.find("article.cls", Format::Tex),
            Some(selected.clone())
        );
        let (present, _, _, _, _, complete) =
            kpse.lookup_dependencies("article.cls", Format::Tex, Some(&selected));
        assert!(complete);
        assert!(present.contains(&selected));
        assert!(present.iter().all(|path| !path.starts_with(&lower)));
    }

    #[test]
    fn lookup_dependencies_stop_at_an_indexed_root_hit() {
        let tmp = TempDir::new("lookup-deps-indexed");
        let cwd = tmp.path().join("project");
        let indexed = tmp.path().join("indexed");
        let lower = tmp.path().join("lower");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(lower.join("tex/latex/unindexed")).unwrap();
        let selected = tmp.write("indexed/tex/latex/base/article.cls", "% indexed class");
        tmp.write("indexed/ls-R", "./tex/latex/base:\narticle.cls\n");
        let kpse = Kpse::explicit(&cwd, vec![indexed.clone(), lower.clone()]);

        assert_eq!(
            kpse.find("article.cls", Format::Tex),
            Some(selected.clone())
        );
        let (present, _, content, _, _, complete) =
            kpse.lookup_dependencies("article.cls", Format::Tex, Some(&selected));
        assert!(complete);
        assert!(content
            .iter()
            .any(|(path, _, _)| path == &indexed.join("ls-R")));
        assert!(present.contains(&selected));
        assert!(present.iter().all(|path| !path.starts_with(&lower)));
    }

    #[test]
    fn lookup_dependencies_snapshot_an_unindexed_recursive_search() {
        let tmp = TempDir::new("lookup-deps-unindexed");
        let cwd = tmp.path().join("project");
        let root = tmp.path().join("tree");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(root.join("tex/latex/packages")).unwrap();
        let kpse = Kpse::explicit(&cwd, vec![root.clone()]);

        assert!(kpse.find("absent.sty", Format::Tex).is_none());
        let (_, directories, _, _, _, complete) =
            kpse.lookup_dependencies("absent.sty", Format::Tex, None);
        assert!(complete);
        assert!(directories
            .iter()
            .any(|(path, _)| path == &root.join("tex/latex/packages")));
    }

    #[test]
    fn an_earlier_unindexed_root_is_snapshotted_before_a_later_hit() {
        let tmp = TempDir::new("lookup-deps-earlier-unindexed");
        let cwd = tmp.path().join("project");
        let unindexed = tmp.path().join("unindexed");
        let indexed = tmp.path().join("indexed");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(unindexed.join("tex/latex/packages")).unwrap();
        let selected = tmp.write("indexed/tex/latex/base/article.cls", "% indexed class");
        tmp.write("indexed/ls-R", "./tex/latex/base:\narticle.cls\n");
        let kpse = Kpse::explicit(&cwd, vec![unindexed.clone(), indexed]);

        assert_eq!(
            kpse.find("article.cls", Format::Tex),
            Some(selected.clone())
        );
        let (_, directories, _, _, _, complete) =
            kpse.lookup_dependencies("article.cls", Format::Tex, Some(&selected));
        assert!(complete);
        assert!(directories
            .iter()
            .any(|(path, _)| path == &unindexed.join("tex/latex/packages")));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn terminal_local_directory_snapshots_casefold_parent() {
        let tmp = TempDir::new("terminal-directory-casefold");
        let cwd = tmp.path().join("project");
        let root = tmp.path().join("tree");
        std::fs::create_dir_all(cwd.join("shadow.tex")).unwrap();
        let selected = tmp.write("tree/tex/latex/test/shadow.tex", "lower priority");
        tmp.write("tree/ls-R", "./tex/latex/test:\nshadow.tex\n");
        let kpse = Kpse::explicit(&cwd, vec![root]);

        assert_eq!(kpse.find("shadow", Format::Tex), Some(selected.clone()));
        let (_, directories, _, _, _, complete) =
            kpse.lookup_dependencies("shadow", Format::Tex, Some(&selected));
        assert!(complete);
        let observed = directories
            .iter()
            .find_map(|(path, fingerprint)| (path == &cwd).then_some(*fingerprint))
            .expect("the local parent directory must be snapshotted");

        std::fs::remove_dir(cwd.join("shadow.tex")).unwrap();
        let local = tmp.write("project/SHADOW.TEX", "local override");
        assert_ne!(directory_fingerprint(&cwd), Some(observed));
        assert_eq!(kpse.find("shadow", Format::Tex), Some(local));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn advance_directory_clock(path: &Path, prev: &DirectoryGeneration) {
        for _ in 0..100 {
            if let Some(current) = directory_generation(path) {
                if &current != prev {
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn local_snapshot_cache_tracks_live_membership_and_case_changes() {
        let tmp = TempDir::new("local-snapshot-generation");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let original = tmp.write("project/mIxEd.TeX", "original");
        let kpse = Kpse::explicit(&cwd, Vec::new());

        assert_eq!(kpse.find("mixed.tex", Format::Tex), Some(original.clone()));
        let first = kpse.local_directory_snapshot(&cwd).unwrap();
        let reused = kpse.local_directory_snapshot(&cwd).unwrap();
        assert!(Rc::ptr_eq(&first, &reused));

        // A newly-created, lexically earlier casefold match must replace the
        // cached choice, while a true exact candidate always wins live.
        advance_directory_clock(&cwd, first.generation.as_ref().unwrap());
        let earlier = tmp.write("project/MIXED.TEX", "earlier");
        assert_eq!(kpse.find("mixed.tex", Format::Tex), Some(earlier.clone()));
        let after_creation = kpse.local_directory_snapshot(&cwd).unwrap();
        assert!(!Rc::ptr_eq(&first, &after_creation));
        let exact = tmp.write("project/mixed.tex", "exact");
        assert_eq!(kpse.find("mixed.tex", Format::Tex), Some(exact.clone()));

        advance_directory_clock(&cwd, after_creation.generation.as_ref().unwrap());
        std::fs::remove_file(exact).unwrap();
        std::fs::remove_file(earlier).unwrap();
        assert_eq!(kpse.find("mixed.tex", Format::Tex), Some(original.clone()));
        let after_removal = kpse.local_directory_snapshot(&cwd).unwrap();
        assert!(!Rc::ptr_eq(&after_creation, &after_removal));

        advance_directory_clock(&cwd, after_removal.generation.as_ref().unwrap());
        let renamed = cwd.join("MiXeD.tEx");
        std::fs::rename(&original, &renamed).unwrap();
        assert_eq!(kpse.find("mixed.tex", Format::Tex), Some(renamed.clone()));
        let after_case_change = kpse.local_directory_snapshot(&cwd).unwrap();
        assert!(!Rc::ptr_eq(&after_removal, &after_case_change));

        advance_directory_clock(&cwd, after_case_change.generation.as_ref().unwrap());
        std::fs::remove_file(&renamed).unwrap();
        std::fs::create_dir(&renamed).unwrap();
        assert_eq!(kpse.find("mixed.tex", Format::Tex), None);
        let after_replacement = kpse.local_directory_snapshot(&cwd).unwrap();
        assert!(!Rc::ptr_eq(&after_case_change, &after_replacement));

        advance_directory_clock(&cwd, after_replacement.generation.as_ref().unwrap());
        std::fs::remove_dir(&renamed).unwrap();
        let final_file = tmp.write("project/mIXeD.tex", "final");
        assert_eq!(kpse.find("mixed.tex", Format::Tex), Some(final_file));
    }
}
