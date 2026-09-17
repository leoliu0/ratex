use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_JOB: AtomicU64 = AtomicU64::new(0);

struct Job(PathBuf);
impl Job {
    fn new() -> Self {
        let serial = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tex-cache-regression-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn compile(&self) -> Output {
        self.compile_with(&[])
    }
    fn compile_with(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .args(args)
            .arg("main.tex")
            .current_dir(&self.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", self.0.join("cache"))
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

fn any_file_with_extension(root: &std::path::Path, extension: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            any_file_with_extension(&path, extension)
        } else {
            path.extension().and_then(|value| value.to_str()) == Some(extension)
        }
    })
}

fn files_with_extension(root: &std::path::Path, extension: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_with_extension(&path, extension, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            files.push(path);
        }
    }
}

fn one_depcache(root: &std::path::Path) -> PathBuf {
    let mut files = Vec::new();
    files_with_extension(root, "depcache", &mut files);
    assert_eq!(files.len(), 1, "expected one dependency cache: {files:?}");
    files.pop().unwrap()
}

fn write_one_pixel_png(path: &std::path::Path) {
    std::fs::write(
        path,
        [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00,
            0x00, 0xb5, 0x1c, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78,
            0xda, 0x63, 0x64, 0xf8, 0x0f, 0x00, 0x01, 0x05, 0x01, 0x01, 0x27, 0x18, 0xe3, 0x66,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ],
    )
    .unwrap();
}

impl Drop for Job {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn version_seven_records_are_complete_and_authenticated() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\\nofiles\\begin{document}Stamp.\\end{document}\n",
    )
    .unwrap();
    job.successful_compile();

    let record = std::fs::read_to_string(one_depcache(&job.0.join("cache"))).unwrap();
    assert!(record.starts_with("TEX-DEPCACHE-7 "));
    assert!(
        record
            .lines()
            .last()
            .is_some_and(|line| line.starts_with("END\t")),
        "v7 records require a final integrity marker"
    );
    let stamped: Vec<_> = record
        .lines()
        .filter(|line| {
            ["PDF\t", "FILE\t", "READ\t", "AUX\t"]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .collect();
    assert!(stamped.iter().any(|line| line.starts_with("PDF\t")));
    assert!(stamped.iter().any(|line| line.starts_with("READ\t")));
    assert!(stamped.iter().any(|line| line.starts_with("AUX\t")));
    for line in stamped {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(
            fields.len(),
            10,
            "v7 stamped record must contain kind, path, seven stamp fields, and hash: {line}"
        );
        for field in &fields[2..] {
            field
                .parse::<i128>()
                .unwrap_or_else(|_| panic!("non-numeric v6 stamp field in {line}"));
        }
    }
}

#[test]
fn truncated_record_cannot_drop_dependency_validation() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\\nofiles\\begin{document}Old.\\end{document}\n",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    let old_pdf = std::fs::read(job.0.join("main.pdf")).unwrap();

    let cache = one_depcache(&job.0.join("cache"));
    let record = std::fs::read_to_string(&cache).unwrap();
    let truncated = record.lines().take(4).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(&cache, truncated).unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\\nofiles\\begin{document}New.\\end{document}\n",
    )
    .unwrap();
    std::fs::write(job.0.join("main.log"), "truncated-record sentinel").unwrap();

    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "truncated-record sentinel",
        "a record without its authenticated END marker was accepted"
    );
    assert_ne!(std::fs::read(job.0.join("main.pdf")).unwrap(), old_pdf);
}

#[cfg(unix)]
#[test]
fn main_symlink_spelling_is_part_of_the_cache_identity() {
    use std::os::unix::fs::symlink;

    let job = Job::new();
    for directory in ["real", "a", "b"] {
        std::fs::create_dir(job.0.join(directory)).unwrap();
    }
    std::fs::write(
        job.0.join("real/main.tex"),
        "\\documentclass{article}\\nofiles\\begin{document}\\input{child.tex}\\end{document}\n",
    )
    .unwrap();
    std::fs::write(job.0.join("a/child.tex"), "AAAA").unwrap();
    std::fs::write(job.0.join("b/child.tex"), "BBBB").unwrap();
    symlink("../real/main.tex", job.0.join("a/main.tex")).unwrap();
    symlink("../real/main.tex", job.0.join("b/main.tex")).unwrap();
    let compile = |main: &str| {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg(main)
            .current_dir(&job.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .output()
            .unwrap()
    };

    for _ in 0..2 {
        let output = compile("a/main.tex");
        assert!(output.status.success());
    }
    let a_pdf = std::fs::read(job.0.join("main.pdf")).unwrap();
    std::fs::write(job.0.join("main.log"), "symlink cache sentinel").unwrap();
    let output = compile("b/main.tex");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "symlink cache sentinel",
        "the second main-directory spelling incorrectly reused the first cache"
    );
    assert_ne!(std::fs::read(job.0.join("main.pdf")).unwrap(), a_pdf);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn cache_paths_round_trip_control_and_non_utf8_bytes() {
    use std::os::unix::ffi::OsStringExt;

    let job = Job::new();
    let unusual = std::ffi::OsString::from_vec(b"cwd-%-tab\t-line\n-\xff".to_vec());
    let cwd = job.0.join(unusual);
    std::fs::create_dir(&cwd).unwrap();
    std::fs::write(
        cwd.join("main.tex"),
        "\\documentclass{article}\\nofiles\\begin{document}Paths.\\end{document}\n",
    )
    .unwrap();
    let compile = || {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&cwd)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .output()
            .unwrap()
    };

    let first = compile();
    assert!(first.status.success());
    let record = std::fs::read_to_string(one_depcache(&job.0.join("cache"))).unwrap();
    for escaped in ["%25", "%09", "%0A", "%FF"] {
        assert!(record.contains(escaped), "record omitted escape {escaped}");
    }
    std::fs::write(cwd.join("main.log"), "encoded-path sentinel").unwrap();
    let second = compile();
    assert!(second.status.success());
    assert_eq!(
        std::fs::read_to_string(cwd.join("main.log")).unwrap(),
        "encoded-path sentinel",
        "an encoded record path did not round-trip on cache lookup"
    );
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
    assert!(!job.0.join("main.depcache").exists());
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

#[test]
fn moved_valid_cache_record_cannot_validate_another_job() {
    let first = Job::new();
    let second = Job::new();
    std::fs::write(
        first.0.join("main.tex"),
        "\\documentclass{article}\\begin{document}First.\\end{document}\n",
    )
    .unwrap();
    std::fs::write(
        second.0.join("main.tex"),
        "\\documentclass{article}\\begin{document}Second.\\end{document}\n",
    )
    .unwrap();
    first.successful_compile();
    first.successful_compile();
    second.successful_compile();
    second.successful_compile();

    let first_cache = one_depcache(&first.0.join("cache"));
    let second_cache = one_depcache(&second.0.join("cache"));
    std::fs::copy(first_cache, second_cache).unwrap();
    std::fs::write(second.0.join("main.log"), "swapped-cache sentinel").unwrap();
    second.successful_compile();
    assert_ne!(
        std::fs::read_to_string(second.0.join("main.log")).unwrap(),
        "swapped-cache sentinel",
        "a cache record for another canonical source/PDF was accepted"
    );
}

#[test]
fn missing_transcript_forces_a_rebuild() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\\begin{document}Transcript.\\end{document}\n",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    std::fs::remove_file(job.0.join("main.log")).unwrap();
    job.successful_compile();
    assert!(job.0.join("main.log").is_file());
}

#[test]
fn jobname_rejects_path_components_before_writing_artifacts() {
    let job = Job::new();
    std::fs::write(job.0.join("main.tex"), "\\relax\\end\n").unwrap();
    let escaped_stem = format!("{}-escaped", job.0.file_name().unwrap().to_string_lossy());
    let unsafe_names = [
        format!("../{escaped_stem}"),
        "sub/job".to_string(),
        r"sub\job".to_string(),
        ".".to_string(),
        "..".to_string(),
    ];
    for unsafe_name in &unsafe_names {
        let output = job.compile_with(&["-jobname", unsafe_name]);
        assert!(
            !output.status.success(),
            "accepted unsafe jobname {unsafe_name:?}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("one filename component"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!job
        .0
        .parent()
        .unwrap()
        .join(format!("{escaped_stem}.pdf"))
        .exists());
    assert!(!job.0.join("sub").exists());
}

#[test]
fn validated_cache_hit_marker_is_confined_to_the_requested_cache() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\\begin{document}Cached.\\end{document}\n",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();

    let cache = job.0.join("cache");
    let marker = cache.join("hit-marker");
    let hit = Command::new(env!("CARGO_BIN_EXE_pdflatex"))
        .arg("main.tex")
        .current_dir(&job.0)
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .env("TEX_RS_CACHE_DIR", &cache)
        .env("TEX_RS_CACHE_HIT_MARKER", &marker)
        .output()
        .unwrap();
    assert!(hit.status.success());
    assert_eq!(std::fs::metadata(&marker).unwrap().len(), 0);
    std::fs::remove_file(&marker).unwrap();

    let outside = job.0.join("outside-marker");
    let rejected = Command::new(env!("CARGO_BIN_EXE_pdflatex"))
        .arg("main.tex")
        .current_dir(&job.0)
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .env("TEX_RS_CACHE_DIR", &cache)
        .env("TEX_RS_CACHE_HIT_MARKER", &outside)
        .output()
        .unwrap();
    assert!(rejected.status.success());
    assert!(!outside.exists());
}

#[test]
fn directory_at_an_extensionless_probe_does_not_block_cache_hits() {
    let job = Job::new();
    std::fs::create_dir(job.0.join("tables")).unwrap();
    std::fs::write(
        job.0.join("tables.tex"),
        "Resolved through the .tex fallback.",
    )
    .unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\\begin{document}\\input{tables}\\end{document}\n",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    std::fs::write(job.0.join("main.log"), "directory-probe cache sentinel").unwrap();
    job.successful_compile();
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "directory-probe cache sentinel"
    );
}

#[test]
fn replacing_terminal_probe_directory_with_casefold_file_invalidates() {
    let job = Job::new();
    let lower = job.0.join("lower");
    std::fs::create_dir(&lower).unwrap();
    std::fs::create_dir(job.0.join("shadow.tex")).unwrap();
    std::fs::write(lower.join("shadow.tex"), "Lower priority text.").unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\\nofiles\\begin{document}\\input{shadow}\\end{document}\n",
    )
    .unwrap();
    let compile = || {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&job.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .env("TEXINPUTS", &lower)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let output = compile();
        assert!(output.status.success());
    }
    let lower_pdf = std::fs::read(job.0.join("main.pdf")).unwrap();

    std::fs::remove_dir(job.0.join("shadow.tex")).unwrap();
    std::fs::write(job.0.join("SHADOW.TEX"), "Local override text.").unwrap();
    std::fs::write(job.0.join("main.log"), "casefold directory sentinel").unwrap();
    let output = compile();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "casefold directory sentinel",
        "the parent snapshot did not observe the casefold override"
    );
    assert_ne!(std::fs::read(job.0.join("main.pdf")).unwrap(), lower_pdf);
}

#[test]
fn page_count_aux_creation_requires_a_real_first_repeat() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        concat!(
            "\\documentclass{article}\n",
            "\\makeatletter\n",
            "\\@ifundefined{@abspage@last}",
            "{\\gdef\\@abspage@last{1073741823}}{}\n",
            "\\AtBeginDocument{\\typeout{OBSERVED-LAST=\\@abspage@last}}\n",
            "\\makeatother\n",
            "\\begin{document}Stable.\\end{document}\n"
        ),
    )
    .unwrap();

    job.successful_compile();
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "a newly written page-count definition can affect the next pass"
    );
    job.successful_compile();
    let log = std::fs::read_to_string(job.0.join("main.log")).unwrap();
    assert!(
        log.contains("OBSERVED-LAST=1"),
        "the first repeat reused output produced before main.aux existed:\n{log}"
    );
    assert!(!log.contains("OBSERVED-LAST=1073741823"));
    assert!(
        any_file_with_extension(&job.0.join("cache"), "depcache"),
        "the stable second pass should seed the cache"
    );
    std::fs::write(job.0.join("main.log"), "third-pass cache sentinel").unwrap();
    job.successful_compile();
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "third-pass cache sentinel",
        "the third pass should reuse the validated stable-state cache"
    );
}

#[test]
fn page_count_aux_creation_requires_a_real_repeat_with_an_aux_directory() {
    let job = Job::new();
    std::fs::create_dir(job.0.join("state")).unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        concat!(
            "\\documentclass{article}\n",
            "\\makeatletter\n",
            "\\@ifundefined{@abspage@last}",
            "{\\gdef\\@abspage@last{1073741823}}{}\n",
            "\\AtBeginDocument{\\typeout{OBSERVED-LAST=\\@abspage@last}}\n",
            "\\makeatother\n",
            "\\begin{document}Stable.\\end{document}\n"
        ),
    )
    .unwrap();
    let compile = || job.compile_with(&["-aux-directory", "state"]);

    let first = compile();
    assert!(
        first.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(!any_file_with_extension(&job.0.join("cache"), "depcache"));

    let repeat = compile();
    assert!(repeat.status.success());
    let log = std::fs::read_to_string(job.0.join("state/main.log")).unwrap();
    assert!(
        log.contains("OBSERVED-LAST=1"),
        "the aux-directory repeat did not read the new page count:\n{log}"
    );
    assert!(!log.contains("OBSERVED-LAST=1073741823"));
    assert!(any_file_with_extension(&job.0.join("cache"), "depcache"));
    std::fs::write(job.0.join("state/main.log"), "aux-dir third-pass sentinel").unwrap();
    assert!(compile().status.success());
    assert_eq!(
        std::fs::read_to_string(job.0.join("state/main.log")).unwrap(),
        "aux-dir third-pass sentinel"
    );
}

#[test]
fn explicit_main_aux_probe_prevents_first_pass_cache_seeding() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        concat!(
            "\\documentclass{article}\n",
            "\\IfFileExists{main.aux}{\\errmessage{main aux became visible}}{}\n",
            "\\begin{document}Stable.\\end{document}\n"
        ),
    )
    .unwrap();

    job.successful_compile();
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "an explicit missing main.aux dependency must not be normalized away"
    );
    assert!(
        !job.compile().status.success(),
        "the next pass must observe the main.aux created by the first pass"
    );
}

#[test]
fn changed_aux_state_is_never_cached_before_cross_references_stabilize() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\begin{document}\ref{answer}\label{answer}\end{document}",
    )
    .unwrap();

    job.successful_compile();
    let mut caches = Vec::new();
    files_with_extension(&job.0.join("cache"), "depcache", &mut caches);
    assert!(
        caches.is_empty(),
        "a pass that creates main.aux must not seed a fast-hit cache"
    );

    job.successful_compile();
    caches.clear();
    files_with_extension(&job.0.join("cache"), "depcache", &mut caches);
    assert_eq!(
        caches.len(),
        1,
        "the resolving pass should become cacheable once aux state is stable"
    );

    std::fs::write(job.0.join("main.log"), "stable-pass sentinel").unwrap();
    job.successful_compile();
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "stable-pass sentinel",
        "the third invocation should reuse the stable second pass"
    );
}

#[test]
fn auxiliary_directory_separates_state_from_the_pdf_and_supports_nested_outputs() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\newwrite\customstate
\begin{document}
\immediate\openout\customstate=nested/custom.state
\immediate\write\customstate{cached state}
Hello.
\end{document}",
    )
    .unwrap();
    let args = [
        "-output-directory",
        "pdf",
        "-aux-directory",
        "state",
        "--cache-directory",
        "private cache",
    ];
    for _ in 0..2 {
        let out = job.compile_with(&args);
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(job.0.join("pdf/main.pdf").is_file());
    assert!(!job.0.join("pdf/main.aux").exists());
    assert!(!job.0.join("pdf/main.log").exists());
    assert!(job.0.join("state/main.aux").is_file());
    assert!(job.0.join("state/main.log").is_file());
    assert_eq!(
        std::fs::read_to_string(job.0.join("state/nested/custom.state"))
            .unwrap()
            .trim(),
        "cached state"
    );
    assert!(!job.0.join("main.aux").exists());
    assert!(!job.0.join("main.log").exists());
    assert!(!any_file_with_extension(&job.0.join("state"), "depcache"));

    std::fs::write(job.0.join("state/main.log"), "cache sentinel").unwrap();
    let out = job.compile_with(&args);
    assert!(out.status.success());
    assert_eq!(
        std::fs::read_to_string(job.0.join("state/main.log")).unwrap(),
        "cache sentinel"
    );
}

#[test]
fn same_size_pdf_edit_invalidates_the_private_cache() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\n\\begin{document}Hello.\\end{document}\n",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    let pdf = job.0.join("main.pdf");
    let modified = std::fs::metadata(&pdf).unwrap().modified().unwrap();
    let mut bytes = std::fs::read(&pdf).unwrap();
    bytes[0] ^= 1;
    std::fs::write(&pdf, bytes).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&pdf)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    std::fs::write(job.0.join("main.log"), "cache sentinel").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "cache sentinel"
    );
}

#[test]
fn plain_mode_never_writes_a_dependency_cache() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        "\\font\\tenrm=cmr10 \\tenrm Hello.\\par\\end\n",
    )
    .unwrap();
    let out = job.compile_with(&["-plain", "--cache-directory", "private cache"]);
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!any_file_with_extension(
        &job.0.join("private cache"),
        "depcache"
    ));
}

#[test]
fn pdf_optimization_modes_have_independent_cache_entries() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\n\\begin{document}Hello.\\end{document}\n",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    std::fs::write(job.0.join("main.log"), "speed sentinel").unwrap();

    let size = job.compile_with(&["--optimize-pdf-size"]);
    assert!(size.status.success());
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "speed sentinel"
    );
    std::fs::write(job.0.join("main.log"), "size sentinel").unwrap();
    let size_hit = job.compile_with(&["--optimize-pdf-size"]);
    assert!(size_hit.status.success());
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "size sentinel"
    );
}

#[test]
fn moved_cache_record_cannot_cross_pdf_optimization_modes() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        "\\documentclass{article}\n\\begin{document}Hello.\\end{document}\n",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    let speed_cache = one_depcache(&job.0.join("cache"));

    let size = job.compile_with(&["--optimize-pdf-size"]);
    assert!(size.status.success());
    let mut caches = Vec::new();
    files_with_extension(&job.0.join("cache"), "depcache", &mut caches);
    assert_eq!(caches.len(), 2, "expected one cache per optimization mode");
    let size_cache = caches
        .into_iter()
        .find(|path| path != &speed_cache)
        .expect("size cache");

    // The size-mode record describes the currently visible PDF, so source/PDF
    // binding alone cannot detect that it was moved onto the speed-mode key.
    std::fs::copy(size_cache, &speed_cache).unwrap();
    std::fs::write(job.0.join("main.log"), "cross-mode cache sentinel").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "cross-mode cache sentinel",
        "a dependency-cache record was accepted under another invocation key"
    );
}

#[test]
fn same_size_same_mtime_text_edit_invalidates_by_content() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\begin{document}\input{extra.tex}\end{document}",
    )
    .unwrap();
    let extra = job.0.join("extra.tex");
    std::fs::write(&extra, "AAAA stable text.").unwrap();
    job.successful_compile();
    job.successful_compile();
    let modified = std::fs::metadata(&extra).unwrap().modified().unwrap();
    std::fs::write(&extra, "BBBB stable text.").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&extra)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    std::fs::write(job.0.join("main.log"), "cache sentinel").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "cache sentinel"
    );
}

#[test]
fn optional_file_appearance_invalidates_a_negative_probe() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\begin{document}\IfFileExists{optional.flag}{found}{missing}\end{document}",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    std::fs::write(job.0.join("main.log"), "cache sentinel").unwrap();
    std::fs::write(job.0.join("optional.flag"), "now present").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "cache sentinel"
    );
}

#[cfg(unix)]
#[test]
fn generated_pdf_does_not_hide_a_casefolded_missing_file_dependency() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        concat!(
            "\\documentclass{article}\\nofiles\\begin{document}",
            "\\IfFileExists{MAIN.PDF}{\\errmessage{generated PDF became visible}}{}",
            "Stable.\\end{document}\n"
        ),
    )
    .unwrap();

    let first = job.compile_with(&["-halt-on-error"]);
    assert!(
        first.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(job.0.join("main.pdf").is_file());

    let second = job.compile_with(&["-halt-on-error"]);
    assert!(
        !second.status.success(),
        "the generated lowercase PDF was hidden from the casefolded missing-file probe"
    );
}

#[test]
fn newly_appearing_texinputs_class_shadows_a_cached_lower_priority_class() {
    let job = Job::new();
    let overrides = job.0.join("texinputs");
    std::fs::create_dir(&overrides).unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\nofiles\begin{document}Cached class.\end{document}",
    )
    .unwrap();

    let compile = || {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&job.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .env("TEXINPUTS", &overrides)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let output = compile();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::write(job.0.join("main.log"), "texinputs cache sentinel").unwrap();
    let cached = compile();
    assert!(cached.status.success());
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "texinputs cache sentinel",
        "the stable lower-priority class lookup should be cacheable"
    );

    std::fs::write(
        overrides.join("article.cls"),
        r"\errmessage{new TEXINPUTS class selected}\endinput",
    )
    .unwrap();
    let rebuilt = compile();
    assert!(
        !rebuilt.status.success(),
        "a newly created higher-priority class was hidden by a stale cache hit"
    );
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "texinputs cache sentinel",
        "the engine did not reopen the newly shadowing class"
    );
}

#[test]
fn newly_appearing_texmfhome_tree_invalidates_a_missing_root_probe() {
    let job = Job::new();
    let texmf_home = job.0.join("future-texmf-home");
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\nofiles\begin{document}Cached class.\end{document}",
    )
    .unwrap();

    let compile = || {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&job.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .env("TEXMFHOME", &texmf_home)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let output = compile();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::write(job.0.join("main.log"), "texmf cache sentinel").unwrap();
    let cached = compile();
    assert!(cached.status.success());
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "texmf cache sentinel",
        "the absent higher-priority TEXMFHOME should be cacheable"
    );

    let class = texmf_home.join("tex/latex/override/article.cls");
    std::fs::create_dir_all(class.parent().unwrap()).unwrap();
    std::fs::write(class, r"\errmessage{new TEXMFHOME class selected}\endinput").unwrap();
    let rebuilt = compile();
    assert!(
        !rebuilt.status.success(),
        "creating a formerly absent search root did not invalidate the cache"
    );
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "texmf cache sentinel"
    );
}

#[test]
fn newly_appearing_file_inside_unindexed_texmfhome_invalidates_directory_snapshot() {
    let job = Job::new();
    let texmf_home = job.0.join("texmf-home");
    let package_dir = texmf_home.join("tex/latex/local");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\nofiles\begin{document}Cached class.\end{document}",
    )
    .unwrap();

    let compile = || {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&job.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .env("TEXMFHOME", &texmf_home)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let output = compile();
        assert!(output.status.success());
    }
    std::fs::write(job.0.join("main.log"), "directory snapshot sentinel").unwrap();
    let cached = compile();
    assert!(cached.status.success());
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "directory snapshot sentinel"
    );

    std::fs::write(
        package_dir.join("article.cls"),
        r"\errmessage{new unindexed class selected}\endinput",
    )
    .unwrap();
    let rebuilt = compile();
    assert!(
        !rebuilt.status.success(),
        "a new file inside an unindexed tree was hidden by the cache"
    );
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "directory snapshot sentinel"
    );
}

#[test]
fn file_created_inside_unindexed_texmfhome_during_pass_cannot_seed_cache() {
    let job = Job::new();
    let texmf_home = job.0.join("texmf-home");
    let package_dir = texmf_home.join("tex/latex/local");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\newwrite\created
\immediate\openout\created=texmf-home/tex/latex/local/article.cls
\immediate\write\created{\string\errmessage{same-pass unindexed class selected}\string\endinput}
\immediate\closeout\created
\begin{document}Created after class lookup.\end{document}",
    )
    .unwrap();

    let compile = || {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&job.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .env("TEXMFHOME", &texmf_home)
            .output()
            .unwrap()
    };
    let first = compile();
    assert!(
        first.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "a directory snapshot taken after the write seeded a stale cache"
    );
    let second = compile();
    assert!(
        !second.status.success(),
        "the class created by the preceding pass was not selected"
    );
}

#[cfg(unix)]
#[test]
fn differently_cased_local_class_invalidates_casefold_directory_snapshot() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\nofiles\begin{document}Cached class.\end{document}",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    std::fs::write(job.0.join("main.log"), "casefold sentinel").unwrap();
    std::fs::write(
        job.0.join("ARTICLE.CLS"),
        r"\errmessage{uppercase local class selected}\endinput",
    )
    .unwrap();

    let rebuilt = job.compile();
    assert!(!rebuilt.status.success());
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "casefold sentinel"
    );
}

#[test]
fn index_rewritten_during_pass_cannot_seed_cache() {
    let job = Job::new();
    let texmf_home = job.0.join("texmf-home");
    let package_dir = texmf_home.join("tex/latex/local");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        package_dir.join("article.cls"),
        r"\errmessage{indexed override selected}\endinput",
    )
    .unwrap();
    std::fs::write(texmf_home.join("ls-R"), "./tex/latex/local:\ndummy.sty\n").unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\newwrite\indexfile
\immediate\openout\indexfile=texmf-home/ls-R
\immediate\write\indexfile{./tex/latex/local:}
\immediate\write\indexfile{article.cls}
\immediate\closeout\indexfile
\begin{document}Changed index after class lookup.\end{document}",
    )
    .unwrap();

    let compile = || {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&job.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .env("TEXMFHOME", &texmf_home)
            .output()
            .unwrap()
    };
    let first = compile();
    assert!(
        first.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "an index identity sampled after it was rewritten seeded a stale cache"
    );
    assert!(!compile().status.success());
}

#[test]
fn home_is_part_of_the_cache_identity() {
    let job = Job::new();
    let home_a = job.0.join("home-a");
    let home_b = job.0.join("home-b");
    std::fs::create_dir_all(&home_a).unwrap();
    let class = home_b.join("texmf/tex/latex/local/article.cls");
    std::fs::create_dir_all(class.parent().unwrap()).unwrap();
    std::fs::write(
        class,
        r"\errmessage{class from changed HOME selected}\endinput",
    )
    .unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\nofiles\begin{document}Home identity.\end{document}",
    )
    .unwrap();

    let compile = |home: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&job.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .env_remove("TEXMFHOME")
            .env("HOME", home)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        assert!(compile(&home_a).status.success());
    }
    std::fs::write(job.0.join("main.log"), "home cache sentinel").unwrap();
    let changed_home = compile(&home_b);
    assert!(!changed_home.status.success());
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "home cache sentinel"
    );
}

#[test]
fn file_created_after_a_missing_probe_forces_the_next_compile() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\newread\probe
\newwrite\created
\begin{document}
\openin\probe=created.flag
\ifeof\probe
  missing
  \immediate\openout\created=created.flag
  \immediate\write\created{now present}
  \immediate\closeout\created
\else found\fi
\end{document}",
    )
    .unwrap();

    job.successful_compile();
    assert!(job.0.join("created.flag").is_file());
    std::fs::write(job.0.join("main.log"), "missing-probe sentinel").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "missing-probe sentinel",
        "the file created after the first probe must invalidate that pass"
    );
}

#[test]
fn newly_appearing_local_font_map_invalidates_embedded_or_system_lookup() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\begin{document}Mapped text.\end{document}",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    std::fs::write(job.0.join("main.log"), "font-cache sentinel").unwrap();
    job.successful_compile();
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "font-cache sentinel",
        "the unchanged font lookup should be cacheable"
    );

    std::fs::write(
        job.0.join("pdftex.map"),
        "% local map now shadows prior lookup\n",
    )
    .unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "font-cache sentinel",
        "a new higher-precedence font map must invalidate the cached PDF"
    );
}

#[test]
fn input_rewritten_after_read_cannot_seed_a_false_cache_hit() {
    let job = Job::new();
    std::fs::write(job.0.join("mutable.tex"), "first pass text").unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\newwrite\mutable
\begin{document}
\input{mutable.tex}
\immediate\openout\mutable=mutable.tex
\immediate\write\mutable{second pass text}
\immediate\closeout\mutable
\end{document}",
    )
    .unwrap();
    job.successful_compile();
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "a pass that rewrites an input after reading it must not seed a cache"
    );
    std::fs::write(job.0.join("main.log"), "cache sentinel").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "cache sentinel",
        "the second run must compile from the post-write input contents"
    );
    assert!(any_file_with_extension(&job.0.join("cache"), "depcache"));
    std::fs::write(job.0.join("main.log"), "stable cache sentinel").unwrap();
    job.successful_compile();
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "stable cache sentinel",
        "unchanged bytes and their recorded stamp should become a cache hit"
    );
}

#[test]
fn openin_uses_the_same_bytes_for_tex_and_the_cache_digest() {
    let job = Job::new();
    std::fs::write(job.0.join("mutable.txt"), "first stream line\n").unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\newread\mutablein
\newwrite\mutableout
\begin{document}
\openin\mutablein=mutable.txt
\read\mutablein to \captured
\closein\mutablein
\captured
\immediate\openout\mutableout=mutable.txt
\immediate\write\mutableout{second stream lin}
\immediate\closeout\mutableout
\end{document}",
    )
    .unwrap();

    job.successful_compile();
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "a stream rewritten after it was opened must not seed a cache"
    );
    std::fs::write(job.0.join("main.log"), "openin sentinel").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "openin sentinel"
    );
}

#[test]
fn pdf_file_dependency_rewritten_after_consumption_cannot_seed_a_hit() {
    let job = Job::new();
    std::fs::write(job.0.join("mutable.obj"), "(first object)").unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\newwrite\mutable
\begin{document}
\edef\beforehash{\pdfmdfivesum file {mutable.obj}}
\edef\beforedump{\pdffiledump offset 0 length 4 {mutable.obj}}
\pdfobj file {mutable.obj}
\immediate\openout\mutable=mutable.obj
\immediate\write\mutable{(next object)}
\immediate\closeout\mutable
Object \the\pdflastobj.
\end{document}",
    )
    .unwrap();

    job.successful_compile();
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "PDF side inputs rewritten after consumption must not seed a cache"
    );
    std::fs::write(job.0.join("main.log"), "PDF dependency sentinel").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "PDF dependency sentinel"
    );
}

#[test]
fn pdffilesize_rewritten_after_observation_cannot_seed_a_hit() {
    let job = Job::new();
    std::fs::write(job.0.join("mutable.size"), "tiny").unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\newwrite\mutable
\begin{document}
\edef\observedsize{\pdffilesize{mutable.size}}
\immediate\openout\mutable=mutable.size
\immediate\write\mutable{a substantially longer replacement}
\immediate\closeout\mutable
Observed size: \observedsize.
\end{document}",
    )
    .unwrap();

    job.successful_compile();
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "a pass that rewrites a file after observing its size must not seed a cache"
    );

    std::fs::write(job.0.join("main.log"), "pdffilesize sentinel").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "pdffilesize sentinel",
        "the next run must observe the replacement file's size"
    );

    let record = std::fs::read_to_string(one_depcache(&job.0.join("cache"))).unwrap();
    assert!(
        record.lines().any(|line| line.starts_with("SIZE\t")),
        "the stable pass did not preserve its size-only dependency"
    );
    std::fs::write(job.0.join("main.log"), "stable pdffilesize sentinel").unwrap();
    job.successful_compile();
    assert_eq!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "stable pdffilesize sentinel",
        "an unchanged observed size should permit a cache hit"
    );
}

#[test]
fn texinputs_pdffilesize_stays_a_size_only_dependency() {
    let job = Job::new();
    let inputs = job.0.join("inputs");
    std::fs::create_dir(&inputs).unwrap();
    let payload = inputs.join("payload.bin");
    std::fs::write(&payload, vec![b'x'; 1024 * 1024]).unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\begin{document}Size: \pdffilesize{payload.bin}.\end{document}",
    )
    .unwrap();

    let compile = || {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("main.tex")
            .current_dir(&job.0)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .env("TEXINPUTS", &inputs)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        assert!(compile().status.success());
    }
    let record = std::fs::read_to_string(one_depcache(&job.0.join("cache"))).unwrap();
    let payload = payload.to_string_lossy();
    assert!(record
        .lines()
        .any(|line| line.starts_with("SIZE\t") && line.contains(payload.as_ref())));
    assert!(
        !record.lines().any(|line| {
            (line.starts_with("FILE\t") || line.starts_with("READ\t"))
                && line.contains(payload.as_ref())
        }),
        "metadata-only lookup caused the payload contents to be hashed"
    );
}

#[test]
fn pdffiledump_reads_only_the_requested_range_and_disables_cache() {
    use std::io::{Seek, SeekFrom, Write};

    let job = Job::new();
    let payload_size = 64 * 1024 * 1024_u64;
    let mut payload = std::fs::File::create(job.0.join("large.bin")).unwrap();
    payload.set_len(payload_size).unwrap();
    payload.seek(SeekFrom::Start(payload_size - 1)).unwrap();
    payload.write_all(&[0xa5]).unwrap();
    drop(payload);
    std::fs::write(
        job.0.join("main.tex"),
        format!(
            r"\documentclass{{article}}
\nofiles
\begin{{document}}
\typeout{{PDF-FILE-DUMP:\pdffiledump offset {} length 1 {{large.bin}}}}
Range read.
\end{{document}}",
            payload_size - 1
        ),
    )
    .unwrap();

    let output = job.compile();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("PDF-FILE-DUMP:A5"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "a byte-range read must not seed a whole-file dependency cache"
    );
}

#[test]
fn pdffiledump_rejects_an_actual_result_larger_than_token_capacity() {
    let job = Job::new();
    let max_dump_bytes = tex_core::input::MAX_TOKEN_LIST_TOKENS / 2;
    let payload = std::fs::File::create(job.0.join("oversized.bin")).unwrap();
    payload.set_len(max_dump_bytes as u64 + 1).unwrap();
    drop(payload);
    std::fs::write(
        job.0.join("main.tex"),
        format!(
            r"\documentclass{{article}}
\nofiles
\begin{{document}}
\pdffiledump offset 0 length {} {{oversized.bin}}
\end{{document}}",
            max_dump_bytes + 1
        ),
    )
    .unwrap();

    let output = job.compile();
    assert!(
        !output.status.success(),
        "oversized dump unexpectedly succeeded"
    );
    let diagnostics = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostics.contains("token list size=5000000"),
        "{diagnostics}"
    );
}

#[test]
fn pdffiledump_allows_a_huge_limit_when_only_a_tiny_tail_exists() {
    use std::io::{Seek, SeekFrom, Write};

    let job = Job::new();
    let mut payload = std::fs::File::create(job.0.join("tail.bin")).unwrap();
    payload.set_len(1024).unwrap();
    payload.seek(SeekFrom::Start(1023)).unwrap();
    payload.write_all(&[0xa5]).unwrap();
    drop(payload);
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\begin{document}
\typeout{PDF-FILE-DUMP-TAIL:\pdffiledump offset 1023 length 2147483647 {tail.bin}}
Tail read.
\end{document}",
    )
    .unwrap();

    let output = job.compile();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("PDF-FILE-DUMP-TAIL:A5"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn pdfximage_rewritten_after_dimension_read_cannot_cache_success() {
    let job = Job::new();
    write_one_pixel_png(&job.0.join("mutable.png"));
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}
\nofiles
\newwrite\mutable
\begin{document}
\pdfximage{mutable.png}
\immediate\openout\mutable=mutable.png
\immediate\write\mutable{no longer a PNG}
\immediate\closeout\mutable
Image object \the\pdflastximage.
\end{document}",
    )
    .unwrap();

    job.successful_compile();
    assert!(
        !any_file_with_extension(&job.0.join("cache"), "depcache"),
        "an image rewritten after its dimensions were consumed must not seed a cache"
    );
    std::fs::write(job.0.join("main.log"), "image sentinel").unwrap();
    let rerun = job.compile();
    assert!(!rerun.status.success(), "the rewritten image is invalid");
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "image sentinel",
        "a stale successful PDF must not mask the invalid image"
    );
}

#[test]
fn map_file_content_is_a_dependency_even_with_unchanged_metadata() {
    let job = Job::new();
    std::fs::write(job.0.join("custom.map"), "% map variant A\n").unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\pdfmapfile{+custom.map}\begin{document}Map.\end{document}",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    let map = job.0.join("custom.map");
    let modified = std::fs::metadata(&map).unwrap().modified().unwrap();
    std::fs::write(&map, "% map variant B\n").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&map)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    std::fs::write(job.0.join("main.log"), "cache sentinel").unwrap();
    job.successful_compile();
    assert_ne!(
        std::fs::read_to_string(job.0.join("main.log")).unwrap(),
        "cache sentinel"
    );
}

#[test]
fn depcache_gc_removes_only_owned_valid_stale_files() {
    let job = Job::new();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\begin{document}GC.\end{document}",
    )
    .unwrap();
    job.successful_compile();
    job.successful_compile();
    let mut caches = Vec::new();
    files_with_extension(&job.0.join("cache"), "depcache", &mut caches);
    assert_eq!(caches.len(), 1);
    let directory = caches[0].parent().unwrap();
    let stale = directory.join("stale.depcache");
    let foreign = directory.join("foreign.depcache");
    std::fs::copy(&caches[0], &stale).unwrap();
    std::fs::write(&foreign, "foreign data\n").unwrap();
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    for path in [&stale, &foreign] {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(directory.join(".gc-stamp"))
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old))
        .unwrap();
    job.successful_compile();
    assert!(!stale.exists());
    assert!(foreign.exists(), "GC must preserve unrecognized files");
}

#[test]
fn canonical_working_directory_is_part_of_the_cache_identity() {
    let job = Job::new();
    let first_cwd = job.0.join("cwd-one");
    let second_cwd = job.0.join("cwd-two");
    let output_dir = job.0.join("output");
    std::fs::create_dir_all(&first_cwd).unwrap();
    std::fs::create_dir_all(&second_cwd).unwrap();
    std::fs::write(
        job.0.join("main.tex"),
        r"\documentclass{article}\begin{document}CWD.\end{document}",
    )
    .unwrap();
    let run = |cwd: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_pdflatex"))
            .arg("-output-directory")
            .arg(&output_dir)
            .arg(job.0.join("main.tex"))
            .current_dir(cwd)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("TEX_RS_CACHE_DIR", job.0.join("cache"))
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let output = run(&first_cwd);
        assert!(output.status.success());
    }
    std::fs::write(output_dir.join("main.log"), "cache sentinel").unwrap();
    assert!(run(&first_cwd).status.success());
    assert_eq!(
        std::fs::read_to_string(output_dir.join("main.log")).unwrap(),
        "cache sentinel"
    );
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/default.fmt.zst"),
        first_cwd.join("pdflatex.fmt"),
    )
    .unwrap();
    assert!(run(&first_cwd).status.success());
    assert_ne!(
        std::fs::read_to_string(output_dir.join("main.log")).unwrap(),
        "cache sentinel",
        "a newly present cwd format override must invalidate the old key"
    );
    std::fs::write(output_dir.join("main.log"), "cache sentinel").unwrap();
    assert!(run(&second_cwd).status.success());
    assert_ne!(
        std::fs::read_to_string(output_dir.join("main.log")).unwrap(),
        "cache sentinel"
    );
}
