//! LuaTeX math constructs: scanning of `\radical`/`\Uradical`/`\Uroot`/
//! `\Uunderdelimiter`..`\Uhextensible`, `\mathaccent`/`\Umathaccent`,
//! `\Uskewed`, `\Ustartmath`..`\Ustopdisplaymath`, and the delimiter scanner
//! (texmath.c `scan_delimiter`, `math_radical`, `math_ac`, `math_fraction`,
//! `init_math`, `after_math`).
//!
//! The mlist typesetting of these noads lives in the conversion half of
//! `math.rs`/`math_otf.rs`; the node shapes are in `boxes.rs`.

use crate::boxes::{noad_option, AccentSpec, Delim, FenceOpts, Node};
use crate::engine::{Engine, EngineKind, Mode};
use crate::eqtb::Equiv;
use crate::prim::{IntParam, Prim};
use crate::show_state::ScanKind;
use crate::token::{CsId, Token};
use crate::uprim::UPrim;
use crate::uprims::MathExt;

impl Engine {
    /// texmath.c `scan_delimiter(p, no_mathcode)` for the delimiter of a
    /// `\left`, `\middle`, `\right` or a fraction. `report` is false when
    /// luatex passes a null `p` (an ignored ambiguous fraction): the
    /// delimiter is read and no "Missing delimiter" error is issued.
    pub(crate) fn scan_delim(&mut self, report: bool) -> Delim {
        if self.engine_kind != EngineKind::LuaTeX {
            return Delim::from_code(self.scan_delim_int());
        }
        self.skip_spaces_relax();
        let t = self.get_token();
        let source = self.current_token_source_mark();
        let scanned = if t.is_char() && matches!(t.cc(), 11 | 12) {
            let (small_fam, small_char, large_fam, large_char) = self.eqtb.lua_del_code(t.chr());
            (small_fam >= 0).then_some(Delim {
                small_fam: small_fam as u8,
                small_char,
                large_fam: large_fam as u8,
                large_char,
            })
        } else if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()) {
                Some(Equiv::Prim(Prim::Delimiter)) => Some(self.scan_delimiter_number(MathExt::Tex, true)),
                Some(Equiv::Prim(Prim::U(UPrim::UDelimiter))) => Some(self.scan_delimiter_number(MathExt::U, true)),
                _ => None,
            }
        } else {
            None
        };
        match scanned {
            Some(d) => d,
            None => {
                if report {
                    self.error_at("Missing delimiter (. inserted)", source.map(|mark| mark.to_context()));
                    // back_error: the offending token is read again
                    if t != crate::input::EOF_MARKER {
                        self.push_token(t);
                    }
                }
                Delim::default()
            }
        }
    }

    /// `do_scan_extdef_del_code(ext, with_class)` as a delimiter field.
    pub(crate) fn scan_delimiter_number(&mut self, ext: MathExt, with_class: bool) -> Delim {
        let (_, small_fam, small_char, large_fam, large_char) = self.scan_delcode_lua(ext, with_class);
        Delim {
            small_fam: small_fam as u8,
            small_char: small_char as u32,
            large_fam: large_fam as u8,
            large_char: large_char as u32,
        }
    }

    /// texmath.c `math_radical`: `chr` is 0 for `\radical`, 1 `\Uradical`,
    /// 2 `\Uroot`, 3 `\Uunderdelimiter`, 4 `\Uoverdelimiter`, 5
    /// `\Udelimiterunder`, 6 `\Udelimiterover`, 7 `\Uhextensible`.
    pub(crate) fn math_radical_lua(&mut self, chr: u8, source: Option<crate::input::SourceMark>) {
        let origin = self.math_diagnostic_origin_at(source);
        let mut width = 0i32;
        let mut options = 0u16;
        loop {
            if self.scan_keyword(b"width") {
                width = self.scan_dimen(false, false);
            } else if self.scan_keyword(b"left") {
                options |= noad_option::LEFT;
            } else if self.scan_keyword(b"middle") {
                options |= noad_option::MIDDLE;
            } else if self.scan_keyword(b"right") {
                options |= noad_option::RIGHT;
            } else {
                break;
            }
        }
        let delim = if chr == 0 {
            self.scan_delimiter_number(MathExt::Tex, true)
        } else {
            self.scan_delimiter_number(MathExt::U, false)
        };
        if chr == 7 {
            self.append_mlist_node(Node::Radical {
                body: Vec::new(),
                delim,
                subtype: 7,
                width,
                options,
                degree: None,
                origin,
            });
            return;
        }
        let mut degree = None;
        if chr == 2 {
            self.show.scan_owner = Some(ScanKind::Degree { delim, width, options });
            degree = Some(self.scan_math_group_or_token());
        }
        self.show.scan_owner = Some(ScanKind::Radical { delim, subtype: chr, width, options, degree: degree.clone() });
        let body = self.scan_math_group_or_token();
        self.append_mlist_node(Node::Radical {
            body,
            delim,
            subtype: chr,
            width,
            options,
            degree,
            origin,
        });
    }

    /// An accent character of `math_ac`: `None` for the null math char.
    fn accent_char(&self, class: i32, family: i32, character: i32) -> Option<(u8, u32)> {
        if character == 0 && family == 0 {
            return None;
        }
        let cur_fam = self.eqtb.int_params[IntParam::CurFam.idx() as usize];
        let var_fam = self.eqtb.int_params[IntParam::VariableFam.idx() as usize];
        let in_range = |v: i32| (0..=255).contains(&v);
        let fam = if class == 7 && in_range(cur_fam) || family == var_fam && in_range(var_fam) && in_range(cur_fam) {
            cur_fam
        } else {
            family
        };
        Some((fam as u8, character as u32))
    }

    /// texmath.c `math_ac`: `\mathaccent` (`chr` 0, also `\accent` in math
    /// mode) and `\Umathaccent` (`chr` 1).
    pub(crate) fn math_ac_lua(&mut self, chr: u8, from_accent: bool, source: Option<crate::input::SourceMark>) {
        if from_accent {
            self.error("Please use \\mathaccent for accents in math mode");
        }
        let origin = self.math_diagnostic_origin_at(source);
        let mut spec = AccentSpec::default();
        let (mut t, mut b, mut o) = ((0, 0, 0), (0, 0, 0), (0, 0, 0));
        if chr == 0 {
            t = self.scan_mathchar_lua(MathExt::Tex);
        } else if self.scan_keyword(b"fixed") {
            spec.subtype = 1;
            t = self.scan_mathchar_lua(MathExt::U);
        } else if self.scan_keyword(b"both") {
            if self.scan_keyword(b"fixed") {
                spec.subtype = 1;
            }
            t = self.scan_mathchar_lua(MathExt::U);
            if self.scan_keyword(b"fixed") {
                spec.subtype += 2;
            }
            b = self.scan_mathchar_lua(MathExt::U);
        } else if self.scan_keyword(b"bottom") {
            if self.scan_keyword(b"fixed") {
                spec.subtype = 2;
            }
            b = self.scan_mathchar_lua(MathExt::U);
        } else if self.scan_keyword(b"top") {
            if self.scan_keyword(b"fixed") {
                spec.subtype = 1;
            }
            t = self.scan_mathchar_lua(MathExt::U);
        } else if self.scan_keyword(b"overlay") {
            if self.scan_keyword(b"fixed") {
                spec.subtype = 1;
            }
            o = self.scan_mathchar_lua(MathExt::U);
        } else {
            t = self.scan_mathchar_lua(MathExt::U);
        }
        if chr == 1 && self.scan_keyword(b"fraction") {
            spec.fraction = self.scan_int();
        }
        spec.top = self.accent_char(t.0, t.1, t.2);
        spec.bottom = self.accent_char(b.0, b.1, b.2);
        spec.overlay = self.accent_char(o.0, o.1, o.2);
        self.append_accent_noad(spec, origin);
    }

    /// texmath.c `math_left_right`: the keywords after `\Uleft`, `\Umiddle`,
    /// `\Uright` and `\Uvextensible`.
    pub(crate) fn scan_fence_options(&mut self) -> FenceOpts {
        let mut fence = FenceOpts::NONE;
        loop {
            if self.scan_keyword(b"height") {
                fence.height = self.scan_dimen(false, false);
            } else if self.scan_keyword(b"depth") {
                fence.depth = self.scan_dimen(false, false);
            } else if self.scan_keyword(b"axis") {
                fence.options |= noad_option::AXIS;
            } else if self.scan_keyword(b"noaxis") {
                fence.options |= noad_option::NO_AXIS;
            } else if self.scan_keyword(b"exact") {
                fence.options |= noad_option::EXACT;
            } else if self.scan_keyword(b"class") {
                let class = self.scan_int();
                // `math_class_to_type` ignores classes beyond 6
                if (0..=6).contains(&class) {
                    fence.class = class;
                }
            } else {
                break;
            }
        }
        fence
    }

    /// `\Uvextensible`: an inner noad holding one fence of subtype
    /// `no_noad_side` (texmath.c `math_left_right`, `t == no_noad_side`).
    pub(crate) fn math_vextensible(&mut self, source: Option<crate::input::SourceMark>) {
        let fence = self.scan_fence_options();
        let delim = self.scan_delim(true);
        let origin = self.math_diagnostic_origin_at(source);
        self.append_mlist_node(Node::Scripts {
            nucleus: vec![
                Node::MathChar {
                    fam: 255,
                    c: 0,
                    class: crate::math::CL_INNER,
                    origin: Default::default(),
                },
                Node::DelimBox {
                    small: (delim.small_fam, delim.small_char),
                    large: (delim.large_fam, delim.large_char),
                    size: 4,
                    fence,
                    origin,
                },
            ],
            sup: None,
            sub: None,
        });
    }

    /// maincontrol.c `non_math(...)`: outside math the command is read again
    /// after an inserted `$`. True when that happened.
    pub(crate) fn lua_insert_dollar(&mut self, id: CsId) -> bool {
        if self.mode.is_m() {
            return false;
        }
        self.push_token(Token::from_cs(id));
        self.push_token(Token::char(3, u32::from(b'$')));
        self.error("Missing $ inserted");
        true
    }

    /// `\Ustartmath` (`chr` 2), `\Ustopmath` (3), `\Ustartdisplaymath` (0),
    /// `\Ustopdisplaymath` (1): the `math_shift_cs_cmd` of maincontrol.c.
    pub(crate) fn math_shift_cs(&mut self, chr: u8, id: CsId) {
        match self.mode {
            Mode::Vertical | Mode::InternalVertical => {
                // run_new_graf: the command runs again in horizontal mode
                self.push_token(Token::from_cs(id));
                self.start_paragraph(true);
            }
            Mode::Horizontal | Mode::RestrictedHorizontal => {
                // texmath.c init_math
                if chr == 0 && self.mode == Mode::Horizontal {
                    self.enter_math_cs(true);
                } else if chr == 2 {
                    self.enter_math_cs(false);
                } else {
                    self.report_illegal_case(id);
                }
            }
            _ => {
                if self.eqtb.cur_group_type() == Some(crate::eqtb::LevelType::MathShift) {
                    self.exit_math_with(Some((chr, id)));
                } else {
                    self.off_save(Token::from_cs(id));
                }
            }
        }
    }
}

// ---------- typesetting (mlist.c) ----------

use crate::boxes::{HBOX, VBOX};
use crate::eqtb::UNDEFINED_MATH_PARAMETER;
use crate::math::{accent_noad_of, den_style, font_size, num_style, sub_style, sup_style, GStyle, DEFAULT_CODE};
use crate::math_otf::{box_shift, box_whd, half, hpack_nat, null_box, set_shift, vpack_nat, xn_over_d, CharTag};
use crate::prim::IntParam as Ip;
use crate::uprim::mp::*;

/// luatex's delimiter tuple `(small fam, small char, large fam, large char)`;
/// a missing delimiter (null pointer) stays `None`.
pub(crate) fn delim_tuple(d: Option<&Delim>) -> Option<(u8, u32, u8, u32)> {
    d.map(|d| (d.small_fam, d.small_char, d.large_fam, d.large_char))
}

fn set_dims(n: &mut Node, w: Option<i32>, h: Option<i32>, d: Option<i32>) {
    if let Node::Box { w: bw, h: bh, d: bd, .. } = n {
        if let Some(w) = w {
            *bw = w;
        }
        if let Some(h) = h {
            *bh = h;
        }
        if let Some(d) = d {
            *bd = d;
        }
    }
}

/// luatex `ext_xn_over_d`
fn ext_xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let mut r = f64::from(x) * f64::from(n) / f64::from(d);
    if r > f64::EPSILON {
        r += 0.5;
    } else {
        r -= 0.5;
    }
    r as i32
}

impl Engine {
    fn math_defaults_mode(&self) -> bool {
        self.eqtb.int_params[Ip::MathDefaultsMode.idx() as usize] > 0
    }

    /// mlist.c `overbar`: kern `ht`, rule `t`, kern `k`, then `b`.
    fn lua_overbar(&mut self, b: Node, k: i32, t: i32, ht: i32) -> Node {
        let rule = Node::Rule { width: crate::build::RULE_FILL, height: t, depth: 0 };
        vpack_nat(self, vec![Node::Kern(ht), rule, Node::Kern(k), b])
    }

    /// `wrapup_over_under_delimiter`: `x` above `y`.
    fn lua_wrapup(&mut self, x: Node, y: Node, shift_up: i32, shift_down: i32) -> Node {
        let (_, hx, dx) = box_whd(&x);
        let (_, hy, dy) = box_whd(&y);
        let mut v = null_box(VBOX);
        set_dims(&mut v, None, Some(shift_up + hx), Some(dy + shift_down));
        if let Node::Box { list, .. } = &mut v {
            *list = vec![x, Node::Kern((shift_up - dx) - (hy - shift_down)), y];
        }
        v
    }

    /// `check_radical`: re-box the delimiter `r` of an over/under delimiter
    /// to the requested width (`t` is the limit it is set against).
    fn lua_check_radical(&mut self, width: i32, options: u16, stack: bool, r: Node, t: &Node) -> Node {
        let (rw, _, _) = box_whd(&r);
        if !stack && rw >= box_whd(t).0 && width != 0 && width != rw {
            let list = if noad_option::has(options, noad_option::LEFT) {
                Some(vec![Node::Kern(width - rw), r.clone()])
            } else if noad_option::has(options, noad_option::MIDDLE) {
                Some(vec![Node::Kern(half(width - rw)), r.clone()])
            } else if noad_option::has(options, noad_option::RIGHT) {
                Some(vec![r.clone()])
            } else {
                None
            };
            if let Some(list) = list {
                let mut b = hpack_nat(self, list);
                set_dims(&mut b, Some(width), None, None);
                return b;
            }
        }
        r
    }

    /// `fixup_widths`
    fn lua_fixup_widths(width: i32, x: &mut Node, y: &mut Node) {
        let (wx, _, _) = box_whd(x);
        let (wy, _, _) = box_whd(y);
        if wy >= wx {
            if width != 0 {
                let s = box_shift(x) + half(wy - wx);
                set_shift(x, s);
            }
            set_dims(x, Some(wy), None, None);
        } else {
            if width != 0 {
                let s = box_shift(y) + half(wx - wy);
                set_shift(y, s);
            }
            set_dims(y, Some(wx), None, None);
        }
    }

    /// mlist.c `make_radical`, `make_hextension` and the four
    /// over/under-delimiter makers: the nucleus box of a radical noad.
    pub(crate) fn make_radical_lua(
        &mut self,
        body: &[Node],
        delim: &Delim,
        subtype: u8,
        width: i32,
        options: u16,
        degree: Option<&[Node]>,
        g: GStyle,
    ) -> Node {
        let size = font_size(g);
        let dt = delim_tuple(Some(delim));
        let defaults = self.math_defaults_mode();
        let size_up = size + usize::from(size != 2);
        match subtype {
            7 => {
                let (e, info) = self.do_delimiter(dt, size, width, true, g, true, 0);
                let (mut w, _, _) = box_whd(&e);
                let mut list = vec![e];
                if !info.stack && width != 0 && width != w {
                    if noad_option::has(options, noad_option::MIDDLE) {
                        list.insert(0, Node::Kern(half(width - w)));
                        w = width;
                    } else if noad_option::has(options, noad_option::EXACT) {
                        w = width;
                    }
                }
                let mut b = hpack_nat(self, list);
                set_dims(&mut b, Some(w), None, None);
                b
            }
            3..=6 => {
                let nuc_style = match subtype {
                    4 => if defaults { sup_style(g) } else { sub_style(g) },
                    3 => if defaults { sub_style(g) } else { sup_style(g) },
                    6 => if defaults { g | 1 } else { g },
                    _ => g,
                };
                let mut n = self.lm_clean_list(body, nuc_style);
                let wd = if width != 0 { width } else { box_whd(&n).0 };
                let dsize = if subtype >= 5 { size_up } else { size };
                let (d, info) = self.do_delimiter(dt, dsize, wd, true, g, true, 0);
                let mut d = self.lua_check_radical(width, options, info.stack, d, &n);
                Self::lua_fixup_widths(width, &mut n, &mut d);
                let (hn, dn) = (box_whd(&n).1, box_whd(&n).2);
                let (hd, dd) = (box_whd(&d).1, box_whd(&d).2);
                let (w_n, w_d) = (box_whd(&n).0, box_whd(&d).0);
                match subtype {
                    4 => {
                        let mut shift_up = self.mparam_err(MATH_PARAM_OVER_DELIMITER_BGAP, g);
                        let clr = self.mparam_err(MATH_PARAM_OVER_DELIMITER_VGAP, g);
                        let delta = clr - ((shift_up - dn) - (hd - 0));
                        if delta > 0 {
                            shift_up += delta;
                        }
                        let mut v = self.lua_wrapup(n, d, shift_up, 0);
                        set_dims(&mut v, Some(w_n), None, None);
                        v
                    }
                    3 => {
                        let mut shift_down = self.mparam_err(MATH_PARAM_UNDER_DELIMITER_BGAP, g);
                        let clr = self.mparam_err(MATH_PARAM_UNDER_DELIMITER_VGAP, g);
                        let delta = clr - ((0 - dd) - (hn - shift_down));
                        if delta > 0 {
                            shift_down += delta;
                        }
                        let mut v = self.lua_wrapup(d, n, 0, shift_down);
                        set_dims(&mut v, Some(w_n), None, None);
                        v
                    }
                    6 => {
                        let mut shift_up = self.mparam_err(MATH_PARAM_OVER_DELIMITER_BGAP, g) - hd - dd;
                        let clr = self.mparam_err(MATH_PARAM_OVER_DELIMITER_VGAP, g);
                        let actual = shift_up - hn;
                        if actual < clr {
                            shift_up += clr - actual;
                        }
                        let mut v = self.lua_wrapup(d, n, shift_up, 0);
                        set_dims(&mut v, Some(w_d), None, None);
                        v
                    }
                    _ => {
                        let mut shift_down = self.mparam_err(MATH_PARAM_UNDER_DELIMITER_BGAP, g) - hd - dd;
                        let clr = self.mparam_err(MATH_PARAM_UNDER_DELIMITER_VGAP, g);
                        let actual = shift_down - dn;
                        if actual < clr {
                            shift_down += clr - actual;
                        }
                        let mut v = self.lua_wrapup(n, d, 0, shift_down);
                        set_dims(&mut v, Some(w_d), None, None);
                        v
                    }
                }
            }
            _ => {
                // \radical, \Uradical, \Uroot
                let x = self.lm_clean_list(body, g | 1);
                let mut clr = self.mparam_err(MATH_PARAM_RADICAL_VGAP, g);
                let mut theta = self.mparam(MATH_PARAM_RADICAL_RULE, g);
                if self.eqtb.int_params[Ip::MathRuleThicknessMode.idx() as usize] > 0 {
                    let f = self.fam_fnt(u32::from(delim.small_fam), size);
                    if self.assume_new_math(f) {
                        let t = self.font_math_par(f, crate::math_otf::mc::RADICAL_RULE_THICKNESS);
                        if t != UNDEFINED_MATH_PARAMETER {
                            theta = t;
                        }
                    }
                }
                let (_, hx, dx) = box_whd(&x);
                let mut y;
                if theta == UNDEFINED_MATH_PARAMETER {
                    let fr = self.mparam_err(MATH_PARAM_FRACTION_RULE, g);
                    y = self.do_delimiter(dt, size, hx + dx + clr + fr, false, g, true, 0).0;
                    theta = match &y {
                        Node::Box { list, .. } => match list.first() {
                            Some(Node::Box { kind, list: l2, .. }) if *kind == HBOX => match l2.first() {
                                Some(Node::Char { font, c }) => self.mc_metrics(*font, u32::from(*c)).height,
                                Some(Node::LuaGlyph(gl)) => self.mc_metrics(gl.font, gl.c).height,
                                _ => box_whd(&y).1,
                            },
                            _ => box_whd(&y).1,
                        },
                        _ => box_whd(&y).1,
                    };
                } else {
                    y = self.do_delimiter(dt, size, hx + dx + clr + theta, false, g, true, 0).0;
                }
                let (_, hy, dy) = box_whd(&y);
                let delta = (dy + hy - theta) - (hx + dx + clr);
                if delta > 0 {
                    clr += half(delta);
                }
                let shift = (hy - theta) - (hx + clr);
                set_shift(&mut y, shift);
                let h = dy + hy;
                let kern = self.mparam_err(MATH_PARAM_RADICAL_KERN, g);
                let p = self.lua_overbar(x, clr, theta, kern);
                let mut list = vec![y, p];
                if let Some(degree) = degree {
                    let r = self.lm_clean_list(degree, 6);
                    let (wr, _, _) = box_whd(&r);
                    if wr != 0 {
                        let mut r = r;
                        let br = self.mparam_err(MATH_PARAM_RADICAL_DEGREE_BEFORE, g);
                        let mut ar = self.mparam_err(MATH_PARAM_RADICAL_DEGREE_AFTER, g);
                        if -ar > wr + br {
                            ar = -(wr + br);
                        }
                        let raise = self.mparam_err(MATH_PARAM_RADICAL_DEGREE_RAISE, g);
                        set_shift(&mut r, -(xn_over_d(h, raise, 100) - dy - shift));
                        list.insert(0, Node::Kern(ar));
                        list.insert(0, r);
                        list.insert(0, Node::Kern(br));
                    }
                }
                hpack_nat(self, list)
            }
        }
    }

    /// mlist.c `make_fraction`: the box holding the delimiters and the
    /// fraction (`\Uskewed` when `middle` is set).
    pub(crate) fn make_fraction_lua(
        &mut self,
        num: &[Node],
        den: &[Node],
        thickness: i32,
        left: Option<&Delim>,
        right: Option<&Delim>,
        middle: Option<&Delim>,
        options: u16,
        g: GStyle,
    ) -> Node {
        let size = font_size(g);
        let mut thickness = thickness;
        if self.eqtb.int_params[Ip::MathRuleThicknessMode.idx() as usize] > 0 && thickness != 0 {
            let f = self.fam_fnt(0, size);
            if self.assume_new_math(f) {
                let t = self.font_math_par(f, crate::math_otf::mc::FRACTION_RULE_THICKNESS);
                if t != UNDEFINED_MATH_PARAMETER {
                    thickness = t;
                }
            }
        }
        if thickness == DEFAULT_CODE {
            thickness = self.mparam_err(MATH_PARAM_FRACTION_RULE, g);
        }
        let mut x = self.lm_clean_list(num, num_style(g));
        let mut z = self.lm_clean_list(den, den_style(g));
        let axis = self.math_axis_size(size);
        let m = if let Some(md) = middle {
            Some(self.do_delimiter(delim_tuple(Some(md)), size, 0, false, g, true, 0).0)
        } else {
            let (wx, _, _) = box_whd(&x);
            let (wz, _, _) = box_whd(&z);
            if wx < wz {
                x = self.lm_rebox(x, wz);
            } else {
                z = self.lm_rebox(z, wx);
            }
            None
        };
        let (_, hx, dx) = box_whd(&x);
        let (_, hz, dz) = box_whd(&z);
        let exact = noad_option::has(options, noad_option::EXACT);
        let (mut shift_up, mut shift_down);
        let mut delta = 0;
        if m.is_some() {
            shift_up = 0;
            shift_down = 0;
        } else if thickness == 0 {
            shift_up = self.mparam_err(MATH_PARAM_STACK_NUM_UP, g);
            shift_down = self.mparam_err(MATH_PARAM_STACK_DENOM_DOWN, g);
            let clr1 = self.mparam_err(MATH_PARAM_STACK_VGAP, g);
            let d = half(clr1 - ((shift_up - dx) - (hz - shift_down)));
            if d > 0 {
                shift_up += d;
                shift_down += d;
            }
        } else {
            shift_up = self.mparam_err(MATH_PARAM_FRACTION_NUM_UP, g);
            shift_down = self.mparam_err(MATH_PARAM_FRACTION_DENOM_DOWN, g);
            let mut clr1 = self.mparam_err(MATH_PARAM_FRACTION_NUM_VGAP, g);
            let mut clr2 = self.mparam_err(MATH_PARAM_FRACTION_DENOM_VGAP, g);
            delta = half(thickness);
            if !exact {
                let fr = self.mparam_err(MATH_PARAM_FRACTION_RULE, g);
                clr1 = ext_xn_over_d(clr1, thickness, fr);
                clr2 = ext_xn_over_d(clr2, thickness, fr);
            }
            let delta1 = clr1 - ((shift_up - dx) - (axis + delta));
            let delta2 = clr2 - ((shift_down - hz) + (axis - delta));
            if delta1 > 0 {
                shift_up += delta1;
            }
            if delta2 > 0 {
                shift_down += delta2;
            }
        }
        let v;
        if let Some(mut m) = m {
            shift_up = self.mparam_err(MATH_PARAM_SKEWED_FRACTION_VGAP, g);
            if !noad_option::has(options, noad_option::NO_AXIS) {
                shift_up += half(axis);
            }
            shift_down = shift_up;
            let (wx, _, _) = box_whd(&x);
            let (wz, _, _) = box_whd(&z);
            let mut xb = null_box(HBOX);
            set_dims(&mut xb, Some(wx), Some(hx + shift_up), Some(dx));
            set_shift(&mut xb, -shift_up);
            if let Node::Box { list, .. } = &mut xb {
                *list = vec![x];
            }
            let mut zb = null_box(HBOX);
            set_dims(&mut zb, Some(wz), Some(hz), Some(dz + shift_down));
            set_shift(&mut zb, shift_down);
            if let Node::Box { list, .. } = &mut zb {
                *list = vec![z];
            }
            let (wm, hm, dm) = box_whd(&m);
            let hgap = self.mparam_err(MATH_PARAM_SKEWED_FRACTION_HGAP, g);
            let hh = (hx + shift_up).max(hz).max(hm);
            let dd = dx.max(dz + shift_down).max(dm);
            let (d1, d2, total_w);
            if exact {
                d1 = -half(hgap);
                d2 = d1;
                total_w = wx + wz + wm - hgap;
            } else {
                d1 = half(hgap - wm);
                d2 = half(hgap + wm);
                total_w = wx + wz + hgap;
                set_dims(&mut m, Some(0), None, None);
            }
            let mut vb = null_box(HBOX);
            set_dims(&mut vb, Some(total_w), Some(hh), Some(dd));
            if let Node::Box { list, .. } = &mut vb {
                *list = vec![xb, Node::Kern(d1), m, Node::Kern(d2), zb];
            }
            v = vb;
        } else {
            let mut vb = null_box(VBOX);
            let (wx, _, _) = box_whd(&x);
            set_dims(&mut vb, Some(wx), Some(shift_up + hx), Some(dz + shift_down));
            let list = if thickness != 0 && !noad_option::has(options, noad_option::NO_RULE) {
                vec![
                    x,
                    Node::Kern((shift_up - dx) - (axis + delta)),
                    Node::Rule { width: crate::build::RULE_FILL, height: thickness, depth: 0 },
                    Node::Kern((axis - delta) - (hz - shift_down)),
                    z,
                ]
            } else {
                vec![x, Node::Kern((shift_up - dx) - (hz - shift_down)), z]
            };
            if let Node::Box { list: l, .. } = &mut vb {
                *l = list;
            }
            v = vb;
        }
        let mut dsize = self.mparam(MATH_PARAM_FRACTION_DEL_SIZE, g);
        if dsize == UNDEFINED_MATH_PARAMETER {
            let (_, hv, dv) = box_whd(&v);
            dsize = self.lm_delimiter_height(dv, hv, true, size);
        }
        let (l, _) = self.do_delimiter(delim_tuple(left), size, dsize, false, g, true, 0);
        let (r, _) = self.do_delimiter(delim_tuple(right), size, dsize, false, g, true, 0);
        hpack_nat(self, vec![l, v, r])
    }

    /// mlist.c `make_math_accent` (top, bottom and overlay accents of one
    /// noad): the nucleus box, and whether the scripts were swapped into the
    /// accentee (so the noad has none left).
    pub(crate) fn make_math_accent_lua(
        &mut self,
        spec: &AccentSpec,
        body: &[Node],
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
        g: GStyle,
    ) -> (Node, bool) {
        let size = font_size(g);
        let topstretch = spec.subtype % 2 == 0;
        let botstretch = spec.subtype / 2 == 0;
        let stages = [(spec.top, 1u8, topstretch), (spec.bottom, 2, botstretch), (spec.overlay, 4, true)];
        let mut nucleus_box: Option<Node> = None;
        let mut consumed = false;
        for (acc, code, stretch) in stages {
            let Some((afam, ac)) = acc else { continue };
            let (f, ac) = self.lm_fetch(afam, ac, size);
            if f == 0 || !self.mc_exists(f, ac) {
                continue;
            }
            let (mut c, mut s) = (ac, 1i32);
            let mut s_abs = false;
            let mut nuc_f = f;
            let first = nucleus_box.is_none();
            if first {
                // compute_accent_skew over the nucleus (recursing into a lone accent)
                let mut target = body;
                let mut guard = 0;
                while let Some((_, inner, _)) = accent_noad_of(target) {
                    target = inner;
                    guard += 1;
                    if guard > 64 {
                        break;
                    }
                }
                if let [Node::MathChar { fam, c: nc, .. }] = target {
                    if *fam != 255 {
                        let (nf, nc) = self.lm_fetch(*fam, *nc, size);
                        nuc_f = nf;
                        if self.assume_new_math(nf) {
                            let ta = self.mc_metrics(nf, nc).top_accent;
                            if ta != i32::MIN {
                                s = ta;
                                s_abs = true;
                            }
                        } else if code & 1 != 0 {
                            s = self.mc_skew_kern(nf, nc);
                        } else {
                            s = 0;
                        }
                    }
                }
            }
            let mut x = match nucleus_box.take() {
                Some(b) => {
                    if box_shift(&b) == 0 {
                        b
                    } else {
                        hpack_nat(self, vec![b])
                    }
                }
                None => self.lm_clean_list(body, g | 1),
            };
            let (w, mut h, _) = box_whd(&x);
            if self.assume_new_math(nuc_f) && !s_abs {
                s = half(w);
                s_abs = true;
            }
            let fraction = if spec.fraction == 0 { 1000 } else { spec.fraction };
            let target = if code == 4 {
                if fraction > 0 { xn_over_d(h, fraction, 1000) } else { h }
            } else if fraction > 0 {
                xn_over_d(w, fraction, 1000)
            } else {
                w
            };
            let mut y: Option<Node> = None;
            let mut ext = false;
            if (stretch || code == 4) && self.mc_metrics(f, c).width < w {
                loop {
                    let tag = self.mc_tag(f, c);
                    if tag == CharTag::Ext && self.mc_variants(f, c, true).is_some() {
                        let ov = self.mparam_err(MATH_PARAM_CONNECTOR_OVERLAP_MIN, g);
                        y = Some(self.make_extensible(f, c, w, ov, true));
                        ext = true;
                        break;
                    } else if let CharTag::List(yy) = tag {
                        if !self.mc_exists(f, yy) {
                            break;
                        }
                        let m = self.mc_metrics(f, yy);
                        if code == 4 {
                            if m.height > target {
                                break;
                            }
                        } else if m.width > target {
                            break;
                        }
                        c = yy;
                    } else {
                        break;
                    }
                }
            }
            let mut y = y.unwrap_or_else(|| self.char_box(f, c));
            let (_, hy, dy) = box_whd(&y);
            let (_, hx, dx) = box_whd(&x);
            let mut delta = if code == 1 {
                let abh = self.accent_base_height(f);
                if h < abh { h } else { abh }
            } else if code == 4 {
                half(hy + dy + hx + dx)
            } else {
                0
            };
            if (sup.is_some() || sub.is_some()) && first && matches!(body, [Node::MathChar { fam, .. }] if *fam != 255) {
                let scripted = [Node::Scripts {
                    nucleus: body.to_vec(),
                    sup: sup.map(<[Node]>::to_vec),
                    sub: sub.map(<[Node]>::to_vec),
                }];
                x = self.lm_clean_list(&scripted, g);
                let nh = box_whd(&x).1;
                delta += nh - h;
                h = nh;
                consumed = true;
            }
            let wy = box_whd(&y).0;
            if s_abs {
                let sa = if ext { half(wy) } else { self.mc_metrics(f, c).top_accent };
                let sa = if sa == i32::MIN { half(wy) } else { sa };
                set_shift(&mut y, s - sa);
            } else if wy == 0 {
                set_shift(&mut y, s + w);
            } else {
                set_shift(&mut y, s + half(w - wy));
            }
            set_dims(&mut y, Some(0), None, None);
            let xw = box_whd(&x).0;
            let top = code & 5 != 0;
            let list = if top { vec![y, Node::Kern(-delta), x] } else { vec![x, y] };
            let mut r = vpack_nat(self, list);
            set_dims(&mut r, Some(xw), None, None);
            if top {
                let hr = box_whd(&r).1;
                if hr < h {
                    if let Node::Box { list, .. } = &mut r {
                        list.insert(0, Node::Kern(h - hr));
                    }
                    set_dims(&mut r, None, Some(h), None);
                }
            } else {
                let hr = box_whd(&r).1;
                set_shift(&mut r, -(h - hr));
            }
            nucleus_box = Some(r);
        }
        match nucleus_box {
            Some(b) => (b, consumed),
            None => (self.lm_clean_list(body, g | 1), false),
        }
    }
}
