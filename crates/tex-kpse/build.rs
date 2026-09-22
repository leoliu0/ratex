use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;

// Independently compressing every small .sty/.fd file throws away almost all
// cross-file redundancy. Four MiB chunks preserve random access while getting
// close to the compression ratio of the original solid archive.
const CHUNK_TARGET: usize = 4 * 1024 * 1024;
const MAX_ARCHIVE_WINDOW_BYTES: u64 = 512 * 1024 * 1024;

fn main() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    if std::env::var("CARGO_FEATURE_EMBED_PACKAGES").is_err() {
        // Lean build: do not embed the 80+ MB package archive in the binary.
        std::fs::write(out.join("packages.bin"), b"").unwrap();
        std::fs::write(out.join("package_names.bin"), b"").unwrap();
        let generated = "static PACKAGE_CHUNKS: &[(u32, u32, u32)] = &[];\n\
                         static PACKAGE_NAMES: &[u8; 0] = b\"\";\n\
                         static PACKAGE_INDEX: &[(u32, u32, u32, u32, u32)] = &[];\n\
                         static PACKAGE_FOLDED: &[u32] = &[];\n\
                         static EMBEDDED_FONT_FACES: &[EmbeddedFontFace] = &[];\n";
        std::fs::write(out.join("packages_index.rs"), generated).unwrap();
        return;
    }
    println!("cargo:rerun-if-changed=assets/packages.lock.json");
    let lock_text = std::fs::read_to_string("assets/packages.lock.json")
        .expect("assets/packages.lock.json must be present");
    let mut part_names = Vec::new();
    let mut in_parts = false;
    for line in lock_text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("\"parts\":") {
            in_parts = true;
        } else if in_parts {
            if trimmed.starts_with(']') {
                break;
            }
            if trimmed.starts_with("\"name\":") {
                if let Some(val) = trimmed.split(':').nth(1) {
                    let name = val
                        .trim()
                        .trim_matches(|c| c == '"' || c == ',' || c == ' ');
                    if name.starts_with("packages.tar.zst.") {
                        part_names.push(name.to_string());
                    }
                }
            }
        }
    }
    assert!(
        !part_names.is_empty(),
        "No packages.tar.zst.* parts declared in assets/packages.lock.json"
    );

    let assets_dir = std::path::Path::new("assets");
    let mut chained_reader: Box<dyn Read> = Box::new(std::io::empty());
    for part_name in &part_names {
        let p = assets_dir.join(part_name);
        println!("cargo:rerun-if-changed={}", p.display());
        let file = std::fs::File::open(&p)
            .unwrap_or_else(|e| panic!("failed to open locked part {}: {e}", p.display()));
        chained_reader = Box::new(chained_reader.chain(file));
    }
    let decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
        chained_reader,
        MAX_ARCHIVE_WINDOW_BYTES,
    )
    .expect("locked package archive must be a valid bounded zstd frame");
    let mut archive = tar::Archive::new(decoder);
    let mut blob =
        std::io::BufWriter::new(std::fs::File::create(out.join("packages.bin")).unwrap());
    let mut index: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut chunks: Vec<(usize, usize, usize)> = Vec::new();
    let mut chunk = Vec::with_capacity(CHUNK_TARGET);
    let mut blob_offset = 0usize;
    let mut font_faces = Vec::new();
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
        if !chunk.is_empty() && chunk.len().saturating_add(data.len()) > CHUNK_TARGET {
            write_chunk(&mut blob, &mut chunks, &mut blob_offset, &mut chunk);
        }
        let chunk_index = chunks.len();
        let member_offset = chunk.len();
        let member_len = data.len();
        chunk.extend_from_slice(&data);
        index.insert(name.to_owned(), (chunk_index, member_offset, member_len));
        if chunk.len() >= CHUNK_TARGET {
            write_chunk(&mut blob, &mut chunks, &mut blob_offset, &mut chunk);
        }
        let is_font = name.ends_with(".otf")
            || name.ends_with(".ttf")
            || name.ends_with(".ttc")
            || name.ends_with(".otc");
        if is_font {
            let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
            for face_idx in 0..count {
                if let Ok(face) = ttf_parser::Face::parse(&data, face_idx) {
                    // Legacy name 1 (Family) and name 2 (Subfamily) preserve distinct optical
                    // sizes (e.g. 'LM Roman 10' vs 'LM Roman 12') and avoid ambiguity.
                    // Postscript name 6 gives exact font identification.
                    let family = best_name(&face, 1).unwrap_or_default();
                    let subfamily = best_name(&face, 2).unwrap_or_else(|| "Regular".to_string());
                    let postscript = best_name(&face, 6).unwrap_or_default();
                    let weight = face.weight().to_number();
                    let italic = face.is_italic();
                    font_faces.push((
                        name.to_owned(),
                        face_idx,
                        family,
                        subfamily,
                        postscript,
                        weight,
                        italic,
                    ));
                }
            }
        }
    }
    write_chunk(&mut blob, &mut chunks, &mut blob_offset, &mut chunk);
    blob.flush().unwrap();
    let mut generated =
        std::io::BufWriter::new(std::fs::File::create(out.join("packages_index.rs")).unwrap());
    let chunks: Vec<(u32, u32, u32)> = chunks
        .into_iter()
        .map(|(offset, compressed, decoded)| (packed(offset), packed(compressed), packed(decoded)))
        .collect();
    writeln!(
        generated,
        "static PACKAGE_CHUNKS: &[(u32, u32, u32)] = &{chunks:?};"
    )
    .unwrap();

    // A Rust `&str` in every generated record costs both a pointer-sized
    // field and a dynamic relocation in position-independent executables.
    // Keep exact names in one byte blob and refer to them with u32
    // offset/length pairs; the folded index reuses those same names. Besides
    // shrinking the installed binary, this substantially reduces the dynamic
    // loader's relocation work before `main`.
    let entries: Vec<_> = index.into_iter().collect();
    let mut names = Vec::new();
    let mut folded = BTreeMap::new();
    let mut packed_entries = Vec::with_capacity(entries.len());
    for (position, (name, (chunk, offset, length))) in entries.iter().enumerate() {
        let name_offset = packed(names.len());
        let name_length = packed(name.len());
        names.extend_from_slice(name.as_bytes());
        packed_entries.push((
            name_offset,
            name_length,
            packed(*chunk),
            packed(*offset),
            packed(*length),
        ));
        folded.entry(name.to_ascii_lowercase()).or_insert(position);
    }
    // Folded order needs only package positions. Runtime folds the exact name
    // while comparing, avoiding a second name table and a query allocation.
    let packed_folded: Vec<u32> = folded.into_values().map(packed).collect();
    std::fs::write(out.join("package_names.bin"), &names).unwrap();
    writeln!(
        generated,
        "static PACKAGE_NAMES: &[u8; {}] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/package_names.bin\"));",
        names.len()
    )
    .unwrap();
    writeln!(
        generated,
        "static PACKAGE_INDEX: &[(u32, u32, u32, u32, u32)] = &{packed_entries:?};"
    )
    .unwrap();
    writeln!(
        generated,
        "static PACKAGE_FOLDED: &[u32] = &{packed_folded:?};"
    )
    .unwrap();
    writeln!(
        generated,
        "static EMBEDDED_FONT_FACES: &[EmbeddedFontFace] = &["
    )
    .unwrap();
    for (file, face_index, family, subfamily, postscript, weight, italic) in font_faces {
        writeln!(
            generated,
            "    EmbeddedFontFace {{ file: {:?}, face_index: {}, family: {:?}, subfamily: {:?}, postscript: {:?}, weight: {}, italic: {} }},",
            file, face_index, family, subfamily, postscript, weight, italic
        )
        .unwrap();
    }
    writeln!(generated, "];").unwrap();
}

fn packed(value: usize) -> u32 {
    u32::try_from(value).expect("embedded package index exceeds 4 GiB")
}

fn write_chunk(
    blob: &mut impl Write,
    chunks: &mut Vec<(usize, usize, usize)>,
    blob_offset: &mut usize,
    chunk: &mut Vec<u8>,
) {
    if chunk.is_empty() {
        return;
    }
    let decoded_len = chunk.len();
    let compressed = ruzstd::encoding::compress_to_vec(
        chunk.as_slice(),
        ruzstd::encoding::CompressionLevel::Fastest,
    );
    blob.write_all(&compressed).unwrap();
    chunks.push((*blob_offset, compressed.len(), decoded_len));
    *blob_offset += compressed.len();
    chunk.clear();
}

/// Prefer English-US Windows records (0x0409) over localized or platform-specific records,
/// falling back to Unicode and generic records.
fn best_name(face: &ttf_parser::Face, name_id: u16) -> Option<String> {
    let mut best_match: Option<(u8, String)> = None;
    for record in face.names() {
        if record.name_id != name_id {
            continue;
        }
        if let Some(s) = record.to_string() {
            let prio = match record.platform_id {
                ttf_parser::PlatformId::Windows if record.language_id == 0x0409 => 4,
                ttf_parser::PlatformId::Windows => 3,
                ttf_parser::PlatformId::Unicode => 2,
                _ => 1,
            };
            if best_match.as_ref().map_or(true, |(p, _)| prio > *p) {
                best_match = Some((prio, s));
            }
        }
    }
    best_match.map(|(_, s)| s)
}
