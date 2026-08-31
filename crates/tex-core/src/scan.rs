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
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            return;
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
        t.is_cs() && self.cur_prim == Some(Prim::Relax)
    }

    /// scan optional `=` with spaces/relax skipped
    pub fn scan_optional_equals(&mut self) {
        self.skip_spaces_relax();
        let t = self.get_token();
        if !(t.is_char() && t.chr() == b'=' as u32) {
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                break;
            }
            let c = t.chr();
            let d = match () {
                _ if (b'0' as u32..=b'9' as u32).contains(&c) => c - b'0' as u32,
                _ if allow_letters && (b'a' as u32..=b'f' as u32).contains(&c) => c - b'a' as u32 + 10,
                _ if allow_letters && (b'A' as u32..=b'F' as u32).contains(&c) => c - b'A' as u32 + 10,
                _ => {
                    { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
                    break;
                }
            };
            if d >= radix {
                { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                loop {
                    // expanding fetch without space skip (tex.web get_x_token):
                    // expandables continue the number, a space terminates it
                    let t2 = self.get_x_raw();
                    if t2.is_space() {
                        break;
                    }
                    if Self::is_digit_token(t2) {
                        v = v * 10 + (t2.chr() - b'0' as u32) as i64;
                        if v > 0x7FFF_FFFF {
                            v = 0x7FFF_FFFF;
                        }
                    } else {
                        { let __pt = t2; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                self.scan_optional_space();
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
                        // shouldn't reach (expandable), but just in case
                        v = self.scan_expr_num() as i64;
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
                            _ => {}
                        }
                    }
                }
            }
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
                let nm = if t.is_cs() { String::from_utf8_lossy(self.cs.name(t.cs_id())).into_owned() } else { format!("cc{}", t.cc()) };
                let ek = match self.eqtb.resolve(t.cs_id()) { Some(e) => e.kind_name(), None => "U" };
                eprintln!("MISSNUM tok={} kind={} prim={:?} srcs={:?}", nm, ek, self.cur_prim, self.input.stack.iter().rev().take(2).map(|src| match src { crate::input::Source::TokList{name,pos,toks,..} => format!("T:{} {}/{}",name,pos,toks.len()), crate::input::Source::File{name,line_no,..} => format!("F:{}",line_no)}).collect::<Vec<_>>());
            }
            self.error("Missing number, treated as zero");
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

    pub fn scan_reg_num(&mut self) -> u16 {
        let n = self.scan_int();
        if !(0..=255).contains(&n) {
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
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                        { let __pt = t2; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                if Self::is_digit_token(t2) && scale > 1e-7 {
                    frac += (t2.chr() - b'0' as u32) as f64 * scale;
                    scale *= 0.1;
                } else {
                    { let __pt = t2; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                Some(Prim::DimP(p)) => {
                    factor = 1.0;
                    direct = Some(self.eqtb.dim_params[p.idx() as usize]);
                }
                Some(Prim::FontDimen) => {
                    let idx = self.scan_int();
                    let f = self.scan_font_id();
                    let i = if idx > 0 { idx as usize - 1 } else { 0 };
                    factor = 1.0;
                    direct = Some(self.eqtb.font_params[f as usize].get(i).copied().unwrap_or(0));
                }
                _ => match self.eqtb.resolve(t.cs_id()).cloned() {
                    Some(Equiv::DimenReg(i)) => {
                        factor = 1.0;
                        direct = Some(self.eqtb.dimen[i as usize]);
                    }
                    Some(Equiv::CountReg(i)) => {
                        factor = self.eqtb.count[i as usize] as f64;
                        direct = None;
                    }
                    Some(Equiv::CharDef(c)) => {
                        factor = c as f64;
                        direct = None;
                    }
                    Some(Equiv::Prim(Prim::IntP(p))) => {
                        factor = self.int_param_value(p) as f64;
                        direct = None;
                    }
                    _ => {
                        { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }

                        self.error("Missing number, treated as zero");
                        factor = 0.0;
                        direct = None;
                    }
                },
            }
        } else {
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }

            self.error("Missing number, treated as zero");
            factor = 0.0;
            direct = None;
        }
        if let Some(d) = direct {
            return if negate { -d } else { d };
        }
        // unit
        self.skip_spaces_relax();
        let t = self.get_token();
        let mut unit_sp: i64;
        if t.is_cs() {
            match self.cur_prim {
                Some(Prim::DimP(p)) => {
                    unit_sp = self.eqtb.dim_params[p.idx() as usize] as i64;
                }
                _ => match self.eqtb.resolve(t.cs_id()).cloned() {
                    Some(Equiv::DimenReg(i)) => unit_sp = self.eqtb.dimen[i as usize] as i64,
                    _ => {
                        { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                        { let __pt = t2; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                    { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
                    if std::env::var("DEFTRACE").map(|v|v=="1").unwrap_or(false) {
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
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            self.error("Illegal unit of measure (pt inserted).");
            unit_sp = ONE as i64;
        }
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
    pub fn scan_glue(&mut self, mu: bool) -> Glue {
        let prev = self.in_expanded_scan;
        self.in_expanded_scan = false;
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
                let c = t.chr() as u8;
                if c == kw[0] {
                    let mut kt: Vec<Token> = Vec::new();
                    let mut all = true;
                    for &k in &kw[1..] {
                        let tx = self.get_token();
                        kt.push(tx);
                        if !(tx.is_char() && tx.chr() as u8 == k) {
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
                { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
        { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
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
            return Vec::new();
        }
        self.scan_balanced_raw()
    }

    /// like scan_general_text but expanding (\edef semantics)
    pub fn scan_general_text_expanded(&mut self) -> Vec<Token> {
        self.skip_spaces_relax();
        if std::env::var("IFTRACE").map(|v| v == "1").unwrap_or(false) {
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
        self.in_expanded_scan = true;
        let mut out = Vec::new();
        let mut depth = 1i32;
        loop {
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                self.error("Missing } in expanded text");
                self.in_expanded_scan = prev_expanded_scan;
                return out;
            }
            if self.cur_prim == Some(Prim::UnExpanded) {
                self.skip_spaces_relax();
                let nxt = self.raw_token();
                if nxt.is_cs() {
                    if let Some(Equiv::ToksReg(i)) = self.eqtb.resolve(nxt.cs_id()).cloned() {
                        let toks = (*self.eqtb.toks[i as usize]).clone();
                        out.extend(toks);
                        continue;
                    }
                }
                self.pushed.push(nxt);
                let toks = self.scan_general_text();
                out.extend(toks);
                continue;
            }
            if t.is_char() {
                if t.cc() == 1 {
                    depth += 1;
                } else if t.cc() == 2 {
                    depth -= 1;
                    if depth == 0 {
                        self.in_expanded_scan = prev_expanded_scan;
                        return out;
                    }
                } else if t.cc() == 6 && t.chr() == 0x23 {
                    // tex.web scan_toks: ## collapses to a single # in the
                    // collected text (macro-def doubling inside \edef bodies)
                    if let Some(last) = out.last() {
                        if last.is_char() && last.cc() == 6 && last.chr() == 0x23 {
                            continue; // drop this duplicate #: ## -> #
                        }
                    }
                }
            }
            out.push(t);
            if out.len() % 500 == 0 {
                eprintln!("SGET-BIG n={} line={} stack_len={} srcs={:?} macros={:?}", out.len(), self.input.current_file_line(), self.input.stack.len(), self.input.stack.iter().rev().take(3).map(|src| match src { crate::input::Source::TokList{name,pos,toks,..} => format!("T:{} {}/{}",name,pos,toks.len()), crate::input::Source::File{name,line_no,..} => format!("F:{}#{}", name.split('/').last().unwrap_or(name), line_no)}).collect::<Vec<_>>(), self.last_macros);
                if out.len() > 2500 {
                    eprintln!(
                        "SGET-DUMP head=[{}] tail=[{}]",
                        self.tokens_to_string(&out[..60.min(out.len())]),
                        self.tokens_to_string(&out[out.len().saturating_sub(40)..])
                    );
                    self.error("SGET-BIG abort");
                    self.in_expanded_scan = prev_expanded_scan;
                    return out;
                }
            }
        }
    }

    // ---------- \the ----------

    /// implement \the: scans an internal quantity and pushes its expansion
    pub fn the_scan(&mut self) {
        self.skip_spaces_relax();
        let t = self.get_token();
        if !t.is_cs() {
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            self.error("You can't use `\\the' after ");
            return;
        }
        let id = t.cs_id();
        // register aliases (countdef'd/dimendef'd/skipdef'd/toksdef'd cs) and
        // toks registers are valid 	he operands (tex.web scan_toks part)
        match self.eqtb.resolve(id).cloned() {
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
                self.push_tokens(Self::freeze_unexpanded_toks(toks));
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
                let s = self.scaled_to_string(self.eqtb.dim_params[p.idx() as usize]);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::GlueP(p)) => {
                let g = self.eqtb.glue_params[p.idx() as usize].clone();
                let s = self.glue_to_string(&g);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::ToksP(p)) => {
                let toks = (*self.eqtb.tok_params[p.idx() as usize]).clone();
                self.push_tokens(Self::freeze_unexpanded_toks(toks));
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
                let v = self.eqtb.font_params[f as usize].get(i).copied().unwrap_or(0);
                let s = self.scaled_to_string(v);
                self.exp_string(s.as_bytes());
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
            Some(Prim::GlueExpr) | Some(Prim::MuExpr) => {
                let g = self.scan_expr_glue();
                let s = self.glue_to_string(&g);
                self.exp_string(s.as_bytes());
            }
            Some(Prim::PdfLastXPos) => {
                self.exp_string(self.pdf_last_x.to_string().as_bytes());
            }
            Some(Prim::PdfLastYPos) => {
                self.exp_string(self.pdf_last_y.to_string().as_bytes());
            }
            Some(Prim::PdfPageAttr) => {
                let s = self.pdf_page_attr.clone();
                self.exp_string(s.as_bytes());
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
                    self.push_tokens(Self::freeze_unexpanded_toks(toks));
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
                13 => "the active character ",
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
        self.eqtb.fonts.get(self.cur_font as usize).map(|f| f.quad()).unwrap_or(0)
    }
    pub fn cur_x_height(&self) -> i32 {
        self.eqtb.fonts.get(self.cur_font as usize).map(|f| f.x_height()).unwrap_or(0)
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

    pub fn glue_to_string(&self, g: &Glue) -> String {
        let mut s = self.scaled_to_string(g.width);
        if g.stretch != 0 || g.stretch_order > 0 {
            s.push_str(" plus ");
            s.push_str(&self.scaled_to_string(g.stretch));
            match g.stretch_order {
                1 => s.push_str("fil"),
                2 => s.push_str("fill"),
                3 => s.push_str("filll"),
                _ => {}
            }
        }
        if g.shrink != 0 || g.shrink_order > 0 {
            s.push_str(" minus ");
            s.push_str(&self.scaled_to_string(g.shrink));
            match g.shrink_order {
                1 => s.push_str("fil"),
                2 => s.push_str("fill"),
                3 => s.push_str("filll"),
                _ => {}
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
            { let __pt = t; if std::env::var("PUSHWATCH").map(|w|w=="1").unwrap_or(false) && __pt.is_cs() && self.cs.name(__pt.cs_id()) == b"ifx" && self.input.current_file_line() > 9000 { eprintln!("PUSHIFX crates/tex-core/src/scan.rs:{} line={}", {line!()}, self.input.current_file_line()); } self.pushed.push(__pt); }
            self.error("Missing font identifier");
            return 0;
        }
        match self.eqtb.resolve(t.cs_id()).cloned() {
            Some(Equiv::FontRef(f)) => f,
            Some(Equiv::Prim(Prim::Font)) => {
                // \font refers to current font
                self.cur_font
            }
            _ => {
                self.error("Not a font identifier");
                0
            }
        }
    }

    // ---------- expressions (e-TeX) ----------

    pub fn scan_expr_num(&mut self) -> i32 {
        let v = self.expr_eval(ExprKind::Int);
        match v {
            ExprVal::Int(x) => x,
            ExprVal::Dim(x) => x / ONE,
            ExprVal::Glue(g) => g.width / ONE,
        }
    }

    pub fn scan_expr_dim(&mut self) -> i32 {
        let v = self.expr_eval(ExprKind::Dim);
        match v {
            ExprVal::Int(x) => mult(x, ONE),
            ExprVal::Dim(x) => x,
            ExprVal::Glue(g) => g.width,
        }
    }

    pub fn scan_expr_glue(&mut self) -> Glue {
        let v = self.expr_eval(ExprKind::Glue);
        match v {
            ExprVal::Int(x) => Glue::new(mult(x, ONE)),
            ExprVal::Dim(x) => Glue::new(x),
            ExprVal::Glue(g) => g,
        }
    }

    fn expr_eval(&mut self, kind: ExprKind) -> ExprVal {
        let mut left = self.expr_term(kind);
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
            let right = self.expr_term(kind);
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

    fn expr_term(&mut self, kind: ExprKind) -> ExprVal {
        let mut left = self.expr_factor(kind);
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
            let right = self.expr_factor(ExprKind::Int);
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

    fn expr_factor(&mut self, kind: ExprKind) -> ExprVal {
        let t = self.expr_next_token();
        if t.is_char() && t.chr() == b'(' as u32 {
            let v = self.expr_eval(kind);
            let t = self.expr_next_token();
            if !(t.is_char() && t.chr() == b')' as u32) {
                self.pushed.push(t);
                self.error("Missing ) in expression");
            }
            return v;
        }
        if t.is_char() && t.chr() == b'-' as u32 {
            let v = self.expr_factor(kind);
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
            ExprKind::Glue => ExprVal::Glue(self.scan_glue(false)),
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
