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

/// One base font declared by a virtual font's `fnt_def`.
#[derive(Clone)]
pub struct VfBase {
    pub tfm_name: String,
    /// resolved `at` size in sp (scaled with the VF's use size)
    pub at_size: i32,
}

/// One glyph-setting step of a VF character packet: draw `ch` from base
/// font `base` at (dx, dy) offset from the virtual glyph's origin (sp).
#[derive(Clone, Copy)]
pub struct VfStep {
    pub base: u8,
    pub ch: u8,
    pub dx: i32,
    pub dy: i32,
}

/// Parsed virtual font. Packet movements follow DVI semantics with the
/// VF's own DVI unit = `at_size / 2^20` sp; set_char advances by the base
/// font's TFM width at the derived base size.
#[derive(Clone)]
pub struct VfFont {
    pub bases: Vec<VfBase>,
    /// per character code 0..=255
    pub chars: Vec<Option<Rc<[VfStep]>>>,
}

pub struct FontLoader {
    pub kpse: tex_kpse::Kpse,
    pub map: std::collections::HashMap<String, MapEntry>,
    pub tfm_cache: std::collections::HashMap<(String, i32), Rc<Font>>,
    pub enc_cache: std::collections::HashMap<String, Rc<Vec<String>>>,
    /// virtual fonts by (tfm name, resolved at size)
    pub vf_fonts: std::collections::HashMap<(String, i32), Rc<VfFont>>,
    /// engine font id of a VF-backed font -> base engine font ids
    /// (u16::MAX = the base TFM was missing at load time)
    pub vf_bases: std::collections::HashMap<u16, Vec<u16>>,
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
            vf_fonts: std::collections::HashMap::new(),
            vf_bases: std::collections::HashMap::new(),
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
        // Virtual font support: when this TFM has no usable physical font
        // of its own (no map entry / no resolvable pfb, or the map points
        // at a .vf) but a same-stem .vf exists, glyph rendering is
        // delegated to the VF's base fonts; the VF name itself never
        // reaches the PDF.
        let pfb = self.map.get(name).and_then(|m| m.pfb.clone());
        let has_pfb = match &pfb {
            Some(p) => !p.ends_with(".vf") && self.kpse.find(p, tex_kpse::Format::Type1).is_some(),
            None => false,
        };
        if !has_pfb {
            if let Some(vf) = self
                .kpse
                .read(name, tex_kpse::Format::Vf)
                .and_then(|d| self.parse_vf(&d, font.at_size))
            {
                font.map_fontname = None;
                font.type1_path = None;
                font.enc_name = None;
                font.encoding = None;
                self.vf_fonts.insert((name.to_string(), font.at_size), vf);
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
        let rc = Rc::new(parse_enc_names(&text)?);
        self.enc_cache.insert(name.to_string(), rc.clone());
        Some(rc)
    }
}

/// Strip PostScript `%`-to-end-of-line comments, keeping string literals
/// `( ... )` (with nesting and `\` escapes) intact.
fn strip_ps_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut in_str = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            match c {
                '\\' => {
                    if let Some(&n) = chars.peek() {
                        out.push(n);
                        chars.next();
                    }
                }
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        in_str = false;
                    }
                }
                _ => {}
            }
        } else {
            match c {
                '%' => {
                    while let Some(&n) = chars.peek() {
                        if n == '\n' {
                            break;
                        }
                        chars.next();
                    }
                }
                '(' => {
                    in_str = true;
                    depth = 1;
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
    }
    out
}

/// Glyph names from an encoding vector: `/Name [ /glyph1 /glyph2 ... ] def`.
/// PostScript comments and blank lines are ignored.
fn parse_enc_names(text: &str) -> Option<Vec<String>> {
    let text = strip_ps_comments(text);
    let start = text.find('[')?;
    let end = start + text[start..].find(']')?;
    Some(
        text[start + 1..end]
            .split_whitespace()
            .filter_map(|tok| tok.strip_prefix('/'))
            .map(str::to_string)
            .collect(),
    )
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

// ---------- Virtual font (VF) binary parsing ----------

fn vf_uint(data: &[u8], pos: &mut usize, n: usize) -> Option<u32> {
    if n == 0 || n > 4 || *pos + n > data.len() {
        return None;
    }
    let mut v = 0u32;
    for i in 0..n {
        v = (v << 8) | data[*pos + i] as u32;
    }
    *pos += n;
    Some(v)
}

fn vf_sint(data: &[u8], pos: &mut usize, n: usize) -> Option<i32> {
    if n == 0 || n > 4 || *pos + n > data.len() {
        return None;
    }
    let mut v = data[*pos] as i8 as i32;
    *pos += 1;
    for _ in 1..n {
        v = (v << 8) | data[*pos] as i32;
        *pos += 1;
    }
    Some(v)
}

impl FontLoader {
    /// Parse a VF font used at `at` sp. Base fonts declared by `fnt_def`
    /// are loaded through the normal TFM/map machinery so that their char
    /// widths (at the derived base sizes) drive packet advance computation.
    pub fn parse_vf(&mut self, data: &[u8], at: i32) -> Option<Rc<VfFont>> {
        if data.len() < 11 || data[0] != 247 || data[1] != 202 {
            return None;
        }
        let k = data[2] as usize;
        let mut pos = 3 + k + 8; // skip comment, checksum, design size
        let mut base_idx: std::collections::HashMap<u32, u8> = std::collections::HashMap::new();
        let mut bases: Vec<Option<Rc<Font>>> = Vec::new();
        let mut base_specs: Vec<VfBase> = Vec::new();
        let mut chars: Vec<Option<Vec<VfStep>>> = vec![None; 256];
        loop {
            if pos >= data.len() {
                return None; // ran off the end without `post`
            }
            let op = data[pos];
            pos += 1;
            match op {
                243..=246 => {
                    // fnt_def: k(i) c[4] s[4] d[4] a[1] l[1] dir[a] name[l]
                    let id = vf_uint(data, &mut pos, 1 + (op - 243) as usize)?;
                    pos += 4; // checksum
                    let s = vf_sint(data, &mut pos, 4)?; // scaled size (20.12)
                    pos += 4; // design size
                    let a = *data.get(pos)? as usize;
                    pos += 1;
                    let l = *data.get(pos)? as usize;
                    pos += 1;
                    pos += a; // directory part is ignored for lookup
                    if pos + l > data.len() {
                        return None;
                    }
                    let name = String::from_utf8_lossy(&data[pos..pos + l]).to_string();
                    pos += l;
                    // 1 DVI unit of this VF = at / 2^20 sp
                    let at_base =
                        ((s as i64 * at as i64 + if s >= 0 { 0x80000 } else { -0x80000 }) >> 20) as i32;
                    let loaded = if at_base > 0 { self.load_tfm(&name, at_base) } else { None };
                    if id <= u8::MAX as u32 {
                        base_idx.insert(id, bases.len() as u8);
                    }
                    bases.push(loaded);
                    base_specs.push(VfBase { tfm_name: name, at_size: at_base });
                }
                242 => {
                    // long_char: 242 pl[4] cc[4] tfm[4] dvi[pl]
                    let pl = vf_uint(data, &mut pos, 4)? as usize;
                    let cc = vf_uint(data, &mut pos, 4)? as usize;
                    pos += 4; // char width (already in the outer TFM)
                    if cc >= 256 || pos + pl > data.len() {
                        return None;
                    }
                    let mut sub = (pos, pos + pl);
                    pos += pl;
                    chars[cc] = Self::vf_packet(data, &mut sub, &base_idx, &bases, at);
                    pos = sub.0.min(pos);
                }
                0..=241 => {
                    // short_char: pl(=op) cc[1] tfm[3] dvi[pl]
                    let pl = op as usize;
                    if pos + 4 + pl > data.len() {
                        return None;
                    }
                    let cc = data[pos] as usize;
                    pos += 4; // cc + char width
                    let mut sub = (pos, pos + pl);
                    pos += pl;
                    chars[cc] = Self::vf_packet(data, &mut sub, &base_idx, &bases, at);
                    pos = sub.0.min(pos);
                }
                248 => break, // post: end of packets
                _ => return None, // 249..=255 invalid
            }
        }
        Some(Rc::new(VfFont {
            bases: base_specs,
            chars: chars.into_iter().map(|c| c.map(Rc::from)).collect(),
        }))
    }

    /// DVI commands of one char packet; stops (without consuming) at the
    /// first op that begins the next packet (`>= 242`) or `post`.
    fn vf_packet(
        data: &[u8],
        cur: &mut (usize, usize),
        base_idx: &std::collections::HashMap<u32, u8>,
        bases: &[Option<Rc<Font>>],
        at: i32,
    ) -> Option<Vec<VfStep>> {
        let scale = |raw: i32| -> i64 {
            (raw as i64 * at as i64 + if raw >= 0 { 0x80000 } else { -0x80000 }) >> 20
        };
        let mut steps: Vec<VfStep> = Vec::new();
        let (mut x, mut y) = (0i64, 0i64);
        let (mut reg_w, mut reg_x, mut reg_y, mut reg_z) = (0i64, 0i64, 0i64, 0i64);
        // the font register starts at 0 for every packet (VF spec)
        let mut font: u8 = 0;
        let mut stack: Vec<(i64, i64, i64, i64, i64, i64, u8)> = Vec::new();
        loop {
            let pos = cur.0;
            if pos >= cur.1 {
                break;
            }
            let op = data[pos];
            if op >= 242 {
                break; // next packet (fnt_def / long packet / post)
            }
            cur.0 += 1;
            match op {
                0..=127 => {
                    Self::vf_step(&mut steps, base_idx, bases, font, op as u8, x, y);
                    x += Self::vf_advance(bases, font, op as u8); // set_char advances
                }
                128..=131 => {
                    let c = vf_uint(data, &mut cur.0, 1 + (op - 128) as usize)? as u8;
                    Self::vf_step(&mut steps, base_idx, bases, font, c, x, y);
                    x += Self::vf_advance(bases, font, c);
                }
                132 => cur.0 = (cur.0 + 8).min(cur.1), // set_rule: ignored
                133..=136 => {
                    let c = vf_uint(data, &mut cur.0, 1 + (op - 133) as usize)? as u8;
                    Self::vf_step(&mut steps, base_idx, bases, font, c, x, y); // put: no advance
                }
                137 => cur.0 = (cur.0 + 8).min(cur.1), // put_rule: ignored
                138 => {}                              // nop
                141 => stack.push((x, y, reg_w, reg_x, reg_y, reg_z, font)),
                142 => {
                    let s = stack.pop()?;
                    (x, y, reg_w, reg_x, reg_y, reg_z, font) = s;
                }
                143..=146 => x += scale(vf_sint(data, &mut cur.0, 1 + (op - 143) as usize)?),
                147 => x += reg_w,
                148..=151 => {
                    reg_w = scale(vf_sint(data, &mut cur.0, 1 + (op - 148) as usize)?);
                    x += reg_w;
                }
                152 => x += reg_x,
                153..=156 => {
                    reg_x = scale(vf_sint(data, &mut cur.0, 1 + (op - 153) as usize)?);
                    x += reg_x;
                }
                157..=160 => y += scale(vf_sint(data, &mut cur.0, 1 + (op - 157) as usize)?),
                161 => y += reg_y,
                162..=165 => {
                    reg_y = scale(vf_sint(data, &mut cur.0, 1 + (op - 162) as usize)?);
                    y += reg_y;
                }
                166 => y += reg_z,
                167..=170 => {
                    reg_z = scale(vf_sint(data, &mut cur.0, 1 + (op - 167) as usize)?);
                    y += reg_z;
                }
                171..=234 => font = (op - 171) as u8,
                235..=238 => font = vf_uint(data, &mut cur.0, 1 + (op - 235) as usize)? as u8,
                239..=241 => {
                    // xxx: specials are ignored
                    let n = vf_uint(data, &mut cur.0, 1 + (op - 239) as usize)? as usize;
                    cur.0 = (cur.0 + n).min(cur.1);
                }
                _ => unreachable!(), // ops >= 242 break out above
            }
        }
        Some(steps)
    }

    /// record a glyph step if the referenced base font is usable
    fn vf_step(
        steps: &mut Vec<VfStep>,
        base_idx: &std::collections::HashMap<u32, u8>,
        bases: &[Option<Rc<Font>>],
        font: u8,
        ch: u8,
        x: i64,
        y: i64,
    ) {
        if let Some(&bi) = base_idx.get(&(font as u32)) {
            if bases.get(bi as usize).map_or(false, |b| b.is_some()) {
                steps.push(VfStep { base: bi, ch, dx: x as i32, dy: y as i32 });
            }
        }
    }

    /// set_char advance: the base font's TFM width at its derived size (sp)
    fn vf_advance(bases: &[Option<Rc<Font>>], font: u8, ch: u8) -> i64 {
        bases
            .get(font as usize)
            .and_then(|b| b.as_ref())
            .map(|b| b.char_width(ch) as i64)
            .unwrap_or(0)
    }
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
        let cs = self.cs.intern(b"nullfont");
        self.eqtb.font_cs.push(cs);
        self.eqtb.assign(cs, crate::eqtb::Equiv::FontRef(0), true);
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
        self.skip_spaces_relax();
        let first = self.get_x_raw();
        if first == crate::input::EOF_MARKER {
            return String::new();
        }
        if first.is_char() && first.chr() == b'"' as u32 {
            // Quoted font name: \font\f="[FontFile.otf]:features" or "Font Name"
            while let t = self.get_x_raw() {
                if t == crate::input::EOF_MARKER || (t.is_char() && t.chr() == b'"' as u32) {
                    break;
                }
                if t.is_char() {
                    let c = t.chr();
                    if c < 128 {
                        name.push(c as u8);
                    } else {
                        let mut buf = [0u8; 4];
                        if let Some(ch) = char::from_u32(c) {
                            name.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                    }
                }
            }
            return String::from_utf8_lossy(&name).to_string();
        }
        self.pushed.push(first);
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
        // If name refers to an OpenType / TrueType font (e.g. "[path/to/font.otf]" or ends with .otf/.ttf/.ttc):
        let clean_name = name.trim_start_matches('[').trim_end_matches(']');
        let is_otf = clean_name.ends_with(".otf") || clean_name.ends_with(".ttf") || clean_name.ends_with(".ttc") || clean_name.starts_with('/');
        if is_otf {
            if let Ok(data) = std::fs::read(clean_name) {
                if let Ok(face) = ttf_parser::Face::parse(&data, 0) {
                    let upem = face.units_per_em() as i32;
                    let at_size = if at > 0 { at } else { 10 * 65536 };
                    let mut font = crate::tfm::Font {
                        name: String::from_utf8_lossy(self.cs.name(cs)).to_string(),
                        tfm_name: clean_name.to_string(),
                        at_size,
                        dsize: at_size,
                        chars: Vec::new(),
                        bc: 0,
                        ec: 255,
                        lig_kern: Vec::new(),
                        kerns: Vec::new(),
                        ext: Vec::new(),
                        params: vec![0; 8],
                        hyphen_char: 45,
                        skew_char: -1,
                        type1_path: None,
                        enc_name: None,
                        map_fontname: Some(clean_name.to_string()),
                        encoding: None,
                    };
                    // Populate default ASCII characters from OpenType metrics
                    for c in 0..=255u8 {
                        let w = face.glyph_index(c as char)
                            .and_then(|gid| face.glyph_hor_advance(gid))
                            .map(|adv| ((adv as i64 * at_size as i64) / upem as i64) as i32)
                            .unwrap_or(0);
                        font.chars.push(crate::tfm::CharInfo {
                            width: w,
                            height: (at_size as f64 * 0.7) as i32,
                            depth: (at_size as f64 * 0.2) as i32,
                            italic: 0,
                            tag: 0,
                            remainder: 0,
                        });
                    }
                    let id = self.push_engine_font(std::rc::Rc::new(font), cs);
                    self.eqtb.assign(cs, Equiv::FontRef(id), self.global_flag);
                    self.global_flag = false;
                    self.term.push_str(&format!("{} (OpenType) at {}\n", clean_name, self.scaled_to_string(at_size)));
                    return;
                }
            }
        }
        let Some(font) = self.font_loader.load_tfm(name, at) else {
            self.error(&format!("Font \\{}={} not found", String::from_utf8_lossy(self.cs.name(cs)), name));
            return;
        };
        let at_size = font.at_size;
        let id = self.push_engine_font(font, cs);
        self.eqtb.assign(cs, Equiv::FontRef(id), self.global_flag);
        self.global_flag = false;
        self.term.push_str(&format!("{} at {}\n", name, self.scaled_to_string(self.eqtb.fonts[id as usize].at_size)));
        // A VF-backed font gets its base fonts registered as engine fonts
        // (without control-sequence bindings) so glyph emission can address
        // them directly.
        if let Some(vf) = self.font_loader.vf_fonts.get(&(name.to_string(), at_size)).cloned() {
            let mut fids = Vec::with_capacity(vf.bases.len());
            for b in &vf.bases {
                let fid = match self.font_loader.load_tfm(&b.tfm_name, b.at_size) {
                    Some(bf) => self.push_engine_font(bf, 0), // unbound: font_cs 0 like nullfont
                    None => u16::MAX,
                };
                fids.push(fid);
            }
            self.font_loader.vf_bases.insert(id, fids);
        }
    }

    /// append a font to the engine font tables; returns its font id
    fn push_engine_font(&mut self, font: Rc<Font>, cs: crate::token::CsId) -> u16 {
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
        id
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
    fn enc_parser_ignores_ps_comments() {
        // shape of real files like ntx-ec-tlf.enc / t1-raw.enc: leading
        // header comment, a full-line comment inside the array (with a
        // stray `]` and `/glyph` mention), and a trailing `%` comment
        let src = concat!(
            "%PS-AdobeFont encoding vector\n",
            "/encntx-ec [\n",
            "% comment mentioning /fake and a ] bracket\n",
            "/grave /acute %caron breve\n",
            "\n",
            "/circumflex\n",
            "] def\n",
        );
        let names = parse_enc_names(src).unwrap();
        assert_eq!(names, vec!["grave", "acute", "circumflex"]);
    }

    #[test]
    fn enc_string_literals_protect_percent() {
        // a `%` inside a PostScript string literal is not a comment
        let names = parse_enc_names("/enc [/a (100% done) % real comment\n/b] def").unwrap();
        assert_eq!(names, vec!["a", "b"]);
    }
    /// Minimal synthetic TFM: chars 0..=1, char 1 width 0.25em, dsize 10pt.
    /// Layout: 12 u16 params, checksum, dsize, 2 char infos, 2 widths,
    /// height, depth, italic (64 bytes = lf 16 words).
    fn toy_tfm() -> Vec<u8> {
        let mut b = Vec::new();
        for v in [15u16, 2, 0, 1, 2, 1, 1, 1, 0, 0, 0, 0] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.extend_from_slice(&0u32.to_be_bytes()); // checksum
        b.extend_from_slice(&(10 * 0x100000u32).to_be_bytes()); // design size
        b.extend_from_slice(&0u32.to_be_bytes()); // char 0: no width (idx 0)
        b.extend_from_slice(&[1, 0, 0, 0]); // char 1: width idx 1
        b.extend_from_slice(&0u32.to_be_bytes()); // width[0] = 0 (unused)
        b.extend_from_slice(&0x40000u32.to_be_bytes()); // width[1] = 0.25em
        b.extend_from_slice(&0u32.to_be_bytes()); // height[0]
        b.extend_from_slice(&0u32.to_be_bytes()); // depth[0]
        b.extend_from_slice(&0u32.to_be_bytes()); // italic[0]
        b
    }

    /// Synthetic VF over `toybase`: short+long packets, right/w/down moves,
    /// put vs set, rule skipping.
    #[test]
    fn vf_parse_synthetic() {
        let dir = std::env::temp_dir().join(format!("vfparse_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("toybase.tfm"), toy_tfm()).unwrap();
        std::fs::write(dir.join("toyvf.tfm"), toy_tfm()).unwrap();
        let mut vf = Vec::new();
        vf.extend_from_slice(&[247, 202, 0]); // id, version, empty comment
        vf.extend_from_slice(&0u32.to_be_bytes()); // checksum
        vf.extend_from_slice(&(10 * 0x100000u32).to_be_bytes()); // design size
        vf.extend_from_slice(&[243, 0, 0, 0, 0, 0, 0x00, 0x10, 0, 0, 0, 0xa0, 0, 0, 0, 7]);
        vf.extend_from_slice(b"toybase");
        // short packets: pl(=op) cc[1] tfm[3] dvi[pl]; tfm width 0.26em = 0428f6
        // char 5: set_char_1 (advance 163840), right4 (+20480sp), set_char_1
        vf.extend_from_slice(&[7, 5, 0x04, 0x28, 0xf6, 0x01, 146, 0, 0, 0x80, 0, 0x01]);
        // char 6: down4 (+655360sp), put1 char 1 (no advance)
        vf.extend_from_slice(&[7, 6, 0x04, 0x28, 0xf6, 160, 0, 0x10, 0, 0, 133, 0x01]);
        // char 7: w2 (+10486 raw = 6554sp), set_char_1, set_rule (skipped), set_char_1
        vf.extend_from_slice(&[
            14, 7, 0x04, 0x28, 0xf6, 149, 0x28, 0xf6, 0x01, 132, 0, 0, 0, 0x10, 0, 0, 0, 0x10, 0x01,
        ]);
        // char 200: long_char 242 pl[4] cc[4] tfm[4] dvi[pl]; put1 char 1
        vf.extend_from_slice(&[242, 0, 0, 0, 2, 0, 0, 0, 200, 0x04, 0x28, 0xf6, 0x00, 133, 0x01]);
        vf.push(248); // post
        std::fs::write(dir.join("toyvf.vf"), &vf).unwrap();

        let mut fl = FontLoader {
            kpse: tex_kpse::Kpse::explicit(&dir, vec![]),
            map: std::collections::HashMap::new(),
            tfm_cache: std::collections::HashMap::new(),
            enc_cache: std::collections::HashMap::new(),
            vf_fonts: std::collections::HashMap::new(),
            vf_bases: std::collections::HashMap::new(),
            map_loaded: true,
        };
        let font = fl.load_tfm("toyvf", 655360).expect("toyvf loads");
        assert_eq!(font.at_size, 655360);
        assert_eq!(font.type1_path, None, "virtual font must not carry a pfb");
        let vfv = fl
            .vf_fonts
            .get(&("toyvf".to_string(), 655360))
            .expect("vf parsed");
        assert_eq!(vfv.bases.len(), 1);
        assert_eq!(vfv.bases[0].tfm_name, "toybase");
        assert_eq!(vfv.bases[0].at_size, 655360);
        let at = 655360i64;
        let steps5 = vfv.chars[5].as_ref().unwrap();
        assert_eq!(steps5.len(), 2);
        assert_eq!((steps5[0].base, steps5[0].ch, steps5[0].dx, steps5[0].dy), (0, 1, 0, 0));
        // advance 0.25em = 163840 + right 20480
        assert_eq!(steps5[1].dx as i64, 163840 + 20480);
        let steps6 = vfv.chars[6].as_ref().unwrap();
        assert_eq!(steps6.len(), 1);
        assert_eq!(steps6[0].dy as i64, 655360, "down4 moves the glyph down");
        let steps7 = vfv.chars[7].as_ref().unwrap();
        assert_eq!(steps7.len(), 2);
        // w1 raw 10486 -> 10486*655360/2^20 = 6553.9sp -> 6554
        assert_eq!(steps7[1].dx as i64, 163840 + 6554);
        let steps200 = vfv.chars[200].as_ref().unwrap();
        assert_eq!(steps200.len(), 1);
        assert_eq!((steps200[0].ch, steps200[0].dx), (1, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-file sanity: the newtx virtual fonts must parse and map onto
    /// their physical base fonts. Skips silently when TeX Live is absent.
    #[test]
    fn vf_newtx_real_files() {
        if !std::path::Path::new("/usr/share/texmf-dist/fonts/vf/public/newtx/ntxsy.vf").exists() {
            return;
        }
        let mut fl = FontLoader::new();
        for (name, base) in [("ntxsy", "txsys"), ("ntxexx", "txexs"), ("ntxmi", "NewTXMI")] {
            let Some(font) = fl.load_tfm(name, 655360) else {
                panic!("{name} tfm missing");
            };
            let vfv = fl
                .vf_fonts
                .get(&(name.to_string(), font.at_size))
                .unwrap_or_else(|| panic!("{name}.vf failed to parse"));
            assert_eq!(font.type1_path, None, "{name} must not carry a pfb");
            assert!(
                vfv.bases.iter().any(|b| b.tfm_name == base),
                "{name}: base {base} missing, got {:?}",
                vfv.bases.iter().map(|b| b.tfm_name.as_str()).collect::<Vec<_>>()
            );
            let n_steps: usize = vfv.chars.iter().flatten().map(|s| s.len()).sum();
            assert!(n_steps > 100, "{name}: too few steps ({n_steps})");
        }
    }
}
