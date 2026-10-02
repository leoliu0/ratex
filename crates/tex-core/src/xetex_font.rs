//! XeTeX font definition: `\font` with native fonts (`read_font_info`,
//! `load_native_font` of xetex.web; `findnativefont`, `loadOTfont` of
//! XeTeX_ext.c).

use crate::engine::Engine;
use crate::eqtb::Equiv;
use crate::native_font::{
    d2fix, fix2d, parse_font_options, split_font_name, FontOptionEvent, NativeFont, ReqEngine,
};
use crate::tfm::Font;
use std::rc::Rc;

/// Everything `findnativefont` hands back besides the engine.
struct Located {
    program: Rc<crate::font_program::FontProgram>,
    full_name: String,
    feat: Option<String>,
    req: ReqEngine,
    design_size: i32,
    point_size: i32,
    origin: String,
}

impl Engine {
    /// `begin_diagnostic; print_nl(text); end_diagnostic(false)`
    pub(crate) fn xetex_diag(&mut self, text: &str) {
        let term = self.diagnostic_to_term();
        self.print_nl_diagnostic(text, term);
    }

    /// `print` of a character code as the `Missing character` message does:
    /// printable characters as UTF-8, the rest in `^^` notation.
    fn xe_printed_char(&self, c: u32) -> String {
        if c < 128 {
            if (32..127).contains(&c) {
                return (c as u8 as char).to_string();
            }
            let b = c as u8;
            return if b < 64 { format!("^^{}", (b + 64) as char) } else { format!("^^{}", (b - 64) as char) };
        }
        if c < 256 && !self.xprn[c as usize] {
            return format!("^^{c:02x}");
        }
        char::from_u32(c).map(String::from).unwrap_or_default()
    }

    /// `char_warning(f, c)` of TeX Live's XeTeX (web2c `tex.ch` change:
    /// code point in hex, error at `\tracinglostchars>=3`).
    pub(crate) fn xetex_char_warning(&mut self, f: u16, c: u32) {
        let lost = self.xe_int(crate::xetex_text::XeParam::TracingLostChars);
        if lost <= 0 {
            return;
        }
        let font = self.eqtb.fonts.get(f as usize);
        let name = font.map_or(String::new(), |font| font.tfm_name.clone());
        let native = font.is_some_and(|font| font.native.is_some());
        let ch = self.xe_printed_char(c);
        let code = if native { format!("(U+{c:04X})") } else { format!("(\"{c:X})") };
        if lost >= 3 {
            self.error(&format!("Missing character: There is no {ch} {code} in font {name}"));
            return;
        }
        let msg = format!("Missing character: There is no {ch} {code} in font {name}!");
        // eTeX: \tracinglostchars>1 shows the message on the terminal too
        let term = lost > 1 || self.diagnostic_to_term();
        self.print_nl_diagnostic(&msg, term);
    }

    fn find_font_program(&mut self, name: &str, index: u32) -> Option<Rc<crate::font_program::FontProgram>> {
        // kpse_find_file(name, opentype / truetype / type1): the file as
        // spelled, then with the suffixes of those formats
        let mut candidates = vec![name.to_string()];
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".otf") || lower.ends_with(".ttf") || lower.ends_with(".ttc") || lower.ends_with(".pfb") || lower.ends_with(".pfa") || lower.ends_with(".dfont")) {
            for ext in [".otf", ".ttf", ".ttc", ".pfb", ".pfa"] {
                candidates.push(format!("{name}{ext}"));
            }
        }
        for c in candidates {
            if let Some(bytes) = self.font_loader.read_program_bytes(&c) {
                return self.font_loader.load_program(bytes, index, Vec::new()).ok();
            }
        }
        None
    }

    /// `getDesignSize` of a program, in sp (D2Fix of TeX points; 10pt default).
    fn design_size_of(&self, program: &crate::font_program::FontProgram) -> i32 {
        d2fix(gpos_design_size(program).unwrap_or(10.0))
    }

    /// `findnativefont`. `scaled_size` is sp, or `-n` for `scaled n`.
    fn xetex_find_native_font(&mut self, name: &str, scaled_size: i32) -> Option<Located> {
        let tracing = self.xe_tracing_fonts();
        let sp = split_font_name(name);
        let mut scaled = scaled_size;
        if sp.name.starts_with('[') {
            let path = &sp.name[1..];
            let program = self.find_font_program(path, sp.index)?;
            if scaled < 0 {
                let dsize = self.design_size_of(&program);
                scaled = if scaled == -1000 { dsize } else { crate::scaled::xn_over_d(dsize, -scaled, 1000) };
            }
            let design = self.design_size_of(&program);
            let mut req = ReqEngine::Default;
            if let Some(v) = sp.var {
                // varString here still carries its leading slash
                let v = v.trim_start_matches('/');
                if v.starts_with("AAT") {
                    req = ReqEngine::Aat;
                } else if v.starts_with("OT") || v.starts_with("ICU") {
                    req = ReqEngine::Ot;
                } else if v.starts_with("GR") {
                    req = ReqEngine::Graphite;
                }
            }
            if tracing > 0 {
                self.xetex_diag(&format!(" -> {path}"));
            }
            return Some(Located {
                program,
                full_name: name.to_string(),
                feat: sp.feat.map(str::to_string),
                req,
                design_size: design,
                point_size: scaled,
                origin: path.to_string(),
            });
        }
        let size_pt = fix2d(scaled);
        let mut mgr = std::mem::take(&mut self.font_loader.font_mgr);
        let found = mgr.find_font(sp.name, sp.var, size_pt);
        self.font_loader.font_mgr = mgr;
        let found = found?;
        let program = self.find_font_program(found.face.file, found.face.face_index)?;
        if scaled < 0 {
            let dsize = self.design_size_of(&program);
            scaled = if scaled == -1000 { dsize } else { crate::scaled::xn_over_d(dsize, -scaled, 1000) };
        }
        let mut full = found.full_name.clone();
        if !found.variant.is_empty() {
            full.push('/');
            full.push_str(&found.variant);
        }
        if let Some(f) = sp.feat {
            if !f.is_empty() {
                full.push(':');
                full.push_str(f);
            }
        }
        if tracing > 0 {
            self.xetex_diag(&format!(" -> {}", found.face.file));
        }
        Some(Located {
            program,
            full_name: full,
            feat: sp.feat.map(str::to_string),
            req: found.req_engine,
            design_size: found.design_size,
            point_size: scaled,
            origin: found.face.file.to_string(),
        })
    }

    /// `loadOTfont` + `load_native_font`: returns the font id (existing or new).
    pub(crate) fn xetex_load_native_font(
        &mut self,
        name: &str,
        s: i32,
        cs: crate::token::CsId,
    ) -> Option<u16> {
        let loc = self.xetex_find_native_font(name, s)?;
        let opts = parse_font_options(loc.feat.as_deref().unwrap_or(""), loc.req);
        let full_name = loc.full_name.clone();
        let actual_size = if s >= 0 {
            s
        } else if s != -1000 {
            crate::scaled::xn_over_d(loc.design_size, -s, 1000)
        } else {
            loc.design_size
        };
        // option diagnostics, in order
        let mut mapping = None;
        for ev in &opts.events {
            match ev {
                FontOptionEvent::BadOption(t) => {
                    self.xetex_diag(&format!("Unknown feature `{t}' in font `{full_name}'."));
                }
                FontOptionEvent::Mapping(m) => {
                    mapping = self.xetex_load_mapping(m, &full_name);
                }
            }
        }
        // already loaded (canonical name and size)?
        for (k, f) in self.eqtb.fonts.iter().enumerate().skip(1) {
            if f.native.is_some() && f.tfm_name == full_name && f.at_size == actual_size {
                let _ = k;
                return Some(k as u16);
            }
        }
        let point_size = loc.point_size;
        let ps_f = fix2d(point_size) as f32;
        let program = loc.program;
        let (ascent, descent, x_ht_raw, cap_ht_raw, italic_angle) = {
            let face = program.shape_face()?;
            let upem = program.units_per_em.max(1) as f32;
            let utp = |u: f32| (u * ps_f) / upem;
            // FreeType (sfobjs.c): hhea ascender/descender, else OS/2 typo, else win.
            let tables = face.tables();
            let (mut asc, mut desc) = (tables.hhea.ascender as i32, tables.hhea.descender as i32);
            if asc == 0 && desc == 0 {
                if let Some(os2) = tables.os2 {
                    if os2.typographic_ascender() != 0 || os2.typographic_descender() != 0 {
                        asc = os2.typographic_ascender() as i32;
                        desc = os2.typographic_descender() as i32;
                    } else {
                        asc = os2.windows_ascender() as i32;
                        desc = os2.windows_descender() as i32;
                    }
                }
            }
            (
                utp(asc as f32),
                utp(desc as f32),
                utp(face.x_height().unwrap_or(0) as f32),
                utp(face.capital_height().unwrap_or(0) as f32),
                face.italic_angle().unwrap_or(0.0) as f64,
            )
        };
        let mut nf = NativeFont {
            program,
            full_name: full_name.clone(),
            req_engine: loc.req,
            script: opts.script,
            language: opts.language,
            features: opts.features.clone(),
            shapers: opts.shapers.clone(),
            vertical: opts.vertical,
            colored: opts.colored,
            rgba: if opts.colored { opts.rgba } else { 0x0000_00FF },
            extend: opts.extend,
            slant: opts.slant,
            embolden: if opts.embolden != 0.0 {
                (opts.embolden as f64 * fix2d(point_size) / 100.0) as f32
            } else {
                0.0
            },
            letter_space: if opts.letterspace != 0.0 {
                ((opts.letterspace as f64 / 100.0) * point_size as f64) as i32
            } else {
                0
            },
            mapping,
            design_size: loc.design_size,
            point_size: ps_f,
            origin: loc.origin,
            height_base: d2fix(ascent as f64),
            depth_base: -d2fix(descent as f64),
            slant_param: 0,
            x_height: 0,
            cap_height: 0,
            bbox_cache: Default::default(),
        };
        // ot_get_font_metrics
        let font_slant = d2fix(
            d2fix_slant(italic_angle) * nf.extend as f64 + nf.slant as f64,
        );
        let mut x_ht = d2fix(x_ht_raw as f64);
        let mut cap_ht = d2fix(cap_ht_raw as f64);
        if x_ht == 0 {
            let g = nf.map_char('x' as u32);
            x_ht = if g != 0 { d2fix(nf.glyph_height_depth(g).0 as f64) } else { nf.height_base / 2 };
        }
        if cap_ht == 0 {
            let g = nf.map_char('X' as u32);
            cap_ht = if g != 0 { d2fix(nf.glyph_height_depth(g).0 as f64) } else { nf.height_base };
        }
        nf.slant_param = font_slant;
        nf.x_height = x_ht;
        nf.cap_height = cap_ht;
        // width of the space character
        let space_w = {
            let mut probe = nf.clone();
            probe.mapping = None;
            crate::native_layout::measure_native_word(&probe, " ", false).width
        };
        let s_space = space_w + nf.letter_space;
        let cs_name = String::from_utf8_lossy(self.cs.name(cs)).to_string();
        let mut params = vec![font_slant, s_space, s_space / 2, s_space / 3, x_ht, actual_size, s_space / 3, cap_ht];
        // xetex.web load_native_font: an OpenType math font gets \fontdimen9 =
        // the number of assigned dimens and the MathConstants as 10..65
        if let Some(constants) = crate::math_xetex::ot_math_constants(&nf.program, actual_size) {
            params.push(9 + constants.len() as i32);
            params.extend_from_slice(&constants);
        }
        let nf = Rc::new(nf);
        let font = Font {
            name: cs_name,
            tfm_name: full_name,
            at_size: actual_size,
            dsize: loc.design_size,
            chars: Vec::new(),
            bc: 1,
            ec: 0,
            lig_kern: Vec::new(),
            kerns: Vec::new(),
            ext: Vec::new(),
            params,
            hyphen_char: 45,
            skew_char: -1,
            bchar: None,
            type1_path: None,
            enc_name: None,
            map_fontname: Some(nf.program.postscript_name.clone()),
            encoding: None,
            lua: None,
            native: Some(nf),
        };
        let id = self.push_engine_font(Rc::new(font), cs);
        self.eqtb.has_native_fonts = true;
        // space char missing from the font: tracinglostchars warning
        if self.xe_int(crate::xetex_text::XeParam::TracingLostChars) > 0
            && self.eqtb.fonts[id as usize].native.as_ref().unwrap().map_char(' ' as u32) == 0
        {
            self.xetex_char_warning(id, ' ' as u32);
        }
        Some(id)
    }

    fn xetex_load_mapping(&mut self, name: &str, font_name: &str) -> Option<Rc<crate::teckit::TextMapping>> {
        let file = format!("{name}.tec");
        let tracing = self.xe_tracing_fonts();
        match crate::teckit::TextMapping::builtin(name) {
            Some(m) => {
                if tracing > 1 {
                    self.xetex_diag(&format!("Loaded mapping `{file}' for font `{font_name}'."));
                }
                Some(Rc::new(m))
            }
            None => {
                self.xetex_diag(&format!("Font mapping `{file}' for font `{font_name}' not found."));
                None
            }
        }
    }

    /// File-name scan of `\font` with web2c quoting (`more_name`): returns
    /// the name without quote characters, the quote character used (0 if
    /// none) and `quoted_filename`.
    fn scan_xetex_font_name(&mut self) -> (String, u32) {
        self.skip_spaces_relax();
        let mut name: Vec<u8> = Vec::new();
        let mut quote: u32 = 0;
        let mut first_quote: u32 = 0;
        let mut t = self.get_x_raw();
        loop {
            if t == crate::input::EOF_MARKER {
                break;
            }
            if !t.is_char() {
                self.push_token(t);
                break;
            }
            if quote == 0 && self.file_name_line_ended(t) {
                break;
            }
            let c = t.chr();
            if c == 32 && quote == 0 {
                break;
            }
            if quote != 0 && c == quote {
                quote = 0;
            } else if quote == 0 && (c == '"' as u32 || c == '\'' as u32) {
                quote = c;
                if first_quote == 0 {
                    first_quote = c;
                }
            } else {
                t.append_character_bytes(&mut name);
            }
            t = self.get_x_raw();
        }
        (String::from_utf8_lossy(&name).into_owned(), first_quote)
    }

    /// XeTeX `\font`: `new_font` (§1257) with `read_font_info`.
    pub(crate) fn xetex_do_font(&mut self, cs: crate::token::CsId, global: bool, source: Option<crate::input::SourceContext>) {
        // tex.web §1257: `define(u, set_font, null_font)` before scanning
        self.eqtb.assign(cs, Equiv::FontRef(0), global);
        let (name, quote) = self.scan_xetex_font_name();
        let quoted = quote != 0;
        // at / scaled
        let mut s: i32 = -1000;
        if self.scan_keyword(b"at") {
            let v = self.scan_dimen(false, false);
            if v <= 0 || v >= 0x800_0000 {
                self.error_at(
                    &format!("Improper `at' size ({}pt), replaced by 10pt", crate::build::print_scaled(v as i64)),
                    source.clone(),
                );
                s = 10 * 65536;
            } else {
                s = v;
            }
        } else if self.scan_keyword(b"scaled") {
            let v = self.scan_int();
            if v <= 0 || v > 32768 {
                self.error_at(
                    "Illegal magnification has been changed to 1000",
                    source.clone(),
                );
                s = -1000;
            } else {
                s = -v;
            }
        }
        let tracing = self.xe_tracing_fonts();
        if tracing > 0 {
            let size = if s < 0 {
                format!(" scaled {}", -s)
            } else {
                format!(" at {}pt", crate::build::print_scaled(s as i64))
            };
            self.xetex_diag(&format!("Requested font \"{name}\"{size}"));
        }
        let mut found: Option<u16> = None;
        if quoted {
            found = self.xetex_load_native_font(&name, s, cs);
        }
        let mut tfm_opened = false;
        if found.is_none() {
            // a TFM file of that name
            let at = if s >= 0 {
                s
            } else if s == -1000 {
                0
            } else {
                let dsize = self.font_loader.load_tfm(&name, 0).map(|f| f.dsize).unwrap_or(655360);
                crate::scaled::xn_over_d(dsize, -s, 1000)
            };
            if let Some(font) = self.font_loader.load_tfm(&name, at) {
                tfm_opened = true;
                let at_size = font.at_size;
                let mut reuse = None;
                for (k, existing) in self.eqtb.fonts.iter().enumerate().skip(1) {
                    if existing.native.is_none() && existing.tfm_name == font.tfm_name && existing.at_size == at_size {
                        reuse = Some(k as u16);
                        break;
                    }
                }
                found = Some(match reuse {
                    Some(k) => k,
                    None => self.push_engine_font(font, cs),
                });
            }
        }
        if found.is_none() && !quoted {
            found = self.xetex_load_native_font(&name, s, cs);
        }
        let Some(id) = found else {
            if self.eqtb.int_params[crate::prim::IntParam::SuppressFontNotFoundError.idx() as usize] == 0 {
                let cs_name = String::from_utf8_lossy(self.cs.name(cs)).to_string();
                let q = if quote != 0 { char::from_u32(quote).unwrap().to_string() } else { String::new() };
                let size = if s >= 0 {
                    format!(" at {}pt", crate::build::print_scaled(s as i64))
                } else if s != -1000 {
                    format!(" scaled {}", -s)
                } else {
                    String::new()
                };
                self.error_at(
                    &format!("Font \\{cs_name}={q}{name}{q}{size} not loadable: Metric (TFM) file or installed font not found"),
                    source,
                );
            }
            if tracing > 0 {
                self.xetex_diag(" -> font not found, using \"nullfont\"");
            }
            return;
        };
        if tracing > 0 && tfm_opened {
            let n = self.eqtb.fonts[id as usize].tfm_name.clone();
            self.xetex_diag(&format!(" -> {n}.tfm"));
        }
        self.eqtb.font_cs[id as usize] = cs;
        self.eqtb.assign(cs, Equiv::FontRef(id), global);
    }
}

/// `D2Fix(tan(-italAngle * pi/180))` as a double (the argument of `Fix2D`).
fn d2fix_slant(italic_angle: f64) -> f64 {
    let fix = d2fix((-italic_angle * std::f64::consts::PI / 180.0).tan());
    fix2d(fix)
}

/// Design size (TeX points) from the GPOS `size` feature, if any.
pub(crate) fn gpos_design_size(program: &crate::font_program::FontProgram) -> Option<f64> {
    let face = program.face().ok()?;
    let gpos = face.raw_face().table(ttf_parser::Tag::from_bytes(b"GPOS"))?;
    let u16at = |o: usize| gpos.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize);
    let flo = u16at(6)?;
    let count = u16at(flo)?;
    for i in 0..count {
        let rec = flo + 2 + i * 6;
        if gpos.get(rec..rec + 4)? != b"size" {
            continue;
        }
        let ft = flo + u16at(rec + 4)?;
        let po = u16at(ft)?;
        if po == 0 {
            return None;
        }
        let design = u16at(ft + po)?;
        if design == 0 {
            return None;
        }
        return Some(design as f64 * 72.27 / 72.0 / 10.0);
    }
    None
}
