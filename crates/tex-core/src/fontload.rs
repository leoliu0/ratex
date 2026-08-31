use std::rc::Rc;
use crate::engine::Engine;
use crate::tfm::{Font, parse_tfm};
use crate::eqtb::Equiv;

#[derive(Clone)]
pub struct MapEntry {
    pub tfm: String,
    pub fontname: String,
    /// encoding vector FILE from `<[foo.enc` / `<foo.enc`
    pub enc_file: Option<String>,
    /// encoding NAME inside the quoted options (`" T1Encoding ReEncodeFont "`);
    /// resolved as `<name>.enc` through kpathsea when no explicit file is given
    pub enc_name: Option<String>,
    pub pfb: Option<String>,
    pub slant: f64,
    pub extend: f64,
}

pub struct FontLoader {
    pub kpse: tex_kpse::Kpse,
    pub map: std::collections::HashMap<String, MapEntry>,
    pub tfm_cache: std::collections::HashMap<(String, i32), Rc<Font>>,
    pub enc_cache: std::collections::HashMap<String, Rc<Vec<String>>>,
    /// pdftex.map is loaded on first font lookup, not at construction:
    /// the find forces kpse database setup, which is pure startup waste
    /// for format-booted runs that never select a mapped font.
    map_loaded: bool,
}

impl FontLoader {
    pub fn new() -> Self {
        FontLoader {
            kpse: tex_kpse::Kpse::new(),
            map: std::collections::HashMap::new(),
            tfm_cache: std::collections::HashMap::new(),
            enc_cache: std::collections::HashMap::new(),
            map_loaded: false,
        }
    }
    /// Load pdftex.map on first use (idempotent).
    pub fn ensure_map(&mut self) {
        if !self.map_loaded {
            self.load_map("pdftex.map");
        }
    }

    /// Record that an explicit map operation took over (e.g. \pdfmapfile
    /// `=`-replace cleared `map`): suppresses the deferred default load.
    pub fn mark_map_loaded(&mut self) {
        self.map_loaded = true;
    }

    pub fn load_map(&mut self, name: &str) {
        self.map_loaded = true;
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
        self.ensure_map();
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
            } else if let Some(en) = &me.enc_name {
                // no explicit vector file: try the named encoding as
                // <name>.enc (e.g. TeXBase1Encoding alongside <8r.enc)
                if let Some(e) = self.load_enc(en) {
                    font.enc_name = Some(en.clone());
                    font.encoding = Some((*e).clone());
                }
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

/// Split a map line into bare tokens and quoted option sections. Quotes
/// toggle sections and may be attached to the first/last word of a section
/// (`".167 SlantFont"`), as emitted by updmap and dvips maps alike.
fn split_map_tokens(line: &str) -> (Vec<String>, Vec<String>) {
    let mut bare = Vec::new();
    let mut quoted = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in line.chars() {
        match c {
            '"' => {
                if in_quote {
                    quoted.push(std::mem::take(&mut cur));
                }
                in_quote = !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    bare.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        if in_quote {
            quoted.push(cur);
        } else {
            bare.push(cur);
        }
    }
    (bare, quoted)
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Parse one pdftex.map entry:
/// `tfm psname [<enc-file>|<[enc-file]|<<font-file|<font-file> ["option words"]`
/// Option words: `<num> SlantFont` / `<num> ExtendFont` / `<name> ReEncodeFont`
/// (num may also be fused: `.167SlantFont`).
pub fn parse_map_line(line: &str) -> Option<MapEntry> {
    let (bare, quoted) = split_map_tokens(line);
    let mut it = bare.into_iter();
    let tfm = it.next()?.to_string();
    let fontname = it.next()?.to_string();
    let mut enc_file: Option<String> = None;
    let mut enc_name: Option<String> = None;
    let mut pfb: Option<String> = None;
    let mut slant = 0.0;
    let mut extend = 1.0;
    for tok in it {
        if let Some(rest) = tok.strip_prefix('<') {
            // `<<` (include without re-encoding) takes the same file kind
            let rest = rest.trim_start_matches('<');
            let rest = rest.strip_prefix('[').unwrap_or(rest);
            if rest.is_empty() {
                continue;
            }
            if rest.ends_with(".enc") {
                if enc_file.is_none() {
                    enc_file = Some(basename(rest).to_string());
                }
            } else if pfb.is_none() {
                pfb = Some(basename(rest).to_string());
            }
        }
    }
    for section in quoted {
        let mut prev: Option<String> = None;
        for w in section.split_whitespace() {
            let (word, fused) = match (w.strip_suffix("SlantFont"), w.strip_suffix("ExtendFont")) {
                (Some(v), _) => (v, 1),
                (_, Some(v)) => (v, 2),
                (None, None) => ("", 0),
            };
            match fused {
                1 => {
                    slant = if word.is_empty() {
                        prev.as_deref().and_then(|p| p.parse().ok()).unwrap_or(slant)
                    } else {
                        word.parse().unwrap_or(slant)
                    };
                    prev = None;
                }
                2 => {
                    extend = if word.is_empty() {
                        prev.as_deref().and_then(|p| p.parse().ok()).unwrap_or(extend)
                    } else {
                        word.parse().unwrap_or(extend)
                    };
                    prev = None;
                }
                _ => {
                    if w == "ReEncodeFont" {
                        if let Some(p) = prev.take().filter(|p| p.parse::<f64>().is_err()) {
                            enc_name = Some(p);
                        }
                    } else {
                        prev = Some(w.to_string());
                    }
                }
            }
        }
    }
    Some(MapEntry { tfm, fontname, enc_file, enc_name, pfb, slant, extend })
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
        self.term.push_str(&format!("{} at {}\n", name, self.scaled_to_string(self.eqtb.fonts[id as usize].at_size)));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_pfb_entry() {
        let e = parse_map_line("cmr10 CMR10 <cmr10.pfb").unwrap();
        assert_eq!(e.tfm, "cmr10");
        assert_eq!(e.fontname, "CMR10");
        assert_eq!(e.pfb.as_deref(), Some("cmr10.pfb"));
        assert_eq!(e.enc_file, None);
        assert_eq!(e.enc_name, None);
        assert_eq!(e.slant, 0.0);
    }

    #[test]
    fn updmap_bracket_enc_entry() {
        // real TeX Live line (newtx): <[ntx-ec-tlf.enc must be an
        // ENCODING file, not a font file
        let e = parse_map_line(
            "ntx-Regular-tlf-t1 TeXGyreTermesX-Regular \" encntx-ec-tlf ReEncodeFont \" <[ntx-ec-tlf.enc <ztmr.pfb",
        )
        .unwrap();
        assert_eq!(e.enc_file.as_deref(), Some("ntx-ec-tlf.enc"));
        assert_eq!(e.enc_name.as_deref(), Some("encntx-ec-tlf"));
        assert_eq!(e.pfb.as_deref(), Some("ztmr.pfb"));
    }

    #[test]
    fn cmsuper_named_encoding_entry() {
        let e = parse_map_line(
            "ecrm1000 SFRM1000 \" T1Encoding ReEncodeFont \" <cm-super-t1.enc <sfrm1000.pfb",
        )
        .unwrap();
        assert_eq!(e.enc_file.as_deref(), Some("cm-super-t1.enc"));
        assert_eq!(e.enc_name.as_deref(), Some("T1Encoding"));
        assert_eq!(e.pfb.as_deref(), Some("sfrm1000.pfb"));
    }

    #[test]
    fn slant_with_separate_value() {
        let e = parse_map_line("rtcxl rtcxr \".167 SlantFont\" <rtcxr.pfb").unwrap();
        assert!((e.slant - 0.167).abs() < 1e-9);
        assert_eq!(e.pfb.as_deref(), Some("rtcxr.pfb"));
    }

    #[test]
    fn slant_with_separate_value_in_open_quote() {
        let e = parse_map_line(
            "pbkdo8r URWBookmanL-DemiBold \" .167 SlantFont TeXBase1Encoding ReEncodeFont \" <8r.enc <ubkd8a.pfb",
        )
        .unwrap();
        assert!((e.slant - 0.167).abs() < 1e-9);
        assert_eq!(e.enc_file.as_deref(), Some("8r.enc"));
        assert_eq!(e.enc_name.as_deref(), Some("TeXBase1Encoding"));
    }

    #[test]
    fn double_angle_means_no_reencode() {
        let e = parse_map_line("foo Foo <<foo.pfb").unwrap();
        assert_eq!(e.pfb.as_deref(), Some("foo.pfb"));
    }

    #[test]
    fn path_components_are_stripped() {
        let e = parse_map_line("foo Foo <fonts/enc/foo.enc <fonts/type1/foo.pfb").unwrap();
        assert_eq!(e.enc_file.as_deref(), Some("foo.enc"));
        assert_eq!(e.pfb.as_deref(), Some("foo.pfb"));
    }

    #[test]
    fn extend_fused_form() {
        let e = parse_map_line("foo Foo \"1.5ExtendFont\" <foo.pfb").unwrap();
        assert!((e.extend - 1.5).abs() < 1e-9);
    }
}
