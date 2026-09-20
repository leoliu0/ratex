#![allow(dead_code)]

#[path = "../allocator.rs"]
mod allocator;
#[global_allocator]
static GLOBAL: allocator::EngineAllocator = allocator::EngineAllocator;

#[cfg(test)]
use tex_core::driver::png_embed_options;
/// Precompiled format containing standard LaTeX packages baked directly into the binary.
use tex_core::driver::{finalize_format_load, install_pdftex_config_registers};

static EMBEDDED_DEFAULT_FMT: &[u8] = include_bytes!("../../assets/default.fmt.zst");
const DEPCACHE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const DEPCACHE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 60 * 60);
const DEPCACHE_GC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);
const DEPCACHE_TOUCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);
const DEPCACHE_RECORD_MAX_BYTES: u64 = 8 * 1024 * 1024;
const TEXMK_CACHE_HIT_MARKER_ENV: &str = "TEX_RS_CACHE_HIT_MARKER";
const TEXMK_PUBLISHED_OUTPUT_ENV: &str = "TEX_RS_TEXMK_PUBLISHED_OUTPUT";
const DEPCACHE_END_DOMAIN: &[u8] = b"TEX-DEPCACHE-7-END";

fn platform_cache_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("TEX_RS_CACHE_DIR").filter(|value| !value.is_empty()) {
        return std::path::PathBuf::from(dir);
    }
    #[cfg(target_os = "windows")]
    if let Some(dir) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return std::path::PathBuf::from(dir).join("tex-rs").join("cache");
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return std::path::PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("tex-rs");
    }
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME").filter(|value| !value.is_empty()) {
        return std::path::PathBuf::from(dir).join("tex-rs");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return std::path::PathBuf::from(home).join(".cache").join("tex-rs");
    }
    std::env::temp_dir().join("tex-rs-cache")
}

fn absolute_path(path: &std::path::Path) -> std::path::PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(path)
    };
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Anchor a dependency spelling without resolving symlinks. Metadata-only
/// dependencies must keep the path TeX queried so retargeting a symlink is
/// visible to cache validation.
fn anchored_path(path: &std::path::Path) -> std::path::PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(path)
    }
}

/// Return texmk's public output only when the companion cache-hit marker
/// proves that this process was launched with the requested private cache.
/// Canonicalizing the existing parent gives directory dependency paths the
/// same spelling even while the output itself does not exist yet.
fn texmk_published_output(cache_root: &std::path::Path) -> Option<std::path::PathBuf> {
    let marker = std::env::var_os(TEXMK_CACHE_HIT_MARKER_ENV)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)?;
    let output = std::env::var_os(TEXMK_PUBLISHED_OUTPUT_ENV)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)?;
    if !marker.is_absolute() || !output.is_absolute() {
        return None;
    }
    let cache_root = std::fs::canonicalize(cache_root).ok()?;
    let marker_parent = std::fs::canonicalize(marker.parent()?).ok()?;
    if marker_parent != cache_root {
        return None;
    }
    let name = output.file_name()?.to_owned();
    let parent = std::fs::canonicalize(output.parent()?).ok()?;
    Some(parent.join(name))
}

fn hex_digit(value: u8) -> char {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    HEX[usize::from(value & 0x0f)] as char
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Encode a path as one delimiter-safe, reversible record field. Ordinary
/// ASCII paths remain readable; `%` and bytes/code units which cannot appear
/// literally in the line protocol are escaped.
#[cfg(unix)]
fn encode_record_path(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_os_str().as_bytes();
    let mut encoded = String::with_capacity(bytes.len());
    for &byte in bytes {
        if matches!(byte, 0x20..=0x7e) && byte != b'%' {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte));
        }
    }
    encoded
}

#[cfg(unix)]
fn decode_record_path(encoded: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    let encoded = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut offset = 0;
    while offset < encoded.len() {
        if encoded[offset] == b'%' {
            let high = hex_value(*encoded.get(offset + 1)?)?;
            let low = hex_value(*encoded.get(offset + 2)?)?;
            decoded.push((high << 4) | low);
            offset += 3;
        } else {
            if !matches!(encoded[offset], 0x20..=0x7e) {
                return None;
            }
            decoded.push(encoded[offset]);
            offset += 1;
        }
    }
    Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(
        decoded,
    )))
}

#[cfg(windows)]
fn encode_record_path(path: &std::path::Path) -> String {
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = String::new();
    for unit in path.as_os_str().encode_wide() {
        if matches!(unit, 0x20..=0x7e) && unit != u16::from(b'%') {
            encoded.push(char::from(unit as u8));
        } else {
            encoded.push('%');
            for shift in [12, 8, 4, 0] {
                encoded.push(hex_digit(((unit >> shift) & 0x0f) as u8));
            }
        }
    }
    encoded
}

#[cfg(windows)]
fn decode_record_path(encoded: &str) -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    let encoded = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut offset = 0;
    while offset < encoded.len() {
        if encoded[offset] == b'%' {
            let mut unit = 0u16;
            for digit in encoded.get(offset + 1..offset + 5)? {
                unit = (unit << 4) | u16::from(hex_value(*digit)?);
            }
            decoded.push(unit);
            offset += 5;
        } else {
            if !matches!(encoded[offset], 0x20..=0x7e) {
                return None;
            }
            decoded.push(u16::from(encoded[offset]));
            offset += 1;
        }
    }
    Some(std::path::PathBuf::from(std::ffi::OsString::from_wide(
        &decoded,
    )))
}

#[cfg(not(any(unix, windows)))]
fn encode_record_path(path: &std::path::Path) -> String {
    let bytes = path.to_string_lossy();
    let mut encoded = String::with_capacity(bytes.len());
    for byte in bytes.bytes() {
        if matches!(byte, 0x20..=0x7e) && byte != b'%' {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte));
        }
    }
    encoded
}

#[cfg(not(any(unix, windows)))]
fn decode_record_path(encoded: &str) -> Option<std::path::PathBuf> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            let high = hex_value(*bytes.get(offset + 1)?)?;
            let low = hex_value(*bytes.get(offset + 2)?)?;
            decoded.push((high << 4) | low);
            offset += 3;
        } else {
            if !matches!(bytes[offset], 0x20..=0x7e) {
                return None;
            }
            decoded.push(bytes[offset]);
            offset += 1;
        }
    }
    Some(std::path::PathBuf::from(String::from_utf8(decoded).ok()?))
}

fn stable_hash(parts: &[&[u8]]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn valid_jobname(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && matches!(
            std::path::Path::new(name)
                .components()
                .collect::<Vec<_>>()
                .as_slice(),
            [std::path::Component::Normal(_)]
        )
}

fn depcache_path(
    cache_root: &std::path::Path,
    primary_file: &str,
    job: &str,
    out_dir: &str,
    aux_dir: &str,
    optimize_pdf_size: bool,
) -> std::path::PathBuf {
    // Keep the spelling used by this invocation: relative inputs are resolved
    // from that spelling's parent, so two symlinks to one source are not
    // interchangeable cache jobs.
    let source = anchored_path(std::path::Path::new(primary_file));
    let output = absolute_path(std::path::Path::new(if out_dir.is_empty() {
        "."
    } else {
        out_dir
    }));
    let aux = absolute_path(std::path::Path::new(if aux_dir.is_empty() {
        "."
    } else {
        aux_dir
    }));
    let source = encode_record_path(&source);
    let output = encode_record_path(&output);
    let aux = encode_record_path(&aux);
    let identity = cache_identity().unwrap_or_default();
    let program = program_name();
    let cwd = absolute_path(std::path::Path::new("."));
    let cwd = encode_record_path(&cwd);
    let format_overrides = format_override_identity();
    let clock = effective_clock_identity();
    let key = stable_hash(&[
        source.as_bytes(),
        job.as_bytes(),
        output.as_bytes(),
        aux.as_bytes(),
        identity.as_bytes(),
        program.as_bytes(),
        cwd.as_bytes(),
        format_overrides.as_bytes(),
        clock.as_bytes(),
        if optimize_pdf_size {
            b"size" as &[u8]
        } else {
            b"speed" as &[u8]
        },
    ]);
    cache_root
        .join("depcache")
        .join(format!("{key:016x}.depcache"))
}

fn format_override_identity() -> String {
    let cwd = absolute_path(std::path::Path::new("pdflatex.fmt"));
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("pdflatex.fmt")));
    let mut candidates = vec![cwd];
    if let Some(path) = executable {
        candidates.push(path);
    }
    candidates.sort();
    candidates.dedup();
    let mut identity = String::new();
    for path in candidates {
        let path = absolute_path(&path);
        identity.push_str(&encode_record_path(&path));
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                let hash = dependency_fingerprint(&path, metadata.len()).unwrap_or(0);
                identity.push_str(&format!("={}:{}", metadata.len(), hash));
            }
            _ => identity.push_str("=<missing>"),
        }
        identity.push('\n');
    }
    identity
}

fn effective_clock_identity() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    effective_clock_identity_at(std::env::var_os("SOURCE_DATE_EPOCH").as_deref(), now)
}

fn effective_clock_identity_at(source_date_epoch: Option<&std::ffi::OsStr>, now: u64) -> String {
    if let Some(epoch) = source_date_epoch
        .and_then(std::ffi::OsStr::to_str)
        .and_then(|epoch| epoch.trim().parse::<i64>().ok())
    {
        return format!("source-date-epoch={epoch}");
    }
    format!("live-minute={}", now / 60)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct FileStamp {
    size: u64,
    mtime_sec: i64,
    mtime_nsec: i64,
    dev: u64,
    ino: u64,
    ctime_sec: i64,
    ctime_nsec: i64,
}

impl FileStamp {
    fn from_metadata(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                size: meta.len(),
                mtime_sec: meta.mtime(),
                mtime_nsec: meta.mtime_nsec(),
                dev: meta.dev(),
                ino: meta.ino(),
                ctime_sec: meta.ctime(),
                ctime_nsec: meta.ctime_nsec(),
            }
        }
        #[cfg(not(unix))]
        {
            let (mtime_sec, mtime_nsec) = match meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            {
                Some(time) => (time.as_secs() as i64, time.subsec_nanos() as i64),
                None => (i64::MIN, 0),
            };
            Self {
                size: meta.len(),
                mtime_sec,
                mtime_nsec,
                dev: 0,
                ino: 0,
                ctime_sec: 0,
                ctime_nsec: 0,
            }
        }
    }

    fn missing() -> Self {
        Self {
            size: u64::MAX,
            mtime_sec: i64::MIN,
            mtime_nsec: 0,
            dev: 0,
            ino: 0,
            ctime_sec: i64::MIN,
            ctime_nsec: 0,
        }
    }

    fn is_missing(self) -> bool {
        self.size == u64::MAX
    }

    fn parse(parts: &mut std::str::Split<'_, char>) -> Option<Self> {
        let size = parts.next()?.parse().ok()?;
        let mtime_sec = parts.next()?.parse().ok()?;
        let mtime_nsec = parts.next()?.parse().ok()?;
        Some(Self {
            size,
            mtime_sec,
            mtime_nsec,
            dev: parts.next()?.parse().ok()?,
            ino: parts.next()?.parse().ok()?,
            ctime_sec: parts.next()?.parse().ok()?,
            ctime_nsec: parts.next()?.parse().ok()?,
        })
    }
}

fn push_stamped_entry(
    out: &mut String,
    kind: &str,
    path: &std::path::Path,
    stamp: FileStamp,
    hash: u64,
) {
    use std::fmt::Write;
    let _ = writeln!(
        out,
        "{kind}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{hash}",
        encode_record_path(path),
        stamp.size,
        stamp.mtime_sec,
        stamp.mtime_nsec,
        stamp.dev,
        stamp.ino,
        stamp.ctime_sec,
        stamp.ctime_nsec,
    );
}

fn parse_stamped_entry(rest: &str) -> Option<(std::path::PathBuf, FileStamp, u64)> {
    let mut parts = rest.split('\t');
    let path = decode_record_path(parts.next()?)?;
    let stamp = FileStamp::parse(&mut parts)?;
    let hash = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((path, stamp, hash))
}

fn seal_depcache_record(record: &mut String) -> bool {
    use std::fmt::Write;

    let body_len = record.len();
    let authenticator = stable_hash(&[DEPCACHE_END_DOMAIN, record.as_bytes()]);
    let _ = writeln!(record, "END\t{body_len}\t{authenticator:016x}");
    record.len() as u64 <= DEPCACHE_RECORD_MAX_BYTES
}

/// Return the authenticated record body, including its final newline. A
/// missing/truncated marker and any bytes appended after it fail closed.
fn authenticated_depcache_body(record: &str) -> Option<&str> {
    let without_final_newline = record.strip_suffix('\n')?;
    let marker_start = without_final_newline.rfind('\n')?.checked_add(1)?;
    let marker = &without_final_newline[marker_start..];
    let mut fields = marker.split('\t');
    if fields.next()? != "END" {
        return None;
    }
    let expected_len: usize = fields.next()?.parse().ok()?;
    let expected_authenticator = fields.next()?;
    if expected_authenticator.len() != 16 || fields.next().is_some() {
        return None;
    }
    let expected_authenticator = u64::from_str_radix(expected_authenticator, 16).ok()?;
    let body = record.get(..marker_start)?;
    if body.len() != expected_len
        || stable_hash(&[DEPCACHE_END_DOMAIN, body.as_bytes()]) != expected_authenticator
    {
        return None;
    }
    Some(body)
}

#[cfg(unix)]
fn stamp_allows_hash_skip(
    current: FileStamp,
    stored: FileStamp,
    cache_meta: &std::fs::Metadata,
) -> bool {
    use std::os::unix::fs::MetadataExt;

    let published = (cache_meta.mtime(), cache_meta.mtime_nsec());
    current == stored && (current.ctime_sec, current.ctime_nsec) < published
}

#[cfg(not(unix))]
fn stamp_allows_hash_skip(
    _current: FileStamp,
    _stored: FileStamp,
    _cache_meta: &std::fs::Metadata,
) -> bool {
    false
}

fn cache_identity() -> Option<String> {
    static IDENTITY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    IDENTITY.get_or_init(compute_cache_identity).clone()
}

fn compute_cache_identity() -> Option<String> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let exe = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(&exe).ok()?;
    exe.hash(&mut hash);
    #[cfg(unix)]
    FileStamp::from_metadata(&meta).hash(&mut hash);
    #[cfg(not(unix))]
    {
        meta.len().hash(&mut hash);
        meta.modified().ok()?.hash(&mut hash);
        dependency_fingerprint(&exe, meta.len())?.hash(&mut hash);
    }
    for key in [
        "TEXINPUTS",
        "TEXMFHOME",
        "TEXMFLOCAL",
        "TEXMFDIST",
        "TEX_SUITE_DATA",
        "HOME",
        "TFMFONTS",
        "VFFONTS",
        "T1FONTS",
        "TTFONTS",
        "OPENTYPEFONTS",
        "ENCFONTS",
        "TEXFONTMAPS",
        "BSTINPUTS",
        "BIBINPUTS",
        "TEX_RS_HERMETIC",
        "SOURCE_DATE_EPOCH",
        "TZ",
        TEXMK_PUBLISHED_OUTPUT_ENV,
    ] {
        std::env::var_os(key).hash(&mut hash);
    }
    Some(format!("TEX-DEPCACHE-7 {:016x}", hash.finish()))
}

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn dependency_fingerprint(path: &std::path::Path, size: u64) -> Option<u64> {
    use std::io::Read;
    let mut hash = hash_bytes(0xcbf2_9ce4_8422_2325, &size.to_le_bytes());
    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hash = hash_bytes(hash, &buffer[..read]);
    }
    Some(hash)
}

fn stable_dependency_identity(path: &std::path::Path) -> Option<(FileStamp, u64)> {
    let before_meta = std::fs::metadata(path).ok()?;
    if !before_meta.is_file() {
        return None;
    }
    let before = FileStamp::from_metadata(&before_meta);
    let hash = dependency_fingerprint(path, before.size)?;
    let after_meta = std::fs::metadata(path).ok()?;
    if !after_meta.is_file() {
        return None;
    }
    let after = FileStamp::from_metadata(&after_meta);
    (before == after).then_some((after, hash))
}

fn dependency_identity_matches(
    path: &std::path::Path,
    stored: FileStamp,
    hash: u64,
    cache_meta: &std::fs::Metadata,
) -> bool {
    if stored.is_missing() {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let current = FileStamp::from_metadata(&meta);
    if stamp_allows_hash_skip(current, stored, cache_meta) {
        return true;
    }
    current.size == stored.size && dependency_fingerprint(path, current.size) == Some(hash)
}

fn stable_directory_identity(path: &std::path::Path) -> Option<(FileStamp, u64)> {
    let before_meta = std::fs::metadata(path).ok()?;
    if !before_meta.is_dir() {
        return None;
    }
    let before = FileStamp::from_metadata(&before_meta);
    let hash = tex_kpse::directory_fingerprint(path)?;
    let after_meta = std::fs::metadata(path).ok()?;
    if !after_meta.is_dir() {
        return None;
    }
    let after = FileStamp::from_metadata(&after_meta);
    (before == after).then_some((after, hash))
}

fn stable_directory_identity_excluding(
    path: &std::path::Path,
    excluded_name: &std::ffi::OsStr,
) -> Option<(FileStamp, u64)> {
    let before_meta = std::fs::metadata(path).ok()?;
    if !before_meta.is_dir() {
        return None;
    }
    let before = FileStamp::from_metadata(&before_meta);
    let hash = tex_kpse::directory_fingerprint_excluding(path, &[excluded_name])?;
    let after_meta = std::fs::metadata(path).ok()?;
    if !after_meta.is_dir() {
        return None;
    }
    let after = FileStamp::from_metadata(&after_meta);
    (before == after).then_some((after, hash))
}

fn published_name_in_directory<'a>(
    directory: &std::path::Path,
    published_output: Option<&'a std::path::Path>,
) -> Option<&'a std::ffi::OsStr> {
    // Full recursive TEXMF directory snapshots do not enumerate a concrete
    // MISS for every possible child. Restrict this exception to cwd, where
    // every local candidate is probed and recorded before casefold fallback.
    if absolute_path(directory) != absolute_path(std::path::Path::new(".")) {
        return None;
    }
    let output = published_output?;
    let output_parent = output.parent()?;
    (absolute_path(output_parent) == absolute_path(directory))
        .then(|| output.file_name())
        .flatten()
}

fn dependency_name_may_match(
    path: &std::path::Path,
    directory: &std::path::Path,
    published_name: &std::ffi::OsStr,
) -> bool {
    let path = anchored_path(path);
    path.parent() == Some(directory)
        && path.file_name().is_some_and(|name| {
            if name == published_name {
                return true;
            }
            match (name.to_str(), published_name.to_str()) {
                (Some(name), Some(published_name)) => {
                    name.to_lowercase() == published_name.to_lowercase()
                }
                // The resolver compares non-UTF-8 names through a lossy
                // spelling. Treat any such name as ambiguous and retain the
                // complete directory fingerprint.
                _ => true,
            }
        })
}

fn directory_identity_matches(
    path: &std::path::Path,
    stored: FileStamp,
    hash: u64,
    cache_meta: &std::fs::Metadata,
    excluded_name: Option<&std::ffi::OsStr>,
) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_dir() {
        return false;
    }
    let current = FileStamp::from_metadata(&meta);
    if stamp_allows_hash_skip(current, stored, cache_meta) {
        return true;
    }
    (match excluded_name {
        Some(name) => tex_kpse::directory_fingerprint_excluding(path, &[name]),
        None => tex_kpse::directory_fingerprint(path),
    }) == Some(hash)
}

fn depcache_record_key(path: &std::path::Path) -> Option<&str> {
    path.file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .filter(|key| key.len() == 16 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn read_depcache_record(path: &std::path::Path) -> Option<(std::fs::Metadata, String)> {
    use std::io::Read;

    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > DEPCACHE_RECORD_MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).ok()?);
    file.take(DEPCACHE_RECORD_MAX_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
        return None;
    }
    Some((metadata, String::from_utf8(bytes).ok()?))
}

/// Dependency-cache hit: if no tracked input changed since the last
/// successful compile, the output PDF is already current.
fn check_depcache(
    cache_path: &std::path::Path,
    primary_file: &str,
    expected_pdf: &std::path::Path,
    expected_log: &std::path::Path,
    published_output: Option<&std::path::Path>,
) -> Option<usize> {
    let (cache_meta, content) = read_depcache_record(cache_path)?;
    let mut lines = authenticated_depcache_body(&content)?.lines();
    if lines.next()? != cache_identity()?.as_str() {
        return None;
    }
    if lines.next()?.strip_prefix("KEY\t")? != depcache_record_key(cache_path)? {
        return None;
    }
    let stored_source = decode_record_path(lines.next()?.strip_prefix("SOURCE\t")?)?;
    if stored_source != anchored_path(std::path::Path::new(primary_file)) {
        return None;
    }
    let pdf_line = lines.next()?.strip_prefix("PDF\t")?;
    let (pdf_path, pdf_stamp, pdf_hash) = parse_stamped_entry(pdf_line)?;
    if absolute_path(&pdf_path) != absolute_path(expected_pdf) {
        return None;
    }
    let pdf_size = usize::try_from(pdf_stamp.size).ok()?;
    if !dependency_identity_matches(&pdf_path, pdf_stamp, pdf_hash, &cache_meta) {
        return None;
    }
    if std::fs::metadata(primary_file).is_err() || !expected_log.is_file() {
        return None;
    }
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(path) = line.strip_prefix("DIRMISS\t") {
            if decode_record_path(path)?.is_dir() {
                return None;
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("MISS\t") {
            if decode_record_path(path)?.is_file() {
                return None;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("AUX\t") {
            let (path, stamp, hash) = parse_stamped_entry(rest)?;
            if content_identity_matches(&path, stamp, hash, &cache_meta) {
                continue;
            }
            return None;
        }
        if let Some(rest) = line.strip_prefix("READ\t") {
            let (path, stamp, hash) = parse_stamped_entry(rest)?;
            if !content_identity_matches(&path, stamp, hash, &cache_meta) {
                return None;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("DIRX\t") {
            let (path, stamp, hash) = parse_stamped_entry(rest)?;
            let excluded_name = published_name_in_directory(&path, published_output)?;
            if !directory_identity_matches(&path, stamp, hash, &cache_meta, Some(excluded_name)) {
                return None;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("DIR\t") {
            let (path, stamp, hash) = parse_stamped_entry(rest)?;
            if !directory_identity_matches(&path, stamp, hash, &cache_meta, None) {
                return None;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("SIZE\t") {
            let (path, expected_size) = rest.split_once('\t')?;
            let path = decode_record_path(path)?;
            let expected_size: u64 = expected_size.parse().ok()?;
            if std::fs::metadata(&path)
                .ok()
                .filter(|metadata| metadata.is_file())
                .map(|metadata| metadata.len())
                != Some(expected_size)
            {
                return None;
            }
            continue;
        }
        let rest = line.strip_prefix("FILE\t")?;
        let (path, stamp, hash) = parse_stamped_entry(rest)?;
        if !dependency_identity_matches(&path, stamp, hash, &cache_meta) {
            return None;
        }
    }
    Some(pdf_size)
}

/// Derived-state files (aux/toc/out): the engine both READS (previous pass)
/// and WRITES (this pass) them, so their mtimes always change between runs.
/// Track them by CONTENT HASH of the state the compile started from: a cache
/// hit is valid iff the current aux content equals what this compile saw.
fn aux_state_paths(job: &str, out_dir: &str) -> Vec<std::path::PathBuf> {
    ["aux", "toc", "out", "lof", "lot", "nav", "snm"]
        .iter()
        .map(|ext| std::path::PathBuf::from(format!("{}{}.{}", out_dir, job, ext)))
        .collect()
}

fn extend_content_digest(h1: &mut u64, h2: &mut u64, size: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *h1 ^= u64::from(*byte);
        *h1 = h1.wrapping_mul(0x1000_0000_01b3);
        *h2 = (*h2 + u64::from(*byte) + *size).wrapping_mul(0x1000_0000_01b3);
        *size += 1;
    }
}

fn content_digest(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = [0u8; 64 * 1024];
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        extend_content_digest(&mut h1, &mut h2, &mut size, &buffer[..read]);
    }
    Some((size, h1 ^ h2))
}

fn stable_content_identity(path: &std::path::Path) -> Option<(FileStamp, (u64, u64))> {
    let before_meta = std::fs::metadata(path).ok()?;
    if !before_meta.is_file() {
        return None;
    }
    let before = FileStamp::from_metadata(&before_meta);
    let digest = content_digest(path)?;
    let after_meta = std::fs::metadata(path).ok()?;
    if !after_meta.is_file() {
        return None;
    }
    let after = FileStamp::from_metadata(&after_meta);
    (before == after && digest.0 == after.size).then_some((after, digest))
}

fn content_identity_matches(
    path: &std::path::Path,
    stored: FileStamp,
    hash: u64,
    cache_meta: &std::fs::Metadata,
) -> bool {
    if stored.is_missing() {
        return !std::fs::metadata(path).is_ok_and(|meta| meta.is_file());
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let current = FileStamp::from_metadata(&meta);
    if stamp_allows_hash_skip(current, stored, cache_meta) {
        return true;
    }
    content_digest(path) == Some((stored.size, hash))
}

fn snapshot_aux_state(job: &str, out_dir: &str) -> Vec<(std::path::PathBuf, u64, u64)> {
    aux_state_paths(job, out_dir)
        .into_iter()
        .map(|p| {
            let entry = content_digest(&p).unwrap_or((u64::MAX, 0)); // absent = MAX/0
            (p, entry.0, entry.1)
        })
        .collect()
}

fn aux_state_is_unchanged(snapshot: &[(std::path::PathBuf, u64, u64)]) -> bool {
    snapshot
        .iter()
        .all(|(path, size, hash)| content_digest(path).unwrap_or((u64::MAX, 0)) == (*size, *hash))
}

#[cfg(not(windows))]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn atomic_write_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        drop(file);
        replace_file(&temporary, path)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

struct DepcacheInputs<'a> {
    primary_file: &'a str,
    pdf_path: &'a str,
    pdf_size: usize,
    deps: &'a [std::path::PathBuf],
    directories: &'a [(std::path::PathBuf, u64)],
    reads: &'a [(std::path::PathBuf, u64, u64)],
    sizes: &'a [(std::path::PathBuf, u64)],
    missing: &'a [std::path::PathBuf],
    missing_directories: &'a [std::path::PathBuf],
    outputs_missing_at_start: &'a [std::path::PathBuf],
    published_output: Option<&'a std::path::Path>,
    aux_start: &'a [(std::path::PathBuf, u64, u64)],
}

fn write_depcache(cache_path: &std::path::Path, inputs: DepcacheInputs<'_>) {
    use std::collections::{btree_map::Entry, BTreeSet};
    let Some(identity) = cache_identity() else {
        return;
    };
    let primary_file = anchored_path(std::path::Path::new(inputs.primary_file));
    let pdf_path = absolute_path(std::path::Path::new(inputs.pdf_path));
    let Some((pdf_stamp, pdf_hash)) = stable_dependency_identity(&pdf_path) else {
        return;
    };
    if u64::try_from(inputs.pdf_size).ok() != Some(pdf_stamp.size) {
        return;
    }
    let Some(record_key) = depcache_record_key(cache_path) else {
        return;
    };
    let mut out = format!(
        "{identity}\nKEY\t{record_key}\nSOURCE\t{}\n",
        encode_record_path(&primary_file)
    );
    push_stamped_entry(&mut out, "PDF", &pdf_path, pdf_stamp, pdf_hash);
    if out.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
        return;
    }
    let mut recorded_stamps = vec![(pdf_path.clone(), pdf_stamp)];
    let mut recorded_directory_stamps = Vec::new();
    let mut missing_file_paths = Vec::new();
    let mut missing_directory_paths = Vec::new();
    let mut read_identities = std::collections::BTreeMap::new();
    for (path, size, hash) in inputs.reads {
        match read_identities.entry(anchored_path(path)) {
            Entry::Vacant(entry) => {
                entry.insert((*size, *hash));
            }
            Entry::Occupied(entry) => {
                if *entry.get() != (*size, *hash) {
                    return;
                }
            }
        }
    }
    for (path, (size, hash)) in &read_identities {
        let Some((stamp, digest)) = stable_content_identity(path) else {
            return;
        };
        if digest != (*size, *hash) {
            return;
        }
        push_stamped_entry(&mut out, "READ", path, stamp, *hash);
        if out.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
            return;
        }
        recorded_stamps.push((path.clone(), stamp));
    }
    let mut size_identities = std::collections::BTreeMap::new();
    for (path, size) in inputs.sizes {
        match size_identities.entry(anchored_path(path)) {
            Entry::Vacant(entry) => {
                entry.insert(*size);
            }
            Entry::Occupied(entry) => {
                if *entry.get() != *size {
                    return;
                }
            }
        }
    }
    let mut recorded_sizes = Vec::with_capacity(size_identities.len());
    for (path, size) in &size_identities {
        if std::fs::metadata(path)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
            != Some(*size)
        {
            return;
        }
        use std::fmt::Write;
        let _ = writeln!(out, "SIZE\t{}\t{size}", encode_record_path(path));
        if out.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
            return;
        }
        recorded_sizes.push((path.clone(), *size));
    }
    let mut unique: BTreeSet<std::path::PathBuf> =
        inputs.deps.iter().map(|path| anchored_path(path)).collect();
    unique.insert(anchored_path(std::path::Path::new(inputs.primary_file)));
    for d in unique {
        if read_identities.contains_key(&d) {
            continue;
        }
        let Some((stamp, hash)) = stable_dependency_identity(&d) else {
            return;
        };
        push_stamped_entry(&mut out, "FILE", &d, stamp, hash);
        if out.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
            return;
        }
        recorded_stamps.push((d, stamp));
    }
    let missing: BTreeSet<std::path::PathBuf> = inputs
        .missing
        .iter()
        .map(|path| anchored_path(path))
        .collect();
    let missing_directories: BTreeSet<std::path::PathBuf> = inputs
        .missing_directories
        .iter()
        .map(|path| anchored_path(path))
        .collect();
    let mut directory_identities = std::collections::BTreeMap::new();
    for (path, fingerprint) in inputs.directories {
        match directory_identities.entry(anchored_path(path)) {
            Entry::Vacant(entry) => {
                entry.insert(*fingerprint);
            }
            Entry::Occupied(entry) => {
                if *entry.get() != *fingerprint {
                    return;
                }
            }
        }
    }
    for (directory, observed_fingerprint) in directory_identities {
        let Some((stamp, hash)) = stable_directory_identity(&directory) else {
            return;
        };
        if hash != observed_fingerprint {
            let excluded_names: Vec<_> = inputs
                .outputs_missing_at_start
                .iter()
                .map(|path| anchored_path(path))
                .filter(|path| path.parent() == Some(directory.as_path()) && path.is_file())
                .filter_map(|path| path.file_name().map(std::ffi::OsStr::to_owned))
                .filter(|name| {
                    !missing
                        .iter()
                        .chain(&missing_directories)
                        .any(|path| dependency_name_may_match(path, &directory, name))
                })
                .collect();
            let excluded_names: Vec<_> = excluded_names
                .iter()
                .map(std::ffi::OsString::as_os_str)
                .collect();
            if excluded_names.is_empty()
                || tex_kpse::directory_fingerprint_excluding(&directory, &excluded_names)
                    != Some(observed_fingerprint)
                || std::fs::metadata(&directory)
                    .ok()
                    .filter(|metadata| metadata.is_dir())
                    .map(|metadata| FileStamp::from_metadata(&metadata))
                    != Some(stamp)
            {
                return;
            }
        }
        let published_name = published_name_in_directory(&directory, inputs.published_output)
            .filter(|name| {
                !missing
                    .iter()
                    .chain(&missing_directories)
                    .any(|path| dependency_name_may_match(path, &directory, name))
            });
        let (record_kind, stored_hash) = if let Some(name) = published_name {
            let Some((excluded_stamp, excluded_hash)) =
                stable_directory_identity_excluding(&directory, name)
            else {
                return;
            };
            if excluded_stamp != stamp {
                return;
            }
            ("DIRX", excluded_hash)
        } else {
            ("DIR", hash)
        };
        push_stamped_entry(&mut out, record_kind, &directory, stamp, stored_hash);
        if out.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
            return;
        }
        recorded_directory_stamps.push((directory, stamp));
    }
    for path in &missing {
        if path.is_file() {
            return;
        }
        out.push_str(&format!("MISS\t{}\n", encode_record_path(path)));
        if out.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
            return;
        }
        missing_file_paths.push(path.clone());
    }
    for path in &missing_directories {
        if path.is_dir() {
            return;
        }
        out.push_str(&format!("DIRMISS\t{}\n", encode_record_path(path)));
        if out.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
            return;
        }
        missing_directory_paths.push(path.clone());
    }
    for (p, len, h) in inputs.aux_start {
        let path = absolute_path(p);
        if *len == u64::MAX {
            if path.is_file() {
                return;
            }
            push_stamped_entry(&mut out, "AUX", &path, FileStamp::missing(), 0);
            if out.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
                return;
            }
            missing_file_paths.push(path);
            continue;
        }
        let Some((stamp, digest)) = stable_content_identity(&path) else {
            return;
        };
        if digest != (*len, *h) {
            return;
        }
        push_stamped_entry(&mut out, "AUX", &path, stamp, *h);
        if out.len() as u64 > DEPCACHE_RECORD_MAX_BYTES {
            return;
        }
        recorded_stamps.push((path, stamp));
    }
    if !seal_depcache_record(&mut out) {
        return;
    }
    let Some(parent) = cache_path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary = cache_path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut file| std::io::Write::write_all(&mut file, out.as_bytes()));
    let inputs_unchanged = recorded_stamps.iter().all(|(path, stamp)| {
        std::fs::metadata(path)
            .ok()
            .filter(|meta| meta.is_file())
            .map(|meta| FileStamp::from_metadata(&meta))
            == Some(*stamp)
    }) && recorded_sizes.iter().all(|(path, size)| {
        std::fs::metadata(path)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
            == Some(*size)
    }) && recorded_directory_stamps.iter().all(|(path, stamp)| {
        std::fs::metadata(path)
            .ok()
            .filter(|metadata| metadata.is_dir())
            .map(|metadata| FileStamp::from_metadata(&metadata))
            == Some(*stamp)
    }) && missing_file_paths.iter().all(|path| !path.is_file())
        && missing_directory_paths.iter().all(|path| !path.is_dir());
    if written.is_ok() && inputs_unchanged {
        let _ = replace_file(&temporary, cache_path);
    }
    let _ = std::fs::remove_file(temporary);
    if cache_path.is_file() {
        maybe_gc_depcache(parent, cache_path);
    }
}

fn valid_depcache_file(path: &std::path::Path) -> bool {
    use std::io::{BufRead, Read};
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > DEPCACHE_RECORD_MAX_BYTES {
        return false;
    }
    let mut first = String::new();
    if std::io::BufReader::new(file.take(128))
        .read_line(&mut first)
        .is_err()
    {
        return false;
    }
    let first = first.trim_end();
    let Some(identity) = first
        .strip_prefix("TEX-DEPCACHE-7 ")
        .or_else(|| first.strip_prefix("TEX-DEPCACHE-6 "))
        .or_else(|| first.strip_prefix("TEX-DEPCACHE-5 "))
        .or_else(|| first.strip_prefix("TEX-DEPCACHE-4 "))
        .or_else(|| first.strip_prefix("TEX-DEPCACHE-3 "))
        .or_else(|| first.strip_prefix("TEX-DEPCACHE-2 "))
    else {
        return false;
    };
    identity.len() == 16 && identity.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn maybe_touch_depcache(path: &std::path::Path) {
    let recently_touched = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < DEPCACHE_TOUCH_INTERVAL);
    if recently_touched {
        return;
    }
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ =
            file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
    }
}

/// Report a validated dependency-cache hit to texmk without adding a private
/// command-line option that another TeX engine could reject. The marker must
/// be a direct child of the explicitly requested cache directory. `create_new`
/// prevents an inherited environment variable from replacing an existing file.
fn report_texmk_cache_hit(cache_root: &std::path::Path) {
    let Some(marker) = std::env::var_os(TEXMK_CACHE_HIT_MARKER_ENV)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
    else {
        return;
    };
    if !marker.is_absolute() {
        return;
    }
    let (Ok(cache_root), Some(parent)) = (std::fs::canonicalize(cache_root), marker.parent())
    else {
        return;
    };
    let Ok(parent) = std::fs::canonicalize(parent) else {
        return;
    };
    if parent != cache_root {
        return;
    }
    let _ = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker);
}

fn gc_depcache(directory: &std::path::Path, current: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut caches = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if path == current
            || !kind.is_file()
            || kind.is_symlink()
            || path.extension().and_then(std::ffi::OsStr::to_str) != Some("depcache")
            || !valid_depcache_file(&path)
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        if now
            .duration_since(modified)
            .is_ok_and(|age| age > DEPCACHE_MAX_AGE)
        {
            let _ = std::fs::remove_file(path);
        } else {
            caches.push((modified, metadata.len(), path));
        }
    }
    let mut total = caches
        .iter()
        .map(|(_, size, _)| *size)
        .sum::<u64>()
        .saturating_add(std::fs::metadata(current).map_or(0, |meta| meta.len()));
    caches.sort_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in caches {
        if total <= DEPCACHE_MAX_BYTES {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

fn maybe_gc_depcache(directory: &std::path::Path, current: &std::path::Path) {
    let stamp = directory.join(".gc-stamp");
    let recent = || {
        std::fs::metadata(&stamp)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age < DEPCACHE_GC_INTERVAL)
    };
    if recent() {
        return;
    }
    let lock = directory.join(".gc-lock");
    let acquire = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
    };
    let guard = match acquire() {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let stale = std::fs::metadata(&lock)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok())
                .is_some_and(|age| age > DEPCACHE_GC_INTERVAL);
            if !stale || std::fs::remove_file(&lock).is_err() {
                return;
            }
            let Ok(file) = acquire() else {
                return;
            };
            file
        }
        Err(_) => return,
    };
    if !recent() {
        gc_depcache(directory, current);
        let _ = atomic_write_file(&stamp, b"");
    }
    drop(guard);
    let _ = std::fs::remove_file(lock);
}

use tex_core::engine::{Engine, InteractionMode, DEFAULT_MAX_ERRORS};
use tex_core::prim::{DimParam, IntParam};

#[cfg(unix)]
fn apply_mem_limit() {
    let mib: u64 = match std::env::var("TEX_MEM_LIMIT_MIB") {
        Ok(s) if s.trim() == "0" => return,
        Ok(s) => s.trim().parse().unwrap_or(512),
        Err(_) => 512,
    };
    let rss = mib.saturating_mul(1 << 20);
    // 4x RSS, at least 2 GiB: mimalloc may reserve arenas. Still << host RAM.
    let as_bytes = rss.saturating_mul(4).max(2 << 30);
    let lim = libc::rlimit {
        rlim_cur: as_bytes as libc::rlim_t,
        rlim_max: as_bytes as libc::rlim_t,
    };
    unsafe {
        libc::setrlimit(libc::RLIMIT_AS, &lim);
    }
}
#[cfg(not(unix))]
fn apply_mem_limit() {}

/// Optional phase measurements, kept outside the token-processing hot path.
struct PhaseTimer(Option<std::time::Instant>);

impl PhaseTimer {
    fn new() -> Self {
        Self(std::env::var_os("PHASE_TIMING").map(|_| std::time::Instant::now()))
    }

    fn mark(&mut self, name: &str) {
        if let Some(last) = &mut self.0 {
            let now = std::time::Instant::now();
            eprintln!(
                "PHASE_TIMING {name} {:.3} ms",
                now.duration_since(*last).as_secs_f64() * 1000.0
            );
            *last = now;
        }
    }
}

fn program_name() -> String {
    if let Some(name) = std::env::var_os("TEX_SUITE_PROGRAM_NAME")
        .and_then(|value| value.into_string().ok())
        .filter(|value| matches!(value.as_str(), "pdflatex" | "xelatex" | "lualatex"))
    {
        return name;
    }
    std::env::args_os()
        .next()
        .and_then(|arg| {
            std::path::Path::new(&arg)
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "pdflatex".to_string())
}

fn usage(program: &str) {
    eprintln!(
        "usage: {program} [options] file.tex
  -ini                         build a format
  -plain                       run without the LaTeX format
  -output-directory DIR        write output files in DIR
  -aux-directory DIR           write auxiliary files and the transcript in DIR
  --cache-directory DIR        store the private dependency cache in DIR
  --optimize-pdf-size          spend more CPU minimizing converted PNG streams
  -jobname NAME                set the output job name
  -interaction MODE            errorstopmode, scrollmode, nonstopmode, or batchmode
  -halt-on-error               stop after the first TeX error
  --max-errors N               stop after N errors (default {DEFAULT_MAX_ERRORS})
  -file-line-error             accepted; rich file/line diagnostics are always enabled
  -no-shell-escape             accepted; shell execution is always disabled
  -h, --help                   show this help
  -v, --version                show version"
    );
}

fn usage_error(program: &str, message: &str) -> ! {
    eprintln!("{program}: {message}");
    eprintln!("Try '{program} --help' for usage.");
    std::process::exit(2);
}

fn parse_interaction(program: &str, value: &str) -> InteractionMode {
    match value {
        "batchmode" => InteractionMode::Batch,
        "nonstopmode" => InteractionMode::Nonstop,
        "scrollmode" => InteractionMode::Scroll,
        "errorstopmode" => InteractionMode::ErrorStop,
        _ => usage_error(
            program,
            &format!(
                "invalid interaction mode '{value}'; expected errorstopmode, scrollmode, nonstopmode, or batchmode"
            ),
        ),
    }
}

fn configure_engine(
    engine: &mut Engine,
    halt_on_error: bool,
    interaction_mode: InteractionMode,
    max_errors: usize,
) {
    engine.halt_on_error = halt_on_error;
    engine.set_interaction_mode(interaction_mode);
    engine.max_errors = max_errors;
}

fn emit_transcript(engine: &Engine) {
    if !engine.term.is_empty() {
        print!("{}", engine.term);
    }
    if !engine.diagnostic_output.is_empty() {
        eprint!("{}", engine.diagnostic_output);
    }
}

fn emit_cli_message(interaction_mode: InteractionMode, message: std::fmt::Arguments<'_>) {
    if interaction_mode != InteractionMode::Batch {
        eprintln!("{message}");
    }
}

#[derive(Debug, PartialEq, Eq)]
enum FormatBootFailure {
    Errors(i32),
    Incomplete { file: String, line: u32 },
}

fn format_boot_failure(engine: &Engine) -> Option<FormatBootFailure> {
    if engine.error_count > 0 {
        Some(FormatBootFailure::Errors(engine.error_count))
    } else if !engine.format_done {
        Some(FormatBootFailure::Incomplete {
            file: engine.input.current_file_name().to_string(),
            line: engine.input.current_file_line(),
        })
    } else {
        None
    }
}

/// pdftexconfig.tex assigns these legacy pdfTeX integer controls before
/// latex.ltx builds the format. The core does not otherwise use their values,
/// so stable count-register aliases provide the required assignable behavior.

fn write_early_transcript(engine: &Engine, out_dir: &str, job: &str) -> Result<String, String> {
    let path = format!("{out_dir}{job}.log");
    std::fs::write(&path, &engine.log)
        .map(|()| path.clone())
        .map_err(|error| format!("cannot write transcript {path}: {error}"))
}

fn fail_after_transcript(engine: &mut Engine, log_path: &str, message: &str, help: &str) -> ! {
    engine.external_fatal_error(message, Some(help));
    if engine.interaction_mode != InteractionMode::Batch {
        if let Some(diagnostic) = engine.diagnostics.last() {
            eprint!("{}", diagnostic.render());
        }
    }
    if let Err(error) = std::fs::write(log_path, &engine.log) {
        emit_cli_message(
            engine.interaction_mode,
            format_args!("pdflatex: cannot update transcript {log_path}: {error}"),
        );
    }
    std::process::exit(1)
}

fn install_panic_reporter() {
    std::panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("unknown internal failure");
        if message.starts_with("TeX capacity exceeded") {
            eprintln!("fatal: {message}");
            eprintln!(
                "  = help: check for recursive macros or runaway input before increasing an engine limit"
            );
        } else {
            eprintln!("fatal: internal TeX engine failure: {message}");
            if let Some(location) = info.location() {
                eprintln!(
                    "  --> {}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                );
            }
            eprintln!(
                "  = help: this is an engine bug; rerun with RUST_BACKTRACE=1 when reporting it"
            );
        }
        if backtrace_requested(std::env::var_os("RUST_BACKTRACE").as_deref()) {
            eprintln!("{}", std::backtrace::Backtrace::force_capture());
        }
    }));
}

fn backtrace_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.is_empty() && value != "0"
    })
}

pub(crate) fn main() {
    main_with_args(std::env::args_os().collect());
}

pub(crate) fn main_with_args(args_os: Vec<std::ffi::OsString>) {
    install_panic_reporter();
    let mut phase_timer = PhaseTimer::new();
    apply_mem_limit();
    let program = program_name();
    let args: Vec<String> = args_os
        .into_iter()
        .map(|arg| {
            arg.into_string().unwrap_or_else(|arg| {
                usage_error(
                    &program,
                    &format!("argument is not valid UTF-8: {}", arg.to_string_lossy()),
                )
            })
        })
        .collect();
    let mut file: Option<String> = None;
    let mut out_dir = String::new();
    let mut requested_aux_dir: Option<std::path::PathBuf> = None;
    let mut requested_cache_dir: Option<std::path::PathBuf> = None;
    let mut optimize_pdf_size = false;
    let mut jobname: Option<String> = None;
    let mut ini = false;
    let mut plain = false;
    let mut halt_on_error = false;
    let mut interaction_mode = InteractionMode::ErrorStop;
    let mut max_errors = DEFAULT_MAX_ERRORS;
    let mut synctex_enabled = true; // Enabled by default
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--" {
            i += 1;
            if file.is_some() || i >= args.len() || i + 1 != args.len() {
                usage_error(&program, "exactly one input file is required");
            }
            file = Some(args[i].clone());
        } else if matches!(args[i].as_str(), "-h" | "-help" | "--help") {
            usage(&program);
            return;
        } else if args[i] == "-output-directory" {
            i += 1;
            let Some(dir) = args.get(i).filter(|dir| !dir.is_empty()) else {
                usage_error(&program, "-output-directory requires a non-empty directory");
            };
            out_dir = format!("{}/", dir.trim_end_matches('/'));
        } else if let Some(dir) = args[i].strip_prefix("-output-directory=") {
            if dir.is_empty() {
                usage_error(&program, "-output-directory requires a non-empty directory");
            }
            out_dir = format!("{}/", dir.trim_end_matches('/'));
        } else if matches!(
            args[i].as_str(),
            "-aux-directory" | "-auxdir" | "--aux-directory"
        ) {
            i += 1;
            let Some(dir) = args.get(i).filter(|dir| !dir.is_empty()) else {
                usage_error(&program, "-aux-directory requires a non-empty directory");
            };
            requested_aux_dir = Some(std::path::PathBuf::from(dir));
        } else if let Some(dir) = args[i]
            .strip_prefix("-aux-directory=")
            .or_else(|| args[i].strip_prefix("-auxdir="))
            .or_else(|| args[i].strip_prefix("--aux-directory="))
        {
            if dir.is_empty() {
                usage_error(&program, "-aux-directory requires a non-empty directory");
            }
            requested_aux_dir = Some(std::path::PathBuf::from(dir));
        } else if matches!(args[i].as_str(), "-cache-directory" | "--cache-directory") {
            i += 1;
            let Some(dir) = args.get(i).filter(|dir| !dir.is_empty()) else {
                usage_error(&program, "--cache-directory requires a non-empty directory");
            };
            requested_cache_dir = Some(std::path::PathBuf::from(dir));
        } else if let Some(dir) = args[i]
            .strip_prefix("-cache-directory=")
            .or_else(|| args[i].strip_prefix("--cache-directory="))
        {
            if dir.is_empty() {
                usage_error(&program, "--cache-directory requires a non-empty directory");
            }
            requested_cache_dir = Some(std::path::PathBuf::from(dir));
        } else if args[i] == "-jobname" {
            i += 1;
            let Some(name) = args.get(i).filter(|name| !name.is_empty()) else {
                usage_error(&program, "-jobname requires a non-empty name");
            };
            if !valid_jobname(name) {
                usage_error(
                    &program,
                    "-jobname must be one filename component without '/' or '\\'",
                );
            }
            jobname = Some(name.clone());
        } else if let Some(jn) = args[i].strip_prefix("-jobname=") {
            if !valid_jobname(jn) {
                usage_error(
                    &program,
                    "-jobname must be one filename component without '/' or '\\'",
                );
            }
            jobname = Some(jn.to_string());
        } else if args[i] == "-ini" {
            ini = true;
        } else if args[i] == "-plain" {
            plain = true;
        } else if args[i] == "--optimize-pdf-size" || args[i] == "--optimize=size" {
            optimize_pdf_size = true;
        } else if args[i] == "--optimize=speed" {
            optimize_pdf_size = false;
        } else if args[i].starts_with("--optimize=") {
            usage_error(&program, "--optimize expects 'speed' or 'size'");
        } else if args[i] == "-halt-on-error" {
            halt_on_error = true;
        } else if args[i] == "-interaction" {
            i += 1;
            let Some(mode) = args.get(i) else {
                usage_error(&program, "-interaction requires a mode");
            };
            interaction_mode = parse_interaction(&program, mode);
        } else if let Some(mode) = args[i].strip_prefix("-interaction=") {
            interaction_mode = parse_interaction(&program, mode);
        } else if matches!(args[i].as_str(), "--max-errors" | "-max-errors") {
            i += 1;
            let Some(value) = args.get(i) else {
                usage_error(&program, "--max-errors requires a positive integer");
            };
            max_errors = value
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .unwrap_or_else(|| {
                    usage_error(&program, "--max-errors requires a positive integer")
                });
        } else if let Some(value) = args[i]
            .strip_prefix("--max-errors=")
            .or_else(|| args[i].strip_prefix("-max-errors="))
        {
            max_errors = value
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .unwrap_or_else(|| {
                    usage_error(&program, "--max-errors requires a positive integer")
                });
        } else if matches!(args[i].as_str(), "-synctex" | "--synctex") {
            i += 1;
            let Some(value) = args.get(i) else {
                usage_error(&program, "-synctex requires a value (1 or 0)");
            };
            synctex_enabled = value != "0" && value != "off" && value != "false";
        } else if let Some(val) = args[i]
            .strip_prefix("-synctex=")
            .or_else(|| args[i].strip_prefix("--synctex="))
        {
            synctex_enabled = val != "0" && val != "off" && val != "false";
        } else if matches!(
            args[i].as_str(),
            "-file-line-error" | "-file-line-error-style" | "-no-shell-escape"
        ) {
            // Rich file/line diagnostics are always enabled; shell execution
            // is never enabled.
        } else if matches!(args[i].as_str(), "-v" | "-version" | "--version") {
            let version = env!("CARGO_PKG_VERSION");
            if program == "xelatex" {
                println!("Ratex {version} (xelatex compatibility mode; pdfTeX-2h 1.40.29-rs)");
            } else if program == "lualatex" {
                println!("Ratex {version} (lualatex compatibility mode; pdfTeX-2h 1.40.29-rs)");
            } else {
                println!("pdfTeX-2h 1.40.29-rs (Ratex {version})");
            }
            return;
        } else if !args[i].starts_with('-') {
            if file.is_some() {
                usage_error(&program, "exactly one input file is required");
            }
            file = Some(args[i].clone());
        } else {
            usage_error(&program, &format!("unknown option '{}'", args[i]));
        }
        i += 1;
    }
    let Some(file) = file else {
        usage_error(&program, "no input file");
    };

    if !out_dir.is_empty() {
        if let Err(error) = std::fs::create_dir_all(&out_dir) {
            emit_cli_message(
                interaction_mode,
                format_args!("{program}: cannot create output directory {out_dir}: {error}"),
            );
            std::process::exit(1);
        }
    }
    let aux_dir = requested_aux_dir
        .as_ref()
        .map(|dir| format!("{}/", dir.to_string_lossy().trim_end_matches(['/', '\\'])))
        .unwrap_or_else(|| out_dir.clone());
    if !aux_dir.is_empty() {
        if let Err(error) = std::fs::create_dir_all(aux_dir.trim_end_matches(['/', '\\'])) {
            emit_cli_message(
                interaction_mode,
                format_args!("{program}: cannot create auxiliary directory {aux_dir}: {error}"),
            );
            std::process::exit(1);
        }
    }

    let job = jobname.unwrap_or_else(|| {
        std::path::Path::new(&file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "texput".to_string())
    });
    let aux_start = snapshot_aux_state(&job, &aux_dir);
    let cache_root = requested_cache_dir.unwrap_or_else(platform_cache_dir);
    let published_output = texmk_published_output(&cache_root);
    let private_cache = depcache_path(
        &cache_root,
        &file,
        &job,
        &out_dir,
        &aux_dir,
        optimize_pdf_size,
    );
    let expected_pdf = std::path::PathBuf::from(format!("{}{}.pdf", out_dir, job));
    let expected_log = std::path::PathBuf::from(format!("{}{}.log", aux_dir, job));
    let expected_synctex =
        synctex_enabled.then(|| std::path::PathBuf::from(format!("{}{}.synctex.gz", out_dir, job)));
    let outputs_missing_at_start: Vec<_> = [
        Some(expected_pdf.clone()),
        Some(expected_log.clone()),
        expected_synctex,
    ]
    .into_iter()
    .flatten()
    .filter(|path| !path.is_file())
    .collect();
    if !plain && !ini {
        if let Some(pdf_size) = check_depcache(
            &private_cache,
            &file,
            &expected_pdf,
            &expected_log,
            published_output.as_deref(),
        ) {
            report_texmk_cache_hit(&cache_root);
            maybe_touch_depcache(&private_cache);
            if let Some(directory) = private_cache.parent() {
                maybe_gc_depcache(directory, &private_cache);
            }
            let out = format!("{}{}.pdf", out_dir, job);
            if interaction_mode != InteractionMode::Batch {
                println!("\nOutput written on {} ({} bytes).", out, pdf_size);
            }
            std::process::exit(0);
        }
        // Establish our private cache directory before Kpathsea records any
        // lookup-time directory snapshots. If the cache lives beneath the
        // working directory, creating it during publication would itself
        // change that snapshot and unnecessarily defer caching by one pass.
        if let Some(directory) = private_cache.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
    }
    let mut eng = Engine::new(ini || !plain);
    eng.init_primitives();
    eng.allow_missing_main_aux = !plain && !ini;
    eng.synctex_enabled = synctex_enabled;
    configure_engine(&mut eng, halt_on_error, interaction_mode, max_errors);
    phase_timer.mark("startup");
    eng.out_dir = out_dir.clone();
    eng.aux_dir = requested_aux_dir.clone();
    if let Some(dir) = std::path::Path::new(&file).parent() {
        if !dir.as_os_str().is_empty() {
            eng.main_dir = Some(dir.to_path_buf());
        }
    }
    eng.job_name = job.clone();
    if !plain && !ini {
        let exe_fmt = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("pdflatex.fmt")));
        let cand_paths = [
            Some(std::path::PathBuf::from("pdflatex.fmt")),
            exe_fmt.clone(),
        ];
        let mut loaded = false;
        for cand in cand_paths.into_iter().flatten() {
            if cand.exists() {
                // load in place: the engine (and its kpse/ls-R setup) is reused
                match tex_core::format::load_format_into(&cand, &mut eng) {
                    Ok(()) => {
                        // Sanity: a dump taken from a broken boot (zeroed
                        // catcodes etc.) silently poisons every later run.
                        // Detect and fall through to a fresh boot.
                        if eng.eqtb.cat[b'd' as usize] != 11 || eng.eqtb.cat[b'@' as usize] == 0 {
                            emit_cli_message(
                                interaction_mode,
                                format_args!(
                                    "{program}: cannot use format {} because its catcode table is invalid; trying another format source",
                                    cand.display()
                                ),
                            );
                            eng = Engine::new(ini || !plain);
                            eng.init_primitives();
                            configure_engine(&mut eng, halt_on_error, interaction_mode, max_errors);
                            eng.out_dir = out_dir.clone();
                            eng.aux_dir = requested_aux_dir.clone();
                            if let Some(dir) = std::path::Path::new(&file).parent() {
                                if !dir.as_os_str().is_empty() {
                                    eng.main_dir = Some(dir.to_path_buf());
                                }
                            }
                            eng.job_name = job.clone();
                            break;
                        }
                        loaded = true;
                        eng.loaded_files.push(cand.clone());
                        finalize_format_load(&mut eng);
                        break;
                    }
                    Err(error) => emit_cli_message(
                        interaction_mode,
                        format_args!(
                            "{program}: cannot use format {} ({error}); trying another format source",
                            cand.display()
                        ),
                    ),
                }
            }
        }
        if !loaded && !EMBEDDED_DEFAULT_FMT.is_empty() {
            match tex_core::format::load_format_bytes_into(EMBEDDED_DEFAULT_FMT, &mut eng) {
                Ok(()) => {
                    if eng.eqtb.cat[b'd' as usize] == 11 && eng.eqtb.cat[b'@' as usize] != 0 {
                        loaded = true;
                        finalize_format_load(&mut eng);
                    } else {
                        emit_cli_message(
                            interaction_mode,
                            format_args!(
                                "{program}: the embedded LaTeX format has an invalid catcode table; booting from format sources"
                            ),
                        );
                    }
                }
                Err(error) => emit_cli_message(
                    interaction_mode,
                    format_args!(
                        "{program}: cannot load the embedded LaTeX format ({error}); booting from format sources"
                    ),
                ),
            }
        }
        if !loaded {
            // Match fmtutil's pdfLaTeX bootstrap: pdflatex.ini applies
            // pdftexconfig.tex (paper size and driver settings) before
            // latex.ltx builds and dumps the format.
            install_pdftex_config_registers(&mut eng);
            let hyphen_path = eng.resolve_input_path("hyphen.tex").unwrap_or_else(|| {
                std::path::PathBuf::from("/usr/share/texmf-dist/tex/generic/hyphen/hyphen.tex")
            });
            let _ = eng.hyphen_trie.load_hyphen_file(&hyphen_path);
            eng.add_nullfont();
            eng.input_file("pdflatex.ini");
            eng.run();
            eng.finish_job_diagnostics();
            if let Some(failure) = format_boot_failure(&eng) {
                emit_transcript(&eng);
                let log_result = write_early_transcript(&eng, &aux_dir, &job);
                let transcript_note = log_result
                    .as_ref()
                    .map(|path| format!("; transcript written to {path}"))
                    .unwrap_or_default();
                match failure {
                    FormatBootFailure::Errors(count) => emit_cli_message(
                        eng.interaction_mode,
                        format_args!(
                            "{program}: cannot compile {file}: LaTeX format boot reported {count} error{}; fix the diagnostic above or install a valid pdflatex.fmt{transcript_note}",
                            if count == 1 { "" } else { "s" },
                        ),
                    ),
                    FormatBootFailure::Incomplete { file, line } => emit_cli_message(
                        eng.interaction_mode,
                        format_args!(
                            "{program}: cannot compile the document: LaTeX format boot ended before \\dump at {file}:{line}; install a valid pdflatex.fmt or fix the format sources{transcript_note}"
                        ),
                    ),
                }
                if let Err(error) = log_result {
                    emit_cli_message(eng.interaction_mode, format_args!("{program}: {error}"));
                }
                std::process::exit(1);
            }
            let dump_target = exe_fmt.unwrap_or_else(|| std::path::PathBuf::from("pdflatex.fmt"));
            if let Err(error) = tex_core::format::save_format_compressed(&eng, &dump_target) {
                emit_cli_message(
                    eng.interaction_mode,
                    format_args!(
                        "{program}: could not cache the freshly built LaTeX format at {} ({error}); continuing with the in-memory format",
                        dump_target.display()
                    ),
                );
            }
        }
        tex_core::driver::prepare_latex_job(&mut eng);
        configure_engine(&mut eng, halt_on_error, interaction_mode, max_errors);
    } else {
        let _ = eng.hyphen_trie.load_hyphen_file(std::path::Path::new(
            "/usr/share/texmf-dist/tex/generic/hyphen/hyphen.tex",
        ));
        eng.eqtb.dim_params[DimParam::HSize.idx() as usize] = (6.25 * 72.27 * 65536.0) as i32;
        eng.eqtb.dim_params[DimParam::VSize.idx() as usize] = 0;
        eng.eqtb.dim_params[DimParam::MaxDepth.idx() as usize] = (4.0 * 65536.0) as i32;
        eng.eqtb.dim_params[DimParam::ParIndent.idx() as usize] = (1.5 * 65536.0 * 10.0) as i32;
        eng.eqtb.int_params[IntParam::EndLineChar.idx() as usize] = 13;
        eng.eqtb.int_params[IntParam::EscapeChar.idx() as usize] = 92;
        eng.eqtb.int_params[IntParam::NewLineChar.idx() as usize] = 10;
        eng.eqtb.int_params[IntParam::MaxDeadCycles.idx() as usize] = 25;
        eng.eqtb.int_params[tex_core::prim::IntParam::EtxVersion.idx() as usize] = 2;
        eng.eqtb.int_params[IntParam::LeftHyphenMin.idx() as usize] = 2;
        eng.eqtb.int_params[IntParam::RightHyphenMin.idx() as usize] = 3;
        eng.eqtb.int_params[IntParam::Tolerance.idx() as usize] = 200;
        eng.eqtb.int_params[IntParam::Pretolerance.idx() as usize] = 100;
        eng.eqtb.int_params[IntParam::LinePenalty.idx() as usize] = 10;
        eng.eqtb.dim_params[DimParam::Hfuzz.idx() as usize] = 6554;
        eng.eqtb.dim_params[DimParam::Vfuzz.idx() as usize] = 6554;
        eng.eqtb.dim_params[DimParam::OverfullRule.idx() as usize] = 327_680;
        eng.eqtb.dim_params[DimParam::BoxMaxDepth.idx() as usize] = 262_144;
        eng.eqtb.int_params[IntParam::HBadness.idx() as usize] = 1000;
        eng.eqtb.int_params[IntParam::VBadness.idx() as usize] = 1000;
        eng.eqtb.int_params[IntParam::HyphenPenalty.idx() as usize] = 50;
        eng.eqtb.int_params[IntParam::ExHyphenPenalty.idx() as usize] = 50;
        eng.eqtb.int_params[IntParam::ClubPenalty.idx() as usize] = 150;
        eng.eqtb.int_params[IntParam::WidowPenalty.idx() as usize] = 150;
        eng.add_nullfont();
        install_pdftex_config_registers(&mut eng);
    }
    let engine_banner = match program.as_str() {
        "xelatex" => format!(
            "This is pdfTeX-2h 1.40.29-rs (Ratex {})\nRatex note: xelatex / -xelatex is a compatibility invocation flag, not the XeTeX runtime.\n",
            env!("CARGO_PKG_VERSION")
        ),
        "lualatex" => format!(
            "This is pdfTeX-2h 1.40.29-rs (Ratex {})\nRatex note: lualatex / -lualatex is a compatibility invocation flag, not the LuaHBTeX runtime.\n",
            env!("CARGO_PKG_VERSION")
        ),
        _ => format!(
            "This is pdfTeX-2h 1.40.29-rs (Ratex {})\n",
            env!("CARGO_PKG_VERSION")
        ),
    };
    eng.log.push_str(&engine_banner);
    if interaction_mode != InteractionMode::Batch {
        eng.term.push_str(&engine_banner);
    }
    phase_timer.mark("format");
    if eng.input_file(&file) {
        // Knuth: everyjob is inserted on top of the * file so it runs first.
        if !plain && !ini {
            tex_core::driver::insert_everyjob(&mut eng);
        }
        eng.run();
        eng.finish_job_diagnostics();
    }
    phase_timer.mark("typeset");
    // -ini mode: the file ended in \dump — write the format and exit,
    // like initex does.
    if ini && eng.format_done {
        if eng.error_count > 0 {
            emit_transcript(&eng);
            let log_result = write_early_transcript(&eng, &aux_dir, &job);
            let current_mode = eng.interaction_mode;
            let transcript_note = log_result
                .as_ref()
                .map(|path| format!("; transcript written to {path}"))
                .unwrap_or_default();
            emit_cli_message(
                current_mode,
                format_args!(
                    "{program}: format build reported {} error{}; pdflatex.fmt was not written{transcript_note}",
                    eng.error_count,
                    if eng.error_count == 1 { "" } else { "s" }
                ),
            );
            if let Err(error) = log_result {
                emit_cli_message(current_mode, format_args!("{program}: {error}"));
            }
            std::process::exit(1);
        }
        match tex_core::format::save_format_compressed(&eng, std::path::Path::new("pdflatex.fmt")) {
            Ok(n) => emit_cli_message(
                eng.interaction_mode,
                format_args!("Format written to pdflatex.fmt ({n} bytes)"),
            ),
            Err(e) => {
                eng.external_fatal_error(
                    &format!("Cannot write format `pdflatex.fmt`: {e}"),
                    Some("check that the working directory is writable and that `pdflatex.fmt` is not a directory"),
                );
                emit_transcript(&eng);
                if let Err(error) = write_early_transcript(&eng, &aux_dir, &job) {
                    emit_cli_message(eng.interaction_mode, format_args!("{program}: {error}"));
                }
                std::process::exit(1);
            }
        }
        emit_transcript(&eng);
        std::process::exit(if eng.error_count > 0 { 1 } else { 0 });
    }
    emit_transcript(&eng);
    let log_path = format!("{}{}.log", aux_dir, job);
    if let Err(error) = std::fs::write(&log_path, &eng.log) {
        eng.external_fatal_error(
            &format!("Cannot write transcript `{log_path}`: {error}"),
            Some("check that the auxiliary or transcript directory exists and is writable"),
        );
        if eng.interaction_mode != InteractionMode::Batch {
            if let Some(diagnostic) = eng.diagnostics.last() {
                eprint!("{}", diagnostic.render());
            }
        }
        std::process::exit(1);
    }
    let compilation_had_errors = eng.error_count > 0;
    if compilation_had_errors {
        if eng.interaction_mode != InteractionMode::Batch {
            let outcome = if eng.stopped_on_error {
                "compilation failed after"
            } else {
                "compilation completed with"
            };
            eprintln!(
                "{program}: {outcome} {} error{}; transcript written to {log_path}",
                eng.error_count,
                if eng.error_count == 1 { "" } else { "s" }
            );
        }
        if eng.stopped_on_error {
            std::process::exit(1);
        }
    }
    if !eng.pdf_doc.pages.is_empty() {
        let pdf = match tex_core::driver::finish_pdf(&mut eng, optimize_pdf_size) {
            Ok(pdf) => pdf,
            Err(error) => {
                let help = if error.starts_with("Unsupported or invalid image:") {
                    "verify that each image is a supported, valid JPEG or PNG file"
                } else {
                    "check the document fonts and images"
                };
                fail_after_transcript(&mut eng, &log_path, &error, help)
            }
        };
        phase_timer.mark("pdf_serialize");
        let out = format!("{}{}.pdf", eng.out_dir, job);
        if !eng.out_dir.is_empty() {
            let _ = std::fs::create_dir_all(eng.out_dir.trim_end_matches('/'));
        }
        if let Err(error) = atomic_write_file(std::path::Path::new(&out), &pdf) {
            fail_after_transcript(
                &mut eng,
                &log_path,
                &format!("Cannot write PDF `{out}`: {error}"),
                "check that the output directory exists, has free space, and is writable",
            );
        }
        if eng.synctex_enabled && !eng.synctex.pages.is_empty() {
            let synctex_out = format!("{}{}.synctex.gz", eng.out_dir, job);
            if let Ok(gz_bytes) = eng.synctex.to_synctex_gz() {
                let _ = atomic_write_file(std::path::Path::new(&synctex_out), &gz_bytes);
            }
        }
        phase_timer.mark("pdf_write");
        let pdf_len = std::fs::metadata(&out)
            .map(|m| m.len() as usize)
            .unwrap_or(pdf.len());
        if eng.error_count == 0
            && !plain
            && !ini
            && eng.font_loader.dependency_tracking_complete
            && aux_state_is_unchanged(&aux_start)
        {
            eng.loaded_files
                .extend(eng.font_loader.dependency_files.iter().cloned());
            eng.loaded_file_digests
                .extend(eng.font_loader.dependency_file_digests.iter().cloned());
            eng.missing_files
                .extend(eng.font_loader.dependency_missing_files.iter().cloned());
            write_depcache(
                &private_cache,
                DepcacheInputs {
                    primary_file: &file,
                    pdf_path: &out,
                    pdf_size: pdf_len,
                    deps: &eng.loaded_files,
                    directories: &eng.font_loader.dependency_directories,
                    reads: &eng.loaded_file_digests,
                    sizes: &eng.loaded_file_sizes,
                    missing: &eng.missing_files,
                    missing_directories: &eng.font_loader.dependency_missing_directories,
                    outputs_missing_at_start: &outputs_missing_at_start,
                    published_output: published_output.as_deref(),
                    aux_start: &aux_start,
                },
            );
        }
        if eng.interaction_mode != InteractionMode::Batch {
            if compilation_had_errors {
                println!(
                    "\nOutput written on {} ({} bytes; {} error{}).",
                    out,
                    pdf_len,
                    eng.error_count,
                    if eng.error_count == 1 { "" } else { "s" }
                );
            } else {
                println!("\nOutput written on {} ({} bytes).", out, pdf_len);
            }
        }
        phase_timer.mark("finish");
        std::process::exit(if eng.error_count > 0 { 1 } else { 0 });
    } else {
        if eng.interaction_mode != InteractionMode::Batch {
            eprintln!("No pages of output.");
        }
        std::process::exit(if eng.error_count > 0 { 1 } else { 0 });
    }
}

#[cfg(test)]
mod startup_tests {
    use super::{
        authenticated_depcache_body, backtrace_requested, check_depcache, decode_record_path,
        dependency_fingerprint, dependency_name_may_match, effective_clock_identity_at,
        encode_record_path, finalize_format_load, format_boot_failure,
        install_pdftex_config_registers, png_embed_options, published_name_in_directory,
        seal_depcache_record, write_depcache, DepcacheInputs, FormatBootFailure,
        DEPCACHE_RECORD_MAX_BYTES, EMBEDDED_DEFAULT_FMT,
    };
    use std::ffi::OsStr;
    use tex_core::engine::Engine;

    #[test]
    fn record_path_encoding_round_trips_delimiters() {
        let path = std::path::Path::new("directory%name/line\n-tab\t-file.tex");
        let encoded = encode_record_path(path);
        assert!(!encoded.contains(['\n', '\r', '\t']));
        assert_eq!(decode_record_path(&encoded).as_deref(), Some(path));
    }

    #[test]
    fn published_output_dependency_guard_covers_unicode_case_variants() {
        let directory = std::env::current_dir().unwrap();
        assert!(dependency_name_may_match(
            &directory.join("ämain.pdf"),
            &directory,
            OsStr::new("ÄMAIN.PDF")
        ));
        assert!(!dependency_name_may_match(
            &directory.join("other.pdf"),
            &directory,
            OsStr::new("ÄMAIN.PDF")
        ));
    }

    #[test]
    fn published_output_directory_exclusion_is_limited_to_cwd() {
        let directory = std::env::current_dir().unwrap();
        let local_output = directory.join("main.pdf");
        assert_eq!(
            published_name_in_directory(&directory, Some(&local_output)),
            Some(OsStr::new("main.pdf"))
        );

        let recursive_walk_directory = directory.join("texmf/tex/latex/generated");
        let remote_output = recursive_walk_directory.join("main.pdf");
        assert_eq!(
            published_name_in_directory(&recursive_walk_directory, Some(&remote_output)),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn published_output_dependency_guard_rejects_non_utf8_ambiguity() {
        use std::os::unix::ffi::OsStringExt;

        let directory = std::env::current_dir().unwrap();
        let ambiguous = std::ffi::OsString::from_vec(vec![b'm', 0xff, b'.', b'p', b'd', b'f']);
        assert!(dependency_name_may_match(
            &directory.join(ambiguous),
            &directory,
            OsStr::new("main.pdf")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn record_path_encoding_round_trips_non_utf8() {
        use std::os::unix::ffi::OsStringExt;

        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![
            b'n', b'o', b'n', b'-', 0xff, b'\n', b'\t', b'%',
        ]));
        let encoded = encode_record_path(&path);
        assert!(!encoded.contains(['\n', '\r', '\t']));
        assert_eq!(decode_record_path(&encoded), Some(path));
    }

    #[test]
    fn authenticated_record_requires_untampered_final_marker() {
        let mut record = "TEX-DEPCACHE-7 identity\nKEY\t0123456789abcdef\n".to_owned();
        assert!(seal_depcache_record(&mut record));
        assert!(authenticated_depcache_body(&record).is_some());

        let marker = record.rfind("END\t").unwrap();
        assert!(authenticated_depcache_body(&record[..marker]).is_none());
        let mut modified = record.into_bytes();
        modified[0] ^= 1;
        assert!(authenticated_depcache_body(std::str::from_utf8(&modified).unwrap()).is_none());
    }

    #[test]
    fn completed_format_boot_with_recoverable_errors_is_rejected() {
        let mut engine = Engine::new(true);
        engine.format_done = true;
        engine.error_count = 2;

        assert_eq!(
            format_boot_failure(&engine),
            Some(FormatBootFailure::Errors(2))
        );

        engine.error_count = 0;
        assert_eq!(format_boot_failure(&engine), None);
    }

    fn backtrace_zero_and_empty_disable_panic_backtraces() {
        assert!(!backtrace_requested(None));
        assert!(!backtrace_requested(Some(OsStr::new(""))));
        assert!(!backtrace_requested(Some(OsStr::new("0"))));
        assert!(backtrace_requested(Some(OsStr::new("1"))));
        assert!(backtrace_requested(Some(OsStr::new("full"))));
    }

    #[test]
    fn speed_png_compression_caps_at_the_measured_level() {
        use tex_core::pdf_images::{PngEmbedOptions, PngOptimization};

        assert_eq!(png_embed_options(false, 9), PngEmbedOptions::speed(3));
        assert_eq!(png_embed_options(false, 2), PngEmbedOptions::speed(2));
        assert_eq!(png_embed_options(true, 9), PngEmbedOptions::size(9));
        assert_eq!(
            png_embed_options(true, 12).optimization,
            PngOptimization::Size
        );
        assert_eq!(png_embed_options(true, 12).compression_level, 9);
    }

    #[test]
    fn live_clock_cache_identity_changes_each_minute_but_source_epoch_is_stable() {
        assert_eq!(effective_clock_identity_at(None, 119), "live-minute=1");
        assert_eq!(effective_clock_identity_at(None, 120), "live-minute=2");
        let epoch = OsStr::new("1700000000");
        assert_eq!(
            effective_clock_identity_at(Some(epoch), 119),
            effective_clock_identity_at(Some(epoch), 9_999_999)
        );
        assert_ne!(
            effective_clock_identity_at(Some(OsStr::new("invalid")), 119),
            effective_clock_identity_at(Some(OsStr::new("invalid")), 120)
        );
    }

    #[test]
    fn large_dependency_fingerprint_covers_bytes_between_old_sample_windows() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tex-dependency-fingerprint-{}-{nonce}.bin",
            std::process::id()
        ));
        let mut bytes = vec![b'A'; 2 * 1024 * 1024];
        std::fs::write(&path, &bytes).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let modified = metadata.modified().unwrap();
        let before = dependency_fingerprint(&path, metadata.len()).unwrap();

        bytes[256 * 1024] = b'B';
        std::fs::write(&path, &bytes).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let after = dependency_fingerprint(&path, metadata.len()).unwrap();
        let _ = std::fs::remove_file(path);

        assert_ne!(before, after);
    }

    #[test]
    fn oversized_depcache_record_is_rejected_before_reading() {
        use std::io::Write;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "0123456789abcdef-{}-{nonce}.depcache",
            std::process::id()
        ));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"TEX-DEPCACHE-7 0123456789abcdef\n")
            .unwrap();
        file.set_len(DEPCACHE_RECORD_MAX_BYTES + 1).unwrap();
        drop(file);

        assert!(super::read_depcache_record(&path).is_none());
        assert!(!super::valid_depcache_file(&path));
        std::fs::write(&path, b"TEX-DEPCACHE-7 0123456789abcdef\n").unwrap();
        assert!(super::valid_depcache_file(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn size_only_dependencies_require_the_observed_size_at_publish_and_reuse() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tex-size-dependency-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let primary = root.join("main.tex");
        let pdf = root.join("main.pdf");
        let log = root.join("main.log");
        let observed = root.join("observed.bin");
        let cache = root.join("cache/0123456789abcdef.depcache");
        std::fs::write(&primary, b"source").unwrap();
        std::fs::write(&pdf, b"pdf").unwrap();
        std::fs::write(&log, b"log").unwrap();
        std::fs::write(&observed, b"four").unwrap();
        let primary_text = primary.to_string_lossy();
        let pdf_text = pdf.to_string_lossy();

        let stale_observation = [(observed.clone(), 3)];
        write_depcache(
            &cache,
            DepcacheInputs {
                primary_file: &primary_text,
                pdf_path: &pdf_text,
                pdf_size: 3,
                deps: &[],
                directories: &[],
                reads: &[],
                sizes: &stale_observation,
                missing: &[],
                missing_directories: &[],
                outputs_missing_at_start: &[],
                published_output: None,
                aux_start: &[],
            },
        );
        assert!(!cache.exists(), "a stale observed size was published");

        let stable_observation = [(observed.clone(), 4)];
        write_depcache(
            &cache,
            DepcacheInputs {
                primary_file: &primary_text,
                pdf_path: &pdf_text,
                pdf_size: 3,
                deps: &[],
                directories: &[],
                reads: &[],
                sizes: &stable_observation,
                missing: &[],
                missing_directories: &[],
                outputs_missing_at_start: &[],
                published_output: None,
                aux_start: &[],
            },
        );
        assert!(
            std::fs::read_to_string(&cache)
                .unwrap()
                .lines()
                .any(|line| line.starts_with("SIZE\t")),
            "the size-only dependency was not recorded"
        );
        assert_eq!(
            check_depcache(&cache, &primary_text, &pdf, &log, None),
            Some(3)
        );

        std::fs::write(&observed, b"changed size").unwrap();
        assert_eq!(
            check_depcache(&cache, &primary_text, &pdf, &log, None),
            None
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
