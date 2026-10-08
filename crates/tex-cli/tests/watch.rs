//! `texres -pvc`: the watch mode of the build driver, run as a real process.
//!
//! Every wait is bounded and ends as soon as the awaited line of the driver's
//! status output appears; nothing synchronizes by sleeping.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

mod support;

/// Upper bound for any single wait; a debug build on a loaded machine needs
/// seconds, a hung driver must still fail the test.
const WAIT: Duration = Duration::from_secs(120);

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("texres-watch-{}-{serial}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.path(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    /// An editor-style atomic save: write a temporary file, rename over the target.
    fn save(&self, name: &str, text: &str) {
        let temporary = self.path(&format!(".{name}.swp"));
        std::fs::write(&temporary, text).unwrap();
        std::fs::rename(&temporary, self.path(name)).unwrap();
    }

    fn pdf_text(&self, name: &str) -> String {
        let pdf = lopdf::Document::load(self.path(name)).unwrap();
        let pages: Vec<u32> = pdf.get_pages().keys().copied().collect();
        let text = pdf.extract_text(&pages).unwrap();
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn article(body: &str) -> String {
    format!("\\documentclass{{article}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n")
}

/// One finished build as the driver reports it.
struct Build {
    ok: bool,
    /// Everything the driver printed from the previous status line up to
    /// this build's, including the engine's error report.
    output: String,
}

/// What one detected change cost: its report, the rebuild, the new file count.
struct Cycle {
    changed: String,
    build: Build,
    watching: usize,
}

struct Session {
    child: Child,
    lines: Receiver<String>,
    /// Lines read since the last status line.
    pending: Vec<String>,
    /// Every line, for failure messages.
    log: Vec<String>,
    builds: usize,
}

impl Session {
    fn start(project: &Project, args: &[&str]) -> Self {
        Self::start_with(project, args, &[])
    }

    fn start_with(project: &Project, args: &[&str], environment: &[(&str, &Path)]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_texres"))
            .args(args)
            .current_dir(&project.0)
            .env("HOME", project.path("home"))
            .env("TEX_RS_CACHE_DIR", project.path("cache"))
            .envs(environment.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            pending: Vec::new(),
            log: Vec::new(),
            builds: 0,
        }
    }

    fn fail(&self, what: &str) -> ! {
        panic!("{what}\n--- driver output ---\n{}", self.log.join("\n"));
    }

    fn next_line(&mut self, what: &str, deadline: Instant) -> String {
        match self
            .lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(line) => {
                self.log.push(line.clone());
                line
            }
            Err(RecvTimeoutError::Timeout) => self.fail(&format!("timed out waiting for {what}")),
            Err(RecvTimeoutError::Disconnected) => {
                self.fail(&format!("the driver exited while waiting for {what}"))
            }
        }
    }

    /// The next status line starting with `prefix` (after `texmk: `, skipping
    /// its timestamp), with the lines before it kept in `pending`.
    fn status(&mut self, prefix: &str) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            let line = self.next_line(prefix, deadline);
            if let Some(rest) = line.strip_prefix("texmk: ") {
                let rest = match rest.strip_prefix('[') {
                    Some(stamped) => stamped.split_once("] ").map_or(rest, |(_, text)| text),
                    None => rest,
                };
                if rest.starts_with(prefix) {
                    return rest.to_string();
                }
            }
            self.pending.push(line);
        }
    }

    fn build(&mut self) -> Build {
        let line = self.status("build ");
        self.builds += 1;
        let ok = line.starts_with("build OK");
        assert!(ok || line.starts_with("build FAILED"), "{line}");
        Build {
            ok,
            output: std::mem::take(&mut self.pending).join("\n"),
        }
    }

    fn watching(&mut self) -> usize {
        let line = self.status("watching ");
        assert!(line.ends_with("files (Ctrl-C to stop)"), "{line}");
        line["watching ".len()..]
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }

    /// The first build, up to the point where the driver waits for changes.
    fn idle(&mut self) -> (Build, usize) {
        let build = self.build();
        (build, self.watching())
    }

    /// The next detected change through its rebuild.
    fn cycle(&mut self) -> Cycle {
        let changed = self.status("changed: ")["changed: ".len()..].to_string();
        let build = self.build();
        Cycle {
            changed,
            build,
            watching: self.watching(),
        }
    }

    fn interrupt(&mut self) {
        let status = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn exit_status(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= deadline {
                self.fail("timed out waiting for the driver to exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_running(&mut self) {
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "the watching driver exited\n{}",
            self.log.join("\n")
        );
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn leftovers(root: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".lock") || name.contains(".tmp-") {
            found.push(path.clone());
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            leftovers(&path, found);
        }
    }
}

#[test]
fn editing_the_main_file_rebuilds_the_pdf() {
    let project = Project::new();
    project.write("main.tex", &article("Alpha text"));
    let mut session = Session::start(&project, &["-pvc", "main.tex"]);
    let (first, watching) = session.idle();
    assert!(first.ok, "{}", first.output);
    assert_eq!(watching, 1);
    assert!(project.pdf_text("main.pdf").contains("Alpha text"));

    project.save("main.tex", &article("Bravo text"));
    let cycle = session.cycle();
    assert!(cycle.build.ok, "{}", cycle.build.output);
    assert_eq!(cycle.changed, "main.tex");
    assert!(project.pdf_text("main.pdf").contains("Bravo text"));
    assert_eq!(session.builds, 2);
}

#[test]
fn editing_an_input_file_rebuilds_the_pdf() {
    let project = Project::new();
    project.write("main.tex", &article("Main text \\input{chapter}"));
    project.write("chapter.tex", "Chapter one");
    let mut session = Session::start(&project, &["--watch", "main.tex"]);
    let (first, watching) = session.idle();
    assert!(first.ok, "{}", first.output);
    assert_eq!(watching, 2, "main.tex and chapter.tex");
    assert!(project.pdf_text("main.pdf").contains("Chapter one"));

    project.save("chapter.tex", "Chapter two");
    let cycle = session.cycle();
    assert!(cycle.build.ok, "{}", cycle.build.output);
    assert_eq!(cycle.changed, "chapter.tex");
    assert_eq!(cycle.watching, 2);
    assert!(project.pdf_text("main.pdf").contains("Chapter two"));
}

#[test]
fn a_build_error_is_reported_and_watching_continues() {
    let project = Project::new();
    project.write("main.tex", &article("Fine text"));
    let mut session = Session::start(&project, &["-w", "main.tex"]);
    assert!(session.idle().0.ok);

    project.save("main.tex", &article("Broken \\undefinedcommandxyz text"));
    let broken = session.cycle();
    assert!(!broken.build.ok);
    assert!(
        broken.build.output.contains("undefinedcommandxyz"),
        "the error report is missing:\n{}",
        broken.build.output
    );
    session.assert_running();

    project.save("main.tex", &article("Repaired text"));
    let fixed = session.cycle();
    assert!(fixed.build.ok, "{}", fixed.build.output);
    assert!(project.pdf_text("main.pdf").contains("Repaired text"));
    session.assert_running();
}

/// vimtex's continuous mode passes these latexmk options and learns from
/// its `-e` callbacks when a build starts and how it ended; it then reads
/// the errors from `main.log` beside the PDF.
#[test]
fn vimtex_continuous_mode_runs_its_callbacks() {
    let project = Project::new();
    project.write("main.tex", &article("Fine text"));
    // s:wrap_option_appendcmd in vimtex's autoload/vimtex/compiler/latexmk.vim
    let callback = |name: &str, value: &str| {
        format!("${name} = (${name} ? ${name} . \" ; \" : \"\") . \"echo {value} >&2\"")
    };
    let compiling = callback("compiling_cmd", "vimtex_compiler_callback_compiling");
    let success = callback("success_cmd", "vimtex_compiler_callback_success");
    let failure = callback("failure_cmd", "vimtex_compiler_callback_failure");
    let mut session = Session::start(
        &project,
        &[
            // vimtex also passes -interaction=nonstopmode; without it a failed
            // build is reported as `build FAILED`, which `Session` expects.
            "-verbose", "-file-line-error", "-synctex=1", "-pvc",
            "-pvctimeout-", "-view=none", "-e", &compiling, "-e", &success, "-e", &failure,
            "main.tex",
        ],
    );
    let (first, _) = session.idle();
    assert!(first.ok, "{}", first.output);
    let started = first.output.find("vimtex_compiler_callback_compiling");
    let passed = first.output.find("vimtex_compiler_callback_success");
    assert!(started.is_some() && started < passed, "{}", first.output);
    assert!(project.path("main.log").is_file());

    project.save("main.tex", &article("Broken \\undefinedcommandxyz text"));
    let broken = session.cycle();
    assert!(!broken.build.ok);
    assert!(broken.build.output.contains("vimtex_compiler_callback_failure"), "{}", broken.build.output);
    let log = std::fs::read_to_string(project.path("main.log")).unwrap();
    assert!(
        log.contains("main.tex:3: Undefined control sequence.\nl.3 Broken \\undefinedcommandxyz\n"),
        "{log}"
    );
    session.assert_running();
}

/// latexmk fills in its placeholders in the hooks (`Run_subst`): with
/// `-outdir=build`, this hook prints `hook D=build/main.pdf S=main.tex
/// R=main pct=%` under latexmk 4.87.
#[test]
fn hooks_get_latexmks_placeholders() {
    let project = Project::new();
    project.write("main.tex", &article("Fine text"));
    let mut session = Session::start(
        &project,
        &["-pvc", "-outdir=build", "-e", "$success_cmd = 'echo hook D=%D S=%S R=%R pct=%% >&2'", "main.tex"],
    );
    let (first, _) = session.idle();
    assert!(first.ok, "{}", first.output);
    assert!(first.output.contains("hook D=build/main.pdf S=main.tex R=main pct=%"), "{}", first.output);
    session.assert_running();
}

#[test]
fn a_save_without_content_change_does_not_rebuild() {
    let project = Project::new();
    project.write("main.tex", &article("Text \\input{chapter}"));
    project.write("chapter.tex", "Chapter");
    let mut session = Session::start(&project, &["-pvc", "main.tex"]);
    assert!(session.idle().0.ok);

    // A bare touch, an identical in-place rewrite and an identical atomic save.
    let chapter = project.path("chapter.tex");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&chapter)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(10))
        .unwrap();
    project.write("chapter.tex", "Chapter");
    project.save("chapter.tex", "Chapter");
    // A real change afterwards: the first change the driver reports must be
    // this one alone, and the only rebuild so far.
    project.save("main.tex", &article("Changed \\input{chapter}"));
    let cycle = session.cycle();
    assert_eq!(cycle.changed, "main.tex");
    assert!(cycle.build.ok, "{}", cycle.build.output);
    assert_eq!(session.builds, 2);
}

#[test]
fn a_newly_input_file_becomes_watched() {
    let project = Project::new();
    project.write("main.tex", &article("Main text"));
    project.write("extra.tex", "Extra one");
    let mut session = Session::start(&project, &["-pvc", "main.tex"]);
    let (first, watching) = session.idle();
    assert!(first.ok, "{}", first.output);
    assert_eq!(watching, 1);

    project.save("main.tex", &article("Main text \\input{extra}"));
    let added = session.cycle();
    assert!(added.build.ok, "{}", added.build.output);
    assert_eq!(added.watching, 2);
    assert!(project.pdf_text("main.pdf").contains("Extra one"));

    project.save("extra.tex", "Extra two");
    let edited = session.cycle();
    assert_eq!(edited.changed, "extra.tex");
    assert!(edited.build.ok, "{}", edited.build.output);
    assert!(project.pdf_text("main.pdf").contains("Extra two"));

    project.save("main.tex", &article("Main text only"));
    let dropped = session.cycle();
    assert!(dropped.build.ok, "{}", dropped.build.output);
    assert_eq!(dropped.watching, 1, "an input that was removed is no longer watched");
}

#[test]
fn creating_a_missing_input_triggers_the_build_that_needed_it() {
    let project = Project::new();
    project.write("main.tex", &article("Main text \\input{later}"));
    let mut session = Session::start(&project, &["-pvc", "main.tex"]);
    let (first, watching) = session.idle();
    assert!(!first.ok);
    assert!(first.output.contains("later"), "{}", first.output);
    assert_eq!(watching, 1);

    project.write("later.tex", "Arrived");
    let cycle = session.cycle();
    assert_eq!(cycle.changed, "later.tex");
    assert!(cycle.build.ok, "{}", cycle.build.output);
    assert_eq!(cycle.watching, 2);
    assert!(project.pdf_text("main.pdf").contains("Arrived"));

    // Deleting the file breaks the build; restoring it repairs the build.
    std::fs::remove_file(project.path("later.tex")).unwrap();
    let broken = session.cycle();
    assert_eq!(broken.changed, "later.tex");
    assert!(!broken.build.ok);
    project.write("later.tex", "Restored");
    let restored = session.cycle();
    assert!(restored.build.ok, "{}", restored.build.output);
    assert!(project.pdf_text("main.pdf").contains("Restored"));
}

#[test]
fn options_combine_and_the_builds_own_outputs_do_not_retrigger() {
    let project = Project::new();
    project.write("main.tex", &article("Original \\input{part}"));
    project.write("part.tex", "Part");
    let mut session = Session::start(
        &project,
        &[
            "-w",
            "-pdf",
            "-k",
            "-jobname",
            "renamed",
            "-output-directory",
            "build",
            "main.tex",
        ],
    );
    let (first, watching) = session.idle();
    assert!(first.ok, "{}", first.output);
    assert_eq!(watching, 2);
    assert!(project.path("build/renamed.pdf").is_file());
    assert!(project.path("build/renamed.aux").is_file(), "-k exports the aux file");
    assert_web2c_recorder(&project.path("build/renamed.fls"));
    assert!(
        !project.path("build/.texmk-watch-dependencies").exists(),
        "the watch list is private"
    );

    project.save("main.tex", &article("Rewritten \\input{part}"));
    let cycle = session.cycle();
    assert_eq!(
        cycle.changed, "main.tex",
        "an output of the previous build was reported as a change"
    );
    assert!(cycle.build.ok, "{}", cycle.build.output);
    assert_eq!(cycle.watching, 2);
    assert!(project.pdf_text("build/renamed.pdf").contains("Rewritten"));
    assert_eq!(session.builds, 2);
}

#[test]
fn ctrl_c_exits_cleanly_without_leftovers() {
    let project = Project::new();
    project.write("main.tex", &article("Text"));
    let mut session = Session::start(&project, &["-pvc", "main.tex"]);
    assert!(session.idle().0.ok);
    session.interrupt();
    let status = session.exit_status();
    assert_eq!(status.code(), Some(0), "{status:?}\n{}", session.log.join("\n"));
    let mut found = Vec::new();
    leftovers(&project.0, &mut found);
    assert!(found.is_empty(), "leftover lock or temporary files: {found:?}");
}

#[test]
fn watch_mode_rejects_the_clean_options() {
    let project = Project::new();
    project.write("main.tex", &article("Text"));
    for (watch, clean) in [("-pvc", "-c"), ("--watch", "-C"), ("-w", "-c")] {
        let output = Command::new(env!("CARGO_BIN_EXE_texres"))
            .args([watch, clean, "main.tex"])
            .current_dir(&project.0)
            .env("HOME", project.path("home"))
            .env("TEX_RS_CACHE_DIR", project.path("cache"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{watch} {clean}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("cannot be combined"), "{stderr}");
    }
}

#[test]
fn a_missing_main_file_ends_the_first_build_with_an_error() {
    let project = Project::new();
    let output = Command::new(env!("CARGO_BIN_EXE_texres"))
        .args(["-pvc", "absent.tex"])
        .current_dir(&project.0)
        .env("HOME", project.path("home"))
        .env("TEX_RS_CACHE_DIR", project.path("cache"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}

/// A stand-in engine that, while `hold` exists, announces `started` and
/// blocks until `release` exists: a build the test can edit files during.
fn install_blocking_engine(project: &Project) {
    let root = project.0.display();
    support::install_executable(
        &project.path("pdflatex"),
        format!(
            r#"#!/bin/sh
set -eu
out=. aux=. job=main
while [ "$#" -gt 0 ]; do
  case "$1" in
    -output-directory) out=$2; shift 2 ;;
    -aux-directory|-auxdir) aux=$2; shift 2 ;;
    -jobname) job=$2; shift 2 ;;
    *) shift ;;
  esac
done
mkdir -p "$out" "$aux"
if [ -f "{root}/hold" ]; then
  : > "{root}/started"
  while [ ! -f "{root}/release" ]; do sleep 0.02; done
fi
echo '\relax' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Pages /Count 1 /Type /Page ' > "$out/$job.pdf"
"#
        )
        .as_bytes(),
    );
}

#[test]
fn changes_during_a_build_schedule_exactly_one_more_build() {
    let project = Project::new();
    project.write("main.tex", "version 1\n");
    install_blocking_engine(&project);
    // TEXMK_LIB makes the driver run the stand-in engine from the project.
    let mut session = Session::start_with(
        &project,
        &["-pvc", "main.tex"],
        &[("TEXMK_LIB", project.0.as_path())],
    );
    assert!(session.idle().0.ok);

    project.write("hold", "");
    project.save("main.tex", "version 2\n");
    let deadline = Instant::now() + WAIT;
    while !project.path("started").exists() {
        assert!(Instant::now() < deadline, "the rebuild never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The rebuild is running: edit the file twice, then let it finish.
    project.save("main.tex", "version 3\n");
    project.save("main.tex", "version 4\n");
    std::fs::remove_file(project.path("hold")).unwrap();
    project.write("release", "");

    let during = session.cycle();
    assert_eq!(during.changed, "main.tex");
    assert!(during.build.ok, "{}", during.build.output);
    let after = session.cycle();
    assert_eq!(after.changed, "main.tex");
    assert!(after.build.ok, "{}", after.build.output);
    assert_eq!(session.builds, 3);

    // The edits during the build produced one build; a further edit one more.
    project.save("main.tex", "version 5\n");
    let last = session.cycle();
    assert_eq!(last.changed, "main.tex");
    assert_eq!(session.builds, 4);
}

#[test]
fn an_eps_figure_is_a_dependency_and_its_conversion_does_not_retrigger() {
    let eps = |width: u32| {
        format!(
            "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 {width} 72\n%%EndComments\n\
             0 0 0 setrgbcolor newpath 0 0 moveto {width} 0 lineto {width} 72 lineto \
             0 72 lineto closepath fill\nshowpage\n%%EOF\n"
        )
    };
    let project = Project::new();
    project.write(
        "main.tex",
        "\\documentclass{article}\n\\usepackage{graphicx}\n\\begin{document}\n\\includegraphics{figure}\n\\end{document}\n",
    );
    project.write("figure.eps", &eps(144));
    let mut session = Session::start(&project, &["-pvc", "main.tex"]);
    let (first, watching) = session.idle();
    assert!(first.ok, "{}", first.output);
    assert!(watching >= 2, "main.tex and figure.eps, got {watching}");

    project.save("figure.eps", &eps(96));
    let cycle = session.cycle();
    assert_eq!(cycle.changed, "figure.eps");
    assert!(cycle.build.ok, "{}", cycle.build.output);

    // The conversion the rebuild rewrote is no change of its own.
    project.save("main.tex", &article("Text only"));
    let next = session.cycle();
    assert_eq!(next.changed, "main.tex");
    assert_eq!(session.builds, 3);
}

/// A `-recorder` file as TeX Live writes it: `PWD` first, then only `INPUT`
/// and `OUTPUT` lines, which latexmk and editors parse.
fn assert_web2c_recorder(path: &Path) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut lines = text.lines();
    assert!(lines.next().is_some_and(|line| line.starts_with("PWD ")), "{text}");
    for line in lines {
        assert!(
            line.starts_with("INPUT ") || line.starts_with("OUTPUT "),
            "non-standard recorder line {line:?} in {}",
            path.display()
        );
    }
    assert!(text.contains("INPUT "), "{text}");
}

#[test]
fn exported_recorder_files_stay_web2c_compatible() {
    let project = Project::new();
    project.write("main.tex", &article("Text"));
    let environment = |mut command: Command| {
        command
            .current_dir(&project.0)
            .env("HOME", project.path("home"))
            .env("TEX_RS_CACHE_DIR", project.path("cache"))
            .output()
            .unwrap()
    };
    let mut driver = Command::new(env!("CARGO_BIN_EXE_texres"));
    driver.args(["-k", "main.tex"]);
    let output = environment(driver);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_web2c_recorder(&project.path("main.fls"));

    let standalone = Project::new();
    standalone.write("main.tex", &article("Text"));
    let output = Command::new(env!("CARGO_BIN_EXE_pdflatex"))
        .args(["-recorder", "main.tex"])
        .current_dir(&standalone.0)
        .env("HOME", standalone.path("home"))
        .env("TEX_RS_CACHE_DIR", standalone.path("cache"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_web2c_recorder(&standalone.path("main.fls"));
}
