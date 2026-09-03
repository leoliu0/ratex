fn main() {
    let mut h = tex_core::hyphen::Trie::new();
    h.load_hyphen_file(std::path::Path::new("/usr/share/texmf-dist/tex/generic/hyphen/hyphen.tex")).unwrap();
    for w in ["signi", "cant", "effect", "remains", "significant", "specifications", "specific", "ations"] {
        let pts = h.hyphenate(w.as_bytes(), 2, 2);
        println!("{}: {:?}", w, pts);
    }
}
