//! `XeTeXFontMgr` (XeTeXFontMgr.cpp, XeTeXFontMgr_FC.cpp) over the embedded
//! font index: installed-font name lookup with `/B`, `/I`, optical sizes.
//!
//! TeXres is hermetic: the "installed" fonts are the faces of the embedded
//! package archive (`tex_kpse::embedded_font_faces`), where TeX Live's
//! fontconfig would see the system's fonts.

use crate::native_font::{parse_variant, ReqEngine};
use std::collections::{BTreeMap, HashMap};
use tex_kpse::EmbeddedFontFace;

#[derive(Clone, Copy, Debug, Default)]
pub struct OpSize {
    pub design_size: f64,
    pub sub_family_id: u32,
    pub name_code: u32,
    pub min_size: f64,
    pub max_size: f64,
}

#[derive(Debug)]
pub struct MgrFont {
    pub face: &'static EmbeddedFontFace,
    pub ps_name: String,
    pub full_name: Option<String>,
    pub weight: i32,
    pub width: i32,
    pub slant: i32,
    pub is_reg: bool,
    pub is_bold: bool,
    pub is_italic: bool,
    pub op: OpSize,
    pub parent: Option<String>,
}

#[derive(Debug, Default)]
struct Family {
    styles: BTreeMap<String, usize>,
    min_weight: i32,
    max_weight: i32,
    min_width: i32,
    max_width: i32,
    min_slant: i32,
    max_slant: i32,
}

/// Result of `findFont`.
#[derive(Debug)]
pub struct FoundFont {
    pub face: &'static EmbeddedFontFace,
    /// Name `getFullName` reports.
    pub full_name: String,
    pub design_size: i32,
    pub req_engine: ReqEngine,
    /// The variant string with `/B`, `/I`, `/S=` removed.
    pub variant: String,
}

#[derive(Default)]
pub struct FontMgr {
    fonts: Vec<MgrFont>,
    name_to_font: HashMap<String, usize>,
    ps_to_font: HashMap<String, usize>,
    name_to_family: HashMap<String, Family>,
    cached: Vec<bool>,
    cached_all: bool,
    started: bool,
}

fn split_list(s: &str) -> Vec<String> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split('\u{1f}').map(str::to_string).collect()
    }
}

impl FontMgr {
    fn faces() -> &'static [EmbeddedFontFace] {
        tex_kpse::embedded_font_faces()
    }

    fn init(&mut self) {
        if !self.started {
            self.started = true;
            self.cached = vec![false; Self::faces().len()];
        }
    }

    /// FC_FAMILY values of a face
    fn fc_families(face: &EmbeddedFontFace) -> Vec<String> {
        let mut v = vec![face.family.to_string()];
        for f in split_list(face.families) {
            if !v.contains(&f) {
                v.push(f);
            }
        }
        v
    }

    fn fc_styles(face: &EmbeddedFontFace) -> Vec<String> {
        let mut v = vec![face.subfamily.to_string()];
        for f in split_list(face.styles) {
            if !v.contains(&f) {
                v.push(f);
            }
        }
        v
    }

    fn family_names(face: &EmbeddedFontFace) -> Vec<String> {
        let l = split_list(face.families);
        if l.is_empty() {
            vec![face.postscript.to_string()]
        } else {
            l
        }
    }

    /// `addToMaps` for face `idx` of the embedded table.
    fn add_to_maps(&mut self, idx: usize) {
        if self.cached[idx] {
            return;
        }
        self.cached[idx] = true;
        let face = &Self::faces()[idx];
        if face.postscript.is_empty() || self.ps_to_font.contains_key(face.postscript) {
            return;
        }
        let family_names = Self::family_names(face);
        let style_names = split_list(face.styles);
        let full_names = split_list(face.fulls);
        let mut f = MgrFont {
            face,
            ps_name: face.postscript.to_string(),
            full_name: full_names.first().cloned(),
            weight: face.weight as i32,
            width: face.width as i32,
            slant: face.slant,
            is_reg: face.flags & 1 != 0,
            is_bold: face.flags & 2 != 0,
            is_italic: face.flags & 4 != 0,
            op: OpSize::default(),
            parent: None,
        };
        let [d, s, n, lo, hi] = face.opsize;
        if d != 0 {
            f.op.design_size = d as f64 * 72.27 / 72.0 / 10.0;
            if !(s == 0 && n == 0 && lo == 0 && hi == 0) {
                f.op.sub_family_id = s as u32;
                f.op.name_code = n as u32;
                f.op.min_size = lo as f64 * 72.27 / 72.0 / 10.0;
                f.op.max_size = hi as f64 * 72.27 / 72.0 / 10.0;
            }
        }
        let id = self.fonts.len();
        self.ps_to_font.insert(face.postscript.to_string(), id);
        let (w, wd, sl) = (f.weight, f.width, f.slant);
        for fam in &family_names {
            let family = self.name_to_family.entry(fam.clone()).or_insert_with(|| Family {
                min_weight: w,
                max_weight: w,
                min_width: wd,
                max_width: wd,
                min_slant: sl,
                max_slant: sl,
                ..Default::default()
            });
            family.min_weight = family.min_weight.min(w);
            family.max_weight = family.max_weight.max(w);
            family.min_width = family.min_width.min(wd);
            family.max_width = family.max_width.max(wd);
            family.min_slant = family.min_slant.min(sl);
            family.max_slant = family.max_slant.max(sl);
            if f.parent.is_none() {
                f.parent = Some(fam.clone());
            }
            for st in &style_names {
                family.styles.entry(st.clone()).or_insert(id);
            }
        }
        for full in &full_names {
            self.name_to_font.entry(full.clone()).or_insert(id);
        }
        self.fonts.push(f);
    }

    fn cache_family_members(&mut self, families: &[String]) {
        if families.is_empty() {
            return;
        }
        for i in 0..Self::faces().len() {
            if self.cached[i] {
                continue;
            }
            let face = &Self::faces()[i];
            if Self::fc_families(face).iter().any(|f| families.contains(f)) {
                self.add_to_maps(i);
            }
        }
    }

    /// `searchForHostPlatformFonts`
    fn search(&mut self, name: &str) {
        if self.cached_all {
            return;
        }
        let (fam_name, hyph) = match name.find('-') {
            Some(h) if h > 0 && h < name.len() - 1 => (name[..h].to_string(), true),
            _ => (String::new(), false),
        };
        let mut found = false;
        loop {
            for i in 0..Self::faces().len() {
                if self.cached[i] {
                    continue;
                }
                let face = &Self::faces()[i];
                if self.cached_all {
                    self.add_to_maps(i);
                    continue;
                }
                let mut hit = split_list(face.fulls).iter().any(|s| s == name);
                if !hit {
                    let styles = Self::fc_styles(face);
                    for fam in Self::fc_families(face) {
                        if fam == name || (hyph && fam == fam_name) {
                            hit = true;
                            break;
                        }
                        if styles.iter().any(|st| format!("{fam} {st}") == name) {
                            hit = true;
                            break;
                        }
                    }
                }
                if hit {
                    let fams = Self::family_names(face);
                    self.add_to_maps(i);
                    self.cache_family_members(&fams);
                    found = true;
                }
            }
            if found || self.cached_all {
                break;
            }
            self.cached_all = true;
        }
    }

    fn weight_and_width_diff(a: &MgrFont, b: &MgrFont) -> i32 {
        if a.weight == 0 && a.width == 0 {
            return if a.is_bold == b.is_bold { 0 } else { 10000 };
        }
        let mut wid = (a.width - b.width).abs();
        if wid < 10 {
            wid *= 50;
        }
        (a.weight - b.weight).abs() + wid
    }

    fn style_diff(a: &MgrFont, wt: i32, wd: i32, slant: i32) -> i32 {
        let mut wid = (a.width - wd).abs();
        if wid < 10 {
            wid *= 200;
        }
        (a.slant.abs() - slant.abs()).abs() * 2 + (a.weight - wt).abs() + wid
    }

    fn best_match(&self, fam: &Family, wt: i32, wd: i32, slant: i32) -> Option<usize> {
        let mut best: Option<usize> = None;
        for &i in fam.styles.values() {
            if best.is_none_or(|b| {
                Self::style_diff(&self.fonts[i], wt, wd, slant) < Self::style_diff(&self.fonts[b], wt, wd, slant)
            }) {
                best = Some(i);
            }
        }
        best
    }

    /// `findFont`: `pt_size` in TeX points, negative for a `scaled` factor.
    pub fn find_font(&mut self, name: &str, variant: Option<&str>, mut pt_size: f64) -> Option<FoundFont> {
        self.init();
        let mut dsize = 10.0f64;
        let mut loaded_design = 655360i32;
        let mut font: Option<usize> = None;
        for pass in 0..2 {
            if let Some(&i) = self.name_to_font.get(name) {
                font = Some(i);
                if self.fonts[i].op.design_size != 0.0 {
                    dsize = self.fonts[i].op.design_size;
                }
                break;
            }
            if let Some(h) = name.find('-') {
                if h > 0 && h < name.len() - 1 {
                    if let Some(f) = self.name_to_family.get(&name[..h]) {
                        if let Some(&i) = f.styles.get(&name[h + 1..]) {
                            font = Some(i);
                            if self.fonts[i].op.design_size != 0.0 {
                                dsize = self.fonts[i].op.design_size;
                            }
                            break;
                        }
                    }
                }
            }
            if let Some(&i) = self.ps_to_font.get(name) {
                font = Some(i);
                if self.fonts[i].op.design_size != 0.0 {
                    dsize = self.fonts[i].op.design_size;
                }
                break;
            }
            if let Some(f) = self.name_to_family.get(name) {
                let mut reg = 0;
                for &i in f.styles.values() {
                    if self.fonts[i].is_reg {
                        if reg == 0 {
                            font = Some(i);
                        }
                        reg += 1;
                    }
                }
                if font.is_none() || reg > 1 {
                    for s in ["Regular", "Plain", "Normal", "Roman"] {
                        if let Some(&i) = f.styles.get(s) {
                            font = Some(i);
                            break;
                        }
                    }
                }
                if font.is_none() {
                    font = self.best_match(f, 80, 100, 0);
                }
                if font.is_some() {
                    break;
                }
            }
            if pass == 0 {
                self.search(name);
            }
        }
        let mut idx = font?;
        let parent_name = self.fonts[idx].parent.clone();
        let mut req = ReqEngine::Default;
        let mut var_out = String::new();
        if let Some(v) = variant {
            let (req_bold, req_ital, size, r, vs) = parse_variant(v);
            req = r;
            var_out = vs;
            if let Some(s) = size {
                pt_size = s;
            }
            if let Some(parent) = parent_name.as_ref().and_then(|pn| self.name_to_family.get(pn)) {
                if req_ital {
                    let cur = idx;
                    let mut best = cur;
                    let f = &self.fonts[cur];
                    if f.slant < parent.max_slant {
                        if let Some(b) = self.best_match(parent, f.weight, f.width, parent.max_slant) {
                            best = b;
                        }
                    }
                    if best == cur && f.slant > parent.min_slant {
                        if let Some(b) = self.best_match(parent, f.weight, f.width, parent.min_slant) {
                            best = b;
                        }
                    }
                    if parent.min_weight == parent.max_weight && self.fonts[best].is_bold != f.is_bold {
                        for &i in parent.styles.values() {
                            if self.fonts[i].is_bold == f.is_bold && self.fonts[i].is_italic != f.is_italic {
                                best = i;
                                break;
                            }
                        }
                    }
                    let mut best_opt = Some(best);
                    if best == cur {
                        best_opt = None;
                        for &i in parent.styles.values() {
                            if self.fonts[i].is_italic == !f.is_italic {
                                if parent.min_weight != parent.max_weight {
                                    if best_opt.is_none_or(|b| {
                                        Self::weight_and_width_diff(&self.fonts[i], f)
                                            < Self::weight_and_width_diff(&self.fonts[b], f)
                                    }) {
                                        best_opt = Some(i);
                                    }
                                } else if best_opt.is_none() && self.fonts[i].is_bold == f.is_bold {
                                    best_opt = Some(i);
                                    break;
                                }
                            }
                        }
                    }
                    if let Some(b) = best_opt {
                        idx = b;
                    }
                }
                if req_bold {
                    let f = &self.fonts[idx];
                    let mut best = idx;
                    if f.weight < parent.max_weight {
                        if let Some(b) = self.best_match(
                            parent,
                            f.weight + (parent.max_weight - parent.min_weight) / 2 + 1,
                            f.width,
                            f.slant,
                        ) {
                            best = b;
                        }
                        if parent.min_slant == parent.max_slant {
                            let mut new_best: Option<usize> = None;
                            for &i in parent.styles.values() {
                                if self.fonts[i].is_italic == f.is_italic
                                    && new_best.is_none_or(|nb| {
                                        Self::weight_and_width_diff(&self.fonts[i], &self.fonts[best])
                                            < Self::weight_and_width_diff(&self.fonts[nb], &self.fonts[best])
                                    })
                                {
                                    new_best = Some(i);
                                }
                            }
                            if let Some(nb) = new_best {
                                best = nb;
                            }
                        }
                    }
                    if best == idx && !f.is_bold {
                        for &i in parent.styles.values() {
                            if self.fonts[i].is_italic == f.is_italic && self.fonts[i].is_bold {
                                best = i;
                                break;
                            }
                        }
                    }
                    idx = best;
                }
            }
        }
        // optical size
        if pt_size < 0.0 {
            pt_size = dsize;
        }
        let cur = &self.fonts[idx];
        if cur.op.sub_family_id != 0 && pt_size > 0.0 {
            let mut best_mismatch = (cur.op.min_size - pt_size).max(pt_size - cur.op.max_size);
            if best_mismatch > 0.0 {
                let mut best = idx;
                if let Some(fam) = parent_name.as_ref().and_then(|p| self.name_to_family.get(p)) {
                    for &i in fam.styles.values() {
                        let o = &self.fonts[i].op;
                        if o.sub_family_id != cur.op.sub_family_id {
                            continue;
                        }
                        let mismatch = (o.min_size - pt_size).max(pt_size - o.max_size);
                        if mismatch < best_mismatch {
                            best = i;
                            best_mismatch = mismatch;
                        }
                        if best_mismatch <= 0.0 {
                            break;
                        }
                    }
                }
                idx = best;
            }
        }
        let f = &self.fonts[idx];
        if f.op.design_size != 0.0 {
            loaded_design = (f.op.design_size * 65536.0 + 0.5) as u32 as i32;
        }
        Some(FoundFont {
            face: f.face,
            full_name: f.full_name.clone().unwrap_or_else(|| f.ps_name.clone()),
            design_size: loaded_design,
            req_engine: req,
            variant: var_out,
        })
    }
}
