//! Value scanners: scan_int, scan_dimen, scan_glue, expressions, \the,
//! \meaning, general text.

use crate::boxes::Glue;
use crate::engine::{Engine, EngineKind, PhysicalTokenSource};
use crate::eqtb::Equiv;
use crate::input::{SourceContext, SourceMark};
use crate::prim::{DimParam, IntParam, Prim};

#[derive(Clone, Copy, Debug)]
enum UnitKind {
    Ratio(i64, i64),
    InternalSp(i64),
    FontRelativeSp(i64),
    Sp,
}

/// A scanner location that is cheap to retain while valid input is parsed.
/// Physical tokens keep only coordinates; macro-generated tokens need the
/// already-captured call-site bookmark so a later unit scan cannot erase it.
enum NumericOrigin {
    Physical(PhysicalTokenSource),
    Mark(SourceMark),
}
use crate::scaled::{mult, ONE};
use crate::token::Token;

fn ascii_character(token: Token) -> Option<u8> {
    token
        .is_char()
        .then(|| u8::try_from(token.chr()).ok())
        .flatten()
        .filter(u8::is_ascii)
}

fn font_character_code_primitive(primitive: Prim) -> &'static str {
    match primitive {
        Prim::EfCode => "\\efcode",
        Prim::LpCode => "\\lpcode",
        Prim::RpCode => "\\rpcode",
        Prim::TagCode => "\\tagcode",
        Prim::KnBsCode => "\\knbscode",
        Prim::StBsCode => "\\stbscode",
        Prim::ShBsCode => "\\shbscode",
        Prim::KnBcCode => "\\knbccode",
        Prim::KnAcCode => "\\knaccode",
        _ => "font character table",
    }
}

impl Engine {
    #[inline]
    fn numeric_origin(&self) -> Option<NumericOrigin> {
        self.diagnostic_physical_source
            .map(NumericOrigin::Physical)
            .or_else(|| self.current_token_source_mark().map(NumericOrigin::Mark))
    }

    fn numeric_origin_context(&self, origin: NumericOrigin) -> Option<SourceContext> {
        match origin {
            NumericOrigin::Physical(source) => self
                .input
                .source_mark_at(source.source_index, source.line, source.byte_column)
                .map(|mark| mark.to_context()),
            NumericOrigin::Mark(mark) => Some(mark.to_context()),
        }
    }

    /// Check a prospective append to a scanner-owned token list without
    /// overflowing `usize`. Formatting and source materialization happen only
    /// on the exceptional path, leaving the usual scan loop as one branch.
    #[inline(always)]
    pub(crate) fn scanned_token_list_has_room(
        &mut self,
        current: usize,
        additional: usize,
        description: &str,
        origin: Option<&crate::input::SourceMark>,
    ) -> bool {
        if current <= crate::input::MAX_TOKEN_LIST_TOKENS
            && additional <= crate::input::MAX_TOKEN_LIST_TOKENS - current
        {
            return true;
        }
        self.fatal_error_at(
            &format!(
                "TeX capacity exceeded, sorry [{description}={}]",
                crate::input::MAX_TOKEN_LIST_TOKENS
            ),
            origin.map(crate::input::SourceMark::to_context),
        );
        false
    }

    /// skip spaces and \relax tokens (TeX's "scan something" preamble)
    pub fn skip_spaces_relax(&mut self) {
        loop {
            let t = self.get_token();
            if t.is_char() && (t.cc() == 10 || t.cc() == 9) {
                continue;
            }
            if t.is_cs() && self.cur_prim == Some(Prim::Relax) {
                continue;
            }
            self.push_token(t);
            return;
        }
    }
    pub fn skip_spaces(&mut self) {
        loop {
            let t = self.get_token();
            if t.is_char() && (t.cc() == 10 || t.cc() == 9) {
                continue;
            }
            self.push_token(t);
            return;
        }
    }

    fn token_is_fi_or_else(&self, t: Token) -> bool {
        if !t.is_cs() {
            return false;
        }
        matches!(
            self.eqtb.resolve(t.cs_id()),
            Some(Equiv::Prim(
                Prim::Else | Prim::Or | Prim::Fi | Prim::ElIf | Prim::ElIfX
            ))
        )
    }

    /// A delimiter terminates an unfinished conditional's numeric operand;
    /// delimiters of already selected branches still expand normally.
    fn get_x_raw_keep_cond(&mut self) -> Token {
        let t = self.raw_token();
        if self.token_is_fi_or_else(t)
            && self
                .pending_if_depth
                .is_some_and(|depth| self.if_stack.len() <= depth)
        {
            return t;
        }
        self.push_token(t);
        self.get_x_raw()
    }

    /// Glue parameter, `\\skip n`, or skipdef'd CS. Knuth copies these as a
    /// whole glue value (and their width is a legal dimen/unit).
    fn glue_from_cur_cs(&mut self, t: Token) -> Option<Glue> {
        if !t.is_cs() {
            return None;
        }
        match self.eqtb.resolve(t.cs_id()).cloned() {
            Some(Equiv::Prim(Prim::GlueP(p))) => {
                Some(self.eqtb.glue_params[p.idx() as usize].clone())
            }
            Some(Equiv::Prim(Prim::Skip)) => {
                let i = self.scan_reg_num();
                Some(self.eqtb.skip[i as usize].clone())
            }
            Some(Equiv::Prim(Prim::MuSkip)) => {
                let i = self.scan_reg_num();
                Some(self.eqtb.muskip[i as usize].clone())
            }
            Some(Equiv::Prim(Prim::LastSkip)) => Some(self.last_skip_value()),
            Some(Equiv::SkipReg(i)) => Some(self.eqtb.skip[i as usize].clone()),
            Some(Equiv::MuSkipReg(i)) => Some(self.eqtb.muskip[i as usize].clone()),
            _ => None,
        }
    }

    /// 0=stretch, 1=shrink, 2=stretch_order, 3=shrink_order
    fn scan_etex_glue_field(&mut self, field: u8) -> i32 {
        let g = self.scan_glue(false);
        match field {
            0 => g.stretch,
            1 => g.shrink,
            2 => g.stretch_order as i32,
            3 => g.shrink_order as i32,
            _ => 0,
        }
    }

    /// tex.web @<Scan an optional space@>: one expanding fetch; consume a
    /// space, otherwise back it up. After an alphabetic constant this is
    /// what drives expl3 f-expansion (`\romannumeral`^^@\foo` expands `\foo`).
    fn scan_optional_space(&mut self) {
        let t = self.get_x_raw_keep_cond();
        if !t.is_space() {
            self.push_token(t);
        }
    }

    /// Next non-space token with expansion (macros, not `\fi` via get_token).
    /// `\relax` is returned, not skipped: it terminates `\numexpr`.
    fn expr_next_token(&mut self) -> Token {
        loop {
            let t = self.get_x_raw();
            if t.is_space() {
                continue;
            }
            return t;
        }
    }

    fn token_is_relax(&self, t: Token) -> bool {
        t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Relax)))
    }

    /// scan optional `=` with spaces/relax skipped
    pub fn scan_optional_equals(&mut self) {
        self.skip_spaces_relax();
        let t = self.get_token();
        if !(t.is_char() && t.chr() == b'=' as u32) {
            self.push_token(t);
        }
    }

    /// read a sequence of digit char tokens in the given radix
    fn scan_digits(
        &mut self,
        radix: u32,
        allow_letters: bool,
    ) -> (i64, bool, Option<SourceContext>, bool, bool) {
        let mut v: i64 = 0;
        let mut overflowed = false;
        let mut overflow_source = None;
        let mut saw_digit = false;
        let mut ended_at_eof = false;
        loop {
            // expanding fetch, no space skip: a space terminates the constant
            let t = self.get_x_raw();
            if t == crate::input::EOF_MARKER {
                ended_at_eof = true;
                self.push_token(t);
                break;
            }
            if t.is_space() {
                break; // absorb one space, number complete
            }
            if !t.is_char() {
                self.push_token(t);
                break;
            }
            let c = t.chr();
            let d = match () {
                _ if (b'0' as u32..=b'9' as u32).contains(&c) => c - b'0' as u32,
                _ if allow_letters && (b'a' as u32..=b'f' as u32).contains(&c) => {
                    c - b'a' as u32 + 10
                }
                _ if allow_letters && (b'A' as u32..=b'F' as u32).contains(&c) => {
                    c - b'A' as u32 + 10
                }
                _ => {
                    self.push_token(t);
                    break;
                }
            };
            if d >= radix {
                self.push_token(t);
                break;
            }
            saw_digit = true;
            v = v * radix as i64 + d as i64;
            if v > 0x7FFF_FFFF {
                v = 0x7FFF_FFFF;
                if !overflowed {
                    overflowed = true;
                    overflow_source = self
                        .numeric_origin()
                        .and_then(|origin| self.numeric_origin_context(origin));
                }
            }
        }
        (v, overflowed, overflow_source, saw_digit, ended_at_eof)
    }

    fn is_digit_token(t: Token) -> bool {
        t.cc() == 12 && ascii_character(t).is_some_and(|c| c.is_ascii_digit())
    }

    /// TeX scan_int: signs, digits (dec/oct/hex), char consts, cs values.
    pub fn scan_int(&mut self) -> i32 {
        // \\romannumeral (and friends) must expand \\protected macros
        // even inside \\expanded/\\edef; e-TeX only freezes them in the
        // outer token-list scan, not in nested number scanning. (The l3
        // \\exp_end_continue_f:w protocol depends on this.)
        let prev = self.in_expanded_scan;
        self.in_expanded_scan = false;
        let r = self.scan_int_inner();
        self.in_expanded_scan = prev;
        r
    }

    fn scan_int_inner(&mut self) -> i32 {
        let mut negate = false;
        let mut v: i64;
        'scan_loop: loop {
            self.skip_spaces_relax();
            let t = self.get_x_raw();
            if t.is_char() && t.chr() == b'+' as u32 {
                continue;
            }
            if t.is_char() && t.chr() == b'-' as u32 {
                negate = !negate;
                continue;
            }
            if Self::is_digit_token(t) {
                v = (t.chr() - b'0' as u32) as i64;
                let mut overflowed = false;
                let mut overflow_source = None;
                loop {
                    // expanding fetch without space skip (tex.web get_x_token):
                    // expandables continue the number, a space terminates it
                    let t2 = self.get_x_raw_keep_cond();

                    if t2.is_space() {
                        break;
                    }
                    if Self::is_digit_token(t2) {
                        v = v * 10 + (t2.chr() - b'0' as u32) as i64;
                        if v > 0x7FFF_FFFF {
                            v = 0x7FFF_FFFF;
                            if !overflowed {
                                overflowed = true;
                                overflow_source = self
                                    .numeric_origin()
                                    .and_then(|origin| self.numeric_origin_context(origin));
                            }
                        }
                    } else {
                        self.push_token(t2);
                        break;
                    }
                }
                if overflowed {
                    self.error_at("Number too big", overflow_source);
                }
                break;
            }
            if t.is_char() && t.chr() == b'\'' as u32 {
                let prefix_origin = self.numeric_origin();
                let (value, overflowed, source, saw_digit, ended_at_eof) =
                    self.scan_digits(8, false);
                v = value;
                if !saw_digit {
                    let source = if ended_at_eof {
                        prefix_origin.and_then(|origin| self.numeric_origin_context(origin))
                    } else {
                        self.numeric_origin()
                            .and_then(|origin| self.numeric_origin_context(origin))
                    };
                    self.error_at("Missing number, treated as zero", source);
                }
                if overflowed {
                    self.error_at("Number too big", source);
                }
                break;
            }
            if t.is_char() && t.chr() == b'"' as u32 {
                let prefix_origin = self.numeric_origin();
                let (value, overflowed, source, saw_digit, ended_at_eof) =
                    self.scan_digits(16, true);
                v = value;
                if !saw_digit {
                    let source = if ended_at_eof {
                        prefix_origin.and_then(|origin| self.numeric_origin_context(origin))
                    } else {
                        self.numeric_origin()
                            .and_then(|origin| self.numeric_origin_context(origin))
                    };
                    self.error_at("Missing number, treated as zero", source);
                }
                if overflowed {
                    self.error_at("Number too big", source);
                }
                break;
            }
            if t.is_char() && t.chr() == b'`' as u32 {
                // char constant: next token RAW (no expansion; tex.web get_token
                // does not expand); a cs contributes its name's first char
                let t2 = self.raw_token();
                if t2.is_char() {
                    v = t2.chr() as i64;
                    // tex.web §442: undo raw_token's brace-depth adjustment for
                    // an alphabetic char constant (`\ifnum 0=`}\fi` idiom).
                    if t2.cc() == 2 {
                        self.align_brace_depth = self.align_brace_depth.saturating_add(1);
                    } else if t2.cc() == 1 {
                        self.align_brace_depth = self.align_brace_depth.saturating_sub(1);
                    }
                } else if t2.is_cs() {
                    let name = self.cs.name(t2.cs_id());
                    v = if self.engine_kind == EngineKind::PdfTeX {
                        name.first().copied().unwrap_or(0) as i64
                    } else {
                        std::str::from_utf8(name)
                            .ok()
                            .and_then(|name| name.chars().next())
                            .map_or(0, |character| character as i64)
                    };
                } else {
                    self.error("Missing character after `");
                    v = 0;
                }
                // Char constants end the number (tex.web §444). The trailing
                // optional space is scanned EXPANDING (expl3 file-name walkers
                // rely on it to advance their f-expansion), but a following
                // \\fi/\\else of the enclosing \\ifnum must stay raw: expanding
                // it would pop the not-yet-pushed conditional state
                // (longtable's \\ifnum0=`}\\fi brace-hiding idiom).
                let t3 = self.get_x_raw_keep_cond();
                if !t3.is_space() {
                    self.push_token(t3);
                }
                break;
            }
            if t.is_cs() {
                match self.cur_prim {
                    Some(Prim::Count | Prim::Attribute) => {
                        let idx = self.scan_reg_num();
                        v = self.eqtb.count[idx as usize] as i64;
                        break 'scan_loop;
                    }
                    // tex.web §413: `\parshape` used as an integer is the
                    // number of active shape specifications.
                    Some(Prim::ParShape) => {
                        v = self.par_shape.len() as i64;
                        break 'scan_loop;
                    }
                    Some(
                        p @ (Prim::InterLinePenalties
                        | Prim::ClubPenalties
                        | Prim::WidowPenalties
                        | Prim::DisplayWidowPenalties),
                    ) => {
                        let index = self.scan_int();
                        v = self.penalty_shape_value(p, index) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::CatCode) => {
                        let c = self.scan_profile_character_code("\\catcode");
                        v = i64::from(self.eqtb.cat_code(c));
                        break 'scan_loop;
                    }
                    Some(Prim::MathCode) => {
                        let c = self.scan_profile_character_code("\\mathcode");
                        v = i64::from(self.eqtb.math_code_for(c));
                        break 'scan_loop;
                    }
                    Some(Prim::DelCode) => {
                        let c = self.scan_profile_character_code("\\delcode");
                        v = self.eqtb.delimiter_code_for(c);
                        break 'scan_loop;
                    }
                    Some(Prim::LcCodeP) => {
                        let c = self.scan_profile_character_code("\\lccode");
                        v = self.eqtb.case_code(c, false) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::SfCodeP) => {
                        let c = self.scan_profile_character_code("\\sfcode");
                        v = i64::from(self.eqtb.space_factor_code(c));
                        break 'scan_loop;
                    }
                    Some(Prim::UcCodeP) => {
                        let c = self.scan_profile_character_code("\\uccode");
                        v = self.eqtb.case_code(c, true) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::IntP(p)) => {
                        v = self.int_param_value(p) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::PdfShellEscape) => {
                        // This engine never executes shell commands.
                        v = 0;
                        break 'scan_loop;
                    }
                    Some(Prim::PdfRandomSeed) => {
                        v = self.random_seed as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::PdfElapsedTime) => {
                        v = 0;
                        break 'scan_loop;
                    }
                    Some(Prim::PdfLastXPos) => {
                        v = self.pdf_last_x as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::PdfLastYPos) => {
                        v = self.pdf_last_y as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::Wd) => {
                        let n = self.scan_reg_num();
                        v = self.box_reg_dimen(n, 0) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::Ht) => {
                        let n = self.scan_reg_num();
                        v = self.box_reg_dimen(n, 1) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::Dp) => {
                        let n = self.scan_reg_num();
                        v = self.box_reg_dimen(n, 2) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::NumExpr) => {
                        v = self.scan_expr_num() as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::GlueStretch) => {
                        v = self.scan_etex_glue_field(0) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::GlueShrink) => {
                        v = self.scan_etex_glue_field(1) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::GlueStretchOrder) => {
                        v = self.scan_etex_glue_field(2) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::GlueShrinkOrder) => {
                        v = self.scan_etex_glue_field(3) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::DimExpr) => {
                        v = self.scan_expr_dim() as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::HyphenChar) => {
                        let f = self.scan_font_id() as usize;
                        v = self.eqtb.hyphen_char.get(f).copied().unwrap_or(0) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::SkewChar) => {
                        let f = self.scan_font_id() as usize;
                        v = self.eqtb.skew_char.get(f).copied().unwrap_or(0) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::PdfNoLigatures) => {
                        let f = self.scan_font_id() as usize;
                        v = self.test_no_ligatures(f as u16) as i64;
                        break 'scan_loop;
                    }
                    Some(
                        p @ (Prim::EfCode
                        | Prim::LpCode
                        | Prim::RpCode
                        | Prim::TagCode
                        | Prim::KnBsCode
                        | Prim::StBsCode
                        | Prim::ShBsCode
                        | Prim::KnBcCode
                        | Prim::KnAcCode),
                    ) => {
                        let f = self.scan_font_id() as usize;
                        let c = self.scan_character_code(font_character_code_primitive(p));
                        let ex = match self.eqtb.expand.get(f) {
                            Some(x) => x,
                            None => {
                                v = -1;
                                break 'scan_loop;
                            }
                        };
                        v = match p {
                            Prim::EfCode => ex.ef_code(c),
                            Prim::LpCode => ex.lp_code(c),
                            Prim::RpCode => ex.rp_code(c),
                            Prim::TagCode => self.get_tag_code(f as u16, c),
                            Prim::KnBsCode => ex.kn_bs_code(c),
                            Prim::StBsCode => ex.st_bs_code(c),
                            Prim::ShBsCode => ex.sh_bs_code(c),
                            Prim::KnBcCode => ex.kn_bc_code(c),
                            _ => ex.kn_ac_code(c),
                        } as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::Dimen) => {
                        let i = self.scan_reg_num();
                        v = self.eqtb.dimen[i as usize] as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::DimP(p)) => {
                        v = self.dim_param_value(p) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::LastPenalty) => {
                        v = self.last_penalty_value() as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::LastKern) => {
                        v = self.last_kern_value() as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::LastSkip) => {
                        v = self.last_skip_value().width as i64;
                        break 'scan_loop;
                    }
                    Some(
                        p @ (Prim::PdfLastObj
                        | Prim::PdfLastXForm
                        | Prim::PdfLastXImage
                        | Prim::PdfLastXImagePages
                        | Prim::PdfLastLink
                        | Prim::PdfLastAnnot),
                    ) => {
                        v = self.pdf_last_value(p) as i64;
                        break 'scan_loop;
                    }
                    Some(
                        p @ (Prim::FontCharWd
                        | Prim::FontCharHt
                        | Prim::FontCharDp
                        | Prim::FontCharIc),
                    ) => {
                        v = self.scan_font_char_dimen(p) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::FontDimen) => {
                        let idx = self.scan_int();
                        let f = self.scan_font_id();
                        let i = if idx > 0 { idx as usize - 1 } else { 0 };
                        v = self.eqtb.font_params[f as usize]
                            .get(i)
                            .copied()
                            .unwrap_or(0) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::PdfXImageBBox) => {
                        v = self.scan_pdf_ximage_bbox() as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::XeTeXCharClass) => {
                        v = self.scan_xetex_charclass_val() as i64;
                        break 'scan_loop;
                    }
                    Some(
                        p @ (Prim::XeTeXVersion
                        | Prim::XeTeXFontType
                        | Prim::XeTeXCountGlyphs
                        | Prim::XeTeXGlyphIndex
                        | Prim::XeTeXCharGlyph
                        | Prim::XeTeXGlyphBounds
                        | Prim::XeTeXUseGlyphMetrics
                        | Prim::XeTeXInterCharTokenState
                        | Prim::XeTeXCountFeatures
                        | Prim::XeTeXFeatureCode
                        | Prim::XeTeXCountVariations
                        | Prim::XeTeXVariation
                        | Prim::XeTeXInputNormalization
                        | Prim::XeTeXGenerateActualText
                        | Prim::XeTeXDashBreakState),
                    ) => {
                        v = self.scan_xetex_int_query(p) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::LuaTeXVersion) => {
                        v = 124;
                        break 'scan_loop;
                    }
                    Some(Prim::OutputMode) => {
                        v = 1;
                        break 'scan_loop;
                    }
                    Some(Prim::CatCodeTable) => {
                        v = self.cur_catcode_table as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::Skip) => {
                        let i = self.scan_reg_num();
                        v = self.eqtb.skip[i as usize].width as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::GlueP(p)) => {
                        v = self.eqtb.glue_params[p.idx() as usize].width as i64;
                        break 'scan_loop;
                    }
                    _ => match self.eqtb.resolve(t.cs_id()).cloned() {
                        Some(Equiv::CountReg(i)) => {
                            v = self.eqtb.count[i as usize] as i64;
                            break 'scan_loop;
                        }
                        Some(Equiv::CharDef(c)) => {
                            v = c as i64;
                            break 'scan_loop;
                        }
                        Some(Equiv::MathCharDef(c)) => {
                            v = c as i64;
                            break 'scan_loop;
                        }
                        Some(Equiv::DimenReg(i)) => {
                            v = self.eqtb.dimen[i as usize] as i64;
                            break 'scan_loop;
                        }
                        Some(Equiv::SkipReg(i)) => {
                            v = self.eqtb.skip[i as usize].width as i64;
                            break 'scan_loop;
                        }
                        Some(Equiv::MuSkipReg(i)) => {
                            v = self.eqtb.muskip[i as usize].width as i64;
                            break 'scan_loop;
                        }
                        Some(Equiv::Prim(Prim::GlueP(p))) => {
                            v = self.eqtb.glue_params[p.idx() as usize].width as i64;
                            break 'scan_loop;
                        }
                        Some(Equiv::Prim(Prim::DimP(p))) => {
                            v = self.dim_param_value(p) as i64;
                            break 'scan_loop;
                        }
                        Some(Equiv::Prim(Prim::IntP(p))) => {
                            v = self.int_param_value(p) as i64;
                            break 'scan_loop;
                        }
                        _ => {}
                    },
                }
            }
            self.push_token(t);

            // tex.web \S470: a char token ends the number with an error; a
            // control sequence (e.g. a frozen \protected macro stopping an
            // f-expansion) ends it SILENTLY — the token was already pushed
            // back above.
            if !t.is_cs() {
                self.error("Missing number, treated as zero");
            }
            v = 0;
            break;
        }
        let r = v as i32;
        if negate {
            -r
        } else {
            r
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn local_clock() -> (i32, i32, i32, i32) {
        let mut t: libc::time_t = 0;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe {
            libc::time(&mut t);
            #[cfg(unix)]
            libc::localtime_r(&t, &mut tm);
            #[cfg(windows)]
            libc::localtime_s(&mut tm, &t);
        }
        (
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour * 60 + tm.tm_min,
        )
    }

    pub fn int_param_value(&self, p: IntParam) -> i32 {
        match p {
            IntParam::CurrentGroupLevel => (self.eqtb.cur_level.saturating_sub(1)) as i32,
            IntParam::CurrentGroupType => {
                if self.scanner_status == crate::engine::ScannerStatus::Aligning {
                    if self.align_in_noalign {
                        return 7; // no_align_group
                    }
                    if self.align_phase() == crate::align::PH_IDLE {
                        return 6; // align_group
                    }
                }
                let ty = self.eqtb.cur_group_type();
                let v = match ty {
                    None => 0,
                    Some(crate::eqtb::LevelType::Simple) => 1,
                    Some(crate::eqtb::LevelType::SemiSimple) => 14,
                    Some(crate::eqtb::LevelType::MathShift) => 15,
                    Some(crate::eqtb::LevelType::MathLeft) => 16,
                    Some(crate::eqtb::LevelType::MathGroup) => 9,
                    Some(crate::eqtb::LevelType::Group) => 9,
                    Some(crate::eqtb::LevelType::Box) => match self.box_kinds.last().copied() {
                        Some(0) => 2,
                        Some(1) => 4,
                        Some(2) => 5,
                        Some(3) => 12,
                        Some(7) => 6,
                        _ => 2,
                    },
                    _ => 1,
                };
                v
            }
            IntParam::CurrentIfLevel => self.if_stack.len() as i32,
            IntParam::CurrentIfType => 0,
            IntParam::CurrentIfBranch => 0,
            IntParam::LastNodeType => self.last_node_type_value(),
            IntParam::Badness => self.last_badness,
            IntParam::InputLineNo => self.current_diagnostic_line() as i32,
            IntParam::SpaceFactor => self.space_factor,
            IntParam::PrevGraf => self.prev_graf(),
            IntParam::Time | IntParam::Day | IntParam::Month | IntParam::Year => {
                // TeX Live / Web2C §241: \time, \day, \month, \year are initialized
                // from system local time (or SOURCE_DATE_EPOCH in UTC when set).
                let (year, month, day, time_mins) = if let Some(epoch) = tex_kpse::fs::epoch() {
                    crate::clock::utc(epoch as i64)
                } else if let Some(epoch) = std::env::var("SOURCE_DATE_EPOCH")
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                {
                    crate::clock::utc(epoch)
                } else {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        Self::local_clock()
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        crate::clock::utc(0)
                    }
                };
                match p {
                    IntParam::Time => time_mins,
                    IntParam::Day => day,
                    IntParam::Month => month,
                    _ => year,
                }
            }
            _ => self.eqtb.int_params[p.idx() as usize],
        }
    }

    pub fn dim_param_value(&self, p: DimParam) -> i32 {
        match p {
            DimParam::PrevDepth => self.prev_depth,
            // fire_up has reset the next page's counters, but an active
            // output routine reads the completed page's register snapshot.
            DimParam::PageGoal
            | DimParam::PageTotal
            | DimParam::PageDepth
            | DimParam::PageStretch
            | DimParam::PageFilStretch
            | DimParam::PageFillStretch
            | DimParam::PageFilllStretch
            | DimParam::PageShrink
                if self.in_output =>
            {
                self.eqtb.dim_params[p.idx() as usize]
            }
            DimParam::PageGoal => {
                let pg = if !self.page_goal_set {
                    0x3FFF_FFFF
                } else {
                    self.page_goal
                };
                pg.clamp(i32::MIN as i64, i32::MAX as i64) as i32
            }
            DimParam::PageTotal => self.page_total.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            DimParam::PageDepth => self.page_depth.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            DimParam::PageStretch => {
                self.page_stretch[0].clamp(i32::MIN as i64, i32::MAX as i64) as i32
            }
            DimParam::PageFilStretch => {
                self.page_stretch[1].clamp(i32::MIN as i64, i32::MAX as i64) as i32
            }
            DimParam::PageFillStretch => {
                self.page_stretch[2].clamp(i32::MIN as i64, i32::MAX as i64) as i32
            }
            DimParam::PageFilllStretch => {
                self.page_stretch[3].clamp(i32::MIN as i64, i32::MAX as i64) as i32
            }
            DimParam::PageShrink => {
                self.page_shrink[0].clamp(i32::MIN as i64, i32::MAX as i64) as i32
            }
            _ => self.eqtb.dim_params[p.idx() as usize],
        }
    }

    /// \pdflastobj, \pdflastxform, \pdflastximage, \pdflastlink,
    /// \pdflastannot: object number of the last allocated PDF object of
    /// that kind (0 before any allocation).
    pub fn pdf_last_value(&self, p: Prim) -> i32 {
        match p {
            Prim::PdfLastObj => self.pdf_last_obj,
            Prim::PdfLastXForm => self.pdf_last_xform,
            Prim::PdfLastXImage => self.pdf_last_ximage,
            Prim::PdfLastXImagePages => self.pdf_last_ximage_pages,
            Prim::PdfLastLink => self.pdf_last_link,
            Prim::PdfLastAnnot => self.pdf_last_annot,
            _ => 0,
        }
    }

    pub fn scan_reg_num(&mut self) -> u16 {
        let (n, source) = self.scan_int_with_source();
        let max = self.eqtb.count.len() as i32 - 1;
        if n < 0 || n > max {
            self.error_at(
                &format!(
                    "Register number {n} is out of range; expected a number from 0 through {max}"
                ),
                source,
            );
            return 0;
        }
        n as u16
    }

    /// Scan a character/integer operand and retain the first source token,
    /// before numeric lookahead advances to the following delimiter.
    pub(crate) fn scan_int_with_source(&mut self) -> (i32, Option<SourceContext>) {
        self.skip_spaces_relax();
        let origin = self.numeric_origin();
        let value = self.scan_int();
        let source = origin.and_then(|origin| self.numeric_origin_context(origin));
        (value, source)
    }

    /// Scan an operand used to index one of TeX's 256-entry character tables.
    /// Invalid input recovers with character zero instead of reaching an
    /// unchecked slice index.
    pub(crate) fn scan_character_code(&mut self, command: &str) -> u8 {
        let (character, source) = self.scan_int_with_source();
        if (0..=255).contains(&character) {
            character as u8
        } else {
            self.error_at(
                &format!(
                    "Character code {character} is out of range for {command}; expected 0 through 255 and used character 0"
                ),
                source,
            );
            0
        }
    }

    pub(crate) fn scan_unicode_character_code(&mut self, command: &str) -> u32 {
        let (character, source) = self.scan_int_with_source();
        if u32::try_from(character)
            .ok()
            .and_then(char::from_u32)
            .is_some()
        {
            character as u32
        } else {
            self.error_at(
                &format!("Invalid Unicode scalar {character} for {command}; used character 0"),
                source,
            );
            0
        }
    }

    pub(crate) fn valid_profile_character_code(&self, character: i32) -> Option<u32> {
        if self.engine_kind == EngineKind::PdfTeX {
            u8::try_from(character).ok().map(u32::from)
        } else {
            u32::try_from(character)
                .ok()
                .filter(|&scalar| char::from_u32(scalar).is_some())
        }
    }
    pub(crate) fn scan_profile_character_code(&mut self, command: &str) -> u32 {
        if self.engine_kind == EngineKind::PdfTeX {
            u32::from(self.scan_character_code(command))
        } else {
            self.scan_unicode_character_code(command)
        }
    }

    pub(crate) fn scan_math_family(&mut self, command: &str) -> usize {
        let (family, source) = self.scan_int_with_source();
        if (0..=15).contains(&family) {
            family as usize
        } else {
            self.error_at(
                &format!(
                    "Font family {family} is out of range for {command}; expected 0 through 15 and used family 0"
                ),
                source,
            );
            0
        }
    }

    /// TeX scan_dimen. mu: scan in math units. inf_ok: allow fil/fill/filll
    /// keywords (returns stretch in Glue form via scan_glue instead).
    pub fn scan_dimen(&mut self, mu: bool, trail: bool) -> i32 {
        let prev = self.in_expanded_scan;
        self.in_expanded_scan = false;
        let r = self.scan_dimen_inner(mu, trail);
        self.in_expanded_scan = prev;
        r
    }

    fn scan_math_style_param(&mut self) {
        self.skip_spaces_relax();
        let tok = self.get_token();
        if tok.is_char() && tok.chr() >= u32::from(b'0') && tok.chr() <= u32::from(b'9') {
            self.push_token(tok);
            let _ = self.scan_int();
        }
    }

    fn scan_dimen_inner(&mut self, mu: bool, _trail: bool) -> i32 {
        // signs
        let mut negate = false;
        loop {
            self.skip_spaces_relax();
            let t = self.get_x_raw();
            if t.is_char() && t.chr() == b'+' as u32 {
                continue;
            }
            if t.is_char() && t.chr() == b'-' as u32 {
                negate = !negate;
                continue;
            }
            self.push_token(t);
            break;
        }
        // factor: integer or decimal, or a direct dimen source
        // tex.web §445-448: the factor is kept as (int_part, f) with f in
        // 2^-16 units via round_decimals' truncating digit loop — f64
        // products round differently by a few sp and flip glue/badness
        // boundaries (observed: newtx `.2em` = 157283sp in real pdftex)
        let int_part: i64;
        let frac_f: i64;
        let direct: Option<i32>;
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        let factor_origin = self.numeric_origin();
        if Self::is_digit_token(t)
            || (t.is_char() && (t.chr() == b'.' as u32 || t.chr() == b',' as u32))
        {
            let mut ip: i64 = 0;
            if Self::is_digit_token(t) {
                ip = (t.chr() - b'0' as u32) as i64;
                let mut overflowed = false;
                let mut overflow_source = None;
                loop {
                    let t2 = self.get_x_raw();
                    if Self::is_digit_token(t2) {
                        ip = ip * 10 + (t2.chr() - b'0' as u32) as i64;
                        if ip > 0x7FFF_FFFF {
                            ip = 0x7FFF_FFFF;
                            if !overflowed {
                                overflowed = true;
                                overflow_source = self
                                    .numeric_origin()
                                    .and_then(|origin| self.numeric_origin_context(origin));
                            }
                        }
                    } else {
                        self.push_token(t2);
                        break;
                    }
                }
                if overflowed {
                    self.error_at("Number too big", overflow_source);
                }
            }
            // fraction digits (all consumed even past precision, tex.web §102)
            let mut digits: Vec<u8> = Vec::new();
            let mut seen_point = false;
            loop {
                let t2 = self.get_x_raw();
                if t2.is_char()
                    && (t2.chr() == b'.' as u32 || t2.chr() == b',' as u32)
                    && !seen_point
                {
                    seen_point = true;
                    continue;
                }
                if Self::is_digit_token(t2) {
                    if digits.len() < 17 {
                        digits.push((t2.chr() - b'0' as u32) as u8);
                    }
                } else {
                    self.push_token(t2);
                    break;
                }
            }
            // round_decimals (tex.web §2189): truncating per-digit loop,
            // result in 2^-16 units
            let mut a: i64 = 0;
            for &d in digits.iter().rev() {
                a = (a + d as i64 * 131072) / 10;
            }
            int_part = ip;
            frac_f = (a + 1) / 2;
            direct = None;
        } else if t.is_char() && t.chr() == b'`' as u32 {
            let t2 = self.raw_token();
            if t2.is_char() {
                int_part = t2.chr() as i64;
                // tex.web §442: undo raw_token's brace-depth adjustment
                if t2.cc() == 2 {
                    self.align_brace_depth = self.align_brace_depth.saturating_add(1);
                } else if t2.cc() == 1 {
                    self.align_brace_depth = self.align_brace_depth.saturating_sub(1);
                }
            } else if t2.is_cs() {
                let name = self.cs.name(t2.cs_id());
                int_part = name.first().copied().unwrap_or(0) as i64;
            } else {
                self.error("Missing character after `");
                int_part = 0;
            }
            frac_f = 0;
            direct = None;
        } else if t.is_cs() {
            match self.cur_prim {
                Some(Prim::DimExpr) => {
                    let v = self.scan_expr_dim();
                    return if negate { -v } else { v };
                }
                Some(Prim::NumExpr) => {
                    int_part = self.scan_expr_num() as i64;
                    frac_f = 0;
                    direct = None;
                }
                Some(Prim::GlueExpr) | Some(Prim::MuExpr) => {
                    let g = self.scan_expr_glue(mu);
                    return if negate { -g.width } else { g.width };
                }
                Some(Prim::DimP(p)) => {
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.dim_param_value(p));
                }
                Some(Prim::GlueP(p)) => {
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.eqtb.glue_params[p.idx() as usize].width);
                }
                Some(Prim::Skip) => {
                    let i = self.scan_reg_num();
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.eqtb.skip[i as usize].width);
                }
                Some(Prim::MuSkip) => {
                    let i = self.scan_reg_num();
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.eqtb.muskip[i as usize].width);
                }
                Some(Prim::Wd) => {
                    let n = self.scan_reg_num();
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.box_reg_dimen(n, 0));
                }
                Some(Prim::Ht) => {
                    let n = self.scan_reg_num();
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.box_reg_dimen(n, 1));
                }
                Some(Prim::Dp) => {
                    let n = self.scan_reg_num();
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.box_reg_dimen(n, 2));
                }
                Some(Prim::LastSkip) => {
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.last_skip_value().width);
                }
                Some(Prim::LastKern) => {
                    // tex.web scan_something_internal: \lastkern is a direct
                    // dimen. Without this arm scan_dimen falls to "Missing
                    // number", pushing \lastkern back so \ifdim\lastkern=3sp
                    // leaks "=3sp" into the page (footmisc multiple-marker
                    // \ifdim\lastkern=\multiplefootnotemarker).
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.last_kern_value());
                }
                Some(Prim::LastPenalty) => {
                    // internal integer coerced to sp, like Count above
                    int_part = self.last_penalty_value() as i64;
                    frac_f = 0;
                    direct = None;
                }
                Some(Prim::PdfLastXPos) => {
                    int_part = self.pdf_last_x as i64;
                    frac_f = 0;
                    direct = None;
                }
                Some(Prim::PdfLastYPos) => {
                    int_part = self.pdf_last_y as i64;
                    frac_f = 0;
                    direct = None;
                }
                Some(Prim::GlueStretch) => {
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.scan_etex_glue_field(0));
                }
                Some(Prim::GlueShrink) => {
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.scan_etex_glue_field(1));
                }
                Some(
                    p @ (Prim::FontCharWd | Prim::FontCharHt | Prim::FontCharDp | Prim::FontCharIc),
                ) => {
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.scan_font_char_dimen(p));
                }
                Some(Prim::FontDimen) => {
                    let idx = self.scan_int();
                    let f = self.scan_font_id();
                    let i = if idx > 0 { idx as usize - 1 } else { 0 };
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(
                        self.eqtb.font_params[f as usize]
                            .get(i)
                            .copied()
                            .unwrap_or(0),
                    );
                }
                Some(Prim::PdfXImageBBox) => {
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.scan_pdf_ximage_bbox());
                }
                Some(Prim::Umathfractiondelsize) => {
                    self.scan_math_style_param();
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(20 * 65536);
                }
                Some(Prim::Umathstacknumup | Prim::Umathstackdenomdown) => {
                    self.scan_math_style_param();
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(6 * 65536);
                }
                Some(Prim::Umathstackvgap) => {
                    self.scan_math_style_param();
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(2 * 65536);
                }
                Some(Prim::Count | Prim::Attribute) => {
                    // internal integer coerced to dimen (sp), tex.web scan_something_internal
                    let i = self.scan_reg_num();
                    int_part = self.eqtb.count[i as usize] as i64;
                    frac_f = 0;
                    direct = None;
                }
                Some(Prim::Dimen) => {
                    let i = self.scan_reg_num();
                    int_part = 1;
                    frac_f = 0;
                    direct = Some(self.eqtb.dimen[i as usize]);
                }
                _ => match self.eqtb.resolve(t.cs_id()).cloned() {
                    Some(Equiv::DimenReg(i)) => {
                        int_part = 1;
                        frac_f = 0;
                        direct = Some(self.eqtb.dimen[i as usize]);
                    }
                    Some(Equiv::SkipReg(i)) => {
                        int_part = 1;
                        frac_f = 0;
                        direct = Some(self.eqtb.skip[i as usize].width);
                    }
                    Some(Equiv::MuSkipReg(i)) => {
                        int_part = 1;
                        frac_f = 0;
                        direct = Some(self.eqtb.muskip[i as usize].width);
                    }
                    Some(Equiv::CountReg(i)) => {
                        int_part = self.eqtb.count[i as usize] as i64;
                        frac_f = 0;
                        direct = None;
                    }
                    Some(Equiv::CharDef(c)) => {
                        int_part = c as i64;
                        frac_f = 0;
                        direct = None;
                    }
                    Some(Equiv::MathCharDef(c)) => {
                        // \@m/\@M constants (\mathchardef'd); \offinterlineskip
                        // computes \baselineskip-\@m\p@ through this path.
                        int_part = c as i64;
                        frac_f = 0;
                        direct = None;
                    }
                    Some(Equiv::Prim(Prim::IntP(p))) => {
                        int_part = self.int_param_value(p) as i64;
                        frac_f = 0;
                        direct = None;
                    }
                    _ => {
                        self.push_token(t);
                        self.error("Missing number, treated as zero");
                        int_part = 0;
                        frac_f = 0;
                        direct = None;
                    }
                },
            }
        } else {
            self.push_token(t);

            self.error("Missing number, treated as zero");
            int_part = 0;
            frac_f = 0;
            direct = None;
        }
        if let Some(d) = direct {
            return if negate { -d } else { d };
        }
        // unit (tex.web §453): standard units scale with (num, denom)
        // while internal dimensions multiply directly
        let unit = self.scan_unit(mu);
        // tex.web §8868: goto attach_sign if the units are internal (no optional space)
        if !matches!(unit, UnitKind::InternalSp(_)) {
            self.scan_optional_space();
        }
        let v: i64 = match unit {
            UnitKind::Ratio(num, denom) => {
                let quotient = (int_part * num) / denom;
                let remainder = (int_part * num) % denom;
                let f_new = (num * frac_f as i64 + 65536 * remainder) / denom;
                let cur_val = quotient + (f_new / 65536);
                let f_final = f_new % 65536;
                cur_val * 65536 + f_final
            }
            UnitKind::InternalSp(unit_sp) | UnitKind::FontRelativeSp(unit_sp) => {
                let v = int_part as i128 * 65536 + frac_f as i128;
                ((v * unit_sp as i128) / 65536) as i64
            }
            UnitKind::Sp => int_part,
        };
        const MAX_DIMEN: i64 = 0x3FFF_FFFF;
        let v = if (-MAX_DIMEN..=MAX_DIMEN).contains(&v) {
            v
        } else {
            let source = factor_origin.and_then(|origin| self.numeric_origin_context(origin));
            self.error_at("Dimension too large", source);
            v.clamp(-MAX_DIMEN, MAX_DIMEN)
        } as i32;
        if negate {
            -v
        } else {
            v
        }
    }

    /// scan glue: width [plus stretch] [minus shrink]
    /// tex.web scan_dimen unit fetch: `<optional spaces> <unit>` where the
    /// unit may be a letter pair, a dimen parameter/register, or a macro
    /// that EXPANDS to one of those (`\p@` -> pt).
    fn scan_unit_sp(&mut self, mu: bool) -> i64 {
        match self.scan_unit(mu) {
            UnitKind::Ratio(n, d) => (n * 65536) / d,
            UnitKind::InternalSp(sp) | UnitKind::FontRelativeSp(sp) => sp,
            UnitKind::Sp => 1,
        }
    }

    fn scan_unit(&mut self, mu: bool) -> UnitKind {
        self.scan_unit_d(mu, 0)
    }

    fn scan_unit_d(&mut self, mu: bool, depth: u32) -> UnitKind {
        if depth > 32 {
            self.error("Illegal unit of measure (pt inserted).");
            return UnitKind::Ratio(1, 1);
        }
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_cs() {
            let equiv = self.eqtb.resolve(t.cs_id()).cloned();
            if let Some(value) = equiv.as_ref().and_then(|equiv| self.eqtb.int_of(equiv)) {
                // tex.web scan_dimen: every internal integer accepted here
                // denotes that many scaled points. This includes chardef and
                // mathchardef constants as well as count-register aliases.
                return UnitKind::InternalSp(i64::from(value));
            }
            match equiv {
                Some(Equiv::Prim(Prim::GlueP(p))) => {
                    return UnitKind::InternalSp(
                        self.eqtb.glue_params[p.idx() as usize].width as i64,
                    );
                }
                Some(Equiv::Prim(Prim::Skip)) => {
                    let i = self.scan_reg_num();
                    return UnitKind::InternalSp(self.eqtb.skip[i as usize].width as i64);
                }
                Some(Equiv::Prim(Prim::MuSkip)) => {
                    let i = self.scan_reg_num();
                    return UnitKind::InternalSp(self.eqtb.muskip[i as usize].width as i64);
                }
                Some(Equiv::Prim(Prim::LastSkip)) => {
                    return UnitKind::InternalSp(self.last_skip_value().width as i64);
                }
                Some(Equiv::SkipReg(i)) => {
                    return UnitKind::InternalSp(self.eqtb.skip[i as usize].width as i64);
                }
                Some(Equiv::MuSkipReg(i)) => {
                    return UnitKind::InternalSp(self.eqtb.muskip[i as usize].width as i64);
                }
                Some(Equiv::Prim(Prim::DimExpr)) => {
                    return UnitKind::InternalSp(self.scan_expr_dim() as i64);
                }
                Some(Equiv::Prim(Prim::DimP(p))) => {
                    return UnitKind::InternalSp(self.dim_param_value(p) as i64);
                }
                Some(Equiv::Prim(Prim::Wd)) => {
                    let n = self.scan_reg_num();
                    return UnitKind::InternalSp(self.box_reg_dimen(n, 0) as i64);
                }
                Some(Equiv::Prim(Prim::Ht)) => {
                    let n = self.scan_reg_num();
                    return UnitKind::InternalSp(self.box_reg_dimen(n, 1) as i64);
                }
                Some(Equiv::Prim(Prim::Dp)) => {
                    let n = self.scan_reg_num();
                    return UnitKind::InternalSp(self.box_reg_dimen(n, 2) as i64);
                }
                Some(Equiv::Prim(
                    p @ (Prim::FontCharWd | Prim::FontCharHt | Prim::FontCharDp | Prim::FontCharIc),
                )) => {
                    return UnitKind::InternalSp(self.scan_font_char_dimen(p) as i64);
                }
                Some(Equiv::Prim(Prim::FontDimen)) => {
                    let idx = self.scan_int();
                    let f = self.scan_font_id();
                    let i = if idx > 0 { idx as usize - 1 } else { 0 };
                    let sp = self
                        .eqtb
                        .font_params
                        .get(f as usize)
                        .and_then(|fp| fp.get(i))
                        .copied()
                        .unwrap_or(0);
                    return UnitKind::InternalSp(sp as i64);
                }
                Some(Equiv::Prim(Prim::PdfXImageBBox)) => {
                    return UnitKind::InternalSp(self.scan_pdf_ximage_bbox() as i64);
                }
                Some(Equiv::Prim(Prim::Dimen)) => {
                    let i = self.scan_reg_num();
                    return UnitKind::InternalSp(self.eqtb.dimen[i as usize] as i64);
                }
                Some(Equiv::DimenReg(i)) => {
                    return UnitKind::InternalSp(self.eqtb.dimen[i as usize] as i64);
                }
                Some(Equiv::Prim(Prim::Count | Prim::Attribute)) => {
                    let i = self.scan_reg_num();
                    UnitKind::InternalSp(self.eqtb.count[i as usize] as i64)
                }
                Some(Equiv::CountReg(i)) => {
                    // tex.web scan_dimen: an internal integer in unit
                    // position denotes that many scaled points. Xy-pic
                    // relies on `\dimen@=-3\K@` with \K@ a count register.
                    UnitKind::InternalSp(self.eqtb.count[i as usize] as i64)
                }
                Some(Equiv::Macro(m)) if !m.protected => {
                    self.expand_macro(t.cs_id(), &m, t.cs_id());
                    return self.scan_unit_d(mu, depth + 1);
                }
                _ => {
                    self.push_token(t);
                    self.error("Illegal unit of measure (pt inserted).");
                    return UnitKind::Ratio(1, 1);
                }
            }
        } else if t.is_char() {
            if ascii_character(t).is_some_and(|c| c.eq_ignore_ascii_case(&b't')) {
                let t2 = self.get_token();
                if ascii_character(t2).is_some_and(|c| c.eq_ignore_ascii_case(&b'r')) {
                    let t3 = self.get_token();
                    if ascii_character(t3).is_some_and(|c| c.eq_ignore_ascii_case(&b'u')) {
                        let t4 = self.get_token();
                        if ascii_character(t4).is_some_and(|c| c.eq_ignore_ascii_case(&b'e')) {
                            self.skip_spaces();
                            return self.scan_unit_d(mu, depth + 1);
                        } else {
                            self.push_token(t4);
                            self.push_token(t3);
                            self.push_token(t2);
                        }
                    } else {
                        self.push_token(t3);
                        self.push_token(t2);
                    }
                } else {
                    self.push_token(t2);
                }
            }
            // read unit keyword letters, only while they can extend a valid unit
            const UNITS: [&str; 16] = [
                "pt", "in", "pc", "cm", "mm", "bp", "dd", "cc", "sp", "em", "ex", "px", "mu",
                "fil", "fill", "filll",
            ];
            let Some(first) = ascii_character(t) else {
                self.push_token(t);
                self.error("Illegal unit of measure (pt inserted).");
                return UnitKind::Ratio(1, 1);
            };
            let mut kw: Vec<u8> = vec![first];
            let mut cur = String::new();
            cur.push(first.to_ascii_lowercase() as char);
            if kw[0].is_ascii_alphabetic() {
                while kw.len() < 5 {
                    let can_extend = UNITS
                        .iter()
                        .any(|u| u.len() > kw.len() && u.starts_with(&cur));
                    if !can_extend {
                        break;
                    }
                    let t2 = self.get_token();
                    if let Some(character) = ascii_character(t2).filter(u8::is_ascii_alphabetic) {
                        kw.push(character);
                        cur.push(character.to_ascii_lowercase() as char);
                    } else {
                        self.push_token(t2);
                        break;
                    }
                }
            }
            let mut s: String = kw.iter().map(|&b| b.to_ascii_lowercase() as char).collect();
            // trim over-read letters back to the longest valid unit keyword
            if !UNITS.contains(&s.as_str()) {
                let mut excess: Vec<Token> = Vec::new();
                while kw.len() > 1 && !UNITS.contains(&s.as_str()) {
                    let last = kw.pop().unwrap();
                    excess.push(Token::char(self.eqtb.cat[last as usize], last as u32));
                    s.pop();
                }
                for t in excess.into_iter().rev() {
                    self.push_token(t)
                }
                let s2: String = kw.iter().map(|&b| b.to_ascii_lowercase() as char).collect();
                s = s2;
            }
            self.cur_fill_order = 0;
            match s.as_str() {
                "fil" | "fill" | "filll" => {
                    self.cur_fill_order = match s.as_str() {
                        "fil" => 1,
                        "fill" => 2,
                        _ => 3,
                    };
                    UnitKind::Ratio(1, 1)
                }
                "pt" | "p" => UnitKind::Ratio(1, 1),
                "in" => UnitKind::Ratio(7227, 100),
                "pc" => UnitKind::Ratio(12, 1),
                "cm" => UnitKind::Ratio(7227, 254),
                "mm" => UnitKind::Ratio(7227, 2540),
                "bp" => UnitKind::Ratio(7227, 7200),
                "dd" => UnitKind::Ratio(1238, 1157),
                "cc" => UnitKind::Ratio(14856, 1157),
                "sp" => UnitKind::Sp,
                "em" => UnitKind::FontRelativeSp(self.cur_quad() as i64),
                "ex" => UnitKind::FontRelativeSp(self.cur_x_height() as i64),
                "px" => UnitKind::Ratio(7227, 7200),
                "mu" if mu => UnitKind::Ratio(1, 1),
                _ => {
                    self.error("Illegal unit of measure (pt inserted).");
                    UnitKind::Ratio(1, 1)
                }
            }
        } else {
            self.push_token(t);
            self.error("Illegal unit of measure (pt inserted).");
            UnitKind::Ratio(1, 1)
        }
    }

    pub fn scan_glue(&mut self, mu: bool) -> Glue {
        let prev = self.in_expanded_scan;
        self.in_expanded_scan = false;
        self.skip_spaces_relax();
        // tex.web scan_glue: optional signs are consumed here and negate the
        // WHOLE glue spec (width, stretch, shrink) — not just the width.
        // `\skip0=-\skip1` must yield -w plus -s minus -k; dropping the
        // component signs cost LaTeX \@startsection its beforeskip
        // stretch/shrink (`\@tempskipa -\@tempskipa`).
        let mut negate = false;
        let mut t = self.get_x_raw();
        while t.is_char() && (t.chr() == 43 || t.chr() == 45) {
            if t.chr() == 45 {
                negate = !negate;
            }
            self.skip_spaces_relax();
            t = self.get_x_raw();
        }
        if t.is_cs() && matches!(self.cur_prim, Some(Prim::GlueExpr) | Some(Prim::MuExpr)) {
            let mut g = self.scan_expr_glue(mu);
            if negate {
                g.width = -g.width;
                g.stretch = -g.stretch;
                g.shrink = -g.shrink;
            }
            self.in_expanded_scan = prev;
            return g;
        }
        if let Some(mut g) = self.glue_from_cur_cs(t) {
            if negate {
                g.width = -g.width;
                g.stretch = -g.stretch;
                g.shrink = -g.shrink;
            }
            self.in_expanded_scan = prev;
            return g;
        }
        self.push_token(t);
        let mut g = Glue::zero();
        self.cur_fill_order = 0;
        g.width = self.scan_dimen(mu, false);
        if negate {
            g.width = -g.width;
        }
        for &(kw, is_stretch) in &[(&b"plus"[..], true), (&b"minus"[..], false)] {
            loop {
                let t0 = self.get_token();
                if t0.is_space() {
                    continue;
                }
                self.push_token(t0);
                break;
            }
            let t = self.get_token();
            let mut matched = false;
            if let Some(c) = ascii_character(t).map(|c| c.to_ascii_lowercase()) {
                if c == kw[0] {
                    let mut kt: Vec<Token> = Vec::new();
                    let mut all = true;
                    for &k in &kw[1..] {
                        let tx = self.get_token();
                        kt.push(tx);
                        if !ascii_character(tx).is_some_and(|c| c.to_ascii_lowercase() == k) {
                            all = false;
                            break;
                        }
                    }
                    if all {
                        matched = true;
                        if is_stretch {
                            g.stretch_order = self.cur_fill_order;
                            g.stretch = self.scan_dimen(mu, false);
                            if self.cur_fill_order > 0 {
                                g.stretch_order = self.cur_fill_order;
                            }
                        } else {
                            g.shrink_order = self.cur_fill_order;
                            g.shrink = self.scan_dimen(mu, false);
                            if self.cur_fill_order > 0 {
                                g.shrink_order = self.cur_fill_order;
                            }
                        }
                    } else {
                        for tx in kt.into_iter().rev() {
                            self.push_token(tx);
                        }
                    }
                }
            }
            if !matched {
                self.push_token(t);
            }
            self.cur_fill_order = 0;
        }
        g
    }
    /// after "fil" keyword letters: count extra 'l's for fil/fill/filll
    fn scan_fil_order(&mut self) -> u8 {
        let mut order = 1u8;
        loop {
            let t = self.get_token();
            if ascii_character(t) == Some(b'l') && order < 3 {
                order += 1;
            } else {
                self.push_token(t);
                return order;
            }
        }
    }

    pub fn scan_relational(&mut self) -> u8 {
        self.skip_spaces_relax();
        let t = self.get_x_raw_keep_cond();
        if let Some(c) = ascii_character(t) {
            if matches!(c, b'<' | b'=' | b'>') {
                return c;
            }
        }
        self.push_token(t);
        self.error("Expected a relational operator (`<`, `=`, or `>`); inserted `=`");
        b'='
    }

    /// scan a braced general text (raw, balanced); opening brace consumed by
    /// caller? Here: expects next token to be `{`; returns contents.
    pub fn scan_left_brace(&mut self) -> bool {
        loop {
            let t = self.get_x_raw();
            if t.is_space() {
                continue;
            }
            if t.is_char() && t.cc() == 1 {
                return true;
            }
            if t == crate::input::EOF_MARKER {
                self.error("Missing { inserted");
                return false;
            }
            let got = if t.is_cs() {
                self.display_cs(t.cs_id())
            } else {
                format!("cc{}:{}", t.cc(), t.chr())
            };
            self.error(&format!("Missing {{ inserted (got {})", got));
            self.push_token(t);
            return false;
        }
    }

    /// scan a braced general text (raw, balanced); opening brace consumed by
    /// caller? Here: expects next token to be `{`; returns contents.
    pub fn scan_general_text(&mut self) -> Vec<Token> {
        if !self.scan_left_brace() {
            return Vec::new();
        }
        self.scan_balanced_raw(true).into_vec()
    }

    /// like scan_general_text but expanding (\edef semantics)
    pub fn scan_general_text_expanded(&mut self) -> Vec<Token> {
        if !self.scan_left_brace() {
            return Vec::new();
        }
        let origin = self.current_token_source_mark();
        let prev_expanded_scan = self.in_expanded_scan;
        let prev_csname_depth = self.csname_depth;
        // \\expanded is an e-TeX edef context even when invoked from \\csname.
        // Leaving csname_depth>0 would expand \\protected macros and let them
        // steal \\endcsname / closing braces (utf8.def filehook sanitize).
        self.in_expanded_scan = true;
        self.csname_depth = 0;
        let mut out = Vec::new();
        let mut depth = 1i32;
        loop {
            let raw = self.raw_token();
            if raw == crate::input::EOF_MARKER {
                self.fatal_error_at(
                    "Missing } in expanded text",
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                self.in_expanded_scan = prev_expanded_scan;
                self.csname_depth = prev_csname_depth;
                return out;
            }
            // Copy the argument verbatim, but do not execute an \unexpanded
            // token already frozen by an enclosing expansion context.
            if raw.is_cs() && raw.0 < crate::expand::NOEXP_FLAG {
                if let Some(Equiv::Prim(Prim::UnExpanded)) = self.eqtb.resolve(raw.cs_id()) {
                    self.skip_spaces_relax();
                    let nxt = self.raw_token();
                    if nxt.is_cs() {
                        if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(nxt.cs_id()).cloned() {
                            let additional = self.eqtb.toks[i as usize].len();
                            if !self.scanned_token_list_has_room(
                                out.len(),
                                additional,
                                "expanded text size",
                                origin.as_ref(),
                            ) {
                                self.in_expanded_scan = prev_expanded_scan;
                                self.csname_depth = prev_csname_depth;
                                return out;
                            }
                            out.extend_from_slice(&self.eqtb.toks[i as usize]);
                            continue;
                        }
                    }
                    self.push_token(nxt);
                    let u = self.scan_general_text();
                    if self.stopped_on_error {
                        self.in_expanded_scan = prev_expanded_scan;
                        self.csname_depth = prev_csname_depth;
                        return out;
                    }
                    if !self.scanned_token_list_has_room(
                        out.len(),
                        u.len(),
                        "expanded text size",
                        origin.as_ref(),
                    ) {
                        self.in_expanded_scan = prev_expanded_scan;
                        self.csname_depth = prev_csname_depth;
                        return out;
                    }
                    out.extend(u);
                    continue;
                }
            }
            let t = self.get_token_from(raw);
            if t == crate::input::EOF_MARKER {
                self.fatal_error_at(
                    "Missing } in expanded text",
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                self.in_expanded_scan = prev_expanded_scan;
                self.csname_depth = prev_csname_depth;
                return out;
            }
            if self.cur_prim == Some(Prim::UnExpanded) {
                self.skip_spaces_relax();
                let nxt = self.raw_token();
                if nxt.is_cs() {
                    if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(nxt.cs_id()).cloned() {
                        let additional = self.eqtb.toks[i as usize].len();
                        if !self.scanned_token_list_has_room(
                            out.len(),
                            additional,
                            "expanded text size",
                            origin.as_ref(),
                        ) {
                            self.in_expanded_scan = prev_expanded_scan;
                            self.csname_depth = prev_csname_depth;
                            return out;
                        }
                        out.extend_from_slice(&self.eqtb.toks[i as usize]);
                        continue;
                    }
                }
                self.push_token(nxt);
                let u = self.scan_general_text();
                if self.stopped_on_error {
                    self.in_expanded_scan = prev_expanded_scan;
                    self.csname_depth = prev_csname_depth;
                    return out;
                }
                if !self.scanned_token_list_has_room(
                    out.len(),
                    u.len(),
                    "expanded text size",
                    origin.as_ref(),
                ) {
                    self.in_expanded_scan = prev_expanded_scan;
                    self.csname_depth = prev_csname_depth;
                    return out;
                }
                out.extend(u);
                continue;
            }
            if t.is_char() {
                if t.cc() == 1 {
                    depth += 1;
                } else if t.cc() == 2 {
                    depth -= 1;
                    if depth == 0 {
                        self.in_expanded_scan = prev_expanded_scan;
                        self.csname_depth = prev_csname_depth;
                        return out;
                    }
                }
            }
            if !self.scanned_token_list_has_room(
                out.len(),
                1,
                "expanded text size",
                origin.as_ref(),
            ) {
                self.in_expanded_scan = prev_expanded_scan;
                self.csname_depth = prev_csname_depth;
                return out;
            }
            out.push(self.unfreeze_input_token(t));
        }
    }

    // ---------- \the ----------

    /// implement \the: scans an internal quantity and pushes its expansion
    fn push_the_toks(&mut self, toks: Vec<Token>) {
        // Knuth: \\the\\toks inserts raw tokens. Freeze only inside edef/expanded
        // so the contents are not re-expanded while collecting. At execute time
        // (geometry \\the\\Gm@dimlist) macros like \\Gm@len must still expand.
        if self.in_expanded_scan && self.csname_depth == 0 {
            // tex.web hash doubling (TeXbook App D): \\the\\toks inside \\edef
            // doubles every literal # so that the edef body collapse (## -> #)
            // preserves the original count. Verified: real TeX gives
            // \toks0{\def\zz{VAL[##1]}} \edef\zzz{\the\toks0} -> meaning
            // prints VAL[####1]; collapsing instead (VAL[#1] as a param ref)
            // breaks pgfkeys .store in (self-assigning \def\ww{\ww}).
            let hash_count = toks
                .iter()
                .filter(|t| t.is_char() && t.cc() == 6 && t.chr() == 0x23)
                .count();
            let mut doubled = Vec::with_capacity(toks.len() + hash_count);
            for t in toks {
                doubled.push(t);
                if t.is_char() && t.cc() == 6 && t.chr() == 0x23 {
                    doubled.push(t);
                }
            }
            let mut doubled = self.freeze_unexpanded_toks(doubled);
            // The doubled hashes are definition syntax, not \unexpanded
            // tokens: collect_def_body must collapse each ## back to one #.
            for t in &mut doubled {
                if t.0 >= 0x1000_0000 && t.0 < 0x2000_0000 {
                    *t = t.unfreeze();
                }
            }
            self.push_tokens(doubled);
        } else {
            self.push_tokens(toks);
        }
    }

    fn capture_the_bytes(&mut self, bytes: &[u8]) {
        const LIMIT: usize = 8 * 1024;
        let end = bytes.len().min(LIMIT);
        let mut shown = String::from_utf8_lossy(&bytes[..end]).into_owned();
        if end < bytes.len() {
            shown.push('…');
        }
        self.pending_the_string = Some(shown);
    }

    fn emit_the_tokens(&mut self, tokens: Vec<Token>, capture_for_show: bool) {
        if capture_for_show {
            self.pending_the_string = Some(self.diagnostic_tokens_to_string(&tokens, 8 * 1024));
        } else {
            self.push_the_toks(tokens);
        }
    }

    pub fn the_scan(&mut self) {
        // \showthe needs the exact unexpanded representation that \the would
        // insert, but it must not leak those tokens back into the document.
        // Taking the marker before operand scanning also leaves nested
        // expandable \number/\the operations unaffected.
        let capture_for_show = self.pending_the_string.take().is_some();
        macro_rules! emit_the {
            ($bytes:expr) => {
                if capture_for_show {
                    self.capture_the_bytes($bytes);
                } else {
                    self.exp_string($bytes);
                }
            };
        }
        macro_rules! emit_the_toks {
            ($tokens:expr) => {{
                let tokens = $tokens;
                self.emit_the_tokens(tokens, capture_for_show);
            }};
        }
        self.skip_spaces();
        let t = self.get_token();
        if !t.is_cs() {
            self.push_token(t);
            self.error("You can't use `\\the' after ");
            return;
        }
        let id = t.cs_id();
        // register aliases (countdef'd/dimendef'd/skipdef'd/toksdef'd cs) and
        // toks registers are valid 	he operands (tex.web scan_toks part)
        match self.eqtb.resolve(id).cloned() {
            Some(Equiv::CharDef(c)) => {
                emit_the!(c.to_string().as_bytes());
                return;
            }
            Some(Equiv::MathCharDef(c)) => {
                emit_the!(c.to_string().as_bytes());
                return;
            }
            Some(Equiv::CountReg(i)) => {
                emit_the!(self.eqtb.count[i as usize].to_string().as_bytes());
                return;
            }
            Some(Equiv::DimenReg(i)) => {
                let s = self.scaled_to_string(self.eqtb.dimen[i as usize]);
                emit_the!(s.as_bytes());
                return;
            }
            Some(Equiv::SkipReg(i)) => {
                let g = self.eqtb.skip[i as usize].clone();
                let s = self.glue_to_string(&g);
                emit_the!(s.as_bytes());
                return;
            }
            Some(Equiv::MuSkipReg(i)) => {
                let g = self.eqtb.muskip[i as usize].clone();
                let s = self.mu_glue_to_string(&g);
                emit_the!(s.as_bytes());
                return;
            }
            Some(Equiv::ToksReg(i)) => {
                let toks = (*self.eqtb.toks[i as usize]).clone();
                emit_the_toks!(toks);
                return;
            }
            _ => {}
        }
        match self.cur_prim {
            Some(Prim::IntP(p)) => {
                let s = self.int_param_value(p).to_string();
                emit_the!(s.as_bytes());
            }
            Some(Prim::PdfShellEscape) => {
                emit_the!(b"0");
            }
            Some(Prim::PdfRandomSeed) => {
                let s = self.random_seed.to_string();
                emit_the!(s.as_bytes());
            }
            Some(Prim::PdfElapsedTime) => {
                emit_the!(b"0");
            }
            Some(Prim::Count | Prim::Attribute) => {
                let idx = self.scan_reg_num();
                emit_the!(self.eqtb.count[idx as usize].to_string().as_bytes());
            }
            Some(Prim::DimP(p)) => {
                let s = self.scaled_to_string(self.dim_param_value(p));
                emit_the!(s.as_bytes());
            }
            Some(Prim::GlueP(p)) => {
                let g = self.eqtb.glue_params[p.idx() as usize].clone();
                let s = if p.is_mu() {
                    self.mu_glue_to_string(&g)
                } else {
                    self.glue_to_string(&g)
                };
                emit_the!(s.as_bytes());
            }
            Some(Prim::ToksP(p)) => {
                let toks = (*self.eqtb.tok_params[p.idx() as usize]).clone();
                emit_the_toks!(toks);
            }
            Some(Prim::CatCode) => {
                let c = self.scan_profile_character_code("\\catcode");
                emit_the!(self.eqtb.cat_code(c).to_string().as_bytes());
            }
            Some(Prim::MathCode) => {
                let c = self.scan_profile_character_code("\\mathcode");
                emit_the!(self.eqtb.math_code_for(c).to_string().as_bytes());
            }
            Some(Prim::DelCode) => {
                let c = self.scan_profile_character_code("\\delcode");
                emit_the!(self.eqtb.delimiter_code_for(c).to_string().as_bytes());
            }
            Some(Prim::LcCodeP) => {
                let c = self.scan_profile_character_code("\\lccode");
                emit_the!(self.eqtb.case_code(c, false).to_string().as_bytes());
            }
            Some(Prim::SfCodeP) => {
                let c = self.scan_profile_character_code("\\sfcode");
                emit_the!(self.eqtb.space_factor_code(c).to_string().as_bytes());
            }
            Some(Prim::UcCodeP) => {
                let c = self.scan_profile_character_code("\\uccode");
                emit_the!(self.eqtb.case_code(c, true).to_string().as_bytes());
            }
            Some(
                p @ (Prim::FontCharWd | Prim::FontCharHt | Prim::FontCharDp | Prim::FontCharIc),
            ) => {
                let value = self.scan_font_char_dimen(p);
                emit_the!(self.scaled_to_string(value).as_bytes());
            }
            Some(Prim::FontDimen) => {
                let idx = self.scan_int();
                let f = self.scan_font_id();
                let i = if idx > 0 { idx as usize - 1 } else { 0 };
                let v = self
                    .eqtb
                    .font_params
                    .get(f as usize)
                    .and_then(|fp| fp.get(i))
                    .copied()
                    .unwrap_or(0);
                let s = self.scaled_to_string(v);
                emit_the!(s.as_bytes());
            }
            Some(Prim::PdfXImageBBox) => {
                let value = self.scan_pdf_ximage_bbox();
                emit_the!(self.scaled_to_string(value).as_bytes());
            }
            Some(Prim::HyphenChar) => {
                let f = self.scan_font_id() as usize;
                let v = self.eqtb.hyphen_char.get(f).copied().unwrap_or(0);
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::SkewChar) => {
                let f = self.scan_font_id() as usize;
                let v = self.eqtb.skew_char.get(f).copied().unwrap_or(0);
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::PdfNoLigatures) => {
                let f = self.scan_font_id() as usize;
                let v = self.test_no_ligatures(f as u16);
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::Umathfractiondelsize) => {
                self.scan_math_style_param();
                emit_the!(b"20.0pt");
            }
            Some(Prim::Umathstacknumup | Prim::Umathstackdenomdown) => {
                self.scan_math_style_param();
                emit_the!(b"6.0pt");
            }
            Some(Prim::Umathstackvgap) => {
                self.scan_math_style_param();
                emit_the!(b"2.0pt");
            }
            Some(
                p @ (Prim::EfCode
                | Prim::LpCode
                | Prim::RpCode
                | Prim::TagCode
                | Prim::KnBsCode
                | Prim::StBsCode
                | Prim::ShBsCode
                | Prim::KnBcCode
                | Prim::KnAcCode),
            ) => {
                let f = self.scan_font_id() as usize;
                let c = self.scan_character_code(font_character_code_primitive(p));
                let v = match self.eqtb.expand.get(f) {
                    Some(ex) => match p {
                        Prim::EfCode => ex.ef_code(c),
                        Prim::LpCode => ex.lp_code(c),
                        Prim::RpCode => ex.rp_code(c),
                        Prim::TagCode => self.get_tag_code(f as u16, c),
                        Prim::KnBsCode => ex.kn_bs_code(c),
                        Prim::StBsCode => ex.st_bs_code(c),
                        Prim::ShBsCode => ex.sh_bs_code(c),
                        Prim::KnBcCode => ex.kn_bc_code(c),
                        _ => ex.kn_ac_code(c),
                    },
                    None => -1,
                };
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::TopMark) => emit_the_toks!(self.mark_tokens_class(0, 0)),
            Some(Prim::FirstMark) => emit_the_toks!(self.mark_tokens_class(1, 0)),
            Some(Prim::BotMark) => emit_the_toks!(self.mark_tokens_class(2, 0)),
            Some(Prim::SplitFirstMark) => emit_the_toks!(self.mark_tokens_class(3, 0)),
            Some(Prim::SplitBotMark) => emit_the_toks!(self.mark_tokens_class(4, 0)),
            Some(Prim::TopMarksClass) => {
                let class = self.scan_int();
                emit_the_toks!(self.mark_tokens_class(0, class));
            }
            Some(Prim::FirstMarksClass) => {
                let class = self.scan_int();
                emit_the_toks!(self.mark_tokens_class(1, class));
            }
            Some(Prim::BotMarksClass) => {
                let class = self.scan_int();
                emit_the_toks!(self.mark_tokens_class(2, class));
            }
            Some(Prim::SplitFirstMarksClass) => {
                let class = self.scan_int();
                emit_the_toks!(self.mark_tokens_class(3, class));
            }
            Some(Prim::SplitBotMarksClass) => {
                let class = self.scan_int();
                emit_the_toks!(self.mark_tokens_class(4, class));
            }
            Some(Prim::JobName) => {
                let s = self.job_name.clone();
                emit_the!(s.as_bytes());
            }
            Some(Prim::NumExpr) => {
                let v = self.scan_expr_num();
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::DimExpr) => {
                let v = self.scan_expr_dim();
                let s = self.scaled_to_string(v);
                emit_the!(s.as_bytes());
            }
            Some(Prim::GlueExpr) => {
                let g = self.scan_expr_glue(false);
                let s = self.glue_to_string(&g);
                emit_the!(s.as_bytes());
            }
            Some(Prim::MuExpr) => {
                let g = self.scan_expr_glue(true);
                let s2 = self.mu_glue_to_string(&g);
                emit_the!(s2.as_bytes());
            }
            Some(Prim::PdfLastXPos) => {
                emit_the!(self.pdf_last_x.to_string().as_bytes());
            }
            Some(Prim::PdfLastYPos) => {
                emit_the!(self.pdf_last_y.to_string().as_bytes());
            }
            Some(
                p @ (Prim::PdfLastObj
                | Prim::PdfLastXForm
                | Prim::PdfLastXImage
                | Prim::PdfLastXImagePages
                | Prim::PdfLastLink
                | Prim::PdfLastAnnot),
            ) => {
                let s = self.pdf_last_value(p).to_string();
                emit_the!(s.as_bytes());
            }
            Some(Prim::PdfPageAttr) => {
                emit_the_toks!(self.pdf_page_attr_toks.clone());
            }
            Some(Prim::PdfPageResources) => {
                emit_the_toks!(self.pdf_page_resources_toks.clone());
            }
            Some(Prim::PdfPagesAttr) => {
                emit_the_toks!(self.pdf_pages_attr_toks.clone());
            }
            Some(Prim::XeTeXVersion) => {
                emit_the!(b"0");
            }
            Some(Prim::XeTeXRevision) => {
                emit_the!(b".999998");
            }
            Some(Prim::LuaTeXVersion) => {
                emit_the!(b"124");
            }
            Some(Prim::LuaTeXRevision) => {
                emit_the!(b"0");
            }
            Some(Prim::LuaTeXBanner) => {
                emit_the!(b"This is LuaTeX, Version 1.24.0");
            }
            Some(Prim::OutputMode) => {
                emit_the!(b"1");
            }
            Some(Prim::CatCodeTable) => {
                emit_the!(self.cur_catcode_table.to_string().as_bytes());
            }
            Some(Prim::XeTeXCharClass) => {
                let v = self.scan_xetex_charclass_val();
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::XeTeXInterCharToks) => {
                let toks = self.scan_xetex_interchartoks_the();
                emit_the_toks!(toks);
            }
            Some(
                p @ (Prim::XeTeXFontType
                | Prim::XeTeXCountGlyphs
                | Prim::XeTeXGlyphIndex
                | Prim::XeTeXCharGlyph
                | Prim::XeTeXGlyphBounds
                | Prim::XeTeXUseGlyphMetrics
                | Prim::XeTeXInterCharTokenState
                | Prim::XeTeXCountFeatures
                | Prim::XeTeXFeatureCode
                | Prim::XeTeXCountVariations
                | Prim::XeTeXVariation
                | Prim::XeTeXInputNormalization
                | Prim::XeTeXGenerateActualText
                | Prim::XeTeXDashBreakState),
            ) => {
                let v = self.scan_xetex_int_query(p);
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::Dimen) => {
                let idx = self.scan_reg_num();
                let s = self.scaled_to_string(self.eqtb.dimen[idx as usize]);
                emit_the!(s.as_bytes());
            }
            Some(Prim::Skip) => {
                let idx = self.scan_reg_num();
                let s = self.glue_to_string(&self.eqtb.skip[idx as usize].clone());
                emit_the!(s.as_bytes());
            }
            Some(Prim::MuSkip) => {
                let idx = self.scan_reg_num();
                let s = self.mu_glue_to_string(&self.eqtb.muskip[idx as usize].clone());
                emit_the!(s.as_bytes());
            }
            Some(Prim::Toks) => {
                let idx = self.scan_reg_num();
                let toks = (*self.eqtb.toks[idx as usize]).clone();
                emit_the_toks!(toks);
            }
            Some(Prim::Wd) => {
                let n = self.scan_reg_num();
                let s = self.scaled_to_string(self.box_reg_dimen(n, 0));
                emit_the!(s.as_bytes());
            }
            Some(Prim::GlueStretch) => {
                let v = self.scan_etex_glue_field(0);
                let s = self.scaled_to_string(v);
                emit_the!(s.as_bytes());
            }
            Some(Prim::GlueShrink) => {
                let v = self.scan_etex_glue_field(1);
                let s = self.scaled_to_string(v);
                emit_the!(s.as_bytes());
            }
            Some(Prim::GlueStretchOrder) => {
                let v = self.scan_etex_glue_field(2);
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::GlueShrinkOrder) => {
                let v = self.scan_etex_glue_field(3);
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::Ht) => {
                let n = self.scan_reg_num();
                let s = self.scaled_to_string(self.box_reg_dimen(n, 1));
                emit_the!(s.as_bytes());
            }
            Some(Prim::Dp) => {
                let n = self.scan_reg_num();
                let s = self.scaled_to_string(self.box_reg_dimen(n, 2));
                emit_the!(s.as_bytes());
            }
            Some(Prim::ParShape) => {
                let s = self.par_shape.len().to_string();
                emit_the!(s.as_bytes());
            }
            Some(
                p @ (Prim::InterLinePenalties
                | Prim::ClubPenalties
                | Prim::WidowPenalties
                | Prim::DisplayWidowPenalties),
            ) => {
                let index = self.scan_int();
                let value = self.penalty_shape_value(p, index);
                emit_the!(value.to_string().as_bytes());
            }
            Some(Prim::LastPenalty) => {
                let v = self.last_penalty_value();
                emit_the!(v.to_string().as_bytes());
            }
            Some(Prim::LastKern) => {
                let v = self.last_kern_value();
                let s = self.scaled_to_string(v);
                emit_the!(s.as_bytes());
            }
            Some(Prim::LastSkip) => {
                let g = self.last_skip_value();
                let s = self.glue_to_string(&g);
                emit_the!(s.as_bytes());
            }
            Some(Prim::Font) => {
                let csid = self
                    .eqtb
                    .font_cs
                    .get(self.eqtb.cur_font_val as usize)
                    .copied()
                    .unwrap_or(0);
                emit_the_toks!(vec![Token::from_cs(csid)]);
            }
            Some(Prim::TextFont) | Some(Prim::ScriptFont) | Some(Prim::ScriptScriptFont) => {
                let style = match self.cur_prim {
                    Some(Prim::TextFont) => 0usize,
                    Some(Prim::ScriptFont) => 1,
                    _ => 2,
                };
                let command = match self.cur_prim {
                    Some(Prim::TextFont) => "\\textfont",
                    Some(Prim::ScriptFont) => "\\scriptfont",
                    _ => "\\scriptscriptfont",
                };
                let fam = self.scan_math_family(command);
                let fid = self.eqtb.style_fonts[style][fam];
                let csid = self.eqtb.font_cs.get(fid as usize).copied().unwrap_or(0);
                emit_the_toks!(vec![Token::from_cs(csid)]);
            }
            _ => match self.eqtb.resolve(id).cloned() {
                Some(Equiv::CountReg(i)) => {
                    emit_the!(self.eqtb.count[i as usize].to_string().as_bytes());
                }
                Some(Equiv::CharDef(c)) => {
                    emit_the!((c as i32).to_string().as_bytes());
                }
                Some(Equiv::MathCharDef(c)) => {
                    emit_the!((c as i32).to_string().as_bytes());
                }
                Some(Equiv::DimenReg(i)) => {
                    let s = self.scaled_to_string(self.eqtb.dimen[i as usize]);
                    emit_the!(s.as_bytes());
                }
                Some(Equiv::SkipReg(i)) => {
                    let g = self.eqtb.skip[i as usize].clone();
                    let s = self.glue_to_string(&g);
                    emit_the!(s.as_bytes());
                }
                Some(Equiv::MuSkipReg(i)) => {
                    let g = self.eqtb.muskip[i as usize].clone();
                    let s = self.mu_glue_to_string(&g);
                    emit_the!(s.as_bytes());
                }
                Some(Equiv::ToksReg(i)) => {
                    let toks = (*self.eqtb.toks[i as usize]).clone();
                    emit_the_toks!(toks);
                }
                Some(Equiv::FontRef(f)) => {
                    // \the\font -> the cs name of the font
                    let csid = self.eqtb.font_cs.get(f as usize).copied().unwrap_or(0);
                    emit_the_toks!(vec![Token::from_cs(csid)]);
                }
                _ => {
                    self.error("You can't use `\\the' after that");
                }
            },
        }
    }

    fn push_mark_tokens(&mut self, which: usize) {
        self.push_tokens(self.mark_tokens_class(which, 0));
    }

    pub(crate) fn push_mark_tokens_class(&mut self, which: usize, class: i32) {
        self.push_tokens(self.mark_tokens_class(which, class));
    }

    fn mark_tokens_class(&self, which: usize, class: i32) -> Vec<Token> {
        let toks = if class >= 0 {
            self.marks[which]
                .get(class as usize)
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        toks
    }

    /// TeX \meaning of a token
    pub fn meaning_of(&self, t: Token) -> String {
        if !t.is_cs() {
            let c = t.chr();
            let cat = t.cc();
            // tex.web §213: an active character is a control sequence, so
            // \meaning reports its eqtb binding (\meaning~ → macro:->…);
            // only a truly unbound one falls back, as "undefined".
            if cat == 13 {
                return match self.active_cs_lookup(c) {
                    Some(id) if self.eqtb.resolve(id).is_some() => {
                        self.meaning_of(Token::from_cs(id))
                    }
                    _ => "undefined".to_string(),
                };
            }
            let word = match cat {
                0 => "escape character ",
                1 => "begin-group character ",
                2 => "end-group character ",
                3 => "math shift character ",
                4 => "alignment tab character ",
                5 => "end-of-line character ",
                6 => "macro parameter character ",
                7 => "superscript character ",
                8 => "subscript character ",
                9 => "ignored character ",
                10 => "blank space ",
                11 => "the letter ",
                12 => "the character ",
                14 => "comment character ",
                15 => "invalid character ",
                _ => "character ",
            };
            let ch = ((c as u8) as char).to_string();
            return format!("{}{}", word, ch);
        }
        let esc = self.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize];
        let esc_byte = [esc as u8];
        let esc_str = if (0..=255).contains(&esc) {
            std::str::from_utf8(&esc_byte).unwrap_or("\\")
        } else {
            ""
        };
        let name = String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned();
        match self.eqtb.resolve(t.cs_id()).cloned() {
            None => "undefined".to_string(),
            Some(Equiv::CharTok(v)) => self.meaning_of(Token(v)),
            Some(Equiv::Macro(m)) => {
                let mut s = String::new();
                if m.protected {
                    s.push_str(esc_str);
                    s.push_str("protected");
                }
                if m.long {
                    s.push_str(esc_str);
                    s.push_str("long");
                }
                if m.outer {
                    s.push_str(esc_str);
                    s.push_str("outer");
                }
                if !s.is_empty() {
                    s.push_str(" macro:");
                } else {
                    s.push_str("macro:");
                }
                if !m.prefix.is_empty() {
                    s.push_str(&self.tokens_to_string(&m.prefix));
                }
                for (i, d) in m.params.iter().enumerate() {
                    s.push_str(&format!("#{}", i + 1));
                    if !d.is_empty() {
                        s.push_str(&self.tokens_to_string(d));
                    }
                }
                s.push_str("->");
                s.push_str(&self.tokens_to_string(&m.body));
                s
            }
            Some(Equiv::Prim(p)) => format!("{}{}", esc_str, self.prim_name(p)),
            Some(Equiv::CharDef(c)) => format!("{}char\"{:X}", esc_str, c),
            Some(Equiv::MathCharDef(c)) => format!("{}mathchar\"{:X}", esc_str, c),
            Some(Equiv::FontRef(f)) => format!("select font {}", self.font_display_name(f)),
            Some(Equiv::CountReg(i)) => format!("{}count{}", esc_str, i),
            Some(Equiv::DimenReg(i)) => format!("{}dimen{}", esc_str, i),
            Some(Equiv::SkipReg(i)) => format!("{}skip{}", esc_str, i),
            Some(Equiv::MuSkipReg(i)) => format!("{}muskip{}", esc_str, i),
            Some(Equiv::ToksReg(i)) => format!("{}toks{}", esc_str, i),
            Some(Equiv::BoxReg(i)) => format!("{}box{}", esc_str, i),
            Some(Equiv::Alias(_)) => format!("{}{}", esc_str, name),
        }
    }

    pub fn prim_name(&self, p: Prim) -> String {
        if let Some(name) = self.primitive_names.get(&p.code()) {
            return String::from_utf8_lossy(name).into_owned();
        }
        // Tests may construct a partial engine without init_primitives.
        for id in self.cs.all_ids() {
            if matches!(self.eqtb.get(id), Some(Equiv::Prim(q)) if *q == p) {
                return String::from_utf8_lossy(self.cs.name(id)).into_owned();
            }
        }
        "unknown".into()
    }

    pub fn cur_quad(&self) -> i32 {
        self.eqtb
            .fonts
            .get(self.eqtb.cur_font_val as usize)
            .map(|f| f.quad())
            .unwrap_or(0)
    }
    pub fn cur_x_height(&self) -> i32 {
        self.eqtb
            .fonts
            .get(self.eqtb.cur_font_val as usize)
            .map(|f| f.x_height())
            .unwrap_or(0)
    }

    // ---------- formatting ----------

    /// TeX's print_scaled: value in sp as decimal pt string
    pub fn scaled_to_string(&self, v: i32) -> String {
        let neg = v < 0;
        let mut s = (v as i64).abs();
        let int_part = s / ONE as i64;
        let mut res = String::new();
        if neg {
            res.push('-');
        }
        res.push_str(&int_part.to_string());
        res.push('.');
        s = 10 * (s % ONE as i64) + 5;
        let mut delta = 10i64;
        loop {
            if delta > ONE as i64 {
                s += 32768 - 50000; // tex.web §103: round the last digit (@'100000 - 50000)
            }
            res.push((b'0' + (s / ONE as i64) as u8) as char);
            s = 10 * (s % ONE as i64);
            delta *= 10;
            if s <= delta {
                break;
            }
        }
        res.push_str("pt");
        res
    }
    pub fn scaled_number_to_string(&self, v: i32) -> String {
        let mut s = self.scaled_to_string(v);
        if s.ends_with("pt") {
            s.truncate(s.len() - 2);
        }
        s
    }

    fn glue_to_string_with_unit(&self, g: &Glue, unit: &str) -> String {
        let scaled = |value| {
            let mut text = self.scaled_to_string(value);
            text.truncate(text.len().saturating_sub(2));
            text.push_str(unit);
            text
        };
        let mut s = scaled(g.width);
        if g.stretch != 0 || g.stretch_order > 0 {
            s.push_str(" plus ");
            if g.stretch_order == 0 {
                s.push_str(&scaled(g.stretch));
            } else {
                s.push_str(&self.scaled_number_to_string(g.stretch));
                match g.stretch_order {
                    1 => s.push_str("fil"),
                    2 => s.push_str("fill"),
                    3 => s.push_str("filll"),
                    _ => {}
                }
            }
        }
        if g.shrink != 0 || g.shrink_order > 0 {
            s.push_str(" minus ");
            if g.shrink_order == 0 {
                s.push_str(&scaled(g.shrink));
            } else {
                s.push_str(&self.scaled_number_to_string(g.shrink));
                match g.shrink_order {
                    1 => s.push_str("fil"),
                    2 => s.push_str("fill"),
                    3 => s.push_str("filll"),
                    _ => {}
                }
            }
        }

        s
    }

    pub fn glue_to_string(&self, g: &Glue) -> String {
        self.glue_to_string_with_unit(g, "pt")
    }

    pub fn mu_glue_to_string(&self, g: &Glue) -> String {
        self.glue_to_string_with_unit(g, "mu")
    }

    pub fn font_display_name(&self, f: u16) -> String {
        match self.eqtb.fonts.get(f as usize) {
            Some(font) => {
                let mut s = font.tfm_name.clone();
                if font.at_size != font.dsize {
                    s.push_str(&format!(" at {}", self.scaled_to_string(font.at_size)));
                }
                s
            }
            None => String::new(),
        }
    }

    fn scan_font_char_dimen(&mut self, p: Prim) -> i32 {
        let font = self.scan_font_id();
        let (character, source) = self.scan_int_with_source();
        if let Ok(scalar) = u32::try_from(character) {
            if let Some((width, height, depth, italic)) = self.native_char_dimensions(font, scalar)
            {
                return match p {
                    Prim::FontCharWd => width,
                    Prim::FontCharHt => height,
                    Prim::FontCharDp => depth,
                    Prim::FontCharIc => italic,
                    _ => unreachable!(),
                };
            }
        }
        let c = if (0..=255).contains(&character) {
            character as u8
        } else {
            let command = match p {
                Prim::FontCharWd => "\\fontcharwd",
                Prim::FontCharHt => "\\fontcharht",
                Prim::FontCharDp => "\\fontchardp",
                Prim::FontCharIc => "\\fontcharic",
                _ => "font character metric",
            };
            self.error_at(
                &format!(
                    "Character code {character} is out of range for {command}; expected 0 through 255 and used character 0"
                ),
                source,
            );
            0
        };
        let Some(font) = self.eqtb.fonts.get(font as usize) else {
            return 0;
        };
        match p {
            Prim::FontCharWd => font.char_width(c),
            Prim::FontCharHt => font.char_height(c),
            Prim::FontCharDp => font.char_depth(c),
            Prim::FontCharIc => font.char_italic(c),
            _ => unreachable!(),
        }
    }

    pub fn scan_pdf_ximage_bbox(&mut self) -> i32 {
        let obj = self.scan_int();
        let coord = self.scan_int();
        if let Some(info) = self.pdf_images.get(&obj) {
            match coord {
                1 => info.bbox[0],
                2 => info.bbox[1],
                3 => info.bbox[2],
                4 => info.bbox[3],
                _ => {
                    self.error("pdfTeX error (pdfximagebbox): invalid parameter");
                    0
                }
            }
        } else {
            self.error("pdfTeX error (ext1): cannot find referenced object");
            0
        }
    }

    // ---------- font id scan ----------

    pub fn scan_font_id(&mut self) -> u16 {
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        if !t.is_cs() {
            self.push_token(t);
            self.error("Missing font identifier");
            return 0;
        }
        match self.eqtb.resolve(t.cs_id()).cloned() {
            Some(Equiv::FontRef(f)) => f,
            Some(Equiv::Prim(Prim::Font)) => {
                // \font refers to current font
                self.eqtb.cur_font_val
            }
            // tex.web §1023 scan_font_ident: \textfont/\scriptfont/
            // \scriptscriptfont <fam> yield the family's font id.
            Some(Equiv::Prim(p @ (Prim::TextFont | Prim::ScriptFont | Prim::ScriptScriptFont))) => {
                let command = match p {
                    Prim::TextFont => "\\textfont",
                    Prim::ScriptFont => "\\scriptfont",
                    _ => "\\scriptscriptfont",
                };
                let fam = self.scan_math_family(command);
                let slot = match p {
                    Prim::TextFont => 0,
                    Prim::ScriptFont => 1,
                    _ => 2,
                };
                self.eqtb.style_fonts[slot][fam]
            }
            _ => {
                self.error(&format!(
                    "{} is not a font identifier; use a font defined with \\font",
                    self.display_cs(t.cs_id())
                ));
                0
            }
        }
    }

    // ---------- expressions (e-TeX) ----------

    pub fn scan_expr_num(&mut self) -> i32 {
        let origin = self.current_token_source_mark();
        let mut arithmetic = ExprArithmetic::default();
        let v = self.expr_eval(ExprKind::Int, false, &mut arithmetic);
        let result = match v {
            ExprVal::Int(x) => x,
            ExprVal::Dim(x) => x / ONE,
            ExprVal::Glue(g) => g.width / ONE,
        };
        if arithmetic.overflowed {
            self.error_at(
                "Arithmetic overflow",
                origin.as_ref().map(SourceMark::to_context),
            );
            0
        } else {
            result
        }
    }

    pub fn scan_expr_dim(&mut self) -> i32 {
        let origin = self.current_token_source_mark();
        let mut arithmetic = ExprArithmetic::default();
        let v = self.expr_eval(ExprKind::Dim, false, &mut arithmetic);
        let result = match v {
            ExprVal::Int(x) => mult(x, ONE),
            ExprVal::Dim(x) => x,
            ExprVal::Glue(g) => g.width,
        };
        if arithmetic.overflowed {
            self.error_at(
                "Arithmetic overflow",
                origin.as_ref().map(SourceMark::to_context),
            );
            0
        } else {
            result
        }
    }

    pub fn scan_expr_glue(&mut self, mu: bool) -> Glue {
        let origin = self.current_token_source_mark();
        let mut arithmetic = ExprArithmetic::default();
        let v = self.expr_eval(ExprKind::Glue, mu, &mut arithmetic);
        let result = match v {
            ExprVal::Int(x) => Glue::new(mult(x, ONE)),
            ExprVal::Dim(x) => Glue::new(x),
            ExprVal::Glue(g) => g,
        };
        if arithmetic.overflowed {
            self.error_at(
                "Arithmetic overflow",
                origin.as_ref().map(SourceMark::to_context),
            );
            Glue::zero()
        } else {
            result
        }
    }

    fn expr_eval(&mut self, kind: ExprKind, mu: bool, arithmetic: &mut ExprArithmetic) -> ExprVal {
        let mut left = self.expr_term(kind, mu, arithmetic);
        loop {
            let t = self.expr_next_token();
            if self.token_is_relax(t) {
                return left;
            }
            let op = if t.is_char() && t.chr() == b'+' as u32 {
                1
            } else if t.is_char() && t.chr() == b'-' as u32 {
                2
            } else {
                self.push_token(t);
                return left;
            };
            let right = self.expr_term(kind, mu, arithmetic);
            left = match (left, right) {
                (ExprVal::Int(a), ExprVal::Int(b)) => {
                    ExprVal::Int(arithmetic.add(a, b, op == 2, MAX_EXPR_INT))
                }
                (ExprVal::Dim(a), ExprVal::Dim(b)) => {
                    ExprVal::Dim(arithmetic.add(a, b, op == 2, MAX_EXPR_DIMEN))
                }
                (ExprVal::Dim(a), ExprVal::Int(b)) => {
                    ExprVal::Dim(arithmetic.add(a, mult(b, ONE), op == 2, MAX_EXPR_DIMEN))
                }
                (ExprVal::Int(a), ExprVal::Dim(b)) => {
                    ExprVal::Dim(arithmetic.add(mult(a, ONE), b, op == 2, MAX_EXPR_DIMEN))
                }
                (ExprVal::Glue(a), ExprVal::Glue(b)) => {
                    ExprVal::Glue(arithmetic.glue_add(a, b, op == 2))
                }
                (ExprVal::Glue(a), ExprVal::Dim(b)) => {
                    let g = Glue::new(b);
                    ExprVal::Glue(arithmetic.glue_add(a, g, op == 2))
                }
                (ExprVal::Glue(a), ExprVal::Int(b)) => {
                    let g = Glue::new(mult(b, ONE));
                    ExprVal::Glue(arithmetic.glue_add(a, g, op == 2))
                }
                (l, r) => {
                    let _ = r;
                    l
                }
            };
        }
    }

    fn expr_term(&mut self, kind: ExprKind, mu: bool, arithmetic: &mut ExprArithmetic) -> ExprVal {
        let mut left = self.expr_factor(kind, mu, arithmetic);
        loop {
            let t = self.expr_next_token();
            let multiply = t.is_char() && t.chr() == b'*' as u32;
            let divide = t.is_char() && t.chr() == b'/' as u32;
            if !multiply && !divide {
                self.push_token(t);
                return left;
            }
            let right = self.expr_factor(ExprKind::Int, false, arithmetic);
            let b = match right {
                ExprVal::Int(x) => x,
                _ => 0,
            };

            // e-TeX treats an immediately adjacent `* n / d` as one scaled
            // operation.  Besides matching TeX's rounding, this lets a valid
            // final value survive when the intermediate product is outside
            // the expression's range (a common font-package idiom).  A second
            // multiplication still commits the first product and therefore
            // reports overflow at the same point as e-TeX.
            let divisor = if multiply {
                let next = self.expr_next_token();
                if next.is_char() && next.chr() == b'/' as u32 {
                    match self.expr_factor(ExprKind::Int, false, arithmetic) {
                        ExprVal::Int(x) => Some(x),
                        _ => Some(0),
                    }
                } else {
                    self.push_token(next);
                    None
                }
            } else {
                None
            };
            left = match left {
                ExprVal::Int(a) => ExprVal::Int(if let Some(d) = divisor {
                    arithmetic.multiply_divide(a, b, d, MAX_EXPR_INT)
                } else if multiply {
                    arithmetic.multiply(a, b, MAX_EXPR_INT)
                } else {
                    arithmetic.divide(a, b, MAX_EXPR_INT)
                }),
                ExprVal::Dim(a) => ExprVal::Dim(if let Some(d) = divisor {
                    arithmetic.multiply_divide(a, b, d, MAX_EXPR_DIMEN)
                } else if multiply {
                    arithmetic.multiply(a, b, MAX_EXPR_DIMEN)
                } else {
                    arithmetic.divide(a, b, MAX_EXPR_DIMEN)
                }),
                ExprVal::Glue(a) => ExprVal::Glue(if let Some(d) = divisor {
                    arithmetic.scale_glue_ratio(a, b, d)
                } else {
                    arithmetic.scale_glue(a, b, divide)
                }),
            };
        }
    }

    fn expr_factor(
        &mut self,
        kind: ExprKind,
        mu: bool,
        arithmetic: &mut ExprArithmetic,
    ) -> ExprVal {
        let t = self.expr_next_token();
        if t.is_char() && t.chr() == b'(' as u32 {
            let v = self.expr_eval(kind, mu, arithmetic);
            let t = self.expr_next_token();
            if !(t.is_char() && t.chr() == b')' as u32) {
                self.push_token(t);
                self.error("Missing ) in expression");
            }
            return v;
        }
        if kind != ExprKind::Glue && t.is_char() && t.chr() == b'-' as u32 {
            let v = self.expr_factor(kind, mu, arithmetic);
            return match v {
                ExprVal::Int(x) => ExprVal::Int(arithmetic.negate(x, MAX_EXPR_INT)),
                ExprVal::Dim(x) => ExprVal::Dim(arithmetic.negate(x, MAX_EXPR_DIMEN)),
                ExprVal::Glue(_) => unreachable!(),
            };
        }
        self.push_token(t);
        match kind {
            ExprKind::Int => {
                let value = self.scan_int();
                ExprVal::Int(arithmetic.checked_or_zero(value as i64, MAX_EXPR_INT))
            }
            ExprKind::Dim => {
                let value = self.scan_dimen(false, false);
                ExprVal::Dim(arithmetic.checked_or_zero(value as i64, MAX_EXPR_DIMEN))
            }
            ExprKind::Glue => {
                let value = self.scan_glue(mu);
                ExprVal::Glue(arithmetic.checked_glue(value))
            }
        }
    }
}

const MAX_EXPR_INT: i64 = 0x7FFF_FFFF;
const MAX_EXPR_DIMEN: i64 = 0x3FFF_FFFF;

/// e-TeX delays expression arithmetic errors until the complete expression
/// has been consumed, then replaces the complete result with zero. Keeping a
/// single latch also prevents one bad glue component from leaking the others.
#[derive(Default)]
struct ExprArithmetic {
    overflowed: bool,
}

impl ExprArithmetic {
    fn checked(&mut self, value: i64, limit: i64) -> Option<i32> {
        if (-limit..=limit).contains(&value) {
            Some(value as i32)
        } else {
            self.overflowed = true;
            None
        }
    }

    fn checked_or_zero(&mut self, value: i64, limit: i64) -> i32 {
        self.checked(value, limit).unwrap_or(0)
    }

    fn add(&mut self, left: i32, right: i32, subtract: bool, limit: i64) -> i32 {
        let right = if subtract {
            -(right as i64)
        } else {
            right as i64
        };
        self.checked_or_zero(left as i64 + right, limit)
    }

    fn multiply(&mut self, left: i32, right: i32, limit: i64) -> i32 {
        self.checked_or_zero(left as i64 * right as i64, limit)
    }

    fn multiply_divide(&mut self, value: i32, numerator: i32, denominator: i32, limit: i64) -> i32 {
        if denominator == 0 {
            self.overflowed = true;
            return 0;
        }

        let product = value as i64 * numerator as i64;
        self.rounded_divide(product, denominator as i64, limit)
    }

    fn divide(&mut self, numerator: i32, denominator: i32, limit: i64) -> i32 {
        if denominator == 0 {
            self.overflowed = true;
            return 0;
        }

        self.rounded_divide(numerator as i64, denominator as i64, limit)
    }

    fn rounded_divide(&mut self, numerator: i64, denominator: i64, limit: i64) -> i32 {
        let negative = (numerator < 0) != (denominator < 0);
        let numerator = numerator.abs();
        let denominator = denominator.abs();
        let mut quotient = numerator / denominator;
        let remainder = numerator - quotient * denominator;
        if remainder * 2 >= denominator {
            quotient += 1;
        }
        if negative {
            quotient = -quotient;
        }
        self.checked_or_zero(quotient, limit)
    }

    fn negate(&mut self, value: i32, limit: i64) -> i32 {
        self.checked_or_zero(-(value as i64), limit)
    }

    fn checked_glue(&mut self, glue: Glue) -> Glue {
        let width = self.checked(glue.width as i64, MAX_EXPR_DIMEN);
        let stretch = self.checked(glue.stretch as i64, MAX_EXPR_DIMEN);
        let shrink = self.checked(glue.shrink as i64, MAX_EXPR_DIMEN);
        match (width, stretch, shrink) {
            (Some(width), Some(stretch), Some(shrink)) => Glue {
                width,
                stretch,
                shrink,
                ..glue
            },
            _ => Glue::zero(),
        }
    }

    fn glue_component(
        left: i32,
        left_order: u8,
        right: i32,
        right_order: u8,
        subtract: bool,
    ) -> (i64, u8) {
        if left_order == right_order {
            let right = if subtract {
                -(right as i64)
            } else {
                right as i64
            };
            (left as i64 + right, left_order)
        } else if left_order < right_order {
            let value = if subtract {
                -(right as i64)
            } else {
                right as i64
            };
            (value, right_order)
        } else {
            (left as i64, left_order)
        }
    }

    fn glue_add(&mut self, left: Glue, right: Glue, subtract: bool) -> Glue {
        let width = left.width as i64
            + if subtract {
                -(right.width as i64)
            } else {
                right.width as i64
            };
        let (stretch, stretch_order) = Self::glue_component(
            left.stretch,
            left.stretch_order,
            right.stretch,
            right.stretch_order,
            subtract,
        );
        let (shrink, shrink_order) = Self::glue_component(
            left.shrink,
            left.shrink_order,
            right.shrink,
            right.shrink_order,
            subtract,
        );

        let width = self.checked(width, MAX_EXPR_DIMEN);
        let stretch = self.checked(stretch, MAX_EXPR_DIMEN);
        let shrink = self.checked(shrink, MAX_EXPR_DIMEN);
        match (width, stretch, shrink) {
            (Some(width), Some(stretch), Some(shrink)) => Glue {
                width,
                stretch,
                shrink,
                stretch_order,
                shrink_order,
            },
            _ => Glue::zero(),
        }
    }

    fn scale_glue(&mut self, glue: Glue, scalar: i32, divide: bool) -> Glue {
        if divide && scalar == 0 {
            self.overflowed = true;
            return Glue::zero();
        }

        let width = if divide {
            self.divide(glue.width, scalar, MAX_EXPR_DIMEN)
        } else {
            self.multiply(glue.width, scalar, MAX_EXPR_DIMEN)
        };
        let stretch = if divide {
            self.divide(glue.stretch, scalar, MAX_EXPR_DIMEN)
        } else {
            self.multiply(glue.stretch, scalar, MAX_EXPR_DIMEN)
        };
        let shrink = if divide {
            self.divide(glue.shrink, scalar, MAX_EXPR_DIMEN)
        } else {
            self.multiply(glue.shrink, scalar, MAX_EXPR_DIMEN)
        };

        if self.overflowed {
            Glue::zero()
        } else {
            Glue {
                width,
                stretch,
                shrink,
                ..glue
            }
        }
    }

    fn scale_glue_ratio(&mut self, glue: Glue, numerator: i32, denominator: i32) -> Glue {
        if denominator == 0 {
            self.overflowed = true;
            return Glue::zero();
        }

        let width = self.multiply_divide(glue.width, numerator, denominator, MAX_EXPR_DIMEN);
        let stretch = self.multiply_divide(glue.stretch, numerator, denominator, MAX_EXPR_DIMEN);
        let shrink = self.multiply_divide(glue.shrink, numerator, denominator, MAX_EXPR_DIMEN);

        if self.overflowed {
            Glue::zero()
        } else {
            Glue {
                width,
                stretch,
                shrink,
                ..glue
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ExprKind {
    Int,
    Dim,
    Glue,
}

#[derive(Clone)]
enum ExprVal {
    Int(i32),
    Dim(i32),
    Glue(Glue),
}

#[cfg(test)]
mod expression_arithmetic_tests {
    use super::*;
    use crate::engine::InteractionMode;

    fn scanner(source: &str) -> Engine {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.interaction_mode = InteractionMode::Nonstop;
        engine
            .input
            .push_file("expr.tex".to_string(), source.as_bytes().to_vec());
        engine
    }

    fn assert_one_arithmetic_overflow(engine: &Engine) {
        assert_eq!(engine.diagnostics.len(), 1, "{}", engine.term);
        let diagnostic = &engine.diagnostics[0];
        assert_eq!(diagnostic.message, "Arithmetic overflow");
        let primary = diagnostic.primary.as_ref().expect("located diagnostic");
        assert_eq!(
            (primary.name.as_str(), primary.line, primary.column),
            ("expr.tex", 1, 1)
        );
    }

    fn assert_zero_glue(glue: &Glue) {
        assert_eq!(
            (
                glue.width,
                glue.stretch,
                glue.shrink,
                glue.stretch_order,
                glue.shrink_order,
            ),
            (0, 0, 0, 0, 0)
        );
    }

    #[test]
    fn numexpr_overflow_and_zero_division_match_etex_recovery() {
        // Each case was checked against e-TeX: the complete expression result
        // becomes zero and only one delayed arithmetic diagnostic is emitted.
        for source in [
            "2147483647+1\\relax",
            "2147483647+1+7\\relax",
            "-2147483647-1\\relax",
            "2147483647--1\\relax",
            "1073741824*2\\relax",
            "-1073741824*2\\relax",
            "1/0\\relax",
        ] {
            let mut engine = scanner(&format!("\\numexpr{source}"));
            assert_eq!(engine.scan_int(), 0, "source: {source}");
            assert_one_arithmetic_overflow(&engine);
        }
    }

    #[test]
    fn numexpr_uses_symmetric_integer_bounds_and_tex_rounding() {
        for (source, expected) in [
            ("2147483647\\relax", 2_147_483_647),
            ("-2147483647\\relax", -2_147_483_647),
            ("--2147483647\\relax", 2_147_483_647),
            ("5/2\\relax", 3),
            ("-5/2\\relax", -3),
            ("5/-2\\relax", -3),
            ("-5/-2\\relax", 3),
        ] {
            let mut engine = scanner(&format!("\\numexpr{source}"));
            assert_eq!(engine.scan_int(), expected, "source: {source}");
            assert!(engine.diagnostics.is_empty(), "{}", engine.term);
        }

        // A count can reach i32::MIN through register arithmetic. e-TeX still
        // rejects it as an expression factor, and unary minus must not panic.
        let mut engine = scanner("\\numexpr-\\count0\\relax");
        engine.eqtb.count[0] = i32::MIN;
        assert_eq!(engine.scan_int(), 0);
        assert_one_arithmetic_overflow(&engine);
    }

    #[test]
    fn expressions_fuse_adjacent_multiply_divide_like_etex() {
        for (source, expected) in [
            ("2147483647*2/2\\relax", 2_147_483_647),
            ("1*2147483647*2/2/2147483647\\relax", 1),
            ("1*2/3\\relax", 1),
            ("-1*2/3\\relax", -1),
        ] {
            let mut engine = scanner(&format!("\\numexpr{source}"));
            assert_eq!(engine.scan_int(), expected, "source: {source}");
            assert!(engine.diagnostics.is_empty(), "{}", engine.term);
        }

        let mut engine = scanner("\\dimexpr10000pt*2/2\\relax");
        assert_eq!(engine.scan_dimen(false, false), 10_000 * ONE);
        assert!(engine.diagnostics.is_empty(), "{}", engine.term);

        let mut engine = scanner("\\glueexpr10000pt plus 10000pt*2/2\\relax");
        let result = engine.scan_glue(false);
        assert_eq!((result.width, result.stretch), (10_000 * ONE, 10_000 * ONE));
        assert!(engine.diagnostics.is_empty(), "{}", engine.term);
    }

    #[test]
    fn multiply_divide_only_defers_the_adjacent_product_check() {
        for source in [
            "2147483647*2/1\\relax",
            "2147483647/2*2\\relax",
            "1073741824*2*1/2\\relax",
        ] {
            let mut engine = scanner(&format!("\\numexpr{source}"));
            assert_eq!(engine.scan_int(), 0, "source: {source}");
            assert_one_arithmetic_overflow(&engine);
        }
    }

    #[test]
    fn dimexpr_checks_max_dimen_for_every_operator() {
        for source in [
            "16383pt+1pt\\relax",
            "-16383pt-1pt\\relax",
            "6000pt*3\\relax",
            "1pt/0\\relax",
        ] {
            let mut engine = scanner(&format!("\\dimexpr{source}"));
            assert_eq!(engine.scan_dimen(false, false), 0, "source: {source}");
            assert_one_arithmetic_overflow(&engine);
        }
    }

    #[test]
    fn glueexpr_overflow_discards_width_stretch_and_shrink_atomically() {
        for source in [
            "10000pt+10000pt\\relax",
            "1pt plus 10000pt+2pt plus 10000pt\\relax",
            "1pt minus 10000pt+2pt minus 10000pt\\relax",
            "1pt plus 10000fil+2pt plus 10000fil\\relax",
            "10000pt plus 10000pt minus 10000pt+10000pt plus 10000pt minus 10000pt\\relax",
            "1pt plus 6000pt*3\\relax",
            "1pt plus 2pt/0\\relax",
        ] {
            let mut engine = scanner(&format!("\\glueexpr{source}"));
            let result = engine.scan_glue(false);
            assert_zero_glue(&result);
            assert_one_arithmetic_overflow(&engine);
        }
    }

    #[test]
    fn muexpr_uses_the_same_max_dimen_limit() {
        let mut engine = scanner("\\muexpr10000mu+10000mu\\relax");
        let result = engine.scan_glue(true);
        assert_zero_glue(&result);
        assert_one_arithmetic_overflow(&engine);
    }
}

#[cfg(test)]
mod token_list_capacity_tests {
    use super::*;
    use crate::eqtb::Equiv;
    use crate::input::MAX_TOKEN_LIST_TOKENS;
    use std::rc::Rc;

    #[test]
    fn capacity_check_allows_exact_limit_and_rejects_one_more() {
        let mut engine = Engine::new(true);
        assert!(engine.scanned_token_list_has_room(
            MAX_TOKEN_LIST_TOKENS - 1,
            1,
            "test token list size",
            None,
        ));
        assert!(!engine.scanned_token_list_has_room(
            MAX_TOKEN_LIST_TOKENS,
            1,
            "test token list size",
            None,
        ));
        assert!(engine.stopped_on_error);
    }

    #[test]
    fn expanded_text_rejects_oversized_bulk_insert_before_copying_it() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.eqtb.assign_cat(b'{', 1, true);
        engine.eqtb.assign_cat(b'}', 2, true);
        let huge = engine.cs.intern(b"huge");
        engine.eqtb.assign(huge, Equiv::ToksReg(0), true);
        engine.eqtb.toks[0] = Rc::new(vec![Token::letter(b'x'); MAX_TOKEN_LIST_TOKENS]);
        engine.input.push_file(
            "capacity.tex".to_string(),
            b"{A\\unexpanded\\huge}".to_vec(),
        );

        let tokens = engine.scan_general_text_expanded();

        assert_eq!(tokens.len(), 1, "{}", engine.term);
        assert!(engine.stopped_on_error);
        assert!(!engine.in_expanded_scan);
    }

    #[test]
    fn expanded_definition_rejects_oversized_bulk_insert_before_copying_it() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.eqtb.assign_cat(b'{', 1, true);
        engine.eqtb.assign_cat(b'}', 2, true);
        let huge = engine.cs.intern(b"huge");
        engine.eqtb.assign(huge, Equiv::ToksReg(0), true);
        engine.eqtb.toks[0] = Rc::new(vec![Token::letter(b'x'); MAX_TOKEN_LIST_TOKENS]);
        engine.input.push_file(
            "capacity.tex".to_string(),
            b"\\edef\\result{A\\unexpanded\\huge}\\end".to_vec(),
        );

        engine.run();

        assert!(engine.stopped_on_error);
    }

    #[test]
    fn file_name_scan_is_bounded_at_its_opening_source() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.eqtb.assign_cat(b'{', 1, true);
        engine.eqtb.assign_cat(b'}', 2, true);
        let source = format!("{{{}}}", "a".repeat(4097));
        engine
            .input
            .push_file("capacity.tex".to_string(), source.into_bytes());

        assert!(engine.scan_file_name().is_empty());
        assert!(engine.stopped_on_error);
        assert_eq!(engine.diagnostics[0].primary.as_ref().unwrap().line, 1);
    }

    #[test]
    fn explicit_control_sequence_name_scan_is_bounded() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        let mut source = "a".repeat(2001);
        source.push_str("\\relax");
        engine
            .input
            .push_file("capacity.tex".to_string(), source.into_bytes());
        let relax = engine.cs.lookup(b"relax").unwrap();

        assert_eq!(engine.scan_csname_explicit(), relax);
        assert!(engine.stopped_on_error);
        assert_eq!(engine.diagnostics[0].primary.as_ref().unwrap().line, 1);
    }
}

#[cfg(test)]
mod numeric_diagnostic_tests {
    use super::*;

    fn scanner(source: &str) -> Engine {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine
            .input
            .push_file("numbers.tex".to_string(), source.as_bytes().to_vec());
        engine
    }

    #[test]
    fn oversized_decimal_integer_is_clamped_and_located_at_the_first_bad_digit() {
        let mut engine = scanner("2147483648 ");

        assert_eq!(engine.scan_int(), i32::MAX);
        assert_eq!(engine.diagnostics.len(), 1, "{}", engine.term);
        let diagnostic = &engine.diagnostics[0];
        assert_eq!(diagnostic.message, "Number too big");
        let primary = diagnostic.primary.as_ref().unwrap();
        assert_eq!(
            (primary.name.as_str(), primary.line, primary.column),
            ("numbers.tex", 1, 10)
        );
    }

    #[test]
    fn integer_overflow_is_reported_for_every_literal_radix() {
        for literal in ["2147483648 ", "'20000000000 ", "\"80000000 "] {
            let mut engine = scanner(literal);

            assert_eq!(engine.scan_int(), i32::MAX, "literal {literal}");
            assert_eq!(
                engine.diagnostics.len(),
                1,
                "literal {literal}: {}",
                engine.term
            );
            assert_eq!(engine.diagnostics[0].message, "Number too big");
            assert!(engine.diagnostics[0].primary.is_some());
        }
    }

    #[test]
    fn radix_prefix_without_a_digit_reports_and_preserves_the_invalid_token() {
        for literal in ["'Z", "'8", "\"G"] {
            let mut engine = scanner(literal);

            assert_eq!(engine.scan_int(), 0, "literal {literal}");
            assert_eq!(
                engine.diagnostics.len(),
                1,
                "literal {literal}: {}",
                engine.term
            );
            let diagnostic = &engine.diagnostics[0];
            assert_eq!(diagnostic.message, "Missing number, treated as zero");
            let primary = diagnostic.primary.as_ref().expect("located diagnostic");
            assert_eq!(
                (primary.name.as_str(), primary.line, primary.column),
                ("numbers.tex", 1, 2),
                "literal {literal}"
            );

            let invalid = engine.get_token();
            assert!(invalid.is_char(), "literal {literal}");
            assert_eq!(
                invalid.chr(),
                literal.as_bytes()[1] as u32,
                "literal {literal}"
            );
        }

        for literal in ["'", "\""] {
            let mut engine = scanner(literal);

            assert_eq!(engine.scan_int(), 0, "literal {literal}");
            assert_eq!(engine.diagnostics.len(), 1, "literal {literal}");
            let diagnostic = &engine.diagnostics[0];
            assert_eq!(diagnostic.message, "Missing number, treated as zero");
            let primary = diagnostic.primary.as_ref().expect("located diagnostic");
            assert_eq!(
                (primary.name.as_str(), primary.line, primary.column),
                ("numbers.tex", 1, 1),
                "literal {literal}"
            );
        }
    }

    #[test]
    fn invalid_register_number_is_located_at_its_operand() {
        let mut engine = scanner("  32768=");

        assert_eq!(engine.scan_reg_num(), 0);
        assert_eq!(engine.diagnostics.len(), 1, "{}", engine.term);
        let diagnostic = &engine.diagnostics[0];
        assert_eq!(
            diagnostic.message,
            "Register number 32768 is out of range; expected a number from 0 through 32767"
        );
        let primary = diagnostic.primary.as_ref().expect("located diagnostic");
        assert_eq!(
            (primary.name.as_str(), primary.line, primary.column),
            ("numbers.tex", 1, 3)
        );

        let delimiter = engine.get_token();
        assert!(delimiter.is_char());
        assert_eq!(delimiter.chr(), b'=' as u32);
    }

    #[test]
    fn largest_integer_and_legal_dimension_do_not_report_errors() {
        let mut integer = scanner("2147483647 ");
        assert_eq!(integer.scan_int(), i32::MAX);
        assert!(integer.diagnostics.is_empty(), "{}", integer.term);

        let mut dimension = scanner("16383pt ");
        assert_eq!(dimension.scan_dimen(false, false), 16383 * 65536);
        assert!(dimension.diagnostics.is_empty(), "{}", dimension.term);
    }

    #[test]
    fn oversized_dimension_is_clamped_and_located_at_its_factor() {
        let mut engine = scanner("16384pt ");

        assert_eq!(engine.scan_dimen(false, false), 0x3FFF_FFFF);
        assert_eq!(engine.diagnostics.len(), 1, "{}", engine.term);
        let diagnostic = &engine.diagnostics[0];
        assert_eq!(diagnostic.message, "Dimension too large");
        let primary = diagnostic.primary.as_ref().unwrap();

        assert_eq!(
            (primary.name.as_str(), primary.line, primary.column),
            ("numbers.tex", 1, 1)
        );
    }
    #[test]
    fn count_register_in_unit_position_denotes_scaled_points() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.eqtb.count[0] = 1_024;
        let unit = engine.cs.intern(b"K");
        engine.eqtb.assign(unit, Equiv::CountReg(0), true);
        engine
            .input
            .push_file("numbers.tex".to_string(), b"-3\\K ".to_vec());

        assert_eq!(engine.scan_dimen(false, false), -3_072);
        assert!(engine.diagnostics.is_empty(), "{}", engine.term);
    }

    #[test]
    fn negative_oversized_dimension_clamps_symmetrically() {
        let mut engine = scanner("-16384pt ");

        assert_eq!(engine.scan_dimen(false, false), -0x3FFF_FFFF);
        assert_eq!(engine.diagnostics.len(), 1, "{}", engine.term);
        assert_eq!(engine.diagnostics[0].message, "Dimension too large");
    }
}

#[cfg(test)]
mod showthe_mark_tests {
    use super::*;

    #[test]
    fn mark_token_capture_does_not_push_the_shown_value() {
        let mut engine = Engine::new(false);
        for which in 0..engine.marks.len() {
            engine.marks[which].resize(8, Vec::new());
            engine.marks[which][7] = vec![Token::letter(b'M')];

            let tokens = engine.mark_tokens_class(which, 7);
            engine.emit_the_tokens(tokens, true);

            assert_eq!(engine.pending_the_string.as_deref(), Some("M"));
            assert!(engine.pushed.is_empty());
        }
    }

    #[test]
    fn meaning_macro_prefixes_have_no_spaces_between_prefixes() {
        // tex.web §296: print_esc does not add spaces after control words;
        // e-TeX prints `\protected\long macro:` and `\protected\long\outer macro:`,
        // not `\protected \long macro:`. expl3's \token_if_protected_long_macro:N
        // checks `\meaning` against `\protected\long macro:`.
        let mut engine = Engine::new(true);
        engine.init_primitives();

        let id = engine.cs.intern(b"testfoo");
        let m = crate::eqtb::Macro {
            replacement: Default::default(),
            prefix: Vec::new(),
            params: Vec::new(),
            body: Vec::new().into(),
            num_params: 0,
            has_param_refs: false,
            long: true,
            outer: false,
            protected: true,
        };
        engine
            .eqtb
            .assign(id, Equiv::Macro(std::rc::Rc::new(m)), true);
        assert_eq!(
            engine.meaning_of(Token::from_cs(id)),
            "\\protected\\long macro:->"
        );

        let m_plo = crate::eqtb::Macro {
            replacement: Default::default(),
            prefix: Vec::new(),
            params: Vec::new(),
            body: Vec::new().into(),
            num_params: 0,
            has_param_refs: false,
            long: true,
            outer: true,
            protected: true,
        };
        engine
            .eqtb
            .assign(id, Equiv::Macro(std::rc::Rc::new(m_plo)), true);
        assert_eq!(
            engine.meaning_of(Token::from_cs(id)),
            "\\protected\\long\\outer macro:->"
        );
    }

    #[test]
    fn meaning_macro_prefixes_honor_escapechar() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize] = b'/' as i32;

        let id = engine.cs.intern(b"testslash");
        let m = crate::eqtb::Macro {
            replacement: Default::default(),
            prefix: Vec::new(),
            params: Vec::new(),
            body: Vec::new().into(),
            num_params: 0,
            has_param_refs: false,
            long: true,
            outer: false,
            protected: true,
        };
        engine
            .eqtb
            .assign(id, Equiv::Macro(std::rc::Rc::new(m)), true);
        assert_eq!(
            engine.meaning_of(Token::from_cs(id)),
            "/protected/long macro:->"
        );
    }

    #[test]
    fn currentgrouptype_reports_correct_tex_web_codes() {
        // tex.web §5870-5887:
        // simple_group = 1, semi_simple_group = 14, math_shift_group = 15,
        // math_left_group = 16, math_group = 9.
        let mut engine = Engine::new(true);
        engine.init_primitives();

        let ty_outside = engine.int_param_value(crate::prim::IntParam::CurrentGroupType);
        assert_eq!(ty_outside, 0);

        engine.push_group_level(crate::eqtb::LevelType::Simple);
        let ty_simple = engine.int_param_value(crate::prim::IntParam::CurrentGroupType);
        assert_eq!(ty_simple, 1);
        engine.pop_group();

        engine.push_group_level(crate::eqtb::LevelType::SemiSimple);
        let ty_semi = engine.int_param_value(crate::prim::IntParam::CurrentGroupType);
        assert_eq!(ty_semi, 14);
        engine.pop_group();

        engine.push_group_level(crate::eqtb::LevelType::MathShift);
        let ty_shift = engine.int_param_value(crate::prim::IntParam::CurrentGroupType);
        assert_eq!(ty_shift, 15);

        engine.push_group_level(crate::eqtb::LevelType::MathLeft);
        let ty_left = engine.int_param_value(crate::prim::IntParam::CurrentGroupType);
        assert_eq!(ty_left, 16);
        engine.pop_group();

        engine.push_group_level(crate::eqtb::LevelType::MathGroup);
        let ty_mathgrp = engine.int_param_value(crate::prim::IntParam::CurrentGroupType);
        assert_eq!(ty_mathgrp, 9);
        engine.pop_group();

        engine.pop_group();
    }

    #[test]
    fn pagediscards_and_splitdiscards_are_registered_primitives() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        let pd = engine
            .cs
            .lookup(b"pagediscards")
            .expect("pagediscards primitive");
        assert!(matches!(
            engine.eqtb.get(pd),
            Some(Equiv::Prim(crate::prim::Prim::PageDiscards))
        ));
        let sd = engine
            .cs
            .lookup(b"splitdiscards")
            .expect("splitdiscards primitive");
        assert!(matches!(
            engine.eqtb.get(sd),
            Some(Equiv::Prim(crate::prim::Prim::SplitDiscards))
        ));
    }

    #[test]
    fn skip_case_skip_handles_nested_ifeof() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        // Run \ifcase0 matched\or\ifeof16 inner\fi\or second\fi tail
        // When case 0 matches, the subsequent \or calls skip_case_skip.
        // It must not terminate early at the inner \fi matching \ifeof.
        engine.input.push_file(
            "test.tex".to_string(),
            b"\\ifcase0 matched\\or\\ifeof16 inner\\fi\\or second\\fi tail\\par".to_vec(),
        );
        engine.run();
        assert_eq!(engine.error_count, 0);
    }

    #[test]
    fn dimension_comparison_with_space_macro_after_unit() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.eqtb.cat[b'{' as usize] = 1;
        engine.eqtb.cat[b'}' as usize] = 2;
        engine.input.push_file(
            "test_dim_space.tex".to_string(),
            br"\def\space{ }\ifnum0\ifdim10pt=10pt\space1\fi=1 YES\else NO\fi\par".to_vec(),
        );
        engine.run();
        assert_eq!(engine.error_count, 0, "{}", engine.diagnostic_output);
    }

    #[test]
    fn par_in_restricted_horizontal_mode_is_noop() {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.eqtb.cat[b'{' as usize] = 1;
        engine.eqtb.cat[b'}' as usize] = 2;
        engine.input.push_file(
            "test_par_hbox.tex".to_string(),
            br"\hbox{A\par B}\par".to_vec(),
        );
        engine.run();
        assert_eq!(engine.error_count, 0, "{}", engine.diagnostic_output);
    }
    #[test]
    fn lastnodetype_character_node_is_zero() {
        let mut engine = Engine::new(true);
        engine.mode = crate::engine::Mode::RestrictedHorizontal;
        engine
            .cur_list
            .push(crate::boxes::Node::Char { c: b'A', font: 0 });
        assert_eq!(
            engine.last_node_type_value(),
            0,
            "lastnodetype for character node should be 0"
        );
    }
}
