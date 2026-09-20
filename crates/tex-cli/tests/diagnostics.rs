use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_JOB: AtomicUsize = AtomicUsize::new(0);

struct Job {
    dir: PathBuf,
}

impl Job {
    fn new(label: &str) -> Self {
        let serial = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tex-cli-diagnostics-{label}-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.dir.join(name), text).unwrap();
    }

    fn write_bytes(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.dir.join(name), bytes).unwrap();
    }

    fn compile(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .args(args)
            .arg("main.tex")
            .current_dir(&self.dir)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env_remove("PHASE_TIMING")
            .env_remove("TEXDEBUG")
            .output()
            .unwrap()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("main.log")).unwrap()
    }

    #[cfg(unix)]
    fn tool(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;

        self.write(name, &format!("#!/bin/sh\nset -eu\n{body}\n"));
        std::fs::set_permissions(self.dir.join(name), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }

    #[cfg(unix)]
    fn texmk(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_texmk"))
            .arg("main.tex")
            .current_dir(&self.dir)
            .env("TEXMK_LIB", &self.dir)
            .env("TEX_RS_CACHE_DIR", self.dir.join("cache"))
            .output()
            .unwrap()
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

fn failure_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        text(&output.stdout),
        text(&output.stderr)
    )
}

fn png_crc(kind: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in kind.iter().chain(data) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn push_png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&png_crc(kind, data).to_be_bytes());
}

fn stored_zlib(data: &[u8]) -> Vec<u8> {
    assert!(data.len() <= u16::MAX as usize);
    let length = data.len() as u16;
    let mut stream = vec![0x78, 0x01, 0x01];
    stream.extend_from_slice(&length.to_le_bytes());
    stream.extend_from_slice(&(!length).to_le_bytes());
    stream.extend_from_slice(data);
    let (mut first, mut second) = (1u32, 0u32);
    for &byte in data {
        first = (first + u32::from(byte)) % 65_521;
        second = (second + first) % 65_521;
    }
    stream.extend_from_slice(&((second << 16) | first).to_be_bytes());
    stream
}

fn corrupt_passthrough_png(indexed: bool) -> Vec<u8> {
    let (width, bit_depth, color_type, scanline) = if indexed {
        (4u32, 2u8, 3u8, vec![0, 0b00_01_10_01])
    } else {
        (2, 8, 2, vec![0, 10, 20, 30, 40, 50, 60])
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&[bit_depth, color_type, 0, 0, 0]);

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    push_png_chunk(&mut out, b"IHDR", &ihdr);
    if indexed {
        push_png_chunk(&mut out, b"PLTE", &[255, 0, 0, 0, 255, 0, 0, 0, 255]);
    }
    let mut idat = stored_zlib(&scanline);
    *idat.last_mut().unwrap() ^= 0x80;
    // The PNG chunk CRC covers the corrupted payload correctly. Only zlib's
    // Adler-32 is invalid, exercising the streaming IDAT validator.
    push_png_chunk(&mut out, b"IDAT", &idat);
    push_png_chunk(&mut out, b"IEND", &[]);
    out
}

fn line_after<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    let mut lines = haystack.lines();
    while let Some(line) = lines.next() {
        if line.contains(needle) {
            return lines.next();
        }
    }
    None
}

#[test]
fn halted_undefined_command_has_a_single_located_diagnostic_and_no_pdf() {
    let job = Job::new("halt-location");
    job.write(
        "main.tex",
        r"\documentclass{article}
\begin{document}
A complete page before the error.
\newpage
Prefix \undefinedDiagnosticCommand suffix
\end{document}
",
    );

    let output = job.compile(&["-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));

    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    let marker = "Undefined control sequence \\undefinedDiagnosticCommand";
    assert_eq!(occurrences(&stderr, marker), 1, "{stderr}");
    assert_eq!(occurrences(&stdout, marker), 0, "{stdout}");
    assert!(stderr.contains("main.tex:5:8"), "{stderr}");
    assert!(
        stderr.contains("5 | Prefix \\undefinedDiagnosticCommand suffix"),
        "{stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|line| line.contains('|') && line.contains('^')),
        "{stderr}"
    );
    assert!(!stderr.contains("rest=["), "{stderr}");
    assert!(!stderr.contains("ch:"), "{stderr}");

    let log = job.log();
    assert_eq!(occurrences(&log, marker), 1, "{log}");
    assert!(log.contains("main.tex:5:8"), "{log}");
    assert!(!job.dir.join("main.pdf").exists());
    assert!(!job.dir.join("main.depcache").exists());
}

#[test]
fn batch_mode_is_silent_but_keeps_the_error_in_the_log() {
    let job = Job::new("batch-log");
    job.write(
        "main.tex",
        r"\documentclass{article}
\begin{document}
\undefinedBatchDiagnostic
\end{document}
",
    );

    let output = job.compile(&["-interaction=batchmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    assert!(output.stdout.is_empty(), "{}", failure_output(&output));
    assert!(output.stderr.is_empty(), "{}", failure_output(&output));

    let log = job.log();
    let marker = "Undefined control sequence \\undefinedBatchDiagnostic";
    assert_eq!(occurrences(&log, marker), 1, "{log}");
    assert!(log.contains("main.tex:3:1"), "{log}");
}

#[test]
fn interaction_mode_changes_filter_each_event_when_it_occurs() {
    let entering_batch = Job::new("mode-transition-into-batch");
    entering_batch.write(
        "main.tex",
        "\\message{VISIBLE BEFORE BATCH MODE=\\the\\interactionmode}\n\\interactionmode=0\n\\undefinedHiddenInBatch\n\\end\n",
    );
    let entering_output = entering_batch.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(
        entering_output.status.code(),
        Some(1),
        "{}",
        failure_output(&entering_output)
    );
    let entering_stdout = text(&entering_output.stdout);
    let entering_stderr = text(&entering_output.stderr);
    assert!(
        entering_stdout.contains("VISIBLE BEFORE BATCH MODE=1"),
        "{entering_stdout}"
    );
    assert!(
        !entering_stdout.contains("undefinedHiddenInBatch"),
        "{entering_stdout}"
    );
    assert!(entering_stderr.is_empty(), "{entering_stderr}");
    let entering_log = entering_batch.log();
    assert!(
        entering_log.contains("VISIBLE BEFORE BATCH MODE=1"),
        "{entering_log}"
    );
    assert!(
        entering_log.contains("Undefined control sequence \\undefinedHiddenInBatch"),
        "{entering_log}"
    );

    let leaving_batch = Job::new("mode-transition-out-of-batch");
    leaving_batch.write(
        "main.tex",
        "\\message{HIDDEN BEFORE NONSTOP MODE=\\the\\interactionmode}\n\\interactionmode=1\n\\undefinedVisibleAfterBatch\n\\end\n",
    );
    let leaving_output = leaving_batch.compile(&["-plain", "-interaction=batchmode"]);
    assert_eq!(
        leaving_output.status.code(),
        Some(1),
        "{}",
        failure_output(&leaving_output)
    );
    let leaving_stdout = text(&leaving_output.stdout);
    let leaving_stderr = text(&leaving_output.stderr);
    assert!(
        !leaving_stdout.contains("HIDDEN BEFORE NONSTOP"),
        "{leaving_stdout}"
    );
    assert!(
        leaving_stderr.contains("Undefined control sequence \\undefinedVisibleAfterBatch"),
        "{leaving_stderr}"
    );
    let leaving_log = leaving_batch.log();
    assert!(
        leaving_log.contains("HIDDEN BEFORE NONSTOP MODE=0"),
        "{leaving_log}"
    );
    assert!(
        leaving_log.contains("Undefined control sequence \\undefinedVisibleAfterBatch"),
        "{leaving_log}"
    );
}

#[test]
fn ini_completion_uses_the_mode_active_when_each_message_is_emitted() {
    let entering_batch = Job::new("ini-enters-batch");
    entering_batch.write(
        "main.tex",
        "\\catcode123=1\n\\catcode125=2\n\\interactionmode=0\n\\errmessage{HIDDEN IN BATCH}\n\\dump\n",
    );
    let output = entering_batch.compile(&["-ini", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    assert!(output.stderr.is_empty(), "{}", failure_output(&output));
    assert!(
        !text(&output.stdout).contains("format build reported"),
        "{}",
        failure_output(&output)
    );
    assert!(entering_batch.log().contains("HIDDEN IN BATCH"));

    let leaving_batch = Job::new("ini-leaves-batch");
    leaving_batch.write(
        "main.tex",
        "\\catcode123=1\n\\catcode125=2\n\\interactionmode=1\n\\errmessage{VISIBLE IN NONSTOP}\n\\dump\n",
    );
    let output = leaving_batch.compile(&["-ini", "-interaction=batchmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("VISIBLE IN NONSTOP"), "{stderr}");
    assert!(stderr.contains("format build reported 1 error"), "{stderr}");
}

#[test]
fn cached_format_fallback_respects_batch_mode() {
    let source = r"\documentclass{article}
\begin{document}
Format fallback.
\end{document}
";

    let batch = Job::new("bad-cwd-format-batch");
    batch.write("main.tex", source);
    batch.write_bytes("pdflatex.fmt", b"not a TeX format");
    let batch_output = batch.compile(&["-interaction=batchmode"]);
    assert!(
        batch_output.status.success(),
        "{}",
        failure_output(&batch_output)
    );
    assert!(
        batch_output.stdout.is_empty(),
        "{}",
        failure_output(&batch_output)
    );
    assert!(
        batch_output.stderr.is_empty(),
        "{}",
        failure_output(&batch_output)
    );

    let visible = Job::new("bad-cwd-format-visible");
    visible.write("main.tex", source);
    visible.write_bytes("pdflatex.fmt", b"not a TeX format");
    let visible_output = visible.compile(&["-interaction=nonstopmode"]);
    assert!(
        visible_output.status.success(),
        "{}",
        failure_output(&visible_output)
    );
    let stderr = text(&visible_output.stderr);
    assert!(
        stderr.contains("pdflatex: cannot use format pdflatex.fmt"),
        "{stderr}"
    );
    assert!(stderr.contains("trying another format source"), "{stderr}");
}

#[test]
fn startup_filesystem_errors_respect_batch_mode() {
    let batch = Job::new("startup-filesystem-batch");
    batch.write("main.tex", "\\end\n");
    batch.write("blocked-output", "this path is a file");
    let batch_output = batch.compile(&[
        "-plain",
        "-interaction=batchmode",
        "-output-directory=blocked-output",
    ]);
    assert_eq!(
        batch_output.status.code(),
        Some(1),
        "{}",
        failure_output(&batch_output)
    );
    assert!(
        batch_output.stdout.is_empty(),
        "{}",
        failure_output(&batch_output)
    );
    assert!(
        batch_output.stderr.is_empty(),
        "{}",
        failure_output(&batch_output)
    );

    let visible = Job::new("startup-filesystem-visible");
    visible.write("main.tex", "\\end\n");
    visible.write("blocked-output", "this path is a file");
    let visible_output = visible.compile(&[
        "-plain",
        "-interaction=nonstopmode",
        "-output-directory=blocked-output",
    ]);
    assert_eq!(
        visible_output.status.code(),
        Some(1),
        "{}",
        failure_output(&visible_output)
    );
    let stderr = text(&visible_output.stderr);
    let expected = format!(
        "pdflatex: cannot create output directory blocked-output{}",
        std::path::MAIN_SEPARATOR
    );
    assert!(stderr.contains(&expected), "{stderr}");
}

#[test]
fn ini_mode_does_not_write_a_format_after_a_recoverable_error() {
    let job = Job::new("errored-format-build");
    job.write(
        "main.tex",
        "\\catcode123=1\n\\catcode125=2\n\\errmessage{FORMAT BUILD SENTINEL}\n\\dump\n",
    );

    let output = job.compile(&["-ini", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("FORMAT BUILD SENTINEL"), "{stderr}");
    assert!(
        stderr.contains("format build reported 1 error; pdflatex.fmt was not written"),
        "{stderr}"
    );
    assert!(!job.dir.join("pdflatex.fmt").exists());
    assert!(job.log().contains("FORMAT BUILD SENTINEL"));

    let batch = Job::new("errored-format-build-batch");
    batch.write(
        "main.tex",
        "\\catcode123=1\n\\catcode125=2\n\\errmessage{BATCH FORMAT BUILD SENTINEL}\n\\dump\n",
    );
    let batch_output = batch.compile(&["-ini", "-interaction=batchmode"]);
    assert_eq!(
        batch_output.status.code(),
        Some(1),
        "{}",
        failure_output(&batch_output)
    );
    assert!(
        batch_output.stdout.is_empty(),
        "{}",
        failure_output(&batch_output)
    );
    assert!(
        batch_output.stderr.is_empty(),
        "{}",
        failure_output(&batch_output)
    );
    assert!(!batch.dir.join("pdflatex.fmt").exists());
    assert!(
        batch.log().contains("BATCH FORMAT BUILD SENTINEL"),
        "{}",
        batch.log()
    );
}

#[test]
fn ini_format_write_failure_is_structured_logged_and_batch_aware() {
    for mode in ["nonstopmode", "batchmode"] {
        let job = Job::new(&format!("format-write-{mode}"));
        job.write("main.tex", "\\catcode123=1\n\\catcode125=2\n\\dump\n");
        std::fs::create_dir(job.dir.join("pdflatex.fmt")).unwrap();

        let output = job.compile(&["-ini", &format!("-interaction={mode}")]);
        assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
        let log = job.log();
        assert!(
            log.contains("! Cannot write format `pdflatex.fmt`"),
            "{log}"
        );
        assert!(
            log.contains("check that the working directory is writable"),
            "{log}"
        );
        if mode == "batchmode" {
            assert!(output.stdout.is_empty(), "{}", failure_output(&output));
            assert!(output.stderr.is_empty(), "{}", failure_output(&output));
        } else {
            assert!(
                text(&output.stderr).contains("Cannot write format `pdflatex.fmt`"),
                "{}",
                failure_output(&output)
            );
        }
    }
}

#[test]
fn recursive_input_capacity_is_a_logged_fatal_error_in_batch_mode() {
    let job = Job::new("recursive-input-capacity");
    job.write("main.tex", "\\input{main.tex}\n");

    let output = job.compile(&["-interaction=batchmode", "-halt-on-error"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    assert!(output.stdout.is_empty(), "{}", failure_output(&output));
    assert!(output.stderr.is_empty(), "{}", failure_output(&output));

    let log = job.log();
    let marker = "TeX capacity exceeded, sorry [input stack size=5000]";
    assert_eq!(occurrences(&log, marker), 1, "{log}");
    assert!(log.contains("main.tex:1:1"), "{log}");
    assert!(!log.contains("panicked at"), "{log}");
    assert!(!log.contains("stack backtrace"), "{log}");
}

#[test]
fn recursive_macro_capacity_is_a_logged_fatal_error_in_batch_mode() {
    let job = Job::new("recursive-macro-capacity");
    job.write(
        "main.tex",
        "\\errorcontextlines=5\\def\\a{\\b x}\\def\\b{\\a y}\\a\\end\n",
    );

    let output = job.compile(&["-interaction=batchmode", "-halt-on-error"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    assert!(output.stdout.is_empty(), "{}", failure_output(&output));
    assert!(output.stderr.is_empty(), "{}", failure_output(&output));

    let log = job.log();
    let marker = "TeX capacity exceeded, sorry [input stack size=5000]";
    assert_eq!(occurrences(&log, marker), 1, "{log}");
    assert!(log.contains("main.tex:1:"), "{log}");
    assert!(log.contains("while expanding"), "{log}");
    assert!(!log.contains("panicked at"), "{log}");
    assert!(!log.contains("stack backtrace"), "{log}");
}

#[test]
fn batch_mode_never_leaks_internal_debug_dumps() {
    for (label, source, expected) in [
        (
            "batch-malformed-definition",
            "\\def a{}\\end\n",
            "Missing control sequence inserted",
        ),
        (
            "batch-misplaced-eqno",
            "\\eqno\\end\n",
            "You can't use \\eqno here",
        ),
        (
            "batch-misplaced-noalign",
            "\\noalign{}\\end\n",
            "Misplaced \\noalign",
        ),
    ] {
        let job = Job::new(label);
        job.write("main.tex", source);
        let output = job.compile(&["-plain", "-interaction=batchmode", "-halt-on-error"]);
        assert!(!output.status.success(), "{}", failure_output(&output));
        assert!(output.stdout.is_empty(), "{}", failure_output(&output));
        assert!(output.stderr.is_empty(), "{}", failure_output(&output));
        let log = job.log();
        assert!(log.contains(expected), "{log}");
        assert!(!log.contains("DEF-NONCS"), "{log}");
        assert!(!log.contains("EQNO FAILED"), "{log}");
        assert!(!log.contains("dump:"), "{log}");
    }

    let write = Job::new("batch-write-debug-marker");
    write.write("main.tex", "\\immediate\\write16{futurelet}\\end\n");
    let output = write.compile(&["-plain", "-interaction=batchmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    assert!(output.stdout.is_empty(), "{}", failure_output(&output));
    assert!(output.stderr.is_empty(), "{}", failure_output(&output));
    assert!(!write.log().contains("CORRUPTED"));
}

#[test]
fn help_succeeds_and_unknown_options_are_actionable_usage_errors() {
    let help = Command::new(env!("CARGO_BIN_EXE_pdflatex"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success(), "{}", failure_output(&help));
    let help_text = format!("{}{}", text(&help.stdout), text(&help.stderr));
    assert!(help_text.contains("usage: pdflatex [options] file.tex"));
    assert!(help_text.contains("-halt-on-error"));
    assert!(help_text.contains("-interaction MODE"));

    let shell_disabled = Command::new(env!("CARGO_BIN_EXE_pdflatex"))
        .args(["-no-shell-escape", "--version"])
        .output()
        .unwrap();
    assert!(
        shell_disabled.status.success(),
        "{}",
        failure_output(&shell_disabled)
    );

    let unknown = Command::new(env!("CARGO_BIN_EXE_pdflatex"))
        .arg("--definitely-not-a-real-option")
        .output()
        .unwrap();
    assert!(!unknown.status.success(), "{}", failure_output(&unknown));
    assert_eq!(
        unknown.status.code(),
        Some(2),
        "{}",
        failure_output(&unknown)
    );
    let stderr = text(&unknown.stderr);
    assert!(
        stderr.contains("unknown option '--definitely-not-a-real-option'"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Try 'pdflatex --help' for usage."),
        "{stderr}"
    );
}

#[test]
fn end_of_input_reports_unclosed_conditional_and_group() {
    let job = Job::new("unfinished-structures");
    job.write("main.tex", "\\iftrue\n{\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert_eq!(occurrences(&stderr, "Unclosed conditional"), 1, "{stderr}");
    assert_eq!(occurrences(&stderr, "Unclosed group"), 1, "{stderr}");
    assert!(stderr.contains("missing \\fi"), "{stderr}");
    assert!(stderr.contains("main.tex:1"), "{stderr}");

    let log = job.log();
    assert_eq!(occurrences(&log, "Unclosed conditional"), 1, "{log}");
    assert_eq!(occurrences(&log, "Unclosed group"), 1, "{log}");
}

#[test]
fn tail_macro_trace_keeps_outer_and_inner_but_drops_a_completed_sibling() {
    let job = Job::new("tail-macro-trace");
    job.write(
        "main.tex",
        r"\errorcontextlines=5
\def\sibling{\relax}
\sibling
\def\outer{\inner}
\def\inner{\undefinedTailCommand}
\outer
\end
",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("= while expanding: \\outer -> \\inner"),
        "{stderr}"
    );
    assert!(!stderr.contains("while expanding: \\sibling"), "{stderr}");
    assert!(stderr.contains("main.tex:6:1"), "{stderr}");
    assert!(stderr.contains("6 | \\outer"), "{stderr}");
    let caret = line_after(&stderr, "6 | \\outer").expect("caret after source line");
    assert!(caret.ends_with("| ^^^^^^"), "caret={caret:?}\n{stderr}");
}

#[test]
fn aliased_macro_diagnostic_uses_the_spelling_at_the_call_site() {
    let job = Job::new("aliased-macro-spelling");
    job.write(
        "main.tex",
        r"\def\longdescriptivename{\undefinedAliasTail}
\let\x=\longdescriptivename
\x
\end
",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Undefined control sequence \\undefinedAliasTail"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:3:1"), "{stderr}");
    assert!(stderr.contains("3 | \\x"), "{stderr}");
    let caret = line_after(&stderr, "3 | \\x").expect("caret after aliased call site");
    assert!(caret.ends_with("| ^^"), "caret={caret:?}\n{stderr}");
    assert!(stderr.contains("= while expanding: \\x"), "{stderr}");
    assert!(
        !stderr.contains("while expanding: \\longdescriptivename"),
        "{stderr}"
    );
}

#[test]
fn runaway_definition_in_an_include_points_to_the_child_and_names_the_parent() {
    let job = Job::new("included-runaway-definition");
    job.write("main.tex", "\\input child.tex\n\\end\n");
    job.write("child.tex", "\\def\\broken#1{unfinished\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("File ended while scanning the definition of \\broken"),
        "{stderr}"
    );
    assert!(stderr.contains("child.tex:1:1"), "{stderr}");
    assert!(
        stderr.contains("1 | \\def\\broken#1{unfinished"),
        "{stderr}"
    );
    assert!(stderr.contains("= included from main.tex:1:"), "{stderr}");
}

#[test]
fn direct_error_in_an_include_hides_latex_file_hook_scratch_macros() {
    let job = Job::new("included-direct-error");
    job.write(
        "main.tex",
        "\\documentclass{article}\n\\def\\loadchapter{\\input{chapter}}\n\\begin{document}\n\\loadchapter\n\\end{document}\n",
    );
    job.write("chapter.tex", "Result: \\undefinedResult\n");

    let output = job.compile(&["-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Undefined control sequence \\undefinedResult"),
        "{stderr}"
    );
    assert!(stderr.contains("chapter.tex:1:9"), "{stderr}");
    assert!(stderr.contains("included from main.tex:4:1"), "{stderr}");
    assert!(!stderr.contains("\\reserved@"), "{stderr}");
    assert!(!stderr.contains("\\@swaptwoargs"), "{stderr}");
    assert!(!stderr.contains("while expanding:"), "{stderr}");
}

#[test]
fn alignment_preamble_eof_in_an_include_points_to_the_halign() {
    let job = Job::new("included-alignment-preamble-eof");
    job.write("main.tex", "\\input child.tex\n");
    job.write("child.tex", "\\halign{#\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("File ended while scanning an alignment preamble"),
        "{stderr}"
    );
    assert!(stderr.contains("child.tex:1:1"), "{stderr}");
    assert!(stderr.contains("1 | \\halign{#"), "{stderr}");
    assert!(stderr.contains("= included from main.tex:1:1"), "{stderr}");
}

#[test]
fn alignment_row_eof_in_an_include_points_to_the_halign() {
    let job = Job::new("included-alignment-row-eof");
    job.write("main.tex", "\\input child.tex\n");
    job.write("child.tex", "\\halign{#\\cr a\\cr\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("File ended during an alignment"),
        "{stderr}"
    );
    assert!(stderr.contains("child.tex:1:1"), "{stderr}");
    assert!(stderr.contains("1 | \\halign{#\\cr a\\cr"), "{stderr}");
    assert!(stderr.contains("= included from main.tex:1:1"), "{stderr}");
}

#[test]
fn expanded_text_eof_in_an_include_points_to_the_opening_brace() {
    let job = Job::new("included-expanded-text-eof");
    job.write("main.tex", "\\input child.tex\n");
    job.write("child.tex", "\\message{unfinished\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("Missing } in expanded text"), "{stderr}");
    assert!(stderr.contains("child.tex:1:9"), "{stderr}");
    assert!(stderr.contains("1 | \\message{unfinished"), "{stderr}");
    assert!(stderr.contains("= included from main.tex:1:1"), "{stderr}");
    assert!(!stderr.contains("\nunfinished "), "{stderr}");
}

#[test]
fn balanced_text_eof_in_an_include_points_to_the_opening_brace() {
    let job = Job::new("included-balanced-text-eof");
    job.write("main.tex", "\\input child.tex\n");
    job.write("child.tex", "\\toks0={unfinished\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("Runaway argument / missing }"), "{stderr}");
    assert!(stderr.contains("child.tex:1:8"), "{stderr}");
    assert!(stderr.contains("1 | \\toks0={unfinished"), "{stderr}");
    assert!(stderr.contains("= included from main.tex:1:1"), "{stderr}");
}

#[test]
fn runaway_macro_argument_keeps_its_call_trace_and_include_site() {
    let job = Job::new("included-runaway-argument");
    job.write("main.tex", "\\def\\take#1{}\n\\input child.tex\n\\end\n");
    job.write("child.tex", "\\take{unterminated\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("Runaway argument / missing }"), "{stderr}");
    assert!(stderr.contains("child.tex:1:1"), "{stderr}");
    assert!(stderr.contains("1 | \\take{unterminated"), "{stderr}");
    let caret = line_after(&stderr, "1 | \\take{unterminated").expect("caret after call");
    assert!(caret.ends_with("| ^^^^^"), "caret={caret:?}\n{stderr}");
    assert!(stderr.contains("= while expanding: \\take"), "{stderr}");
    assert!(stderr.contains("= included from main.tex:2:1"), "{stderr}");
}

#[test]
fn nested_macro_argument_error_keeps_the_complete_expansion_path() {
    let job = Job::new("nested-argument-trace");
    job.write(
        "main.tex",
        "\\def\\id#1{#1}\n\\def\\outer{\\id{\\undefinedGrouped}}\n\\outer\n\\end\n",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("= while expanding: \\outer -> \\id"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:3:1"), "{stderr}");
}

#[test]
fn deep_macro_trace_keeps_the_outer_call_consistent_with_the_caret() {
    let job = Job::new("deep-macro-trace");
    let mut source = String::from("\\errorcontextlines=5\n");
    for index in 0..25 {
        let name = char::from(b'a' + index as u8);
        let next = if index == 24 {
            "\\undefinedDeepTrace".to_string()
        } else {
            format!("\\m{}", char::from(b'a' + index as u8 + 1))
        };
        source.push_str(&format!("\\def\\m{name}{{{next}}}\n"));
    }
    source.push_str("\\ma\n\\end\n");
    job.write("main.tex", &source);

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("main.tex:27:1"), "{stderr}");
    assert!(stderr.contains("27 | \\ma"), "{stderr}");
    let caret = line_after(&stderr, "27 | \\ma").expect("caret after outer macro");
    assert!(caret.ends_with("| ^^^"), "caret={caret:?}\n{stderr}");
    assert!(
        stderr.contains("= while expanding: \\ma -> … -> \\mv -> \\mw -> \\mx -> \\my"),
        "{stderr}"
    );
    assert!(!stderr.contains("\\ma -> \\mg"), "{stderr}");

    let capped = Job::new("stored-trace-cap-is-visible");
    let capped_source = source.replacen("\\errorcontextlines=5", "\\errorcontextlines=20", 1);
    capped.write("main.tex", &capped_source);
    let output = capped.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    let expansion = stderr
        .lines()
        .find(|line| line.contains("= while expanding:"))
        .expect("macro expansion line");
    assert!(expansion.contains("\\ma -> … -> \\mg"), "{expansion}");
    assert_eq!(occurrences(expansion, "…"), 1, "{expansion}");
}

#[test]
fn macro_generated_runaway_definition_keeps_the_generating_macro() {
    let job = Job::new("macro-generated-runaway-definition");
    job.write(
        "main.tex",
        "\\def\\make{\\def\\broken##1}\n\\make{unfinished\n",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("File ended while scanning the definition of \\broken"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:2:1"), "{stderr}");
    assert!(stderr.contains("= while expanding: \\make"), "{stderr}");
}

#[test]
fn missing_input_file_points_to_the_input_command() {
    let job = Job::new("missing-input-location");
    job.write(
        "main.tex",
        "\\input definitely-missing-diagnostic-file.tex\n\\end\n",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("File `definitely-missing-diagnostic-file.tex` not found"));
    assert!(stderr.contains("main.tex:1:1"), "{stderr}");
    assert!(
        stderr.contains("1 | \\input definitely-missing-diagnostic-file.tex"),
        "{stderr}"
    );
}

#[test]
fn missing_latex_package_reports_the_file_at_the_use_site_without_a_prompt() {
    let job = Job::new("missing-latex-package");
    job.write(
        "main.tex",
        r"\documentclass{article}
\usepackage{definitelymissingdiagnosticpackage}
\begin{document}x\end{document}
",
    );

    let output = job.compile(&["-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    let marker = "LaTeX Error: File `definitelymissingdiagnosticpackage.sty' not found.";
    assert_eq!(occurrences(&stderr, marker), 1, "{stderr}");
    assert!(
        stderr.contains("main.tex:2:13"),
        "missing package was not located at its name:\n{stderr}"
    );
    assert!(
        stderr.contains("2 | \\usepackage{definitelymissingdiagnosticpackage}"),
        "{stderr}"
    );
    assert!(stderr.contains("check the file name and path"), "{stderr}");
    for obsolete in ["Enter file name:", "Bad input stream", "Type X", "<return>"] {
        assert!(!stderr.contains(obsolete), "{stderr}");
    }
    assert!(stdout.contains("Document Class: article"), "{stdout}");
    assert!(!stderr.contains("Document Class: article"), "{stderr}");
}

#[test]
fn latex_environment_mismatch_keeps_only_the_actionable_error() {
    let job = Job::new("latex-environment-mismatch");
    job.write(
        "main.tex",
        r"\documentclass{article}
\begin{document}
\begin{center}
x
\end{flushleft}
\end{document}
",
    );

    let output = job.compile(&["-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("cannot close flushleft while center is still open"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:5:1"), "{stderr}");
    assert!(stderr.contains("5 | \\end{flushleft}"), "{stderr}");
    assert!(
        stderr.contains("center") && stderr.contains("flushleft"),
        "{stderr}"
    );
    let log = job.log();
    assert!(
        log.contains("LaTeX Error: \\begin{center} on input line 3 ended by \\end{flushleft}."),
        "{log}"
    );
    for legacy in [
        "Type  H",
        "Type H",
        "Type I",
        "<return>",
        "Your command was ignored",
    ] {
        assert!(!stderr.contains(legacy), "{stderr}");
    }
    assert!(!stderr.contains("while expanding:"), "{stderr}");
    job.write(
        "main.tex",
        r"\documentclass{article}
\begin{document}
\begin{center}
x
\end{center}
\end{document}
",
    );
    let fixed = job.compile(&["-interaction=nonstopmode", "-halt-on-error"]);
    assert!(fixed.status.success(), "{}", failure_output(&fixed));
}

#[test]
fn negative_error_context_uses_a_bounded_trace_while_zero_opts_out() {
    let source = |context| {
        format!(
            "\\errorcontextlines={context}\n\\def\\outer{{\\inner}}\n\\def\\inner{{\\undefinedContextCommand}}\n\\outer\n\\end\n"
        )
    };
    let default_job = Job::new("negative-error-context");
    default_job.write("main.tex", &source(-1));
    let default_output =
        default_job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(
        !default_output.status.success(),
        "{}",
        failure_output(&default_output)
    );
    let default_stderr = text(&default_output.stderr);
    assert!(
        default_stderr.contains("= while expanding: \\outer -> \\inner"),
        "{default_stderr}"
    );

    let zero_job = Job::new("zero-error-context-explicit");
    zero_job.write("main.tex", &source(0));
    let zero_output = zero_job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(
        !zero_output.status.success(),
        "{}",
        failure_output(&zero_output)
    );
    let zero_stderr = text(&zero_output.stderr);
    assert!(!zero_stderr.contains("while expanding:"), "{zero_stderr}");
    assert!(default_stderr.contains("main.tex:4:1"), "{default_stderr}");
    assert!(zero_stderr.contains("main.tex:4:1"), "{zero_stderr}");
}

#[test]
fn include_ancestry_does_not_compete_with_the_macro_trace_budget() {
    let job = Job::new("independent-include-context");
    job.write(
        "main.tex",
        "\\errorcontextlines=2\n\\input first.tex\n\\end\n",
    );
    job.write("first.tex", "\\input second.tex\n");
    job.write(
        "second.tex",
        "\\def\\outer{\\middle}\n\\def\\middle{\\undefinedIncluded}\n\\outer\n",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("while expanding: \\outer -> \\middle"),
        "{stderr}"
    );
    assert!(stderr.contains("included from first.tex:1:1"), "{stderr}");
    assert!(stderr.contains("included from main.tex:2:1"), "{stderr}");
}

#[test]
fn arithmetic_faults_name_the_operation_and_preserve_the_value() {
    for (label, initial, operation, expected, value) in [
        (
            "divide-by-zero",
            "42",
            "\\divide\\count0 by 0",
            "Cannot divide by zero in \\divide; value left unchanged",
            "VALUE=42",
        ),
        (
            "multiply-overflow",
            "2147483647",
            "\\multiply\\count0 by 2",
            "Arithmetic overflow in \\multiply; value left unchanged",
            "VALUE=2147483647",
        ),
        (
            "negative-multiply-overflow",
            "-1073741824",
            "\\multiply\\count0 by 2",
            "Arithmetic overflow in \\multiply; value left unchanged",
            "VALUE=-1073741824",
        ),
    ] {
        let job = Job::new(label);
        job.write(
            "main.tex",
            &format!("\\count0={initial}\n{operation}\n\\message{{VALUE=\\the\\count0}}\n\\end\n"),
        );
        let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
        assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert_eq!(occurrences(&stderr, expected), 1, "{stderr}");
        assert!(stderr.contains("main.tex:2:1"), "{stderr}");
        assert!(
            text(&output.stdout).contains(value),
            "{}",
            failure_output(&output)
        );
    }
}

#[test]
fn oversized_numbers_and_dimensions_report_the_limit_and_clamped_value() {
    for (label, assignment, expected, help, value) in [
        (
            "integer-literal-overflow",
            "\\count0=999999999999999999999",
            "Number too big",
            "integer no larger than 2147483647",
            "VALUE=2147483647",
        ),
        (
            "dimension-literal-overflow",
            "\\dimen0=20000pt",
            "Dimension too large",
            "dimension no larger than 16383.99998pt",
            "VALUE=16383.99998pt",
        ),
    ] {
        let job = Job::new(label);
        let register = if label == "integer-literal-overflow" {
            "\\count0"
        } else {
            "\\dimen0"
        };
        job.write(
            "main.tex",
            &format!("{assignment}\n\\message{{VALUE=\\the{register}}}\n\\end\n"),
        );
        let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
        assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert_eq!(occurrences(&stderr, expected), 1, "{stderr}");
        assert!(stderr.contains("main.tex:1:"), "{stderr}");
        assert!(stderr.contains(help), "{stderr}");
        assert!(
            text(&output.stdout).contains(value),
            "{}",
            failure_output(&output)
        );
    }
}

#[test]
fn math_glue_arithmetic_reports_faults_and_preserves_the_value() {
    let job = Job::new("math-glue-arithmetic");
    job.write(
        "main.tex",
        "\\muskip0=6mu plus 2fil\n\\divide\\muskip0 by 2\n\\message{HALVED=\\the\\muskip0}\n\\divide\\muskip0 by 0\n\\message{PRESERVED=\\the\\muskip0}\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stdout = text(&output.stdout);
    assert!(stdout.contains("HALVED=3.0mu plus 1.0fil"), "{stdout}");
    assert!(stdout.contains("PRESERVED=3.0mu plus 1.0fil"), "{stdout}");
    let stderr = text(&output.stderr);
    assert_eq!(
        occurrences(
            &stderr,
            "Cannot divide by zero in \\divide; value left unchanged"
        ),
        1,
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:4:1"), "{stderr}");
}

#[test]
fn advance_and_divide_accept_texs_minimum_integer() {
    let job = Job::new("minimum-integer-arithmetic");
    job.write(
        "main.tex",
        "\\count0=-2147483647\n\\advance\\count0 by -1\n\\divide\\count0 by 1\n\\message{VALUE=\\the\\count0}\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    assert!(
        text(&output.stdout).contains("VALUE=-2147483648"),
        "{}",
        failure_output(&output)
    );
    assert!(
        !text(&output.stderr).contains("! "),
        "{}",
        failure_output(&output)
    );
}

#[test]
fn read_tokenization_preserves_the_enclosing_macro_location() {
    let job = Job::new("read-provenance");
    job.write("data.txt", "abc\n");
    job.write(
        "main.tex",
        "\\openin0=data.txt\n\\def\\outer{\\read0 to \\answer \\undefinedAfterRead}\n\\outer\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Undefined control sequence \\undefinedAfterRead"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:3:1"), "{stderr}");
    assert!(stderr.contains("3 | \\outer"), "{stderr}");
    assert!(stderr.contains("= while expanding: \\outer"), "{stderr}");
}

#[test]
fn malformed_macro_parameters_explain_the_expected_number() {
    let parameter_text = Job::new("bad-parameter-order");
    parameter_text.write("main.tex", "\\def\\bad#2{x}\n\\end\n");
    let output = parameter_text.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("expected #1 but found #2"), "{stderr}");
    assert!(stderr.contains("main.tex:1:7"), "{stderr}");

    let replacement = Job::new("bad-parameter-reference");
    replacement.write("main.tex", "\\def\\bad#1{#2}\n\\bad{x}\n\\end\n");
    let output = replacement.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Illegal parameter reference #2 in the definition of \\bad"),
        "{stderr}"
    );
    assert!(
        stderr.contains("only #1 through #1 are declared"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:1:13"), "{stderr}");

    let no_parameters = Job::new("bad-parameter-reference-without-parameters");
    no_parameters.write("main.tex", "\\def\\bad{#1}\n\\end\n");
    let output = no_parameters.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(
            "Illegal parameter reference #1 in the definition of \\bad; this macro declares no parameters"
        ),
        "{stderr}"
    );
}

#[test]
fn invalid_character_table_assignments_report_values_and_valid_ranges() {
    for (label, assignment, expected) in [
        (
            "invalid-category-code",
            "\\catcode65=16",
            "Category code 16 is out of range; expected 0 through 15",
        ),
        (
            "invalid-character-target",
            "\\catcode256=11",
            "Character code 256 is out of range for \\catcode; expected 0 through 255",
        ),
        (
            "invalid-math-code",
            "\\mathcode65=40000",
            "Math code 40000 is out of range; expected 0 through 32768",
        ),
        (
            "invalid-delimiter-code",
            "\\delcode65=20000000",
            "Delimiter code 20000000 is out of range; expected -1 through 16777215",
        ),
        (
            "invalid-space-factor-code",
            "\\sfcode65=40000",
            "Space-factor code 40000 is out of range; expected 0 through 32767",
        ),
        (
            "invalid-mathchardef-code",
            "\\mathchardef\\bad=32768",
            "Math code 32768 is out of range for \\mathchardef; expected 0 through 32767",
        ),
    ] {
        let job = Job::new(label);
        job.write("main.tex", &format!("{assignment}\n\\end\n"));
        let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
        assert!(!output.status.success(), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert_eq!(occurrences(&stderr, expected), 1, "{stderr}");
        assert!(stderr.contains("main.tex:1:"), "{stderr}");
    }
}

#[test]
fn invalid_character_definitions_report_and_use_texs_recovery_value() {
    for (label, definition) in [
        ("chardef-recovery", "\\chardef\\bad=1114112"),
        ("mathchardef-recovery", "\\mathchardef\\bad=32768"),
    ] {
        let job = Job::new(label);
        job.write(
            "main.tex",
            &format!("{definition}\n\\message{{RECOVERED=\\the\\bad}}\n\\end\n"),
        );
        let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
        assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert!(stderr.contains("main.tex:1:"), "{stderr}");
        assert!(text(&output.stdout).contains("RECOVERED=0"));
    }
}

#[test]
fn invalid_character_table_queries_are_located_and_never_panic() {
    for (label, command, invalid) in [
        ("query-catcode", "\\catcode", 256),
        ("query-mathcode", "\\mathcode", 256),
        ("query-delcode", "\\delcode", 256),
        ("query-lccode", "\\lccode", 1114112),
        ("query-sfcode", "\\sfcode", 256),
        ("query-uccode", "\\uccode", 1114112),
    ] {
        let job = Job::new(label);
        let source = format!("\\message{{VALUE=\\the{command}{invalid}}}\n\\end\n");
        let operand_column = source.find(&invalid.to_string()).unwrap() + 1;
        job.write("main.tex", &source);
        let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
        assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert!(
            stderr.contains(&format!("main.tex:1:{operand_column}")),
            "{stderr}"
        );
        assert!(!stderr.contains("panicked at"), "{stderr}");
        assert!(text(&output.stdout).contains("VALUE="));
    }
}

#[test]
fn invalid_font_metric_character_uses_zero_with_a_located_error() {
    for value in ["256", "-1"] {
        let job = Job::new(&format!("font-metric-character-{value}"));
        let query = format!("\\message{{VALUE=\\the\\fontcharwd\\ten{value}}}");
        let operand_column = query.rfind(value).unwrap() + 1;
        job.write("main.tex", &format!("\\font\\ten=cmr10\n{query}\n\\end\n"));
        let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
        assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert!(
            stderr.contains(&format!(
                "Character code {value} is out of range for \\fontcharwd; expected 0 through 255 and used character 0"
            )),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!("main.tex:2:{operand_column}")),
            "{stderr}"
        );
        assert!(text(&output.stdout).contains("VALUE=6.25002pt"));
    }
}

#[test]
fn invalid_math_family_reports_the_operand_and_recovers_with_family_zero() {
    let job = Job::new("math-family-range");
    job.write(
        "main.tex",
        "\\font\\ten=cmr10\n\\textfont0=\\nullfont\n\\textfont16=\\ten\n\\message{RECOVERED=\\the\\textfont0}\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(
            "Font family 16 is out of range for \\textfont; expected 0 through 15 and used family 0"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:3:10"), "{stderr}");
    assert!(text(&output.stdout).contains("RECOVERED=\\ten"));
}

#[test]
fn invalid_radical_and_font_character_codes_are_located_and_recoverable() {
    let job = Job::new("math-and-font-character-ranges");
    job.write(
        "main.tex",
        "\\font\\ten=cmr10\\relax\n\\iffontchar\\ten256 \\message{TRUE}\\else\\message{FALSE}\\fi\\message{AFTER CONDITIONAL}\n$\\radical134217728 x$\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(
            "Character code 256 is out of range for \\iffontchar; expected 0 through 255 and used character 0"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "Delimiter code 134217728 is out of range for \\radical; expected 0 through 134217727 and used 0"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:2:16"), "{stderr}");
    assert!(stderr.contains("main.tex:3:10"), "{stderr}");
    assert!(text(&output.stdout).contains("AFTER CONDITIONAL"));
}

#[test]
fn character_and_math_commands_report_their_own_numeric_contracts() {
    for (label, source, expected) in [
        (
            "char-code-range",
            "\\char256\n\\end\n",
            "Character code 256 is out of range for \\char; expected 0 through 255 and used 0",
        ),
        (
            "mathchar-code-range",
            "$\\mathchar32768$\n\\end\n",
            "Math character code 32768 is out of range for \\mathchar; expected 0 through 32767 and used 0",
        ),
        (
            "mathaccent-code-range",
            "$\\mathaccent32768 x$\n\\end\n",
            "Math character code 32768 is out of range for \\mathaccent; expected 0 through 32767 and used 0",
        ),
        (
            "text-accent-code-range",
            "\\font\\ten=cmr10\\relax\n\\setbox0=\\hbox{\\ten\\accent256 A}\n\\end\n",
            "Character code 256 is out of range for \\accent; expected 0 through 255 and used character 0",
        ),
    ] {
        let job = Job::new(label);
        job.write("main.tex", source);
        let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
        assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert_eq!(occurrences(&stderr, expected), 1, "{stderr}");
        assert!(stderr.contains("main.tex:"), "{stderr}");
        assert!(!stderr.contains("panicked at"), "{stderr}");
    }
}

#[test]
fn packing_warnings_are_structured_located_actionable_and_mode_aware() {
    let visible = Job::new("structured-pack-warnings");
    visible.write(
        "main.tex",
        "\\tracingonline=1\n\\hbox to 1pt{\\vrule width 10pt height 1pt}\n\\vbox to 1pt{\\hrule height 10pt}\n\\end\n",
    );
    let output = visible.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert!(stderr.contains("warning: Overfull \\hbox ("), "{stderr}");
    assert!(stderr.contains("warning: Overfull \\vbox ("), "{stderr}");
    assert!(stderr.contains("  --> main.tex:2:"), "{stderr}");
    assert!(stderr.contains("  --> main.tex:3:"), "{stderr}");
    assert!(
        stderr.contains("shorten or reflow the affected text"),
        "{stderr}"
    );
    assert!(
        stderr.contains("reduce the box contents or increase the available height"),
        "{stderr}"
    );
    assert!(!stdout.contains("Overfull \\hbox"), "{stdout}");
    assert!(!stdout.contains("Overfull \\vbox"), "{stdout}");
    let log = visible.log();
    assert_eq!(occurrences(&log, "warning: Overfull \\hbox"), 1, "{log}");
    assert_eq!(occurrences(&log, "warning: Overfull \\vbox"), 1, "{log}");

    let hidden = Job::new("pack-warning-tracingonline-zero");
    hidden.write(
        "main.tex",
        "\\tracingonline=0\n\\hbox to 1pt{\\vrule width 10pt height 1pt}\n\\end\n",
    );
    let output = hidden.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    assert!(
        !text(&output.stdout).contains("Overfull \\hbox"),
        "{}",
        failure_output(&output)
    );
    assert!(
        !text(&output.stderr).contains("Overfull \\hbox"),
        "{}",
        failure_output(&output)
    );
    assert!(
        hidden.log().contains("warning: Overfull \\hbox"),
        "{}",
        hidden.log()
    );

    let batch = Job::new("pack-warning-batch");
    batch.write(
        "main.tex",
        "\\tracingonline=1\n\\hbox to 1pt{\\vrule width 10pt height 1pt}\n\\end\n",
    );
    let output = batch.compile(&["-plain", "-interaction=batchmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    assert!(output.stdout.is_empty(), "{}", failure_output(&output));
    assert!(output.stderr.is_empty(), "{}", failure_output(&output));
    assert!(
        batch.log().contains("warning: Overfull \\hbox"),
        "{}",
        batch.log()
    );
}

#[test]
fn paragraph_overfull_warning_points_to_the_paragraph_end() {
    let job = Job::new("structured-paragraph-overfull");
    job.write(
        "main.tex",
        "\\tracingonline=1\n\\hsize=1pt\nabcdefghijklmnop\\par\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("warning: Overfull \\hbox (")
            && stderr.contains("in paragraph ending here"),
        "{stderr}"
    );
    assert!(stderr.contains("  --> main.tex:3:"), "{stderr}");
    assert!(stderr.contains("3 | abcdefghijklmnop\\par"), "{stderr}");
    assert!(
        stderr.contains("shorten or reflow the affected text"),
        "{stderr}"
    );
    assert!(!stdout.contains("Overfull \\hbox"), "{stdout}");
    assert!(
        job.log().contains("in paragraph ending here"),
        "{}",
        job.log()
    );
}

#[test]
fn missing_font_character_is_a_located_actionable_warning() {
    let job = Job::new("missing-font-character");
    job.write(
        "main.tex",
        "\\tracinglostchars=1\\font\\ten=cmr10\n\\setbox0=\\hbox{\\ten \\char128}\n\\message{AFTER WARNING}\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(
            "warning: Character code 128 (0x80) is not available in font `cmr10`; character omitted"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:2:29"), "{stderr}");
    assert!(
        stderr.contains("choose a font containing this character"),
        "{stderr}"
    );
    assert!(text(&output.stdout).contains("AFTER WARNING"));
    assert!(job.log().contains("Character code 128 (0x80)"));

    let batch = Job::new("missing-font-character-batch");
    batch.write(
        "main.tex",
        "\\tracinglostchars=1\\font\\ten=cmr10\n\\setbox0=\\hbox{\\ten \\char128}\n\\end\n",
    );
    let output = batch.compile(&["-plain", "-interaction=batchmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    assert!(output.stdout.is_empty(), "{}", failure_output(&output));
    assert!(output.stderr.is_empty(), "{}", failure_output(&output));
    assert!(batch.log().contains("Character code 128 (0x80)"));
}

#[test]
fn missing_math_characters_name_the_selected_font_and_keep_their_origin() {
    for (label, source, selected_font) in [
        (
            "missing-math-character",
            "$\\mathchar\"0180$\n\\end\n",
            "cmmi10",
        ),
        (
            "missing-script-math-character",
            "$x^{\\mathchar\"0180}$\n\\end\n",
            "cmmi7",
        ),
        (
            "missing-math-accent",
            "$\\mathaccent\"0180 x$\n\\end\n",
            "cmmi10",
        ),
        (
            "missing-math-delimiter",
            "$\\delimiter\"180000$\n\\end\n",
            "cmmi10",
        ),
        (
            "missing-radical-delimiter",
            "$\\radical\"180000 x$\n\\end\n",
            "cmmi10",
        ),
    ] {
        let job = Job::new(label);
        let source = format!(
            "\\tracinglostchars=1\\font\\mathtext=cmmi10\n\\font\\mathscript=cmmi7\n\\textfont1=\\mathtext\n\\scriptfont1=\\mathscript\n\\scriptscriptfont1=\\mathscript\n{source}"
        );
        job.write("main.tex", &source);
        let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
        assert!(output.status.success(), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        let marker = "warning: Character code 128 (0x80) is not available in font";
        assert_eq!(occurrences(&stderr, marker), 1, "{stderr}");
        assert!(stderr.contains(selected_font), "{stderr}");
        assert!(stderr.contains("selected for this math style"), "{stderr}");
        assert!(stderr.contains("main.tex:6:"), "{stderr}");
        assert!(stderr.contains("character omitted"), "{stderr}");
        assert!(
            stderr.contains("choose a font containing this character"),
            "{stderr}"
        );
        assert_eq!(
            occurrences(&job.log(), marker.trim_start_matches("warning: ")),
            1
        );
    }
}

#[test]
fn missing_scripted_accent_nucleus_warns_once_and_tracing_can_disable_it() {
    let warned = Job::new("missing-scripted-accent-nucleus");
    warned.write(
        "main.tex",
        "\\tracinglostchars=1\\font\\mathtext=cmmi10\n\\font\\mathscript=cmmi7\n\\textfont1=\\mathtext\n\\scriptfont1=\\mathscript\n\\scriptscriptfont1=\\mathscript\n$\\mathaccent\"015E \\mathchar\"0180^2$\n\\end\n",
    );
    let output = warned.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    let marker = "Character code 128 (0x80) is not available in font `cmmi10`";
    assert_eq!(occurrences(&stderr, marker), 1, "{stderr}");

    let quiet = Job::new("missing-math-character-disabled");
    quiet.write(
        "main.tex",
        "\\font\\mathtext=cmmi10\n\\textfont1=\\mathtext\n\\tracinglostchars=0\n$\\mathchar\"0180$\n\\end\n",
    );
    let output = quiet.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    assert!(!text(&output.stderr).contains("Character code 128"));
    assert!(!quiet.log().contains("Character code 128"));
}

#[test]
fn invalid_bytes_read_from_a_stream_blame_the_read_call_site() {
    let job = Job::new("read-invalid-character-location");
    job.write_bytes("data.txt", &[b'A', 0xff, b'B', b'\n']);
    job.write(
        "main.tex",
        "\\catcode255=15\n\\openin0=data.txt\n\\read0 to \\answer\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Text line contains an invalid character (byte 0xFF)"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:3:1"), "{stderr}");
    assert!(!stderr.contains("<read>:1"), "{stderr}");
}

#[test]
fn inspection_commands_produce_structured_bounded_diagnostics() {
    let job = Job::new("structured-inspection");
    job.write(
        "main.tex",
        "\\def\\foo#1{Hello #1}\n\\show\\foo\n\\count0=42\\showthe\\count0\n\\showtokens{abc\\foo}\n\\message{AFTER}\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Inspection requested by \\show\n"),
        "{stderr}"
    );
    assert!(
        stderr.contains("| \\foo = macro:#1 -> Hello #1"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Inspection requested by \\showthe\n"),
        "{stderr}"
    );
    assert!(stderr.contains("| value: 42"), "{stderr}");
    assert!(
        stderr.contains("Inspection requested by \\showtokens\n"),
        "{stderr}"
    );
    assert!(stderr.contains("| tokens: abc\\foo"), "{stderr}");
    assert!(stderr.contains("main.tex:2:1"), "{stderr}");
    assert!(
        stderr.len() < 16 * 1024,
        "inspection output was not bounded"
    );
    let stdout = text(&output.stdout);
    assert!(stdout.contains("AFTER"), "{stdout}");
    assert!(
        !stdout.contains("Hello abc"),
        "shown tokens leaked into input: {stdout}"
    );
}

#[test]
fn eof_syntax_and_terminal_read_errors_state_the_required_fix() {
    for (label, source, expected, help) in [
        (
            "braced-file-name-eof",
            "\\input{unfinished\n",
            "File ended while scanning a braced file name; add the missing }",
            "close the file name with `}`",
        ),
        (
            "quoted-file-name-eof",
            "\\input \"unfinished\n",
            "File ended while scanning a quoted file name; add the closing quote",
            "close the file name with a matching double quote",
        ),
        (
            "show-target-eof",
            "\\show\n",
            "File ended after \\show; add the token or control sequence to inspect",
            "place the token or control sequence to inspect immediately after `\\show`",
        ),
        (
            "terminal-read-unavailable",
            "\\read-1 to \\answer\n\\end\n",
            "Terminal input is unavailable for \\read-1",
            "file-backed stream",
        ),
        (
            "unopened-read-stream",
            "\\read0 to \\answer\n\\end\n",
            "Input stream 0 is not open for \\read",
            "open this stream with `\\openin`",
        ),
    ] {
        let job = Job::new(label);
        job.write("main.tex", source);
        let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
        assert!(!output.status.success(), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert_eq!(occurrences(&stderr, expected), 1, "{stderr}");
        assert!(stderr.contains("main.tex:1:1"), "{stderr}");
        assert!(stderr.contains(help), "{stderr}");
    }
}

#[test]
fn recoverable_error_before_raw_eof_does_not_hide_the_missing_end() {
    let job = Job::new("recoverable-before-missing-end");
    job.write("main.tex", "\\undefinedEarlier\n");
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert_eq!(
        occurrences(&stderr, "Undefined control sequence \\undefinedEarlier"),
        1
    );
    assert_eq!(
        occurrences(&stderr, "Emergency stop: no legal \\end found"),
        1
    );
    assert!(
        stderr.contains("compilation failed after 2 errors"),
        "{stderr}"
    );
}

#[test]
fn multiline_errmessage_cannot_spoof_source_or_help_records() {
    let job = Job::new("multiline-message-prefixes");
    job.write(
        "main.tex",
        "\\errmessage{first^^Jmain.tex:999:999^^J999 | fake^^J  = help: fake}\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("first\n  | main.tex:999:999"), "{stderr}");
    assert!(stderr.contains("\n  | 999 | fake"), "{stderr}");
    assert!(stderr.contains("\n  |  = help: fake"), "{stderr}");
    assert!(stderr.contains("  --> main.tex:1:"), "{stderr}");
}

#[test]
fn csname_generated_macro_points_to_the_csname_expression() {
    let job = Job::new("csname-origin");
    job.write(
        "main.tex",
        "\\def\\foo{\\undefinedInsideCsname}\n\\csname foo\\endcsname\n\\end\n",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("main.tex:2:1"), "{stderr}");
    assert!(stderr.contains("2 | \\csname foo\\endcsname"), "{stderr}");
    let caret = line_after(&stderr, "2 | \\csname foo\\endcsname").expect("caret after csname");
    assert!(caret.ends_with("| ^^^^^^^"), "caret={caret:?}\n{stderr}");
    assert!(stderr.contains("= while expanding: \\foo"), "{stderr}");
}

#[test]
fn explicit_end_warns_for_unclosed_structures_and_writes_output() {
    let job = Job::new("explicit-end-unclosed");
    job.write(
        "main.tex",
        "\\font\\ten=cmr10 \\ten\n\\iftrue\n{hello\\end\n",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    assert!(job.dir.join("main.pdf").is_file());
    let stdout = text(&output.stdout);
    assert!(!stdout.contains("Unclosed"), "{stdout}");
    let stderr = text(&output.stderr);
    assert_eq!(
        occurrences(&stderr, "warning: Unclosed conditional at \\end"),
        1,
        "{stderr}"
    );
    assert_eq!(
        occurrences(&stderr, "warning: Unclosed group at \\end"),
        1,
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:2:1"), "{stderr}");
    assert!(stderr.contains("2 | \\iftrue"), "{stderr}");
    assert!(stderr.contains("main.tex:3:1"), "{stderr}");
    assert!(stderr.contains("3 | {hello\\end"), "{stderr}");
    let log = job.log();
    assert!(log.contains("warning: Unclosed conditional at \\end"));
    assert!(log.contains("warning: Unclosed group at \\end"));
}

#[test]
fn long_csname_obeys_batch_mode_and_emits_one_bounded_error() {
    let job = Job::new("long-csname-batch");
    job.write(
        "main.tex",
        &format!("\\csname {}\\endcsname\n\\end\n", "a".repeat(2001)),
    );

    let output = job.compile(&["-plain", "-interaction=batchmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    assert!(output.stdout.is_empty(), "{}", failure_output(&output));
    assert!(output.stderr.is_empty(), "{}", failure_output(&output));
    let log = job.log();
    assert_eq!(
        occurrences(&log, "control sequence name exceeds 2000 bytes"),
        1,
        "{log}"
    );
    assert!(log.len() < 16 * 1024, "log grew to {} bytes", log.len());
}

#[test]
fn active_character_diagnostics_hide_internal_control_sequence_names() {
    let undefined = Job::new("undefined-active");
    undefined.write("main.tex", "\\catcode`\\!=13\n!\n\\end\n");
    let output = undefined.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("Undefined control sequence !"), "{stderr}");
    assert!(stderr.contains("main.tex:2:1"), "{stderr}");
    assert!(!stderr.contains("ACT"), "{stderr}");
    assert!(!stderr.contains('�'), "{stderr}");

    let expanded = Job::new("expanded-active");
    expanded.write(
        "main.tex",
        "\\catcode`\\!=13\n\\def!{\\undefinedFromActive}\n!\n\\end\n",
    );
    let output = expanded.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("main.tex:3:1"), "{stderr}");
    assert!(stderr.contains("= while expanding: !"), "{stderr}");
    assert!(!stderr.contains("ACT"), "{stderr}");
}

#[test]
fn active_conditional_opener_is_located_at_the_character() {
    let job = Job::new("active-conditional-location");
    job.write("main.tex", "\\catcode`\\!=13\n\\let!=\\iftrue\nabc !\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("Unclosed conditional"), "{stderr}");
    assert!(stderr.contains("main.tex:3:5"), "{stderr}");
    assert!(stderr.contains("3 | abc !"), "{stderr}");
}

#[test]
fn very_long_undefined_control_sequence_is_bounded_and_starts_at_column_one() {
    let job = Job::new("long-undefined-control-sequence");
    let name = "a".repeat(5000);
    job.write("main.tex", &format!("\\{name}\n\\end\n"));

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Undefined control sequence \\aaaa"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:1:1"), "{stderr}");
    assert!(
        stderr.len() < 8 * 1024,
        "stderr grew to {} bytes",
        stderr.len()
    );
    assert!(!stderr.contains(&"a".repeat(500)), "{stderr}");
}

#[test]
fn tab_and_wide_unicode_before_an_error_keep_the_caret_visually_aligned() {
    let job = Job::new("unicode-caret-alignment");
    job.write("main.tex", "\té漢 \\undefinedWide\n\\end\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("main.tex:1:8"), "{stderr}");
    let source = "1 |     é漢 \\undefinedWide";
    assert!(stderr.contains(source), "{stderr}");
    let caret = line_after(&stderr, source).expect("caret after Unicode source line");
    let after_gutter = caret
        .split_once("| ")
        .map(|(_, suffix)| suffix)
        .expect("caret line has a gutter");
    assert_eq!(
        after_gutter.chars().take_while(|ch| *ch == ' ').count(),
        8,
        "caret={caret:?}\n{stderr}"
    );
    assert!(after_gutter.trim_start().starts_with('^'), "{stderr}");
}

#[test]
fn macro_generated_unclosed_structures_point_to_the_outer_call() {
    let job = Job::new("macro-generated-structures");
    job.write("main.tex", "\\def\\openxx{\\iftrue\\bgroup}\n\\openxx\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert_eq!(occurrences(&stderr, "Unclosed conditional"), 1, "{stderr}");
    assert_eq!(occurrences(&stderr, "Unclosed group"), 1, "{stderr}");
    assert_eq!(occurrences(&stderr, "main.tex:2:1"), 2, "{stderr}");
    assert_eq!(occurrences(&stderr, "2 | \\openxx"), 2, "{stderr}");
    for caret in stderr
        .lines()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|pair| pair[0].contains("2 | \\openxx"))
        .map(|pair| pair[1])
    {
        assert!(
            caret
                .split_once("| ")
                .is_some_and(|(_, suffix)| suffix.starts_with('^')),
            "caret={caret:?}\n{stderr}"
        );
    }
}

#[test]
fn zero_error_context_lines_hides_trace_without_moving_the_caret() {
    let source = |context_lines| {
        format!(
            "\\errorcontextlines={context_lines}\n\\def\\sibling{{\\relax}}\n\\sibling\n\\def\\outer{{\\inner}}\n\\def\\inner{{\\undefinedTailCommand}}\n\\outer\n\\end\n"
        )
    };
    let default_job = Job::new("context-five");
    default_job.write("main.tex", &source(5));
    let default_output =
        default_job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(
        !default_output.status.success(),
        "{}",
        failure_output(&default_output)
    );

    let zero_job = Job::new("context-zero");
    zero_job.write("main.tex", &source(0));
    let zero_output = zero_job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(
        !zero_output.status.success(),
        "{}",
        failure_output(&zero_output)
    );

    let default_stderr = text(&default_output.stderr);
    let zero_stderr = text(&zero_output.stderr);
    assert!(
        default_stderr.contains("= while expanding: \\outer -> \\inner"),
        "{default_stderr}"
    );
    assert!(!zero_stderr.contains("while expanding:"), "{zero_stderr}");
    assert!(default_stderr.contains("main.tex:6:1"), "{default_stderr}");
    assert!(zero_stderr.contains("main.tex:6:1"), "{zero_stderr}");
    let default_caret =
        line_after(&default_stderr, "6 | \\outer").expect("default caret after source line");
    let zero_caret =
        line_after(&zero_stderr, "6 | \\outer").expect("zero-context caret after source line");
    assert_eq!(default_caret, zero_caret);
    assert!(zero_caret.ends_with("| ^^^^^^"), "{zero_stderr}");
}

#[test]
fn raw_text_followed_by_end_is_a_valid_job() {
    let job = Job::new("raw-legal-end");
    job.write("main.tex", "hello\\end\n");

    let output = job.compile(&["-plain"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    let combined = format!(
        "{}{}{}",
        text(&output.stdout),
        text(&output.stderr),
        job.log()
    );
    assert!(!combined.contains("Emergency stop"), "{combined}");
    assert!(!combined.contains("no legal \\end"), "{combined}");
}

#[test]
fn raw_eof_without_end_is_a_fatal_error() {
    let job = Job::new("raw-missing-end");
    job.write("main.tex", "hello\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    let marker = "Emergency stop: no legal \\end found";
    assert_eq!(occurrences(&stderr, marker), 1, "{stderr}");
    assert!(stderr.contains("main.tex:1:"), "{stderr}");
    assert_eq!(occurrences(&job.log(), marker), 1);
    assert!(!job.dir.join("main.pdf").exists());
}

#[test]
fn invalid_character_is_located_without_a_stale_macro_trace() {
    let job = Job::new("invalid-character");
    job.write_bytes("main.tex", b"\\def\\prior{\\relax}\n\\prior\n\x7f\n\\end\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    let marker = "Text line contains an invalid character";
    assert_eq!(occurrences(&stderr, marker), 1, "{stderr}");
    assert!(stderr.contains("byte 0x7F"), "{stderr}");
    assert!(stderr.contains("main.tex:3:1"), "{stderr}");
    assert!(!stderr.contains("while expanding: \\prior"), "{stderr}");
}

#[test]
fn translated_escape_uses_its_complete_physical_spelling() {
    let job = Job::new("translated-escape-location");
    job.write_bytes("main.tex", b"^^5cundefined\n\\end\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "-halt-on-error"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Undefined control sequence \\undefined"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:1:1"), "{stderr}");
    assert!(stderr.contains("1 | ^^5cundefined"), "{stderr}");
    let caret = line_after(&stderr, "1 | ^^5cundefined").expect("caret after translated command");
    assert!(
        caret.ends_with("| ^^^^^^^^^^^^^"),
        "caret={caret:?}\n{stderr}"
    );
}

#[test]
fn invalid_utf8_before_a_command_keeps_its_raw_byte_column() {
    let job = Job::new("invalid-utf8-prefix-location");
    job.write_bytes("main.tex", b"\xff \\undefined\n\\end\n");

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Undefined control sequence \\undefined"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:1:3"), "{stderr}");
    let source = "1 | � \\undefined";
    assert!(stderr.contains(source), "{stderr}");
    let caret = line_after(&stderr, source).expect("caret after lossy source line");
    let after_gutter = caret
        .split_once("| ")
        .map(|(_, suffix)| suffix)
        .expect("caret line has a gutter");
    assert_eq!(
        after_gutter.chars().take_while(|ch| *ch == ' ').count(),
        2,
        "caret={caret:?}\n{stderr}"
    );
    assert!(after_gutter.trim_start().starts_with('^'), "{stderr}");
}

#[test]
fn max_errors_stops_once_at_the_requested_count() {
    let job = Job::new("max-errors");
    job.write(
        "main.tex",
        "\\undefinedErrorOne\n\\undefinedErrorTwo\n\\undefinedErrorThree\n\\end\n",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode", "--max-errors=2"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    let marker = "Too many errors; stopping after 2 errors.";
    assert_eq!(occurrences(&stderr, marker), 1, "{stderr}");
    assert!(stderr.contains("Undefined control sequence \\undefinedErrorOne"));
    assert!(stderr.contains("Undefined control sequence \\undefinedErrorTwo"));
    assert!(!stderr.contains("undefinedErrorThree"), "{stderr}");
    assert_eq!(occurrences(&job.log(), marker), 1);
}

#[test]
fn max_errors_also_bounds_ini_mode_recovery() {
    let job = Job::new("ini-max-errors");
    job.write(
        "main.tex",
        "\\catcode123=1\n\\catcode125=2\n\\undefinedFirst\n\\undefinedMustNotRun\n\\dump\n",
    );
    let output = job.compile(&[
        "-ini",
        "-plain",
        "-interaction=nonstopmode",
        "--max-errors=1",
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert_eq!(
        occurrences(&stderr, "Undefined control sequence"),
        1,
        "{stderr}"
    );
    assert!(
        stderr.contains("Too many errors; stopping after 1 error"),
        "{stderr}"
    );
    assert!(!stderr.contains("undefinedMustNotRun"), "{stderr}");
    assert!(!job.dir.join("pdflatex.fmt").exists());
}

#[test]
fn nonstop_mode_writes_a_pdf_after_a_recoverable_error_but_exits_one() {
    let job = Job::new("nonstop-pdf");
    job.write(
        "main.tex",
        r"\documentclass{article}
\begin{document}
Text before \undefinedRecoverableCommand text after.
\end{document}
",
    );

    let output = job.compile(&["-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert!(stderr.contains("Undefined control sequence \\undefinedRecoverableCommand"));
    assert!(
        stdout.contains("Output written on"),
        "{}",
        failure_output(&output)
    );
    let pdf = std::fs::read(job.dir.join("main.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(!job.dir.join("main.depcache").exists());
}

#[test]
fn late_pdf_write_failure_is_structured_logged_and_batch_aware() {
    for (label, mode, silent) in [
        ("late-pdf-write", "nonstopmode", false),
        ("late-pdf-write-batch", "batchmode", true),
    ] {
        let job = Job::new(label);
        job.write("main.tex", "\\font\\ten=cmr10 \\ten A page\\end\n");
        std::fs::create_dir(job.dir.join("main.pdf")).unwrap();

        let output = job.compile(&["-plain", &format!("-interaction={mode}")]);
        assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        if silent {
            assert!(output.stdout.is_empty(), "{}", failure_output(&output));
            assert!(stderr.is_empty(), "{}", failure_output(&output));
        } else {
            assert!(stderr.contains("Cannot write PDF `main.pdf`:"), "{stderr}");
            assert!(stderr.contains("output directory"), "{stderr}");
            assert!(!stderr.contains("  --> "), "{stderr}");
        }
        let log = job.log();
        assert!(log.contains("! Cannot write PDF `main.pdf`:"), "{log}");
        assert!(log.contains("output directory"), "{log}");
    }
}

#[test]
fn pdf_resource_errors_name_the_file_format_and_source_command() {
    let missing_image = Job::new("missing-pdf-image");
    missing_image.write("main.tex", "\\pdfximage{missing.png}\n\\end\n");
    let output = missing_image.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Image file `missing.png` was not found"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:1:1"), "{stderr}");
    assert!(stderr.contains("check the file name and path"), "{stderr}");

    let invalid_image = Job::new("invalid-pdf-image");
    invalid_image.write("broken.img", "this is not an image");
    invalid_image.write("main.tex", "\\pdfximage{broken.img}\n\\end\n");
    let output = invalid_image.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Unsupported or invalid image `broken.img` (expected PDF, JPEG, or PNG)"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:1:1"), "{stderr}");
    assert!(
        stderr.contains("file is not truncated or corrupt"),
        "{stderr}"
    );

    let missing_object = Job::new("missing-pdf-object-file");
    missing_object.write("main.tex", "\\pdfobj file{missing.dat}\n\\end\n");
    let output = missing_object.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("PDF object file `missing.dat` was not found"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:1:1"), "{stderr}");
}

#[test]
fn corrupt_passthrough_pngs_fail_without_publishing_a_pdf() {
    for (label, indexed) in [("corrupt-rgb-png", false), ("corrupt-indexed-png", true)] {
        let job = Job::new(label);
        job.write_bytes("broken.png", &corrupt_passthrough_png(indexed));
        job.write(
            "main.tex",
            "\\pdfximage{broken.png}\n\\shipout\\hbox{\\pdfrefximage\\pdflastximage}\n\\end\n",
        );

        let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
        assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert!(stderr.contains("Unsupported or invalid image:"), "{stderr}");
        assert!(stderr.contains("broken.png"), "{stderr}");
        assert!(stderr.contains("supported, valid JPEG or PNG"), "{stderr}");
        assert!(
            !job.dir.join("main.pdf").exists(),
            "a corrupt image must not publish a PDF"
        );
    }
}

#[test]
fn deferred_pdf_state_errors_keep_their_command_source_and_actionable_help() {
    let malformed_matrix = Job::new("invalid-pdf-matrix");
    malformed_matrix.write(
        "main.tex",
        "\\setbox0=\\hbox{\n\\pdfsetmatrix{1 0 invalid 1}\n}\n\\shipout\\box0\n\\end\n",
    );
    let output = malformed_matrix.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Invalid \\pdfsetmatrix value; expected exactly four finite numbers"),
        "{stderr}"
    );
    assert!(stderr.contains("got `1 0 invalid 1`"), "{stderr}");
    assert!(stderr.contains("main.tex:2:1"), "{stderr}");
    assert!(stderr.contains("\\pdfsetmatrix{1 0 invalid 1}"), "{stderr}");
    assert!(
        stderr.contains("use `\\pdfsetmatrix{a b c d}` with four finite decimal numbers"),
        "{stderr}"
    );

    let unmatched_save = Job::new("unmatched-pdf-save");
    unmatched_save.write(
        "main.tex",
        "\\setbox0=\\hbox{\n\\pdfsave\n}\n\\shipout\\box0\n\\end\n",
    );
    let output = unmatched_save.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr
            .contains("Unmatched \\pdfsave: the shipped page ended before a matching \\pdfrestore"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:2:1"), "{stderr}");
    assert!(stderr.contains("add a matching `\\pdfrestore`"), "{stderr}");

    let unmatched_restore = Job::new("unmatched-pdf-restore");
    unmatched_restore.write(
        "main.tex",
        "\\setbox0=\\hbox{\n\\pdfrestore\n}\n\\shipout\\box0\n\\end\n",
    );
    let output = unmatched_restore.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(
            "warning: Unmatched \\pdfrestore: no preceding \\pdfsave exists in this shipped box"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:2:1"), "{stderr}");
    assert!(
        stderr.contains("add `\\pdfsave` before this command"),
        "{stderr}"
    );
    assert!(!stderr.contains("pdfTeX warning:"), "{stderr}");

    let misplaced_restore = Job::new("misplaced-pdf-restore");
    misplaced_restore.write(
        "main.tex",
        "\\setbox0=\\hbox{\n\\pdfsave\\kern1pt\n\\pdfrestore\n}\n\\shipout\\box0\n\\end\n",
    );
    let output = misplaced_restore.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("warning: Misplaced \\pdfrestore: position changed by"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:3:1"), "{stderr}");
    assert!(
        stderr.contains("at the same typesetting position"),
        "{stderr}"
    );
    assert!(!stderr.contains("pdfTeX warning:"), "{stderr}");
}

#[test]
fn pdfxform_uses_the_standard_located_register_range_error() {
    let job = Job::new("pdfxform-register-range");
    job.write("main.tex", "\\pdfxform-1\n\\end\n");
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert_eq!(output.status.code(), Some(1), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Register number -1 is out of range; expected a number from 0 through"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:1:10"), "{stderr}");
    assert!(!stderr.contains("panicked at"), "{stderr}");
}

#[test]
fn invalid_pdfmatch_pattern_is_a_located_warning_with_parser_detail() {
    let job = Job::new("invalid-pdfmatch-pattern");
    job.write(
        "main.tex",
        "\\count0=\\pdfmatch{[}{a}\n\\message{RESULT=\\the\\count0}\n\\end\n",
    );
    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("warning: Invalid regular expression in \\pdfmatch:"),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:1:9"), "{stderr}");
    assert!(
        stderr.contains("POSIX extended regular-expression syntax"),
        "{stderr}"
    );
    assert!(text(&output.stdout).contains("RESULT=-1"));
    assert!(job
        .log()
        .contains("Invalid regular expression in \\pdfmatch"));
}

#[test]
fn default_and_explicit_errorstop_mode_halt_before_writing_a_pdf() {
    let source = r"\documentclass{article}
\begin{document}
A complete page before the error.
\newpage
\undefinedFirstError
\undefinedMustNotRun
\end{document}
";
    for (label, args) in [
        ("default-errorstop", &[][..]),
        ("explicit-errorstop", &["-interaction=errorstopmode"][..]),
    ] {
        let job = Job::new(label);
        job.write("main.tex", source);
        let output = job.compile(args);
        assert!(!output.status.success(), "{}", failure_output(&output));
        let stderr = text(&output.stderr);
        assert!(stderr.contains("Undefined control sequence \\undefinedFirstError"));
        assert!(!stderr.contains("undefinedMustNotRun"), "{stderr}");
        assert!(!job.dir.join("main.pdf").exists());
    }
}

#[test]
fn errhelp_applies_to_errmessage_without_masking_later_specific_help() {
    let job = Job::new("errhelp-scope");
    job.write(
        "main.tex",
        "\\errhelp{CUSTOM ERRMESSAGE HELP}\n\\errmessage{deliberate error}\n\\undefinedAfterErrmessage\n\\end\n",
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    assert_eq!(
        occurrences(&stderr, "CUSTOM ERRMESSAGE HELP"),
        1,
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:2:1"), "{stderr}");
    assert!(
        stderr.contains("2 | \\errmessage{deliberate error}"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Undefined control sequence \\undefinedAfterErrmessage"),
        "{stderr}"
    );
    assert!(
        stderr.contains("check the command spelling; if a package defines it"),
        "{stderr}"
    );
}

#[test]
fn huge_message_help_and_source_are_bounded() {
    let job = Job::new("bounded-diagnostics");
    let help = "H".repeat(16 * 1024);
    let message = "M".repeat(32 * 1024);
    let source_prefix = "S".repeat(100_000);
    job.write(
        "main.tex",
        &format!(
            "\\errhelp{{{help}}}\n\\errmessage{{{message}}}\n{source_prefix}\\undefinedHugeSource\n\\end\n"
        ),
    );

    let output = job.compile(&["-plain", "-interaction=nonstopmode"]);
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    let log = job.log();
    assert!(
        stderr.contains("bytes omitted"),
        "diagnostic was not truncated"
    );
    assert!(stderr.contains("Undefined control sequence \\undefinedHugeSource"));
    assert!(stderr.contains("main.tex:3:100001"), "{stderr}");
    assert!(
        !stderr.contains(&"S".repeat(500)),
        "source excerpt was not bounded"
    );
    assert!(
        stderr.len() < 64 * 1024,
        "stderr grew to {} bytes",
        stderr.len()
    );
    assert!(log.len() < 64 * 1024, "log grew to {} bytes", log.len());
}

#[cfg(unix)]
#[test]
fn texmk_preserves_child_streams_when_a_build_fails() {
    let job = Job::new("texmk-streams");
    job.write("main.tex", "test\n");
    job.tool(
        "pdflatex",
        "printf 'OUT-MARKER\\n'\nprintf 'ERR-MARKER\\n' >&2\nexit 1",
    );

    let output = job.texmk();
    assert!(!output.status.success(), "{}", failure_output(&output));
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert_eq!(occurrences(&stdout, "OUT-MARKER"), 1, "{stdout}");
    assert!(!stdout.contains("ERR-MARKER"), "{stdout}");
    assert_eq!(occurrences(&stderr, "ERR-MARKER"), 1, "{stderr}");
    assert!(!stderr.contains("OUT-MARKER"), "{stderr}");
    assert_eq!(
        occurrences(&stderr, "texmk: pdflatex failed on pass 1"),
        1,
        "{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn texmk_reports_one_summary_for_stable_unresolved_references_and_citations() {
    let job = Job::new("texmk-unresolved");
    job.write("main.tex", "test\n");
    job.tool(
        "pdflatex",
        r#"out=.
aux=.
while [ "$#" -gt 0 ]; do
  case "$1" in
    -output-directory) out=$2; shift 2 ;;
    -aux-directory|-auxdir) aux=$2; shift 2 ;;
    *) shift ;;
  esac
done
mkdir -p "$out" "$aux"
printf '%s\n' 'There were undefined references.' 'There were undefined citations.' > "$aux/main.log"
printf '%%PDF-1.4 /Type /Pages /Count 1 /Type /Page ' > "$out/main.pdf""#,
    );

    let output = job.texmk();
    assert!(output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    let warning = "texmk: warning: unresolved references and citations remain";
    assert_eq!(occurrences(&stderr, warning), 1, "{stderr}");
    assert!(stderr.contains("main.log"), "{stderr}");
}
