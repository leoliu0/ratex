#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PROJECT: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for _ in 0..32 {
            let sequence = NEXT_PROJECT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ratex-synctex-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create {}: {error}", path.display()),
            }
        }
        panic!("could not reserve an isolated SyncTeX test directory");
    }

    fn texmk(&self) -> Output {
        let tool_dir = Path::new(env!("CARGO_BIN_EXE_pdflatex")).parent().unwrap();
        Command::new(env!("CARGO_BIN_EXE_texmk"))
            .arg("-output-directory")
            .arg("published output")
            .arg("main.tex")
            .current_dir(&self.0)
            .env("TEXMK_LIB", tool_dir)
            .env("TEX_RS_CACHE_DIR", self.0.join("cache"))
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .output()
            .unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
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

#[derive(Debug, PartialEq)]
struct Position {
    page: u32,
    x: f64,
    y: f64,
}

fn result_value<'a>(output: &'a str, key: &str) -> &'a str {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .unwrap_or_else(|| panic!("SyncTeX output omitted {key:?}:\n{output}"))
}

fn forward(source: &Path, pdf: &Path, line: u32) -> Option<Position> {
    let output = match Command::new("synctex")
        .arg("view")
        .arg("-i")
        .arg(format!("{line}:1:{}", source.display()))
        .arg("-o")
        .arg(pdf)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => panic!("cannot run synctex view: {error}"),
    };
    assert_success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    Some(Position {
        page: result_value(&text, "Page:").parse().unwrap(),
        x: result_value(&text, "x:").parse().unwrap(),
        y: result_value(&text, "y:").parse().unwrap(),
    })
}

fn assert_inverse(pdf: &Path, page: u32, x: f64, y: f64, source: &Path, line: u32) {
    let output = Command::new("synctex")
        .arg("edit")
        .arg("-o")
        .arg(format!("{page}:{x}:{y}:{}", pdf.display()))
        .output()
        .unwrap();
    assert_success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        result_value(&text, "Input:"),
        source.to_string_lossy().as_ref()
    );
    assert_eq!(result_value(&text, "Line:").parse::<u32>().unwrap(), line);
}

fn assert_serialized_pages(sidecar: &Path, source: &Path) {
    let output = Command::new("gzip")
        .arg("-cd")
        .arg(sidecar)
        .output()
        .expect("neither synctex nor gzip is available to validate the generated sidecar");
    assert_success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    let source_name = source.to_string_lossy();
    assert!(
        text.lines()
            .any(|record| record.starts_with("Input:") && record.ends_with(source_name.as_ref())),
        "generated sidecar omitted source path {}:\n{text}",
        source.display()
    );
    let page_one = text
        .split_once("{1\n")
        .and_then(|(_, rest)| rest.split_once("}1\n"))
        .map(|(page, _)| page)
        .expect("generated sidecar omitted page 1");
    let page_two = text
        .split_once("{2\n")
        .and_then(|(_, rest)| rest.split_once("}2\n"))
        .map(|(page, _)| page)
        .expect("generated sidecar omitted page 2");
    assert!(page_one.lines().any(|record| record.contains(",3:")));
    assert!(page_one.lines().any(|record| record.contains(",4:")));
    assert!(page_two.lines().any(|record| record.contains(",6:")));
}

fn assert_queries(source: &Path, pdf: &Path, sidecar: &Path) {
    let Some(first) = forward(source, pdf, 3) else {
        assert_serialized_pages(sidecar, source);
        return;
    };
    let second = forward(source, pdf, 4).expect("synctex disappeared between queries");
    let third = forward(source, pdf, 6).expect("synctex disappeared between queries");
    assert_eq!(first.page, 1, "line 3 mapped to {first:?}");
    assert_eq!(second.page, 1, "line 4 mapped to {second:?}");
    assert_eq!(third.page, 2, "line 6 mapped to {third:?}");
    assert!(
        first.x > 0.0 && first.y > 0.0,
        "invalid first position: {first:?}"
    );
    assert!(
        second.y > first.y,
        "source lines mapped to the same PDF row: {first:?} {second:?}"
    );
    assert!(
        third.x > 0.0 && third.y > 0.0,
        "invalid third position: {third:?}"
    );
    assert_inverse(pdf, first.page, first.x, first.y, source, 3);
    assert_inverse(pdf, second.page, second.x, second.y, source, 4);
    assert_inverse(pdf, third.page, third.x, third.y, source, 6);
}

#[test]
fn texmk_publishes_queryable_synctex_with_output_directory_and_cache_hits() {
    let project = Project::new();
    std::fs::write(
        project.0.join("main.tex"),
        "\\documentclass{article}\n\\begin{document}\nFirst page marker.\\par\nSecond line marker.\n\\newpage\nThird page marker.\n\\end{document}\n",
    )
    .unwrap();

    assert_success(&project.texmk());
    let source = std::fs::canonicalize(project.0.join("main.tex")).unwrap();
    let output_dir = project.0.join("published output");
    let pdf = output_dir.join("main.pdf");
    let sidecar = output_dir.join("main.synctex.gz");
    assert!(pdf.is_file());
    assert!(sidecar.is_file(), "SyncTeX was left in the private cache");
    assert_queries(&source, &pdf, &sidecar);

    let private_log = find_file(&project.0.join("cache/texmk/jobs"), "main.log")
        .expect("texmk did not retain its private transcript");
    std::fs::write(&private_log, "cache-hit sentinel\n").unwrap();
    std::fs::remove_file(&sidecar).unwrap();

    assert_success(&project.texmk());
    assert_eq!(
        std::fs::read_to_string(private_log).unwrap(),
        "cache-hit sentinel\n",
        "the second build did not use the engine dependency cache"
    );
    assert!(sidecar.is_file(), "cache hit did not republish SyncTeX");
    assert_queries(&source, &pdf, &sidecar);
}

#[test]
fn texmk_does_not_publish_synctex_from_a_failed_build() {
    let project = Project::new();
    std::fs::write(
        project.0.join("main.tex"),
        "\\documentclass{article}\n\\begin{document}\nPartial output before \\undefinedSyncTeXCommand.\n\\end{document}\n",
    )
    .unwrap();

    let output = project.texmk();
    assert!(
        !output.status.success(),
        "an undefined command unexpectedly compiled successfully"
    );
    let output_dir = project.0.join("published output");
    assert!(!output_dir.join("main.pdf").exists());
    assert!(
        !output_dir.join("main.synctex.gz").exists(),
        "a failed private build leaked its SyncTeX sidecar"
    );
}
