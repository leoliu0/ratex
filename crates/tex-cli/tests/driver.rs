#![cfg(unix)]

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;
use std::{hash::Hasher, io::Write};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str, engine: &str) -> Self {
        let path = std::env::temp_dir().join(format!("texmk-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let f = Self(path);
        f.write("main.tex", "test");
        f.tool(
            "pdflatex",
            &format!(
                r#"out=.
aux=.
job=main
while [ "$#" -gt 0 ]; do
  case "$1" in
    -output-directory) out=$2; shift 2 ;;
    -aux-directory|-auxdir) aux=$2; shift 2 ;;
    -jobname) job=$2; shift 2 ;;
    *) shift ;;
  esac
done
mkdir -p "$out" "$aux"
{engine}"#
            ),
        );
        f
    }
    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(name), text).unwrap();
    }
    fn tool(&self, name: &str, body: &str) {
        self.write(name, &format!("#!/bin/sh\nset -eu\n{body}\n"));
        std::fs::set_permissions(self.0.join(name), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    fn output(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_texmk"))
            .args(args)
            .arg("main.tex")
            .current_dir(&self.0)
            .env("TEXMK_LIB", &self.0)
            .env("TEX_RS_CACHE_DIR", self.0.join("cache"))
            .output()
            .unwrap()
    }
    fn run(&self) {
        let out = self.output(&[]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn contains_file(root: &std::path::Path, name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            contains_file(&path, name)
        } else {
            path.file_name().and_then(|value| value.to_str()) == Some(name)
        }
    })
}

fn find_file(root: &std::path::Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

fn file_hash(path: &std::path::Path) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    hash.write(&std::fs::read(path).unwrap());
    hash.finish()
}

fn hex_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len() * 2);
    for byte in text.bytes() {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn run_real_texmk(root: &std::path::Path, source: &str) -> std::process::Output {
    let tool_dir = std::path::Path::new(env!("CARGO_BIN_EXE_pdflatex"))
        .parent()
        .unwrap();
    Command::new(env!("CARGO_BIN_EXE_texmk"))
        .arg(source)
        .current_dir(root)
        .env("TEXMK_LIB", tool_dir)
        .env("TEX_RS_CACHE_DIR", root.join("cache"))
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .output()
        .unwrap()
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn one_copied_texmk_builds_with_embedded_latex_and_bibtex_resources() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "texmk-self-contained-{}-{nonce}",
        std::process::id()
    )));
    let bin = fixture.0.join("bin");
    let project = fixture.0.join("project");
    let poison = fixture.0.join("external-texmf/tex/latex/base");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&poison).unwrap();
    let standalone = bin.join("texmk");
    std::fs::copy(env!("CARGO_BIN_EXE_texmk"), &standalone).unwrap();
    std::fs::set_permissions(&standalone, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        poison.join("article.cls"),
        "\\errmessage{external article.cls was read}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("main.tex"),
        concat!(
            "\\documentclass[12pt]{article}\n",
            "\\usepackage[T1]{fontenc}\n",
            "\\usepackage{newtx}\n",
            "\\begin{document}Embedded citation: \\cite{entry}. \\textcopyright{} \\texttt{mono}.\n",
            "\\bibliographystyle{plain}\\bibliography{refs}\n",
            "\\end{document}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        project.join("refs.bib"),
        "@book{entry, author={Donald Knuth}, title={The TeXbook}, year={1984}}\n",
    )
    .unwrap();

    let mut output = None;
    for _ in 0..20 {
        match Command::new(&standalone)
            .arg("main.tex")
            .current_dir(&project)
            .env_clear()
            .env("HOME", fixture.0.join("home"))
            .env("TEX_RS_CACHE_DIR", fixture.0.join("cache"))
            .env("TEXMFHOME", fixture.0.join("external-texmf"))
            .env("TEXMFLOCAL", fixture.0.join("external-texmf"))
            .env("TEXMFDIST", fixture.0.join("external-texmf"))
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .output()
        {
            Ok(out) => {
                output = Some(out);
                break;
            }
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => panic!("failed to run standalone texmk: {e}"),
        }
    }
    let output = output.expect("standalone texmk output");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project.join("main.pdf").is_file());
    let blg = find_file(&fixture.0.join("cache/texmk/jobs"), "main.blg").unwrap();
    assert!(std::fs::read_to_string(blg)
        .unwrap()
        .contains("The style file: <embedded:plain.bst>"));
    let log = std::fs::read_to_string(
        find_file(&fixture.0.join("cache/texmk/jobs"), "main.log").unwrap(),
    )
    .unwrap();
    assert!(log.contains("(<embedded:xkeyval.tex>"));
    assert!(log.contains("(<embedded:binhex.tex>"));
    assert!(log.contains("ts1-qtmr"));
    assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 1);
}

#[test]
fn copied_texmk_ignores_external_tex_trees_until_explicitly_enabled() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "texmk-hermetic-search-{}-{nonce}",
        std::process::id()
    )));
    let bin = fixture.0.join("bin");
    let project = fixture.0.join("project");
    let texinputs = fixture.0.join("texinputs");
    let texmfhome = fixture.0.join("texmfhome");
    let adjacent = fixture.0.join("share/tex-suite/texmf");
    for directory in [&bin, &project, &texinputs] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let standalone = bin.join("texmk");
    std::fs::copy(env!("CARGO_BIN_EXE_texmk"), &standalone).unwrap();
    std::fs::set_permissions(&standalone, std::fs::Permissions::from_mode(0o755)).unwrap();

    let external_packages = [
        (
            "onlyinputs",
            texinputs.join("onlyinputs.sty"),
            "EXTERNAL-TEXINPUTS-RAN",
        ),
        (
            "onlyhome",
            texmfhome.join("tex/latex/test/onlyhome.sty"),
            "EXTERNAL-TEXMFHOME-RAN",
        ),
        (
            "onlyadjacent",
            adjacent.join("tex/latex/test/onlyadjacent.sty"),
            "EXTERNAL-ADJACENT-RAN",
        ),
    ];
    for (package, path, marker) in &external_packages {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("\\ProvidesPackage{{{package}}}\\typeout{{{marker}}}\\endinput\n"),
        )
        .unwrap();
        std::fs::write(
            project.join(format!("{package}.tex")),
            format!(
                "\\documentclass{{article}}\\usepackage{{{package}}}\\begin{{document}}x\\end{{document}}\n"
            ),
        )
        .unwrap();
    }

    for (package, _, _) in &external_packages {
        let output = Command::new(&standalone)
            .arg(format!("{package}.tex"))
            .current_dir(&project)
            .env_clear()
            .env("HOME", fixture.0.join("home"))
            .env(
                "TEX_RS_CACHE_DIR",
                fixture.0.join(format!("cache-default-{package}")),
            )
            .env("TEXINPUTS", &texinputs)
            .env("TEXMFHOME", &texmfhome)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{package} unexpectedly resolved");
        assert!(
            stderr.contains(&format!("{package}.sty")) && stderr.contains("not found"),
            "unexpected diagnostic for {package}:\n{}\n{stderr}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    std::fs::write(
        project.join("allowed.tex"),
        concat!(
            "\\documentclass{article}\n",
            "\\usepackage{onlyinputs,onlyhome,onlyadjacent}\n",
            "\\begin{document}external opt-in\\end{document}\n"
        ),
    )
    .unwrap();
    let output = Command::new(&standalone)
        .args(["--allow-system-texmf", "allowed.tex"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "--allow-system-texmf must be rejected"
    );
    let hermetic_after_opt_in = Command::new(&standalone)
        .arg("allowed.tex")
        .current_dir(&project)
        .env_clear()
        .env("HOME", fixture.0.join("home"))
        .env("TEX_RS_CACHE_DIR", fixture.0.join("cache-allowed"))
        .env("TEXINPUTS", &texinputs)
        .env("TEXMFHOME", &texmfhome)
        .output()
        .unwrap();
    assert!(
        !hermetic_after_opt_in.status.success(),
        "a hermetic build reused state produced with external TeX trees"
    );
    let stderr = String::from_utf8_lossy(&hermetic_after_opt_in.stderr);
    assert!(
        stderr.contains("onlyinputs.sty") && stderr.contains("not found"),
        "unexpected post-opt-in hermetic diagnostic:\n{}\n{stderr}",
        String::from_utf8_lossy(&hermetic_after_opt_in.stdout)
    );
}

#[test]
fn copied_texmk_symlink_personalities_need_no_sibling_executables() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "texmk-multicall-aliases-{}-{nonce}",
        std::process::id()
    )));
    let bin = fixture.0.join("bin");
    let project = fixture.0.join("project");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let standalone = bin.join("texmk");
    std::fs::copy(env!("CARGO_BIN_EXE_texmk"), &standalone).unwrap();
    std::fs::set_permissions(&standalone, std::fs::Permissions::from_mode(0o755)).unwrap();
    for alias in [
        "ratex",
        "pdflatex",
        "xelatex",
        "lualatex",
        "bibtex",
        "tex-bibtex",
        "latexmk",
    ] {
        std::os::unix::fs::symlink("texmk", bin.join(alias)).unwrap();
    }
    let physical_files = std::fs::read_dir(&bin)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_type().unwrap().is_file())
        .count();
    assert_eq!(physical_files, 1);
    let run_with_retry = |mut cmd: Command| -> std::process::Output {
        for _ in 0..20 {
            match cmd.output() {
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Ok(out) => return out,
                Err(e) => panic!("command failed: {e}"),
            }
        }
        cmd.output().expect("command failed after retries")
    };

    for alias in [
        "ratex", "texmk", "pdflatex", "xelatex", "lualatex", "latexmk",
    ] {
        let mut cmd = Command::new(bin.join(alias));
        cmd.arg("--version")
            .env_clear()
            .env("HOME", fixture.0.join("home"));
        let output = run_with_retry(cmd);
        assert!(output.status.success(), "{alias} --version failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.to_lowercase().contains("ratex"),
            "alias {alias} did not identify Ratex engine: {stdout}"
        );
        assert!(
            stdout.contains(env!("CARGO_PKG_VERSION")),
            "alias {alias} reported a stale version: {stdout}"
        );
        assert!(
            !stdout.contains("LuaHBTeX") && !stdout.contains("XeTeX 3."),
            "alias {alias} falsely claimed non-Ratex runtime: {stdout}"
        );
    }

    std::fs::write(
        project.join("engine.tex"),
        "\\documentclass{article}\\begin{document}alias\\end{document}\n",
    )
    .unwrap();
    let mut engine_cmd = Command::new(bin.join("pdflatex"));
    engine_cmd
        .args(["-interaction=batchmode", "-halt-on-error", "engine.tex"])
        .current_dir(&project)
        .env_clear()
        .env("HOME", fixture.0.join("home"));
    let engine = run_with_retry(engine_cmd);
    assert!(
        engine.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&engine.stdout),
        String::from_utf8_lossy(&engine.stderr)
    );
    assert!(project.join("engine.pdf").is_file());

    std::fs::write(
        project.join("refs.bib"),
        "@book{entry, author={Donald Knuth}, title={The TeXbook}, year={1984}}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("main.aux"),
        "\\citation{entry}\n\\bibstyle{plain}\n\\bibdata{refs}\n",
    )
    .unwrap();
    let mut bibtex_cmd = Command::new(bin.join("bibtex"));
    bibtex_cmd
        .arg("main")
        .current_dir(&project)
        .env_clear()
        .env("HOME", fixture.0.join("home"));
    let bibtex = run_with_retry(bibtex_cmd);
    assert!(
        bibtex.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&bibtex.stdout),
        String::from_utf8_lossy(&bibtex.stderr)
    );
    assert!(project.join("main.bbl").is_file());
    assert!(std::fs::read_to_string(project.join("main.blg"))
        .unwrap()
        .contains("The style file: <embedded:plain.bst>"));
}

#[test]
fn publishing_the_pdf_does_not_defer_the_first_engine_cache_hit() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "texmk-published-output-cache-{}-{nonce}",
        std::process::id()
    )));
    let source = fixture.0.join("docs");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("main.tex"),
        "\\documentclass{article}\\nofiles\\begin{document}Stable.\\end{document}\n",
    )
    .unwrap();

    let cold = run_real_texmk(&fixture.0, "docs/main.tex");
    assert!(
        cold.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&cold.stdout),
        String::from_utf8_lossy(&cold.stderr)
    );
    assert!(source.join("main.pdf").is_file());

    let private_log = find_file(&fixture.0.join("cache/texmk/jobs"), "main.log").unwrap();
    std::fs::write(&private_log, "first-warm-run cache sentinel").unwrap();
    let warm = run_real_texmk(&fixture.0, "docs/main.tex");
    assert!(
        warm.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&warm.stdout),
        String::from_utf8_lossy(&warm.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(private_log).unwrap(),
        "first-warm-run cache sentinel",
        "publishing main.pdf invalidated an unrelated source-directory membership snapshot"
    );
}

#[test]
fn redefined_at_input_observes_created_aux_before_texmk_converges() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "texmk-boilerplate-first-repeat-{}-{nonce}",
        std::process::id()
    )));
    let source = fixture.0.join("docs");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("main.tex"),
        concat!(
            "\\documentclass{article}\n",
            "\\makeatletter\n",
            "\\def\\@input#1{\\IfFileExists{#1}",
            "{\\typeout{AUX-STATE=PRESENT}}{\\typeout{AUX-STATE=MISSING}}}\n",
            "\\makeatother\n",
            "\\begin{document}Stable.\\end{document}\n"
        ),
    )
    .unwrap();

    let cold = run_real_texmk(&fixture.0, "docs/main.tex");
    assert!(
        cold.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&cold.stdout),
        String::from_utf8_lossy(&cold.stderr)
    );
    let private_log = find_file(&fixture.0.join("cache/texmk/jobs"), "main.log").unwrap();
    let log = std::fs::read_to_string(&private_log).unwrap();
    assert!(
        log.contains("AUX-STATE=PRESENT"),
        "the converged pass did not observe the newly created aux:\n{log}"
    );
    assert!(
        !log.contains("AUX-STATE=MISSING"),
        "texmk reused first-pass output before main.aux existed:\n{log}"
    );

    std::fs::write(&private_log, "stable aux-state cache sentinel").unwrap();

    let warm = run_real_texmk(&fixture.0, "docs/main.tex");
    assert!(
        warm.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&warm.stdout),
        String::from_utf8_lossy(&warm.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(private_log).unwrap(),
        "stable aux-state cache sentinel",
        "the stable second pass should seed the later invocation cache"
    );
}

#[test]
fn non_boilerplate_aux_creation_still_executes_the_next_texmk_pass() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "texmk-non-boilerplate-repeat-{}-{nonce}",
        std::process::id()
    )));
    let source = fixture.0.join("docs");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("main.tex"),
        concat!(
            "\\documentclass{article}\n",
            "\\begin{document}\\ref{answer}\\label{answer}\\end{document}\n"
        ),
    )
    .unwrap();

    let output = run_real_texmk(&fixture.0, "docs/main.tex");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let private_log = find_file(&fixture.0.join("cache/texmk/jobs"), "main.log").unwrap();
    assert!(
        !std::fs::read_to_string(private_log)
            .unwrap()
            .contains("undefined references"),
        "the non-boilerplate label state did not receive a resolving pass"
    );
}

#[test]
fn page_count_aux_is_observed_when_a_rerun_signal_is_present() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "texmk-provisional-boilerplate-{}-{nonce}",
        std::process::id()
    )));
    let source = fixture.0.join("docs");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("main.tex"),
        concat!(
            "\\documentclass{article}\n",
            "\\makeatletter\n",
            "\\@ifundefined{@abspage@last}",
            "{\\gdef\\@abspage@last{1073741823}}{}\n",
            "\\AtBeginDocument{\\typeout{OBSERVED-LAST=\\@abspage@last}}\n",
            "\\makeatother\n",
            "\\typeout{Rerun to get cross-references right.}\n",
            "\\begin{document}Stable.\\end{document}\n"
        ),
    )
    .unwrap();

    let output = run_real_texmk(&fixture.0, "docs/main.tex");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let private_log = find_file(&fixture.0.join("cache/texmk/jobs"), "main.log").unwrap();
    let log = std::fs::read_to_string(private_log).unwrap();
    assert!(
        log.contains("OBSERVED-LAST=1"),
        "the required second pass reused provisional first-pass output:\n{log}"
    );
    assert!(!log.contains("OBSERVED-LAST=1073741823"));
}

fn assert_published_pdf_probe_invalidates(probed_name: &str) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "texmk-published-output-probe-{}-{nonce}-{probed_name}",
        std::process::id(),
    )));
    let source = fixture.0.join("docs");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("main.tex"),
        format!(
            concat!(
                "\\documentclass{{article}}\\nofiles\\begin{{document}}",
                "\\IfFileExists{{{}}}",
                "{{\\errmessage{{published PDF became visible}}}}{{}}",
                "Stable.\\end{{document}}\n"
            ),
            probed_name
        ),
    )
    .unwrap();

    let cold = run_real_texmk(&fixture.0, "docs/main.tex");
    assert!(
        cold.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&cold.stdout),
        String::from_utf8_lossy(&cold.stderr)
    );
    assert!(source.join("main.pdf").is_file());

    let warm = run_real_texmk(&fixture.0, "docs/main.tex");
    assert!(
        !warm.status.success(),
        "the published PDF's appearance was hidden from \\IfFileExists{{{probed_name}}}"
    );
}

#[test]
fn published_pdf_exclusion_does_not_hide_an_explicit_missing_file_dependency() {
    assert_published_pdf_probe_invalidates("main.pdf");
}

#[test]
fn published_pdf_exclusion_preserves_casefolded_missing_file_dependencies() {
    assert_published_pdf_probe_invalidates("MAIN.PDF");
}

#[cfg(unix)]
#[test]
fn explicit_aux_symlink_fails_before_the_engine_and_preserves_its_target() {
    use std::os::unix::fs::symlink;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(
        std::env::temp_dir().join(format!("texmk-aux-symlink-{}-{nonce}", std::process::id())),
    );
    let source = fixture.0.join("docs");
    let aux = fixture.0.join("state");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir(&aux).unwrap();
    std::fs::write(
        source.join("main.tex"),
        "\\documentclass{article}\\begin{document}\\ref{x}\\label{x}\\end{document}\n",
    )
    .unwrap();
    let outside = fixture.0.join("outside-target.aux");
    std::fs::write(&outside, "outside sentinel\n").unwrap();
    symlink(&outside, aux.join("main.aux")).unwrap();

    let tool_dir = std::path::Path::new(env!("CARGO_BIN_EXE_pdflatex"))
        .parent()
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_texmk"))
        .args(["-aux-directory", "state", "docs/main.tex"])
        .current_dir(&fixture.0)
        .env("TEXMK_LIB", tool_dir)
        .env("TEX_RS_CACHE_DIR", fixture.0.join("cache"))
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot inspect auxiliary state")
            && stderr.contains("main.aux")
            && stderr.contains("symbolic link"),
        "{}\n{stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "outside sentinel\n"
    );
    assert!(
        find_file(&fixture.0.join("cache/texmk/jobs"), "main.pdf").is_none(),
        "the TeX engine ran before the unsafe aux tree was rejected"
    );
}

#[test]
fn default_build_keeps_recursive_state_private_and_only_publishes_the_pdf() {
    let f = Fixture::new(
        "private-state",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
mkdir -p "$aux/nested"
if [ "$n" -eq 1 ]; then value=first; else value=stable; fi
echo "$value" > "$aux/nested/custom.state"
echo 'volatile transcript' > "$aux/$job.log"
echo '\relax' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Pages /Count 1 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.run();
    assert!(f.0.join("main.pdf").is_file());
    assert!(!f.0.join("main.aux").exists());
    assert!(!f.0.join("main.log").exists());
    assert!(!f.0.join("nested/custom.state").exists());
    assert!(contains_file(&f.0.join("cache"), "custom.state"));
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "3",
        "recursive custom state must participate in convergence"
    );
}

#[test]
fn retention_flags_export_only_the_requested_artifacts() {
    let logs = Fixture::new(
        "keep-log",
        r#"
mkdir -p "$aux/nested"
echo log > "$aux/$job.log"
echo aux > "$aux/$job.aux"
echo nested > "$aux/nested/custom.state"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    let out = logs.output(&["--keep-logs"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(logs.0.join("main.log").is_file());
    assert!(!logs.0.join("main.aux").exists());
    assert!(!logs.0.join("nested/custom.state").exists());

    let all = Fixture::new(
        "keep-all",
        r#"
mkdir -p "$aux/nested"
echo log > "$aux/$job.log"
echo aux > "$aux/$job.aux"
echo nested > "$aux/nested/custom.state"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    let out = all.output(&["-k"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.0.join("main.log").is_file());
    assert!(all.0.join("main.aux").is_file());
    assert!(all.0.join("nested/custom.state").is_file());
}

#[test]
fn keep_intermediates_exports_only_job_files_and_preserves_conflicts() {
    let f = Fixture::new(
        "safe-explicit-export",
        r#"
mkdir -p "$aux/nested"
echo generated-aux > "$aux/$job.aux"
echo generated-log > "$aux/$job.log"
echo generated-custom > "$aux/nested/custom.state"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    std::fs::create_dir_all(f.0.join("state/nested")).unwrap();
    std::fs::create_dir_all(f.0.join("out/nested")).unwrap();
    f.write("state/unrelated.txt", "source foreign");
    f.write("state/main.notes", "job-prefixed foreign file");
    f.write("state/main.toc", "unchanged unowned job artifact");
    f.write("state/nested/foreign.txt", "nested foreign");
    f.write("out/main.aux", "destination owner");
    f.write("out/nested/custom.state", "nested destination owner");

    let out = f.output(&["-output-directory", "out", "-aux-directory", "state", "-k"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!f.0.join("out/unrelated.txt").exists());
    assert!(!f.0.join("out/main.notes").exists());
    assert!(!f.0.join("out/main.toc").exists());
    assert!(!f.0.join("out/nested/foreign.txt").exists());
    assert_eq!(
        std::fs::read_to_string(f.0.join("out/main.aux")).unwrap(),
        "destination owner"
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("out/nested/custom.state")).unwrap(),
        "nested destination owner"
    );
    assert!(f.0.join("out/main.log").is_file());
}

#[test]
fn keep_intermediates_refuses_symlinked_export_parents() {
    let f = Fixture::new(
        "symlink-export-parent",
        r#"
mkdir -p "$aux/nested"
echo generated > "$aux/nested/custom.state"
echo stable > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    std::fs::create_dir_all(f.0.join("out")).unwrap();
    std::fs::create_dir_all(f.0.join("outside")).unwrap();
    std::os::unix::fs::symlink("../outside", f.0.join("out/nested")).unwrap();

    let out = f.output(&["-output-directory", "out", "-k"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(f.0.join("out/main.pdf").is_file());
    assert!(
        !f.0.join("outside/custom.state").exists(),
        "retained artifact escaped through an output-directory symlink"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("refusing export"));
}

#[test]
fn clean_removes_only_unchanged_owned_explicit_aux_files() {
    let f = Fixture::new(
        "explicit-aux-clean",
        r#"
mkdir -p "$aux/nested"
echo generated-aux > "$aux/$job.aux"
echo generated-log > "$aux/$job.log"
echo generated-custom > "$aux/nested/custom.state"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    std::fs::create_dir_all(f.0.join("state")).unwrap();
    f.write("state/unrelated.keep", "foreign");
    let args = ["-aux-directory", "state"];
    let build = f.output(&args);
    assert!(build.status.success());
    f.write("state/main.log", "user changed this log");
    let clean = f.output(&["-aux-directory", "state", "-c"]);
    assert!(clean.status.success());
    assert!(!f.0.join("state/main.aux").exists());
    assert!(!f.0.join("state/nested/custom.state").exists());
    assert_eq!(
        std::fs::read_to_string(f.0.join("state/main.log")).unwrap(),
        "user changed this log"
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("state/unrelated.keep")).unwrap(),
        "foreign"
    );
    assert!(f.0.join("main.pdf").is_file());
}

#[test]
fn normal_build_removes_unchanged_exports_from_a_prior_keep_build() {
    let f = Fixture::new(
        "drop-old-exports",
        r#"
mkdir -p "$aux/nested"
echo aux > "$aux/$job.aux"
echo log > "$aux/$job.log"
echo custom > "$aux/nested/custom.state"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    assert!(f.output(&["-k"]).status.success());
    assert!(f.0.join("main.aux").is_file());
    assert!(f.0.join("main.log").is_file());
    assert!(f.0.join("nested/custom.state").is_file());
    f.run();
    assert!(!f.0.join("main.aux").exists());
    assert!(!f.0.join("main.log").exists());
    assert!(!f.0.join("nested/custom.state").exists());
}

#[test]
fn persisted_manifest_paths_cannot_escape_managed_directories() {
    let f = Fixture::new(
        "manifest-traversal",
        r#"
echo stable > "$aux/$job.aux"
echo log > "$aux/$job.log"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    let args = ["-output-directory", "out"];
    assert!(f.output(&args).status.success());
    std::fs::create_dir_all(f.0.join("out")).unwrap();
    let victim = f.0.join("victim.txt");
    f.write("victim.txt", "user-owned bytes");
    let traversal = f.0.join("out/../victim.txt");
    let manifest = find_file(&f.0.join("cache/texmk/jobs"), "manifest").unwrap();
    let malicious = format!(
        "export\t{}\t{}\naux\t{}\t{}\n",
        hex_encode(&traversal.to_string_lossy()),
        file_hash(&victim),
        hex_encode(&traversal.to_string_lossy()),
        file_hash(&victim)
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(&manifest)
        .unwrap()
        .write_all(malicious.as_bytes())
        .unwrap();

    assert!(f.output(&args).status.success());
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "user-owned bytes"
    );

    std::fs::OpenOptions::new()
        .append(true)
        .open(&manifest)
        .unwrap()
        .write_all(malicious.as_bytes())
        .unwrap();
    assert!(f
        .output(&["-output-directory", "out", "-c"])
        .status
        .success());
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "user-owned bytes"
    );
}

#[test]
fn clean_modes_respect_pdf_ownership_and_modifications() {
    let owned = Fixture::new(
        "clean-owned",
        r#"
echo aux > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    owned.run();
    assert!(owned.output(&["-C"]).status.success());
    assert!(!owned.0.join("main.pdf").exists());

    let modified = Fixture::new(
        "clean-modified",
        r#"
echo aux > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    modified.run();
    std::fs::write(modified.0.join("main.pdf"), "user replacement").unwrap();
    assert!(modified.output(&["-C"]).status.success());
    assert_eq!(
        std::fs::read_to_string(modified.0.join("main.pdf")).unwrap(),
        "user replacement"
    );

    let preserve = Fixture::new(
        "clean-aux",
        r#"
echo aux > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    preserve.run();
    assert!(preserve.output(&["-c"]).status.success());
    assert!(preserve.0.join("main.pdf").is_file());
    assert!(!preserve
        .0
        .join("cache/texmk/jobs")
        .read_dir()
        .unwrap()
        .next()
        .is_some());
}

#[test]
fn concurrent_builds_of_one_job_are_serialized() {
    let f = Fixture::new(
        "concurrent-lock",
        r#"
if ! mkdir engine-active 2>/dev/null; then echo overlap > overlap; fi
sleep 0.10
echo aux > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
rmdir engine-active
"#,
    );
    let spawn = || {
        Command::new(env!("CARGO_BIN_EXE_texmk"))
            .arg("main.tex")
            .current_dir(&f.0)
            .env("TEXMK_LIB", &f.0)
            .env("TEX_RS_CACHE_DIR", f.0.join("cache"))
            .spawn()
            .unwrap()
    };
    let mut first = spawn();
    let mut second = spawn();
    assert!(first.wait().unwrap().success());
    assert!(second.wait().unwrap().success());
    assert!(
        !f.0.join("overlap").exists(),
        "engine processes overlapped despite the per-job lock"
    );
}

#[test]
fn changed_aux_without_warnings_requires_another_pass() {
    let f = Fixture::new(
        "stable-aux",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
echo 'stable auxiliary contents' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "2"
    );
}

#[test]
fn newly_created_aux_always_requires_a_second_pass() {
    let f = Fixture::new(
        "boilerplate-aux",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
printf '%s\n' '\relax' '\gdef \@abspage@last{1}' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "2"
    );
}

#[test]
fn labels_and_undefined_references_require_another_pass() {
    let f = Fixture::new(
        "label-ref-state",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
printf '%s\n' '\relax' '\newlabel{section}{{1}{1}}' '\gdef \@abspage@last{1}' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
echo 'LaTeX Warning: There were undefined references.'
"#,
    );
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "2"
    );
}

#[test]
fn table_of_contents_state_requires_another_pass() {
    let f = Fixture::new(
        "toc-state",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
printf '%s\n' '\relax' '\gdef \@abspage@last{1}' > "$aux/$job.aux"
printf '%s\n' '\contentsline {section}{Heading}{1}{}%' > "$aux/$job.toc"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "2"
    );
}

#[test]
fn unknown_aux_command_requires_another_pass() {
    let f = Fixture::new(
        "unknown-aux-state",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
printf '%s\n' '\relax' '\customauxstate{opaque}' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "2"
    );
}

#[test]
fn bibliography_state_still_runs_bibtex_and_another_typeset() {
    let f = Fixture::new(
        "bibliography-multipass",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
printf '%s\n' '\citation{entry}' '\bibdata{refs}' '\bibstyle{plain}' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.write("refs.bib", "@book{entry, title={Available}}\n");
    f.tool(
        "tex-bibtex",
        "echo bbl > \"$1.bbl\"\necho called >> bibcalls",
    );
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "2"
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn current_source_bibliography_is_used_without_regeneration() {
    let f = Fixture::new(
        "source-bibliography",
        r#"
printf '%s\n' '\citation{entry,unavailable}' '\bibdata{refs}' '\bibstyle{plain}' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.write("refs.bib", "@book{entry, title={Current}}\n");
    f.write(
        "main.bbl",
        "\\begin{thebibliography}{1}\n\\bibitem{entry} curated\n\\end{thebibliography}\n",
    );
    f.tool(
        "tex-bibtex",
        "echo called >> bibcalls\nprintf 'generated\\n' > \"$1.bbl\"",
    );

    f.run();

    assert!(
        !f.0.join("bibcalls").exists(),
        "an up-to-date source bibliography must remain authoritative"
    );
    let staged = find_file(&f.0.join("cache/texmk/jobs"), "main.bbl").unwrap();
    assert!(std::fs::read_to_string(staged).unwrap().contains("curated"));
}

#[test]
fn missing_bibliography_database_keeps_the_pdf_build_nonfatal() {
    let f = Fixture::new(
        "missing-bibliography-database",
        r#"
printf '%s\n' '\citation{entry}' '\bibdata{missing}' '\bibstyle{plain}' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.tool("tex-bibtex", "echo called >> bibcalls\nexit 99");

    f.run();
    assert!(
        !f.0.join("bibcalls").exists(),
        "BibTeX must remain conditional when a declared database is unavailable"
    );
}

#[test]
fn bibliography_in_recursive_aux_is_discovered_once_despite_a_cycle() {
    let f = Fixture::new(
        "recursive-aux-bibliography",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
mkdir -p "$aux/chapters"
printf '%s\n' '\@input{chapters/one.aux}' > "$aux/$job.aux"
printf '%s\n' '\@input{main.aux}' '\citation{entry}' '\bibdata{refs}' '\bibstyle{plain}' > "$aux/chapters/one.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.write("refs.bib", "@book{entry, title={Available}}\n");
    f.tool(
        "tex-bibtex",
        "echo bbl > \"$1.bbl\"\necho called >> bibcalls",
    );
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "2"
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn recursive_aux_uses_including_directory_and_accepts_bibtex_whitespace() {
    let f = Fixture::new(
        "recursive-aux-shadow",
        r#"
choice=$(cat bibchoice)
mkdir -p "$aux/chapters"
printf '%s\n' '  \@input {chapters/one.aux} trailing text' > "$aux/$job.aux"
printf '%s\n' ' \@input {two} trailing text' > "$aux/chapters/one.aux"
printf '  \\citation {%s} trailing\n \\bibdata {refs} trailing\n \\bibstyle {plain} trailing\n' "$choice" > "$aux/chapters/two.aux"
printf '%s\n' '\citation{shadow}' '\bibdata{shadow}' '\bibstyle{shadow}' > "$aux/two.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.write("bibchoice", "old");
    f.write(
        "refs.bib",
        "@book{old, title={Old}}\n@book{new, title={New}}\n",
    );
    f.tool(
        "tex-bibtex",
        r#"auxdir=$(dirname "$1")
cp "$auxdir/chapters/two.aux" "$1.bbl"
echo called >> bibcalls"#,
    );

    f.run();
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        1
    );

    f.write("bibchoice", "new");
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        2,
        "nested sibling aux change was hidden by the aux-root shadow"
    );
    let bbl = find_file(&f.0.join("cache/texmk/jobs"), "main.bbl").unwrap();
    let bbl = std::fs::read_to_string(bbl).unwrap();
    assert!(bbl.contains("citation {new}"), "{bbl}");
    assert!(!bbl.contains("shadow"), "{bbl}");
}

#[test]
fn missing_recursive_aux_disables_stale_bibliography_reuse() {
    let f = Fixture::new(
        "missing-recursive-aux",
        r#"
printf '%s\n' '\@input{chapter.aux}' > "$aux/$job.aux"
if [ -f include-bib ]; then
  printf '%s\n' '\citation{entry}' '\bibdata{refs}' '\bibstyle{plain}' > "$aux/chapter.aux"
else
  rm -f "$aux/chapter.aux"
fi
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.write("refs.bib", "@book{entry, title={Available}}\n");
    f.tool(
        "tex-bibtex",
        "echo bbl > \"$1.bbl\"\necho called >> bibcalls",
    );
    f.write("include-bib", "yes");
    f.run();
    std::fs::remove_file(f.0.join("include-bib")).unwrap();
    f.run();
    f.write("include-bib", "yes");
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        2,
        "a missing included aux must invalidate the old bibliography signature"
    );
}

#[test]
fn explicit_child_cache_hit_keeps_the_published_pdf_inode() {
    let f = Fixture::new(
        "child-cache-hit",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
if [ -f report-hit ]; then
  : > "${TEX_RS_CACHE_HIT_MARKER:?}"
else
  echo stable > "$aux/$job.aux"
  echo 'LaTeX Warning: There were undefined references.' > "$aux/$job.log"
  printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
fi
"#,
    );
    f.run();
    let staged = find_file(&f.0.join("cache/texmk/jobs"), "main.pdf").unwrap();
    let before = std::fs::metadata(f.0.join("main.pdf")).unwrap();
    assert_eq!(before.ino(), std::fs::metadata(&staged).unwrap().ino());

    f.write("report-hit", "yes");
    let hit = f.output(&[]);
    assert!(
        hit.status.success(),
        "{}",
        String::from_utf8_lossy(&hit.stderr)
    );
    assert!(
        String::from_utf8_lossy(&hit.stderr).contains("unresolved references remain"),
        "warm cache hit suppressed unresolved-reference diagnostics"
    );
    let after = std::fs::metadata(f.0.join("main.pdf")).unwrap();
    assert_eq!(before.ino(), after.ino(), "cache hit republished the PDF");
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "3",
        "cache hit should finish in one child process"
    );

    std::fs::remove_file(f.0.join("main.pdf")).unwrap();
    std::os::unix::fs::symlink(&staged, f.0.join("main.pdf")).unwrap();
    let hit = f.output(&[]);
    assert!(
        hit.status.success(),
        "{}",
        String::from_utf8_lossy(&hit.stderr)
    );
    assert!(
        std::fs::symlink_metadata(f.0.join("main.pdf"))
            .unwrap()
            .file_type()
            .is_file(),
        "same-inode shortcut accepted a symlink as the published PDF"
    );
}

#[test]
fn managed_cache_directories_cannot_escape_the_job_tree() {
    for private_directory in ["pdf", "aux"] {
        let f = Fixture::new(
            &format!("private-{private_directory}-containment"),
            r#"
echo stable > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
        );
        f.run();
        let staged = find_file(&f.0.join("cache/texmk/jobs"), "main.pdf").unwrap();
        let job_dir = staged.parent().unwrap().parent().unwrap().to_path_buf();
        let private = job_dir.join(private_directory);
        std::fs::remove_dir_all(&private).unwrap();
        let outside = f.0.join(format!("outside-{private_directory}"));
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, &private).unwrap();

        let output = f.output(&[]);
        assert!(
            !output.status.success(),
            "escaped private {private_directory} directory was accepted"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("escapes its private cache root"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            std::fs::read_dir(outside).unwrap().next().is_none(),
            "engine wrote through escaped private {private_directory} directory"
        );
    }
}

#[test]
fn jobname_is_forwarded_as_one_option_value_and_one_source() {
    let f = Fixture::new("jobname-args", "exit 99");
    f.tool(
        "pdflatex",
        r#"
positional=0
input=
out=.
aux=.
job=main
while [ "$#" -gt 0 ]; do
  case "$1" in
    -output-directory) out=$2; shift 2 ;;
    -aux-directory|-auxdir) aux=$2; shift 2 ;;
    --cache-directory|-cache-directory) shift 2 ;;
    -jobname) job=$2; shift 2 ;;
    -*) shift ;;
    *) positional=$((positional + 1)); input=$1; shift ;;
  esac
done
[ "$positional" -eq 1 ] || { echo "received $positional positional arguments" >&2; exit 42; }
[ -f "$input" ] || { echo "source argument is not a file: $input" >&2; exit 43; }
mkdir -p "$out" "$aux"
echo aux > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    let out = f.output(&["-jobname", "renamed"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(f.0.join("renamed.pdf").is_file());
}

#[test]
fn jobname_must_be_one_safe_path_component() {
    let f = Fixture::new(
        "jobname-containment",
        r#"
echo "engine unexpectedly ran" >&2
exit 91
"#,
    );
    for name in ["../escaped", "nested/name", "nested\\name", ".", ".."] {
        let out = f.output(&["-jobname", name]);
        assert_eq!(out.status.code(), Some(2), "job name {name:?} was accepted");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("must be one file-name component"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn clean_recovers_a_mismatched_manifest_without_deleting_the_pdf() {
    let f = Fixture::new(
        "clean-mismatched-manifest",
        r#"
echo stable > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.run();
    let manifest = find_file(&f.0.join("cache/texmk/jobs"), "manifest").unwrap();
    let text = std::fs::read_to_string(&manifest).unwrap();
    let mut rewritten = String::new();
    for line in text.lines() {
        if line.starts_with("identity\t") {
            rewritten.push_str(&format!("identity\t{}\n", hex_encode("corrupt identity")));
        } else {
            rewritten.push_str(line);
            rewritten.push('\n');
        }
    }
    std::fs::write(&manifest, rewritten).unwrap();

    let clean = f.output(&["-c"]);
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    assert!(f.0.join("main.pdf").is_file());
    assert!(!manifest.parent().unwrap().exists());
}

#[test]
fn pdf_changes_do_not_prevent_convergence_when_aux_equals_output() {
    let f = Fixture::new(
        "shared-output-aux",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
echo stable > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page pass=%s ' "$n" > "$out/$job.pdf"
"#,
    );
    let out = f.output(&[
        "-output-directory",
        "artifacts",
        "-aux-directory",
        "artifacts",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "2"
    );
}

#[test]
fn cache_gc_preserves_foreign_directories_and_removes_corrupt_job_trees() {
    let f = Fixture::new(
        "gc-ownership",
        r#"
echo stable > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    let foreign = f.0.join("cache/texmk/jobs/foreign-cache");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::File::create(foreign.join("foreign.data"))
        .unwrap()
        .set_len(513 * 1024 * 1024)
        .unwrap();
    let corrupt = f.0.join("cache/texmk/jobs/0123456789abcdef");
    std::fs::create_dir_all(&corrupt).unwrap();
    std::fs::write(corrupt.join("manifest"), "not a texmk manifest\n").unwrap();
    std::fs::File::create(corrupt.join("partial.data"))
        .unwrap()
        .set_len(513 * 1024 * 1024)
        .unwrap();

    f.run();
    assert!(
        foreign.join("foreign.data").is_file(),
        "cache GC removed a directory it does not own"
    );
    assert!(!corrupt.exists(), "corrupt hex-named job cache survived GC");
}

#[test]
fn first_pass_citation_changes_refresh_preliminary_bibliography() {
    let f = Fixture::new(
        "new-citation",
        r#"
printf '\\citation{new}\n\\bibdata{refs}\n\\bibstyle{plain}\n' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
echo "LaTeX Warning: Citation new undefined"
"#,
    );
    f.write(
        "refs.bib",
        "@book{old, title={Old}}\n@book{new, title={New}}\n",
    );
    std::fs::create_dir_all(f.0.join("state")).unwrap();
    f.write(
        "state/main.aux",
        "\\citation{old}\n\\bibdata{refs}\n\\bibstyle{plain}\n",
    );
    f.tool(
        "tex-bibtex",
        "cp \"$1.aux\" \"$1.bbl\"\necho called >> bibcalls",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_texmk"))
        .args(["-aux-directory", "state", "main.tex"])
        .current_dir(&f.0)
        .env("TEXMK_LIB", &f.0)
        .env("TEX_RS_CACHE_DIR", f.0.join("cache"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bbl = std::fs::read_to_string(f.0.join("state/main.bbl")).unwrap();
    assert!(bbl.contains("citation{new}"), "{bbl}");
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[test]
fn warm_bibliography_build_skips_unchanged_bibtex_inputs() {
    let f = Fixture::new(
        "warm-bibliography",
        r#"
printf '\\citation{entry}\n\\bibdata{refs}\n\\bibstyle{plain}\n' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.write("refs.bib", "@book{entry, title={Stable}}\n");
    f.tool(
        "tex-bibtex",
        "echo bbl > \"$1.bbl\"\necho called >> bibcalls",
    );

    f.run();
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        1,
        "an unchanged warm build should reuse its cached bibliography"
    );
}

#[test]
fn changed_cached_bibliography_output_forces_bibtex() {
    let f = Fixture::new(
        "changed-bibliography-output",
        r#"
printf '\\citation{entry}\n\\bibdata{refs}\n\\bibstyle{plain}\n' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.write("refs.bib", "@book{entry, title={Stable}}\n");
    f.tool(
        "tex-bibtex",
        "echo canonical-bbl > \"$1.bbl\"\necho called >> bibcalls",
    );
    f.run();
    f.run();
    let bbl = find_file(&f.0.join("cache/texmk/jobs"), "main.bbl").unwrap();
    std::fs::write(&bbl, "tampered cached bibliography\n").unwrap();

    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        2,
        "a changed cached bbl was reused"
    );
    assert_eq!(
        std::fs::read_to_string(bbl).unwrap().trim(),
        "canonical-bbl"
    );
}

#[test]
fn bibliography_cache_tracks_tex_search_paths_and_bibtex_executable() {
    let f = Fixture::new(
        "bibliography-tool-identity",
        r#"
printf '\\citation{entry}\n\\bibdata{refs}\n\\bibstyle{plain}\n' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    std::fs::create_dir_all(f.0.join("bibliography")).unwrap();
    f.write(
        "bibliography/refs.bib",
        "@book{entry, title={Version one}}\n",
    );
    f.tool(
        "tex-bibtex",
        "# tool version one\necho bbl > \"$1.bbl\"\necho called >> bibcalls",
    );
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_texmk"))
            .arg("main.tex")
            .current_dir(&f.0)
            .env("TEXMK_LIB", &f.0)
            .env("TEX_RS_CACHE_DIR", f.0.join("cache"))
            .env("TEXBIBINPUTS", f.0.join("bibliography"))
            .output()
            .unwrap()
    };

    assert!(run().status.success());
    assert!(run().status.success());
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        1
    );

    f.write(
        "bibliography/refs.bib",
        "@book{entry, title={Version two}}\n",
    );
    assert!(run().status.success());
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        2,
        "a TEXBIBINPUTS dependency change reused stale bibliography output"
    );

    f.tool(
        "tex-bibtex",
        "# tool version two\necho bbl > \"$1.bbl\"\necho called >> bibcalls",
    );
    assert!(run().status.success());
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        3,
        "a changed BibTeX executable reused stale bibliography output"
    );
}

#[test]
fn bibliography_cache_hashes_the_file_resolved_through_texmfhome() {
    let f = Fixture::new(
        "bibliography-texmfhome",
        r#"
printf '\\citation{entry}\n\\bibdata{refs}\n\\bibstyle{plain}\n' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    let texmf = f.0.join("texmf");
    let extra = f.0.join("tex-extra");
    std::fs::create_dir_all(texmf.join("bibtex/bib")).unwrap();
    std::fs::create_dir_all(texmf.join("bibtex/bst")).unwrap();
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(
        texmf.join("bibtex/bib/refs.bib"),
        "@book{entry, title={Version one}}\n",
    )
    .unwrap();
    std::fs::write(texmf.join("bibtex/bst/plain.bst"), "ENTRY{}{}{}\n").unwrap();
    std::fs::write(
        extra.join("refs.bib"),
        "@book{entry, title={Unused TEXBIBINPUTS shadow}}\n",
    )
    .unwrap();
    f.tool(
        "tex-bibtex",
        "echo bbl > \"$1.bbl\"\necho called >> bibcalls",
    );
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_texmk"))
            .arg("main.tex")
            .current_dir(&f.0)
            .env("TEXMK_LIB", &f.0)
            .env("TEX_RS_CACHE_DIR", f.0.join("cache"))
            .env("TEXMFHOME", &texmf)
            .env("TEXBIBINPUTS", &extra)
            .output()
            .unwrap()
    };

    assert!(run().status.success());
    assert!(run().status.success());
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        1
    );

    std::fs::write(
        texmf.join("bibtex/bib/refs.bib"),
        "@book{entry, title={Version two}}\n",
    )
    .unwrap();
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        2,
        "the TEXBIBINPUTS shadow masked a changed Kpse-resolved bibliography"
    );
}

#[test]
fn changing_bibliography_directive_to_an_older_file_refreshes_bibtex() {
    let f = Fixture::new(
        "bibliography-directive-hash",
        r#"
choice=$(cat bibchoice)
printf '\\citation{entry}\n\\bibdata{%s}\n\\bibstyle{plain}\n' "$choice" > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.write("refs-one.bib", "@book{entry, title={One}}\n");
    f.write("refs-two.bib", "@book{entry, title={Two}}\n");
    f.write("bibchoice", "refs-one");
    f.tool(
        "tex-bibtex",
        "cp \"$1.aux\" \"$1.bbl\"\necho called >> bibcalls",
    );
    f.run();
    f.run();
    f.write("bibchoice", "refs-two");
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(contains_file(&f.0.join("cache"), "main.bbl"));
}

#[test]
fn stale_dead_lock_does_not_make_a_valid_job_immortal_to_gc() {
    let f = Fixture::new(
        "stale-lock-gc",
        r#"
echo stable > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    f.run();
    let jobs = f.0.join("cache/texmk/jobs");
    let old_job = std::fs::read_dir(&jobs)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(old_job.with_extension("lock"), "999999-1").unwrap();
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    for path in [old_job.with_extension("lock"), old_job.join("manifest")] {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(f.0.join("cache/texmk/.gc-stamp"))
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old))
        .unwrap();
    let next = f.output(&["-jobname", "other"]);
    assert!(
        next.status.success(),
        "{}",
        String::from_utf8_lossy(&next.stderr)
    );
    assert!(!old_job.exists(), "stale locked job was not collected");
}

#[test]
fn failed_and_nonconvergent_builds_never_publish_partial_pdfs() {
    let failed = Fixture::new(
        "failed-pdf-stage",
        r#"
echo aux > "$aux/$job.aux"
printf 'partial pdf' > "$out/$job.pdf"
exit 1
"#,
    );
    failed.write("main.pdf", "previous pdf");
    assert!(!failed.output(&[]).status.success());
    assert_eq!(
        std::fs::read_to_string(failed.0.join("main.pdf")).unwrap(),
        "previous pdf"
    );

    let looping = Fixture::new(
        "nonconvergent-pdf-stage",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
echo "$n" > "$aux/$job.aux"
printf 'partial pass %s' "$n" > "$out/$job.pdf"
"#,
    );
    looping.write("main.pdf", "stable published pdf");
    assert!(!looping.output(&[]).status.success());
    assert_eq!(
        std::fs::read_to_string(looping.0.join("main.pdf")).unwrap(),
        "stable published pdf"
    );
}

#[test]
fn a_successful_zero_page_pass_cannot_publish_a_stale_staged_pdf() {
    let f = Fixture::new(
        "zero-page-stale-stage",
        r#"
echo stable > "$aux/$job.aux"
if grep -q MAKE_PDF main.tex; then
  printf '%%PDF-1.4 /Type /Page current' > "$out/$job.pdf"
else
  echo 'Output written on stale.pdf (1 page, 1 byte).'
fi
"#,
    );
    f.write("main.tex", "MAKE_PDF");
    f.run();
    let published = std::fs::read(f.0.join("main.pdf")).unwrap();

    f.write("main.tex", "NO_PAGES");
    let output = f.output(&[]);
    assert!(
        !output.status.success(),
        "a no-output pass reported success"
    );
    assert_eq!(std::fs::read(f.0.join("main.pdf")).unwrap(), published);
}

#[test]
fn a_failed_rebuild_does_not_adopt_a_user_modified_pdf() {
    let f = Fixture::new(
        "failed-build-pdf-ownership",
        r#"
echo stable > "$aux/$job.aux"
if [ -f fail-build ]; then exit 1; fi
printf '%%PDF-1.4 /Type /Page generated' > "$out/$job.pdf"
"#,
    );
    f.run();
    f.write("main.pdf", "user replacement after successful build");
    f.write("fail-build", "yes");
    assert!(!f.output(&[]).status.success());
    assert!(f.output(&["-C"]).status.success());
    assert_eq!(
        std::fs::read_to_string(f.0.join("main.pdf")).unwrap(),
        "user replacement after successful build"
    );
}

#[test]
fn real_driver_finds_source_local_bibliography_while_state_is_hidden() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("texmk-real-bib-{}-{nonce}", std::process::id()));
    let source = root.join("docs");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("main.tex"),
        r"\documentclass{article}
\begin{document}
A citation~\cite{sample}.
\bibliographystyle{plain}
\bibliography{refs}
\end{document}",
    )
    .unwrap();
    std::fs::write(
        source.join("refs.bib"),
        "@book{sample, author={Ada Lovelace}, title={Notes}, year={1843}}\n",
    )
    .unwrap();
    let tool_dir = std::path::Path::new(env!("CARGO_BIN_EXE_pdflatex"))
        .parent()
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_texmk"))
        .arg("docs/main.tex")
        .current_dir(&root)
        .env("TEXMK_LIB", tool_dir)
        .env("TEX_RS_CACHE_DIR", root.join("cache"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(source.join("main.pdf").is_file());
    assert!(!source.join("main.aux").exists());
    assert!(!source.join("main.bbl").exists());
    assert!(!source.join("main.blg").exists());
    assert!(!source.join("main.log").exists());
    assert!(contains_file(&root.join("cache"), "main.bbl"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn latexdiff_can_be_invoked_via_ratex_and_standalone() {
    let old_tex = "\\begin{document}\nFirst version.\n\\end{document}";
    let new_tex = "\\begin{document}\nSecond version.\n\\end{document}";

    let root = std::env::temp_dir().join(format!("latexdiff-driver-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let old_file = root.join("old.tex");
    let new_file = root.join("new.tex");
    std::fs::write(&old_file, old_tex).unwrap();
    std::fs::write(&new_file, new_tex).unwrap();

    // Standalone latexdiff
    let output = Command::new(env!("CARGO_BIN_EXE_latexdiff"))
        .arg(&old_file)
        .arg(&new_file)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\\DIFdel") && stdout.contains("\\DIFadd"));

    // ratex latexdiff ...
    let output2 = Command::new(env!("CARGO_BIN_EXE_ratex"))
        .arg("latexdiff")
        .arg(&old_file)
        .arg(&new_file)
        .output()
        .unwrap();
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout2.contains("\\DIFdel") && stdout2.contains("\\DIFadd"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_automatic_optimal_pass_detection_runs_second_pass_for_cross_references() {
    let f = Fixture::new(
        "optimal-pass-detection-refs",
        r#"printf '\\relax\n\\newlabel{sec:intro}{{1}{1}}\n' > "$aux/$job.aux"
printf '%%PDF-1.4 /Type /Pages /Count 1 /Type /Page ' > "$out/$job.pdf"
"#,
    );
    let out = f.output(&["-V"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("2 Ratex pass(es)") || stderr.contains("2 pdflatex pass(es)"),
        "Expected 2 passes for cross-reference document, got: {stderr}"
    );
}
