//! Lua patterns: a port of the matcher in lstrlib.c.
//!
//! The matcher works on bytes and interprets the pattern directly. Like C
//! Lua it reports malformed patterns lazily, when matching reaches the bad
//! element, and bounds recursion with `MAXCCALLS` ("pattern too complex").

use crate::lua_vm::lua_limits::{LUA_MAXCAPTURES, MAXCCALLS_PATTERN};

const L_ESC: u8 = b'%';

/// Characters that make a pattern non-plain (`nospecials`).
const SPECIALS: &[u8] = b"^$*+?.([%-";

/// Error message of a malformed pattern or capture misuse.
pub(crate) type PatternError = String;

#[derive(Clone, Copy)]
enum CapLen {
    Unfinished,
    Position,
    Len(usize),
}

#[derive(Clone, Copy)]
struct Capture {
    init: usize,
    len: CapLen,
}

/// A capture value: a substring of the subject or a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cap {
    /// Byte range `start..end` of the subject.
    Str(usize, usize),
    /// 1-based position (from `()`).
    Pos(usize),
}

pub(crate) struct MatchState<'a> {
    pub src: &'a [u8],
    pat: &'a [u8],
    matchdepth: usize,
    level: usize,
    capture: [Capture; LUA_MAXCAPTURES],
}

/// `nospecials`: true if the pattern has no magic characters.
pub(crate) fn is_plain(pat: &[u8]) -> bool {
    !pat.iter().any(|b| SPECIALS.contains(b))
}

/// `lmemfind`: first occurrence of `needle` in `hay`.
pub(crate) fn find_plain(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let Some((&first, rest)) = needle.split_first() else {
        return Some(0);
    };
    if needle.len() > hay.len() {
        return None;
    }
    let last_start = hay.len() - needle.len();
    let mut i = 0;
    while i <= last_start {
        let offset = hay[i..=last_start].iter().position(|&b| b == first)?;
        let start = i + offset;
        if &hay[start + 1..start + needle.len()] == rest {
            return Some(start);
        }
        i = start + 1;
    }
    None
}

/// C-locale `<ctype.h>` class of `c` for the class letter `cl`; letters
/// that are not classes match themselves.
#[inline]
fn match_class(c: u8, cl: u8) -> bool {
    let res = match cl.to_ascii_lowercase() {
        b'a' => c.is_ascii_alphabetic(),
        b'c' => c.is_ascii_control(),
        b'd' => c.is_ascii_digit(),
        b'g' => c.is_ascii_graphic(),
        b'l' => c.is_ascii_lowercase(),
        b'p' => c.is_ascii_punctuation(),
        b's' => matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'),
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        b'z' => c == 0,
        _ => return cl == c,
    };
    if cl.is_ascii_uppercase() { !res } else { res }
}

/// `matchbracketclass`: `pat[p]` is '[' and `pat[ec]` its closing ']'.
fn match_bracket_class(c: u8, pat: &[u8], mut p: usize, ec: usize) -> bool {
    let mut sig = true;
    if pat[p + 1] == b'^' {
        sig = false;
        p += 1;
    }
    p += 1;
    while p < ec {
        if pat[p] == L_ESC {
            p += 1;
            if match_class(c, pat[p]) {
                return sig;
            }
        } else if pat[p + 1] == b'-' && p + 2 < ec {
            if pat[p] <= c && c <= pat[p + 2] {
                return sig;
            }
            p += 2;
        } else if pat[p] == c {
            return sig;
        }
        p += 1;
    }
    !sig
}

impl<'a> MatchState<'a> {
    pub(crate) fn new(src: &'a [u8], pat: &'a [u8]) -> Self {
        MatchState {
            src,
            pat,
            matchdepth: MAXCCALLS_PATTERN,
            level: 0,
            capture: [Capture { init: 0, len: CapLen::Unfinished }; LUA_MAXCAPTURES],
        }
    }

    /// `reprepstate`: forget captures before a new match attempt.
    #[inline]
    pub(crate) fn reprep(&mut self) {
        self.level = 0;
        debug_assert_eq!(self.matchdepth, MAXCCALLS_PATTERN);
    }

    /// Number of values a match produces (`push_captures`): the captures, or
    /// the whole match when `whole` and there are none.
    pub(crate) fn capture_count(&self, whole: bool) -> usize {
        if self.level == 0 && whole { 1 } else { self.level }
    }

    /// `get_onecapture`: capture `i` of the match `s..e`.
    pub(crate) fn get_capture(&self, i: usize, s: usize, e: usize) -> Result<Cap, PatternError> {
        if i >= self.level {
            if i != 0 {
                return Err(format!("invalid capture index %{}", i + 1));
            }
            return Ok(Cap::Str(s, e));
        }
        let capture = self.capture[i];
        match capture.len {
            CapLen::Unfinished => Err("unfinished capture".to_string()),
            CapLen::Position => Ok(Cap::Pos(capture.init + 1)),
            CapLen::Len(len) => Ok(Cap::Str(capture.init, capture.init + len)),
        }
    }

    /// `match`: try to match `pat[p..]` at `src[s..]`; the end of the match.
    pub(crate) fn do_match(&mut self, s: usize, p: usize) -> Result<Option<usize>, PatternError> {
        if self.matchdepth == 0 {
            return Err("pattern too complex".to_string());
        }
        self.matchdepth -= 1;
        let result = self.match_body(s, p);
        self.matchdepth += 1;
        result
    }

    fn match_body(&mut self, mut s: usize, mut p: usize) -> Result<Option<usize>, PatternError> {
        let pat = self.pat;
        loop {
            if p == pat.len() {
                return Ok(Some(s));
            }
            match pat[p] {
                b'(' => {
                    return if pat.get(p + 1) == Some(&b')') {
                        self.start_capture(s, p + 2, CapLen::Position)
                    } else {
                        self.start_capture(s, p + 1, CapLen::Unfinished)
                    };
                }
                b')' => return self.end_capture(s, p + 1),
                b'$' if p + 1 == pat.len() => {
                    return Ok((s == self.src.len()).then_some(s));
                }
                L_ESC => match pat.get(p + 1) {
                    Some(b'b') => {
                        match self.match_balance(s, p + 2)? {
                            Some(end) => {
                                s = end;
                                p += 4;
                                continue;
                            }
                            None => return Ok(None),
                        }
                    }
                    Some(b'f') => {
                        p += 2;
                        if pat.get(p) != Some(&b'[') {
                            return Err("missing '[' after '%f' in pattern".to_string());
                        }
                        let ep = self.class_end(p)?;
                        let previous = if s == 0 { 0 } else { self.src[s - 1] };
                        let current = self.src.get(s).copied().unwrap_or(0);
                        if !match_bracket_class(previous, pat, p, ep - 1)
                            && match_bracket_class(current, pat, p, ep - 1)
                        {
                            p = ep;
                            continue;
                        }
                        return Ok(None);
                    }
                    Some(&digit) if digit.is_ascii_digit() => {
                        match self.match_capture(s, digit)? {
                            Some(end) => {
                                s = end;
                                p += 2;
                                continue;
                            }
                            None => return Ok(None),
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
            // Default: a single-character class with an optional suffix.
            let ep = self.class_end(p)?;
            let suffix = pat.get(ep).copied();
            if !self.single_match(s, p, ep) {
                if matches!(suffix, Some(b'*' | b'?' | b'-')) {
                    p = ep + 1;
                    continue;
                }
                return Ok(None);
            }
            match suffix {
                Some(b'?') => {
                    if let Some(end) = self.do_match(s + 1, ep + 1)? {
                        return Ok(Some(end));
                    }
                    p = ep + 1;
                }
                Some(b'+') => return self.max_expand(s + 1, p, ep),
                Some(b'*') => return self.max_expand(s, p, ep),
                Some(b'-') => return self.min_expand(s, p, ep),
                _ => {
                    s += 1;
                    p = ep;
                }
            }
        }
    }

    /// `classEnd`: index just past the single-character class at `p`.
    fn class_end(&self, p: usize) -> Result<usize, PatternError> {
        let pat = self.pat;
        let mut p = p + 1;
        match pat[p - 1] {
            L_ESC => {
                if p >= pat.len() {
                    return Err("malformed pattern (ends with '%')".to_string());
                }
                Ok(p + 1)
            }
            b'[' => {
                if pat.get(p) == Some(&b'^') {
                    p += 1;
                }
                loop {
                    if p >= pat.len() {
                        return Err("malformed pattern (missing ']')".to_string());
                    }
                    let c = pat[p];
                    p += 1;
                    if c == L_ESC && p < pat.len() {
                        p += 1;
                    }
                    if pat.get(p) == Some(&b']') {
                        return Ok(p + 1);
                    }
                }
            }
            _ => Ok(p),
        }
    }

    #[inline]
    fn single_match(&self, s: usize, p: usize, ep: usize) -> bool {
        let Some(&c) = self.src.get(s) else {
            return false;
        };
        match self.pat[p] {
            b'.' => true,
            L_ESC => match_class(c, self.pat[p + 1]),
            b'[' => match_bracket_class(c, self.pat, p, ep - 1),
            literal => literal == c,
        }
    }

    fn match_balance(&self, s: usize, p: usize) -> Result<Option<usize>, PatternError> {
        if p + 1 >= self.pat.len() {
            return Err("malformed pattern (missing arguments to '%b')".to_string());
        }
        let (open, close) = (self.pat[p], self.pat[p + 1]);
        if self.src.get(s) != Some(&open) {
            return Ok(None);
        }
        let mut depth = 1;
        for (i, &c) in self.src.iter().enumerate().skip(s + 1) {
            if c == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(Some(i + 1));
                }
            } else if c == open {
                depth += 1;
            }
        }
        Ok(None)
    }

    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> Result<Option<usize>, PatternError> {
        let mut i = 0;
        while self.single_match(s + i, p, ep) {
            i += 1;
        }
        loop {
            if let Some(end) = self.do_match(s + i, ep + 1)? {
                return Ok(Some(end));
            }
            if i == 0 {
                return Ok(None);
            }
            i -= 1;
        }
    }

    fn min_expand(&mut self, mut s: usize, p: usize, ep: usize) -> Result<Option<usize>, PatternError> {
        loop {
            if let Some(end) = self.do_match(s, ep + 1)? {
                return Ok(Some(end));
            }
            if self.single_match(s, p, ep) {
                s += 1;
            } else {
                return Ok(None);
            }
        }
    }

    fn start_capture(&mut self, s: usize, p: usize, what: CapLen) -> Result<Option<usize>, PatternError> {
        let level = self.level;
        if level >= LUA_MAXCAPTURES {
            return Err("too many captures".to_string());
        }
        self.capture[level] = Capture { init: s, len: what };
        self.level = level + 1;
        let result = self.do_match(s, p)?;
        if result.is_none() {
            self.level -= 1;
        }
        Ok(result)
    }

    fn end_capture(&mut self, s: usize, p: usize) -> Result<Option<usize>, PatternError> {
        let Some(l) = (0..self.level)
            .rev()
            .find(|&l| matches!(self.capture[l].len, CapLen::Unfinished))
        else {
            return Err("invalid pattern capture".to_string());
        };
        self.capture[l].len = CapLen::Len(s - self.capture[l].init);
        let result = self.do_match(s, p)?;
        if result.is_none() {
            self.capture[l].len = CapLen::Unfinished;
        }
        Ok(result)
    }

    /// Back reference `%1`..`%9`.
    fn match_capture(&self, s: usize, digit: u8) -> Result<Option<usize>, PatternError> {
        let l = usize::from(digit).wrapping_sub(usize::from(b'1'));
        let capture = (l < self.level).then(|| self.capture[l]);
        let len = match capture.map(|c| c.len) {
            Some(CapLen::Len(len)) => len,
            // A position capture has no text: it never matches.
            Some(CapLen::Position) => return Ok(None),
            _ => return Err(format!("invalid capture index %{}", l.wrapping_add(1) as isize)),
        };
        let init = capture.map_or(0, |c| c.init);
        let matches = self.src.len() - s >= len && self.src[init..init + len] == self.src[s..s + len];
        Ok(matches.then_some(s + len))
    }
}

/// A literal byte that every match at a position must start with, letting
/// unanchored searches skip ahead (`None` when the first element can match
/// emptily or is not a plain byte).
pub(crate) fn first_literal(pat: &[u8]) -> Option<u8> {
    let &first = pat.first()?;
    if SPECIALS.contains(&first) || matches!(first, b')' | b'[') {
        return None;
    }
    match pat.get(1) {
        Some(b'*' | b'?' | b'-') => None,
        _ => Some(first),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(src: &str, pat: &str) -> Result<Option<(usize, usize)>, PatternError> {
        let mut ms = MatchState::new(src.as_bytes(), pat.as_bytes());
        for s in 0..=src.len() {
            ms.reprep();
            if let Some(e) = ms.do_match(s, 0)? {
                return Ok(Some((s, e)));
            }
        }
        Ok(None)
    }

    #[test]
    fn malformed_patterns_are_reported_when_reached() {
        assert_eq!(find("abc", "x["), Ok(None));
        assert_eq!(find("xabc", "x[").unwrap_err(), "malformed pattern (missing ']')");
        assert_eq!(find("a", "[%").unwrap_err(), "malformed pattern (missing ']')");
        assert_eq!(find("a", "%").unwrap_err(), "malformed pattern (ends with '%')");
        assert_eq!(find("a", "a)").unwrap_err(), "invalid pattern capture");
        assert_eq!(find("a", "%f").unwrap_err(), "missing '[' after '%f' in pattern");
        assert_eq!(find("a", "%b").unwrap_err(), "malformed pattern (missing arguments to '%b')");
        assert_eq!(find("a", "%1").unwrap_err(), "invalid capture index %1");
        assert_eq!(find("a", "%0").unwrap_err(), "invalid capture index %0");
    }

    #[test]
    fn classes_follow_the_c_locale() {
        assert_eq!(find("x\x0by", "%s"), Ok(Some((1, 2))));
        assert_eq!(find("\u{e9}a", "%a"), Ok(Some((2, 3))));
        assert_eq!(find("a-b", "[a%-]+"), Ok(Some((0, 2))));
        assert_eq!(find("]", "[]]"), Ok(Some((0, 1))));
    }

    #[test]
    fn recursion_and_capture_limits() {
        let long = "a".repeat(300);
        let nested = "(".repeat(33) + &")".repeat(33);
        assert_eq!(find(&long, &nested).unwrap_err(), "too many captures");
        let complex = "a?".repeat(300);
        assert_eq!(find(&long, &complex).unwrap_err(), "pattern too complex");
    }
}
