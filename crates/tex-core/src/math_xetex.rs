//! XeTeX math with OpenType MATH fonts (xetex.web "Typesetting math
//! formulas" with the `is_new_mathfont` branches, and XeTeXOTMath.cpp).
//!
//! A native font is a *new math font* when it carries a MATH table
//! (`isOpenTypeMathFont`). The glyph services below reproduce XeTeX's
//! arithmetic exactly: font units become points with single precision
//! (`XeTeXFontInst::unitsToPoints`) and points become 16.16 fixed values with
//! `D2Fix`; HarfBuzz' `hb_ot_math_*` entry points are re-implemented on top of
//! `ttf_parser::math` (the hb font scale is the units per em, so every value is
//! a raw design unit).
//!
//! Everything here serves `EngineKind::XeTeX` only.

use std::rc::Rc;

use ttf_parser::GlyphId;

use crate::boxes::{Glue, Node, NodeList, HBOX, VBOX};
use crate::engine::{Engine, EngineKind};
use crate::font_program::FontProgram;
use crate::tfm::FontId;

/// `lastMathConstant + 1` (xetex.web "total OpenType math constants").
pub(crate) const NUM_CONSTANTS: usize = 56;

/// OpenType MATH constant indices (`hb_ot_math_constant_t`, xetex.web 16445).
pub(crate) mod k {
    pub const DELIMITED_SUB_FORMULA_MIN_HEIGHT: usize = 2;
    pub const DISPLAY_OPERATOR_MIN_HEIGHT: usize = 3;
    pub const AXIS_HEIGHT: usize = 5;
    pub const ACCENT_BASE_HEIGHT: usize = 6;
    pub const SUBSCRIPT_SHIFT_DOWN: usize = 8;
    pub const SUBSCRIPT_TOP_MAX: usize = 9;
    pub const SUBSCRIPT_BASELINE_DROP_MIN: usize = 10;
    pub const SUPERSCRIPT_SHIFT_UP: usize = 11;
    pub const SUPERSCRIPT_SHIFT_UP_CRAMPED: usize = 12;
    pub const SUPERSCRIPT_BOTTOM_MIN: usize = 13;
    pub const SUPERSCRIPT_BASELINE_DROP_MAX: usize = 14;
    pub const SUB_SUPERSCRIPT_GAP_MIN: usize = 15;
    pub const SUPERSCRIPT_BOTTOM_MAX_WITH_SUBSCRIPT: usize = 16;
    pub const UPPER_LIMIT_GAP_MIN: usize = 18;
    pub const UPPER_LIMIT_BASELINE_RISE_MIN: usize = 19;
    pub const LOWER_LIMIT_GAP_MIN: usize = 20;
    pub const LOWER_LIMIT_BASELINE_DROP_MIN: usize = 21;
    pub const STACK_TOP_SHIFT_UP: usize = 22;
    pub const STACK_GAP_MIN: usize = 26;
    pub const STACK_DISPLAY_STYLE_GAP_MIN: usize = 27;
    pub const FRACTION_NUMERATOR_SHIFT_UP: usize = 32;
    pub const FRACTION_NUMERATOR_DISPLAY_STYLE_SHIFT_UP: usize = 33;
    pub const FRACTION_DENOMINATOR_SHIFT_DOWN: usize = 34;
    pub const FRACTION_DENOMINATOR_DISPLAY_STYLE_SHIFT_DOWN: usize = 35;
    pub const FRACTION_NUMERATOR_GAP_MIN: usize = 36;
    pub const FRACTION_NUM_DISPLAY_STYLE_GAP_MIN: usize = 37;
    pub const FRACTION_RULE_THICKNESS: usize = 38;
    pub const FRACTION_DENOMINATOR_GAP_MIN: usize = 39;
    pub const FRACTION_DENOM_DISPLAY_STYLE_GAP_MIN: usize = 40;
    pub const RADICAL_VERTICAL_GAP: usize = 49;
    pub const RADICAL_DISPLAY_STYLE_VERTICAL_GAP: usize = 50;
    pub const RADICAL_RULE_THICKNESS: usize = 51;
}

/// `TeX_sym_to_OT_map` (XeTeXOTMath.cpp); `usize::MAX` = unknown.
const SYM_TO_OT: [usize; 23] = [
    usize::MAX,
    usize::MAX,
    usize::MAX,
    usize::MAX,
    usize::MAX,
    k::ACCENT_BASE_HEIGHT,
    usize::MAX,
    usize::MAX,
    k::FRACTION_NUMERATOR_DISPLAY_STYLE_SHIFT_UP,
    k::FRACTION_NUMERATOR_SHIFT_UP,
    k::STACK_TOP_SHIFT_UP,
    k::FRACTION_DENOMINATOR_DISPLAY_STYLE_SHIFT_DOWN,
    k::FRACTION_DENOMINATOR_SHIFT_DOWN,
    k::SUPERSCRIPT_SHIFT_UP,
    k::SUPERSCRIPT_SHIFT_UP,
    k::SUPERSCRIPT_SHIFT_UP_CRAMPED,
    k::SUBSCRIPT_SHIFT_DOWN,
    k::SUBSCRIPT_SHIFT_DOWN,
    k::SUPERSCRIPT_BASELINE_DROP_MAX,
    k::SUBSCRIPT_BASELINE_DROP_MIN,
    k::DELIMITED_SUB_FORMULA_MIN_HEIGHT,
    usize::MAX,
    k::AXIS_HEIGHT,
];

/// `TeX_ext_to_OT_map`.
const EXT_TO_OT: [usize; 14] = [
    usize::MAX,
    usize::MAX,
    usize::MAX,
    usize::MAX,
    usize::MAX,
    k::ACCENT_BASE_HEIGHT,
    usize::MAX,
    usize::MAX,
    k::FRACTION_RULE_THICKNESS,
    k::UPPER_LIMIT_GAP_MIN,
    k::LOWER_LIMIT_GAP_MIN,
    k::UPPER_LIMIT_BASELINE_RISE_MIN,
    k::LOWER_LIMIT_BASELINE_DROP_MIN,
    k::STACK_GAP_MIN,
];

/// xetex.web `D2Fix`: `(int)(d * 65536.0 + 0.5)`.
#[inline]
fn d2fix(d: f64) -> i32 {
    (d * 65536.0 + 0.5) as i32
}

/// One part of a glyph assembly with its lengths already in sp.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Part {
    pub glyph: u16,
    pub start: i32,
    pub end: i32,
    pub full: i32,
    pub extender: bool,
}

/// A native font seen as an OpenType math font at one size.
pub(crate) struct OtFont {
    program: Rc<FontProgram>,
    upem: f32,
    point_size: f32,
    /// `isOpenTypeMathFont`
    pub has_math: bool,
}

impl OtFont {
    fn new(program: Rc<FontProgram>, size_sp: i32) -> OtFont {
        let has_math = program.face().map(|f| f.tables().math.is_some()).unwrap_or(false);
        OtFont {
            upem: f32::from(program.units_per_em.max(1)),
            // `XeTeXFontInst(..., Fix2D(pointSize))` stores a float
            point_size: (f64::from(size_sp) / 65536.0) as f32,
            program,
            has_math,
        }
    }

    /// `D2Fix(font->unitsToPoints(units))`
    #[inline]
    pub fn fix(&self, units: i32) -> i32 {
        d2fix(f64::from(self.units_to_points(units as f32)))
    }

    /// `XeTeXFontInst::unitsToPoints`
    #[inline]
    fn units_to_points(&self, units: f32) -> f32 {
        (units * self.point_size) / self.upem
    }

    /// `XeTeXFontInst::pointsToUnits`
    #[inline]
    fn points_to_units(&self, points: f32) -> f32 {
        (points * self.upem) / self.point_size
    }

    /// `font_size[f]` as a float (`getPointSize`)
    #[inline]
    fn point_size(&self) -> f32 {
        self.point_size
    }

    /// raw (design unit) MathConstants record; percentages stay percentages
    fn raw_constant(&self, n: usize) -> i32 {
        let Ok(face) = self.program.face() else { return 0 };
        let Some(c) = face.tables().math.and_then(|m| m.constants) else { return 0 };
        match n {
            0 => i32::from(c.script_percent_scale_down()),
            1 => i32::from(c.script_script_percent_scale_down()),
            2 => i32::from(c.delimited_sub_formula_min_height()),
            3 => i32::from(c.display_operator_min_height()),
            4 => i32::from(c.math_leading().value),
            5 => i32::from(c.axis_height().value),
            6 => i32::from(c.accent_base_height().value),
            7 => i32::from(c.flattened_accent_base_height().value),
            8 => i32::from(c.subscript_shift_down().value),
            9 => i32::from(c.subscript_top_max().value),
            10 => i32::from(c.subscript_baseline_drop_min().value),
            11 => i32::from(c.superscript_shift_up().value),
            12 => i32::from(c.superscript_shift_up_cramped().value),
            13 => i32::from(c.superscript_bottom_min().value),
            14 => i32::from(c.superscript_baseline_drop_max().value),
            15 => i32::from(c.sub_superscript_gap_min().value),
            16 => i32::from(c.superscript_bottom_max_with_subscript().value),
            17 => i32::from(c.space_after_script().value),
            18 => i32::from(c.upper_limit_gap_min().value),
            19 => i32::from(c.upper_limit_baseline_rise_min().value),
            20 => i32::from(c.lower_limit_gap_min().value),
            21 => i32::from(c.lower_limit_baseline_drop_min().value),
            22 => i32::from(c.stack_top_shift_up().value),
            23 => i32::from(c.stack_top_display_style_shift_up().value),
            24 => i32::from(c.stack_bottom_shift_down().value),
            25 => i32::from(c.stack_bottom_display_style_shift_down().value),
            26 => i32::from(c.stack_gap_min().value),
            27 => i32::from(c.stack_display_style_gap_min().value),
            28 => i32::from(c.stretch_stack_top_shift_up().value),
            29 => i32::from(c.stretch_stack_bottom_shift_down().value),
            30 => i32::from(c.stretch_stack_gap_above_min().value),
            31 => i32::from(c.stretch_stack_gap_below_min().value),
            32 => i32::from(c.fraction_numerator_shift_up().value),
            33 => i32::from(c.fraction_numerator_display_style_shift_up().value),
            34 => i32::from(c.fraction_denominator_shift_down().value),
            35 => i32::from(c.fraction_denominator_display_style_shift_down().value),
            36 => i32::from(c.fraction_numerator_gap_min().value),
            37 => i32::from(c.fraction_num_display_style_gap_min().value),
            38 => i32::from(c.fraction_rule_thickness().value),
            39 => i32::from(c.fraction_denominator_gap_min().value),
            40 => i32::from(c.fraction_denom_display_style_gap_min().value),
            41 => i32::from(c.skewed_fraction_horizontal_gap().value),
            42 => i32::from(c.skewed_fraction_vertical_gap().value),
            43 => i32::from(c.overbar_vertical_gap().value),
            44 => i32::from(c.overbar_rule_thickness().value),
            45 => i32::from(c.overbar_extra_ascender().value),
            46 => i32::from(c.underbar_vertical_gap().value),
            47 => i32::from(c.underbar_rule_thickness().value),
            48 => i32::from(c.underbar_extra_descender().value),
            49 => i32::from(c.radical_vertical_gap().value),
            50 => i32::from(c.radical_display_style_vertical_gap().value),
            51 => i32::from(c.radical_rule_thickness().value),
            52 => i32::from(c.radical_extra_ascender().value),
            53 => i32::from(c.radical_kern_before_degree().value),
            54 => i32::from(c.radical_kern_after_degree().value),
            55 => i32::from(c.radical_degree_bottom_raise_percent()),
            _ => 0,
        }
    }

    /// `get_ot_math_constant`: sp, except for the three percentages
    pub fn constant(&self, n: usize) -> i32 {
        if !self.has_math || n >= NUM_CONSTANTS {
            return 0;
        }
        let raw = self.raw_constant(n);
        match n {
            0 | 1 | 55 => raw,
            _ => self.fix(raw),
        }
    }

    /// `char -> glyph` through the cmap (`mapchartoglyph`)
    pub fn glyph_of_char(&self, c: u32) -> u16 {
        if c > 0x10ffff || (0xd800..=0xdfff).contains(&c) {
            return 0;
        }
        let Some(ch) = char::from_u32(c) else { return 0 };
        self.program
            .face()
            .ok()
            .and_then(|f| f.glyph_index(ch))
            .map_or(0, |g| g.0)
    }

    /// What PDF text extraction reports for a glyph that no character
    /// produced (size variants, assembly parts): the Unicode value of its
    /// glyph name without the `.v1`-style suffix, else a character the cmap
    /// maps to it (xdvipdfmx's ToUnicode for the embedded subset).
    fn glyph_text(&self, gid: u16) -> String {
        let Ok(face) = self.program.face() else { return String::new() };
        let id = ttf_parser::GlyphId(gid);
        if let Some(t) = face.glyph_name(id).and_then(crate::pdf_fonts::glyph_to_unicode) {
            return t;
        }
        let mut found: Option<u32> = None;
        if let Some(cmap) = face.tables().cmap {
            for st in cmap.subtables {
                if st.is_unicode() {
                    st.codepoints(|cp| {
                        if found.is_none() && st.glyph_index(cp) == Some(id) {
                            found = Some(cp);
                        }
                    });
                }
            }
        }
        found.and_then(char::from_u32).map(String::from).unwrap_or_default()
    }

    /// `getGlyphWidth`
    pub fn glyph_width(&self, gid: u16) -> i32 {
        let adv = self
            .program
            .face()
            .ok()
            .and_then(|f| f.glyph_hor_advance(GlyphId(gid)))
            .unwrap_or(0);
        self.fix(i32::from(adv))
    }

    /// unscaled `(yMax, yMin)` of the glyph's control box (`FT_Glyph_Get_CBox`)
    fn glyph_y_extent(&self, gid: u16) -> (i32, i32) {
        self.program
            .face()
            .ok()
            .and_then(|f| f.glyph_bounding_box(GlyphId(gid)))
            .map_or((0, 0), |r| (i32::from(r.y_max), i32::from(r.y_min)))
    }

    /// `getGlyphHeightDepth` in sp (`D2Fix(ht)`, `D2Fix(dp)`)
    pub fn glyph_height_depth(&self, gid: u16) -> (i32, i32) {
        let (ymax, ymin) = self.glyph_y_extent(gid);
        let ht = self.units_to_points(ymax as f32);
        let dp = -self.units_to_points(ymin as f32);
        (d2fix(f64::from(ht)), d2fix(f64::from(dp)))
    }

    /// `get_ot_math_ital_corr`
    pub fn italic_correction(&self, gid: u16) -> i32 {
        let v = self
            .program
            .face()
            .ok()
            .and_then(|f| f.tables().math)
            .and_then(|m| m.glyph_info)
            .and_then(|g| g.italic_corrections)
            .and_then(|t| t.get(GlyphId(gid)))
            .map_or(0, |v| i32::from(v.value));
        self.fix(v)
    }

    /// `get_ot_math_accent_pos`: HarfBuzz returns half the advance for a
    /// glyph the TopAccentAttachment table does not cover
    pub fn accent_pos(&self, gid: u16) -> i32 {
        let Ok(face) = self.program.face() else { return 0 };
        let v = face
            .tables()
            .math
            .and_then(|m| m.glyph_info)
            .and_then(|g| g.top_accent_attachments)
            .and_then(|t| t.get(GlyphId(gid)))
            .map_or_else(
                || i32::from(face.glyph_hor_advance(GlyphId(gid)).unwrap_or(0)) / 2,
                |v| i32::from(v.value),
            );
        self.fix(v)
    }

    /// `get_ot_math_variant`: `(glyph, advance)`; advance is -1 and the glyph
    /// is `gid` itself when there is no variant number `v`
    pub fn variant(&self, gid: u16, v: usize, horiz: bool) -> (u16, i32) {
        let found = self.program.face().ok().and_then(|f| {
            let variants = f.tables().math?.variants?;
            let constructions = if horiz {
                variants.horizontal_constructions
            } else {
                variants.vertical_constructions
            };
            let cons = constructions.get(GlyphId(gid))?;
            let idx = u16::try_from(v).ok()?;
            cons.variants.get(idx).map(|gv| (gv.variant_glyph.0, i32::from(gv.advance_measurement)))
        });
        match found {
            Some((g, adv)) => (g, self.fix(adv)),
            None => (gid, -1),
        }
    }

    /// `get_ot_assembly_ptr`
    pub fn assembly(&self, gid: u16, horiz: bool) -> Option<Vec<Part>> {
        let face = self.program.face().ok()?;
        let variants = face.tables().math?.variants?;
        let constructions = if horiz {
            variants.horizontal_constructions
        } else {
            variants.vertical_constructions
        };
        let asm = constructions.get(GlyphId(gid))?.assembly?;
        if asm.parts.len() == 0 {
            return None;
        }
        Some(
            asm.parts
                .into_iter()
                .map(|p| Part {
                    glyph: p.glyph_id.0,
                    start: self.fix(i32::from(p.start_connector_length)),
                    end: self.fix(i32::from(p.end_connector_length)),
                    full: self.fix(i32::from(p.full_advance)),
                    extender: p.part_flags.extender(),
                })
                .collect(),
        )
    }

    /// `ot_min_connector_overlap`
    pub fn min_connector_overlap(&self) -> i32 {
        let v = self
            .program
            .face()
            .ok()
            .and_then(|f| f.tables().math)
            .and_then(|m| m.variants)
            .map_or(0, |v| i32::from(v.min_connector_overlap));
        self.fix(v)
    }

    /// `getMathKernAt`; `side`: 0 top right, 1 top left, 2 bottom right,
    /// 3 bottom left (`hb_ot_math_kern_t`)
    fn kern_at(&self, gid: u16, height: i32, side: u8) -> i32 {
        let Ok(face) = self.program.face() else { return 0 };
        let Some(info) = face
            .tables()
            .math
            .and_then(|m| m.glyph_info)
            .and_then(|g| g.kern_infos)
            .and_then(|ki| ki.get(GlyphId(gid)))
        else {
            return 0;
        };
        let table = match side {
            0 => info.top_right,
            1 => info.top_left,
            2 => info.bottom_right,
            _ => info.bottom_left,
        };
        let Some(table) = table else { return 0 };
        let mut i = 0u16;
        let mut count = table.count();
        while count > 0 {
            let half = count / 2;
            let h = table.height(i + half).map_or(0, |v| i32::from(v.value));
            if h < height {
                i += half + 1;
                count -= half + 1;
            } else {
                count = half;
            }
        }
        table.kern(i).map_or(0, |v| i32::from(v.value))
    }

    /// `glyph_height` / `glyph_depth` of XeTeXOTMath.cpp in points
    fn glyph_height_pts(&self, gid: u16) -> f32 {
        self.units_to_points(self.glyph_y_extent(gid).0 as f32)
    }
    fn glyph_depth_pts(&self, gid: u16) -> f32 {
        -self.units_to_points(self.glyph_y_extent(gid).1 as f32)
    }
}

/// `get_ot_math_kern`: kern between glyph `g` of `f` and the first glyph
/// `sg` of the script font `sf` (`sup_cmd` = false, `sub_cmd` = true);
/// `shift_scaled` is the script shift in sp.
pub(crate) fn ot_math_kern(f: &OtFont, g: u16, sf: &OtFont, sg: u16, sub: bool, shift_scaled: i32) -> i32 {
    let g_height = f.points_to_units(f.glyph_height_pts(g)) as i32;
    let g_depth = f.points_to_units(f.glyph_depth_pts(g)) as i32;
    let sg_height = sf.points_to_units(sf.glyph_height_pts(sg)) as i32;
    let sg_depth = sf.points_to_units(sf.glyph_depth_pts(sg)) as i32;
    let shift = f.points_to_units((f64::from(shift_scaled) / 65536.0) as f32) as i32;
    let scale_factor = sf.point_size() / f.point_size();
    let rval;
    if !sub {
        let kern = f.kern_at(g, (shift as f32 - scale_factor * sg_depth as f32) as i32, 0);
        let skern = sf.kern_at(sg, -sg_depth, 3);
        let top_kern = (kern as f32 + scale_factor * skern as f32) as i32;
        let kern = f.kern_at(g, g_height, 0);
        let skern = sf.kern_at(sg, ((g_height - shift) as f32 / scale_factor) as i32, 3);
        let bot_kern = (kern as f32 + scale_factor * skern as f32) as i32;
        rval = if top_kern > bot_kern { top_kern } else { bot_kern };
    } else {
        let kern = f.kern_at(g, (scale_factor * sg_height as f32 - shift as f32) as i32, 2);
        let skern = sf.kern_at(sg, sg_height, 1);
        let top_kern = (kern as f32 + scale_factor * skern as f32) as i32;
        let kern = f.kern_at(g, -g_depth, 2);
        let skern = sf.kern_at(sg, ((shift - g_depth) as f32 / scale_factor) as i32, 1);
        let bot_kern = (kern as f32 + scale_factor * skern as f32) as i32;
        rval = if top_kern > bot_kern { top_kern } else { bot_kern };
    }
    f.fix(rval)
}

/// Per-engine XeTeX math state.
#[derive(Default)]
pub(crate) struct XeMathState {
    /// xetex.web global `cur_f`: the font of the last `fetch`
    pub cur_f: std::cell::Cell<FontId>,
    fonts: std::cell::RefCell<crate::FxHashMap<FontId, Rc<OtFont>>>,
    glyphs: std::cell::RefCell<crate::FxHashMap<(FontId, u32), u16>>,
    texts: std::cell::RefCell<crate::FxHashMap<(FontId, u16), Rc<str>>>,
}

/// `OtFont` data for the font program of a native font at `size_sp` -- used
/// by the native font loader to fill `\fontdimen` 10.. of a math font
/// (xetex.web `load_native_font`).
pub fn ot_math_constants(program: &Rc<FontProgram>, size_sp: i32) -> Option<[i32; NUM_CONSTANTS]> {
    let f = OtFont::new(program.clone(), size_sp);
    if !f.has_math {
        return None;
    }
    let mut out = [0; NUM_CONSTANTS];
    for (n, slot) in out.iter_mut().enumerate() {
        *slot = f.constant(n);
    }
    Some(out)
}

/// xetex.web `half`
#[inline]
fn half(x: i32) -> i32 {
    if x % 2 == 0 {
        x / 2
    } else {
        (x + 1) / 2
    }
}

impl Engine {
    #[inline]
    pub(crate) fn is_xetex_math(&self) -> bool {
        self.engine_kind == EngineKind::XeTeX
    }

    /// The native font `fid` as an OpenType font (`None` for TFM fonts).
    pub(crate) fn xe_ot(&self, fid: FontId) -> Option<Rc<OtFont>> {
        let program = self.eqtb.fonts.get(fid as usize)?.native.as_ref()?.program.clone();
        if let Some(o) = self.xe_math.fonts.borrow().get(&fid) {
            if Rc::ptr_eq(&o.program, &program) {
                return Some(o.clone());
            }
        }
        let size = self.eqtb.fonts.get(fid as usize)?.at_size;
        let ot = Rc::new(OtFont::new(program, size));
        self.xe_math.fonts.borrow_mut().insert(fid, ot.clone());
        self.xe_math.glyphs.borrow_mut().retain(|(f, _), _| *f != fid);
        Some(ot)
    }

    /// xetex.web `is_new_mathfont`
    pub(crate) fn xe_is_new_mathfont(&self, fid: FontId) -> bool {
        self.xe_ot(fid).is_some_and(|o| o.has_math)
    }

    /// `is_new_mathfont(cur_f)`
    pub(crate) fn xe_cur_f_is_math(&self) -> bool {
        self.xe_is_new_mathfont(self.xe_math.cur_f.get())
    }

    /// The font of family `fam` at the size of `style`.
    #[inline]
    pub(crate) fn xe_fam_fnt(&self, style: u8, fam: u8) -> FontId {
        self.eqtb.style_fonts[crate::math::font_size(style)][fam as usize]
    }

    /// Note `fid` as the font of the last `fetch` (TeX's `cur_f`).
    #[inline]
    pub(crate) fn xe_note_fetch(&self, fid: FontId) {
        if self.engine_kind == EngineKind::XeTeX {
            self.xe_math.cur_f.set(fid);
        }
    }

    /// `get_native_mathsy_param(f, n)` for the new math font `fid`
    pub(crate) fn xe_mathsy(&self, fid: FontId, n: usize) -> i32 {
        let Some(o) = self.xe_ot(fid) else { return 0 };
        let size = self.eqtb.fonts.get(fid as usize).map_or(0, |f| f.at_size);
        if n == 6 {
            size
        } else if n == 21 {
            // delim2: 1.5em clamped to delim1
            let d1 = self.xe_mathsy(fid, 20);
            ((1.5 * f64::from(size)) as i32).min(d1)
        } else if n < SYM_TO_OT.len() && SYM_TO_OT[n] != usize::MAX {
            o.constant(SYM_TO_OT[n])
        } else {
            0
        }
    }

    /// `get_native_mathex_param(f, n)`
    pub(crate) fn xe_mathex(&self, fid: FontId, n: usize) -> i32 {
        let Some(o) = self.xe_ot(fid) else { return 0 };
        if n == 6 {
            self.eqtb.fonts.get(fid as usize).map_or(0, |f| f.at_size)
        } else if n < EXT_TO_OT.len() && EXT_TO_OT[n] != usize::MAX {
            o.constant(EXT_TO_OT[n])
        } else {
            0
        }
    }

    /// The glyph `get_native_glyph(new_native_character(f, c), 0)`: the first
    /// glyph of the character shaped with the font's features.
    pub(crate) fn xe_char_glyph(&self, fid: FontId, c: u32) -> u16 {
        if let Some(&g) = self.xe_math.glyphs.borrow().get(&(fid, c)) {
            return g;
        }
        let shaped = char::from_u32(c).and_then(|ch| {
            self.shape_native_slice(fid, &ch.to_string()).ok().and_then(|nodes| {
                nodes.iter().find_map(|n| match n {
                    Node::NativeGlyphRun { run, start, .. } => run.glyphs.get(*start).map(|g| g.glyph_id),
                    _ => None,
                })
            })
        });
        let gid = match shaped {
            Some(g) => g,
            None => self.xe_ot(fid).map_or(0, |o| o.glyph_of_char(c)),
        };
        self.xe_math.glyphs.borrow_mut().insert((fid, c), gid);
        gid
    }

    /// A glyph node (xetex.web `glyph_node` with `set_native_glyph_metrics(p, 1)`).
    /// A glyph node is a one-glyph run with an empty cluster; `text` is only
    /// what PDF text extraction (ToUnicode) reports for the glyph.
    pub(crate) fn xe_glyph_node(&self, fid: FontId, gid: u16, text: &str) -> Node {
        let ot = self.xe_ot(fid);
        let (w, h, d) = match &ot {
            Some(o) => {
                let (h, d) = o.glyph_height_depth(gid);
                (o.glyph_width(gid), h, d)
            }
            None => (0, 0, 0),
        };
        let text: Rc<str> = match (&ot, text.is_empty()) {
            (Some(o), true) => self
                .xe_math
                .texts
                .borrow_mut()
                .entry((fid, gid))
                .or_insert_with(|| Rc::from(o.glyph_text(gid)))
                .clone(),
            _ => Rc::from(text),
        };
        let run = Rc::new(crate::native_layout::NativeRun {
            actual_text: false,
            font: fid,
            text,
            glyphs: vec![crate::native_layout::NativeGlyph {
                glyph_id: gid,
                cluster_start: 0,
                cluster_end: 0,
                x_advance: w,
                y_advance: 0,
                x_offset: 0,
                y_offset: 0,
            }],
        });
        Node::NativeGlyphRun { run, start: 0, end: 1, width: w, height: h, depth: d }
    }

    /// `(font, glyph)` of a glyph node made by [`Self::xe_glyph_node`]
    pub(crate) fn xe_glyph_of(n: &Node) -> Option<(FontId, u16)> {
        match n {
            Node::NativeGlyphRun { run, start, end, .. } if *end == *start + 1 => {
                run.glyphs.get(*start).map(|g| (run.font, g.glyph_id))
            }
            _ => None,
        }
    }

    fn xe_new_box(kind: u8) -> Node {
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
            dir: 0,
            subtype: 0,
            attr: crate::boxes::Attr::NONE,
        }
    }

    /// xetex.web `build_opentype_assembly`: a box of at least size `s`
    /// (height for a vertical, width for a horizontal assembly) made from the
    /// glyph assembly `parts` of the font `fid`.
    pub(crate) fn xe_build_assembly(&self, fid: FontId, parts: &[Part], s: i32, horiz: bool) -> Node {
        let min_o = self.xe_ot(fid).map_or(0, |o| o.min_connector_overlap());
        let mut n: i32 = -1;
        let mut no_extenders = true;
        loop {
            n += 1;
            let mut s_max: i32 = 0;
            let mut prev_o: i32 = 0;
            for p in parts {
                if p.extender {
                    no_extenders = false;
                    for _ in 1..=n {
                        let mut o = p.start;
                        if min_o < o {
                            o = min_o;
                        }
                        if prev_o < o {
                            o = prev_o;
                        }
                        s_max = s_max.wrapping_sub(o).wrapping_add(p.full);
                        prev_o = p.end;
                    }
                } else {
                    let mut o = p.start;
                    if min_o < o {
                        o = min_o;
                    }
                    if prev_o < o {
                        o = prev_o;
                    }
                    s_max = s_max.wrapping_sub(o).wrapping_add(p.full);
                    prev_o = p.end;
                }
            }
            if s_max >= s || no_extenders {
                break;
            }
        }

        let mut b = Self::xe_new_box(if horiz { HBOX } else { VBOX });
        let mut list: NodeList = Vec::new();
        let (mut bh, mut bd, mut bw) = (0i32, 0i32, 0i32);
        let mut prev_o = 0i32;
        let mut push_part = |this: &Self, p: &Part, prev_o: &mut i32, list: &mut NodeList| {
            let mut o = p.start;
            if *prev_o < o {
                o = *prev_o;
            }
            let oo = o;
            if min_o < o {
                o = min_o;
            }
            if oo > 0 {
                // stack_glue_into_box(b, -oo, -o)
                let glue = Node::Glue(Glue::spec(-oo, -o - (-oo), 0, 0, 0), crate::boxes::Attr::NONE);
                if horiz {
                    list.push(glue);
                } else {
                    list.insert(0, glue);
                }
            }
            // stack_glyph_into_box
            let glyph = this.xe_glyph_node(fid, p.glyph, "");
            let (gw, gh, gd) = match &glyph {
                Node::NativeGlyphRun { width, height, depth, .. } => (*width, *height, *depth),
                _ => (0, 0, 0),
            };
            if horiz {
                if list.is_empty() {
                    list.push(glyph);
                } else {
                    list.push(glyph);
                    if bh < gh {
                        bh = gh;
                    }
                    if bd < gd {
                        bd = gd;
                    }
                }
            } else {
                list.insert(0, glyph);
                bh = gh;
                if bw < gw {
                    bw = gw;
                }
            }
            *prev_o = p.end;
        };
        for p in parts {
            if p.extender {
                for _ in 1..=n {
                    push_part(self, p, &mut prev_o, &mut list);
                }
            } else {
                push_part(self, p, &mut prev_o, &mut list);
            }
        }

        // natural size and total stretch
        let mut nat: i32 = 0;
        let mut stretch: i32 = 0;
        for item in &list {
            match item {
                Node::NativeGlyphRun { width, height, depth, .. } => {
                    if horiz {
                        nat += *width;
                    } else {
                        nat += *height + *depth;
                    }
                }
                Node::Glue(g, _) => {
                    nat += g.width;
                    stretch += g.stretch;
                }
                _ => {}
            }
        }
        let size;
        let (mut sign, mut set) = (0u8, 0.0f64);
        if s > nat && stretch > 0 {
            let mut o = s - nat;
            if o > stretch {
                o = stretch;
            }
            sign = 1;
            set = f64::from(o) / f64::from(stretch);
            let r = f64::from(stretch) * set;
            size = nat + (if r < 0.0 { (r - 0.5).ceil() } else { (r + 0.5).floor() }) as i32;
        } else {
            size = nat;
        }
        if let Node::Box { w, h, d, list: l, glue_sign, glue_set, .. } = &mut b {
            *l = list;
            *glue_sign = sign;
            *glue_set = set;
            if horiz {
                *w = size;
                *h = bh;
                *d = bd;
            } else {
                *w = bw;
                *h = size;
                *d = 0;
            }
        }
        b
    }

    /// Box dimensions `(w, h, d)` of a node
    pub(crate) fn xe_dims(n: &Node) -> (i32, i32, i32) {
        match n {
            Node::Box { w, h, d, .. } => (*w, *h, *d),
            Node::NativeGlyphRun { width, height, depth, .. } => (*width, *height, *depth),
            _ => (0, 0, 0),
        }
    }

    /// Character nodes for a math character in a native font (xetex.web
    /// "Create a character node `p` for `nucleus(q)`"): the glyph and its
    /// italic-correction kern; returns the nodes and `delta`.
    pub(crate) fn xe_native_char(
        &self,
        fid: FontId,
        c: u32,
        math_text_char: bool,
        sub_present: bool,
    ) -> (NodeList, i32) {
        let gid = self.xe_char_glyph(fid, c);
        let text = char::from_u32(c).map(String::from).unwrap_or_default();
        let glyph = self.xe_glyph_node(fid, gid, &text);
        let new_math = self.xe_is_new_mathfont(fid);
        let mut delta = self.xe_ot(fid).map_or(0, |o| o.italic_correction(gid));
        if math_text_char && !new_math {
            delta = 0;
        }
        let mut out = vec![glyph];
        if !sub_present && delta != 0 {
            out.push(Node::Kern(delta, self.eqtb.cur_attr));
            delta = 0;
        }
        (out, delta)
    }

    /// `clean_box` of a native math character: `hpack` of its character
    /// nodes (the italic kern stays: a glyph is not a char node).
    pub(crate) fn xe_clean_native_char(&self, fid: FontId, c: u32) -> Node {
        let (list, _) = self.xe_native_char(fid, c, false, false);
        crate::boxes::hpack(list, None, HBOX, &self.eqtb).node
    }

    /// xetex.web `make_op` for a native-font character nucleus. Returns the
    /// axis-centred box and the italic correction `delta`.
    pub(crate) fn xe_op_char_box(
        &self,
        fid: FontId,
        c: u32,
        style: u8,
        sub_present: bool,
        limits: bool,
    ) -> (Node, i32) {
        self.xe_math.cur_f.set(fid);
        let text = char::from_u32(c).map(String::from).unwrap_or_default();
        let mut x = self.xe_clean_native_char(fid, c);
        let mut delta = 0i32;
        if self.xe_is_new_mathfont(fid) {
            let first = match &x {
                Node::Box { list, .. } => list.first().cloned(),
                _ => None,
            };
            if let Some(p) = first.filter(|p| Self::xe_glyph_of(p).is_some()) {
                let ot = self.xe_ot(fid).unwrap();
                let (_, ph, pd) = Self::xe_dims(&p);
                let mut p = p;
                let mut found = false;
                if style < 2 {
                    let mut h1 = ot.constant(k::DISPLAY_OPERATOR_MIN_HEIGHT);
                    let want = (i64::from(ph + pd) * 5 / 4) as i32;
                    if h1 < want {
                        h1 = want;
                    }
                    let (_, mut cg) = Self::xe_glyph_of(&p).unwrap();
                    let base = cg;
                    let mut n = 0usize;
                    let mut h2;
                    loop {
                        let (g, adv) = ot.variant(base, n, false);
                        h2 = adv;
                        if h2 > 0 {
                            cg = g;
                            p = self.xe_glyph_node(fid, cg, &text);
                        }
                        n += 1;
                        if h2 < 0 || h2 >= h1 || n > 4096 {
                            break;
                        }
                    }
                    if h2 < 0 {
                        if let Some(parts) = ot.assembly(base, false) {
                            p = self.xe_build_assembly(fid, &parts, h1, false);
                            delta = 0;
                            found = true;
                        }
                    }
                }
                if !found {
                    let (_, g) = Self::xe_glyph_of(&p).unwrap();
                    delta = ot.italic_correction(g);
                }
                // `found:` width(x) := width(p) ... ; list_ptr(x) := p when replaced
                let (pw, ph, pd) = Self::xe_dims(&p);
                if let Node::Box { w, h, d, list, .. } = &mut x {
                    if found {
                        *list = vec![p.clone()];
                    } else {
                        list[0] = p.clone();
                    }
                    *w = pw;
                    *h = ph;
                    *d = pd;
                }
            }
        }
        if sub_present && !limits {
            if let Node::Box { w, .. } = &mut x {
                *w -= delta;
            }
        }
        let axis = self.axis_height(style);
        if let Node::Box { w: _, h, d, shift, .. } = &mut x {
            *shift = half(*h - *d) - axis;
        }
        (x, delta)
    }
}

impl Engine {
    /// `mathsy`/`mathex` of family 2/3 at size index `size_idx` when that
    /// family member is a new math font (xetex.web `define_mathsy_body`).
    pub(crate) fn xe_family_param(&self, size_idx: usize, fam: u8, i: usize) -> Option<i32> {
        if fam != 2 && fam != 3 {
            return None;
        }
        let fid = self.eqtb.style_fonts[size_idx][fam as usize];
        if fid == 0 || !self.xe_is_new_mathfont(fid) {
            return None;
        }
        Some(if fam == 2 { self.xe_mathsy(fid, i) } else { self.xe_mathex(fid, i) })
    }
}

impl Engine {
    /// XeTeX `fetch` + conversion of a math character that is not a plain
    /// TFM character: native fonts (glyph nodes) and TFM characters above 255
    /// (no truncation to a byte). `None` leaves the character to the TFM code.
    pub(crate) fn xe_convert_math_char(
        &mut self,
        fam: u8,
        c: u32,
        class: u8,
        style: u8,
        math_text_char: bool,
        origin: &crate::boxes::MathDiagnosticOrigin,
    ) -> Option<NodeList> {
        if self.engine_kind != EngineKind::XeTeX {
            return None;
        }
        let fid = self.xe_fam_fnt(style, fam);
        self.xe_math.cur_f.set(fid);
        if self.xe_ot(fid).is_some() {
            if class == crate::math::CL_OP {
                let (b, _) = self.xe_op_char_box(fid, c, style, false, false);
                return Some(vec![b]);
            }
            let (list, _) = self.xe_native_char(fid, c, math_text_char, false);
            return Some(list);
        }
        if c > 255 {
            self.xe_missing_math_char(fid, c, origin);
            return Some(Vec::new());
        }
        None
    }

    /// `char_warning` for a TFM font that lacks character `c` (> 255)
    pub(crate) fn xe_missing_math_char(
        &mut self,
        fid: FontId,
        c: u32,
        origin: &crate::boxes::MathDiagnosticOrigin,
    ) {
        let _ = (fid, c, origin);
        if self.eqtb.int_params[crate::prim::IntParam::TracingLostChars.idx() as usize] > 0 {
            let name = self
                .eqtb
                .fonts
                .get(fid as usize)
                .map(|f| f.tfm_name.clone())
                .unwrap_or_default();
            let ch = char::from_u32(c).unwrap_or('?');
            self.warning_at(&format!("Missing character: There is no {ch} in font {name}!"), None);
        }
    }
}

impl Engine {
    /// `get_ot_math_constant(f, n)` for the native font `f`
    pub(crate) fn xe_const(&self, fid: FontId, n: usize) -> i32 {
        self.xe_ot(fid).map_or(0, |o| o.constant(n))
    }

    /// xetex.web "Fetch first character of a sub/superscript": the glyph of
    /// the first character-like thing of a script, in a new math font.
    pub(crate) fn xe_first_script_glyph(&self, script: &[Node], cur_style: u8) -> Option<(FontId, u16)> {
        let this_style = crate::math::sub_style(cur_style);
        self.xe_walk_script(script, this_style, 0)
    }

    fn xe_walk_script(&self, list: &[Node], mut this_style: u8, depth: u32) -> Option<(FontId, u16)> {
        if depth > 16 {
            return None;
        }
        let mut i = 0;
        while i < list.len() {
            match &list[i] {
                Node::Kern(..)
                | Node::ExplicitKern(..)
                | Node::MathKern(..)
                | Node::Glue(..)
                | Node::MuGlue(..)
                | Node::NonScript => {}
                Node::Style(s, _) => this_style = crate::math::gstyle_of(*s),
                Node::Choice => {
                    // display, text, script, scriptscript branches follow
                    let branch = usize::from(this_style / 2).min(3);
                    return match list.get(i + 1 + branch) {
                        Some(Node::ChoiceAlt { body, .. }) => self.xe_walk_script(body, this_style, depth + 1),
                        _ => None,
                    };
                }
                Node::MathChar { fam, c, class, .. } if *fam != 255 && *class <= crate::math::CL_PUNCT => {
                    return self.xe_script_char_glyph(*fam, *c, this_style);
                }
                Node::Scripts { nucleus, .. } => {
                    return match nucleus.as_slice() {
                        [Node::MathChar { fam, c, class, .. }] if *fam != 255 && *class <= crate::math::CL_PUNCT => {
                            self.xe_script_char_glyph(*fam, *c, this_style)
                        }
                        _ => None,
                    };
                }
                Node::OpLimits { op, .. } => {
                    return match op.as_slice() {
                        [Node::MathChar { fam, c, .. }] if *fam != 255 => {
                            self.xe_script_char_glyph(*fam, *c, this_style)
                        }
                        [Node::MathChar { fam: 255, .. }, Node::MathChar { fam, c, .. }] if *fam != 255 => {
                            self.xe_script_char_glyph(*fam, *c, this_style)
                        }
                        _ => None,
                    };
                }
                _ => return None,
            }
            i += 1;
        }
        None
    }

    fn xe_script_char_glyph(&self, fam: u8, c: u32, style: u8) -> Option<(FontId, u16)> {
        let fid = self.xe_fam_fnt(style, fam);
        if fid != 0 && self.xe_is_new_mathfont(fid) {
            Some((fid, self.xe_char_glyph(fid, c)))
        } else {
            None
        }
    }

    /// The math kern between the nucleus glyph `(pf, pg)` and the first
    /// character of `script`: 0 when the script has none (`sub_g`/`sup_g`
    /// are 0 in a non-OpenType font).
    pub(crate) fn xe_script_kern(
        &self,
        pf: FontId,
        pg: u16,
        script: &[Node],
        style: u8,
        sub: bool,
        shift: i32,
    ) -> i32 {
        let Some((sf, sg)) = self.xe_first_script_glyph(script, style) else { return 0 };
        let (Some(f), Some(s)) = (self.xe_ot(pf), self.xe_ot(sf)) else { return 0 };
        ot_math_kern(&f, pg, &s, sg, sub, shift)
    }
}

impl Engine {
    /// xetex.web var_delimiter, OpenType case: the chosen glyph in a box (or
    /// the built-up assembly), centred on the math axis.
    pub(crate) fn xe_delimiter_box(
        &self,
        fid: FontId,
        gid: u16,
        parts: Option<Vec<Part>>,
        v: i32,
        style: u8,
    ) -> Node {
        let mut b = match parts {
            Some(parts) => self.xe_build_assembly(fid, &parts, v, false),
            None => {
                let glyph = self.xe_glyph_node(fid, gid, "");
                let (w, h, d) = Self::xe_dims(&glyph);
                let mut b = Self::xe_new_box(VBOX);
                if let Node::Box { w: bw, h: bh, d: bd, list, .. } = &mut b {
                    *list = vec![glyph];
                    *bw = w;
                    *bh = h;
                    *bd = d;
                }
                b
            }
        };
        if let Node::Box { h, d, shift, .. } = &mut b {
            *shift = half(*h - *d) - self.axis_height(style);
        }
        b
    }
}

impl Engine {
    /// `compute_ot_math_accent_pos`: the attachment point of the nucleus
    /// glyph, or `i32::MAX` (`"7FFFFFFF`) when there is none.
    fn xe_accent_pos(&self, body: &[Node], style: u8, depth: u32) -> i32 {
        const NONE: i32 = 0x7FFF_FFFF;
        if depth > 16 {
            return NONE;
        }
        match body {
            [Node::MathChar { fam, c, .. }] if *fam != 255 => {
                let fid = self.xe_fam_fnt(style, *fam);
                self.xe_math.cur_f.set(fid);
                match self.xe_ot(fid) {
                    Some(ot) => {
                        let g = self.xe_char_glyph(fid, *c);
                        ot.accent_pos(g)
                    }
                    None => NONE,
                }
            }
            [Node::Accent { body: inner, .. }, ..] => self.xe_accent_pos(inner, style, depth + 1),
            _ => NONE,
        }
    }

    /// xetex.web `make_math_accent` for an accent in a native font.
    pub(crate) fn xe_make_accent(
        &mut self,
        afid: FontId,
        ac: u32,
        subtype: u8,
        body: &[Node],
        style: u8,
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
    ) -> NodeList {
        const NONE: i32 = 0x7FFF_FFFF;
        let bottom = subtype == 2 || subtype == 3;
        let fixed = subtype & 1 != 0;
        let has_scripts = sup.is_some() || sub.is_some();
        let single_char = matches!(body, [Node::MathChar { fam, .. }] if *fam != 255);
        let mut s = if bottom { 0 } else { self.xe_accent_pos(body, style, 0) };
        let x = self.clean_math_box(body, style | 1);
        let (w, mut h, _) = crate::math::box_dims(&x);
        let ot = self.xe_ot(afid).expect("native accent font");
        let new_math = ot.has_math;
        let mut delta = if bottom {
            0
        } else if new_math {
            h.min(ot.constant(k::ACCENT_BASE_HEIGHT))
        } else {
            let xh = self
                .eqtb
                .font_params
                .get(afid as usize)
                .and_then(|v| v.get(4).copied())
                .unwrap_or(0);
            h.min(xh)
        };
        let mut x = x;
        let mut xw = w;
        if has_scripts && single_char {
            // swap the scripts into box x
            let inner = self.make_scripts(body, sup, sub, style);
            x = crate::boxes::hpack(inner, None, HBOX, &self.eqtb).node;
            let (w2, h2, _) = crate::math::box_dims(&x);
            delta += h2 - h;
            h = h2;
            xw = w2;
        }

        // y := char_box(f, c) turned into a glyph node
        let text = char::from_u32(ac).map(String::from).unwrap_or_default();
        let gid = self.xe_char_glyph(afid, ac);
        let mut p = self.xe_glyph_node(afid, gid, &text);
        if !fixed {
            // switch to a larger accent glyph (or an assembly) that fits
            let c0 = gid;
            let mut a = 0usize;
            let mut w2;
            loop {
                let (g, adv) = ot.variant(c0, a, true);
                w2 = adv;
                if w2 > 0 && w2 <= w {
                    p = self.xe_glyph_node(afid, g, &text);
                    a += 1;
                }
                if w2 < 0 || w2 >= w || a > 4096 || w2 == 0 {
                    break;
                }
            }
            if w2 < 0 {
                if let Some(parts) = ot.assembly(c0, true) {
                    p = self.xe_build_assembly(afid, &parts, w, true);
                }
            }
        }
        let (pw, mut ph, mut pd) = Self::xe_dims(&p);
        if bottom {
            if ph < 0 {
                ph = 0;
            }
        } else if pd < 0 {
            pd = 0;
        }
        let sa = match Self::xe_glyph_of(&p) {
            Some((_, g)) => {
                let v = ot.accent_pos(g);
                if v == NONE {
                    half(pw)
                } else {
                    v
                }
            }
            None => half(pw),
        };
        if bottom || s == NONE {
            s = half(w);
        }
        let mut y = Self::xe_new_box(HBOX);
        if let Node::Box { w: bw, h: bh, d: bd, shift, list, .. } = &mut y {
            *list = vec![p];
            *bw = 0;
            *bh = ph;
            *bd = pd;
            *shift = s - sa;
        }
        let mut out;
        if bottom {
            out = crate::boxes::vpack(vec![x, y], None, VBOX, &self.eqtb).node;
            let (_, vh, _) = crate::math::box_dims(&out);
            if let Node::Box { shift, .. } = &mut out {
                *shift = -(h - vh);
            }
        } else {
            out = crate::boxes::vpack(
                vec![y, Node::Kern(-delta, self.eqtb.cur_attr), x],
                None,
                VBOX,
                &self.eqtb,
            )
            .node;
            let (_, vh, _) = crate::math::box_dims(&out);
            if vh < h {
                if let Node::Box { list, h: bh, .. } = &mut out {
                    list.insert(0, Node::Kern(h - vh, self.eqtb.cur_attr));
                    *bh = h;
                }
            }
        }
        if let Node::Box { w: bw, .. } = &mut out {
            *bw = xw;
        }
        vec![out]
    }
}

impl Engine {
    /// xetex.web `rebox`: put the contents of `b` in a box of width `w`,
    /// centred with `\hss` glue at both ends.
    pub(crate) fn xe_rebox(&self, b: Node, w: i32) -> Node {
        let (bw, list_empty) = match &b {
            Node::Box { w, list, .. } => (*w, list.is_empty()),
            _ => return b,
        };
        if bw != w && !list_empty {
            let b = match &b {
                Node::Box { kind, .. } if *kind == VBOX || *kind == crate::boxes::VTOP => {
                    crate::boxes::hpack(vec![b], None, HBOX, &self.eqtb).node
                }
                _ => b,
            };
            let Node::Box { w: bw, list, .. } = b else { unreachable!() };
            let mut list = list;
            if let [Node::Char { c, font, .. }] = list.as_slice() {
                let v = self.eqtb.fonts.get(*font as usize).map_or(0, |f| f.char_width(*c));
                if v != bw {
                    list.push(Node::Kern(bw - v, self.eqtb.cur_attr));
                }
            }
            let ss = || {
                Node::Glue(
                    Glue::spec(0, crate::scaled::ONE, crate::boxes::GLUE_FIL, crate::scaled::ONE, crate::boxes::GLUE_FIL),
                    self.eqtb.cur_attr,
                )
            };
            let mut out = vec![ss()];
            out.extend(list);
            out.push(ss());
            crate::boxes::hpack(out, Some(w), HBOX, &self.eqtb).node
        } else {
            let mut b = b;
            if let Node::Box { w: bw, .. } = &mut b {
                *bw = w;
            }
            b
        }
    }
}
