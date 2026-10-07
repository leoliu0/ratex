//! Kpathsea-compatible TDS file resolution for the Rust TeX engine.
//!
//! Searches the standard TeX Live directory roots using their `ls-R`
//! filename databases, plus the current working directory for user files.

pub mod fs;
use fs::PathExt;

/// Case-insensitive (ASCII) match of a directory entry against a wanted
/// path component, comparing their lossy UTF-8 spellings. ASCII names skip
/// the lossy conversion: bytes equal up to ASCII case have equal lossy forms.
fn local_name_matches(name: &std::ffi::OsStr, wanted: &std::ffi::OsStr) -> bool {
    let (name_bytes, wanted_bytes) = (name.as_encoded_bytes(), wanted.as_encoded_bytes());
    name_bytes.eq_ignore_ascii_case(wanted_bytes)
        || (!(name_bytes.is_ascii() && wanted_bytes.is_ascii())
            && name.to_string_lossy().eq_ignore_ascii_case(&wanted.to_string_lossy()))
}

/// Metadata for an embedded OpenType or TrueType font face discovered at build time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbeddedFontFace {
    pub file: &'static str,
    pub face_index: u32,
    pub family: &'static str,
    pub subfamily: &'static str,
    pub postscript: &'static str,
    pub weight: u16,
    pub italic: bool,
    /// Family names (name ids 16 else 1), `\u{1f}`-separated, as XeTeX's font manager reads them.
    pub families: &'static str,
    /// Style names (name ids 17 else 2).
    pub styles: &'static str,
    /// Full names (name id 4).
    pub fulls: &'static str,
    /// OS/2 usWidthClass.
    pub width: u16,
    /// bit 0 OS/2 REGULAR, bit 1 bold (OS/2 or head.macStyle), bit 2 italic
    pub flags: u8,
    /// `1000 * tan(-italicAngle)`
    pub slant: i32,
    /// GPOS `size` feature: design size, subfamily id, name id, min, max (deci-points); design 0 when absent.
    pub opsize: [i32; 5],
}

/// Complete Lua font-loader metadata for an immutable embedded SFNT face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbeddedFontInfo {
    pub face_index: u32,
    pub fontname: &'static str,
    pub fullname: &'static str,
    pub familyname: &'static str,
    pub copyright: &'static str,
    pub version: &'static str,
    pub units_per_em: u16,
    pub glyph_count: u16,
    pub ascent: i16,
    /// Raw signed descender; the Lua metadata reports its negation.
    pub descender: i16,
    /// Exact `f32::to_bits()` of the italic angle (zero when absent).
    pub italic_angle_bits: u32,
    pub weight: u16,
    pub width: u16,
}

/// All native font faces available in the embedded packages archive.
pub fn embedded_font_faces() -> &'static [EmbeddedFontFace] {
    if fs::embedded_allowed() {
        EMBEDDED_FONT_FACES
    } else {
        &[]
    }
}

fn embedded_font_entry(path: &str) -> Option<[u32; 5]> {
    if !fs::embedded_allowed() {
        return None;
    }
    let entry = embedded_tree::path_file_entry(path)?;
    let position = PACKAGE_FONT_INFO.binary_search_by(|[member, _, _, _, _]| {
        (member as usize).cmp(&entry)
    })?;
    PACKAGE_FONT_INFO.get(position)
}

/// Complete metadata in face-index order for one exact embedded virtual font
/// file. Bare names and local paths never qualify. No font payload is decoded.
pub fn embedded_font_info(path: &str) -> Option<&'static [EmbeddedFontInfo]> {
    let [_, start, count, _, _] = embedded_font_entry(path)?;
    let start = start as usize;
    EMBEDDED_FONT_INFOS.get(start..start.checked_add(count as usize)?)
}

/// An immutable font opened while embedded-file access is allowed. Its small
/// metadata ranges are exact program bytes, not a substitute font or parsed
/// metadata approximation. Other reads materialize the original program.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedFontFile {
    member: usize,
    length: usize,
    window_start: usize,
    window_end: usize,
}

impl EmbeddedFontFile {
    pub fn open(path: &str) -> Option<Self> {
        let [member, _, _, start, count] = embedded_font_entry(path)?;
        let [_, _, _, _, length, _] = PACKAGE_INDEX.get(member as usize)?;
        Some(Self {
            member: member as usize,
            length: length as usize,
            window_start: start as usize,
            window_end: start.checked_add(count)? as usize,
        })
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// A zero-based, end-exclusive read served entirely by one stored range.
    /// `None` means the caller must read the complete original program.
    pub fn metadata_slice(&self, start: usize, end: usize) -> Option<&'static [u8]> {
        if start > end || end > self.length {
            return None;
        }
        if start == end {
            return Some(&[]);
        }
        let (mut low, mut high) = (self.window_start, self.window_end);
        while low < high {
            let middle = low + (high - low) / 2;
            let [offset, _, length] = FONT_METADATA_WINDOWS.get(middle)?;
            if offset as usize + length as usize <= start {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        if low == self.window_end {
            return None;
        }
        let [offset, data, length] = FONT_METADATA_WINDOWS.get(low)?;
        let offset = offset as usize;
        if start < offset || end > offset + length as usize {
            return None;
        }
        let data = data as usize;
        FONT_METADATA_BYTES.get(data + start - offset..data + end - offset)
    }

    /// Preserve an already-open file's access even if a later host scope
    /// disables new embedded opens, just as an owned decoded buffer does.
    pub fn read_all(&self) -> Option<Vec<u8>> {
        read_package_entry(self.member)
    }
}

// Independently compressed chunks and a sorted member index are generated
// once at build time. Runtime lookup inflates only the containing chunk, while
// related small files still share enough context for effective compression.
include!(concat!(env!("OUT_DIR"), "/packages_index.rs"));

pub mod embedded_tree;

/// The per-user cache root: `TEX_RS_CACHE_DIR`, else the platform's user
/// cache directory. Dependency records, and the Lua font loader's name
/// database, live below it.
pub fn platform_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TEX_RS_CACHE_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "windows")]
    if let Some(dir) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return PathBuf::from(dir).join("tex-rs").join("cache");
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join("Library").join("Caches").join("tex-rs");
    }
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(dir).join("tex-rs");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(".cache").join("tex-rs");
    }
    std::env::temp_dir().join("tex-rs-cache")
}

/// Build-time table of `FIELDS`-wide records of little-endian `u32`s.
#[derive(Clone, Copy)]
struct PackedTable<const FIELDS: usize>(&'static [u8]);

impl<const FIELDS: usize> PackedTable<FIELDS> {
    const RECORD_BYTES: usize = FIELDS * 4;

    fn len(self) -> usize {
        self.0.len() / Self::RECORD_BYTES
    }

    fn get(self, index: usize) -> Option<[u32; FIELDS]> {
        let start = index.checked_mul(Self::RECORD_BYTES)?;
        let record = self.0.get(start..start.checked_add(Self::RECORD_BYTES)?)?;
        let mut fields = [0; FIELDS];
        for (field, bytes) in fields.iter_mut().zip(record.chunks_exact(4)) {
            *field = u32::from_le_bytes(bytes.try_into().ok()?);
        }
        Some(fields)
    }

    /// Like `slice::binary_search_by` over the records.
    fn binary_search_by(
        self,
        mut compare: impl FnMut([u32; FIELDS]) -> std::cmp::Ordering,
    ) -> Option<usize> {
        let (mut low, mut high) = (0, self.len());
        while low < high {
            let middle = low + (high - low) / 2;
            match compare(self.get(middle)?) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }
}

/// Byte budget of the decoded-chunk cache. Package reads cluster in a few
/// directories (a font family's tfm/vf/fd files, a bundle of related .sty
/// files) and the engine rereads members (every TFM size load), so a small
/// LRU of the 128 KiB build-time chunks absorbs nearly all repeat decoding.
const CHUNK_CACHE_BYTES: usize = 8 * 1024 * 1024;
// Members larger than a chunk (an outline font, pdftex.map) span several
// frames; `decode_member_frames` decodes them straight into the caller's
// buffer instead of evicting the shared working set.

#[derive(Default)]
struct ChunkCache {
    /// Least recently used first.
    entries: std::collections::VecDeque<(usize, std::sync::Arc<[u8]>)>,
    bytes: usize,
}

impl ChunkCache {
    fn get(&mut self, chunk_index: usize) -> Option<std::sync::Arc<[u8]>> {
        let position = self.entries.iter().position(|(index, _)| *index == chunk_index)?;
        let entry = self.entries.remove(position)?;
        let bytes = entry.1.clone();
        self.entries.push_back(entry);
        Some(bytes)
    }

    fn insert(&mut self, chunk_index: usize, bytes: std::sync::Arc<[u8]>) {
        // Another thread may have decoded the same chunk concurrently.
        if self.entries.iter().any(|(index, _)| *index == chunk_index) {
            return;
        }
        while self.bytes + bytes.len() > CHUNK_CACHE_BYTES {
            let Some((_, evicted)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= evicted.len();
        }
        self.bytes += bytes.len();
        self.entries.push_back((chunk_index, bytes));
    }
}

static CHUNK_CACHE: std::sync::Mutex<Option<ChunkCache>> = std::sync::Mutex::new(None);
const MAX_PACKAGE_CHUNK_BYTES: usize = 128 * 1024 * 1024;

fn decode_package_chunk(compressed: &[u8], expected_len: usize) -> Option<Vec<u8>> {
    if expected_len > MAX_PACKAGE_CHUNK_BYTES {
        return None;
    }
    let mut decoder = ruzstd::decoding::FrameDecoder::new();
    // The build-time encoder declares its fixed match window (128 KiB), which
    // can exceed a short chunk's length; the output itself is bounded below.
    decoder.set_max_window_size(MAX_PACKAGE_CHUNK_BYTES as u64);
    let mut decoded = vec![0; expected_len];
    // Fails with `TargetTooSmall` when the frame holds more than expected.
    let written = decoder.decode_all(compressed, &mut decoded).ok()?;
    (written == expected_len).then_some(decoded)
}

/// kpathsea never searches its path for absolute or explicitly relative
/// (`./`, `../`) names: they denote exactly one file, never a bundled one.
fn is_explicit_path(name: &str) -> bool {
    matches!(
        Path::new(name).components().next(),
        Some(Component::RootDir | Component::Prefix(_) | Component::CurDir | Component::ParentDir)
    )
}

fn package_entry(filename: &str) -> Option<usize> {
    if is_explicit_path(filename) {
        return None;
    }
    let name = Path::new(filename).file_name().and_then(|s| s.to_str()).unwrap_or(filename);
    PACKAGE_INDEX
        .binary_search_by(|entry| package_name(entry).cmp(name.as_bytes()))
        .or_else(|| {
            let position = PACKAGE_FOLDED.binary_search_by(|[index]| {
                let entry = PACKAGE_INDEX.get(index as usize).unwrap_or_default();
                ascii_folded_cmp(package_name(entry), name.as_bytes())
            })?;
            Some(PACKAGE_FOLDED.get(position)?[0] as usize)
        })
}

#[inline]
fn package_name([offset, length, ..]: [u32; 6]) -> &'static [u8] {
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
    fs::embedded_allowed() && package_entry(filename).is_some()
}

pub fn get_embedded_package(filename: &str) -> Option<Vec<u8>> {
    if !fs::embedded_allowed() {
        return None;
    }
    read_package_entry(package_entry(filename)?)
}

fn read_package_entry(index: usize) -> Option<Vec<u8>> {
    let [_, _, chunk_index, member_offset, member_length, _] = PACKAGE_INDEX.get(index)?;
    let chunk_index = chunk_index as usize;
    let member_offset = member_offset as usize;
    let member = member_offset..member_offset.checked_add(member_length as usize)?;
    let [offset, length, decoded_length] = PACKAGE_CHUNKS.get(chunk_index)?;
    let decoded_length = decoded_length as usize;
    let compressed = || {
        let offset = offset as usize;
        PACKAGES.get(offset..offset.checked_add(length as usize)?)
    };
    if member.end > decoded_length {
        // A large member: it fills this chunk and the following ones.
        return (member_offset == 0)
            .then(|| decode_member_frames(chunk_index, member.end))
            .flatten();
    }
    let cached = CHUNK_CACHE
        .lock()
        .ok()?
        .get_or_insert_with(Default::default)
        .get(chunk_index);
    let chunk = match cached {
        Some(bytes) => bytes,
        None => {
            // Decode outside the lock so concurrent readers of other chunks
            // do not serialize behind this one.
            let bytes: std::sync::Arc<[u8]> =
                decode_package_chunk(compressed()?, decoded_length)?.into();
            CHUNK_CACHE
                .lock()
                .ok()?
                .get_or_insert_with(Default::default)
                .insert(chunk_index, bytes.clone());
            bytes
        }
    };
    Some(chunk.get(member)?.to_vec())
}

/// Threads decoding one large member at once (as for formats, each new
/// thread's malloc arena counts against the engine's address-space limit).
#[cfg(not(target_arch = "wasm32"))]
const MAX_MEMBER_DECODE_THREADS: usize = 4;

/// A member of `length` bytes stored as the consecutive chunks from
/// `first`, each its own frame (build.rs `LARGE_MEMBER_FRAME`), decoded into
/// one buffer on as many threads as the process may run on.
fn decode_member_frames(first: usize, length: usize) -> Option<Vec<u8>> {
    if length > MAX_PACKAGE_CHUNK_BYTES {
        return None;
    }
    let mut out = vec![0u8; length];
    let mut jobs = Vec::new();
    let mut free = &mut out[..];
    let mut chunk_index = first;
    while !free.is_empty() {
        let [offset, compressed_length, decoded_length] = PACKAGE_CHUNKS.get(chunk_index)?;
        let offset = offset as usize;
        let compressed = PACKAGES.get(offset..offset.checked_add(compressed_length as usize)?)?;
        let (target, tail) = free.split_at_mut((decoded_length as usize).min(free.len()));
        if target.len() != decoded_length as usize {
            return None;
        }
        jobs.push((compressed, target));
        free = tail;
        chunk_index += 1;
    }
    fn decode((input, target): (&[u8], &mut [u8])) -> bool {
        let mut decoder = ruzstd::decoding::FrameDecoder::new();
        decoder.set_max_window_size(MAX_PACKAGE_CHUNK_BYTES as u64);
        // Fails with `TargetTooSmall` when the frame holds more than indexed.
        matches!(decoder.decode_all(input, target), Ok(n) if n == target.len())
    }
    #[cfg(not(target_arch = "wasm32"))]
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(MAX_MEMBER_DECODE_THREADS)
        .min(jobs.len());
    #[cfg(target_arch = "wasm32")]
    let threads = 1;
    let ok = if threads <= 1 {
        jobs.into_iter().all(decode)
    } else {
        // Lane `k` takes frames k, k + threads, ...; this thread runs lane 0.
        let mut lanes: Vec<Vec<(&[u8], &mut [u8])>> = (0..threads).map(|_| Vec::new()).collect();
        for (i, job) in jobs.into_iter().enumerate() {
            lanes[i % threads].push(job);
        }
        let mut lanes = lanes.into_iter();
        let own = lanes.next().unwrap_or_default();
        std::thread::scope(|scope| {
            let workers: Vec<_> = lanes
                .map(|lane| scope.spawn(move || lane.into_iter().all(decode)))
                .collect();
            let mine = own.into_iter().all(decode);
            workers
                .into_iter()
                .fold(mine, |ok, worker| worker.join().unwrap_or(false) && ok)
        })
    };
    ok.then_some(out)
}

/// Resolve an input from the embedded TeX tree using TeX's default-extension
/// rule. An extensionless `\\input foo` must prefer `foo.tex`; otherwise an
/// unrelated `foo.sty` can shadow the generic implementation that a package
/// intended to load.
pub fn get_embedded_tex_input(name: &str) -> Option<(String, Vec<u8>)> {
    embedded_tex_input(name, false)
}

/// Like [`get_embedded_tex_input`], but only members that live below the TDS
/// `tex/` subtree qualify, as in a kpathsea search of the TEXINPUTS path: a
/// TFM, encoding or map file that happens to share the name is not a TeX
/// input.
pub fn get_embedded_tex_tree_input(name: &str) -> Option<(String, Vec<u8>)> {
    embedded_tex_input(name, true)
}

fn embedded_tex_input(name: &str, tex_tree_only: bool) -> Option<(String, Vec<u8>)> {
    if is_explicit_path(name) || !fs::embedded_allowed() {
        return None;
    }
    let clean = Path::new(name).file_name().and_then(|value| value.to_str()).unwrap_or(name);
    Kpse::candidates(clean, Format::Tex)
        .into_iter()
        .find_map(|candidate| {
            let index = package_entry(&candidate)?;
            if tex_tree_only && PACKAGE_INDEX.get(index)?[5] == 0 {
                return None;
            }
            read_package_entry(index).map(|data| (candidate, data))
        })
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
    Type1, // .pfb .pfa
    Truetype,
    Enc, // .enc (glyph encoding files)
    Map, // .map (pdftex map files)
    Bst,
    Bib,
    Otf,
    /// kpathsea `lua` format (LUAINPUTS): Lua modules for `require`.
    Lua,
    /// kpathsea `pk` format (PKFONTS): `<font>.<dpi>pk` bitmap fonts.
    Pk,
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
            Format::Type1 => &[".pfb", ".pfa"],
            Format::Truetype => &[".ttf", ".ttc", ".otf"],
            Format::Enc => &[".enc"],
            Format::Map => &[".map"],
            Format::Bst => &[".bst"],
            Format::Bib => &[".bib"],
            Format::Otf => &[".otf"],
            Format::Lua => &[".luc", ".luctex", ".texluc", ".lua", ".luatex", ".texlua"],
            // names are spelled with their resolution (`bbm12.600pk`)
            Format::Pk => &["pk"],
        }
    }

    /// Whether a root-relative path (an `ls-R` entry may spell it `./tex/...`)
    /// lies below one of this format's TDS subtrees.
    fn tds_tree_contains(&self, rel: &Path) -> bool {
        let normal: PathBuf = rel
            .components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .collect();
        self.tds_paths()
            .iter()
            .any(|spec| normal.starts_with(spec.trim_end_matches('/')))
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
            Format::Type1 => &["fonts/type1//"],
            Format::Truetype => &["fonts/truetype//"],
            Format::Otf => &["fonts/opentype//"],
            Format::Enc => &["fonts/enc//"],
            Format::Map => &["fonts/map//"],
            Format::Bst => &["bibtex/bst//"],
            Format::Bib => &["bibtex/bib//"],
            // texmf.cnf LUAINPUTS: scripts/{$progname,$engine,}/{lua,}//
            // then tex/{luatex,plain,generic,latex,}//
            Format::Lua => &[
                "scripts//",
                "tex/luatex//",
                "tex/plain//",
                "tex/generic//",
                "tex/latex//",
                "tex//",
            ],
            Format::Pk => &["fonts/pk//"],
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
    /// Path-variable elements (TEXINPUTS & co.) searched before every other
    /// source, as kpathsea does; `true` marks a recursive `dir//` element.
    extra_paths: HashMap<Format, Vec<(PathBuf, bool)>>,
    /// Project directory, searched right after the path-variable elements.
    cwd: PathBuf,
    /// Results of recursive walks, cached per (start, TDS format or `None`
    /// for a recursive path element, name).
    walk_cache: RefCell<HashMap<(PathBuf, Option<Format>, String), Option<PathBuf>>>,
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
    let metadata = crate::fs::metadata(path).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
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
    let read_dir = crate::fs::read_dir(path).ok()?;
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupSourceKind {
    Local,
    Absolute,
    EnvironmentPath,
    Database,
    SubtreeWalk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupExplanation {
    pub name: String,
    pub format: Format,
    pub resolved: Option<PathBuf>,
    pub source_kind: Option<LookupSourceKind>,
    pub searched_roots: usize,
}

impl Default for Kpse {
    fn default() -> Self {
        Self::new()
    }
}

impl Kpse {
    /// Build a resolver rooted at the process working directory.
    pub fn new() -> Self {
        let cwd = crate::fs::current_dir().unwrap_or_else(|_| PathBuf::from("."));
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
    /// precedence). `texres` operates in strict hermetic mode: it resolves from
    /// project files and its own bundled installation assets (`share/tex-suite/texmf`
    /// or `share/texres/texmf`), with zero fallback to external TeX Live.
    pub fn with_roots(cwd: &Path, extra_roots: &[&Path]) -> Self {
        if crate::fs::is_memory() {
            return Self::explicit(cwd, Vec::new());
        }
        let mut roots: Vec<PathBuf> = extra_roots.iter().map(|p| p.to_path_buf()).collect();

        // Bundled installation roots: always discovered relative to the executable
        // or through TEX_SUITE_DATA / TEXRES_DATA_DIR.
        if let Ok(data_dir) =
            std::env::var("TEX_SUITE_DATA").or_else(|_| std::env::var("TEXRES_DATA_DIR"))
        {
            let p = PathBuf::from(data_dir).join("texmf");
            if p.tex_is_dir() && !roots.iter().any(|r| r == &p) {
                roots.push(p);
            }
        }

        if !Self::environment_flag("TEX_RS_HERMETIC") {
            for env in [
                "TEXMFHOME",
                "TEXMFVAR",
                "TEXMFCONFIG",
                "TEXMFLOCAL",
                "TEXMFDIST",
            ] {
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
                    if cand.tex_exists() && !roots.iter().any(|r| r == &cand) {
                        roots.push(cand);
                    }
                }
            }
            if let Ok(exe) = std::env::current_exe() {
                if let Some(bin_dir) = exe.parent() {
                    for cand in [
                        bin_dir.join("texmf"),
                        bin_dir.join("../share/tex-suite/texmf"),
                        bin_dir.join("../share/texres/texmf"),
                        bin_dir.join("../share/texmf"),
                        bin_dir.join("../../texmf"),
                    ] {
                        if cand.tex_is_dir() && !roots.iter().any(|r| r == &cand) {
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
                if Path::new(p).tex_exists() && !roots.iter().any(|r| r.as_os_str() == p) {
                    roots.push(PathBuf::from(p));
                }
            }
        }

        let dbs = roots.iter().map(|_| RefCell::new(None)).collect();
        let extra_paths = if Self::environment_flag("TEX_RS_HERMETIC") {
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

    fn parse_extra_paths() -> HashMap<Format, Vec<(PathBuf, bool)>> {
        Self::extra_paths_from(|variable| std::env::var_os(variable))
    }

    fn extra_paths_from(
        variable: impl Fn(&str) -> Option<OsString>,
    ) -> HashMap<Format, Vec<(PathBuf, bool)>> {
        let mut m = HashMap::new();
        for (name, formats) in [
            ("TEXINPUTS", &[Format::Tex][..]),
            ("TFMFONTS", &[Format::Tfm]),
            ("VFFONTS", &[Format::Vf]),
            ("T1FONTS", &[Format::Type1]),
            ("TTFONTS", &[Format::Truetype]),
            ("OPENTYPEFONTS", &[Format::Truetype, Format::Otf]),
            ("ENCFONTS", &[Format::Enc]),
            ("TEXFONTMAPS", &[Format::Map]),
            ("BSTINPUTS", &[Format::Bst]),
            ("BIBINPUTS", &[Format::Bib]),
            ("LUAINPUTS", &[Format::Lua]),
            ("PKFONTS", &[Format::Pk]),
        ] {
            let Some(value) = variable(name) else {
                continue;
            };
            // kpathsea's ENV_SEP: ':' on Unix, ';' on Windows. An empty
            // element stands for the default path, which is always searched.
            for element in std::env::split_paths(&value) {
                if element.as_os_str().is_empty() {
                    continue;
                }
                let element = search_element(element);
                for format in formats {
                    m.entry(*format).or_insert_with(Vec::new).push(element.clone());
                }
            }
        }
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
            let is_file = path.tex_is_file();
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
            crate::fs::canonicalize(left)
                .ok()
                .zip(crate::fs::canonicalize(right).ok())
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

        // 1. Path-variable elements (TEXINPUTS & co.) precede everything,
        // the invocation directory included, as in kpathsea.
        if let Some(paths) = self.extra_paths.get(&fmt) {
            for (base, recursive) in paths {
                if *recursive && !base.tex_is_dir() {
                    missing_directories.push(base.clone());
                    continue;
                }
                for candidate in &candidates {
                    let hit = if *recursive {
                        let (hit, directories, walk_complete) =
                            walk_find_traced(&[(base.clone(), true)], candidate);
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
                        hit
                    } else {
                        Some(base.join(candidate))
                    };
                    let Some(path) = hit else {
                        continue;
                    };
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
                    if *recursive {
                        // The walk saw a file that is gone now.
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
            }
        }

        // 2. The invocation directory, including its case-insensitive fallback.
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

        // 3. Names containing a directory component are probed directly
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

        // 4. Each TDS root uses the first existing ls-R/ls-R.lua database.
        // A usable database makes future additions observable through that
        // database dependency. Missing paths named by the database must also
        // be recorded because they may appear without the index changing.
        for i in 0..self.roots.len() {
            let root = &self.roots[i];
            if !root.tex_is_dir() {
                missing_directories.push(root.clone());
                continue;
            }

            let lsr = root.join("ls-R");
            let lsr_lua = root.join("ls-R.lua");
            let index_path = if lsr.tex_is_file() {
                Some(lsr)
            } else {
                record_file(lsr, &mut present, &mut missing_files);
                if lsr_lua.tex_is_file() {
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

            // Match find_uncached's candidate-major order. Directory snapshots
            // make an unindexed recursive walk finite and cacheable: adding a
            // candidate file or subtree changes one of the visited parents.
            let starts = tds_walk_starts(root, fmt);
            for candidate in &candidates {
                for (start, _) in &starts {
                    if !start.tex_is_dir() {
                        missing_directories.push(start.clone());
                    }
                }
                let (hit, directories, walk_complete) = walk_find_traced(&starts, candidate);
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
        if exact.tex_is_file() {
            return Some(exact);
        }
        let mut path = self.cwd.clone();
        for component in Path::new(name).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => path.push(".."),
                Component::Normal(wanted) => {
                    let exact = path.join(wanted);
                    if exact.tex_exists() {
                        path = exact;
                        continue;
                    }
                    let snapshot = self.local_directory_snapshot(&path)?;
                    let entry = snapshot
                        .entries
                        .iter()
                        .find(|entry| local_name_matches(&entry.name, wanted))?;
                    path.push(&entry.name);
                }
                Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        path.tex_is_file().then_some(path)
    }

    fn find_local_traced(&self, name: &str) -> (Option<PathBuf>, Vec<(PathBuf, u64)>, bool) {
        let exact = self.cwd.join(name);
        if exact.tex_is_file() {
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
                    if exact.tex_exists() {
                        // At the last component an exact directory or special
                        // file suppresses the case-insensitive fallback. Track
                        // its parent membership so replacing it with a
                        // differently-cased regular file cannot hide behind a
                        // cached lower-priority result.
                        if component_index + 1 == component_count && !exact.tex_is_file() {
                            let Some(snapshot) = self.local_directory_snapshot(&path) else {
                                return (None, directories, false);
                            };
                            let Some(fingerprint) = snapshot.fingerprint else {
                                return (None, directories, false);
                            };
                            directories.push((path.clone(), fingerprint));
                            if !exact.tex_exists() || exact.tex_is_file() {
                                return (None, directories, false);
                            }
                        }
                        path = exact;
                        continue;
                    }
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
                        .find(|entry| local_name_matches(&entry.name, wanted))
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
        (path.tex_is_file().then_some(path), directories, complete)
    }

    pub fn candidates(name: &str, fmt: Format) -> Vec<String> {
        // Candidate names: when the name carries none of the format's
        // extensions, the extension-appended spellings are tried before the
        // bare name so that unrelated files sharing the bare name (e.g.
        // tex4ht alias scripts next to font names) cannot shadow the real
        // format file.
        let mut candidates: Vec<String>;
        let ext_already = fmt.extensions().iter().any(|ext| name.ends_with(ext));
        if ext_already || name.ends_with(".tex") || name.ends_with(".ltx") {
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
        let candidates = Self::candidates(name, fmt);
        if !Path::new(name).is_absolute() {
            if let Some(hit) = self.find_in_extra_paths(fmt, &candidates) {
                return Some(hit);
            }
        }
        for candidate in &candidates {
            if let Some(local) = self.find_local(candidate) {
                return Some(local);
            }
        }
        let key = (name.to_string(), fmt);
        if let Some(Some(hit)) = self.find_cache.borrow().get(&key) {
            if hit.tex_is_file() {
                return Some(hit.clone());
            }
        }
        let hit = self.find_uncached(name, fmt);
        if hit.is_some() {
            self.find_cache.borrow_mut().insert(key, hit.clone());
        }
        hit
    }

    /// [`Kpse::find`] for a search of the format's own path variable only
    /// (kpathsea's `kpse_find_file`): a filename-database hit counts only
    /// when its directory lies below one of the format's TDS subtrees, so
    /// `TEXINPUTS` does not reach a TFM or map file of the same name.
    pub fn find_in_format_tree(&self, name: &str, fmt: Format) -> Option<PathBuf> {
        let candidates = Self::candidates(name, fmt);
        if !Path::new(name).is_absolute() {
            if let Some(hit) = self.find_in_extra_paths(fmt, &candidates) {
                return Some(hit);
            }
        }
        for candidate in &candidates {
            if let Some(local) = self.find_local(candidate) {
                return Some(local);
            }
        }
        self.find_uncached_in(name, fmt, true)
    }
    /// Explain how a lookup was resolved and which precedence source matched.
    pub fn explain_lookup(&self, name: &str, fmt: Format) -> LookupExplanation {
        let p = Path::new(name);
        if p.is_absolute() {
            let found = p.tex_is_file();
            return LookupExplanation {
                name: name.to_string(),
                format: fmt,
                resolved: found.then(|| p.to_path_buf()),
                source_kind: found.then_some(LookupSourceKind::Absolute),
                searched_roots: 0,
            };
        }
        let candidates = Self::candidates(name, fmt);
        if let Some(resolved) = self.find_in_extra_paths(fmt, &candidates) {
            return LookupExplanation {
                name: name.to_string(),
                format: fmt,
                resolved: Some(resolved),
                source_kind: Some(LookupSourceKind::EnvironmentPath),
                searched_roots: 0,
            };
        }
        for candidate in &candidates {
            if let Some(local) = self.find_local(candidate) {
                return LookupExplanation {
                    name: name.to_string(),
                    format: fmt,
                    resolved: Some(local),
                    source_kind: Some(LookupSourceKind::Local),
                    searched_roots: 0,
                };
            }
        }
        if name.contains('/') {
            for (idx, root) in self.roots.iter().enumerate() {
                let full = root.join(name);
                if full.tex_is_file() {
                    return LookupExplanation {
                        name: name.to_string(),
                        format: fmt,
                        resolved: Some(clean(full)),
                        source_kind: Some(LookupSourceKind::Database),
                        searched_roots: idx + 1,
                    };
                }
            }
            return LookupExplanation {
                name: name.to_string(),
                format: fmt,
                resolved: None,
                source_kind: None,
                searched_roots: self.roots.len(),
            };
        }
        for i in 0..self.roots.len() {
            let db = self.db_of(i);
            for cand in &candidates {
                if let Some(dirs) = db.get(cand) {
                    for rel in dirs {
                        let full = self.roots[i].join(rel);
                        if full.tex_is_file() {
                            return LookupExplanation {
                                name: name.to_string(),
                                format: fmt,
                                resolved: Some(clean(full)),
                                source_kind: Some(LookupSourceKind::Database),
                                searched_roots: i + 1,
                            };
                        }
                    }
                }
            }
            if db
                .packed
                .as_ref()
                .map_or_else(|| db.db.is_empty(), |p| p.is_empty())
            {
                for cand in &candidates {
                    if let Some(hit) = self.walk_cached(&self.roots[i], Some(fmt), cand) {
                        return LookupExplanation {
                            name: name.to_string(),
                            format: fmt,
                            resolved: Some(hit),
                            source_kind: Some(LookupSourceKind::SubtreeWalk),
                            searched_roots: i + 1,
                        };
                    }
                }
            }
        }
        LookupExplanation {
            name: name.to_string(),
            format: fmt,
            resolved: None,
            source_kind: None,
            searched_roots: self.roots.len(),
        }
    }

    fn find_uncached(&self, name: &str, fmt: Format) -> Option<PathBuf> {
        self.find_uncached_in(name, fmt, false)
    }

    fn find_uncached_in(&self, name: &str, fmt: Format, format_tree_only: bool) -> Option<PathBuf> {
        let p = Path::new(name);
        if p.is_absolute() {
            return if p.tex_is_file() {
                Some(p.to_path_buf())
            } else {
                None
            };
        }
        let candidates = Self::candidates(name, fmt);
        // A name with a directory part is only probed against the roots.
        if name.contains('/') {
            for root in &self.roots {
                let full = root.join(name);
                if full.tex_is_file() {
                    return Some(clean(full));
                }
            }
            return None;
        }
        // TDS roots: ls-R database (exact, then case-insensitive), then a
        // recursive walk of the format's subtrees (`tex/latex//` style)
        for i in 0..self.roots.len() {
            let db = self.db_of(i);
            for cand in &candidates {
                if let Some(dirs) = db.get(cand) {
                    for rel in dirs {
                        if format_tree_only && !fmt.tds_tree_contains(&rel) {
                            continue;
                        }
                        let full = self.roots[i].join(rel);
                        if full.tex_is_file() {
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
                    if let Some(hit) = self.walk_cached(&self.roots[i], Some(fmt), cand) {
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
            return if Path::new(name).tex_is_file() {
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
                    if full.tex_is_file() {
                        return Some(clean(full));
                    }
                }
            }
        }
        None
    }

    /// The first path-variable element holding a candidate.
    fn find_in_extra_paths(&self, fmt: Format, candidates: &[String]) -> Option<PathBuf> {
        for (base, recursive) in self.extra_paths.get(&fmt)? {
            for candidate in candidates {
                if *recursive {
                    if let Some(hit) = self.walk_cached(base, None, candidate) {
                        return Some(hit);
                    }
                } else {
                    let path = base.join(candidate);
                    if path.tex_is_file() {
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    pub fn read(&self, name: &str, fmt: Format) -> Option<Vec<u8>> {
        if let Some(p) = self.find(name, fmt) {
            if let Ok(d) = crate::fs::read(p) {
                return Some(d);
            }
        }
        // The archive is indexed by basename; keep the search's candidate
        // order (format extensions before the bare name).
        Self::candidates(name, fmt)
            .iter()
            .find_map(|candidate| get_embedded_package(candidate))
    }

    /// Cached recursive walk: of the TDS subtrees of `fmt` below `start`, or
    /// with `fmt == None` of the whole tree below a `dir//` path element.
    fn walk_cached(&self, start: &Path, fmt: Option<Format>, cand: &str) -> Option<PathBuf> {
        let key = (start.to_path_buf(), fmt, cand.to_string());
        if let Some(Some(hit)) = self.walk_cache.borrow().get(&key) {
            if hit.tex_is_file() {
                return Some(hit.clone());
            }
        }
        let starts = match fmt {
            Some(fmt) => tds_walk_starts(start, fmt),
            None => vec![(start.to_path_buf(), true)],
        };
        let hit = walk_find_impl(&starts, cand, WALK_BUDGET, |_, _| {}).0;
        if hit.is_some() {
            self.walk_cache.borrow_mut().insert(key, hit.clone());
        }
        hit
    }
}

/// Directories searched for `fmt` below a TDS root, each flagged recursive
/// when its [`Format::tds_paths`] spec carries the `//` marker.
fn tds_walk_starts(root: &Path, fmt: Format) -> Vec<(PathBuf, bool)> {
    fmt.tds_paths()
        .iter()
        .map(|spec| match spec.strip_suffix("//") {
            Some(sub) => (root.join(sub), true),
            None => (root.join(spec), false),
        })
        .collect()
}

/// A path-variable element; kpathsea's trailing `//` searches its subtree.
fn search_element(element: PathBuf) -> (PathBuf, bool) {
    match element.to_str().and_then(|text| text.strip_suffix("//")) {
        Some(base) => {
            let base = base.trim_end_matches('/');
            (PathBuf::from(if base.is_empty() { "/" } else { base }), true)
        }
        None => (element, false),
    }
}

fn walk_find_traced(
    starts: &[(PathBuf, bool)],
    cand: &str,
) -> (Option<PathBuf>, Vec<(PathBuf, u64)>, bool) {
    let mut directories = Vec::new();
    let mut trace_complete = true;
    let (hit, complete) = walk_find_impl(starts, cand, WALK_BUDGET, |path, entries| {
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

/// Recursive fallback search: `starts` in order, a recursive start covering
/// its whole subtree and following directory symlinks like kpathsea (cycles
/// are cut by canonical path). Prefers an exact filename match, otherwise
/// the first case-insensitive one. `cand` may carry directory components,
/// which must then end the path of the match. The second result is false
/// when a directory could not be read or `budget` directories did not
/// suffice, so that a miss is not taken as proof of absence.
fn walk_find_impl(
    starts: &[(PathBuf, bool)],
    cand: &str,
    mut budget: usize,
    mut visited_directory: impl FnMut(&Path, Option<&[(crate::fs::DirEntry, crate::fs::FileType)]>),
) -> (Option<PathBuf>, bool) {
    let wanted = Path::new(cand);
    let Some(file_name) = wanted.file_name().and_then(|name| name.to_str()) else {
        return (None, true);
    };
    let nested = wanted.components().count() > 1;
    let cand_lower = file_name.to_lowercase();
    let mut ci_hit: Option<PathBuf> = None;
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut complete = true;
    for (start, recursive) in starts {
        if !start.tex_is_dir() {
            continue;
        }
        if !recursive {
            visited_directory(start, None);
            let hit = start.join(cand);
            if hit.tex_is_file() {
                return (Some(hit), complete);
            }
            continue;
        }
        let mut stack = vec![start.clone()];
        while let Some(dir) = stack.pop() {
            if budget == 0 {
                return (ci_hit, false);
            }
            budget -= 1;
            // Canonicalize to break symlink cycles.
            let Ok(canon) = dir.tex_canonicalize() else {
                complete = false;
                continue;
            };
            if !visited.insert(canon) {
                continue;
            }
            let Ok(entries) = crate::fs::read_dir(&dir) else {
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
                let (is_dir, is_file) = if ft.is_symlink() {
                    (path.tex_is_dir(), path.tex_is_file())
                } else {
                    (ft.is_dir(), ft.is_file())
                };
                if is_dir {
                    stack.push(path);
                    continue;
                }
                if !is_file {
                    continue;
                }
                if name == file_name && (!nested || path.ends_with(wanted)) {
                    return (Some(path), complete);
                }
                if !nested && ci_hit.is_none() && name.to_lowercase() == cand_lower {
                    ci_hit = Some(path);
                }
            }
        }
    }
    (ci_hit, complete)
}

fn directory_entries_fingerprint(entries: &[(crate::fs::DirEntry, crate::fs::FileType)]) -> u64 {
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
        if !path.tex_is_file() {
            continue;
        }
        if let Some(packed) = filename_index::Packed::load(&path) {
            let (size, hash) = dependency_content_identity(packed.source_bytes());
            lsr.source_dependency = Some((path, size, hash));
            lsr.packed = Some(packed);
        } else if let Ok(bytes) = crate::fs::read(&path) {
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
        let assets = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"));
        let mut part_paths: Vec<_> = std::fs::read_dir(assets)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|name| name.starts_with("packages.tar.zst."))
            })
            .collect();
        part_paths.sort();
        assert!(!part_paths.is_empty());
        // Same precedence as build.rs: overlay members shadow main members.
        let mut readers: Vec<(Box<dyn Read>, usize)> = Vec::new();
        let supplement = assets.join("packages-supplement.tar.zst");
        if supplement.exists() {
            readers.push((Box::new(std::fs::File::open(supplement).unwrap()), usize::MAX));
        }
        let mut chained: Box<dyn Read> = Box::new(std::io::empty());
        for p in part_paths {
            chained = Box::new(chained.chain(std::fs::File::open(p).unwrap()));
        }
        readers.push((chained, 200));
        let mut seen = HashSet::new();
        for (reader, limit) in readers {
            let decoder =
                ruzstd::decoding::StreamingDecoder::new_with_max_window_size(reader, 512 << 20)
                    .unwrap();
            let mut archive = tar::Archive::new(decoder);
            let mut checked = 0;
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
                checked += 1;
                if checked >= limit {
                    break;
                }
            }
        }
        assert!(seen.len() >= 200);
        assert_eq!(
            get_embedded_package("ARTICLE.CLS"),
            get_embedded_package("article.cls")
        );
    }

    #[test]
    fn supplement_completes_beamer_icons_and_newpx_outlines() {
        let icon = get_embedded_package("beamericonbook.pdf").expect("beamer icon");
        assert!(icon.starts_with(b"%PDF"));
        assert!(get_embedded_package("example-image-a.pdf").is_some());
        assert!(has_embedded_package("zplb.pfb") && has_embedded_package("zplmi.vf"));
        let map = String::from_utf8(get_embedded_package("pdftex.map").unwrap()).unwrap();
        let record = map.lines().find(|line| line.starts_with("zpl-Bold-tlf-t1 ")).unwrap();
        assert!(record.ends_with("<zplb.pfb"), "{record}");
        // The consolidated map keeps the main archive's records.
        assert!(map.lines().any(|line| line.starts_with("cmr10 ")));
    }

    #[test]
    fn embedded_virtual_font_lookup_without_host_files() {
        let memory = fs::MemoryFs::new(Path::new("/project"), 0).unwrap();
        let _scope = memory.enter();
        let kpse = Kpse::explicit(Path::new("/project"), Vec::new());
        let data = kpse
            .read("udmj65", Format::Vf)
            .expect("bundled virtual font");
        assert_eq!(&data[..2], &[247, 202], "valid VF preamble");
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
    fn embedded_tex_tree_lookup_excludes_font_and_map_files() {
        // kpathsea's TEXINPUTS search never reaches fonts/ or other trees.
        for name in ["cmr10.tfm", "cmr10.pfb", "8r.enc", "pdftex.map"] {
            assert!(has_embedded_package(name), "{name} is embedded");
            assert!(get_embedded_tex_tree_input(name).is_none(), "{name}");
        }
        assert_eq!(get_embedded_tex_tree_input("article.cls").unwrap().0, "article.cls");
    }

    #[test]
    fn packed_package_index_is_sorted_and_self_consistent() {
        fn records<const N: usize>(table: PackedTable<N>) -> Vec<[u32; N]> {
            assert_eq!(table.0.len() % (N * 4), 0, "table holds whole records");
            (0..table.len()).map(|i| table.get(i).unwrap()).collect()
        }
        let index = records(PACKAGE_INDEX);
        let folded: Vec<u32> = records(PACKAGE_FOLDED).into_iter().map(|[i]| i).collect();
        let chunks = records(PACKAGE_CHUNKS);
        assert!(!index.is_empty() && !chunks.is_empty());
        assert!(index
            .windows(2)
            .all(|entries| package_name(entries[0]) < package_name(entries[1])));
        assert!(folded.windows(2).all(|positions| {
            let left = index[positions[0] as usize];
            let right = index[positions[1] as usize];
            ascii_folded_cmp(package_name(left), package_name(right)) == std::cmp::Ordering::Less
        }));
        let mut expected = std::collections::BTreeMap::new();
        for (position, &package) in index.iter().enumerate() {
            let name =
                std::str::from_utf8(package_name(package)).expect("archive filenames are UTF-8");
            expected
                .entry(name.to_ascii_lowercase())
                .or_insert(position as u32);
            let [_, _, chunk, offset, length, _] = package;
            let [_, _, decoded] = chunks[chunk as usize];
            if offset + length > decoded {
                // A large member fills consecutive whole chunks.
                assert_eq!(offset, 0, "{name} starts its first frame");
                let mut covered = 0;
                let mut next = chunk as usize;
                while covered < length {
                    covered += chunks[next][2];
                    next += 1;
                }
                assert_eq!(covered, length, "{name} ends with its last frame");
            }
        }
        assert_eq!(folded, expected.values().copied().collect::<Vec<_>>());
        let mut next_offset = 0;
        for &[offset, length, _] in &chunks {
            assert_eq!(offset, next_offset, "chunks are contiguous");
            next_offset += length;
        }
        assert_eq!(next_offset as usize, PACKAGES.len());
        // Both ends of the embedded archive decode, so it is neither
        // truncated nor shifted relative to the chunk table. The shortest
        // chunk is smaller than the window its frame header declares.
        let shortest = *chunks.iter().min_by_key(|[_, _, decoded]| *decoded).unwrap();
        for [offset, length, decoded] in [chunks[0], chunks[chunks.len() - 1], shortest] {
            let compressed = &PACKAGES[offset as usize..(offset + length) as usize];
            assert!(decode_package_chunk(compressed, decoded as usize).is_some());
        }
    }

    #[test]
    fn large_members_decode_from_parallel_frames_like_one_stream() {
        let index = (0..PACKAGE_INDEX.len())
            .max_by_key(|&i| PACKAGE_INDEX.get(i).unwrap()[4])
            .unwrap();
        let [_, _, first, _, length, _] = PACKAGE_INDEX.get(index).unwrap();
        let mut sequential = Vec::new();
        let mut chunk = first as usize;
        while sequential.len() < length as usize {
            let [offset, compressed, decoded] = PACKAGE_CHUNKS.get(chunk).unwrap();
            let frame = &PACKAGES[offset as usize..(offset + compressed) as usize];
            sequential.extend(decode_package_chunk(frame, decoded as usize).unwrap());
            chunk += 1;
        }
        assert!(chunk - first as usize > 1, "the largest member spans several frames");
        assert_eq!(read_package_entry(index).unwrap(), sequential);
    }

    #[test]
    fn chunk_cache_stays_within_its_byte_budget() {
        let mut cache = ChunkCache::default();
        // A chunk holding one member as large as a frame (build.rs).
        const LARGEST_CACHED: usize = 1024 * 1024;
        let chunk = |len: usize| std::sync::Arc::<[u8]>::from(vec![0; len]);
        for index in 0..64 {
            cache.insert(index, chunk(LARGEST_CACHED / 3));
            assert!(cache.bytes <= CHUNK_CACHE_BYTES);
            assert_eq!(cache.bytes, cache.entries.iter().map(|(_, c)| c.len()).sum::<usize>());
        }
        // Least recently used goes first; a lookup refreshes an entry.
        let oldest = cache.entries.front().unwrap().0;
        assert!(cache.get(oldest).is_some());
        cache.insert(1000, chunk(LARGEST_CACHED));
        assert!(cache.get(oldest).is_some());
        assert!(cache.get(0).is_none());
    }

    #[test]
    fn explicit_paths_never_resolve_to_bundled_files() {
        assert!(get_embedded_package("article.cls").is_some());
        assert!(get_embedded_package("base/article.cls").is_some());
        for name in ["./article.cls", "../article.cls", "/no/such/dir/article.cls"] {
            assert!(!has_embedded_package(name), "{name}");
            assert!(get_embedded_package(name).is_none(), "{name}");
        }
        assert!(get_embedded_tex_input("xkeyval").is_some());
        assert!(get_embedded_tex_input("./xkeyval").is_none());
        assert!(get_embedded_tex_input("../xkeyval.tex").is_none());
        let kpse = Kpse::explicit(Path::new("/nonexistent-project"), Vec::new());
        assert!(kpse.read("cmr10", Format::Tfm).is_some());
        assert!(kpse.read("../cmr10", Format::Tfm).is_none());
    }

    #[test]
    fn path_variables_map_to_their_kpathsea_formats() {
        let paths = Kpse::extra_paths_from(|variable| match variable {
            "T1FONTS" => Some("/t1".into()),
            "TTFONTS" => Some("/tt".into()),
            "TEXINPUTS" => Some(std::env::join_paths(["/a//", "", "/b"]).unwrap()),
            _ => None,
        });
        assert_eq!(paths[&Format::Type1], [(PathBuf::from("/t1"), false)]);
        assert_eq!(paths[&Format::Truetype], [(PathBuf::from("/tt"), false)]);
        assert_eq!(
            paths[&Format::Tex],
            [(PathBuf::from("/a"), true), (PathBuf::from("/b"), false)]
        );
    }

    #[test]
    fn recursive_path_variables_precede_the_project_directory() {
        let tmp = TempDir::new("texinputs-recursive");
        let cwd = tmp.path().join("project");
        let styles = tmp.path().join("styles");
        tmp.write("project/shared.sty", "% project copy");
        let selected = tmp.write("styles/deep/er/shared.sty", "% styles copy");
        let mut kpse = Kpse::explicit(&cwd, Vec::new());
        kpse.extra_paths.insert(Format::Tex, vec![(styles.clone(), true)]);

        assert_eq!(kpse.find("shared.sty", Format::Tex), Some(selected.clone()));
        assert_eq!(
            kpse.explain_lookup("shared.sty", Format::Tex).source_kind,
            Some(LookupSourceKind::EnvironmentPath)
        );
        assert_eq!(kpse.read("shared.sty", Format::Tex).unwrap(), b"% styles copy");
        let (present, directories, _, _, _, complete) =
            kpse.lookup_dependencies("shared.sty", Format::Tex, Some(&selected));
        assert!(complete);
        assert!(present.contains(&selected));
        assert!(directories.iter().any(|(path, _)| path == &styles.join("deep")));
        // A nested name matches by path suffix within the subtree.
        assert_eq!(kpse.find("er/shared.sty", Format::Tex), Some(selected));
    }

    #[test]
    fn an_exhausted_walk_budget_is_not_proof_of_absence() {
        let tmp = TempDir::new("walk-budget");
        tmp.write("tree/a/b/c/other.sty", "");
        let starts = [(tmp.path().join("tree"), true)];
        assert_eq!(walk_find_impl(&starts, "absent.sty", 2, |_, _| {}), (None, false));
        assert_eq!(walk_find_impl(&starts, "absent.sty", 100, |_, _| {}), (None, true));
    }

    #[cfg(unix)]
    #[test]
    fn unindexed_walks_follow_directory_symlinks() {
        let tmp = TempDir::new("walk-symlink");
        let want = tmp.write("elsewhere/linked.fd", "% fd");
        std::fs::create_dir_all(tmp.path().join("tree/tex/latex")).unwrap();
        std::os::unix::fs::symlink(
            tmp.path().join("elsewhere"),
            tmp.path().join("tree/tex/latex/pkg"),
        )
        .unwrap();
        let kpse = Kpse::explicit(tmp.path(), vec![tmp.path().join("tree")]);
        assert_eq!(
            kpse.find("linked.fd", Format::Tex),
            Some(tmp.path().join("tree/tex/latex/pkg/linked.fd"))
        );
        assert!(want.is_file());
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
        let tlf = kpse
            .find("t1ntxtlf.fd", Format::Tex)
            .expect("t1ntxtlf.fd");
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
        kpse.extra_paths.insert(Format::Tex, vec![(extra, false)]);

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
