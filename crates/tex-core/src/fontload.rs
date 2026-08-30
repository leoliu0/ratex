//! \font loading: TFM lookup via kpse, pdftex.map resolution, encoding
//! files, and the FontResolver implementation on Engine.

use crate::engine::Engine;
use crate::eqtb::Equiv;
use crate::prim::Prim;
use crate::tfm::{parse_tfm, Font};
use std::rc::Rc;

#[derive(Clone)]
pub struct MapEntry {
    pub tfm: String,
    pub fontname: String,
    pub enc_file: Option<String>,
    pub pfb: Option<String>,
    pub slant: f64,
    pub extend: f64,
}

pub struct FontLoader {
    pub kpse: tex_kpse::Kpse,
    pub map: std::collections::HashMap<String, MapEntry>,
    pub tfm_cache: std::collections::HashMap<(String, i32), Rc<Font>>,
    pub enc_cache: std::collections::HashMap<String, Rc<Vec<String>>>,
}

impl FontLoader {
    pub fn new() -> Self {
        let mut loader = FontLoader {
            kpse: tex_kpse::Kpse::new(),
            map: std::collections::HashMap::new(),
            tfm_cache: std::collections::HashMap::new(),
            enc_cache: std::collections::HashMap::new(),
        };
        loader.load_map("pdftex.map");
        loader
    }

    pub fn load_map(&mut self, name: &str) {
        let Some(path) = self.kpse.find(name, tex_kpse::Format::Map) else { return };
        let Ok(text) = std::fs::read_to_string(path) else { return };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('%') || line.starts_with('*') {
                continue;
            }
            if let Some(e) = parse_map_line(line) {
                self.map.insert(e.tfm.clone(), e);
            }
        }
    }

    pub fn load_tfm(&mut self, name: &str, at: i32) -> Option<Rc<Font>> {
        let key = (name.to_string(), at);
        if let Some(f) = self.tfm_cache.get(&key) {
            return Some(f.clone());
        }
        let data = self.kpse.read(name, tex_kpse::Format::Tfm)?;
        let mut font = parse_tfm(&data, name, at).ok()?;
        if let Some(me) = self.map.get(name).cloned() {
            font.map_fontname = Some(me.fontname.clone());
            if let Some(enc) = &me.enc_file {
                font.enc_name = Some(enc.clone());
                font.encoding = self.load_enc(enc).map(|e| (*e).clone());
            }
            if let Some(pfb) = &me.pfb {
                font.type1_path = Some(pfb.clone());
            }
        }
        let rc = Rc::new(font);
        self.tfm_cache.insert(key, rc.clone());
        Some(rc)
    }

    pub fn load_enc(&mut self, name: &str) -> Option<Rc<Vec<String>>> {
        if let Some(e) = self.enc_cache.get(name) {
            return Some(e.clone());
        }
        let data = self.kpse.read(name, tex_kpse::Format::Enc)?;
        let text = String::from_utf8_lossy(&data);
        // /Name [ /glyph1 /glyph2 ... ] def
        let start = text.find('[')?;
        let end = text.find(']')?;
        let body = &text[start + 1..end];
        let mut names = Vec::new();
        for tok in body.split_whitespace() {
            if let Some(g) = tok.strip_prefix('/') {
                names.push(g.to_string());
            }
        }
        let rc = Rc::new(names);
        self.enc_cache.insert(name.to_string(), rc.clone());
        Some(rc)
    }
}

pub fn parse_map_line(line: &str) -> Option<MapEntry> {
    let mut it = line.split_whitespace();
    let tfm = it.next()?.to_string();
    let fontname = it.next()?.to_string();
    let mut enc_file = None;
    let mut pfb = None;
    let mut slant = 0.0;
    let mut extend = 1.0;
    let mut expect_enc = false;
    let mut expect_pfb = false;
    for tok in it {
        if expect_enc {
            enc_file = Some(tok.to_string());
            expect_enc = false;
            continue;
        }
        if expect_pfb {
            let f = tok.trim_start_matches('"');
            pfb = Some(f.to_string());
            expect_pfb = false;
            continue;
        }
        if let Some(e) = tok.strip_prefix('@') {
            // @enc-file@ form
            if e.ends_with('@') {
                enc_file = Some(e[..e.len() - 1].to_string());
            }
            continue;
        }
        match tok {
            "<" | "<<" => expect_pfb = true,
            "\"" => {}
            _ => {
                if let Some(v) = tok.strip_suffix("SlantFont") {
                    slant = v.parse().unwrap_or(0.0);
                } else if let Some(v) = tok.strip_suffix("ExtendFont") {
                    extend = v.parse().unwrap_or(1.0);
                } else if tok.ends_with(".enc") {
                    enc_file = Some(tok.to_string());
                } else if tok == "ReEncodeFont" {
                } else if tok.starts_with('<') {
                    let f = tok.trim_start_matches('<');
                    if f.ends_with(".enc") {
                        enc_file = Some(f.to_string());
                    } else if !f.is_empty() {
                        pfb = Some(f.to_string());
                    }
                } else if tok.starts_with('"') {
                    // quoted options: skip
                }
            }
        }
    }
    Some(MapEntry { tfm, fontname, enc_file, pfb, slant, extend })
}

// ---------- Engine integration ----------

impl Engine {
    /// install the nullfont as font id 0 (TeX convention: font 0 = nullfont)
    pub fn add_nullfont(&mut self) {
        let nf = crate::tfm::Font {
            name: "nullfont".into(),
            tfm_name: "nullfont".into(),
            at_size: 0,
            dsize: 0,
            chars: Vec::new(),
            bc: 1,
            ec: 0,
            lig_kern: Vec::new(),
            kerns: Vec::new(),
            ext: Vec::new(),
            params: Vec::new(),
            hyphen_char: b'-' as i32,
            skew_char: -1,
            type1_path: None,
            enc_name: None,
            map_fontname: None,
            encoding: None,
        };
        self.eqtb.fonts.push(std::rc::Rc::new(nf));
        self.eqtb.font_params.push(Vec::new());
        self.eqtb.font_param_levels.push(Vec::new());
        self.eqtb.hyphen_char.push(b'-' as i32);
        self.eqtb.hyphen_char_levels.push(1);
        self.eqtb.skew_char.push(-1);
        self.eqtb.skew_char_levels.push(1);
        self.eqtb.font_cs.push(0);
    }

    pub fn do_font(&mut self) {
        // \font<cs>=<tfm> [at <dimen>|scaled <int>]
        let cs = self.scan_definable_cs();
        self.scan_optional_equals();
        let name = self.scan_font_name();
        let mut at = 0i32;
        self.skip_spaces_relax();
        if self.scan_keyword(b"at") {
            at = self.scan_dimen(false, false);
        } else if self.scan_keyword(b"scaled") {
            let s = self.scan_int();
            let dsize = self
                .font_loader
                .load_tfm(&name, 0)
                .map(|f| f.dsize)
                .unwrap_or(655360);
            at = crate::scaled::mult(dsize, s);
        }
        self.load_font_and_bind(&name, at, cs);
    }

    /// tex.web-style file-name scan for \font: letter/other chars accumulate,
    /// a space terminates the name (and is absorbed), anything else is pushed
    /// back. Uses the non-space-skipping fetch so "cmr10 at 10pt" splits.
    fn scan_font_name(&mut self) -> String {
        let mut name = Vec::new();
        loop {
            let t = self.get_x_raw();
            if t == crate::input::EOF_MARKER {
                break;
            }
            if t.is_cs() {
                self.pushed.push(t);
                break;
            }
            match t.cc() {
                11 | 12 => name.push(t.chr() as u8),
                10 => break, // space: name complete
                _ => {
                    self.pushed.push(t);
                    break;
                }
            }
            if name.len() > 256 {
                break;
            }
        }
        String::from_utf8_lossy(&name).to_string()
    }

    pub fn load_font_and_bind(&mut self, name: &str, at: i32, cs: crate::token::CsId) {
        let Some(font) = self.font_loader.load_tfm(name, at) else {
            self.error(&format!("Font \\{}={} not found", String::from_utf8_lossy(self.cs.name(cs)), name));
            return;
        };
        let id = self.eqtb.fonts.len() as u16;
        let params = font.params.clone();
        self.eqtb.fonts.push(font);
        self.eqtb.font_params.push(params);
        self.eqtb.font_param_levels.push(vec![1; self.eqtb.font_params[id as usize].len()]);
        self.eqtb.hyphen_char.push(b'-' as i32);
        self.eqtb.hyphen_char_levels.push(1);
        self.eqtb.skew_char.push(-1);
        self.eqtb.skew_char_levels.push(1);
        self.eqtb.font_cs.push(cs);
        self.eqtb.assign(cs, Equiv::FontRef(id), self.global_flag);
        self.global_flag = false;
        self.term.push_str(&format!("{} at {}\n", name, self.scaled_to_string(at)));
    }

    pub fn scan_pdf_origin(&mut self) -> u8 {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_cs() && self.cs.name(t.cs_id()) == b"direct" {
            1
        } else {
            self.pushed.push(t);
            0
        }
    }

    pub fn scan_pdf_string(&mut self) -> String {
        let toks = self.scan_general_text_expanded();
        self.write_tokens_to_string(&toks)
    }

    pub fn scan_link_attr(&mut self) -> String {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_cs() && self.cs.name(t.cs_id()) == b"user" {
            self.scan_pdf_string()
        } else {
            self.pushed.push(t);
            String::new()
        }
    }

    pub fn scan_link_dest(&mut self) -> (Option<String>, Option<String>) {
        self.skip_spaces_relax();
        let t = self.get_token();
        if !t.is_cs() {
            return (None, None);
        }
        match self.cs.name(t.cs_id()) {
            b"url" => {
                let s = self.scan_general_text();
                let text = self.write_tokens_to_string(&s);
                (Some(text), None)
            }
            b"name" => {
                let s = self.scan_general_text();
                let text = self.write_tokens_to_string(&s);
                (None, Some(text))
            }
            _ => (None, None),
        }
    }

    pub fn do_pdfdest(&mut self) {
        // \pdfdest name {name} xyz
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_cs() && self.cs.name(t.cs_id()) == b"name" {
            let s = self.scan_general_text();
            let name = self.write_tokens_to_string(&s);
            self.skip_spaces_relax();
            let t2 = self.get_token();
            let kind = if t2.is_cs() && self.cs.name(t2.cs_id()) == b"fitbh" { 1 } else { 0 };
            let node = crate::boxes::Node::Whatsit(crate::boxes::WhatIt::PdfDest { name, kind });
            match self.mode {
                crate::engine::Mode::Vertical | crate::engine::Mode::InternalVertical => self.vlist_append(node),
                _ => self.cur_list.push(node),
            }
        }
    }

    pub fn do_pdfoutline(&mut self) {
        // \pdfoutline goto name{dest} count -<n> {text}
        let mut dest = String::new();
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_cs() && self.cs.name(t.cs_id()) == b"goto" {
            self.skip_spaces_relax();
            let t2 = self.get_token();
            if t2.is_cs() && self.cs.name(t2.cs_id()) == b"name" {
                let s = self.scan_general_text();
                dest = self.write_tokens_to_string(&s);
            }
        }
        let mut count = 0i32;
        self.skip_spaces_relax();
        let t3 = self.get_token();
        if t3.is_cs() && self.cs.name(t3.cs_id()) == b"count" {
            count = self.scan_int();
        } else {
            self.pushed.push(t3);
        }
        let toks = self.scan_general_text();
        let title = self.write_tokens_to_string(&toks);
        self.pdf_outlines.push((title, dest, count));
    }
}
