//! Regression test: pdfTeX writes a TFM without a pdftex.map entry (bbm,
//! cmbcsc10) as a Type 3 font of its PK bitmaps (writet3.c), one font per
//! size, and advances the text position on the PK raster. Expectations are
//! TeX Live 2026's `pdftex -ini` output for the same source (mktexpk at
//! ljfour, 600 dpi).
use tex_core::engine::Engine;

const SRC: &[u8] = br"\catcode`\{=1 \catcode`\}=2
\pdfoutput=1 \pdfcompresslevel=0 \pdfpkresolution=600
\font\a=bbm10 at 10.95pt \font\b=bbm10 \font\c=bbm12 \font\d=cmbcsc10 at 24.88pt
\shipout\hbox{\a 1AB\b 1AB\hskip 3.33pt\c 1\hskip 2pt\d Ab\c 11}
\end";

/// (FontMatrix, FontBBox, FirstChar, LastChar, nonzero Widths,
/// Differences, [(glyph, `d1` line, MD5 prefix of the glyph procedure)]).
type Expected = (f32, [i64; 4], i64, i64, &'static [(usize, f32)], &'static str, &'static [(&'static str, &'static str, &'static str)]);

const TL: [Expected; 4] = [
    (
        0.011,
        [3, 0, 75, 65],
        49,
        66,
        &[(0, 54.8), (16, 77.53), (17, 73.74)],
        "49 /a49 50 /.notdef 65 /a65 /a66",
        &[
            ("a49", "54.8 0 9 0 48 61 d1", "16fdb885a154"),
            ("a65", "77.53 0 3 0 75 65 d1", "321ab251a318"),
            ("a66", "73.74 0 3 0 69 62 d1", "5697380f1370"),
        ],
    ),
    (
        0.01204,
        [2, 0, 68, 60],
        49,
        66,
        &[(0, 50.06), (16, 70.83), (17, 67.37)],
        "49 /a49 50 /.notdef 65 /a65 /a66",
        &[
            ("a49", "50.06 0 8 0 44 56 d1", "fa023eb8d99e"),
            ("a65", "70.83 0 3 0 68 60 d1", "ee95c0bad000"),
            ("a66", "67.37 0 2 0 62 57 d1", "6a7a6b18c726"),
        ],
    ),
    (0.01004, [9, 0, 51, 66], 49, 49, &[(0, 58.22)], "49 /a49", &[("a49", "58.22 0 9 0 51 66 d1", "f0e535146f8b")]),
    (
        0.00484,
        [8, 0, 158, 144],
        65,
        98,
        &[(0, 169.31), (33, 120.41)],
        "65 /a65 66 /.notdef 98 /a98",
        &[
            ("a65", "169.31 0 11 0 158 144 d1", "82491e61737a"),
            ("a98", "120.41 0 8 0 109 106 d1", "b2a6390dabe3"),
        ],
    ),
];

fn number(object: &lopdf::Object) -> f32 {
    object.as_float().or_else(|_| object.as_i64().map(|n| n as f32)).unwrap()
}

#[test]
fn unmapped_tfm_is_a_type3_font_of_its_pk_bitmaps() {
    let mut engine = Engine::new(true);
    engine.init_primitives();
    engine.add_nullfont();
    engine.input.push_file("pk.tex".into(), SRC.to_vec());
    engine.run();
    assert_eq!(engine.error_count, 0, "{}", engine.term);
    let bytes = tex_core::driver::finish_pdf(&mut engine, false).expect("PDF");
    let pdf = lopdf::Document::load_mem(&bytes).expect("valid PDF");

    let fonts: Vec<&lopdf::Dictionary> = pdf
        .objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .filter(|dict| dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok() == Some(b"Type3"))
        .collect();
    assert_eq!(fonts.len(), 4, "one Type 3 font per PK font size");
    for (matrix, bbox, first, last, widths, differences, procs) in TL {
        let font = fonts
            .iter()
            .find(|font| {
                let font_matrix: Vec<f32> =
                    font.get(b"FontMatrix").unwrap().as_array().unwrap().iter().map(number).collect();
                font_matrix == [matrix, 0.0, 0.0, matrix, 0.0, 0.0]
            })
            .unwrap_or_else(|| panic!("no Type 3 font with /FontMatrix [{matrix} 0 0 {matrix} 0 0]"));
        let font_bbox: Vec<i64> =
            font.get(b"FontBBox").unwrap().as_array().unwrap().iter().map(|n| n.as_i64().unwrap()).collect();
        assert_eq!(font_bbox, bbox, "FontBBox of {matrix}");
        assert_eq!(font.get(b"FirstChar").unwrap().as_i64().unwrap(), first);
        assert_eq!(font.get(b"LastChar").unwrap().as_i64().unwrap(), last);
        let deref = |key: &[u8]| pdf.get_object(font.get(key).unwrap().as_reference().unwrap()).unwrap();
        let font_widths: Vec<f32> = deref(b"Widths").as_array().unwrap().iter().map(number).collect();
        assert_eq!(font_widths.len() as i64, last - first + 1);
        for (index, width) in font_widths.iter().enumerate() {
            let want = widths.iter().find(|(i, _)| *i == index).map_or(0.0, |(_, w)| *w);
            assert!((width - want).abs() < 1e-4, "width {index} of {matrix}: {width} != {want}");
        }
        let encoding = deref(b"Encoding").as_dict().unwrap();
        let diff: Vec<String> = encoding
            .get(b"Differences")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|item| match item {
                lopdf::Object::Name(name) => format!("/{}", String::from_utf8_lossy(name)),
                other => other.as_i64().unwrap().to_string(),
            })
            .collect();
        assert_eq!(diff.join(" "), differences);
        let char_procs = deref(b"CharProcs").as_dict().unwrap();
        assert_eq!(char_procs.len(), procs.len());
        for &(glyph, d1, digest) in procs {
            let stream = pdf
                .get_object(char_procs.get(glyph.as_bytes()).unwrap().as_reference().unwrap())
                .unwrap()
                .as_stream()
                .unwrap();
            let content = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
            let content = content.strip_suffix(b"\n").unwrap_or(&content);
            assert_eq!(content.split(|&b| b == b'\n').next().unwrap(), d1.as_bytes());
            assert_eq!(&format!("{:x}", md5::compute(content))[..12], digest, "glyph {glyph} of {matrix}");
        }
    }

    let page = pdf.get_pages()[&1];
    let content = String::from_utf8_lossy(&pdf.get_page_content(page)).into_owned();
    let tj: Vec<&str> = content.split("Tf ").skip(1).map(|run| run.split("TJ").next().unwrap()).collect();
    assert_eq!(tj, ["72 72 Td [(1AB)]", "[(1AB)]", "[-277(1)]", "[-81(Ab)]", "[1(11)]"], "{content}");
    for size in ["10.9091 Tf", "9.9626 Tf", "11.9552 Tf", "24.7871 Tf"] {
        assert!(content.contains(size), "{content}");
    }
}
