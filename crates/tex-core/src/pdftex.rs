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
    /// (full PFB, no subsetting) and rewrite page font references from
    /// engine font ids to document font indices.
    pub fn embed_used_fonts(&mut self) {
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
        let mut remap: Vec<(u16, usize)> = Vec::new();
        for (n, fid) in used.iter().enumerate() {
            let Some(font) = self.eqtb.fonts.get(*fid as usize).cloned() else {
                continue;
            };
            let pfb_bytes = font
                .type1_path
                .as_ref()
                .and_then(|name| self.font_loader.kpse.read(name, tex_kpse::Format::Type1));
            let widths = (0..=255u8)
                .map(|c| {
                    let w = font.char_width(c);
                    if font.at_size != 0 {
                        ((w as i64 * 10_000 + font.at_size as i64 / 2) / font.at_size as i64) as i32
                    } else {
                        0
                    }
                })
                .collect();
            let mut ef = crate::pdffile::make_embed_font(
                font.map_fontname
                    .clone()
                    .unwrap_or_else(|| font.tfm_name.clone()),
                pfb_bytes.as_deref(),
                font.encoding.as_deref(),
                0,
                255,
                widths,
            );
            crate::pdffile::set_font_usage(
                &mut ef,
                self.pdf_doc
                    .font_chars
                    .get(&(*fid as usize))
                    .copied()
                    .unwrap_or([0; 4]),
            );
            // PFBs of the CM family lack Ascent/Descent/CapHeight/StemV;
            // fall back to TFM-derived values where the cleartext had none.
            let (ta, td, tc, ts) = crate::pdf_fonts::tfm_descriptor(&font);
            if ef.ascent == 0.0 {
                ef.ascent = ta;
            }
            if ef.descent == 0.0 {
                ef.descent = td;
            }
            if ef.cap_height == 0.0 {
                ef.cap_height = tc;
            }
            if ef.stem_v == 0.0 {
                ef.stem_v = ts;
            }
            self.pdf_doc.fonts.push(ef);
            remap.push((*fid, n));
        }
        for fonts in self
            .pdf_doc
            .pages
            .iter_mut()
            .map(|p| &mut p.fonts)
            .chain(self.pdf_doc.form_fonts.iter_mut().map(|(_, fonts)| fonts))
        {
            for pf in fonts.iter_mut() {
                if let Some(pos) = remap.iter().find(|(fid, _)| *fid == pf.0 as u16) {
                    pf.0 = pos.1;
                }
            }
        }
    }
}
