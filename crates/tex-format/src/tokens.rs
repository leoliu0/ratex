//! Safety net: TeX's input tokenizer with LaTeX's default category codes.
//!
//! The formatter only changes whitespace that TeX's reading rules make
//! irrelevant (indentation, trailing blanks, a space turned into a line end,
//! tab versus space). Every result is tokenized here together with its source
//! and the two token lists must agree, up to the changes the formatter makes on
//! purpose: runs of `\par` after a token that cannot take one as an argument
//! count as one, a `\par` may precede a sectioning command whose definition
//! is known to start with `\par` (the same list the formatter uses, see
//! `sections`), and (when aligning) spaces next to `&` are ignored. Any
//! other new or lost `\par` is a difference. Each line is read on its own
//! from state N, as TeX does, so lines kept verbatim always produce the same
//! tokens on both sides.
//!
//! Material TeX reads with other category codes is compared character by
//! character, blanks and line ends included: the arguments of verbatim
//! commands (`\verb|..|`, `\lstinline@..@`, `\url{..}` and the project's
//! own, found by the same rules as the formatter's), short-verb text and the
//! bodies of verbatim environments. `@` is a letter in packages and after
//! `\makeatletter`, so there `\Q@x@` is one control word.

use crate::format::{env_name, scan_span, Lexicon, Raw, SourceKind, Span};

const CAT_ESCAPE: u8 = 0;
const CAT_BEGIN: u8 = 1;
const CAT_END: u8 = 2;
const CAT_ALIGN: u8 = 4;
const CAT_EOL: u8 = 5;
const CAT_SUPER: u8 = 7;
const CAT_IGNORED: u8 = 9;
const CAT_SPACE: u8 = 10;
const CAT_LETTER: u8 = 11;
const CAT_OTHER: u8 = 12;
const CAT_ACTIVE: u8 = 13;
const CAT_COMMENT: u8 = 14;

fn catcode(c: u32, at_letter: bool) -> u8 {
    match c {
        0x5C => CAT_ESCAPE,
        0x7B => CAT_BEGIN,
        0x7D => CAT_END,
        0x24 => 3,
        0x26 => CAT_ALIGN,
        0x0D => CAT_EOL,
        0x23 => 6,
        0x5E => CAT_SUPER,
        0x5F => 8,
        0x00 => CAT_IGNORED,
        0x20 | 0x09 => CAT_SPACE,
        0x41..=0x5A | 0x61..=0x7A => CAT_LETTER,
        0x40 if at_letter => CAT_LETTER,
        0x7E => CAT_ACTIVE,
        0x25 => CAT_COMMENT,
        0x7F => 15,
        _ => CAT_OTHER,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tok {
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
    if c == 0x5E && i + 2 < buf.len() && buf[i + 1] == c {
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

/// Reads a file line by line. The category of `@` and verbatim material
/// that continues on the next line carry over between lines.
struct Reader<'a> {
    lex: Lexicon<'a>,
    par: Tok,
    /// Whether `@` is a letter (packages, `\makeatletter`).
    at_letter: bool,
    /// `at_letter` at each open `{`, restored at its `}`.
    saved_at: Vec<bool>,
    raw: Option<Raw>,
    out: Vec<Token>,
    /// The current line, then `\r` for its end.
    buf: Vec<u32>,
    name: Vec<u32>,
    /// `name` as text, and the rest of the line as text, for lookups.
    word: String,
    rest: String,
}

impl Reader<'_> {
    fn push(&mut self, tok: Tok, line: usize) {
        self.out.push(Token { tok, line });
    }

    /// Puts the line from `i` (before its end) into `rest`.
    fn fill_rest(&mut self, i: usize) {
        let end = self.buf.len() - 1;
        self.rest.clear();
        self.rest.extend(
            self.buf[i..end]
                .iter()
                .map(|&c| char::from_u32(c).unwrap_or('\u{fffd}')),
        );
    }

    /// Copies verbatim material from `i` as character tokens, blanks
    /// included: an environment body up to its `\end{..}` (searched from
    /// `start` bytes in), or verbatim arguments scanned from `start` as the
    /// formatter does. Returns where ordinary reading resumes, or `None`
    /// when the material continues on the next line.
    fn read_raw(&mut self, i: usize, start: usize, mut raw: Raw, line: usize) -> Option<usize> {
        self.fill_rest(i);
        let found = match &mut raw {
            Raw::Env(close) => self.rest[start..].find(&**close).map(|p| start + p),
            Raw::Span(span) => scan_span(self.rest.as_bytes(), start, span),
        };
        let end = self.buf.len() - 1;
        let taken = match found {
            Some(e) => self.rest.as_bytes()[..e]
                .iter()
                .filter(|&&b| b & 0xC0 != 0x80)
                .count(),
            None => end - i,
        };
        for k in i..i + taken {
            self.push(Tok::Char(CAT_OTHER, self.buf[k]), line);
        }
        if found.is_none() {
            self.push(Tok::Char(CAT_OTHER, 0x0D), line);
            self.raw = Some(raw);
        }
        found.map(|_| i + taken)
    }

    /// After the control word in `name`, ending at `i`: tracks `@` and reads
    /// verbatim material that follows. Returns where reading resumes, or
    /// `None` when the line is used up.
    fn after_word(&mut self, i: usize, line: usize) -> Option<usize> {
        self.word.clear();
        self.word.extend(
            self.name
                .iter()
                .map(|&c| char::from_u32(c).unwrap_or('\u{fffd}')),
        );
        match self.word.as_str() {
            "makeatletter" => self.at_letter = true,
            "makeatother" => self.at_letter = false,
            "begin" => {
                self.fill_rest(i);
                let close = match env_name(&self.rest, 0) {
                    Some((env, after)) if self.lex.is_verbatim_env(env) => {
                        Some((format!("\\end{{{env}}}"), after))
                    }
                    _ => None,
                };
                if let Some((close, after)) = close {
                    return self.read_raw(i, after, Raw::Env(close.into()), line);
                }
            }
            word => {
                if let Some(spec) = self.lex.verbatim_command(word) {
                    let star = matches!(word, "verb" | "Verb" | "spverb")
                        && self.buf.get(i) == Some(&u32::from(b'*'));
                    let span = Span {
                        spec,
                        done: 0,
                        depth: 0,
                    };
                    return self.read_raw(i, usize::from(star), Raw::Span(span), line);
                }
            }
        }
        Some(i)
    }

    /// Reads one source line the way TeX does and appends its tokens.
    fn tokenize_line(&mut self, line: &str, line_no: usize) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        self.buf.clear();
        self.buf.extend(line.chars().map(u32::from));
        while self.buf.last() == Some(&0x20) {
            self.buf.pop();
        }
        self.buf.push(0x0D);
        #[derive(PartialEq)]
        enum State {
            N,
            M,
            S,
        }
        let mut state = State::N;
        let mut i = 0;
        if let Some(raw) = self.raw.take() {
            match self.read_raw(0, 0, raw, line_no) {
                Some(e) => {
                    i = e;
                    state = State::M;
                }
                None => return,
            }
        }
        let space = Tok::Char(CAT_SPACE, 0x20);
        while i < self.buf.len() {
            reduce(&mut self.buf, i);
            let c = self.buf[i];
            i += 1;
            let cat = catcode(c, self.at_letter);
            match cat {
                CAT_ESCAPE => {
                    if i >= self.buf.len() {
                        break;
                    }
                    reduce(&mut self.buf, i);
                    let first = self.buf[i];
                    self.name.clear();
                    self.name.push(first);
                    i += 1;
                    let word = catcode(first, self.at_letter) == CAT_LETTER;
                    if word {
                        while i < self.buf.len() {
                            reduce(&mut self.buf, i);
                            if catcode(self.buf[i], self.at_letter) != CAT_LETTER {
                                break;
                            }
                            self.name.push(self.buf[i]);
                            i += 1;
                        }
                        state = State::S;
                    } else {
                        state = if catcode(first, self.at_letter) == CAT_SPACE {
                            State::S
                        } else {
                            State::M
                        };
                    }
                    self.push(Tok::Cs(hash_name(&self.name)), line_no);
                    if word {
                        match self.after_word(i, line_no) {
                            Some(e) => {
                                if e > i {
                                    state = State::M;
                                }
                                i = e;
                            }
                            None => return,
                        }
                    }
                }
                CAT_EOL => {
                    match state {
                        State::N => self.push(self.par, line_no),
                        State::M => self.push(space, line_no),
                        State::S => {}
                    }
                    break;
                }
                CAT_SPACE => {
                    if state == State::M {
                        self.push(space, line_no);
                        state = State::S;
                    }
                }
                CAT_COMMENT => break,
                CAT_IGNORED => {}
                _ => {
                    self.push(Tok::Char(cat, c), line_no);
                    state = State::M;
                    match cat {
                        CAT_BEGIN => self.saved_at.push(self.at_letter),
                        CAT_END => {
                            if let Some(at) = self.saved_at.pop() {
                                self.at_letter = at;
                            }
                        }
                        _ => {}
                    }
                    // Short-verb text runs to the same character or the
                    // end of the line, as the formatter reads it.
                    if u8::try_from(c).is_ok_and(|b| self.lex.short_verb(b)) {
                        let end = self.buf.len() - 1;
                        let close = self.buf[i..end]
                            .iter()
                            .position(|&x| x == c)
                            .map_or(end, |p| i + p + 1);
                        for k in i..close {
                            self.push(Tok::Char(CAT_OTHER, self.buf[k]), line_no);
                        }
                        i = close;
                    }
                }
            }
        }
    }
}

fn tokenize(text: &str, lex: Lexicon, kind: SourceKind) -> Vec<Token> {
    let mut reader = Reader {
        lex,
        par: cs("par"),
        at_letter: kind == SourceKind::Package,
        saved_at: Vec::new(),
        raw: None,
        out: Vec::with_capacity(text.len() / 3),
        buf: Vec::new(),
        name: Vec::new(),
        word: String::new(),
        rest: String::new(),
    };
    if text.is_empty() {
        return reader.out;
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    for (index, line) in body.split('\n').enumerate() {
        reader.tokenize_line(line, index + 1);
    }
    reader.out
}

/// Which deliberate differences the comparison accepts.
pub(crate) struct Allowances {
    /// A `\par` may come before these sectioning commands (those known to
    /// start with `\par` themselves, see `sections`).
    pub par_sections: Vec<Tok>,
    pub spaces_around_ampersands: bool,
}

/// The control-sequence tokens of the sectioning commands in `mask`.
pub(crate) fn section_tokens(mask: u8) -> Vec<Tok> {
    crate::sections::names(mask).map(cs).collect()
}

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
        if run_safe
            && allow.par_sections.contains(&token.tok)
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
    lex: Lexicon,
    kind: SourceKind,
) -> Option<(usize, usize)> {
    let a = normalize(tokenize(before, lex, kind), allow);
    let b = normalize(tokenize(after, lex, kind), allow);
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
    use crate::config::Config;
    use crate::format::Extras;

    fn none() -> Allowances {
        Allowances {
            par_sections: Vec::new(),
            spaces_around_ampersands: false,
        }
    }

    fn differs_with(
        a: &str,
        b: &str,
        allow: &Allowances,
        extras: &Extras,
        kind: SourceKind,
    ) -> bool {
        let cfg = Config::default();
        let lex = Lexicon { cfg: &cfg, extras };
        first_difference(a, b, allow, lex, kind).is_some()
    }

    fn same(a: &str, b: &str) -> bool {
        !differs_with(a, b, &none(), &Extras::default(), SourceKind::Document)
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
            par_sections: vec![cs("section")],
            spaces_around_ampersands: false,
        };
        let differs = |a: &str, b: &str| {
            differs_with(a, b, &sections, &Extras::default(), SourceKind::Document)
        };
        assert!(!differs("a\n\\section{x}\n", "a\n\n\\section{x}\n"));
        assert!(differs(
            "\\fbox\n\\section{x}\n",
            "\\fbox\n\n\\section{x}\n"
        ));
        // Only before the commands known to start with `\par`.
        assert!(differs(
            "a\n\\subsubsection{x}\n",
            "a\n\n\\subsubsection{x}\n"
        ));
        assert!(!same("a\n\\section{x}\n", "a\n\n\\section{x}\n"));
    }

    #[test]
    fn caret_notation_is_decoded() {
        assert!(same("^^41\n", "A\n"));
        assert!(same("\\^^41BC x\n", "\\ABC x\n"));
    }

    #[test]
    fn verbatim_material_keeps_its_blanks() {
        // Arguments of verbatim commands, any delimiter.
        assert!(!same("\\verb|a b|\n", "\\verb|a\nb|\n"));
        assert!(!same("\\lstinline@a b@\n", "\\lstinline@a  b@\n"));
        assert!(!same("\\url{a\n b}\n", "\\url{a\n    b}\n"));
        // Mentioned, not used: `{\url}` reads nothing.
        assert!(same("{\\url} a b\n", "{\\url} a\nb\n"));
        // Verbatim environment bodies, indentation included.
        let env = "\\begin{verbatim}\n x\n\\end{verbatim} a b\n";
        assert!(!same(env, "\\begin{verbatim}\n   x\n\\end{verbatim} a b\n"));
        assert!(same(env, "\\begin{verbatim}\n x\n\\end{verbatim} a\nb\n"));
        // Short-verb characters.
        let mut extras = Extras::default();
        extras.scan("\\MakeShortVerb{\\|}\n\\newcommand{\\Q}{\\lstinline}");
        let differs = |a: &str, b: &str, kind| differs_with(a, b, &none(), &extras, kind);
        assert!(differs("|a b|\n", "|a\nb|\n", SourceKind::Document));
        // A project's alias of `\lstinline` read with `@` delimiters.
        let doc = SourceKind::Document;
        assert!(differs("\\Q@a b@\n", "\\Q@a\n  b@\n", doc));
        // With `@` a letter, `\Q@a` is a different control word.
        assert!(!differs("\\Q@a b@\n", "\\Q@a\n  b@\n", SourceKind::Package));
        assert!(!differs(
            "\\makeatletter\n\\Q@a b@\n",
            "\\makeatletter\n\\Q@a\n  b@\n",
            doc
        ));
        assert!(differs(
            "{\\makeatletter}\n\\Q@a b@\n",
            "{\\makeatletter}\n\\Q@a\n  b@\n",
            doc
        ));
    }
}
