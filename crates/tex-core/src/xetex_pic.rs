//! XeTeX picture files: `\XeTeXpicfile`/`\XeTeXpdffile` (`load_picture`,
//! xetex.web), `find_pic_file` (XeTeX_pic.c), `pdf_get_rect`/`pdf_count_pages`
//! (pdfimage.cpp), `\XeTeXpdfpagecount`, the `pic_out` special text, and the
//! image loader the xdvipdfmx `pdf:image` special handler uses
//! ([`Engine::dpx_load_image`]).

use crate::boxes::{
    Node, WhatIt, PDFBOX_ART, PDFBOX_BLEED, PDFBOX_CROP, PDFBOX_MEDIA, PDFBOX_NONE, PDFBOX_TRIM,
};
use crate::engine::{Engine, ImageKind, PdfFixedParams};
use crate::maincontrol::{ImageFail, ImageReadRequest};
use crate::pdf_images::{
    PDF_BOX_SPEC_ART, PDF_BOX_SPEC_BLEED, PDF_BOX_SPEC_CROP, PDF_BOX_SPEC_MEDIA, PDF_BOX_SPEC_TRIM,
};

/// One image registered for xdvipdfmx's `pdf:image`.
#[derive(Clone, Debug)]
pub(crate) struct DpxImage {
    /// The `pdf_images` entry (same file, page and box give the same object).
    pub obj: i32,
    pub kind: ImageKind,
    /// Pixel size of a raster image (0 for PDF pages).
    pub px_w: i32,
    pub px_h: i32,
    /// dvipdfmx's bp per pixel: 72/dpi from JFIF/Exif/pHYs, else 1.0.
    pub xdensity: f64,
    pub ydensity: f64,
    /// The included PDF page box in bp after `/Rotate`, in form space
    /// (`pdf_ximage_set_form`); zeros for raster images.
    pub bbox: [f64; 4],
}

// ---------------------------------------------------------------------------
// trans.h / D2Fix

/// trans.h `transform`.
#[derive(Clone, Copy)]
struct Transform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    x: f64,
    y: f64,
}

impl Transform {
    const IDENTITY: Transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, x: 0.0, y: 0.0 };

    fn scale(xscale: f64, yscale: f64) -> Transform {
        Transform { a: xscale, d: yscale, ..Self::IDENTITY }
    }

    fn translation(dx: f64, dy: f64) -> Transform {
        Transform { x: dx, y: dy, ..Self::IDENTITY }
    }

    fn rotation(angle: f64) -> Transform {
        let (s, c) = angle.sin_cos();
        Transform { a: c, b: s, c: -s, d: c, x: 0.0, y: 0.0 }
    }

    /// `transform_point` on a `realpoint` (single precision, like trans.h).
    fn apply(&self, p: (f32, f32)) -> (f32, f32) {
        let (x, y) = (f64::from(p.0), f64::from(p.1));
        ((self.a * x + self.c * y + self.x) as f32, (self.b * x + self.d * y + self.y) as f32)
    }

    /// `transform_concat(self, t2)`: `self := self * t2`.
    fn concat(&mut self, t2: &Transform) {
        let t1 = *self;
        *self = Transform {
            a: t1.a * t2.a + t1.b * t2.c,
            b: t1.a * t2.b + t1.b * t2.d,
            c: t1.c * t2.a + t1.d * t2.c,
            d: t1.c * t2.b + t1.d * t2.d,
            x: t1.x * t2.a + t1.y * t2.c + t2.x,
            y: t1.x * t2.b + t1.y * t2.d + t2.y,
        };
    }
}

/// XeTeX_ext.c `D2Fix`: `(int)(d * 65536.0 + 0.5)`; C's out-of-range
/// conversion on x86 yields `INT_MIN`.
fn d2fix(d: f64) -> i32 {
    let v = (d * 65536.0 + 0.5).trunc();
    if v.is_finite() && v >= f64::from(i32::MIN) && v <= f64::from(i32::MAX) {
        v as i32
    } else {
        i32::MIN
    }
}

fn fix2d(f: i32) -> f64 {
    f64::from(f) / 65536.0
}

/// The state of `load_picture`'s transform loop: `corners`, `t`, and the
/// pending `x_size_req`/`y_size_req`.
struct PicTransform {
    corners: [(f32, f32); 4],
    t: Transform,
    x_size_req: f64,
    y_size_req: f64,
}

impl PicTransform {
    /// `calc_min_and_max`: (xmin, xmax, ymin, ymax).
    fn min_max(&self) -> (f64, f64, f64, f64) {
        let (mut xmin, mut xmax, mut ymin, mut ymax) = (1_000_000.0f64, -1_000_000.0f64, 1_000_000.0f64, -1_000_000.0f64);
        for &(x, y) in &self.corners {
            let (x, y) = (f64::from(x), f64::from(y));
            xmin = xmin.min(x);
            xmax = xmax.max(x);
            ymin = ymin.min(y);
            ymax = ymax.max(y);
        }
        (xmin, xmax, ymin, ymax)
    }

    /// `update_corners`
    fn update_corners(&mut self, t2: &Transform) {
        for corner in &mut self.corners {
            *corner = t2.apply(*corner);
        }
    }

    /// `update_corners; transform_concat(t, t2)` of the scaled keywords.
    fn apply(&mut self, t2: Transform) {
        self.update_corners(&t2);
        self.t.concat(&t2);
    }

    /// `do_size_requests`
    fn do_size_requests(&mut self) {
        let (xmin, xmax, ymin, ymax) = self.min_max();
        let t2 = if self.x_size_req == 0.0 {
            let s = self.y_size_req / (ymax - ymin);
            Transform::scale(s, s)
        } else if self.y_size_req == 0.0 {
            let s = self.x_size_req / (xmax - xmin);
            Transform::scale(s, s)
        } else {
            Transform::scale(self.x_size_req / (xmax - xmin), self.y_size_req / (ymax - ymin))
        };
        self.apply(t2);
        self.x_size_req = 0.0;
        self.y_size_req = 0.0;
    }
}

// ---------------------------------------------------------------------------
// pic_out

/// xetex.web `pic_out`: the `pdf:image` special a picture node becomes.
pub(crate) fn pic_out_text(w: &WhatIt) -> String {
    let WhatIt::XePic { path, page, pdf_box, transform, .. } = w else {
        return String::new();
    };
    let mut out = String::from("pdf:image matrix ");
    for value in transform {
        out.push_str(&crate::build::print_scaled(i64::from(*value)));
        out.push(' ');
    }
    out.push_str("page ");
    out.push_str(&page.to_string());
    out.push(' ');
    out.push_str(match *pdf_box {
        PDFBOX_CROP => "pagebox cropbox ",
        PDFBOX_MEDIA => "pagebox mediabox ",
        PDFBOX_BLEED => "pagebox bleedbox ",
        PDFBOX_ART => "pagebox artbox ",
        PDFBOX_TRIM => "pagebox trimbox ",
        _ => "",
    });
    out.push('(');
    out.push_str(path);
    out.push(')');
    out
}

// ---------------------------------------------------------------------------
// Raster headers (XeTeX's image/{jpeg,png,bmp}image.c `*_scan_file`)

/// Resolution in dpi as the header gives it; `None` when the file does not
/// say (XeTeX then assumes 72, dvipdfmx a density of 1.0).
type Dpi = Option<(f64, f64)>;

fn jpeg_dpi(bytes: &[u8]) -> Dpi {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| f64::from(u16::from_be_bytes([b[0], b[1]])));
    match bytes.get(2..4)? {
        // JFIF: density in dots per inch (units 1) or per centimetre (2)
        [0xff, 0xe0] if bytes.get(6..11) == Some(b"JFIF\0") => {
            let (x, y) = (u16_at(14)?, u16_at(16)?);
            let (x, y) = match *bytes.get(13)? {
                1 => (x, y),
                2 => (x * 2.54, y * 2.54),
                _ => return None,
            };
            (x != 0.0 || y != 0.0).then_some((x, y))
        }
        [0xff, 0xe1] => {
            let length = u16_at(4)? as usize;
            let length = length.wrapping_sub(2) as u16 as usize;
            if length > 5 && bytes.get(6..11) == Some(b"Exif\0") {
                let end = (11 + length - 5).min(bytes.len());
                let (x, y) = crate::pdf_images::exif_resolution(&bytes[11..end]);
                (x != 0 || y != 0).then_some((f64::from(x), f64::from(y)))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn png_dpi(bytes: &[u8]) -> Dpi {
    let mut at = 8usize;
    while let Some(header) = bytes.get(at..at.checked_add(8)?) {
        let length = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
        let data = at + 8;
        match &header[4..8] {
            b"pHYs" if length >= 9 => {
                let phys = bytes.get(data..data + 9)?;
                if phys[8] != 1 {
                    return None;
                }
                let ppm = |b: &[u8]| f64::from(u32::from_be_bytes(b.try_into().unwrap()));
                return Some((ppm(&phys[0..4]) * 0.0254, ppm(&phys[4..8]) * 0.0254));
            }
            b"IDAT" | b"IEND" => return None,
            _ => {}
        }
        at = data.checked_add(length)?.checked_add(4)?;
    }
    None
}

/// `bmp_scan_file`: (width, height, dpi).
fn bmp_info(bytes: &[u8]) -> Option<(i32, i32, Dpi)> {
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    match u32_at(14)? {
        12 => {
            let w = i32::from(u16::from_le_bytes(bytes.get(18..20)?.try_into().ok()?));
            let h = i32::from(u16::from_le_bytes(bytes.get(20..22)?.try_into().ok()?));
            Some((w, h, None))
        }
        40.. => {
            let w = u32_at(18)? as i32;
            let h = (u32_at(22)? as i32).wrapping_abs();
            let (px, py) = (f64::from(u32_at(38)?), f64::from(u32_at(42)?));
            let dpi = (px != 0.0 && py != 0.0).then_some((px * 0.0254, py * 0.0254));
            Some((w, h, dpi))
        }
        _ => None,
    }
}

/// Densities for [`DpxImage`] (dvipdfmx: bp per pixel, 1.0 without a resolution).
pub(crate) fn raster_density(bytes: &[u8]) -> (f64, f64) {
    let dpi = if bytes.starts_with(&[0xff, 0xd8]) {
        jpeg_dpi(bytes)
    } else if bytes.starts_with(b"\x89PNG") {
        png_dpi(bytes)
    } else {
        None
    };
    match dpi {
        Some((x, y)) if x > 0.0 && y > 0.0 => (72.0 / x, 72.0 / y),
        _ => (1.0, 1.0),
    }
}

// ---------------------------------------------------------------------------
// Files

/// A located picture file (`kpse_find_file(.., kpse_pict_format, 1)`).
struct Pict {
    path: std::path::PathBuf,
    /// The name kpathsea reports (`./name` for the current directory).
    shown: String,
    bytes: Vec<u8>,
}

/// XeTeX_pic.c `realrect` (single precision), in TeX points.
#[derive(Clone, Copy, Default)]
struct RealRect {
    x: f32,
    y: f32,
    wd: f32,
    ht: f32,
}

fn dpi_extent(px_w: f64, px_h: f64, dpi: Dpi) -> RealRect {
    let (xdpi, ydpi) = dpi.unwrap_or((72.0, 72.0));
    RealRect { x: 0.0, y: 0.0, wd: (px_w * 72.27 / xdpi) as f32, ht: (px_h * 72.27 / ydpi) as f32 }
}

fn pdf_box_spec(pdf_box: u8) -> i32 {
    match pdf_box {
        PDFBOX_MEDIA => PDF_BOX_SPEC_MEDIA,
        PDFBOX_BLEED => PDF_BOX_SPEC_BLEED,
        PDFBOX_TRIM => PDF_BOX_SPEC_TRIM,
        PDFBOX_ART => PDF_BOX_SPEC_ART,
        _ => PDF_BOX_SPEC_CROP,
    }
}

impl Engine {
    /// Reads the picture `name` the way kpathsea's `kpse_pict_format` search
    /// ends up: an embedded archive path, a file found on the input paths, or
    /// a bundled package file. Unlike `\input`, no suffix is appended.
    fn find_pict(&mut self, name: &str) -> Option<Pict> {
        if name.is_empty() {
            return None;
        }
        let (path, bytes, bundled) = if tex_kpse::embedded_tree::is_embedded_path(name) {
            (std::path::PathBuf::from(name), tex_kpse::embedded_tree::read(name)?, true)
        } else if let Some(path) = self.resolve_input_path(name) {
            let bytes = tex_kpse::fs::read(&path).ok()?;
            self.record_loaded_bytes(&path, &bytes);
            self.loaded_files.push(path.clone());
            (path, bytes, false)
        } else if !std::path::Path::new(name).is_absolute() {
            (std::path::PathBuf::from(name), tex_kpse::get_embedded_package(name)?, true)
        } else {
            return None;
        };
        let shown = self.kpse_found_name(&path, bundled);
        Some(Pict { path, shown, bytes })
    }

    /// The parsed PDF `pict` (one `PdfSource` per file, shared with `\pdfximage`
    /// style inclusion).
    fn pict_pdf_source(&mut self, pict: &Pict) -> Option<&mut crate::pdf_images::PdfSource> {
        use std::collections::hash_map::Entry;
        let key = pict.path.to_string_lossy().into_owned();
        match self.pdf_doc.pdf_sources.entry(key) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                if !pict.bytes.starts_with(b"%PDF-") {
                    return None;
                }
                crate::pdf_images::PdfSource::open(&pict.bytes).ok().map(|source| entry.insert(source))
            }
        }
    }

    /// XeTeX_pic.c `find_pic_file` (+ pdfimage.cpp `pdf_get_rect`): the file
    /// and its bounds in TeX points, or `None` when it cannot be read.
    /// `pdf_box` is 0 for `\XeTeXpicfile`.
    fn find_pic_file(&mut self, name: &str, pdf_box: u8, page: i32) -> Option<(Pict, RealRect)> {
        let pict = self.find_pict(name)?;
        if pdf_box != 0 {
            let source = self.pict_pdf_source(&pict)?;
            let pages = source.page_count();
            let page = if page > pages { pages } else { page };
            let page = if page < 0 { pages + 1 + page } else { page }.max(1);
            let (rect, rotate) = source.xetex_page_box(page, pdf_box)?;
            let scale = 72.27 / 72.0;
            let (width, height) = ((rect[2] - rect[0]).abs(), (rect[3] - rect[1]).abs());
            let (wd, ht) = if rotate == 90 || rotate == 270 { (height, width) } else { (width, height) };
            let bounds = RealRect {
                x: (scale * rect[0].min(rect[2])) as f32,
                y: (scale * rect[1].min(rect[3])) as f32,
                wd: (scale * wd) as f32,
                ht: (scale * ht) as f32,
            };
            return Some((pict, bounds));
        }
        let bytes = &pict.bytes;
        let bounds = if bytes.starts_with(&[0xff, 0xd8]) {
            let info = crate::pdf_images::jpeg_info(bytes).ok()?;
            dpi_extent(f64::from(info.width), f64::from(info.height), jpeg_dpi(bytes))
        } else if bytes.starts_with(b"BM") {
            let (w, h, dpi) = bmp_info(bytes)?;
            dpi_extent(f64::from(w), f64::from(h), dpi)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            let info = crate::pdf_images::png_info(bytes)?;
            dpi_extent(f64::from(info.width), f64::from(info.height), png_dpi(bytes))
        } else {
            return None;
        };
        Some((pict, bounds))
    }

    /// xetex.web `load_picture(is_pdf)`: `\XeTeXpicfile`/`\XeTeXpdffile`.
    pub(crate) fn load_picture(&mut self, is_pdf: bool) {
        let name = self.scan_file_name();
        let mut page = 0;
        let mut pdf_box_type = 0u8;
        if is_pdf {
            if self.scan_keyword(b"page") {
                page = self.scan_int();
            }
            pdf_box_type = PDFBOX_NONE;
            if self.scan_keyword(b"crop") {
                pdf_box_type = PDFBOX_CROP;
            } else if self.scan_keyword(b"media") {
                pdf_box_type = PDFBOX_MEDIA;
            } else if self.scan_keyword(b"bleed") {
                pdf_box_type = PDFBOX_BLEED;
            } else if self.scan_keyword(b"trim") {
                pdf_box_type = PDFBOX_TRIM;
            } else if self.scan_keyword(b"art") {
                pdf_box_type = PDFBOX_ART;
            }
        }
        let found = self.find_pic_file(
            &name,
            if pdf_box_type == PDFBOX_NONE { PDFBOX_CROP } else { pdf_box_type },
            page,
        );
        let bounds = found.as_ref().map_or_else(RealRect::default, |(_, bounds)| *bounds);
        let mut pic = PicTransform {
            corners: [
                (bounds.x, bounds.y),
                (bounds.x, bounds.y + bounds.ht),
                (bounds.x + bounds.wd, bounds.y + bounds.ht),
                (bounds.x + bounds.wd, bounds.y),
            ],
            t: Transform::IDENTITY,
            x_size_req: 0.0,
            y_size_req: 0.0,
        };
        loop {
            if self.scan_keyword(b"scaled") {
                let v = self.scan_int();
                if pic.x_size_req == 0.0 && pic.y_size_req == 0.0 {
                    let s = f64::from(v) / 1000.0;
                    pic.apply(Transform::scale(s, s));
                }
            } else if self.scan_keyword(b"xscaled") {
                let v = self.scan_int();
                if pic.x_size_req == 0.0 && pic.y_size_req == 0.0 {
                    pic.apply(Transform::scale(f64::from(v) / 1000.0, 1.0));
                }
            } else if self.scan_keyword(b"yscaled") {
                let v = self.scan_int();
                if pic.x_size_req == 0.0 && pic.y_size_req == 0.0 {
                    pic.apply(Transform::scale(1.0, f64::from(v) / 1000.0));
                }
            } else if self.scan_keyword(b"width") {
                let v = self.scan_dimen(false, false);
                if v <= 0 {
                    self.improper_image_size(v);
                } else {
                    pic.x_size_req = fix2d(v);
                }
            } else if self.scan_keyword(b"height") {
                let v = self.scan_dimen(false, false);
                if v <= 0 {
                    self.improper_image_size(v);
                } else {
                    pic.y_size_req = fix2d(v);
                }
            } else if self.scan_keyword(b"rotated") {
                let v = self.scan_decimal();
                if pic.x_size_req != 0.0 || pic.y_size_req != 0.0 {
                    pic.do_size_requests();
                }
                let t2 = Transform::rotation(fix2d(v) * 3.141592653589793 / 180.0);
                pic.update_corners(&t2);
                let (xmin, xmax, ymin, ymax) = pic.min_max();
                pic.corners = [
                    (xmin as f32, ymin as f32),
                    (xmin as f32, ymax as f32),
                    (xmax as f32, ymax as f32),
                    (xmax as f32, ymin as f32),
                ];
                pic.t.concat(&t2);
            } else {
                break;
            }
        }
        if pic.x_size_req != 0.0 || pic.y_size_req != 0.0 {
            pic.do_size_requests();
        }
        let (xmin, xmax, ymin, ymax) = pic.min_max();
        pic.t.concat(&Transform::translation(-xmin * 72.0 / 72.27, -ymin * 72.0 / 72.27));

        match found {
            Some((pict, _)) => {
                let t = pic.t;
                let node = WhatIt::XePic {
                    pdf: is_pdf,
                    path: pict.shown,
                    // `pic_page` is a 16-bit quarterword
                    page: i32::from(page as u16),
                    pdf_box: pdf_box_type,
                    transform: [d2fix(t.a), d2fix(t.b), d2fix(t.c), d2fix(t.d), d2fix(t.x), d2fix(t.y)],
                    w: d2fix(xmax - xmin),
                    h: d2fix(ymax - ymin),
                    d: 0,
                };
                self.append_whatsit(Node::Whatsit(node, self.eqtb.cur_attr));
            }
            None => {
                let shown = if name.contains(' ') { format!("\"{name}\"") } else { name };
                self.error(&format!("Unable to load picture or PDF file '{shown}'"));
            }
        }
    }

    fn improper_image_size(&mut self, value: i32) {
        self.error(&format!(
            "Improper image size ({}pt) will be ignored",
            crate::build::print_scaled(i64::from(value))
        ));
    }

    /// xetex.web `scan_decimal`: a decimal fraction without units, as a
    /// 16.16 fixed value (`xetex_scan_dimen(false, false, false, false)`).
    fn scan_decimal(&mut self) -> i32 {
        let is_point = |t: crate::token::Token| t.is_char() && (t.chr() == u32::from(b'.') || t.chr() == u32::from(b','));
        let is_digit = |t: crate::token::Token| t.is_char() && (u32::from(b'0')..=u32::from(b'9')).contains(&t.chr());
        let mut negative = false;
        let first = loop {
            let t = self.get_x_raw();
            if t.is_space() || (t.is_char() && t.chr() == u32::from(b'+')) {
                continue;
            }
            if t.is_char() && t.chr() == u32::from(b'-') {
                negative = !negative;
                continue;
            }
            break t;
        };
        let value = if is_digit(first) || is_point(first) {
            self.push_token(first);
            let int_part = if is_point(first) { 0 } else { i64::from(self.scan_int()) };
            let mut fraction = 0i64;
            let next = self.get_x_raw();
            if is_point(next) {
                let mut digits = Vec::new();
                loop {
                    let t = self.get_x_raw();
                    if is_digit(t) {
                        if digits.len() < 17 {
                            digits.push((t.chr() - u32::from(b'0')) as i64);
                        }
                    } else {
                        if !t.is_space() {
                            self.push_token(t);
                        }
                        break;
                    }
                }
                // tex.web round_decimals
                let mut a = 0i64;
                for &d in digits.iter().rev() {
                    a = (a + d * 131_072) / 10;
                }
                fraction = (a + 1) / 2;
            } else {
                self.push_token(next);
            }
            if int_part >= 0x4000 {
                self.error("Dimension too large");
                0x3FFF_FFFF
            } else {
                (int_part * 65536 + fraction) as i32
            }
        } else {
            // an internal quantity: handled by the ordinary dimension scanner
            self.push_token(first);
            return if negative { -self.scan_dimen(false, false) } else { self.scan_dimen(false, false) };
        };
        if negative {
            -value
        } else {
            value
        }
    }

    /// `\XeTeXpdfpagecount <file name>`: `count_pdf_file_pages`.
    pub(crate) fn xetex_pdf_page_count(&mut self) -> i32 {
        let name = self.scan_file_name();
        let Some(pict) = self.find_pict(&name) else {
            return 0;
        };
        self.pict_pdf_source(&pict).map_or(0, |source| source.page_count())
    }

    /// The image loader behind xdvipdfmx's `pdf:image`: registers `name`
    /// (the path in the special) with the PDF writer and returns its
    /// dvipdfmx attributes. `page` follows `pdf_get_rect` (negative counts
    /// from the end, out of range clamps); `pdf_box` is a `PDFBOX_*` code
    /// (anything else means the crop box).
    pub(crate) fn dpx_load_image(&mut self, name: &str, page: i32, pdf_box: u8) -> Result<DpxImage, String> {
        let key = (name.to_owned(), page, pdf_box);
        if let Some(image) = self.xe_images.get(&key) {
            return Ok(image.clone());
        }
        let Some(pict) = self.find_pict(name) else {
            return Err(format!("Image file `{name}` was not found"));
        };
        let is_pdf = pict.bytes.starts_with(b"%PDF-");
        let mut page_no = page;
        if is_pdf {
            let pages = self.pict_pdf_source(&pict).map_or(0, |source| source.page_count());
            if pages > 0 {
                page_no = if page > pages { pages } else { page };
                page_no = if page_no < 0 { pages + 1 + page_no } else { page_no }.max(1);
            }
        }
        let fixed = self.pdf_fixed.unwrap_or(PdfFixedParams {
            major_version: 1,
            minor_version: 7,
            draftmode: 0,
            decimal_digits: 3,
            gamma: 0,
            image_gamma: 0,
            image_hicolor: true,
            image_apply_gamma: false,
            // xdvipdfmx embeds an included page's fonts as they are
            inclusion_copy_font: true,
        });
        let read = self
            .read_image_file(ImageReadRequest {
                file: name,
                lookup: name,
                from_callback: false,
                attr: None,
                colorspace: 0,
                named: None,
                page: page_no,
                page_box: pdf_box_spec(pdf_box),
                fixed,
            })
            .map_err(|failure| match failure {
                ImageFail::Error(message) | ImageFail::Fatal(message) | ImageFail::LuaRes(message) => message,
            })?;
        let info = read.info;
        let image = if info.kind == ImageKind::Pdf {
            DpxImage {
                obj: read.obj,
                kind: info.kind,
                px_w: 0,
                px_h: 0,
                xdensity: 1.0,
                ydensity: 1.0,
                bbox: read.page_box.map_or([0.0; 4], |page_box| rotated_bbox(page_box, info.rotate)),
            }
        } else {
            let (xdensity, ydensity) = raster_density(&pict.bytes);
            DpxImage {
                obj: read.obj,
                kind: info.kind,
                px_w: info.image_width,
                px_h: info.image_height,
                xdensity,
                ydensity,
                bbox: [0.0; 4],
            }
        };
        self.pdf_images.insert(read.obj, info);
        self.xe_images.insert(key, image.clone());
        Ok(image)
    }
}

/// `pdf_ximage_set_form`'s bbox: the page box through the `/Rotate` matrix of
/// `set_transform_matrix` (pdfdoc.c), as min/max of the four corners.
fn rotated_bbox([llx, lly, urx, ury]: [f64; 4], rotate: i32) -> [f64; 4] {
    // (a, b, c, d, e, f) with x' = a x + c y + e, y' = b x + d y + f
    let m = match rotate {
        90 => [0.0, -1.0, 1.0, 0.0, llx - lly, lly + urx],
        180 => [-1.0, 0.0, 0.0, -1.0, llx + urx, lly + ury],
        270 => [0.0, 1.0, -1.0, 0.0, llx + ury, lly - llx],
        _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    };
    let points = [(llx, lly), (urx, lly), (urx, ury), (llx, ury)]
        .map(|(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]));
    let fold = |pick: fn(&(f64, f64)) -> f64, op: fn(f64, f64) -> f64| {
        points.iter().map(pick).reduce(op).unwrap_or(0.0)
    };
    [
        fold(|p| p.0, f64::min),
        fold(|p| p.1, f64::min),
        fold(|p| p.0, f64::max),
        fold(|p| p.1, f64::max),
    ]
}
