//! Outputs of TeX Live's `makeindex` 2.18 for a set of inputs; the port must
//! reproduce the `.ind` and `.ilg` files (and the exit status) byte for byte.
//!
//! Each directory under `tests/oracle/` holds the input files, `args` (the
//! command line) and, under `expected/`, what `/usr/bin/makeindex` wrote when
//! run with those arguments in a copy of the directory, plus `status`. The
//! transcript's first line names the program and is compared without it.

use std::path::{Path, PathBuf};

const BANNER: &[u8] = b"This is makeindex, version 2.18 [TeX Live 2026] (TeXres).\n";
const TL_BANNER: &[u8] = b"This is makeindex, version 2.18 [TeX Live 2026] (kpathsea + Thai support).\n";

fn replace(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if text[at..].starts_with(from) {
            out.extend_from_slice(to);
            at += from.len();
        } else {
            out.push(text[at]);
            at += 1;
        }
    }
    out
}

fn check(case: &str) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle").join(case);
    let work: PathBuf = std::env::temp_dir().join(format!("texres-makeindex-oracle-{}-{case}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    for entry in std::fs::read_dir(&source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() && entry.file_name() != "args" {
            std::fs::copy(entry.path(), work.join(entry.file_name())).unwrap();
        }
    }
    let args: Vec<String> = std::fs::read_to_string(source.join("args"))
        .unwrap()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let no_tree = |_: &str| None;
    let host = tex_makeindex::DirHost { work_dir: &work, output_dir: None, tree: &no_tree, stdin: false };
    let status = tex_makeindex::run_cli(&args, &host);

    let expected = source.join("expected");
    let expected_status: i32 = std::fs::read_to_string(expected.join("status")).unwrap().trim().parse().unwrap();
    assert_eq!(status, expected_status, "{case}: exit status");
    for entry in std::fs::read_dir(&expected).unwrap() {
        let name = entry.unwrap().file_name();
        if name == "status" {
            continue;
        }
        let want = replace(&std::fs::read(expected.join(&name)).unwrap(), TL_BANNER, BANNER);
        let got = std::fs::read(work.join(&name)).unwrap_or_else(|_| panic!("{case}: {name:?} not written"));
        assert_eq!(
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(&want),
            "{case}: {name:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&work);
}

macro_rules! oracle_cases {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                check(stringify!($name));
            }
        )*
    };
}

oracle_cases!(
    basic,
    letter,
    noranges,
    wrap,
    styled,
    long_delimiters,
    start_page,
    start_page_odd,
    start_page_no_log_page,
    german,
    utf8_keys,
    page_types,
    precedence,
    style_keywords,
    style_errors,
    input_errors,
    range_warnings,
    mst_style,
    multiple_files,
    transcript_name,
    too_many_fields,
    letter_ordering_trailing_blank,
    compress_blanks,
    duplicates,
);
