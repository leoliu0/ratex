//! TeX Live's makeindex writes its `.ind` and `.ilg` only where kpathsea's
//! `openout_any = p` (the default) allows: what `/usr/bin/makeindex` (TeX
//! Live 2026) does with each command line below, run in `sub/`.

use std::path::Path;

fn run(work: &Path, args: &[&str], output_dir: Option<&Path>) -> i32 {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    let no_tree = |_: &str| None;
    let host = tex_makeindex::DirHost { work_dir: work, output_dir, tree: &no_tree, stdin: false };
    tex_makeindex::run_cli(&args, &host)
}

#[test]
fn refused_output_names_write_nothing_there() {
    let root = std::env::temp_dir().join(format!("texres-makeindex-openout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let work = root.join("sub");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("x.idx"), "\\indexentry{a}{1}\n").unwrap();
    let absolute = root.join("abs.ind");
    let absolute = absolute.to_str().unwrap();

    // `Not writing to ../e.ind (openout_any = p; no extended check).` and
    // `Can't create output index file ../e.ind.`, status 1, no files.
    for ind in ["../e.ind", ".hidden.ind", "a/../b.ind", "a/..", "sub2/.x/y.ind", "~/zz.ind", absolute] {
        assert_eq!(run(&work, &["-q", "-o", ind, "-t", "../e.ilg", "x.idx"], None), 1, "{ind}");
        assert!(!root.join("e.ilg").exists() && !root.join("abs.ind").exists(), "{ind}");
        assert!(!work.join("x.ind").exists() && !work.join(".hidden.ind").exists(), "{ind}");
    }
    assert!(!root.join("e.ind").exists() && !root.join("b.ind").exists());

    // A refused transcript: the output index has been created, empty.
    assert_eq!(run(&work, &["-q", "-t", "../e.ilg", "x.idx"], None), 1);
    assert_eq!(std::fs::read(work.join("x.ind")).unwrap(), b"");
    assert!(!root.join("e.ilg").exists() && !work.join("x.ilg").exists());

    // `./`, `..` inside a name and `.` inside a component are fine.
    for ind in ["./y.ind", "x..ind"] {
        assert_eq!(run(&work, &["-q", "-o", ind, "x.idx"], None), 0, "{ind}");
        assert!(work.join(ind).is_file(), "{ind}");
    }

    // TeX exports its output directory as TEXMF_OUTPUT_DIRECTORY, below
    // which absolute names are allowed.
    assert_eq!(run(&work, &["-q", "-o", absolute, "x.idx"], Some(&root)), 0);
    assert!(root.join("abs.ind").is_file());
    let _ = std::fs::remove_dir_all(&root);
}
