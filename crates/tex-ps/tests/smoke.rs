use tex_ps::{eps_to_pdf, extract_bounding_box, extract_ps_payload, EpsBoundingBox};

#[test]
fn test_dos_binary_eps_header_extraction() {
    let mut data = vec![0xC5, 0xD0, 0xD3, 0xC6]; // magic
    data.extend_from_slice(&(30u32).to_le_bytes()); // PS offset
    data.extend_from_slice(&(15u32).to_le_bytes()); // PS length
    data.extend_from_slice(&(0u32).to_le_bytes());  // WMF offset
    data.extend_from_slice(&(0u32).to_le_bytes());  // WMF length
    data.extend_from_slice(&(0u32).to_le_bytes());  // TIFF offset
    data.extend_from_slice(&(0u32).to_le_bytes());  // TIFF length
    data.extend_from_slice(&(0xFFFFu16).to_le_bytes()); // checksum
    assert_eq!(data.len(), 30);

    data.extend_from_slice(b"%!PS-Adobe-3.0\n");
    assert_eq!(data.len(), 45);

    let extracted = extract_ps_payload(&data);
    assert_eq!(extracted, b"%!PS-Adobe-3.0\n");
}

#[test]
fn test_bounding_box_extraction() {
    let eps = b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 10 20 210 320\n%%HiResBoundingBox: 10.5 20.25 209.5 319.75\n%%EndComments\n";
    let bbox = extract_bounding_box(eps).expect("bounding box should be found");
    assert_eq!(
        bbox,
        EpsBoundingBox {
            llx: 10.5,
            lly: 20.25,
            urx: 209.5,
            ury: 319.75,
        }
    );
}

#[test]
fn test_eps_to_pdf_conversion() {
    let eps = br#"%!PS-Adobe-3.0 EPSF-3.0
%%BoundingBox: 0 0 100 100
%%Creator: Test
%%EndComments
newpath
10 10 moveto
90 10 lineto
90 90 lineto
closepath
0.5 setgray
fill
1 0 0 setrgbcolor
2 setlinewidth
newpath
20 20 moveto
80 80 lineto
stroke
showpage
%%EOF
"#;

    let res = eps_to_pdf(eps).expect("conversion should succeed");
    assert_eq!(res.bbox.llx, 0.0);
    assert_eq!(res.bbox.lly, 0.0);
    assert_eq!(res.bbox.urx, 100.0);
    assert_eq!(res.bbox.ury, 100.0);

    let pdf_str = String::from_utf8_lossy(&res.pdf_bytes);
    assert!(pdf_str.starts_with("%PDF-1.4"));
    assert!(pdf_str.contains("/MediaBox [0.0000 0.0000 100.0000 100.0000]"));
    assert!(pdf_str.contains("stream"));
    assert!(pdf_str.contains("endstream"));
    assert!(pdf_str.contains("%%EOF"));

    let content_str = String::from_utf8_lossy(&res.content_stream);
    assert!(content_str.contains("10.0000 10.0000 m"));
    assert!(content_str.contains("90.0000 10.0000 l"));
    assert!(content_str.contains("h"));
    assert!(content_str.contains("0.5000 g"));
    assert!(content_str.contains("f"));
    assert!(content_str.contains("1.0000 0.0000 0.0000 rg"));
    assert!(content_str.contains("2.0000 w"));
    assert!(content_str.contains("S"));
}

#[test]
fn test_postscript_control_flow_and_procedures() {
    let eps = br#"%!PS-Adobe-3.0 EPSF-3.0
%%BoundingBox: 0 0 50 50
/drawbox {
    newpath
    moveto
    0 20 rlineto
    20 0 rlineto
    0 -20 rlineto
    closepath
    stroke
} def
10 10 drawbox
showpage
"#;

    let res = eps_to_pdf(eps).expect("procedure execution should succeed");
    let content = String::from_utf8_lossy(&res.content_stream);
    assert!(content.contains("10.0000 10.0000 m"));
    assert!(content.contains("10.0000 30.0000 l"));
    assert!(content.contains("30.0000 30.0000 l"));
    assert!(content.contains("30.0000 10.0000 l"));
    assert!(content.contains("h"));
    assert!(content.contains("S"));
}
