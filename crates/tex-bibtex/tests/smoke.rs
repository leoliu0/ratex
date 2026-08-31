use tex_bibtex::{aux, bst};

#[test]
fn parse_rfs_bst() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rfs.bst"
    ))
    .unwrap();
    let cmds = bst::BstParser::new(&text).parse();
    let mut counts: std::collections::HashMap<String, usize> = Default::default();
    for c in &cmds {
        let k = match c {
            bst::BstCommand::Entry { .. } => "entry",
            bst::BstCommand::Function(_, _) => "function",
            bst::BstCommand::Macro(_, _) => "macro",
            bst::BstCommand::Read => "read",
            bst::BstCommand::Execute(_) => "execute",
            bst::BstCommand::Iterate(_) => "iterate",
            bst::BstCommand::Reverse(_) => "reverse",
            bst::BstCommand::Sort => "sort",
            bst::BstCommand::Integers(_) => "integers",
            bst::BstCommand::Strings(_) => "strings",
        };
        *counts.entry(k.to_string()).or_default() += 1;
    }
    println!("{:?}", counts);
    assert!(cmds.len() > 100);
    let aux_text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/main.aux"
    ))
    .unwrap();
    let a = aux::parse_aux(&aux_text);
    assert_eq!(a.bibstyle.as_deref(), Some("rfs"));
    assert_eq!(a.bibdata, vec!["references"]);
    assert_eq!(a.cites.len(), 173);
}

#[test]
fn format_sort_names() {
    let out = tex_bibtex::names::format_name(
        "{vv{ } }{ll{ }}{  f{ }}{  jj{ }}",
        1,
        "Appel, I. R.",
    );
    println!("got: {:?}", out);
    let out2 = tex_bibtex::names::format_name(
        "{vv~}{ll}{, jj}{, f.}",
        1,
        "Appel, I. R.",
    );
    println!("bibitem fmt: {:?}", out2);
    assert_eq!(out, "Appel  I R");
    assert_eq!(out2, "Appel, I.~R.");
}
