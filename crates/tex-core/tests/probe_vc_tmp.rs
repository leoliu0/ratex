// probe: does a paragraph inside $\vcenter{\vbox{...}}$ leak lines?
use tex_core::boxes::Node;
use tex_core::engine::Engine;

fn run(src: &str) -> Engine {
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    let full = format!(
        "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6 \\catcode`\\$=3 \\font\\cmr=cmr10 \\cmr {}\n",
        src
    );
    e.input.push_file("probe.tex".to_string(), full.as_bytes().to_vec());
    e.run();
    e
}

fn count_lines(n: &Node) -> usize {
    match n {
        Node::Box { kind, list, .. } if *kind == tex_core::boxes::VBOX => {
            let direct = list
                .iter()
                .filter(|m| matches!(m, Node::Box { kind, .. } if *kind == tex_core::boxes::HBOX))
                .count();
            if direct > 0 {
                return direct;
            }
            list.iter().map(count_lines).sum()
        }
        _ => 0,
    }
}
#[allow(dead_code)]
fn count_lines_orig(n: &Node) -> usize {
    match n {
        Node::Box { kind, list, .. } if *kind == tex_core::boxes::VBOX => {
            list.iter()
                .filter(|m| matches!(m, Node::Box { kind, .. } if *kind == tex_core::boxes::HBOX))
                .count()
        }
        _ => 0,
    }
}

#[test]
fn vcenter_paragraph_leak() {
    let para = "The quick brown fox jumps over the lazy dog and runs far away into the deep dark forest where nobody can find it ever again today or tomorrow or ever. ";
    let body = para.repeat(6);
    // plain vbox control
    let e1 = run(&format!("\\setbox0=\\vbox{{\\hsize=200pt {}}}\\showbox0", body));
    let b1 = e1.eqtb.boxed[0].clone().unwrap();
    eprintln!("BOX0: {:?}", match &b1 { Node::Box { kind, list, .. } => format!("kind={} n={} kinds={:?}", kind, list.len(), list.iter().map(|m| match m { Node::Box{kind,..}=>format!("B{}",kind), Node::Glue(_)=>"G".into(), Node::Penalty(p)=>format!("p{}",p), _=>"?".into() }).collect::<Vec<_>>()), _ => "??".into() });
    let n1 = count_lines(&b1);
    eprintln!("plain vbox lines: {} term: {}", n1, e1.term);
    // vcenter-wrapped vbox
    let e2 = run(&format!("$\\vcenter{{\\vbox{{\\hsize=200pt {}}}}}$", body));
    eprintln!("vcenter page nodes: {} cur: {}", e2.page_list.len(), e2.cur_list.len());
    let mut found = 0;
    for n in e2.page_list.iter().chain(e2.cur_list.iter()) {
        let desc = match n {
            Node::Box { kind, list, .. } => format!("B{} n={} innerlines={}", kind, list.len(), count_lines(n)),
            Node::Glue(_) => "G".into(),
            Node::Penalty(p) => format!("p{}", p),
            _ => "?".into(),
        };
        eprintln!("  node: {}", desc);
        if let Node::Box { .. } = n {
            let cnt = count_lines(n);
            if cnt > 0 { found = cnt; }
        }
    }
    eprintln!("vcenter inner lines: {} term: {}", found, e2.term);
    panic!("introspect");
}
