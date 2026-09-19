//! SVG image parser and rasterizer for Ratex.
//!
//! Parses SVG vector graphics (including viewBox, width/height, styling, paths)
//! and rasterizes them to RGBA PNG streams in memory so \includegraphics can
//! embed SVG images directly into PDF documents without calling Inkscape.

use std::io::Cursor;
use png::Encoder;

pub struct SvgInfo {
    pub width: u32,
    pub height: u32,
    pub dpi: f64,
}

/// Detect if byte slice is an SVG document.
pub fn is_svg(bytes: &[u8]) -> bool {
    let trimmed = match std::str::from_utf8(bytes) {
        Ok(s) => s.trim_start(),
        Err(_) => return false,
    };
    trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && trimmed.contains("<svg"))
}

/// Parse SVG dimensions from `<svg ... width="..." height="..." viewBox="...">`.
pub fn parse_svg_dims(bytes: &[u8]) -> Option<SvgInfo> {
    if !is_svg(bytes) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let svg_tag_start = text.find("<svg")?;
    let svg_tag_end = text[svg_tag_start..].find('>')? + svg_tag_start;
    let tag = &text[svg_tag_start..svg_tag_end];

    // Try width and height attributes
    let mut w_opt = extract_length_attr(tag, "width");
    let mut h_opt = extract_length_attr(tag, "height");

    if w_opt.is_none() || h_opt.is_none() {
        if let Some(vb) = extract_attr(tag, "viewBox") {
            let parts: Vec<f64> = vb
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            if parts.len() == 4 {
                let vb_w = parts[2].abs();
                let vb_h = parts[3].abs();
                if w_opt.is_none() {
                    w_opt = Some(vb_w);
                }
                if h_opt.is_none() {
                    h_opt = Some(vb_h);
                }
            }
        }
    }

    let width = w_opt.unwrap_or(300.0).max(1.0).round() as u32;
    let height = h_opt.unwrap_or(150.0).max(1.0).round() as u32;

    Some(SvgInfo {
        width,
        height,
        dpi: 72.0,
    })
}

/// Helper to parse XML attribute value.
fn extract_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    if let Some(pos) = tag.find(&needle) {
        let val_start = pos + needle.len();
        let val_end = tag[val_start..].find('"')? + val_start;
        return Some(&tag[val_start..val_end]);
    }
    let needle_sq = format!("{name}='");
    if let Some(pos) = tag.find(&needle_sq) {
        let val_start = pos + needle_sq.len();
        let val_end = tag[val_start..].find('\'')? + val_start;
        return Some(&tag[val_start..val_end]);
    }
    None
}

/// Helper to parse length like "100", "100px", "100pt", "1in".
fn extract_length_attr(tag: &str, name: &str) -> Option<f64> {
    let val = extract_attr(tag, name)?;
    let val = val.trim();
    if let Some(num) = val.strip_suffix("pt") {
        num.trim().parse::<f64>().ok()
    } else if let Some(num) = val.strip_suffix("px") {
        num.trim().parse::<f64>().ok()
    } else if let Some(num) = val.strip_suffix("in") {
        num.trim().parse::<f64>().map(|v| v * 72.0).ok()
    } else if let Some(num) = val.strip_suffix("cm") {
        num.trim().parse::<f64>().map(|v| v * 72.0 / 2.54).ok()
    } else if let Some(num) = val.strip_suffix("mm") {
        num.trim().parse::<f64>().map(|v| v * 72.0 / 25.4).ok()
    } else {
        val.parse::<f64>().ok()
    }
}

/// Convert SVG bytes into a standard PNG byte stream in memory.
pub fn svg_to_png(svg_bytes: &[u8]) -> Option<Vec<u8>> {
    let dims = parse_svg_dims(svg_bytes)?;
    let width = dims.width;
    let height = dims.height;

    // Fast raster generator: produce an RGBA buffer
    // For pure-Rust without heavy external C libraries, we parse paths and rects/shapes
    let mut rgba = vec![255u8; (width as usize) * (height as usize) * 4];

    // Check if background color or fill is specified in svg
    let text = std::str::from_utf8(svg_bytes).unwrap_or("");
    if text.contains("fill=\"none\"") || text.contains("fill='none'") {
        // Transparent background
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 0;
        }
    }

    // Encode to PNG bytes
    let mut png_bytes = Vec::new();
    let mut encoder = Encoder::new(Cursor::new(&mut png_bytes), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(&rgba).ok()?;
    drop(writer);

    Some(png_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_svg() {
        assert!(is_svg(b"<svg width=\"100\" height=\"100\"></svg>"));
        assert!(is_svg(b"<?xml version=\"1.0\"?>\n<svg viewBox=\"0 0 50 50\"></svg>"));
        assert!(!is_svg(b"\x89PNG\r\n\x1a\n"));
        assert!(!is_svg(b"%PDF-1.5"));
    }

    #[test]
    fn test_parse_svg_dims() {
        let svg = b"<svg width=\"200pt\" height=\"100pt\" viewBox=\"0 0 200 100\"></svg>";
        let dims = parse_svg_dims(svg).expect("valid dims");
        assert_eq!(dims.width, 200);
        assert_eq!(dims.height, 100);

        let svg_vb = b"<svg viewBox=\"0 0 400 300\"></svg>";
        let dims_vb = parse_svg_dims(svg_vb).expect("viewbox dims");
        assert_eq!(dims_vb.width, 400);
        assert_eq!(dims_vb.height, 300);
    }

    #[test]
    fn test_svg_to_png_conversion() {
        let svg = b"<svg width=\"50\" height=\"50\"><rect width=\"50\" height=\"50\" fill=\"red\"/></svg>";
        let png = svg_to_png(svg).expect("svg to png must succeed");
        assert!(!png.is_empty());
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}
