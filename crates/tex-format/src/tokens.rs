//! Safety net: TeX's input tokenizer with LaTeX's default category codes.
//!
//! The formatter only changes whitespace that TeX's reading rules make
//! irrelevant (indentation, trailing blanks, a space turned into a line end,
//! tab versus space). Every result is tokenized here together with its source
//! and the two token lists must agree, up to the changes the formatter makes on
//! purpose: runs of `\par` count as one, a `\par` may precede a sectioning
//! command, and (when aligning) spaces next to `&` are ignored. Each line is
//! read on its own from state N, as TeX does, so lines kept verbatim always
//! produce the same tokens on both sides.

const CAT_ESCAPE: u8 = 0;
const CAT_ALIGN: u8 = 4;
const CAT_EOL: u8 = 5;
const CAT_SUPER: u8 = 7;
const CAT_IGNORED: u8 = 9;
const CAT_SPACE: u8 = 10;
const CAT_LETTER: u8 = 11;
const CAT_OTHER: u8 = 12;
const CAT_ACTIVE: u8 = 13;
const CAT_COMMENT: u8 = 14;

fn catcode(c: u32) -> u8 {
    match c {
        0x5C => CAT_ESCAPE,
        0x7B => 1,
        0x7D => 2,
        0x24 => 3,
        0x26 => CAT_ALIGN,
        0x0D => CAT_EOL,
        0x23 => 6,
        0x5E => CAT_SUPER,
        0x5F => 8,
        0x00 => CAT_IGNORED,
        0x20 | 0x09 => CAT_SPACE,
        0x41..=0x5A | 0x61..=0x7A => CAT_LETTER,
        0x7E => CAT_ACTIVE,
        0x25 => CAT_COMMENT,
        0x7F => 15,
        _ => CAT_OTHER,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tok {
    /// A control sequence, identified by a 64-bit hash of its name.
    Cs(u64),
    /// A character token: category code and character code.
    Char(u8, u32),
}

struct Token {
    tok: Tok,
    line: usize,
}

fn hash_name(name: &[u32]) -> u64 {
    // FNV-1a over the character codes.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in name {
        for b in c.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn cs(name: &str) -> Tok {
    let chars: Vec<u32> = name.chars().map(u32::from).collect();
    Tok::Cs(hash_name(&chars))
}

/// TeX's `^^` notation: returns the character at `i` and the index after it.
fn char_at(buf: &[u32], i: usize) -> (u32, usize) {
    let c = buf[i];
    if catcode(c) == CAT_SUPER && i + 2 < buf.len() && buf[i + 1] == c {
        let is_hex = |x: u32| matches!(x, 0x30..=0x39 | 0x61..=0x66);
        let hex_val = |x: u32| if x <= 0x39 { x - 0x30 } else { x - 0x61 + 10 };
        if i + 3 < buf.len() && is_hex(buf[i + 2]) && is_hex(buf[i + 3]) {
            return (hex_val(buf[i + 2]) * 16 + hex_val(buf[i + 3]), i + 4);
        }
        let d = buf[i + 2];
        if d < 128 {
            return (if d < 64 { d + 64 } else { d - 64 }, i + 3);
        }
    }
    (c, i + 1)
}

/// Replaces `^^` notation at `i` by the character it denotes, repeatedly,
/// as TeX does when it meets such a pair while reading.
fn reduce(buf: &mut Vec<u32>, i: usize) {
    loop {
        let (c, next) = char_at(buf, i);
        if next == i + 1 {
            return;
        }
        buf.splice(i..next, [c]);
    }
}

/// Reads one source line the way TeX does and appends its tokens.
fn tokenize_line(line: &str, line_no: usize, par: Tok, out: &mut Vec<Token>, buf: &mut Vec<u32>) {
    let line = line.strip_suffix('\r').unwrap_or(line);
    buf.clear();
    buf.extend(line.chars().map(u32::from));
    while buf.last() == Some(&0x20) {
        buf.pop();
    }
    buf.push(0x0D);
    #[derive(PartialEq)]
    enum State {
        N,
        M,
        S,
    }
    let mut state = State::N;
    let mut i = 0;
    let mut name = Vec::new();
    let space = Tok::Char(CAT_SPACE, 0x20);
    while i < buf.len() {
        reduce(buf, i);
        let c = buf[i];
        i += 1;
        let cat = catcode(c);
        match cat {
            CAT_ESCAPE => {
                if i >= buf.len() {
                    break;
                }
                reduce(buf, i);
                let first = buf[i];
                name.clear();
                name.push(first);
                i += 1;
                if catcode(first) == CAT_LETTER {
                    while i < buf.len() {
                        reduce(buf, i);
                        if catcode(buf[i]) != CAT_LETTER {
                            break;
                        }
                        name.push(buf[i]);
                        i += 1;
                    }
                    state = State::S;
                } else {
                    state = if catcode(first) == CAT_SPACE {
                        State::S
                    } else {
                        State::M
                    };
                }
                out.push(Token {
                    tok: Tok::Cs(hash_name(&name)),
                    line: line_no,
                });
            }
            CAT_EOL => {
                match state {
                    State::N => out.push(Token {
                        tok: par,
                        line: line_no,
                    }),
                    State::M => out.push(Token {
                        tok: space,
                        line: line_no,
                    }),
                    State::S => {}
                }
                break;
            }
            CAT_SPACE => {
                if state == State::M {
                    out.push(Token {
                        tok: space,
                        line: line_no,
                    });
                    state = State::S;
                }
            }
            CAT_COMMENT => break,
            CAT_IGNORED => {}
            _ => {
                out.push(Token {
                    tok: Tok::Char(cat, c),
                    line: line_no,
                });
                state = State::M;
            }
        }
    }
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut out = Vec::with_capacity(text.len() / 3);
    if text.is_empty() {
        return out;
    }
    let mut buf = Vec::new();
    let par = cs("par");
    let body = text.strip_suffix('\n').unwrap_or(text);
    for (index, line) in body.split('\n').enumerate() {
        tokenize_line(line, index + 1, par, &mut out, &mut buf);
    }
    out
}

/// Which deliberate differences the comparison accepts.
pub(crate) struct Allowances {
    pub par_before_sections: bool,
    pub spaces_around_ampersands: bool,
}

const SECTIONS: &[&str] = &["part", "chapter", "section", "subsection", "subsubsection"];

/// Whether a `\par` after this token cannot be taken as a macro argument: a
/// character that is not active (`~`, babel's `"`), a `$`, `&` or `}`.
/// After a control sequence the `\par` may be its argument (`\fbox` followed
/// by blank lines takes the first `\par`), so the number of `\par` matters.
fn ends_safely(tok: Tok) -> bool {
    match tok {
        Tok::Cs(_) => false,
        Tok::Char(cat, c) => matches!(cat, 2..=4 | CAT_LETTER | CAT_OTHER) && c != 0x22,
    }
}

fn normalize(tokens: Vec<Token>, allow: &Allowances) -> Vec<Token> {
    let par = cs("par");
    let sections: Vec<Tok> = SECTIONS.iter().map(|s| cs(s)).collect();
    let space = Tok::Char(CAT_SPACE, 0x20);
    let is_amp = |t: Tok| matches!(t, Tok::Char(CAT_ALIGN, _));
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    // Whether the current run of `\par` follows a token that cannot take it
    // as an argument; only such runs may change length.
    let mut run_safe = false;
    for token in tokens {
        if token.tok == par {
            if out.last().is_some_and(|t| t.tok == par) {
                if run_safe {
                    continue;
                }
            } else {
                run_safe = out
                    .iter()
                    .rev()
                    .find(|t| t.tok != space)
                    .is_some_and(|t| ends_safely(t.tok));
            }
        }
        if allow.par_before_sections
            && sections.contains(&token.tok)
            && run_safe
            && out.last().is_some_and(|t| t.tok == par)
        {
            out.pop();
        }
        if allow.spaces_around_ampersands {
            if token.tok == space && out.last().is_some_and(|t| is_amp(t.tok)) {
                continue;
            }
            if is_amp(token.tok) {
                while out.last().is_some_and(|t| t.tok == space) {
                    out.pop();
                }
            }
        }
        out.push(token);
    }
    out
}

/// Compares the token streams of `before` and `after`. On a difference,
/// returns the line numbers (in `before` and `after`) where it starts.
pub(crate) fn first_difference(
    before: &str,
    after: &str,
    allow: &Allowances,
) -> Option<(usize, usize)> {
    let a = normalize(tokenize(before), allow);
    let b = normalize(tokenize(after), allow);
    for (x, y) in a.iter().zip(&b) {
        if x.tok != y.tok {
            return Some((x.line, y.line));
        }
    }
    match a.len().cmp(&b.len()) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Less => Some((a.last().map_or(1, |t| t.line), b[a.len()].line)),
        std::cmp::Ordering::Greater => Some((a[b.len()].line, b.last().map_or(1, |t| t.line))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Allowances {
        Allowances {
            par_before_sections: false,
            spaces_around_ampersands: false,
        }
    }

    fn same(a: &str, b: &str) -> bool {
        first_difference(a, b, &none()).is_none()
    }

    #[test]
    fn indentation_and_trailing_blanks_are_invisible() {
        assert!(same("a\n b\n", "a\n      b\n"));
        assert!(same("a  \nb\n", "a\nb\n"));
        assert!(same("a\t\nb\n", "a\nb\n"));
        assert!(same("\\foo \\item x\n", "\\foo\n\\item x\n"));
        assert!(same("a \\item x\n", "a\n  \\item x\n"));
    }

    #[test]
    fn significant_whitespace_is_detected() {
        assert!(!same("a%\nb\n", "a\nb\n"));
        assert!(!same("ab\n", "a\nb\n"));
        assert!(!same("a\n\nb\n", "a\nb\n"));
        assert!(!same("\\ x\n", "\\\nx\n"));
        // TeX drops trailing spaces itself, so these two are the same.
        assert!(same("\\ \n", "\\\n"));
        assert!(!same("x^^ y\n", "x^^\ny\n"));
    }

    #[test]
    fn par_runs_collapse_only_after_safe_tokens() {
        assert!(same("a\n\n\n\nb\n", "a\n\nb\n"));
        assert!(same("\\x{a}\n\n\n\nb\n", "\\x{a}\n\nb\n"));
        // `\fbox` takes the first \par as its argument.
        assert!(!same("\\fbox\n\n\n\nb\n", "\\fbox\n\nb\n"));
        assert!(!same("~\n\n\n\nb\n", "~\n\nb\n"));
        let sections = Allowances {
            par_before_sections: true,
            spaces_around_ampersands: false,
        };
        assert!(first_difference("a\n\\section{x}\n", "a\n\n\\section{x}\n", &sections).is_none());
        assert!(first_difference(
            "\\fbox\n\\section{x}\n",
            "\\fbox\n\n\\section{x}\n",
            &sections
        )
        .is_some());
    }

    #[test]
    fn caret_notation_is_decoded() {
        assert!(same("^^41\n", "A\n"));
        assert!(same("\\^^41BC x\n", "\\ABC x\n"));
    }
}
