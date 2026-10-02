//! XeTeX native (installed/OpenType) fonts: the font-name syntax and the
//! font option list of `XeTeX_ext.c` (`splitFontName`, `loadOTfont`,
//! `readCommonFeatures`), and the loaded-font record used by the layout code.

use std::rc::Rc;

/// `reqEngine` of XeTeX_ext.c: `/AAT`, `/OT` (`/ICU`), `/GR` request a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReqEngine {
    #[default]
    Default,
    Aat,
    Ot,
    Graphite,
}

impl ReqEngine {
    /// The letter XeTeX keeps in `sReqEngine` (`A`, `O`, `G` or 0).
    pub fn letter(self) -> u8 {
        match self {
            ReqEngine::Default => 0,
            ReqEngine::Aat => b'A',
            ReqEngine::Ot => b'O',
            ReqEngine::Graphite => b'G',
        }
    }
}

/// A loaded native font (`XeTeXLayoutEngine` + the `loaded_font_*` globals).
#[derive(Clone, Debug)]
pub struct NativeFont {
    pub program: Rc<crate::font_program::FontProgram>,
    /// Canonical name (`name_of_file` after `findnativefont`): `font_name[f]`.
    pub full_name: String,
    pub req_engine: ReqEngine,
    /// OpenType script tag (`script=`), 0 when none.
    pub script: u32,
    /// OpenType language tag (`language=`) as four bytes, `None` when absent.
    pub language: Option<[u8; 4]>,
    pub features: Vec<rustybuzz::Feature>,
    pub shapers: Vec<String>,
    pub vertical: bool,
    pub colored: bool,
    /// `0xRRGGBBAA`.
    pub rgba: u32,
    pub extend: f32,
    pub slant: f32,
    /// Already scaled: `embolden * pointsize / 100` (points).
    pub embolden: f32,
    /// `loaded_font_letter_space`, sp.
    pub letter_space: i32,
    pub mapping: Option<Rc<crate::teckit::TextMapping>>,
    /// `loaded_font_design_size`, sp.
    pub design_size: i32,
    /// The point size XeTeXFontInst was created with (`Fix2D(scaled_size)` as f32).
    pub point_size: f32,
    /// Filesystem-style description of where the font came from (tracing).
    pub origin: String,
    /// `height_base` / `depth_base`: ascent and descent (`ot_get_font_metrics`), sp.
    pub height_base: i32,
    pub depth_base: i32,
    /// `\fontdimen1`, `5`, `8`: slant, x-height, cap height (sp).
    pub slant_param: i32,
    pub x_height: i32,
    pub cap_height: i32,
    /// Glyph bounding boxes (`sGlyphBoxes`).
    pub bbox_cache: Rc<std::cell::RefCell<crate::FxHashMap<u16, crate::native_layout::GlyphBBox>>>,
}

impl NativeFont {
    pub fn units_per_em(&self) -> f32 {
        self.program.units_per_em.max(1) as f32
    }

    /// `XeTeXFontInst::unitsToPoints`, in f32 exactly as the C code.
    #[inline]
    pub fn units_to_points(&self, units: f32) -> f32 {
        (units * self.point_size) / self.units_per_em()
    }
}

/// XeTeX's `Fix2D`/`D2Fix`.
#[inline]
pub fn d2fix(d: f64) -> i32 {
    (d * 65536.0 + 0.5) as i32
}

#[inline]
pub fn fix2d(f: i32) -> f64 {
    f as f64 / 65536.0
}

/// Result of `splitFontName` as slices of the name.
#[derive(Debug, PartialEq, Eq)]
pub struct SplitName<'a> {
    /// Name before the variant (for `[path]` names this includes the `[`).
    pub name: &'a str,
    /// Variant string after `/` (without the slash), `None` if absent.
    pub var: Option<&'a str>,
    /// Feature string after `:` (without the colon), `None` if absent.
    pub feat: Option<&'a str>,
    /// Face index from `[path:index]`.
    pub index: u32,
}

/// `splitFontName` + the slicing of `findnativefont`.
pub fn split_font_name(name: &str) -> SplitName<'_> {
    let b = name.as_bytes();
    let mut var: Option<usize> = None;
    let mut feat: Option<usize> = None;
    let mut index: u32 = 0;
    let end;
    if b.first() == Some(&b'[') {
        let mut within = true;
        let mut i = 1;
        while i < b.len() {
            if within && b[i] == b']' {
                within = false;
                if var.is_none() {
                    var = Some(i);
                }
            } else if b[i] == b':' {
                if within && var.is_none() {
                    var = Some(i);
                    i += 1;
                    let mut idx: u32 = 0;
                    while i < b.len() && b[i].is_ascii_digit() {
                        idx = idx.wrapping_mul(10).wrapping_add((b[i] - b'0') as u32);
                        i += 1;
                    }
                    index = idx;
                    i -= 1;
                } else if !within && feat.is_none() {
                    feat = Some(i);
                }
            }
            i += 1;
        }
        end = b.len();
    } else {
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'/' && var.is_none() && feat.is_none() {
                var = Some(i);
            } else if b[i] == b':' && feat.is_none() {
                feat = Some(i);
            }
            i += 1;
        }
        end = b.len();
    }
    let feat_pos = feat.unwrap_or(end);
    let var_pos = var.unwrap_or(feat_pos);
    let slice = |from: usize, to: usize| name.get(from..to).unwrap_or("");
    SplitName {
        name: slice(0, var_pos),
        var: (feat_pos > var_pos).then(|| slice(var_pos + 1, feat_pos)),
        feat: (end > feat_pos).then(|| slice(feat_pos + 1, end)),
        index,
    }
}

/// `hb_tag_from_string(s, len)`: up to four bytes, padded with spaces.
pub fn hb_tag_from_string(s: &[u8]) -> u32 {
    if s.is_empty() {
        return 0;
    }
    let mut t = [b' '; 4];
    for (i, c) in s.iter().take(4).enumerate() {
        t[i] = *c;
    }
    u32::from_be_bytes(t)
}

pub fn tag_bytes(tag: u32) -> [u8; 4] {
    tag.to_be_bytes()
}

/// An option of the feature string that needs a diagnostic or a side effect,
/// in source order.
#[derive(Debug, Clone, PartialEq)]
pub enum FontOptionEvent {
    /// `fontfeaturewarning`: unknown/invalid option text.
    BadOption(String),
    /// `mapping=name` (loaded by the caller).
    Mapping(String),
}

/// The settings `loadOTfont` collects from the feature string.
#[derive(Debug, Clone)]
pub struct ParsedOptions {
    pub script: u32,
    pub language: Option<[u8; 4]>,
    pub features: Vec<rustybuzz::Feature>,
    pub shapers: Vec<String>,
    pub extend: f32,
    pub slant: f32,
    /// percent, unscaled
    pub embolden: f32,
    /// percent, unscaled
    pub letterspace: f32,
    pub colored: bool,
    pub rgba: u32,
    pub vertical: bool,
    pub events: Vec<FontOptionEvent>,
}

impl Default for ParsedOptions {
    fn default() -> Self {
        ParsedOptions {
            script: 0,
            language: None,
            features: Vec::new(),
            shapers: Vec::new(),
            extend: 1.0,
            slant: 0.0,
            embolden: 0.0,
            letterspace: 0.0,
            colored: false,
            rgba: 0x0000_00FF,
            vertical: false,
            events: Vec::new(),
        }
    }
}

/// `read_double`
fn read_double(s: &[u8], pos: &mut usize) -> f64 {
    let mut neg = false;
    let mut val = 0.0f64;
    while *pos < s.len() && (s[*pos] == b' ' || s[*pos] == b'\t') {
        *pos += 1;
    }
    if *pos < s.len() && s[*pos] == b'-' {
        neg = true;
        *pos += 1;
    } else if *pos < s.len() && s[*pos] == b'+' {
        *pos += 1;
    }
    while *pos < s.len() && s[*pos].is_ascii_digit() {
        val = val * 10.0 + (s[*pos] - b'0') as f64;
        *pos += 1;
    }
    if *pos < s.len() && s[*pos] == b'.' {
        let mut dec = 10.0;
        *pos += 1;
        while *pos < s.len() && s[*pos].is_ascii_digit() {
            val += (s[*pos] - b'0') as f64 / dec;
            *pos += 1;
            dec *= 10.0;
        }
    }
    if neg {
        -val
    } else {
        val
    }
}

/// `read_rgb_a`: returns `(rgba, chars consumed)`.
fn read_rgb_a(s: &[u8]) -> (u32, usize) {
    let hex = |c: u8| (c as char).to_digit(16);
    let mut rgb: u32 = 0;
    let mut p = 0;
    for _ in 0..6 {
        match s.get(p).and_then(|c| hex(*c)) {
            Some(d) => rgb = (rgb << 4) + d,
            None => return (0x0000_00FF, p),
        }
        p += 1;
    }
    rgb <<= 8;
    let mut alpha = 0;
    let mut i = 0;
    while i < 2 {
        match s.get(p).and_then(|c| hex(*c)) {
            Some(d) => alpha = (alpha << 4) + d,
            None => break,
        }
        p += 1;
        i += 1;
    }
    if i == 2 {
        rgb += alpha;
    } else {
        rgb += 0xFF;
    }
    (rgb, p)
}

/// `read_tag_with_param` for `+tag=param`
fn read_tag_with_param(s: &[u8]) -> (u32, i32) {
    let mut end = 0;
    while end < s.len() && !matches!(s[end], b':' | b';' | b',' | b'=') {
        end += 1;
    }
    let tag = hb_tag_from_string(&s[..end]);
    let mut param: i32 = 0;
    if s.get(end) == Some(&b'=') {
        let mut p = end + 1;
        let mut neg = false;
        if s.get(p) == Some(&b'-') {
            neg = true;
            p += 1;
        }
        while p < s.len() && s[p].is_ascii_digit() {
            param = param.wrapping_mul(10).wrapping_add((s[p] - b'0') as i32);
            p += 1;
        }
        if neg {
            param = -param;
        }
    }
    (tag, param)
}

fn feature(tag: u32, value: u32) -> rustybuzz::Feature {
    rustybuzz::Feature::new(ttf_parser::Tag(tag), value, ..)
}

/// `readFeatureNumber` (Graphite `id=setting`)
fn read_feature_number(s: &[u8]) -> Option<(u32, u32)> {
    let mut i = 0;
    if !s.first()?.is_ascii_digit() {
        return None;
    }
    let mut f: u32 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        f = f.wrapping_mul(10).wrapping_add((s[i] - b'0') as u32);
        i += 1;
    }
    while i < s.len() && (s[i] == b' ' || s[i] == b'\t') {
        i += 1;
    }
    if s.get(i) != Some(&b'=') {
        return None;
    }
    i += 1;
    if !s.get(i)?.is_ascii_digit() {
        return None;
    }
    let mut v: u32 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add((s[i] - b'0') as u32);
        i += 1;
    }
    while i < s.len() && (s[i] == b' ' || s[i] == b'\t') {
        i += 1;
    }
    (i == s.len()).then_some((f, v))
}

/// Parse the feature string like `loadOTfont` (the part before the engine is
/// created). `req` is the requested renderer.
pub fn parse_font_options(feat: &str, req: ReqEngine) -> ParsedOptions {
    let mut o = ParsedOptions::default();
    let b = feat.as_bytes();
    let mut p = 0;
    if req == ReqEngine::Ot {
        o.shapers.push("ot".into());
    } else if req == ReqEngine::Graphite {
        o.shapers.push("graphite2".into());
    }
    while p < b.len() {
        if matches!(b[p], b':' | b';' | b',') {
            p += 1;
        }
        while p < b.len() && (b[p] == b' ' || b[p] == b'\t') {
            p += 1;
        }
        if p >= b.len() {
            break;
        }
        let cp1 = p;
        let mut cp2 = p;
        while cp2 < b.len() && !matches!(b[cp2], b':' | b';' | b',') {
            cp2 += 1;
        }
        let opt = &b[cp1..cp2];
        let text = |s: &[u8]| String::from_utf8_lossy(s).into_owned();
        let starts = |kw: &str| opt.starts_with(kw.as_bytes());
        // return Err(()) for bad_option
        let r: Result<(), ()> = (|| {
            if starts("script") {
                if opt.get(6) != Some(&b'=') {
                    return Err(());
                }
                o.script = hb_tag_from_string(&opt[7..]);
                return Ok(());
            }
            if starts("language") {
                if opt.get(8) != Some(&b'=') {
                    return Err(());
                }
                let t = &opt[9..];
                o.language = Some(tag_bytes(hb_tag_from_string(t)));
                return Ok(());
            }
            if starts("shaper") {
                if opt.get(6) != Some(&b'=') {
                    return Err(());
                }
                o.shapers.push(text(&opt[7..]));
                return Ok(());
            }
            // readCommonFeatures
            if starts("mapping") {
                if opt.get(7) != Some(&b'=') {
                    return Err(());
                }
                o.events.push(FontOptionEvent::Mapping(text(&opt[8..])));
                return Ok(());
            }
            for (kw, which) in [("extend", 0), ("slant", 1), ("embolden", 2), ("letterspace", 3)] {
                if starts(kw) {
                    if opt.get(kw.len()) != Some(&b'=') {
                        return Err(());
                    }
                    let mut q = kw.len() + 1;
                    let v = read_double(opt, &mut q);
                    match which {
                        0 => o.extend = v as f32,
                        1 => o.slant = v as f32,
                        2 => o.embolden = v as f32,
                        _ => o.letterspace = v as f32,
                    }
                    return Ok(());
                }
            }
            if starts("color") {
                if opt.get(5) != Some(&b'=') {
                    return Err(());
                }
                let (rgba, used) = read_rgb_a(&opt[6..]);
                if used == 6 || used == 8 {
                    o.colored = true;
                    o.rgba = rgba;
                    return Ok(());
                }
                return Err(());
            }
            if req == ReqEngine::Graphite {
                if let Some((t, v)) = read_feature_number(opt) {
                    o.features.push(feature(t, v));
                    return Ok(());
                }
            }
            if opt.first() == Some(&b'+') {
                let (tag, mut param) = read_tag_with_param(&opt[1..]);
                // pre-0.9999 compatibility: feature indices started from 0
                if param >= 0 {
                    param += 1;
                }
                o.features.push(feature(tag, param as u32));
                return Ok(());
            }
            if opt.first() == Some(&b'-') {
                let tag = hb_tag_from_string(&opt[1..]);
                o.features.push(feature(tag, 0));
                return Ok(());
            }
            if starts("vertical") {
                // exactly "vertical" modulo trailing blanks
                let rest = &opt[8..];
                if rest.iter().all(|c| *c == b' ' || *c == b'\t') {
                    o.vertical = true;
                    return Ok(());
                }
            }
            Err(())
        })();
        if r.is_err() {
            o.events.push(FontOptionEvent::BadOption(text(opt)));
        }
        p = cp2;
    }
    o
}

/// Parse `/B /I /BI /S=size /AAT /OT /ICU /GR` variant strings the way
/// `XeTeXFontMgr::findFont` does. Returns `(bold, italic, size override,
/// engine, retained variant string)`.
pub fn parse_variant(variant: &str) -> (bool, bool, Option<f64>, ReqEngine, String) {
    let b = variant.as_bytes();
    let mut cp = 0;
    let mut var = String::new();
    let mut req = ReqEngine::Default;
    let (mut bold, mut ital) = (false, false);
    let mut size = None;
    let push = |var: &mut String, s: &str| {
        if !var.is_empty() && !var.ends_with('/') {
            var.push('/');
        }
        var.push_str(s);
    };
    while cp < b.len() {
        let rest = &b[cp..];
        let mut skip = true;
        if rest.starts_with(b"AAT") {
            req = ReqEngine::Aat;
            cp += 3;
            push(&mut var, "AAT");
        } else if rest.starts_with(b"ICU") {
            req = ReqEngine::Ot;
            cp += 3;
            push(&mut var, "OT");
        } else if rest.starts_with(b"OT") {
            req = ReqEngine::Ot;
            cp += 2;
            push(&mut var, "OT");
        } else if rest.starts_with(b"GR") {
            req = ReqEngine::Graphite;
            cp += 2;
            push(&mut var, "GR");
        } else if rest[0] == b'S' {
            cp += 1;
            if b.get(cp) == Some(&b'=') {
                cp += 1;
            }
            let mut v = 0.0;
            while cp < b.len() && b[cp].is_ascii_digit() {
                v = v * 10.0 + (b[cp] - b'0') as f64;
                cp += 1;
            }
            if b.get(cp) == Some(&b'.') {
                let mut dec = 1.0;
                cp += 1;
                while cp < b.len() && b[cp].is_ascii_digit() {
                    dec *= 10.0;
                    v += (b[cp] - b'0') as f64 / dec;
                    cp += 1;
                }
            }
            size = Some(v);
        } else {
            skip = false;
            loop {
                match b.get(cp) {
                    Some(b'B') => {
                        bold = true;
                        cp += 1;
                    }
                    Some(b'I') => {
                        ital = true;
                        cp += 1;
                    }
                    _ => break,
                }
            }
        }
        let _ = skip;
        while cp < b.len() && b[cp] != b'/' {
            cp += 1;
        }
        if cp < b.len() && b[cp] == b'/' {
            cp += 1;
        }
    }
    (bold, ital, size, req, var)
}

/// `hb_ot_tag_to_script`-like conversion of an OpenType script tag.
pub fn ot_tag_to_script(tag: u32) -> Option<rustybuzz::Script> {
    if tag == 0 {
        return None;
    }
    let mut t = tag_bytes(tag);
    // new-style Indic tags ('dev2' -> 'deva')
    match &t {
        b"bng2" => t = *b"beng",
        b"dev2" => t = *b"deva",
        b"gjr2" => t = *b"gujr",
        b"gur2" => t = *b"guru",
        b"knd2" => t = *b"knda",
        b"mlm2" => t = *b"mlym",
        b"ory2" => t = *b"orya",
        b"tml2" => t = *b"taml",
        b"tel2" => t = *b"telu",
        b"mym2" => t = *b"mymr",
        b"DFLT" => return None,
        _ => {}
    }
    // spaces at the end are replaced by repeating the last letter ('nko ' -> 'Nkoo')
    let mut last = t[0];
    for c in t.iter_mut() {
        if *c == b' ' || *c == 0 {
            *c = last;
        } else {
            last = *c;
        }
    }
    rustybuzz::Script::from_iso15924_tag(ttf_parser::Tag::from_bytes(&t))
}

/// `hb_ot_tag_to_language` for tags without a BCP-47 table entry: the
/// `x-hbot` private-use form, which selects exactly that OT language system.
pub fn ot_tag_to_language(tag: [u8; 4]) -> Option<rustybuzz::Language> {
    use std::str::FromStr;
    if tag == [0; 4] {
        return None;
    }
    let s: String = tag.iter().map(|c| (*c as char).to_ascii_lowercase()).collect();
    rustybuzz::Language::from_str(&format!("x-hbot{s}")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_plain_name() {
        let s = split_font_name("Latin Modern Roman/I:+liga;-kern");
        assert_eq!(s.name, "Latin Modern Roman");
        assert_eq!(s.var, Some("I"));
        assert_eq!(s.feat, Some("+liga;-kern"));
        assert_eq!(split_font_name("Foo").feat, None);
    }

    #[test]
    fn split_bracket_name() {
        let s = split_font_name("[fonts/a.ttc:2]:color=FF0000;slant=0.2");
        assert_eq!(s.name, "[fonts/a.ttc");
        assert_eq!(s.index, 2);
        assert_eq!(s.feat, Some("color=FF0000;slant=0.2"));
        let s = split_font_name("[a.otf]");
        assert_eq!(s.name, "[a.otf");
        assert_eq!(s.var, None);
    }

    #[test]
    fn options() {
        let o = parse_font_options("+liga;-kern;smcp;script=latn;color=ff000080;extend=1.5;vertical", ReqEngine::Default);
        assert_eq!(o.features.len(), 2);
        assert_eq!(o.features[0].value, 1);
        assert!(o.vertical && o.colored);
        assert_eq!(o.rgba, 0xff000080);
        assert_eq!(o.events, vec![FontOptionEvent::BadOption("smcp".into())]);
    }
}
