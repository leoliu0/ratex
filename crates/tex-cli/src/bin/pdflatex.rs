#![allow(dead_code)]

#[path = "../allocator.rs"]
mod allocator;
#[global_allocator]
static GLOBAL: allocator::EngineAllocator = allocator::EngineAllocator;

/// Precompiled format containing standard LaTeX packages baked directly into the binary.
static EMBEDDED_DEFAULT_FMT: &[u8] = include_bytes!("../../assets/default.fmt");

// Portable timestamps keep cache files usable on every release platform.
fn mtime(meta: &std::fs::Metadata) -> (i64, i64) {
    match meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
    {
        Some(t) => (t.as_secs() as i64, t.subsec_nanos() as i64),
        None => (i64::MIN, 0),
    }
}

fn cache_identity() -> Option<String> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let exe = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(&exe).ok()?;
    exe.hash(&mut hash);
    meta.len().hash(&mut hash);
    meta.modified().ok()?.hash(&mut hash);
    std::env::args_os().collect::<Vec<_>>().hash(&mut hash);
    for key in [
        "TEXINPUTS",
        "TEXMFHOME",
        "TEXMFLOCAL",
        "TEXMFDIST",
        "TEX_SUITE_DATA",
        "TFMFONTS",
        "T1FONTS",
        "TTFONTS",
        "OPENTYPEFONTS",
        "ENCFONTS",
        "TEXFONTMAPS",
        "SOURCE_DATE_EPOCH",
        "TZ",
    ] {
        std::env::var_os(key).hash(&mut hash);
    }
    Some(format!("TEX-DEPCACHE-2 {:016x}", hash.finish()))
}

/// Dependency-cache hit: if no tracked input changed since the last
/// successful compile, the output PDF is already current.
fn check_depcache(job: &str, out_dir: &str, primary_file: &str) -> Option<usize> {
    let cache_path = format!("{}{}.depcache", out_dir, job);
    let content = std::fs::read_to_string(&cache_path).ok()?;
    let mut lines = content.lines();
    if lines.next()? != cache_identity()?.as_str() {
        return None;
    }
    let pdf_line = lines.next()?;
    let mut pdf_parts = pdf_line.split('\t');
    let pdf_path = pdf_parts.next()?;
    let pdf_size: usize = pdf_parts.next()?.parse().ok()?;
    let pdf_meta = std::fs::metadata(pdf_path).ok()?;
    if pdf_meta.len() as usize != pdf_size {
        return None;
    }
    if std::fs::metadata(primary_file).is_err() {
        return None;
    }
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("AUX") {
            let mut parts = rest.split('\t');
            let path = parts.next()?;
            let size: u64 = parts.next()?.parse().ok()?;
            let hash: u64 = parts.next()?.parse().ok()?;
            let cur = content_digest(std::path::Path::new(path)).unwrap_or((u64::MAX, 0));
            if cur.0 == size && cur.1 == hash {
                continue;
            }
            return None;
        }
        let mut parts = line.split('\t');
        let path = parts.next()?;
        let stored_mtime: i64 = parts.next()?.parse().ok()?;
        let nsec: i64 = parts.next()?.parse().ok()?;
        let size: u64 = parts.next()?.parse().ok()?;
        match std::fs::metadata(path) {
            Ok(meta) => {
                if mtime(&meta).0 != stored_mtime || mtime(&meta).1 != nsec || meta.len() != size {
                    return None;
                }
            }
            Err(_) => return None,
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

fn content_digest(path: &std::path::Path) -> Option<(u64, u64)> {
    let data = std::fs::read(path).ok()?;
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x9e37_79b9_7f4a_7c15;
    for (i, b) in data.iter().enumerate() {
        h1 ^= *b as u64;
        h1 = h1.wrapping_mul(0x1000_0000_01b3);
        h2 = (h2 + *b as u64 + (i as u64)).wrapping_mul(0x1000_0000_01b3);
    }
    Some((data.len() as u64, h1 ^ h2))
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

fn write_depcache(
    job: &str,
    out_dir: &str,
    pdf_path: &str,
    pdf_size: usize,
    deps: &[std::path::PathBuf],
    aux_start: &[(std::path::PathBuf, u64, u64)],
) {
    use std::collections::BTreeSet;
    let cache_path = format!("{}{}.depcache", out_dir, job);
    let Some(identity) = cache_identity() else {
        return;
    };
    let mut out = format!("{identity}\n{}\t{}\n", pdf_path, pdf_size);
    let unique: BTreeSet<&std::path::PathBuf> = deps.iter().collect();
    for d in unique {
        if let Ok(meta) = std::fs::metadata(d) {
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\n",
                d.display(),
                mtime(&meta).0,
                mtime(&meta).1,
                meta.len()
            ));
        }
    }
    for (p, len, h) in aux_start {
        out.push_str(&format!("AUX{}\t{}\t{}\n", p.display(), len, h));
    }
    let _ = std::fs::write(cache_path, out);
}

use tex_core::engine::{Engine, InteractionMode, DEFAULT_MAX_ERRORS};
use tex_core::pdffile;
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
    engine.interaction_mode = interaction_mode;
    engine.max_errors = max_errors;
}

fn emit_transcript(engine: &Engine) {
    if engine.interaction_mode == InteractionMode::Batch {
        return;
    }
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

fn write_early_transcript(engine: &Engine, out_dir: &str, job: &str) -> Result<String, String> {
    let path = format!("{out_dir}{job}.log");
    std::fs::write(&path, &engine.log)
        .map(|()| path.clone())
        .map_err(|error| format!("cannot write transcript {path}: {error}"))
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
        if std::env::var_os("RUST_BACKTRACE").is_some() {
            eprintln!("{}", std::backtrace::Backtrace::force_capture());
        }
    }));
}

pub(crate) fn main() {
    install_panic_reporter();
    let mut phase_timer = PhaseTimer::new();
    apply_mem_limit();
    let program = program_name();
    let args: Vec<String> = std::env::args_os()
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
    let mut jobname: Option<String> = None;
    let mut ini = false;
    let mut plain = false;
    let mut halt_on_error = false;
    let mut interaction_mode = InteractionMode::ErrorStop;
    let mut max_errors = DEFAULT_MAX_ERRORS;
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
        } else if args[i] == "-jobname" {
            i += 1;
            let Some(name) = args.get(i).filter(|name| !name.is_empty()) else {
                usage_error(&program, "-jobname requires a non-empty name");
            };
            jobname = Some(name.clone());
        } else if let Some(jn) = args[i].strip_prefix("-jobname=") {
            if jn.is_empty() {
                usage_error(&program, "-jobname requires a non-empty name");
            }
            jobname = Some(jn.to_string());
        } else if args[i] == "-ini" {
            ini = true;
        } else if args[i] == "-plain" {
            plain = true;
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
        } else if matches!(
            args[i].as_str(),
            "-file-line-error" | "-file-line-error-style" | "-no-shell-escape"
        ) {
            // Rich file/line diagnostics are always enabled; shell execution
            // is never enabled.
        } else if matches!(args[i].as_str(), "-v" | "-version" | "--version") {
            if program == "xelatex" {
                println!("XeTeX 3.141592653-2.6-0.999996 (TeX Live 2026/Rust)");
            } else if program == "lualatex" {
                println!("This is LuaHBTeX, Version 1.18.0 (TeX Live 2026/Rust)");
            } else {
                println!("pdfTeX-2h 1.40.29-rs (TeX Live 2026/Rust)");
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

    let job = jobname.unwrap_or_else(|| {
        std::path::Path::new(&file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "texput".to_string())
    });
    let aux_start = snapshot_aux_state(&job, &out_dir);
    if !plain && !ini {
        if let Some(pdf_size) = check_depcache(&job, &out_dir, &file) {
            let out = format!("{}{}.pdf", out_dir, job);
            if interaction_mode != InteractionMode::Batch {
                println!("\nOutput written on {} ({} bytes).", out, pdf_size);
            }
            std::process::exit(0);
        }
    }
    let mut eng = Engine::new(ini || !plain);
    eng.init_primitives();
    configure_engine(&mut eng, halt_on_error, interaction_mode, max_errors);
    phase_timer.mark("startup");
    eng.out_dir = out_dir.clone();
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
            Some(std::path::PathBuf::from("/tmp/pdflatex.fmt")),
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
                let log_result = write_early_transcript(&eng, &out_dir, &job);
                let transcript_note = log_result
                    .as_ref()
                    .map(|path| format!("; transcript written to {path}"))
                    .unwrap_or_default();
                match failure {
                    FormatBootFailure::Errors(count) => emit_cli_message(
                        interaction_mode,
                        format_args!(
                            "{program}: cannot compile {file}: LaTeX format boot reported {count} error{}; fix the diagnostic above or install a valid pdflatex.fmt{transcript_note}",
                            if count == 1 { "" } else { "s" },
                        ),
                    ),
                    FormatBootFailure::Incomplete { file, line } => emit_cli_message(
                        interaction_mode,
                        format_args!(
                            "{program}: cannot compile the document: LaTeX format boot ended before \\dump at {file}:{line}; install a valid pdflatex.fmt or fix the format sources{transcript_note}"
                        ),
                    ),
                }
                if let Err(error) = log_result {
                    emit_cli_message(interaction_mode, format_args!("{program}: {error}"));
                }
                std::process::exit(1);
            }
            let dump_target = exe_fmt.unwrap_or_else(|| std::path::PathBuf::from("pdflatex.fmt"));
            if let Err(error) = tex_core::format::save_format(&eng, &dump_target) {
                emit_cli_message(
                    interaction_mode,
                    format_args!(
                        "{program}: could not cache the freshly built LaTeX format at {} ({error}); continuing with the in-memory format",
                        dump_target.display()
                    ),
                );
            }
        }
        // The format is settled (loaded or just dumped); the user file runs in
        // production mode, so a stray \dump cannot end the job silently.
        eng.ini_mode = false;
        eng.end_occurred = false;
        eng.explicit_end_seen = false;
        eng.reset_job_diagnostics();
        eng.main_steps = 0;
        eng.expansion_steps = 0;
        eng.term.clear();
        eng.log.clear();
        eng.input.clear_sources();
        configure_engine(&mut eng, halt_on_error, interaction_mode, max_errors);
        // (tex.web §372). color.cfg then takes the luatex branch.
        // pdfTeX identity: those names must compare \\ifx-equal \\@undefined.
        for name in [
            b"luatexversion" as &[u8],
            b"luatexrevision",
            b"luatexbanner",
            b"directlua",
            b"outputmode",
            b"tex_luatexversion:D",
            b"tex_directlua:D",
        ] {
            if let Some(id) = eng.cs.lookup(name) {
                if matches!(
                    eng.eqtb.get(id),
                    Some(tex_core::eqtb::Equiv::Prim(tex_core::prim::Prim::Relax)) | None
                ) {
                    eng.eqtb.undefine(id, true);
                }
            }
        }
        let lang = eng.cs.intern(b"languagename");
        if eng.eqtb.get(lang).is_none() {
            eng.eqtb.assign(
                lang,
                tex_core::eqtb::Equiv::Macro(std::rc::Rc::new(tex_core::eqtb::Macro {
                    replacement: Default::default(),
                    num_params: 0,
                    has_param_refs: false,
                    params: vec![],
                    prefix: vec![],
                    body: b"english"
                        .iter()
                        .map(|&c| tex_core::token::Token::letter(c))
                        .collect::<Vec<_>>()
                        .into(),
                    long: false,
                    outer: false,
                    protected: false,
                })),
                true,
            );
        }
        let sp = eng.cs.intern(b" ");
        if eng.eqtb.get(sp).is_none() {
            eng.eqtb.assign(
                sp,
                tex_core::eqtb::Equiv::Prim(tex_core::prim::Prim::ExSpace),
                true,
            );
        }
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
        let pdd = eng.cs.intern(b"pdfdecimaldigits");
        eng.eqtb
            .assign(pdd, tex_core::eqtb::Equiv::CountReg(250), true);
        let ppk = eng.cs.intern(b"pdfpkresolution");
        eng.eqtb
            .assign(ppk, tex_core::eqtb::Equiv::CountReg(251), true);
    }
    phase_timer.mark("format");
    if eng.input_file(&file) {
        // Knuth: everyjob is inserted on top of the * file so it runs first.
        if !plain && !ini {
            // Format \\everyjob contains \\directlua{...}. Install the
            // swallow-group stub for that, then \\let it to \\@undefined
            // so color.cfg / iftex see a pdfTeX engine.
            let dl = eng.cs.intern(b"directlua");
            eng.eqtb.assign(
                dl,
                tex_core::eqtb::Equiv::Prim(tex_core::prim::Prim::DirectLua),
                true,
            );
            if let Some(let_id) = eng.cs.lookup(b"let") {
                let undef = eng.cs.intern(b"@undefined");
                eng.push_tokens_named(
                    vec![
                        tex_core::token::Token::from_cs(let_id),
                        tex_core::token::Token::from_cs(dl),
                        tex_core::token::Token::from_cs(undef),
                    ],
                    "<pdftex-not-luatex>",
                );
            }
            if let Some(def_id) = eng.cs.lookup(b"def") {
                // xcolor `\\providecommand*\\rangeRGB{255}` is a no-op if
                // the name was csname-poisoned to \\relax; force pdfTeX
                // defaults so \\ifnum\\rangeRGB=255 takes the RGB driver.
                for (name, body) in [
                    (b"rangeRGB" as &[u8], b"255" as &[u8]),
                    (b"rangeHSB", b"240"),
                    (b"rangeHsb", b"360"),
                    (b"rangeGray", b"15"),
                ] {
                    let id = eng.cs.intern(name);
                    let mut toks = vec![
                        tex_core::token::Token::from_cs(def_id),
                        tex_core::token::Token::from_cs(id),
                        tex_core::token::Token::char(1, b'{' as u32),
                    ];
                    for &b in body {
                        toks.push(tex_core::token::Token::char(12, b as u32));
                    }
                    toks.push(tex_core::token::Token::char(2, b'}' as u32));
                    eng.push_tokens_named(toks, "<pdftex-range>");
                }
            }
            let ej =
                (*eng.eqtb.tok_params[tex_core::prim::ToksParam::EveryJob.idx() as usize]).clone();
            if !ej.is_empty() {
                eng.push_tokens_named(ej, "<everyjob>");
            }
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
            let log_result = write_early_transcript(&eng, &out_dir, &job);
            let transcript_note = log_result
                .as_ref()
                .map(|path| format!("; transcript written to {path}"))
                .unwrap_or_default();
            emit_cli_message(
                interaction_mode,
                format_args!(
                    "{program}: format build reported {} error{}; pdflatex.fmt was not written{transcript_note}",
                    eng.error_count,
                    if eng.error_count == 1 { "" } else { "s" }
                ),
            );
            if let Err(error) = log_result {
                emit_cli_message(interaction_mode, format_args!("{program}: {error}"));
            }
            std::process::exit(1);
        }
        match tex_core::format::save_format(&eng, std::path::Path::new("pdflatex.fmt")) {
            Ok(n) => emit_cli_message(
                interaction_mode,
                format_args!("Format written to pdflatex.fmt ({n} bytes)"),
            ),
            Err(e) => {
                emit_transcript(&eng);
                emit_cli_message(
                    interaction_mode,
                    format_args!("{program}: cannot write pdflatex.fmt: {e}"),
                );
                std::process::exit(1);
            }
        }
        emit_transcript(&eng);
        std::process::exit(if eng.error_count > 0 { 1 } else { 0 });
    }
    emit_transcript(&eng);
    let log_path = format!("{}{}.log", out_dir, job);
    if let Err(error) = std::fs::write(&log_path, &eng.log) {
        eprintln!("{program}: cannot write transcript {log_path}: {error}");
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
        // embed fonts used
        use std::collections::BTreeSet;
        let mut used: BTreeSet<u16> = BTreeSet::new();
        for fonts in eng
            .pdf_doc
            .pages
            .iter()
            .map(|p| &p.fonts)
            .chain(eng.pdf_doc.form_fonts.iter().map(|(_, fonts)| fonts))
        {
            for (fid, _) in fonts {
                used.insert(*fid as u16);
            }
        }
        let mut fidx: Vec<(u16, usize)> = Vec::new();
        for (n, fid) in used.iter().enumerate() {
            if let Some(font) = eng.eqtb.fonts.get(*fid as usize) {
                let pfb_bytes = font
                    .type1_path
                    .as_ref()
                    .and_then(|name| eng.font_loader.kpse.read(name, tex_kpse::Format::Type1));
                let widths = (0..=255u8)
                    .map(|c| {
                        let w = font.char_width(c);
                        if font.at_size != 0 {
                            ((w as i64 * 10_000 + font.at_size as i64 / 2) / font.at_size as i64)
                                as i32
                        } else {
                            0
                        }
                    })
                    .collect();
                let mut ef = pdffile::make_embed_font(
                    font.map_fontname
                        .clone()
                        .unwrap_or_else(|| font.tfm_name.clone()),
                    pfb_bytes.as_deref(),
                    font.encoding.as_deref(),
                    0,
                    255,
                    widths,
                );
                pdffile::set_font_usage(
                    &mut ef,
                    eng.pdf_doc
                        .font_chars
                        .get(&(*fid as usize))
                        .copied()
                        .unwrap_or([0; 4]),
                );
                if font.at_size != 0 {
                    let to_units =
                        |val: i32| -> f64 { (val as f64 * 1000.0 / font.at_size as f64).round() };
                    let (ta, td, tc, ts) = tex_core::pdf_fonts::tfm_descriptor(font);
                    let asc = to_units(font.char_height(b'd'));
                    let cap = to_units(font.char_height(b'H'));
                    let desc = -to_units(font.char_depth(b'p'));
                    ef.ascent = if asc > 0.0 { asc } else { ta };
                    ef.cap_height = if cap > 0.0 { cap } else { tc };
                    ef.descent = if desc != 0.0 { desc } else { td };
                    ef.stem_v = ts.max(100.0);
                }
                eng.pdf_doc.fonts.push(ef);
                fidx.insert(n, (*fid, n));
            }
        }
        // remap page font indices: pages reference engine font ids; convert to doc font index
        for fonts in eng
            .pdf_doc
            .pages
            .iter_mut()
            .map(|p| &mut p.fonts)
            .chain(eng.pdf_doc.form_fonts.iter_mut().map(|(_, fonts)| fonts))
        {
            for pf in fonts.iter_mut() {
                if let Some(pos) = fidx
                    .iter()
                    .find(|(fid, _)| *fid as u16 as u16 == pf.0 as u16)
                {
                    pf.0 = pos.1;
                }
            }
        }
        phase_timer.mark("fonts");
        // embed image XObjects
        struct ImageJob<'a> {
            object: i32,
            mask: i32,
            bytes: Vec<u8>,
            path: &'a str,
        }
        fn embed_chunk(
            jobs: &[ImageJob<'_>],
        ) -> Result<Vec<tex_core::pdf_images::EmbeddedImage>, String> {
            let mut objects = Vec::with_capacity(jobs.len().saturating_mul(2));
            for job in jobs {
                let mut next = job.mask;
                let embedded = if job.bytes.starts_with(&[0xff, 0xd8]) {
                    tex_core::pdf_images::embed_jpeg(&job.bytes, job.object)
                        .map(|image| vec![image])
                } else {
                    tex_core::pdf_images::embed_png(&job.bytes, job.object, &mut next)
                }
                .ok_or_else(|| format!("Unsupported or invalid image: {}", job.path))?;
                objects.extend(embedded);
            }
            Ok(objects)
        }
        let mut used_images: Vec<_> = eng
            .pdf_images
            .iter()
            .filter(|(_, image)| image.used)
            .collect();
        used_images.sort_unstable_by_key(|(object, _)| **object);
        let mut next_obj = eng.pdf_next_obj;
        let mut jobs = Vec::with_capacity(used_images.len());
        for (object, image) in used_images {
            // PDF page resources were imported while scanning \pdfximage.
            if image.embedded {
                continue;
            }
            let bytes = std::fs::read(&image.path).unwrap_or_else(|error| {
                eprintln!("Cannot read image {}: {}", image.path, error);
                std::process::exit(1);
            });
            let mask = next_obj;
            if tex_core::pdf_images::png_needs_soft_mask(&bytes) {
                next_obj = next_obj.checked_add(1).unwrap_or_else(|| {
                    eprintln!("PDF image object number overflow");
                    std::process::exit(1);
                });
            }
            jobs.push(ImageJob {
                object: *object,
                mask,
                bytes,
                path: &image.path,
            });
        }
        let workers = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(8)
            .min(jobs.len());
        let embedded = if workers <= 1 {
            embed_chunk(&jobs)
        } else {
            std::thread::scope(|scope| {
                let handles: Vec<_> = jobs
                    .chunks(jobs.len().div_ceil(workers))
                    .map(|chunk| scope.spawn(move || embed_chunk(chunk)))
                    .collect();
                let mut objects = Vec::with_capacity(jobs.len().saturating_mul(2));
                let mut error = None;
                for handle in handles {
                    match handle.join() {
                        Ok(Ok(chunk)) => objects.extend(chunk),
                        Ok(Err(message)) => error = Some(message),
                        Err(_) => error = Some("Image embedding worker panicked".to_string()),
                    }
                }
                match error {
                    Some(message) => Err(message),
                    None => Ok(objects),
                }
            })
        }
        .unwrap_or_else(|error| {
            eprintln!("{}", error);
            std::process::exit(1);
        });
        for image in embedded {
            eng.pdf_doc.objects.push((image.obj_num, image.bytes));
        }
        phase_timer.mark("images");
        let pdf = pdffile::write_pdf(&eng.pdf_doc);
        phase_timer.mark("pdf_serialize");
        let out = format!("{}{}.pdf", eng.out_dir, job);
        if !eng.out_dir.is_empty() {
            let _ = std::fs::create_dir_all(eng.out_dir.trim_end_matches('/'));
        }
        if let Err(error) = std::fs::write(&out, &pdf) {
            eprintln!("{program}: cannot write PDF {out}: {error}");
            std::process::exit(1);
        }
        phase_timer.mark("pdf_write");
        let pdf_len = std::fs::metadata(&out)
            .map(|m| m.len() as usize)
            .unwrap_or(pdf.len());
        if eng.error_count == 0 {
            write_depcache(
                &job,
                &eng.out_dir,
                &out,
                pdf_len,
                &eng.loaded_files,
                &aux_start,
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

fn finalize_format_load(eng: &mut Engine) {
    // Format images predating the italic-correction primitive
    // stored LaTeX's `\@@italiccorr` as `\relax`. Rebind both
    // names so loaded and freshly bootstrapped formats agree.
    for name in [b"/" as &[u8], b"@@italiccorr"] {
        let id = eng.cs.intern(name);
        eng.eqtb.assign(
            id,
            tex_core::eqtb::Equiv::Prim(tex_core::prim::Prim::ItalicCorrection),
            true,
        );
    }
    // Compat shim for formats dumped before the
    // active-char namespace split: their boot wrote the
    // kernel tie to the hash slot, where encoding
    // defaults (\DeclareTextAccentDefault) later
    // clobbered it, so no usable tie survives on either
    // slot. Synthesize latex.ltx:9414's protected tie
    // directly on the active slot; no-op when the format
    // already carries one (post-split dumps).
    let act = eng
        .cs
        .intern(&tex_core::engine::Engine::active_cs_name(b'~'));
    if eng.eqtb.get(act).is_none() {
        let id_of = |eng: &tex_core::engine::Engine, name: &[u8]| eng.cs.lookup(name);
        let tie: Option<tex_core::eqtb::Equiv> = {
            let ifincs = id_of(&eng, b"ifincsname");
            let expafter = id_of(&eng, b"expandafter");
            let nobreak = id_of(&eng, b"nobreakspace");
            let fi = id_of(&eng, b"fi");
            match (ifincs, expafter, nobreak, fi) {
                (Some(a), Some(b), Some(c), Some(d)) => {
                    let body = vec![
                        tex_core::token::Token::from_cs(a),
                        tex_core::token::Token::from_cs(b),
                        tex_core::token::Token::char(13, b'~' as u32),
                        tex_core::token::Token::from_cs(d),
                        tex_core::token::Token::from_cs(b),
                        tex_core::token::Token::from_cs(c),
                        tex_core::token::Token::from_cs(d),
                    ];
                    Some(tex_core::eqtb::Equiv::Macro(std::rc::Rc::new(
                        tex_core::eqtb::Macro {
                            replacement: Default::default(),
                            num_params: 0,
                            has_param_refs: false,
                            params: Vec::new(),
                            prefix: Vec::new(),
                            body: body.into(),
                            long: false,
                            outer: false,
                            protected: true,
                        },
                    )))
                }
                _ => None,
            }
        };
        if let Some(eq) = tie {
            eng.eqtb.assign(act, eq, true);
        }
    }
    // tex.web §240: period is the null delimiter (code 0)
    eng.eqtb.del_code[b'.' as usize] = 0;
    // tex.web §1014: page_goal starts at max_dimen
    eng.eqtb.dim_params[tex_core::prim::DimParam::PageGoal.idx() as usize] = 0x3FFF_FFFF;
    eng.page_goal_set = false;
}

#[cfg(test)]
mod startup_tests {
    use super::{format_boot_failure, FormatBootFailure};
    use tex_core::engine::Engine;

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
}
