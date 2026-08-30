//! texmk — latexmk-style driver for the Rust TeX engine.
//!
//! Runs pdflatex (the engine binary built alongside this tool) repeatedly
//! until the document's cross-reference / bibliography state is stable,
//! inserting bibtex runs when the document uses \bibliography:
//!
//!   - parses engine output (and <job>.log when present) for rerun hints,
//!     undefined references, undefined citations, and a missing <job>.bbl
//!   - compares hashes of <job>.aux/.toc/.lof/.lot between passes
//!   - runs bibtex when the .aux has \bibdata and citations are undefined,
//!     the .bbl is missing, or the .aux citation set changed since bibtex
//!   - at most MAX_PASSES pdflatex passes
//!
//! Exit codes: 0 = converged, 1 = engine/bibtex failure or no convergence,
//! 2 = usage error.
//!
//! Deliberately std-only (no tex-core dependency): this is a process driver.

use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
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

impl Signals {
    fn clean(self) -> bool {
        !self.rerun && !self.undef_refs && !self.undef_cites && !self.bbl_missing
    }
}

struct Options {
    file: PathBuf,
    out_dir: Option<String>,
    jobname: Option<String>,
    silent: bool,
    /// Extra flags forwarded verbatim to pdflatex (-interaction=..., -halt-on-error, ...).
    passthrough: Vec<String>,
}

fn usage() {
    eprintln!(
        "usage: texmk [options] file.tex
  -output-directory DIR   write artifacts in DIR (passed to pdflatex)
  -jobname NAME           job name (default: file stem)
  -interaction=MODE       passed to pdflatex (default nonstopmode)
  -halt-on-error          passed to pdflatex
  --silent, -q            suppress engine output
  -h, --help              this text
  -v, --version           version"
    );
}

fn parse_args(argv: &[String]) -> Result<Options, String> {
    let mut file: Option<PathBuf> = None;
    let mut out_dir: Option<String> = None;
    let mut jobname: Option<String> = None;
    let mut silent = false;
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
            "-interaction" => {
                let mode = take_value(&mut i, &inline_val)
                    .ok_or_else(|| "-interaction needs a value".to_string())?;
                passthrough.push(format!("-interaction={mode}"));
            }
            "-halt-on-error" => passthrough.push(a.clone()),
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
            passthrough,
        }),
        None => Err("no input file".to_string()),
    }
}

/// Locate a tool binary: next to this executable first (both bins come from
/// the same crate build), then on PATH. `names` is tried in order.
fn find_tool(names: &[&str]) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    for name in names {
        if let Some(dir) = &exe_dir {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
        if let Some(p) = which(name) {
            return Some(p);
        }
    }
    None
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
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
    std::fs::read(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
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

/// Run a tool, capturing stdout+stderr. Echoes the output unless `silent`.
fn run_tool(bin: &Path, args: &[String], silent: bool) -> std::io::Result<(bool, String)> {
    let out = Command::new(bin).args(args).stdin(Stdio::null()).output()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&err);
    }
    if !silent {
        print!("{text}");
    }
    Ok((out.status.success(), text))
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

    let engine = match find_tool(&["pdflatex"]) {
        Some(p) => p,
        None => {
            eprintln!("texmk: pdflatex binary not found (looked next to texmk and in PATH)");
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

    let mut prev_snap: Option<Vec<(bool, u64)>> = None;
    let mut prev_cites: Option<BTreeSet<String>> = None;
    let mut prev_sig: Option<Signals> = None;
    let mut bibtex_done = false;
    let mut bibtex_runs = 0u32;
    let mut passes = 0u32;
    let mut converged = false;
    if let Some(d) = &opt.out_dir {
        if let Err(e) = std::fs::create_dir_all(d) {
            eprintln!("texmk: cannot create output directory {d}: {e}");
            return 1;
        }
    }

    while passes < MAX_PASSES {
        passes += 1;

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
            eprintln!("texmk: pdflatex pass {passes}/{MAX_PASSES}");
        }
        let (ok, output) = match run_tool(&engine, &args, opt.silent) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("texmk: cannot run {}: {e}", engine.display());
                return 1;
            }
        };
        if !ok {
            if opt.silent {
                print!("{output}");
            }
            eprintln!("texmk: pdflatex failed on pass {passes}");
            return 1;
        }

        // Parse engine output plus the on-disk log (when the engine writes one).
        let combined = match read_text(&log_path) {
            Some(log) => format!("{output}\n{log}"),
            None => output,
        };
        let aux_text = read_text(&aux_path).unwrap_or_default();
        let cites = aux_citations(&aux_text);
        let bibdata = aux_has_bibdata(&aux_text);
        // Log text plus a direct .bbl existence check (an engine may not
        // emit a recognizable "No file" line for it).
        let sig = scan_signals(&combined, &job);
        let sig = Signals {
            bbl_missing: sig.bbl_missing || (bibdata && !bbl_path.is_file()),
            ..sig
        };
        let snap = snapshot(&[
            aux_path.clone(),
            toc_path.clone(),
            lof_path.clone(),
            lot_path.clone(),
        ]);
        let files_changed = prev_snap.as_ref().map_or(false, |p| *p != snap);
        let cites_changed = prev_cites.as_ref().map_or(false, |p| *p != cites);
        // Identical artifacts + identical warnings twice in a row: the engine
        // is deterministic, another pass cannot change anything.
        let stuck = !files_changed && !cites_changed && prev_sig == Some(sig);
        prev_snap = Some(snap);
        prev_cites = Some(cites);
        prev_sig = Some(sig);

        // Bibliography pass: first trigger is undefined citations / missing
        // .bbl; later triggers only a changed citation set (otherwise bibtex
        // cannot help and we would loop forever).
        let need_bibtex =
            bibdata && ((!bibtex_done && (sig.bbl_missing || sig.undef_cites)) || cites_changed);
        if need_bibtex {
            let Some(bibtex) = find_tool(&["tex-bibtex", "bibtex"]) else {
                eprintln!("texmk: bibtex binary not found (looked next to texmk and in PATH); cannot resolve citations");
                return 1;
            };
            // real bibtex convention: aux path without the .aux extension
            let bib_args = vec![aux_path.with_extension("").to_string_lossy().into_owned()];
            if !opt.silent {
                eprintln!("texmk: bibtex {}", bib_args[0]);
            }
            let (ok, out) = match run_tool(&bibtex, &bib_args, opt.silent) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("texmk: cannot run {}: {e}", bibtex.display());
                    return 1;
                }
            };
            bibtex_done = true;
            bibtex_runs += 1;
            if !ok {
                if opt.silent {
                    print!("{out}");
                }
                eprintln!("texmk: bibtex failed");
                return 1;
            }
            continue;
        }

        if sig.clean() && !files_changed {
            converged = true;
            break;
        }
        if stuck {
            eprintln!(
                "texmk: not converging: artifacts unchanged two passes in a row{}",
                if sig.undef_cites {
                    " (citations remain undefined)"
                } else if sig.undef_refs {
                    " (references remain undefined)"
                } else {
                    ""
                }
            );
            return 1;
        }
    }

    if !converged {
        eprintln!("texmk: giving up after {MAX_PASSES} passes");
        return 1;
    }
    if !opt.silent {
        eprintln!(
            "texmk: stable after {passes} pdflatex pass(es){}",
            if bibtex_runs > 0 {
                format!(", {bibtex_runs} bibtex run(s)")
            } else {
                String::new()
            },
        );
    }
    0
}

fn main() {
    std::process::exit(real_main());
}
