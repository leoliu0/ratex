use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/packages.tar.zst");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let archive = std::fs::File::open("assets/packages.tar.zst").unwrap();
    let mut archive = tar::Archive::new(zstd::Decoder::new(archive).unwrap());
    let mut blob =
        std::io::BufWriter::new(std::fs::File::create(out.join("packages.bin")).unwrap());
    let mut index = BTreeMap::new();
    let mut offset = 0usize;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().unwrap().into_owned();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        // Match the old archive search's first-basename-wins behavior.
        if index.contains_key(name) {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        let compressed = zstd::encode_all(data.as_slice(), 3).unwrap();
        blob.write_all(&compressed).unwrap();
        index.insert(name.to_owned(), (offset, compressed.len()));
        offset += compressed.len();
    }
    blob.flush().unwrap();
    let mut generated =
        std::io::BufWriter::new(std::fs::File::create(out.join("packages_index.rs")).unwrap());
    writeln!(
        generated,
        "static PACKAGE_INDEX: &[(&str, usize, usize)] = &["
    )
    .unwrap();
    let mut folded = BTreeMap::new();
    for (position, (name, (offset, length))) in index.into_iter().enumerate() {
        writeln!(generated, "({name:?}, {offset}, {length}),").unwrap();
        folded.entry(name.to_ascii_lowercase()).or_insert(position);
    }
    writeln!(generated, "];").unwrap();
    writeln!(generated, "static PACKAGE_FOLDED: &[(&str, usize)] = &[").unwrap();
    for (name, position) in folded {
        writeln!(generated, "({name:?}, {position}),").unwrap();
    }
    writeln!(generated, "];").unwrap();
}
