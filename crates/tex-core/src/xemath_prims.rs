//! XeTeX's own math primitives and math-code values (xetex.web `XeTeX_def_code`,
//! `math_char_num` with `chr_code` 1/2, `XeTeX_math_given`, `delim_num` 1,
//! `radical` 1 and `math_accent` 1; §§9783-9866, 10298-10388, 26524-26841,
//! 27670-27990).
//!
//! A XeTeX math code is one 32-bit integer for every character:
//! `(class + 8 * fam) * 0x200000 + char`, i.e. `char = v mod 0x200000`
//! (21 bits), `class = (v div 0x200000) mod 8` and `fam = (v div 0x1000000)
//! mod 256`; `0x1FFFFF` as the character field is an active math character.
//! `\delcode` is either a tex.web 27-bit code or `0x40000000 + fam * 0x200000
//! + usv`. The scanners below raise xetex.web's own errors.

use crate::boxes::{AccentSpec, Delim};
use crate::engine::Engine;
use crate::eqtb::Equiv;
use crate::math::CL_ORD;
use crate::prim::{IntParam, Prim};

/// XeTeX math primitives without a tex.web counterpart of the same shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum XeMath {
    /// `\Umathcode`: `<usv> = <class> <fam> <usv>`
    MathCode,
    /// `\Umathcodenum`: `<usv> = <integer>`
    MathCodeNum,
    /// `\Udelcode`: `<usv> = <fam> <usv>`
    DelCode,
    /// `\Udelcodenum`: `<usv> = <integer>`
    DelCodeNum,
    /// `\Umathchardef`: `\cs = <class> <fam> <usv>`
    MathCharDef,
    /// `\Umathcharnumdef`: `\cs = <integer>`
    MathCharNumDef,
    /// `\Umathchar` (`math_char_num`, chr 2)
    MathChar,
    /// `\Umathcharnum` (`math_char_num`, chr 1)
    MathCharNum,
    /// `\Udelimiter` (`delim_num`, chr 1)
    Delimiter,
    /// `\Uradical` (`radical`, chr 1)
    Radical,
    /// `\Umathaccent` (`math_accent`, chr 1)
    MathAccent,
}

impl XeMath {
    pub const ALL: [XeMath; 11] = [
        XeMath::MathCode,
        XeMath::MathCodeNum,
        XeMath::DelCode,
        XeMath::DelCodeNum,
        XeMath::MathCharDef,
        XeMath::MathCharNumDef,
        XeMath::MathChar,
        XeMath::MathCharNum,
        XeMath::Delimiter,
        XeMath::Radical,
        XeMath::MathAccent,
    ];

    pub fn idx(self) -> u16 {
        self as u16
    }

    pub fn from_idx(i: u16) -> Option<XeMath> {
        Self::ALL.get(usize::from(i)).copied()
    }

    /// `\Umathcode`, `\Umathcodenum`, `\Udelcode` and `\Udelcodenum` can be
    /// read as internal quantities (xetex.web §9783).
    pub fn is_value(self) -> bool {
        matches!(self, XeMath::MathCode | XeMath::MathCodeNum | XeMath::DelCode | XeMath::DelCodeNum)
    }
}

/// `(print_cmd_chr name, XeTeX* alias, primitive)`; both names are defined and
/// `\meaning` shows the first.
pub static XEMATH_NAMES: &[(&[u8], &[u8], XeMath)] = &[
    (b"Umathcode", b"XeTeXmathcode", XeMath::MathCode),
    (b"Umathcodenum", b"XeTeXmathcodenum", XeMath::MathCodeNum),
    (b"Udelcode", b"XeTeXdelcode", XeMath::DelCode),
    (b"Udelcodenum", b"XeTeXdelcodenum", XeMath::DelCodeNum),
    (b"Umathchardef", b"XeTeXmathchardef", XeMath::MathCharDef),
    (b"Umathcharnumdef", b"XeTeXmathcharnumdef", XeMath::MathCharNumDef),
    (b"Umathchar", b"XeTeXmathchar", XeMath::MathChar),
    (b"Umathcharnum", b"XeTeXmathcharnum", XeMath::MathCharNum),
    (b"Udelimiter", b"XeTeXdelimiter", XeMath::Delimiter),
    (b"Uradical", b"XeTeXradical", XeMath::Radical),
    (b"Umathaccent", b"XeTeXmathaccent", XeMath::MathAccent),
];


pub const ACTIVE_MATH_CHAR: i64 = 0x1F_FFFF;
const BIGGEST_USV: i64 = 0x10_FFFF;
/// xetex.web `@"40000000`: an extended delimiter code
pub const EXTENDED_DELCODE: i32 = 0x4000_0000;

// The fields of a math code are bit fields of the (up to 32-bit) value; the
// probes of `xetex -ini` treat a negative code like its two's complement.

/// xetex.web `math_class_field`.
pub fn class_field(v: i64) -> i64 {
    (v >> 21) & 7
}

/// xetex.web `math_fam_field`.
pub fn fam_field(v: i64) -> i64 {
    (v >> 24) & 0xFF
}

/// xetex.web `math_char_field`.
pub fn char_field(v: i64) -> i64 {
    v & 0x1F_FFFF
}

/// `set_class_field(class) + set_family_field(fam) + usv`
pub fn pack(class: i32, fam: i32, usv: i32) -> i64 {
    i64::from(class) * 0x20_0000 + i64::from(fam) * 0x100_0000 + i64::from(usv)
}

/// A tex.web mathchar `"CFXX` in the XeTeX layout (§26546, §26551).
pub fn legacy_to_packed(v: i32) -> i64 {
    pack(v / 0x1000, (v % 0x1000) / 0x100, v % 0x100)
}

impl Engine {
    /// xetex.web `scan_usv_num`.
    pub(crate) fn scan_xe_usv(&mut self) -> i32 {
        let v = self.scan_int();
        if (0..=BIGGEST_USV as i32).contains(&v) {
            v
        } else {
            self.error(&format!("Bad character code ({v})"));
            0
        }
    }

    /// xetex.web `scan_xetex_math_char_int`.
    pub(crate) fn scan_xe_math_char_int(&mut self) -> i64 {
        let n = self.scan_int();
        let mut v = i64::from(n);
        if char_field(v) == ACTIVE_MATH_CHAR {
            if v != ACTIVE_MATH_CHAR {
                self.error(&format!("Bad active XeTeX math code ({n})"));
                v = ACTIVE_MATH_CHAR;
            }
        } else if char_field(v) > BIGGEST_USV {
            self.error(&format!("Bad XeTeX math character code ({n})"));
            v = 0;
        }
        v
    }

    /// xetex.web `scan_math_class_int`.
    pub(crate) fn scan_xe_math_class(&mut self) -> i32 {
        let v = self.scan_int();
        if (0..=7).contains(&v) {
            v
        } else {
            self.error(&format!("Bad math class ({v})"));
            0
        }
    }

    /// xetex.web `scan_math_fam_int`.
    pub(crate) fn scan_xe_math_fam(&mut self) -> i32 {
        let v = self.scan_int();
        if (0..=255).contains(&v) {
            v
        } else {
            self.error(&format!("Bad math family ({v})"));
            0
        }
    }

    /// xetex.web `scan_fifteen_bit_int`.
    pub(crate) fn scan_xe_fifteen_bit(&mut self) -> i32 {
        let v = self.scan_int();
        if (0..=0x7FFF).contains(&v) {
            v
        } else {
            self.error(&format!("Bad mathchar ({v})"));
            0
        }
    }

    /// xetex.web `scan_delimiter_int`.
    pub(crate) fn scan_xe_delimiter_int(&mut self) -> i32 {
        let v = self.scan_int();
        if (0..=0x7FF_FFFF).contains(&v) {
            v
        } else {
            self.error(&format!("Bad delimiter code ({v})"));
            0
        }
    }

    /// `<class> <fam> <usv>` as one XeTeX math code.
    fn scan_xe_class_fam_usv(&mut self) -> i64 {
        let class = self.scan_xe_math_class();
        let fam = self.scan_xe_math_fam();
        let usv = self.scan_xe_usv();
        pack(class, fam, usv)
    }

    /// `<fam> <usv>` as an extended delimiter code.
    pub(crate) fn scan_xe_fam_usv_delcode(&mut self) -> i32 {
        let fam = self.scan_xe_math_fam();
        let usv = self.scan_xe_usv();
        EXTENDED_DELCODE + fam * 0x20_0000 + usv
    }

    /// `\the\mathcode` (xetex.web §9833): the code as a tex.web mathchar.
    pub(crate) fn xe_the_mathcode(&mut self) -> i32 {
        let c = self.scan_xe_usv();
        let mut v = self.eqtb.xe_math_code(c as u32);
        if char_field(v) == ACTIVE_MATH_CHAR {
            v = 0x8000;
        } else if fam_field(v) > 15 || char_field(v) > 255 {
            self.error(&format!("Extended mathchar used as mathchar ({v})"));
            v = 0;
        }
        (class_field(v) * 0x1000 + fam_field(v) * 0x100 + char_field(v)) as i32
    }

    /// `\the\delcode`.
    pub(crate) fn xe_the_delcode(&mut self) -> i32 {
        let c = self.scan_xe_usv();
        let v = self.eqtb.xe_del_code(c as u32);
        if v >= EXTENDED_DELCODE {
            self.error("Extended delcode used as delcode");
            0
        } else {
            v
        }
    }

    /// The value of `\Umathcode`, `\Umathcodenum`, `\Udelcode` or
    /// `\Udelcodenum` (xetex.web §9783); `None` for the other primitives.
    pub(crate) fn xemath_internal(&mut self, x: XeMath) -> Option<i64> {
        match x {
            XeMath::MathCodeNum => {
                let c = self.scan_xe_usv();
                Some(self.eqtb.xe_math_code(c as u32))
            }
            XeMath::DelCodeNum => {
                let c = self.scan_xe_usv();
                Some(i64::from(self.eqtb.xe_del_code(c as u32)))
            }
            XeMath::MathCode => {
                self.scan_xe_usv();
                self.error("Can't use \\Umathcode as a number (try \\Umathcodenum)");
                Some(0)
            }
            XeMath::DelCode => {
                self.scan_xe_usv();
                self.error("Can't use \\Udelcode as a number (try \\Udelcodenum)");
                Some(0)
            }
            _ => None,
        }
    }

    /// `\mathcode` / `\delcode` assignment (xetex.web `def_code`, §27964):
    /// both take an integer and convert a tex.web mathchar to the XeTeX layout.
    pub(crate) fn xe_def_code_command(&mut self, math: bool) {
        let name = if math { "\\mathcode" } else { "\\delcode" };
        let g = self.take_assignment_prefixes(name);
        let c = self.scan_xe_usv();
        self.scan_optional_equals();
        let mut v = self.scan_int();
        let max: i32 = if math { 0x8000 } else { 0xFF_FFFF };
        if (v < 0 && math) || v > max {
            if math {
                self.error(&format!("Invalid code ({v}), should be in the range 0..{max}"));
            } else {
                self.error(&format!("Invalid code ({v}), should be at most {max}"));
            }
            v = 0;
        }
        if math {
            let packed = if v == 0x8000 { ACTIVE_MATH_CHAR } else { legacy_to_packed(v) };
            self.eqtb.assign_xe_math_code(c as u32, packed, g);
        } else {
            self.eqtb.assign_xe_del_code(c as u32, v, g);
        }
    }

    /// The assignment primitives of [`XeMath`] (xetex.web §27723, §27923);
    /// `false` for the others.
    pub(crate) fn xemath_assign(&mut self, x: XeMath) -> bool {
        match x {
            XeMath::MathCode => {
                let g = self.take_assignment_prefixes("\\Umathcode");
                let c = self.scan_xe_usv();
                self.scan_optional_equals();
                let n = self.scan_xe_class_fam_usv();
                self.eqtb.assign_xe_math_code(c as u32, n, g);
            }
            XeMath::MathCodeNum => {
                let g = self.take_assignment_prefixes("\\Umathcodenum");
                let c = self.scan_xe_usv();
                self.scan_optional_equals();
                let n = self.scan_xe_math_char_int();
                self.eqtb.assign_xe_math_code(c as u32, n, g);
            }
            XeMath::DelCode => {
                let g = self.take_assignment_prefixes("\\Udelcode");
                let c = self.scan_xe_usv();
                self.scan_optional_equals();
                let n = self.scan_xe_fam_usv_delcode();
                self.eqtb.assign_xe_del_code(c as u32, n, g);
            }
            XeMath::DelCodeNum => {
                let g = self.take_assignment_prefixes("\\Udelcodenum");
                let c = self.scan_xe_usv();
                self.scan_optional_equals();
                let n = self.scan_int();
                self.eqtb.assign_xe_del_code(c as u32, n, g);
            }
            XeMath::MathCharDef | XeMath::MathCharNumDef => {
                let t = self.scan_definable_cs();
                let g = self.take_global();
                // xetex.web §27724: the target is made \relax before the value is scanned
                self.eqtb.assign(t, Equiv::Prim(Prim::Relax), g);
                self.scan_optional_equals();
                let n = if x == XeMath::MathCharDef {
                    self.scan_xe_class_fam_usv()
                } else {
                    self.scan_xe_math_char_int()
                };
                self.eqtb.assign(t, Equiv::UMathCharDef(n as i32), g);
                self.clear_prefixes();
            }
            _ => return false,
        }
        true
    }

    /// `\Umathchar`, `\Umathcharnum`, `\Udelimiter`, `\Uradical` and
    /// `\Umathaccent` in math mode (the dispatcher has inserted a `$` outside
    /// it, xetex.web `non_math`).
    pub(crate) fn xemath_command(&mut self, x: XeMath) {
        let source = self.current_token_source_mark();
        match x {
            XeMath::MathChar | XeMath::Delimiter => {
                let n = self.scan_xe_class_fam_usv();
                self.xe_set_math_char_at(n, 2, source);
            }
            XeMath::MathCharNum => {
                let n = self.scan_xe_math_char_int();
                // `\Umathcharnum` has `chr_code` 1: the character of an active
                // math code (§26580)
                self.xe_set_math_char_at(n, 1, source);
            }
            XeMath::Radical => {
                // xetex.web `scan_delimiter(p, true)` with `cur_chr=1`
                let code = self.scan_xe_fam_usv_delcode();
                self.do_radical_at(Delim::from_xetex_code(code), source);
            }
            XeMath::MathAccent => {
                let subtype = self.scan_xe_accent_keywords();
                let n = self.scan_xe_class_fam_usv();
                self.do_xe_math_accent(n, subtype, source);
            }
            _ => {}
        }
    }

    /// xetex.web §26816: `[fixed | bottom [fixed]]` as the accent noad subtype
    /// (`fixed_acc` 1, `bottom_acc` 2).
    fn scan_xe_accent_keywords(&mut self) -> u8 {
        if self.scan_keyword(b"fixed") {
            1
        } else if self.scan_keyword(b"bottom") {
            if self.scan_keyword(b"fixed") { 3 } else { 2 }
        } else {
            0
        }
    }

    /// xetex.web `fam_in_range` for `\fam` (0..255 in XeTeX).
    fn xe_cur_fam_in_range(&self) -> Option<u8> {
        let cur = self.eqtb.int_params[IntParam::CurFam.idx() as usize];
        (0..256).contains(&cur).then_some(cur as u8)
    }

    /// xetex.web `set_math_char(c)` for the character token `cur_chr` (§26645).
    #[inline(never)]
    pub(crate) fn xe_set_math_char_at(
        &mut self,
        c: i64,
        cur_chr: u32,
        source: Option<crate::input::SourceMark>,
    ) {
        if char_field(c) == ACTIVE_MATH_CHAR {
            self.active_char(cur_chr);
            return;
        }
        let origin = self.math_diagnostic_origin_at(source);
        self.xe_append_math_char(c, origin);
    }

    /// The noad of `set_math_char` for a code that is not active: a variable
    /// family (class 7) takes `\fam` and is an ord.
    pub(crate) fn xe_append_math_char(&mut self, c: i64, origin: crate::boxes::MathDiagnosticOrigin) {
        let ch = char_field(c) as u32;
        let class = class_field(c);
        let mut fam = fam_field(c) as u8;
        let class = if class == 7 {
            if let Some(cur) = self.xe_cur_fam_in_range() {
                fam = cur;
            }
            CL_ORD
        } else {
            class as u8
        };
        self.append_mlist_node(crate::boxes::Node::MathChar {
            fam,
            c: ch,
            class,
            origin,
            attr: self.eqtb.cur_attr,
        });
    }

    /// A letter or other character in math mode: `set_math_char(math_code(c))`.
    #[inline(never)]
    pub(crate) fn xe_math_char_token(&mut self, c: u32) {
        let source = self.current_token_source_mark();
        let code = self.eqtb.xe_math_code(c);
        self.xe_set_math_char_at(code, c, source);
    }

    /// xetex.web `math_ac` after the accent code is scanned: the accent
    /// character of class/fam/usv `n`; a variable family takes `\fam`.
    pub(crate) fn do_xe_math_accent(&mut self, n: i64, subtype: u8, source: Option<crate::input::SourceMark>) {
        let origin = self.math_diagnostic_origin_at(source);
        let mut fam = fam_field(n) as u8;
        if class_field(n) == 7 {
            if let Some(cur) = self.xe_cur_fam_in_range() {
                fam = cur;
            }
        }
        let spec = AccentSpec {
            top: Some((fam, char_field(n) as u32)),
            subtype,
            ..AccentSpec::default()
        };
        self.append_accent_noad(spec, origin);
    }
}

/// xetex.web §27712 `print_cmd_chr(XeTeX_math_given)`: the class, family and
/// character in unpadded hexadecimal (without the escape character).
pub fn umathchardef_meaning(v: i32) -> String {
    format!(
        "Umathchar\"{:X}\"{:X}\"{:X}",
        (v >> 21) & 7,
        (v >> 24) & 0xFF,
        v & 0x1F_FFFF
    )
}

/// xetex.web `scan_math` (§26524) turns a character-valued token into a
/// `math_char` field: the class of its math code is dropped (only family and
/// character stay). `field` is the list the token produced.
pub fn xe_char_field(field: &mut crate::boxes::NodeList) {
    if let [crate::boxes::Node::MathChar { fam, class, .. }] = field.as_mut_slice() {
        if *fam != crate::boxes::NO_FAM {
            *class = CL_ORD;
        }
    }
}

impl Engine {
    /// Whether `t` is one of the tokens xetex.web's `scan_math` reads as a
    /// character: a letter, other character, `\chardef`, `\char`,
    /// `\mathchar`, `\mathchardef`, `\delimiter` or a XeTeX math char form.
    pub(crate) fn xe_scan_math_char_token(&self, t: crate::token::Token) -> bool {
        if t.is_char() {
            return matches!(t.cc(), 11 | 12);
        }
        if !t.is_cs() {
            return false;
        }
        match self.eqtb.resolve(t.cs_id()) {
            Some(Equiv::CharDef(_) | Equiv::MathCharDef(_) | Equiv::UMathCharDef(_)) => true,
            Some(Equiv::CharTok(v)) => matches!(crate::token::Token(*v).cc(), 11 | 12),
            Some(Equiv::Prim(p)) => matches!(
                p,
                Prim::Char
                    | Prim::MathChar
                    | Prim::Delimiter
                    | Prim::XeMath(XeMath::MathChar | XeMath::MathCharNum | XeMath::Delimiter)
            ),
            _ => false,
        }
    }
}
