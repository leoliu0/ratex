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
        let generated = "static PACKAGES: &[u8] = &[];\n\
                         static PACKAGE_CHUNKS: PackedTable<3> = PackedTable(&[]);\n\
                         static PACKAGE_NAMES: &[u8] = &[];\n\
                         static PACKAGE_INDEX: PackedTable<5> = PackedTable(&[]);\n\
                         static PACKAGE_FOLDED: PackedTable<1> = PackedTable(&[]);\n\
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
    let blob_path = out.join("packages.bin");
    let mut blob = BlobWriter {
        file: std::io::BufWriter::new(std::fs::File::create(&blob_path).unwrap()),
        len: 0,
        hash: FNV_OFFSET,
    };
    let mut index: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut chunks: Vec<(usize, usize, usize)> = Vec::new();
    let mut chunk = Vec::with_capacity(CHUNK_TARGET);
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
            write_chunk(&mut blob, &mut chunks, &mut chunk);
        }
        let chunk_index = chunks.len();
        let member_offset = chunk.len();
        let member_len = data.len();
        chunk.extend_from_slice(&data);
        index.insert(name.to_owned(), (chunk_index, member_offset, member_len));
        if chunk.len() >= CHUNK_TARGET {
            write_chunk(&mut blob, &mut chunks, &mut chunk);
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
    write_chunk(&mut blob, &mut chunks, &mut chunk);
    blob.file.flush().unwrap();

    // Every table is a flat array of little-endian u32 records embedded as
    // bytes, so rustc sees one byte string instead of an array literal with
    // tens of thousands of tuples to type-check, lower and optimize.
    let mut chunk_table = Vec::with_capacity(chunks.len() * 12);
    for (offset, compressed, decoded) in chunks {
        push_u32s(&mut chunk_table, [offset, compressed, decoded]);
    }

    // A Rust `&str` per record would cost a pointer-sized field and a dynamic
    // relocation in position-independent executables. Keep exact names in one
    // byte blob and refer to them with u32 offset/length pairs; the folded
    // index reuses those same names.
    let mut names = Vec::new();
    let mut folded = BTreeMap::new();
    let mut index_table = Vec::with_capacity(index.len() * 20);
    for (position, (name, (chunk, offset, length))) in index.into_iter().enumerate() {
        push_u32s(
            &mut index_table,
            [names.len(), name.len(), chunk, offset, length],
        );
        names.extend_from_slice(name.as_bytes());
        folded.entry(name.to_ascii_lowercase()).or_insert(position);
    }
    // Folded order needs only package positions. Runtime folds the exact name
    // while comparing, avoiding a second name table and a query allocation.
    let mut folded_table = Vec::with_capacity(folded.len() * 4);
    for position in folded.into_values() {
        push_u32s(&mut folded_table, [position]);
    }
    std::fs::write(out.join("package_chunks.bin"), &chunk_table).unwrap();
    std::fs::write(out.join("package_names.bin"), &names).unwrap();
    std::fs::write(out.join("package_index.bin"), &index_table).unwrap();
    std::fs::write(out.join("package_folded.bin"), &folded_table).unwrap();

    let mut generated =
        std::io::BufWriter::new(std::fs::File::create(out.join("packages_index.rs")).unwrap());
    write_packages_blob(&mut generated, &blob_path, blob.len, blob.hash);
    for (name, file, fields) in [
        ("PACKAGE_CHUNKS", "package_chunks.bin", Some(3)),
        ("PACKAGE_NAMES", "package_names.bin", None),
        ("PACKAGE_INDEX", "package_index.bin", Some(5)),
        ("PACKAGE_FOLDED", "package_folded.bin", Some(1)),
    ] {
        let bytes = format!("include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{file}\"))");
        match fields {
            Some(fields) => writeln!(
                generated,
                "static {name}: PackedTable<{fields}> = PackedTable({bytes});"
            ),
            None => writeln!(generated, "static {name}: &[u8] = {bytes};"),
        }
        .unwrap();
    }
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
    generated.flush().unwrap();
}

fn push_u32s<const N: usize>(table: &mut Vec<u8>, values: [usize; N]) {
    for value in values {
        table.extend_from_slice(&packed(value).to_le_bytes());
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

struct BlobWriter {
    file: std::io::BufWriter<std::fs::File>,
    len: usize,
    /// Content fingerprint for the assembly symbol name. The `.incbin` text
    /// alone would stay identical when the archive changes, letting
    /// incremental compilation reuse an object holding stale bytes.
    hash: u64,
}

impl BlobWriter {
    fn write(&mut self, bytes: &[u8]) {
        self.file.write_all(bytes).unwrap();
        self.len += bytes.len();
        let words = bytes.chunks_exact(8);
        let tail = words.remainder();
        for word in words {
            let word = u64::from_le_bytes(word.try_into().unwrap());
            self.hash = (self.hash ^ word).wrapping_mul(FNV_PRIME);
        }
        for &byte in tail {
            self.hash = (self.hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
        }
    }
}

#[derive(Clone, Copy)]
enum ObjectFormat {
    Elf,
    MachO,
    Coff,
}

/// Object format for embedding the archive with `global_asm!`, or `None` to
/// fall back to `include_bytes!` (e.g. wasm32, which has no assembler).
fn asm_object_format() -> Option<ObjectFormat> {
    let cfg = |key: &str| std::env::var(format!("CARGO_CFG_TARGET_{key}")).unwrap_or_default();
    let arch = cfg("ARCH");
    // `global_asm!` is stable only where Rust inline assembly is stable.
    let asm_arch = matches!(
        arch.as_str(),
        "x86" | "x86_64" | "arm" | "aarch64" | "riscv32" | "riscv64" | "loongarch64"
    );
    if !asm_arch {
        return None;
    }
    if cfg("VENDOR") == "apple" {
        return Some(ObjectFormat::MachO);
    }
    if cfg("OS") == "windows" {
        return Some(ObjectFormat::Coff);
    }
    cfg("FAMILY")
        .split(',')
        .any(|family| family == "unix")
        .then_some(ObjectFormat::Elf)
}

/// Emit `PACKAGES`. rustc copies an `include_bytes!` static through const
/// evaluation, crate metadata, LLVM IR, bitcode and (with ThinLTO) every
/// module importing it, which needs tens of GiB for a 600+ MiB archive. The
/// assembler's `.incbin` streams the file straight into the object instead.
fn write_packages_blob(generated: &mut impl Write, path: &std::path::Path, len: usize, hash: u64) {
    let Some(format) = asm_object_format() else {
        writeln!(
            generated,
            "static PACKAGES: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/packages.bin\"));"
        )
        .unwrap();
        return;
    };
    let symbol = format!("tex_kpse_packages_{hash:016x}");
    // Mach-O and 32-bit Windows prefix C symbol names with an underscore.
    let prefix = match format {
        ObjectFormat::MachO => "_",
        ObjectFormat::Coff if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86") => "_",
        _ => "",
    };
    let label = format!("{prefix}{symbol}");
    let path = path
        .to_str()
        .expect("OUT_DIR must be valid UTF-8")
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let incbin = format!(".incbin \"{path}\"");
    let lines: Vec<String> = match format {
        ObjectFormat::Elf => vec![
            ".pushsection .rodata.tex_kpse_packages,\"a\"".into(),
            format!(".globl {label}"),
            format!(".hidden {label}"),
            format!(".type {label}, STT_OBJECT"),
            ".p2align 4".into(),
            format!("{label}:"),
            incbin,
            format!(".size {label}, . - {label}"),
            ".popsection".into(),
        ],
        ObjectFormat::MachO => vec![
            ".pushsection __TEXT,__const".into(),
            format!(".globl {label}"),
            format!(".private_extern {label}"),
            ".p2align 4".into(),
            format!("{label}:"),
            incbin,
            ".popsection".into(),
        ],
        ObjectFormat::Coff => vec![
            ".pushsection .rdata,\"dr\"".into(),
            format!(".globl {label}"),
            ".p2align 4".into(),
            format!("{label}:"),
            incbin,
            ".popsection".into(),
        ],
    };
    writeln!(generated, "core::arch::global_asm!(").unwrap();
    for line in lines {
        // Braces are `global_asm!` operand placeholders.
        let line = line.replace('{', "{{").replace('}', "}}");
        writeln!(generated, "    {line:?},").unwrap();
    }
    writeln!(generated, ");").unwrap();
    writeln!(
        generated,
        "extern \"C\" {{\n    #[link_name = \"{symbol}\"]\n    static PACKAGES_BLOB: [u8; {len}];\n}}\n\
         // SAFETY: the `global_asm!` above defines this symbol as exactly {len}\n\
         // bytes of `packages.bin` in a read-only section that nothing mutates.\n\
         static PACKAGES: &[u8] = unsafe {{ &PACKAGES_BLOB }};"
    )
    .unwrap();
}

fn packed(value: usize) -> u32 {
    u32::try_from(value).expect("embedded package index exceeds 4 GiB")
}

fn write_chunk(
    blob: &mut BlobWriter,
    chunks: &mut Vec<(usize, usize, usize)>,
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
    chunks.push((blob.len, compressed.len(), decoded_len));
    blob.write(&compressed);
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
