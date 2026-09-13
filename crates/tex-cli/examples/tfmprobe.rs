fn main() {
    let mut fl = tex_core::fontload::FontLoader::new();
    let f = fl.load_tfm("ntx-Regular-tlf-t1", 786432).expect("load");
    println!(
        "space={} stretch={} shrink={} (sp)",
        f.space(),
        f.space_stretch(),
        f.space_shrink()
    );
    println!(
        "pt: {:.5} {:.5} {:.5}",
        f.space() as f64 / 65536.0,
        f.space_stretch() as f64 / 65536.0,
        f.space_shrink() as f64 / 65536.0
    );
    // tex-exact expectation: real pdftex showed 3.0 / 2.39996 / 1.20007
}
