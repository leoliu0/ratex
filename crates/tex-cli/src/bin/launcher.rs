//! Small distribution launcher used where filesystem symlinks are not a
//! dependable packaging primitive (principally Windows zip archives).

use std::path::Path;
use std::process::{Command, ExitCode};

fn invoked_name(path: &Path) -> String {
    let display = path.as_os_str().to_string_lossy();
    let file = display.rsplit(['/', '\\']).next().unwrap_or_default();
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
        .to_ascii_lowercase()
}

pub(crate) fn main() -> ExitCode {
    let current = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("tex-suite launcher: cannot locate itself: {error}");
            return ExitCode::from(127);
        }
    };
    let invoked = invoked_name(&current);
    let (mode, engine_name): (Option<&str>, Option<&str>) = match invoked.as_str() {
        "pdflatex" => (Some("engine"), Some("pdflatex")),
        "xelatex" => (Some("engine"), Some("xelatex")),
        "lualatex" => (Some("engine"), Some("lualatex")),
        "tex-bibtex" | "bibtex" => (Some("bibtex"), None),
        "latexdiff" => (Some("latexdiff"), None),
        "ratex" | "latexmk" => (None, None),
        _ => {
            eprintln!(
                "tex-suite launcher: unsupported executable name `{}`",
                if invoked.is_empty() { "?" } else { &invoked }
            );
            return ExitCode::from(2);
        }
    };

    let mut target_path = current
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("texmk");
    if !std::env::consts::EXE_EXTENSION.is_empty() {
        target_path.set_extension(std::env::consts::EXE_EXTENSION);
    }
    let mut command = Command::new(&target_path);
    command.args(std::env::args_os().skip(1));
    if let Some(mode) = mode {
        command.env("TEXMK_INTERNAL_MODE", mode);
    }
    if let Some(name) = engine_name {
        command.env("TEX_SUITE_PROGRAM_NAME", name);
    }
    match command.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(error) => {
            eprintln!(
                "{}: cannot start {}: {error}",
                if invoked.is_empty() {
                    "tex-suite"
                } else {
                    &invoked
                },
                target_path.display()
            );
            ExitCode::from(127)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_windows_executable_suffix() {
        assert_eq!(invoked_name(Path::new(r"C:\\bin\\xelatex.exe")), "xelatex");
        assert_eq!(invoked_name(Path::new("/bin/latexmk")), "latexmk");
    }
}
