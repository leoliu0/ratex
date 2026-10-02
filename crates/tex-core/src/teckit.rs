//! TECkit text mappings (`mapping=NAME` font option, XeTeX `apply_mapping`).
//!
//! TeX Live's `tex-text.map` (compiled to `tex-text.tec`) is the mapping
//! every LaTeX document uses; its rules are reproduced here. Other mappings
//! are looked up as `NAME.tec` by the font loader and reported as XeTeX does
//! when they cannot be loaded.

/// A Unicode-to-Unicode TECkit mapping: longest match wins, everything else
/// passes through (`pass(Unicode)`).
#[derive(Debug, PartialEq, Eq)]
pub struct TextMapping {
    pub name: String,
    /// `(source, replacement)` UTF-16 sequences, longest source first.
    rules: Vec<(Vec<u16>, Vec<u16>)>,
}

impl TextMapping {
    fn new(name: &str, rules: &[(&str, &str)]) -> TextMapping {
        let mut rules: Vec<(Vec<u16>, Vec<u16>)> = rules
            .iter()
            .map(|(a, b)| (a.encode_utf16().collect(), b.encode_utf16().collect()))
            .collect();
        rules.sort_by_key(|(a, _)| std::cmp::Reverse(a.len()));
        TextMapping { name: name.to_string(), rules }
    }

    /// `tex-text.map` of TeX Live (`fonts/misc/xetex/fontmapping/base`).
    pub fn tex_text() -> TextMapping {
        TextMapping::new(
            "tex-text",
            &[
                ("--", "\u{2013}"),
                ("---", "\u{2014}"),
                ("'", "\u{2019}"),
                ("''", "\u{201D}"),
                ("\"", "\u{201D}"),
                ("`", "\u{2018}"),
                ("``", "\u{201C}"),
                ("!`", "\u{00A1}"),
                ("?`", "\u{00BF}"),
                (",,", "\u{201E}"),
                ("<<", "\u{00AB}"),
                (">>", "\u{00BB}"),
            ],
        )
    }

    /// The mapping `name` (without `.tec`) if it is built in.
    pub fn builtin(name: &str) -> Option<TextMapping> {
        match name {
            "tex-text" => Some(TextMapping::tex_text()),
            _ => None,
        }
    }

    /// `apply_mapping`: convert a UTF-16 buffer.
    pub fn apply(&self, text: &[u16]) -> Vec<u16> {
        let mut out = Vec::with_capacity(text.len());
        let mut i = 0;
        'outer: while i < text.len() {
            for (src, dst) in &self.rules {
                if text[i..].starts_with(src) {
                    out.extend_from_slice(dst);
                    i += src.len();
                    continue 'outer;
                }
            }
            out.push(text[i]);
            i += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tex_text_ligatures() {
        let m = TextMapping::tex_text();
        let u = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        assert_eq!(m.apply(&u("a--b---c")), u("a\u{2013}b\u{2014}c"));
        assert_eq!(m.apply(&u("``q'' \"x\" it's !` ?` ,,")), u("\u{201C}q\u{201D} \u{201D}x\u{201D} it\u{2019}s \u{00A1} \u{00BF} \u{201E}"));
        assert_eq!(m.apply(&u("----")), u("\u{2014}-"));
    }
}
