use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
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
    pub dpi_x: f64,
    pub dpi_y: f64,
    pub adobe: bool,
}

pub fn jpeg_info(bytes: &[u8]) -> Option<JpegInfo> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut result = JpegInfo {
        width: 0,
        height: 0,
        components: 0,
        bits: 8,
        dpi_x: 72.0,
        dpi_y: 72.0,
        adobe: false,
    };
    let mut pos = 2;
    while pos + 1 < bytes.len() {
        if bytes[pos] != 0xff {
            return None;
        }
        while bytes.get(pos) == Some(&0xff) {
            pos += 1;
        }
        let marker = *bytes.get(pos)?;
        pos += 1;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let length = u16::from_be_bytes(bytes.get(pos..pos + 2)?.try_into().ok()?) as usize;
        if length < 2 {
            return None;
        }
        let data = bytes.get(pos + 2..pos.checked_add(length)?)?;
        match marker {
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                result.bits = *data.first()?;
                result.height = u16::from_be_bytes(data.get(1..3)?.try_into().ok()?);
                result.width = u16::from_be_bytes(data.get(3..5)?.try_into().ok()?);
                result.components = *data.get(5)?;
            }
            0xe0 if data.starts_with(b"JFIF\0") && data.len() >= 12 => {
                let x = u16::from_be_bytes([data[8], data[9]]) as f64;
                let y = u16::from_be_bytes([data[10], data[11]]) as f64;
                let scale = match data[7] {
                    1 => 1.0,
                    2 => 2.54,
                    _ => 0.0,
                };
                if scale > 0.0 && x > 0.0 && y > 0.0 {
                    result.dpi_x = x * scale;
                    result.dpi_y = y * scale;
                }
            }
            0xee if data.starts_with(b"Adobe") => result.adobe = true,
            _ => {}
        }
        pos += length;
    }
    (result.width > 0 && result.height > 0 && matches!(result.components, 1 | 3 | 4))
        .then_some(result)
}

pub fn embed_jpeg(bytes: &[u8], object: i32) -> Option<EmbeddedImage> {
    let info = jpeg_info(bytes)?;
    let space = match info.components {
        1 => "DeviceGray",
        3 => "DeviceRGB",
        _ => "DeviceCMYK",
    };
    let decode = if info.components == 4 && info.adobe {
        " /Decode [1 0 1 0 1 0 1 0]"
    } else {
        ""
    };
    let mut body = format!(
        "<< /Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent {} /ColorSpace /{} /Filter /DCTDecode{} /Length {} >>\nstream\n",
        info.width, info.height, info.bits, space, decode, bytes.len(),
    ).into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\nendstream");
    Some(EmbeddedImage {
        obj_num: object,
        bytes: body,
    })
}

/// Import one PDF page as a unit-square Form XObject, preserving vector
/// content and copying only the resources reachable from that page.
pub fn import_pdf_page(
    bytes: &[u8],
    page: u32,
    page_box: &[u8],
    object: i32,
    next_object: &mut i32,
) -> Result<(f64, f64, Vec<EmbeddedImage>), String> {
    use lopdf::{Dictionary, Document, Object, ObjectId};
    fn inherited<'a>(doc: &'a Document, mut id: ObjectId, key: &[u8]) -> Option<&'a Object> {
        let mut seen = std::collections::HashSet::new();
        while seen.insert(id) {
            let dict = doc.get_dictionary(id).ok()?;
            if let Ok(value) = dict.get(key) {
                return doc.dereference(value).ok().map(|(_, value)| value);
            }
            id = dict.get(b"Parent").ok()?.as_reference().ok()?;
        }
        None
    }
    fn copy_object(
        value: &Object,
        doc: &Document,
        ids: &mut std::collections::BTreeMap<ObjectId, i32>,
        objects: &mut Vec<EmbeddedImage>,
        next: &mut i32,
        depth: usize,
    ) -> Result<Object, String> {
        if depth > 256 {
            return Err("PDF resource nesting too deep".into());
        }
        Ok(match value {
            Object::Reference(id) => {
                let new_id = if let Some(mapped) = ids.get(id) {
                    *mapped
                } else {
                    let mapped = *next;
                    *next = next.checked_add(1).ok_or("PDF object number overflow")?;
                    ids.insert(*id, mapped);
                    let original = doc
                        .objects
                        .get(id)
                        .ok_or_else(|| format!("Missing PDF resource {id:?}"))?;
                    let copied = copy_object(original, doc, ids, objects, next, depth + 1)?;
                    let mut bytes = Vec::new();
                    serialize_pdf_object(&copied, &mut bytes);
                    objects.push(EmbeddedImage {
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
                    .map(|v| copy_object(v, doc, ids, objects, next, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            Object::Dictionary(dict) => {
                let mut copy = Dictionary::new();
                for (key, value) in dict {
                    copy.set(
                        key.clone(),
                        copy_object(value, doc, ids, objects, next, depth + 1)?,
                    );
                }
                Object::Dictionary(copy)
            }
            Object::Stream(stream) => {
                let mut dict = Dictionary::new();
                for (key, value) in &stream.dict {
                    if key != b"Length" {
                        dict.set(
                            key.clone(),
                            copy_object(value, doc, ids, objects, next, depth + 1)?,
                        );
                    }
                }
                Object::Stream(lopdf::Stream::new(dict, stream.content.clone()))
            }
            _ => value.clone(),
        })
    }
    let doc = Document::load_mem(bytes).map_err(|e| e.to_string())?;
    let page_id = *doc
        .get_pages()
        .get(&page)
        .ok_or_else(|| format!("PDF page {page} does not exist"))?;
    let bounds = inherited(&doc, page_id, page_box)
        .or_else(|| inherited(&doc, page_id, b"CropBox"))
        .or_else(|| inherited(&doc, page_id, b"MediaBox"))
        .ok_or("PDF page has no page box")?
        .as_array()
        .map_err(|e| e.to_string())?;
    if bounds.len() != 4 {
        return Err("Invalid PDF page box".into());
    }
    let mut b = [0.0f64; 4];
    for (i, v) in bounds.iter().enumerate() {
        b[i] = doc
            .dereference(v)
            .map_err(|e| e.to_string())?
            .1
            .as_float()
            .map_err(|e| e.to_string())? as f64;
    }
    let [x0, y0, x1, y1] = b;
    let (w, h) = (x1 - x0, y1 - y0);
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return Err("Empty or invalid PDF page box".into());
    }
    let rotate = inherited(&doc, page_id, b"Rotate")
        .and_then(|v| v.as_i64().ok())
        .unwrap_or(0)
        .rem_euclid(360);
    let matrix = match rotate {
        0 => [1.0 / w, 0.0, 0.0, 1.0 / h, -x0 / w, -y0 / h],
        90 => [0.0, -1.0 / w, 1.0 / h, 0.0, -y0 / h, x1 / w],
        180 => [-1.0 / w, 0.0, 0.0, -1.0 / h, x1 / w, y1 / h],
        270 => [0.0, 1.0 / w, -1.0 / h, 0.0, y1 / h, -x0 / w],
        _ => return Err("PDF page rotation is not a multiple of 90".into()),
    };
    let mut objects = Vec::new();
    let mut ids = std::collections::BTreeMap::new();
    let resources = match inherited(&doc, page_id, b"Resources") {
        Some(value) => copy_object(value, &doc, &mut ids, &mut objects, next_object, 0)?,
        None => Object::Dictionary(Dictionary::new()),
    };
    let content = doc
        .get_page_content_with_limit(page_id, 256 * 1024 * 1024)
        .map_err(|e| e.to_string())?;
    let content = crate::pdffile::flate(&content);
    let mut bytes = format!(
        "<< /Type /XObject /Subtype /Form /FormType 1 /BBox [{x0} {y0} {x1} {y1}] /Matrix [{} {} {} {} {} {}] /Resources ",
        matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5],
    ).into_bytes();
    serialize_pdf_object(&resources, &mut bytes);
    if let Ok(group) = doc.get_dictionary(page_id).and_then(|d| d.get(b"Group")) {
        let group = copy_object(group, &doc, &mut ids, &mut objects, next_object, 0)?;
        bytes.extend_from_slice(b" /Group ");
        serialize_pdf_object(&group, &mut bytes);
    }
    bytes.extend_from_slice(
        format!(
            " /Filter /FlateDecode /Length {} >>\nstream\n",
            content.len()
        )
        .as_bytes(),
    );
    bytes.extend_from_slice(&content);
    bytes.extend_from_slice(b"\nendstream");
    objects.push(EmbeddedImage {
        obj_num: object,
        bytes,
    });
    let unit = inherited(&doc, page_id, b"UserUnit")
        .and_then(|v| v.as_float().ok())
        .unwrap_or(1.0) as f64;
    if !unit.is_finite() || unit <= 0.0 {
        return Err("Invalid PDF UserUnit".into());
    }
    let (w, h) = if rotate % 180 == 0 { (w, h) } else { (h, w) };
    Ok((w * unit, h * unit, objects))
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

pub fn png_needs_soft_mask(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() < 29 {
        return false;
    }
    if matches!(bytes[25], 4 | 6) {
        return true;
    }
    let mut pos = 8usize;
    while let Some(header) = bytes.get(pos..pos.saturating_add(8)) {
        if &header[4..8] == b"tRNS" {
            return true;
        }
        let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let Some(next) = pos.checked_add(length).and_then(|n| n.checked_add(12)) else {
            return false;
        };
        pos = next;
    }
    false
}

fn embed_decoded_png(bytes: &[u8], object: i32, next: &mut i32) -> Option<Vec<EmbeddedImage>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(info.buffer_size());
    let (colors, alpha) = match info.color_type {
        png::ColorType::Grayscale => (1, false),
        png::ColorType::GrayscaleAlpha => (1, true),
        png::ColorType::Rgb => (3, false),
        png::ColorType::Rgba => (3, true),
        _ => return None,
    };
    let mut objects = Vec::with_capacity(if alpha { 2 } else { 1 });
    let mut mask = String::new();
    if alpha {
        let count = pixels.len() / (colors + 1);
        let mut alpha_pixels = Vec::with_capacity(count);
        for i in 0..count {
            let start = i * (colors + 1);
            alpha_pixels.push(pixels[start + colors]);
            pixels.copy_within(start..start + colors, i * colors);
        }
        pixels.truncate(count * colors);
        let mask_object = *next;
        let following = next.checked_add(1)?;
        objects.push(raw_raster_object(
            mask_object,
            info.width,
            info.height,
            1,
            &alpha_pixels,
            "",
        ));
        *next = following;
        mask = format!(" /SMask {mask_object} 0 R");
    }
    objects.push(raw_raster_object(
        object,
        info.width,
        info.height,
        colors,
        &pixels,
        &mask,
    ));
    Some(objects)
}

fn raw_raster_object(
    object: i32,
    width: u32,
    height: u32,
    colors: usize,
    pixels: &[u8],
    extra: &str,
) -> EmbeddedImage {
    let compressed = crate::pdffile::flate(pixels);
    let space = if colors == 1 {
        "DeviceGray"
    } else {
        "DeviceRGB"
    };
    let mut bytes = format!(
        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /BitsPerComponent 8 /ColorSpace /{space} /Filter /FlateDecode{extra} /Length {} >>\nstream\n",
        compressed.len(),
    ).into_bytes();
    bytes.extend_from_slice(&compressed);
    bytes.extend_from_slice(b"\nendstream");
    EmbeddedImage {
        obj_num: object,
        bytes,
    }
}

pub fn embed_png(png_bytes: &[u8], img_obj: i32, next_obj: &mut i32) -> Option<Vec<EmbeddedImage>> {
    if png_bytes.len() < 33
        || &png_bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &png_bytes[12..16] != b"IHDR"
    {
        return None;
    }
    let w = u32::from_be_bytes(png_bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(png_bytes[20..24].try_into().ok()?);
    let bit_depth = png_bytes[24];
    let color_type = png_bytes[25];

    if w == 0 || h == 0 {
        return None;
    }
    if bit_depth != 8
        || png_bytes[26..29] != [0, 0, 0]
        || matches!(color_type, 3 | 4)
        || (matches!(color_type, 0 | 2) && png_needs_soft_mask(png_bytes))
    {
        return embed_decoded_png(png_bytes, img_obj, next_obj);
    }

    // Collect IDAT chunks
    let mut idat = Vec::new();
    let mut i = 8usize;
    while i + 8 <= png_bytes.len() {
        let len = u32::from_be_bytes(png_bytes[i..i + 4].try_into().ok()?) as usize;
        let chunk_type = &png_bytes[i + 4..i + 8];
        let end = i.checked_add(12)?.checked_add(len)?;
        if end > png_bytes.len() {
            return None;
        }
        if chunk_type == b"IDAT" {
            idat.extend_from_slice(&png_bytes[i + 8..end - 4]);
        }
        i = end;
    }

    match color_type {
        0 => {
            // Grayscale (8-bit)
            let header = format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns {} /Colors 1 /BitsPerComponent 8 >> /Length {} >>\nstream\n",
                w, h, w, idat.len()
            );
            let mut obj_bytes = header.into_bytes();
            obj_bytes.extend_from_slice(&idat);
            obj_bytes.extend_from_slice(b"\nendstream");
            Some(vec![EmbeddedImage {
                obj_num: img_obj,
                bytes: obj_bytes,
            }])
        }
        2 => {
            // RGB (8-bit)
            let header = format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent 8 /ColorSpace /DeviceRGB /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns {} /Colors 3 /BitsPerComponent 8 >> /Length {} >>\nstream\n",
                w, h, w, idat.len()
            );
            let mut obj_bytes = header.into_bytes();
            obj_bytes.extend_from_slice(&idat);
            obj_bytes.extend_from_slice(b"\nendstream");
            Some(vec![EmbeddedImage {
                obj_num: img_obj,
                bytes: obj_bytes,
            }])
        }
        6 => {
            // RGBA (8-bit)
            let row_len = (w as usize).checked_mul(4)?.checked_add(1)?;
            let expected = (h as usize).checked_mul(row_len)?;
            let mut decoder =
                ZlibDecoder::new(&idat[..]).take(u64::try_from(expected).ok()?.checked_add(1)?);
            let mut decomp = Vec::with_capacity(expected);
            decoder.read_to_end(&mut decomp).ok()?;
            if decomp.len() != expected {
                return None;
            }
            // Unfilter all scanlines to raw RGBA
            let mut curr_row = vec![0u8; (w as usize) * 4];
            let mut prev_row = vec![0u8; (w as usize) * 4];
            let mut rgb_row = vec![0u8; 1 + (w as usize) * 3];
            let mut alpha_row = vec![0u8; 1 + (w as usize)];

            let mut rgb_enc = ZlibEncoder::new(Vec::new(), Compression::fast());
            let mut a_enc = ZlibEncoder::new(Vec::new(), Compression::fast());

            for r in 0..(h as usize) {
                let row_data = &decomp[r * row_len..(r + 1) * row_len];
                let filter = row_data[0];
                let src = &row_data[1..];
                match filter {
                    0 => curr_row.copy_from_slice(src),
                    1 => {
                        for c in 0..src.len() {
                            let left = if c >= 4 { curr_row[c - 4] } else { 0 };
                            curr_row[c] = src[c].wrapping_add(left);
                        }
                    }
                    2 => {
                        for c in 0..src.len() {
                            curr_row[c] = src[c].wrapping_add(prev_row[c]);
                        }
                    }
                    3 => {
                        for c in 0..src.len() {
                            let left = if c >= 4 { curr_row[c - 4] as u16 } else { 0 };
                            let up = prev_row[c] as u16;
                            let avg = ((left + up) / 2) as u8;
                            curr_row[c] = src[c].wrapping_add(avg);
                        }
                    }
                    4 => {
                        for c in 0..src.len() {
                            let left = if c >= 4 { curr_row[c - 4] as i16 } else { 0 };
                            let up = prev_row[c] as i16;
                            let up_left = if c >= 4 { prev_row[c - 4] as i16 } else { 0 };
                            let p = paeth_predictor(left, up, up_left);
                            curr_row[c] = src[c].wrapping_add(p);
                        }
                    }
                    _ => return None,
                }
                prev_row.copy_from_slice(&curr_row);

                rgb_row[0] = 0;
                alpha_row[0] = 0;
                for p in 0..(w as usize) {
                    rgb_row[1 + p * 3..1 + p * 3 + 3].copy_from_slice(&curr_row[p * 4..p * 4 + 3]);
                    alpha_row[1 + p] = curr_row[p * 4 + 3];
                }
                rgb_enc.write_all(&rgb_row).ok()?;
                a_enc.write_all(&alpha_row).ok()?;
            }

            let rgb_comp = rgb_enc.finish().ok()?;
            let alpha_comp = a_enc.finish().ok()?;

            let smask_obj = *next_obj;
            *next_obj = next_obj.checked_add(1)?;

            let smask_header = format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns {} /Colors 1 /BitsPerComponent 8 >> /Length {} >>\nstream\n",
                w, h, w, alpha_comp.len()
            );
            let mut smask_bytes = smask_header.into_bytes();
            smask_bytes.extend_from_slice(&alpha_comp);
            smask_bytes.extend_from_slice(b"\nendstream");

            let img_header = format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent 8 /ColorSpace /DeviceRGB /SMask {} 0 R /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns {} /Colors 3 /BitsPerComponent 8 >> /Length {} >>\nstream\n",
                w, h, smask_obj, w, rgb_comp.len()
            );
            let mut img_bytes = img_header.into_bytes();
            img_bytes.extend_from_slice(&rgb_comp);
            img_bytes.extend_from_slice(b"\nendstream");

            Some(vec![
                EmbeddedImage {
                    obj_num: smask_obj,
                    bytes: smask_bytes,
                },
                EmbeddedImage {
                    obj_num: img_obj,
                    bytes: img_bytes,
                },
            ])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &[u8; 4], data: &[u8], out: &mut Vec<u8>) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        // embed_png never reads chunk CRCs, so a placeholder is sufficient.
        out.extend_from_slice(&[0, 0, 0, 0]);
    }

    fn png(color_type: u8, w: u32, h: u32, idats: &[&[u8]]) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.push(8);
        ihdr.push(color_type);
        ihdr.extend_from_slice(&[0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(b"IHDR", &ihdr, &mut out);
        for d in idats {
            chunk(b"IDAT", d, &mut out);
        }
        chunk(b"IEND", &[], &mut out);
        out
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

        // The streamed RGB output must carry filter byte 0 and source pixels.
        let rgb = decoded_stream(&out[1]);
        assert_eq!(rgb.len(), h * (1 + w * 3));
        for r in 0..h {
            assert_eq!(rgb[r * (1 + w * 3)], 0, "output row filter byte");
            for x in 0..w {
                assert_eq!(
                    &rgb[r * (1 + w * 3) + 1 + x * 3..r * (1 + w * 3) + 1 + x * 3 + 3],
                    &rgba[r * w * 4 + x * 4..r * w * 4 + x * 4 + 3],
                    "pixel ({r},{x}) RGB"
                );
            }
        }
        // The soft mask must carry the source alpha, unchanged.
        let alpha = decoded_stream(&out[0]);
        assert_eq!(alpha.len(), h * (1 + w));
        for r in 0..h {
            assert_eq!(alpha[r * (1 + w)], 0, "output mask row filter byte");
            for x in 0..w {
                assert_eq!(
                    alpha[r * (1 + w) + 1 + x],
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
        let payload = deflate(&[0u8, 10, 20, 30, 200, 0, 40, 50][..]);
        for color_type in [0u8, 2u8] {
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
}
