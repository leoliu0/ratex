//! Byte-exact comparison against TeX Live's `bibtex` (0.99e, TeX Live
//! 2026). Each directory under `tests/oracle/` holds the `.aux` file(s) of
//! one run, an optional `args` file (default: `job`), and the outputs of the
//! real program: `expected.bbl`, `expected.blg` (without the banner and the
//! usage statistics) and `expected.status`. Inputs shared between cases
//! (styles and databases) live in `tests/oracle/shared/`.

use std::path::{Path, PathBuf};

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle")
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("expected.") {
            std::fs::copy(entry.path(), to.join(name)).unwrap();
        }
    }
}

/// The `.blg` without the banner and TeX Live's usage statistics.
fn normalized_blg(blg: &[u8]) -> Vec<u8> {
    let mut lines: Vec<&[u8]> = blg.split(|&c| c == b'\n').collect();
    assert!(lines[0].starts_with(b"This is BibTeX"));
    lines.remove(0);
    lines.join(&b'\n')
}

fn check(case: &str) {
    let src = oracle_dir().join(case);
    let tmp = std::env::temp_dir().join(format!("tex-bibtex-oracle-{}-{case}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    copy_dir(&oracle_dir().join("shared"), &tmp);
    copy_dir(&src, &tmp);
    let args: Vec<String> = match std::fs::read_to_string(src.join("args")) {
        Ok(a) => a.split_whitespace().map(str::to_string).collect(),
        Err(_) => vec!["job".to_string()],
    };
    let job = args.last().unwrap().clone();

    // resolve relative names against the case directory, as when running
    // `bibtex` from there, without changing the process working directory
    let ctx = tex_kpse::fs::ResourceContext::disk(&tmp, &[], &tmp, &tmp, false, None).unwrap();
    let status = {
        let _scope = ctx.enter();
        tex_bibtex::run(&args, "test")
    };

    let read = |name: &str| std::fs::read(tmp.join(name)).unwrap();
    let expected_status: i32 = std::fs::read_to_string(src.join("expected.status"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let bbl = read(&format!("{job}.bbl"));
    let blg = normalized_blg(&read(&format!("{job}.blg")));
    let expected_bbl = std::fs::read(src.join("expected.bbl")).unwrap();
    let expected_blg = std::fs::read(src.join("expected.blg")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&bbl),
        String::from_utf8_lossy(&expected_bbl),
        "{case}: .bbl differs"
    );
    assert_eq!(bbl, expected_bbl, "{case}: .bbl bytes differ");
    assert_eq!(
        String::from_utf8_lossy(&blg),
        String::from_utf8_lossy(&expected_blg),
        "{case}: .blg differs"
    );
    assert_eq!(status, expected_status, "{case}: exit status");
    let _ = std::fs::remove_dir_all(&tmp);
}

macro_rules! oracle_cases {
    ($($test:ident => $case:literal,)*) => {
        $(#[test] fn $test() { check($case); })*

        #[test]
        fn every_case_directory_has_a_test() {
            let mut known = vec![$($case),*];
            known.sort_unstable();
            let mut dirs: Vec<String> = std::fs::read_dir(oracle_dir())
                .unwrap()
                .map(|e| e.unwrap())
                .filter(|e| e.path().is_dir() && e.file_name() != "shared")
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            dirs.sort_unstable();
            assert_eq!(dirs, known);
        }
    };
}

oracle_cases! {
    plain_xampl => "plain-xampl",
    alpha_xampl_min_crossrefs => "alpha-xampl-min-crossrefs",
    apalike_xampl_subset => "apalike-xampl-subset",
    plainnat_names_accents => "plainnat-names-accents",
    plain_error_recovery => "plain-error-recovery",
    unsrt_crossref_order => "unsrt-crossref-order",
    abbrv_crossref_all => "abbrv-crossref-all",
    rfs_references => "rfs-references",
    builtins_probe => "builtins-probe",
    sort_ties_and_limits => "sort-ties-and-limits",
    bst_errors => "bst-errors",
    bst_eof => "bst-eof",
    bib_syntax => "bib-syntax",
    crlf_lines => "crlf-lines",
    missing_files => "missing-files",
    aux_errors => "aux-errors",
    dotted_jobname => "dotted-jobname",
}
