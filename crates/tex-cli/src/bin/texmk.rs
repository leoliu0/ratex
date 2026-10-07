//! texmk — latexmk-style driver for the Rust TeX engine.
//!
//! The only physical executable in a distribution (`texres`). It runs its
//! embedded TeX engine and BibTeX implementation in isolated child processes,
//! and Biber in-process, until references and bibliography output stabilize.
//! Invoked as `pdflatex`, `xelatex`, `lualatex`, `bibtex`, `biber`, or `latexdiff`, it
//! runs that tool directly instead.
//!
//! Exit codes: 0 = converged, 1 = engine/bibliography failure, TeX errors (even
//! when a nonstop-mode PDF was published), or no convergence, 2 = usage error.
//!
//! TeX support files always come from the archive embedded in the executable;
//! project inputs remain ordinary files.

#[path = "../bibtex/mod.rs"]
mod embedded_bibtex;
#[path = "../index/mod.rs"]
mod embedded_index;
#[path = "pdflatex.rs"]
mod embedded_engine;
#[path = "latexdiff.rs"]
mod latexdiff;
#[path = "../watch/mod.rs"]
mod watch;

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::hash::Hasher;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

const MAX_PASSES: u32 = 5;
const CACHE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const CACHE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const CACHE_GC_INTERVAL: Duration = Duration::from_secs(60 * 60);
const TEXMK_CACHE_HIT_MARKER_ENV: &str = "TEX_RS_CACHE_HIT_MARKER";
const TEXMK_PUBLISHED_OUTPUT_ENV: &str = "TEX_RS_TEXMK_PUBLISHED_OUTPUT";
const TEXMK_WATCH_DEPENDENCIES_ENV: &str = "TEX_RS_TEXMK_WATCH_DEPENDENCIES";
const TEXMK_FONT_CACHE_ENV: &str = "TEX_RS_TEXMK_FONT_CACHE";
const TEXMK_INTERNAL_MODE_ENV: &str = "TEXMK_INTERNAL_MODE";
const HERMETIC_ENV: &str = "TEX_RS_HERMETIC";
const AUX_GRAPH_MAX_DEPTH: usize = 32;
const AUX_GRAPH_MAX_FILES: usize = 256;
const AUX_GRAPH_MAX_INCLUDES: usize = 1024;
const AUX_GRAPH_MAX_BYTES: u64 = 8 * 1024 * 1024;
const AUX_GRAPH_MAX_PATH_BYTES: usize = 4096;
const MANIFEST_MAX_BYTES: u64 = 4 * 1024 * 1024;
// Tool output must always be drained to avoid blocking a child, but retaining
// an untrusted transcript without a limit lets a noisy tool exhaust memory.
// Keep the diagnostic-heavy beginning and the final summary from each stream.
const TOOL_OUTPUT_MAX_BYTES_PER_STREAM: usize = 1024 * 1024;
const TOOL_OUTPUT_HEAD_BYTES: usize = 128 * 1024;
const IO_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CleanMode {
    None,
    Aux,
    All,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Signals {
    rerun: bool,
    undef_refs: bool,
    undef_cites: bool,
    bbl_missing: bool,
    biber_requested: bool,
    user_warnings: Vec<String>,
}

struct Options {
    file: PathBuf,
    out_dir: Option<PathBuf>,
    aux_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    jobname: Option<String>,
    silent: bool,
    keep_intermediates: bool,
    keep_logs: bool,
    clean: CleanMode,
    /// Explicit engine override ("pdflatex", "xelatex", "lualatex"), or auto-detect.
    engine: Option<String>,
    /// Extra flags forwarded verbatim to engine (-interaction=..., -halt-on-error, ...).
    passthrough: Vec<String>,
    /// `-pvc`/`--watch`/`-w`: rebuild whenever an input of the last build changes.
    watch: bool,
}

/// Packages and classes which TeX Live compiles only with a Unicode engine
/// (XeTeX or LuaTeX); pdfLaTeX rejects them or lacks their fonts.
const UNICODE_ENGINE_PACKAGES: &[&str] = &["ctex", "xeCJK", "fontspec", "unicode-math", "polyglossia"];
const UNICODE_ENGINE_CLASSES: &[&str] = &["ctexart", "ctexbook", "ctexrep", "ctexbeamer"];

/// Without an explicit engine option, documents whose preamble requires a
/// Unicode engine run as xelatex; everything else runs as pdflatex.
fn detect_engine(src_path: &Path) -> &'static str {
    match std::fs::read(src_path) {
        Ok(source) if preamble_requires_unicode_engine(&String::from_utf8_lossy(&source)) => {
            "xelatex"
        }
        _ => "pdflatex",
    }
}

/// Strip a `%` comment, honoring `\%` (an odd run of backslashes escapes it).
fn strip_tex_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut backslashes = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'\\' => backslashes += 1,
            b'%' if backslashes % 2 == 0 => return &line[..index],
            _ => backslashes = 0,
        }
    }
    line
}

fn preamble_requires_unicode_engine(source: &str) -> bool {
    let mut preamble = String::new();
    for line in source.lines() {
        let line = strip_tex_comment(line);
        if let Some(end) = line.find("\\begin{document}") {
            preamble.push_str(&line[..end]);
            break;
        }
        preamble.push_str(line);
        preamble.push('\n');
    }
    for (command, names) in [
        ("\\documentclass", UNICODE_ENGINE_CLASSES),
        ("\\usepackage", UNICODE_ENGINE_PACKAGES),
        ("\\RequirePackage", UNICODE_ENGINE_PACKAGES),
    ] {
        let mut rest = preamble.as_str();
        while let Some(start) = rest.find(command) {
            rest = &rest[start + command.len()..];
            if rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '@') {
                continue;
            }
            let mut argument = rest.trim_start();
            if let Some(options) = argument.strip_prefix('[') {
                let Some(close) = options.find(']') else {
                    break;
                };
                argument = options[close + 1..].trim_start();
            }
            let Some(list) = argument.strip_prefix('{') else {
                continue;
            };
            let Some(close) = list.find('}') else {
                break;
            };
            if list[..close]
                .split(',')
                .any(|name| names.contains(&name.trim()))
            {
                return true;
            }
        }
    }
    false
}

/// SyncTeX file extension selected by the engine options, as in pdfTeX:
/// `-synctex=0` disables it and a negative value writes an uncompressed file.
/// TeXres writes compressed SyncTeX by default.
fn synctex_extension(passthrough: &[String]) -> Option<&'static str> {
    let value = passthrough
        .iter()
        .rev()
        .find_map(|argument| argument.strip_prefix("-synctex="));
    match value.map(str::trim) {
        None => Some(".synctex.gz"),
        Some("off" | "false") => None,
        Some(value) => match value.parse::<i64>() {
            Ok(0) => None,
            Ok(number) if number < 0 => Some(".synctex"),
            _ => Some(".synctex.gz"),
        },
    }
}

fn usage() {
    eprintln!(
        "usage: texmk [options] file.tex
  -pdf / -xelatex / -lualatex   engine (default: auto from preamble)
  -output-directory DIR         write the final PDF in DIR
  -aux-directory DIR            use DIR for persistent build state
  --cache-directory DIR         use DIR as the private cache root
  -jobname NAME                 job name (default: file stem)
  --keep-intermediates, -k       export auxiliary files beside the PDF
  --keep-logs                    export the transcript beside the PDF
  --optimize-pdf-size            spend more CPU minimizing converted PNG streams
  -c                             remove cached state; preserve the PDF
  -C                             remove cached state and an owned PDF
  -pvc, --watch, -w              build, then rebuild whenever an input changes
                                 (Ctrl-C stops; cannot be combined with -c/-C)
  -interaction=MODE             passed to the engine (default nonstopmode)
  -halt-on-error                passed to the engine
  --silent, -q                  suppress engine output (default)
  --verbose, -V                 print detailed engine and tool output
  -h, --help                    this text
  -v, --version                 version"
    );
}

fn parse_args(argv: &[String]) -> Result<Options, String> {
    let mut file: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut aux_dir: Option<PathBuf> = None;
    let mut cache_dir: Option<PathBuf> = None;
    let mut jobname: Option<String> = None;
    let mut silent = true;
    let mut keep_intermediates = false;
    let mut keep_logs = false;
    let mut clean = CleanMode::None;
    let mut watch = false;
    let mut engine: Option<String> = None;
    let mut passthrough: Vec<String> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let a = argv[i].clone();
        let (key, inline_val) = match a.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (a.clone(), None),
        };
        let take_value = |i: &mut usize, inline_val: &Option<String>| -> Option<String> {
            match inline_val {
                Some(v) => Some(v.clone()),
                None => {
                    *i += 1;
                    argv.get(*i).cloned()
                }
            }
        };
        match key.as_str() {
            "-output-directory" | "-outdir" | "--output-directory" => {
                let value = take_value(&mut i, &inline_val)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "-output-directory needs a non-empty value".to_string())?;
                out_dir = Some(PathBuf::from(value));
            }
            "-aux-directory" | "-auxdir" | "--aux-directory" => {
                let value = take_value(&mut i, &inline_val)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "-aux-directory needs a non-empty value".to_string())?;
                aux_dir = Some(PathBuf::from(value));
            }
            "-cache-directory" | "--cache-directory" => {
                let value = take_value(&mut i, &inline_val)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "--cache-directory needs a non-empty value".to_string())?;
                cache_dir = Some(PathBuf::from(value));
            }
            "-jobname" | "--jobname" => {
                let value = take_value(&mut i, &inline_val)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "-jobname needs a non-empty value".to_string())?;
                validate_job_name(&value)?;
                jobname = Some(value);
            }
            "--silent" | "-silent" | "-quiet" | "-q" => silent = true,
            "--verbose" | "-verbose" | "--noisy" | "-noisy" | "-V" => silent = false,
            "--keep-intermediates" | "-keep-intermediates" | "-k" => keep_intermediates = true,
            "--keep-logs" | "-keep-logs" => keep_logs = true,
            "-c" => clean = CleanMode::Aux,
            "-C" => clean = CleanMode::All,
            "-pvc" | "--pvc" | "-watch" | "--watch" | "-w" => watch = true,
            "-interaction" | "--interaction" => {
                let mode = take_value(&mut i, &inline_val)
                    .ok_or_else(|| "-interaction needs a value".to_string())?;
                passthrough.push(format!("-interaction={mode}"));
            }
            "-halt-on-error" | "--halt-on-error" => passthrough.push("-halt-on-error".to_string()),
            "-synctex" | "--synctex" => {
                let value = take_value(&mut i, &inline_val)
                    .ok_or_else(|| "-synctex needs a value".to_string())?;
                passthrough.push(format!("-synctex={value}"));
            }
            "-pdf" | "--pdf" => engine = Some("pdflatex".to_string()),
            "-pdfxe" | "-xelatex" | "--xelatex" => engine = Some("xelatex".to_string()),
            "-pdflua" | "-lualatex" | "--lualatex" => engine = Some("lualatex".to_string()),
            "-h" | "-help" | "--help" => {
                usage();
                std::process::exit(0);
            }
            "-v" | "-version" | "--version" => {
                let version = env!("CARGO_PKG_VERSION");
                let invoked = invoked_name();
                match invoked.as_str() {
                    "texres" => println!("texres {version} (Rust TeX engine)"),
                    "texmk" => println!("texmk {version} (TeXres; Rust TeX engine)"),
                    "latexmk" => println!("latexmk (TeXres {version}; Rust TeX engine)"),
                    _ => println!("texres {version} (Rust TeX engine)"),
                }
                std::process::exit(0);
            }
            other if other.starts_with('-') => passthrough.push(a.clone()),
            _ => {
                if file.is_some() {
                    return Err(format!("unexpected argument: {a}"));
                }
                file = Some(PathBuf::from(a));
            }
        }
        i += 1;
    }
    if watch && clean != CleanMode::None {
        return Err("-pvc/--watch cannot be combined with -c or -C".to_string());
    }
    match file {
        Some(file) => Ok(Options {
            file,
            out_dir,
            aux_dir,
            cache_dir,
            jobname,
            silent,
            keep_intermediates,
            keep_logs,
            clean,
            engine,
            passthrough,
            watch,
        }),
        None => Err("no input file".to_string()),
    }
}

fn make_absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Explicit development/test override. Normal installations never set this
/// variable and always re-enter the current executable.
fn tool_override(names: &[&str]) -> Option<PathBuf> {
    let directory = std::env::var_os("TEXMK_LIB").filter(|value| !value.is_empty())?;
    names.iter().find_map(|name| {
        let candidate =
            PathBuf::from(&directory).join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        candidate.is_file().then_some(candidate)
    })
}

fn canonical_private_child(path: &Path, root: &Path, description: &str) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root).map_err(|error| {
        format!(
            "cannot resolve private cache root {}: {error}",
            root.display()
        )
    })?;
    let child = std::fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve {description} {}: {error}", path.display()))?;
    if child == root || !child.starts_with(&root) {
        return Err(format!(
            "{description} {} escapes its private cache root {}",
            path.display(),
            root.display()
        ));
    }
    Ok(child)
}

fn validate_job_name(name: &str) -> Result<(), String> {
    let mut components = Path::new(name).components();
    let one_normal_component = matches!(components.next(), Some(std::path::Component::Normal(part)) if part == OsStr::new(name))
        && components.next().is_none();
    if !one_normal_component || name.contains(['/', '\\']) || name.chars().any(char::is_control) {
        return Err("-jobname must be one file-name component without path separators".to_string());
    }
    Ok(())
}

fn artifact_path(out_dir: &Path, job: &str, ext: &str) -> PathBuf {
    out_dir.join(format!("{job}{ext}"))
}

use tex_kpse::platform_cache_dir;

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn hex_encode(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(text.len() * 2);
    for byte in text.bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 15) as usize] as char);
    }
    encoded
}

fn hex_decode(text: &str) -> Option<String> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    let mut decoded = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().as_chunks::<2>().0 {
        decoded.push(digit(pair[0])? << 4 | digit(pair[1])?);
    }
    String::from_utf8(decoded).ok()
}

#[derive(Default)]
struct Manifest {
    identity: String,
    pdf: PathBuf,
    pdf_owned: bool,
    pdf_hash: Option<u64>,
    synctex: PathBuf,
    synctex_owned: bool,
    synctex_hash: Option<u64>,
    exports: BTreeMap<PathBuf, u64>,
    aux_files: BTreeMap<PathBuf, u64>,
    bibliography_signature: Option<u64>,
    bibliography_output_hash: Option<u64>,
    /// Per formatted index (`.ind`, named relative to the auxiliary
    /// directory): the signature of the makeindex run that wrote it and the
    /// hash of what it wrote.
    indexes: BTreeMap<PathBuf, (u64, u64)>,
}

fn read_to_string_bounded(path: &Path, max_bytes: u64) -> std::io::Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Ok(None);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn read_manifest(path: &Path) -> Option<Manifest> {
    let text = read_to_string_bounded(path, MANIFEST_MAX_BYTES).ok()??;
    let mut lines = text.lines();
    if !matches!(
        lines.next()?,
        "TEXMK-CACHE-1" | "TEXMK-CACHE-2" | "TEXMK-CACHE-3" | "TEXMK-CACHE-4" | "TEXMK-CACHE-5"
    ) {
        return None;
    }
    let mut manifest = Manifest::default();
    for line in lines {
        let mut fields = line.split('\t');
        match fields.next()? {
            "identity" => manifest.identity = hex_decode(fields.next()?)?,
            "pdf" => {
                manifest.pdf = PathBuf::from(hex_decode(fields.next()?)?);
                manifest.pdf_owned = fields.next()? == "1";
                manifest.pdf_hash = match fields.next()? {
                    "-" => None,
                    value => value.parse().ok(),
                };
            }
            "synctex" => {
                manifest.synctex = PathBuf::from(hex_decode(fields.next()?)?);
                manifest.synctex_owned = fields.next()? == "1";
                manifest.synctex_hash = match fields.next()? {
                    "-" => None,
                    value => value.parse().ok(),
                };
            }
            "export" => {
                let file = PathBuf::from(hex_decode(fields.next()?)?);
                let digest = fields.next()?.parse().ok()?;
                manifest.exports.insert(file, digest);
            }
            "aux" => {
                let file = PathBuf::from(hex_decode(fields.next()?)?);
                let digest = fields.next()?.parse().ok()?;
                manifest.aux_files.insert(file, digest);
            }
            "bibliography" => {
                manifest.bibliography_signature = match fields.next()? {
                    "-" => None,
                    value => Some(value.parse().ok()?),
                };
            }
            "bibliography-output" => {
                manifest.bibliography_output_hash = match fields.next()? {
                    "-" => None,
                    value => Some(value.parse().ok()?),
                };
            }
            "index" => {
                let file = PathBuf::from(hex_decode(fields.next()?)?);
                let signature = fields.next()?.parse().ok()?;
                let output = fields.next()?.parse().ok()?;
                manifest.indexes.insert(file, (signature, output));
            }
            _ => return None,
        }
    }
    Some(manifest)
}

fn build_identity(
    source: &Path,
    job: &str,
    engine: &str,
    output_dir: &Path,
    aux_identity: &str,
    passthrough: &[String],
) -> String {
    format!(
        "source={}\njob={job}\nengine={engine}\noutput={}\naux={aux_identity}\noptions={passthrough:?}",
        source.display(),
        output_dir.display()
    )
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
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

fn unique_temporary(destination: &Path) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    destination.with_extension(format!("tmp-{}-{nonce}", std::process::id()))
}

fn atomic_publish(source: &Path, destination: &Path) -> std::io::Result<()> {
    let temporary = unique_temporary(destination);
    let result = (|| {
        if std::fs::hard_link(source, &temporary).is_err() {
            let mut input = std::fs::File::open(source)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            std::io::copy(&mut input, &mut output)?;
            drop(output);
        }
        replace_file(&temporary, destination)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

fn atomic_copy(source: &Path, destination: &Path) -> std::io::Result<()> {
    let temporary = unique_temporary(destination);
    let result = (|| {
        let mut input = std::fs::File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        std::io::copy(&mut input, &mut output)?;
        drop(output);
        replace_file(&temporary, destination)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

fn atomic_write_marker(destination: &Path) -> std::io::Result<()> {
    let temporary = unique_temporary(destination);
    let result = (|| {
        drop(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?,
        );
        replace_file(&temporary, destination)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

fn take_cache_hit_marker(path: &Path) -> bool {
    let valid = std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_file() && metadata.len() == 0);
    let removed = std::fs::remove_file(path).is_ok();
    valid && removed
}

fn write_manifest(path: &Path, manifest: &Manifest) -> std::io::Result<()> {
    let mut text = String::from("TEXMK-CACHE-5\n");
    text.push_str(&format!("identity\t{}\n", hex_encode(&manifest.identity)));
    text.push_str(&format!(
        "pdf\t{}\t{}\t{}\n",
        hex_encode(&manifest.pdf.to_string_lossy()),
        u8::from(manifest.pdf_owned),
        manifest
            .pdf_hash
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    text.push_str(&format!(
        "synctex\t{}\t{}\t{}\n",
        hex_encode(&manifest.synctex.to_string_lossy()),
        u8::from(manifest.synctex_owned),
        manifest
            .synctex_hash
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    for (path, digest) in &manifest.exports {
        text.push_str(&format!(
            "export\t{}\t{digest}\n",
            hex_encode(&path.to_string_lossy())
        ));
    }
    for (path, digest) in &manifest.aux_files {
        text.push_str(&format!(
            "aux\t{}\t{digest}\n",
            hex_encode(&path.to_string_lossy())
        ));
    }
    text.push_str(&format!(
        "bibliography\t{}\n",
        manifest
            .bibliography_signature
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    text.push_str(&format!(
        "bibliography-output\t{}\n",
        manifest
            .bibliography_output_hash
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    for (file, (signature, output)) in &manifest.indexes {
        text.push_str(&format!(
            "index\t{}\t{signature}\t{output}\n",
            hex_encode(&file.to_string_lossy())
        ));
    }
    if text.len() as u64 > MANIFEST_MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "cache manifest exceeds the {} MiB safety limit",
                MANIFEST_MAX_BYTES / (1024 * 1024)
            ),
        ));
    }
    if file_equals(path, text.as_bytes()) {
        return Ok(());
    }
    let temporary = unique_temporary(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        drop(file);
        replace_file(&temporary, path)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

/// Exclusive advisory lock (`flock`/`LockFileEx`) on a lock file. The OS
/// releases it when every holder exits, so a crashed or killed build never
/// leaves a stale lock behind and no process-ID heuristics are needed.
// Off Unix `path` is unused (nothing unlinks the lock file) and `file` only
// holds the lock until dropped.
#[cfg_attr(not(unix), allow(dead_code))]
struct CacheLock {
    path: PathBuf,
    file: std::fs::File,
}

impl CacheLock {
    /// The lock, or `None` while another process holds it.
    fn try_acquire(path: &Path) -> std::io::Result<Option<Self>> {
        loop {
            let file = Self::open(path)?;
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
                Err(std::fs::TryLockError::Error(error)) => return Err(error),
            }
            if Self::still_linked(&file, path) {
                return Ok(Some(Self {
                    path: path.to_path_buf(),
                    file,
                }));
            }
        }
    }

    /// Wait for concurrent builds of the same job (including an engine
    /// still running after its texmk was killed) to finish.
    fn acquire(path: &Path) -> Result<Self, String> {
        let failure = |error: std::io::Error| format!("cannot lock {}: {error}", path.display());
        if let Some(lock) = Self::try_acquire(path).map_err(failure)? {
            return Ok(lock);
        }
        eprintln!("texmk: waiting for another build of this document to finish");
        loop {
            let file = Self::open(path).map_err(failure)?;
            file.lock().map_err(failure)?;
            if Self::still_linked(&file, path) {
                return Ok(Self {
                    path: path.to_path_buf(),
                    file,
                });
            }
        }
    }

    fn open(path: &Path) -> std::io::Result<std::fs::File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
    }

    /// A previous holder unlinks the lock file when it finishes. A waiter
    /// that then locks the orphaned inode must retry with the current file.
    #[cfg(unix)]
    fn still_linked(file: &std::fs::File, path: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        match (file.metadata(), std::fs::symlink_metadata(path)) {
            (Ok(locked), Ok(current)) => {
                current.file_type().is_file()
                    && locked.dev() == current.dev()
                    && locked.ino() == current.ino()
            }
            _ => false,
        }
    }

    /// Lock files are never unlinked off Unix, so the opened file is current.
    #[cfg(not(unix))]
    fn still_linked(_file: &std::fs::File, _path: &Path) -> bool {
        true
    }

    /// Let child processes inherit the locked descriptor. If texmk itself is
    /// killed, an engine it started keeps the job locked until it exits, so
    /// the next build cannot interleave with its writes to the private
    /// staging and auxiliary directories.
    #[cfg(unix)]
    fn share_with_children(&self) {
        use std::os::fd::AsRawFd;
        let fd = self.file.as_raw_fd();
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }
        }
    }

    #[cfg(not(unix))]
    fn share_with_children(&self) {}
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        // Unlink while still holding the lock; waiters notice and retry.
        #[cfg(unix)]
        if Self::still_linked(&self.file, &self.path) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn file_equals(path: &Path, expected: &[u8]) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() != expected.len() as u64 {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut offset = 0;
    let mut buffer = [0u8; IO_BUFFER_BYTES];
    while offset < expected.len() {
        let wanted = (expected.len() - offset).min(buffer.len());
        if file.read_exact(&mut buffer[..wanted]).is_err()
            || buffer[..wanted] != expected[offset..offset + wanted]
        {
            return false;
        }
        offset += wanted;
    }
    true
}

/// (exists, hash of contents); missing files hash to a distinct stable value
/// so that appearing/disappearing artifacts count as changes.
fn strict_file_hash(p: &Path) -> std::io::Result<u64> {
    let mut file = std::fs::File::open(p)?;
    let mut h = DefaultHasher::new();
    let mut buffer = [0u8; IO_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(h.finish());
        }
        h.write(&buffer[..read]);
    }
}

fn file_hash(p: &Path) -> (bool, u64) {
    match strict_file_hash(p) {
        Ok(hash) => (true, hash),
        Err(_) => (false, 0),
    }
}

/// Identity of a staged engine output. The engine replaces outputs by
/// renaming a new file into place, so on Unix the inode and timestamps
/// identify a rewrite without rereading large PDF and SyncTeX files twice
/// per pass.
#[cfg(unix)]
fn file_identity(path: &Path) -> Option<(u64, u64, u64, i64, i64, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    metadata.is_file().then(|| {
        (
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    })
}

#[cfg(not(unix))]
fn file_identity(path: &Path) -> Option<(u64, u64, Option<SystemTime>)> {
    let metadata = std::fs::metadata(path).ok()?;
    metadata
        .is_file()
        .then(|| (metadata.len(), file_hash(path).1, metadata.modified().ok()))
}

#[cfg(unix)]
fn same_file(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(left), Ok(right)) = (
        std::fs::symlink_metadata(left),
        std::fs::symlink_metadata(right),
    ) else {
        return false;
    };
    left.file_type().is_file()
        && right.file_type().is_file()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(_left: &Path, _right: &Path) -> bool {
    false
}

fn ignored_state_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some("log" | "blg" | "ilg" | "depcache" | "pdf" | "fls")
    ) || path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            name == ".lock"
                || name == "manifest"
                || name.ends_with(".synctex.gz")
                || name.ends_with(".synctex")
                || name.contains(".tmp-")
                || name.ends_with(".tmp")
        })
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_files(root, &path, files);
        } else if kind.is_file() {
            if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_path_buf());
            }
        }
    }
}

#[cfg(unix)]
fn state_file_metadata_matches(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    after.file_type().is_file()
        && before.len() == after.len()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

#[cfg(not(unix))]
fn state_file_metadata_matches(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    after.file_type().is_file()
        && before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
}

fn state_snapshot_files(
    root: &Path,
    directory: &Path,
    snapshot: &mut BTreeMap<PathBuf, (u64, u64)>,
) -> std::io::Result<()> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("cannot read directory {}: {error}", directory.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot read an entry in {}: {error}", directory.display()),
            )
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot inspect {}: {error}", path.display()),
            )
        })?;
        if kind.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("refusing symbolic link {}", path.display()),
            ));
        }
        if kind.is_dir() {
            state_snapshot_files(root, &path, snapshot)?;
            continue;
        }
        if !kind.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported auxiliary-state entry {}", path.display()),
            ));
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot read metadata for {}: {error}", path.display()),
            )
        })?;
        if !metadata.file_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "auxiliary-state entry changed while inspecting {}",
                    path.display()
                ),
            ));
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "auxiliary-state path escaped {}: {}",
                    root.display(),
                    path.display()
                ),
            )
        })?;
        if ignored_state_file(relative) {
            continue;
        }
        let hash = strict_file_hash(&path).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                    "cannot hash auxiliary-state file {}: {error}",
                    path.display()
                ),
            )
        })?;
        let after = std::fs::symlink_metadata(&path).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot recheck metadata for {}: {error}", path.display()),
            )
        })?;
        if !state_file_metadata_matches(&metadata, &after) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "auxiliary-state file changed while hashing {}",
                    path.display()
                ),
            ));
        }
        snapshot.insert(relative.to_path_buf(), (metadata.len(), hash));
    }
    Ok(())
}

fn strict_state_snapshot(root: &Path) -> std::io::Result<BTreeMap<PathBuf, (u64, u64)>> {
    let mut snapshot = BTreeMap::new();
    state_snapshot_files(root, root, &mut snapshot)?;
    Ok(snapshot)
}

fn state_change_summary(
    before: &BTreeMap<PathBuf, (u64, u64)>,
    after: &BTreeMap<PathBuf, (u64, u64)>,
) -> String {
    const SHOWN: usize = 6;
    let changed: Vec<_> = before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| before.get(*path) != after.get(*path))
        .collect();
    let mut names = changed
        .iter()
        .take(SHOWN)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if changed.len() > SHOWN {
        names.push_str(&format!(", and {} more", changed.len() - SHOWN));
    }
    names
}

/// Inspects whether the auxiliary state produced on the first pass is already
/// complete and stable, making a second pass redundant.
///
/// A document requires a second pass if:
/// - Cross-references, citations, or labels are pending resolution (`sig.rerun`, `sig.undef_refs`, `sig.undef_cites`).
/// - Bibliography generation is needed (`need_bibtex`, `sig.bbl_missing`).
/// - Secondary auxiliary structures were written (.toc, .lof, .lot, .out, .nav, .snm, .vrb, .idx, etc.).
/// - The .aux files contain non-inert commands like `\newlabel`, `\citation`, `\bibdata`, `\@writefile`.
fn is_trivially_converged_first_pass(
    aux_dir: &Path,
    snap_after: &BTreeMap<PathBuf, (u64, u64)>,
    sig: &Signals,
    need_bibtex: bool,
) -> bool {
    if sig.rerun || sig.undef_refs || sig.undef_cites || sig.bbl_missing || need_bibtex {
        return false;
    }
    for rel_path in snap_after.keys() {
        if rel_path.extension().and_then(|ext| ext.to_str()) != Some("aux") {
            return false;
        }
        let full_path = aux_dir.join(rel_path);
        let Ok(content) = std::fs::read_to_string(&full_path) else {
            return false;
        };
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('%') || trimmed == "\\relax" {
                continue;
            }
            if trimmed.starts_with("\\gdef \\@abspage@last{")
                || trimmed.starts_with("\\gdef\\@abspage@last{")
                || trimmed.starts_with("\\providecommand\\color")
                || trimmed.starts_with("\\providecommand\\transparent")
                || trimmed.starts_with("\\providecommand\\HyperFirstAtBeginDocument")
            {
                continue;
            }
            return false;
        }
    }
    true
}

fn artifact_snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, u64, Option<SystemTime>)> {
    let mut paths = Vec::new();
    collect_files(root, root, &mut paths);
    paths
        .into_iter()
        .filter_map(|relative| {
            let full = root.join(&relative);
            let meta = std::fs::metadata(&full).ok()?;
            Some((
                relative,
                (meta.len(), file_hash(&full).1, meta.modified().ok()),
            ))
        })
        .collect()
}

fn exportable_artifact(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str) != Some("pdf")
        && !matches!(path.extension().and_then(OsStr::to_str), Some("depcache"))
        && !path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| {
                name == ".lock"
                    || name == "manifest"
                    || name.contains(".tmp-")
                    || name.ends_with(".tmp")
            })
}

fn owned_file_matches(path: &Path, expected: u64) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
        && file_hash(path) == (true, expected)
}

fn regular_file_hash(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.file_type().is_file())
        .and_then(|_| {
            let (readable, hash) = file_hash(path);
            readable.then_some(hash)
        })
}

fn owned_descendant_matches(path: &Path, root: &Path, expected: u64) -> bool {
    if !path.is_absolute() || !owned_file_matches(path, expected) {
        return false;
    }
    let (Ok(root), Ok(path)) = (std::fs::canonicalize(root), std::fs::canonicalize(path)) else {
        return false;
    };
    path != root && path.starts_with(root)
}

fn trusted_owned_relative(path: &Path, root: &Path, expected: u64) -> Option<PathBuf> {
    if !path.is_absolute() || !owned_file_matches(path, expected) {
        return None;
    }
    let root = std::fs::canonicalize(root).ok()?;
    let path = std::fs::canonicalize(path).ok()?;
    let relative = path.strip_prefix(&root).ok()?;
    (!relative.as_os_str().is_empty()).then(|| relative.to_path_buf())
}

fn contained_export_destination(output_dir: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.is_absolute() || relative.file_name().is_none() {
        return Err(format!(
            "refusing unsafe export path {}",
            relative.display()
        ));
    }
    let root = std::fs::canonicalize(output_dir).map_err(|error| {
        format!(
            "cannot resolve output directory {}: {error}",
            output_dir.display()
        )
    })?;
    let Some(relative_parent) = relative.parent() else {
        return Err(format!(
            "refusing unsafe export path {}",
            relative.display()
        ));
    };
    let mut parent = root.clone();
    for component in relative_parent.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(format!(
                "refusing unsafe export path {}",
                relative.display()
            ));
        };
        let candidate = parent.join(component);
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(format!(
                    "refusing export through non-directory {}",
                    candidate.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&candidate).map_err(|error| {
                    format!(
                        "cannot create export directory {}: {error}",
                        candidate.display()
                    )
                })?;
            }
            Err(error) => {
                return Err(format!(
                    "cannot inspect export directory {}: {error}",
                    candidate.display()
                ));
            }
        }
        parent = std::fs::canonicalize(&candidate).map_err(|error| {
            format!(
                "cannot resolve export directory {}: {error}",
                candidate.display()
            )
        })?;
        if parent == root || !parent.starts_with(&root) {
            return Err(format!(
                "refusing export outside output directory {}",
                output_dir.display()
            ));
        }
    }
    let destination = parent.join(relative.file_name().expect("checked above"));
    Ok(destination)
}

fn trusted_aux_ownership(
    manifest: &mut Manifest,
    aux_dir: &Path,
    pdf_path: &Path,
) -> BTreeSet<PathBuf> {
    let mut trusted = BTreeSet::new();
    manifest.aux_files.retain(|path, expected| {
        let Some(relative) = trusted_owned_relative(path, aux_dir, *expected) else {
            return false;
        };
        if path == pdf_path
            || !exportable_artifact(&relative)
            || !owned_file_matches(path, *expected)
        {
            return false;
        }
        trusted.insert(relative);
        true
    });
    trusted
}

/// Aux-directory files this build may export: everything in a private
/// managed directory; in an explicit (possibly shared) directory only files
/// the engine or BibTeX reported writing, or that texmk already owns.
fn generated_artifacts(
    final_snapshot: &BTreeMap<PathBuf, (u64, u64, Option<SystemTime>)>,
    recorded: &BTreeSet<PathBuf>,
    trusted_owned: &BTreeSet<PathBuf>,
    managed_aux: bool,
) -> BTreeSet<PathBuf> {
    final_snapshot
        .keys()
        .filter(|relative| exportable_artifact(relative))
        .filter(|relative| {
            managed_aux || trusted_owned.contains(*relative) || recorded.contains(*relative)
        })
        .cloned()
        .collect()
}

/// Ownership (what `-c` may delete) comes only from files texmk, the engine
/// (`-recorder` OUTPUT lines) or BibTeX wrote. A directory diff would also
/// claim files a user created in a shared directory during the build.
fn update_aux_ownership(
    manifest: &mut Manifest,
    aux_dir: &Path,
    pdf_path: &Path,
    recorded: &BTreeSet<PathBuf>,
    final_snapshot: &BTreeMap<PathBuf, (u64, u64, Option<SystemTime>)>,
    trusted_owned: &BTreeSet<PathBuf>,
) {
    manifest.aux_files.clear();
    for (relative, (_, hash, _)) in final_snapshot {
        let path = aux_dir.join(relative);
        if path == pdf_path || !exportable_artifact(relative) {
            continue;
        }
        if trusted_owned.contains(relative) || recorded.contains(relative) {
            manifest.aux_files.insert(path, *hash);
        }
    }
}

/// Add the aux-directory OUTPUT entries of an engine `-recorder` file.
fn record_engine_outputs(fls: &Path, aux_dir: &Path, cwd: &Path, recorded: &mut BTreeSet<PathBuf>) {
    let Ok(Some(text)) = read_to_string_bounded(fls, MANIFEST_MAX_BYTES) else {
        return;
    };
    for line in text.lines() {
        let Some(path) = line.strip_prefix("OUTPUT ") else {
            continue;
        };
        let path = cwd.join(path);
        if let Ok(relative) = path.strip_prefix(aux_dir) {
            if !relative.as_os_str().is_empty() {
                recorded.insert(relative.to_path_buf());
            }
        }
    }
}

fn prepare_owned_exports(
    manifest: &mut Manifest,
    output_dir: &Path,
    keep_intermediates: bool,
    keep_logs: bool,
    job: &str,
) {
    let previous = std::mem::take(&mut manifest.exports);
    let log_name = format!("{job}.log");
    for (path, expected) in previous {
        if !owned_descendant_matches(&path, output_dir, expected) {
            continue;
        }
        let keep = keep_intermediates
            || (keep_logs && path.file_name().and_then(OsStr::to_str) == Some(log_name.as_str()));
        if keep {
            manifest.exports.insert(path, expected);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn export_artifacts(
    aux_dir: &Path,
    output_dir: &Path,
    job: &str,
    all: bool,
    generated: &BTreeSet<PathBuf>,
    manifest: &mut Manifest,
) -> Result<(), String> {
    let relative_paths: Vec<PathBuf> = if all {
        generated.iter().cloned().collect()
    } else {
        vec![PathBuf::from(format!("{job}.log"))]
    };
    for relative in relative_paths {
        if !generated.contains(&relative) || !exportable_artifact(&relative) {
            continue;
        }
        let source = aux_dir.join(&relative);
        if !std::fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.file_type().is_file())
        {
            continue;
        }
        let destination = contained_export_destination(output_dir, &relative)?;
        if source == destination {
            continue;
        }
        if std::fs::symlink_metadata(&destination).is_ok()
            && !manifest
                .exports
                .get(&destination)
                .is_some_and(|expected| owned_file_matches(&destination, *expected))
        {
            eprintln!(
                "texmk: warning: preserving unowned artifact {} instead of overwriting it",
                destination.display()
            );
            continue;
        }
        atomic_copy(&source, &destination).map_err(|error| {
            format!(
                "cannot export {} to {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
        manifest
            .exports
            .insert(destination.clone(), file_hash(&destination).1);
    }
    Ok(())
}

struct RetentionContext<'a> {
    aux_dir: &'a Path,
    output_dir: &'a Path,
    job: &'a str,
    keep_intermediates: bool,
    keep_logs: bool,
    managed_aux: bool,
    recorded_outputs: &'a std::cell::RefCell<BTreeSet<PathBuf>>,
    trusted_owned: &'a BTreeSet<PathBuf>,
    manifest_path: &'a Path,
}

fn retain_requested(context: &RetentionContext<'_>, manifest: &mut Manifest) {
    let final_snapshot = artifact_snapshot(context.aux_dir);
    let recorded = context.recorded_outputs.borrow();
    let generated = generated_artifacts(
        &final_snapshot,
        &recorded,
        context.trusted_owned,
        context.managed_aux,
    );
    let pdf_path = manifest.pdf.clone();
    update_aux_ownership(
        manifest,
        context.aux_dir,
        &pdf_path,
        &recorded,
        &final_snapshot,
        context.trusted_owned,
    );
    let result = if context.keep_intermediates {
        export_artifacts(
            context.aux_dir,
            context.output_dir,
            context.job,
            true,
            &generated,
            manifest,
        )
    } else if context.keep_logs {
        export_artifacts(
            context.aux_dir,
            context.output_dir,
            context.job,
            false,
            &generated,
            manifest,
        )
    } else {
        Ok(())
    };
    if let Err(error) = result {
        eprintln!("texmk: warning: {error}");
    }
    if let Err(error) = write_manifest(context.manifest_path, manifest) {
        eprintln!(
            "texmk: warning: cannot update cache manifest {}: {error}",
            context.manifest_path.display()
        );
    }
}

fn clean_owned_artifacts(
    manifest: &Manifest,
    output_dir: &Path,
    aux_dir: &Path,
    pdf_path: &Path,
    remove_outputs: bool,
) {
    for (path, expected) in &manifest.exports {
        if owned_descendant_matches(path, output_dir, *expected) {
            let _ = std::fs::remove_file(path);
        }
    }
    for (path, expected) in &manifest.aux_files {
        if path != pdf_path && owned_descendant_matches(path, aux_dir, *expected) {
            let _ = std::fs::remove_file(path);
        }
    }
    if remove_outputs
        && manifest.pdf_owned
        && manifest.pdf == pdf_path
        && manifest.pdf_hash.is_some()
        && manifest.pdf.is_file()
        && manifest.pdf_hash == Some(file_hash(&manifest.pdf).1)
    {
        let _ = std::fs::remove_file(&manifest.pdf);
    }
    if remove_outputs
        && manifest.synctex_owned
        && manifest.synctex_hash.is_some()
        && manifest.synctex.is_file()
        && manifest.synctex_hash == Some(file_hash(&manifest.synctex).1)
    {
        let _ = std::fs::remove_file(&manifest.synctex);
    }
}

fn directory_size(path: &Path) -> u64 {
    let mut relative = Vec::new();
    collect_files(path, path, &mut relative);
    relative
        .iter()
        .filter_map(|file| std::fs::metadata(path.join(file)).ok())
        .map(|meta| meta.len())
        .sum()
}

fn gc_cache(jobs_dir: &Path, current: &Path) {
    let Ok(entries) = std::fs::read_dir(jobs_dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut jobs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if path == current || !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if name.len() != 16 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(key) = u64::from_str_radix(name, 16) else {
            continue;
        };
        let lock_path = path.with_extension("lock");
        let Ok(Some(lock)) = CacheLock::try_acquire(&lock_path) else {
            continue;
        };
        let valid_manifest = read_manifest(&path.join("manifest")).is_some_and(|manifest| {
            !manifest.identity.is_empty() && stable_hash(manifest.identity.as_bytes()) == key
        });
        if !valid_manifest {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        let stamp = std::fs::metadata(path.join("manifest"))
            .and_then(|meta| meta.modified())
            .or_else(|_| std::fs::metadata(&path).and_then(|meta| meta.modified()))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let size = directory_size(&path);
        if now
            .duration_since(stamp)
            .is_ok_and(|age| age > CACHE_MAX_AGE)
        {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            jobs.push((stamp, size, path, lock));
        }
    }
    let mut total: u64 = jobs
        .iter()
        .map(|(_, size, _, _)| *size)
        .sum::<u64>()
        .saturating_add(directory_size(current));
    if total <= CACHE_MAX_BYTES {
        return;
    }
    jobs.sort_by_key(|(stamp, _, _, _)| *stamp);
    for (_, size, path, _lock) in jobs {
        if total <= CACHE_MAX_BYTES {
            break;
        }
        if std::fs::remove_dir_all(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

fn maybe_gc_cache(jobs_dir: &Path, current: &Path) {
    let Some(root) = jobs_dir.parent() else {
        return;
    };
    let stamp = root.join(".gc-stamp");
    let recent = || {
        std::fs::metadata(&stamp)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age < CACHE_GC_INTERVAL)
    };
    if recent() {
        return;
    }
    let lock_path = root.join(".gc-lock");
    let Ok(Some(lock)) = CacheLock::try_acquire(&lock_path) else {
        return;
    };
    if recent() {
        return;
    }
    gc_cache(jobs_dir, current);
    let _ = atomic_write_marker(&stamp);
    drop(lock);
}

struct PrefixMatcher {
    pattern: &'static [u8],
    fallback: Vec<usize>,
    matched: usize,
}

impl PrefixMatcher {
    fn new(pattern: &'static [u8]) -> Self {
        let mut fallback = vec![0; pattern.len()];
        let mut matched = 0;
        for index in 1..pattern.len() {
            while matched > 0 && pattern[index] != pattern[matched] {
                matched = fallback[matched - 1];
            }
            if pattern[index] == pattern[matched] {
                matched += 1;
                fallback[index] = matched;
            }
        }
        Self {
            pattern,
            fallback,
            matched: 0,
        }
    }

    fn feed(&mut self, byte: u8) -> bool {
        while self.matched > 0 && byte != self.pattern[self.matched] {
            self.matched = self.fallback[self.matched - 1];
        }
        if byte == self.pattern[self.matched] {
            self.matched += 1;
            if self.matched == self.pattern.len() {
                self.matched = self.fallback[self.matched - 1];
                return true;
            }
        }
        false
    }
}

/// Page count from an "Output written on <file> (N pages, M bytes)." summary
/// line (real pdflatex convention; singular "1 page").
fn pages_from_output(text: &str) -> Option<u32> {
    let i = text.find("Output written on ")?;
    let tail = &text[i + "Output written on ".len()..];
    let paren = tail.find('(')?;
    let after = tail[paren + 1..].trim_start();
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || !after[digits.len()..].starts_with(" page") {
        return None;
    }
    digits.parse().ok()
}

/// Page count of a produced PDF: `/Count` of the `/Type /Pages` page-tree
/// root, else a scan for `/Type /Page` page objects. None when the file is
/// missing or its structure is not recognizable (compressed object streams).
fn pdf_pages(p: &Path) -> Option<u32> {
    let mut file = std::fs::File::open(p).ok()?;
    let mut buffer = [0u8; IO_BUFFER_BYTES];
    let mut page = PrefixMatcher::new(b"/Type /Page");
    let mut pages_root = PrefixMatcher::new(b"/Type /Pages");
    let mut count = PrefixMatcher::new(b"/Count ");
    let mut page_pending = false;
    let mut page_objects = 0u32;
    let mut root_seen = false;
    let mut count_seen = false;
    let mut reading_count = false;
    let mut count_digits = 0usize;
    let mut count_value = Some(0u32);

    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        for &byte in &buffer[..read] {
            // `/Type /Page` is a prefix of `/Type /Pages`; defer the object
            // count until the following byte distinguishes the two tokens.
            if page_pending {
                if byte != b's' {
                    page_objects = page_objects.saturating_add(1);
                }
                page_pending = false;
            }

            if pages_root.feed(byte) {
                root_seen = true;
            }
            if page.feed(byte) {
                page_pending = true;
            }

            if reading_count {
                if byte.is_ascii_digit() {
                    count_digits += 1;
                    count_value = count_value.and_then(|value| {
                        value
                            .checked_mul(10)
                            .and_then(|value| value.checked_add(u32::from(byte - b'0')))
                    });
                    continue;
                }
                reading_count = false;
                if count_digits > 0 {
                    if let Some(value) = count_value {
                        return Some(value);
                    }
                }
            } else if root_seen && !count_seen && count.feed(byte) {
                count_seen = true;
                reading_count = true;
            }
        }
    }

    if page_pending {
        page_objects = page_objects.saturating_add(1);
    }
    if reading_count && count_digits > 0 {
        if let Some(value) = count_value {
            return Some(value);
        }
    }
    (page_objects > 0).then_some(page_objects)
}

impl Signals {
    fn merge(&mut self, other: Self) {
        self.rerun |= other.rerun;
        self.undef_refs |= other.undef_refs;
        self.undef_cites |= other.undef_cites;
        self.bbl_missing |= other.bbl_missing;
        self.biber_requested |= other.biber_requested;
        for w in other.user_warnings {
            if !self.user_warnings.contains(&w) {
                self.user_warnings.push(w);
            }
        }
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Incremental scanner for the few transcript phrases that influence pass
/// scheduling. It retains only enough overlap to recognize a phrase split at
/// an I/O boundary, even when a tool emits an arbitrarily long line.
struct SignalScanner {
    signals: Signals,
    bbl_marker: String,
    overlap: Vec<u8>,
    window: Vec<u8>,
    max_pattern_len: usize,
    /// The start of the current line, for the warning summary. Reported
    /// warnings are short; the cap bounds memory for pathological lines.
    current_line_bytes: Vec<u8>,
    line_citation: bool,
    line_undefined: bool,
    line_no_file: bool,
    line_bbl: bool,
    /// Bytes of the physical line read so far. TeX breaks a transcript
    /// line after `max_print_line` (79) bytes; such a piece continues on
    /// the next line, so a phrase may straddle the break.
    physical_len: usize,
}

const SIGNAL_LINE_CAPTURE_BYTES: usize = 4096;

/// TeX's `max_print_line`: transcript lines are broken after this many bytes.
const TRANSCRIPT_LINE_WIDTH: usize = 79;

impl SignalScanner {
    fn new(job: &str) -> Self {
        let bbl_marker = format!("{job}.bbl");
        let max_pattern_len = [
            "Rerun to get".len(),
            "Label(s) may have changed".len(),
            "There were undefined references".len(),
            "There were undefined citations".len(),
            "Please (re)run Biber".len(),
            "Citation".len(),
            "undefined".len(),
            "No file ".len(),
            bbl_marker.len(),
        ]
        .into_iter()
        .max()
        .unwrap_or(1);
        Self {
            signals: Signals::default(),
            bbl_marker,
            overlap: Vec::with_capacity(max_pattern_len.saturating_sub(1)),
            window: Vec::new(),
            max_pattern_len,
            current_line_bytes: Vec::new(),
            line_citation: false,
            line_undefined: false,
            line_no_file: false,
            line_bbl: false,
            physical_len: 0,
        }
    }
    fn scan_fragment(&mut self, fragment: &[u8]) {
        self.physical_len += fragment.len();
        let room = SIGNAL_LINE_CAPTURE_BYTES.saturating_sub(self.current_line_bytes.len());
        self.current_line_bytes
            .extend_from_slice(&fragment[..room.min(fragment.len())]);
        let mut window = std::mem::take(&mut self.window);
        window.clear();
        window.extend_from_slice(&self.overlap);
        window.extend_from_slice(fragment);
        // Transcripts are almost always UTF-8: search them as text, which
        // is much faster than comparing every byte window.
        let text = std::str::from_utf8(&window);
        let contains = |needle: &str| match text {
            Ok(text) => text.contains(needle),
            Err(_) => contains_bytes(&window, needle.as_bytes()),
        };
        self.signals.rerun |= contains("Rerun to get") || contains("Label(s) may have changed");
        self.signals.undef_refs |= contains("There were undefined references");
        self.signals.undef_cites |= contains("There were undefined citations");
        self.signals.biber_requested |= contains("Please (re)run Biber");
        self.line_citation |= contains("Citation");
        self.line_undefined |= contains("undefined");
        self.line_no_file |= contains("No file ");
        self.line_bbl |= contains(&self.bbl_marker);

        let retain = window.len().min(self.max_pattern_len.saturating_sub(1));
        self.overlap.clear();
        self.overlap
            .extend_from_slice(&window[window.len().saturating_sub(retain)..]);
        self.window = window;
    }

    fn finish_line(&mut self) {
        let wrapped = std::mem::take(&mut self.physical_len) == TRANSCRIPT_LINE_WIDTH;
        if wrapped {
            return;
        }
        self.signals.undef_cites |= self.line_citation && self.line_undefined;
        self.signals.bbl_missing |= self.line_no_file && self.line_bbl;

        let line_str = String::from_utf8_lossy(self.current_line_bytes.trim_ascii());
        // The engine's terminal diagnostics already carry a `warning: `
        // severity; texmk adds its own, and the transcript spells the same
        // box warning without it, so store one canonical message.
        let trimmed = line_str.trim();
        let message = trimmed.strip_prefix("warning: ").unwrap_or(trimmed);
        if message.starts_with("LaTeX Warning: Reference")
            || message.starts_with("LaTeX Warning: Citation")
            || message.starts_with("LaTeX Warning: There were undefined")
            || message.starts_with("Overfull \\hbox")
            || message.starts_with("Underfull \\hbox")
            || message.starts_with("Overfull \\vbox")
        {
            if !self.signals.user_warnings.iter().any(|known| known == message) {
                self.signals.user_warnings.push(message.to_string());
            }
        }
        self.current_line_bytes.clear();
        self.line_citation = false;
        self.line_undefined = false;
        self.line_no_file = false;
        self.line_bbl = false;
        self.overlap.clear();
    }

    fn feed(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(IO_BUFFER_BYTES) {
            let mut start = 0;
            for (index, byte) in chunk.iter().enumerate() {
                if *byte == b'\n' {
                    self.scan_fragment(&chunk[start..index]);
                    self.finish_line();
                    start = index + 1;
                }
            }
            self.scan_fragment(&chunk[start..]);
        }
    }

    fn finish(mut self) -> Signals {
        self.physical_len = 0;
        self.finish_line();
        self.signals
    }
}

fn scan_signals(text: &str, job: &str) -> Signals {
    let mut scanner = SignalScanner::new(job);
    scanner.feed(text.as_bytes());
    scanner.finish()
}

fn scan_signal_file(path: &Path, job: &str) -> Option<Signals> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut scanner = SignalScanner::new(job);
    let mut buffer = [0u8; IO_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            return Some(scanner.finish());
        }
        scanner.feed(&buffer[..read]);
    }
}

struct AuxGraph {
    root: String,
    combined: String,
    complete: bool,
    issue: Option<String>,
}

impl Default for AuxGraph {
    fn default() -> Self {
        Self {
            root: String::new(),
            combined: String::new(),
            complete: true,
            issue: None,
        }
    }
}

impl AuxGraph {
    fn incomplete(issue: String) -> Self {
        Self {
            complete: false,
            issue: Some(issue),
            ..Self::default()
        }
    }

    fn mark_incomplete(&mut self, issue: String) {
        self.complete = false;
        if self.issue.is_none() {
            self.issue = Some(issue);
        }
    }
}

#[derive(Clone, Copy)]
enum AuxCommand<'a> {
    Citation(&'a str),
    Bibdata(&'a str),
    Bibstyle(&'a str),
    Input(&'a str),
}

fn aux_argument<'a>(source: &'a str, index: &mut usize) -> Option<&'a str> {
    let bytes = source.as_bytes();
    while *index < bytes.len() && bytes[*index].is_ascii_whitespace() {
        *index += 1;
    }
    if bytes.get(*index) != Some(&b'{') {
        return None;
    }
    *index += 1;
    let start = *index;
    while *index < bytes.len() && bytes[*index] != b'}' {
        *index += 1;
    }
    let argument = &source[start..*index];
    if *index < bytes.len() {
        *index += 1;
    }
    Some(argument)
}

/// Match the packaged BibTeX scanner: commands may be indented, whitespace
/// may separate a command from its argument, and trailing text is ignored.
fn aux_commands(source: &str) -> Vec<AuxCommand<'_>> {
    let bytes = source.as_bytes();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphabetic() || bytes[end] == b'@') {
            end += 1;
        }
        let name = &source[start..end];
        let mut rest = end;
        let command = aux_argument(source, &mut rest).and_then(|argument| match name {
            "citation" => Some(AuxCommand::Citation(argument)),
            "bibdata" => Some(AuxCommand::Bibdata(argument)),
            "bibstyle" => Some(AuxCommand::Bibstyle(argument)),
            "@input" => Some(AuxCommand::Input(argument)),
            _ => None,
        });
        if let Some(command) = command {
            commands.push(command);
            index = rest;
        } else if matches!(name, "citation" | "bibdata" | "bibstyle" | "@input") {
            index = rest;
        } else {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        }
    }
    commands
}

fn push_aux_include_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn resolve_aux_include(
    including_file: &Path,
    source_dir: &Path,
    roots: &[PathBuf],
    name: &str,
) -> Result<PathBuf, String> {
    let requested = Path::new(name);
    let with_extension = if name.ends_with(".aux") {
        requested.to_path_buf()
    } else {
        PathBuf::from(format!("{name}.aux"))
    };
    let mut candidates = Vec::new();
    if requested.is_absolute() {
        push_aux_include_candidate(&mut candidates, requested.to_path_buf());
        push_aux_include_candidate(&mut candidates, with_extension);
    } else {
        if let Some(parent) = including_file.parent() {
            push_aux_include_candidate(&mut candidates, parent.join(requested));
        }
        push_aux_include_candidate(&mut candidates, source_dir.join(requested));
        if let Some(parent) = including_file.parent() {
            push_aux_include_candidate(&mut candidates, parent.join(&with_extension));
        }
        push_aux_include_candidate(&mut candidates, source_dir.join(&with_extension));
    }
    let Some(candidate) = candidates.into_iter().find(|candidate| candidate.exists()) else {
        return Err(format!("included aux file {name} is missing"));
    };
    if !candidate.is_file() {
        return Err(format!(
            "included aux path {} is not a regular file",
            candidate.display()
        ));
    }
    let canonical = std::fs::canonicalize(&candidate).map_err(|error| {
        format!(
            "cannot resolve included aux file {}: {error}",
            candidate.display()
        )
    })?;
    if !roots
        .iter()
        .any(|directory| canonical.starts_with(directory))
    {
        return Err(format!(
            "included aux file {} escapes the private aux/source directories",
            candidate.display()
        ));
    }
    Ok(canonical)
}

/// Read the bibliography-relevant aux graph in the same search order as the
/// engine: private aux directory first, then the document source directory.
/// Canonical containment and explicit bounds keep a generated aux file from
/// turning this preflight into an unbounded filesystem traversal.
fn read_aux_graph(root: &Path, aux_dir: &Path, source_dir: &Path) -> AuxGraph {
    if !root.is_file() {
        return AuxGraph::default();
    }
    let mut roots = Vec::new();
    for directory in [aux_dir, source_dir] {
        let canonical = match std::fs::canonicalize(directory) {
            Ok(path) => path,
            Err(error) => {
                return AuxGraph::incomplete(format!(
                    "cannot resolve aux search directory {}: {error}",
                    directory.display()
                ));
            }
        };
        if !roots.contains(&canonical) {
            roots.push(canonical);
        }
    }
    let root = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) => {
            return AuxGraph::incomplete(format!(
                "cannot resolve root aux file {}: {error}",
                root.display()
            ));
        }
    };
    if !roots.iter().any(|directory| root.starts_with(directory)) {
        return AuxGraph::incomplete(format!(
            "root aux file {} is outside the private aux/source directories",
            root.display()
        ));
    }

    // Keep the traversal bounds, roots, and accumulators explicit: bundling
    // this state would obscure which values are shared across recursive calls.
    #[allow(clippy::too_many_arguments)]
    fn visit_aux_file(
        path: &Path,
        root: &Path,
        roots: &[PathBuf],
        source_dir: &Path,
        depth: usize,
        graph: &mut AuxGraph,
        visited: &mut HashSet<PathBuf>,
        total_bytes: &mut u64,
        include_count: &mut usize,
    ) {
        if !visited.insert(path.to_path_buf()) {
            return;
        }
        if depth > AUX_GRAPH_MAX_DEPTH {
            graph.mark_incomplete(format!(
                "aux include nesting exceeds {AUX_GRAPH_MAX_DEPTH} levels at {}",
                path.display()
            ));
            return;
        }
        if visited.len() > AUX_GRAPH_MAX_FILES {
            graph.mark_incomplete(format!(
                "aux include graph exceeds {AUX_GRAPH_MAX_FILES} files"
            ));
            return;
        }
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                graph.mark_incomplete(format!(
                    "cannot inspect included aux file {}: {error}",
                    path.display()
                ));
                return;
            }
        };
        *total_bytes = total_bytes.saturating_add(metadata.len());
        if *total_bytes > AUX_GRAPH_MAX_BYTES {
            graph.mark_incomplete(format!(
                "aux include graph exceeds {} MiB",
                AUX_GRAPH_MAX_BYTES / (1024 * 1024)
            ));
            return;
        }
        let text = match read_to_string_bounded(path, metadata.len().min(AUX_GRAPH_MAX_BYTES)) {
            Ok(Some(text)) => text,
            Ok(None) => {
                graph.mark_incomplete(format!(
                    "included aux file {} changed size while it was being read",
                    path.display()
                ));
                return;
            }
            Err(error) => {
                graph.mark_incomplete(format!(
                    "cannot read included aux file {}: {error}",
                    path.display()
                ));
                return;
            }
        };
        if path == root {
            graph.root.clone_from(&text);
        }

        for command in aux_commands(&text) {
            let (name, argument) = match command {
                AuxCommand::Citation(argument) => ("citation", argument),
                AuxCommand::Bibdata(argument) => ("bibdata", argument),
                AuxCommand::Bibstyle(argument) => ("bibstyle", argument),
                AuxCommand::Input(name) => {
                    *include_count += 1;
                    if *include_count > AUX_GRAPH_MAX_INCLUDES {
                        graph.mark_incomplete(format!(
                            "aux include graph exceeds {AUX_GRAPH_MAX_INCLUDES} include records"
                        ));
                        return;
                    }
                    if name.len() > AUX_GRAPH_MAX_PATH_BYTES {
                        graph.mark_incomplete(format!(
                            "aux include path exceeds {AUX_GRAPH_MAX_PATH_BYTES} bytes"
                        ));
                        continue;
                    }
                    match resolve_aux_include(path, source_dir, roots, name) {
                        Ok(include) => visit_aux_file(
                            &include,
                            root,
                            roots,
                            source_dir,
                            depth + 1,
                            graph,
                            visited,
                            total_bytes,
                            include_count,
                        ),
                        Err(issue) => graph.mark_incomplete(issue),
                    }
                    continue;
                }
            };
            graph.combined.push('\\');
            graph.combined.push_str(name);
            graph.combined.push('{');
            graph.combined.push_str(argument);
            graph.combined.push_str("}\n");
        }
    }

    let mut graph = AuxGraph::default();
    let mut visited = HashSet::new();
    let mut total_bytes = 0u64;
    let mut include_count = 0usize;
    visit_aux_file(
        &root,
        &root,
        &roots,
        source_dir,
        0,
        &mut graph,
        &mut visited,
        &mut total_bytes,
        &mut include_count,
    );
    graph
}

/// Keys from `\citation{...}` lines in the .aux (the set bibtex resolves).
fn aux_citations(aux: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for command in aux_commands(aux) {
        if let AuxCommand::Citation(keys) = command {
            for key in keys.split(',').map(str::trim).filter(|key| !key.is_empty()) {
                set.insert(key.to_string());
            }
        }
    }
    set
}

fn aux_has_bibdata(aux: &str) -> bool {
    aux_commands(aux)
        .into_iter()
        .any(|command| matches!(command, AuxCommand::Bibdata(_)))
}

fn aux_bibliography_dependencies(aux: &str) -> Vec<(&str, tex_kpse::Format, &'static str)> {
    let mut dependencies = Vec::new();
    for command in aux_commands(aux) {
        match command {
            AuxCommand::Bibdata(value) => dependencies.extend(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(|name| (name, tex_kpse::Format::Bib, "TEXBIBINPUTS")),
            ),
            AuxCommand::Bibstyle(value) => {
                dependencies.push((value, tex_kpse::Format::Bst, "TEXBSTINPUTS"));
            }
            AuxCommand::Citation(_) | AuxCommand::Input(_) => {}
        }
    }
    dependencies
}

fn bibliography_dependency_path(
    kpse: &tex_kpse::Kpse,
    aux_dir: &Path,
    source_dir: &Path,
    name: &str,
    format: tex_kpse::Format,
) -> Option<PathBuf> {
    let extension = format.extensions()[0];
    let bare = name.strip_suffix(extension).unwrap_or(name);
    let local = aux_dir.join(format!("{bare}{extension}"));
    if local.is_file() {
        return Some(std::fs::canonicalize(&local).unwrap_or(local));
    }
    let first = format!("{bare}{extension}");
    kpse.find(&first, format)
        .filter(|path| path.is_file())
        .or_else(|| kpse.find(name, format).filter(|path| path.is_file()))
        .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
        .or_else(|| {
            // `Kpse::with_roots` already uses source_dir as cwd. Keep this
            // explicit fallback as documentation and protection for unusual
            // resolver configurations that omit local lookup.
            let path = source_dir.join(first);
            path.is_file()
                .then(|| std::fs::canonicalize(&path).unwrap_or(path))
        })
}
fn extra_bibliography_dependency_path(
    name: &str,
    format: tex_kpse::Format,
    variable: &str,
) -> Option<PathBuf> {
    let extension = format.extensions()[0];
    let requested = Path::new(name);
    let with_extension = if requested.extension().is_some() {
        requested.to_path_buf()
    } else {
        PathBuf::from(format!("{name}{extension}"))
    };
    std::env::var_os(variable)
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .map(|directory| directory.join(&with_extension))
        .find(|path| path.is_file())
        .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
}

fn bibliography_tool_identity() -> String {
    static IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            if let Some(path) = tool_override(&["tex-bibtex", "bibtex"]) {
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                let (readable, hash) = file_hash(&path);
                let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
                return format!(
                    "bibtex-override={}:readable={readable}:size={size}:hash={hash}",
                    path.display()
                );
            }
            let executable = std::env::current_exe().ok();
            let metadata = executable
                .as_deref()
                .and_then(|path| std::fs::metadata(path).ok());
            let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
            let modified = metadata
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            format!("embedded-bibtex=1.0:exe-size={size}:exe-modified={modified}")
        })
        .clone()
}

fn embedded_bibliography_dependency(name: &str, format: tex_kpse::Format) -> bool {
    let extension = format.extensions()[0];
    let filename = if name.ends_with(extension) {
        name.to_string()
    } else {
        format!("{name}{extension}")
    };
    tex_kpse::has_embedded_package(&filename)
}

fn bibliography_dependencies_available(aux: &str, aux_dir: &Path, source_dir: &Path) -> bool {
    let mut extra_roots = Vec::new();
    for env in ["TEXMFHOME", "TEXMFLOCAL"] {
        if let Ok(value) = std::env::var(env) {
            extra_roots.extend(std::env::split_paths(&value).filter(|path| path.is_dir()));
        }
    }
    let extra_refs: Vec<&Path> = extra_roots.iter().map(PathBuf::as_path).collect();
    let kpse = tex_kpse::Kpse::with_roots(source_dir, &extra_refs);
    aux_bibliography_dependencies(aux)
        .into_iter()
        .all(|(name, format, extra_variable)| {
            bibliography_dependency_path(&kpse, aux_dir, source_dir, name, format).is_some()
                || embedded_bibliography_dependency(name, format)
                || extra_bibliography_dependency_path(name, format, extra_variable).is_some()
        })
}

fn source_bibliography_is_current(
    source_bbl: &Path,
    aux: &str,
    aux_dir: &Path,
    source_dir: &Path,
) -> bool {
    let Ok(source_metadata) = std::fs::metadata(source_bbl) else {
        return false;
    };
    if !source_metadata.is_file() {
        return false;
    }
    let Ok(source_modified) = source_metadata.modified() else {
        return false;
    };
    let Ok(source_bytes) = std::fs::read(source_bbl) else {
        return false;
    };

    let mut extra_roots = Vec::new();
    for variable in ["TEXMFHOME", "TEXMFLOCAL"] {
        if let Ok(value) = std::env::var(variable) {
            extra_roots.extend(std::env::split_paths(&value).filter(|path| path.is_dir()));
        }
    }
    let extra_refs: Vec<&Path> = extra_roots.iter().map(PathBuf::as_path).collect();
    let kpse = tex_kpse::Kpse::with_roots(source_dir, &extra_refs);
    let comparison_roots: Vec<PathBuf> = [aux_dir, source_dir]
        .into_iter()
        .map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
        .chain(
            extra_roots
                .iter()
                .map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())),
        )
        .collect();
    let mut bibliography_sources = Vec::new();
    for (name, format, extra_variable) in aux_bibliography_dependencies(aux) {
        let primary = bibliography_dependency_path(&kpse, aux_dir, source_dir, name, format);
        let extra = extra_bibliography_dependency_path(name, format, extra_variable);
        let embedded = embedded_bibliography_dependency(name, format);
        if matches!(format, tex_kpse::Format::Bib) {
            if let Some(bytes) = primary
                .as_deref()
                .and_then(|path| std::fs::read(path).ok())
                .or_else(|| kpse.read(name, format))
            {
                bibliography_sources.push(bytes);
            }
            if extra != primary {
                if let Some(bytes) = extra.as_deref().and_then(|path| std::fs::read(path).ok()) {
                    bibliography_sources.push(bytes);
                }
            }
        }
        if primary.is_none() && extra.is_none() && !embedded {
            return false;
        }
        let primary_is_local = primary
            .as_ref()
            .is_some_and(|path| comparison_roots.iter().any(|root| path.starts_with(root)));
        for path in primary
            .as_deref()
            .filter(|_| !embedded || primary_is_local)
            .into_iter()
            .chain(extra.as_deref())
        {
            let Ok(modified) = std::fs::metadata(path).and_then(|metadata| metadata.modified())
            else {
                return false;
            };
            if modified > source_modified {
                return false;
            }
        }
    }
    // Substring search over text (Two-Way, SIMD-accelerated) rather than
    // comparing every byte window: this runs on every warm build, once per
    // citation key over the .bbl and each database.
    let source_text = String::from_utf8_lossy(&source_bytes);
    let databases: Vec<_> = bibliography_sources
        .iter()
        .map(|database| String::from_utf8_lossy(database))
        .collect();
    for key in aux_citations(aux) {
        if key == "*" {
            continue;
        }
        let covered = source_text.contains(&format!("{{{key}}}"));
        let exists_in_database = databases
            .iter()
            .any(|database| database.contains(key.as_str()));
        if exists_in_database && !covered {
            return false;
        }
    }
    true
}

fn adopt_source_bibliography(
    manifest: &mut Manifest,
    source_bbl: &Path,
    staged_bbl: &Path,
    aux: &str,
    aux_dir: &Path,
    source_dir: &Path,
    signature: u64,
) -> bool {
    if !source_bibliography_is_current(source_bbl, aux, aux_dir, source_dir) {
        return false;
    }
    let Some(source_hash) = regular_file_hash(source_bbl) else {
        return false;
    };
    if !owned_file_matches(staged_bbl, source_hash)
        && (source_bbl == staged_bbl
            || std::fs::copy(source_bbl, staged_bbl).is_err()
            || !owned_file_matches(staged_bbl, source_hash))
    {
        return false;
    }
    manifest.bibliography_signature = Some(signature);
    manifest.bibliography_output_hash = Some(source_hash);
    true
}

fn bibliography_signature(aux: &str, aux_dir: &Path, source_dir: &Path) -> u64 {
    let mut identity = bibliography_tool_identity();
    identity.push('\n');
    for variable in [
        "TEXBIBINPUTS",
        "BIBINPUTS",
        "TEXBSTINPUTS",
        "BSTINPUTS",
        "TEXMFHOME",
        "HOME",
        "TEXMFLOCAL",
        "TEXMFDIST",
        "TEX_SUITE_DATA",
        HERMETIC_ENV,
    ] {
        identity.push_str(variable);
        identity.push('=');
        identity.push_str(&format!("{:?}", std::env::var_os(variable)));
        identity.push('\n');
    }
    for command in aux_commands(aux) {
        let (name, argument) = match command {
            AuxCommand::Citation(argument) => ("citation", argument),
            AuxCommand::Bibdata(argument) => ("bibdata", argument),
            AuxCommand::Bibstyle(argument) => ("bibstyle", argument),
            AuxCommand::Input(_) => continue,
        };
        identity.push_str(name);
        identity.push('{');
        identity.push_str(argument);
        identity.push('}');
        identity.push('\n');
    }
    let mut extra_roots = Vec::new();
    for env in ["TEXMFHOME", "TEXMFLOCAL"] {
        if let Ok(v) = std::env::var(env) {
            for p in std::env::split_paths(&v) {
                if p.is_dir() {
                    extra_roots.push(p);
                }
            }
        }
    }
    let extra_refs: Vec<&Path> = extra_roots.iter().map(|p| p.as_path()).collect();
    let kpse = tex_kpse::Kpse::with_roots(source_dir, &extra_refs);
    for (name, format, extra_variable) in aux_bibliography_dependencies(aux) {
        identity.push_str("resolved:");
        identity.push_str(format.extensions()[0]);
        identity.push(':');
        identity.push_str(name);
        match bibliography_dependency_path(&kpse, aux_dir, source_dir, name, format) {
            Some(path) => {
                identity.push('=');
                identity.push_str(&path.to_string_lossy());
                let (exists, hash) = file_hash(&path);
                identity.push_str(&format!(":{exists}:{hash}"));
            }
            None if embedded_bibliography_dependency(name, format) => {
                identity.push_str("=<embedded>")
            }
            None => identity.push_str("=<missing>"),
        }
        identity.push('\n');
        identity.push_str("extra:");
        identity.push_str(extra_variable);
        identity.push(':');
        identity.push_str(name);
        match extra_bibliography_dependency_path(name, format, extra_variable) {
            Some(path) => {
                identity.push('=');
                identity.push_str(&path.to_string_lossy());
                let (exists, hash) = file_hash(&path);
                identity.push_str(&format!(":{exists}:{hash}"));
            }
            None => identity.push_str("=<missing>"),
        }
        identity.push('\n');
    }
    stable_hash(identity.as_bytes())
}

struct ToolOutput {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl ToolOutput {
    /// Replay captured output without moving diagnostics onto stdout.
    fn replay(&self) {
        print!("{}", self.stdout);
        eprint!("{}", self.stderr);
    }

    /// Text retained for the final page-summary fallback. Stream identity does
    /// not matter here, but keep a line boundary when both streams have content.
    fn combined(self) -> String {
        let mut text = self.stdout;
        if !self.stderr.is_empty() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&self.stderr);
        }
        text
    }

    fn signals(&self, job: &str) -> Signals {
        let mut signals = scan_signals(&self.stdout, job);
        signals.merge(scan_signals(&self.stderr, job));
        signals
    }
}

struct BoundedCapture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: u64,
    max_bytes: usize,
    head_bytes: usize,
}

impl BoundedCapture {
    fn new(max_bytes: usize, head_bytes: usize) -> Self {
        let head_bytes = head_bytes.min(max_bytes);
        Self {
            head: Vec::with_capacity(head_bytes),
            tail: VecDeque::with_capacity(max_bytes.saturating_sub(head_bytes)),
            total: 0,
            max_bytes,
            head_bytes,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total = self
            .total
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let head_needed = self.head_bytes.saturating_sub(self.head.len());
        let to_head = head_needed.min(bytes.len());
        self.head.extend_from_slice(&bytes[..to_head]);

        let bytes = &bytes[to_head..];
        let tail_limit = self.max_bytes.saturating_sub(self.head_bytes);
        if tail_limit == 0 || bytes.is_empty() {
            return;
        }
        if bytes.len() >= tail_limit {
            self.tail.clear();
            self.tail
                .extend(&bytes[bytes.len().saturating_sub(tail_limit)..]);
            return;
        }
        let overflow = self
            .tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(tail_limit);
        self.tail.drain(..overflow);
        self.tail.extend(bytes);
    }

    fn into_bytes(self, stream: &str) -> Vec<u8> {
        let retained = self.head.len().saturating_add(self.tail.len());
        let truncated = self.total > retained as u64;
        let mut output = Vec::with_capacity(retained.saturating_add(160));
        output.extend_from_slice(&self.head);
        if truncated {
            let omitted = self.total.saturating_sub(retained as u64);
            output.extend_from_slice(
                format!(
                    "\n[texmk: {stream} truncated; omitted {omitted} bytes; retained first {} and last {} bytes]\n",
                    self.head.len(),
                    self.tail.len()
                )
                .as_bytes(),
            );
        }
        output.extend(self.tail);
        output
    }
}

fn capture_reader<R: Read>(
    mut reader: R,
    max_bytes: usize,
    head_bytes: usize,
    stream: &str,
) -> std::io::Result<String> {
    let mut capture = BoundedCapture::new(max_bytes, head_bytes);
    let mut buffer = [0u8; IO_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            let bytes = capture.into_bytes(stream);
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
        capture.push(&buffer[..read]);
    }
}

fn join_capture(
    handle: std::thread::JoinHandle<std::io::Result<String>>,
) -> std::io::Result<String> {
    handle
        .join()
        .map_err(|_| std::io::Error::other("tool output reader thread panicked"))?
}

/// Run a tool, drain both pipes concurrently, and retain bounded head/tail
/// excerpts as distinct streams.
fn run_tool(
    bin: &Path,
    args: &[String],
    silent: bool,
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> std::io::Result<ToolOutput> {
    let mut command = Command::new(bin);
    command
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let parent = std::process::id();
        // An editor that kills texmk to start a newer build must not leave
        // the old engine running and writing into the shared job state.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() as u32 != parent {
                    return Err(std::io::Error::other("texmk exited before its tool started"));
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("cannot capture tool stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("cannot capture tool stderr"))?;
    let stdout_reader = std::thread::spawn(move || {
        capture_reader(
            stdout,
            TOOL_OUTPUT_MAX_BYTES_PER_STREAM,
            TOOL_OUTPUT_HEAD_BYTES,
            "stdout",
        )
    });
    let stderr_reader = std::thread::spawn(move || {
        capture_reader(
            stderr,
            TOOL_OUTPUT_MAX_BYTES_PER_STREAM,
            TOOL_OUTPUT_HEAD_BYTES,
            "stderr",
        )
    });
    let status = child.wait();
    let stdout = join_capture(stdout_reader);
    let stderr = join_capture(stderr_reader);
    let status = status?;
    let captured = ToolOutput {
        success: status.success(),
        code: status.code(),
        stdout: stdout?,
        stderr: stderr?,
    };
    if !silent {
        captured.replay();
    }
    Ok(captured)
}

fn biber_file(
    kpse: &tex_kpse::Kpse, name: &str, aux_dir: &Path, source_dir: &Path,
) -> Option<PathBuf> {
    bibliography_dependency_path(kpse, aux_dir, source_dir, name, tex_kpse::Format::Bib)
        .or_else(|| extra_bibliography_dependency_path(name, tex_kpse::Format::Bib, "BIBINPUTS"))
        .or_else(|| extra_bibliography_dependency_path(name, tex_kpse::Format::Bib, "TEXINPUTS"))
        .or_else(|| tex_kpse::embedded_tree::member_path(name).map(PathBuf::from))
}

fn biber_signature(bcf: &Path, source_dir: &Path) -> u64 {
    let bytes = std::fs::read(bcf).unwrap_or_default();
    let mut identity = format!("biber:{}\n", env!("CARGO_PKG_VERSION"));
    identity.push_str(&String::from_utf8_lossy(&bytes));
    if let Some(tool) = tool_override(&["biber"]) {
        identity.push_str(&format!("\noverride={}: {:?}", tool.display(), file_hash(&tool)));
    } else if let Some(metadata) = std::env::current_exe().ok()
        .and_then(|path| std::fs::metadata(path).ok())
    {
        identity.push_str(&format!("\nembedded-size={}:modified={:?}",
            metadata.len(), metadata.modified().ok()));
    }
    for variable in ["TEXINPUTS", "BIBINPUTS", "TEXBIBINPUTS", HERMETIC_ENV] {
        identity.push_str(&format!("\n{variable}={:?}", std::env::var_os(variable)));
    }
    if let Ok(names) = tex_biber::datasource_names(&String::from_utf8_lossy(&bytes)) {
        let kpse = tex_kpse::Kpse::with_roots(source_dir, &[]);
        for name in names {
            let path = biber_file(&kpse, &name, bcf.parent().unwrap_or(source_dir), source_dir);
            identity.push_str(&format!("\n{name}={path:?}"));
            if let Some(path) = path {
                let bytes = tex_kpse::fs::read(&path).ok();
                identity.push_str(&format!("{:?}", bytes.as_deref().map(stable_hash)));
            }
        }
    }
    stable_hash(identity.as_bytes())
}

fn run_biber(bcf: &Path, source_dir: &Path, silent: bool) -> i32 {
    if !silent {
        eprintln!("texmk: biber {}", bcf.display());
    }
    if let Some(tool) = tool_override(&["biber"]) {
        return match run_tool(
            &tool, &[bcf.to_string_lossy().into_owned()], silent, source_dir, &[],
        ) {
            Ok(out) if out.success => 0,
            Ok(out) => {
                out.replay();
                1
            }
            Err(error) => {
                eprintln!("texmk: cannot run {}: {error}", tool.display());
                1
            }
        };
    }
    let kpse = tex_kpse::Kpse::with_roots(source_dir, &[]);
    let find_file = |name: &str| biber_file(&kpse, name, bcf.parent().unwrap_or(source_dir), source_dir);
    match tex_biber::run(&tex_biber::Options {
        bcf: bcf.to_path_buf(),
        output: None,
        output_directory: None,
        find_file: &find_file,
    }) {
        Ok(out) => {
            if !silent || out.errors != 0 || out.warnings != 0 {
                eprint!("{}", out.log);
            }
            i32::from(out.errors != 0)
        }
        Err(error) => {
            eprintln!("texmk: biber: {error}");
            1
        }
    }
}

fn run_bibtex(aux_stem: &Path, source_dir: &Path, silent: bool) -> i32 {
    let bibtex = match tool_override(&["tex-bibtex", "bibtex"])
        .map(Ok)
        .unwrap_or_else(std::env::current_exe)
    {
        Ok(path) => path,
        Err(error) => {
            eprintln!("texmk: cannot locate the embedded BibTeX executable: {error}");
            return 1;
        }
    };
    let arg = aux_stem.to_string_lossy().into_owned();
    if !silent {
        eprintln!("texmk: bibtex {arg}");
    }
    let env = vec![(
        OsString::from(TEXMK_INTERNAL_MODE_ENV),
        OsString::from("bibtex"),
    )];
    let mut bbl = aux_stem.as_os_str().to_os_string();
    bbl.push(".bbl");
    match run_tool(&bibtex, &[arg], silent, source_dir, &env) {
        Ok(out) if out.success => 0,
        // TeX Live BibTeX exits 2 after error messages (duplicate entries,
        // bad cross-references) that still produce a complete .bbl.
        Ok(out) if out.code == Some(2) && Path::new(&bbl).is_file() => {
            let mut blg = aux_stem.as_os_str().to_os_string();
            blg.push(".blg");
            eprintln!(
                "texmk: warning: BibTeX reported errors; see {}",
                Path::new(&blg).display()
            );
            0
        }
        Ok(out) => {
            if silent {
                out.replay();
            }
            eprintln!("texmk: bibtex failed");
            1
        }
        Err(e) => {
            eprintln!("texmk: cannot run {}: {e}", bibtex.display());
            1
        }
    }
}

/// Runs the embedded makeindex for each index the last pass wrote whose
/// input, options or style changed since its output was produced. Returns the
/// number of runs, or makeindex's exit status when one fails.
fn run_index_tools(
    manifest: &mut Manifest,
    aux_dir: &Path,
    source_dir: &Path,
    fls_path: &Path,
    log_path: &Path,
    silent: bool,
    recorded: &std::cell::RefCell<BTreeSet<PathBuf>>,
) -> Result<u32, i32> {
    let Ok(Some(fls)) = read_to_string_bounded(fls_path, MANIFEST_MAX_BYTES) else {
        return Ok(0);
    };
    if !fls.lines().any(|line| line.starts_with("OUTPUT ") && line.ends_with(".idx")) {
        return Ok(0);
    }
    let log = std::fs::read(log_path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let jobs = embedded_index::plan(&fls, log.as_deref().unwrap_or(""), aux_dir, source_dir);
    let host = embedded_index::Host { source_dir, aux_dir };
    let mut runs = 0;
    for job in jobs {
        let signature = stable_hash(&embedded_index::signature_input(&job, &host));
        let key = job.ind.strip_prefix(aux_dir).unwrap_or(&job.ind).to_path_buf();
        let current = regular_file_hash(&job.ind);
        let up_to_date = current.is_some()
            && manifest.indexes.get(&key) == Some(&(signature, current.unwrap_or(0)));
        if up_to_date {
            continue;
        }
        if !silent {
            eprintln!("texmk: makeindex {}", job.idx.display());
        }
        let _ = std::fs::remove_file(&job.ind);
        if let Err(status) = embedded_index::run(&job, &host) {
            manifest.indexes.remove(&key);
            eprintln!("texmk: makeindex failed on {}", job.idx.display());
            return Err(status);
        }
        let Some(output_hash) = regular_file_hash(&job.ind) else {
            manifest.indexes.remove(&key);
            eprintln!("texmk: makeindex wrote no output file {}", job.ind.display());
            return Err(1);
        };
        manifest.indexes.insert(key.clone(), (signature, output_hash));
        let mut recorded = recorded.borrow_mut();
        recorded.insert(key.clone());
        recorded.insert(key.with_extension("ilg"));
        runs += 1;
    }
    Ok(runs)
}

/// epstopdf's name for the conversion of an EPS figure.
fn eps_converted_path(eps: &Path) -> PathBuf {
    let stem = eps.file_stem().unwrap_or_default().to_string_lossy();
    eps.with_file_name(format!("{stem}-eps-converted-to.pdf"))
}

/// The EPS figures `convert_eps_figures` found and the PDFs it wrote.
#[derive(Default)]
struct EpsFigures {
    sources: Vec<PathBuf>,
    written: Vec<PathBuf>,
}

/// Convert EPS figures for pdfTeX's graphics rules (epstopdf's
/// `-eps-converted-to.pdf` name, plus `<name>.pdf` when that does not exist).
/// A conversion older than its EPS is redone; `<name>.pdf` is refreshed only
/// while it is still a copy of the previous conversion, never a user file.
fn convert_eps_figures(source_dir: &Path) -> EpsFigures {
    let mut figures = EpsFigures::default();
    let mut visited = HashSet::new();
    let mut dirs_to_visit = vec![source_dir.to_path_buf()];
    while let Some(dir) = dirs_to_visit.pop() {
        // Symlinked directories are followed once: a link to an ancestor
        // would otherwise make this walk endless.
        if !visited.insert(std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() || (kind.is_symlink() && path.is_dir()) {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    dirs_to_visit.push(path);
                }
                continue;
            }
            let is_eps = path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|ext| ext.eq_ignore_ascii_case("eps") || ext.eq_ignore_ascii_case("epsi"));
            if !is_eps {
                continue;
            }
            figures.sources.push(path.clone());
            let Ok(eps_modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
                continue;
            };
            let converted = eps_converted_path(&path);
            let direct_pdf = path.with_extension("pdf");
            let converted_current = std::fs::metadata(&converted)
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| modified >= eps_modified);
            let direct_exists = direct_pdf.exists();
            if converted_current && direct_exists {
                continue;
            }
            let direct_is_previous_conversion = !direct_exists
                || matches!(
                    (std::fs::read(&direct_pdf), std::fs::read(&converted)),
                    (Ok(direct), Ok(previous)) if direct == previous
                );
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(out) = tex_ps::eps_to_pdf(&bytes) else {
                continue;
            };
            if std::fs::write(&converted, &out.pdf_bytes).is_ok() {
                figures.written.push(converted);
            }
            if direct_is_previous_conversion && std::fs::write(&direct_pdf, &out.pdf_bytes).is_ok()
            {
                figures.written.push(direct_pdf);
            }
        }
    }
    figures
}

fn real_main() -> i32 {
    let argv: Vec<String> = std::env::args().collect();
    let opt = match parse_args(&argv) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("texmk: {e}");
            usage();
            return 2;
        }
    };
    if opt.watch {
        return watch_main(&opt);
    }
    build(&opt, &mut BuildInfo::default())
}

/// One complete build of the document; the exit status of the driver.
/// `info` receives what the watch loop needs to know about the build.
fn build(opt: &Options, info: &mut BuildInfo) -> i32 {
    std::env::set_var(HERMETIC_ENV, "1");
    // Like latexmk (and editors such as LaTeX Workshop, whose %DOC% omits the
    // extension), accept the main file's name without `.tex`.
    let file = if !opt.file.is_file() && opt.file.extension() != Some(OsStr::new("tex")) {
        let mut with_extension = opt.file.clone().into_os_string();
        with_extension.push(".tex");
        PathBuf::from(with_extension)
    } else {
        opt.file.clone()
    };
    if !file.is_file() {
        eprintln!("texmk: no such file: {}", opt.file.display());
        return 1;
    }
    let source = std::fs::canonicalize(&file).unwrap_or_else(|_| make_absolute(&file));
    let source_dir = source
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let eps = convert_eps_figures(&source_dir);
    let started = SystemTime::now();
    let target_engine = opt
        .engine
        .as_deref()
        .unwrap_or_else(|| detect_engine(&source));
    let engine_override = tool_override(&[target_engine, "pdflatex"]);
    let using_embedded_engine = engine_override.is_none();
    let executed_engine = if using_embedded_engine {
        "TeXres"
    } else {
        target_engine
    };
    let job = opt.jobname.clone().unwrap_or_else(|| {
        source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "texput".to_string())
    });
    if let Err(error) = validate_job_name(&job) {
        eprintln!("texmk: {error}");
        return 2;
    }
    let mut output_dir = make_absolute(opt.out_dir.as_deref().unwrap_or(&source_dir));
    if let Err(error) = std::fs::create_dir_all(&output_dir) {
        eprintln!(
            "texmk: cannot create output directory {}: {error}",
            output_dir.display()
        );
        return 1;
    }
    output_dir = std::fs::canonicalize(&output_dir).unwrap_or(output_dir);
    let explicit_aux_dir = opt
        .aux_dir
        .as_deref()
        .map(make_absolute)
        .map(|path| {
            std::fs::create_dir_all(&path)?;
            Ok::<_, std::io::Error>(std::fs::canonicalize(&path).unwrap_or(path))
        })
        .transpose();
    let explicit_aux_dir = match explicit_aux_dir {
        Ok(path) => path,
        Err(error) => {
            eprintln!("texmk: cannot create explicit auxiliary directory: {error}");
            return 1;
        }
    };
    let managed_aux = explicit_aux_dir.is_none();
    let aux_identity = explicit_aux_dir
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<managed-job-cache>".to_string());
    let identity = build_identity(
        &source,
        &job,
        target_engine,
        &output_dir,
        &aux_identity,
        &opt.passthrough,
    );
    let key = stable_hash(identity.as_bytes());
    let cache_choice = opt.cache_dir.clone().unwrap_or_else(platform_cache_dir);
    let requested_cache = make_absolute(&cache_choice);
    let mut jobs_dir = requested_cache.join("texmk").join("jobs");
    if let Err(error) = std::fs::create_dir_all(&jobs_dir) {
        let fallback = std::env::temp_dir()
            .join("tex-rs-cache")
            .join("texmk")
            .join("jobs");
        eprintln!(
            "texmk: warning: cannot use cache directory {} ({error}); using {}",
            jobs_dir.display(),
            fallback.display()
        );
        if let Err(fallback_error) = std::fs::create_dir_all(&fallback) {
            eprintln!(
                "texmk: cannot create fallback cache directory {}: {fallback_error}",
                fallback.display()
            );
            return 1;
        }
        jobs_dir = fallback;
    }
    jobs_dir = match std::fs::canonicalize(&jobs_dir) {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!(
                "texmk: cannot resolve private jobs directory {}: {error}",
                jobs_dir.display()
            );
            return 1;
        }
    };
    let requested_job_dir = jobs_dir.join(format!("{key:016x}"));
    // Own the job before creating it; garbage collection must not delete its lock.
    let lock = match CacheLock::acquire(&requested_job_dir.with_extension("lock")) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("texmk: {error}");
            return 1;
        }
    };
    lock.share_with_children();
    if let Err(error) = std::fs::create_dir_all(&requested_job_dir) {
        eprintln!(
            "texmk: cannot create job cache {}: {error}",
            requested_job_dir.display()
        );
        return 1;
    }
    let job_dir = match canonical_private_child(&requested_job_dir, &jobs_dir, "job cache") {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("texmk: {error}");
            return 1;
        }
    };
    maybe_gc_cache(&jobs_dir, &job_dir);
    let pdf_path = artifact_path(&output_dir, &job, ".pdf");
    let synctex_extension = synctex_extension(&opt.passthrough);
    let synctex_path = artifact_path(&output_dir, &job, synctex_extension.unwrap_or(".synctex.gz"));
    let mut aux_dir = explicit_aux_dir.unwrap_or_else(|| job_dir.join("aux"));
    if aux_dir.exists() {
        aux_dir = if managed_aux {
            match canonical_private_child(&aux_dir, &job_dir, "managed auxiliary directory") {
                Ok(directory) => directory,
                Err(error) => {
                    eprintln!("texmk: {error}");
                    return 1;
                }
            }
        } else {
            std::fs::canonicalize(&aux_dir).unwrap_or(aux_dir)
        };
    }
    let manifest_path = job_dir.join("manifest");
    let mut manifest = match read_manifest(&manifest_path) {
        Some(manifest) if manifest.identity == identity => manifest,
        Some(other) => {
            if opt.clean != CleanMode::None && stable_hash(other.identity.as_bytes()) != key {
                drop(lock);
                return match std::fs::remove_dir_all(&job_dir) {
                    Ok(()) => 0,
                    Err(error) => {
                        eprintln!(
                            "texmk: cannot remove mismatched cache {}: {error}",
                            job_dir.display()
                        );
                        1
                    }
                };
            }
            eprintln!(
                "texmk: cache identity collision at {}; refusing to mix build state",
                job_dir.display()
            );
            return 1;
        }
        None if manifest_path.exists() => {
            if opt.clean != CleanMode::None {
                drop(lock);
                return match std::fs::remove_dir_all(&job_dir) {
                    Ok(()) => 0,
                    Err(error) => {
                        eprintln!(
                            "texmk: cannot remove invalid cache {}: {error}",
                            job_dir.display()
                        );
                        1
                    }
                };
            }
            eprintln!(
                "texmk: cache manifest {} is invalid; run texmk -c with the same build options to discard it",
                manifest_path.display()
            );
            return 1;
        }
        None => Manifest {
            identity: identity.clone(),
            pdf: pdf_path.clone(),
            pdf_owned: !pdf_path.exists(),
            synctex: synctex_path.clone(),
            synctex_owned: !synctex_path.exists(),
            ..Manifest::default()
        },
    };
    if manifest.synctex != synctex_path {
        manifest.synctex = synctex_path.clone();
        manifest.synctex_owned = !synctex_path.exists();
        manifest.synctex_hash = None;
    }
    if opt.clean != CleanMode::None {
        clean_owned_artifacts(
            &manifest,
            &output_dir,
            &aux_dir,
            &pdf_path,
            opt.clean == CleanMode::All,
        );
        drop(lock);
        if let Err(error) = std::fs::remove_dir_all(&job_dir) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("texmk: cannot remove cache {}: {error}", job_dir.display());
                return 1;
            }
        }
        return 0;
    }
    if let Err(error) = std::fs::create_dir_all(&aux_dir) {
        eprintln!(
            "texmk: cannot create auxiliary directory {}: {error}",
            aux_dir.display()
        );
        return 1;
    }
    aux_dir = if managed_aux {
        match canonical_private_child(&aux_dir, &job_dir, "managed auxiliary directory") {
            Ok(directory) => directory,
            Err(error) => {
                eprintln!("texmk: {error}");
                return 1;
            }
        }
    } else {
        std::fs::canonicalize(&aux_dir).unwrap_or(aux_dir)
    };
    let stage_dir = job_dir.join("pdf");
    if let Err(error) = std::fs::create_dir_all(&stage_dir) {
        eprintln!(
            "texmk: cannot create private PDF staging directory {}: {error}",
            stage_dir.display()
        );
        return 1;
    }
    let stage_dir = match canonical_private_child(&stage_dir, &job_dir, "PDF staging directory") {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("texmk: {error}");
            return 1;
        }
    };
    let staged_pdf_path = artifact_path(&stage_dir, &job, ".pdf");
    let staged_synctex_path =
        artifact_path(&stage_dir, &job, synctex_extension.unwrap_or(".synctex.gz"));
    let engine_cache_dir = job_dir.join("engine-cache");
    if let Err(error) = std::fs::create_dir_all(&engine_cache_dir) {
        eprintln!(
            "texmk: cannot create private engine cache {}: {error}",
            engine_cache_dir.display()
        );
        return 1;
    }
    let engine_cache_dir = match (
        std::fs::canonicalize(&engine_cache_dir),
        std::fs::canonicalize(&job_dir),
    ) {
        (Ok(cache), Ok(job_root)) if cache != job_root && cache.starts_with(&job_root) => cache,
        _ => {
            eprintln!(
                "texmk: private engine cache {} escapes its job directory",
                engine_cache_dir.display()
            );
            return 1;
        }
    };
    let cache_hit_marker = engine_cache_dir.join(".texmk-cache-hit");
    // The engine may omit this one future output from directory-membership
    // fingerprints. Direct reads and missing-file probes remain dependencies.
    let force_color = tex_core::diagnostics::color_enabled();
    // The Lua font database (luaotfload names, font caches) is per machine, not
    // per document: building it takes tens of seconds, so all jobs share it
    // (and direct engine passes, which keep it in the same place).
    let font_cache_dir = Some(requested_cache.join("texmf-var"));
    let mut engine_env = vec![
        (
            OsString::from(TEXMK_INTERNAL_MODE_ENV),
            OsString::from("engine"),
        ),
        (
            OsString::from("TEX_SUITE_PROGRAM_NAME"),
            OsString::from(target_engine),
        ),
        (
            OsString::from(TEXMK_CACHE_HIT_MARKER_ENV),
            cache_hit_marker.as_os_str().to_os_string(),
        ),
        (
            OsString::from(TEXMK_PUBLISHED_OUTPUT_ENV),
            pdf_path.as_os_str().to_os_string(),
        ),
        (
            OsString::from("CLICOLOR_FORCE"),
            OsString::from(if force_color { "1" } else { "0" }),
        ),
    ];
    if let Some(dir) = font_cache_dir.filter(|dir| std::fs::create_dir_all(dir).is_ok()) {
        engine_env.push((
            OsString::from(TEXMK_FONT_CACHE_ENV),
            dir.into_os_string(),
        ));
    }
    // Watch mode also wants the font files and failed lookups, which the
    // recorder (kept in web2c's format) does not list. The file lives with the
    // engine cache, which is neither snapshotted nor exported.
    let watch_dependencies_file = engine_cache_dir.join(".texmk-watch-dependencies");
    if opt.watch {
        engine_env.push((
            OsString::from(TEXMK_WATCH_DEPENDENCIES_ENV),
            watch_dependencies_file.as_os_str().to_os_string(),
        ));
    }
    let recorded_outputs = std::cell::RefCell::new(BTreeSet::new());
    let trusted_aux_owned = trusted_aux_ownership(&mut manifest, &aux_dir, &pdf_path);
    prepare_owned_exports(
        &mut manifest,
        &output_dir,
        opt.keep_intermediates,
        opt.keep_logs,
        &job,
    );
    if let Err(error) = write_manifest(&manifest_path, &manifest) {
        eprintln!(
            "texmk: cannot write cache manifest {}: {error}",
            manifest_path.display()
        );
        return 1;
    }
    let retention = RetentionContext {
        aux_dir: &aux_dir,
        output_dir: &output_dir,
        job: &job,
        keep_intermediates: opt.keep_intermediates,
        keep_logs: opt.keep_logs,
        managed_aux,
        recorded_outputs: &recorded_outputs,
        trusted_owned: &trusted_aux_owned,
        manifest_path: &manifest_path,
    };
    if opt.watch {
        info.inputs = Some(WatchInputs {
            started,
            source: source.clone(),
            source_dir: source_dir.clone(),
            aux_dir: aux_dir.clone(),
            jobs_dir: jobs_dir.clone(),
            job: job.clone(),
            pdf_path: pdf_path.clone(),
            synctex_path: synctex_path.clone(),
            manifest_path: manifest_path.clone(),
            dependency_file: watch_dependencies_file.clone(),
            eps,
        });
    }
    let engine = match engine_override
        .map(Ok)
        .unwrap_or_else(std::env::current_exe)
    {
        Ok(path) => path,
        Err(error) => {
            eprintln!("texmk: cannot locate its embedded TeX engine: {error}");
            return 1;
        }
    };
    let aux_path = artifact_path(&aux_dir, &job, ".aux");
    let log_path = artifact_path(&aux_dir, &job, ".log");
    let bbl_path = artifact_path(&aux_dir, &job, ".bbl");
    let aux_stem = aux_path.with_extension("");
    let mut prev_cites: Option<BTreeSet<String>> = None;
    let mut bibtex_done = false;
    let mut bibtex_runs = 0u32;
    let mut biber_runs = 0u32;
    let mut index_runs = 0u32;
    let mut passes = 0u32;
    let mut converged = false;
    let mut last_output = String::new();
    let mut last_signals: Option<Signals> = None;
    let mut staged_pdf_written_this_run = false;
    let mut staged_synctex_written_this_run = false;
    let mut child_cache_hit = false;
    let mut last_pass_failed = false;
    let mut recovered_diagnostics = String::new();

    // bibtex depends on .aux, not on a PDF. Refresh .bbl from a previous
    // aux so the first typeset can consume it.
    let initial_aux_graph = read_aux_graph(&aux_path, &aux_dir, &source_dir);
    let source_bbl = source_dir.join(format!("{job}.bbl"));
    if !bbl_path.is_file() && source_bbl.is_file() {
        let _ = std::fs::copy(&source_bbl, &bbl_path);
    }
    let mut aux_graph_warning_emitted = false;
    if let Some(issue) = &initial_aux_graph.issue {
        eprintln!("texmk: warning: {issue}; bibliography cache reuse is disabled for this build");
        aux_graph_warning_emitted = true;
    }
    let initial_aux_text = &initial_aux_graph.combined;
    let initial_bibdata = aux_has_bibdata(initial_aux_text);
    let initial_bcf = aux_dir.join(format!("{job}.bcf"));
    let initial_biber = !initial_bibdata && initial_bcf.is_file();
    let initial_bibliography_ready = initial_biber || (initial_bibdata
        && bibliography_dependencies_available(initial_aux_text, &aux_dir, &source_dir));
    if (initial_aux_graph.complete || initial_biber) && initial_bibliography_ready {
        prev_cites = Some(aux_citations(initial_aux_text));
        let signature = if initial_biber {
            biber_signature(&initial_bcf, &source_dir)
        } else {
            bibliography_signature(initial_aux_text, &aux_dir, &source_dir)
        };
        if !initial_biber && adopt_source_bibliography(
            &mut manifest,
            &source_bbl,
            &bbl_path,
            initial_aux_text,
            &aux_dir,
            &source_dir,
            signature,
        ) {
            bibtex_done = true;
        }
        let bibliography_output_matches = manifest
            .bibliography_output_hash
            .is_some_and(|expected| owned_file_matches(&bbl_path, expected));
        if !bibliography_output_matches || manifest.bibliography_signature != Some(signature) {
            let _ = std::fs::remove_file(&bbl_path);
            let rc = if initial_biber {
                run_biber(&initial_bcf, &source_dir, opt.silent)
            } else {
                run_bibtex(&aux_stem, &source_dir, opt.silent)
            };
            if rc != 0 {
                let source_bbl = source_dir.join(format!("{job}.bbl"));
                if source_bbl.is_file() {
                    let _ = std::fs::copy(&source_bbl, &bbl_path);
                    bibtex_done = true;
                } else {
                    manifest.bibliography_signature = None;
                    manifest.bibliography_output_hash = None;
                    let _ = std::fs::remove_file(&bbl_path);
                    retain_requested(&retention, &mut manifest);
                    return rc;
                }
            } else {
                let Some(output_hash) = regular_file_hash(&bbl_path) else {
                    eprintln!(
                        "texmk: bibliography tool succeeded without producing a regular output file {}",
                        bbl_path.display()
                    );
                    manifest.bibliography_signature = None;
                    manifest.bibliography_output_hash = None;
                    retain_requested(&retention, &mut manifest);
                    return 1;
                };
                bibtex_done = true;
                if initial_biber { biber_runs += 1; } else { bibtex_runs += 1; }
                recorded_outputs.borrow_mut().extend([
                    PathBuf::from(format!("{job}.bbl")),
                    PathBuf::from(format!("{job}.blg")),
                ]);
                manifest.bibliography_signature = Some(signature);
                manifest.bibliography_output_hash = Some(output_hash);
            }
        }
    } else {
        manifest.bibliography_signature = None;
        manifest.bibliography_output_hash = None;
    }

    while passes < MAX_PASSES {
        passes += 1;
        let snap_before = match strict_state_snapshot(&aux_dir) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                eprintln!(
                    "texmk: cannot inspect auxiliary state under {} before pass {passes}: {error}",
                    aux_dir.display()
                );
                retain_requested(&retention, &mut manifest);
                return 1;
            }
        };
        let pdf_before = file_identity(&staged_pdf_path);
        let synctex_before = file_identity(&staged_synctex_path);

        let mut args: Vec<String> = vec![
            "-interaction=nonstopmode".to_string(),
            "-recorder".to_string(),
            "-output-directory".to_string(),
            stage_dir.to_string_lossy().into_owned(),
            "-aux-directory".to_string(),
            aux_dir.to_string_lossy().into_owned(),
            "--cache-directory".to_string(),
            engine_cache_dir.to_string_lossy().into_owned(),
        ];
        if let Some(j) = &opt.jobname {
            args.push("-jobname".to_string());
            args.push(j.clone());
        }
        args.extend(opt.passthrough.iter().cloned());
        args.push(source.to_string_lossy().into_owned());
        if !opt.silent {
            eprintln!("texmk: {target_engine} pass {passes}/{MAX_PASSES}");
        }
        let _ = std::fs::remove_file(&cache_hit_marker);
        let output = match run_tool(&engine, &args, opt.silent, &source_dir, &engine_env) {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_file(&cache_hit_marker);
                eprintln!("texmk: cannot run {}: {e}", engine.display());
                return 1;
            }
        };
        let engine_cache_hit = take_cache_hit_marker(&cache_hit_marker);
        if !engine_cache_hit {
            record_engine_outputs(
                &artifact_path(&aux_dir, &job, ".fls"),
                &aux_dir,
                &source_dir,
                &mut recorded_outputs.borrow_mut(),
            );
        }
        let pass_failed = !output.success;
        if pass_failed {
            let allow_recovery = opt.passthrough.iter().any(|a| {
                a.starts_with("-interaction=nonstopmode") || a.starts_with("-interaction=batchmode")
            });
            let produced_pdf = allow_recovery
                && std::fs::metadata(&staged_pdf_path).is_ok_and(|m| m.is_file() && m.len() > 1000)
                && file_identity(&staged_pdf_path) != pdf_before;
            if !produced_pdf {
                if opt.silent {
                    if using_embedded_engine && !output.stderr.is_empty() {
                        eprint!("{}", output.stderr);
                    } else {
                        output.replay();
                    }
                }
                eprintln!("texmk: {target_engine} failed on pass {passes}");
                if log_path.is_file() {
                    eprintln!("texmk: transcript retained at {}", log_path.display());
                }
                retain_requested(&retention, &mut manifest);
                return 1;
            }
        }
        last_pass_failed = pass_failed;
        recovered_diagnostics = if pass_failed && opt.silent {
            output.stderr.clone()
        } else {
            String::new()
        };

        let mut signals = output.signals(&job);
        let output = output.combined();
        let pdf_after = file_identity(&staged_pdf_path);
        let synctex_after = file_identity(&staged_synctex_path);
        if synctex_after.is_some() && (synctex_before != synctex_after || engine_cache_hit) {
            staged_synctex_written_this_run = true;
        }
        if pdf_after.is_some() && pdf_before != pdf_after {
            staged_pdf_written_this_run = true;
        }
        if engine_cache_hit && pdf_after.is_some() {
            staged_pdf_written_this_run = true;
            if initial_aux_graph.complete {
                if !opt.silent {
                    eprintln!(
                        "texmk: engine cache hit; previously validated auxiliary state is stable"
                    );
                }
                child_cache_hit = true;
                last_output = output;
                if let Some(log_signals) = scan_signal_file(&log_path, &job) {
                    signals.merge(log_signals);
                }
                last_signals = Some(signals);
                converged = true;
                break;
            }
        }
        if let Some(log_signals) = scan_signal_file(&log_path, &job) {
            signals.merge(log_signals);
        }
        last_output = output;
        match run_index_tools(
            &mut manifest,
            &aux_dir,
            &source_dir,
            &artifact_path(&aux_dir, &job, ".fls"),
            &log_path,
            opt.silent,
            &recorded_outputs,
        ) {
            Ok(runs) => index_runs += runs,
            Err(status) => {
                retain_requested(&retention, &mut manifest);
                return status;
            }
        }
        let aux_graph = read_aux_graph(&aux_path, &aux_dir, &source_dir);
        if !aux_graph_warning_emitted {
            if let Some(issue) = &aux_graph.issue {
                eprintln!(
                    "texmk: warning: {issue}; bibliography cache reuse is disabled for this build"
                );
                aux_graph_warning_emitted = true;
            }
        }
        let aux_text = &aux_graph.combined;
        let cites = aux_citations(aux_text);
        let bibdata = aux_has_bibdata(aux_text);
        let bcf_path = aux_dir.join(format!("{job}.bcf"));
        let use_biber = !bibdata && (bcf_path.is_file()
            || aux_text.contains("\\abx@aux@")
            || signals.biber_requested);
        let bibliography_ready = if use_biber {
            true
        } else {
            bibdata && bibliography_dependencies_available(aux_text, &aux_dir, &source_dir)
        };
        let bibliography_signature = if use_biber {
            Some(biber_signature(&bcf_path, &source_dir))
        } else {
            (bibliography_ready && aux_graph.complete)
                .then(|| bibliography_signature(aux_text, &aux_dir, &source_dir))
        };
        if !use_biber && bibliography_signature.is_some_and(|signature| {
            adopt_source_bibliography(
                &mut manifest,
                &source_bbl,
                &bbl_path,
                aux_text,
                &aux_dir,
                &source_dir,
                signature,
            )
        }) {
            bibtex_done = true;
        }
        let sig = Signals {
            bbl_missing: signals.bbl_missing || (bibliography_ready && !bbl_path.is_file()),
            ..signals
        };
        last_signals = Some(sig.clone());
        let snap_after = match strict_state_snapshot(&aux_dir) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                eprintln!(
                    "texmk: cannot inspect auxiliary state under {} after pass {passes}: {error}",
                    aux_dir.display()
                );
                retain_requested(&retention, &mut manifest);
                return 1;
            }
        };
        let files_changed = snap_before != snap_after;
        let cites_changed = prev_cites.as_ref().is_some_and(|p| *p != cites);
        prev_cites = Some(cites);

        let need_bibtex = bibliography_ready
            && (sig.bbl_missing
                || cites_changed
                || (use_biber && sig.biber_requested && !bibtex_done)
                || manifest.bibliography_signature != bibliography_signature
                || !manifest
                    .bibliography_output_hash
                    .is_some_and(|expected| owned_file_matches(&bbl_path, expected))
                || (!aux_graph.complete && !bibtex_done)
                || (!bibtex_done && manifest.bibliography_signature.is_none() && sig.undef_cites));
        if need_bibtex {
            let _ = std::fs::remove_file(&bbl_path);
            let rc = if use_biber {
                run_biber(&bcf_path, &source_dir, opt.silent)
            } else {
                run_bibtex(&aux_stem, &source_dir, opt.silent)
            };
            if rc != 0 {
                let source_bbl = source_dir.join(format!("{job}.bbl"));
                if source_bbl.is_file() {
                    let _ = std::fs::copy(&source_bbl, &bbl_path);
                    bibtex_done = true;
                } else {
                    manifest.bibliography_signature = None;
                    manifest.bibliography_output_hash = None;
                    let _ = std::fs::remove_file(&bbl_path);
                    retain_requested(&retention, &mut manifest);
                    return rc;
                }
            } else {
                let Some(output_hash) = regular_file_hash(&bbl_path) else {
                    eprintln!(
                        "texmk: bibliography tool succeeded without producing a regular output file {}",
                        bbl_path.display()
                    );
                    manifest.bibliography_signature = None;
                    manifest.bibliography_output_hash = None;
                    retain_requested(&retention, &mut manifest);
                    return 1;
                };
                bibtex_done = true;
                if use_biber { biber_runs += 1; } else { bibtex_runs += 1; }
                recorded_outputs.borrow_mut().extend([
                    PathBuf::from(format!("{job}.bbl")),
                    PathBuf::from(format!("{job}.blg")),
                ]);
                manifest.bibliography_signature = bibliography_signature;
                manifest.bibliography_output_hash = Some(output_hash);
                if !opt.silent {
                    eprintln!(
                        "texmk: rerun required after regenerating the bibliography from auxiliary state"
                    );
                }
                continue;
            }
        }
        if !bibliography_ready {
            manifest.bibliography_signature = None;
            manifest.bibliography_output_hash = None;
        }

        // This typeset did not change aux/toc/out: another pass cannot
        // resolve more labels. Sticky "Rerun to get" is not a reason to
        // typeset again.
        let is_stable = !files_changed
            || (passes == 1
                && !snap_before.is_empty()
                && is_trivially_converged_first_pass(&aux_dir, &snap_after, &sig, need_bibtex));
        if is_stable {
            if !opt.silent {
                eprintln!("texmk: auxiliary state stable after pass {passes}");
            }
            converged = true;
            break;
        }
        if !opt.silent {
            eprintln!(
                "texmk: rerun required because auxiliary state changed: {}",
                state_change_summary(&snap_before, &snap_after)
            );
        }
    }

    if !converged {
        eprintln!(
            "texmk: build FAILED: no convergence after {MAX_PASSES} {executed_engine} pass(es)"
        );
        if log_path.is_file() {
            eprintln!("texmk: transcript retained at {}", log_path.display());
        }
        retain_requested(&retention, &mut manifest);
        return 1;
    }
    if !staged_pdf_written_this_run || !staged_pdf_path.is_file() {
        eprintln!(
            "texmk: build FAILED: no output written to private staging path {}",
            staged_pdf_path.display()
        );
        retain_requested(&retention, &mut manifest);
        return 1;
    }
    if staged_synctex_written_this_run {
        let visible_synctex_matches_manifest = child_cache_hit
            && manifest.synctex == synctex_path
            && manifest.synctex_hash.is_some()
            && (same_file(&staged_synctex_path, &synctex_path)
                || manifest
                    .synctex_hash
                    .is_some_and(|expected| owned_file_matches(&synctex_path, expected)));
        if !visible_synctex_matches_manifest {
            if let Err(error) = atomic_publish(&staged_synctex_path, &synctex_path) {
                eprintln!(
                    "texmk: build FAILED: cannot publish {} atomically: {error}",
                    synctex_path.display()
                );
                return 1;
            }
            manifest.synctex_hash = regular_file_hash(&synctex_path);
            if manifest.synctex_hash.is_none() {
                eprintln!(
                    "texmk: build FAILED: SyncTeX output {} is not a readable regular file",
                    synctex_path.display()
                );
                return 1;
            }
        }
        if synctex_extension == Some(".synctex.gz") {
            // pdfTeX removes an uncompressed SyncTeX file from an earlier
            // build when it writes `.synctex.gz`. SyncTeX readers open
            // `<job>.synctex` first, so leaving it would shadow this build.
            let _ = std::fs::remove_file(artifact_path(&output_dir, &job, ".synctex"));
        }
    } else {
        if manifest.synctex == synctex_path
            && manifest.synctex_owned
            && manifest
                .synctex_hash
                .is_some_and(|expected| owned_file_matches(&synctex_path, expected))
        {
            let _ = std::fs::remove_file(&synctex_path);
        }
        manifest.synctex_hash = None;
    }
    let visible_pdf_matches_manifest = child_cache_hit
        && manifest.pdf == pdf_path
        && manifest.pdf_hash.is_some()
        && (same_file(&staged_pdf_path, &pdf_path)
            || manifest
                .pdf_hash
                .is_some_and(|expected| owned_file_matches(&pdf_path, expected)));
    if !visible_pdf_matches_manifest {
        if let Err(error) = atomic_publish(&staged_pdf_path, &pdf_path) {
            eprintln!(
                "texmk: build FAILED: cannot publish {} atomically: {error}",
                pdf_path.display()
            );
            return 1;
        }
        manifest.pdf_hash = Some(file_hash(&pdf_path).1);
    }
    if let Err(error) = write_manifest(&manifest_path, &manifest) {
        eprintln!(
            "texmk: warning: outputs were published but ownership metadata could not be updated at {}: {error}",
            manifest_path.display()
        );
    }
    let pages = pages_from_output(&last_output)
        .or_else(|| (!child_cache_hit).then(|| pdf_pages(&pdf_path)).flatten());
    info.pages = pages;
    let mut bib_note = String::new();
    if bibtex_runs > 0 {
        bib_note.push_str(&format!(", {bibtex_runs} bibtex run(s)"));
    }
    if biber_runs > 0 {
        bib_note.push_str(&format!(", {biber_runs} biber run(s)"));
    }
    if index_runs > 0 {
        bib_note.push_str(&format!(", {index_runs} makeindex run(s)"));
    }
    // As latexmk does, a PDF produced despite TeX errors (recovered in an
    // explicitly requested nonstop/batch mode) is published, but the build
    // still fails.
    eprint!("{recovered_diagnostics}");
    let status = if last_pass_failed {
        "finished with TeX errors"
    } else {
        "OK"
    };
    // Watch mode reports each build in its own status line.
    let quiet_success = opt.watch && !last_pass_failed;
    match pages {
        Some(_) | None if quiet_success && pdf_path.is_file() => {}
        Some(n) => eprintln!(
            "texmk: build {status}: {} ({} page{}, {passes} {executed_engine} pass(es){bib_note})",
            pdf_path.display(),
            n,
            if n == 1 { "" } else { "s" }
        ),
        None if pdf_path.is_file() => eprintln!(
            "texmk: build {status}: {} (page count unknown, {passes} {executed_engine} pass(es){bib_note})",
            pdf_path.display()
        ),
        None => {
            eprintln!(
                "texmk: build FAILED: no output written to {}",
                pdf_path.display()
            );
            return 1;
        }
    }
    if let Some(sig) = last_signals {
        let unresolved = match (sig.undef_refs, sig.undef_cites) {
            (true, true) => Some("references and citations"),
            (true, false) => Some("references"),
            (false, true) => Some("citations"),
            (false, false) => None,
        };
        if let Some(kind) = unresolved {
            if log_path.is_file() {
                eprintln!(
                    "texmk: warning: unresolved {kind} remain; see {}",
                    log_path.display()
                );
            } else {
                eprintln!("texmk: warning: unresolved {kind} remain");
            }
        }
        for w in &sig.user_warnings {
            eprintln!("texmk: warning: {w}");
        }
    }
    if child_cache_hit && !opt.keep_intermediates && !opt.keep_logs {
        // A validated engine cache hit guarantees that private aux state did
        // not change. With no requested exports, refreshing its full artifact
        // snapshot and rewriting identical ownership metadata is pure overhead.
    } else {
        retain_requested(&retention, &mut manifest);
    }
    drop(lock);
    if last_pass_failed {
        if log_path.is_file() {
            eprintln!("texmk: transcript retained at {}", log_path.display());
        }
        return 1;
    }
    0
}

/// What the watch loop learns from a build beyond its exit status.
#[derive(Default)]
struct BuildInfo {
    /// Page count of the published PDF, when known.
    pages: Option<u32>,
    /// Set once the build has located its files; `None` when it failed
    /// during setup, before reading anything.
    inputs: Option<WatchInputs>,
}

/// The locations a build resolved, from which its dependencies are derived.
struct WatchInputs {
    /// Taken once the build's own preparation (EPS conversion) is done:
    /// a dependency modified after this may have been read stale.
    started: SystemTime,
    source: PathBuf,
    source_dir: PathBuf,
    aux_dir: PathBuf,
    /// Private cache root of every job; never a dependency.
    jobs_dir: PathBuf,
    job: String,
    pdf_path: PathBuf,
    synctex_path: PathBuf,
    manifest_path: PathBuf,
    /// The engine's private list of font files and failed lookups.
    dependency_file: PathBuf,
    /// EPS figures the driver converts for the engine.
    eps: EpsFigures,
}

/// `.bib`/`.bst` files (BibTeX) and data sources (Biber) that exist on disk.
fn bibliography_watch_files(
    aux: &str,
    bcf: &Path,
    aux_dir: &Path,
    source_dir: &Path,
) -> Vec<PathBuf> {
    let mut extra_roots = Vec::new();
    for env in ["TEXMFHOME", "TEXMFLOCAL"] {
        if let Ok(value) = std::env::var(env) {
            extra_roots.extend(std::env::split_paths(&value).filter(|path| path.is_dir()));
        }
    }
    let extra_refs: Vec<&Path> = extra_roots.iter().map(PathBuf::as_path).collect();
    let kpse = tex_kpse::Kpse::with_roots(source_dir, &extra_refs);
    let mut files: Vec<PathBuf> = aux_bibliography_dependencies(aux)
        .into_iter()
        .filter_map(|(name, format, extra_variable)| {
            bibliography_dependency_path(&kpse, aux_dir, source_dir, name, format)
                .or_else(|| extra_bibliography_dependency_path(name, format, extra_variable))
        })
        .collect();
    if let Ok(bytes) = std::fs::read(bcf) {
        if let Ok(names) = tex_biber::datasource_names(&String::from_utf8_lossy(&bytes)) {
            files.extend(
                names
                    .into_iter()
                    .filter_map(|name| biber_file(&kpse, &name, aux_dir, source_dir)),
            );
        }
    }
    files
}

/// Every path the last build read, or looked for and did not find: the main
/// file and whatever the engine's recorder (`.fls`) lists as `INPUT`
/// (`\input`/`\include` files, packages and classes, images), plus the
/// engine's private watch list: `FONT` files and `MISSING` lookups (a file
/// absent now would be read once it appears, so creating the file a failed
/// build asked for triggers the next one). Also watched are the
/// bibliography sources the auxiliary files name and a project-supplied
/// `<job>.bbl`. Excluded are the embedded archive, the private cache, and
/// everything the build writes (per the recorder and the ownership
/// manifest), so a build never triggers itself.
///
/// A build can stop before the engine rewrites the recorder; the paths
/// watched until then stay watched.
fn watch_dependencies(
    inputs: &WatchInputs,
    build_failed: bool,
    previous: &watch::Tracker,
) -> BTreeSet<PathBuf> {
    let mut read = BTreeSet::from([inputs.source.clone()]);
    let mut written = BTreeSet::from([inputs.pdf_path.clone(), inputs.synctex_path.clone()]);
    let recorder = artifact_path(&inputs.aux_dir, &inputs.job, ".fls");
    let recorder_current = std::fs::metadata(&recorder)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified >= inputs.started);
    if let Ok(Some(text)) = read_to_string_bounded(&recorder, MANIFEST_MAX_BYTES) {
        let mut cwd = inputs.source_dir.clone();
        for line in text.lines() {
            if let Some(directory) = line.strip_prefix("PWD ") {
                cwd = PathBuf::from(directory);
            } else if let Some(path) = line.strip_prefix("INPUT ") {
                read.insert(cwd.join(path));
            } else if let Some(path) = line.strip_prefix("OUTPUT ") {
                written.insert(cwd.join(path));
            }
        }
    }
    if let Ok(Some(text)) = read_to_string_bounded(&inputs.dependency_file, MANIFEST_MAX_BYTES) {
        let mut cwd = inputs.source_dir.clone();
        for line in text.lines() {
            if let Some(directory) = line.strip_prefix("PWD ") {
                cwd = PathBuf::from(directory);
            } else if let Some(path) = line
                .strip_prefix("FONT ")
                .or_else(|| line.strip_prefix("MISSING "))
            {
                read.insert(cwd.join(path));
            }
        }
    }
    let aux = read_aux_graph(
        &artifact_path(&inputs.aux_dir, &inputs.job, ".aux"),
        &inputs.aux_dir,
        &inputs.source_dir,
    );
    read.extend(bibliography_watch_files(
        &aux.combined,
        &artifact_path(&inputs.aux_dir, &inputs.job, ".bcf"),
        &inputs.aux_dir,
        &inputs.source_dir,
    ));
    read.insert(inputs.source_dir.join(format!("{}.bbl", inputs.job)));
    written.insert(artifact_path(&inputs.aux_dir, &inputs.job, ".bbl"));
    written.insert(artifact_path(&inputs.aux_dir, &inputs.job, ".blg"));
    if let Some(manifest) = read_manifest(&inputs.manifest_path) {
        written.insert(manifest.pdf);
        written.insert(manifest.synctex);
        written.extend(manifest.exports.into_keys());
        written.extend(manifest.aux_files.into_keys());
    }
    // The driver owns every conversion; it also rewrites `<name>.pdf` while that
    // is still a copy of the previous conversion.
    written.extend(inputs.eps.written.iter().cloned());
    written.extend(inputs.eps.sources.iter().map(|eps| eps_converted_path(eps)));
    // A path that does not exist is compared through its directory.
    let resolve = |path: &Path| -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| {
            match (
                path.parent().and_then(|parent| std::fs::canonicalize(parent).ok()),
                path.file_name(),
            ) {
                (Some(parent), Some(name)) => parent.join(name),
                _ => path.to_path_buf(),
            }
        })
    };
    let written: BTreeSet<PathBuf> = written.iter().map(|path| resolve(path)).collect();
    let read: BTreeSet<PathBuf> = read
        .iter()
        .filter(|path| !tex_kpse::embedded_tree::is_embedded_path(&path.to_string_lossy()))
        .map(|path| resolve(path))
        .collect();
    let mut watched: BTreeSet<PathBuf> = read
        .iter()
        .filter(|path| {
            let watchable = path.is_file()
                || (!path.exists() && path.parent().is_some_and(|parent| parent.is_dir()));
            watchable && !path.starts_with(&inputs.jobs_dir) && !written.contains(*path)
        })
        .cloned()
        .collect();
    // The engine reads the PDF the driver converted from an EPS figure, so the
    // figure itself is the dependency.
    for eps in &inputs.eps.sources {
        let converted = resolve(&eps_converted_path(eps));
        if read.contains(&converted) || read.contains(&resolve(&eps.with_extension("pdf"))) {
            watched.insert(resolve(eps));
        }
    }
    if build_failed && !recorder_current {
        watched.extend(previous.paths().map(Path::to_path_buf));
    }
    watched
}

/// `-pvc`: build, then rebuild whenever a dependency of the last build
/// changes, until Ctrl-C. A failed rebuild reports its errors and keeps
/// watching; only a build that fails before it can name any dependency (first
/// build, bad options or missing main file) ends the run with its status.
fn watch_main(opt: &Options) -> i32 {
    watch::install_interrupt_handler();
    let mut tracker = watch::Tracker::default();
    let mut first_build = true;
    loop {
        let mut info = BuildInfo::default();
        let timer = Instant::now();
        let code = build(opt, &mut info);
        let elapsed = timer.elapsed();
        if watch::interrupted() {
            return 0;
        }
        match &info.inputs {
            Some(inputs) => {
                let dependencies = watch_dependencies(inputs, code != 0, &tracker);
                tracker.replace(dependencies, inputs.started);
            }
            None if first_build => return code,
            None => {}
        }
        first_build = false;
        let pages = match info.pages {
            Some(1) => "1 page, ".to_string(),
            Some(count) => format!("{count} pages, "),
            None => String::new(),
        };
        eprintln!(
            "texmk: [{}] build {} ({pages}{:.2} s)",
            watch::clock(),
            if code == 0 { "OK" } else { "FAILED" },
            elapsed.as_secs_f64()
        );
        eprintln!(
            "texmk: watching {} files (Ctrl-C to stop)",
            tracker.present()
        );
        let Some(changed) = tracker.wait_for_change() else {
            return 0;
        };
        let mut names: Vec<String> = changed
            .iter()
            .take(3)
            .map(|path| {
                path.file_name()
                    .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
            })
            .collect();
        if changed.len() > 3 {
            names.push(format!("{} more", changed.len() - 3));
        }
        eprintln!("texmk: [{}] changed: {}", watch::clock(), names.join(", "));
    }
}

fn invoked_name() -> String {
    let arg0 = std::env::args_os()
        .next()
        .unwrap_or_else(|| OsString::from("texmk"));
    let name = Path::new(&arg0)
        .file_name()
        .unwrap_or_else(|| OsStr::new("texmk"))
        .to_string_lossy();
    name.strip_suffix(std::env::consts::EXE_SUFFIX)
        .unwrap_or(&name)
        .to_ascii_lowercase()
}

fn enable_embedded_resources_by_default() {
    std::env::set_var(HERMETIC_ENV, "1");
}

fn run_embedded_bibtex() -> ! {
    enable_embedded_resources_by_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "-v" | "-version" | "--version"))
    {
        println!("BibTeX 0.99d (TeXres {}; Rust)", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    std::process::exit(embedded_bibtex::run(&args, "1.0"));
}

fn run_embedded_makeindex() -> ! {
    enable_embedded_resources_by_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(tex_makeindex::run_cli(&args, &tex_makeindex::FsHost));
}

fn run_embedded_biber() -> ! {
    enable_embedded_resources_by_default();
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    std::process::exit(tex_biber::cli_main(&args));
}

fn run_embedded_engine(program: &str) {
    enable_embedded_resources_by_default();
    std::env::set_var("TEX_SUITE_PROGRAM_NAME", program);
    embedded_engine::main();
}

pub(crate) fn main() {
    match std::env::var(TEXMK_INTERNAL_MODE_ENV).as_deref() {
        Ok("engine") => {
            let program =
                std::env::var("TEX_SUITE_PROGRAM_NAME").unwrap_or_else(|_| "pdflatex".to_string());
            run_embedded_engine(&program);
            return;
        }
        Ok("bibtex") => run_embedded_bibtex(),
        Ok("biber") => run_embedded_biber(),
        Ok("latexdiff") => {
            let args: Vec<String> = std::env::args().skip(1).collect();
            std::process::exit(latexdiff::latexdiff_main(&args));
        }
        _ => {}
    }
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "latexdiff" {
        std::process::exit(latexdiff::latexdiff_main(&args[2..]));
    }
    match invoked_name().as_str() {
        "pdflatex" => run_embedded_engine("pdflatex"),
        "xelatex" => run_embedded_engine("xelatex"),
        "lualatex" => run_embedded_engine("lualatex"),
        "bibtex" | "tex-bibtex" => run_embedded_bibtex(),
        "biber" => run_embedded_biber(),
        "makeindex" => run_embedded_makeindex(),
        "latexdiff" => {
            let diff_args = if args.len() > 1 { &args[1..] } else { &[] };
            std::process::exit(latexdiff::latexdiff_main(diff_args));
        }
        _ => std::process::exit(real_main()),
    }
}

#[cfg(test)]
mod io_safety_tests {
    use super::*;
    use std::io::Cursor;

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!("texmk-{name}-{}-{nonce}", std::process::id())))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn garbage_collection_preserves_a_job_locked_before_creation() {
        let root = TempFile::new("gc-pending-job");
        std::fs::create_dir(&root.0).unwrap();
        let pending = root.0.join("0123456789abcdef");
        let current = root.0.join("fedcba9876543210");
        let lock = CacheLock::try_acquire(&pending.with_extension("lock"))
            .unwrap()
            .unwrap();
        std::fs::create_dir(&pending).unwrap();
        std::fs::write(pending.join("main.aux"), b"pending compilation").unwrap();

        gc_cache(&root.0, &current);
        let preserved = std::fs::read(pending.join("main.aux"))
            .is_ok_and(|bytes| bytes == b"pending compilation");
        drop(lock);
        gc_cache(&root.0, &current);
        let orphan_removed = !pending.exists();
        std::fs::remove_dir_all(&root.0).unwrap();

        assert!(
            preserved,
            "garbage collection removed a pending compilation"
        );
        assert!(
            orphan_removed,
            "unlocked incomplete jobs must remain reclaimable"
        );
    }

    #[test]
    fn live_build_lock_does_not_expire_during_long_compile() {
        let path = TempFile::new("live-build-lock");
        let lock = CacheLock::try_acquire(&path.0).unwrap().unwrap();
        OpenOptions::new()
            .write(true)
            .open(&path.0)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(24 * 60 * 60))
            .unwrap();

        assert!(CacheLock::try_acquire(&path.0).unwrap().is_none());
        drop(lock);
        assert!(CacheLock::try_acquire(&path.0).unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn waiter_on_a_released_lock_file_locks_the_current_file() {
        use std::os::unix::fs::MetadataExt;
        let path = TempFile::new("relinked-lock");
        let first = CacheLock::try_acquire(&path.0).unwrap().unwrap();
        let waiter = std::thread::spawn({
            let path = path.0.clone();
            move || CacheLock::acquire(&path).unwrap()
        });
        std::thread::sleep(Duration::from_millis(100));
        // The first holder unlinks its file on release; a waiter blocked on
        // that inode must not treat the orphan as the job lock.
        drop(first);
        let second = waiter.join().unwrap();
        let current = std::fs::metadata(&path.0).unwrap();
        let locked = second.file.metadata().unwrap();
        assert_eq!((locked.dev(), locked.ino()), (current.dev(), current.ino()));
        assert!(CacheLock::try_acquire(&path.0).unwrap().is_none());
    }

    #[test]
    fn bounded_capture_drains_input_and_keeps_head_and_tail() {
        let mut input = vec![b'A'; 16];
        input.extend(std::iter::repeat_n(b'B', 100));
        input.extend(std::iter::repeat_n(b'Z', 48));

        let output = capture_reader(Cursor::new(input), 64, 16, "stdout").unwrap();

        assert!(output.starts_with(&"A".repeat(16)));
        assert!(output.ends_with(&"Z".repeat(48)));
        assert!(output.contains("stdout truncated; omitted 100 bytes"));
    }

    #[test]
    fn signal_scanner_handles_split_phrases_and_unbounded_lines() {
        let mut scanner = SignalScanner::new("main");
        scanner.feed(b"No fi");
        scanner.feed(b"le ");
        let filler = vec![b'x'; IO_BUFFER_BYTES];
        for _ in 0..40 {
            scanner.feed(&filler);
            assert!(scanner.overlap.len() < scanner.max_pattern_len);
        }
        scanner.feed(b"main.bb");
        scanner.feed(b"l\nCita");
        scanner.feed(b"tion `key' is undef");
        scanner.feed(b"ined\nLabel(s) may have changed\n");
        let signals = scanner.finish();

        assert!(signals.bbl_missing);
        assert!(signals.undef_cites);
        assert!(signals.rerun);
    }

    #[test]
    fn preambles_requiring_a_unicode_engine_select_xelatex() {
        for source in [
            "\\documentclass{ctexart}\n\\begin{document}x\\end{document}\n",
            "\\documentclass[UTF8, fontset=fandol]{ctexbeamer}\n",
            "\\documentclass{article}\n\\usepackage{amsmath,amsfonts,ctex,enumitem}\n",
            "\\documentclass{article}\n\\usepackage[\n  no-math\n]{fontspec}\n",
            "\\documentclass{article}\\RequirePackage{unicode-math}\n",
            "\\documentclass{article}\n\\usepackage { xeCJK }\n",
            "\\documentclass{article}\n\\usepackage{polyglossia} % \\usepackage{x}\n",
        ] {
            assert!(preamble_requires_unicode_engine(source), "{source}");
        }
        for source in [
            "\\documentclass{article}\n% \\usepackage{fontspec}\n\\begin{document}\n",
            "\\documentclass{article}\n\\begin{document}\n\\usepackage{fontspec}\n",
            "\\documentclass{article}\\begin{document}\\usepackage{ctex}\n",
            "\\documentclass{article}\n\\usepackage{fontspecx,ctex-xecjk}\n",
            "\\documentclass{article}\n\\usepackagex{fontspec}\n",
            "\\documentclass{article}\n\\newcommand\\x{100\\% fontspec}\n",
            "\\documentclass{ctexartx}\n\\usepackage[ctex]{foo}\n",
        ] {
            assert!(!preamble_requires_unicode_engine(source), "{source}");
        }
        assert!(preamble_requires_unicode_engine(
            "\\documentclass{article}\n\\def\\p{100\\%}\\usepackage{fontspec}\n"
        ));
    }

    #[test]
    fn engine_and_transcript_box_warnings_print_once_with_one_severity() {
        let message = "Overfull \\hbox (12.0pt too wide) in paragraph at lines 3--4";
        let mut signals = scan_signals(&format!("warning: {message}\n"), "main");
        signals.merge(scan_signals(&format!("{message}\n"), "main"));
        assert_eq!(signals.user_warnings, [message]);
    }

    #[test]
    fn engine_detection_and_pdf_page_scan_cross_io_boundaries() {
        let source = TempFile::new("engine-detect.tex");
        let mut bytes = vec![b'x'; IO_BUFFER_BYTES - 4];
        bytes.extend_from_slice(b"fontspec\n\\begin{document}\n");
        std::fs::write(&source.0, bytes).unwrap();
        assert_eq!(detect_engine(&source.0), "pdflatex");
        std::fs::write(&source.0, b"\\begin{document}\nfontspec").unwrap();
        assert_eq!(detect_engine(&source.0), "pdflatex");

        let pdf = TempFile::new("pages.pdf");
        let mut bytes = vec![b'x'; IO_BUFFER_BYTES - 5];
        bytes.extend_from_slice(b"/Type /Pages\n/Count 37\n");
        std::fs::write(&pdf.0, bytes).unwrap();
        assert_eq!(pdf_pages(&pdf.0), Some(37));
    }

    #[test]
    fn streaming_file_hash_matches_the_previous_whole_file_hash() {
        let file = TempFile::new("streaming-hash.bin");
        let bytes: Vec<u8> = (0..IO_BUFFER_BYTES * 3 + 17)
            .map(|index| (index % 251) as u8)
            .collect();
        std::fs::write(&file.0, &bytes).unwrap();
        let mut expected = DefaultHasher::new();
        expected.write(&bytes);

        assert_eq!(file_hash(&file.0), (true, expected.finish()));
    }

    #[test]
    fn oversized_manifest_is_rejected_before_allocation() {
        let file = TempFile::new("oversized-manifest");
        let handle = std::fs::File::create(&file.0).unwrap();
        handle.set_len(MANIFEST_MAX_BYTES + 1).unwrap();

        assert!(read_to_string_bounded(&file.0, MANIFEST_MAX_BYTES)
            .unwrap()
            .is_none());
    }
}
