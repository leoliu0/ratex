//! End-to-end acceptance: run `rfs.bst` against `references.bib` with the
//! citations from `main.aux` and verify the generated `.bbl` byte-for-byte
//! against the oracle produced by real bibtex 0.99e.

use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn rfs_bst_reproduces_oracle_bbl() {
    let dir = fixtures();
    let aux_text = std::fs::read_to_string(dir.join("main.aux")).unwrap();
    let a = tex_bibtex::aux::parse_aux(&aux_text);
    assert_eq!(a.bibstyle.as_deref(), Some("rfs"));
    assert_eq!(a.bibdata, vec!["references"]);
    assert_eq!(a.cites.len(), 173);

    let out = tex_bibtex::run("main", &dir, false);
    assert_eq!(out.status, 0, "run should be spotless; blg: {}", out.blg);
    let oracle = std::fs::read_to_string(dir.join("main.bbl.oracle")).unwrap();
    assert_eq!(out.bbl, oracle, ".bbl must match real bibtex byte-for-byte");
}

#[test]
fn crossref_inclusion_and_repair() {
    // kid1/kid2 crossref a parent that appears AFTER them: the parent joins
    // the citation list (2 crossrefs >= min 2) and the kids inherit its fields.
    let tmp = std::env::temp_dir().join(format!("tex-bibtex-xref-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::copy(fixtures().join("plain.bst"), tmp.join("plain.bst")).unwrap();
    std::fs::write(
        tmp.join("t.bib"),
        concat!(
            "@inproceedings{k1, title={Kid One}, author={A. B}, crossref={par}, pages={1--9}}\n",
            "@inproceedings{k2, title={Kid Two}, author={C. D}, crossref={PAR}, pages={2--9}}\n",
            "@article{par, title={The Parent}, author={E. F}, journal={J}, year=2001}\n",
        ),
    )
    .unwrap();
    std::fs::write(
        tmp.join("t.aux"),
        "\\relax\n\\citation{k1}\n\\citation{k2}\n\\bibstyle{plain}\n\\bibdata{t}\n",
    )
    .unwrap();

    let out = tex_bibtex::run("t", &tmp, false);
    assert_eq!(out.status, 0, "blg: {}", out.blg);
    // parent included, case-insensitive crossref match, cite$ shows the
    // database key spelling
    assert!(out.bbl.contains("\\bibitem{par}"), "blg: {}", out.blg);
    assert!(out.bbl.contains("\\bibitem{k1}"));
    assert!(out.bbl.contains("\\bibitem{k2}"));

    // one crossref alone is below min_crossrefs: the parent stays out of bbl
    // without error because the parent entry exists in the database
    std::fs::write(
        tmp.join("u.aux"),
        "\\relax\n\\citation{k1}\n\\bibstyle{plain}\n\\bibdata{t}\n",
    )
    .unwrap();
    let out = tex_bibtex::run("u", &tmp, false);
    assert_eq!(out.status, 0);
    assert!(!out.bbl.contains("\\bibitem{par}"), "blg: {}", out.blg);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn cite_all_includes_every_entry() {
    let tmp = std::env::temp_dir().join(format!("tex-bibtex-all-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::copy(fixtures().join("plain.bst"), tmp.join("plain.bst")).unwrap();
    std::fs::write(
        tmp.join("a.bib"),
        concat!(
            "@article{one, title={One}, author={A. B}, journal={J}, year=2001}\n",
            "@book{two, title={Two}, author={C. D}, publisher={P}, year=2002}\n",
        ),
    )
    .unwrap();
    std::fs::write(
        tmp.join("a.aux"),
        "\\relax\n\\citation{two}\n\\citation{*}\n\\bibstyle{plain}\n\\bibdata{a}\n",
    )
    .unwrap();

    let out = tex_bibtex::run("a", &tmp, false);
    assert_eq!(out.status, 0, "blg: {}", out.blg);
    assert!(out.bbl.contains("\\bibitem{two}"));
    assert!(out.bbl.contains("\\bibitem{one}"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn missing_entries_and_min_crossrefs_option() {
    let tmp = std::env::temp_dir().join(format!("tex-bibtex-opt-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::copy(fixtures().join("plain.bst"), tmp.join("plain.bst")).unwrap();
    std::fs::write(
        tmp.join("m.bib"),
        "@article{x, title={X}, author={A. B}, journal={J}, year=2001}\n",
    )
    .unwrap();
    std::fs::write(
        tmp.join("m.aux"),
        "\\relax\n\\citation{ghost}\n\\bibstyle{plain}\n\\bibdata{m}\n",
    )
    .unwrap();

    let out = tex_bibtex::run("m", &tmp, false);
    assert_eq!(out.status, 0, "warnings alone do not fail: {}", out.blg);
    assert!(out.blg.contains("I didn't find a database entry for \"ghost\""));

    let out = tex_bibtex::run_opts(
        "m",
        &tmp,
        false,
        &tex_bibtex::RunOpts {
            min_crossrefs: 5,
            terse: true,
        },
    );
    assert_eq!(out.status, 0);
    assert!(out.stdout.is_empty(), "terse mode silences stdout");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn d5_rfs_reproduces_oracle_bbl() {
    // Fixture generated with real bibtex 0.99e (TeX Live 2026):
    // `cd /tmp/fanout/docs/d5-bib && bibtex d5`, aux produced by real
    // pdflatex via latexmk. Isolates BST interpretation from our engine.
    let dir = fixtures().join("d5");
    let out = tex_bibtex::run("d5", &dir, false);
    assert_eq!(out.status, 0, "run should be spotless; blg: {}", out.blg);
    let oracle = std::fs::read_to_string(dir.join("d5.bbl.oracle")).unwrap();
    let trim_r = |s: &str| s
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        trim_r(&out.bbl),
        trim_r(&oracle),
        ".bbl must match real bibtex (modulo per-line trailing whitespace)"
    );
}
