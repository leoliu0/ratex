use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::fmt::Write as _;
use std::io::BufRead;
use std::io::Read;
use std::io::Write;

pub struct EmbeddedImage {
    pub obj_num: i32,
    pub bytes: Vec<u8>,
}

pub struct JpegInfo {
    pub width: u16,
    pub height: u16,
    pub components: u8,
    pub bits: u8,
    /// pdfTeX `img_xres`/`img_yres` from JFIF or Exif (0 = unknown).
    pub x_res: i32,
    pub y_res: i32,
    /// SOF2 (progressive DCT).
    pub progressive: bool,
}

/// writejpg.c `read_APP1_Exif`: X/YResolution and ResolutionUnit of the
/// first IFD; (0, 0) when the TIFF header is malformed.
fn exif_resolution(data: &[u8]) -> (i32, i32) {
    fn read(data: &[u8], at: usize, len: usize, big: bool) -> Option<i64> {
        let bytes = data.get(at..at.checked_add(len)?)?;
        Some(if big {
            bytes.iter().fold(0i64, |v, &b| (v << 8) | i64::from(b))
        } else {
            bytes.iter().rev().fold(0i64, |v, &b| (v << 8) | i64::from(b))
        })
    }
    let parse = || -> Option<(i32, i32)> {
        let tiff = data.iter().position(|&b| b != 0)?;
        let big = match data.get(tiff..tiff + 2)? {
            b"MM" => true,
            b"II" => false,
            _ => return None,
        };
        if read(data, tiff + 2, 2, big)? != 42 {
            return None;
        }
        let mut p = tiff + read(data, tiff + 4, 4, big)? as usize;
        let mut fields = read(data, p, 2, big)?;
        p += 2;
        let (mut xres, mut yres, mut unit) = (72.0f64, 72.0f64, 1.0f64);
        let (mut value, mut num, mut den) = (0i64, 0i64, 0i64);
        while fields > 0 {
            fields -= 1;
            let tag = read(data, p, 2, big)?;
            let kind = read(data, p + 2, 2, big)?;
            p += 8;
            match kind {
                1 | 7 => value = i64::from(*data.get(p)?),
                3 => value = read(data, p, 2, big)?,
                4 | 9 => value = read(data, p, 4, big)? as i32 as i64,
                5 | 10 => {
                    value = read(data, p, 4, big)?;
                    let rp = tiff + value as usize;
                    num = read(data, rp, 4, big)? as i32 as i64;
                    den = read(data, rp + 4, 4, big)? as i32 as i64;
                }
                _ => {}
            }
            p += 4;
            match tag {
                // C integer division, as in writejpg.c
                282 if den != 0 => xres = (num / den) as f64,
                283 if den != 0 => yres = (num / den) as f64,
                296 => match value {
                    2 => unit = 1.0,
                    3 => unit = 2.54,
                    _ => {}
                },
                _ => {}
            }
        }
        Some(((xres * unit) as i32, (yres * unit) as i32))
    };
    parse().unwrap_or((0, 0))
}

/// writejpg.c `read_jpg_info`: the resolution comes from a JFIF APP0 or an
/// Exif APP1 marker that immediately follows SOI; the frame header gives
/// the size and components. Errors are pdfTeX's fatal messages.
pub fn jpeg_info(bytes: &[u8]) -> Result<JpegInfo, String> {
    let u16_at = |at: usize| -> Result<usize, String> {
        bytes
            .get(at..at + 2)
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
            .ok_or_else(|| "reading JPEG image failed (premature file end)".to_string())
    };
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return Err("reading JPEG image failed (no JPEG header found)".into());
    }
    let (mut x_res, mut y_res) = (0i32, 0i32);
    match u16_at(2)? {
        0xffe0 => {
            if bytes.get(6..11) == Some(b"JFIF\0") {
                let units = *bytes.get(13).unwrap_or(&0);
                x_res = u16_at(14)? as i32;
                y_res = u16_at(16)? as i32;
                match units {
                    1 => {}
                    2 => {
                        x_res = (f64::from(x_res) * 2.54) as i32;
                        y_res = (f64::from(y_res) * 2.54) as i32;
                    }
                    _ => (x_res, y_res) = (0, 0),
                }
            }
            if x_res == 0 && y_res != 0 {
                x_res = y_res;
            }
            if y_res == 0 && x_res != 0 {
                y_res = x_res;
            }
        }
        0xffe1 => {
            let length = u16_at(4)?.wrapping_sub(2) as u16 as usize;
            if length > 5 && bytes.get(6..11) == Some(b"Exif\0") {
                let end = (11 + length - 5).min(bytes.len());
                (x_res, y_res) = exif_resolution(&bytes[11..end]);
            }
        }
        _ => {}
    }
    let mut at = 0usize;
    loop {
        if at >= bytes.len() {
            return Err("reading JPEG image failed (premature file end)".into());
        }
        if bytes[at] != 0xff {
            return Err("reading JPEG image failed (no marker found)".into());
        }
        let marker = *bytes
            .get(at + 1)
            .ok_or("reading JPEG image failed (premature file end)")?;
        match marker {
            0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                return Err("unsupported type of compression".into())
            }
            0xc0..=0xc3 => {
                let frame = bytes
                    .get(at + 4..at + 10)
                    .ok_or("reading JPEG image failed (premature file end)")?;
                let components = frame[5];
                if !matches!(components, 1 | 3 | 4) {
                    return Err(format!("Unsupported color space {components}"));
                }
                return Ok(JpegInfo {
                    bits: frame[0],
                    height: u16::from_be_bytes([frame[1], frame[2]]),
                    width: u16::from_be_bytes([frame[3], frame[4]]),
                    components,
                    x_res,
                    y_res,
                    progressive: marker == 0xc2,
                });
            }
            // markers without parameters
            0xd8 | 0xd9 | 0x01 | 0xd0..=0xd7 => at += 2,
            _ => at += 2 + u16_at(at + 2)?,
        }
    }
}

/// writejpg.c `write_jpg`: the file is the DCT stream; CMYK always gets the
/// inverted /Decode array.
pub fn embed_jpeg(bytes: &[u8], object: i32, settings: &RasterSettings<'_>) -> Option<EmbeddedImage> {
    let info = jpeg_info(bytes).ok()?;
    let space = match info.components {
        1 => "/DeviceGray",
        3 => "/DeviceRGB",
        _ => "/DeviceCMYK /Decode [1 0 1 0 1 0 1 0]",
    };
    let space = if settings.colorspace != 0 {
        format!("{} 0 R", settings.colorspace)
    } else {
        space.to_owned()
    };
    let mut body = format!(
        "{} /Width {} /Height {} /BitsPerComponent {} /Length {} /ColorSpace {} /Filter /DCTDecode >>\nstream\n",
        settings.dict_start(),
        info.width,
        info.height,
        info.bits,
        bytes.len(),
        space,
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\nendstream");
    Some(EmbeddedImage {
        obj_num: object,
        bytes: body,
    })
}

fn repair_xref_single_newline(bytes: &[u8]) -> Option<Vec<u8>> {
    // Find startxref from tail
    let sx_marker = b"startxref";
    let sx_idx = bytes
        .windows(sx_marker.len())
        .rposition(|w| w == sx_marker)?;
    let mut i = sx_idx + sx_marker.len();
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let start_digit = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if start_digit == i {
        return None;
    }
    let offset: usize = std::str::from_utf8(&bytes[start_digit..i])
        .ok()?
        .parse()
        .ok()?;
    if offset >= bytes.len() {
        return None;
    }
    // Check if startxref offset has leading whitespace before `xref`
    let mut actual_offset = offset;
    while actual_offset < bytes.len() && bytes[actual_offset].is_ascii_whitespace() {
        actual_offset += 1;
    }
    if actual_offset >= bytes.len() || !bytes[actual_offset..].starts_with(b"xref") {
        actual_offset = offset;
    }

    let mut repaired = Vec::with_capacity(bytes.len() + 128);
    repaired.extend_from_slice(&bytes[..actual_offset]);
    let xref_slice = &bytes[actual_offset..];
    let mut modified = actual_offset != offset;
    let mut j = 0;
    while j < xref_slice.len() {
        if j + 19 <= xref_slice.len()
            && xref_slice[j..j + 10].iter().all(|c| c.is_ascii_digit())
            && xref_slice[j + 10] == b' '
            && xref_slice[j + 11..j + 16]
                .iter()
                .all(|c| c.is_ascii_digit())
            && xref_slice[j + 16] == b' '
            && (xref_slice[j + 17] == b'f' || xref_slice[j + 17] == b'n')
            && xref_slice[j + 18] == b'\n'
        {
            repaired.extend_from_slice(&xref_slice[j..j + 18]);
            repaired.extend_from_slice(b" \n");
            j += 19;
            modified = true;
        } else {
            repaired.push(xref_slice[j]);
            j += 1;
        }
    }
    if actual_offset != offset {
        let new_sx_marker = b"startxref";
        if let Some(pos) = repaired
            .windows(new_sx_marker.len())
            .rposition(|w| w == new_sx_marker)
        {
            let mut k = pos + new_sx_marker.len();
            while k < repaired.len() && repaired[k].is_ascii_whitespace() {
                k += 1;
            }
            let d_start = k;
            while k < repaired.len() && repaired[k].is_ascii_digit() {
                k += 1;
            }
            if d_start < k {
                let new_offset_bytes = actual_offset.to_string().into_bytes();
                repaired.splice(d_start..k, new_offset_bytes);
            }
        }
    }
    if modified {
        Some(repaired)
    } else {
        None
    }
}

/// Fix empty xref section `xref\ntrailer` -> `xref\n0 0\ntrailer` so lopdf's
/// `many1(xref_section)` nom parser can parse the section and follow `/Prev`.
fn repair_xref_empty_section(bytes: &[u8]) -> Option<Vec<u8>> {
    let xref_marker = b"xref";
    let trailer_marker = b"trailer";
    let mut i = 0;
    let mut modified = false;
    let mut result = Vec::with_capacity(bytes.len() + 16);
    while i < bytes.len() {
        if bytes[i..].starts_with(xref_marker) {
            let mut j = i + xref_marker.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if bytes[j..].starts_with(trailer_marker) {
                result.extend_from_slice(xref_marker);
                result.extend_from_slice(b"\n0 0\n");
                i = j;
                modified = true;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    if modified {
        Some(result)
    } else {
        None
    }
}

/// Rebuild a clean cross-reference table by scanning all indirect objects
/// in the file and reconstructing a single standard 20-byte-entry xref table.
/// Mirrors xpdf's XRef::recover() fallback for damaged/shifted xref tables.
fn repair_pdf_xref_rebuild(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut objects = std::collections::BTreeMap::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"obj") {
            if i + 3 < bytes.len()
                && !bytes[i + 3].is_ascii_whitespace()
                && bytes[i + 3] != b'<'
                && bytes[i + 3] != b'['
                && bytes[i + 3] != b'/'
            {
                i += 3;
                continue;
            }
            let mut k = i;
            while k > 0 && bytes[k - 1].is_ascii_whitespace() {
                k -= 1;
            }
            let gen_end = k;
            while k > 0 && bytes[k - 1].is_ascii_digit() {
                k -= 1;
            }
            let gen_start = k;
            if gen_start < gen_end {
                while k > 0 && bytes[k - 1].is_ascii_whitespace() {
                    k -= 1;
                }
                let id_end = k;
                while k > 0 && bytes[k - 1].is_ascii_digit() {
                    k -= 1;
                }
                let id_start = k;
                if id_start < id_end
                    && (id_start == 0
                        || bytes[id_start - 1].is_ascii_whitespace()
                        || bytes[id_start - 1] == b'\n'
                        || bytes[id_start - 1] == b'\r')
                {
                    if let (Ok(obj_id), Ok(gen)) = (
                        std::str::from_utf8(&bytes[id_start..id_end])
                            .unwrap_or("")
                            .parse::<u32>(),
                        std::str::from_utf8(&bytes[gen_start..gen_end])
                            .unwrap_or("")
                            .parse::<u16>(),
                    ) {
                        if obj_id > 0 && obj_id < 1_000_000 {
                            objects.entry(obj_id).or_insert((id_start, gen));
                        }
                    }
                }
            }
            i += 3;
        } else {
            i += 1;
        }
    }

    if objects.is_empty() {
        return None;
    }
    let max_id = *objects.keys().max()?;

    let trailer_marker = b"trailer";
    let trailer_idx = bytes
        .windows(trailer_marker.len())
        .rposition(|w| w == trailer_marker)?;
    let sx_marker = b"startxref";
    let sx_idx = bytes
        .windows(sx_marker.len())
        .rposition(|w| w == sx_marker)?;
    if sx_idx <= trailer_idx {
        return None;
    }
    let mut trailer_bytes = bytes[trailer_idx + trailer_marker.len()..sx_idx]
        .trim_ascii()
        .to_vec();
    // Strip /Prev from trailer dictionary so lopdf does not follow broken old xref
    if let Some(prev_idx) = trailer_bytes.windows(5).position(|w| w == b"/Prev") {
        let mut end = prev_idx + 5;
        while end < trailer_bytes.len() && trailer_bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        while end < trailer_bytes.len() && trailer_bytes[end].is_ascii_digit() {
            end += 1;
        }
        for b in &mut trailer_bytes[prev_idx..end] {
            *b = b' ';
        }
    }

    let mut new_xref = Vec::new();
    new_xref.extend_from_slice(format!("xref\n0 {}\n", max_id + 1).as_bytes());
    new_xref.extend_from_slice(b"0000000000 65535 f \n");
    for id in 1..=max_id {
        if let Some(&(offset, gen)) = objects.get(&id) {
            new_xref.extend_from_slice(format!("{offset:010} {gen:05} n \n").as_bytes());
        } else {
            new_xref.extend_from_slice(b"0000000000 65535 f \n");
        }
    }

    let mut rebuilt = Vec::with_capacity(trailer_idx + new_xref.len() + trailer_bytes.len() + 64);
    rebuilt.extend_from_slice(&bytes[..trailer_idx]);
    let new_xref_pos = rebuilt.len();
    rebuilt.extend_from_slice(&new_xref);
    rebuilt.extend_from_slice(b"trailer\n");
    rebuilt.extend_from_slice(&trailer_bytes);
    rebuilt.extend_from_slice(format!("\nstartxref\n{new_xref_pos}\n%%EOF\n").as_bytes());

    Some(rebuilt)
}

fn base14_program(name: &[u8]) -> Option<(&'static str, i64)> {
    match name {
        b"Times-Roman" => Some(("utmr8a.pfb", 34)),
        b"Times-Bold" => Some(("utmb8a.pfb", 34)),
        b"Times-Italic" => Some(("utmri8a.pfb", 98)),
        b"Times-BoldItalic" => Some(("utmbi8a.pfb", 98)),
        b"Helvetica" => Some(("uhvr8a.pfb", 32)),
        b"Helvetica-Bold" => Some(("uhvb8a.pfb", 32)),
        b"Helvetica-Oblique" => Some(("uhvro8a.pfb", 96)),
        b"Helvetica-BoldOblique" => Some(("uhvbo8a.pfb", 96)),
        b"Courier" => Some(("ucrr8a.pfb", 33)),
        b"Courier-Bold" => Some(("ucrb8a.pfb", 33)),
        b"Courier-Oblique" => Some(("ucrro8a.pfb", 97)),
        b"Courier-BoldOblique" => Some(("ucrbo8a.pfb", 97)),
        b"Symbol" => Some(("usyr.pfb", 4)),
        b"ZapfDingbats" => Some(("uzdr.pfb", 4)),
        _ => None,
    }
}

fn embed_imported_base14_font(
    name: &[u8],
    cache: &mut std::collections::BTreeMap<Vec<u8>, i32>,
    objects: &mut Vec<EmbeddedImage>,
    next: &mut i32,
    resolve_type1: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Option<i32> {
    if let Some(&descriptor) = cache.get(name) {
        return Some(descriptor);
    }
    let (filename, flags) = base14_program(name)?;
    let bytes = resolve_type1(filename)?;
    let program = crate::pdf_fonts::parse_type1(&bytes);
    if program.data.is_empty() {
        return None;
    }
    let cleartext = &program.data[..program.length1.min(program.data.len())];
    let metrics = crate::pdf_fonts::parse_metrics(cleartext);

    let font_file_object = *next;
    *next = (*next).checked_add(1)?;
    let compressed = crate::pdffile::flate(&program.data);
    let mut font_file = lopdf::Dictionary::new();
    font_file.set(b"Length", compressed.len() as i64);
    font_file.set(b"Length1", program.length1 as i64);
    font_file.set(b"Length2", program.length2 as i64);
    font_file.set(b"Length3", program.length3 as i64);
    font_file.set(b"Filter", lopdf::Object::Name(b"FlateDecode".to_vec()));
    let mut font_file_bytes = Vec::new();
    serialize_pdf_object(
        &lopdf::Object::Stream(lopdf::Stream::new(font_file, compressed)),
        &mut font_file_bytes,
    );
    objects.push(EmbeddedImage {
        obj_num: font_file_object,
        bytes: font_file_bytes,
    });

    let descriptor_object = *next;
    *next = (*next).checked_add(1)?;
    let bbox = if metrics.font_bbox == [0.0; 4] {
        [-200.0, -300.0, 1200.0, 1000.0]
    } else {
        metrics.font_bbox
    };
    let mut descriptor = lopdf::Dictionary::new();
    descriptor.set(b"Type", lopdf::Object::Name(b"FontDescriptor".to_vec()));
    descriptor.set(b"FontName", lopdf::Object::Name(name.to_vec()));
    descriptor.set(b"Flags", flags);
    descriptor.set(
        b"FontBBox",
        lopdf::Object::Array(
            bbox.into_iter()
                .map(|value| lopdf::Object::Real(value as f32))
                .collect(),
        ),
    );
    descriptor.set(
        b"ItalicAngle",
        lopdf::Object::Real(metrics.italic_angle as f32),
    );
    descriptor.set(
        b"Ascent",
        lopdf::Object::Real(if metrics.ascent == 0.0 {
            718.0
        } else {
            metrics.ascent
        } as f32),
    );
    descriptor.set(
        b"Descent",
        lopdf::Object::Real(if metrics.descent == 0.0 {
            -207.0
        } else {
            metrics.descent
        } as f32),
    );
    descriptor.set(
        b"CapHeight",
        lopdf::Object::Real(if metrics.cap_height == 0.0 {
            718.0
        } else {
            metrics.cap_height
        } as f32),
    );
    descriptor.set(
        b"StemV",
        lopdf::Object::Real(if metrics.stem_v == 0.0 {
            80.0
        } else {
            metrics.stem_v
        } as f32),
    );
    descriptor.set(
        b"FontFile",
        lopdf::Object::Reference((font_file_object as u32, 0)),
    );
    let mut descriptor_bytes = Vec::new();
    serialize_pdf_object(
        &lopdf::Object::Dictionary(descriptor),
        &mut descriptor_bytes,
    );
    objects.push(EmbeddedImage {
        obj_num: descriptor_object,
        bytes: descriptor_bytes,
    });
    cache.insert(name.to_vec(), descriptor_object);
    Some(descriptor_object)
}

/// pdfTeX page-box specifications (`pdf_box_spec_media` .. `pdf_box_spec_art`).
pub const PDF_BOX_SPEC_MEDIA: i32 = 1;
pub const PDF_BOX_SPEC_CROP: i32 = 2;
pub const PDF_BOX_SPEC_BLEED: i32 = 3;
pub const PDF_BOX_SPEC_TRIM: i32 = 4;
pub const PDF_BOX_SPEC_ART: i32 = 5;

/// `\pdfsuppressptexinfo` bits honored by `write_epdf` (pdftoepdf.cc).
const SUPPRESS_PTEX_FILENAME: i32 = 0x02;
const SUPPRESS_PTEX_PAGENUMBER: i32 = 0x04;
const SUPPRESS_PTEX_INFODICT: i32 = 0x08;

/// Page selection of `\pdfximage`: `page <n>` or `named {<destination>}`.
#[derive(Clone, Copy, Debug)]
pub enum PdfPageSelector<'a> {
    Number(i32),
    Named(&'a [u8]),
}

/// Inclusion settings (`read_pdf_info` / `write_epdf` arguments).
pub struct PdfIncludeOptions<'a> {
    pub page: PdfPageSelector<'a>,
    /// One of the `PDF_BOX_SPEC_*` values; anything else is fatal.
    pub page_box: i32,
    /// The kpathsea file name, written as `/PTEX.FileName`.
    pub file_name: &'a str,
    pub suppress_ptex_info: i32,
    /// `\pdfptexuseunderscore`: `PTEX_` instead of `PTEX.` key prefixes.
    pub ptex_underscore: bool,
}

/// One included PDF page: pdfTeX's `read_pdf_info` results plus the copied
/// page objects. The form XObject itself is written when the image is first
/// shipped (`write_epdf`), because its /Group depends on that page.
pub struct PdfInclusion {
    /// Header version (`PDFDoc::getPDFVersion`).
    pub version: f64,
    pub page: i32,
    pub total_pages: i32,
    /// `epdf_width`, `epdf_height`, `epdf_orig_x`, `epdf_orig_y`: single
    /// precision in pdfTeX (writeimg.c).
    pub width: f32,
    pub height: f32,
    pub orig_x: f32,
    pub orig_y: f32,
    pub rotate: i32,
    /// Resources and other indirect objects the form refers to.
    pub objects: Vec<EmbeddedImage>,
    pub form: PdfForm,
    /// pdfTeX warnings issued while reading the page.
    pub warnings: Vec<String>,
}

/// The pieces of the form XObject `write_epdf` emits for an included page.
#[derive(Clone, Debug)]
pub struct PdfForm {
    head: Vec<u8>,
    /// The page /Group: copied as given (possibly a reference) and as the
    /// dereferenced dictionary used for a separate page-group object.
    group: Option<(Vec<u8>, Vec<u8>)>,
    tail: Vec<u8>,
}

impl PdfForm {
    pub fn has_group(&self) -> bool {
        self.group.is_some()
    }

    /// The page's /Group dictionary, written as the separate page-group
    /// object when this form supplies the page group.
    pub fn group_dict(&self) -> Option<&[u8]> {
        self.group.as_ref().map(|(_, dict)| dict.as_slice())
    }

    /// `pdf_write_image` + `write_epdf`: the `attr` tokens first, then the
    /// form keys. The page /Group refers to `group_object` when this image
    /// supplies the page group, else it is copied inline.
    pub fn write(&self, attr: Option<&str>, group_object: Option<i32>) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.head.len() + self.tail.len() + 96);
        out.extend_from_slice(b"<<");
        if let Some(attr) = attr {
            out.push(b' ');
            out.extend_from_slice(attr.as_bytes());
        }
        out.extend_from_slice(b" /Type /XObject /Subtype /Form /FormType 1");
        out.extend_from_slice(&self.head);
        if let Some((inline, _)) = &self.group {
            match group_object {
                Some(object) => write!(out, " /Group {object} 0 R").unwrap(),
                None => {
                    out.extend_from_slice(b" /Group ");
                    out.extend_from_slice(inline);
                }
            }
        }
        out.extend_from_slice(&self.tail);
        out
    }
}

/// pdftoepdf `sprintf("%.8f")` + `stripzeros`.
fn push_real8(out: &mut Vec<u8>, value: f64) {
    let text = format!("{value:.8}");
    let text = text.trim_end_matches('0');
    out.extend_from_slice(text.strip_suffix('.').unwrap_or(text).as_bytes());
}

/// xpdf `convertStringToPDFString`: octal for non-printables, escaped
/// parentheses and backslashes.
fn push_pdf_string(out: &mut Vec<u8>, text: &[u8]) {
    out.push(b'(');
    for &c in text {
        match c {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', c]),
            b'!'..=b'~' => out.push(c),
            _ => write!(out, "\\{c:03o}").unwrap(),
        }
    }
    out.push(b')');
}

/// A PDF number as xpdf reads it (double precision). lopdf stores reals in
/// single precision; their shortest decimal form recovers the source digits.
fn pdf_number(doc: &lopdf::Document, value: &lopdf::Object) -> Option<f64> {
    match doc.dereference(value).ok()?.1 {
        lopdf::Object::Integer(v) => Some(*v as f64),
        lopdf::Object::Real(v) => format!("{v}").parse().ok(),
        _ => None,
    }
}

/// xpdf `PageAttrs::readBox`: four numbers, clamped and normalized.
fn read_box(doc: &lopdf::Document, dict: &lopdf::Dictionary, key: &[u8]) -> Option<[f64; 4]> {
    let values = doc.dereference(dict.get(key).ok()?).ok()?.1.as_array().ok()?;
    if values.len() != 4 {
        return None;
    }
    let mut b = [0.0; 4];
    for (slot, value) in b.iter_mut().zip(values) {
        *slot = pdf_number(doc, value)?.clamp(-1e9, 1e9);
    }
    if b[0] > b[2] {
        b.swap(0, 2);
    }
    if b[1] > b[3] {
        b.swap(1, 3);
    }
    Some(b)
}

/// xpdf `PDFRectangle::clipTo`.
fn clip_box(b: &mut [f64; 4], to: &[f64; 4]) {
    b[0] = b[0].clamp(to[0], to[2]);
    b[2] = b[2].clamp(to[0], to[2]);
    b[1] = b[1].clamp(to[1], to[3]);
    b[3] = b[3].clamp(to[1], to[3]);
}

/// Copies objects of an included PDF into the output, renumbering every
/// indirect object it reaches (pdftoepdf `copyObject`/`addOther`).
struct PdfCopier<'a> {
    doc: &'a lopdf::Document,
    ids: std::collections::BTreeMap<lopdf::ObjectId, i32>,
    objects: Vec<EmbeddedImage>,
    next: &'a mut i32,
    base14_fonts: std::collections::BTreeMap<Vec<u8>, i32>,
    resolve_type1: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
}

impl PdfCopier<'_> {
    fn copy(&mut self, value: &lopdf::Object, depth: usize) -> Result<lopdf::Object, String> {
        use lopdf::{Dictionary, Object};
        if depth > 256 {
            return Err("PDF resource nesting too deep".into());
        }
        Ok(match value {
            Object::Reference(id) => {
                let new_id = if let Some(mapped) = self.ids.get(id) {
                    *mapped
                } else {
                    let mapped = *self.next;
                    *self.next = self.next.checked_add(1).ok_or("PDF object number overflow")?;
                    self.ids.insert(*id, mapped);
                    let doc = self.doc;
                    let original = doc
                        .objects
                        .get(id)
                        .ok_or("PDF inclusion: reference to invalid object (is the included pdf broken?)")?;
                    let copied = self.copy(original, depth + 1)?;
                    let mut bytes = Vec::new();
                    serialize_pdf_object(&copied, &mut bytes);
                    self.objects.push(EmbeddedImage {
                        obj_num: mapped,
                        bytes,
                    });
                    mapped
                };
                Object::Reference((new_id as u32, 0))
            }
            Object::Array(values) => Object::Array(
                values
                    .iter()
                    .map(|value| self.copy(value, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            Object::Dictionary(dict) => {
                let mut copy = Dictionary::new();
                for (key, value) in dict {
                    copy.set(key.clone(), self.copy(value, depth + 1)?);
                }
                if copy
                    .get(b"Type")
                    .and_then(Object::as_name)
                    .map(|n| n == b"FontDescriptor")
                    .unwrap_or(false)
                {
                    let asc = copy.get(b"Ascent").ok().and_then(|o| {
                        o.as_float()
                            .ok()
                            .map(|x| x as f64)
                            .or_else(|| o.as_i64().ok().map(|x| x as f64))
                    });
                    let desc = copy.get(b"Descent").ok().and_then(|o| {
                        o.as_float()
                            .ok()
                            .map(|x| x as f64)
                            .or_else(|| o.as_i64().ok().map(|x| x as f64))
                    });
                    if let (Some(a), Some(d)) = (asc, desc) {
                        if a - d > 3000.0 {
                            copy.set(b"Descent", Object::Integer((a - 3000.0) as i64));
                        }
                    }
                }
                let unembedded_base14 = copy
                    .get(b"Type")
                    .and_then(Object::as_name)
                    .is_ok_and(|name| name == b"Font")
                    && copy
                        .get(b"Subtype")
                        .and_then(Object::as_name)
                        .is_ok_and(|name| name == b"Type1")
                    && copy.get(b"FontDescriptor").is_err();
                let base_font = unembedded_base14
                    .then(|| {
                        copy.get(b"BaseFont")
                            .ok()
                            .and_then(|value| value.as_name().ok())
                    })
                    .flatten()
                    .map(<[u8]>::to_vec);
                if let Some(base_font) = base_font {
                    if let Some(descriptor) = embed_imported_base14_font(
                        &base_font,
                        &mut self.base14_fonts,
                        &mut self.objects,
                        self.next,
                        self.resolve_type1,
                    ) {
                        copy.set(b"FontDescriptor", Object::Reference((descriptor as u32, 0)));
                    }
                }
                Object::Dictionary(copy)
            }
            Object::Stream(stream) => {
                let mut dict = Dictionary::new();
                for (key, value) in &stream.dict {
                    if key != b"Length" {
                        dict.set(key.clone(), self.copy(value, depth + 1)?);
                    }
                }
                let content = if matches!(
                    dict.get(b"Filter").and_then(Object::as_name),
                    Ok(b"FlateDecode")
                ) {
                    let mut decoder = flate2::read::ZlibDecoder::new(&stream.content[..]);
                    let mut decompressed = Vec::new();
                    let _ = decoder.read_to_end(&mut decompressed);
                    if !decompressed.is_empty() {
                        crate::pdffile::flate(&decompressed)
                    } else {
                        stream.content.clone()
                    }
                } else {
                    stream.content.clone()
                };
                dict.set(b"Length", content.len() as i64);
                Object::Stream(lopdf::Stream::new(dict, content))
            }
            _ => value.clone(),
        })
    }

    fn copy_into(&mut self, value: &lopdf::Object, out: &mut Vec<u8>) -> Result<(), String> {
        let copied = self.copy(value, 0)?;
        serialize_pdf_object(&copied, out);
        Ok(())
    }
}

fn load_pdf_document(bytes: &[u8]) -> Result<lopdf::Document, String> {
    use lopdf::Document;
    match Document::load_mem(bytes) {
        Ok(doc) => Ok(doc),
        Err(e) => {
            if let Some(repaired) =
                repair_xref_empty_section(bytes).and_then(|r| Document::load_mem(&r).ok())
            {
                Ok(repaired)
            } else if let Some(repaired) =
                repair_xref_single_newline(bytes).and_then(|r| Document::load_mem(&r).ok())
            {
                Ok(repaired)
            } else if let Some(rebuilt) =
                repair_pdf_xref_rebuild(bytes).and_then(|r| Document::load_mem(&r).ok())
            {
                Ok(rebuilt)
            } else {
                Err(e.to_string())
            }
        }
    }
}

/// xpdf `Object::getTypeName`, for pdfTeX's inclusion diagnostics.
fn xpdf_type_name(value: &lopdf::Object) -> &'static str {
    use lopdf::Object;
    match value {
        Object::Null => "null",
        Object::Boolean(_) => "boolean",
        Object::Integer(_) => "integer",
        Object::Real(_) => "real",
        Object::Name(_) => "name",
        Object::String(..) => "string",
        Object::Array(_) => "array",
        Object::Dictionary(_) => "dictionary",
        Object::Stream(_) => "stream",
        Object::Reference(_) => "ref",
    }
}

/// xpdf `PDFDoc::findDest`: a named destination from the catalog's /Dests
/// dictionary or the /Names /Dests name tree, resolved to its page object.
fn find_dest_page(doc: &lopdf::Document, name: &[u8]) -> Option<lopdf::ObjectId> {
    use lopdf::Object;
    fn deref<'a>(doc: &'a lopdf::Document, value: &'a Object) -> Option<&'a Object> {
        doc.dereference(value).ok().map(|(_, value)| value)
    }
    fn name_tree<'a>(
        doc: &'a lopdf::Document,
        node: &'a lopdf::Dictionary,
        name: &[u8],
        depth: usize,
    ) -> Option<&'a Object> {
        if depth > 64 {
            return None;
        }
        if let Some(names) = node.get(b"Names").ok().and_then(|v| deref(doc, v)?.as_array().ok()) {
            for pair in names.chunks_exact(2) {
                if deref(doc, &pair[0])?.as_str().ok() == Some(name) {
                    return deref(doc, &pair[1]);
                }
            }
        }
        let kids = node.get(b"Kids").ok().and_then(|v| deref(doc, v)?.as_array().ok())?;
        kids.iter().find_map(|kid| {
            let kid = deref(doc, kid)?.as_dict().ok()?;
            name_tree(doc, kid, name, depth + 1)
        })
    }
    let catalog = doc.catalog().ok()?;
    let dest = catalog
        .get(b"Dests")
        .ok()
        .and_then(|dests| deref(doc, dests)?.as_dict().ok())
        .and_then(|dests| deref(doc, dests.get(name).ok()?))
        .or_else(|| {
            let names = deref(doc, catalog.get(b"Names").ok()?)?.as_dict().ok()?;
            let tree = deref(doc, names.get(b"Dests").ok()?)?.as_dict().ok()?;
            name_tree(doc, tree, name, 0)
        })?;
    let array = match dest {
        Object::Array(array) => array,
        Object::Dictionary(dict) => deref(doc, dict.get(b"D").ok()?)?.as_array().ok()?,
        _ => return None,
    };
    array.first()?.as_reference().ok()
}

/// Include one page of a PDF file the way pdfTeX's `read_pdf_info` and
/// `write_epdf` do (pdftoepdf.cc): xpdf page attributes (inherited
/// MediaBox/CropBox/Rotate, merged Resources, boxes clipped to the media
/// box), the requested page box as the form's /BBox, a /Matrix only for
/// rotated pages, the PTEX.* keys and the page's own content stream.
pub fn include_pdf_page(
    bytes: &[u8],
    options: &PdfIncludeOptions<'_>,
    next_object: &mut i32,
    imported_base14_fonts: &mut std::collections::BTreeMap<Vec<u8>, i32>,
    resolve_type1: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Result<PdfInclusion, String> {
    use lopdf::{Dictionary, Object};
    let doc = load_pdf_document(bytes)?;
    let version = doc.version.trim().parse::<f64>().unwrap_or(0.0);
    let pages = doc.get_pages();
    let total_pages = pages.len() as i32;
    let page_num = match options.page {
        PdfPageSelector::Named(name) => {
            let shown = String::from_utf8_lossy(name);
            let page_id = find_dest_page(&doc, name)
                .ok_or_else(|| format!("PDF inclusion: invalid destination <{shown}>"))?;
            pages
                .iter()
                .find_map(|(&number, &id)| (id == page_id).then_some(number as i32))
                .ok_or_else(|| format!("PDF inclusion: destination is not a page <{shown}>"))?
        }
        PdfPageSelector::Number(page) => {
            if page <= 0 || page > total_pages {
                return Err(format!(
                    "PDF inclusion: required page does not exist <{total_pages}>"
                ));
            }
            page
        }
    };
    let page_id = pages[&(page_num as u32)];

    // xpdf PageAttrs, from the page tree root down to the page.
    let mut chain = vec![page_id];
    while let Some(parent) = doc
        .get_dictionary(*chain.last().unwrap())
        .ok()
        .and_then(|dict| dict.get(b"Parent").ok()?.as_reference().ok())
    {
        if chain.contains(&parent) || chain.len() > 256 {
            break;
        }
        chain.push(parent);
    }
    let mut media_box = [0.0, 0.0, 612.0, 792.0];
    let mut crop_box = [0.0; 4];
    let mut have_crop_box = false;
    let mut rotate = 0i64;
    let mut resources: Option<Dictionary> = None;
    for &id in chain.iter().rev() {
        let Ok(dict) = doc.get_dictionary(id) else {
            continue;
        };
        if let Some(b) = read_box(&doc, dict, b"MediaBox") {
            media_box = b;
        }
        if let Some(b) = read_box(&doc, dict, b"CropBox") {
            crop_box = b;
            have_crop_box = true;
        }
        if let Some(Object::Integer(value)) = dict
            .get(b"Rotate")
            .ok()
            .and_then(|value| doc.dereference(value).ok())
            .map(|(_, value)| value)
        {
            rotate = *value;
        }
        let child = dict
            .get(b"Resources")
            .ok()
            .and_then(|value| doc.dereference(value).ok())
            .and_then(|(_, value)| value.as_dict().ok());
        resources = match (resources, child) {
            // Some files expect the node's resources merged into the
            // parent's; only dictionary-valued categories survive.
            (Some(parent), Some(child)) => {
                let mut merged = Dictionary::new();
                for (key, value) in parent.iter() {
                    if let Ok(sub) = doc.dereference(value).and_then(|(_, v)| v.as_dict()) {
                        merged.set(key.clone(), Object::Dictionary(sub.clone()));
                    }
                }
                for (key, value) in child.iter() {
                    let Ok(sub) = doc.dereference(value).and_then(|(_, v)| v.as_dict()) else {
                        continue;
                    };
                    match merged.get_mut(key) {
                        Ok(Object::Dictionary(existing)) => {
                            for (name, entry) in sub.iter() {
                                existing.set(name.clone(), entry.clone());
                            }
                        }
                        _ => merged.set(key.clone(), Object::Dictionary(sub.clone())),
                    }
                }
                Some(merged)
            }
            (parent, None) => parent,
            (None, Some(child)) => Some(child.clone()),
        };
    }
    let rotate = rotate.rem_euclid(360) as i32;
    if !have_crop_box {
        crop_box = media_box;
    }
    let page_dict = doc.get_dictionary(page_id).map_err(|e| e.to_string())?;
    let other_box = |key: &[u8]| {
        let mut b = read_box(&doc, page_dict, key).unwrap_or(crop_box);
        clip_box(&mut b, &media_box);
        b
    };
    let bleed_box = other_box(b"BleedBox");
    let trim_box = other_box(b"TrimBox");
    let art_box = other_box(b"ArtBox");
    clip_box(&mut crop_box, &media_box);
    let page_box = match options.page_box {
        PDF_BOX_SPEC_MEDIA => media_box,
        PDF_BOX_SPEC_CROP => crop_box,
        PDF_BOX_SPEC_BLEED => bleed_box,
        PDF_BOX_SPEC_TRIM => trim_box,
        PDF_BOX_SPEC_ART => art_box,
        other => {
            return Err(format!(
                "PDF inclusion: unknown value of pagebox spec ({other})"
            ))
        }
    };
    let [x1, y1, x2, y2] = page_box;

    let mut copier = PdfCopier {
        doc: &doc,
        ids: std::collections::BTreeMap::new(),
        objects: Vec::new(),
        next: next_object,
        base14_fonts: imported_base14_fonts.clone(),
        resolve_type1,
    };
    let mut warnings = Vec::new();
    let key_prefix: &[u8] = if options.ptex_underscore { b"PTEX_" } else { b"PTEX." };
    let mut head = Vec::new();
    if options.suppress_ptex_info & SUPPRESS_PTEX_FILENAME == 0 {
        head.extend_from_slice(b" /");
        head.extend_from_slice(key_prefix);
        head.extend_from_slice(b"FileName ");
        push_pdf_string(&mut head, options.file_name.as_bytes());
    }
    if options.suppress_ptex_info & SUPPRESS_PTEX_PAGENUMBER == 0 {
        head.extend_from_slice(b" /");
        head.extend_from_slice(key_prefix);
        write!(head, "PageNumber {page_num}").unwrap();
    }
    if options.suppress_ptex_info & SUPPRESS_PTEX_INFODICT == 0 {
        // the info dictionary must be indirect (PDF Reference p. 61)
        if let Ok(info @ Object::Reference(_)) = doc.trailer.get(b"Info") {
            head.extend_from_slice(b" /");
            head.extend_from_slice(key_prefix);
            head.extend_from_slice(b"InfoDict ");
            copier.copy_into(info, &mut head)?;
        }
    }
    // Only rotated pages get a /Matrix; the page turns about its box.
    let matrix = match rotate {
        90 => Some([0.0, -1.0, 1.0, 0.0, x1 - y1, y1 + x2]),
        180 => Some([-1.0, 0.0, 0.0, -1.0, x1 + x2, y1 + y2]),
        270 => Some([0.0, 1.0, -1.0, 0.0, x1 + y2, y1 - x1]),
        _ => None,
    };
    if let Some(matrix) = matrix {
        head.extend_from_slice(b" /Matrix [");
        for (i, value) in matrix.into_iter().enumerate() {
            if i > 0 {
                head.push(b' ');
            }
            push_real8(&mut head, value);
        }
        head.push(b']');
    }
    head.extend_from_slice(b" /BBox [");
    for (i, value) in page_box.into_iter().enumerate() {
        if i > 0 {
            head.push(b' ');
        }
        push_real8(&mut head, value);
    }
    head.push(b']');
    if matches!(page_dict.get(b"Metadata"), Ok(value) if !matches!(value, Object::Reference(_))) {
        warnings.push("PDF inclusion: /Metadata must be indirect object".to_string());
    }
    for key in [&b"LastModified"[..], b"Metadata", b"PieceInfo", b"SeparationInfo"] {
        if let Ok(value) = page_dict.get(key) {
            head.push(b' ');
            serialize_pdf_name(key, &mut head);
            head.push(b' ');
            copier.copy_into(value, &mut head)?;
        }
    }
    let group = match page_dict.get(b"Group") {
        Ok(value) => {
            let mut inline = Vec::new();
            copier.copy_into(value, &mut inline)?;
            let dict = doc
                .dereference(value)
                .ok()
                .and_then(|(_, value)| value.as_dict().ok())
                .ok_or("PDF inclusion: /Group dict missing")?;
            let mut separate = Vec::new();
            copier.copy_into(&Object::Dictionary(dict.clone()), &mut separate)?;
            Some((inline, separate))
        }
        Err(_) => None,
    };

    let mut tail = Vec::new();
    match &resources {
        None => warnings.push(
            "PDF inclusion: /Resources missing. 'This practice is not recommended' (PDF Ref)"
                .to_string(),
        ),
        Some(resources) => {
            tail.extend_from_slice(b" /Resources <<");
            for (key, value) in resources.iter() {
                let value = doc.dereference(value).map_err(|e| e.to_string())?.1;
                let valid = match key.as_slice() {
                    b"Font" => value.as_dict().is_ok(),
                    b"ProcSet" => value.as_array().is_ok(),
                    b"Subtype" => {
                        if !matches!(value, Object::Name(_)) {
                            warnings.push(format!(
                                "PDF inclusion: Subtype in Resources dict is not a name (key 'Subtype', type <{}>); ignored.",
                                xpdf_type_name(value)
                            ));
                            continue;
                        }
                        true
                    }
                    _ => {
                        if value.as_dict().is_err() {
                            warnings.push(format!(
                                "PDF inclusion: invalid other resource which is no dict (key '{}', type <{}>); ignored.",
                                String::from_utf8_lossy(key),
                                xpdf_type_name(value)
                            ));
                            continue;
                        }
                        true
                    }
                };
                if !valid {
                    return Err(format!(
                        "PDF inclusion: invalid {} type <{}>",
                        if key == b"ProcSet" { "ProcSet array" } else { "font resources dict" },
                        xpdf_type_name(value)
                    ));
                }
                tail.push(b' ');
                serialize_pdf_name(key, &mut tail);
                tail.push(b' ');
                copier.copy_into(value, &mut tail)?;
            }
            tail.extend_from_slice(b" >>");
        }
    }
    // Variant B of write_epdf: a single content stream is copied without
    // recompression, with its own /Filter and /DecodeParms.
    let contents = page_dict
        .get(b"Contents")
        .ok()
        .and_then(|value| doc.dereference(value).ok())
        .map(|(_, value)| value);
    let (data, filter) = match contents {
        Some(Object::Stream(stream)) => {
            if stream.dict.get(b"F").is_ok() {
                return Err("PDF inclusion: Unsupported external stream".into());
            }
            let mut filter = Vec::new();
            for key in [&b"Filter"[..], b"DecodeParms"] {
                if key == b"DecodeParms" && filter.is_empty() {
                    break;
                }
                if let Some((_, value)) = stream.dict.get(key).ok().and_then(|v| doc.dereference(v).ok()) {
                    filter.push(b' ');
                    serialize_pdf_name(key, &mut filter);
                    filter.push(b' ');
                    copier.copy_into(value, &mut filter)?;
                }
            }
            (stream.content.clone(), filter)
        }
        Some(Object::Array(parts)) => {
            let mut data = Vec::new();
            for (i, part) in parts.iter().enumerate() {
                if let Ok((_, Object::Stream(stream))) = doc.dereference(part) {
                    data.extend_from_slice(
                        &stream
                            .decompressed_content_with_limit(256 * 1024 * 1024)
                            .unwrap_or_else(|_| stream.content.clone()),
                    );
                }
                if i + 1 < parts.len() {
                    data.push(b'\n');
                }
            }
            (crate::pdffile::flate(&data), b" /Filter /FlateDecode".to_vec())
        }
        _ => (crate::pdffile::flate(&[]), b" /Filter /FlateDecode".to_vec()),
    };
    write!(tail, " /Length {}", data.len()).unwrap();
    tail.extend_from_slice(&filter);
    tail.extend_from_slice(b" >>\nstream\n");
    tail.extend_from_slice(&data);
    tail.extend_from_slice(b"\nendstream");

    let objects = std::mem::take(&mut copier.objects);
    *imported_base14_fonts = std::mem::take(&mut copier.base14_fonts);
    Ok(PdfInclusion {
        version,
        page: page_num,
        total_pages,
        width: (x2 - x1) as f32,
        height: (y2 - y1) as f32,
        orig_x: x1 as f32,
        orig_y: y1 as f32,
        rotate,
        objects,
        form: PdfForm { head, group, tail },
        warnings,
    })
}

fn serialize_pdf_object(value: &lopdf::Object, out: &mut Vec<u8>) {
    use lopdf::Object;
    match value {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(v) => out.extend_from_slice(if *v { b"true" } else { b"false" }),
        Object::Integer(v) => write!(out, "{v}").unwrap(),
        Object::Real(v) => write!(out, "{v}").unwrap(),
        Object::Name(name) => serialize_pdf_name(name, out),
        Object::String(bytes, _) => {
            out.push(b'<');
            for c in bytes {
                write!(out, "{c:02X}").unwrap();
            }
            out.push(b'>');
        }
        Object::Array(values) => {
            out.push(b'[');
            for value in values {
                serialize_pdf_object(value, out);
                out.push(b' ');
            }
            out.push(b']');
        }
        Object::Dictionary(dict) => serialize_pdf_dict(dict, out),
        Object::Stream(stream) => {
            serialize_pdf_dict(&stream.dict, out);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&stream.content);
            out.extend_from_slice(b"\nendstream");
        }
        Object::Reference((id, generation)) => write!(out, "{id} {generation} R").unwrap(),
    }
}

fn serialize_pdf_name(name: &[u8], out: &mut Vec<u8>) {
    out.push(b'/');
    for &c in name {
        if (33..=126).contains(&c) && !b"#%()/<>[]{}".contains(&c) {
            out.push(c);
        } else {
            write!(out, "#{c:02X}").unwrap();
        }
    }
}

fn serialize_pdf_dict(dict: &lopdf::Dictionary, out: &mut Vec<u8>) {
    out.extend_from_slice(b"<<");
    for (key, value) in dict {
        serialize_pdf_name(key, out);
        out.push(b' ');
        serialize_pdf_object(value, out);
        out.push(b' ');
    }
    out.extend_from_slice(b">>");
}

fn paeth_predictor(a: i16, b: i16, c: i16) -> u8 {
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

/// Controls the CPU/file-size tradeoff when a PNG has to be recompressed.
/// Opaque, non-interlaced PNG streams that PDF can consume directly are never
/// recompressed, regardless of this setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PngOptimization {
    /// Select a cheap predictor and run one full-image deflate stream.
    Speed,
    /// Try the unfiltered and fully adaptive streams and retain the smaller.
    Size,
}

/// PNG embedding settings for image data that cannot be passed through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PngEmbedOptions {
    /// The zlib compression level. Values above zlib's maximum are clamped to
    /// 9, matching the useful range of `\pdfcompresslevel`.
    pub compression_level: u32,
    pub optimization: PngOptimization,
}

impl PngEmbedOptions {
    pub const fn speed(compression_level: u32) -> Self {
        Self {
            compression_level,
            optimization: PngOptimization::Speed,
        }
    }

    pub const fn size(compression_level: u32) -> Self {
        Self {
            compression_level,
            optimization: PngOptimization::Size,
        }
    }
}

impl Default for PngEmbedOptions {
    fn default() -> Self {
        // Level 3 is the measured knee for zlib-rs: it avoids the severe size
        // penalty of levels 0-1 while retaining most of their throughput.
        Self::speed(3)
    }
}

// This is an allocation/CPU safety bound for paths that must decode pixels or
// validate a compressed raster before copying it into a PDF.
const MAX_DECODED_RASTER_BYTES: usize = 2048 * 1024 * 1024;

struct ParsedPng<'a> {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
    idat: Vec<&'a [u8]>,
    palette: Option<&'a [u8]>,
    transparency: Option<&'a [u8]>,
    /// libpng's file gamma (fixed point): gAMA, or sRGB's 45455.
    gamma: Option<u32>,
}

fn valid_png_bit_depth(color_type: u8, bit_depth: u8) -> bool {
    match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        4 | 6 => matches!(bit_depth, 8 | 16),
        _ => false,
    }
}

fn parse_png_impl(bytes: &[u8], verify_crc: bool) -> Option<ParsedPng<'_>> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() < 33 {
        return None;
    }
    let mut pos = 8usize;
    let mut width = 0;
    let mut height = 0;
    let mut bit_depth = 0;
    let mut color_type = 0;
    let mut interlace = 0;
    let mut saw_header = false;
    let mut saw_idat = false;
    let mut saw_idat_payload = false;
    let mut ended_idat = false;
    let mut saw_end = false;
    let mut idat = Vec::new();
    let mut palette = None;
    let mut transparency = None;
    let mut gamma = None;
    let mut srgb = false;

    while pos < bytes.len() {
        let header = bytes.get(pos..pos.checked_add(8)?)?;
        let length = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
        let kind = &header[4..8];
        let data_start = pos.checked_add(8)?;
        let data_end = data_start.checked_add(length)?;
        let chunk_end = data_end.checked_add(4)?;
        let data = bytes.get(data_start..data_end)?;
        let stored_crc = u32::from_be_bytes(bytes.get(data_end..chunk_end)?.try_into().ok()?);
        if verify_crc {
            let mut crc = crc32fast::Hasher::new();
            crc.update(kind);
            crc.update(data);
            if crc.finalize() != stored_crc {
                return None;
            }
        }

        if !saw_header && kind != b"IHDR" {
            return None;
        }
        if saw_idat && kind != b"IDAT" {
            ended_idat = true;
        }
        match kind {
            b"IHDR" => {
                if saw_header || pos != 8 || data.len() != 13 {
                    return None;
                }
                width = u32::from_be_bytes(data[0..4].try_into().ok()?);
                height = u32::from_be_bytes(data[4..8].try_into().ok()?);
                bit_depth = data[8];
                color_type = data[9];
                if width == 0
                    || height == 0
                    || !valid_png_bit_depth(color_type, bit_depth)
                    || data[10] != 0
                    || data[11] != 0
                    || !matches!(data[12], 0 | 1)
                {
                    return None;
                }
                interlace = data[12];
                saw_header = true;
            }
            b"PLTE" => {
                if !saw_header
                    || saw_idat
                    || palette.is_some()
                    || data.is_empty()
                    || data.len() % 3 != 0
                    || data.len() > 3 * 256
                {
                    return None;
                }
                palette = Some(data);
            }
            b"tRNS" => {
                if !saw_header || saw_idat || transparency.is_some() {
                    return None;
                }
                transparency = Some(data);
            }
            b"gAMA" if data.len() == 4 => {
                gamma = Some(u32::from_be_bytes(data.try_into().ok()?));
            }
            b"sRGB" => srgb = true,
            b"IDAT" => {
                if !saw_header || ended_idat {
                    return None;
                }
                saw_idat = true;
                saw_idat_payload |= !data.is_empty();
                idat.push(data);
            }
            b"IEND" => {
                if data.len() != 0 || !saw_idat {
                    return None;
                }
                saw_end = true;
                pos = chunk_end;
                break;
            }
            // Reject unknown critical chunks. Ancillary metadata has no effect
            // on the PDF image samples and can safely be skipped.
            _ if kind.first().is_some_and(|c| c & 0x20 == 0) => return None,
            _ => {}
        }
        pos = chunk_end;
    }

    if !saw_header || !saw_end || !saw_idat_payload {
        return None;
    }
    match color_type {
        0 => {
            if palette.is_some() || transparency.is_some_and(|v| v.len() != 2) {
                return None;
            }
        }
        2 => {
            if transparency.is_some_and(|v| v.len() != 6) {
                return None;
            }
        }
        3 => {
            let entries = palette?.len() / 3;
            if entries > (1usize << bit_depth)
                || transparency.is_some_and(|v| v.is_empty() || v.len() > entries)
            {
                return None;
            }
        }
        4 | 6 if transparency.is_some() => return None,
        4 | 6 => {}
        _ => return None,
    }
    Some(ParsedPng {
        width,
        height,
        bit_depth,
        color_type,
        interlace,
        idat,
        palette,
        transparency,
        gamma: if srgb { Some(45455) } else { gamma },
    })
}

fn parse_png(bytes: &[u8]) -> Option<ParsedPng<'_>> {
    parse_png_impl(bytes, true)
}

fn concatenate_idat(parsed: &ParsedPng<'_>) -> Option<Vec<u8>> {
    let length = parsed
        .idat
        .iter()
        .try_fold(0usize, |sum, part| sum.checked_add(part.len()))?;
    let mut bytes = Vec::with_capacity(length);
    for part in &parsed.idat {
        bytes.extend_from_slice(part);
    }
    Some(bytes)
}

struct IdatReader<'a> {
    chunks: &'a [&'a [u8]],
    chunk: usize,
    offset: usize,
}

impl<'a> IdatReader<'a> {
    fn new(chunks: &'a [&'a [u8]]) -> Self {
        Self {
            chunks,
            chunk: 0,
            offset: 0,
        }
    }
}

impl Read for IdatReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        while let Some(chunk) = self.chunks.get(self.chunk) {
            let remaining = &chunk[self.offset..];
            if remaining.is_empty() {
                self.chunk += 1;
                self.offset = 0;
                continue;
            }
            let count = remaining.len().min(output.len());
            output[..count].copy_from_slice(&remaining[..count]);
            self.offset += count;
            return Ok(count);
        }
        Ok(0)
    }
}

impl BufRead for IdatReader<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        while let Some(chunk) = self.chunks.get(self.chunk) {
            let remaining = &chunk[self.offset..];
            if !remaining.is_empty() {
                return Ok(remaining);
            }
            self.chunk += 1;
            self.offset = 0;
        }
        Ok(&[])
    }

    fn consume(&mut self, amount: usize) {
        let remaining = self
            .chunks
            .get(self.chunk)
            .map_or(0, |chunk| chunk.len().saturating_sub(self.offset));
        debug_assert!(amount <= remaining);
        self.offset += amount.min(remaining);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IndexedTransparency {
    Opaque,
    ColorKey(u8, u8),
    SoftMask,
}

fn indexed_transparency(bytes: Option<&[u8]>) -> IndexedTransparency {
    let Some(alpha) = bytes else {
        return IndexedTransparency::Opaque;
    };
    let mut first = None;
    let mut last = None;
    for (index, &value) in alpha.iter().enumerate() {
        if value == 0 {
            first.get_or_insert(index as u8);
            last = Some(index as u8);
        } else if value != 255 {
            return IndexedTransparency::SoftMask;
        }
    }
    let Some(first) = first else {
        return IndexedTransparency::Opaque;
    };
    let last = last.unwrap_or(first);
    if alpha[first as usize..=last as usize]
        .iter()
        .all(|&value| value == 0)
    {
        IndexedTransparency::ColorKey(first, last)
    } else {
        IndexedTransparency::SoftMask
    }
}

/// pdfTeX image settings: the PDF version and `\pdfimage*` parameters
/// fixed at the first output, plus the per-image `attr` and `colorspace`
/// keywords of `\pdfximage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterSettings<'a> {
    /// `attr {...}`: written first in the image dictionary.
    pub attr: Option<&'a str>,
    /// `colorspace <n>`: object number replacing the image's color space.
    pub colorspace: i32,
    /// PDF 1.4 or newer: alpha becomes a soft mask; older PDFs drop it.
    pub alpha: bool,
    /// `fixed_image_hicolor` with PDF 1.5 or newer: 16-bit samples stay.
    pub hicolor: bool,
    /// `\pdfimageapplygamma`: (`\pdfgamma`, `\pdfimagegamma`), thousandths.
    pub gamma: Option<(i32, i32)>,
}

impl Default for RasterSettings<'_> {
    fn default() -> Self {
        Self {
            attr: None,
            colorspace: 0,
            alpha: true,
            hicolor: true,
            gamma: None,
        }
    }
}

impl RasterSettings<'_> {
    fn dict_start(&self) -> String {
        match self.attr {
            Some(attr) => format!("<< {attr} /Type /XObject /Subtype /Image"),
            None => SMASK_START.to_owned(),
        }
    }

    fn color_space(&self, natural: &str) -> String {
        if self.colorspace != 0 {
            format!("{} 0 R", self.colorspace)
        } else {
            natural.to_owned()
        }
    }
}

/// Soft masks carry no user attributes.
const SMASK_START: &str = "<< /Type /XObject /Subtype /Image";

/// pdfTeX's writepng.c copies the PNG stream only when no sample needs
/// changing; everything else goes through libpng's transformations.
fn png_needs_decoding(parsed: &ParsedPng<'_>, settings: &RasterSettings<'_>) -> bool {
    let transparency = matches!(parsed.color_type, 4 | 6) || parsed.transparency.is_some();
    parsed.interlace != 0
        || settings.gamma.is_some()
        || (transparency && !settings.alpha)
        || (parsed.bit_depth == 16 && (!settings.hicolor || matches!(parsed.color_type, 4 | 6)))
}

/// Returns whether the caller must reserve one additional PDF object for a
/// possible alpha soft mask. PNG color-key transparency is represented inline
/// and does not consume an object. Gray-alpha/RGBA images always get a soft
/// mask (pdfTeX writepng.c), even when every alpha sample is opaque.
pub fn png_needs_soft_mask(bytes: &[u8], settings: &RasterSettings<'_>) -> bool {
    // This is only an object-number reservation prepass. The embed pass below
    // performs the CRC and compressed-raster validation once, avoiding a
    // second full read of every IDAT payload on the common path.
    let Some(parsed) = parse_png_impl(bytes, false) else {
        return false;
    };
    if !settings.alpha {
        return false;
    }
    if png_needs_decoding(&parsed, settings) {
        // libpng's tRNS-to-alpha expansion always yields an alpha channel.
        return matches!(parsed.color_type, 4 | 6) || parsed.transparency.is_some();
    }
    match parsed.color_type {
        4 | 6 => true,
        3 => indexed_transparency(parsed.transparency) == IndexedTransparency::SoftMask,
        _ => false,
    }
}

/// What pdfTeX learns from a PNG header (`read_png_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: u8,
    /// `img_xres`/`img_yres`: pHYs pixels per meter as dpi (0 = unknown).
    pub x_res: i32,
    pub y_res: i32,
}

pub fn png_info(bytes: &[u8]) -> Option<PngInfo> {
    if bytes.len() < 33 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let mut info = PngInfo {
        width: u32::from_be_bytes(bytes[16..20].try_into().ok()?),
        height: u32::from_be_bytes(bytes[20..24].try_into().ok()?),
        bit_depth: bytes[24],
        color_type: bytes[25],
        x_res: 0,
        y_res: 0,
    };
    let mut at = 8usize;
    while let Some(header) = bytes.get(at..at.checked_add(8)?) {
        let length = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
        let data = at + 8;
        match &header[4..8] {
            // libpng reports pixels per meter only for the meter unit
            b"pHYs" if length >= 9 => {
                let phys = bytes.get(data..data + 9)?;
                if phys[8] == 1 {
                    let ppm = |b: &[u8]| u32::from_be_bytes(b.try_into().unwrap()) as f64;
                    info.x_res = (0.0254 * ppm(&phys[0..4])).round() as i32;
                    info.y_res = (0.0254 * ppm(&phys[4..8])).round() as i32;
                }
            }
            b"IDAT" | b"IEND" => break,
            _ => {}
        }
        at = data.checked_add(length)?.checked_add(4)?;
    }
    Some(info)
}

fn checked_raster_layout(width: u32, height: u32, components: usize) -> Option<(usize, usize)> {
    let row = usize::try_from(width).ok()?.checked_mul(components)?;
    let total = row
        .checked_add(1)?
        .checked_mul(usize::try_from(height).ok()?)?;
    (total <= MAX_DECODED_RASTER_BYTES).then_some((row, total))
}

#[inline]
fn residual_score(value: u8) -> u64 {
    // Treat residuals as signed bytes. This is the standard cheap PNG
    // heuristic and avoids trial-deflating every row.
    i16::from(value as i8).unsigned_abs() as u64
}

fn select_filter(row: &[u8], previous: &[u8], bpp: usize, optimization: PngOptimization) -> u8 {
    let mut scores = [0u64; 5];
    for (at, &value) in row.iter().enumerate() {
        let left = if at >= bpp { row[at - bpp] } else { 0 };
        let up = previous[at];
        scores[0] += residual_score(value);
        scores[1] += residual_score(value.wrapping_sub(left));
        scores[2] += residual_score(value.wrapping_sub(up));
        if optimization == PngOptimization::Size {
            scores[3] += residual_score(value.wrapping_sub(((left as u16 + up as u16) / 2) as u8));
            scores[4] += residual_score(value.wrapping_sub(paeth_predictor(
                left as i16,
                up as i16,
                if at >= bpp {
                    previous[at - bpp] as i16
                } else {
                    0
                },
            )));
        }
    }
    let count = if optimization == PngOptimization::Size {
        5
    } else {
        3
    };
    scores[..count]
        .iter()
        .enumerate()
        .min_by_key(|(_, score)| *score)
        .map(|(kind, _)| kind as u8)
        .unwrap_or(0)
}

fn make_filtered_row(kind: u8, row: &[u8], previous: &[u8], bpp: usize, output: &mut [u8]) {
    output[0] = kind;
    match kind {
        0 => output[1..].copy_from_slice(row),
        1 => {
            for at in 0..row.len() {
                let left = if at >= bpp { row[at - bpp] } else { 0 };
                output[at + 1] = row[at].wrapping_sub(left);
            }
        }
        2 => {
            for at in 0..row.len() {
                output[at + 1] = row[at].wrapping_sub(previous[at]);
            }
        }
        3 => {
            for at in 0..row.len() {
                let left = if at >= bpp { row[at - bpp] } else { 0 };
                output[at + 1] =
                    row[at].wrapping_sub(((left as u16 + previous[at] as u16) / 2) as u8);
            }
        }
        4 => {
            for at in 0..row.len() {
                let left = if at >= bpp { row[at - bpp] } else { 0 };
                output[at + 1] = row[at].wrapping_sub(paeth_predictor(
                    left as i16,
                    previous[at] as i16,
                    if at >= bpp {
                        previous[at - bpp] as i16
                    } else {
                        0
                    },
                ));
            }
        }
        _ => unreachable!(),
    }
}

struct RasterEncoder {
    previous: Vec<u8>,
    filtered: ZlibEncoder<Vec<u8>>,
    raw: Option<ZlibEncoder<Vec<u8>>>,
    scratch: Vec<u8>,
    bpp: usize,
    optimization: PngOptimization,
}

impl RasterEncoder {
    fn new(row_bytes: usize, bpp: usize, options: PngEmbedOptions) -> Self {
        let compression = Compression::new(options.compression_level.min(9));
        Self {
            previous: vec![0; row_bytes],
            filtered: ZlibEncoder::new(Vec::new(), compression),
            raw: (options.optimization == PngOptimization::Size)
                .then(|| ZlibEncoder::new(Vec::new(), compression)),
            scratch: vec![0; row_bytes + 1],
            bpp,
            optimization: options.optimization,
        }
    }

    fn write_row(&mut self, row: &[u8]) -> Option<()> {
        if row.len() != self.previous.len() {
            return None;
        }
        if let Some(raw) = &mut self.raw {
            raw.write_all(&[0]).ok()?;
            raw.write_all(row).ok()?;
        }

        let best_filter = select_filter(row, &self.previous, self.bpp, self.optimization);
        make_filtered_row(
            best_filter,
            row,
            &self.previous,
            self.bpp,
            &mut self.scratch,
        );
        self.filtered.write_all(&self.scratch).ok()?;
        self.previous.copy_from_slice(row);
        Some(())
    }

    fn finish(self) -> Option<Vec<u8>> {
        let filtered = self.filtered.finish().ok()?;
        let Some(raw) = self.raw else {
            return Some(filtered);
        };
        let raw = raw.finish().ok()?;
        Some(if raw.len() <= filtered.len() {
            raw
        } else {
            filtered
        })
    }
}

/// Deflate an unfiltered raster with per-row PNG predictors. `bpp` is the
/// predictor's bytes per pixel (components times bytes per sample).
fn encode_flat_raster(
    pixels: &[u8],
    width: u32,
    height: u32,
    bpp: usize,
    options: PngEmbedOptions,
) -> Option<Vec<u8>> {
    let (row_bytes, _) = checked_raster_layout(width, height, bpp)?;
    let rows = usize::try_from(height).ok()?;
    if pixels.len() != row_bytes.checked_mul(rows)? {
        return None;
    }
    let mut encoder = RasterEncoder::new(row_bytes, bpp, options);
    for row in pixels.chunks_exact(row_bytes) {
        encoder.write_row(row)?;
    }
    encoder.finish()
}

fn compressed_raster_object(
    start: &str,
    object: i32,
    width: u32,
    height: u32,
    bit_depth: u8,
    color_space: &str,
    colors: usize,
    compressed: Vec<u8>,
    extra: &str,
) -> EmbeddedImage {
    let mut bytes = format!(
        "{start} /Width {width} /Height {height} /BitsPerComponent {bit_depth} /ColorSpace {color_space} /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns {width} /Colors {colors} /BitsPerComponent {bit_depth} >>{extra} /Length {} >>\nstream\n",
        compressed.len(),
    )
    .into_bytes();
    bytes.extend_from_slice(&compressed);
    bytes.extend_from_slice(b"\nendstream");
    EmbeddedImage {
        obj_num: object,
        bytes,
    }
}

/// libpng `convert_gamma_value`: a gamma passed as a double becomes fixed
/// point (values below 128 are scaled by 100000).
fn png_fixed_gamma(value: f64) -> Option<i64> {
    let value = if value > 0.0 && value < 128.0 {
        value * 100000.0
    } else {
        value
    };
    let fixed = (value + 0.5).floor();
    (fixed > 0.0 && fixed <= i32::MAX as f64).then_some(fixed as i64)
}

/// libpng `png_gamma_significant`.
fn png_gamma_significant(gamma: i64) -> bool {
    !(95000..=105000).contains(&gamma)
}

/// The libpng gamma correction pdfTeX requests with
/// `png_set_gamma(png, \pdfgamma/1000, file_gamma)`.
struct PngGamma {
    /// `png_reciprocal2(file, screen)`: the correction exponent (fixed).
    exponent: i64,
    /// `png_product2(file, screen)`: its inverse, for 16-to-8 tables.
    inverse: i64,
}

impl PngGamma {
    /// None when libpng leaves the samples alone (product within 5% of 1).
    fn new(screen_milli: i32, image_gamma_milli: i32, file_gamma: Option<u32>) -> Result<Option<Self>, String> {
        let screen = png_fixed_gamma(screen_milli as f64 / 1000.0);
        let file = match file_gamma {
            Some(fixed) => png_fixed_gamma(fixed as f64 * 0.00001),
            None => png_fixed_gamma(1000.0 / image_gamma_milli as f64),
        };
        let (Some(screen), Some(file)) = (screen, file) else {
            return Err("libpng: internal error".into());
        };
        let product = ((file as f64 * screen as f64) / 100000.0 + 0.5).floor() as i64;
        if !png_gamma_significant(product) {
            return Ok(None);
        }
        Ok(Some(Self {
            exponent: ((1e15 / file as f64) / screen as f64 + 0.5).floor() as i64,
            inverse: product,
        }))
    }

    /// `png_build_8bit_table`.
    fn table8(&self) -> [u8; 256] {
        let mut table = [0u8; 256];
        let significant = png_gamma_significant(self.exponent);
        for (i, slot) in table.iter_mut().enumerate() {
            *slot = if significant && i > 0 && i < 255 {
                (255.0 * (i as f64 / 255.0).powf(self.exponent as f64 * 0.00001) + 0.5).floor() as u8
            } else {
                i as u8
            };
        }
        table
    }

    /// `png_build_16bit_table` (no sBIT shift).
    fn correct16(&self, value: u16) -> u16 {
        if !png_gamma_significant(self.exponent) {
            return value;
        }
        let x = value as f64 / 65535.0;
        (65535.0 * x.powf(self.exponent as f64 * 0.00001) + 0.5).floor() as u16
    }

    /// `png_build_16to8_table` (shift 5) followed by the chop to 8 bits,
    /// indexed by the top 11 bits of a 16-bit sample.
    fn table16to8(&self) -> Vec<u8> {
        const SHIFT: u32 = 5;
        let max = (1u32 << (16 - SHIFT)) - 1;
        let mut table = vec![255u8; 1 << (16 - SHIFT)];
        let mut last = 0usize;
        for i in 0..255u32 {
            let out = i * 257 + 128;
            let corrected = (65535.0
                * (out as f64 / 65535.0).powf(self.inverse as f64 * 0.00001)
                + 0.5)
                .floor() as u32;
            let bound = ((corrected * max + 32768) / 65535 + 1) as usize;
            while last < bound.min(table.len()) {
                table[last] = i as u8;
                last += 1;
            }
        }
        table
    }
}

/// The libpng path of writepng.c: expand palettes and transparency, keep or
/// strip 16-bit samples (`\pdfimagehicolor`), drop alpha before PDF 1.4,
/// apply `\pdfimageapplygamma`, and split alpha into an 8-bit soft mask.
fn embed_decoded_png(
    bytes: &[u8],
    parsed: &ParsedPng<'_>,
    object: i32,
    next: &mut i32,
    options: PngEmbedOptions,
    settings: &RasterSettings<'_>,
) -> Option<Vec<EmbeddedImage>> {
    let gamma = match settings.gamma {
        Some((screen, image)) => PngGamma::new(screen, image, parsed.gamma).ok()?,
        None => None,
    };
    let keep16 = parsed.bit_depth == 16 && settings.hicolor;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // 16-bit samples are decoded in full when gamma correction must see them.
    let strip_in_decoder = parsed.bit_depth == 16 && !keep16 && gamma.is_none();
    decoder.set_transformations(if strip_in_decoder {
        png::Transformations::EXPAND | png::Transformations::STRIP_16
    } else {
        png::Transformations::EXPAND
    });
    let mut reader = decoder.read_info().ok()?;
    let output_size = reader.output_buffer_size()?;
    if output_size > MAX_DECODED_RASTER_BYTES {
        return None;
    }
    let mut pixels = vec![0; output_size];
    let info = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(info.buffer_size());
    let (colors, has_alpha) = match info.color_type {
        png::ColorType::Grayscale => (1, false),
        png::ColorType::GrayscaleAlpha => (1, true),
        png::ColorType::Rgb => (3, false),
        png::ColorType::Rgba => (3, true),
        _ => return None,
    };
    let sample_bytes = if info.bit_depth == png::BitDepth::Sixteen { 2 } else { 1 };
    let channels = colors + usize::from(has_alpha);
    let pixel_count = usize::try_from(info.width)
        .ok()?
        .checked_mul(usize::try_from(info.height).ok()?)?;
    if pixels.len() != pixel_count.checked_mul(channels * sample_bytes)? {
        return None;
    }
    let out_bytes = if keep16 { 2 } else { 1 };
    let mut main = Vec::with_capacity(pixel_count * colors * out_bytes);
    let mut alpha = Vec::with_capacity(if has_alpha { pixel_count } else { 0 });
    if sample_bytes == 2 {
        let table16to8 = gamma.as_ref().filter(|_| !keep16).map(PngGamma::table16to8);
        for pixel in pixels.chunks_exact(channels * 2) {
            for sample in pixel[..colors * 2].chunks_exact(2) {
                let value = u16::from_be_bytes([sample[0], sample[1]]);
                if keep16 {
                    let value = gamma.as_ref().map_or(value, |g| g.correct16(value));
                    main.extend_from_slice(&value.to_be_bytes());
                } else {
                    main.push(match &table16to8 {
                        Some(table) => table[usize::from(value >> 5)],
                        None => sample[0],
                    });
                }
            }
            if has_alpha {
                // writepng.c keeps the high byte of 16-bit alpha samples
                alpha.push(pixel[colors * 2]);
            }
        }
    } else {
        let table = gamma.as_ref().map(PngGamma::table8);
        // libpng corrects 2- and 4-bit gray on the replicated 8-bit value and
        // keeps the top bits of the result.
        let low_gray_bits = (parsed.color_type == 0 && parsed.bit_depth < 8)
            .then_some(parsed.bit_depth)
            .filter(|_| parsed.transparency.is_none());
        for pixel in pixels.chunks_exact(channels) {
            for &value in &pixel[..colors] {
                main.push(match (&table, low_gray_bits) {
                    (None, _) | (Some(_), Some(1)) => value,
                    (Some(table), Some(bits)) => {
                        let top = table[usize::from(value)] >> (8 - bits);
                        (u16::from(top) * 255 / ((1u16 << bits) - 1)) as u8
                    }
                    (Some(table), None) => table[usize::from(value)],
                });
            }
            if has_alpha {
                alpha.push(pixel[colors]);
            }
        }
    }
    let natural = if colors == 1 { "/DeviceGray" } else { "/DeviceRGB" };
    let compressed = encode_flat_raster(&main, info.width, info.height, colors * out_bytes, options)?;
    if !(has_alpha && settings.alpha) {
        return Some(vec![compressed_raster_object(
            &settings.dict_start(),
            object,
            info.width,
            info.height,
            8 * out_bytes as u8,
            &settings.color_space(natural),
            colors,
            compressed,
            "",
        )]);
    }
    let mask_object = *next;
    let following = next.checked_add(1)?;
    let mask = encode_flat_raster(&alpha, info.width, info.height, 1, options)?;
    let objects = vec![
        compressed_raster_object(
            SMASK_START,
            mask_object,
            info.width,
            info.height,
            8,
            "/DeviceGray",
            1,
            mask,
            "",
        ),
        compressed_raster_object(
            &settings.dict_start(),
            object,
            info.width,
            info.height,
            8 * out_bytes as u8,
            &settings.color_space(natural),
            colors,
            compressed,
            &format!(" /SMask {mask_object} 0 R"),
        ),
    ];
    *next = following;
    Some(objects)
}

fn unfilter_row(filter: u8, source: &[u8], row: &mut [u8], previous: &[u8], bpp: usize) -> bool {
    if source.len() != row.len() || previous.len() != row.len() {
        return false;
    }
    for (at, &byte) in source.iter().enumerate() {
        let left = if at >= bpp { row[at - bpp] } else { 0 };
        let up = previous[at];
        let prediction = match filter {
            0 => 0,
            1 => left,
            2 => up,
            3 => ((left as u16 + up as u16) / 2) as u8,
            4 => paeth_predictor(
                left as i16,
                up as i16,
                if at >= bpp {
                    previous[at - bpp] as i16
                } else {
                    0
                },
            ),
            _ => return false,
        };
        row[at] = byte.wrapping_add(prediction);
    }
    true
}

fn embed_split_alpha(
    parsed: &ParsedPng<'_>,
    img_obj: i32,
    next_obj: &mut i32,
    components: usize,
    options: PngEmbedOptions,
    settings: &RasterSettings<'_>,
) -> Option<Vec<EmbeddedImage>> {
    if options.optimization == PngOptimization::Speed {
        return embed_split_alpha_fast(parsed, img_obj, next_obj, components, options, settings);
    }
    let channels = components.checked_add(1)?;
    let (source_row_bytes, _) = checked_raster_layout(parsed.width, parsed.height, channels)?;
    let main_row_bytes = usize::try_from(parsed.width)
        .ok()?
        .checked_mul(components)?;
    let alpha_row_bytes = usize::try_from(parsed.width).ok()?;
    let mut decoder = ZlibDecoder::new(IdatReader::new(&parsed.idat));
    let mut encoded_row = vec![0; source_row_bytes + 1];
    let mut current = vec![0; source_row_bytes];
    let mut previous = vec![0; source_row_bytes];
    let mut main = vec![0; main_row_bytes];
    let mut alpha = vec![0; alpha_row_bytes];
    let mut main_encoder = RasterEncoder::new(main_row_bytes, components, options);
    let mut alpha_encoder = RasterEncoder::new(alpha_row_bytes, 1, options);

    for _ in 0..parsed.height {
        decoder.read_exact(&mut encoded_row).ok()?;
        if !unfilter_row(
            encoded_row[0],
            &encoded_row[1..],
            &mut current,
            &previous,
            channels,
        ) {
            return None;
        }
        for pixel in 0..alpha_row_bytes {
            let source = pixel * channels;
            let target = pixel * components;
            main[target..target + components]
                .copy_from_slice(&current[source..source + components]);
            alpha[pixel] = current[source + components];
        }
        main_encoder.write_row(&main)?;
        alpha_encoder.write_row(&alpha)?;
        std::mem::swap(&mut current, &mut previous);
    }
    let mut extra = [0u8; 1];
    if decoder.read(&mut extra).ok()? != 0 {
        return None;
    }
    let main = main_encoder.finish()?;
    let alpha = alpha_encoder.finish()?;
    let smask_obj = *next_obj;
    let following = next_obj.checked_add(1)?;
    let smask = compressed_raster_object(
        SMASK_START,
        smask_obj,
        parsed.width,
        parsed.height,
        8,
        "/DeviceGray",
        1,
        alpha,
        "",
    );
    let image = compressed_raster_object(
        &settings.dict_start(),
        img_obj,
        parsed.width,
        parsed.height,
        8,
        &settings.color_space(if components == 1 { "/DeviceGray" } else { "/DeviceRGB" }),
        components,
        main,
        &format!(" /SMask {smask_obj} 0 R"),
    );
    *next_obj = following;
    Some(vec![smask, image])
}

/// Split a non-interlaced gray-alpha/RGBA stream without reconstructing and
/// filtering every pixel a second time. PNG filters operate independently on
/// each color component, so removing the alpha component from a filtered row
/// preserves filters 1-4 exactly. An unfiltered source row is changed to the
/// cheap Sub filter while its raw samples are already in hand; this avoids the
/// severe size penalty of forwarding filter 0 without doing a predictor search.
fn embed_split_alpha_fast(
    parsed: &ParsedPng<'_>,
    img_obj: i32,
    next_obj: &mut i32,
    components: usize,
    options: PngEmbedOptions,
    settings: &RasterSettings<'_>,
) -> Option<Vec<EmbeddedImage>> {
    let channels = components.checked_add(1)?;
    let (source_row_bytes, _) = checked_raster_layout(parsed.width, parsed.height, channels)?;
    let width = usize::try_from(parsed.width).ok()?;
    let main_row_bytes = width.checked_mul(components)?;
    let alpha_row_bytes = width;
    let mut decoder = ZlibDecoder::new(IdatReader::new(&parsed.idat));
    let mut source = vec![0; source_row_bytes + 1];
    let mut main = vec![0; main_row_bytes + 1];
    let mut alpha = vec![0; alpha_row_bytes + 1];
    let compression = Compression::new(options.compression_level.min(9));
    let mut main_encoder = ZlibEncoder::new(Vec::new(), compression);
    let mut alpha_encoder = ZlibEncoder::new(Vec::new(), compression);

    for _ in 0..parsed.height {
        decoder.read_exact(&mut source).ok()?;
        let source_filter = source[0];
        if source_filter > 4 {
            return None;
        }
        // Sub is essentially free while splitting raw samples and generally
        // prevents filter-0 rows from dominating the resulting PDF size.
        let output_filter = if source_filter == 0 { 1 } else { source_filter };
        main[0] = output_filter;
        alpha[0] = output_filter;
        for pixel in 0..width {
            let source_at = 1 + pixel * channels;
            let main_at = 1 + pixel * components;
            if source_filter == 0 {
                for channel in 0..components {
                    let value = source[source_at + channel];
                    let left = if pixel == 0 {
                        0
                    } else {
                        source[source_at - channels + channel]
                    };
                    main[main_at + channel] = value.wrapping_sub(left);
                }
                let value = source[source_at + components];
                let left = if pixel == 0 { 0 } else { source[source_at - 1] };
                alpha[pixel + 1] = value.wrapping_sub(left);
            } else {
                main[main_at..main_at + components]
                    .copy_from_slice(&source[source_at..source_at + components]);
                alpha[pixel + 1] = source[source_at + components];
            }
        }
        main_encoder.write_all(&main).ok()?;
        alpha_encoder.write_all(&alpha).ok()?;
    }
    let mut extra = [0u8; 1];
    if decoder.read(&mut extra).ok()? != 0 {
        return None;
    }
    let main = main_encoder.finish().ok()?;
    let alpha = alpha_encoder.finish().ok()?;
    let smask_obj = *next_obj;
    let following = next_obj.checked_add(1)?;
    let smask = compressed_raster_object(
        SMASK_START,
        smask_obj,
        parsed.width,
        parsed.height,
        8,
        "/DeviceGray",
        1,
        alpha,
        "",
    );
    let image = compressed_raster_object(
        &settings.dict_start(),
        img_obj,
        parsed.width,
        parsed.height,
        8,
        &settings.color_space(if components == 1 { "/DeviceGray" } else { "/DeviceRGB" }),
        components,
        main,
        &format!(" /SMask {smask_obj} 0 R"),
    );
    *next_obj = following;
    Some(vec![smask, image])
}

fn packed_row_bytes(width: u32, bit_depth: u8) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(bit_depth as usize)?
        .checked_add(7)
        .map(|bits| bits / 8)
}

fn validate_passthrough_idat(parsed: &ParsedPng<'_>) -> bool {
    let components = match parsed.color_type {
        0 | 3 => 1usize,
        2 => 3,
        _ => return false,
    };
    let Some(row_bytes) = usize::try_from(parsed.width)
        .ok()
        .and_then(|width| width.checked_mul(components))
        .and_then(|samples| samples.checked_mul(parsed.bit_depth as usize))
        .and_then(|bits| bits.checked_add(7))
        .map(|bits| bits / 8)
    else {
        return false;
    };
    let Some(row_stride) = row_bytes.checked_add(1) else {
        return false;
    };
    let Some(expected) = usize::try_from(parsed.height)
        .ok()
        .and_then(|height| row_stride.checked_mul(height))
        .filter(|total| *total <= MAX_DECODED_RASTER_BYTES)
    else {
        return false;
    };

    // BufRead keeps bytes after zlib's end marker visible, which lets us
    // reject concatenated/trailing compressed data. Decoded data is drained
    // through a fixed buffer rather than materializing the raster.
    let mut decoder = flate2::bufread::ZlibDecoder::new(IdatReader::new(&parsed.idat));
    let mut buffer = [0u8; 32 * 1024];
    let mut decoded = 0usize;
    let mut next_filter = 0usize;
    while decoded < expected {
        let wanted = (expected - decoded).min(buffer.len());
        let count = match decoder.read(&mut buffer[..wanted]) {
            Ok(0) | Err(_) => return false,
            Ok(count) => count,
        };
        let end = decoded + count;
        while next_filter < end {
            if buffer[next_filter - decoded] > 4 {
                return false;
            }
            next_filter += row_stride;
        }
        decoded = end;
    }

    // Reading once past the expected raster forces zlib to validate its end
    // marker and Adler-32 while also detecting extra decoded bytes.
    let mut extra = [0u8; 1];
    if !matches!(decoder.read(&mut extra), Ok(0)) {
        return false;
    }
    decoder
        .into_inner()
        .fill_buf()
        .is_ok_and(|remaining| remaining.is_empty())
}

fn indexed_alpha_stream(
    parsed: &ParsedPng<'_>,
    transparency: &[u8],
    options: PngEmbedOptions,
) -> Option<Vec<u8>> {
    let source_row_bytes = packed_row_bytes(parsed.width, parsed.bit_depth)?;
    let total = source_row_bytes
        .checked_add(1)?
        .checked_mul(usize::try_from(parsed.height).ok()?)?;
    if total > MAX_DECODED_RASTER_BYTES {
        return None;
    }
    checked_raster_layout(parsed.width, parsed.height, 1)?;
    let width = usize::try_from(parsed.width).ok()?;
    let mut decoder = ZlibDecoder::new(IdatReader::new(&parsed.idat));
    let mut encoded = vec![0; source_row_bytes + 1];
    let mut current = vec![0; source_row_bytes];
    let mut previous = vec![0; source_row_bytes];
    let mut alpha = vec![0; width];
    let mut alpha_encoder = RasterEncoder::new(width, 1, options);
    let sample_mask = (1u16 << parsed.bit_depth) - 1;
    let palette_entries = parsed.palette?.len() / 3;

    for _ in 0..parsed.height {
        decoder.read_exact(&mut encoded).ok()?;
        if !unfilter_row(encoded[0], &encoded[1..], &mut current, &previous, 1) {
            return None;
        }
        for (x, value) in alpha.iter_mut().enumerate() {
            let bit = x.checked_mul(parsed.bit_depth as usize)?;
            let byte = *current.get(bit / 8)?;
            let shift = 8usize.checked_sub(parsed.bit_depth as usize + bit % 8)?;
            let index = ((byte as u16 >> shift) & sample_mask) as usize;
            if index >= palette_entries {
                return None;
            }
            *value = transparency.get(index).copied().unwrap_or(255);
        }
        alpha_encoder.write_row(&alpha)?;
        std::mem::swap(&mut current, &mut previous);
    }
    let mut extra = [0u8; 1];
    if decoder.read(&mut extra).ok()? != 0 {
        return None;
    }
    alpha_encoder.finish()
}

fn palette_color_space(palette: &[u8]) -> String {
    let mut value = format!("[/Indexed /DeviceRGB {} <", palette.len() / 3 - 1);
    for byte in palette {
        write!(value, "{byte:02X}").unwrap();
    }
    value.push_str(">]");
    value
}

fn color_key(parsed: &ParsedPng<'_>) -> Option<String> {
    let value = parsed.transparency?;
    match parsed.color_type {
        0 => {
            let gray = u16::from_be_bytes(value.try_into().ok()?);
            let maximum = if parsed.bit_depth == 16 {
                u16::MAX
            } else {
                (1u16 << parsed.bit_depth) - 1
            };
            (gray <= maximum).then(|| format!(" /Mask [{gray} {gray}]"))
        }
        2 => {
            let red = u16::from_be_bytes(value[0..2].try_into().ok()?);
            let green = u16::from_be_bytes(value[2..4].try_into().ok()?);
            let blue = u16::from_be_bytes(value[4..6].try_into().ok()?);
            let maximum = if parsed.bit_depth == 16 {
                u16::MAX
            } else {
                (1u16 << parsed.bit_depth) - 1
            };
            (red <= maximum && green <= maximum && blue <= maximum)
                .then(|| format!(" /Mask [{red} {red} {green} {green} {blue} {blue}]"))
        }
        _ => None,
    }
}

fn embed_passthrough(
    parsed: &ParsedPng<'_>,
    object: i32,
    color_space: &str,
    colors: usize,
    extra: &str,
    settings: &RasterSettings<'_>,
) -> Option<EmbeddedImage> {
    if !validate_passthrough_idat(parsed) {
        return None;
    }
    Some(compressed_raster_object(
        &settings.dict_start(),
        object,
        parsed.width,
        parsed.height,
        parsed.bit_depth,
        &settings.color_space(color_space),
        colors,
        concatenate_idat(parsed)?,
        extra,
    ))
}

/// Embed a PNG as pdfTeX's writepng.c does under `settings`. Compatible
/// PNG IDAT streams are copied byte-for-byte; `options` control the
/// compression of rasters that need re-encoding.
pub fn embed_png_with_options(
    png_bytes: &[u8],
    img_obj: i32,
    next_obj: &mut i32,
    options: PngEmbedOptions,
    settings: &RasterSettings<'_>,
) -> Option<Vec<EmbeddedImage>> {
    let parsed = parse_png(png_bytes)?;
    if png_needs_decoding(&parsed, settings) {
        return embed_decoded_png(png_bytes, &parsed, img_obj, next_obj, options, settings);
    }

    match parsed.color_type {
        0 | 2 => {
            let extra = if parsed.transparency.is_some() {
                color_key(&parsed)?
            } else {
                String::new()
            };
            let colors = if parsed.color_type == 0 { 1 } else { 3 };
            let space = if colors == 1 {
                "/DeviceGray"
            } else {
                "/DeviceRGB"
            };
            Some(vec![embed_passthrough(
                &parsed, img_obj, space, colors, &extra, settings,
            )?])
        }
        3 => {
            let palette = parsed.palette?;
            let space = palette_color_space(palette);
            match indexed_transparency(parsed.transparency) {
                IndexedTransparency::Opaque => Some(vec![embed_passthrough(
                    &parsed, img_obj, &space, 1, "", settings,
                )?]),
                IndexedTransparency::ColorKey(first, last) => Some(vec![embed_passthrough(
                    &parsed,
                    img_obj,
                    &space,
                    1,
                    &format!(" /Mask [{first} {last}]"),
                    settings,
                )?]),
                IndexedTransparency::SoftMask => {
                    let alpha = indexed_alpha_stream(&parsed, parsed.transparency?, options)?;
                    let smask_obj = *next_obj;
                    let following = next_obj.checked_add(1)?;
                    let smask = compressed_raster_object(
                        SMASK_START,
                        smask_obj,
                        parsed.width,
                        parsed.height,
                        8,
                        "/DeviceGray",
                        1,
                        alpha,
                        "",
                    );
                    let image = embed_passthrough(
                        &parsed,
                        img_obj,
                        &space,
                        1,
                        &format!(" /SMask {smask_obj} 0 R"),
                        settings,
                    )?;
                    *next_obj = following;
                    Some(vec![smask, image])
                }
            }
        }
        4 => embed_split_alpha(&parsed, img_obj, next_obj, 1, options, settings),
        6 => embed_split_alpha(&parsed, img_obj, next_obj, 3, options, settings),
        _ => None,
    }
}

/// Embed a PNG with the default speed-oriented settings.
pub fn embed_png(png_bytes: &[u8], img_obj: i32, next_obj: &mut i32) -> Option<Vec<EmbeddedImage>> {
    embed_png_with_options(
        png_bytes,
        img_obj,
        next_obj,
        PngEmbedOptions::default(),
        &RasterSettings::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &[u8; 4], data: &[u8], out: &mut Vec<u8>) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.finalize().to_be_bytes());
    }

    fn png_with_metadata(
        color_type: u8,
        bit_depth: u8,
        w: u32,
        h: u32,
        palette: Option<&[u8]>,
        transparency: Option<&[u8]>,
        idats: &[&[u8]],
    ) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.push(bit_depth);
        ihdr.push(color_type);
        ihdr.extend_from_slice(&[0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(b"IHDR", &ihdr, &mut out);
        if let Some(palette) = palette {
            chunk(b"PLTE", palette, &mut out);
        }
        if let Some(transparency) = transparency {
            chunk(b"tRNS", transparency, &mut out);
        }
        for d in idats {
            chunk(b"IDAT", d, &mut out);
        }
        chunk(b"IEND", &[], &mut out);
        out
    }

    fn png(color_type: u8, w: u32, h: u32, idats: &[&[u8]]) -> Vec<u8> {
        png_with_metadata(color_type, 8, w, h, None, None, idats)
    }

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn inflate(data: &[u8]) -> Vec<u8> {
        let mut dec = ZlibDecoder::new(data);
        let mut v = Vec::new();
        dec.read_to_end(&mut v).unwrap();
        v
    }

    /// Independently encode `rgba` (h rows of w*4 bytes) applying filters[i]
    /// to row i, exactly per the PNG spec (wrapping subtraction).
    fn filter_rows(rgba: &[u8], w: usize, filters: &[u8]) -> Vec<u8> {
        let stride = w * 4;
        let mut out = Vec::new();
        for (r, &f) in filters.iter().enumerate() {
            let row = &rgba[r * stride..(r + 1) * stride];
            let prev: Vec<u8> = if r == 0 {
                vec![0u8; stride]
            } else {
                rgba[(r - 1) * stride..r * stride].to_vec()
            };
            out.push(f);
            for c in 0..stride {
                let left = if c >= 4 { row[c - 4] } else { 0 };
                let pred = match f {
                    0 => 0u8,
                    1 => left,
                    2 => prev[c],
                    3 => ((left as u16 + prev[c] as u16) / 2) as u8,
                    4 => paeth_predictor(
                        left as i16,
                        prev[c] as i16,
                        if c >= 4 { prev[c - 4] as i16 } else { 0 },
                    ),
                    _ => panic!("test filter must be 0..=4"),
                };
                out.push(row[c].wrapping_sub(pred));
            }
        }
        out
    }

    fn decoded_stream(obj: &EmbeddedImage) -> Vec<u8> {
        let b = &obj.bytes;
        let start = b.windows(7).position(|w| w == b"stream\n").unwrap() + 7;
        let end = start
            + b[start..]
                .windows(10)
                .position(|w| w == b"\nendstream")
                .unwrap();
        assert_eq!(&b[end..], b"\nendstream");
        inflate(&b[start..end])
    }

    fn compressed_stream(obj: &EmbeddedImage) -> &[u8] {
        let b = &obj.bytes;
        let start = b.windows(7).position(|w| w == b"stream\n").unwrap() + 7;
        let end = start
            + b[start..]
                .windows(10)
                .position(|w| w == b"\nendstream")
                .unwrap();
        &b[start..end]
    }

    fn predictor_pixels(obj: &EmbeddedImage, width: usize, height: usize, bpp: usize) -> Vec<u8> {
        let stream = decoded_stream(obj);
        let row_bytes = width * bpp;
        assert_eq!(stream.len(), height * (row_bytes + 1));
        let mut previous = vec![0; row_bytes];
        let mut current = vec![0; row_bytes];
        let mut pixels = Vec::with_capacity(height * row_bytes);
        for encoded in stream.chunks_exact(row_bytes + 1) {
            assert!(unfilter_row(
                encoded[0],
                &encoded[1..],
                &mut current,
                &previous,
                bpp,
            ));
            pixels.extend_from_slice(&current);
            std::mem::swap(&mut current, &mut previous);
        }
        pixels
    }

    fn test_pixels(w: usize, h: usize) -> Vec<u8> {
        let mut px = Vec::new();
        for r in 0..h {
            for x in 0..w {
                // Non-trivial alpha: varies per pixel, includes 0 and 255.
                let a = (r * 37 + x * 91) % 256;
                px.extend_from_slice(&[
                    (r * 200 + x * 13) as u8,
                    (x * 71 + r * 5) as u8,
                    255u8.wrapping_sub((r + x * 3) as u8),
                    if a < 3 {
                        0
                    } else if a > 250 {
                        255
                    } else {
                        a as u8
                    },
                ]);
            }
        }
        px
    }

    #[test]
    fn oversized_rgba_dimensions_do_not_consume_an_object() {
        // (4 * width + 1) * height is u64::MAX: the sentinel byte
        // must also fit before allocating either row buffer.
        let image = png(6, 1 << 30, u32::MAX, &[]);
        let mut next = 10;
        assert!(embed_png(&image, 1, &mut next).is_none());
        assert_eq!(next, 10);
    }

    #[test]
    fn embed_png_rgba_streams_all_filters_with_exact_pixels() {
        let (w, h) = (3usize, 5usize);
        let rgba = test_pixels(w, h);
        // Each of the five PNG row filters applied to exactly one row.
        let rows = filter_rows(&rgba, w, &[0, 1, 2, 3, 4]);
        let png_bytes = png(6, w as u32, h as u32, &[&deflate(&rows)]);

        let mut next_obj = 20i32;
        let out = embed_png(&png_bytes, 10, &mut next_obj).expect("valid RGBA PNG must embed");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].obj_num, 20); // soft mask consumed next_obj…
        assert_eq!(out[1].obj_num, 10); // …and the image keeps img_obj
        assert_eq!(next_obj, 21); // exactly one id consumed
        let expected_filters = [1, 1, 2, 3, 4];
        for (object, components) in [(&out[0], 1usize), (&out[1], 3usize)] {
            let encoded = decoded_stream(object);
            let row_bytes = w * components + 1;
            let filters: Vec<_> = encoded.chunks_exact(row_bytes).map(|row| row[0]).collect();
            assert_eq!(filters, expected_filters);
        }

        // PDF predictor rows may choose a new filter, but they must reconstruct
        // exactly to the source pixels.
        let rgb = predictor_pixels(&out[1], w, h, 3);
        for r in 0..h {
            for x in 0..w {
                assert_eq!(
                    &rgb[r * w * 3 + x * 3..r * w * 3 + x * 3 + 3],
                    &rgba[r * w * 4 + x * 4..r * w * 4 + x * 4 + 3],
                    "pixel ({r},{x}) RGB"
                );
            }
        }
        // The soft mask must carry the source alpha, unchanged.
        let alpha = predictor_pixels(&out[0], w, h, 1);
        for r in 0..h {
            for x in 0..w {
                assert_eq!(
                    alpha[r * w + x],
                    rgba[r * w * 4 + x * 4 + 3],
                    "pixel ({r},{x}) alpha"
                );
            }
        }
    }

    #[test]
    fn embed_png_rgba_multiple_idat_chunks_concatenates() {
        let (w, h) = (4usize, 6usize);
        let rgba = test_pixels(w, h);
        let rows = filter_rows(&rgba, w, &[4, 3, 2, 1, 0, 4]);
        let comp = deflate(&rows);
        let (head, tail) = comp.split_at(comp.len() / 2);
        let png_bytes = png(6, w as u32, h as u32, &[head, tail]);

        let mut next_obj = 31i32;
        let out = embed_png(&png_bytes, 44, &mut next_obj).expect("two-IDAT RGBA PNG must embed");
        assert_eq!(out.len(), 2);
        assert_eq!(next_obj, 32);
        // Same reconstruction as the single-chunk encoding.
        let png_single = png(6, w as u32, h as u32, &[&comp]);
        let mut next_single = 31i32;
        let out2 = embed_png(&png_single, 44, &mut next_single).unwrap();
        assert_eq!(decoded_stream(&out[1]), decoded_stream(&out2[1]));
        assert_eq!(decoded_stream(&out[0]), decoded_stream(&out2[0]));
    }

    /// pdfTeX writepng.c `write_png_rgb_alpha`: an RGBA PNG always gets an
    /// /SMask, even when every alpha sample is opaque (viewers composite it
    /// in a transparency group, so dropping it changes the rendering)
    #[test]
    fn opaque_rgba_keeps_its_soft_mask_like_pdftex() {
        let (width, height) = (5usize, 4usize);
        let mut rgba = test_pixels(width, height);
        for alpha in rgba.iter_mut().skip(3).step_by(4) {
            *alpha = 255;
        }
        let rows = filter_rows(&rgba, width, &[0, 2, 3, 4]);
        let image = png(6, width as u32, height as u32, &[&deflate(&rows)]);
        let mut next = 30;
        let objects = embed_png(&image, 9, &mut next).expect("valid opaque RGBA PNG");
        assert_eq!(objects.len(), 2);
        assert_eq!((objects[0].obj_num, objects[1].obj_num), (30, 9));
        assert_eq!(next, 31);
        assert!(String::from_utf8_lossy(&objects[1].bytes).contains("/SMask 30 0 R"));
        let rgb = predictor_pixels(&objects[1], width, height, 3);
        for (actual, source) in rgb.chunks_exact(3).zip(rgba.chunks_exact(4)) {
            assert_eq!(actual, &source[..3]);
        }
    }

    #[test]
    fn empty_idat_chunks_are_permitted() {
        let (width, height) = (2usize, 2usize);
        let rgba = test_pixels(width, height);
        let rows = filter_rows(&rgba, width, &[1, 4]);
        let compressed = deflate(&rows);
        let (head, tail) = compressed.split_at(compressed.len() / 2);
        let image = png(6, width as u32, height as u32, &[&[], head, &[], tail, &[]]);
        let mut next = 30;
        let objects = embed_png(&image, 9, &mut next).expect("empty IDAT chunks are legal");
        assert_eq!(predictor_pixels(&objects[1], width, height, 3).len(), 12);

        // The pass-through path also concatenates around empty chunks.
        let opaque_stream = deflate(&[0, 173]);
        let (opaque_head, opaque_tail) = opaque_stream.split_at(opaque_stream.len() / 2);
        let opaque = png(0, 1, 1, &[&[], opaque_head, &[], opaque_tail]);
        let mut next = 30;
        let objects = embed_png(&opaque, 9, &mut next).unwrap();
        assert_eq!(compressed_stream(&objects[0]), opaque_stream);
    }

    #[test]
    fn embed_png_rgba_corruption_fails_without_consuming_object_id() {
        let (w, h) = (3usize, 4usize);
        let rgba = test_pixels(w, h);
        let rows = filter_rows(&rgba, w, &[0, 1, 2, 4]);
        let comp = deflate(&rows);

        let mut bad_comp = comp.clone();
        if let Some(b) = bad_comp.last_mut() {
            *b ^= 0xFF; // corrupt the zlib adler32 trailer
        }
        let mut short_rows = rows.clone();
        short_rows.truncate(short_rows.len() - (1 + w * 4)); // drop the final row

        let cases: Vec<(&str, Vec<u8>)> = vec![
            (
                "truncated compressed stream",
                png(6, w as u32, h as u32, &[&comp[..comp.len() - 3]]),
            ),
            (
                "truncated pixel data",
                png(6, w as u32, h as u32, &[&deflate(&short_rows)]),
            ),
            (
                "bad zlib checksum",
                png(6, w as u32, h as u32, &[&bad_comp]),
            ),
            ("invalid filter byte", {
                let mut f = rows.clone();
                f[0] = 9;
                png(6, w as u32, h as u32, &[&deflate(&f)])
            }),
            ("one extra decoded byte", {
                let mut extra_rows = rows.clone();
                extra_rows.push(0x5A);
                png(6, w as u32, h as u32, &[&deflate(&extra_rows)])
            }),
        ];
        for (name, png_bytes) in cases {
            let mut next_obj = 60i32;
            let r = embed_png(&png_bytes, 50, &mut next_obj);
            assert!(r.is_none(), "{name}: must return None");
            assert_eq!(next_obj, 60, "{name}: must not consume an object id");
        }
        // Sanity: the uncorrupted base in this fixture embeds fine.
        let mut next_obj = 60i32;
        assert!(embed_png(&png(6, w as u32, h as u32, &[&comp]), 50, &mut next_obj).is_some());
        assert_eq!(next_obj, 61);
    }

    #[test]
    fn embed_png_gray_and_rgb_passthrough_idat_bytes() {
        // Color types 0 and 2 must forward the (concatenated) IDAT bytes verbatim.
        for color_type in [0u8, 2u8] {
            let components = if color_type == 0 { 1 } else { 3 };
            let mut rows = Vec::new();
            for row in 0..2 {
                rows.push(row);
                rows.extend((0..2 * components).map(|sample| (17 * row + sample) as u8));
            }
            let payload = deflate(&rows);
            let mut next_obj = 5i32;
            let out = embed_png(&png(color_type, 2, 2, &[&payload]), 7, &mut next_obj).unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].obj_num, 7);
            assert_eq!(next_obj, 5, "no soft mask id for opaque types");
            let b = &out[0].bytes;
            let start = b.windows(7).position(|w| w == b"stream\n").unwrap() + 7;
            let end = start
                + b[start..]
                    .windows(10)
                    .position(|w| w == b"\nendstream")
                    .unwrap();
            assert_eq!(&b[start..end], &payload[..]);
            assert!(std::str::from_utf8(&b[..start])
                .unwrap()
                .contains(&format!("/Length {} >>", payload.len())));
        }
        // Split across two IDAT chunks: identical concatenated passthrough.
        let payload = deflate(&[0, 10, 20, 0, 30, 40]);
        let (head, tail) = payload.split_at(payload.len() / 2);
        let mut next_obj = 5i32;
        let out = embed_png(&png(0, 2, 2, &[head, tail]), 7, &mut next_obj).unwrap();
        let b = &out[0].bytes;
        let start = b.windows(7).position(|w| w == b"stream\n").unwrap() + 7;
        let end = start
            + b[start..]
                .windows(10)
                .position(|w| w == b"\nendstream")
                .unwrap();
        assert_eq!(&b[start..end], &payload[..]);
    }

    #[test]
    fn passthrough_png_rejects_corrupt_rgb_and_indexed_streams() {
        let palette = [255, 0, 0, 0, 255, 0, 0, 0, 255];
        let valid_rows = [
            (2u8, vec![0, 1, 2, 3, 4, 5, 6]),
            (3, vec![0, 0b00_01_10_01]),
        ];

        for (color_type, rows) in valid_rows {
            let valid = deflate(&rows);
            let make_png = |idat: &[u8]| {
                png_with_metadata(
                    color_type,
                    if color_type == 3 { 2 } else { 8 },
                    if color_type == 3 { 4 } else { 2 },
                    1,
                    (color_type == 3).then_some(palette.as_slice()),
                    None,
                    &[idat],
                )
            };

            let mut bad_checksum = valid.clone();
            *bad_checksum.last_mut().unwrap() ^= 0x80;
            let short_rows = deflate(&rows[..rows.len() - 1]);
            let mut extra_rows = rows.clone();
            extra_rows.push(0x5a);
            let extra_rows = deflate(&extra_rows);
            let mut bad_filter = rows.clone();
            bad_filter[0] = 5;
            let bad_filter = deflate(&bad_filter);
            let mut trailing_stream = valid.clone();
            trailing_stream.push(0x5a);

            for (name, idat) in [
                ("truncated zlib stream", &valid[..valid.len() - 2]),
                ("bad zlib checksum", bad_checksum.as_slice()),
                ("short decoded raster", short_rows.as_slice()),
                ("extra decoded byte", extra_rows.as_slice()),
                ("invalid filter", bad_filter.as_slice()),
                ("data after zlib end", trailing_stream.as_slice()),
            ] {
                let mut next = 40;
                assert!(
                    embed_png(&make_png(idat), 7, &mut next).is_none(),
                    "color type {color_type}: {name}"
                );
                assert_eq!(next, 40, "color type {color_type}: {name}");
            }

            let mut next = 40;
            assert!(embed_png(&make_png(&valid), 7, &mut next).is_some());
        }
    }

    #[test]
    fn png_chunk_crc_corruption_is_rejected() {
        let idat = deflate(&[0, 1, 2, 3]);
        let mut image = png(2, 1, 1, &[&idat]);
        let idat_crc =
            image.windows(4).position(|bytes| bytes == b"IDAT").unwrap() + 4 + idat.len();
        image[idat_crc] ^= 0x80;

        let mut next = 40;
        assert!(embed_png(&image, 7, &mut next).is_none());
        assert_eq!(next, 40);
    }

    #[test]
    fn indexed_png_preserves_palette_and_idat() {
        // Four 2-bit palette indices: 0, 1, 2, 1.
        let scanline = [0, 0b00_01_10_01];
        let idat = deflate(&scanline);
        let palette = [255, 0, 0, 0, 255, 0, 0, 0, 255];
        let image = png_with_metadata(3, 2, 4, 1, Some(&palette), None, &[&idat]);
        let mut next = 40;
        let objects = embed_png(&image, 7, &mut next).expect("valid indexed PNG");
        assert_eq!(objects.len(), 1);
        assert_eq!(next, 40);
        assert_eq!(compressed_stream(&objects[0]), idat);
        let dictionary = String::from_utf8_lossy(&objects[0].bytes);
        assert!(dictionary.contains("/BitsPerComponent 2"));
        assert!(dictionary.contains("/ColorSpace [/Indexed /DeviceRGB 2 <FF000000FF000000FF>]"));
        assert!(dictionary.contains("/Colors 1 /BitsPerComponent 2"));
    }

    #[test]
    fn indexed_binary_transparency_uses_inline_color_key() {
        let idat = deflate(&[0, 0b00_01_10_01]);
        let palette = [255, 0, 0, 0, 255, 0, 0, 0, 255];
        // Indices 1 and 2 are a contiguous, fully-transparent range.
        let transparency = [255, 0, 0];
        let image = png_with_metadata(3, 2, 4, 1, Some(&palette), Some(&transparency), &[&idat]);
        assert!(!png_needs_soft_mask(&image, &RasterSettings::default()));
        let mut next = 40;
        let objects = embed_png(&image, 7, &mut next).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(next, 40);
        assert!(String::from_utf8_lossy(&objects[0].bytes).contains("/Mask [1 2]"));
        assert_eq!(compressed_stream(&objects[0]), idat);
    }

    #[test]
    fn indexed_partial_transparency_preserves_idat_and_builds_exact_smask() {
        let idat = deflate(&[0, 0b00_01_10_01]);
        let palette = [255, 0, 0, 0, 255, 0, 0, 0, 255];
        let transparency = [0, 128]; // index 2 and later default to opaque
        let image = png_with_metadata(3, 2, 4, 1, Some(&palette), Some(&transparency), &[&idat]);
        assert!(png_needs_soft_mask(&image, &RasterSettings::default()));
        let mut next = 40;
        let objects = embed_png(&image, 7, &mut next).unwrap();
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].obj_num, 40);
        assert_eq!(objects[1].obj_num, 7);
        assert_eq!(next, 41);
        assert_eq!(predictor_pixels(&objects[0], 4, 1, 1), [0, 128, 255, 128]);
        assert_eq!(compressed_stream(&objects[1]), idat);
        assert!(String::from_utf8_lossy(&objects[1].bytes).contains("/SMask 40 0 R"));
    }

    #[test]
    fn grayscale_color_key_keeps_original_stream() {
        let idat = deflate(&[0, 17, 18]);
        let transparent = 17u16.to_be_bytes();
        let image = png_with_metadata(0, 8, 2, 1, None, Some(&transparent), &[&idat]);
        assert!(!png_needs_soft_mask(&image, &RasterSettings::default()));
        let mut next = 9;
        let objects = embed_png(&image, 3, &mut next).unwrap();
        assert_eq!(next, 9);
        assert_eq!(compressed_stream(&objects[0]), idat);
        assert!(String::from_utf8_lossy(&objects[0].bytes).contains("/Mask [17 17]"));
    }

    #[test]
    fn indexed_malformed_inputs_fail_without_consuming_object() {
        let idat = deflate(&[0, 0b11_00_00_00]);
        let palette = [255, 0, 0, 0, 255, 0];
        let invalid_cases = [
            // Indexed images require a palette.
            png_with_metadata(3, 2, 1, 1, None, None, &[&idat]),
            // Palette byte count must be a multiple of three.
            png_with_metadata(3, 2, 1, 1, Some(&[1, 2]), None, &[&idat]),
            // Partial alpha forces index decoding; index 3 is outside this
            // two-entry palette and must not be silently clamped.
            png_with_metadata(3, 2, 1, 1, Some(&palette), Some(&[128]), &[&idat]),
        ];
        for image in invalid_cases {
            let mut next = 55;
            assert!(embed_png(&image, 4, &mut next).is_none());
            assert_eq!(next, 55);
        }
    }

    #[test]
    fn decoded_raster_bound_rejects_large_declared_image_before_allocation() {
        let empty_zlib = deflate(&[]);
        let image = png(6, 200_000_000, 1, &[&empty_zlib]);
        let mut next = 12;
        assert!(embed_png(&image, 4, &mut next).is_none());
        assert_eq!(next, 12);
    }

    #[test]
    fn rgba_predictors_reduce_size_and_compression_level_is_honored() {
        let (width, height) = (192usize, 128usize);
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[
                    (x / 2) as u8,
                    (y / 2) as u8,
                    ((x + y) / 3) as u8,
                    (128 + (y % 2) * 127) as u8,
                ]);
            }
        }
        let source = filter_rows(&rgba, width, &vec![0; height]);
        let image = png(6, width as u32, height as u32, &[&deflate(&source)]);

        let mut next = 20;
        let fast =
            embed_png_with_options(
                &image,
                10,
                &mut next,
                PngEmbedOptions::speed(1),
                &RasterSettings::default(),
            )
            .unwrap();
        let fast_bytes: usize = fast
            .iter()
            .map(|object| compressed_stream(object).len())
            .sum();

        // Reproduce the old RGBA path: filter byte 0 for every split row and
        // zlib level 1. The new speed path must be materially smaller without
        // adding a second whole-image compression pass.
        let mut old_rgb = Vec::with_capacity(height * (1 + width * 3));
        let mut old_alpha = Vec::with_capacity(height * (1 + width));
        for row in rgba.chunks_exact(width * 4) {
            old_rgb.push(0);
            old_alpha.push(0);
            for pixel in row.chunks_exact(4) {
                old_rgb.extend_from_slice(&pixel[..3]);
                old_alpha.push(pixel[3]);
            }
        }
        let old_bytes = deflate(&old_rgb).len() + deflate(&old_alpha).len();
        assert!(
            fast_bytes * 2 < old_bytes,
            "{fast_bytes} versus {old_bytes}"
        );

        let mut next = 20;
        let uncompressed =
            embed_png_with_options(
                &image,
                10,
                &mut next,
                PngEmbedOptions::speed(0),
                &RasterSettings::default(),
            )
            .unwrap();
        let uncompressed_bytes: usize = uncompressed
            .iter()
            .map(|object| compressed_stream(object).len())
            .sum();
        let mut next = 20;
        let compact =
            embed_png_with_options(
                &image,
                10,
                &mut next,
                PngEmbedOptions::size(9),
                &RasterSettings::default(),
            )
            .unwrap();
        let compact_bytes: usize = compact
            .iter()
            .map(|object| compressed_stream(object).len())
            .sum();
        assert!(compact_bytes < uncompressed_bytes / 4);
    }

    #[test]
    fn png_with_trailing_data_after_iend_is_accepted() {
        let (w, h) = (2usize, 2usize);
        let rgba = test_pixels(w, h);
        let rows = filter_rows(&rgba, w, &[0, 0]);
        let deflated = deflate(&rows);
        let mut test_png = png(6, w as u32, h as u32, &[&deflated]);
        // Append trailing metadata / padding bytes after IEND
        test_png.extend_from_slice(b"TRAILING_PADDING_AFTER_IEND");
        let parsed = parse_png(&test_png).expect("PNG with trailing data after IEND should parse");
        assert_eq!(parsed.width, 2);
        assert_eq!(parsed.height, 2);
        let mut next = 10;
        assert!(embed_png(&test_png, 1, &mut next).is_some());
    }

    #[test]
    fn checked_raster_layout_supports_large_images() {
        // 14400 x 10800 RGBA is ~622 MB uncompressed raster
        assert!(checked_raster_layout(14400, 10800, 4).is_some());
        // 24899 x 7534 RGBA is ~750 MB uncompressed raster
        assert!(checked_raster_layout(24899, 7534, 4).is_some());
    }

    fn include_first_page(bytes: &[u8], page_box: i32) -> Result<PdfInclusion, String> {
        let mut next = 10;
        let mut fonts = std::collections::BTreeMap::new();
        let mut no_type1_font = |_: &str| None;
        include_pdf_page(
            bytes,
            &PdfIncludeOptions {
                page: PdfPageSelector::Number(1),
                page_box,
                file_name: "./t.pdf",
                suppress_ptex_info: 0,
                ptex_underscore: false,
            },
            &mut next,
            &mut fonts,
            &mut no_type1_font,
        )
    }

    #[test]
    fn pdf_page_box_with_inverted_coordinates_is_normalized() {
        let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /CropBox [300 400 100 200] >>\nendobj\n\
xref\n0 4\n0000000000 65535 f \n\
0000000009 00000 n \n\
0000000058 00000 n \n\
0000000115 00000 n \n\
trailer\n<< /Size 4 /Root 1 0 R >>\n\
startxref\n220\n%%EOF\n";
        let page = include_first_page(pdf_data, PDF_BOX_SPEC_CROP)
            .expect("inverted CropBox should be normalized");
        assert_eq!((page.width, page.height), (200.0, 200.0));
        assert_eq!((page.orig_x, page.orig_y), (100.0, 200.0));
    }

    #[test]
    fn pdf_with_empty_xref_section_is_repaired() {
        let raw = b"xref\ntrailer\n<< /Size 0 >>\n";
        let repaired = repair_xref_empty_section(raw).expect("empty xref should be repaired");
        assert!(repaired.starts_with(b"xref\n0 0\ntrailer"));
    }

    #[test]
    fn pdf_with_shifted_xref_is_rebuilt() {
        let pdf_data = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>\nendobj\n\
trailer\n<< /Size 4 /Root 1 0 R >>\n\
startxref\n99999\n%%EOF\n";
        let rebuilt = repair_pdf_xref_rebuild(pdf_data).expect("xref should be rebuilt");
        let res = include_first_page(&rebuilt, PDF_BOX_SPEC_MEDIA);
        assert!(res.is_ok(), "rebuilt PDF should import: {:?}", res.err());
    }
}
