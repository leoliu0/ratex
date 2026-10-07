//! Helpers shared by the integration tests that execute files they create.
//! Every test binary includes this module and uses a subset of it.
#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Creates an executable file with the given contents.
///
/// Running a file that any process still holds open for writing fails with
/// ETXTBSY ("Text file busy"). When this process writes the file itself it
/// holds a write descriptor, and a sibling test thread that forks meanwhile
/// duplicates that descriptor into a child which keeps it until its `exec`
/// runs. Under load that outlasts the write, so a tool the test, or the texmk
/// it starts, launches next fails at random. A helper process is the only
/// writer here and has exited when this returns, so no descriptor can leak.
pub fn install_executable(destination: &Path, contents: &[u8]) {
    let mut writer = Command::new("/bin/sh")
        .args(["-c", "cat > \"$0\" && chmod 755 \"$0\""])
        .arg(destination)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    // A failed write is reported through the exit status checked below.
    let _ = writer.stdin.take().unwrap().write_all(contents);
    let status = writer.wait().unwrap();
    assert!(status.success(), "cannot install {}: {status}", destination.display());
}

/// Copies an existing executable, for the same reason as [`install_executable`].
pub fn copy_executable(source: &Path, destination: &Path) {
    let status = Command::new("cp")
        .arg("--")
        .arg(source)
        .arg(destination)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "cannot copy {} to {}: {status}",
        source.display(),
        destination.display()
    );
}

/// Restrict luaotfload's font names database to the bundled fonts. On the
/// first LuaLaTeX run luaotfload scans `$OSFONTDIR` and the directories of
/// the system's `fonts.conf`; over a desktop's thousands of fonts that takes
/// minutes in the debug profile and makes the test depend on the machine.
/// TeX Live's own switches do it: `$OSFONTDIR` names an empty directory and a
/// `luaotfload.conf` in `$XDG_CONFIG_HOME` keeps only the `texmf` location.
pub fn bundled_fonts_only<'a>(command: &'a mut Command, dir: &Path) -> &'a mut Command {
    let config = dir.join("xdg-config");
    std::fs::create_dir_all(config.join("luaotfload")).unwrap();
    std::fs::write(
        config.join("luaotfload/luaotfload.conf"),
        "[db]\n    location-precedence = texmf\n",
    )
    .unwrap();
    let no_fonts = dir.join("no-os-fonts");
    std::fs::create_dir_all(&no_fonts).unwrap();
    command.env("OSFONTDIR", no_fonts).env("XDG_CONFIG_HOME", config)
}
