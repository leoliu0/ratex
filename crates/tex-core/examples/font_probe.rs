use tex_core::fontload::FontLoader;
fn main() {
    let mut loader = FontLoader::new();
    for name in ["udmj65", "udmj67", "udmj8a", "udmj30"] {
        let vf = tex_kpse::get_embedded_package(&format!("{name}.vf"));
        println!(
            "{name}: embedded VF {} bytes",
            vf.as_ref().map_or(0, Vec::len)
        );
        if let Some(font) = loader.load_tfm(name, 655360) {
            println!(
                "tfm={} size={} outline={:?}",
                font.tfm_name, font.at_size, font.type1_path
            );
            let vf = loader.vf_fonts.get(&(font.tfm_name.clone(), font.at_size));
            println!(
                "parsed VF: {:?}",
                vf.map(|vf| vf
                    .bases
                    .iter()
                    .map(|b| (&b.tfm_name, b.at_size))
                    .collect::<Vec<_>>())
            );
        }
    }
}
