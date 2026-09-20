//! Offline Babel loading, language switching, and aux/TOC round trips.

use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str, source: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("ratex-babel-{name}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("main.tex"), source).unwrap();
        Self(path)
    }

    fn run(&self, binary: &str) -> Output {
        let out = Command::new(binary)
            .arg("main.tex")
            .current_dir(&self.0)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.0)
            .env("TEX_RS_CACHE_DIR", self.0.join("cache"))
            .env("TEX_RS_HERMETIC", "1")
            .env("TEXMK_LIB", std::path::Path::new(binary).parent().unwrap())
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.join(name)).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn issue_9_spanish_report_survives_toc_rereading() {
    let source = include_str!("fixtures/babel/issue_9_spanish_report.tex");
    let fixture = Fixture::new("issue9", source);
    fixture.run(env!("CARGO_BIN_EXE_pdflatex"));
    let first_toc = fixture.read("main.toc");
    for expected in [r"\babel@toc {spanish}", "section A", "section B"] {
        assert!(
            first_toc.contains(expected),
            "Missing {expected}: {first_toc}"
        );
    }
    fixture.run(env!("CARGO_BIN_EXE_pdflatex"));
    assert_eq!(fixture.read("main.toc"), first_toc);

    let automatic = Fixture::new("issue9-automatic", source);
    automatic.run(env!("CARGO_BIN_EXE_texmk"));
    assert!(automatic.0.join("main.pdf").is_file());
}

#[test]
fn language_switching_restores_outer_language_and_patterns() {
    let fixture = Fixture::new(
        "switching",
        include_str!("fixtures/babel/bilingual_switching.tex"),
    );
    fixture.run(env!("CARGO_BIN_EXE_pdflatex"));
    let log = fixture.read("main.log");
    for expected in [
        "P1_LANG=spanish",
        "P2_LANG=english",
        "P3_LANG=spanish",
        "P4_LANG=english",
        "P5_LANG=spanish",
    ] {
        assert!(log.contains(expected), "Missing {expected}: {log}");
    }
}

#[test]
fn brazilian_and_english_modules_load_and_switch_offline() {
    let fixture = Fixture::new(
        "brazilian",
        include_str!("fixtures/babel/corpus_2108_13531_brazilian.tex"),
    );
    fixture.run(env!("CARGO_BIN_EXE_pdflatex"));
    let log = fixture.read("main.log");
    assert!(log.contains("CORPUS_LANG=english"), "{log}");
    assert!(log.contains("BRAZIL_LANG=brazilian"), "{log}");
    let first_toc = fixture.read("main.toc");
    assert!(first_toc.contains(r"\babel@toc {brazilian}"), "{first_toc}");
    fixture.run(env!("CARGO_BIN_EXE_pdflatex"));
    assert_eq!(fixture.read("main.toc"), first_toc);
}
