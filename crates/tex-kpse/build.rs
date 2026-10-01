use std::collections::BTreeMap;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

// Independently compressing every small .sty/.fd file throws away almost all
// cross-file redundancy, while every embedded read decodes its whole chunk.
// 128 KiB chunks keep the compressed size within 0.5% of 4 MiB chunks (the
// fast encoder's matches are short-range anyway) and make a read decode
// ~30x less data.
const CHUNK_TARGET: usize = 128 * 1024;
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
                         static PACKAGE_DIR_NAMES: &[u8] = &[];\n\
                         static PACKAGE_DIRS: PackedTable<2> = PackedTable(&[]);\n\
                         static PACKAGE_MEMBER_DIRS: PackedTable<1> = PackedTable(&[]);\n\
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
    // Append-only overlay written by `bundle_packages.py --supplement`.
    let supplement_name = lock_text
        .split_once("\"supplement_archive\":")
        .and_then(|(_, rest)| rest.split_once("\"name\":"))
        .and_then(|(_, rest)| rest.split('"').nth(1))
        .map(str::to_owned);

    let assets_dir = std::path::Path::new("assets");
    let open = |name: &str| {
        let p = assets_dir.join(name);
        println!("cargo:rerun-if-changed={}", p.display());
        std::fs::File::open(&p)
            .unwrap_or_else(|e| panic!("failed to open locked asset {}: {e}", p.display()))
    };
    // The overlay is read first, so its members shadow same-named members of
    // the main parts (first basename wins), e.g. its consolidated pdftex.map.
    let mut readers: Vec<Box<dyn Read>> = Vec::new();
    if let Some(name) = &supplement_name {
        readers.push(Box::new(open(name)));
    }
    let mut chained_reader: Box<dyn Read> = Box::new(std::io::empty());
    for part_name in &part_names {
        chained_reader = Box::new(chained_reader.chain(open(part_name)));
    }
    readers.push(chained_reader);
    let mut archives: Vec<_> = readers
        .into_iter()
        .map(|reader| {
            let decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
                reader,
                MAX_ARCHIVE_WINDOW_BYTES,
            )
            .expect("locked package archive must be a valid bounded zstd frame");
            tar::Archive::new(decoder)
        })
        .collect();
    let mut blob = BlobWriter::create(&out);
    let mut index: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    // Directory (`/`-separated, relative to the TDS root) of each member.
    let mut member_dirs: BTreeMap<String, String> = BTreeMap::new();
    let mut chunks: Vec<(usize, usize, usize)> = Vec::new();
    let mut chunk = Vec::with_capacity(CHUNK_TARGET);
    let mut font_faces = Vec::new();
    for entry in archives.iter_mut().flat_map(|archive| archive.entries().unwrap()) {
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
        let directory = path
            .parent()
            .map(|parent| {
                parent
                    .components()
                    .filter_map(|part| match part {
                        std::path::Component::Normal(part) => part.to_str(),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default();
        member_dirs.insert(name.to_owned(), directory);
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
    blob.finish();

    // Every table is a flat array of little-endian u32 records embedded as
    // bytes, so rustc sees one byte string instead of an array literal with
    // tens of thousands of tuples to type-check, lower and optimize.
    let mut chunk_table = Vec::with_capacity(chunks.len() * 12);
    for (offset, compressed, decoded) in chunks {
        push_u32s(&mut chunk_table, [offset, compressed, decoded]);
    }

    // Every directory of the archive (with all ancestors), sorted, and the
    // directory of each member in index order: the virtual TDS tree that
    // `<embedded>/…` paths expose.
    let mut directories: BTreeMap<String, usize> = BTreeMap::new();
    directories.insert(String::new(), 0);
    for directory in member_dirs.values() {
        let mut prefix = String::new();
        for part in directory.split('/').filter(|part| !part.is_empty()) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            directories.entry(prefix.clone()).or_insert(0);
        }
    }
    for (position, id) in directories.values_mut().enumerate() {
        *id = position;
    }
    let mut dir_names = Vec::new();
    let mut dir_table = Vec::with_capacity(directories.len() * 8);
    for name in directories.keys() {
        push_u32s(&mut dir_table, [dir_names.len(), name.len()]);
        dir_names.extend_from_slice(name.as_bytes());
    }
    let mut member_dir_table = Vec::with_capacity(index.len() * 4);

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
        push_u32s(&mut member_dir_table, [directories[&member_dirs[&name]]]);
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
    std::fs::write(out.join("package_dir_names.bin"), &dir_names).unwrap();
    std::fs::write(out.join("package_dirs.bin"), &dir_table).unwrap();
    std::fs::write(out.join("package_member_dirs.bin"), &member_dir_table).unwrap();

    let mut generated =
        std::io::BufWriter::new(std::fs::File::create(out.join("packages_index.rs")).unwrap());
    write_packages_blob(&mut generated, &blob, &out);
    for (name, file, fields) in [
        ("PACKAGE_CHUNKS", "package_chunks.bin", Some(3)),
        ("PACKAGE_NAMES", "package_names.bin", None),
        ("PACKAGE_INDEX", "package_index.bin", Some(5)),
        ("PACKAGE_FOLDED", "package_folded.bin", Some(1)),
        ("PACKAGE_DIR_NAMES", "package_dir_names.bin", None),
        ("PACKAGE_DIRS", "package_dirs.bin", Some(2)),
        ("PACKAGE_MEMBER_DIRS", "package_member_dirs.bin", Some(1)),
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

/// Library the embedded archive is linked from (see `write_packages_blob`).
const BLOB_LIB: &str = "tex_kpse_packages";
/// Member name of the object inside that library.
const BLOB_MEMBER: &str = "packages.o";

/// Streams the compressed chunks to disk: straight into the data section of a
/// one-member static library when the target has a native object format,
/// otherwise into a plain `packages.bin` for `include_bytes!`.
struct BlobWriter {
    file: std::io::BufWriter<std::fs::File>,
    format: Option<ObjectFormat>,
    /// Archive and object headers precede the data; their size is fixed, but
    /// their contents depend on the final length, so `finish` fills them in.
    header_len: usize,
    len: usize,
    /// Content fingerprint for the symbol name, so an archive with different
    /// bytes never satisfies code compiled against an older one.
    hash: u64,
}

impl BlobWriter {
    fn create(out: &Path) -> Self {
        let format = object_format();
        let name = format
            .as_ref()
            .map_or_else(|| "packages.bin".to_owned(), ObjectFormat::library_file);
        let mut file = std::io::BufWriter::new(std::fs::File::create(out.join(name)).unwrap());
        let header_len = format
            .as_ref()
            .map_or(0, |format| format.library(&blob_symbol(0), 0).0.len());
        file.write_all(&vec![0; header_len]).unwrap();
        BlobWriter { file, format, header_len, len: 0, hash: FNV_OFFSET }
    }

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

    fn finish(&mut self) {
        if let Some(format) = &self.format {
            let (header, trailer) = format.library(&blob_symbol(self.hash), self.len);
            assert_eq!(header.len(), self.header_len);
            self.file.write_all(&trailer).unwrap();
            self.file.seek(std::io::SeekFrom::Start(0)).unwrap();
            self.file.write_all(&header).unwrap();
        }
        self.file.flush().unwrap();
    }
}

fn blob_symbol(hash: u64) -> String {
    format!("tex_kpse_packages_{hash:016x}")
}

/// Native object format of the target, with the header fields a data-only
/// object needs. Linkers reject objects whose header disagrees with the code
/// objects, e.g. a RISC-V or LoongArch float ABI mismatch.
enum ObjectFormat {
    Elf { wide: bool, big_endian: bool, machine: u16, flags: u32 },
    MachO { cpu_type: u32, cpu_subtype: u32, platform: u32, min_os: u32 },
    Coff { machine: u16, x86: bool, msvc: bool },
}

/// `None` falls back to `include_bytes!` (e.g. wasm32, which has no native
/// object files, or an architecture without a known object header).
fn object_format() -> Option<ObjectFormat> {
    let cfg = |key: &str| std::env::var(format!("CARGO_CFG_TARGET_{key}")).unwrap_or_default();
    let arch = cfg("ARCH");
    let features = cfg("FEATURE");
    let feature = |name: &str| features.split(',').any(|feature| feature == name);
    if cfg("VENDOR") == "apple" {
        let (cpu_type, cpu_subtype) = match arch.as_str() {
            "x86_64" => (0x0100_0007, 3),
            // arm64e needs pointer-authentication subtype bits.
            "aarch64" if !std::env::var("TARGET").unwrap_or_default().starts_with("arm64e") => {
                (0x0100_000c, 0)
            }
            _ => return None,
        };
        let simulator = cfg("ABI") == "sim";
        // rustc's lowest deployment target per platform unless the usual
        // variable raises it; ld64 accepts objects built for an older OS.
        let (platform, variable, default) = match (cfg("OS").as_str(), simulator) {
            ("macos", _) => (1, "MACOSX_DEPLOYMENT_TARGET", if arch == "aarch64" { "11.0" } else { "10.12" }),
            ("ios", _) if cfg("ABI") == "macabi" => (6, "IPHONEOS_DEPLOYMENT_TARGET", "13.1"),
            ("ios", false) => (2, "IPHONEOS_DEPLOYMENT_TARGET", "10.0"),
            ("ios", true) => (7, "IPHONEOS_DEPLOYMENT_TARGET", "10.0"),
            ("tvos", false) => (3, "TVOS_DEPLOYMENT_TARGET", "10.0"),
            ("tvos", true) => (8, "TVOS_DEPLOYMENT_TARGET", "10.0"),
            ("watchos", false) => (4, "WATCHOS_DEPLOYMENT_TARGET", "5.0"),
            ("watchos", true) => (9, "WATCHOS_DEPLOYMENT_TARGET", "5.0"),
            ("visionos", false) => (11, "XROS_DEPLOYMENT_TARGET", "1.0"),
            ("visionos", true) => (12, "XROS_DEPLOYMENT_TARGET", "1.0"),
            _ => return None,
        };
        println!("cargo:rerun-if-env-changed={variable}");
        let version = std::env::var(variable).unwrap_or_else(|_| default.to_owned());
        let mut parts = version.split('.').map(|part| part.trim().parse::<u32>().unwrap_or(0));
        let mut min_os = 0;
        for (shift, max) in [(16, 0xffff), (8, 0xff), (0, 0xff)] {
            min_os |= parts.next().unwrap_or(0).min(max) << shift;
        }
        return Some(ObjectFormat::MachO { cpu_type, cpu_subtype, platform, min_os });
    }
    if cfg("OS") == "windows" {
        let machine = match arch.as_str() {
            "x86" => 0x014c,
            "x86_64" => 0x8664,
            "aarch64" => 0xaa64,
            "arm" => 0x01c4,
            _ => return None,
        };
        let msvc = cfg("ENV") == "msvc";
        return Some(ObjectFormat::Coff { machine, x86: arch == "x86", msvc });
    }
    if !cfg("FAMILY").split(',').any(|family| family == "unix") {
        return None;
    }
    let float_abi = |double, single, soft| {
        if feature("d") {
            double
        } else if feature("f") {
            single
        } else {
            soft
        }
    };
    let (machine, flags) = match arch.as_str() {
        "x86" => (3, 0),
        "x86_64" => (62, 0),
        "aarch64" => (183, 0),
        // EABI version 5. The float ABI lives in attributes a data object omits.
        "arm" => (40, 0x0500_0000),
        // RVC, float ABI and RVE flags.
        "riscv32" | "riscv64" => {
            (243, u32::from(feature("c")) | float_abi(4, 2, 0) | if feature("e") { 8 } else { 0 })
        }
        // Object ABI v1 plus the float ABI modifier.
        "loongarch64" => (258, 0x40 | float_abi(3, 2, 1)),
        _ => return None,
    };
    Some(ObjectFormat::Elf {
        wide: cfg("POINTER_WIDTH") == "64",
        big_endian: cfg("ENDIAN") == "big",
        machine,
        flags,
    })
}

impl ObjectFormat {
    /// File name the target's linker searches for `-l tex_kpse_packages`.
    fn library_file(&self) -> String {
        match self {
            ObjectFormat::Coff { msvc: true, .. } => format!("{BLOB_LIB}.lib"),
            _ => format!("lib{BLOB_LIB}.a"),
        }
    }

    /// Symbol name as stored in the object: Mach-O and 32-bit Windows prefix
    /// C names with an underscore.
    fn linker_symbol(&self, symbol: &str) -> String {
        match self {
            ObjectFormat::MachO { .. } | ObjectFormat::Coff { x86: true, .. } => format!("_{symbol}"),
            _ => symbol.to_owned(),
        }
    }

    /// Bytes before and after `len` blob bytes that form an `ar` library with
    /// one object defining `symbol` over exactly the blob. The header length
    /// depends on neither `len` nor the symbol's hash.
    fn library(&self, symbol: &str, len: usize) -> (Vec<u8>, Vec<u8>) {
        let symbol = self.linker_symbol(symbol);
        let (object_header, object_trailer) = match *self {
            ObjectFormat::Elf { wide, big_endian, machine, flags } => {
                elf_object(wide, big_endian, machine, flags, &symbol, len)
            }
            ObjectFormat::MachO { cpu_type, cpu_subtype, platform, min_os } => {
                macho_object(cpu_type, cpu_subtype, platform, min_os, &symbol, len)
            }
            ObjectFormat::Coff { machine, x86, .. } => coff_object(machine, x86, &symbol, len),
        };
        let object_len = object_header.len() + len + object_trailer.len();
        let bsd = matches!(self, ObjectFormat::MachO { .. });
        let mut header = ar_library_header(bsd, &symbol, object_len);
        header.extend_from_slice(&object_header);
        let mut trailer = object_trailer;
        if object_len % 2 == 1 {
            trailer.push(b'\n');
        }
        (header, trailer)
    }
}

/// Little- or big-endian field writer for object and archive headers.
struct Fields {
    bytes: Vec<u8>,
    big_endian: bool,
}

impl Fields {
    fn new(big_endian: bool) -> Self {
        Fields { bytes: Vec::new(), big_endian }
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// `bytes` NUL-padded to a fixed-width name field.
    fn name(&mut self, bytes: &[u8], width: usize) {
        assert!(bytes.len() <= width);
        self.raw(bytes);
        self.zeros(width - bytes.len());
    }

    fn zeros(&mut self, count: usize) {
        self.bytes.resize(self.bytes.len() + count, 0);
    }

    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u16(&mut self, value: u16) {
        let bytes = if self.big_endian { value.to_be_bytes() } else { value.to_le_bytes() };
        self.raw(&bytes);
    }

    fn u32(&mut self, value: usize) {
        let value = packed(value);
        let bytes = if self.big_endian { value.to_be_bytes() } else { value.to_le_bytes() };
        self.raw(&bytes);
    }

    fn u64(&mut self, value: usize) {
        let value = value as u64;
        let bytes = if self.big_endian { value.to_be_bytes() } else { value.to_le_bytes() };
        self.raw(&bytes);
    }

    /// ELF address, offset or size: 8 bytes in ELF64, 4 in ELF32.
    fn word(&mut self, wide: bool, value: usize) {
        if wide {
            self.u64(value)
        } else {
            self.u32(value)
        }
    }
}

fn align(offset: usize, alignment: usize) -> usize {
    offset.next_multiple_of(alignment)
}

/// ELF relocatable object: the blob in `.rodata.tex_kpse_packages`
/// (16-aligned), one hidden global object symbol over it, and an empty
/// `.note.GNU-stack` so GNU ld keeps the stack non-executable.
fn elf_object(
    wide: bool,
    big_endian: bool,
    machine: u16,
    flags: u32,
    symbol: &str,
    len: usize,
) -> (Vec<u8>, Vec<u8>) {
    const DATA: usize = 64;
    let data_end = DATA + len;
    let symbol_size = if wide { 24 } else { 16 };
    let symtab = align(data_end, 8);
    let strtab = symtab + 2 * symbol_size;
    let strtab_len = symbol.len() + 2;
    let section_names = [".rodata.tex_kpse_packages", ".note.GNU-stack", ".symtab", ".strtab", ".shstrtab"];
    let shstrtab = strtab + strtab_len;
    let mut name_offsets = Vec::new();
    let mut shstrtab_bytes = vec![0];
    for name in section_names {
        name_offsets.push(shstrtab_bytes.len());
        shstrtab_bytes.extend_from_slice(name.as_bytes());
        shstrtab_bytes.push(0);
    }
    let section_headers = align(shstrtab + shstrtab_bytes.len(), 8);

    let mut header = Fields::new(big_endian);
    header.raw(&[0x7f, b'E', b'L', b'F', if wide { 2 } else { 1 }, if big_endian { 2 } else { 1 }, 1]);
    header.zeros(9);
    header.u16(1); // ET_REL
    header.u16(machine);
    header.u32(1);
    header.word(wide, 0); // e_entry
    header.word(wide, 0); // e_phoff
    header.word(wide, section_headers);
    header.u32(flags as usize);
    header.u16(if wide { 64 } else { 52 });
    header.u16(0);
    header.u16(0);
    header.u16(if wide { 64 } else { 40 });
    header.u16(1 + section_names.len() as u16);
    header.u16(section_names.len() as u16); // .shstrtab is the last section
    header.zeros(DATA - header.bytes.len());

    let mut trailer = Fields::new(big_endian);
    trailer.zeros(symtab - data_end);
    trailer.zeros(symbol_size);
    // st_info STB_GLOBAL|STT_OBJECT, st_other STV_HIDDEN, st_shndx 1.
    if wide {
        trailer.u32(1);
        trailer.u8(0x11);
        trailer.u8(2);
        trailer.u16(1);
        trailer.u64(0);
        trailer.u64(len);
    } else {
        trailer.u32(1);
        trailer.u32(0);
        trailer.u32(len);
        trailer.u8(0x11);
        trailer.u8(2);
        trailer.u16(1);
    }
    trailer.u8(0);
    trailer.raw(symbol.as_bytes());
    trailer.u8(0);
    trailer.raw(&shstrtab_bytes);
    trailer.zeros(section_headers - (shstrtab + shstrtab_bytes.len()));
    // name, type, flags, offset, size, link, info, alignment, entry size
    let sections = [
        (name_offsets[0], 1, 2, DATA, len, 0, 0, 16, 0), // PROGBITS, SHF_ALLOC
        (name_offsets[1], 1, 0, data_end, 0, 0, 0, 1, 0),
        (name_offsets[2], 2, 0, symtab, 2 * symbol_size, 4, 1, 8, symbol_size), // SYMTAB
        (name_offsets[3], 3, 0, strtab, strtab_len, 0, 0, 1, 0), // STRTAB
        (name_offsets[4], 3, 0, shstrtab, shstrtab_bytes.len(), 0, 0, 1, 0),
    ];
    trailer.zeros(if wide { 64 } else { 40 });
    for (name, kind, flags, offset, size, link, info, alignment, entry_size) in sections {
        trailer.u32(name);
        trailer.u32(kind);
        trailer.word(wide, flags);
        trailer.word(wide, 0);
        trailer.word(wide, offset);
        trailer.word(wide, size);
        trailer.u32(link);
        trailer.u32(info);
        trailer.word(wide, alignment);
        trailer.word(wide, entry_size);
    }
    (header.bytes, trailer.bytes)
}

/// Mach-O 64-bit object with one private-extern symbol. The blob keeps the
/// `__TEXT,__tex_kpse_blob` section of the former `.incbin`: arm64 relocations
/// to assembler-temporary labels are encoded relative to the preceding symbol
/// in the same section, and a 600+ MiB neighbor would overflow the 24-bit
/// ARM64_RELOC_ADDEND field.
fn macho_object(
    cpu_type: u32,
    cpu_subtype: u32,
    platform: u32,
    min_os: u32,
    symbol: &str,
    len: usize,
) -> (Vec<u8>, Vec<u8>) {
    // LC_SEGMENT_64 with one section, LC_BUILD_VERSION, LC_SYMTAB, LC_DYSYMTAB
    const COMMANDS: usize = 152 + 24 + 24 + 80;
    let data = align(32 + COMMANDS, 16);
    let symtab = align(data + len, 8);
    let strtab = symtab + 16;
    let strtab_len = align(symbol.len() + 2, 8);

    let mut header = Fields::new(false);
    for value in [0xfeed_facf, cpu_type as usize, cpu_subtype as usize, 1, 4, COMMANDS] {
        header.u32(value); // magic, CPU, MH_OBJECT, command count and size
    }
    header.u32(0x2000); // MH_SUBSECTIONS_VIA_SYMBOLS
    header.u32(0);
    header.u32(0x19);
    header.u32(152);
    header.zeros(16);
    for value in [0, len, data, len] {
        header.u64(value); // vmaddr, vmsize, fileoff, filesize
    }
    for value in [7, 7, 1, 0] {
        header.u32(value); // maxprot, initprot, nsects, flags
    }
    header.name(b"__tex_kpse_blob", 16);
    header.name(b"__TEXT", 16);
    header.u64(0);
    header.u64(len);
    header.u32(data);
    header.u32(4); // 2^4 alignment
    header.zeros(24); // no relocations, S_REGULAR, reserved
    for value in [0x32, 24, platform as usize, min_os as usize, 0, 0] {
        header.u32(value); // LC_BUILD_VERSION, no SDK, no tools
    }
    for value in [2, 24, symtab, 1, strtab, strtab_len] {
        header.u32(value); // LC_SYMTAB
    }
    for value in [0xb, 80, 0, 0, 0, 1, 1, 0] {
        header.u32(value); // LC_DYSYMTAB: one external defined symbol
    }
    header.zeros(48);
    header.zeros(data - header.bytes.len());

    let mut trailer = Fields::new(false);
    trailer.zeros(symtab - (data + len));
    trailer.u32(1);
    trailer.u8(0x1f); // N_SECT | N_EXT | N_PEXT
    trailer.u8(1);
    trailer.u16(0);
    trailer.u64(0);
    trailer.u8(0);
    trailer.raw(symbol.as_bytes());
    trailer.zeros(strtab_len - symbol.len() - 1);
    (header.bytes, trailer.bytes)
}

/// COFF object. `$` grouping links the `.rdata$` section into `.rdata`.
/// 32-bit x86 also gets `@feat.00` = 1 so `/SAFESEH` links accept the object.
fn coff_object(machine: u16, x86: bool, symbol: &str, len: usize) -> (Vec<u8>, Vec<u8>) {
    const DATA: usize = 64;
    let section_name = b".rdata$tex_kpse_packages";
    let symbol_name = 4 + section_name.len() + 1;

    let mut header = Fields::new(false);
    header.u16(machine);
    header.u16(1);
    header.u32(0);
    header.u32(DATA + len); // symbol table
    header.u32(if x86 { 2 } else { 1 });
    header.u16(0);
    header.u16(0);
    header.name(b"/4", 8); // long name at string table offset 4
    header.u32(0);
    header.u32(0);
    header.u32(len);
    header.u32(DATA);
    header.zeros(12); // no relocations or line numbers
    header.u32(0x4050_0040); // INITIALIZED_DATA | ALIGN_16BYTES | MEM_READ
    header.zeros(DATA - header.bytes.len());

    let mut trailer = Fields::new(false);
    if x86 {
        trailer.name(b"@feat.00", 8);
        trailer.u32(1);
        trailer.u16(0xffff); // IMAGE_SYM_ABSOLUTE
        trailer.u16(0);
        trailer.u8(3); // IMAGE_SYM_CLASS_STATIC
        trailer.u8(0);
    }
    trailer.u32(0);
    trailer.u32(symbol_name);
    trailer.u32(0);
    trailer.u16(1);
    trailer.u16(0);
    trailer.u8(2); // IMAGE_SYM_CLASS_EXTERNAL
    trailer.u8(0);
    trailer.u32(symbol_name + symbol.len() + 1);
    trailer.raw(section_name);
    trailer.u8(0);
    trailer.raw(symbol.as_bytes());
    trailer.u8(0);
    (header.bytes, trailer.bytes)
}

/// `ar` signature, symbol index and member header of a library whose only
/// member is an `object_len`-byte object defining `symbol`. Linkers need the
/// index to pull the member. GNU ld, lld and MSVC link.exe read a GNU/SysV
/// `/` index; Apple linkers read a BSD `__.SYMDEF`, which (like the member
/// name) is stored as a `#1/` name in front of the member data, as llvm-ar and
/// Apple's ar write it. Padding makes the object start 8-aligned.
fn ar_library_header(bsd: bool, symbol: &str, object_len: usize) -> Vec<u8> {
    let mut library = Fields::new(!bsd);
    library.raw(b"!<arch>\n");
    if bsd {
        const INDEX_NAME: &[u8; 12] = b"__.SYMDEF\0\0\0";
        const MEMBER_NAME_LEN: usize = 16;
        // The strings end where member header + name leave the object 8-aligned.
        let strings = align(symbol.len() + 1, 8) + 4;
        let index_len = INDEX_NAME.len() + 16 + strings;
        let member = 8 + 60 + index_len;
        ar_member_header(&mut library, &format!("#1/{}", INDEX_NAME.len()), index_len);
        library.raw(INDEX_NAME);
        // ranlib entries size, {name offset, member offset}, names size
        for value in [8, 0, member, strings] {
            library.u32(value);
        }
        library.name(symbol.as_bytes(), strings);
        let name = format!("#1/{MEMBER_NAME_LEN}");
        ar_member_header(&mut library, &name, MEMBER_NAME_LEN + object_len);
        library.name(BLOB_MEMBER.as_bytes(), MEMBER_NAME_LEN);
    } else {
        let index_len = align(8 + symbol.len() + 1, 8);
        let member = 8 + 60 + index_len;
        ar_member_header(&mut library, "/", index_len);
        library.u32(1);
        library.u32(member);
        library.name(symbol.as_bytes(), index_len - 8);
        ar_member_header(&mut library, &format!("{BLOB_MEMBER}/"), object_len);
    }
    assert_eq!(library.bytes.len() % 8, 0);
    library.bytes
}

fn ar_member_header(library: &mut Fields, name: &str, size: usize) {
    // name, mtime, uid, gid, mode, size, terminator
    library.raw(format!("{name:<16}{:<12}{:<6}{:<6}{:<8}{size:<10}`\n", 0, 0, 0, 644).as_bytes());
}

/// Emit `PACKAGES`. rustc copies an `include_bytes!` static through const
/// evaluation, crate metadata, LLVM IR, bitcode and (with ThinLTO) every
/// module importing it, which needs tens of GiB for a 600+ MiB archive, and
/// LTO re-assembles even a `global_asm!` `.incbin`. The library written by
/// `BlobWriter` reaches only the linker.
///
/// By default the library stays in `OUT_DIR` (`-bundle`): rustc would
/// otherwise copy it into the rlib and, under LTO, copy it once more into a
/// temporary archive at the final link. A static library built from this
/// crate (libtex) needs the `bundle-packages` feature, or its consumers
/// would have to link `tex_kpse_packages` themselves.
fn write_packages_blob(generated: &mut impl Write, blob: &BlobWriter, out: &Path) {
    if blob.format.is_none() {
        writeln!(
            generated,
            "static PACKAGES: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/packages.bin\"));"
        )
        .unwrap();
        return;
    }
    let bundle = if std::env::var_os("CARGO_FEATURE_BUNDLE_PACKAGES").is_some() { "" } else { ":-bundle" };
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static{bundle}={BLOB_LIB}");
    let symbol = blob_symbol(blob.hash);
    let len = blob.len;
    writeln!(
        generated,
        "extern \"C\" {{\n    #[link_name = \"{symbol}\"]\n    static PACKAGES_BLOB: [u8; {len}];\n}}\n\
         // SAFETY: build.rs's {BLOB_LIB} library defines this symbol as exactly\n\
         // {len} bytes in a read-only section that nothing mutates.\n\
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
