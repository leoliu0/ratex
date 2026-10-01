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
        self.show.scan_owner = Some(ScanKind::Radical { delim, subtype: chr, width, options });
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
