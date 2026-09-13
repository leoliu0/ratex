#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str, engine: &str) -> Self {
        let path = std::env::temp_dir().join(format!("texmk-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let f = Self(path);
        f.write("main.tex", "test");
        f.tool("pdflatex", engine);
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
    fn run(&self) {
        let out = Command::new(env!("CARGO_BIN_EXE_texmk"))
            .arg("main.tex")
            .current_dir(&self.0)
            .env("TEXMK_LIB", &self.0)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn changed_aux_without_warnings_requires_another_pass() {
    let f = Fixture::new(
        "stable-aux",
        r#"
n=0; if [ -f passes ]; then read -r n < passes; fi
n=$((n + 1)); echo "$n" > passes
echo 'stable auxiliary contents' > main.aux
printf '%%PDF-1.4 /Type /Page ' > main.pdf
"#,
    );
    f.run();
    assert_eq!(
        std::fs::read_to_string(f.0.join("passes")).unwrap().trim(),
        "2"
    );
}

#[test]
fn first_pass_citation_changes_refresh_preliminary_bibliography() {
    let f = Fixture::new(
        "new-citation",
        r#"
printf '\\citation{new}\n\\bibdata{refs}\n\\bibstyle{plain}\n' > main.aux
printf '%%PDF-1.4 /Type /Page ' > main.pdf
echo "LaTeX Warning: Citation new undefined"
"#,
    );
    f.write(
        "main.aux",
        "\\citation{old}\n\\bibdata{refs}\n\\bibstyle{plain}\n",
    );
    f.tool(
        "tex-bibtex",
        "cp main.aux main.bbl\necho called >> bibcalls",
    );
    f.run();
    let bbl = std::fs::read_to_string(f.0.join("main.bbl")).unwrap();
    assert!(bbl.contains("citation{new}"), "{bbl}");
    assert_eq!(
        std::fs::read_to_string(f.0.join("bibcalls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}
