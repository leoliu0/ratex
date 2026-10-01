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
