//! `texres fmt`: command-line front end of the formatter.
//!
//! Exit status: 0 when everything is formatted (or was formatted now), 1 when
//! `--check` found files that would change or a file could not be read,
//! formatted or written, 2 for a bad command line.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::bib::format_bib;
use crate::config::{find_config, Config};
use crate::diff::unified_diff;
use crate::format::{format_source, Extras, SourceKind};

const USAGE: &str = "usage: texres fmt [options] [FILE|DIR ...]
Formats LaTeX sources and BibTeX databases in place. Directories are
searched recursively for .tex, .sty, .cls, .ltx and .bib files. With no
files, or with -, reads standard input and writes the result to standard
output.

  --check                 change nothing; list the files that would change
                          and exit with status 1 if there are any
  --diff                  change nothing; print what would change as a
                          unified diff (with --check: exit 1 if non-empty)
  --stdin, -              read standard input, write standard output
  --stdin-filename PATH   with standard input: find settings and project
                          definitions as if the text were in PATH
  --config FILE           use these settings instead of .texresfmt.toml
  --print-config          print the settings in effect as TOML and exit
  -h, --help              this text

Settings are read from the nearest .texresfmt.toml in the file's directory
or above it. Exit status: 0 formatted, 1 --check found changes or a file
failed, 2 bad command line.";

/// LaTeX sources: formatted when walking directories, and read for
/// project definitions.
const SOURCE_EXTENSIONS: &[&str] = &["tex", "sty", "cls", "ltx"];
/// BibTeX databases, formatted with their own rules.
const BIB_EXTENSIONS: &[&str] = &["bib"];
/// Files that look like TeX but are not sources the formatter understands.
const REFUSED_EXTENSIONS: &[&str] = &["bbl", "dtx", "ins", "bst"];
/// Sibling files larger than this are not scanned for definitions.
const MAX_SCAN_BYTES: u64 = 8 << 20;

struct Options {
    check: bool,
    diff: bool,
    stdin: bool,
    stdin_filename: Option<PathBuf>,
    config: Option<PathBuf>,
    print_config: bool,
    paths: Vec<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options {
        check: false,
        diff: false,
        stdin: false,
        stdin_filename: None,
        config: None,
        print_config: false,
        paths: Vec::new(),
    };
    let mut only_paths = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if only_paths {
            options.paths.push(PathBuf::from(arg));
            continue;
        }
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| iter.next().cloned())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match key {
            "-h" | "--help" => return Ok(None),
            "--check" => options.check = true,
            "--diff" => options.diff = true,
            "--stdin" | "-" => options.stdin = true,
            "--stdin-filename" => {
                options.stdin_filename = Some(PathBuf::from(value("--stdin-filename")?))
            }
            "--config" => options.config = Some(PathBuf::from(value("--config")?)),
            "--print-config" => options.print_config = true,
            "--" => only_paths = true,
            _ if key.starts_with('-') && key.len() > 1 => {
                return Err(format!("unknown option {arg}"))
            }
            _ => options.paths.push(PathBuf::from(arg)),
        }
    }
    if options.stdin && !options.paths.is_empty() {
        return Err("standard input cannot be combined with file arguments".to_string());
    }
    Ok(Some(options))
}

/// Runs `texres fmt` with the arguments after `fmt`; returns the exit status.
pub fn run(args: &[String]) -> i32 {
    let mut options = match parse_args(args) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{USAGE}");
            return 0;
        }
        Err(message) => {
            eprintln!("texres fmt: {message}\n{USAGE}");
            return 2;
        }
    };
    let explicit = match &options.config {
        Some(path) => match Config::load(path) {
            Ok(config) => Some(config),
            Err(message) => {
                eprintln!("texres fmt: {message}");
                return 1;
            }
        },
        None => None,
    };
    if options.print_config {
        let config = match explicit {
            Some(config) => config,
            None => {
                let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                match find_config(&dir).map(|p| Config::load(&p)).transpose() {
                    Ok(config) => config.unwrap_or_default(),
                    Err(message) => {
                        eprintln!("texres fmt: {message}");
                        return 1;
                    }
                }
            }
        };
        print!("{}", config.to_toml());
        return 0;
    }
    if options.paths.is_empty() && !options.stdin {
        if std::io::stdin().is_terminal() {
            eprintln!("texres fmt: no input files (use - to read standard input)\n{USAGE}");
            return 2;
        }
        options.stdin = true;
    }
    let mut configs = ConfigCache {
        explicit,
        loaded: HashMap::new(),
    };
    if options.stdin {
        run_stdin(&options, &mut configs)
    } else {
        run_files(&options, &mut configs)
    }
}

struct ConfigCache {
    explicit: Option<Config>,
    loaded: HashMap<PathBuf, Config>,
}

impl ConfigCache {
    fn for_dir(&mut self, dir: &Path) -> Result<Config, String> {
        if let Some(config) = &self.explicit {
            return Ok(config.clone());
        }
        let Some(path) = find_config(dir) else {
            return Ok(Config::default());
        };
        if let Some(config) = self.loaded.get(&path) {
            return Ok(config.clone());
        }
        let config = Config::load(&path)?;
        self.loaded.insert(path, config.clone());
        Ok(config)
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn parent_dir(path: &Path) -> PathBuf {
    let absolute = absolute(path);
    absolute.parent().map(Path::to_path_buf).unwrap_or(absolute)
}

fn has_extension(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| list.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Decodes a source file: UTF-8, or else Latin-1 (one char per byte), so
/// 8-bit files survive unchanged.
fn decode(bytes: Vec<u8>) -> (String, bool) {
    match String::from_utf8(bytes) {
        Ok(text) => (text, true),
        Err(err) => (
            err.into_bytes().iter().map(|&b| char::from(b)).collect(),
            false,
        ),
    }
}

fn encode(text: &str, utf8: bool) -> Vec<u8> {
    if utf8 {
        text.as_bytes().to_vec()
    } else {
        // Only characters from the input and ASCII blanks can appear.
        text.chars().map(|c| c as u32 as u8).collect()
    }
}

/// Reads definitions of verbatim environments and commands from the given
/// texts and from the TeX sources next to them, then the project's own
/// classes, packages and input files they name (for the sectioning rule).
fn project_extras<'a>(
    texts: impl Iterator<Item = &'a str>,
    dirs: &BTreeSet<PathBuf>,
    skip: &BTreeSet<PathBuf>,
) -> Extras {
    let mut extras = Extras::default();
    for text in texts {
        extras.scan(text);
    }
    let mut scanned = skip.clone();
    let mut scan_file = |path: &Path, extras: &mut Extras| {
        if !scanned.insert(path.to_path_buf()) {
            return;
        }
        let small = std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_SCAN_BYTES);
        if small {
            if let Ok(bytes) = std::fs::read(path) {
                extras.scan(&decode(bytes).0);
            }
        }
    };
    // Entries of the listed directories, so that names are looked up
    // without a system call per directory and candidate.
    let mut listed: HashSet<PathBuf> = HashSet::new();
    let mut subdirs: HashSet<PathBuf> = HashSet::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let (file, sub) = if kind.is_symlink() {
                (path.is_file(), path.is_dir())
            } else {
                (kind.is_file(), kind.is_dir())
            };
            if sub {
                subdirs.insert(path);
            } else if file {
                if has_extension(&path, SOURCE_EXTENSIONS) {
                    scan_file(&path, &mut extras);
                }
                listed.insert(path);
            }
        }
    }
    let mut elsewhere: HashMap<PathBuf, bool> = HashMap::new();
    let mut exists = |dir: &Path, name: &str| -> Option<PathBuf> {
        let path = dir.join(name);
        let mut parts = Path::new(name).components();
        let found = match (parts.next(), parts.next()) {
            (Some(Component::Normal(_)), None) => listed.contains(&path),
            // `sub/file`: only where `sub` exists.
            (Some(Component::Normal(first)), Some(_)) if !subdirs.contains(&dir.join(first)) => {
                false
            }
            _ => *elsewhere
                .entry(path.clone())
                .or_insert_with(|| path.is_file()),
        };
        found.then_some(path)
    };
    loop {
        let wanted = extras.sections.wanted();
        if wanted.is_empty() {
            break;
        }
        for (kind, name) in wanted {
            let candidates = kind.candidates(&name);
            let found = dirs
                .iter()
                .find_map(|dir| candidates.iter().find_map(|c| exists(dir, c)));
            if let Some(path) = &found {
                scan_file(&absolute(path), &mut extras);
            }
            extras.sections.resolve(kind, name, found.is_some());
        }
    }
    extras
}

fn run_stdin(options: &Options, configs: &mut ConfigCache) -> i32 {
    let mut bytes = Vec::new();
    if let Err(err) = std::io::stdin().read_to_end(&mut bytes) {
        eprintln!("texres fmt: cannot read standard input: {err}");
        return 1;
    }
    let (text, utf8) = decode(bytes);
    let dir = match &options.stdin_filename {
        Some(path) => parent_dir(path),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let config = match configs.for_dir(&dir) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("texres fmt: {message}");
            return 1;
        }
    };
    let is_bib = options
        .stdin_filename
        .as_deref()
        .is_some_and(|p| has_extension(p, BIB_EXTENSIONS));
    let result = if is_bib {
        format_bib(&text, &config).map_err(|e| e.to_string())
    } else {
        let kind = options
            .stdin_filename
            .as_deref()
            .map_or(SourceKind::Document, SourceKind::of);
        let skip: BTreeSet<PathBuf> = options.stdin_filename.iter().map(|p| absolute(p)).collect();
        let extras = project_extras(
            std::iter::once(text.as_str()),
            &BTreeSet::from([dir]),
            &skip,
        );
        format_source(&text, &config, &extras, kind).map_err(|e| e.to_string())
    };
    let formatted = match result {
        Ok(formatted) => formatted,
        Err(err) => {
            eprintln!("texres fmt: <stdin>: {err}");
            return 1;
        }
    };
    let name = options
        .stdin_filename
        .as_ref()
        .map_or_else(|| "<stdin>".to_string(), |p| p.display().to_string());
    let mut stdout = std::io::stdout().lock();
    if options.diff {
        let _ = stdout.write_all(unified_diff(&text, &formatted, &name, &name).as_bytes());
    } else if !options.check {
        let _ = stdout.write_all(&encode(&formatted, utf8));
    }
    if options.check && formatted != text {
        return 1;
    }
    0
}

/// Collects the files to format: named files, and sources under named
/// directories (hidden entries and symbolic links skipped), sorted.
fn collect_files(paths: &[PathBuf], status: &mut i32) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                walk(&path, out)?;
            } else if kind.is_file()
                && (has_extension(&path, SOURCE_EXTENSIONS) || has_extension(&path, BIB_EXTENSIONS))
            {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            if let Err(err) = walk(path, &mut files) {
                eprintln!("texres fmt: {}: {err}", path.display());
                *status = 1;
            }
        } else if has_extension(path, REFUSED_EXTENSIONS) {
            eprintln!(
                "texres fmt: {}: not a LaTeX source; skipped",
                path.display()
            );
            *status = 1;
        } else {
            files.push(path.clone());
        }
    }
    files
}

fn run_files(options: &Options, configs: &mut ConfigCache) -> i32 {
    let mut status = 0;
    let files = collect_files(&options.paths, &mut status);
    let mut sources: Vec<(PathBuf, String, bool)> = Vec::with_capacity(files.len());
    for path in files {
        match std::fs::read(&path) {
            Ok(bytes) => {
                let (text, utf8) = decode(bytes);
                sources.push((path, text, utf8));
            }
            Err(err) => {
                eprintln!("texres fmt: {}: {err}", path.display());
                status = 1;
            }
        }
    }
    let is_bib = |path: &Path| has_extension(path, BIB_EXTENSIONS);
    let tex: Vec<&(PathBuf, String, bool)> = sources.iter().filter(|s| !is_bib(&s.0)).collect();
    let extras = if tex.is_empty() {
        Extras::default()
    } else {
        let dirs: BTreeSet<PathBuf> = tex.iter().map(|(p, _, _)| parent_dir(p)).collect();
        let skip: BTreeSet<PathBuf> = tex.iter().map(|(p, _, _)| absolute(p)).collect();
        project_extras(tex.iter().map(|(_, t, _)| t.as_str()), &dirs, &skip)
    };
    // Settings are looked up in order (and cached); formatting, the bulk of
    // the work, runs on all cores; results are reported in order.
    let configs: Vec<Result<Config, String>> = sources
        .iter()
        .map(|(path, _, _)| configs.for_dir(&parent_dir(path)))
        .collect();
    let results = parallel_map(&sources, |index, (path, text, _)| {
        let config = configs[index].as_ref().map_err(Clone::clone)?;
        if is_bib(path) {
            format_bib(text, config).map_err(|e| format!("{}: {e}", path.display()))
        } else {
            format_source(text, config, &extras, SourceKind::of(path))
                .map_err(|e| format!("{}: {e}", path.display()))
        }
    });
    let mut stdout = std::io::stdout().lock();
    for ((path, text, utf8), result) in sources.iter().zip(results) {
        let formatted = match result {
            Ok(formatted) => formatted,
            Err(message) => {
                eprintln!("texres fmt: {message}");
                status = 1;
                continue;
            }
        };
        if formatted == *text {
            continue;
        }
        if options.diff {
            let name = path.display().to_string();
            let _ = stdout.write_all(unified_diff(text, &formatted, &name, &name).as_bytes());
        } else if options.check {
            let _ = writeln!(stdout, "{}", path.display());
        }
        if options.check {
            status = 1;
        } else if !options.diff {
            if let Err(err) = std::fs::write(path, encode(&formatted, *utf8)) {
                eprintln!("texres fmt: {}: {err}", path.display());
                status = 1;
            }
        }
    }
    status
}

/// `f` applied to every item, on as many threads as there are cores.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(usize, &T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(items.len());
    if threads <= 1 {
        return items
            .iter()
            .enumerate()
            .map(|(i, item)| f(i, item))
            .collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = items.iter().map(|_| None).collect();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            return done;
                        };
                        done.push((i, f(i, item)));
                    }
                })
            })
            .collect();
        for worker in workers {
            for (i, result) in worker.join().expect("formatting thread panicked") {
                out[i] = Some(result);
            }
        }
    });
    out.into_iter()
        .map(|r| r.expect("every item is done"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_options() {
        let o = parse_args(&args(&["--check", "--config=a.toml", "x.tex", "dir"]))
            .unwrap()
            .unwrap();
        assert!(o.check && !o.diff && !o.stdin);
        assert_eq!(o.config, Some(PathBuf::from("a.toml")));
        assert_eq!(o.paths, [PathBuf::from("x.tex"), PathBuf::from("dir")]);
        let o = parse_args(&args(&["--stdin-filename", "p/a.tex", "-"]))
            .unwrap()
            .unwrap();
        assert!(o.stdin);
        assert_eq!(o.stdin_filename, Some(PathBuf::from("p/a.tex")));
        assert!(parse_args(&args(&["--help"])).unwrap().is_none());
        assert!(parse_args(&args(&["--frobnicate"])).is_err());
        assert!(parse_args(&args(&["-", "a.tex"])).is_err());
        assert!(parse_args(&args(&["--config"])).is_err());
    }

    #[test]
    fn latin1_round_trips() {
        let bytes = vec![b'a', 0xE9, b'\n'];
        let (text, utf8) = decode(bytes.clone());
        assert!(!utf8);
        assert_eq!(encode(&text, utf8), bytes);
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("texres-fmt-{name}-{}-{nonce}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn path_arg(path: &Path) -> String {
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn check_reports_then_formatting_fixes_in_place() {
        let dir = TempDir::new("check");
        let sub = dir.0.join("chapters");
        std::fs::create_dir(&sub).unwrap();
        let messy = sub.join("intro.tex");
        let clean = dir.0.join("main.tex");
        let bib = dir.0.join("refs.bib");
        let refused = dir.0.join("main.bbl");
        std::fs::write(&messy, "\\begin{itemize}\n\\item a  \n\\end{itemize}\n").unwrap();
        std::fs::write(&clean, "x\n").unwrap();
        std::fs::write(&bib, "@book{a,\ntitle={x}}\n").unwrap();
        std::fs::write(&refused, "\\begin{thebibliography}\n").unwrap();
        let root = path_arg(&dir.0);
        assert_eq!(run(&args(&["--check", &root])), 1);
        assert_eq!(run(&args(&["--diff", &root])), 0);
        assert!(std::fs::read_to_string(&messy)
            .unwrap()
            .contains("\\item a  "));
        assert_eq!(run(&args(&[&root])), 0);
        assert_eq!(
            std::fs::read_to_string(&messy).unwrap(),
            "\\begin{itemize}\n  \\item a\n\\end{itemize}\n"
        );
        assert_eq!(
            std::fs::read_to_string(&bib).unwrap(),
            "@book{a,\n  title = {x},\n}\n"
        );
        assert_eq!(run(&args(&["--check", &root])), 0);
        // Named .bbl files are refused.
        assert_eq!(run(&args(&[&path_arg(&refused)])), 1);
    }

    #[test]
    fn settings_file_is_found_above_the_file() {
        let dir = TempDir::new("config");
        let sub = dir.0.join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            dir.0.join(crate::config::CONFIG_FILE_NAME),
            "indent-width = 4\n",
        )
        .unwrap();
        let file = sub.join("x.tex");
        std::fs::write(&file, "\\begin{center}\nx\n\\end{center}\n").unwrap();
        assert_eq!(run(&args(&[&path_arg(&file)])), 0);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "\\begin{center}\n    x\n\\end{center}\n"
        );
        // An explicit --config wins.
        let other = dir.0.join("other.toml");
        std::fs::write(&other, "indent-width = 1\n").unwrap();
        assert_eq!(
            run(&args(&["--config", &path_arg(&other), &path_arg(&file)])),
            0
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "\\begin{center}\n x\n\\end{center}\n"
        );
        // A broken settings file is an error.
        std::fs::write(dir.0.join(crate::config::CONFIG_FILE_NAME), "indent = 4\n").unwrap();
        assert_eq!(run(&args(&[&path_arg(&file)])), 1);
    }

    #[test]
    fn sibling_files_contribute_verbatim_definitions() {
        let dir = TempDir::new("siblings");
        std::fs::write(
            dir.0.join("preamble.sty"),
            "\\lstnewenvironment{code}{}{}\n",
        )
        .unwrap();
        let file = dir.0.join("ch.tex");
        let src = "\\begin{center}\n\\begin{code}\n   keep\n\\end{code}\n\\end{center}\n";
        std::fs::write(&file, src).unwrap();
        assert_eq!(run(&args(&[&path_arg(&file)])), 0);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "\\begin{center}\n  \\begin{code}\n   keep\n\\end{code}\n\\end{center}\n"
        );
    }
}
