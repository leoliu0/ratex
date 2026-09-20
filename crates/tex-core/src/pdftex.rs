//! pdfTeX extension surface beyond primitive dispatch: map-file loading
//! (\pdfmapfile, \pdfmapline), position query accessors
//! (\pdflastxpos/\pdflastypos), and end-of-job font embedding.
//!
//! Dispatch arms (maincontrol): `PdfMapFile => self.do_pdfmapfile()`,
//! `PdfMapLine => self.do_pdfmapline()`.

use crate::engine::Engine;
use crate::fontload::parse_map_line;

impl Engine {
    /// \pdfmapfile {+name|-name|=name|name}: load (add), or with `=`
    /// replace the whole map database. `-name` (remove a specific file's
    /// entries) is accepted and ignored: entry provenance is not tracked.
    pub fn do_pdfmapfile(&mut self) {
        self.skip_spaces_relax();
        let arg = self.scan_pdf_string();
        let arg = arg.trim().to_string();
        if arg.is_empty() {
            return;
        }
        let (replace, name) = match arg.as_bytes()[0] {
            b'+' => (false, arg[1..].trim().to_string()),
            b'-' => {
                self.font_loader.ensure_map();
                let _ = &arg[1..]; // removal unsupported; entry provenance untracked
                return;
            }
            b'=' => (true, arg[1..].trim().to_string()),
            _ => (false, arg.clone()),
        };
        if name.is_empty() {
            return;
        }
        if replace {
            // `=` replaces the whole database: the deferred default map
            // must not be re-added on top of it afterwards
            self.font_loader.map.clear();
            self.font_loader.mark_map_loaded();
        } else {
            // `+`/plain adds layer on top of the default database
            self.font_loader.ensure_map();
        }
        self.font_loader.load_map(&name);
    }

    /// \pdfmapline {<map line>}: install one map entry; `-tfm` removes.
    pub fn do_pdfmapline(&mut self) {
        self.skip_spaces_relax();
        let line = self.scan_pdf_string();
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if line.as_bytes()[0] == b'-' {
            self.font_loader.ensure_map();
            if let Some(tfm) = line[1..].split_whitespace().next() {
                self.font_loader.map.remove(tfm);
            }
            return;
        }
        self.font_loader.ensure_map();
        if let Some(e) = parse_map_line(line) {
            self.font_loader.map.insert(e.tfm.clone(), e);
        }
    }

    /// \pdflastxpos: sp from the left page edge (set by the last
    /// \pdfsavepos node shipped).
    pub fn pdflast_xpos(&self) -> i32 {
        self.pdf_last_x
    }

    /// \pdflastypos: sp from the bottom page edge.
    pub fn pdflast_ypos(&self) -> i32 {
        self.pdf_last_y
    }

    /// End-of-job: embed every engine font referenced by a shipped page
    /// and rewrite page/form font-binding references to document font indices.
    pub fn embed_used_fonts(&mut self) -> Result<(), String> {
        self.pdf_doc.minor_version =
            Some(self.eqtb.int_params[crate::prim::IntParam::PdfMinorVersion.idx() as usize]);
        use std::collections::BTreeSet;
        let mut used: BTreeSet<u16> = BTreeSet::new();
        for fonts in self
            .pdf_doc
            .pages
            .iter()
            .map(|p| &p.fonts)
            .chain(self.pdf_doc.form_fonts.iter().map(|(_, fonts)| fonts))
        {
            for (fid, _) in fonts {
                used.insert(*fid as u16);
            }
        }
        let mut remap: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for fid in used {
            let Some(font) = self.eqtb.fonts.get(fid as usize).cloned() else {
                continue;
            };
            let prog = self.font_loader.program_for_font(&font)?;
            let doc_font_idx = self.pdf_doc.fonts.len();

            let at_size = font.at_size;
            let to_units = |val: i32| -> f64 {
                if at_size != 0 {
                    (val as f64 * 1000.0 / at_size as f64).round()
                } else {
                    0.0
                }
            };
            let (ta, td, tc, ts) = crate::pdf_fonts::tfm_descriptor(&font);
            let asc = to_units(font.char_height(b'd'));
            let cap = to_units(font.char_height(b'H'));
            let desc = -to_units(font.char_depth(b'p'));
            let ascent = if asc > 0.0 { asc } else { ta };
            let cap_height = if cap > 0.0 { cap } else { tc };
            let mut descent = if ascent == 0.0 {
                0.0
            } else if desc != 0.0 {
                desc
            } else {
                td
            };
            if ascent - descent > 3000.0 {
                descent = ascent - 3000.0;
            }
            let stem_v = ts.max(100.0);

            match prog.kind {
                crate::font_program::FontProgramKind::Type1 => {
                    let has_bindings = self.pdf_doc.legacy_bindings.contains_key(&(fid as usize));
                    if has_bindings {
                        let bindings = self
                            .pdf_doc
                            .legacy_bindings
                            .get(&(fid as usize))
                            .cloned()
                            .unwrap();
                        let pfb_bytes = prog.data.as_slice();
                        let base_encoding = font.encoding.as_ref().cloned().or_else(|| {
                            let type1 = crate::pdf_fonts::parse_type1(pfb_bytes);
                            crate::pdf_fonts::builtin_encoding(&type1.data[..type1.length1])
                        });
                        for (b_idx, binding) in bindings.iter().enumerate() {
                            let cur_idx = self.pdf_doc.fonts.len();
                            let mut used_chars = [0u64; 4];
                            let mut widths = vec![0i32; 256];
                            let mut diffs = vec![String::new(); 256];
                            let mut to_unicode = Vec::new();
                            for &(code, base_char, ref text) in &binding.entries {
                                used_chars[code as usize / 64] |= 1_u64 << (code as usize % 64);
                                let w = font.char_width(base_char);
                                if at_size != 0 {
                                    widths[code as usize] =
                                        ((w as i64 * 10_000 + at_size as i64 / 2) / at_size as i64)
                                            as i32;
                                }
                                let glyph_name = base_encoding
                                    .as_ref()
                                    .and_then(|enc| enc.get(base_char as usize))
                                    .filter(|name| !name.is_empty() && name.as_str() != ".notdef")
                                    .cloned()
                                    .ok_or_else(|| {
                                        format!(
                                        "Font `{}` has no encoded glyph for used slot {base_char}",
                                        font.tfm_name
                                    )
                                    })?;
                                diffs[code as usize] = glyph_name;
                                to_unicode.push((code, text.clone()));
                            }
                            let mut ef = crate::pdffile::make_embed_font(
                                font.map_fontname
                                    .clone()
                                    .unwrap_or_else(|| font.tfm_name.clone()),
                                Some(pfb_bytes),
                                Some(&diffs),
                                0,
                                255,
                                widths,
                            );
                            ef.to_unicode = to_unicode;
                            crate::pdffile::set_font_usage(&mut ef, used_chars);
                            ef.ascent = ascent;
                            ef.cap_height = cap_height;
                            ef.descent = descent;
                            ef.stem_v = stem_v;
                            self.pdf_doc.fonts.push(ef);
                            remap.insert(crate::pdfout::font_resource_key(fid, b_idx), cur_idx);
                        }
                    } else {
                        let pfb_bytes = prog.data.as_slice();
                        let widths = (0..=255u8)
                            .map(|c| {
                                let w = font.char_width(c);
                                if at_size != 0 {
                                    ((w as i64 * 10_000 + at_size as i64 / 2) / at_size as i64)
                                        as i32
                                } else {
                                    0
                                }
                            })
                            .collect();
                        let mut ef = crate::pdffile::make_embed_font(
                            font.map_fontname
                                .clone()
                                .unwrap_or_else(|| font.tfm_name.clone()),
                            Some(pfb_bytes),
                            font.encoding.as_deref(),
                            0,
                            255,
                            widths,
                        );
                        crate::pdffile::set_font_usage(
                            &mut ef,
                            self.pdf_doc
                                .font_chars
                                .get(&(fid as usize))
                                .copied()
                                .unwrap_or([0; 4]),
                        );
                        ef.ascent = ascent;
                        ef.cap_height = cap_height;
                        ef.descent = descent;
                        ef.stem_v = stem_v;
                        self.pdf_doc.fonts.push(ef);
                        remap.insert(fid as usize, doc_font_idx);
                    }
                }
                crate::font_program::FontProgramKind::TrueType
                | crate::font_program::FontProgramKind::Cff => {
                    let base_font = font
                        .map_fontname
                        .clone()
                        .unwrap_or_else(|| prog.postscript_name.clone());
                    let is_native = self.pdf_doc.native_bindings.contains_key(&(fid as usize));
                    if is_native {
                        let bindings = self
                            .pdf_doc
                            .native_bindings
                            .get(&(fid as usize))
                            .ok_or_else(|| {
                                format!("Native font `{base_font}` has no used glyph bindings")
                            })?;
                        for (b_idx, binding) in bindings.iter().enumerate() {
                            let cur_idx = self.pdf_doc.fonts.len();
                            let mut used_gids = std::collections::BTreeSet::new();
                            let mut native_cids = Vec::new();
                            let mut to_unicode_2byte = Vec::new();
                            for &(code, gid, ref txt) in &binding.entries {
                                used_gids.insert(gid);
                                native_cids.push((code, gid, txt.clone()));
                                if !txt.is_empty() {
                                    to_unicode_2byte.push((code, txt.clone()));
                                }
                            }
                            let ef = crate::pdfout::EmbedFont {
                                obj_font: 0,
                                base_font: base_font.clone(),
                                font_file: prog.data.clone(),
                                length1: prog.data.len(),
                                length2: 0,
                                length3: 0,
                                is_truetype: prog.kind
                                    == crate::font_program::FontProgramKind::TrueType,
                                subtype: if prog.kind
                                    == crate::font_program::FontProgramKind::TrueType
                                {
                                    crate::pdfout::EmbedFontSubtype::TrueType
                                } else {
                                    crate::pdfout::EmbedFontSubtype::Cff
                                },
                                face_index: prog.face_index,
                                variations: prog.variations.clone(),
                                allow_subsetting: prog.allow_subsetting,
                                content_hash: prog.content_hash,
                                units_per_em: prog.units_per_em,
                                encoding_diff: None,
                                first_char: 0,
                                last_char: 255,
                                widths: Vec::new(),
                                font_matrix_scale: 1.0,
                                font_bbox: [-500.0, -300.0, 1500.0, 1200.0],
                                italic_angle: 0.0,
                                ascent,
                                descent,
                                cap_height,
                                stem_v,
                                flags: 4,
                                to_unicode: Vec::new(),
                                used_chars: [0; 4],
                                is_cid: true,
                                is_native: true,
                                legacy_cids: Vec::new(),
                                native_cids,
                                used_gids,
                                to_unicode_2byte,
                            };
                            self.pdf_doc.fonts.push(ef);
                            remap.insert(crate::pdfout::font_resource_key(fid, b_idx), cur_idx);
                        }
                    } else {
                        // Legacy mapped SFNT font
                        let recorded_chars = self
                            .pdf_doc
                            .font_chars
                            .get(&(fid as usize))
                            .copied()
                            .unwrap_or([0; 4]);
                        let face = prog.face()?;
                        let bindings = self.pdf_doc.legacy_bindings.get(&(fid as usize));
                        for b_idx in 0..bindings.map_or(1, Vec::len) {
                            let mut used_chars = [0; 4];
                            let mut legacy_cids = Vec::new();
                            let mut used_gids = std::collections::BTreeSet::new();
                            let mut to_unicode = Vec::new();
                            let mut add_glyph = |code: u8,
                                                 slot: u8,
                                                 text: Option<&str>|
                             -> Result<(), String> {
                                let (gid, unicode) = crate::font_program::legacy_glyph(
                                    &face,
                                    font.encoding.as_deref(),
                                    slot,
                                )
                                .map_err(|error| format!("Font `{}`: {error}", font.tfm_name))?;
                                let unicode = text.map_or(unicode, str::to_owned);
                                legacy_cids.push((code, gid, unicode.clone()));
                                used_gids.insert(gid);
                                to_unicode.push((code, unicode));
                                used_chars[code as usize / 64] |= 1_u64 << (code as usize % 64);
                                Ok(())
                            };
                            if let Some(bindings) = bindings {
                                for (code, slot, text) in &bindings[b_idx].entries {
                                    add_glyph(*code, *slot, Some(text))?;
                                }
                            } else {
                                for slot in 0..=255u8 {
                                    if recorded_chars[slot as usize / 64]
                                        & (1_u64 << (slot as usize % 64))
                                        != 0
                                    {
                                        add_glyph(slot, slot, None)?;
                                    }
                                }
                            }

                            let ef = crate::pdfout::EmbedFont {
                                obj_font: 0,
                                base_font: base_font.clone(),
                                font_file: prog.data.clone(),
                                length1: prog.data.len(),
                                length2: 0,
                                length3: 0,
                                is_truetype: prog.kind
                                    == crate::font_program::FontProgramKind::TrueType,
                                subtype: if prog.kind
                                    == crate::font_program::FontProgramKind::TrueType
                                {
                                    crate::pdfout::EmbedFontSubtype::TrueType
                                } else {
                                    crate::pdfout::EmbedFontSubtype::Cff
                                },
                                face_index: prog.face_index,
                                variations: prog.variations.clone(),
                                allow_subsetting: prog.allow_subsetting,
                                content_hash: prog.content_hash,
                                units_per_em: prog.units_per_em,
                                encoding_diff: font.encoding.clone(),
                                first_char: 0,
                                last_char: 255,
                                widths: Vec::new(),
                                font_matrix_scale: 1.0,
                                font_bbox: [-500.0, -300.0, 1500.0, 1200.0],
                                italic_angle: 0.0,
                                ascent,
                                descent,
                                cap_height,
                                stem_v,
                                flags: 4,
                                to_unicode,
                                used_chars,
                                is_cid: true,
                                is_native: false,
                                legacy_cids,
                                native_cids: Vec::new(),
                                used_gids,
                                to_unicode_2byte: Vec::new(),
                            };
                            self.pdf_doc.fonts.push(ef);
                            remap.insert(
                                crate::pdfout::font_resource_key(fid, b_idx),
                                self.pdf_doc.fonts.len() - 1,
                            );
                        }
                    }
                }
            }
        }
        for fonts in self
            .pdf_doc
            .pages
            .iter_mut()
            .map(|p| &mut p.fonts)
            .chain(self.pdf_doc.form_fonts.iter_mut().map(|(_, fonts)| fonts))
        {
            for pf in fonts.iter_mut() {
                if let Some(&new_idx) = remap.get(&pf.0) {
                    pf.0 = new_idx;
                }
            }
        }
        Ok(())
    }
}
