// probe: multicolumn header row + trailing plain cell
use tex_core::boxes::Node;
use tex_core::engine::Engine;

fn text_of(n: &Node, out: &mut String) {
    match n {
        Node::Char { c, .. } => out.push(*c as char),
        Node::Ligature { letters, n_letters, .. } => {
            for j in 0..*n_letters as usize {
                out.push(letters[j] as char);
            }
        }
        Node::Glue(_) => out.push(' '),
        Node::Box { list, .. } => {
            for m in list {
                text_of(m, out);
            }
        }
        _ => {}
    }
}

#[test]
fn multicolumn_trailing_cell() {
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    let src = concat!(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\&=4 \\baselineskip=12pt\n",
        "\\def\\multicolumn#1#2#3{#3}\n",
        "\\setbox0=\\vbox{\\halign{#\\hfil&&\\hfil#\\hfil\\cr\n",
        "&\\multicolumn{3}{c}{Exit}&&X\\cr\n",
        "Clock&k0&k1&k2\\cr\n",
        "}}\\showbox0\n",
    );
    e.input.push_file("probe.tex".to_string(), src.as_bytes().to_vec());
    e.run();
    eprintln!("TERM: {}", e.term);
    let b = e.eqtb.boxed[0].clone().expect("box0");
    let mut s = String::new();
    if let Node::Box { list, .. } = &b {
        for row in list {
            let mut t = String::new();
            text_of(row, &mut t);
            s.push_str(&format!("[{}]", t));
        }
    }
    eprintln!("ROWS: {}", s);
    panic!("introspect");
}
