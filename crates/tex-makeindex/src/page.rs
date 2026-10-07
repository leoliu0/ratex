//! Page numbers: decoding, the integer encoding that orders them, and the
//! type-precedence rules.

use crate::style::Style;

/// The five numeral types a page number field may have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    RomanLower,
    RomanUpper,
    Arabic,
    AlphaLower,
    AlphaUpper,
}

impl Kind {
    fn from_letter(letter: u8) -> Option<Kind> {
        Some(match letter {
            b'r' => Kind::RomanLower,
            b'R' => Kind::RomanUpper,
            b'n' => Kind::Arabic,
            b'a' => Kind::AlphaLower,
            b'A' => Kind::AlphaUpper,
            _ => return None,
        })
    }

    fn index(self) -> usize {
        match self {
            Kind::RomanLower => 0,
            Kind::RomanUpper => 1,
            Kind::Arabic => 2,
            Kind::AlphaLower => 3,
            Kind::AlphaUpper => 4,
        }
    }

    /// Width of the value range reserved for the type when ordering.
    fn span(self) -> i64 {
        match self {
            Kind::AlphaLower | Kind::AlphaUpper => 26,
            _ => 10000,
        }
    }
}

/// Offsets added to each type's value. Without a style file makeindex uses a
/// fixed table; with one it lays the types out along `page_precedence`, with
/// arabic numerals anchored at zero.
#[derive(Clone, Debug)]
pub(crate) struct Bases {
    base: [i32; 5],
    allowed: [bool; 5],
}

impl Bases {
    pub(crate) fn without_style() -> Bases {
        Bases { base: [-20000, -10000, 0, 26, 10026], allowed: [true; 5] }
    }

    pub(crate) fn from_style(style: &Style) -> Bases {
        let order: Vec<Kind> =
            style.page_precedence.iter().filter_map(|&b| Kind::from_letter(b)).collect();
        let mut base = [0i32; 5];
        let mut allowed = [false; 5];
        let anchor = order.iter().position(|&kind| kind == Kind::Arabic);
        let mut running = 0i64;
        let start = anchor.unwrap_or(0);
        for &kind in &order[start..] {
            base[kind.index()] = running as i32;
            allowed[kind.index()] = true;
            running += kind.span();
        }
        let mut running = 0i64;
        for &kind in order[..start].iter().rev() {
            running -= kind.span();
            base[kind.index()] = running as i32;
            allowed[kind.index()] = true;
        }
        Bases { base, allowed }
    }
}

/// One numeral of a page number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Field {
    pub(crate) kind: Kind,
    pub(crate) value: i32,
}

/// A decoded page number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Page {
    /// The text to print (surrounding blanks removed).
    pub(crate) text: Vec<u8>,
    pub(crate) fields: Vec<Field>,
}

impl Page {
    pub(crate) fn compare(&self, other: &Page) -> std::cmp::Ordering {
        let mine = self.fields.iter().map(|field| field.value);
        let theirs = other.fields.iter().map(|field| field.value);
        mine.cmp(theirs)
    }

    /// Number of pages from `self` through `other` when they differ only in
    /// the last numeral; `None` for pages of different shapes.
    pub(crate) fn pages_until(&self, other: &Page) -> Option<i64> {
        let count = self.fields.len();
        if count != other.fields.len() || self.fields[..count - 1] != other.fields[..count - 1] {
            return None;
        }
        let (first, last) = (self.fields[count - 1], other.fields[count - 1]);
        (first.kind == last.kind)
            .then(|| i64::from(last.value) - i64::from(first.value) + 1)
    }

    /// Whether `next` directly follows `self` (same shape, last numeral +1).
    pub(crate) fn precedes_consecutively(&self, next: &Page) -> bool {
        let count = self.fields.len();
        if count != next.fields.len() {
            return false;
        }
        for index in 0..count - 1 {
            if self.fields[index] != next.fields[index] {
                return false;
            }
        }
        let (last, following) = (self.fields[count - 1], next.fields[count - 1]);
        last.kind == following.kind && last.value.wrapping_add(1) == following.value
    }
}

/// Decoding context: the style's compositor and precedence, plus the type of
/// the previous numeral, which resolves single letters that are valid in two
/// numeral systems.
pub(crate) struct PageParser<'a> {
    compositor: &'a [u8],
    precedence: &'a [u8],
    bases: Bases,
    previous: Option<Kind>,
}

const ROMAN_LOWER: &[u8] = b"ivxlcdm";
const ROMAN_UPPER: &[u8] = b"IVXLCDM";
pub(crate) const MAX_FIELDS: usize = 10;

impl<'a> PageParser<'a> {
    pub(crate) fn new(style: &'a Style, styled: bool) -> Self {
        PageParser {
            compositor: &style.page_compositor,
            precedence: &style.page_precedence,
            bases: if styled { Bases::from_style(style) } else { Bases::without_style() },
            previous: None,
        }
    }

    /// Decodes the text of a page argument; the error is makeindex's message.
    pub(crate) fn parse(&mut self, raw: &[u8]) -> Result<Page, String> {
        let start = raw.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(raw.len());
        let end = raw.iter().rposition(|&b| b != b' ' && b != b'\t').map_or(start, |i| i + 1);
        let text = &raw[start..end];
        let mut fields = Vec::new();
        let mut rest = text;
        loop {
            let cut = if self.compositor.is_empty() {
                None
            } else {
                rest.windows(self.compositor.len()).position(|w| w == self.compositor)
            };
            let (piece, tail) = match cut {
                Some(at) => (&rest[..at], Some(&rest[at + self.compositor.len()..])),
                None => (rest, None),
            };
            if fields.len() >= MAX_FIELDS {
                return Err(format!(
                    "Page number {} has too many fields (max. {MAX_FIELDS}).",
                    String::from_utf8_lossy(piece)
                ));
            }
            let field = self.field(piece, text)?;
            fields.push(field);
            match tail {
                Some(tail) => rest = tail,
                None => break,
            }
        }
        self.previous = fields.first().map(|field| field.kind);
        Ok(Page { text: text.to_vec(), fields })
    }

    fn illegal(&self, text: &[u8]) -> String {
        format!(
            "Illegal page number {} or page_precedence {}.",
            String::from_utf8_lossy(text),
            String::from_utf8_lossy(self.precedence)
        )
    }

    fn field(&mut self, piece: &[u8], whole: &[u8]) -> Result<Field, String> {
        let Some(&first) = piece.first() else {
            return Err(self.illegal(piece));
        };
        let field = if first.is_ascii_digit() {
            if !self.bases.allowed[Kind::Arabic.index()] {
                return Err(self.illegal(piece));
            }
            let mut value = 0i32;
            for (index, &byte) in piece.iter().enumerate() {
                if !byte.is_ascii_digit() {
                    return Err(format!(
                        "Illegal Arabic digit: position {} in {}.",
                        index + 1,
                        String::from_utf8_lossy(piece)
                    ));
                }
                value = value.wrapping_mul(10).wrapping_add(i32::from(byte - b'0'));
            }
            Field { kind: Kind::Arabic, value: value.wrapping_add(self.bases.base[2]) }
        } else if first.is_ascii_alphabetic() {
            let lower = first.is_ascii_lowercase();
            let (roman_kind, alpha_kind, roman_set) = if lower {
                (Kind::RomanLower, Kind::AlphaLower, ROMAN_LOWER)
            } else {
                (Kind::RomanUpper, Kind::AlphaUpper, ROMAN_UPPER)
            };
            let roman_allowed = self.bases.allowed[roman_kind.index()];
            let alpha_allowed = self.bases.allowed[alpha_kind.index()];
            let roman_first = roman_set.contains(&first);
            let use_roman = if roman_first && roman_allowed {
                if piece.len() > 1 {
                    if let Some(bad) = piece.iter().position(|byte| !roman_set.contains(byte)) {
                        return Err(format!(
                            "Illegal Roman number: position {} in {}.",
                            bad + 1,
                            String::from_utf8_lossy(piece)
                        ));
                    }
                    true
                } else if !alpha_allowed {
                    true
                } else if self.previous == Some(alpha_kind) {
                    false
                } else if self.previous == Some(roman_kind) {
                    true
                } else {
                    matches!(first.to_ascii_lowercase(), b'i' | b'v' | b'x')
                }
            } else {
                false
            };
            if use_roman {
                Field {
                    kind: roman_kind,
                    value: roman_value(piece).wrapping_add(self.bases.base[roman_kind.index()]),
                }
            } else if alpha_allowed {
                // Only the first letter counts.
                let index = i32::from(first.to_ascii_lowercase() - b'a');
                Field { kind: alpha_kind, value: index.wrapping_add(self.bases.base[alpha_kind.index()]) }
            } else {
                return Err(self.illegal(whole));
            }
        } else {
            return Err(self.illegal(whole));
        };
        Ok(field)
    }
}

fn roman_value(text: &[u8]) -> i32 {
    let digit = |byte: u8| match byte.to_ascii_lowercase() {
        b'i' => 1,
        b'v' => 5,
        b'x' => 10,
        b'l' => 50,
        b'c' => 100,
        b'd' => 500,
        _ => 1000,
    };
    let mut total = 0i32;
    for (index, &byte) in text.iter().enumerate() {
        let value = digit(byte);
        match text.get(index + 1) {
            Some(&next) if digit(next) > value => total -= value,
            _ => total += value,
        }
    }
    total
}
