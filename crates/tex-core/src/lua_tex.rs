//! Engine side of LuaTeX's `tex` library (ltexlib.c): parameter, register
//! and code access, marks, fonts and the pure helpers. The Lua-visible
//! API shape (`tex.set`, `tex.count[n]`, ...) is built on these natives by
//! `lua_tex.lua`.

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString, LuaTable};

use crate::boxes::Glue;
use crate::engine::Engine;
use crate::eqtb::Equiv;
use crate::lua_bridge::{bytes_of, with_engine};
use crate::prim::Prim;

macro_rules! reg {
    ($lua:expr, $tbl:expr, $name:literal, $f:expr) => {
        $tbl.set($name, $lua.create_function($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}

type GlueTuple = (i64, i64, i64, i64, i64);

fn glue_tuple(g: &Glue) -> GlueTuple {
    (
        i64::from(g.width),
        i64::from(g.stretch),
        i64::from(g.shrink),
        i64::from(g.stretch_order),
        i64::from(g.shrink_order),
    )
}

fn order(v: i64) -> u8 {
    v.clamp(0, 3) as u8
}

/// The glue of a `setglue` call: `ltexlib.c` stores `lua_tointeger`
/// results, absent values being zero.
fn make_glue(v: [i64; 5]) -> Glue {
    let sp = |x: i64| x as i32;
    Glue::spec(sp(v[0]), sp(v[1]), order(v[3]), sp(v[2]), order(v[4])).eqtb_value()
}

/// A `{w, stretch, shrink, stretch_order, shrink_order}` Lua array.
fn spec_of(t: &LuaTable) -> Result<[i64; 5], String> {
    let mut v = [0i64; 5];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = t.raw_geti::<Option<i64>>(i as i64 + 1).map_err(|e| format!("{e:?}"))?.unwrap_or(0);
    }
    Ok(v)
}

/// Per-engine state of the Lua-side libraries.
#[derive(Default)]
pub(crate) struct TexState {
    /// `pdf.print` calls of the running `\latelua`: (literal mode, text), applied
    /// to the page content by the renderer after the Lua code returned. Modes:
    /// 0 origin, 1 page, 2 text, 3 direct, 4 raw.
    pub pdf_print: Vec<(u8, Vec<u8>)>,
    /// A late Lua call is running (`global_shipping_mode != NOT_SHIPPING`).
    pub in_late_lua: bool,
    /// Annotation objects `pdf.registerannot` added to the page being shipped.
    pub late_annots: Vec<i32>,
    /// `pdf.setforcefile(true)`: write the PDF file even when no page was shipped.
    pub force_file: bool,
    /// `pdf.settypeonewidemode` (an experimental flag of luatex's Type 1 writer).
    pub type1_wide_mode: i32,
    /// Fonts `pdf.includefont` initialised.
    pub included_fonts: Vec<i64>,
    /// Current output position (sp) while a late Lua call runs during
    /// shipout, for `pdf.gethpos`/`getvpos`.
    pub pdf_pos: (i32, i32),
    /// `tex.set_synctex_*` overrides.
    pub synctex_tag: Option<i32>,
    pub synctex_line: Option<i32>,
    pub synctex_no_files: bool,
    /// `texio.setescape(false)`: terminal output keeps control characters.
    pub texio_noescape: bool,
    /// `lang.*` values per language.
    pub lang: crate::FxHashMap<u8, crate::lua_lang::LangParams>,
    /// `pdf.getmatrix`: the current page matrix during a late Lua call.
    pub pdf_matrix: Option<[f64; 6]>,
    /// Lists imported for Lua that are tied to the engine (see
    /// `lua_texnodes`).
    pub links: Vec<crate::lua_texnodes::Link>,
    /// `tex.setlist` lists the engine has no counterpart of.
    pub scratch: Vec<(String, u32)>,
    /// `token.scan_list`: the box nesting depth whose box goes to Lua, and
    /// the box once it is complete.
    pub scan_depth: Option<usize>,
    pub scan_result: Option<crate::boxes::Node>,
    /// `tex.permitmathobsolete`: the obsolete `\math...mode` parameters
    /// accept values.
    pub permit_math_obsolete: bool,
    /// Margins of the forms made by `tex.saveboxresource`.
    pub xform_margin: crate::FxHashMap<i32, i32>,
}

/// How a register key resolves.
enum Slot {
    Reg(u16),
    Param(Prim),
}

impl Engine {
    /// The primitive LuaTeX knows as `name`, whatever the control sequence
    /// of that name currently means.
    fn lua_tex_prim(&self, name: &[u8]) -> Option<Prim> {
        match &self.lua_primitives.iter().find(|p| p.name == name)?.equiv {
            Equiv::Prim(p) => Some(*p),
            _ => None,
        }
    }

    /// Resolve a register key of class `kind` ("count", "dimen", "skip",
    /// "muskip", "toks", "attribute") as ltexlib.c `get_item_index`: a
    /// number, or the name of a `\countdef`-style control sequence, or (for
    /// the classes with parameters) a parameter name.
    fn lua_tex_slot(&self, kind: &str, idx: Option<i64>, name: Option<&[u8]>) -> Result<Slot, String> {
        let what = || format!("incorrect {kind} name");
        if let Some(i) = idx {
            return u16::try_from(i).map(Slot::Reg).map_err(|_| format!("incorrect {kind} index"));
        }
        let name = name.ok_or_else(|| format!("incorrect {kind} index"))?;
        if let Some(p) = self.lua_tex_prim(name) {
            let ok = matches!(
                (kind, p),
                ("count", Prim::IntP(_)) | ("dimen", Prim::DimP(_)) | ("toks", Prim::ToksP(_))
            ) || matches!((kind, p), ("skip", Prim::GlueP(g)) if !g.is_mu())
                || matches!((kind, p), ("muskip", Prim::GlueP(g)) if g.is_mu());
            if ok {
                return Ok(Slot::Param(p));
            }
        }
        let id = self.cs.lookup(name).ok_or_else(what)?;
        match (kind, self.eqtb.resolve(id)) {
            ("count", Some(Equiv::CountReg(i)))
            | ("dimen", Some(Equiv::DimenReg(i)))
            | ("skip", Some(Equiv::SkipReg(i)))
            | ("muskip", Some(Equiv::MuSkipReg(i)))
            | ("toks", Some(Equiv::ToksReg(i)))
            | ("attribute", Some(Equiv::AttributeReg(i))) => Ok(Slot::Reg(*i)),
            _ => Err(what()),
        }
    }

    fn lua_tex_glue_get(&mut self, mu: bool, idx: Option<i64>, name: Option<&[u8]>) -> Result<Glue, String> {
        let kind = if mu { "muskip" } else { "skip" };
        Ok(match self.lua_tex_slot(kind, idx, name)? {
            Slot::Reg(i) => {
                let regs = if mu { &self.eqtb.muskip } else { &self.eqtb.skip };
                regs.get(i as usize).copied().unwrap_or_default()
            }
            Slot::Param(Prim::GlueP(p)) => self.eqtb.glue_params[p.idx() as usize],
            Slot::Param(_) => return Err(format!("incorrect {kind} name")),
        })
    }

    fn lua_tex_glue_set(
        &mut self,
        mu: bool,
        idx: Option<i64>,
        name: Option<&[u8]>,
        glue: Glue,
        global: bool,
    ) -> Result<(), String> {
        let kind = if mu { "muskip" } else { "skip" };
        let global = self.lua_tex_global(global);
        match self.lua_tex_slot(kind, idx, name)? {
            Slot::Reg(i) if mu => self.eqtb.assign_muskip(i, glue, global),
            Slot::Reg(i) => self.eqtb.assign_skip(i, glue, global),
            Slot::Param(Prim::GlueP(p)) => self.eqtb.assign_glue_param(p, glue, global),
            Slot::Param(_) => return Err(format!("incorrect {kind} name")),
        }
        Ok(())
    }

    pub(crate) fn lua_tex_global(&self, global: bool) -> bool {
        global || self.eqtb.int_params[crate::prim::IntParam::GlobalDefs.idx() as usize] > 0
    }

    /// `tex.get(name)` for the parameters LuaTeX exposes: (class, value...).
    fn lua_tex_param_get(&mut self, name: &[u8]) -> Option<(&'static str, GlueTuple, Option<Vec<u8>>)> {
        let zero = (0, 0, 0, 0, 0);
        let p = self.lua_tex_prim(name)?;
        Some(match p {
            Prim::IntP(ip) => ("int", (i64::from(self.int_param_value(ip)), 0, 0, 0, 0), None),
            Prim::DimP(dp) => ("dim", (i64::from(self.dim_param_value(dp)), 0, 0, 0, 0), None),
            Prim::GlueP(gp) => {
                let g = self.eqtb.glue_params[gp.idx() as usize];
                (if gp.is_mu() { "muglue" } else { "glue" }, glue_tuple(&g), None)
            }
            Prim::ToksP(tp) => {
                let toks = self.eqtb.tok_params[tp.idx() as usize].clone();
                ("toks", zero, Some(self.tokens_to_bytes(&toks)))
            }
            Prim::LastPenalty => ("int", (i64::from(self.last_penalty_value()), 0, 0, 0, 0), None),
            Prim::LastKern => ("dim", (i64::from(self.last_kern_value()), 0, 0, 0, 0), None),
            Prim::LastSkip => ("glue", glue_tuple(&self.last_skip_value()), None),
            _ => return None,
        })
    }

    /// `tex.set(name, value)` for numeric and glue parameters; false when
    /// `name` is not a settable parameter of that class (LuaTeX ignores it).
    fn lua_tex_param_set(&mut self, name: &[u8], global: bool, value: [i64; 5], text: Option<Vec<u8>>) -> bool {
        use crate::prim::{DimParam, IntParam};
        let Some(p) = self.lua_tex_prim(name) else {
            return false;
        };
        let global = self.lua_tex_global(global);
        match p {
            Prim::IntP(ip) => {
                let v = value[0] as i32;
                match ip {
                    IntParam::PrevGraf => *self.prev_graf_mut() = v,
                    IntParam::DeadCycles => self.dead_cycles = v,
                    IntParam::SpaceFactor => self.space_factor = v,
                    IntParam::InteractionMode if !(0..=3).contains(&v) => return true,
                    _ => {}
                }
                self.eqtb.assign_int_param(ip, v, global);
            }
            Prim::DimP(dp) => {
                let v = value[0] as i32;
                if dp == DimParam::PrevDepth {
                    self.prev_depth = v;
                } else {
                    if dp == DimParam::PageGoal {
                        self.page_goal = i64::from(v);
                        self.page_goal_set = true;
                    } else if dp == DimParam::VSize && !self.page_box_seen {
                        self.page_goal = if v <= 0 { 0x3FFF_FFFF } else { i64::from(v) };
                    }
                    self.eqtb.assign_dim_param(dp, v, global);
                }
            }
            Prim::GlueP(gp) => self.eqtb.assign_glue_param(gp, make_glue(value), global),
            Prim::ToksP(tp) => {
                let toks = Engine::lua_str_toks(&text.unwrap_or_default());
                self.eqtb.assign_toks_param(tp, std::rc::Rc::new(toks), global);
            }
            _ => return false,
        }
        true
    }

    /// The text of mark `which` (0 top, 1 first, 2 bot, 3 splitfirst, 4
    /// splitbot) of `class`, `None` when empty.
    fn lua_tex_mark(&self, which: usize, class: usize) -> Option<Vec<u8>> {
        let toks = self.marks[which].get(class)?;
        (!toks.is_empty()).then(|| self.tokens_to_bytes(toks))
    }
}

/// tex.web `xn_over_d` for non-negative `x`: `(x*n)/d` and the remainder.
fn xn_over_d(x: i64, n: i64, d: i64) -> (i64, i64) {
    let t = (x % 65536) * n;
    let u = (x / 65536) * n + (t / 65536);
    let v = (u % d) * 65536 + (t % 65536);
    ((u / d) * 65536 + v / d, v % d)
}

/// tex.web `round_decimals` over digits `d[0..k]`.
fn round_decimals(d: &[u8]) -> i64 {
    let mut a = 0i64;
    for &digit in d.iter().rev() {
        a = (a + i64::from(digit) * 131072) / 10;
    }
    (a + 1) / 2
}

impl Engine {
    /// `tex.sp(string)`: tex.web §448 `scan_dimen` over the characters of
    /// `s` (ltexlib.c `tex_sp` runs it on a string pseudo-file), without
    /// internal quantities.
    fn lua_tex_sp(&self, s: &[u8]) -> Result<i64, String> {
        let mut i = 0;
        let skip_spaces = |i: &mut usize| {
            while s.get(*i) == Some(&b' ') {
                *i += 1;
            }
        };
        skip_spaces(&mut i);
        let mut negative = false;
        while matches!(s.get(i), Some(b'+' | b'-')) {
            negative ^= s[i] == b'-';
            i += 1;
            skip_spaces(&mut i);
        }
        let mut int = 0i64;
        let mut digits = 0;
        while let Some(d) = s.get(i).filter(|c| c.is_ascii_digit()) {
            int = (int * 10 + i64::from(d - b'0')).min(1 << 40);
            digits += 1;
            i += 1;
        }
        let mut frac = Vec::new();
        let mut has_point = false;
        if matches!(s.get(i), Some(b'.' | b',')) {
            has_point = true;
            i += 1;
            while let Some(d) = s.get(i).filter(|c| c.is_ascii_digit()) {
                if frac.len() < 17 {
                    frac.push(d - b'0');
                }
                i += 1;
            }
        }
        if digits == 0 && !has_point {
            return Err("Missing number, treated as zero".to_string());
        }
        skip_spaces(&mut i);
        let word = |i: &mut usize, kw: &[u8]| -> bool {
            let end = *i + kw.len();
            if s.len() >= end && s[*i..end].eq_ignore_ascii_case(kw) {
                *i = end;
                true
            } else {
                false
            }
        };
        let font = self.eqtb.cur_font_val as usize;
        let font_dim = |n: usize| i64::from(self.eqtb.font_params.get(font).and_then(|p| p.get(n - 1)).copied().unwrap_or(0));
        let frac_value = round_decimals(&frac);
        let (mut v, mut f);
        if word(&mut i, b"em") || word(&mut i, b"ex") {
            let m = if s[i - 1].eq_ignore_ascii_case(&b'm') { font_dim(6) } else { font_dim(5) };
            // nx_plus_y(f_unit): int * unit + frac * unit / 65536
            v = int * m + ((m * frac_value) >> 16);
            f = 0;
            skip_spaces(&mut i);
            if i != s.len() {
                return Err("conversion failed (trailing junk?)".to_string());
            }
            if v > 0x3FFF_FFFF {
                return Err("Dimension too large".to_string());
            }
            let r = v + f;
            return Ok(if negative { -r } else { r });
        }
        let _ = word(&mut i, b"true");
        let (num, den): (i64, i64) = if word(&mut i, b"pt") {
            (1, 1)
        } else if word(&mut i, b"in") {
            (7227, 100)
        } else if word(&mut i, b"pc") {
            (12, 1)
        } else if word(&mut i, b"cm") {
            (7227, 254)
        } else if word(&mut i, b"mm") {
            (7227, 2540)
        } else if word(&mut i, b"bp") {
            (7227, 7200)
        } else if word(&mut i, b"dd") {
            (1238, 1157)
        } else if word(&mut i, b"cc") {
            (14856, 1157)
        } else if word(&mut i, b"nd") {
            (685, 642)
        } else if word(&mut i, b"nc") {
            (1370, 107)
        } else if word(&mut i, b"sp") {
            (0, 0)
        } else {
            return Err("Illegal unit of measure (pt inserted)".to_string());
        };
        skip_spaces(&mut i);
        if i != s.len() {
            return Err("conversion failed (trailing junk?)".to_string());
        }
        if num == 0 {
            v = int;
            f = 0;
        } else {
            let (q, r) = xn_over_d(int, num, den);
            v = q;
            f = (num * frac_value + 65536 * r) / den;
            v += f / 65536;
            f %= 65536;
            if v >= 16384 {
                return Err("Dimension too large".to_string());
            }
            v = v * 65536 + f;
            f = 0;
        }
        let r = v + f;
        Ok(if negative { -r } else { r })
    }
}

fn code_char(c: i64, what: &str) -> Result<u32, String> {
    u32::try_from(c)
        .ok()
        .filter(|c| *c <= 0x10_FFFF)
        .ok_or_else(|| format!("incorrect character value {c} for tex.{what}()"))
}

/// Install the natives into the table `__ratex_texlib`.
pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let t: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;

    // ---- parameters ----
    reg!(lua, t, "pget", |name: LuaString| -> Result<(Option<String>, i64, i64, i64, i64, i64, Option<LuaBytes>), String> {
        let name = bytes_of(&name);
        with_engine(|e| match e.lua_tex_param_get(&name) {
            None => (None, 0, 0, 0, 0, 0, None),
            Some((kind, (w, st, sh, so, sho), text)) => {
                (Some(kind.to_string()), w, st, sh, so, sho, text.map(LuaBytes))
            }
        })
    });
    reg!(lua, t, "pset", |name: LuaString, global: bool, spec: LuaTable, text: Option<LuaString>| -> Result<bool, String> {
        let name = bytes_of(&name);
        let text = text.as_ref().map(bytes_of);
        let spec = spec_of(&spec)?;
        with_engine(|e| e.lua_tex_param_set(&name, global, spec, text))
    });

    // ---- glue registers ----
    reg!(lua, t, "glue_get", |mu: bool, idx: Option<i64>, name: Option<LuaString>| -> Result<GlueTuple, String> {
        let name = name.as_ref().map(bytes_of);
        with_engine(|e| e.lua_tex_glue_get(mu, idx, name.as_deref()).map(|g| glue_tuple(&g)))?
    });
    reg!(lua, t, "glue_set", |mu: bool, idx: Option<i64>, name: Option<LuaString>, global: bool, spec: LuaTable| -> Result<(), String> {
        let name = name.as_ref().map(bytes_of);
        let glue = make_glue(spec_of(&spec)?);
        with_engine(|e| e.lua_tex_glue_set(mu, idx, name.as_deref(), glue, global))?
    });

    // ---- character codes ----
    reg!(lua, t, "code_get", |kind: String, c: i64, what: String| -> Result<i64, String> {
        let c = code_char(c, &what)?;
        with_engine(|e| match kind.as_str() {
            "lc" => i64::from(e.eqtb.case_code(c, false)),
            "uc" => i64::from(e.eqtb.case_code(c, true)),
            "sf" => i64::from(e.eqtb.space_factor_code(c)),
            "math" => {
                let (class, family, slot) = e.eqtb.lua_math_code(c);
                i64::from(class) | (i64::from(family) << 4) | (i64::from(slot) << 12)
            }
            _ => {
                let (sf, sc, lf, lc) = e.eqtb.lua_del_code(c);
                i64::from(sf + 1) | (i64::from(sc) << 9) | (i64::from(lf) << 30) | (i64::from(lc) << 38)
            }
        })
    });
    reg!(lua, t, "code_set", |kind: String, c: i64, v: i64, global: bool, what: String| -> Result<(), String> {
        let c = code_char(c, &what)?;
        with_engine(|e| {
            let g = e.lua_tex_global(global);
            match kind.as_str() {
                "lc" => e.eqtb.assign_case_code(c, v as u32, false, g),
                "uc" => e.eqtb.assign_case_code(c, v as u32, true, g),
                "sf" => e.eqtb.assign_space_factor_code(c, v as u16, g),
                "math" => {
                    let class = (v & 0xF) as i32;
                    let family = ((v >> 4) & 0xFF) as i32;
                    let slot = ((v >> 12) & 0x1F_FFFF) as i32;
                    e.eqtb.assign_lua_math_code(c, class, family, slot, g);
                }
                _ => {
                    let sf = (v & 0x1FF) as i32 - 1;
                    let sc = ((v >> 9) & 0x1F_FFFF) as i32;
                    let lf = ((v >> 30) & 0xFF) as i32;
                    let lc = ((v >> 38) & 0x1F_FFFF) as i32;
                    e.eqtb.assign_lua_del_code(c, sf, sc, lf, lc, g);
                }
            }
        })
    });

    // ---- misc ----
    reg!(lua, t, "sp", |s: LuaString| -> Result<i64, String> {
        let s = bytes_of(&s);
        with_engine(|e| e.lua_tex_sp(&s))?
    });
    reg!(lua, t, "font_name", |f: i64| -> Result<Option<LuaBytes>, String> {
        with_engine(|e| {
            let f = u16::try_from(f).ok().filter(|f| (*f as usize) < e.eqtb.fonts.len())?;
            Some(LuaBytes(e.font_display_name(f).into_bytes()))
        })
    });
    reg!(lua, t, "font_identifier", |f: i64| -> Result<Option<LuaBytes>, String> {
        with_engine(|e| {
            let f = usize::try_from(f).ok().filter(|f| *f < e.eqtb.fonts.len())?;
            let cs = e.eqtb.font_cs.get(f).copied().unwrap_or(0);
            let mut out = vec![b'\\'];
            out.extend_from_slice(e.cs.name(cs));
            Some(LuaBytes(out))
        })
    });
    reg!(lua, t, "job_name", || -> Result<LuaBytes, String> {
        with_engine(|e| LuaBytes(e.job_name.clone().into_bytes()))
    });
    reg!(lua, t, "format_name", || -> Result<LuaBytes, String> {
        with_engine(|e| LuaBytes(e.format_name.clone().into_bytes()))
    });
    reg!(lua, t, "cur_font", || -> Result<i64, String> { with_engine(|e| i64::from(e.eqtb.cur_font_val)) });

    // ---- random numbers (pdftex.web \pdfuniformdeviate & co.) ----
    reg!(lua, t, "rand_init", |seed: i64| -> Result<(), String> {
        with_engine(|e| e.rng = crate::random::Randoms::new((seed as i32).saturating_abs()))
    });
    reg!(lua, t, "uniform_rand", |x: i64| -> Result<i64, String> {
        with_engine(|e| i64::from(e.rng.unif_rand(x as i32)))
    });
    reg!(lua, t, "normal_rand", || -> Result<i64, String> { with_engine(|e| i64::from(e.rng.norm_rand())) });

    // ---- mode and page ----
    reg!(lua, t, "force_hmode", |indented: bool| -> Result<(), String> {
        with_engine(|e| {
            if e.mode.is_v() {
                e.start_paragraph(indented);
            }
        })
    });
    reg!(lua, t, "trigger_build_page", || -> Result<(), String> { with_engine(|e| e.build_page()) });
    reg!(lua, t, "page_state", || -> Result<i64, String> {
        with_engine(|e| {
            if e.page_box_seen {
                2
            } else if e.page_list.iter().any(|n| matches!(n, crate::boxes::Node::Ins { .. })) {
                1
            } else {
                0
            }
        })
    });
    reg!(lua, t, "reset_paragraph", || -> Result<(), String> {
        use crate::prim::{DimParam, IntParam};
        with_engine(|e| {
            e.eqtb.dim_params[DimParam::HangIndent.idx() as usize] = 0;
            e.eqtb.int_params[IntParam::HangAfter.idx() as usize] = 1;
            e.eqtb.int_params[IntParam::Looseness.idx() as usize] = 0;
            e.par_shape.clear();
            e.penalty_shapes = Default::default();
        })
    });

    // ---- token lists and definitions ----
    reg!(lua, t, "quit_local", || -> Result<(), String> {
        with_engine(Engine::end_local_control)
    });
    reg!(lua, t, "local_level", || -> Result<i64, String> { with_engine(|e| i64::from(e.local_level)) });
    reg!(lua, t, "hash_tokens", || -> Result<Vec<LuaBytes>, String> {
        with_engine(|e| {
            let mut names = Vec::new();
            for id in e.cs.all_ids() {
                let name = e.cs.name(id);
                if name.first() == Some(&0xFF) || name.starts_with(b"\0") || e.eqtb.get(id).is_none() {
                    continue;
                }
                names.push(LuaBytes(name.to_vec()));
            }
            names
        })
    });
    reg!(lua, t, "scan_toks_into", |idx: Option<i64>, name: Option<LuaString>, table: Option<i64>, text: LuaString, global: bool| -> Result<(), String> {
        let name = name.as_ref().map(bytes_of);
        let text = bytes_of(&text);
        with_engine(|e| {
            let Slot::Reg(i) = e.lua_tex_slot("toks", idx, name.as_deref())? else {
                return Err("incorrect toks name".to_string());
            };
            let table = table.and_then(|t| i32::try_from(t).ok());
            let toks = e.lua_string_to_macro_body(table, &text);
            let global = e.lua_tex_global(global);
            e.eqtb.assign_toks_reg(i, std::rc::Rc::new(toks), global);
            Ok(())
        })?
    });
    reg!(lua, t, "define_font", |name: LuaString, font: i64, global: bool| -> Result<(), String> {
        let name = bytes_of(&name);
        with_engine(|e| {
            let f = u16::try_from(font).ok().filter(|f| (*f as usize) < e.eqtb.fonts.len().max(1));
            let Some(f) = f else {
                return Err(format!("font {font} does not exist"));
            };
            let id = e.cs.intern(&name);
            let global = e.lua_tex_global(global);
            e.eqtb.assign(id, Equiv::FontRef(f), global);
            Ok(())
        })?
    });
    reg!(lua, t, "register_kind", |name: LuaString| -> Result<Option<String>, String> {
        // isbox/iscount/... : what a control sequence is bound to.
        let name = bytes_of(&name);
        with_engine(|e| {
            let id = e.cs.lookup(&name)?;
            Some(match e.eqtb.resolve(id)? {
                Equiv::CountReg(i) => format!("count {i}"),
                Equiv::DimenReg(i) => format!("dimen {i}"),
                Equiv::SkipReg(i) => format!("skip {i}"),
                Equiv::MuSkipReg(i) => format!("muskip {i}"),
                Equiv::ToksReg(i) => format!("toks {i}"),
                Equiv::BoxReg(i) => format!("box {i}"),
                Equiv::AttributeReg(i) => format!("attribute {i}"),
                _ => return None,
            })
        })
    });

    // ---- synctex ----
    reg!(lua, t, "synctex_get", |which: String| -> Result<i64, String> {
        with_engine(|e| match which.as_str() {
            "mode" => i64::from(e.eqtb.int_params[crate::prim::IntParam::Synctex.idx() as usize]),
            "tag" => match e.lua_tex.synctex_tag {
                Some(t) => i64::from(t),
                None => e
                    .input
                    .current_file_position()
                    .filter(|(p, _)| !p.is_empty())
                    .map_or(0, |(p, _)| i64::from(e.synctex.get_or_register_file(p))),
            },
            _ => match e.lua_tex.synctex_line {
                Some(l) => i64::from(l),
                None => e.input.current_file_position().map_or(0, |(_, l)| i64::from(l)),
            },
        })
    });
    reg!(lua, t, "synctex_set", |which: String, v: i64| -> Result<(), String> {
        with_engine(|e| match which.as_str() {
            "mode" => e.eqtb.assign_int_param(crate::prim::IntParam::Synctex, v as i32, true),
            "tag" => e.lua_tex.synctex_tag = (v != 0).then_some(v as i32),
            "line" => e.lua_tex.synctex_line = (v != 0).then_some(v as i32),
            _ => e.lua_tex.synctex_no_files = true,
        })
    });

    // ---- texio ----
    reg!(lua, t, "texio_setescape", |on: bool| -> Result<(), String> {
        with_engine(|e| e.lua_tex.texio_noescape = !on)
    });
    reg!(lua, t, "texio_closeinput", || -> Result<(), String> { with_engine(|e| e.do_endinput()) });

    // ---- marks ----
    reg!(lua, t, "mark_get", |which: i64, class: i64| -> Result<Option<LuaBytes>, String> {
        with_engine(|e| {
            let class = usize::try_from(class).ok()?;
            e.lua_tex_mark(which as usize, class).map(LuaBytes)
        })
    });

    crate::lua_texnodes::install(lua, &t)?;
    lua.set_global("__ratex_texlib", t).map_err(|e| format!("{e:?}"))?;
    lua.load(include_str!("lua_tex.lua"))
        .set_name("=[ratex tex]")
        .exec()
        .map_err(|e| format!("tex library: {}", lua.get_error_message(e).message()))?;
    crate::lua_pdf::install(lua)?;
    crate::lua_pdfe::install(lua)?;
    crate::lua_lang::install(lua)
}

