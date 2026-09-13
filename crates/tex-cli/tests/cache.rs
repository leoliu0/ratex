use std::path::PathBuf;
use std::process::{Command, Output};

struct Job(PathBuf);
impl Job {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tex-cache-regression-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn compile(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&self.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .output()
            .unwrap()
    }
    fn successful_compile(&self) {
        let out = self.compile();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cache_hits_stable_jobs_invalidates_inputs_and_never_hides_errors() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\begin{document}\input{extra.tex}\end{document}",
    )
    .unwrap();
    std::fs::write(job.0.join("extra.tex"), "Original text.").unwrap();
    job.successful_compile();
    job.successful_compile();
    // A cache hit must avoid opening/replacing the transcript.
    std::fs::write(job.0.join("main.log"), "cache sentinel").unwrap();
    job.successful_compile();
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "cache sentinel"
    );
    std::fs::write(job.0.join("extra.tex"), "A changed input file.").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "cache sentinel"
    );
    std::fs::write(job.0.join("extra.tex"), r"Text \undefinedReviewCommand").unwrap();
    assert!(!job.compile().status.success());
    assert!(
        !job.compile().status.success(),
        "a previous failed PDF must not become a success cache hit"
    );
}
