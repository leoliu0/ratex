//! texmk — latexmk-style driver for the Rust TeX engine.
//!
//! The only user-facing command. It runs the bundled engine (pdflatex /
//! xelatex / lualatex — same binary, chosen by `-xelatex`/`-lualatex` or
//! preamble auto-detect) repeatedly until cross-refs / bibliography stabilize.
//! Engines are found next to this binary or in `$HOME/.local/lib/tex-rs`
//! (never PATH, so TeX Live is not used).
//!
//! Exit codes: 0 = converged, 1 = engine/bibtex failure or no convergence,
//! 2 = usage error.
//!
//! Deliberately std-only (no tex-core dependency): this is a process driver.

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_PASSES: u32 = 5;
const VERSION: &str = "texmk (Rust TeX engine) 1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Signals {
    rerun: bool,
    undef_refs: bool,
    undef_cites: bool,
    bbl_missing: bool,
}

struct Options {
    file: PathBuf,
    out_dir: Option<String>,
    jobname: Option<String>,
    silent: bool,
    /// Explicit engine override ("pdflatex", "xelatex", "lualatex"), or auto-detect.
    engine: Option<String>,
    /// Extra flags forwarded verbatim to engine (-interaction=..., -halt-on-error, ...).
    passthrough: Vec<String>,
}

/// Scan source file preamble to auto-detect whether document requires XeLaTeX / LuaLaTeX.
fn detect_engine(src_path: &Path) -> &'static str {
    if let Ok(content) = std::fs::read_to_string(src_path) {
        let preamble_end = content.find("\\begin{document}").unwrap_or(content.len());
        let preamble = &content[..preamble_end];
        if preamble.contains("fontsetup")
            || preamble.contains("fontspec")
            || preamble.contains("xeCJK")
            || preamble.contains("unicode-math")
            || preamble.contains("polyglossia")
            || preamble.contains("ucharclasses")
            || preamble.contains("xunicode")
            || preamble.contains("xltxtra")
            || preamble.contains("ctexart")
            || preamble.contains("ctexrep")
            || preamble.contains("ctexbook")
        {
            return "xelatex";
        }
        if preamble.contains("luacode") || preamble.contains("luatex") {
            return "lualatex";
        }
    }
    "pdflatex"
}

fn usage() {
    eprintln!(
        "usage: texmk [options] file.tex
  -pdf / -xelatex / -lualatex   engine (default: auto from preamble)
  -output-directory DIR         write artifacts in DIR
  -jobname NAME                 job name (default: file stem)
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
    let mut out_dir: Option<String> = None;
    let mut jobname: Option<String> = None;
    let mut silent = true;
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
                out_dir = take_value(&mut i, &inline_val);
            }
            "-jobname" | "--jobname" => {
                jobname = take_value(&mut i, &inline_val);
            }
            "--silent" | "-silent" | "-quiet" | "-q" => silent = true,
            "--verbose" | "-verbose" | "--noisy" | "-noisy" | "-V" => silent = false,
            "-interaction" => {
                let mode = take_value(&mut i, &inline_val)
                    .ok_or_else(|| "-interaction needs a value".to_string())?;
                passthrough.push(format!("-interaction={mode}"));
            }
            "-halt-on-error" => passthrough.push(a.clone()),
            "-pdf" | "--pdf" => engine = Some("pdflatex".to_string()),
            "-pdfxe" | "-xelatex" | "--xelatex" => engine = Some("xelatex".to_string()),
            "-pdflua" | "-lualatex" | "--lualatex" => engine = Some("lualatex".to_string()),
            "-h" | "-help" | "--help" => {
                usage();
                std::process::exit(0);
            }
            "-v" | "-version" | "--version" => {
                println!("{VERSION}");
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
    match file {
        Some(file) => Ok(Options {
            file,
            out_dir,
            jobname,
            silent,
            engine,
            passthrough,
        }),
        None => Err("no input file".to_string()),
    }
}

/// Locate a bundled tool. Never PATH — system pdflatex/bibtex must not win.
/// Search order: $TEXMK_LIB, directory of this texmk, <prefix>/lib/tex-rs,
/// $HOME/.local/lib/tex-rs.
fn find_tool(names: &[&str]) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(lib) = std::env::var("TEXMK_LIB") {
        if !lib.is_empty() {
            dirs.push(PathBuf::from(lib));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
            if let Some(prefix) = parent.parent() {
                dirs.push(prefix.join("lib/tex-rs"));
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/lib/tex-rs"));
    }
    for name in names {
        for dir in &dirs {
            let cand = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn artifact_path(out_dir: &Option<String>, job: &str, ext: &str) -> PathBuf {
    match out_dir {
        Some(d) if !d.is_empty() => PathBuf::from(d).join(format!("{job}{ext}")),
        _ => PathBuf::from(format!("{job}{ext}")),
    }
}

fn read_text(p: &Path) -> Option<String> {
    std::fs::read(p)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// (exists, hash of contents); missing files hash to a distinct stable value
/// so that appearing/disappearing artifacts count as changes.
fn file_hash(p: &Path) -> (bool, u64) {
    match std::fs::read(p) {
        Ok(bytes) => {
            let mut h = DefaultHasher::new();
            h.write(&bytes);
            (true, h.finish())
        }
        Err(_) => (false, 0),
    }
}

fn snapshot(paths: &[PathBuf]) -> Vec<(bool, u64)> {
    paths.iter().map(|p| file_hash(p)).collect()
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
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
    let data = std::fs::read(p).ok()?;
    if let Some(i) = find_sub(&data, b"/Type /Pages") {
        let tail = &data[i..];
        if let Some(c) = find_sub(tail, b"/Count ") {
            let after = &tail[c + "/Count ".len()..];
            let end = after
                .iter()
                .position(|b| !b.is_ascii_digit())
                .unwrap_or(after.len());
            if end > 0 {
                if let Ok(s) = std::str::from_utf8(&after[..end]) {
                    if let Ok(n) = s.parse::<u32>() {
                        return Some(n);
                    }
                }
            }
        }
    }
    let mut n = 0u32;
    let mut from = 0usize;
    while let Some(pos) = find_sub(&data[from..], b"/Type /Page") {
        let at = from + pos;
        // "/Type /Pages" (the tree root) must not count as a page object
        if data.get(at + "/Type /Page".len()) != Some(&b's') {
            n += 1;
        }
        from = at + "/Type /Page".len();
    }
    if n > 0 {
        Some(n)
    } else {
        None
    }
}

fn scan_signals(text: &str, job: &str) -> Signals {
    let mut s = Signals {
        rerun: false,
        undef_refs: false,
        undef_cites: false,
        bbl_missing: false,
    };
    let bbl_marker = format!("{job}.bbl");
    for line in text.lines() {
        if line.contains("Rerun to get") || line.contains("Label(s) may have changed") {
            s.rerun = true;
        }
        if line.contains("There were undefined references") {
            s.undef_refs = true;
        }
        if (line.contains("Citation") && line.contains("undefined"))
            || line.contains("There were undefined citations")
        {
            s.undef_cites = true;
        }
        if line.contains("No file ") && line.contains(&bbl_marker) {
            s.bbl_missing = true;
        }
    }
    s
}

/// Keys from `\citation{...}` lines in the .aux (the set bibtex resolves).
fn aux_citations(aux: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for line in aux.lines() {
        if let Some(rest) = line.strip_prefix("\\citation{") {
            if let Some(keys) = rest.strip_suffix('}') {
                for k in keys.split(',').map(str::trim).filter(|k| !k.is_empty()) {
                    set.insert(k.to_string());
                }
            }
        }
    }
    set
}

fn aux_has_bibdata(aux: &str) -> bool {
    aux.lines().any(|l| l.starts_with("\\bibdata{"))
}

struct ToolOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

impl ToolOutput {
    /// Replay captured output without moving diagnostics onto stdout.
    fn replay(&self) {
        print!("{}", self.stdout);
        eprint!("{}", self.stderr);
    }

    /// Text used for latexmk-style signal scanning. Stream identity does not
    /// matter here, but keep a line boundary when both streams have content.
    fn combined(&self) -> String {
        let mut text = self.stdout.clone();
        if !self.stderr.is_empty() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&self.stderr);
        }
        text
    }
}

/// Run a tool and retain stdout and stderr as distinct streams.
fn run_tool(bin: &Path, args: &[String], silent: bool) -> std::io::Result<ToolOutput> {
    let out = Command::new(bin).args(args).stdin(Stdio::null()).output()?;
    let captured = ToolOutput {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    };
    if !silent {
        captured.replay();
    }
    Ok(captured)
}

/// bibtex reads `.aux`, not the PDF — it can run as soon as `\bibdata` exists.
fn run_bibtex(aux_stem: &Path, silent: bool) -> i32 {
    let Some(bibtex) = find_tool(&["tex-bibtex", "bibtex"]) else {
        eprintln!("texmk: bibtex binary not found (looked next to texmk and in $HOME/.local/lib/tex-rs); cannot resolve citations");
        return 1;
    };
    let arg = aux_stem.to_string_lossy().into_owned();
    if !silent {
        eprintln!("texmk: bibtex {arg}");
    }
    match run_tool(&bibtex, &[arg], silent) {
        Ok(out) if out.success => 0,
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
    if !opt.file.is_file() {
        eprintln!("texmk: no such file: {}", opt.file.display());
        return 1;
    }

    let target_engine = opt
        .engine
        .as_deref()
        .unwrap_or_else(|| detect_engine(&opt.file));
    let engine = match find_tool(&[target_engine, "pdflatex"]) {
        Some(p) => p,
        None => {
            eprintln!(
                "texmk: {target_engine} binary not found (looked next to texmk and in $HOME/.local/lib/tex-rs)"
            );
            return 1;
        }
    };

    let job = opt.jobname.clone().unwrap_or_else(|| {
        opt.file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "texput".to_string())
    });
    let aux_path = artifact_path(&opt.out_dir, &job, ".aux");
    let toc_path = artifact_path(&opt.out_dir, &job, ".toc");
    let lof_path = artifact_path(&opt.out_dir, &job, ".lof");
    let lot_path = artifact_path(&opt.out_dir, &job, ".lot");
    let log_path = artifact_path(&opt.out_dir, &job, ".log");
    let bbl_path = artifact_path(&opt.out_dir, &job, ".bbl");
    let out_path = artifact_path(&opt.out_dir, &job, ".out");
    let snap_paths = [
        aux_path.clone(),
        toc_path.clone(),
        lof_path.clone(),
        lot_path.clone(),
        out_path.clone(),
    ];
    let aux_stem = aux_path.with_extension("");

    let mut prev_cites: Option<BTreeSet<String>> = None;
    let mut bibtex_done = false;
    let mut bibtex_runs = 0u32;
    let mut passes = 0u32;
    let mut converged = false;
    let mut last_output = String::new();
    let mut last_signals: Option<Signals> = None;
    if let Some(d) = &opt.out_dir {
        if let Err(e) = std::fs::create_dir_all(d) {
            eprintln!("texmk: cannot create output directory {d}: {e}");
            return 1;
        }
    }

    // bibtex depends on .aux, not on a PDF. Refresh .bbl from a previous
    // aux so the first typeset can consume it.
    if let Some(aux_text) = read_text(&aux_path) {
        if aux_has_bibdata(&aux_text) {
            let rc = run_bibtex(&aux_stem, opt.silent);
            if rc != 0 {
                return rc;
            }
            bibtex_done = true;
            bibtex_runs += 1;
            prev_cites = Some(aux_citations(&aux_text));
        }
    }

    while passes < MAX_PASSES {
        passes += 1;
        let snap_before = snapshot(&snap_paths);

        let mut args: Vec<String> = vec!["-interaction=nonstopmode".to_string()];
        if let Some(d) = &opt.out_dir {
            args.push("-output-directory".to_string());
            args.push(d.clone());
        }
        if let Some(j) = &opt.jobname {
            args.push("-jobname".to_string());
            args.push(j.clone());
        }
        args.extend(opt.passthrough.iter().cloned());
        args.push(opt.file.to_string_lossy().into_owned());
        if !opt.silent {
            eprintln!("texmk: {target_engine} pass {passes}/{MAX_PASSES}");
        }
        let output = match run_tool(&engine, &args, opt.silent) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("texmk: cannot run {}: {e}", engine.display());
                return 1;
            }
        };
        if !output.success {
            if opt.silent {
                output.replay();
            }
            eprintln!("texmk: {target_engine} failed on pass {passes}");
            return 1;
        }

        let output = output.combined();
        let combined = match read_text(&log_path) {
            Some(log) => format!("{output}\n{log}"),
            None => output,
        };
        last_output = combined.clone();
        let aux_text = read_text(&aux_path).unwrap_or_default();
        let cites = aux_citations(&aux_text);
        let bibdata = aux_has_bibdata(&aux_text);
        let sig = scan_signals(&combined, &job);
        let sig = Signals {
            bbl_missing: sig.bbl_missing || (bibdata && !bbl_path.is_file()),
            ..sig
        };
        last_signals = Some(sig);
        let snap_after = snapshot(&snap_paths);
        let files_changed = snap_before != snap_after;
        let cites_changed = prev_cites.as_ref().map_or(false, |p| *p != cites);
        prev_cites = Some(cites);

        let need_bibtex =
            bibdata && ((!bibtex_done && (sig.bbl_missing || sig.undef_cites)) || cites_changed);
        if need_bibtex {
            let rc = run_bibtex(&aux_stem, opt.silent);
            if rc != 0 {
                return rc;
            }
            bibtex_done = true;
            bibtex_runs += 1;
            continue;
        }

        // This typeset did not change aux/toc/out: another pass cannot
        // resolve more labels. Sticky "Rerun to get" is not a reason to
        // typeset again.
        if !files_changed {
            converged = true;
            break;
        }
    }

    if !converged {
        eprintln!(
            "texmk: build FAILED: no convergence after {MAX_PASSES} {target_engine} pass(es)"
        );
        return 1;
    }
    let pdf_path = artifact_path(&opt.out_dir, &job, ".pdf");
    let pages = pages_from_output(&last_output).or_else(|| pdf_pages(&pdf_path));
    let bib_note = if bibtex_runs > 0 {
        format!(", {bibtex_runs} bibtex run(s)")
    } else {
        String::new()
    };
    match pages {
        Some(n) => eprintln!(
            "texmk: build OK: {} ({} page{}, {passes} {target_engine} pass(es){bib_note})",
            pdf_path.display(),
            n,
            if n == 1 { "" } else { "s" }
        ),
        None if pdf_path.is_file() => eprintln!(
            "texmk: build OK: {} (page count unknown, {passes} {target_engine} pass(es){bib_note})",
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
    }
    0
}

fn main() {
    std::process::exit(real_main());
}
