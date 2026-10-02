//! LuaTeX math parameters and math-font access (luatex `mlist.c`
//! `fixup_math_parameters`, `finalize_math_parameters`, `get_math_param`
//! and the `fetch`/`char_*` font accessors that work for TFM and Lua fonts).
//!
//! Everything here serves the LuaTeX engine only: pdfTeX and the other
//! engines keep reading `\fontdimen`s directly (see `math.rs`).

use std::rc::Rc;

use crate::engine::{Engine, EngineKind};
use crate::eqtb::UNDEFINED_MATH_PARAMETER;
use crate::lua_font::MathVariant;
use crate::math::GStyle;
use crate::tfm::{Font, FontId, TAG_EXT, TAG_LIST};
use crate::uprim::mp::*;

/// `font_MATH_par` indices (1-based position in `MATH_param_names`).
pub(crate) mod mc {
    pub const DISPLAY_OPERATOR_MIN_HEIGHT: usize = 4;
    pub const AXIS_HEIGHT: usize = 6;
    pub const ACCENT_BASE_HEIGHT: usize = 7;
    pub const SUBSCRIPT_SHIFT_DOWN: usize = 9;
    pub const SUBSCRIPT_TOP_MAX: usize = 10;
    pub const SUBSCRIPT_BASELINE_DROP_MIN: usize = 11;
    pub const SUPERSCRIPT_SHIFT_UP: usize = 12;
    pub const SUPERSCRIPT_SHIFT_UP_CRAMPED: usize = 13;
    pub const SUPERSCRIPT_BOTTOM_MIN: usize = 14;
    pub const SUPERSCRIPT_BASELINE_DROP_MAX: usize = 15;
    pub const SUB_SUPERSCRIPT_GAP_MIN: usize = 16;
    pub const SUPERSCRIPT_BOTTOM_MAX_WITH_SUBSCRIPT: usize = 17;
    pub const SPACE_AFTER_SCRIPT: usize = 18;
    pub const UPPER_LIMIT_GAP_MIN: usize = 19;
    pub const UPPER_LIMIT_BASELINE_RISE_MIN: usize = 20;
    pub const LOWER_LIMIT_GAP_MIN: usize = 21;
    pub const LOWER_LIMIT_BASELINE_DROP_MIN: usize = 22;
    pub const STACK_TOP_SHIFT_UP: usize = 23;
    pub const STACK_TOP_DISPLAY_STYLE_SHIFT_UP: usize = 24;
    pub const STACK_BOTTOM_SHIFT_DOWN: usize = 25;
    pub const STACK_BOTTOM_DISPLAY_STYLE_SHIFT_DOWN: usize = 26;
    pub const STACK_GAP_MIN: usize = 27;
    pub const STACK_DISPLAY_STYLE_GAP_MIN: usize = 28;
    pub const STRETCH_STACK_TOP_SHIFT_UP: usize = 29;
    pub const STRETCH_STACK_BOTTOM_SHIFT_DOWN: usize = 30;
    pub const STRETCH_STACK_GAP_ABOVE_MIN: usize = 31;
    pub const STRETCH_STACK_GAP_BELOW_MIN: usize = 32;
    pub const FRACTION_NUMERATOR_SHIFT_UP: usize = 33;
    pub const FRACTION_NUMERATOR_DISPLAY_STYLE_SHIFT_UP: usize = 34;
    pub const FRACTION_DENOMINATOR_SHIFT_DOWN: usize = 35;
    pub const FRACTION_DENOMINATOR_DISPLAY_STYLE_SHIFT_DOWN: usize = 36;
    pub const FRACTION_NUMERATOR_GAP_MIN: usize = 37;
    pub const FRACTION_NUMERATOR_DISPLAY_STYLE_GAP_MIN: usize = 38;
    pub const FRACTION_RULE_THICKNESS: usize = 39;
    pub const FRACTION_DENOMINATOR_GAP_MIN: usize = 40;
    pub const FRACTION_DENOMINATOR_DISPLAY_STYLE_GAP_MIN: usize = 41;
    pub const SKEWED_FRACTION_HORIZONTAL_GAP: usize = 42;
    pub const SKEWED_FRACTION_VERTICAL_GAP: usize = 43;
    pub const OVERBAR_VERTICAL_GAP: usize = 44;
    pub const OVERBAR_RULE_THICKNESS: usize = 45;
    pub const OVERBAR_EXTRA_ASCENDER: usize = 46;
    pub const UNDERBAR_VERTICAL_GAP: usize = 47;
    pub const UNDERBAR_RULE_THICKNESS: usize = 48;
    pub const UNDERBAR_EXTRA_DESCENDER: usize = 49;
    pub const RADICAL_VERTICAL_GAP: usize = 50;
    pub const RADICAL_DISPLAY_STYLE_VERTICAL_GAP: usize = 51;
    pub const RADICAL_RULE_THICKNESS: usize = 52;
    pub const RADICAL_EXTRA_ASCENDER: usize = 53;
    pub const RADICAL_KERN_BEFORE_DEGREE: usize = 54;
    pub const RADICAL_KERN_AFTER_DEGREE: usize = 55;
    pub const RADICAL_DEGREE_BOTTOM_RAISE_PERCENT: usize = 56;
    pub const MIN_CONNECTOR_OVERLAP: usize = 57;
    pub const SUBSCRIPT_SHIFT_DOWN_WITH_SUPERSCRIPT: usize = 58;
    pub const FRACTION_DELIMITER_SIZE: usize = 59;
    pub const FRACTION_DELIMITER_DISPLAY_STYLE_SIZE: usize = 60;
    pub const NO_LIMIT_SUB_FACTOR: usize = 61;
    pub const NO_LIMIT_SUP_FACTOR: usize = 62;
}

/// luatex `math_style_names`.
pub(crate) const MATH_STYLE_NAMES: [&str; 8] = [
    "display",
    "crampeddisplay",
    "text",
    "crampedtext",
    "script",
    "crampedscript",
    "scriptscript",
    "crampedscriptscript",
];

/// Names of the `\Umath` parameters in `math_param_error` (without the
/// `Umath` prefix).
fn param_name(id: u32) -> &'static str {
    let full = crate::uprim::UMATH_NAMES[id as usize];
    std::str::from_utf8(&full[5..]).unwrap_or("")
}

/// Dimensions and accent data of one character (luatex `char_*`). All zero
/// for a character the font lacks.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CharMetrics {
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub italic: i32,
    pub vert_italic: i32,
    /// `i32::MIN` when the font gives none
    pub top_accent: i32,
}

/// luatex `char_tag`: `list_tag` with its remainder, `ext_tag` or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CharTag {
    None,
    List(u32),
    Ext,
}

/// Result of a kern/ligature program lookup (`get_kern`/`get_ligature`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct KernLig {
    pub kern: Option<i32>,
    /// `(lig_type, replacement)`
    pub lig: Option<(u8, u32)>,
}

impl Engine {
    #[inline]
    pub(crate) fn is_luamath(&self) -> bool {
        self.engine_kind == EngineKind::LuaTeX
    }

    // ---------- parameters ----------

    /// luatex `inject_display_skip_before/after`: `\mathdisplayskipmode`
    /// (0 and 1 always, 2 when the glue is not zero, 3 never).
    pub(crate) fn display_skip_applies(&self, g: &crate::boxes::Glue) -> bool {
        if !self.is_luamath() {
            return true;
        }
        match self.eqtb.int_params[crate::prim::IntParam::MathDisplaySkipMode.idx() as usize] {
            2 => !(g.width == 0 && g.stretch == 0 && g.shrink == 0),
            3 => false,
            _ => true,
        }
    }

    /// `\mathflattenmode` (1 for the other engines, which flatten ord only).
    pub(crate) fn math_flatten_mode(&self) -> i32 {
        if self.is_luamath() {
            self.eqtb.int_params[crate::prim::IntParam::MathFlattenMode.idx() as usize]
        } else {
            1
        }
    }

    /// `get_math_param`: [`UNDEFINED_MATH_PARAMETER`] when unset.
    #[inline]
    pub(crate) fn mparam(&self, id: u32, g: GStyle) -> i32 {
        self.eqtb.math_param(id, g)
    }

    /// `get_math_param_or_error`.
    pub(crate) fn mparam_err(&mut self, id: u32, g: GStyle) -> i32 {
        let v = self.eqtb.math_param(id, g);
        if v == UNDEFINED_MATH_PARAMETER {
            self.math_param_error(id, g);
            0
        } else {
            v
        }
    }

    pub(crate) fn math_param_error(&mut self, id: u32, g: GStyle) {
        let msg = format!(
            "Math error: parameter \\Umath{}\\{}style is not set",
            param_name(id),
            MATH_STYLE_NAMES[usize::from(g & 7)]
        );
        self.error(&msg);
    }

    /// `get_math_quad_style`.
    pub(crate) fn math_quad_style(&mut self, g: GStyle) -> i32 {
        let v = self.eqtb.math_param(MATH_PARAM_QUAD, g);
        if v == UNDEFINED_MATH_PARAMETER {
            self.math_param_error(MATH_PARAM_QUAD, g);
            0
        } else {
            v
        }
    }

    /// `get_math_quad_size`: size 0/1/2 = text/script/scriptscript.
    pub(crate) fn math_quad_size(&self, size: usize) -> i32 {
        let g = match size {
            0 => 2,
            1 => 4,
            _ => 6,
        };
        self.eqtb.math_param(MATH_PARAM_QUAD, g)
    }

    /// `math_axis_size`.
    pub(crate) fn math_axis_size(&mut self, size: usize) -> i32 {
        let g = match size {
            0 => 2,
            1 => 4,
            _ => 6,
        };
        let v = self.eqtb.math_param(MATH_PARAM_AXIS, g);
        if v == UNDEFINED_MATH_PARAMETER {
            self.math_param_error(MATH_PARAM_AXIS, g);
            0
        } else {
            v
        }
    }

    /// `finalize_math_parameters`: the first conversion fixes
    /// `\Umathspaceafterscript` to `\scriptspace` while it is unset.
    pub(crate) fn finalize_math_parameters(&mut self) {
        if self.eqtb.math_param(MATH_PARAM_SPACE_AFTER_SCRIPT, 0) == UNDEFINED_MATH_PARAMETER {
            let ss = self.eqtb.dim_params[crate::prim::DimParam::ScriptSpace.idx() as usize];
            for style in 0..8u8 {
                self.eqtb.assign_math_param(MATH_PARAM_SPACE_AFTER_SCRIPT, style, ss, true);
            }
        }
    }

    // ---------- fonts ----------

    /// `fam_fnt(fam, size)`; size 0/1/2 = text/script/scriptscript.
    #[inline]
    pub(crate) fn fam_fnt(&self, fam: u32, size: usize) -> FontId {
        if fam > 255 {
            return 0;
        }
        self.eqtb.style_fonts[size.min(2)][fam as usize]
    }

    #[inline]
    pub(crate) fn mfont(&self, f: FontId) -> Option<&Rc<Font>> {
        self.eqtb.fonts.get(usize::from(f))
    }

    /// `font_math_params(f) > 0`.
    pub(crate) fn is_new_mathfont(&self, f: FontId) -> bool {
        self.mfont(f)
            .and_then(|font| font.lua.as_ref())
            .is_some_and(|lf| !lf.math_params.is_empty())
    }

    /// `assume_new_math`.
    pub(crate) fn assume_new_math(&self, f: FontId) -> bool {
        self.mfont(f)
            .and_then(|font| font.lua.as_ref())
            .is_some_and(|lf| !lf.math_params.is_empty() && !lf.oldmath)
    }

    /// `font_MATH_par(f, idx)`.
    pub(crate) fn font_math_par(&self, f: FontId, idx: usize) -> i32 {
        self.mfont(f)
            .and_then(|font| font.lua.as_ref())
            .and_then(|lf| {
                if lf.math_params.len() >= idx {
                    lf.math_params.get(idx - 1).copied()
                } else {
                    None
                }
            })
            .unwrap_or(UNDEFINED_MATH_PARAMETER)
    }

    /// `font_param(f, i)` (1-based) as `\fontdimen` sees it.
    pub(crate) fn font_param_of(&self, f: FontId, i: usize) -> i32 {
        self.eqtb
            .font_params
            .get(usize::from(f))
            .and_then(|v| v.get(i - 1).copied())
            .unwrap_or(0)
    }

    pub(crate) fn font_param_count(&self, f: FontId) -> usize {
        self.eqtb.font_params.get(usize::from(f)).map_or(0, Vec::len)
    }

    /// `char_exists`.
    pub(crate) fn mc_exists(&self, f: FontId, c: u32) -> bool {
        match self.mfont(f) {
            Some(font) => match &font.lua {
                Some(lf) => lf.char_exists(c),
                None => c < 256 && font.char_present(c as u8),
            },
            None => false,
        }
    }

    pub(crate) fn mc_metrics(&self, f: FontId, c: u32) -> CharMetrics {
        let Some(font) = self.mfont(f) else {
            return CharMetrics { top_accent: i32::MIN, ..Default::default() };
        };
        if let Some(lf) = &font.lua {
            return match lf.char_info(c) {
                Some(ci) => CharMetrics {
                    width: ci.width,
                    height: ci.height,
                    depth: ci.depth,
                    italic: ci.italic,
                    vert_italic: ci.vert_italic,
                    top_accent: ci.top_accent,
                },
                None => CharMetrics { top_accent: i32::MIN, ..Default::default() },
            };
        }
        if c < 256 {
            let c = c as u8;
            CharMetrics {
                width: font.char_width(c),
                height: font.char_height(c),
                depth: font.char_depth(c),
                italic: font.char_italic(c),
                vert_italic: font.char_italic(c),
                top_accent: i32::MIN,
            }
        } else {
            CharMetrics { top_accent: i32::MIN, ..Default::default() }
        }
    }

    /// `char_tag`/`char_remainder`.
    pub(crate) fn mc_tag(&self, f: FontId, c: u32) -> CharTag {
        let Some(font) = self.mfont(f) else {
            return CharTag::None;
        };
        if let Some(lf) = &font.lua {
            let Some(ci) = lf.char_info(c) else {
                return CharTag::None;
            };
            if ci.extensible.is_some() || !ci.hor_variants.is_empty() || !ci.vert_variants.is_empty() {
                return CharTag::Ext;
            }
            return match ci.next {
                Some(n) => CharTag::List(n),
                None => CharTag::None,
            };
        }
        match font.chars.get(c as usize).filter(|_| c < 256) {
            Some(ci) if ci.tag == TAG_LIST => CharTag::List(u32::from(ci.remainder)),
            Some(ci) if ci.tag == TAG_EXT => CharTag::Ext,
            _ => CharTag::None,
        }
    }

    /// `get_charinfo_vert_variants`/`hor_variants` (a TFM extensible recipe
    /// is squeezed into the vertical variants as `set_charinfo_extensible`
    /// does).
    pub(crate) fn mc_variants(&self, f: FontId, c: u32, horizontal: bool) -> Option<Vec<MathVariant>> {
        let font = self.mfont(f)?;
        if let Some(lf) = &font.lua {
            let ci = lf.char_info(c)?;
            if horizontal {
                return (!ci.hor_variants.is_empty()).then(|| ci.hor_variants.clone());
            }
            if !ci.vert_variants.is_empty() {
                return Some(ci.vert_variants.clone());
            }
            let e = ci.extensible?;
            return Some(extensible_variants(e.top, e.bot, e.mid, e.rep));
        }
        if horizontal || c >= 256 {
            return None;
        }
        let ci = font.chars.get(c as usize)?;
        if ci.tag != TAG_EXT {
            return None;
        }
        let r = font.ext.get(usize::from(ci.remainder))?;
        Some(extensible_variants(
            i32::from(r.top),
            i32::from(r.bot),
            i32::from(r.mid),
            i32::from(r.rep),
        ))
    }

    /// kern / ligature of the pair `(a, b)` in the lig/kern program of `f`.
    pub(crate) fn mc_kern_lig(&self, f: FontId, a: u32, b: u32) -> KernLig {
        let Some(font) = self.mfont(f) else {
            return KernLig::default();
        };
        if let Some(lf) = &font.lua {
            return KernLig {
                kern: lf.kern(a as i32, b as i32),
                lig: lf.lig(a as i32, b as i32).map(|l| (l.op, l.replacement)),
            };
        }
        if a >= 256 || b >= 256 {
            return KernLig::default();
        }
        let (a, b) = (a as u8, b as u8);
        let Some(ci) = font.chars.get(usize::from(a)) else {
            return KernLig::default();
        };
        if ci.tag != crate::tfm::TAG_LIG {
            return KernLig::default();
        }
        let mut k = usize::from(ci.remainder);
        if let Some(first) = font.lig_kern.get(k) {
            if first.skip > 128 {
                k = (usize::from(first.op) << 8) | usize::from(first.rem);
            }
        }
        for _ in 0..512 {
            let Some(step) = font.lig_kern.get(k) else {
                break;
            };
            if step.next_char == b && step.skip <= 128 {
                if step.op >= 128 {
                    let ki = ((usize::from(step.op) - 128) << 8) | usize::from(step.rem);
                    return KernLig { kern: font.kerns.get(ki).copied(), lig: None };
                }
                return KernLig { kern: None, lig: Some((step.op, u32::from(step.rem))) };
            }
            if step.skip >= 128 {
                break;
            }
            k += usize::from(step.skip) + 1;
        }
        KernLig::default()
    }

    /// `get_kern(f, c, skew_char(f))` for the TFM accent skew.
    pub(crate) fn mc_skew_kern(&self, f: FontId, c: u32) -> i32 {
        let skew = self.eqtb.skew_char.get(usize::from(f)).copied().unwrap_or(-1);
        if skew < 0 {
            return 0;
        }
        self.mc_kern_lig(f, c, skew as u32).kern.unwrap_or(0)
    }

    /// `space(f)`.
    pub(crate) fn font_space_of(&self, f: FontId) -> i32 {
        self.font_param_of(f, 2)
    }

    /// `x_height(f)`.
    pub(crate) fn x_height_of(&self, f: FontId) -> i32 {
        self.font_param_of(f, 5)
    }

    /// `accent_base_height`.
    pub(crate) fn accent_base_height(&self, f: FontId) -> i32 {
        if self.assume_new_math(f) {
            let a = self.font_math_par(f, mc::ACCENT_BASE_HEIGHT);
            if a != UNDEFINED_MATH_PARAMETER {
                return a;
            }
        }
        self.x_height_of(f)
    }

    /// The glyph node for `c` of `f` (`new_glyph`, whether or not the font
    /// has the character).
    pub(crate) fn glyph_node(&self, f: FontId, c: u32) -> crate::boxes::Node {
        match self.mfont(f) {
            Some(font) if font.lua.is_some() => {
                crate::boxes::Node::LuaGlyph(Box::new(crate::boxes::LuaGlyph {
                    c,
                    font: f,
                    lang: 0,
                    left: 0,
                    right: 0,
                    uchyph: 0,
                    xoffset: 0,
                    yoffset: 0,
                    expansion_factor: 0,
                    data: 0,
                    subtype: 0,
                    components: Vec::new(), attr: crate::boxes::Attr::NONE,
                }))
            }
            _ => crate::boxes::Node::Char { c: c as u8, font: f, attr: crate::boxes::Attr::NONE },
        }
    }

    // ---------- OpenType math kerns ----------

    /// `math_kern_at`: side 0 top_right, 1 top_left, 2 bottom_right,
    /// 3 bottom_left; `v` is the height.
    fn math_kern_at(&self, f: FontId, c: u32, side: u8, v: i32) -> i32 {
        let Some(ci) = self
            .mfont(f)
            .and_then(|font| font.lua.as_ref())
            .and_then(|lf| lf.char_info(c))
        else {
            return 0;
        };
        let arr = match side {
            0 => &ci.math_kerns.top_right,
            1 => &ci.math_kerns.top_left,
            2 => &ci.math_kerns.bottom_right,
            _ => &ci.math_kerns.bottom_left,
        };
        if arr.is_empty() {
            return 0;
        }
        if v < arr[0].0 {
            return arr[0].1;
        }
        let mut kern = 0;
        for &(h, k) in arr {
            kern = k;
            if h > v {
                return k;
            }
        }
        kern
    }

    /// `find_math_kern`: `None` = `MATH_KERN_NOT_FOUND`.
    pub(crate) fn find_math_kern(
        &self,
        lf: FontId,
        lc: u32,
        rf: FontId,
        rc: u32,
        sup: bool,
        shift: i32,
    ) -> Option<i32> {
        if !self.assume_new_math(lf)
            || !self.assume_new_math(rf)
            || !self.mc_exists(lf, lc)
            || !self.mc_exists(rf, rc)
        {
            return None;
        }
        let l = self.mc_metrics(lf, lc);
        let r = self.mc_metrics(rf, rc);
        if sup {
            let top = l.height;
            let bot = -r.depth + shift;
            let mut krn = self.math_kern_at(lf, lc, 0, top) + self.math_kern_at(rf, rc, 3, top);
            let k2 = self.math_kern_at(lf, lc, 0, bot) + self.math_kern_at(rf, rc, 3, bot);
            if k2 >= krn {
                krn = k2;
            }
            Some(krn)
        } else {
            let top = r.height - shift;
            let bot = -l.depth;
            let mut krn = self.math_kern_at(lf, lc, 2, top) + self.math_kern_at(rf, rc, 1, top);
            let k2 = self.math_kern_at(lf, lc, 2, bot) + self.math_kern_at(rf, rc, 1, bot);
            if k2 >= krn {
                krn = k2;
            }
            Some(krn)
        }
    }

    // ---------- fixup_math_parameters ----------

    fn def_text_sizes(&mut self, id: u32, size: usize, v: i32, global: bool) {
        let styles: &[u8] = match size {
            0 => &[2, 3],
            1 => &[4, 5],
            _ => &[6, 7],
        };
        for &s in styles {
            self.eqtb.assign_math_param(id, s, v, global);
        }
    }

    fn def_display(&mut self, id: u32, size: usize, v: i32, global: bool) {
        if size == 0 {
            for s in [0u8, 1] {
                self.eqtb.assign_math_param(id, s, v, global);
            }
        }
    }

    /// luatex `fixup_math_parameters`: assigning family font `f` of `fam`
    /// at `size` (0 text, 1 script, 2 scriptscript) defines the `\Umath`
    /// parameters that derive from the font, at the current group level
    /// (`global`: level one).
    pub(crate) fn fixup_math_parameters(&mut self, fam: usize, size: usize, f: u16, g: bool) {
        if !self.is_luamath() {
            return;
        }
        if self.is_new_mathfont(f) {
            self.fixup_new_font(f, size, g);
        } else if fam == 2 && self.is_old_mathfont(f, 22) {
            self.fixup_old_sy(size, g);
        } else if fam == 3 && self.is_old_mathfont(f, 13) {
            self.fixup_old_ex(f, size, g);
        }
    }

    fn is_old_mathfont(&self, f: FontId, n: usize) -> bool {
        !self.is_new_mathfont(f) && self.font_param_count(f) >= n
    }

    /// `DEFINE_MATH_PARAMETERS` + `DEFINE_DMATH_PARAMETERS` with one value.
    fn def_both(&mut self, id: u32, size: usize, v: i32, g: bool) {
        self.def_text_sizes(id, size, v, g);
        self.def_display(id, size, v, g);
    }

    /// `DEFINE_MATH_PARAMETERS(text)` and `DEFINE_DMATH_PARAMETERS(disp)`.
    fn def_split(&mut self, id: u32, size: usize, text: i32, disp: i32, g: bool) {
        self.def_text_sizes(id, size, text, g);
        self.def_display(id, size, disp, g);
    }

    fn fixup_new_font(&mut self, f: FontId, size: usize, g: bool) {
        let m = |e: &Self, i: usize| e.font_math_par(f, i);
        let at_size = self.mfont(f).map_or(0, |font| font.at_size);
        self.def_both(MATH_PARAM_QUAD, size, at_size, g);
        let v = m(self, mc::AXIS_HEIGHT);
        self.def_both(MATH_PARAM_AXIS, size, v, g);
        let v = m(self, mc::OVERBAR_EXTRA_ASCENDER);
        self.def_both(MATH_PARAM_OVERBAR_KERN, size, v, g);
        let v = m(self, mc::OVERBAR_RULE_THICKNESS);
        self.def_both(MATH_PARAM_OVERBAR_RULE, size, v, g);
        let v = m(self, mc::OVERBAR_VERTICAL_GAP);
        self.def_both(MATH_PARAM_OVERBAR_VGAP, size, v, g);
        let v = m(self, mc::UNDERBAR_EXTRA_DESCENDER);
        self.def_both(MATH_PARAM_UNDERBAR_KERN, size, v, g);
        let v = m(self, mc::UNDERBAR_RULE_THICKNESS);
        self.def_both(MATH_PARAM_UNDERBAR_RULE, size, v, g);
        let v = m(self, mc::UNDERBAR_VERTICAL_GAP);
        self.def_both(MATH_PARAM_UNDERBAR_VGAP, size, v, g);
        let v = m(self, mc::STRETCH_STACK_GAP_ABOVE_MIN);
        self.def_both(MATH_PARAM_UNDER_DELIMITER_VGAP, size, v, g);
        let v = m(self, mc::STRETCH_STACK_BOTTOM_SHIFT_DOWN);
        self.def_both(MATH_PARAM_UNDER_DELIMITER_BGAP, size, v, g);
        let v = m(self, mc::STRETCH_STACK_GAP_BELOW_MIN);
        self.def_both(MATH_PARAM_OVER_DELIMITER_VGAP, size, v, g);
        let v = m(self, mc::STRETCH_STACK_TOP_SHIFT_UP);
        self.def_both(MATH_PARAM_OVER_DELIMITER_BGAP, size, v, g);
        let (a, b) = (m(self, mc::STACK_TOP_SHIFT_UP), m(self, mc::STACK_TOP_DISPLAY_STYLE_SHIFT_UP));
        self.def_split(MATH_PARAM_STACK_NUM_UP, size, a, b, g);
        let (a, b) = (
            m(self, mc::STACK_BOTTOM_SHIFT_DOWN),
            m(self, mc::STACK_BOTTOM_DISPLAY_STYLE_SHIFT_DOWN),
        );
        self.def_split(MATH_PARAM_STACK_DENOM_DOWN, size, a, b, g);
        let (a, b) = (m(self, mc::STACK_GAP_MIN), m(self, mc::STACK_DISPLAY_STYLE_GAP_MIN));
        self.def_split(MATH_PARAM_STACK_VGAP, size, a, b, g);
        let v = m(self, mc::RADICAL_EXTRA_ASCENDER);
        self.def_both(MATH_PARAM_RADICAL_KERN, size, v, g);
        // operator size: display styles only
        let v = m(self, mc::DISPLAY_OPERATOR_MIN_HEIGHT);
        self.def_display(MATH_PARAM_OPERATOR_SIZE, size, v, g);
        let v = m(self, mc::RADICAL_RULE_THICKNESS);
        self.def_both(MATH_PARAM_RADICAL_RULE, size, v, g);
        let (a, b) = (m(self, mc::RADICAL_VERTICAL_GAP), m(self, mc::RADICAL_DISPLAY_STYLE_VERTICAL_GAP));
        self.def_split(MATH_PARAM_RADICAL_VGAP, size, a, b, g);
        let v = m(self, mc::RADICAL_KERN_BEFORE_DEGREE);
        self.def_both(MATH_PARAM_RADICAL_DEGREE_BEFORE, size, v, g);
        let v = m(self, mc::RADICAL_KERN_AFTER_DEGREE);
        self.def_both(MATH_PARAM_RADICAL_DEGREE_AFTER, size, v, g);
        let v = m(self, mc::RADICAL_DEGREE_BOTTOM_RAISE_PERCENT);
        self.def_both(MATH_PARAM_RADICAL_DEGREE_RAISE, size, v, g);

        let up = m(self, mc::SUPERSCRIPT_SHIFT_UP);
        let up_cr = m(self, mc::SUPERSCRIPT_SHIFT_UP_CRAMPED);
        let sup_styles: &[(u8, bool)] = match size {
            0 => &[(0, false), (1, true), (2, false), (3, true)],
            1 => &[(4, false), (5, true)],
            _ => &[(6, false), (7, true)],
        };
        for &(s, cramped) in sup_styles {
            self.eqtb.assign_math_param(MATH_PARAM_SUP_SHIFT_UP, s, if cramped { up_cr } else { up }, g);
        }
        let v = m(self, mc::SUBSCRIPT_BASELINE_DROP_MIN);
        self.def_both(MATH_PARAM_SUB_SHIFT_DROP, size, v, g);
        let v = m(self, mc::SUPERSCRIPT_BASELINE_DROP_MAX);
        self.def_both(MATH_PARAM_SUP_SHIFT_DROP, size, v, g);
        let v = m(self, mc::SUBSCRIPT_SHIFT_DOWN);
        self.def_both(MATH_PARAM_SUB_SHIFT_DOWN, size, v, g);
        let with_sup = m(self, mc::SUBSCRIPT_SHIFT_DOWN_WITH_SUPERSCRIPT);
        let v = if with_sup != UNDEFINED_MATH_PARAMETER { with_sup } else { m(self, mc::SUBSCRIPT_SHIFT_DOWN) };
        self.def_both(MATH_PARAM_SUB_SUP_SHIFT_DOWN, size, v, g);
        let v = m(self, mc::SUBSCRIPT_TOP_MAX);
        self.def_both(MATH_PARAM_SUB_TOP_MAX, size, v, g);
        let v = m(self, mc::SUPERSCRIPT_BOTTOM_MIN);
        self.def_both(MATH_PARAM_SUP_BOTTOM_MIN, size, v, g);
        let v = m(self, mc::SUPERSCRIPT_BOTTOM_MAX_WITH_SUBSCRIPT);
        self.def_both(MATH_PARAM_SUP_SUB_BOTTOM_MAX, size, v, g);
        let v = m(self, mc::SUB_SUPERSCRIPT_GAP_MIN);
        self.def_both(MATH_PARAM_SUBSUP_VGAP, size, v, g);
        let v = m(self, mc::UPPER_LIMIT_GAP_MIN);
        self.def_both(MATH_PARAM_LIMIT_ABOVE_VGAP, size, v, g);
        let v = m(self, mc::UPPER_LIMIT_BASELINE_RISE_MIN);
        self.def_both(MATH_PARAM_LIMIT_ABOVE_BGAP, size, v, g);
        self.def_both(MATH_PARAM_LIMIT_ABOVE_KERN, size, 0, g);
        let v = m(self, mc::LOWER_LIMIT_GAP_MIN);
        self.def_both(MATH_PARAM_LIMIT_BELOW_VGAP, size, v, g);
        let v = m(self, mc::LOWER_LIMIT_BASELINE_DROP_MIN);
        self.def_both(MATH_PARAM_LIMIT_BELOW_BGAP, size, v, g);
        self.def_both(MATH_PARAM_LIMIT_BELOW_KERN, size, 0, g);
        let v = m(self, mc::NO_LIMIT_SUB_FACTOR);
        self.def_both(MATH_PARAM_NOLIMIT_SUB_FACTOR, size, v, g);
        let v = m(self, mc::NO_LIMIT_SUP_FACTOR);
        self.def_both(MATH_PARAM_NOLIMIT_SUP_FACTOR, size, v, g);
        let v = m(self, mc::FRACTION_RULE_THICKNESS);
        self.def_both(MATH_PARAM_FRACTION_RULE, size, v, g);
        let (a, b) = (
            m(self, mc::FRACTION_NUMERATOR_GAP_MIN),
            m(self, mc::FRACTION_NUMERATOR_DISPLAY_STYLE_GAP_MIN),
        );
        self.def_split(MATH_PARAM_FRACTION_NUM_VGAP, size, a, b, g);
        let (a, b) = (
            m(self, mc::FRACTION_NUMERATOR_SHIFT_UP),
            m(self, mc::FRACTION_NUMERATOR_DISPLAY_STYLE_SHIFT_UP),
        );
        self.def_split(MATH_PARAM_FRACTION_NUM_UP, size, a, b, g);
        let (a, b) = (
            m(self, mc::FRACTION_DENOMINATOR_GAP_MIN),
            m(self, mc::FRACTION_DENOMINATOR_DISPLAY_STYLE_GAP_MIN),
        );
        self.def_split(MATH_PARAM_FRACTION_DENOM_VGAP, size, a, b, g);
        let (a, b) = (
            m(self, mc::FRACTION_DENOMINATOR_SHIFT_DOWN),
            m(self, mc::FRACTION_DENOMINATOR_DISPLAY_STYLE_SHIFT_DOWN),
        );
        self.def_split(MATH_PARAM_FRACTION_DENOM_DOWN, size, a, b, g);
        let (a, b) = (
            m(self, mc::FRACTION_DELIMITER_SIZE),
            m(self, mc::FRACTION_DELIMITER_DISPLAY_STYLE_SIZE),
        );
        self.def_split(MATH_PARAM_FRACTION_DEL_SIZE, size, a, b, g);
        let v = m(self, mc::SKEWED_FRACTION_HORIZONTAL_GAP);
        self.def_both(MATH_PARAM_SKEWED_FRACTION_HGAP, size, v, g);
        let v = m(self, mc::SKEWED_FRACTION_VERTICAL_GAP);
        self.def_both(MATH_PARAM_SKEWED_FRACTION_VGAP, size, v, g);
        let v = m(self, mc::SPACE_AFTER_SCRIPT);
        self.def_both(MATH_PARAM_SPACE_AFTER_SCRIPT, size, v, g);
        let v = m(self, mc::MIN_CONNECTOR_OVERLAP);
        self.def_both(MATH_PARAM_CONNECTOR_OVERLAP_MIN, size, v, g);
    }

    /// `mathsy(A, 5)` etc: `\fontdimen` of the current family 2 font of
    /// `size`.
    fn sy(&self, size: usize, i: usize) -> i32 {
        self.font_param_of(self.fam_fnt(2, size), i)
    }

    /// `mathex(A, B)`: `\fontdimen` of the current family 3 font.
    fn ex(&self, size: usize, i: usize) -> i32 {
        self.font_param_of(self.fam_fnt(3, size), i)
    }

    fn fixup_old_sy(&mut self, size: usize, g: bool) {
        let s = |e: &Self, i| e.sy(size, i);
        let quad = s(self, 6);
        self.def_both(MATH_PARAM_QUAD, size, quad, g);
        let axis = s(self, 22);
        self.def_both(MATH_PARAM_AXIS, size, axis, g);
        self.def_split(MATH_PARAM_STACK_NUM_UP, size, s(self, 10), s(self, 8), g);
        self.def_split(MATH_PARAM_STACK_DENOM_DOWN, size, s(self, 12), s(self, 11), g);
        self.def_split(MATH_PARAM_FRACTION_NUM_UP, size, s(self, 9), s(self, 8), g);
        self.def_split(MATH_PARAM_FRACTION_DENOM_DOWN, size, s(self, 12), s(self, 11), g);
        self.def_split(MATH_PARAM_FRACTION_DEL_SIZE, size, s(self, 21), s(self, 20), g);
        self.def_both(MATH_PARAM_SKEWED_FRACTION_HGAP, size, 0, g);
        self.def_both(MATH_PARAM_SKEWED_FRACTION_VGAP, size, 0, g);
        let (sup1, sup2, sup3) = (s(self, 13), s(self, 14), s(self, 15));
        let (sub_drop, sup_drop) = (s(self, 19), s(self, 18));
        match size {
            0 => {
                for (st, v) in [(0u8, sup1), (1, sup3), (2, sup2), (3, sup3)] {
                    self.eqtb.assign_math_param(MATH_PARAM_SUP_SHIFT_UP, st, v, g);
                }
            }
            1 => {
                for st in [0u8, 1, 2, 3] {
                    self.eqtb.assign_math_param(MATH_PARAM_SUB_SHIFT_DROP, st, sub_drop, g);
                    self.eqtb.assign_math_param(MATH_PARAM_SUP_SHIFT_DROP, st, sup_drop, g);
                }
                self.eqtb.assign_math_param(MATH_PARAM_SUP_SHIFT_UP, 4, sup2, g);
                self.eqtb.assign_math_param(MATH_PARAM_SUP_SHIFT_UP, 5, sup3, g);
            }
            _ => {
                for st in [4u8, 5, 6, 7] {
                    self.eqtb.assign_math_param(MATH_PARAM_SUB_SHIFT_DROP, st, sub_drop, g);
                    self.eqtb.assign_math_param(MATH_PARAM_SUP_SHIFT_DROP, st, sup_drop, g);
                }
                self.eqtb.assign_math_param(MATH_PARAM_SUP_SHIFT_UP, 6, sup2, g);
                self.eqtb.assign_math_param(MATH_PARAM_SUP_SHIFT_UP, 7, sup3, g);
            }
        }
        self.def_both(MATH_PARAM_SUB_SHIFT_DOWN, size, s(self, 16), g);
        self.def_both(MATH_PARAM_SUB_SUP_SHIFT_DOWN, size, s(self, 17), g);
        let xh = s(self, 5);
        self.def_both(MATH_PARAM_SUB_TOP_MAX, size, (xh * 4).abs() / 5, g);
        self.def_both(MATH_PARAM_SUP_BOTTOM_MIN, size, xh.abs() / 4, g);
        self.def_both(MATH_PARAM_SUP_SUB_BOTTOM_MAX, size, (xh * 4).abs() / 5, g);
        let rule = self.ex(size, 8);
        self.def_display(MATH_PARAM_RADICAL_VGAP, size, rule + xh.abs() / 4, g);
        self.def_both(MATH_PARAM_RADICAL_DEGREE_RAISE, size, 60, g);
        let q = self.math_quad_size(size);
        let before = xn_over_d(q, 5, 18);
        self.def_both(MATH_PARAM_RADICAL_DEGREE_BEFORE, size, before, g);
        let after = -xn_over_d(q, 10, 18);
        self.def_both(MATH_PARAM_RADICAL_DEGREE_AFTER, size, after, g);
    }

    fn fixup_old_ex(&mut self, f: FontId, size: usize, g: bool) {
        let rule = self.ex(size, 8);
        let xh = self.sy(size, 5);
        self.def_both(MATH_PARAM_OVERBAR_KERN, size, rule, g);
        self.def_both(MATH_PARAM_OVERBAR_RULE, size, rule, g);
        self.def_both(MATH_PARAM_OVERBAR_VGAP, size, 3 * rule, g);
        self.def_both(MATH_PARAM_UNDERBAR_KERN, size, rule, g);
        self.def_both(MATH_PARAM_UNDERBAR_RULE, size, rule, g);
        self.def_both(MATH_PARAM_UNDERBAR_VGAP, size, 3 * rule, g);
        self.def_both(MATH_PARAM_RADICAL_KERN, size, rule, g);
        self.def_text_sizes(MATH_PARAM_RADICAL_VGAP, size, rule + rule.abs() / 4, g);
        self.def_split(MATH_PARAM_STACK_VGAP, size, 3 * rule, 7 * rule, g);
        self.def_both(MATH_PARAM_FRACTION_RULE, size, rule, g);
        self.def_split(MATH_PARAM_FRACTION_NUM_VGAP, size, rule, 3 * rule, g);
        self.def_split(MATH_PARAM_FRACTION_DENOM_VGAP, size, rule, 3 * rule, g);
        let (bos1, bos2, bos3, bos4, bos5) =
            (self.ex(size, 9), self.ex(size, 10), self.ex(size, 11), self.ex(size, 12), self.ex(size, 13));
        self.def_both(MATH_PARAM_LIMIT_ABOVE_VGAP, size, bos1, g);
        self.def_both(MATH_PARAM_LIMIT_ABOVE_BGAP, size, bos3, g);
        self.def_both(MATH_PARAM_LIMIT_ABOVE_KERN, size, bos5, g);
        self.def_both(MATH_PARAM_LIMIT_BELOW_VGAP, size, bos2, g);
        self.def_both(MATH_PARAM_LIMIT_BELOW_BGAP, size, bos4, g);
        self.def_both(MATH_PARAM_LIMIT_BELOW_KERN, size, bos5, g);
        let v = self.font_math_par(f, mc::NO_LIMIT_SUB_FACTOR);
        self.def_both(MATH_PARAM_NOLIMIT_SUB_FACTOR, size, v, g);
        let v = self.font_math_par(f, mc::NO_LIMIT_SUP_FACTOR);
        self.def_both(MATH_PARAM_NOLIMIT_SUP_FACTOR, size, v, g);
        self.def_both(MATH_PARAM_SUBSUP_VGAP, size, 4 * rule, g);
        self.def_both(MATH_PARAM_CONNECTOR_OVERLAP_MIN, size, 0, g);
        self.def_both(MATH_PARAM_UNDER_DELIMITER_VGAP, size, bos2, g);
        self.def_both(MATH_PARAM_UNDER_DELIMITER_BGAP, size, bos4, g);
        self.def_both(MATH_PARAM_OVER_DELIMITER_VGAP, size, bos1, g);
        self.def_both(MATH_PARAM_OVER_DELIMITER_BGAP, size, bos3, g);
        self.def_display(MATH_PARAM_RADICAL_VGAP, size, rule + xh.abs() / 4, g);
    }
}

/// luatex `xn_over_d` for non-negative `n`/`d`.
pub(crate) fn xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let neg = x < 0;
    let x = i64::from(x).abs();
    let r = (x * i64::from(n) / i64::from(d)) as i32;
    if neg {
        -r
    } else {
        r
    }
}

/// `set_charinfo_extensible`: the TFM/`extensible` recipe as variants
/// (bottom first; `extender` 1 = repeatable).
fn extensible_variants(top: i32, bot: i32, mid: i32, rep: i32) -> Vec<MathVariant> {
    let v = |glyph: i32, extender: i32| MathVariant { glyph, extender, start: 0, end: 0, advance: 0 };
    let mut out = Vec::new();
    if bot == 0 && top == 0 && mid == 0 && rep != 0 {
        out.push(v(rep, 0));
        out.push(v(rep, 1));
        return out;
    }
    if bot != 0 {
        out.push(v(bot, 0));
    }
    if rep != 0 {
        out.push(v(rep, 1));
    }
    if mid != 0 {
        out.push(v(mid, 0));
        if rep != 0 {
            out.push(v(rep, 1));
        }
    }
    if top != 0 {
        out.push(v(top, 0));
    }
    out
}

// ---------- boxes, delimiters and extensibles (mlist.c) ----------

use crate::boxes::{hpack, vpack, Glue, Node, NodeList, HBOX, VBOX};

/// Extra results of [`Engine::do_delimiter`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DelimInfo {
    /// a built-up (extensible) delimiter
    pub stack: bool,
    /// the italic correction (`*delta`)
    pub delta: i32,
    /// luatex `*same`
    pub same: u8,
}

/// `subtype(b) = st` for a box.
pub(crate) fn with_list_subtype(mut b: Node, st: u8) -> Node {
    if let Node::Box { subtype, .. } = &mut b {
        *subtype = st;
    }
    b
}

/// `new_null_box` of the given kind.
pub(crate) fn null_box(kind: u8) -> Node {
    Node::Box {
        kind,
        w: 0,
        h: 0,
        d: 0,
        shift: 0,
        list: Vec::new(),
        glue_sign: 0,
        glue_order: 0,
        glue_set: 0.0,
        lr: 0,
        dir: 0, attr: crate::boxes::Attr::NONE, subtype: 0,
    }
}

/// `half(x)`.
#[inline]
pub(crate) fn half(x: i32) -> i32 {
    if x % 2 != 0 {
        (x + 1) / 2
    } else {
        x / 2
    }
}

pub(crate) fn box_whd(n: &Node) -> (i32, i32, i32) {
    match n {
        Node::Box { w, h, d, .. } => (*w, *h, *d),
        _ => (0, 0, 0),
    }
}

pub(crate) fn set_shift(n: &mut Node, s: i32) {
    if let Node::Box { shift, .. } = n {
        *shift = s;
    }
}

pub(crate) fn box_shift(n: &Node) -> i32 {
    match n {
        Node::Box { shift, .. } => *shift,
        _ => 0,
    }
}

/// luatex `reset_attributes` over a freshly built box tree: every node of
/// `n` takes attribute list `a`.
pub(crate) fn stamp_attr(n: &mut Node, a: crate::boxes::Attr) {
    if a == crate::boxes::Attr::NONE {
        return;
    }
    n.set_attr(a);
    if let Node::Box { list, .. } = n {
        for c in list.iter_mut() {
            stamp_attr(c, a);
        }
    }
}

impl Engine {
    /// `char_box`: a box with one glyph whose width includes the italic
    /// correction.
    pub(crate) fn char_box(&self, f: FontId, c: u32) -> Node {
        let m = self.mc_metrics(f, c);
        let mut b = null_box(HBOX);
        if let Node::Box { w, h, d, list, subtype, .. } = &mut b {
            *subtype = crate::boxes::list_subtype::MATH_CHAR;
            *w = m.width + m.italic;
            *h = m.height;
            *d = m.depth;
            list.push(self.glyph_node(f, c));
        }
        b
    }

    fn stack_into_box(&self, b: &mut Node, f: FontId, c: u32) -> i32 {
        let p = self.char_box(f, c);
        let (pw, ph, pd) = box_whd(&p);
        match b {
            Node::Box { kind, w, h, d, list, .. } if *kind == VBOX => {
                list.insert(0, p);
                *h = ph;
                if *w < pw {
                    *w = pw;
                }
                let _ = d;
                let m = self.mc_metrics(f, c);
                m.height + m.depth
            }
            Node::Box { h, d, list, .. } => {
                list.push(p);
                if *h < ph {
                    *h = ph;
                }
                if *d < pd {
                    *d = pd;
                }
                self.mc_metrics(f, c).width
            }
            _ => 0,
        }
    }

    fn stack_glue_into_box(b: &mut Node, min: i32, max: i32) {
        let g = Node::Glue(Glue::spec(min, max - min, 0, 0, 0), crate::boxes::Attr::NONE);
        if let Node::Box { kind, list, .. } = b {
            if *kind == VBOX {
                list.insert(0, g);
            } else {
                list.push(g);
            }
        }
    }

    /// mlist.c `get_delim_box`: the `make_extensible` callback may build the
    /// box (it receives the delimiter's attribute list); no result means the
    /// default construction, and anything but a box is a fatal error.
    pub(crate) fn get_delim_box(
        &mut self,
        fnt: FontId,
        chr: u32,
        v: i32,
        min_overlap: i32,
        horizontal: bool,
        att: crate::boxes::Attr,
    ) -> Node {
        use crate::lua_callbacks::{Cb, CbArg, CbRet};
        if self.is_luamath() && self.cb_defined(Cb::MakeExtensible) {
            let att_list = self.lua_attr_handle(att);
            let args = vec![
                CbArg::Int(i64::from(fnt)),
                CbArg::Int(i64::from(chr)),
                CbArg::Int(i64::from(v)),
                CbArg::Int(i64::from(min_overlap)),
                CbArg::Bool(horizontal),
                if att_list == 0 { CbArg::Nil } else { CbArg::Node(att_list) },
            ];
            if let Some(CbRet::Node(h)) = self.lua_cb_call(Cb::MakeExtensible, "make_extensible", args).as_deref().and_then(|r| r.first()) {
                let h = u32::try_from(*h).unwrap_or(0);
                if matches!(self.lua_nodes.id(h), crate::lua_node::HLIST | crate::lua_node::VLIST) {
                    if let Some(b @ Node::Box { .. }) = self.lua_nodes_to_engine(i64::from(h)).into_iter().next() {
                        return b;
                    }
                }
                self.fatal_error(&format!(
                    "error:  (fonts): invalid extensible character {chr} created for font {fnt}, [h|v]list expected"
                ));
                return null_box(HBOX);
            }
        }
        self.make_extensible(fnt, chr, v, min_overlap, horizontal, att)
    }

    /// luatex `make_extensible`.
    pub(crate) fn make_extensible(
        &mut self,
        fnt: FontId,
        chr: u32,
        v: i32,
        min_overlap: i32,
        horizontal: bool,
        att: crate::boxes::Attr,
    ) -> Node {
        let mut b = with_list_subtype(
            null_box(if horizontal { HBOX } else { VBOX }),
            if horizontal { crate::boxes::list_subtype::H_EXTENSIBLE } else { crate::boxes::list_subtype::V_EXTENSIBLE },
        );
        let mut min_overlap = min_overlap.max(0);
        let mut ext = self.mc_variants(fnt, chr, horizontal).unwrap_or_default();
        let mut num_extenders = 0i32;
        let mut num_normal = 0;
        for cur in ext.iter_mut() {
            if !self.mc_exists(fnt, cur.glyph as u32) {
                self.error("Variant part doesn't exist.");
                if let Node::Box { w, .. } = &mut b {
                    *w = self.eqtb.dim_params[crate::prim::DimParam::NullDelimiterSpace.idx() as usize];
                }
                stamp_attr(&mut b, att);
                return b;
            }
            if cur.extender > 0 {
                num_extenders += 1;
            } else {
                num_normal += 1;
            }
            if cur.start < 0 || cur.end < 0 || cur.advance < 0 {
                self.error("Extensible recipe has negative fields.");
                cur.start = cur.start.max(0);
                cur.end = cur.end.max(0);
                cur.advance = cur.advance.max(0);
            }
        }
        if num_normal == 0 {
            self.error("Extensible recipe has no fixed parts.");
            if let Some(first) = ext.first_mut() {
                first.extender = 0;
            }
            num_normal = 1;
            num_extenders -= 1;
        }
        let _ = num_normal;
        let adv = |e: &Self, cur: &MathVariant| -> i32 {
            let mut a = cur.advance;
            if a == 0 {
                let m = e.mc_metrics(fnt, cur.glyph as u32);
                a = if horizontal { m.width } else { m.height + m.depth };
            }
            a
        };
        let mut with_extenders: i32 = -1;
        let mut b_max: i32 = 0;
        while b_max < v && num_extenders > 0 {
            b_max = 0;
            let mut prev_overlap = 0;
            with_extenders += 1;
            for cur in &ext {
                let reps = if cur.extender == 0 { 1 } else { with_extenders };
                for _ in 0..reps {
                    let mut c = cur.start;
                    if min_overlap < c {
                        c = min_overlap;
                    }
                    if prev_overlap < c {
                        c = prev_overlap;
                    }
                    let a = adv(self, cur);
                    b_max = b_max.wrapping_add(a - c);
                    prev_overlap = cur.end;
                }
            }
        }
        // assemble
        let mut prev_overlap = 0;
        b_max = 0;
        let mut s_max = 0;
        for cur in &ext {
            let reps = if cur.extender == 0 { 1 } else { with_extenders.max(0) };
            for _ in 0..reps {
                let mut c = cur.start;
                if prev_overlap < c {
                    c = prev_overlap;
                }
                let d = c;
                if min_overlap < c {
                    c = min_overlap;
                }
                if d > 0 {
                    Self::stack_glue_into_box(&mut b, -d, -c);
                    s_max += (-c) - (-d);
                    b_max -= d;
                }
                b_max += self.stack_into_box(&mut b, fnt, cur.glyph as u32);
                prev_overlap = cur.end;
            }
        }
        min_overlap = 0;
        let _ = min_overlap;
        if v > b_max && s_max > 0 {
            let mut d = v - b_max;
            if d > s_max {
                d = s_max;
            }
            if let Node::Box { glue_order, glue_sign, glue_set, .. } = &mut b {
                *glue_order = 0;
                *glue_sign = 1;
                *glue_set = f64::from(d as f32 / s_max as f32);
            }
            b_max += d;
        }
        if let Node::Box { w, h, .. } = &mut b {
            if horizontal {
                *w = b_max;
            } else {
                *h = b_max;
            }
        }
        stamp_attr(&mut b, att);
        b
    }

    /// luatex `do_delimiter`: the smallest variant of the delimiter `d`
    /// `(small fam, small char, large fam, large char)` of height plus depth
    /// at least `v` (width with `flat`), in size `s` (0 text .. 2
    /// scriptscript). `shift` centres it on the axis.
    pub(crate) fn do_delimiter(
        &mut self,
        d: Option<(u8, u32, u8, u32)>,
        s: usize,
        v: i32,
        flat: bool,
        cur_style: GStyle,
        shift: bool,
        same_in: u8,
        att: crate::boxes::Attr,
    ) -> (Node, DelimInfo) {
        let mut info = DelimInfo::default();
        if let Some((0, 0, 0, 0)) = d {
            let mut b = with_list_subtype(null_box(HBOX), crate::boxes::list_subtype::V_DELIMITER);
            if !flat {
                if let Node::Box { w, .. } = &mut b {
                    *w = self.eqtb.dim_params[crate::prim::DimParam::NullDelimiterSpace.idx() as usize];
                }
            }
            stamp_attr(&mut b, att);
            return (b, info);
        }
        let mut f: FontId = 0;
        let mut c: u32 = 0;
        let mut w = 0i32;
        let mut x_start = 0u32;
        let mut do_parts = false;
        let emas = same_in;
        let mode = self.eqtb.int_params[crate::prim::IntParam::MathDelimitersMode.idx() as usize];
        if let Some((sf, sc, lf, lc)) = d {
            let mut z = u32::from(sf);
            let mut x = sc;
            let mut i = 0;
            let mut large_attempt = false;
            'found: loop {
                x_start = x;
                if z != 0 || x != 0 {
                    let g = self.fam_fnt(z, s);
                    if g != 0 {
                        let mut y = x;
                        loop {
                            i += 1;
                            if self.mc_exists(g, y) {
                                let m = self.mc_metrics(g, y);
                                let u = if flat { m.width } else { m.height + m.depth };
                                let tag = self.mc_tag(g, y);
                                if u > w {
                                    f = g;
                                    c = y;
                                    w = u;
                                    if u >= v && (self.is_new_mathfont(g) || tag != CharTag::Ext) {
                                        break 'found;
                                    }
                                }
                                if tag == CharTag::Ext {
                                    f = g;
                                    c = y;
                                    do_parts = true;
                                    break 'found;
                                }
                                if i > 10000 {
                                    let name = self.mfont(g).map_or(String::new(), |ff| ff.tfm_name.clone());
                                    self.error(&format!(
                                        "Math error: endless loop in charlist (U+{y:04x} in {name})"
                                    ));
                                    break 'found;
                                }
                                if let CharTag::List(r) = tag {
                                    y = r;
                                    continue;
                                }
                            }
                            break;
                        }
                    }
                }
                if large_attempt {
                    break 'found;
                }
                large_attempt = true;
                z = u32::from(lf);
                x = lc;
            }
        }
        let mut b;
        let mut parts_done = false;
        if f != 0 {
            let variants = if do_parts { self.mc_variants(f, c, flat) } else { None };
            if variants.is_some() {
                parts_done = true;
                let ov = self.mparam_err(MATH_PARAM_CONNECTOR_OVERLAP_MIN, cur_style);
                b = self.get_delim_box(f, c, v, ov, flat, att);
                let m = self.mc_metrics(f, x_start);
                info.delta = if self.assume_new_math(f) { m.vert_italic } else { m.italic };
                info.stack = true;
            } else {
                if info_same_char(x_start, c) {
                    info.same = emas;
                }
                b = self.char_box(f, c);
                stamp_attr(&mut b, att);
                info.delta = self.mc_metrics(f, c).italic;
                info.stack = false;
            }
        } else {
            b = with_list_subtype(null_box(HBOX), if flat { crate::boxes::list_subtype::H_DELIMITER } else { crate::boxes::list_subtype::V_DELIMITER });
            if !flat {
                if let Node::Box { w, .. } = &mut b {
                    *w = self.eqtb.dim_params[crate::prim::DimParam::NullDelimiterSpace.idx() as usize];
                }
            }
            stamp_attr(&mut b, att);
            info.stack = false;
        }
        if !flat {
            let samenos = mode & 0x08 != 0;
            let charnos = mode & 0x10 != 0;
            let noshift = mode & 0x01 != 0;
            let skip = (emas != 0 && samenos) || (!parts_done && charnos) || noshift;
            if !skip {
                let (_, h, dd) = box_whd(&b);
                let mut sh = half(h - dd);
                if shift {
                    sh -= self.math_axis_size(s);
                }
                set_shift(&mut b, sh);
            }
        }
        (b, info)
    }
}

#[inline]
fn info_same_char(x: u32, c: u32) -> bool {
    x == c
}

/// TeX's `hpack` of a list at natural size, as a box.
pub(crate) fn hpack_nat(e: &Engine, list: NodeList) -> Node {
    hpack(list, None, HBOX, &e.eqtb).node
}

/// `vpackage(list, 0, additional, max_dimen)`.
pub(crate) fn vpack_nat(e: &Engine, list: NodeList) -> Node {
    vpack(list, None, VBOX, &e.eqtb).node
}

impl Engine {
    /// luatex `initialize_math_spacing`: the TeX math spacing table as
    /// `\Umath<l><r>spacing` values (1 thin, 2 medium, 3 thick `\muskip`
    /// parameter; unset spacing is none).
    pub(crate) fn initialize_math_spacing(&mut self) {
        const TEXT: [&[u8; 8]; 8] = [
            b"01230001", b"11030001", b"22002002", b"33003003", b"00000000", b"01230001", b"11011111",
            b"11231011",
        ];
        const SCRIPT: [&[u8; 8]; 8] = [
            b"01000000", b"11000000", b"00000000", b"00000000", b"00000000", b"01000000", b"00000000",
            b"01000000",
        ];
        for l in 0..8usize {
            for r in 0..8usize {
                let id = MATH_PARAM_ORD_ORD_SPACING + (l * 8 + r) as u32;
                for style in 0..8u8 {
                    let code = if style < 4 { TEXT[l][r] } else { SCRIPT[l][r] } - b'0';
                    if code != 0 {
                        self.eqtb.assign_math_glue_param(id, style, [i32::from(code), 0, 0, 0, 0, 0], true);
                    }
                }
            }
        }
    }
}
