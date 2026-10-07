//! tex-makeindex: a port of `makeindex` 2.18 (TeX Live).
//!
//! Reads `\indexentry{key}{page}` entries, sorts them as makeindex does
//! (symbols, numbers, then letters; `!` sub-entries; `@` sort keys), merges
//! page numbers into runs and ranges, and writes the `.ind` file through the
//! style's templates, with makeindex's `.ilg` transcript. The modules follow
//! makeindex's sources: `scan` (`scanid.c`), `style` (`scanst.c`), `sort`
//! (`sortid.c`, `qsort.c`) and `gen` (`genind.c`).

mod gen;
mod locale;
mod scan;
mod sort;
mod stream;
mod style;

use std::io::Write;

pub use style::Style;

use gen::{generate, next_page, Layout};
use locale::Locale;
use scan::Scanner;
use sort::{qqsort, Collation};

const BANNER: &str = "This is makeindex, version 2.18 [TeX Live 2026] (TeXres).\n";
const USAGE: &str = "Usage: makeindex [-ilqrcgLT] [-s sty] [-o ind] [-t log] [-p num] [idx0 idx1 ...]\n";
/// `ARRAY_MAX`: the most input files.
const MAX_INPUTS: usize = 1024;
/// `STRING_MAX`: the longest file name.
const STRING_MAX: usize = 999;
/// `NUMBER_MAX`: room for the `-p` page.
const NUMBER_MAX: usize = 99;

/// Command-line switches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `-q`: nothing on the terminal.
    pub quiet: bool,
    /// `-i`: read standard input (too).
    pub stdin: bool,
    /// `-l`: ignore blanks when sorting.
    pub letter_ordering: bool,
    /// `-c`: compress blanks in keys.
    pub compress_blanks: bool,
    /// `-r`: no implicit page ranges.
    pub no_ranges: bool,
    /// `-g`: German word ordering.
    pub german: bool,
    /// `-L`: sort with the locale's collation.
    pub locale_sort: bool,
    /// `-T`: Thai ordering (implies `-L`).
    pub thai: bool,
    pub style: Option<String>,
    pub output: Option<String>,
    pub log: Option<String>,
    /// `-p`: a page number, or `even`, `odd`, `any` (the page after the last
    /// one of the document's `.log`).
    pub start_page: Option<String>,
    pub inputs: Vec<String>,
}

/// File system access, so that callers can resolve files and keep outputs
/// under their own control.
pub trait Host {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>>;
    fn write(&self, path: &str, bytes: &[u8]) -> std::io::Result<()>;
    /// Whether `path` names a readable file (`access(path, R_OK)`).
    fn exists(&self, path: &str) -> bool;
    /// Locates a style file as kpathsea's `ist` format would: the path as
    /// makeindex reports it, and the contents.
    fn find_style(&self, name: &str) -> Option<(String, Vec<u8>)>;
    fn read_stdin(&self) -> std::io::Result<Vec<u8>>;
}

/// File names kpathsea tries for a style `name`: with `.ist` added first.
pub fn style_candidates(name: &str) -> Vec<String> {
    if name.ends_with(".ist") { vec![name.to_string()] } else { vec![format!("{name}.ist"), name.to_string()] }
}

/// Whether kpathsea looks a name up as given instead of along a path.
pub fn is_explicit_path(name: &str) -> bool {
    std::path::Path::new(name).is_absolute() || name.starts_with("./") || name.starts_with("../")
}

/// The plain file system; style files are searched in `.` and the
/// `TEXINDEXSTYLE`/`INDEXSTYLE` paths.
pub struct FsHost;

impl Host for FsHost {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, bytes)
    }

    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).is_file()
    }

    fn find_style(&self, name: &str) -> Option<(String, Vec<u8>)> {
        let candidates = style_candidates(name);
        let read = |path: &str| std::fs::read(path).ok().filter(|_| std::path::Path::new(path).is_file());
        if is_explicit_path(name) {
            return candidates.into_iter().find_map(|path| read(&path).map(|bytes| (path, bytes)));
        }
        let mut directories = vec![".".to_string()];
        for variable in ["TEXINDEXSTYLE", "INDEXSTYLE"] {
            if let Some(value) = std::env::var_os(variable) {
                directories.extend(
                    std::env::split_paths(&value)
                        .filter(|path| !path.as_os_str().is_empty())
                        .map(|path| path.to_string_lossy().trim_end_matches('/').to_string()),
                );
            }
        }
        for directory in directories {
            for candidate in &candidates {
                let path = format!("{directory}/{candidate}");
                if let Some(bytes) = read(&path) {
                    return Some((path, bytes));
                }
            }
        }
        None
    }

    fn read_stdin(&self) -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// Parses makeindex's command line; the error is makeindex's message.
pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        let Some(flags) = arg.strip_prefix('-') else {
            if options.inputs.len() >= MAX_INPUTS {
                return Err(format!("Too many input files (max {MAX_INPUTS}).\n"));
            }
            options.inputs.push(arg.clone());
            continue;
        };
        if flags.is_empty() {
            // A lone `-` ends the command line.
            break;
        }
        for flag in flags.chars() {
            match flag {
                'i' => options.stdin = true,
                'l' => options.letter_ordering = true,
                'r' => options.no_ranges = true,
                'q' => options.quiet = true,
                'c' => options.compress_blanks = true,
                'g' => options.german = true,
                'L' => options.locale_sort = true,
                'T' => {
                    options.thai = true;
                    options.locale_sort = true;
                }
                's' | 'o' | 't' | 'p' => {
                    let Some(value) = args.get(index) else {
                        return Err(match flag {
                            's' => "Expected -s <stylefile>\n",
                            'o' => "Expected -o <ind>\n",
                            't' => "Expected -t <logfile>\n",
                            _ => "Expected -p <num>\n",
                        }
                        .to_string());
                    };
                    index += 1;
                    match flag {
                        's' => options.style = Some(value.clone()),
                        'o' => options.output = Some(value.clone()),
                        't' => options.log = Some(value.clone()),
                        _ => {
                            if value.len() >= NUMBER_MAX {
                                return Err("Page number too high\n".to_string());
                            }
                            options.start_page = Some(value.clone());
                        }
                    }
                }
                other => return Err(format!("Unknown option -{other}.\n")),
            }
        }
    }
    Ok(options)
}

/// The transcript (`ilg_fp`) and the terminal echo, with makeindex's
/// progress-dot state.
pub(crate) struct Transcript {
    ilg: Vec<u8>,
    /// The transcript is standard error itself.
    ilg_to_stderr: bool,
    verbose: bool,
    /// `idx_dot`: a progress dot ends the current transcript line.
    pub(crate) idx_dot: bool,
    /// `idx_dc`: progress steps since the last dot.
    pub(crate) idx_dc: usize,
}

impl Transcript {
    /// `MESSAGE`: transcript and, unless quiet, the terminal.
    pub(crate) fn message(&mut self, text: &[u8]) {
        if self.verbose {
            let _ = std::io::stderr().write_all(text);
        }
        self.ilg(text);
    }

    /// Transcript only.
    pub(crate) fn ilg(&mut self, text: &[u8]) {
        if self.ilg_to_stderr {
            let _ = std::io::stderr().write_all(text);
        } else {
            self.ilg.extend_from_slice(text);
        }
    }

    /// Diagnostics start on a line of their own after progress dots.
    pub(crate) fn error_line(&mut self) {
        if self.idx_dot {
            self.ilg(b"\n");
            self.idx_dot = false;
        }
    }

    /// `IDX_DOT`: one dot per `max` steps.
    pub(crate) fn dot(&mut self, max: usize) {
        self.idx_dot = true;
        if self.idx_dc == 0 {
            self.message(b".");
        }
        self.idx_dc += 1;
        if self.idx_dc == max {
            self.idx_dc = 0;
        }
    }
}

/// Runs makeindex as the command `makeindex args...`; returns the exit status.
pub fn run_cli(args: &[String], host: &dyn Host) -> i32 {
    match parse_args(args) {
        Ok(options) => run(&options, host),
        Err(message) => {
            eprint!("{message}{USAGE}");
            1
        }
    }
}

/// `check_idx`'s base name: the name without its extension.
fn base_name(name: &str) -> (String, bool) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && !name[dot + 1..].contains('/') => (name[..dot].to_string(), true),
        _ => (name.to_string(), false),
    }
}

/// `check_idx`: the file an input name stands for, or makeindex's error.
fn check_idx(name: &str, host: &dyn Host) -> Result<(String, String), String> {
    let (base, with_extension) = base_name(name);
    if base.len() >= STRING_MAX {
        return Err(format!("Index file name {base} too long (max {STRING_MAX}).\n"));
    }
    if host.exists(name) {
        return Ok((name.to_string(), base));
    }
    if with_extension {
        return Err(format!("Input index file {name} not found.\n"));
    }
    let with_idx = format!("{base}.idx");
    if host.exists(&with_idx) {
        Ok((with_idx, base))
    } else {
        Err(format!("Couldn't find input index file {base} nor {with_idx}.\n"))
    }
}

/// `find_pageno`: the page number of the last `[<digit>` in a TeX
/// transcript (the last page shipped out).
pub fn log_page_number(log: &[u8]) -> Option<Vec<u8>> {
    let n = log.len();
    if n < 2 {
        return None;
    }
    let found = (0..n - 1)
        .rev()
        .find(|&k| log[k] == b'[' && log[k + 1].is_ascii_digit())
        .or((log[0] == b'[').then_some(0))?;
    let mut pos = found + 1;
    while log.get(pos) == Some(&b' ') {
        pos += 1;
    }
    let mut page = vec![log.get(pos).copied().unwrap_or(0xff)];
    pos += 1;
    while let Some(&digit) = log.get(pos).filter(|byte| byte.is_ascii_digit()) {
        page.push(digit);
        pos += 1;
    }
    Some(page)
}

/// The files a run has opened, written when it ends.
struct Outputs {
    ind: Option<String>,
    ilg: Option<String>,
}

impl Outputs {
    /// What a fatal error leaves behind: the output index created (empty)
    /// and the transcript so far.
    fn abandon(&self, transcript: &Transcript, host: &dyn Host, message: &str) -> i32 {
        if let Some(ind) = &self.ind {
            let _ = host.write(ind, b"");
        }
        if let Some(ilg) = &self.ilg {
            let _ = host.write(ilg, &transcript.ilg);
        }
        eprint!("{message}{USAGE}");
        1
    }
}

/// Runs makeindex with parsed options; returns the exit status.
pub fn run(options: &Options, host: &dyn Host) -> i32 {
    let fatal = |message: String| {
        eprint!("{message}{USAGE}");
        1
    };
    let mut log =
        Transcript { ilg: Vec::new(), ilg_to_stderr: false, verbose: !options.quiet, idx_dot: true, idx_dc: 0 };

    // The command line: style file and input files.
    let mut style_file = match &options.style {
        Some(name) => match host.find_style(name) {
            Some(found) => Some(found),
            None => return fatal(format!("Index style file {name} not found.\n")),
        },
        None => None,
    };
    let mut inputs = Vec::with_capacity(options.inputs.len());
    for name in &options.inputs {
        match check_idx(name, host) {
            Ok(found) => inputs.push(found),
            Err(message) => return fatal(message),
        }
    }
    if inputs.len() == 1 && style_file.is_none() {
        // A style named after the only index file applies by itself.
        let mst = format!("{}.mst", inputs[0].1);
        if host.exists(&mst) {
            match host.find_style(&mst) {
                Some(found) => style_file = Some(found),
                None => return fatal(format!("Index style file {mst} not found.\n")),
            }
        }
    }
    let even_odd = match options.start_page.as_deref() {
        Some("even") => 2,
        Some("odd") => 1,
        Some("any") => 0,
        _ => -1,
    };
    let mut start_page = options.start_page.as_ref().map(|page| page.as_bytes().to_vec());

    let mut style = Style::default();
    let mut outputs = Outputs { ind: None, ilg: None };
    let mut sources: Vec<(String, Vec<u8>)> = Vec::new();
    let ind_name;
    let ilg_name;
    if let Some((_, base)) = inputs.first() {
        ind_name = options.output.clone().unwrap_or_else(|| format!("{base}.ind"));
        ilg_name = options.log.clone().unwrap_or_else(|| format!("{base}.ilg"));
        outputs.ind = Some(ind_name.clone());
        outputs.ilg = Some(ilg_name.clone());
        if even_odd >= 0 {
            let log_name = format!("{base}.log");
            let Ok(bytes) = host.read(&log_name) else {
                return outputs.abandon(&log, host, &format!("Source log file {log_name} not found.\n"));
            };
            start_page = log_page_number(&bytes);
            if start_page.is_none() {
                log.ilg(format!("Couldn't find any page number in {log_name}...ignored\n").as_bytes());
            }
        }
        log.message(BANNER.as_bytes());
        if let Some((path, bytes)) = &style_file {
            style.scan(bytes, path, &mut log);
        }
        if options.german && style.quote == b'"' {
            let message = "Option -g invalid, quote character must be different from '\"'.\n";
            return outputs.abandon(&log, host, message);
        }
        for (path, _) in &inputs {
            match host.read(path) {
                Ok(bytes) => sources.push((path.clone(), bytes)),
                Err(_) => return outputs.abandon(&log, host, &format!("Input index file {path} not found.\n")),
            }
        }
    } else {
        ind_name = options.output.clone().unwrap_or_else(|| "stdout".to_string());
        outputs.ind = options.output.clone();
        ilg_name = match &options.log {
            Some(name) => name.clone(),
            None => {
                // The transcript goes to the terminal, once.
                log.ilg_to_stderr = true;
                log.verbose = false;
                "stderr".to_string()
            }
        };
        outputs.ilg = options.log.clone();
        if let Some((path, bytes)) = &style_file {
            style.scan(bytes, path, &mut log);
        }
        if options.german && style.quote == b'"' {
            let message = "Option -g ignored, quote character must be different from '\"'.\n";
            return outputs.abandon(&log, host, message);
        }
        log.message(BANNER.as_bytes());
    }
    if options.stdin || inputs.is_empty() {
        sources.push(("stdin".to_string(), host.read_stdin().unwrap_or_default()));
    }
    if even_odd >= 0 {
        start_page = start_page.map(|page| next_page(&page, even_odd));
    }

    // Scan.
    let mut scanner = Scanner::new(&style, options.compress_blanks, options.german);
    for (index, (name, bytes)) in sources.iter().enumerate() {
        scanner.scan_file(bytes, index, name, &mut log);
    }
    let accepted = scanner.total - scanner.rejected;
    if sources.len() > 1 {
        let (files, rejected) = (sources.len(), scanner.rejected);
        log.message(
            format!("Overall {files} files read ({accepted} entries accepted, {rejected} rejected).\n").as_bytes(),
        );
    }
    let mut entries = scanner.entries;
    let names: Vec<String> = sources.into_iter().map(|(name, _)| name).collect();

    let mut ind_bytes = Vec::new();
    if accepted > 0 {
        if entries.is_empty() {
            return outputs.abandon(&log, host, "No valid index entries collected.\n");
        }
        // The environment's locale: collation for -L, case of group letters.
        let locale = Locale::from_env();
        log.message(b"Sorting entries...");
        log.idx_dc = 0;
        let collation = Collation {
            letter_ordering: options.letter_ordering,
            german_sort: options.german,
            locale: if options.locale_sort { locale.as_ref() } else { None },
            range_open: style.range_open,
            range_close: style.range_close,
        };
        let mut keys: Vec<usize> = (0..entries.len()).collect();
        let mut comparisons = 0u64;
        qqsort(&mut keys, &mut |a, b| {
            comparisons += 1;
            log.dot(sort::CMP_MAX);
            collation.compare(&mut entries, a, b)
        });
        log.message(format!("done ({comparisons} comparisons).\n").as_bytes());
        let sorted: Vec<scan::Entry> = keys.iter().map(|&index| entries[index].clone()).collect();

        let layout = Layout {
            style: &style,
            merge_page: !options.no_ranges,
            german_sort: options.german,
            thai_sort: options.thai,
            start_page,
            input_names: &names,
            output_name: &ind_name,
            locale: locale.as_ref(),
        };
        ind_bytes = generate(&sorted, &layout, &mut log).bytes;
        log.message(format!("Output written in {ind_name}.\n").as_bytes());
    } else {
        log.message(format!("Nothing written in {ind_name}.\n").as_bytes());
    }
    log.message(format!("Transcript written in {ilg_name}.\n").as_bytes());

    match &outputs.ind {
        Some(path) => {
            if host.write(path, &ind_bytes).is_err() {
                return fatal(format!("Can't create output index file {path}.\n"));
            }
        }
        None => {
            let _ = std::io::stdout().write_all(&ind_bytes);
        }
    }
    if let Some(path) = &outputs.ilg {
        if host.write(path, &log.ilg).is_err() {
            return fatal(format!("Can't create transcript file {path}.\n"));
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_page_follows_insert_page() {
        assert_eq!(next_page(b"12", 0), b"13");
        assert_eq!(next_page(b"12", 1), b"13");
        assert_eq!(next_page(b"12", 2), b"14");
        assert_eq!(next_page(b"9", 0), b"10");
        assert_eq!(next_page(b"a1b", 0), b"a1b1");
    }
}
