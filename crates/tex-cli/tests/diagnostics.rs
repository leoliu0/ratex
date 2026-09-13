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
    assert!(
        stderr.contains("pdflatex: cannot create output directory blocked-output/"),
        "{stderr}"
    );
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
        stderr.contains("LaTeX Error: \\begin{center} on input line 3 ended by \\end{flushleft}."),
        "{stderr}"
    );
    assert!(stderr.contains("main.tex:5:1"), "{stderr}");
    assert!(stderr.contains("5 | \\end{flushleft}"), "{stderr}");
    assert!(
        stderr.contains("change `\\end{flushleft}` to `\\end{center}`"),
        "{stderr}"
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
    for (label, operation, expected, value) in [
        (
            "divide-by-zero",
            "\\divide\\count0 by 0",
            "Cannot divide by zero in \\divide; value left unchanged",
            "VALUE=42",
        ),
        (
            "multiply-overflow",
            "\\multiply\\count0 by 2",
            "Arithmetic overflow in \\multiply; value left unchanged",
            "VALUE=2147483647",
        ),
    ] {
        let job = Job::new(label);
        let initial = if label == "divide-by-zero" {
            "42"
        } else {
            "2147483647"
        };
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
}

#[test]
fn eof_syntax_and_terminal_read_errors_state_the_required_fix() {
    for (label, source, expected, help) in [
        (
            "braced-file-name-eof",
            "\\input{unfinished\n",
            "File ended while scanning a braced file name; add the missing }",
            "unmatched `{`",
        ),
        (
            "quoted-file-name-eof",
            "\\input \"unfinished\n",
            "File ended while scanning a quoted file name; add the closing quote",
            "unmatched `{`",
        ),
        (
            "terminal-read-unavailable",
            "\\read-1 to \\answer\n\\end\n",
            "Terminal input is unavailable for \\read-1",
            "file-backed stream",
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
    assert!(stderr.contains("! first\n  | main.tex:999:999"), "{stderr}");
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
        "printf '%s\\n' 'There were undefined references.' 'There were undefined citations.' > main.log\nprintf '%%PDF-1.4 /Type /Pages /Count 1 /Type /Page ' > main.pdf",
    );

    let output = job.texmk();
    assert!(output.status.success(), "{}", failure_output(&output));
    let stderr = text(&output.stderr);
    let warning = "texmk: warning: unresolved references and citations remain";
    assert_eq!(occurrences(&stderr, warning), 1, "{stderr}");
    assert!(stderr.contains("main.log"), "{stderr}");
}
