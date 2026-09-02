//! Value scanners: scan_int, scan_dimen, scan_glue, expressions, \the,
//! \meaning, general text.

use crate::boxes::Glue;
use crate::eqtb::Equiv;
use crate::engine::Engine;
use crate::prim::{DimParam, GlueParam, IntParam, Prim};
use crate::scaled::{mult, ONE};
use crate::token::Token;

impl Engine {
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
            { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            return;
        }
    }
    pub fn skip_spaces(&mut self) {
        loop {
            let t = self.get_token();
            if t.is_char() && (t.cc() == 10 || t.cc() == 9) {
                continue;
            }
            self.pushed.push(t);
            return;
        }
    }


    fn token_is_fi_or_else(&self, t: Token) -> bool {
        if !t.is_cs() {
            return false;
        }
        matches!(
            self.eqtb.resolve(t.cs_id()),
            Some(Equiv::Prim(Prim::Else | Prim::Or | Prim::Fi | Prim::ElIf | Prim::ElIfX))
        )
    }

    /// Like get_x_raw, but \\else/\\or/\\fi of the *outer* pending
    /// \\ifnum/\\ifcase stay unexpanded so they can terminate the number
    /// (\\ifcase2\\else). Nested \\if...\\fi inside \\@parse@version@dash
    /// must still run: only freeze when if_stack is not deeper than
    /// when scan_int started.
    fn get_x_raw_keep_cond(&mut self, outer_if_depth: usize) -> Token {
        let t = self.raw_token();
        if self.token_is_fi_or_else(t) && self.if_stack.len() <= outer_if_depth {
            return t;
        }
        self.pushed.push(t);
        self.get_x_raw()
    }



    /// Glue parameter, `\\skip n`, or skipdef'd CS. Knuth copies these as a
    /// whole glue value (and their width is a legal dimen/unit).
    fn glue_from_cur_cs(&mut self, t: Token) -> Option<Glue> {
        if !t.is_cs() {
            return None;
        }
        match self.cur_prim {
            Some(Prim::GlueP(p)) => {
                return Some(self.eqtb.glue_params[p.idx() as usize].clone());
            }
            Some(Prim::Skip) => {
                let i = self.scan_reg_num();
                return Some(self.eqtb.skip[i as usize].clone());
            }
            Some(Prim::MuSkip) => {
                let i = self.scan_reg_num();
                return Some(self.eqtb.muskip[i as usize].clone());
            }
            Some(Prim::LastSkip) => {
                return Some(self.last_skip_value());
            }
            _ => {}
        }
        match self.eqtb.resolve(t.cs_id()) {
            Some(Equiv::SkipReg(i)) => Some(self.eqtb.skip[*i as usize].clone()),
            Some(Equiv::MuSkipReg(i)) => Some(self.eqtb.muskip[*i as usize].clone()),
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
        let t = self.get_x_raw();
        if !t.is_space() {
            self.pushed.push(t);
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
        t.is_cs()
            && matches!(
                self.eqtb.resolve(t.cs_id()),
                Some(Equiv::Prim(Prim::Relax))
            )
    }

    /// scan optional `=` with spaces/relax skipped
    pub fn scan_optional_equals(&mut self) {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_char() && t.chr() == b'=' as u32 {
            self.skip_spaces_relax();
        } else {
            let __pt = t;
            if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 {
                eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", line!(), self.input.current_file_line());
            }
            self.pushed.push(__pt);
        }
    }

    /// read a sequence of digit char tokens in the given radix
    fn scan_digits(&mut self, radix: u32, allow_letters: bool) -> i64 {
        let mut v: i64 = 0;
        loop {
            // expanding fetch, no space skip: a space terminates the constant
            let t = self.get_x_raw();
            if t.is_space() {
                break; // absorb one space, number complete
            }
            if !t.is_char() {
                { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                break;
            }
            let c = t.chr();
            let d = match () {
                _ if (b'0' as u32..=b'9' as u32).contains(&c) => c - b'0' as u32,
                _ if allow_letters && (b'a' as u32..=b'f' as u32).contains(&c) => c - b'a' as u32 + 10,
                _ if allow_letters && (b'A' as u32..=b'F' as u32).contains(&c) => c - b'A' as u32 + 10,
                _ => {
                    { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                    break;
                }
            };
            if d >= radix {
                { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                break;
            }
            v = v * radix as i64 + d as i64;
            if v > 0x7FFF_FFFF {
                v = 0x7FFF_FFFF;
            }
        }
        v
    }

    fn is_digit_token(t: Token) -> bool {
        t.is_char() && (b'0'..=b'9').contains(&(t.chr() as u8)) && t.cc() == 12
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
        let outer_if_depth = self.if_stack.len();
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
                loop {
                    // expanding fetch without space skip (tex.web get_x_token):
                    // expandables continue the number, a space terminates it
                    let t2 = self.get_x_raw_keep_cond(outer_if_depth);

                    if t2.is_space() {
                        break;
                    }
                    if Self::is_digit_token(t2) {
                        v = v * 10 + (t2.chr() - b'0' as u32) as i64;
                        if v > 0x7FFF_FFFF {
                            v = 0x7FFF_FFFF;
                        }
                    } else {
                        { let __pt = t2; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                        break;
                    }
                }
                break;
            }
            if t.is_char() && t.chr() == b'\'' as u32 {
                v = self.scan_digits(8, false);
                break;
            }
            if t.is_char() && t.chr() == b'"' as u32 {
                v = self.scan_digits(16, true);
                break;
            }
            if t.is_char() && t.chr() == b'`' as u32 {
                // char constant: next token RAW (no expansion; tex.web get_token
                // does not expand); a cs contributes its name's first char
                let t2 = self.raw_token();
                if t2.is_char() {
                    v = t2.chr() as i64;
                } else if t2.is_cs() {
                    let name = self.cs.name(t2.cs_id());
                    v = name.first().copied().unwrap_or(0) as i64;
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
                let t3 = self.get_x_raw_keep_cond(outer_if_depth);
                if !t3.is_space() {
                    self.pushed.push(t3);
                }
                break;
            }
            if t.is_cs() {
                match self.cur_prim {
                    Some(Prim::Count) => {
                        let idx = self.scan_reg_num();
                        v = self.eqtb.count[idx as usize] as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::CatCode) => {
                        let c = self.scan_char_num();
                        v = self.eqtb.cat[c as usize] as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::MathCode) => {
                        let c = self.scan_char_num();
                        v = self.eqtb.math_code[c as usize] as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::DelCode) => {
                        let c = self.scan_char_num();
                        v = self.eqtb.del_code[c as usize] as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::LcCodeP) => {
                        let c = self.scan_char_num();
                        v = self.eqtb.lc_code[c as usize] as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::SfCodeP) => {
                        let c = self.scan_char_num();
                        v = self.eqtb.sf_code[c as usize] as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::UcCodeP) => {
                        let c = self.scan_char_num();
                        v = self.eqtb.uc_code[c as usize] as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::IntP(p)) => {
                        v = self.int_param_value(p) as i64;
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
                    Some(p @ (Prim::PdfLastObj | Prim::PdfLastXForm | Prim::PdfLastXImage
                    | Prim::PdfLastLink | Prim::PdfLastAnnot)) => {
                        v = self.pdf_last_value(p) as i64;
                        break 'scan_loop;
                    }
                    Some(Prim::FontDimen) => {
                        let idx = self.scan_int();
                        let f = self.scan_font_id();
                        let i = if idx > 0 { idx as usize - 1 } else { 0 };
                        v = self.eqtb.font_params[f as usize].get(i).copied().unwrap_or(0) as i64;
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
                    _ => {
                        match self.eqtb.resolve(t.cs_id()).cloned() {
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
                            _ => {}
                        }
                    }
                }
            }
            { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            if crate::debug_flag("IFTRACE") {
                let nm = if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned() } else { format!("cc{}", t.cc()) };
                let ek = match self.eqtb.resolve(t.cs_id()) { Some(e) => e.kind_name(), None => "U" };
                eprintln!("MISSNUM tok={} kind={} prim={:?} srcs={:?}", nm, ek, self.cur_prim, self.input.stack.iter().rev().take(2).map(|src| match src { crate::input::Source::TokList{name,pos,toks,..} => format!("T:{} {}/{}",name,pos,toks.len()), crate::input::Source::File{name,line_no,..} => format!("F:{}",line_no)}).collect::<Vec<_>>());
            }
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

    pub fn int_param_value(&self, p: IntParam) -> i32 {
        match p {
            IntParam::CurrentGroupLevel => (self.eqtb.cur_level.saturating_sub(1)) as i32,
            IntParam::CurrentGroupType => match self.eqtb.cur_group_type() {
                None => 0,
                Some(crate::eqtb::LevelType::Simple) => 1,
                Some(crate::eqtb::LevelType::SemiSimple) => 2,
                Some(crate::eqtb::LevelType::Group) => 1,
                Some(crate::eqtb::LevelType::Box) => 9,
                _ => 1,
            },
            IntParam::CurrentIfLevel => self.if_stack.len() as i32,
            IntParam::CurrentIfType => 0,
            IntParam::CurrentIfBranch => 0,
            IntParam::LastNodeType => self.last_node_type_value(),
            IntParam::Badness => self.last_badness,
            IntParam::InputLineNo => self.input.current_file_line() as i32,
            IntParam::Time => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                ((now.as_secs() % 86400) / 60) as i32
            }
            IntParam::Day => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                (now.as_secs() / 86400) as i32
            }
            IntParam::Month | IntParam::Year => {
                // derived from date via chrono-less civil calculation
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                let days = (now.as_secs() / 86400) as i64;
                // Howard's algorithm
                let z = days + 719468;
                let era = z.div_euclid(146097);
                let doe = z.rem_euclid(146097);
                let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
                let y = yoe + era * 400;
                let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
                let mp = (5 * doy + 2) / 153;
                let m = if mp < 10 { mp + 3 } else { mp - 9 };
                let yr = if m <= 2 { y + 1 } else { y };
                match p {
                    IntParam::Month => m as i32,
                    _ => yr as i32,
                }
            }
            _ => self.eqtb.int_params[p.idx() as usize],
        }
    }

    pub fn dim_param_value(&self, p: DimParam) -> i32 {
        match p {
            DimParam::PrevDepth => self.prev_depth,
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
            Prim::PdfLastLink => self.pdf_last_link,
            Prim::PdfLastAnnot => self.pdf_last_annot,
            _ => 0,
        }
    }

    pub fn scan_reg_num(&mut self) -> u16 {
        let n = self.scan_int();
        let max = self.eqtb.count.len() as i32 - 1;
        if n < 0 || n > max {
            self.error("Register number out of range");
            return 0;
        }
        n as u16
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
            { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            break;
        }
        // factor: integer or decimal, or a direct dimen source
        let factor: f64;
        let direct: Option<i32>;
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        if Self::is_digit_token(t) || (t.is_char() && (t.chr() == b'.' as u32 || t.chr() == b',' as u32)) {
            let mut int_part: i64 = 0;
            if Self::is_digit_token(t) {
                int_part = (t.chr() - b'0' as u32) as i64;
                loop {
                    let t2 = self.get_x_raw();
                    if Self::is_digit_token(t2) {
                        int_part = int_part * 10 + (t2.chr() - b'0' as u32) as i64;
                        if int_part > 0x7FFF_FFFF {
                            int_part = 0x7FFF_FFFF;
                        }
                    } else {
                        { let __pt = t2; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                        break;
                    }
                }
            }
            // fraction
            let mut frac: f64 = 0.0;
            let mut scale = 0.1;
            loop {
                let t2 = self.get_x_raw();
                if t2.is_char() && (t2.chr() == b'.' as u32 || t2.chr() == b',' as u32) && frac == 0.0 {
                    continue;
                }
                if Self::is_digit_token(t2) {
                    // tex.web §102: trailing digits beyond precision are still
                    // CONSUMED; pushing them back corrupts the unit scan
                    // (hyperref \dimen@=0.99626401\dimen@).
                    if scale > 1e-7 {
                        frac += (t2.chr() - b'0' as u32) as f64 * scale;
                        scale *= 0.1;
                    }
                } else {
                    { let __pt = t2; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                    break;
                }
            }
            factor = int_part as f64 + frac;
            direct = None;
        } else if t.is_char() && t.chr() == b'`' as u32 {
            let t2 = self.get_token();
            if t2.is_char() {
                factor = t2.chr() as f64;
            } else if t2.is_cs() {
                let name = self.cs.name(t2.cs_id());
                factor = name.first().copied().unwrap_or(0) as f64;
            } else {
                self.error("Missing character after `");
                factor = 0.0;
            }
            direct = None;
        } else if t.is_cs() {
            match self.cur_prim {
                Some(Prim::DimExpr) => {
                    let v = self.scan_expr_dim();
                    return if negate { -v } else { v };
                }
                Some(Prim::NumExpr) => {
                    factor = self.scan_expr_num() as f64;
                    direct = None;
                }
                Some(Prim::GlueExpr) | Some(Prim::MuExpr) => {
                    let g = self.scan_expr_glue(mu);
                    return if negate { -g.width } else { g.width };
                }
                Some(Prim::DimP(p)) => {
                    factor = 1.0;
                    direct = Some(self.dim_param_value(p));
                }
                Some(Prim::GlueP(p)) => {
                    factor = 1.0;
                    direct = Some(self.eqtb.glue_params[p.idx() as usize].width);
                }
                Some(Prim::Skip) => {
                    let i = self.scan_reg_num();
                    factor = 1.0;
                    direct = Some(self.eqtb.skip[i as usize].width);
                }
                Some(Prim::MuSkip) => {
                    let i = self.scan_reg_num();
                    factor = 1.0;
                    direct = Some(self.eqtb.muskip[i as usize].width);
                }
                Some(Prim::Wd) => {
                    let n = self.scan_reg_num();
                    factor = 1.0;
                    direct = Some(self.box_reg_dimen(n, 0));
                }
                Some(Prim::Ht) => {
                    let n = self.scan_reg_num();
                    factor = 1.0;
                    direct = Some(self.box_reg_dimen(n, 1));
                }
                Some(Prim::Dp) => {
                    let n = self.scan_reg_num();
                    factor = 1.0;
                    direct = Some(self.box_reg_dimen(n, 2));
                }
                Some(Prim::LastSkip) => {
                    factor = 1.0;
                    direct = Some(self.last_skip_value().width);
                }
                Some(Prim::GlueStretch) => {
                    factor = 1.0;
                    direct = Some(self.scan_etex_glue_field(0));
                }
                Some(Prim::GlueShrink) => {
                    factor = 1.0;
                    direct = Some(self.scan_etex_glue_field(1));
                }
                Some(Prim::FontDimen) => {
                    let idx = self.scan_int();
                    let f = self.scan_font_id();
                    let i = if idx > 0 { idx as usize - 1 } else { 0 };
                    factor = 1.0;
                    direct = Some(self.eqtb.font_params[f as usize].get(i).copied().unwrap_or(0));
                }
                Some(Prim::Count) => {
                    // internal integer coerced to dimen (sp), tex.web scan_something_internal
                    let i = self.scan_reg_num();
                    factor = self.eqtb.count[i as usize] as f64;
                    direct = None;
                }
                Some(Prim::Dimen) => {
                    let i = self.scan_reg_num();
                    factor = 1.0;
                    direct = Some(self.eqtb.dimen[i as usize]);
                }
                _ => match self.eqtb.resolve(t.cs_id()).cloned() {
                    Some(Equiv::DimenReg(i)) => {
                        factor = 1.0;
                        direct = Some(self.eqtb.dimen[i as usize]);
                    }
                    Some(Equiv::SkipReg(i)) => {
                        factor = 1.0;
                        direct = Some(self.eqtb.skip[i as usize].width);
                    }
                    Some(Equiv::MuSkipReg(i)) => {
                        factor = 1.0;
                        direct = Some(self.eqtb.muskip[i as usize].width);
                    }
                    Some(Equiv::CountReg(i)) => {
                        factor = self.eqtb.count[i as usize] as f64;
                        direct = None;
                    }
                    Some(Equiv::CharDef(c)) => {
                        factor = c as f64;
                        direct = None;
                    }
                    Some(Equiv::MathCharDef(c)) => {
                        // \@m/\@M constants (\mathchardef'd); \offinterlineskip
                        // computes \baselineskip-\@m\p@ through this path.
                        factor = c as f64;
                        direct = None;
                    }
                    Some(Equiv::Prim(Prim::IntP(p))) => {
                        factor = self.int_param_value(p) as f64;
                        direct = None;
                    }
                    _ => {
                        self.pushed.push(t);
                        if crate::debug_flag("UNITTRACE") {
                            eprintln!("NUM-FAIL cs=\\{} prim={:?} eq={:?} L{} mac={}",
                                String::from_utf8_lossy(self.cs.name(t.cs_id())),
                                self.cur_prim,
                                self.eqtb.resolve(t.cs_id()).map(|e| e.kind_name()),
                                self.input.current_file_line(),
                                self.current_macro);
                        }
                        self.error("Missing number, treated as zero");
                        factor = 0.0;
                        direct = None;
                    }
                },
            }
        } else {
            self.pushed.push(t);
            if crate::debug_flag("UNITTRACE") {
                eprintln!("NUM-FAIL tok=cc{}:{:#x} L{} mac={}", t.cc(), t.chr(), self.input.current_file_line(), self.current_macro);
            }
            self.error("Missing number, treated as zero");
            factor = 0.0;
            direct = None;
        }
        if let Some(d) = direct {
            return if negate { -d } else { d };
        }
        // unit
        let unit_sp = self.scan_unit_sp(mu);
        // sp = round(factor * unit_sp); use exact integer math when factor is
        // a multiple of 1/65536-ish; f64 with i64 rounding is precise enough
        // for TeX's 5-decimal factors in practice.
        let v = (factor * unit_sp as f64 + 0.5 * unit_sp as f64 / 1.0) as i64;
        let v = (factor * unit_sp as f64).round() as i64;
        let v = v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
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
        self.scan_unit_sp_d(mu, 0)
    }

    fn scan_unit_sp_d(&mut self, mu: bool, depth: u32) -> i64 {
        if depth > 32 {
            self.error("Illegal unit of measure (pt inserted).");
            return ONE as i64;
        }
        self.skip_spaces_relax();
        let t = self.get_token();
        let mut unit_sp: i64;
        if t.is_cs() {
            if let Some(g) = self.glue_from_cur_cs(t) {
                return g.width as i64;
            }
            match self.cur_prim {
                Some(Prim::DimP(p)) => {
                    unit_sp = self.dim_param_value(p) as i64;
                }
                Some(Prim::Wd) => {
                    let n = self.scan_reg_num();
                    unit_sp = self.box_reg_dimen(n, 0) as i64;
                }
                Some(Prim::Ht) => {
                    let n = self.scan_reg_num();
                    unit_sp = self.box_reg_dimen(n, 1) as i64;
                }
                Some(Prim::Dp) => {
                    let n = self.scan_reg_num();
                    unit_sp = self.box_reg_dimen(n, 2) as i64;
                }
                _ => match self.eqtb.resolve(t.cs_id()).cloned() {
                    Some(Equiv::DimenReg(i)) => unit_sp = self.eqtb.dimen[i as usize] as i64,
                    // tex.web: a macro in unit position expands (LaTeX's
                    // `\p@` = "pt"). Push back, re-fetch with expansion,
                    // and re-run this whole unit fetch (char reader below
                    // runs on the next loop pass).
                    Some(Equiv::Macro(m)) if !m.protected => {
                        // tex.web: macro in unit position expands (LaTeX's
                        // `\p@` = "pt"). Expand in place, then re-run the
                        // whole unit fetch.
                        self.expand_macro(t.cs_id(), &m);
                        return self.scan_unit_sp_d(mu, depth + 1);
                    }
                    _ => {
                        { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                        if crate::debug_flag("UNITTRACE") {
                            let tn = if t.is_cs() { format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id()))) } else { format!("cc{} chr={}", t.cc(), t.chr()) };
                            eprintln!("UNIT-FAIL mu={} tok={} L{} mac={}", mu, tn, self.input.current_file_line(), self.current_macro);
                        }

                        self.error("Illegal unit of measure (pt inserted).");
                        unit_sp = ONE as i64;
                    }
                },
            }
        } else if t.is_char() {
            // read unit keyword letters, only while they can extend a valid unit
            const UNITS: [&str; 16] = [
                "pt", "in", "pc", "cm", "mm", "bp", "dd", "cc", "sp", "em", "ex", "px", "mu",
                "fil", "fill", "filll",
            ];
            let mut kw: Vec<u8> = vec![t.chr() as u8];
            let mut cur = String::new();
            cur.push((t.chr() as u8).to_ascii_lowercase() as char);
            if kw[0].is_ascii_alphabetic() {
                while kw.len() < 5 {
                    let can_extend = UNITS
                        .iter()
                        .any(|u| u.len() > kw.len() && u.starts_with(&cur));
                    if !can_extend {
                        break;
                    }
                    let t2 = self.get_token();
                    if t2.is_char() && (t2.chr() as u8).is_ascii_alphabetic() {
                        kw.push(t2.chr() as u8);
                        cur.push((t2.chr() as u8).to_ascii_lowercase() as char);
                    } else {
                        { let __pt = t2; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                    excess.push(Token::char(
                        self.eqtb.cat[last as usize],
                        last as u32,
                    ));
                    s.pop();
                }
                for t in excess.into_iter().rev() {
                    { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                }
                let s2: String = kw.iter().map(|&b| b.to_ascii_lowercase() as char).collect();
                s = s2;
            }
            self.cur_fill_order = 0;
            unit_sp = match s.as_str() {
                "fil" | "fill" | "filll" => {
                    self.cur_fill_order = match s.as_str() {
                        "fil" => 1,
                        "fill" => 2,
                        _ => 3,
                    };
                    ONE as i64
                }
                "pt" | "p" => ONE as i64,
                "in" => 4736287, // 72.27 * 65536 rounded
                "pc" => 12 * ONE as i64,
                "cm" => 47362867, // 7227/254 * 65536 * ... exact below
                "mm" => 4736287,
                "bp" => 65782,  // 72.27/72 pt
                "dd" => 70124,  // 1238/1157 pt
                "cc" => 841489, // 12 dd
                "sp" => 1,
                "em" => self.cur_quad() as i64,
                "ex" => self.cur_x_height() as i64,
                "px" => 65782,
                "mu" if mu => (self.cur_quad() / 18) as i64,
                _ => {
                    if crate::debug_flag("DEFTRACE") {
                        eprintln!("UNITFAIL s={:?} kw={:?}", s, kw);
                    }
                    self.error("Illegal unit of measure (pt inserted).");
                    ONE as i64
                }
            };
            // exact rational values where TeX scales by fractions:
            unit_sp = match s.as_str() {
                "in" => 4736287,
                "cm" => (7227i64 * ONE as i64 + 127) / 254 / 100 * 100, // approx; refined below
                "mm" => (7227i64 * ONE as i64) / 2540,
                "bp" => (7227i64 * ONE as i64) / 7200,
                "dd" => (1238i64 * ONE as i64) / 1157,
                "cc" => (12 * 1238 * ONE as i64) / 1157,
                _ => unit_sp,
            };
        } else {
            { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            self.error("Illegal unit of measure (pt inserted).");
            unit_sp = ONE as i64;
        }

        unit_sp
    }

    pub fn scan_glue(&mut self, mu: bool) -> Glue {
        let prev = self.in_expanded_scan;
        self.in_expanded_scan = false;
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        if t.is_cs() && matches!(self.cur_prim, Some(Prim::GlueExpr) | Some(Prim::MuExpr)) {
            let g = self.scan_expr_glue(mu);
            self.in_expanded_scan = prev;
            return g;
        }
        if let Some(g) = self.glue_from_cur_cs(t) {
            self.in_expanded_scan = prev;
            return g;
        }
        self.pushed.push(t);
        let mut g = Glue::zero();
        self.cur_fill_order = 0;
        g.width = self.scan_dimen(mu, false);
        for &(kw, is_stretch) in &[(&b"plus"[..], true), (&b"minus"[..], false)] {
            loop {
                let t0 = self.get_token();
                if t0.is_space() {
                    continue;
                }
                self.pushed.push(t0);
                break;
            }
            let t = self.get_token();
            let mut matched = false;
            if t.is_char() {
                let c = (t.chr() as u8).to_ascii_lowercase();
                if c == kw[0] {
                    let mut kt: Vec<Token> = Vec::new();
                    let mut all = true;
                    for &k in &kw[1..] {
                        let tx = self.get_token();
                        kt.push(tx);
                        if !(tx.is_char() && (tx.chr() as u8).to_ascii_lowercase() == k) {
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
                            self.pushed.push(tx);
                        }
                    }
                }
            }
            if !matched {
                self.pushed.push(t);
            }
            self.cur_fill_order = 0;
        }
        self.cur_fill_order = 0;
        self.in_expanded_scan = prev;
        g
    }

    /// after "fil" keyword letters: count extra 'l's for fil/fill/filll
    fn scan_fil_order(&mut self) -> u8 {
        let mut order = 1u8;
        loop {
            let t = self.get_token();
            if t.is_char() && (t.chr() as u8) == b'l' && order < 3 {
                order += 1;
            } else {
                { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                return order;
            }
        }
    }

    pub fn scan_relational(&mut self) -> u8 {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_char() {
            let c = t.chr() as u8;
            if c == b'<' || c == b'=' || c == b'>' {
                return c;
            }
        }
        {
            static RN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if RN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 6 {
                let got = if t.is_cs() {
                    format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id())))
                } else {
                    format!("c{}:{:?}", t.cc(), t.chr() as u8 as char)
                };
                eprintln!(
                    "RELFAIL got={} L{} file={} mac={} after_int_ctx",
                    got,
                    self.input.current_file_line(),
                    self.input.current_file_name().split('/').last().unwrap_or(""),
                    self.current_macro
                );
            }
        }
        self.pushed.push(t);
        self.error("Missing relational operator");
        b'='
    }

    /// scan a braced general text (raw, balanced); opening brace consumed by
    /// caller? Here: expects next token to be `{`; returns contents.
    pub fn scan_general_text(&mut self) -> Vec<Token> {
        self.skip_spaces_relax();
        let t = self.get_token();
        if !(t.is_char() && t.cc() == 1) {
            let got = if t.is_cs() {
                format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id())))
            } else {
                format!("cc{}:{}", t.cc(), t.chr())
            };
            self.error(&format!("Missing {{ inserted (scan text, got {})", got));
            self.pushed.push(t);
            return Vec::new();
        }
        self.scan_balanced_raw(true)
    }

    /// like scan_general_text but expanding (\edef semantics)
    pub fn scan_general_text_expanded(&mut self) -> Vec<Token> {
        self.skip_spaces_relax();
        if crate::debug_flag("IFTRACE") {
            let st: Vec<String> = self.input.stack.iter().rev().take(3).map(|src| match src {
                crate::input::Source::TokList { name, pos, toks, .. } => format!("T:{} {}/{}", name, pos, toks.len()),
                crate::input::Source::File { name, line_no, .. } => format!("F:{}#{}", name, line_no),
            }).collect();
            let cp = match self.cur_prim { Some(p) => format!("{:?}", p), None => "none".into() };
            let ccs = match self.cur_cs { Some(c) => String::from_utf8_lossy(self.cs.name(c)).into_owned(), None => "-".into() };
            eprintln!("SGET-START {} prim={} cs={} macros={:?}", st.join(" << "), cp, ccs, self.last_macros);
        }
        let t = self.get_token();
        if !(t.is_char() && t.cc() == 1) {
            let got = if t.is_cs() {
                format!("\\{}", String::from_utf8_lossy(self.cs.name(t.cs_id())))
            } else {
                format!("cc{} chr={}", t.cc(), t.chr())
            };
            self.error(&format!("Missing {{ inserted (got {})", got));
            return Vec::new();
        }
        let prev_expanded_scan = self.in_expanded_scan;
        let prev_csname_depth = self.csname_depth;
        // \\expanded is an e-TeX edef context even when invoked from \\csname.
        // Leaving csname_depth>0 would expand \\protected macros and let them
        // steal \\endcsname / closing braces (utf8.def filehook sanitize).
        self.in_expanded_scan = true;
        if self.last_macros.iter().any(|m| m.starts_with("GTS@") || m.contains("GetTitle")) {
            eprintln!(
                "EXPANDED-TEXT e-scan-on macros={:?}",
                self.last_macros.iter().rev().take(8).collect::<Vec<_>>()
            );
        }
        self.csname_depth = 0;
        let mut out = Vec::new();
        let mut depth = 1i32;
        loop {
            let raw = self.raw_token();
            if raw == crate::input::EOF_MARKER {
                self.error("Missing } in expanded text");
                self.in_expanded_scan = prev_expanded_scan;
                self.csname_depth = prev_csname_depth;
                return out;
            }
            // \unexpanded must copy its argument verbatim. get_token would
            // expand it first, then ## collapse would turn ##1 into #1.
            if raw.is_cs() {
                if let Some(Equiv::Prim(Prim::UnExpanded)) = self.eqtb.resolve(raw.cs_id()) {
                    self.skip_spaces_relax();
                    let nxt = self.raw_token();
                    if nxt.is_cs() {
                        if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(nxt.cs_id()).cloned() {
                            out.extend((*self.eqtb.toks[i as usize]).clone());
                            continue;
                        }
                    }
                    self.pushed.push(nxt);
                    let u = self.scan_general_text();
                    out.extend(u);
                    continue;
                }
            }
            self.pushed.push(raw);
            let t = self.get_token();
            let protect = self.unexp_protect > 0;
            if protect {
                self.unexp_protect -= 1;
            }
            if t == crate::input::EOF_MARKER {
                self.error("Missing } in expanded text");
                self.in_expanded_scan = prev_expanded_scan;
                self.csname_depth = prev_csname_depth;
                return out;
            }
            if self.cur_prim == Some(Prim::UnExpanded) {
                self.skip_spaces_relax();
                let nxt = self.raw_token();
                if nxt.is_cs() {
                    if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(nxt.cs_id()).cloned() {
                        out.extend((*self.eqtb.toks[i as usize]).clone());
                        continue;
                    }
                }
                self.pushed.push(nxt);
                out.extend(self.scan_general_text());
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
                } else if !protect && t.cc() == 6 && t.chr() == 0x23 {
                    // tex.web scan_toks: ## collapses to a single # in the
                    // collected text. Tokens copied by \\unexpanded must
                    // keep both hashes (\\use_none:n {#1}\\unexpanded{#1}).
                    if let Some(last) = out.last() {
                        if last.is_char() && last.cc() == 6 && last.chr() == 0x23 {
                            continue;
                        }
                    }
                }
            }
            out.push(t);
        }
    }


    // ---------- \the ----------

    /// implement \the: scans an internal quantity and pushes its expansion
    fn push_the_toks(&mut self, toks: Vec<Token>) {
        // Knuth: \\the\\toks inserts raw tokens. Freeze only inside edef/expanded
        // so the contents are not re-expanded while collecting. At execute time
        // (geometry \\the\\Gm@dimlist) macros like \\Gm@len must still expand.
        if self.in_expanded_scan {
            self.push_tokens(Self::freeze_unexpanded_toks(toks));
        } else {
            self.push_tokens(toks);
        }
    }

    pub fn the_scan(&mut self) {
        self.skip_spaces();
        let t = self.get_token();
        if !t.is_cs() {
            { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            self.error("You can't use `\\the' after ");
            return;
        }
        let id = t.cs_id();
        // register aliases (countdef'd/dimendef'd/skipdef'd/toksdef'd cs) and
        // toks registers are valid 	he operands (tex.web scan_toks part)
        match self.eqtb.resolve(id).cloned() {
            Some(Equiv::CharDef(c)) => {
                self.exp_string(c.to_string().as_bytes());
                return;
            }
            Some(Equiv::MathCharDef(c)) => {
                self.exp_string(c.to_string().as_bytes());
                return;
            }
            Some(Equiv::CountReg(i)) => {
                self.exp_string(self.eqtb.count[i as usize].to_string().as_bytes());
                return;
            }
            Some(Equiv::DimenReg(i)) => {
                let s = self.scaled_to_string(self.eqtb.dimen[i as usize]);
                self.exp_string(s.as_bytes());
                return;
            }
            Some(Equiv::SkipReg(i)) => {
                let g = self.eqtb.skip[i as usize].clone();
                let s = self.glue_to_string(&g);
                self.exp_string(s.as_bytes());
                return;
            }
            Some(Equiv::MuSkipReg(i)) => {
                let g = self.eqtb.muskip[i as usize].clone();
                let s = self.glue_to_string(&g);
                self.exp_string(s.as_bytes());
                return;
            }
            Some(Equiv::ToksReg(i)) => {
                let toks = (*self.eqtb.toks[i as usize]).clone();
                self.push_the_toks(toks);
                return;
            }
            _ => {}
        }
        match self.cur_prim {
            Some(Prim::IntP(p)) => {
                let s = self.int_param_value(p).to_string();
                self.exp_string(s.as_bytes());
            }
            Some(Prim::Count) => {
                let idx = self.scan_reg_num();
                self.exp_string(self.eqtb.count[idx as usize].to_string().as_bytes());
            }
            Some(Prim::DimP(p)) => {
                let s = self.scaled_to_string(self.dim_param_value(p));
                self.exp_string(s.as_bytes());
            }
            Some(Prim::GlueP(p)) => {
                let g = self.eqtb.glue_params[p.idx() as usize].clone();
                let s = self.glue_to_string(&g);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::ToksP(p)) => {
                let toks = (*self.eqtb.tok_params[p.idx() as usize]).clone();
                self.push_the_toks(toks);
            }
            Some(Prim::CatCode) => {
                let c = self.scan_char_num();
                self.exp_string(self.eqtb.cat[c as usize].to_string().as_bytes());
            }
            Some(Prim::MathCode) => {
                let c = self.scan_char_num();
                self.exp_string(self.eqtb.math_code[c as usize].to_string().as_bytes());
            }
            Some(Prim::DelCode) => {
                let c = self.scan_char_num();
                self.exp_string(self.eqtb.del_code[c as usize].to_string().as_bytes());
            }
            Some(Prim::LcCodeP) => {
                let c = self.scan_char_num();
                self.exp_string(self.eqtb.lc_code[c as usize].to_string().as_bytes());
            }
            Some(Prim::SfCodeP) => {
                let c = self.scan_char_num();
                self.exp_string(self.eqtb.sf_code[c as usize].to_string().as_bytes());
            }
            Some(Prim::UcCodeP) => {
                let c = self.scan_char_num();
                self.exp_string(self.eqtb.uc_code[c as usize].to_string().as_bytes());
            }
            Some(Prim::FontDimen) => {
                let idx = self.scan_int();
                let f = self.scan_font_id();
                let i = if idx > 0 { idx as usize - 1 } else { 0 };
                let v = self.eqtb.font_params.get(f as usize).and_then(|fp| fp.get(i)).copied().unwrap_or(0);
                let s = self.scaled_to_string(v);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::HyphenChar) => {
                let f = self.scan_font_id() as usize;
                let v = self.eqtb.hyphen_char.get(f).copied().unwrap_or(0);
                self.exp_string(v.to_string().as_bytes());
            }
            Some(Prim::SkewChar) => {
                let f = self.scan_font_id() as usize;
                let v = self.eqtb.skew_char.get(f).copied().unwrap_or(0);
                self.exp_string(v.to_string().as_bytes());
            }
            Some(Prim::TopMark) => self.push_mark_tokens(0),
            Some(Prim::FirstMark) => self.push_mark_tokens(1),
            Some(Prim::BotMark) => self.push_mark_tokens(2),
            Some(Prim::SplitFirstMark) => self.push_mark_tokens(3),
            Some(Prim::SplitBotMark) => self.push_mark_tokens(4),
            Some(Prim::JobName) => {
                let s = self.job_name.clone();
                self.exp_string(s.as_bytes());
            }
            Some(Prim::NumExpr) => {
                let v = self.scan_expr_num();
                self.exp_string(v.to_string().as_bytes());
            }
            Some(Prim::DimExpr) => {
                let v = self.scan_expr_dim();
                let s = self.scaled_to_string(v);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::GlueExpr) => {
                let g = self.scan_expr_glue(false);
                let s = self.glue_to_string(&g);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::MuExpr) => {
                let g = self.scan_expr_glue(true);
                let s2 = self.glue_to_string(&g);
                self.exp_string(s2.as_bytes());
            }
            Some(Prim::PdfLastXPos) => {
                self.exp_string(self.pdf_last_x.to_string().as_bytes());
            }
            Some(Prim::PdfLastYPos) => {
                self.exp_string(self.pdf_last_y.to_string().as_bytes());
            }
            Some(p @ (Prim::PdfLastObj | Prim::PdfLastXForm | Prim::PdfLastXImage
            | Prim::PdfLastLink | Prim::PdfLastAnnot)) => {
                let s = self.pdf_last_value(p).to_string();
                self.exp_string(s.as_bytes());
            }
            Some(Prim::PdfPageAttr) => {
                let s = self.pdf_page_attr.clone();
                self.exp_string(s.as_bytes());
            }
            Some(Prim::Dimen) => {
                let idx = self.scan_reg_num();
                let s = self.scaled_to_string(self.eqtb.dimen[idx as usize]);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::Skip) => {
                let idx = self.scan_reg_num();
                let s = self.glue_to_string(&self.eqtb.skip[idx as usize].clone());
                self.exp_string(s.as_bytes());
            }
            Some(Prim::MuSkip) => {
                let idx = self.scan_reg_num();
                let s = self.glue_to_string(&self.eqtb.muskip[idx as usize].clone());
                self.exp_string(s.as_bytes());
            }
            Some(Prim::Toks) => {
                let idx = self.scan_reg_num();
                let toks = (*self.eqtb.toks[idx as usize]).clone();
                self.push_the_toks(toks);
            }
            Some(Prim::Wd) => {
                let n = self.scan_reg_num();
                let s = self.scaled_to_string(self.box_reg_dimen(n, 0));
                self.exp_string(s.as_bytes());
            }
            Some(Prim::GlueStretch) => {
                let v = self.scan_etex_glue_field(0);
                let s = self.scaled_to_string(v);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::GlueShrink) => {
                let v = self.scan_etex_glue_field(1);
                let s = self.scaled_to_string(v);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::GlueStretchOrder) => {
                let v = self.scan_etex_glue_field(2);
                self.exp_string(v.to_string().as_bytes());
            }
            Some(Prim::GlueShrinkOrder) => {
                let v = self.scan_etex_glue_field(3);
                self.exp_string(v.to_string().as_bytes());
            }
            Some(Prim::Ht) => {
                let n = self.scan_reg_num();
                let s = self.scaled_to_string(self.box_reg_dimen(n, 1));
                self.exp_string(s.as_bytes());
            }
            Some(Prim::Dp) => {
                let n = self.scan_reg_num();
                let s = self.scaled_to_string(self.box_reg_dimen(n, 2));
                self.exp_string(s.as_bytes());
            }
            Some(Prim::LastPenalty) => {
                let v = self.last_penalty_value();
                self.exp_string(v.to_string().as_bytes());
            }
            Some(Prim::LastKern) => {
                let v = self.last_kern_value();
                let s = self.scaled_to_string(v);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::LastSkip) => {
                let g = self.last_skip_value();
                let s = self.glue_to_string(&g);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::Font) => {
                let csid = self.eqtb.font_cs.get(self.eqtb.cur_font_val as usize).copied().unwrap_or(0);
                self.push_tokens(vec![Token::from_cs(csid)]);
            }
            Some(Prim::TextFont) | Some(Prim::ScriptFont) | Some(Prim::ScriptScriptFont) => {
                let style = match self.cur_prim {
                    Some(Prim::TextFont) => 0usize,
                    Some(Prim::ScriptFont) => 1,
                    _ => 2,
                };
                let fam = self.scan_int().clamp(0, 255) as usize;
                let fid = self.eqtb.style_fonts[style][fam];
                let csid = self.eqtb.font_cs.get(fid as usize).copied().unwrap_or(0);
                self.push_tokens(vec![Token::from_cs(csid)]);
            }
            _ => match self.eqtb.resolve(id).cloned() {
                Some(Equiv::CountReg(i)) => {
                    self.exp_string(self.eqtb.count[i as usize].to_string().as_bytes());
                }
                Some(Equiv::CharDef(c)) => {
                    self.exp_string((c as i32).to_string().as_bytes());
                }
                Some(Equiv::MathCharDef(c)) => {
                    self.exp_string((c as i32).to_string().as_bytes());
                }
                Some(Equiv::DimenReg(i)) => {
                    let s = self.scaled_to_string(self.eqtb.dimen[i as usize]);
                    self.exp_string(s.as_bytes());
                }
                Some(Equiv::SkipReg(i)) => {
                    let g = self.eqtb.skip[i as usize].clone();
                    let s = self.glue_to_string(&g);
                    self.exp_string(s.as_bytes());
                }
                Some(Equiv::MuSkipReg(i)) => {
                    let g = self.eqtb.muskip[i as usize].clone();
                    let s = self.glue_to_string(&g);
                    self.exp_string(s.as_bytes());
                }
                Some(Equiv::ToksReg(i)) => {
                    let toks = (*self.eqtb.toks[i as usize]).clone();
                    self.push_the_toks(toks);
                }
                Some(Equiv::FontRef(f)) => {
                    // \the\font -> the cs name of the font
                    let csid = self.eqtb.font_cs.get(f as usize).copied().unwrap_or(0);
                    self.push_tokens(vec![Token::from_cs(csid)]);
                }
                _ => {
                    eprintln!("THESCAN fail tok={:#x} cs={} prim={:?}", t.0, if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned() } else { String::new() }, self.cur_prim);
                    self.error("You can't use `\\the' after that");
                }
            },
        }
    }

    fn push_mark_tokens(&mut self, which: usize) {
        let toks = self.marks[which].first().cloned().unwrap_or_default();
        self.push_tokens(toks);
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
                return match self.active_cs_lookup(c as u8) {
                    Some(id) if self.eqtb.resolve(id).is_some() => {
                        self.meaning_of(Token::from_cs(id))
                    }
                    _ => "undefined".to_string(),
                };
            }
            let word = match cat {
                0 => "escape character",
                1 => "begin-group character",
                2 => "end-group character",
                3 => "math shift character",
                4 => "alignment tab character",
                5 => "end-of-line character",
                6 => "macro parameter character",
                7 => "superscript character",
                8 => "subscript character",
                9 => "ignored character",
                10 => "blank space ",
                11 => "the letter ",
                12 => "the character ",
                14 => "comment character",
                15 => "invalid character",
                _ => "character",
            };
            let ch: String = if cat == 10 { String::new() } else { ((c as u8) as char).to_string() };
            return format!("{}{}", word, ch);
        }
        let name = String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned();
        match self.eqtb.resolve(t.cs_id()).cloned() {
            None => "undefined".to_string(),
            Some(Equiv::CharTok(v)) => format!("the character {}", (Token(v).chr() as u8) as char),
            Some(Equiv::Macro(m)) => {
                let mut s = String::new();
                if m.protected {
                    s.push_str("\\protected");
                }
                if m.long {
                    if !s.is_empty() { s.push(' '); }
                    s.push_str("\\long");
                }
                if m.outer {
                    if !s.is_empty() { s.push(' '); }
                    s.push_str("\\outer");
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
            Some(Equiv::Prim(p)) => format!("\\{}", self.prim_name(p)),
            Some(Equiv::CharDef(c)) => format!("\\char\"{:X}", c),
            Some(Equiv::MathCharDef(c)) => format!("\\mathchar\"{:X}", c),
            Some(Equiv::FontRef(f)) => format!("select font {}", self.font_display_name(f)),
            Some(Equiv::CountReg(i)) => format!("\\count{}", i),
            Some(Equiv::DimenReg(i)) => format!("\\dimen{}", i),
            Some(Equiv::SkipReg(i)) => format!("\\skip{}", i),
            Some(Equiv::MuSkipReg(i)) => format!("\\muskip{}", i),
            Some(Equiv::ToksReg(i)) => format!("\\toks{}", i),
            Some(Equiv::BoxReg(i)) => format!("\\box{}", i),
            Some(Equiv::Alias(_)) => format!("\\{}", name),
        }
    }

    pub fn prim_name(&self, p: Prim) -> String {
        // reverse lookup: find first cs whose meaning is this prim
        for id in self.cs.all_ids() {
            if matches!(self.eqtb.get(id), Some(Equiv::Prim(q)) if *q == p) {
                return String::from_utf8_lossy(self.cs.name(id)).into_owned();
            }
        }
        "unknown".into()
    }

    pub fn cur_quad(&self) -> i32 {
        self.eqtb.fonts.get(self.eqtb.cur_font_val as usize).map(|f| f.quad()).unwrap_or(0)
    }
    pub fn cur_x_height(&self) -> i32 {
        self.eqtb.fonts.get(self.eqtb.cur_font_val as usize).map(|f| f.x_height()).unwrap_or(0)
    }

    // ---------- formatting ----------

    /// TeX's print_scaled: value in sp as decimal pt string
    pub fn scaled_to_string(&self, v: i32) -> String {
        let neg = v < 0;
        let mut n = (v as i64).abs();
        let mut int = n / ONE as i64;
        let mut n = n % ONE as i64;
        let mut digits = [0u8; 5];
        for i in 0..5 {
            n *= 10;
            digits[i] = (n / ONE as i64) as u8;
            n %= ONE as i64;
        }
        if n * 2 >= ONE as i64 {
            // round half up on the last digit, propagate the carry
            let mut i = 4;
            loop {
                if digits[i] == 9 {
                    digits[i] = 0;
                    if i == 0 {
                        int += 1;
                        break;
                    }
                    i -= 1;
                } else {
                    digits[i] += 1;
                    break;
                }
            }
        }
        let mut s = String::new();
        if neg {
            s.push('-');
        }
        s.push_str(&int.to_string());
        // strip trailing zeros but keep at least one fractional digit
        let mut len = 5;
        while len > 1 && digits[len - 1] == 0 {
            len -= 1;
        }
        s.push('.');
        for i in 0..len {
            s.push((b'0' + digits[i]) as char);
        }
        s.push_str("pt");
        s
    }
    pub fn scaled_number_to_string(&self, v: i32) -> String {
        let mut s = self.scaled_to_string(v);
        if s.ends_with("pt") {
            s.truncate(s.len() - 2);
        }
        s
    }

    pub fn glue_to_string(&self, g: &Glue) -> String {
        let mut s = self.scaled_to_string(g.width);
        if g.stretch != 0 || g.stretch_order > 0 {
            s.push_str(" plus ");
            if g.stretch_order == 0 {
                s.push_str(&self.scaled_to_string(g.stretch));
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
                s.push_str(&self.scaled_to_string(g.shrink));
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

    // ---------- font id scan ----------

    pub fn scan_font_id(&mut self) -> u16 {
        self.skip_spaces_relax();
        let t = self.get_token();
        if !t.is_cs() {
            { let __pt = t; if crate::debug_flag("PUSHWATCH") && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                let fam = self.scan_int();
                if !(0..=15).contains(&fam) {
                    self.error("Bad font family");
                    return 0;
                }
                let slot = match p {
                    Prim::TextFont => 0,
                    Prim::ScriptFont => 1,
                    _ => 2,
                };
                self.eqtb.style_fonts[slot][fam as usize]
            }
            _ => {
                self.error("Not a font identifier");
                0
            }
        }
    }

    // ---------- expressions (e-TeX) ----------

    pub fn scan_expr_num(&mut self) -> i32 {
        let v = self.expr_eval(ExprKind::Int, false);
        match v {
            ExprVal::Int(x) => x,
            ExprVal::Dim(x) => x / ONE,
            ExprVal::Glue(g) => g.width / ONE,
        }
    }

    pub fn scan_expr_dim(&mut self) -> i32 {
        let v = self.expr_eval(ExprKind::Dim, false);
        match v {
            ExprVal::Int(x) => mult(x, ONE),
            ExprVal::Dim(x) => x,
            ExprVal::Glue(g) => g.width,
        }
    }

    pub fn scan_expr_glue(&mut self, mu: bool) -> Glue {
        let v = self.expr_eval(ExprKind::Glue, mu);
        match v {
            ExprVal::Int(x) => Glue::new(mult(x, ONE)),
            ExprVal::Dim(x) => Glue::new(x),
            ExprVal::Glue(g) => g,
        }
    }

    fn expr_eval(&mut self, kind: ExprKind, mu: bool) -> ExprVal {
        let mut left = self.expr_term(kind, mu);
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
                self.pushed.push(t);
                return left;
            };
            let right = self.expr_term(kind, mu);
            left = match (left, right) {
                (ExprVal::Int(a), ExprVal::Int(b)) => ExprVal::Int(if op == 1 { a.wrapping_add(b) } else { a.wrapping_sub(b) }),
                (ExprVal::Dim(a), ExprVal::Dim(b)) => ExprVal::Dim(if op == 1 { a.wrapping_add(b) } else { a.wrapping_sub(b) }),
                (ExprVal::Dim(a), ExprVal::Int(b)) => {
                    ExprVal::Dim(if op == 1 { a + mult(b, ONE) } else { a - mult(b, ONE) })
                }
                (ExprVal::Int(a), ExprVal::Dim(b)) => {
                    ExprVal::Dim(if op == 1 { mult(a, ONE) + b } else { mult(a, ONE) - b })
                }
                (ExprVal::Glue(a), ExprVal::Glue(b)) => {
                    ExprVal::Glue(if op == 1 { glue_add(&a, &b) } else { glue_sub(&a, &b) })
                }
                (ExprVal::Glue(a), ExprVal::Dim(b)) => {
                    let g = Glue::new(b);
                    ExprVal::Glue(if op == 1 { glue_add(&a, &g) } else { glue_sub(&a, &g) })
                }
                (ExprVal::Glue(a), ExprVal::Int(b)) => {
                    let g = Glue::new(mult(b, ONE));
                    ExprVal::Glue(if op == 1 { glue_add(&a, &g) } else { glue_sub(&a, &g) })
                }
                (l, r) => {
                    let _ = r;
                    l
                }
            };
        }
    }

    fn expr_term(&mut self, kind: ExprKind, mu: bool) -> ExprVal {
        let mut left = self.expr_factor(kind, mu);
        loop {
            let t = self.expr_next_token();
            let op = if t.is_char() && t.chr() == b'*' as u32 {
                1
            } else if t.is_char() && t.chr() == b'/' as u32 {
                2
            } else {
                self.pushed.push(t);
                return left;
            };
            let right = self.expr_factor(ExprKind::Int, false);
            let b = match right {
                ExprVal::Int(x) => x,
                _ => 0,
            };
            left = match left {
                ExprVal::Int(a) => ExprVal::Int(match op {
                    1 => a.saturating_mul(b),
                    _ => {
                        if b == 0 {
                            0
                        } else {
                            crate::scaled::x_over_y(a, b)
                        }
                    }
                }),
                ExprVal::Dim(a) => ExprVal::Dim(match op {
                    1 => mult(a, b),
                    _ => {
                        if b == 0 {
                            0
                        } else {
                            crate::scaled::x_over_y(a, b)
                        }
                    }
                }),
                ExprVal::Glue(a) => ExprVal::Glue(match op {
                    1 => Glue { width: mult(a.width, b), stretch: mult(a.stretch, b), shrink: mult(a.shrink, b), ..a },
                    _ => Glue {
                        width: if b == 0 { 0 } else { crate::scaled::x_over_y(a.width, b) },
                        stretch: if b == 0 { 0 } else { crate::scaled::x_over_y(a.stretch, b) },
                        shrink: if b == 0 { 0 } else { crate::scaled::x_over_y(a.shrink, b) },
                        ..a
                    },
                }),
            };
        }
    }

    fn expr_factor(&mut self, kind: ExprKind, mu: bool) -> ExprVal {
        let t = self.expr_next_token();
        if t.is_char() && t.chr() == b'(' as u32 {
            let v = self.expr_eval(kind, mu);
            let t = self.expr_next_token();
            if !(t.is_char() && t.chr() == b')' as u32) {
                self.pushed.push(t);
                self.error("Missing ) in expression");
            }
            return v;
        }
        if t.is_char() && t.chr() == b'-' as u32 {
            let v = self.expr_factor(kind, mu);
            return match v {
                ExprVal::Int(x) => ExprVal::Int(-x),
                ExprVal::Dim(x) => ExprVal::Dim(-x),
                ExprVal::Glue(g) => ExprVal::Glue(Glue { width: -g.width, stretch: -g.stretch, shrink: -g.shrink, ..g }),
            };
        }
        self.pushed.push(t);
        match kind {
            ExprKind::Int => ExprVal::Int(self.scan_int()),
            ExprKind::Dim => ExprVal::Dim(self.scan_dimen(false, false)),
            ExprKind::Glue => ExprVal::Glue(self.scan_glue(mu)),
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

fn glue_add(a: &Glue, b: &Glue) -> Glue {
    let mut g = a.clone();
    if g.stretch_order == b.stretch_order {
        g.stretch += b.stretch;
    } else if g.stretch_order < b.stretch_order {
        g.stretch = b.stretch;
        g.stretch_order = b.stretch_order;
    }
    if g.shrink_order == b.shrink_order {
        g.shrink += b.shrink;
    } else if g.shrink_order < b.shrink_order {
        g.shrink = b.shrink;
        g.shrink_order = b.shrink_order;
    }
    g.width += b.width;
    g
}

fn glue_sub(a: &Glue, b: &Glue) -> Glue {
    let mut g = a.clone();
    if g.stretch_order == b.stretch_order {
        g.stretch -= b.stretch;
    } else if g.stretch_order < b.stretch_order {
        g.stretch = -b.stretch;
        g.stretch_order = b.stretch_order;
    }
    if g.shrink_order == b.shrink_order {
        g.shrink -= b.shrink;
    } else if g.shrink_order < b.shrink_order {
        g.shrink = -b.shrink;
        g.shrink_order = b.shrink_order;
    }
    g.width -= b.width;
    g
}
