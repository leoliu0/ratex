//! Shared parsed font programs representing Type 1, TrueType, and CFF outlines.
//!
//! Provides cached, sharable font programs with metadata extraction,
//! validation of embedding/subsetting permissions, face selection,
//! and variation coordinates.

use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontProgramKind {
    Type1,
    TrueType,
    Cff,
}

#[derive(Clone, Debug)]
pub struct FontProgram {
    pub data: Rc<Vec<u8>>,
    pub face_index: u32,
    pub variations: Vec<(ttf_parser::Tag, f32)>,
    pub kind: FontProgramKind,
    pub postscript_name: String,
    pub units_per_em: u16,
    pub allow_subsetting: bool,
    pub content_hash: [u8; 16],
}

impl FontProgram {
    /// Parse and validate a font program from raw bytes.
    ///
    /// Correctly classifies Type1, TrueType, and CFF (including collections like TTC/OTC).
    /// Enforces embedding restrictions, validates face index and variation axes,
    /// and extracts PostScript name and units per em.
    pub(crate) fn parse(
        data: Rc<Vec<u8>>,
        content_hash: [u8; 16],
        face_index: u32,
        mut variations: Vec<(ttf_parser::Tag, f32)>,
    ) -> Result<Self, String> {
        if is_type1_data(&data) {
            if face_index > 0 {
                return Err(format!(
                    "Type 1 font programs do not support face_index > 0 (requested {})",
                    face_index
                ));
            }
            if !variations.is_empty() {
                return Err("Type 1 font programs do not support variation axes".to_string());
            }
            let prog = crate::pdf_fonts::parse_type1(&data);
            if prog.data.is_empty() || prog.length1 == 0 {
                return Err("Malformed Type 1 font program".to_string());
            }
            let postscript_name =
                extract_type1_postscript_name(&prog).unwrap_or_else(|| "Type1Font".to_string());

            return Ok(FontProgram {
                data,
                face_index: 0,
                variations: Vec::new(),
                kind: FontProgramKind::Type1,
                postscript_name,
                units_per_em: 1000,
                allow_subsetting: true,
                content_hash,
            });
        }

        // SFNT parsing (TrueType / OpenType / TTC / OTC)
        if let Some(count) = ttf_parser::fonts_in_collection(&data) {
            if face_index >= count {
                return Err(format!(
                    "Requested face index {} exceeds font collection count {}",
                    face_index, count
                ));
            }
        } else if face_index > 0 {
            return Err(format!(
                "Requested face index {} for a non-collection font program",
                face_index
            ));
        }

        let mut face = ttf_parser::Face::parse(&data, face_index)
            .map_err(|e| format!("Failed to parse font face at index {}: {:?}", face_index, e))?;

        let units_per_em = face.units_per_em();
        if units_per_em == 0 {
            return Err(format!(
                "Font face at index {} has invalid unitsPerEm = 0",
                face_index
            ));
        }

        let kind = if face.tables().cff.is_some() || face.tables().cff2.is_some() {
            FontProgramKind::Cff
        } else if face.tables().glyf.is_some() {
            FontProgramKind::TrueType
        } else {
            return Err(format!(
                "Font face at index {} has neither TrueType (glyf) nor CFF outlines (bitmap-only or unsupported format)",
                face_index
            ));
        };

        let postscript_name = extract_postscript_name(&face)
            .or_else(|| extract_name(&face, ttf_parser::name_id::FULL_NAME))
            .or_else(|| extract_name(&face, ttf_parser::name_id::FAMILY))
            .unwrap_or_else(|| {
                format!(
                    "Font_{:02x}{:02x}{:02x}{:02x}",
                    data[0], data[1], data[2], data[3]
                )
            });

        // Validate embedding restrictions
        if let Some(perm) = face.permissions() {
            if perm == ttf_parser::Permissions::Restricted {
                return Err(format!(
                    "Font `{}` denies embedding (fsType = Restricted)",
                    postscript_name
                ));
            }
        }
        if !face.is_outline_embedding_allowed()
            && (face.tables().glyf.is_some() || face.tables().cff.is_some())
        {
            return Err(format!(
                "Font `{}` denies outline embedding",
                postscript_name
            ));
        }

        let allow_subsetting = face
            .tables()
            .os2
            .map(|t| t.is_subsetting_allowed())
            .unwrap_or(true);

        // Validate and apply variation coordinates
        if !variations.is_empty() {
            if !face.is_variable() {
                return Err(format!(
                    "Font `{}` is not a variable font, but variation axes were requested",
                    postscript_name
                ));
            }
            variations.sort_unstable_by_key(|(tag, _)| *tag);
            if variations.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err(format!(
                    "Duplicate variation axis for font `{postscript_name}`"
                ));
            }
            for &(tag, val) in &variations {
                let axis = face
                    .variation_axes()
                    .into_iter()
                    .find(|axis| axis.tag == tag)
                    .ok_or_else(|| {
                        format!("Unknown variation axis `{tag}` for font `{postscript_name}`")
                    })?;
                if !val.is_finite() || val < axis.min_value || val > axis.max_value {
                    return Err(format!(
                        "Variation axis `{tag}`={val} is outside {}..={} for font `{postscript_name}`",
                        axis.min_value, axis.max_value,
                    ));
                }
                if face.set_variation(tag, val).is_none() {
                    return Err(format!(
                        "Invalid or unsupported variation axis `{}` for font `{}`",
                        tag, postscript_name
                    ));
                }
            }
        }

        Ok(FontProgram {
            data,
            face_index,
            variations,
            kind,
            postscript_name,
            units_per_em,
            allow_subsetting,
            content_hash,
        })
    }

    /// Obtain a ttf_parser::Face configured with this program's face_index and variation coordinates.
    pub fn face(&self) -> Result<ttf_parser::Face<'_>, String> {
        match self.kind {
            FontProgramKind::Type1 => {
                Err("Type 1 font programs do not have an SFNT/ttf-parser face".to_string())
            }
            FontProgramKind::TrueType | FontProgramKind::Cff => {
                let mut face =
                    ttf_parser::Face::parse(&self.data, self.face_index).map_err(|e| {
                        format!("Failed to parse face at index {}: {:?}", self.face_index, e)
                    })?;
                for &(tag, val) in &self.variations {
                    if face.set_variation(tag, val).is_none() {
                        return Err(format!(
                            "Failed to set variation axis `{}` to {} on face",
                            tag, val
                        ));
                    }
                }
                Ok(face)
            }
        }
    }

    pub fn is_type1(&self) -> bool {
        self.kind == FontProgramKind::Type1
    }

    pub fn is_truetype(&self) -> bool {
        self.kind == FontProgramKind::TrueType
    }

    pub fn is_cff(&self) -> bool {
        self.kind == FontProgramKind::Cff
    }
}

/// Resolve the same legacy slot for PDF addressing and PDF-pen advances.
pub(crate) fn legacy_glyph(
    face: &ttf_parser::Face<'_>,
    encoding: Option<&[String]>,
    slot: u8,
) -> Result<(u16, String), String> {
    let name = encoding
        .and_then(|names| names.get(slot as usize))
        .map(String::as_str);
    let mut text = name.and_then(crate::pdf_fonts::glyph_to_unicode);
    let glyph = if let Some(name) = name {
        face.glyph_index_by_name(name).or_else(|| {
            crate::pdf_fonts::glyph_to_scalar(name).and_then(|scalar| face.glyph_index(scalar))
        })
    } else if encoding.is_none() {
        text = Some((slot as char).to_string());
        face.glyph_index(slot as char)
    } else {
        None
    }
    .filter(|glyph| glyph.0 != 0)
    .ok_or_else(|| format!("Used slot {slot} ({name:?}) has no mapped outline"))?;
    if text.is_none() {
        if let Some(cmap) = face.tables().cmap {
            for subtable in cmap.subtables {
                if subtable.is_unicode() {
                    subtable.codepoints(|codepoint| {
                        if text.is_none() && subtable.glyph_index(codepoint) == Some(glyph) {
                            text = char::from_u32(codepoint).map(|scalar| scalar.to_string());
                        }
                    });
                    if text.is_some() {
                        break;
                    }
                }
            }
        }
    }
    let text = text.ok_or_else(|| format!("Used slot {slot} ({name:?}) has no Unicode mapping"))?;
    Ok((glyph.0, text))
}

fn is_type1_data(data: &[u8]) -> bool {
    if data.starts_with(&[0x80, 0x01]) || data.starts_with(&[0x80, 0x02]) {
        return true;
    }
    if data.starts_with(b"%!PS-AdobeFont")
        || data.starts_with(b"%!FontType1")
        || data.starts_with(b"%!PS")
    {
        return true;
    }
    // Check if first bytes are not SFNT magic
    if data.starts_with(&[0x00, 0x01, 0x00, 0x00])
        || data.starts_with(b"true")
        || data.starts_with(b"typ1")
        || data.starts_with(b"OTTO")
        || data.starts_with(b"ttcf")
    {
        return false;
    }
    // Check if bare PFA contains eexec
    crate::pdf_fonts::parse_pfb(data).is_some()
}

fn extract_type1_postscript_name(prog: &crate::pdf_fonts::Type1Program) -> Option<String> {
    let cleartext = &prog.data[..prog.length1.min(prog.data.len())];
    let pos = cleartext.windows(9).position(|w| w == b"/FontName")?;
    let mut start = pos + 9;
    while start < cleartext.len() && matches!(cleartext[start], b' ' | b'\t' | b'\r' | b'\n') {
        start += 1;
    }
    if start >= cleartext.len() || cleartext[start] != b'/' {
        return None;
    }
    start += 1;
    let mut end = start;
    while end < cleartext.len()
        && !matches!(
            cleartext[end],
            b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'[' | b']' | b'{' | b'}' | b'(' | b')'
        )
    {
        end += 1;
    }
    std::str::from_utf8(&cleartext[start..end])
        .ok()
        .map(|s| s.to_string())
}

fn extract_postscript_name(face: &ttf_parser::Face<'_>) -> Option<String> {
    for name in face.names() {
        if name.name_id == ttf_parser::name_id::POST_SCRIPT_NAME {
            if let Some(s) = name_to_string(&name) {
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

fn extract_name(face: &ttf_parser::Face<'_>, name_id: u16) -> Option<String> {
    for name in face.names() {
        if name.name_id == name_id {
            if let Some(s) = name_to_string(&name) {
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

fn name_to_string(name: &ttf_parser::name::Name) -> Option<String> {
    if let Some(s) = name.to_string() {
        Some(s)
    } else if let Ok(s) = std::str::from_utf8(name.name) {
        Some(s.to_string())
    } else {
        None
    }
}
