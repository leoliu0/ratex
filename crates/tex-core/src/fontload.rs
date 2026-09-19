use crate::engine::Engine;
use crate::eqtb::Equiv;
use crate::tfm::{parse_tfm, Font};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackedFont {
    pub base_font: u16,
    pub tracking: i32,
}

pub struct FontLoader {
    pub kpse: tex_kpse::Kpse,
    pub map: crate::fontmap::FontMap,
    pub tfm_cache: crate::FxHashMap<(String, i32), Rc<Font>>,
    pub enc_cache: crate::FxHashMap<String, Rc<Vec<String>>>,
    /// virtual fonts by (tfm name, resolved at size)
    pub vf_fonts: crate::FxHashMap<(String, i32), Rc<VfFont>>,
    /// engine font id of a VF-backed font -> base engine font ids
    /// (u16::MAX = the base TFM was missing at load time)
    pub vf_bases: crate::FxHashMap<u16, Vec<u16>>,
    /// Tracked fonts created by \letterspacefont: derived font id -> TrackedFont
    pub tracked_fonts: crate::FxHashMap<u16, TrackedFont>,
    /// pdftex.map is loaded on first font lookup, not at construction:
    /// the find forces kpse database setup, which is pure startup waste
    /// for format-booted runs that never select a mapped font.
    map_loaded: bool,
    /// External font resources consulted by this job. The CLI folds these
    /// into its dependency cache alongside TeX inputs.
    pub dependency_files: Vec<std::path::PathBuf>,
    /// Unindexed search directories whose entries governed a lookup result.
    pub dependency_directories: Vec<(std::path::PathBuf, u64)>,
    /// Content identities captured when font resources were read, before TeX
    /// can rewrite those paths later in the same pass.
    pub dependency_file_digests: Vec<(std::path::PathBuf, u64, u64)>,
    /// Concrete negative probes that could shadow an embedded font resource
    /// if a same-named external file appears later.
    pub dependency_missing_files: Vec<std::path::PathBuf>,
    /// Search roots or subtrees which were not directories during lookup.
    /// Their later creation can expose files at otherwise unenumerable paths.
    pub dependency_missing_directories: Vec<std::path::PathBuf>,
    /// False when an I/O error prevented a complete dependency snapshot.
    pub dependency_tracking_complete: bool,
}

fn dependency_content_hash(bytes: &[u8]) -> u64 {
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x9e37_79b9_7f4a_7c15;
    for (index, byte) in bytes.iter().enumerate() {
        h1 ^= u64::from(*byte);
        h1 = h1.wrapping_mul(0x1000_0000_01b3);
        h2 = (h2 + u64::from(*byte) + index as u64).wrapping_mul(0x1000_0000_01b3);
    }
    h1 ^ h2
}

impl FontLoader {
    pub fn new() -> Self {
        Self::with_kpse(tex_kpse::Kpse::new())
    }

    pub fn with_kpse(kpse: tex_kpse::Kpse) -> Self {
        FontLoader {
            kpse,
            map: crate::fontmap::FontMap::default(),
            tfm_cache: crate::FxHashMap::default(),
            enc_cache: crate::FxHashMap::default(),
            vf_fonts: crate::FxHashMap::default(),
            vf_bases: crate::FxHashMap::default(),
            tracked_fonts: crate::FxHashMap::default(),
            map_loaded: false,
            dependency_files: Vec::new(),
            dependency_directories: Vec::new(),
            dependency_file_digests: Vec::new(),
            dependency_missing_files: Vec::new(),
            dependency_missing_directories: Vec::new(),
            dependency_tracking_complete: true,
        }
    }

    pub fn record_negative_dependency(&mut self, name: &str, format: tex_kpse::Format) {
        let (present, directories, content, missing, missing_directories, complete) =
            self.kpse.negative_lookup_dependencies(name, format);
        self.dependency_files.extend(present);
        self.dependency_directories.extend(directories);
        self.dependency_file_digests.extend(content);
        self.dependency_missing_files.extend(missing);
        self.dependency_missing_directories
            .extend(missing_directories);
        self.dependency_tracking_complete &= complete;
    }

    pub fn record_lookup_dependency(
        &mut self,
        name: &str,
        format: tex_kpse::Format,
        selected: Option<&std::path::Path>,
    ) {
        let (present, directories, content, missing, missing_directories, complete) =
            self.kpse.lookup_dependencies(name, format, selected);
        self.dependency_files.extend(present);
        self.dependency_directories.extend(directories);
        self.dependency_file_digests.extend(content);
        self.dependency_missing_files.extend(missing);
        self.dependency_missing_directories
            .extend(missing_directories);
        self.dependency_tracking_complete &= complete;
    }

    pub fn is_tracked_font(&self, f: u16) -> bool {
        self.tracked_fonts.contains_key(&f)
    }

    pub fn get_tracked_font(&self, f: u16) -> Option<&TrackedFont> {
        self.tracked_fonts.get(&f)
    }

    pub fn register_tracked_font(&mut self, derived: u16, base: u16, tracking: i32) {
        self.tracked_fonts.insert(
            derived,
            TrackedFont {
                base_font: base,
                tracking,
            },
        );
    }

    /// Remove the positive file dependency added by a metadata-only lookup,
    /// while retaining its earlier search-path, index, and directory state.
    pub fn discard_file_dependency_since(&mut self, start: usize, selected: &std::path::Path) {
        let same_path = |path: &std::path::Path| {
            path == selected
                || tex_kpse::fs::canonicalize(path)
                    .ok()
                    .zip(tex_kpse::fs::canonicalize(selected).ok())
                    .is_some_and(|(left, right)| left == right)
        };
        if let Some(offset) = self.dependency_files[start..]
            .iter()
            .position(|path| same_path(path))
        {
            self.dependency_files.remove(start + offset);
        }
    }

    fn read_dependency(&mut self, name: &str, format: tex_kpse::Format) -> Option<Vec<u8>> {
        let resolved = self.kpse.find(name, format);
        self.record_lookup_dependency(name, format, resolved.as_deref());
        if let Some(path) = resolved {
            if let Ok(data) = tex_kpse::fs::read(&path) {
                self.dependency_file_digests.push((
                    path.clone(),
                    data.len() as u64,
                    dependency_content_hash(&data),
                ));
                self.dependency_files.push(path);
                return Some(data);
            }
        }
        // Embedded resources are part of the executable identity and need
        // no separate disk dependency.
        self.kpse.read(name, format)
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
        let Some(data) = self.read_dependency(name, tex_kpse::Format::Map) else {
            return;
        };
        self.map
            .extend_file(String::from_utf8_lossy(&data).into_owned());
    }

    pub fn load_tfm(&mut self, name: &str, at: i32) -> Option<Rc<Font>> {
        self.ensure_map();
        let key = (name.to_string(), at);
        if let Some(f) = self.tfm_cache.get(&key) {
            return Some(f.clone());
        }
        let (resolved_name, data) = if let Some(d) = self.read_dependency(name, tex_kpse::Format::Tfm) {
            let n = name.strip_suffix(".tfm").unwrap_or(name);
            (n, d)
        } else if let Some((stem, _)) = name.split_once('.') {
            // tex.web §1257 / §526: scan_file_name splits extensions at the first dot,
            // but read_font_info only receives cur_name. When LaTeX NFSS specifies
            // `cmr6.5` or `cmr10.0` or `cmr10.tfm`, tex.web strips the extension and
            // loads `cmr6.tfm` / `cmr10.tfm`.
            if !stem.is_empty() {
                let d = self.read_dependency(stem, tex_kpse::Format::Tfm)?;
                (stem, d)
            } else {
                return None;
            }
        } else {
            return None;
        };
        let mut font = parse_tfm(&data, resolved_name, at).ok()?;
        let map_entry = self.map.get(resolved_name);
        if let Some(me) = &map_entry {
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
        let pfb = map_entry.as_ref().and_then(|m| m.pfb.clone());
        let has_pfb = match &pfb {
            Some(p) if !p.ends_with(".vf") => {
                let resolved = self.kpse.find(p, tex_kpse::Format::Type1);
                self.record_lookup_dependency(p, tex_kpse::Format::Type1, resolved.as_deref());
                if let Some(path) = resolved {
                    self.dependency_files.push(path);
                    true
                } else if tex_kpse::has_embedded_package(p) {
                    true
                } else {
                    // A newly installed PFB would switch this font away from
                    // its virtual-font fallback, but kpse cannot expose every
                    // directory it negatively probed. Avoid a stale fast hit.
                    self.dependency_tracking_complete = false;
                    false
                }
            }
            None => false,
            Some(_) => false,
        };
        if !has_pfb {
            if let Some(vf) = self
                .read_dependency(resolved_name, tex_kpse::Format::Vf)
                .and_then(|d| self.parse_vf(&d, font.at_size))
            {
                font.map_fontname = None;
                font.type1_path = None;
                font.enc_name = None;
                font.encoding = None;
                self.vf_fonts.insert((name.to_string(), font.at_size), vf.clone());
                self.vf_fonts.insert((resolved_name.to_string(), font.at_size), vf);
            }
        }
        let rc = Rc::new(font);
        self.tfm_cache.insert(key, rc.clone());
        if resolved_name != name {
            self.tfm_cache.insert((resolved_name.to_string(), at), rc.clone());
        }
        Some(rc)
    }

    pub fn load_enc(&mut self, name: &str) -> Option<Rc<Vec<String>>> {
        if let Some(e) = self.enc_cache.get(name) {
            return Some(e.clone());
        }
        let data = self.read_dependency(name, tex_kpse::Format::Enc)?;
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
fn split_map_tokens(
    line: &str,
) -> (
    smallvec::SmallVec<[std::borrow::Cow<'_, str>; 8]>,
    smallvec::SmallVec<[std::borrow::Cow<'_, str>; 2]>,
) {
    use std::borrow::Cow;
    let mut bare = smallvec::SmallVec::new();
    let mut quoted = smallvec::SmallVec::new();
    let mut sections = line.split('"').enumerate().peekable();
    while let Some((index, section)) = sections.next() {
        if index % 2 == 0 {
            // A quote joined to an existing bare word uses the historical
            // concatenation rules below. Normal map entries borrow slices.
            if sections.peek().is_some()
                && section
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace())
            {
                let (bare, quoted) = split_map_tokens_joined(line);
                return (
                    bare.into_iter().map(Cow::Owned).collect(),
                    quoted.into_iter().map(Cow::Owned).collect(),
                );
            }
            bare.extend(section.split_whitespace().map(Cow::Borrowed));
        } else if !section.is_empty() || sections.peek().is_some() {
            quoted.push(Cow::Borrowed(section));
        }
    }
    (bare, quoted)
}

fn split_map_tokens_joined(line: &str) -> (Vec<String>, Vec<String>) {
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
    let tfm = it.next()?.into_owned();
    let fontname = it.next()?.into_owned();
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
        let mut prev: Option<&str> = None;
        for w in section.split_whitespace() {
            let (word, fused) = match (w.strip_suffix("SlantFont"), w.strip_suffix("ExtendFont")) {
                (Some(v), _) => (v, 1),
                (_, Some(v)) => (v, 2),
                (None, None) => ("", 0),
            };
            match fused {
                1 => {
                    slant = if word.is_empty() {
                        prev.as_deref()
                            .and_then(|p| p.parse().ok())
                            .unwrap_or(slant)
                    } else {
                        word.parse().unwrap_or(slant)
                    };
                    prev = None;
                }
                2 => {
                    extend = if word.is_empty() {
                        prev.as_deref()
                            .and_then(|p| p.parse().ok())
                            .unwrap_or(extend)
                    } else {
                        word.parse().unwrap_or(extend)
                    };
                    prev = None;
                }
                _ => {
                    if w == "ReEncodeFont" {
                        if let Some(p) = prev.take().filter(|p| p.parse::<f64>().is_err()) {
                            enc_name = Some(p.to_string());
                        }
                    } else {
                        prev = Some(w);
                    }
                }
            }
        }
    }
    Some(MapEntry {
        tfm,
        fontname,
        enc_file,
        enc_name,
        pfb,
        slant,
        extend,
    })
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
                    let at_base = ((s as i64 * at as i64 + if s >= 0 { 0x80000 } else { -0x80000 })
                        >> 20) as i32;
                    let loaded = if at_base > 0 {
                        self.load_tfm(&name, at_base)
                    } else {
                        None
                    };
                    if id <= u8::MAX as u32 {
                        base_idx.insert(id, bases.len() as u8);
                    }
                    bases.push(loaded);
                    base_specs.push(VfBase {
                        tfm_name: name,
                        at_size: at_base,
                    });
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
                248 => break,     // post: end of packets
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
                // bop/eop are not legal inside a virtual-font character
                // packet. Reject malformed input instead of panicking.
                139 | 140 => return None,
                _ => return None, // ops >= 242 break out above
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
                steps.push(VfStep {
                    base: bi,
                    ch,
                    dx: x as i32,
                    dy: y as i32,
                });
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
            bchar: None,
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
        self.eqtb.expand.push(Default::default());
        self.eqtb.assign(cs, crate::eqtb::Equiv::FontRef(0), true);
    }

    pub fn do_font(&mut self) {
        // \font<cs>=<tfm> [at <dimen>|scaled <int>]
        let declaration_source = self
            .current_token_source_mark()
            .as_ref()
            .map(crate::input::SourceMark::to_context);
        // Prefix state belongs to this assignment even when scanning or
        // loading the font fails. Retaining it would make the next unrelated
        // assignment global (or long/outer/protected).
        let global = self.take_global();
        self.clear_prefixes();
        let cs = self.scan_definable_cs();
        self.scan_optional_equals();
        let Some(name) = self.scan_font_name(declaration_source.as_ref()) else {
            return;
        };
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
            at = crate::scaled::xn_over_d(dsize, s, 1000);
        }
        self.load_font_and_bind(&name, at, cs, declaration_source, global);
    }

    /// tex.web-style file-name scan for \font: letter/other chars accumulate,
    /// a space terminates the name (and is absorbed), anything else is pushed
    /// back. Uses the non-space-skipping fetch so "cmr10 at 10pt" splits.
    fn scan_font_name(
        &mut self,
        declaration_source: Option<&crate::input::SourceContext>,
    ) -> Option<String> {
        let mut name = Vec::new();
        self.skip_spaces_relax();
        let first = self.get_x_raw();
        if first == crate::input::EOF_MARKER {
            return Some(String::new());
        }
        if first.is_char() && first.chr() == b'"' as u32 {
            // Quoted font name: \font\f="[FontFile.otf]:features" or "Font Name"
            loop {
                let t = self.get_x_raw();
                if t == crate::input::EOF_MARKER {
                    self.fatal_error_at(
                        "File ended while scanning a quoted file name for \\font; add the closing quote",
                        declaration_source.cloned(),
                    );
                    return None;
                }
                if t.is_char() && t.chr() == b'"' as u32 {
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
            return Some(String::from_utf8_lossy(&name).to_string());
        }
        self.push_token(first);
        loop {
            let t = self.get_x_raw();
            if t == crate::input::EOF_MARKER {
                break;
            }
            if t.is_cs() {
                self.push_token(t);
                break;
            }
            match t.cc() {
                11 | 12 => name.push(t.chr() as u8),
                10 => break, // space: name complete
                _ => {
                    self.push_token(t);
                    break;
                }
            }
            if name.len() > 256 {
                break;
            }
        }
        Some(String::from_utf8_lossy(&name).to_string())
    }
    pub fn load_font_and_bind(
        &mut self,
        name: &str,
        at: i32,
        cs: crate::token::CsId,
        declaration_source: Option<crate::input::SourceContext>,
        global: bool,
    ) {
        // If name refers to an OpenType / TrueType font (e.g. "[path/to/font.otf]" or ends with .otf/.ttf/.ttc):
        let clean_name = name.trim_start_matches('[').trim_end_matches(']');
        let is_otf = clean_name.ends_with(".otf")
            || clean_name.ends_with(".ttf")
            || clean_name.ends_with(".ttc")
            || clean_name.starts_with('/');
        if is_otf {
            match tex_kpse::fs::read(clean_name) {
                Ok(data) => match ttf_parser::Face::parse(&data, 0) {
                    Ok(face) => {
                        self.record_loaded_bytes(std::path::Path::new(clean_name), &data);
                        self.loaded_files.push(std::path::PathBuf::from(clean_name));
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
                            bchar: None,
                            type1_path: None,
                            enc_name: None,
                            map_fontname: Some(clean_name.to_string()),
                            encoding: None,
                        };
                        // Populate default ASCII characters from OpenType metrics
                        for c in 0..=255u8 {
                            let w = face
                                .glyph_index(c as char)
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
                        self.eqtb.assign(cs, Equiv::FontRef(id), global);
                        let message = format!(
                            "{} (OpenType) at {}\n",
                            clean_name,
                            self.scaled_to_string(at_size)
                        );
                        self.append_term(&message);
                        self.append_log(&message);
                        return;
                    }
                    Err(error) => {
                        self.error_at(
                            &format!(
                                "Cannot load OpenType font file `{clean_name}` for \\{}: {error}",
                                String::from_utf8_lossy(self.cs.name(cs))
                            ),
                            declaration_source,
                        );
                        return;
                    }
                },
                Err(error) => {
                    self.error_at(
                        &format!(
                            "Cannot read font file `{clean_name}` for \\{}: {error}",
                            String::from_utf8_lossy(self.cs.name(cs))
                        ),
                        declaration_source,
                    );
                    return;
                }
            }
        }
        let Some(font) = self.font_loader.load_tfm(name, at) else {
            self.error_at(
                &format!(
                    "Font \\{}={} not found",
                    String::from_utf8_lossy(self.cs.name(cs)),
                    name
                ),
                declaration_source,
            );
            return;
        };
        let at_size = font.at_size;
        // tex.web §1258 / pdftex.web §1444: if this font has already been
        // loaded with the same TFM name and size, reuse its internal font number.
        for (k, existing) in self.eqtb.fonts.iter().enumerate().skip(1) {
            if existing.tfm_name == font.tfm_name && existing.at_size == at_size {
                self.eqtb.font_cs[k] = cs;
                self.eqtb.assign(cs, Equiv::FontRef(k as u16), global);
                return;
            }
        }
        let id = self.push_engine_font(font, cs);
        self.eqtb.assign(cs, Equiv::FontRef(id), global);
        let message = format!(
            "{} at {}\n",
            name,
            self.scaled_to_string(self.eqtb.fonts[id as usize].at_size)
        );
        self.append_term(&message);
        self.append_log(&message);
        // A VF-backed font gets its base fonts registered as engine fonts
        // (without control-sequence bindings) so glyph emission can address
        // them directly.
        if let Some(vf) = self
            .font_loader
            .vf_fonts
            .get(&(name.to_string(), at_size))
            .cloned()
        {
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

    /// append a font to the engine font tables; returns its font id.
    /// pdfTeX's `read_font_info` allocates a fresh internal font record per
    /// `\font` load (char tables are NOT shared between two ids of the same
    /// tfm), so `tagcode`/`pdfnoligatures` on one identifier must not leak
    /// into the other; deep-clone the cached `Rc<Font>` to mirror that.
    pub fn push_engine_font(&mut self, font: Rc<Font>, cs: crate::token::CsId) -> u16 {
        let font = Rc::new((*font).clone());
        let id = self.eqtb.fonts.len() as u16;
        let params = font.params.clone();
        self.eqtb.fonts.push(font);
        self.eqtb.font_params.push(params);
        self.eqtb
            .font_param_levels
            .push(vec![1; self.eqtb.font_params[id as usize].len()]);
        self.eqtb.hyphen_char.push(b'-' as i32);
        self.eqtb.hyphen_char_levels.push(1);
        self.eqtb.skew_char.push(-1);
        self.eqtb.skew_char_levels.push(1);
        self.eqtb.font_cs.push(cs);
        self.eqtb.expand.push(Default::default());
        id
    }

    pub fn scan_pdf_origin(&mut self) -> u8 {
        if self.scan_keyword(b"direct") {
            1
        } else if self.scan_keyword(b"page") {
            2
        } else if self.scan_keyword(b"origin") {
            0
        } else {
            0
        }
    }

    pub fn scan_pdf_string(&mut self) -> String {
        let toks = self.scan_general_text_expanded();
        self.write_tokens_to_string(&toks)
    }

    // ---------- pdfTeX font expansion (pdftex.web §17399+) ----------

    /// `expand_font_name`: font_name[f] + ("+" if e>0) + print_int(e)
    pub fn expand_font_name(&self, f: u16, e: i32) -> String {
        let name = &self.eqtb.fonts[f as usize].tfm_name;
        if e > 0 {
            format!("{}+{}", name, e)
        } else {
            format!("{}{}", name, e)
        }
    }

    /// `tfm_lookup`: an already-loaded font with this tfm name at this size
    /// (skips nullfont).
    fn tfm_lookup(&self, s: &str, fs: i32) -> u16 {
        for (k, fnt) in self.eqtb.fonts.iter().enumerate().skip(1) {
            if fnt.tfm_name == s && (fs == 0 || fnt.at_size == fs) {
                return k as u16;
            }
        }
        0
    }

    /// `auto_expand_font` + `auto_expand_vf` (pdftex.web §18311): synthesize
    /// an in-memory expanded clone — widths, italic corrections and kerns
    /// scaled by (1000+e)/1000 with pdfTeX's exact rounding; heights/depths/
    /// ligature programs unchanged. A virtual font clone reuses the base's
    /// character packets unchanged (`vf_packet_base[f] := vf_packet_base[bf]`)
    /// but maps every local base font to its own expanded clone, so the
    /// renderer sees clone ids whose ratio/blink are set canonically.
    fn auto_expand_font(&mut self, f: u16, e: i32) -> u16 {
        let mut nf = (*self.eqtb.fonts[f as usize].clone()).clone();
        nf.tfm_name = self.expand_font_name(f, e);
        let n = 1000 + e;
        for ci in nf.chars.iter_mut() {
            ci.width = crate::tfm::round_xn_over_d(ci.width, n, 1000);
            ci.italic = crate::tfm::round_xn_over_d(ci.italic, n, 1000);
        }
        for kk in nf.kerns.iter_mut() {
            *kk = crate::tfm::round_xn_over_d(*kk, n, 1000);
        }
        let k = self.push_engine_font(Rc::new(nf), 0);
        self.copy_expand_params(k, f, e);
        if let Some(bases) = self.font_loader.vf_bases.get(&f).cloned() {
            let mut expanded = Vec::with_capacity(bases.len());
            for &lf in &bases {
                if lf == 0 || lf == u16::MAX || lf as usize >= self.eqtb.fonts.len() {
                    expanded.push(lf);
                } else {
                    expanded.push(self.auto_expand_font(lf, e));
                }
            }
            self.font_loader.vf_bases.insert(k, expanded);
        }
        k
    }

    /// `copy_expand_params`: variant inherits step/auto/blink and aliases
    /// the base font's code tables.
    fn copy_expand_params(&mut self, k: u16, f: u16, e: i32) {
        let fx = &self.eqtb.expand[f as usize];
        let (step, auto) = (fx.step, fx.auto_expand);
        let tables = (
            fx.ef.clone(),
            fx.lp.clone(),
            fx.rp.clone(),
            fx.kn_bs.clone(),
            fx.st_bs.clone(),
            fx.sh_bs.clone(),
            fx.kn_bc.clone(),
            fx.kn_ac.clone(),
        );
        let ex = &mut self.eqtb.expand[k as usize];
        ex.ratio = e;
        ex.step = step;
        ex.auto_expand = auto;
        ex.blink = f;
        ex.set_shared_tables(tables);
    }
    fn copy_letterspace_params(&mut self, k: u16, f: u16) {
        let fx = &self.eqtb.expand[f as usize];
        let (step, auto, stretch, shrink) = (fx.step, fx.auto_expand, fx.stretch, fx.shrink);
        let tables = (
            fx.ef.clone(),
            fx.lp.clone(),
            fx.rp.clone(),
            fx.kn_bs.clone(),
            fx.st_bs.clone(),
            fx.sh_bs.clone(),
            fx.kn_bc.clone(),
            fx.kn_ac.clone(),
        );
        let ex = &mut self.eqtb.expand[k as usize];
        ex.step = step;
        ex.auto_expand = auto;
        ex.stretch = stretch;
        ex.shrink = shrink;
        ex.set_shared_tables(tables);
    }

    /// `load_expand_font`: find or create the variant of `f` expanded by
    /// `e` thousandths (e nonzero, a multiple of step).
    fn load_expand_font(&mut self, f: u16, e: i32) -> u16 {
        let s = self.expand_font_name(f, e);
        let at = self.eqtb.fonts[f as usize].at_size;
        let mut k = self.tfm_lookup(&s, at);
        if k == 0 {
            let auto = self.eqtb.expand[f as usize].auto_expand;
            if auto {
                k = self.auto_expand_font(f, e);
            } else {
                // non-auto: pdfTeX reads the expanded TFM file (cmr10+20.tfm
                // from the distribution); a missing file is a real font error
                match self.font_loader.load_tfm(&s, at) {
                    Some(rc) => {
                        k = self.push_engine_font(rc, 0);
                        self.copy_expand_params(k, f, e);
                    }
                    None => {
                        self.error(&format!("Font {} not found", s));
                        return f;
                    }
                }
            }
        } else {
            self.copy_expand_params(k, f, e);
        }
        k
    }

    /// `get_expand_font`: walk the elink chain, else load; splice in.
    fn get_expand_font(&mut self, f: u16, e: i32) -> u16 {
        let mut k = self.eqtb.expand[f as usize].elink;
        while k != 0 {
            if self.eqtb.expand[k as usize].ratio == e {
                return k;
            }
            k = self.eqtb.expand[k as usize].elink;
        }
        k = self.load_expand_font(f, e);
        if k != f && k != 0 {
            let old = self.eqtb.expand[f as usize].elink;
            self.eqtb.expand[k as usize].elink = old;
            self.eqtb.expand[f as usize].elink = k;
        }
        k
    }

    /// `fix_expand_value`: clamp to the configured limits and snap to the
    /// nearest multiple of the step.
    fn fix_expand_value(&self, f: u16, e: i32) -> i32 {
        if e == 0 {
            return 0;
        }
        let ex = &self.eqtb.expand[f as usize];
        let (mut e, neg) = if e < 0 { (-e, true) } else { (e, false) };
        let max_expand = if neg {
            if ex.shrink != 0 {
                -self
                    .eqtb
                    .expand
                    .get(ex.shrink as usize)
                    .map_or(0, |x| x.ratio)
            } else {
                0
            }
        } else if ex.stretch != 0 {
            self.eqtb
                .expand
                .get(ex.stretch as usize)
                .map_or(0, |x| x.ratio)
        } else {
            0
        };
        if e > max_expand {
            e = max_expand;
        } else if ex.step > 0 && e % ex.step > 0 {
            e = ex.step * crate::tfm::round_xn_over_d(e, 1, ex.step);
        }
        if neg {
            -e
        } else {
            e
        }
    }

    /// `expand_font`: nearest configured variant for arbitrary ratio `e`.
    pub fn expand_font(&mut self, f: u16, e: i32) -> u16 {
        if e == 0 {
            return f;
        }
        let e = self.fix_expand_value(f, e);
        if e == 0 {
            return f;
        }
        if self.eqtb.expand[f as usize].elink == 0 {
            return f;
        }
        self.get_expand_font(f, e)
    }

    /// `set_expand_params`
    fn set_expand_params(
        &mut self,
        f: u16,
        auto_expand: bool,
        stretch_limit: i32,
        shrink_limit: i32,
        font_step: i32,
        expand_ratio: i32,
    ) {
        self.eqtb.expand[f as usize].step = font_step;
        self.eqtb.expand[f as usize].auto_expand = auto_expand;
        if stretch_limit > 0 {
            let k = self.get_expand_font(f, stretch_limit);
            self.eqtb.expand[f as usize].stretch = k;
        }
        if shrink_limit > 0 {
            let k = self.get_expand_font(f, -shrink_limit);
            self.eqtb.expand[f as usize].shrink = k;
        }
        if expand_ratio != 0 {
            self.eqtb.expand[f as usize].ratio = expand_ratio;
        }
    }

    /// `vf_expand_local_fonts`: propagate expansion to a VF's base fonts
    pub fn vf_expand_local_fonts(&mut self, f: u16) {
        let bases: Vec<u16> = match self.font_loader.vf_bases.get(&f) {
            Some(b) => b.clone(),
            None => return,
        };
        let fx = &self.eqtb.expand[f as usize];
        let (auto, step, ratio) = (fx.auto_expand, fx.step, fx.ratio);
        let (st, sh) = (fx.stretch, fx.shrink);
        let sl = if st != 0 {
            self.eqtb.expand.get(st as usize).map_or(0, |x| x.ratio)
        } else {
            0
        };
        let shl = if sh != 0 {
            -self.eqtb.expand.get(sh as usize).map_or(0, |x| x.ratio)
        } else {
            0
        };
        for lf in bases {
            if lf == 0 || lf == u16::MAX || lf as usize >= self.eqtb.expand.len() {
                continue;
            }
            self.set_expand_params(lf, auto, sl, shl, step, ratio);
            if self.font_loader.vf_bases.contains_key(&lf) {
                self.vf_expand_local_fonts(lf);
            }
        }
    }

    /// `\pdffontexpand <font> = <stretch> <shrink> <step> [autoexpand]`
    /// (pdftex.web `read_expand_font`).
    pub fn do_pdffontexpand(&mut self) {
        // This primitive mutates font state directly, but still consumes any
        // assignment prefixes attached to it.
        self.take_global();
        self.clear_prefixes();
        self.skip_spaces_relax();
        let f = self.scan_font_id();
        if f == 0 {
            self.error("font expansion: invalid font identifier");
            return;
        }
        if self.eqtb.expand[f as usize].blink != 0 {
            self.error("font expansion: \\pdffontexpand cannot be used this way (the base font has been expanded)");
            return;
        }
        self.scan_optional_equals();
        let mut stretch_limit = self.scan_int().clamp(0, 1000);
        let mut shrink_limit = self.scan_int().clamp(0, 500);
        let font_step = self.scan_int().clamp(0, 100);
        if font_step == 0 {
            self.error("font expansion: invalid step");
            return;
        }
        stretch_limit -= stretch_limit % font_step;
        shrink_limit -= shrink_limit % font_step;
        if stretch_limit == 0 && shrink_limit == 0 {
            self.error("font expansion: invalid limit(s)");
            return;
        }
        let auto_expand = self.scan_keyword(b"autoexpand");
        if self.eqtb.expand[f as usize].ratio != 0 {
            self.error("font expansion: this font has been expanded by another font so it cannot be used now");
            return;
        }
        let ex = &self.eqtb.expand[f as usize];
        if ex.step != 0 {
            // re-configuration must be consistent with the first one
            let (st, sh) = (ex.stretch, ex.shrink);
            let (step, auto, ratio) = (ex.step, ex.auto_expand, ex.ratio);
            let stl = if st != 0 {
                self.eqtb.expand[st as usize].ratio
            } else {
                0
            };
            let shl = if sh != 0 {
                -self.eqtb.expand[sh as usize].ratio
            } else {
                0
            };
            if step != font_step {
                self.error("font expansion: font has been expanded with different expansion step");
                return;
            }
            if (st == 0) != (stretch_limit == 0) || (st != 0 && stl != stretch_limit) {
                self.error("font expansion: font has been expanded with different stretch limit");
                return;
            }
            if (sh == 0) != (shrink_limit == 0) || (sh != 0 && shl != shrink_limit) {
                self.error("font expansion: font has been expanded with different shrink limit");
                return;
            }
            if auto != auto_expand {
                self.error(
                    "font expansion: font has been expanded with different auto expansion value",
                );
                return;
            }
            let _ = ratio;
        } else {
            self.set_expand_params(f, auto_expand, stretch_limit, shrink_limit, font_step, 0);
            if self.font_loader.vf_bases.contains_key(&f) {
                self.vf_expand_local_fonts(f);
            }
        }
    }

    /// `set_no_ligatures` (\pdfnoligatures\f): strip ligature tags from
    /// every existing character of the font.
    pub fn set_no_ligatures(&mut self, f: u16) {
        let font = Rc::clone(&self.eqtb.fonts[f as usize]);
        // our parser shares Rc<Font> only through push_engine_font's clone,
        // but a font record can back one engine id; mutate through a clone
        // and replace the Rc so no other holder is affected unexpectedly.
        let mut nf = (*font).clone();
        let (bc, ec) = (nf.bc, nf.ec);
        for c in bc..=ec {
            if nf.char_present(c) && nf.chars[c as usize].tag == crate::tfm::TAG_LIG {
                nf.chars[c as usize].tag = crate::tfm::TAG_NO_TAG;
            }
        }
        self.eqtb.fonts[f as usize] = Rc::new(nf);
    }

    /// `test_no_ligatures` (\the\pdfnoligatures\f readback): 1 when no
    /// existing char in bc..ec carries a ligature or extensible tag.
    pub fn test_no_ligatures(&self, f: u16) -> i32 {
        let font = &self.eqtb.fonts[f as usize];
        for c in font.bc..=font.ec {
            if font.char_present(c) {
                let t = font.chars[c as usize].tag;
                if t == crate::tfm::TAG_LIG || t == crate::tfm::TAG_EXT {
                    return 0;
                }
            }
        }
        1
    }

    /// `set_tag_code`: i in -7..0; |i| bits remove ext(4)/list(2)/lig(1)
    /// tags from the character's lig/kern program.
    pub fn set_tag_code(&mut self, f: u16, c: u8, i: i32) {
        let mut fixedi = i.clamp(-7, 0).abs();
        let font = Rc::clone(&self.eqtb.fonts[f as usize]);
        if !(c >= font.bc && c <= font.ec && font.char_present(c)) {
            return;
        }
        let mut nf = (*font).clone();
        let ci = &mut nf.chars[c as usize];
        if fixedi >= 4 {
            if ci.tag == crate::tfm::TAG_EXT {
                ci.tag = crate::tfm::TAG_NO_TAG;
            }
            fixedi -= 4;
        }
        if fixedi >= 2 {
            if ci.tag == crate::tfm::TAG_LIST {
                ci.tag = crate::tfm::TAG_NO_TAG;
            }
            fixedi -= 2;
        }
        if fixedi >= 1 && ci.tag == crate::tfm::TAG_LIG {
            ci.tag = crate::tfm::TAG_NO_TAG;
        }
        self.eqtb.fonts[f as usize] = Rc::new(nf);
    }

    /// `get_tag_code` readback: lig 1, list 2, ext 4, no tag 0, invalid -1.
    pub fn get_tag_code(&self, f: u16, c: u8) -> i32 {
        let font = match self.eqtb.fonts.get(f as usize) {
            Some(ft) => ft,
            None => return -1,
        };
        if !(c >= font.bc && c <= font.ec && font.char_present(c)) {
            return -1;
        }
        match font.chars[c as usize].tag {
            crate::tfm::TAG_LIG => 1,
            crate::tfm::TAG_LIST => 2,
            crate::tfm::TAG_EXT => 4,
            _ => 0,
        }
    }

    // ---------- letterspacing (pdftex.web letter_space_font §17474) ----------

    /// `\letterspacefont<cs><font id><number>[ nolig]`: a fresh copy of the
    /// font with every character width widened by e/1000 of the quad,
    /// wrapped in a synthesized virtual font that shifts each glyph by
    /// half the added space (kern before and after the glyph).
    pub fn do_letterspacefont(&mut self) {
        let global = self.take_global();
        self.clear_prefixes();
        let u = self.scan_definable_cs();
        self.scan_optional_equals();
        let f = self.scan_font_id();
        if f == 0 {
            self.error("letterspacing: invalid font identifier");
            return;
        }
        self.scan_optional_equals();
        let e = self.scan_int().clamp(-1000, 1000);
        let k = self.letter_space_font(u, f, e);
        if k != 0 {
            self.eqtb.assign(u, crate::eqtb::Equiv::FontRef(k), global);
        }
    }

    fn letter_space_font(&mut self, u: crate::token::CsId, f: u16, e: i32) -> u16 {
        let mut nf = match self.eqtb.fonts.get(f as usize) {
            Some(font) => (**font).clone(),
            None => {
                self.error("letterspacing: base font not found");
                return 0;
            }
        };
        let tfm_name = nf.tfm_name.clone();
        let at = self.eqtb.fonts[f as usize].at_size;
        if self.scan_keyword(b"nolig") {
            let (bc, ec) = (nf.bc, nf.ec);
            for c in bc..=ec {
                if nf.char_present(c) && nf.chars[c as usize].tag == crate::tfm::TAG_LIG {
                    nf.chars[c as usize].tag = crate::tfm::TAG_NO_TAG;
                }
            }
        }
        // quad fix: pdftex copies the base font's fontdimen6 when the fresh
        // load has none
        let quad_f = self.eqtb.font_params[f as usize]
            .get(5)
            .copied()
            .unwrap_or(0);
        let quad_k0 = nf.param(6);
        let quad = if quad_k0 == 0 && quad_f > 0 {
            quad_f
        } else {
            quad_k0
        };
        if quad == 0 {
            self.error("letterspacing: font has zero em size (\\fontdimen6)");
        }
        let dw = crate::tfm::round_xn_over_d(quad, e, 1000);
        for ci in nf.chars.iter_mut() {
            ci.width += dw;
        }
        // append e.g. "+100ls" to the font name
        nf.tfm_name = format!(
            "{}{}ls",
            tfm_name,
            if e > 0 {
                format!("+{}", e)
            } else {
                e.to_string()
            }
        );
        let k = self.push_engine_font(Rc::new(nf), u);
        self.copy_letterspace_params(k, f);
        if quad != 0 {
            if let Some(p) = self.eqtb.font_params[k as usize].get_mut(5) {
                if *p == 0 {
                    *p = quad_f;
                }
            }
        }
        // virtual wrapper: half the added space before and after each glyph
        let w = crate::tfm::round_xn_over_d(quad, e, 2000);
        let mut chars: Vec<Option<Rc<[VfStep]>>> = vec![None; 256];
        for c in 0..=255u8 {
            chars[c as usize] = Some(Rc::from(vec![VfStep {
                base: 0,
                ch: c,
                dx: w,
                dy: 0,
            }]));
        }
        let key = (self.eqtb.fonts[k as usize].tfm_name.clone(), at);
        self.font_loader.vf_fonts.insert(
            key,
            Rc::new(VfFont {
                bases: vec![crate::fontload::VfBase {
                    tfm_name,
                    at_size: at,
                }],
                chars,
            }),
        );
        self.font_loader.vf_bases.insert(k, vec![f]);
        self.font_loader.register_tracked_font(k, f, e);
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_font_error(source: String) -> Engine {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.add_nullfont();
        engine.set_interaction_mode(crate::engine::InteractionMode::Nonstop);
        engine
            .input
            .push_file("font-error.tex".to_string(), source.into_bytes());
        engine.run();
        engine
    }

    #[test]
    fn missing_font_is_reported_at_its_declaration() {
        let name = format!("definitely-missing-diagnostic-font-{}", std::process::id());
        let engine = run_font_error(format!("\\relax\n\\font\\broken={name}\n\\end\n"));
        let diagnostic = engine
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains(&name))
            .expect("missing-font diagnostic");
        assert_eq!(
            diagnostic.message,
            format!("Font \\broken={name} not found")
        );
        let source = diagnostic.primary.as_ref().expect("declaration source");
        assert_eq!(
            (source.name.as_str(), source.line, source.column),
            ("font-error.tex", 2, 1)
        );
        assert_eq!(source.text, format!("\\font\\broken={name}"));
    }

    #[test]
    fn failed_font_assignment_cannot_leak_prefixes_to_the_next_assignment() {
        let name = format!("definitely-missing-prefixed-font-{}", std::process::id());
        let mut engine = Engine::new(false);
        engine.init_primitives();
        engine.add_nullfont();
        engine.set_interaction_mode(crate::engine::InteractionMode::Nonstop);
        let source = format!(
            "\\count0=0\n{{\\global\\long\\outer\\protected\\font\\broken={name}\n\\count0=7}}\n\\end\n"
        );
        engine
            .input
            .push_file("font-error.tex".to_string(), source.into_bytes());
        engine.run();

        assert!(engine
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains(&name)));
        assert_eq!(engine.eqtb.count[0], 0, "a failed font leaked \\global");
        assert!(!engine.global_flag);
        assert!(!engine.long_flag);
        assert!(!engine.outer_flag);
        assert!(!engine.protected_flag);
    }

    #[test]
    fn unterminated_quoted_font_name_is_a_located_fatal_error() {
        let engine =
            run_font_error("\\relax\n\\font\\broken=\"unterminated font name\n".to_string());
        assert!(engine.stopped_on_error);
        assert_eq!(engine.diagnostics.len(), 1, "{}", engine.diagnostic_output);
        let diagnostic = &engine.diagnostics[0];
        assert_eq!(
            diagnostic.message,
            "File ended while scanning a quoted file name for \\font; add the closing quote"
        );
        assert!(!diagnostic.message.contains("not found"));
        let source = diagnostic.primary.as_ref().expect("declaration source");
        assert_eq!(
            (source.name.as_str(), source.line, source.column),
            ("font-error.tex", 2, 1)
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("close the file name with a matching double quote before the end of the file")
        );
    }

    #[test]
    fn unterminated_quoted_font_name_consumes_assignment_prefixes() {
        let engine = run_font_error(
            "\\global\\long\\outer\\protected\\font\\broken=\"unterminated font name\n".to_string(),
        );

        assert!(engine.stopped_on_error);
        assert!(!engine.global_flag);
        assert!(!engine.long_flag);
        assert!(!engine.outer_flag);
        assert!(!engine.protected_flag);
    }

    #[test]
    fn malformed_opentype_font_reports_the_parse_failure_at_the_declaration() {
        let path = std::env::temp_dir().join(format!(
            "tex-core-malformed-font-diagnostic-{}.otf",
            std::process::id()
        ));
        std::fs::write(&path, b"not an OpenType font").unwrap();
        let font_path = path.to_string_lossy().replace('\\', "/");
        let source = format!(
            "\\relax\n\\font\\broken={font_path}\n\\end\n"
        );
        let engine = run_font_error(source);
        let _ = std::fs::remove_file(&path);
        let diagnostic = engine
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic
                    .message
                    .starts_with("Cannot load OpenType font file")
            })
            .expect("OpenType parse diagnostic");
        assert!(
            diagnostic.message.contains("unknown magic"),
            "{}",
            diagnostic.message
        );
        let source = diagnostic.primary.as_ref().expect("declaration source");
        assert_eq!(
            (source.name.as_str(), source.line, source.column),
            ("font-error.tex", 2, 1)
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("check that the named file is a valid, supported OpenType or TrueType font")
        );
    }

    #[test]
    fn font_scaled_uses_thousandths_of_design_size() {
        let mut e = Engine::new(true);
        e.init_primitives();
        e.add_nullfont();
        e.input.push_file(
            "scaled-font.tex".to_string(),
            b"\\font\\scaledfont=cmr10 scaled 1100\n".to_vec(),
        );
        e.run();
        let cs = e.cs.lookup(b"scaledfont").expect("font control sequence");
        let Some(Equiv::FontRef(fid)) = e.eqtb.resolve(cs) else {
            panic!("scaledfont was not bound to a font");
        };
        assert_eq!(e.eqtb.fonts[*fid as usize].at_size, 11 * 65536);
    }

    #[test]
    fn borrowed_map_tokens_preserve_quote_and_whitespace_rules() {
        // Exercise attached, unmatched and empty quotes as well as Unicode
        // whitespace against the established concatenating tokenizer.
        let fragments = [
            "",
            " ",
            "a",
            "é",
            "\t",
            "\u{2003}",
            "\"",
            "\"x\"",
            " <font.pfb ",
        ];
        for a in fragments {
            for b in fragments {
                for c in fragments {
                    let line = format!("{a}{b}{c}");
                    let expected = split_map_tokens_joined(&line);
                    let (bare, quoted) = split_map_tokens(&line);
                    assert_eq!(
                        bare.iter().map(|s| s.as_ref()).collect::<Vec<_>>(),
                        expected.0,
                        "{line:?}"
                    );
                    assert_eq!(
                        quoted.iter().map(|s| s.as_ref()).collect::<Vec<_>>(),
                        expected.1,
                        "{line:?}"
                    );
                }
            }
        }
    }

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
        vf.extend_from_slice(&[
            242, 0, 0, 0, 2, 0, 0, 0, 200, 0x04, 0x28, 0xf6, 0x00, 133, 0x01,
        ]);
        vf.push(248); // post
        std::fs::write(dir.join("toyvf.vf"), &vf).unwrap();

        let mut fl = FontLoader {
            kpse: tex_kpse::Kpse::explicit(&dir, vec![]),
            map: crate::fontmap::FontMap::default(),
            tfm_cache: crate::FxHashMap::default(),
            enc_cache: crate::FxHashMap::default(),
            vf_fonts: crate::FxHashMap::default(),
            vf_bases: crate::FxHashMap::default(),
            tracked_fonts: crate::FxHashMap::default(),
            map_loaded: true,
            dependency_files: Vec::new(),
            dependency_directories: Vec::new(),
            dependency_file_digests: Vec::new(),
            dependency_missing_files: Vec::new(),
            dependency_missing_directories: Vec::new(),
            dependency_tracking_complete: true,
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
        let steps5 = vfv.chars[5].as_ref().unwrap();
        assert_eq!(steps5.len(), 2);
        assert_eq!(
            (steps5[0].base, steps5[0].ch, steps5[0].dx, steps5[0].dy),
            (0, 1, 0, 0)
        );
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
        let texmf = std::path::Path::new("/usr/share/texmf-dist");
        if !texmf.join("fonts/vf/public/newtx/ntxsy.vf").exists() {
            return;
        }
        let kpse = tex_kpse::Kpse::with_roots(
            &std::env::current_dir().unwrap(),
            &[texmf],
        );
        let mut fl = FontLoader::with_kpse(kpse);
        for (name, base) in [
            ("ntxsy", "txsys"),
            ("ntxexx", "txexs"),
            ("ntxmi", "NewTXMI"),
        ] {
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
                vfv.bases
                    .iter()
                    .map(|b| b.tfm_name.as_str())
                    .collect::<Vec<_>>()
            );
            let n_steps: usize = vfv.chars.iter().flatten().map(|s| s.len()).sum();
            assert!(n_steps > 100, "{name}: too few steps ({n_steps})");
        }
    }

    #[test]
    fn tfm_load_strips_extension_like_tex_web() {
        // tex.web §1257 / §526: scan_file_name splits extensions at the first dot,
        // but read_font_info only receives cur_name. When LaTeX NFSS specifies
        // `cmr6.5` or `cmr10.0` or `cmr10.tfm`, tex.web strips the extension and
        // loads `cmr6.tfm` / `cmr10.tfm`.
        if !std::path::Path::new("/usr/share/texmf-dist/fonts/tfm/public/cm/cmr6.tfm").exists() {
            return;
        }
        let mut fl = FontLoader::new();
        let f65 = fl.load_tfm("cmr6.5", 655360).expect("cmr6.5 should resolve to cmr6");
        assert_eq!(f65.tfm_name, "cmr6");

        let f100 = fl.load_tfm("cmr10.0", 655360).expect("cmr10.0 should resolve to cmr10");
        assert_eq!(f100.tfm_name, "cmr10");

        let f10tfm = fl.load_tfm("cmr10.tfm", 655360).expect("cmr10.tfm should resolve to cmr10");
        assert_eq!(f10tfm.tfm_name, "cmr10");
    }
}
