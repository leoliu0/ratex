//! Character classes and case tables replicating bibtex.web's lex_class,
//! upper_case / lower_case, and the predefined control sequences.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LexClass {
    Illegal,
    WhiteSpace,
    SepChar,
    Numeric,
    Alpha,
    Other,
}

pub fn lex_class(c: u8) -> LexClass {
    match c {
        0..=31 => LexClass::Illegal,
        b'\t' | b' ' => LexClass::WhiteSpace,
        b'~' | b'-' => LexClass::SepChar,
        b'0'..=b'9' => LexClass::Numeric,
        b'A'..=b'Z' | b'a'..=b'z' => LexClass::Alpha,
        _ => LexClass::Other,
    }
}

#[inline]
pub fn is_white(c: u8) -> bool {
    c == b'\t' || c == b' '
}

#[inline]
pub fn is_alpha(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

#[inline]
pub fn is_numeric(c: u8) -> bool {
    c.is_ascii_digit()
}

#[inline]
pub fn is_sep_char(c: u8) -> bool {
    c == b'~' || c == b'-'
}

pub fn to_lower(c: u8) -> u8 {
    c.to_ascii_lowercase()
}

pub fn to_upper(c: u8) -> u8 {
    c.to_ascii_uppercase()
}

/// The predefined control sequences (bibtex.web @<Pre-define certain strings@>),
/// paired with their ilk_info codes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CtrlSeq {
    I,
    J,
    Oe,
    OeUpper,
    Ae,
    AeUpper,
    Aa,
    AaUpper,
    O,
    OUpper,
    L,
    LUpper,
    Ss,
}

/// Look up a control sequence name (the letters following a backslash).
pub fn lookup_ctrl_seq(name: &[u8]) -> Option<CtrlSeq> {
    match name {
        b"i" => Some(CtrlSeq::I),
        b"j" => Some(CtrlSeq::J),
        b"oe" => Some(CtrlSeq::Oe),
        b"OE" => Some(CtrlSeq::OeUpper),
        b"ae" => Some(CtrlSeq::Ae),
        b"AE" => Some(CtrlSeq::AeUpper),
        b"aa" => Some(CtrlSeq::Aa),
        b"AA" => Some(CtrlSeq::AaUpper),
        b"o" => Some(CtrlSeq::O),
        b"O" => Some(CtrlSeq::OUpper),
        b"l" => Some(CtrlSeq::L),
        b"L" => Some(CtrlSeq::LUpper),
        b"ss" => Some(CtrlSeq::Ss),
        _ => None,
    }
}
