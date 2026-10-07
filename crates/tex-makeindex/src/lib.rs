//! tex-makeindex: a port of `makeindex` 2.18 (TeX Live).
//!
//! Reads `\indexentry{key}{page}` lines, sorts them the way makeindex does
//! (symbols, numbers, then letters; `!` sub-entries; `@` sort keys), merges
//! page numbers into runs and ranges, and writes the `.ind` file through the
//! style's templates. The `.ilg` transcript is written too.

mod gen;
mod page;
mod scan;
mod sort;
mod style;

use std::cell::Cell;
use std::cmp::Ordering;

pub use style::Style;

use gen::{generate, Layout};
use page::PageParser;
use scan::{scan_file, Entry};
use sort::{compare_entries, order_same_page, Collation};

const BANNER: &str = "This is makeindex, version 2.18 [TeX Live 2026] (TeXres)";
const USAGE: &str =
    "Usage: makeindex [-ilqrcgLT] [-s sty] [-o ind] [-t log] [-p num] [idx0 idx1 ...]";

/// Command-line switches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub quiet: bool,
    pub stdin: bool,
    /// `-l`: ignore blanks when sorting.
    pub letter_ordering: bool,
    /// `-c`: compress blanks in keys.
    pub compress_blanks: bool,
    /// `-r`: no implicit page ranges.
    pub no_ranges: bool,
    /// `-g`: German word ordering.
    pub german: bool,
    pub style: Option<String>,
    pub output: Option<String>,
    pub log: Option<String>,
    pub start_page: Option<String>,
    pub inputs: Vec<String>,
}

/// File system access, so that callers can resolve style files and keep
/// outputs under their own control.
pub trait Host {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>>;
    fn write(&self, path: &str, bytes: &[u8]) -> std::io::Result<()>;
    fn exists(&self, path: &str) -> bool;
    /// Locates a style file: its path (for messages) and contents.
    fn find_style(&self, name: &str) -> Option<(String, Vec<u8>)>;
    fn read_stdin(&self) -> std::io::Result<Vec<u8>>;
}

/// The plain file system; style files are searched in `.`, `INDEXSTYLE` and
/// `TEXINPUTS`.
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
        let mut directories = vec![std::path::PathBuf::from(".")];
        for variable in ["INDEXSTYLE", "TEXINPUTS"] {
            if let Some(value) = std::env::var_os(variable) {
                directories.extend(
                    std::env::split_paths(&value)
                        .filter(|path| !path.as_os_str().is_empty()),
                );
            }
        }
        for directory in directories {
            let trimmed = directory.to_string_lossy().trim_end_matches('/').to_string();
            let directory = std::path::PathBuf::from(if trimmed.is_empty() { "/".into() } else { trimmed });
            for candidate in [directory.join(name), directory.join(format!("{name}.ist"))] {
                if candidate.is_file() {
                    if let Ok(bytes) = std::fs::read(&candidate) {
                        return Some((candidate.to_string_lossy().into_owned(), bytes));
                    }
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

/// Parses makeindex's command line.
pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        let Some(flags) = arg.strip_prefix('-').filter(|flags| !flags.is_empty()) else {
            options.inputs.push(arg.clone());
            continue;
        };
        for flag in flags.chars() {
            match flag {
                'i' => options.stdin = true,
                'l' => options.letter_ordering = true,
                'q' => options.quiet = true,
                'r' => options.no_ranges = true,
                'c' => options.compress_blanks = true,
                'g' => options.german = true,
                'L' | 'T' => {}
                's' | 'o' | 't' | 'p' => {
                    let Some(value) = args.get(index) else {
                        let what = match flag {
                            's' => "-s <stylefile>",
                            'o' => "-o <ind>",
                            't' => "-t <logfile>",
                            _ => "-p <num>",
                        };
                        return Err(format!("Expected {what}"));
                    };
                    index += 1;
                    let value = Some(value.clone());
                    match flag {
                        's' => options.style = value,
                        'o' => options.output = value,
                        't' => options.log = value,
                        _ => options.start_page = value,
                    }
                }
                other => return Err(format!("Unknown option -{other}.")),
            }
        }
    }
    Ok(options)
}

/// Transcript: collected for the `.ilg` file and echoed unless quiet.
struct Transcript {
    text: String,
    quiet: bool,
}

impl Transcript {
    fn say(&mut self, text: &str) {
        self.text.push_str(text);
        if !self.quiet {
            eprint!("{text}");
        }
    }

    /// A diagnostic always starts on a fresh line.
    fn diagnostic(&mut self, text: &str) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.say("\n");
        }
        self.say(text);
    }
}

/// Runs makeindex as the command `makeindex args...`; returns the exit status.
pub fn run_cli(args: &[String], host: &dyn Host) -> i32 {
    match parse_args(args) {
        Ok(options) => run(&options, host),
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            1
        }
    }
}

fn replace_extension(name: &str, extension: &str) -> String {
    let file_start = name.rfind('/').map_or(0, |at| at + 1);
    match name[file_start..].rfind('.') {
        Some(dot) => format!("{}{extension}", &name[..file_start + dot]),
        None => format!("{name}{extension}"),
    }
}

/// Runs makeindex with parsed options; returns the exit status.
pub fn run(options: &Options, host: &dyn Host) -> i32 {
    let mut transcript = Transcript { text: String::new(), quiet: options.quiet };
    transcript.say(&format!("{BANNER}.\n"));

    let mut style = Style::default();
    let mut styled = false;
    if let Some(name) = &options.style {
        let Some((path, bytes)) = host.find_style(name) else {
            eprintln!("Index style file {name} not found.\n{USAGE}");
            return 1;
        };
        transcript.say(&format!("Scanning style file {path}"));
        let scan = style.scan(&bytes, &path);
        styled = true;
        for message in &scan.messages {
            transcript.diagnostic(message);
        }
        transcript.say(&format!(
            "{}done ({} attributes redefined, {} ignored).\n",
            ".".repeat(if scan.messages.is_empty() { 3 } else { 1 }),
            scan.redefined,
            scan.ignored
        ));
    }
    if options.german && style.quote == b'"' {
        eprintln!("Option -g invalid, quote character must be different from '\"'.\n{USAGE}");
        return 1;
    }

    // Input files.
    let mut inputs: Vec<(String, Vec<u8>)> = Vec::new();
    if options.inputs.is_empty() || options.stdin {
        match host.read_stdin() {
            Ok(bytes) => inputs.push(("stdin".to_string(), bytes)),
            Err(error) => {
                eprintln!("cannot read standard input: {error}");
                return 1;
            }
        }
    }
    for name in &options.inputs {
        let path = if host.exists(name) {
            name.clone()
        } else {
            let with_extension = format!("{name}.idx");
            if host.exists(&with_extension) {
                with_extension
            } else if name.ends_with(".idx") {
                eprintln!("Input index file {name} not found.\n{USAGE}");
                return 1;
            } else {
                eprintln!("Couldn't find input index file {name} nor {with_extension}.\n{USAGE}");
                return 1;
            }
        };
        match host.read(&path) {
            Ok(bytes) => inputs.push((path, bytes)),
            Err(_) => {
                eprintln!("Input index file {path} not found.\n{USAGE}");
                return 1;
            }
        }
    }
    let to_stdout = options.inputs.is_empty() && options.output.is_none();
    let base = options.inputs.first().map(|name| inputs.iter().find(|(path, _)| {
        path == name || path == &format!("{name}.idx")
    }).map_or(name.clone(), |(path, _)| path.clone()));
    let output_name = match (&options.output, &base) {
        (Some(name), _) => name.clone(),
        (None, Some(base)) => replace_extension(base, ".ind"),
        (None, None) => "stdout".to_string(),
    };
    let log_name = match (&options.log, &base) {
        (Some(name), _) => name.clone(),
        (None, Some(base)) => replace_extension(base, ".ilg"),
        (None, None) => "stderr".to_string(),
    };

    // Scan.
    let mut parser = PageParser::new(&style, styled);
    let mut entries: Vec<Entry> = Vec::new();
    let mut accepted = 0;
    let mut rejected = 0;
    let names: Vec<String> = inputs.iter().map(|(name, _)| name.clone()).collect();
    for (index, (name, bytes)) in inputs.iter().enumerate() {
        transcript.say(&format!("Scanning input file {name}..."));
        let result = {
            let mut on_message = |message: String| {
                if message == "." {
                    transcript.say(".");
                } else {
                    transcript.diagnostic(&message);
                }
            };
            scan_file(bytes, index, name, &style, options, &mut parser, &mut entries, &mut on_message)
        };
        accepted += result.accepted;
        rejected += result.rejected;
        if result.accepted > 0 && transcript.text.ends_with("...") {
            transcript.say(".");
        }
        transcript.say(&format!(
            "done ({} entries accepted, {} rejected).\n",
            result.accepted, result.rejected
        ));
    }
    if inputs.len() > 1 {
        transcript.say(&format!(
            "Overall {} files read ({accepted} entries accepted, {rejected} rejected).\n",
            inputs.len()
        ));
    }

    let output_bytes;
    let mut written = true;
    if entries.is_empty() {
        transcript.say("No valid index entries collected.\n");
        transcript.say(&format!("Nothing written in {output_name}.\n"));
        output_bytes = Vec::new();
        written = false;
    } else {
        // Sort.
        transcript.say("Sorting entries...");
        let collation = Collation {
            letter_ordering: options.letter_ordering,
            german: options.german,
            quote: style.quote,
        };
        let comparisons = Cell::new(0usize);
        entries.sort_by(|a, b| {
            comparisons.set(comparisons.get() + 1);
            compare_entries(a, b, &collation)
        });
        order_same_page(&mut entries, &collation);
        order_same_page(&mut entries, &collation);
        // Entries that repeat each other exactly are merged.
        let mut unique: Vec<Entry> = Vec::with_capacity(entries.len());
        for entry in entries {
            let repeats = unique.last().is_some_and(|last| {
                compare_entries(last, &entry, &collation) == Ordering::Equal
                    && last.encap == entry.encap
                    && last.range == entry.range
            });
            if !repeats {
                unique.push(entry);
            }
        }
        let dots = if comparisons.get() > 0 { "." } else { "" };
        transcript.say(&format!("{dots}done ({} comparisons).\n", comparisons.get()));

        // Generate.
        transcript.say(&format!("Generating output file {output_name}..."));
        let layout = Layout {
            style: &style,
            no_ranges: options.no_ranges,
            start_page: options.start_page.as_deref().map(str::as_bytes),
            input_names: &names,
            output_name: &output_name,
        };
        let mut log = Vec::new();
        let output = generate(&unique, &layout, &mut |message| log.push(message));
        if log.is_empty() {
            transcript.say(".");
        }
        for message in &log {
            transcript.diagnostic(message);
        }
        transcript.say(&format!(
            "done ({} lines written, {} warning{}).\n",
            output.lines,
            output.warnings,
            if output.warnings == 1 { "" } else { "s" }
        ));
        output_bytes = output.bytes;
    }
    let wrote = if to_stdout {
        use std::io::Write;
        std::io::stdout().write_all(&output_bytes).map_err(|e| (output_name.clone(), e))
    } else {
        host.write(&output_name, &output_bytes).map_err(|e| (output_name.clone(), e))
    };
    if let Err((name, _)) = wrote {
        eprintln!("Can't create output index file {name}.\n{USAGE}");
        return 1;
    }
    if written {
        transcript.say(&format!("Output written in {output_name}.\n"));
    }
    transcript.say(&format!("Transcript written in {log_name}.\n"));
    if log_name != "stderr" {
        if host.write(&log_name, transcript.text.as_bytes()).is_err() {
            eprintln!("Can't create transcript file {log_name}.\n{USAGE}");
            return 1;
        }
    }
    0
}
