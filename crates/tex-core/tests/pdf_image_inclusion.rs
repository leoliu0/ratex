//! pdfTeX image inclusion, image parameters, page groups, ProcSets and
//! \pdfsnapy. Every expectation is what `pdftex -ini` (TeX Live 2026,
//! pdfTeX 1.40.29) writes for the same input and fixture files.
use std::io::Write;
use tex_core::engine::Engine;

fn fixture_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pdf-image-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A PDF with a correct xref table.
fn pdf_file(version: &str, objects: &[&[u8]], trailer_extra: &str) -> Vec<u8> {
    let mut out = format!("%PDF-{version}\n").into_bytes();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R{trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Two pages: page 1 with CropBox, ArtBox, Resources and a /Group; page 2
/// rotated by 90 degrees without Resources, also reachable as `second`.
fn fixture_pdf(version: &str) -> Vec<u8> {
    let content = b"0 0 1 rg 10 20 100 50 re f";
    let stream = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    pdf_file(
        version,
        &[
            b"<< /Type /Catalog /Pages 2 0 R /Dests << /second [6 0 R /Fit] >> >>",
            b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 /MediaBox [0 0 200 100] >>",
            b"<< /Type /Page /Parent 2 0 R /CropBox [10 20 110.5 70.25] /ArtBox [0 0 50 40] /Contents 4 0 R /Resources << /ProcSet [/PDF] >> /Group << /S /Transparency /CS /DeviceRGB >> >>",
            &stream,
            b"<< /Producer (probe) >>",
            b"<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents 4 0 R >>",
        ],
        " /Info 5 0 R",
    )
}

fn png_chunk(kind: &[u8; 4], data: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

fn png(width: u32, depth: u8, color: u8, rows: &[&[u8]], extra: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&(rows.len() as u32).to_be_bytes());
    header.extend_from_slice(&[depth, color, 0, 0, 0]);
    png_chunk(b"IHDR", &header, &mut out);
    for (kind, data) in extra {
        png_chunk(kind, data, &mut out);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    for row in rows {
        z.write_all(&[0]).unwrap();
        z.write_all(row).unwrap();
    }
    png_chunk(b"IDAT", &z.finish().unwrap(), &mut out);
    png_chunk(b"IEND", &[], &mut out);
    out
}

fn run(dir: &std::path::Path, source: &str) -> Engine {
    let mut e = Engine::new(true);
    e.init_primitives();
    e.add_nullfont();
    e.main_dir = Some(dir.to_path_buf());
    let source = format!("\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6 \\def\\space{{ }}\n{source}");
    e.input.push_file("probe.tex".into(), source.into_bytes());
    e.run();
    e
}

fn finish(e: &mut Engine) -> lopdf::Document {
    let bytes = tex_core::driver::finish_pdf(e, false).expect("PDF finalization");
    lopdf::Document::load_mem(&bytes).expect("valid PDF")
}

fn page_lines(doc: &lopdf::Document, page: u32, suffix: &str) -> Vec<String> {
    let id = doc.get_pages()[&page];
    String::from_utf8(doc.get_page_content(id))
        .unwrap()
        .lines()
        .filter(|line| line.ends_with(suffix))
        .map(str::to_owned)
        .collect()
}

fn object_text(e: &Engine, needle: &str) -> String {
    e.pdf_doc
        .objects
        .iter()
        .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
        .find(|text| text.contains(needle))
        .unwrap_or_else(|| panic!("no object with {needle}"))
}

/// Undo PNG row predictors (`bpp` bytes per pixel).
fn unpredict(data: &[u8], row_bytes: usize, bpp: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut previous = vec![0u8; row_bytes];
    for row in data.chunks_exact(row_bytes + 1) {
        let mut current = vec![0u8; row_bytes];
        for i in 0..row_bytes {
            let left = if i >= bpp { current[i - bpp] } else { 0 };
            let up = previous[i];
            let corner = if i >= bpp { previous[i - bpp] } else { 0 };
            let predicted = match row[0] {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                _ => {
                    let p = i16::from(left) + i16::from(up) - i16::from(corner);
                    let pa = (p - i16::from(left)).abs();
                    let pb = (p - i16::from(up)).abs();
                    let pc = (p - i16::from(corner)).abs();
                    if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        corner
                    }
                }
            };
            current[i] = row[i + 1].wrapping_add(predicted);
        }
        out.extend_from_slice(&current);
        previous = current;
    }
    out
}

/// (bits per component, samples widened to 8 bits unless 16) of an image.
fn samples(doc: &lopdf::Document, stream: &lopdf::Stream) -> (i64, Vec<u16>) {
    let width = stream.dict.get(b"Width").unwrap().as_i64().unwrap() as usize;
    let bits = stream.dict.get(b"BitsPerComponent").unwrap().as_i64().unwrap();
    let colors = match doc.dereference(stream.dict.get(b"ColorSpace").unwrap()).unwrap().1 {
        lopdf::Object::Name(name) if name == b"DeviceRGB" => 3,
        _ => 1,
    };
    let mut data = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&stream.content[..]), &mut data).unwrap();
    // every row carries a PNG predictor byte (writepng.c and texres alike)
    let row_bytes = (width * colors * bits as usize).div_ceil(8);
    let bpp = (colors * bits as usize).div_ceil(8);
    let raw = unpredict(&data, row_bytes, bpp);
    let values = match bits {
        16 => raw.chunks(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect(),
        8 => raw.iter().map(|&b| u16::from(b)).collect(),
        _ => {
            let per = 8 / bits as usize;
            (0..width * colors)
                .map(|i| {
                    let v = (raw[i / per] >> (8 - bits as usize * (i % per + 1))) & ((1 << bits) - 1);
                    u16::from(v) * 255 / ((1 << bits) - 1)
                })
                .collect()
        }
    };
    (if bits == 16 { 16 } else { 8 }, values)
}

#[test]
fn included_pdf_page_is_written_like_pdftex_write_epdf() {
    let dir = fixture_dir("include");
    std::fs::write(dir.join("fig.pdf"), fixture_pdf("1.4")).unwrap();
    let mut e = run(
        &dir,
        r"\pdfcompresslevel=0 \pdfminorversion=4
\pdfhorigin=0pt \pdfvorigin=0pt \pdfpagewidth=300pt \pdfpageheight=300pt
\pdfximage width 50pt attr{/Group <</S/Transparency/K false/I false>>} {fig.pdf}
\setbox0\hbox{\pdfrefximage\pdflastximage}
\dimen0=\pdfximagebbox\pdflastximage 1 \dimen1=\pdfximagebbox\pdflastximage 2
\dimen2=\pdfximagebbox\pdflastximage 3 \dimen3=\pdfximagebbox\pdflastximage 4
\message{W=\the\wd0 H=\the\ht0 D=\the\dp0 B=\the\dimen0 \the\dimen1 \the\dimen2 \the\dimen3}
\shipout\box0
\pdfximage artbox {fig.pdf}\setbox0\hbox{\pdfrefximage\pdflastximage}
\message{ART W=\the\wd0 H=\the\ht0}
\pdfximage named {second} {fig.pdf}\setbox0\hbox{\pdfrefximage\pdflastximage}
\message{ROT W=\the\wd0 H=\the\ht0}
\shipout\box0
\end",
    );
    assert_eq!(e.error_count, 0, "{}", e.term);
    for expected in [
        "W=50.0ptH=25.0ptD=0.0ptB=10.0375pt20.075pt110.91438pt70.51343pt",
        "ART W=50.1875ptH=40.15pt",
        "ROT W=100.375ptH=200.75pt",
        "PDF inclusion: /Resources missing. 'This practice is not recommended' (PDF Ref)",
    ] {
        assert!(e.log.contains(expected), "missing {expected}:\n{}", e.log);
    }
    // the page group comes from the included page: the form refers to it
    let group = e.pdf_doc.pages[0].group;
    assert!(group > 0);
    let form = object_text(&e, "/PTEX.PageNumber 1");
    assert!(
        form.starts_with(&format!(
            "<< /Group <</S/Transparency/K false/I false>> /Type /XObject /Subtype /Form /FormType 1 /PTEX.FileName (./fig.pdf) /PTEX.PageNumber 1 /PTEX.InfoDict "
        )),
        "{form}"
    );
    // the user's /Group replaces the copied one: a dictionary repeats no key
    assert_eq!(form.matches("/Group").count(), 1, "{form}");
    assert!(form.contains("/BBox [10 20 110.5 70.25] /Resources"), "{form}");
    assert!(!form.contains("/Matrix"), "{form}");
    let rotated = object_text(&e, "/PTEX.PageNumber 2");
    assert!(rotated.contains("/Matrix [0 -1 1 0 0 200] /BBox [0 0 200 100]"), "{rotated}");
    let doc = finish(&mut e);
    assert_eq!(page_lines(&doc, 1, " cm"), ["0.495654 0 0 0.495654 -4.957 264.06 cm"]);
    assert_eq!(page_lines(&doc, 2, " cm"), ["1 0 0 1 0 98.879 cm"]);
    let page = doc.get_dictionary(doc.get_pages()[&1]).unwrap();
    let group = doc.get_dictionary(page.get(b"Group").unwrap().as_reference().unwrap()).unwrap();
    assert_eq!(group.get(b"CS").unwrap().as_name().unwrap(), b"DeviceRGB");
    // pdfTeX: no /Text without fonts, no image procsets for PDF images
    let resources = page.get(b"Resources").unwrap().as_dict().unwrap();
    assert_eq!(resources.get(b"ProcSet").unwrap().as_array().unwrap(), &vec![lopdf::Object::Name(b"PDF".to_vec())]);
    assert!(resources.get(b"Font").is_err());
}

#[test]
fn pdf_inclusion_parameters_select_boxes_and_version_checks_like_pdftex() {
    let dir = fixture_dir("pagebox");
    std::fs::write(dir.join("fig.pdf"), fixture_pdf("1.4")).unwrap();
    std::fs::write(dir.join("fig17.pdf"), fixture_pdf("1.7")).unwrap();
    let e = run(
        &dir,
        r"\pdfminorversion=5
\def\probe#1{\setbox0\hbox{\pdfrefximage\pdflastximage}\message{#1 W=\the\wd0 H=\the\ht0}}
\pdfpagebox=1 \pdfximage{fig.pdf}\probe{MEDIA}
\pdfpagebox=5 \pdfximage cropbox {fig.pdf}\probe{KEYWORD}
\pdfforcepagebox=5 \pdfximage cropbox {fig.pdf}\probe{FORCE}
\pdfforcepagebox=0 \pdfoptionalwaysusepdfpagebox=1 \pdfximage{fig.pdf}\probe{ALWAYS}
\message{AFTER force=\the\pdfforcepagebox\space always=\the\pdfoptionalwaysusepdfpagebox}
\pdfforcepagebox=0 \pdfpagebox=0
\pdfximage{fig17.pdf}\probe{V17}
\pdfinclusionerrorlevel=-1 \pdfximage{fig17.pdf}\probe{QUIET}
\pdfoptionpdfinclusionerrorlevel=-1 \pdfximage{fig17.pdf}\probe{OPT}
\message{AFTER level=\the\pdfinclusionerrorlevel\space opt=\the\pdfoptionpdfinclusionerrorlevel}
\end",
    );
    assert_eq!(e.error_count, 0, "{}", e.term);
    let log = &e.log;
    let mut at = 0;
    for expected in [
        "MEDIA W=200.75ptH=100.375pt",
        "KEYWORD W=100.87688ptH=50.43843pt",
        "Primitive \\pdfforcepagebox is obsolete; use \\pdfpagebox instead.",
        "FORCE W=50.1875ptH=40.15pt",
        "Primitive \\pdfoptionalwaysusepdfpagebox is obsolete; use \\pdfpagebox instead.",
        "ALWAYS W=200.75ptH=100.375pt",
        "AFTER force=1 always=0",
        "PDF inclusion: found PDF version <1.7>, but at most version <1.5> allowed",
        "V17 W=100.87688ptH=50.43843pt",
        "QUIET W=100.87688ptH=50.43843pt",
        "Primitive \\pdfoptionpdfinclusionerrorlevel is obsolete; use \\pdfinclusionerrorlevel instead.",
        "OPT W=100.87688ptH=50.43843pt",
        "AFTER level=-1 opt=0",
    ] {
        let found = log[at..].find(expected).unwrap_or_else(|| panic!("missing {expected} after {at}:\n{log}"));
        at += found + expected.len();
    }
    // the version warning appears once: \pdfinclusionerrorlevel=-1 is silent
    assert_eq!(log.matches("found PDF version").count(), 1, "{log}");
    // the obsolete force warning is not repeated after the always warning
    assert_eq!(log.matches("\\pdfforcepagebox is obsolete").count(), 1, "{log}");

    let e = run(&dir, r"\pdfminorversion=5 \pdfinclusionerrorlevel=1 \pdfximage{fig17.pdf}\message{REACHED}\end");
    assert!(e.stopped_on_error, "a newer PDF must be fatal at level 1");
    assert!(e.log.contains("found PDF version <1.7>, but at most version <1.5> allowed"), "{}", e.log);
}

#[test]
fn ptex_keys_honor_underscore_and_suppression() {
    let dir = fixture_dir("ptex");
    std::fs::write(dir.join("fig.pdf"), fixture_pdf("1.4")).unwrap();
    let e = run(
        &dir,
        r"\pdfptexuseunderscore=1 \pdfsuppressptexinfo=8 \pdfximage{fig.pdf}\shipout\hbox{\pdfrefximage\pdflastximage}\end",
    );
    let form = object_text(&e, "/Subtype /Form");
    assert!(form.contains("/PTEX_FileName (./fig.pdf) /PTEX_PageNumber 1 /BBox"), "{form}");
    assert!(!form.contains("InfoDict") && !form.contains("PTEX."), "{form}");
}

#[test]
fn raster_resolution_procset_and_alpha_page_group_follow_pdftex() {
    let dir = fixture_dir("raster");
    std::fs::write(dir.join("gray.png"), png(2, 8, 0, &[&[64, 200]], &[])).unwrap();
    let mut phys = 7874u32.to_be_bytes().to_vec();
    phys.extend_from_slice(&3937u32.to_be_bytes());
    phys.push(1);
    let rgb = png(3, 8, 2, &[&[0; 9], &[0; 9]], &[(b"pHYs", phys)]);
    std::fs::write(dir.join("res.png"), rgb).unwrap();
    std::fs::write(dir.join("rgba.png"), png(1, 8, 6, &[&[10, 20, 30, 128]], &[])).unwrap();
    std::fs::write(dir.join("pal.png"), png(1, 8, 3, &[&[0]], &[(b"PLTE", vec![1, 2, 3])])).unwrap();
    let mut e = run(
        &dir,
        r"\pdfminorversion=5 \pdfhorigin=0pt \pdfvorigin=0pt \pdfpagewidth=300pt \pdfpageheight=300pt
\pdfimageresolution=144
\pdfximage{gray.png}\setbox0\hbox{\pdfrefximage\pdflastximage}\message{GRAY W=\the\wd0 H=\the\ht0}
\pdfximage{res.png}\setbox2\hbox{\pdfrefximage\pdflastximage}\message{RES W=\the\wd2 H=\the\ht2}
\pdfimageresolution=0
\pdfximage depth 1pt {gray.png}\setbox4\hbox{\pdfrefximage\pdflastximage}\message{D W=\the\wd4 H=\the\ht4 D=\the\dp4}
\pdfximage{rgba.png}\setbox8\hbox{\pdfrefximage\pdflastximage}
\pdfximage{pal.png}\setbox10\hbox{\pdfrefximage\pdflastximage}
\shipout\hbox{\box0\box2\box4\box8\box10}
\end",
    );
    assert_eq!(e.error_count, 0, "{}", e.term);
    for expected in [
        "GRAY W=1.00375ptH=0.50188pt",
        "RES W=1.08405ptH=1.4454pt",
        "D W=2.0075ptH=0.00375ptD=1.0pt",
    ] {
        assert!(e.log.contains(expected), "missing {expected}:\n{}", e.log);
    }
    let doc = finish(&mut e);
    assert_eq!(
        page_lines(&doc, 1, " cm"),
        [
            "1 0 0 0.5 0 297.439 cm",
            "1.08 0 0 1.44 1 297.439 cm",
            "2 0 0 1 2.08 296.443 cm",
            "1 0 0 1 4.08 297.439 cm",
            "1 0 0 1 5.08 297.439 cm",
        ]
    );
    let page = doc.get_dictionary(doc.get_pages()[&1]).unwrap();
    let procset: Vec<&[u8]> = page
        .get(b"Resources")
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"ProcSet")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_name().unwrap())
        .collect();
    assert_eq!(procset, [&b"PDF"[..], b"ImageB", b"ImageC", b"ImageI"]);
    let group = doc.get_dictionary(page.get(b"Group").unwrap().as_reference().unwrap()).unwrap();
    assert_eq!(group.get(b"S").unwrap().as_name().unwrap(), b"Transparency");
    assert_eq!(group.get(b"CS").unwrap().as_name().unwrap(), b"DeviceRGB");
    assert!(group.get(b"I").unwrap().as_bool().unwrap());
}

/// Per PDF version and \pdfimage* setting: the samples pdfTeX writes for an
/// 8-bit gray, 16-bit gray, 2-bit gray, 16-bit gray+alpha, gAMA 1.0 gray and
/// RGBA PNG (soft masks after the `|`).
#[test]
fn png_hicolor_alpha_and_gamma_samples_match_pdftex() {
    let dir = fixture_dir("gamma");
    std::fs::write(dir.join("g8.png"), png(4, 8, 0, &[&[0, 64, 128, 250]], &[])).unwrap();
    std::fs::write(dir.join("g16.png"), png(2, 16, 0, &[&[0x12, 0x34, 0xC0, 0x00]], &[])).unwrap();
    std::fs::write(dir.join("g2.png"), png(4, 2, 0, &[&[0b0001_1011]], &[])).unwrap();
    std::fs::write(dir.join("ga16.png"), png(1, 16, 4, &[&[0x80, 0x00, 0x43, 0x21]], &[])).unwrap();
    let gamma = png(4, 8, 0, &[&[0, 64, 128, 250]], &[(b"gAMA", 100000u32.to_be_bytes().to_vec())]);
    std::fs::write(dir.join("gamma.png"), gamma).unwrap();
    std::fs::write(dir.join("rgba.png"), png(1, 8, 6, &[&[10, 20, 30, 128]], &[])).unwrap();
    type Expected = [(i64, &'static [u16], Option<u16>); 6];
    let cases: [(&str, bool, Expected); 4] = [
        (
            r"\pdfminorversion=5",
            true,
            [
                (8, &[0, 64, 128, 250], None),
                (16, &[0x1234, 0xC000], None),
                (8, &[0, 85, 170, 255], None),
                (16, &[0x8000], Some(0x43)),
                (8, &[0, 64, 128, 250], None),
                (8, &[10, 20, 30], Some(128)),
            ],
        ),
        (
            r"\pdfminorversion=3",
            false,
            [
                (8, &[0, 64, 128, 250], None),
                (8, &[0x12, 0xC0], None),
                (8, &[0, 85, 170, 255], None),
                (8, &[0x80], None),
                (8, &[0, 64, 128, 250], None),
                (8, &[10, 20, 30], None),
            ],
        ),
        (
            r"\pdfminorversion=5 \pdfimageapplygamma=1 \pdfgamma=1000",
            true,
            [
                (8, &[0x00, 0x0c, 0x38, 0xf4], None),
                (16, &[0x00c3, 0x87f4], None),
                (8, &[0, 0, 85, 255], None),
                (16, &[0x37b8], Some(0x43)),
                (8, &[0, 64, 128, 250], None),
                (8, &[0, 1, 2], Some(128)),
            ],
        ),
        (
            r"\pdfminorversion=5 \pdfimageapplygamma=1 \pdfgamma=1500 \pdfimagehicolor=0",
            true,
            [
                (8, &[0x00, 0x22, 0x5d, 0xf8], None),
                (8, &[0x05, 0xa7], None),
                (8, &[0, 0, 170, 255], None),
                (8, &[0x5c], Some(0x43)),
                (8, &[0x00, 0x65, 0xa1, 0xfc], None),
                (8, &[2, 6, 11], Some(128)),
            ],
        ),
    ];
    for (setup, page_group, expected) in cases {
        let mut e = run(
            &dir,
            &format!(
                r"{setup}
\pdfximage{{g8.png}}\pdfrefximage\pdflastximage \pdfximage{{g16.png}}\pdfrefximage\pdflastximage
\pdfximage{{g2.png}}\pdfrefximage\pdflastximage \pdfximage{{ga16.png}}\pdfrefximage\pdflastximage
\pdfximage{{gamma.png}}\pdfrefximage\pdflastximage \pdfximage{{rgba.png}}\pdfrefximage\pdflastximage
\shipout\box255 \end"
            ),
        );
        assert_eq!(e.error_count, 0, "{setup}: {}", e.term);
        let doc = finish(&mut e);
        let page = doc.get_dictionary(doc.get_pages()[&1]).unwrap();
        assert_eq!(page.get(b"Group").is_ok(), page_group, "{setup}");
        let xobjects = page.get(b"Resources").unwrap().as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap();
        let mut images: Vec<(u32, &lopdf::Stream)> = xobjects
            .iter()
            .map(|(_, r)| {
                let id = r.as_reference().unwrap();
                (id.0, doc.get_object(id).unwrap().as_stream().unwrap())
            })
            .collect();
        // object numbers follow the \pdfximage order
        images.sort_by_key(|(id, _)| *id);
        assert_eq!(images.len(), expected.len());
        for ((_, stream), (bits, values, mask)) in images.iter().zip(expected) {
            assert_eq!(samples(&doc, stream), (bits, values.to_vec()), "{setup}");
            let smask = stream.dict.get(b"SMask").ok().map(|r| {
                let mask = doc.get_object(r.as_reference().unwrap()).unwrap().as_stream().unwrap();
                samples(&doc, mask).1[0]
            });
            assert_eq!(smask, mask, "{setup}");
        }
    }
}

#[test]
fn form_procset_uses_pdfomitprocset_when_the_form_is_written() {
    let dir = fixture_dir("formprocset");
    std::fs::write(dir.join("pal.png"), png(1, 8, 3, &[&[0]], &[(b"PLTE", vec![1, 2, 3])])).unwrap();
    std::fs::write(dir.join("gray.png"), png(2, 8, 0, &[&[64, 200]], &[])).unwrap();
    let mut e = run(
        &dir,
        r"\pdfximage{pal.png}\setbox0\hbox{\pdfrefximage\pdflastximage}\pdfxform0 \count1=\pdflastxform
\pdfomitprocset=1 \setbox2\hbox{\pdfrefxform\count1}\shipout\box2
\pdfomitprocset=-1 \pdfximage{gray.png}\setbox0\hbox{\pdfrefximage\pdflastximage}\pdfxform0 \count2=\pdflastxform
\shipout\hbox{\pdfrefxform\count2}
\end",
    );
    assert_eq!(e.error_count, 0, "{}", e.term);
    let first = e.eqtb.count[1];
    let second = e.eqtb.count[2];
    finish(&mut e);
    let form = |obj: i32| {
        let (_, bytes) = e.pdf_doc.objects.iter().find(|(n, _)| *n == obj).unwrap();
        String::from_utf8_lossy(bytes).into_owned()
    };
    // pdfTeX writes the first form with page 1, under \pdfomitprocset=1
    assert!(!form(first).contains("/ProcSet"), "{}", form(first));
    assert!(form(second).contains("/ProcSet [ /PDF /ImageB ]"), "{}", form(second));
    assert!(!e.pdf_doc.pages[0].procset);
    assert!(e.pdf_doc.pages[1].procset);
}

/// \pdfsnaprefpoint, \pdfsnapy and \pdfsnapycomp move the vertical position
/// in a shipped vlist; snap nodes at the top of a page are discarded with
/// pdfTeX's message.
#[test]
fn snap_nodes_position_vlists_and_vanish_at_page_tops_like_pdftex() {
    let dir = fixture_dir("snap");
    let mut e = run(
        &dir,
        r"\pdfcompresslevel=0 \pdfpagewidth=100pt \pdfpageheight=100pt \pdfhorigin=0pt \pdfvorigin=0pt
\shipout\vbox to 60pt{\hrule height 1pt width 5pt\kern 2pt\pdfsnaprefpoint\hrule height 3pt width 10pt\kern 4pt\pdfsnapy 10pt plus 5pt minus 5pt\hrule height 1pt width 10pt\pdfsnapycomp 500 \vskip 2pt plus 1fil\pdfsnapy 10pt plus 9pt\hrule height 2pt width 10pt\kern 6pt\pdfsnapy 10pt minus 3pt\hrule height 1pt width 7pt}
\vsize=50pt \maxdepth=2pt \output={\shipout\box255}
\pdfsnapy 5pt\pdfsnapycomp 300\hrule height 1pt\pdfsnapy 6pt plus 1fil\hrule height 1pt
\penalty-10000
\end",
    );
    assert_eq!(e.error_count, 0, "{}", e.term);
    assert!(e.log.contains("snap node being discardedsnap node being discarded"), "{}", e.log);
    let doc = finish(&mut e);
    assert_eq!(
        page_lines(&doc, 1, " cm"),
        [
            "1 0 0 1 0 99.128 cm",
            "1 0 0 1 0 93.649 cm",
            "1 0 0 1 0 86.177 cm",
            "1 0 0 1 0 43.836 cm",
            "1 0 0 1 0 37.36 cm",
        ]
    );
}

/// Zero \pdfpagewidth/\pdfpageheight size the page from the shipped box
/// plus twice the origin and offset.
#[test]
fn zero_page_size_follows_the_shipped_box() {
    let dir = fixture_dir("pagesize");
    let mut e = run(
        &dir,
        r"\hoffset=2pt \voffset=-1pt
\shipout\hbox{\vrule width 13pt height 20pt depth 5pt}
\end",
    );
    assert_eq!(e.error_count, 0, "{}", e.term);
    let doc = finish(&mut e);
    let page = doc.get_dictionary(doc.get_pages()[&1]).unwrap();
    let media: Vec<f32> = page
        .get(b"MediaBox")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_float().unwrap())
        .collect();
    assert_eq!(media, [0.0, 0.0, 160.936, 166.914]);
}
