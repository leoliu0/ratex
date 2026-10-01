//! pdfTeX extension surface beyond primitive dispatch: map-file loading
//! (\pdfmapfile, \pdfmapline), position query accessors
//! (\pdflastxpos/\pdflastypos), end-of-job font embedding, and the backend
//! state behind the PDF-object primitives (\pdfpageref, \pdffontname,
//! \pdffontobjnum, \pdfxformname, \pdfincludechars, \pdftrailer, ...).
//!
//! Dispatch arms (maincontrol): `PdfMapFile => self.do_pdfmapfile()`,
//! `PdfMapLine => self.do_pdfmapline()`.

use crate::engine::Engine;
use crate::fontmap::MapMode;

impl Engine {
    /// \pdfmapfile {[+|=|-]<file>}: read a map file into the font map.
    pub fn do_pdfmapfile(&mut self) {
        self.skip_spaces_relax();
        let arg = self.scan_pdf_string();
        self.process_map_item(&arg, true);
    }

    /// \pdfmapline {[+|=|-]<map line>}: add, replace or delete one entry.
    pub fn do_pdfmapline(&mut self) {
        self.skip_spaces_relax();
        let line = self.scan_pdf_string();
        self.process_map_item(&line, false);
    }

    /// mapfile.c `process_map_item`: `+` inserts unless the TFM is mapped
    /// (duplicates ignored with a warning unless \pdfsuppresswarningdupmap),
    /// `=` replaces and `-` deletes unless the font is already in use; an
    /// unprefixed item inserts like `+` but first drops the default map file
    /// if it has not been read yet.
    pub(crate) fn process_map_item(&mut self, item: &str, is_file: bool) {
        let item = item.strip_prefix(' ').unwrap_or(item);
        let (mode, rest, flush_default) = match item.as_bytes().first() {
            Some(b'+') => (MapMode::DupIgnore, &item[1..], false),
            Some(b'=') => (MapMode::Replace, &item[1..], false),
            Some(b'-') => (MapMode::Delete, &item[1..], false),
            _ => (MapMode::DupIgnore, item, true),
        };
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        // a file name ends at the first blank; a map line may keep its tail
        let rest = if is_file {
            rest.split(' ').next().unwrap_or_default()
        } else {
            rest
        };
        if flush_default {
            self.font_loader.mark_map_loaded();
        } else {
            self.font_loader.ensure_map();
        }
        if rest.is_empty() {
            return;
        }
        let text = if is_file {
            match self.font_loader.read_map_file(rest) {
                Some(text) => text,
                None => {
                    self.warning_at(
                        &format!("(file {rest}): cannot open font map file"),
                        None,
                    );
                    return;
                }
            }
        } else {
            rest.to_string()
        };
        let used = &self.pdf_backend.mapped_in_use;
        let report = self
            .font_loader
            .map
            .add_layer(text, mode, &|name| used.contains(name), true);
        let suppress = self.eqtb.int_params
            [crate::prim::IntParam::PdfSuppressWarningDupMap.idx() as usize]
            > 0;
        if !suppress {
            for name in &report.duplicates {
                self.warning_at(
                    &format!("fontmap entry for `{name}' already exists, duplicates ignored"),
                    None,
                );
            }
        }
        for name in &report.in_use {
            self.warning_at(
                &format!("fontmap entry for `{name}' has been used, replace/delete not allowed"),
                None,
            );
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
        // Writing the first PDF object freezes the version (pdfTeX
        // `check_pdfversion`); a shipout normally did that already.
        if self.pdf_fixed.is_none() {
            self.fix_pdf_output_params();
        }
        // End-of-job backend parameters and object-number reservations.
        use crate::prim::IntParam;
        self.pdf_doc.omit_info_dict = self.pdf_int(IntParam::PdfOmitInfoDict) != 0;
        // writefont.c prints /CharSet only while `getpdfomitcharset() == 0`
        self.pdf_doc.omit_charset = self.pdf_int(IntParam::PdfOmitCharset) != 0;
        self.pdf_doc.reserved_objects = self.pdf_next_obj - 1;
        self.pdf_doc.page_objnums = self
            .pdf_backend
            .page_objs
            .iter()
            .map(|(&page, &obj)| (page as usize - 1, obj))
            .collect();
        use std::collections::BTreeSet;
        let gen_tounicode =
            self.eqtb.int_params[crate::prim::IntParam::PdfGenToUnicode.idx() as usize];
        // pdftex.web `fixed_gen_tounicode`, cleared when no glyph table exists
        let mut fixed_gen_tounicode = gen_tounicode;
        let mut used: BTreeSet<u16> = BTreeSet::new();
        // pdf_init_font: the first SHIPPED font of a TFM owns the PDF font
        // dictionary; later fonts of that TFM (other sizes) mark their
        // characters in it (pdftex.web "Output fonts definition").
        let mut raw_groups: crate::FxHashMap<String, (u16, [u64; 4])> =
            crate::FxHashMap::default();
        for fonts in self
            .pdf_doc
            .pages
            .iter()
            .map(|p| &p.fonts)
            .chain(self.pdf_doc.form_fonts.iter().map(|(_, fonts)| fonts))
        {
            for &(key, _) in fonts {
                let fid = key as u16;
                if used.insert(fid) {
                    let chars = self.pdf_doc.font_chars.get(&(fid as usize));
                    if let (Some(chars), Some(font)) = (chars, self.eqtb.fonts.get(fid as usize)) {
                        let group = raw_groups.entry(font.tfm_name.clone()).or_insert((fid, [0; 4]));
                        for (word, used_word) in group.1.iter_mut().zip(chars) {
                            *word |= used_word;
                        }
                    }
                }
            }
        }
        // \pdfincludechars initializes fonts that no page or form shows;
        // pdfTeX writes every font with marked characters.
        let mut included: Vec<u16> = self
            .pdf_backend
            .font_ff
            .keys()
            .copied()
            .filter(|fid| {
                !used.contains(fid)
                    && self
                        .pdf_doc
                        .font_chars
                        .get(&(*fid as usize))
                        .is_some_and(|chars| chars.iter().any(|&word| word != 0))
            })
            .collect();
        included.sort_unstable();
        for fid in included {
            used.insert(fid);
            let chars = self.pdf_doc.font_chars[&(fid as usize)];
            if let Some(font) = self.eqtb.fonts.get(fid as usize) {
                let group = raw_groups.entry(font.tfm_name.clone()).or_insert((fid, [0; 4]));
                for (word, used_word) in group.1.iter_mut().zip(chars) {
                    *word |= used_word;
                }
            }
        }
        // pdf_init_font: the font initialized first owns the dictionary.
        for (owner, _) in raw_groups.values_mut() {
            if let Some(&ff) = self.pdf_backend.font_ff.get(owner) {
                let ff_has_chars = self
                    .pdf_doc
                    .font_chars
                    .get(&(ff as usize))
                    .is_some_and(|chars| chars.iter().any(|&word| word != 0));
                if used.contains(&ff) && ff_has_chars {
                    *owner = ff;
                }
            }
        }
        let mut remap: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        // Sizes and expansion steps of one font share its decoded program.
        let mut type1_sources: crate::FxHashMap<[u8; 16], std::rc::Rc<crate::pdffile::Type1Source>> =
            crate::FxHashMap::default();
        let mut raw_group_index: crate::FxHashMap<String, usize> = crate::FxHashMap::default();
        let mut raw_group_members: Vec<(u16, String)> = Vec::new();
        for &fid in &used {
            let Some(font) = self.eqtb.fonts.get(fid as usize).cloned() else {
                continue;
            };
            let prog = self.font_loader.program_for_font(&font)?;
            // pdfTeX shares one font dictionary between used fonts with the
            // same TFM, propagating a \pdffontattr set on any of them.
            let font_attr = self
                .font_loader
                .pdf_font_attrs
                .get(&fid)
                .or_else(|| {
                    used.iter()
                        .filter(|&&other| {
                            self.eqtb.fonts.get(other as usize)
                                .is_some_and(|f| f.tfm_name == font.tfm_name)
                        })
                        .find_map(|other| self.font_loader.pdf_font_attrs.get(other))
                })
                .cloned()
                .unwrap_or_default();
            // writefont.c: the generated CMap needs \pdfgentounicode > 0 (its
            // value at the end of the job) and no \pdfnobuiltintounicode.
            let nobuiltin_tounicode = gen_tounicode <= 0
                || self.font_loader.nobuiltin_tounicode.contains(&fid);

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
                    let source = type1_sources
                        .entry(prog.content_hash)
                        .or_insert_with(|| {
                            std::rc::Rc::new(crate::pdffile::Type1Source::new(&prog.data))
                        })
                        .clone();
                    let base_encoding =
                        font.encoding.clone().or_else(|| source.builtin_encoding.clone());
                    let base_font = font
                        .map_fontname
                        .clone()
                        .unwrap_or_else(|| font.tfm_name.clone());
                    if let Some(bindings) =
                        self.pdf_doc.legacy_bindings.get(&(fid as usize)).cloned()
                    {
                        for (binding_index, binding) in bindings.iter().enumerate() {
                            let document_index = self.pdf_doc.fonts.len();
                            let mut used_chars = [0u64; 4];
                            let mut widths = vec![0i32; 256];
                            let mut differences = vec![String::new(); 256];
                            let mut to_unicode = Vec::new();
                            for &(code, base_char, ref text) in &binding.entries {
                                used_chars[code as usize / 64] |= 1_u64 << (code as usize % 64);
                                widths[code as usize] = crate::pdfrender::pdf_width_tenths(
                                    font.char_width(base_char),
                                    at_size,
                                );
                                let glyph_name = base_encoding
                                    .as_ref()
                                    .and_then(|encoding| encoding.get(base_char as usize))
                                    .filter(|name| {
                                        !name.is_empty() && name.as_str() != ".notdef"
                                    })
                                    .cloned()
                                    .ok_or_else(|| {
                                        format!(
                                            "Font `{}` has no encoded glyph for used slot {base_char}",
                                            font.tfm_name
                                        )
                                    })?;
                                differences[code as usize] = glyph_name;
                                to_unicode.push((code, text.clone()));
                            }
                            let mut embedded = crate::pdffile::make_embed_font(
                                base_font.clone(),
                                Some(&source),
                                Some(differences.into()),
                                widths,
                                used_chars,
                            );
                            embedded.to_unicode = to_unicode;
                            embedded.t1_preset = self.preset_fontmetrics(fid);
                            embedded.init_order = self.pdf_backend.init_order(fid);
                            self.pdf_doc.fonts.push(embedded);
                            remap.insert(
                                crate::pdfout::FontBinding::remapped(binding_index)
                                    .resource_key(fid),
                                document_index,
                            );
                        }
                    }

                    let raw_chars = self
                        .pdf_doc
                        .font_chars
                        .get(&(fid as usize))
                        .copied()
                        .unwrap_or([0; 4]);
                    let group = raw_groups.get(&font.tfm_name).copied();
                    if raw_chars.iter().any(|&word| word != 0)
                        && group.is_some_and(|(owner, _)| owner != fid)
                    {
                        raw_group_members.push((fid, font.tfm_name.clone()));
                    } else if raw_chars.iter().any(|&word| word != 0) {
                        let raw_chars = group.map_or(raw_chars, |(_, chars)| chars);
                        let document_index = self.pdf_doc.fonts.len();
                        raw_group_index.insert(font.tfm_name.clone(), document_index);
                        let widths = (0..=255u8)
                            .map(|character| {
                                crate::pdfrender::pdf_width_tenths(font.char_width(character), at_size)
                            })
                            .collect();
                        let mut embedded = crate::pdffile::make_embed_font(
                            base_font,
                            Some(&source),
                            font.encoding.clone(),
                            widths,
                            raw_chars,
                        );
                        // writefont.c write_fontdictionary: the generated
                        // CMap (the dummy-space font always asks for one)
                        let wants_tounicode = (fixed_gen_tounicode > 0
                            && !self.font_loader.nobuiltin_tounicode.contains(&fid))
                            || font.tfm_name == "dummy-space";
                        let names = font.encoding.as_ref().or(source.builtin_encoding.as_ref());
                        let tounicode = match names {
                            Some(_) if !wants_tounicode => None,
                            Some(_) if self.pdf_backend.glyph_unicode.is_empty() => {
                                // write_tounicode: `fixedgentounicode := 0`
                                self.warning_at(
                                    "no GlyphToUnicode entry has been inserted yet!",
                                    None,
                                );
                                fixed_gen_tounicode = 0;
                                None
                            }
                            Some(names) => {
                                let (cmap, warning) = crate::pdf_fonts::tounicode_cmap(
                                    &self.pdf_backend.glyph_unicode,
                                    names,
                                    &font.tfm_name,
                                    font.encoding.as_ref().and(font.enc_name.as_deref()),
                                );
                                if let Some(warning) = warning {
                                    self.warning_at(&warning, None);
                                }
                                Some(cmap.into())
                            }
                            None => None,
                        };
                        embedded.pdftex = Some(crate::pdfout::PdfTexFont {
                            tfm_name: font.tfm_name.clone(),
                            enc_file: font.encoding.as_ref().and(font.enc_name.clone()),
                            tounicode,
                        });
                        embedded.font_attr = font_attr.clone();
                        // \pdffontobjnum fixed the dictionary's number
                        let ff = self.pdf_backend.font_ff.get(&fid).copied().unwrap_or(fid);
                        embedded.obj_font =
                            self.pdf_backend.font_objs.get(&ff).copied().unwrap_or(0);
                        embedded.t1_preset = self.preset_fontmetrics(fid);
                        embedded.init_order = self.pdf_backend.init_order(fid);
                        self.pdf_doc.fonts.push(embedded);
                        remap.insert(
                            crate::pdfout::FontBinding::RAW.resource_key(fid),
                            document_index,
                        );
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
                                font_attr: String::new(),
                                t1_preset: Default::default(),
                                t1_keys: Default::default(),
                                init_order: 0,
                                pdftex: None,
                            };
                            self.pdf_doc.fonts.push(ef);
                            remap.insert(
                                crate::pdfout::FontBinding::remapped(b_idx).resource_key(fid),
                                cur_idx,
                            );
                        }
                    } else {
                        // A legacy mapped SFNT can be painted through its
                        // original byte encoding and through one or more
                        // semantic remaps. Each code space needs its own PDF
                        // dictionary even though the font program is shared.
                        let recorded_chars = self
                            .pdf_doc
                            .font_chars
                            .get(&(fid as usize))
                            .copied()
                            .unwrap_or([0; 4]);
                        let face = prog.face()?;
                        let bindings = self
                            .pdf_doc
                            .legacy_bindings
                            .get(&(fid as usize))
                            .cloned()
                            .unwrap_or_default();
                        let mut resource_bindings = Vec::with_capacity(bindings.len() + 1);
                        if recorded_chars.iter().any(|&word| word != 0) {
                            resource_bindings.push(crate::pdfout::FontBinding::RAW);
                        }
                        resource_bindings
                            .extend((0..bindings.len()).map(crate::pdfout::FontBinding::remapped));
                        for resource_binding in resource_bindings {
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
                            // Semantic remaps renumber codes, so code-indexed
                            // user attributes only apply to the raw code space.
                            let raw = resource_binding.remapped_index().is_none();
                            if let Some(index) = resource_binding.remapped_index() {
                                for (code, slot, text) in &bindings[index].entries {
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
                            if raw && nobuiltin_tounicode {
                                to_unicode.clear();
                            }

                            let embedded = crate::pdfout::EmbedFont {
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
                                font_attr: if raw { font_attr.clone() } else { String::new() },
                                t1_preset: Default::default(),
                                t1_keys: Default::default(),
                                init_order: 0,
                                pdftex: None,
                            };
                            let document_index = self.pdf_doc.fonts.len();
                            self.pdf_doc.fonts.push(embedded);
                            remap.insert(resource_binding.resource_key(fid), document_index);
                        }
                    }
                }
            }
        }
        for (fid, tfm_name) in raw_group_members {
            if let Some(&document_index) = raw_group_index.get(&tfm_name) {
                remap.insert(crate::pdfout::FontBinding::RAW.resource_key(fid), document_index);
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

/// pdfTeX's default `\pdfspacefont` (pdftex.web `pdf_space_font_name`).
const DEFAULT_SPACE_FONT: &str = "pdftexspace";

/// Backend bookkeeping behind the PDF-object primitives. Object numbers are
/// drawn from the engine's `pdf_next_obj`, like `\pdfobj`, and the writer
/// places the page or font dictionary at the reserved number.
pub(crate) struct PdfBackend {
    /// `\pdfpageref`: object numbers fixed for (1-based) pages.
    pub(crate) page_objs: crate::FxHashMap<i32, i32>,
    /// `pdf_init_font`: each initialized font and the font `ff` whose PDF
    /// font dictionary and `/F<ff>` resource it shares (same TFM).
    pub(crate) font_ff: crate::FxHashMap<u16, u16>,
    /// Fonts owning a dictionary, in initialization order.
    font_reps: Vec<u16>,
    /// TFM names of initialized fonts: their map entries are in use.
    pub(crate) mapped_in_use: crate::FxHashSet<String>,
    /// `\pdffontobjnum`: dictionary object numbers keyed by owner font.
    pub(crate) font_objs: crate::FxHashMap<u16, i32>,
    /// `\pdflastximagecolordepth`.
    pub(crate) last_ximage_colordepth: i32,
    /// `\pdfspacefont`: TFM of the font for faked interword spaces.
    pub(crate) space_font_name: String,
    /// `\pdfglyphtounicode` entries (tounicode.c `glyph_unicode_tree`),
    /// dumped with the format.
    pub(crate) glyph_unicode: crate::pdf_fonts::GlyphUnicodeTable,
}

impl Default for PdfBackend {
    fn default() -> Self {
        PdfBackend {
            page_objs: Default::default(),
            font_ff: Default::default(),
            font_reps: Vec::new(),
            mapped_in_use: Default::default(),
            font_objs: Default::default(),
            last_ximage_colordepth: 0,
            space_font_name: DEFAULT_SPACE_FONT.to_string(),
            glyph_unicode: Default::default(),
        }
    }
}

impl PdfBackend {
    /// Creation order of the font object of owner `f` (`pdf_create_obj`).
    pub(crate) fn init_order(&self, f: u16) -> usize {
        self.font_reps.iter().position(|&k| k == f).unwrap_or(usize::MAX)
    }
}

impl Engine {
    fn pdf_int(&self, p: crate::prim::IntParam) -> i32 {
        self.eqtb.int_params[p.idx() as usize]
    }

    /// writefont.c `preset_fontmetrics`: FontDescriptor values from the TFM
    /// of font `f` in 1/1000 of `pdf_font_size[f]` (`dividescaled(.., 3)`),
    /// including pdfTeX's quirks: the angle in degrees is truncated to an
    /// integer before the division, StemV is a third of `.` (integer
    /// division) and the bounding box is [0 Descent quad max(CapHeight, Ascent)].
    pub(crate) fn preset_fontmetrics(&self, f: u16) -> [i32; crate::pdf_fonts::INT_KEYS_NUM] {
        use crate::pdf_fonts::*;
        let mut dims = [0; INT_KEYS_NUM];
        let Some(font) = self.eqtb.fonts.get(f as usize) else {
            return dims;
        };
        let size = crate::pdfrender::pdf_font_size(font.at_size);
        if size == 0 {
            return dims;
        }
        let scaled = |s: i64| crate::pdfrender::divide_scaled(s, size, 3).0 as i32;
        let param = |n: usize| {
            self.eqtb.font_params.get(f as usize).and_then(|p| p.get(n - 1)).copied().unwrap_or(0)
        };
        // get_charheight & co.: 0 for characters the TFM lacks
        let metric = |c: u8, value: fn(&crate::tfm::Font, u8) -> i32| {
            if font.char_present(c) { i64::from(value(font, c)) } else { 0 }
        };
        let angle = -(f64::from(param(1)) / 65536.0).atan() * (180.0 / std::f64::consts::PI);
        dims[ITALIC_ANGLE_CODE] = scaled(angle as i64);
        dims[ASCENT_CODE] = scaled(metric(b'h', crate::tfm::Font::char_height));
        dims[CAPHEIGHT_CODE] = scaled(metric(b'H', crate::tfm::Font::char_height));
        dims[DESCENT_CODE] = (-scaled(metric(b'y', crate::tfm::Font::char_depth))).min(0);
        dims[STEMV_CODE] = scaled(metric(b'.', crate::tfm::Font::char_width) / 3);
        dims[XHEIGHT_CODE] = scaled(i64::from(param(5)));
        dims[FONTBBOX1_CODE] = 0;
        dims[FONTBBOX1_CODE + 1] = dims[DESCENT_CODE];
        dims[FONTBBOX1_CODE + 2] = scaled(i64::from(param(6)));
        dims[FONTBBOX1_CODE + 3] = dims[CAPHEIGHT_CODE].max(dims[ASCENT_CODE]);
        dims
    }

    /// `\pdfglyphtounicode {<glyph>} {<unicode>}` (pdftex.web
    /// `glyph_to_unicode`, tounicode.c `deftounicode`).
    pub(crate) fn do_pdfglyphtounicode(&mut self) {
        let toks = self.scan_general_text_expanded();
        let glyph = String::from_utf8_lossy(&self.tokens_to_bytes(&toks)).into_owned();
        let toks = self.scan_general_text_expanded();
        let unicode = String::from_utf8_lossy(&self.tokens_to_bytes(&toks)).into_owned();
        let table = &mut self.pdf_backend.glyph_unicode;
        if let Some(warning) = crate::pdf_fonts::def_tounicode(table, &glyph, &unicode) {
            self.warning_at(&warning, None);
        }
    }

    /// pdftex.web `warn_dest_dup` (silenced by `\pdfsuppresswarningdupdest`).
    pub(crate) fn warn_dest_dup(&mut self, id: &crate::pdfout::DestId) {
        if self.pdf_int(crate::prim::IntParam::PdfSuppressWarningDupDest) > 0 {
            return;
        }
        let id = match id {
            crate::pdfout::DestId::Name(name) => format!("name{{{name}}}"),
            crate::pdfout::DestId::Num(n) => format!("num{n}"),
        };
        self.warning_at(
            &format!(
                "destination with the same identifier ({id}) has been already used, duplicate ignored"
            ),
            None,
        );
    }

    /// pdftex.web `pdf_init_font` / `pdf_use_font`: give font `f` its PDF
    /// font resource, shared with an earlier initialized font of the same
    /// TFM (or of its expansion base), and return that owner `ff`.
    pub(crate) fn pdf_init_font(&mut self, f: u16) -> u16 {
        if let Some(&ff) = self.pdf_backend.font_ff.get(&f) {
            return ff;
        }
        let blink = self
            .eqtb
            .expand
            .get(f as usize)
            .filter(|x| x.auto_expand)
            .map_or(0, |x| x.blink);
        let ff = if blink != 0 {
            self.pdf_init_font(blink)
        } else {
            let name = self.eqtb.fonts.get(f as usize).map(|font| font.tfm_name.clone());
            let fonts = &self.eqtb.fonts;
            self.pdf_backend
                .font_reps
                .iter()
                .copied()
                .find(|&k| fonts.get(k as usize).map(|font| &font.tfm_name) == name.as_ref())
                .unwrap_or(f)
        };
        if ff == f {
            self.pdf_backend.font_reps.push(f);
        }
        self.pdf_backend.font_ff.insert(f, ff);
        if let Some(font) = self.eqtb.fonts.get(f as usize) {
            self.pdf_backend.mapped_in_use.insert(font.tfm_name.clone());
        }
        let move_chars = crate::prim::IntParam::PdfMoveChars.idx() as usize;
        if self.eqtb.int_params[move_chars] > 0 {
            self.warning_at("Primitive \\pdfmovechars is obsolete.", None);
            self.eqtb.int_params[move_chars] = 0; // warn only once
        }
        ff
    }

    /// `scan_font_ident` + the font checks of `\pdffontname`,
    /// `\pdffontobjnum` and `\pdfincludechars` (pdf_error is fatal).
    fn scan_pdf_font(&mut self, command: &str) -> Option<u16> {
        let source = self.current_token_source_mark();
        let f = self.scan_font_id();
        let fatal = |eng: &mut Engine, message: &str| {
            eng.fatal_error_at(message, source.as_ref().map(crate::input::SourceMark::to_context));
        };
        if f == 0 {
            fatal(self, &format!("pdfTeX error (font): invalid font identifier for {command}"));
            return None;
        }
        let font = &self.eqtb.fonts[f as usize];
        if self
            .font_loader
            .vf_fonts
            .contains_key(&(font.tfm_name.clone(), font.at_size))
        {
            fatal(self, &format!("pdfTeX error (font): {command} cannot be used with virtual font"));
            return None;
        }
        Some(f)
    }

    /// `\pdffontname <font>`: the number `n` of the font's `/F<n>` resource.
    pub(crate) fn pdf_font_name(&mut self) -> Option<i32> {
        let f = self.scan_pdf_font("\\pdffontname")?;
        Some(i32::from(self.pdf_init_font(f)))
    }

    /// `\pdffontobjnum <font>`: the object number of the font dictionary.
    pub(crate) fn pdf_font_objnum(&mut self) -> Option<i32> {
        let f = self.scan_pdf_font("\\pdffontobjnum")?;
        let ff = self.pdf_init_font(f);
        if let Some(&obj) = self.pdf_backend.font_objs.get(&ff) {
            return Some(obj);
        }
        let obj = self.alloc_pdf_obj();
        self.pdf_backend.font_objs.insert(ff, obj);
        Some(obj)
    }

    /// `\pdfpageref <page>`: the page object number (`get_obj(obj_type_page)`),
    /// fixed now even when the page has not been shipped yet.
    pub(crate) fn pdf_page_ref(&mut self) -> Option<i32> {
        let (page, source) = self.scan_int_with_source();
        if page <= 0 {
            self.fatal_error_at("pdfTeX error (pageref): invalid page number", source);
            return None;
        }
        if let Some(&obj) = self.pdf_backend.page_objs.get(&page) {
            return Some(obj);
        }
        let obj = self.alloc_pdf_obj();
        self.pdf_backend.page_objs.insert(page, obj);
        Some(obj)
    }

    /// `\pdfxformname <object>`: the `n` of the form's `/Fm<n>` resource.
    pub(crate) fn pdf_xform_name(&mut self) -> Option<i32> {
        let (obj, source) = self.scan_int_with_source();
        match self.pdf_doc.form_names.get(&obj) {
            Some(&name) => Some(name),
            None => {
                self.fatal_error_at("pdfTeX error (ext1): cannot find referenced object", source);
                None
            }
        }
    }

    /// `\pdfincludechars <font> {<chars>}`: subset these characters into
    /// the font even when no page shows them.
    pub(crate) fn do_pdfincludechars(&mut self) {
        let Some(f) = self.scan_pdf_font("\\pdfincludechars") else {
            return;
        };
        self.pdf_init_font(f);
        let toks = self.scan_general_text_expanded();
        for byte in self.tokens_to_bytes(&toks) {
            self.pdf_doc.record_font_char(f as usize, byte);
        }
    }

    /// `\pdftrailer {<keys>}`: extra trailer dictionary entries.
    pub(crate) fn do_pdftrailer(&mut self) {
        let toks = self.scan_general_text_expanded();
        let text = self.tokens_to_bytes(&toks);
        if self.pdf_int(crate::prim::IntParam::PdfOutput) > 0 {
            self.pdf_doc.trailer_extra.extend_from_slice(&text);
        }
    }

    /// `\pdfspacefont {<tfm>}`: the font for faked interword spaces.
    pub(crate) fn do_pdfspacefont(&mut self) {
        let toks = self.scan_general_text_expanded();
        let name = self.tokens_to_bytes(&toks);
        self.pdf_backend.space_font_name = String::from_utf8_lossy(&name).into_owned();
    }

    /// pdftex.web `make_font_copy`: `\pdfcopyfont <cs> = <font>` defines a
    /// new internal font with the TFM data, current parameters and hyphen
    /// and skew characters of `<font>` (for a separate `\pdffontexpand`).
    pub(crate) fn do_pdfcopyfont(&mut self) {
        let global = self.take_global();
        self.clear_prefixes();
        let u = self.scan_definable_cs();
        self.eqtb.assign(u, crate::eqtb::Equiv::FontRef(0), global);
        self.scan_optional_equals();
        let source = self.current_token_source_mark();
        let f = self.scan_font_id();
        let x = &self.eqtb.expand[f as usize];
        if x.ratio != 0 || x.step != 0 {
            self.fatal_error_at(
                "pdfTeX error (\\pdfcopyfont): cannot copy an expanded font",
                source.as_ref().map(crate::input::SourceMark::to_context),
            );
            return;
        }
        if self.font_loader.is_tracked_font(f) {
            self.fatal_error_at(
                "pdfTeX error (\\pdfcopyfont): cannot copy a letterspaced font",
                source.as_ref().map(crate::input::SourceMark::to_context),
            );
            return;
        }
        let font = self.eqtb.fonts[f as usize].clone();
        let k = self.push_engine_font(font, u);
        let k = k as usize;
        let f = f as usize;
        self.eqtb.font_params[k] = self.eqtb.font_params[f].clone();
        self.eqtb.font_param_levels[k] = vec![1; self.eqtb.font_params[k].len()];
        self.eqtb.hyphen_char[k] = self.eqtb.hyphen_char[f];
        self.eqtb.skew_char[k] = self.eqtb.skew_char[f];
        self.eqtb.assign(u, crate::eqtb::Equiv::FontRef(k as u16), global);
    }
}

/// writeimg.c `image_colordepth` for `\pdflastximagecolordepth`: the PNG
/// bit depth or JPEG sample precision; 0 for PDF (and other) inclusions.
pub(crate) fn image_color_depth(bytes: &[u8]) -> i32 {
    if bytes.len() > 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return i32::from(bytes[24]);
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut at = 2;
        while at + 4 < bytes.len() && bytes[at] == 0xff {
            let marker = bytes[at + 1];
            let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
            if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                return bytes.get(at + 4).map_or(0, |&p| i32::from(p));
            }
            at += 2 + len;
        }
    }
    0
}
