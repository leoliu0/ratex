//! `unicode` (slnunicode 1.1 as built into LuaTeX): the four string
//! libraries `unicode.ascii`, `unicode.latin1`, `unicode.utf8` and
//! `unicode.grapheme`. Strings are processed by the same Lua 5.1 pattern
//! matcher as `string.*`, extended with UTF-8 characters, Unicode character
//! classes and, in grapheme mode, base characters with their combining
//! marks as units. The native part below implements the character
//! machinery; `lua_sys_unicode.lua` composes `gsub`, `gmatch` and friends.

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString};
use unicode_properties::{GeneralCategory as Gc, UnicodeGeneralCategory};

use crate::lua_sys::{bytes_of, sys_reg};

pub(crate) const PRELUDE: &str = include_str!("lua_sys_unicode.lua");

const MODE_ASCII: i64 = 0;
const MODE_UTF8: i64 = 2;
const MODE_GRAPH: i64 = 3;
const MAX_CAPTURES: usize = 32;
const CAP_UNFINISHED: i64 = -1;
const CAP_POSITION: i64 = -2;
const ESC: u8 = b'%';

/// `UnicodeData` general category of a BMP character (slnunicode only knows
/// the BMP).
fn category(c: u32) -> Option<Gc> {
    if c > 0xFFFF {
        return None;
    }
    char::from_u32(c).map(|ch| ch.general_category())
}

fn grapheme_extend(c: u32) -> bool {
    matches!(category(c), Some(Gc::NonspacingMark | Gc::EnclosingMark))
}

/// Decode one UTF-8 sequence at `p` (< `end`): the code and the next
/// position. Invalid sequences are decoded as their single first byte.
fn utf8_deco(s: &[u8], p: usize, end: usize) -> (u32, usize) {
    let first = u32::from(s[p]);
    let mut q = p + 1;
    let cont = |q: usize| q < end && s[q] & 0xC0 == 0x80;
    if first < 0xC2 || !cont(q) {
        return (first, q);
    }
    let mut code = u32::from(s[q] & 0x3F);
    q += 1;
    if first < 0xE0 {
        return (code | (first & 0x1F) << 6, q);
    }
    if cont(q) {
        code = code << 6 | u32::from(s[q] & 0x3F);
        q += 1;
        if first < 0xF0 {
            code |= (first & 0x0F) << 12;
            if code & 0xF800 != 0 {
                return (code, q);
            }
        } else if cont(q) {
            let full = (first & 0x0F) << 18 | code << 6 | u32::from(s[q] & 0x3F);
            q += 1;
            if full < 0x110100 && full > 0xFFFF {
                return (full, q);
            }
        }
    }
    (first, p + 1)
}

/// Decode backwards: the sequence ending just before `p` (> `start`).
fn utf8_oced(s: &[u8], p: usize, start: usize) -> (u32, usize) {
    let last = u32::from(s[p - 1]);
    let q = p - 1;
    if last & 0xC0 != 0x80 || q == start {
        return (last, q);
    }
    let mut code = last & 0x3F;
    let mut q = q - 1;
    if s[q] & 0xE0 == 0xC0 {
        if s[q] >= 0xC2 {
            code |= u32::from(s[q] & 0x1F) << 6;
            return (code, q);
        }
    } else if s[q] & 0xC0 == 0x80 && start < q {
        code |= u32::from(s[q] & 0x3F) << 6;
        q -= 1;
        if s[q] & 0xF0 == 0xE0 {
            code |= u32::from(s[q] & 0x0F) << 12;
            if code & 0xF800 != 0 {
                return (code, q);
            }
        } else if s[q] & 0xC0 == 0x80 && start <= q.wrapping_sub(1) && q >= 1 {
            q -= 1;
            code |= u32::from(s[q] & 0x0F) << 18 | u32::from(s[q + 1] & 0x3F) << 12;
            if code < 0x110100 && code > 0xFFFF {
                return (code, q);
            }
        }
    }
    (last, p - 1)
}

fn utf8_enco(out: &mut Vec<u8>, c: u32) {
    if c < 0x80 {
        out.push(c as u8);
        return;
    }
    if c < 0x800 {
        out.push(0xC0 | (c >> 6) as u8);
    } else {
        if c < 0x10000 {
            out.push(0xE0 | (c >> 12) as u8);
        } else {
            out.push(0xF0 | (c >> 18) as u8);
            out.push(0x80 | (0x3F & (c >> 12)) as u8);
        }
        out.push(0x80 | (0x3F & (c >> 6)) as u8);
    }
    out.push(0x80 | (0x3F & c) as u8);
}

/// Skip `Grapheme_Extend` codes starting at `p`.
fn utf8_graphext(s: &[u8], mut p: usize, end: usize) -> usize {
    while p < end {
        let (code, next) = utf8_deco(s, p, end);
        if !grapheme_extend(code) {
            break;
        }
        p = next;
    }
    p
}

/// Advance over up to `max` units (negative: all) in `s[pos..end]`;
/// returns (units counted, new position).
fn utf8_count(s: &[u8], mut pos: usize, end: usize, graph: bool, max: i64) -> (i64, usize) {
    let mut count = 0i64;
    while pos < end && count != max {
        let (code, next) = utf8_deco(s, pos, end);
        pos = next;
        count += 1;
        if graph && grapheme_extend(code) && count > 1 {
            count -= 1;
        }
    }
    if graph && count == max {
        pos = utf8_graphext(s, pos, end);
    }
    (count, pos)
}

fn is_mb(mode: i64) -> bool {
    mode >= MODE_UTF8
}

fn posrelat(pos: i64, len: i64) -> i64 {
    if pos >= 0 {
        pos
    } else {
        len + pos + 1
    }
}

fn cat_bit(c: u32, mask: impl Fn(Gc) -> bool) -> bool {
    category(c).is_some_and(mask)
}

fn is_letter(g: Gc) -> bool {
    matches!(g, Gc::UppercaseLetter | Gc::LowercaseLetter | Gc::TitlecaseLetter | Gc::ModifierLetter | Gc::OtherLetter)
}

fn is_number(g: Gc) -> bool {
    matches!(g, Gc::DecimalNumber | Gc::LetterNumber | Gc::OtherNumber)
}

fn is_punct(g: Gc) -> bool {
    matches!(
        g,
        Gc::ConnectorPunctuation
            | Gc::DashPunctuation
            | Gc::OpenPunctuation
            | Gc::ClosePunctuation
            | Gc::InitialPunctuation
            | Gc::FinalPunctuation
            | Gc::OtherPunctuation
    )
}

/// Does character `c` match the class letter `cl` (`%a`, `%D`, ...)?
fn match_class(c: u32, cl: u8, mode: i64) -> bool {
    let lower = cl | 0x20;
    let mut mode = mode;
    let res = match lower {
        b'a' => cat_bit(c, is_letter),
        b'c' => cat_bit(c, |g| g == Gc::Control),
        b'x' => {
            if (0x40..0x80).contains(&c) && (0x7e >> (c & 0x1f)) & 1 == 1 {
                return cl & 0x20 != 0;
            }
            mode = 0;
            cat_bit(c, |g| g == Gc::DecimalNumber)
        }
        b'd' => {
            mode = 0;
            cat_bit(c, |g| g == Gc::DecimalNumber)
        }
        b'l' => cat_bit(c, |g| g == Gc::LowercaseLetter),
        b'n' => cat_bit(c, is_number),
        b'p' => cat_bit(c, is_punct),
        b's' => {
            if c < 0x20 && (0x3e00u32 >> c) & 1 == 1 {
                return cl & 0x20 != 0;
            }
            cat_bit(c, |g| matches!(g, Gc::SpaceSeparator | Gc::LineSeparator | Gc::ParagraphSeparator))
        }
        b'u' => cat_bit(c, |g| g == Gc::UppercaseLetter),
        b'w' => cat_bit(c, |g| is_letter(g) || is_number(g)),
        b'z' => {
            if c == 0 {
                return cl & 0x20 != 0;
            }
            false
        }
        _ => return u32::from(cl) == c,
    };
    let res = if mode == 0 && c & 0x80 != 0 { false } else { res };
    if cl & 0x20 != 0 {
        res
    } else {
        !res
    }
}

struct Capture {
    init: usize,
    len: i64,
}

struct Matcher<'a> {
    src: &'a [u8],
    pat: &'a [u8],
    mode: i64,
    mb: bool,
    caps: Vec<Capture>,
}

type MResult<T> = Result<T, String>;

impl<'a> Matcher<'a> {
    fn pat_at(&self, p: usize) -> u8 {
        self.pat.get(p).copied().unwrap_or(0)
    }

    fn deco(&self, s: &[u8], p: usize, end: usize) -> (u32, usize) {
        if self.mb {
            utf8_deco(s, p, end)
        } else {
            (u32::from(s[p]), p + 1)
        }
    }

    fn classend(&self, mut p: usize) -> MResult<usize> {
        match self.pat_at(p) {
            ESC => {
                p += 1;
                if self.pat_at(p) == 0 {
                    return Err("malformed pattern (ends with '%')".to_string());
                }
            }
            b'[' => loop {
                if self.pat_at(p) == 0 {
                    return Err("malformed pattern (missing ']')".to_string());
                }
                let c = self.pat_at(p);
                p += 1;
                if c == ESC && self.pat_at(p) != 0 {
                    p += 1;
                }
                if self.pat_at(p) == b']' {
                    break;
                }
            },
            _ => {
                if !self.mb {
                    return Ok(p + 1);
                }
                let end = (p + 4).min(self.pat.len());
                let (_, next) = utf8_deco(self.pat, p, end);
                return Ok(next);
            }
        }
        Ok(p + 1)
    }

    /// Match one character of the subject at `s` against the pattern item
    /// `[p, ep)`; the position after it on success.
    fn singlematch(&self, s: usize, p: usize, ep: usize) -> Option<usize> {
        let end = self.src.len();
        let (c, mut next) = self.deco(self.src, s, end);
        let graph = self.mode == MODE_GRAPH;
        match self.pat_at(p) {
            ESC => {
                if match_class(c, self.pat_at(p + 1), self.mode) {
                    if graph {
                        next = utf8_graphext(self.src, next, end);
                    }
                    Some(next)
                } else {
                    None
                }
            }
            b'.' => {
                if graph {
                    next = utf8_graphext(self.src, next, end);
                }
                Some(next)
            }
            b'[' => {
                let ep = ep - 1; // the ']'
                let mut p = p + 1;
                let mut neg = false;
                if self.pat_at(p) == b'^' {
                    neg = true;
                    p += 1;
                }
                while p < ep {
                    if self.pat_at(p) == ESC {
                        p += 1;
                        if match_class(c, self.pat_at(p), self.mode) {
                            return self.bracket_hit(neg, next, graph);
                        }
                        p += 1;
                        continue;
                    }
                    let (c1, np) = self.deco(self.pat, p, ep);
                    p = np;
                    if ep <= p + 1 || self.pat_at(p) != b'-' {
                        let op = p;
                        if graph {
                            p = utf8_graphext(self.pat, p, ep);
                        }
                        if c != c1 {
                            continue;
                        }
                        if !graph {
                            return if neg { None } else { Some(next) };
                        }
                        let es = utf8_graphext(self.src, next, end);
                        if es - next == p - op && (es == next || self.src[next..es] == self.pat[op..p]) {
                            return if neg { None } else { Some(next) };
                        }
                        continue;
                    }
                    p += 1;
                    let (c2, np) = self.deco(self.pat, p, ep);
                    p = np;
                    let (c1, c2) = if c2 < c1 { (c2, c1) } else { (c1, c2) };
                    if c1 <= c && c <= c2 {
                        return self.bracket_hit(neg, next, graph);
                    }
                }
                // not matched
                if neg {
                    Some(next)
                } else {
                    None
                }
            }
            _ => {
                let (c1, _) = self.deco(self.pat, p, ep);
                if c == c1 {
                    Some(next)
                } else {
                    None
                }
            }
        }
    }

    fn bracket_hit(&self, neg: bool, mut next: usize, graph: bool) -> Option<usize> {
        if neg {
            return None;
        }
        if graph {
            next = utf8_graphext(self.src, next, self.src.len());
        }
        Some(next)
    }

    fn matchbalance(&self, mut s: usize, p: usize) -> MResult<Option<usize>> {
        if self.pat_at(p) == 0 || self.pat_at(p + 1) == 0 {
            return Err("unbalanced pattern".to_string());
        }
        if s >= self.src.len() || self.src[s] != self.pat[p] {
            return Ok(None);
        }
        let (b, e) = (self.pat[p], self.pat[p + 1]);
        let mut cont = 1;
        s += 1;
        while s < self.src.len() {
            if self.src[s] == e {
                cont -= 1;
                if cont == 0 {
                    return Ok(Some(s + 1));
                }
            } else if self.src[s] == b {
                cont += 1;
            }
            s += 1;
        }
        Ok(None)
    }

    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> MResult<Option<usize>> {
        let mut sp = s;
        while sp < self.src.len() {
            match self.singlematch(sp, p, ep) {
                Some(es) => sp = es,
                None => break,
            }
        }
        loop {
            let res = self.do_match(sp, ep + 1)?;
            if res.is_some() || sp == s {
                return Ok(res);
            }
            if !self.mb {
                sp -= 1;
            } else {
                let (mut code, mut q) = utf8_oced(self.src, sp, s);
                if self.mode == MODE_GRAPH {
                    while grapheme_extend(code) && q > s {
                        let r = utf8_oced(self.src, q, s);
                        code = r.0;
                        q = r.1;
                    }
                }
                sp = q;
            }
        }
    }

    fn min_expand(&mut self, mut s: usize, p: usize, ep: usize) -> MResult<Option<usize>> {
        loop {
            if let Some(res) = self.do_match(s, ep + 1)? {
                return Ok(Some(res));
            }
            if s >= self.src.len() {
                return Ok(None);
            }
            match self.singlematch(s, p, ep) {
                Some(next) => s = next,
                None => return Ok(None),
            }
        }
    }

    fn start_capture(&mut self, s: usize, p: usize, what: i64) -> MResult<Option<usize>> {
        if self.caps.len() >= MAX_CAPTURES {
            return Err("too many captures".to_string());
        }
        self.caps.push(Capture { init: s, len: what });
        let res = self.do_match(s, p)?;
        if res.is_none() {
            self.caps.pop();
        }
        Ok(res)
    }

    fn end_capture(&mut self, s: usize, p: usize) -> MResult<Option<usize>> {
        let l = self
            .caps
            .iter()
            .rposition(|c| c.len == CAP_UNFINISHED)
            .ok_or_else(|| "invalid pattern capture".to_string())?;
        self.caps[l].len = (s - self.caps[l].init) as i64;
        let res = self.do_match(s, p)?;
        if res.is_none() {
            self.caps[l].len = CAP_UNFINISHED;
        }
        Ok(res)
    }

    fn match_capture(&self, s: usize, digit: u8) -> MResult<Option<usize>> {
        let l = i64::from(digit) - i64::from(b'1');
        if l < 0 || l as usize >= self.caps.len() || self.caps[l as usize].len == CAP_UNFINISHED {
            return Err("invalid capture index".to_string());
        }
        let cap = &self.caps[l as usize];
        let len = cap.len.max(0) as usize;
        if self.src.len() - s >= len && self.src[cap.init..cap.init + len] == self.src[s..s + len] {
            Ok(Some(s + len))
        } else {
            Ok(None)
        }
    }

    fn do_match(&mut self, mut s: usize, mut p: usize) -> MResult<Option<usize>> {
        loop {
            match self.pat_at(p) {
                b'(' => {
                    return if self.pat_at(p + 1) == b')' {
                        self.start_capture(s, p + 2, CAP_POSITION)
                    } else {
                        self.start_capture(s, p + 1, CAP_UNFINISHED)
                    };
                }
                b')' => return self.end_capture(s, p + 1),
                ESC if self.pat_at(p + 1) == b'b' => {
                    match self.matchbalance(s, p + 2)? {
                        Some(next) => s = next,
                        None => return Ok(None),
                    }
                    p += 4;
                    continue;
                }
                ESC if self.pat_at(p + 1).is_ascii_digit() => {
                    match self.match_capture(s, self.pat_at(p + 1))? {
                        Some(next) => s = next,
                        None => return Ok(None),
                    }
                    p += 2;
                    continue;
                }
                0 => return Ok(Some(s)),
                b'$' if self.pat_at(p + 1) == 0 => {
                    return Ok(if s == self.src.len() { Some(s) } else { None });
                }
                _ => {
                    let ep = self.classend(p)?;
                    let es = if s < self.src.len() { self.singlematch(s, p, ep) } else { None };
                    match self.pat_at(ep) {
                        b'?' => {
                            if let Some(es) = es {
                                if let Some(res) = self.do_match(es, ep + 1)? {
                                    return Ok(Some(res));
                                }
                            }
                            p = ep + 1;
                        }
                        b'*' => return self.max_expand(s, p, ep),
                        b'+' => {
                            return match es {
                                Some(es) => self.max_expand(es, p, ep),
                                None => Ok(None),
                            };
                        }
                        b'-' => return self.min_expand(s, p, ep),
                        _ => match es {
                            Some(es) => {
                                s = es;
                                p = ep;
                            }
                            None => return Ok(None),
                        },
                    }
                }
            }
        }
    }

    /// Captures as flat (init, len) pairs; none for a pattern without captures.
    fn flat_captures(&self) -> Vec<i64> {
        self.caps.iter().flat_map(|c| [c.init as i64, c.len]).collect()
    }
}

/// Pattern `p` without the NUL terminator semantics of C: the pattern ends at
/// the first NUL byte.
fn pattern(p: &[u8]) -> &[u8] {
    match p.iter().position(|&b| b == 0) {
        Some(end) => &p[..end],
        None => p,
    }
}

fn units(mode: i64, s: &[u8]) -> i64 {
    if is_mb(mode) {
        utf8_count(s, 0, s.len(), mode == MODE_GRAPH, -1).0
    } else {
        s.len() as i64
    }
}

fn simple_lower(c: u32) -> u32 {
    if c > 0xFFFF {
        return c;
    }
    let Some(ch) = char::from_u32(c) else { return c };
    let mut it = ch.to_lowercase();
    match (it.next(), it.next()) {
        (Some(l), None) => l as u32,
        _ => c,
    }
}

fn simple_upper(c: u32) -> u32 {
    if c > 0xFFFF {
        return c;
    }
    let Some(ch) = char::from_u32(c) else { return c };
    let mut it = ch.to_uppercase();
    match (it.next(), it.next()) {
        (Some(u), None) => u as u32,
        _ => c,
    }
}

fn map_case(mode: i64, s: &[u8], f: fn(u32) -> u32) -> Vec<u8> {
    let mb = is_mb(mode);
    let mut out = Vec::with_capacity(s.len());
    let mut p = 0;
    while p < s.len() {
        let (c, next) = if mb { utf8_deco(s, p, s.len()) } else { (u32::from(s[p]), p + 1) };
        p = next;
        let c = if mode == MODE_ASCII && c & 0x80 != 0 { c } else { f(c) };
        if mb {
            utf8_enco(&mut out, c);
        } else {
            out.push(c as u8);
        }
    }
    out
}

pub(crate) fn register(lua: &mut Lua, s: &tex_lua::LuaTable) -> Result<(), String> {
    sys_reg!(lua, s, "uni_len", |mode: i64, s: LuaString| -> i64 { units(mode, &bytes_of(&s)) });
    sys_reg!(lua, s, "uni_sub", |mode: i64, s: LuaString, start: i64, end: i64| -> LuaBytes {
        let s = bytes_of(&s);
        let graph = mode == MODE_GRAPH;
        let len = units(mode, &s);
        let mut start = posrelat(start, len);
        let mut end = posrelat(end, len);
        if start < 1 {
            start = 1;
        }
        if end > len {
            end = len;
        }
        if start > end {
            return LuaBytes(Vec::new());
        }
        let count = end - (start - 1);
        if !is_mb(mode) {
            let from = (start - 1) as usize;
            return LuaBytes(s[from..from + count as usize].to_vec());
        }
        let from = if start - 1 > 0 { utf8_count(&s, 0, s.len(), graph, start - 1).1 } else { 0 };
        let to = utf8_count(&s, from, s.len(), graph, count).1;
        LuaBytes(s[from..to].to_vec())
    });
    sys_reg!(lua, s, "uni_byte", |mode: i64, s: LuaString, i: i64, j: Option<i64>| -> Vec<i64> {
        let s = bytes_of(&s);
        let mb = is_mb(mode);
        let graph = mode == MODE_GRAPH;
        let len = units(mode, &s);
        let mut posi = posrelat(i, len);
        let mut pose = posrelat(j.unwrap_or(posi), len);
        if posi <= 0 {
            posi = 1;
        }
        if pose > len {
            pose = len;
        }
        posi -= 1;
        let n = pose - posi;
        if n <= 0 {
            return Vec::new();
        }
        if !mb {
            return s[posi as usize..(posi + n) as usize].iter().map(|&b| i64::from(b)).collect();
        }
        let from = if posi > 0 { utf8_count(&s, 0, s.len(), graph, posi).1 } else { 0 };
        let to = utf8_count(&s, from, s.len(), graph, n).1;
        let mut out = Vec::new();
        let mut p = from;
        while p < to {
            let (code, next) = utf8_deco(&s, p, to);
            out.push(i64::from(code));
            p = next;
        }
        out
    });
    sys_reg!(lua, s, "uni_char", |mode: i64, codes: tex_lua::LuaTable| -> Result<LuaBytes, String> {
        let codes: Vec<i64> = codes.sequence_values().map_err(|_| "bad argument to 'char' (number expected)".to_string())?;
        let mb = is_mb(mode);
        let limit: i64 = if mb { 0x110100 } else { 0x100 };
        let mut out = Vec::new();
        for (i, &c) in codes.iter().enumerate() {
            if !(0..limit).contains(&c) {
                return Err(format!("bad argument #{} to '?' (invalid value)", i + 1));
            }
            if mb {
                utf8_enco(&mut out, c as u32);
            } else {
                out.push(c as u8);
            }
        }
        Ok(LuaBytes(out))
    });
    sys_reg!(lua, s, "uni_lower", |mode: i64, s: LuaString| -> LuaBytes {
        LuaBytes(map_case(mode, &bytes_of(&s), simple_lower))
    });
    sys_reg!(lua, s, "uni_upper", |mode: i64, s: LuaString| -> LuaBytes {
        LuaBytes(map_case(mode, &bytes_of(&s), simple_upper))
    });
    sys_reg!(lua, s, "uni_reverse", |mode: i64, s: LuaString| -> LuaBytes {
        let s = bytes_of(&s);
        if !is_mb(mode) {
            return LuaBytes(s.iter().rev().copied().collect());
        }
        let mut out = Vec::with_capacity(s.len());
        let mut p = s.len();
        while p > 0 {
            let q = p;
            let (mut code, mut at) = utf8_oced(&s, p, 0);
            if mode == MODE_GRAPH {
                while grapheme_extend(code) && at > 0 {
                    let r = utf8_oced(&s, at, 0);
                    code = r.0;
                    at = r.1;
                }
            }
            out.extend_from_slice(&s[at..q]);
            p = at;
        }
        LuaBytes(out)
    });
    // Try to match `pat` at byte offset `pos` of `src`: (end, capture pairs).
    sys_reg!(
        lua,
        s,
        "uni_try",
        |mode: i64, src: LuaString, pat: LuaString, pos: i64| -> Result<(Option<i64>, Vec<i64>), String> {
            let src = bytes_of(&src);
            let pat = bytes_of(&pat);
            let mut m = Matcher { src: &src, pat: pattern(&pat), mode, mb: is_mb(mode), caps: Vec::new() };
            let pos = (pos.max(0) as usize).min(src.len());
            match m.do_match(pos, 0)? {
                Some(e) => Ok((Some(e as i64), m.flat_captures())),
                None => Ok((None, Vec::new())),
            }
        }
    );
    // The scan loop of `find`/`match`: (start, end, captures) of the first match.
    sys_reg!(
        lua,
        s,
        "uni_scan",
        |mode: i64, src: LuaString, pat: LuaString, init: i64, anchor: bool| -> Result<(Option<i64>, i64, Vec<i64>), String> {
            let src = bytes_of(&src);
            let pat = bytes_of(&pat);
            let mut m = Matcher { src: &src, pat: pattern(&pat), mode, mb: is_mb(mode), caps: Vec::new() };
            let mut s1 = (init.max(0) as usize).min(src.len());
            loop {
                m.caps.clear();
                if let Some(e) = m.do_match(s1, 0)? {
                    return Ok((Some(s1 as i64), e as i64, m.flat_captures()));
                }
                // LuaTeX advances by the UTF-8 length of the lead byte
                let step = if mode > 1 {
                    match src.get(s1).copied().unwrap_or(0) {
                        0..=0x7f => 1,
                        0x80..=0xdf => 2,
                        0xe0..=0xef => 3,
                        _ => 4,
                    }
                } else {
                    1
                };
                s1 += step;
                if s1 >= src.len() || anchor {
                    return Ok((None, 0, Vec::new()));
                }
            }
        }
    );
    Ok(())
}
