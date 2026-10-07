//! `\write18` and `\pdfshellescape` follow web2c's `shellenabledp`/
//! `restrictedshell`: the transcript lines below are those of `pdftex -ini`
//! (TeX Live 2026) run with `-shell-restricted`, `-no-shell-escape` and
//! `-shell-escape` on the same input.

use tex_core::engine::{Engine, InteractionMode};
use tex_core::{set_shell_escape, ShellEscape};

const SETUP: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\^=7\n";

fn run(mode: ShellEscape, source: &str, out_dir: Option<&std::path::Path>) -> String {
    set_shell_escape(mode);
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.set_interaction_mode(InteractionMode::Nonstop);
    if let Some(dir) = out_dir {
        engine.out_dir = dir.to_string_lossy().into_owned();
    }
    engine.input.push_file("w.tex".into(), format!("{SETUP}{source}\\end\n").into_bytes());
    engine.run();
    engine.log.clone()
}

const COMMANDS: &str = "\\immediate\\write18{echo hello}\n\
    \\immediate\\write18{kpsewhich \"a b\" 'x}\n\
    \\immediate\\write18{kpsewhich -var-value=TEXRES_NOPE}\n\
    \\message{[\\the\\pdfshellescape]}\n";

#[test]
fn restricted_runs_only_listed_commands() {
    let log = run(ShellEscape::Restricted, COMMANDS, None);
    for line in [
        "runsystem(echo hello)...disabled (restricted).\n\n",
        "runsystem(kpsewhich \"a b\" 'x)...quotation error in system command.\n\n",
        "runsystem(kpsewhich -var-value=TEXRES_NOPE)...executed safely (allowed).\n\n",
        "[2]",
    ] {
        assert!(log.contains(line), "missing {line:?} in:\n{log}");
    }
}

#[test]
fn disabled_only_reports_the_command() {
    let log = run(ShellEscape::Disabled, COMMANDS, None);
    for line in [
        "runsystem(echo hello)...disabled.\n\n",
        "runsystem(kpsewhich \"a b\" 'x)...disabled.\n\n",
        "runsystem(kpsewhich -var-value=TEXRES_NOPE)...disabled.\n\n",
        "[0]",
    ] {
        assert!(log.contains(line), "missing {line:?} in:\n{log}");
    }
}

#[cfg(unix)]
#[test]
fn enabled_runs_any_command_with_the_output_directory_exported() {
    let dir = std::env::temp_dir().join(format!("texres-write18-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("env.txt");
    let source = format!(
        "\\immediate\\write18{{echo \"$TEXMF_OUTPUT_DIRECTORY\" \"$SELFAUTOLOC\" > {}}}\n\\message{{[\\the\\pdfshellescape]}}\n",
        marker.display()
    );
    let log = run(ShellEscape::Enabled, &source, Some(&dir));
    assert!(log.contains(")...executed.\n\n"), "{log}");
    assert!(log.contains("[1]"), "{log}");
    let written = std::fs::read_to_string(&marker).unwrap();
    let mut fields = written.split_whitespace();
    assert_eq!(fields.next().map(std::path::Path::new), Some(dir.as_path()), "{written}");
    assert!(fields.next().is_some_and(|loc| std::path::Path::new(loc).is_dir()), "{written}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn deferred_write18_runs_at_shipout() {
    let log = run(
        ShellEscape::Restricted,
        "\\shipout\\hbox{\\write18{echo late}}\n\\showlists\n",
        None,
    );
    assert!(log.contains("runsystem(echo late)...disabled (restricted).\n\n"), "{log}");
}
