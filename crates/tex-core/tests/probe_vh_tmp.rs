// probe: vbox height vs content extent after paragraph
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

fn show(n: &Node, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    match n {
        Node::Box { kind, w, h, d, list, .. } => {
            out.push_str(&format!("{}B{} w={:.1} h={:.1} d={:.1} n={}\n", pad, kind, *w as f64 / 65536.0, *h as f64 / 65536.0, *d as f64 / 65536.0, list.len()));
            for m in list.iter().take(40) {
                show(m, depth + 1, out);
            }
        }
        Node::Glue(g) => out.push_str(&format!("{}G {:.1}\n", pad, g.width as f64 / 65536.0)),
        Node::Penalty(p) => out.push_str(&format!("{}p{}\n", pad, p)),
        Node::Kern(k) | Node::ExplicitKern(k) => out.push_str(&format!("{}k{:.1}\n", pad, *k as f64 / 65536.0)),
        _ => {}
    }
}

#[test]
fn vbox_height_probe() {
    let para = "The quick brown fox jumps over the lazy dog and runs far away into the deep dark forest where nobody can find it ever again today or tomorrow or ever. ";
    let body = para.repeat(6);
    let e1 = run(&format!("\\setbox0=\\vbox{{\\hsize=200pt {}}}\\showbox0", body));
    let b1 = e1.eqtb.boxed[0].clone().unwrap();
    let mut s = String::new();
    show(&b1, 0, &mut s);
    eprintln!("PLAIN VBOX:\n{}", s);
    // sum of children heights+depths+glues at top level
    if let Node::Box { list, h, d, .. } = &b1 {
        let mut sum = 0i64;
        let mut pd = 0i64;
        for m in list {
            match m {
                Node::Box { h: hh, d: dd, .. } => { sum += pd + *hh as i64; pd = *dd as i64; }
                Node::Glue(g) => { sum += pd + g.width as i64; pd = 0; }
                Node::Kern(k) | Node::ExplicitKern(k) => { sum += pd + *k as i64; pd = 0; }
                Node::Penalty(_) => {}
                _ => {}
            }
        }
        eprintln!("outer h={:.1} d={:.1} contentsum={:.1}", *h as f64 / 65536.0, *d as f64 / 65536.0, (sum + pd) as f64 / 65536.0);
    }
    panic!("introspect");
}
