use tex_ps::{eps_to_pdf, extract_bounding_box, extract_ps_payload, EpsBoundingBox, EpsPdfOutput, PsError};

fn run(body: &str) -> Result<EpsPdfOutput, PsError> {
    let eps = format!("%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 100 100\n%%EndComments\n{body}\n%%EOF\n");
    eps_to_pdf(eps.as_bytes())
}

/// Content stream as (operands, operator) records.
fn ops(content: &[u8]) -> Vec<(Vec<String>, String)> {
    let text = String::from_utf8_lossy(content);
    let mut out = Vec::new();
    let mut operands = Vec::new();
    for tok in text.split_whitespace() {
        let operand = tok.parse::<f64>().is_ok()
            || tok.starts_with(['<', '/', '[', ']', '('])
            || tok.ends_with(']');
        if operand {
            operands.push(tok.to_string());
        } else {
            out.push((std::mem::take(&mut operands), tok.to_string()));
        }
    }
    out
}

fn nums(operands: &[String]) -> Vec<f64> {
    operands.iter().filter_map(|s| s.parse().ok()).collect()
}

fn find<'a>(records: &'a [(Vec<String>, String)], op: &str) -> Vec<&'a Vec<String>> {
    records.iter().filter(|(_, o)| o == op).map(|(a, _)| a).collect()
}

/// The program draws a marker stroke only if its condition held.
fn drew_marker(body: &str) -> bool {
    let out = run(&format!("{body} {{ 1 1 moveto 2 2 lineto stroke }} if")).unwrap();
    find(&ops(&out.content_stream), "S").len() == 1
}

fn assert_close(a: &[f64], b: &[f64]) {
    assert_eq!(a.len(), b.len(), "{a:?} vs {b:?}");
    for (x, y) in a.iter().zip(b) {
        assert!((x - y).abs() < 1e-3, "{a:?} vs {b:?}");
    }
}

#[test]
fn dos_binary_eps_header_extraction() {
    let mut data = vec![0xC5, 0xD0, 0xD3, 0xC6];
    data.extend_from_slice(&30u32.to_le_bytes());
    data.extend_from_slice(&15u32.to_le_bytes());
    data.extend_from_slice(&[0; 16]);
    data.extend_from_slice(&0xFFFFu16.to_le_bytes());
    assert_eq!(data.len(), 30);
    data.extend_from_slice(b"%!PS-Adobe-3.0\n");
    assert_eq!(extract_ps_payload(&data), b"%!PS-Adobe-3.0\n");
}

#[test]
fn bounding_box_extraction() {
    let eps = b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 10 20 210 320\n%%HiResBoundingBox: 10.5 20.25 209.5 319.75\n%%EndComments\n";
    let bbox = extract_bounding_box(eps).expect("bounding box should be found");
    assert_eq!(bbox, EpsBoundingBox { llx: 10.5, lly: 20.25, urx: 209.5, ury: 319.75 });
}

#[test]
fn paths_and_colours_become_pdf_operators() {
    let out = run("newpath 10 10 moveto 90 10 lineto 90 90 lineto closepath 0.5 setgray fill
        1 0 0 setrgbcolor 2 setlinewidth newpath 20 20 moveto 80 80 lineto stroke showpage")
    .unwrap();
    assert_eq!(out.bbox, EpsBoundingBox { llx: 0.0, lly: 0.0, urx: 100.0, ury: 100.0 });
    assert!(out.pdf_bytes.starts_with(b"%PDF-1.4"));
    assert!(out.pdf_bytes.ends_with(b"%%EOF\n"));
    let o = ops(&out.content_stream);
    let names: Vec<&str> = o.iter().map(|(_, o)| o.as_str()).collect();
    assert_eq!(names, ["g", "m", "l", "l", "h", "f", "RG", "w", "m", "l", "S"]);
    assert_close(&nums(&o[0].0), &[0.5]);
    assert_close(&nums(&o[1].0), &[10.0, 10.0]);
    assert_close(&nums(&o[6].0), &[1.0, 0.0, 0.0]);
    assert_close(&nums(&o[7].0), &[2.0]);
}

#[test]
fn procedures_compose() {
    let out = run("/box { newpath moveto 0 20 rlineto 20 0 rlineto 0 -20 rlineto closepath stroke } def 10 10 box").unwrap();
    let o = ops(&out.content_stream);
    let points: Vec<Vec<f64>> = o.iter().filter(|(_, o)| o == "m" || o == "l").map(|(a, _)| nums(a)).collect();
    assert_eq!(points, [vec![10.0, 10.0], vec![10.0, 30.0], vec![30.0, 30.0], vec![30.0, 10.0]]);
}

#[test]
fn dictionaries_have_reference_semantics() {
    // EPS prologs define their procedures in a dictionary and `begin` it later.
    assert!(drew_marker("/prolog 10 dict def prolog begin /x 5 def end prolog /x get 5 eq"));
    assert!(drew_marker("/a 1 dict def /b a def b /k 7 put a /k known"));
}

#[test]
fn integer_and_real_arithmetic_follow_postscript() {
    assert!(drew_marker("1 1 add 2 eq"));
    assert!(drew_marker("1 1 add type /integertype eq"));
    assert!(drew_marker("2147483647 1 add type /realtype eq"));
    assert!(drew_marker("-2.5 round -2 eq"));
    assert!(drew_marker("2.5 round 3 eq"));
    assert!(drew_marker("7 -2 mod 1 eq -7 2 idiv -3 eq and"));
    assert!(drew_marker("1 1e-20 div 1e20 eq"));
    assert!(drew_marker("16#FF 255 eq"));
}

#[test]
fn integer_division_overflow_is_an_error_not_a_panic() {
    // Formerly parsed as i64::MIN and panicked in `idiv`.
    let err = run("-9223372036854775808 -1 idiv").unwrap_err();
    assert_eq!(err.error_type, "typecheck");
}

#[test]
fn scanner_handles_immediate_names_escapes_and_number_syntax() {
    assert!(drew_marker("/x 5 def { //x } exec 5 eq"));
    assert!(drew_marker("(a\\\nb) length 2 eq"));
    assert!(drew_marker("(\\8) 0 get 56 eq"));
    assert!(drew_marker("(\\101\\1010) (AA0) eq"));
    assert!(drew_marker("<414> (A@) eq"));
    assert!(drew_marker("<~87cURD]j7BEbo7~> (Hello world) eq"));
    assert!(drew_marker("/inf 3 def inf 3 eq"));
}

#[test]
fn line_width_scales_with_the_ctm() {
    let out = run("0.1 0.1 scale 20 setlinewidth newpath 0 0 moveto 100 0 lineto stroke").unwrap();
    let o = ops(&out.content_stream);
    assert_close(&nums(find(&o, "w")[0]), &[2.0]);
    assert_close(&nums(find(&o, "l")[0]), &[10.0, 0.0]);

    // A non-uniform CTM strokes in user space under `cm`.
    let out = run("2 1 scale 3 setlinewidth newpath 0 0 moveto 10 10 lineto stroke").unwrap();
    let o = ops(&out.content_stream);
    assert_close(&nums(find(&o, "cm")[0]), &[2.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert_close(&nums(find(&o, "w")[0]), &[3.0]);
    assert_close(&nums(find(&o, "l")[0]), &[10.0, 10.0]);
}

#[test]
fn standard_font_text_advances_by_afm_widths() {
    // Helvetica A = 667, B = 667 units.
    assert!(drew_marker("/Helvetica findfont 10 scalefont setfont 0 0 moveto (AB) show currentpoint pop 13.34 sub abs 0.001 lt"));
    let out = run("/ArialMT findfont 12 scalefont setfont 10 20 moveto (Hi) show").unwrap();
    let pdf = String::from_utf8_lossy(&out.pdf_bytes);
    assert!(pdf.contains("/BaseFont /Helvetica"), "{pdf}");
    let o = ops(&out.content_stream);
    assert_close(&nums(find(&o, "Tm")[0]), &[12.0, 0.0, 0.0, 12.0, 10.0, 20.0]);
    assert_eq!(find(&o, "Tj").len(), 1);
}

#[test]
fn type3_glyph_procedures_paint_paths() {
    let out = run("8 dict begin /FontType 3 def /FontMatrix [0.01 0 0 0.01 0 0] def /FontBBox [0 0 100 100] def
        /Encoding 256 array def 0 1 255 { Encoding exch /.notdef put } for Encoding 65 /box put
        /BuildChar { 100 0 0 0 100 100 setcachedevice pop 0 0 moveto 100 0 lineto 100 100 lineto closepath fill } def
        currentdict end /Boxes exch definefont pop
        /Boxes findfont 10 scalefont setfont 5 5 moveto (AA) show").unwrap();
    let o = ops(&out.content_stream);
    assert_eq!(find(&o, "f").len(), 2);
    let moves: Vec<Vec<f64>> = find(&o, "m").into_iter().map(|a| nums(a)).collect();
    assert_eq!(moves, [vec![5.0, 5.0], vec![15.0, 5.0]]);
}

#[test]
fn images_and_inline_data_become_xobjects() {
    let out = run("gsave 10 10 scale 2 2 8 [2 0 0 -2 0 2] {<00FF FF00>} image grestore
        4 1 true [4 0 0 -1 0 1] {currentfile 1 string readhexstring pop} imagemask
        F0
        newpath 0 0 moveto 5 5 lineto stroke").unwrap();
    let pdf = String::from_utf8_lossy(&out.pdf_bytes);
    assert!(pdf.contains("/Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceGray"), "{pdf}");
    assert!(pdf.contains("/Width 4 /Height 1 /BitsPerComponent 1 /ImageMask true /Decode [1 0]"), "{pdf}");
    let o = ops(&out.content_stream);
    assert_eq!(find(&o, "Do").len(), 2);
    // Scanning resumed after the inline image data.
    assert_eq!(find(&o, "S").len(), 1);
}

#[test]
fn stopped_catches_errors_and_keeps_operands() {
    assert!(drew_marker("{ 1 0 div } stopped { pop pop true } { false } ifelse"));
    assert!(drew_marker("{ nosuchoperator } stopped $error /errorname get /undefined eq and"));
    assert!(drew_marker("0 1 1 10 { exch 1 add exch 5 eq { exit } if } for 5 eq"));
}

#[test]
fn restore_undoes_definitions_made_after_save() {
    // CorelDRAW's `/$sv save def ... $sv restore` relies on this.
    assert!(drew_marker("/x 1 def save /x 2 def restore x 1 eq"));
    assert!(drew_marker("/@sv { /$sv save def } def /@rs { $sv restore } def @sv @sv @rs @sv @rs @rs true"));
}

#[test]
fn filters_decode_strings_and_files() {
    assert!(drew_marker("(41 42 4344>) /ASCIIHexDecode filter 4 string readstring pop (ABCD) eq"));
    assert!(drew_marker("<05 61 62 63 64 65 66 FE 7A 80> /RunLengthDecode filter 9 string readstring pop (abcdefzzz) eq"));
    assert!(drew_marker("/s currentfile 0 (%%EOD) /SubFileDecode filter 100 string readstring\nxyz%%EOD\npop def s (xyz) eq"));
}

#[test]
fn eexec_sections_are_decrypted_and_closed() {
    let out = run("currentfile eexec
B8588DA55C2F3D8753B36CB9C80DEA2C747760749017F470064FB424184BC4DD
BBAA2F17026B6D2832C609C57D161748D81387C476B43637B6DCA943A3430F80
0000000000000000000000000000000000000000000000000000000000000000
cleartomark
newpath 30 30 moveto 40 40 lineto stroke").unwrap();
    let o = ops(&out.content_stream);
    let lines: Vec<Vec<f64>> = find(&o, "l").into_iter().map(|a| nums(a)).collect();
    assert_eq!(lines, [vec![20.0, 20.0], vec![40.0, 40.0]]);
}

#[test]
fn unbalanced_graphics_states_are_closed() {
    let out = run("gsave gsave 0 0 moveto 1 1 lineto stroke").unwrap();
    let o = ops(&out.content_stream);
    assert_eq!(find(&o, "q").len(), 2);
    assert_eq!(find(&o, "Q").len(), 2);
}

#[test]
fn hostile_programs_fail_with_postscript_errors() {
    // Each of these used to overflow the Rust stack or attempt a huge allocation.
    let nested = "{".repeat(100_000);
    assert_eq!(run(&nested).unwrap_err().error_type, "syntaxerror");
    assert_eq!(run("/f { f 1 } def f").unwrap_err().error_type, "execstackoverflow");
    assert_eq!(run("2147483647 array").unwrap_err().error_type, "limitcheck");
    assert_eq!(run("16#7FFFFFFF string").unwrap_err().error_type, "limitcheck");
    assert_eq!(run("{ 1 } loop").unwrap_err().error_type, "stackoverflow");
}
