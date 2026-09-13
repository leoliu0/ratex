//! format.name$ / num.names$ — a faithful port of bibtex.web's
//! name-string processing modules.

use crate::classes::{
    is_alpha, is_sep_char, is_white, lex_class, lookup_ctrl_seq, CtrlSeq, LexClass,
};

const TIE: u8 = b'~';
const LONG_TOKEN: usize = 3; // a token this long or longer is "long"
const LONG_NAME: usize = 3; // a name part this long or longer is "long"

struct NameData {
    buf: Vec<u8>,     // name_buf: token text concatenated
    toks: Vec<usize>, // name_tok: start offsets; last entry = end marker
    sep: Vec<u8>,     // name_sep_char[i]: separator preceding token i
    num_commas: usize,
    comma1: usize,
    comma2: usize,
}

/// Scan for " and " starting at p; returns the position just past the "nd"
/// of the found "and" (i.e. at the whitespace following it), or buf.len().
fn name_scan_for_and(buf: &[u8], mut p: usize) -> usize {
    let n = buf.len();
    let mut brace = 0i32;
    let mut prev_white = false;
    let mut found = false;
    while !found && p < n {
        match buf[p] {
            b'a' | b'A' => {
                p += 1;
                if prev_white && p + 2 < n {
                    if (buf[p] == b'n' || buf[p] == b'N')
                        && (buf[p + 1] == b'd' || buf[p + 1] == b'D')
                        && is_white(buf[p + 2])
                    {
                        p += 2;
                        found = true;
                    }
                }
                prev_white = false;
            }
            b'{' => {
                brace += 1;
                p += 1;
                while brace > 0 && p < n {
                    if buf[p] == b'}' {
                        brace -= 1;
                    } else if buf[p] == b'{' {
                        brace += 1;
                    }
                    p += 1;
                }
                prev_white = false;
            }
            b'}' => {
                p += 1;
                prev_white = false;
            }
            c => {
                p += 1;
                prev_white = is_white(c);
            }
        }
    }
    p
}

/// von token check for buf[start..end].
fn von_token(buf: &[u8], start: usize, end: usize) -> bool {
    let mut level = 0i32;
    let mut i = start;
    while i < end {
        let c = buf[i];
        if c.is_ascii_uppercase() {
            return false;
        }
        if c.is_ascii_lowercase() {
            return true;
        }
        if c == b'{' {
            level += 1;
            i += 1;
            if i + 2 < end && buf[i] == b'\\' {
                i += 1; // skip backslash
                let ys = i;
                while i < end && is_alpha(buf[i]) {
                    i += 1;
                }
                if let Some(cs) = lookup_ctrl_seq(&buf[ys..i]) {
                    return match cs {
                        // uppercase forms: not a von token
                        CtrlSeq::OeUpper
                        | CtrlSeq::AeUpper
                        | CtrlSeq::AaUpper
                        | CtrlSeq::OUpper
                        | CtrlSeq::LUpper => false,
                        // lowercase specials: von token
                        _ => true,
                    };
                }
                while i < end && level > 0 {
                    let d = buf[i];
                    if d.is_ascii_uppercase() {
                        return false;
                    }
                    if d.is_ascii_lowercase() {
                        return true;
                    }
                    if d == b'}' {
                        level -= 1;
                    } else if d == b'{' {
                        level += 1;
                    }
                    i += 1;
                }
                return false;
            } else {
                while level > 0 && i < end {
                    if buf[i] == b'}' {
                        level -= 1;
                    } else if buf[i] == b'{' {
                        level += 1;
                    }
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Isolate the `which`-th name from a name list (1-based).
/// Returns the byte range of the name in the list.
fn isolate_name(list: &[u8], which: usize) -> (usize, usize) {
    let n = list.len();
    let mut p = 0usize;
    let mut num = 0usize;
    let mut x = 0usize;
    while num < which && p < n {
        num += 1;
        x = p;
        p = name_scan_for_and(list, p);
    }
    if p < n {
        // remove the trailing "and": p is at the whitespace after "and";
        // back up to the whitespace before it
        p -= 4;
    }
    (x, p)
}

/// Copy the name into tokens, counting commas
/// (bibtex.web @<Copy name and count |comma|s...@>).
fn copy_name(list: &[u8], x: usize, mut p: usize) -> NameData {
    // remove trailing white_space, sep_chars, and commas
    while p > x {
        let c = list[p - 1];
        if is_white(c) || is_sep_char(c) || c == b',' {
            p -= 1;
        } else {
            break;
        }
    }
    let mut nd = NameData {
        buf: Vec::new(),
        toks: Vec::new(),
        sep: Vec::new(),
        num_commas: 0,
        comma1: 0,
        comma2: 0,
    };
    let mut token_starting = true;
    let mut brace = 0i32;
    let mut i = x;
    while i < p {
        let c = list[i];
        match c {
            b',' => {
                if nd.num_commas < 2 {
                    nd.num_commas += 1;
                    if nd.num_commas == 1 {
                        nd.comma1 = nd.toks.len();
                    } else {
                        nd.comma2 = nd.toks.len();
                    }
                    set_sep(&mut nd, b',');
                }
                i += 1;
                token_starting = true;
            }
            b'{' => {
                brace += 1;
                if token_starting {
                    nd.toks.push(nd.buf.len());
                }
                nd.buf.push(b'{');
                i += 1;
                while brace > 0 && i < p {
                    let d = list[i];
                    if d == b'}' {
                        brace -= 1;
                    } else if d == b'{' {
                        brace += 1;
                    }
                    nd.buf.push(d);
                    i += 1;
                }
                token_starting = false;
            }
            b'}' => {
                if token_starting {
                    nd.toks.push(nd.buf.len());
                }
                i += 1;
                token_starting = false;
            }
            w if is_white(w) => {
                if !token_starting {
                    set_sep(&mut nd, b' ');
                }
                i += 1;
                token_starting = true;
            }
            s if is_sep_char(s) => {
                if !token_starting {
                    set_sep(&mut nd, s);
                }
                i += 1;
                token_starting = true;
            }
            other => {
                if token_starting {
                    nd.toks.push(nd.buf.len());
                }
                nd.buf.push(other);
                i += 1;
                token_starting = false;
            }
        }
    }
    nd.toks.push(nd.buf.len()); // end marker
    nd
}

fn set_sep(nd: &mut NameData, c: u8) {
    let k = nd.toks.len();
    if nd.sep.len() < k {
        nd.sep.resize(k, b' ');
    }
    if nd.sep.len() == k {
        nd.sep.push(c);
    } else {
        nd.sep[k] = c;
    }
}

fn get_sep(nd: &NameData, i: usize) -> u8 {
    *nd.sep.get(i).unwrap_or(&b' ')
}

struct Parts {
    first_start: usize,
    first_end: usize,
    von_start: usize,
    von_end: usize,
    last_end: usize,
    jr_end: usize,
}

/// von_name_ends_and_last_name_starts_stuff: von part is [von_start, von_end).
fn von_name_ends(nd: &NameData, von_start: usize, last_end: usize) -> usize {
    let mut von_end = last_end - 1;
    while von_end > von_start {
        if von_token(&nd.buf, nd.toks[von_end - 1], nd.toks[von_end]) {
            return von_end;
        }
        von_end -= 1;
    }
    von_end
}

fn find_parts(nd: &NameData) -> Parts {
    let ntoks = nd.toks.len() - 1;

    if nd.num_commas == 0 {
        let first_start = 0;
        let last_end = ntoks;
        let jr_end = last_end;
        let mut von_start = 0usize;
        let mut found = false;
        while von_start < last_end.saturating_sub(1) {
            if von_token(&nd.buf, nd.toks[von_start], nd.toks[von_start + 1]) {
                found = true;
                break;
            }
            von_start += 1;
        }
        let von_end;
        if found {
            von_end = von_name_ends(nd, von_start, last_end);
        } else {
            // no von name; backtrack over tokens connected by non-tie seps
            while von_start > 0 {
                let s = get_sep(nd, von_start);
                if lex_class(s) != LexClass::SepChar || s == TIE {
                    break;
                }
                von_start -= 1;
            }
            von_end = von_start;
        }
        Parts {
            first_start,
            first_end: von_start,
            von_start,
            von_end,
            last_end,
            jr_end,
        }
    } else if nd.num_commas == 1 {
        let von_start = 0usize;
        let last_end = nd.comma1;
        let jr_end = last_end;
        let first_start = jr_end;
        let first_end = ntoks;
        let von_end = von_name_ends(nd, von_start, last_end);
        Parts {
            first_start,
            first_end,
            von_start,
            von_end,
            last_end,
            jr_end,
        }
    } else {
        let von_start = 0usize;
        let last_end = nd.comma1;
        let jr_end = nd.comma2;
        let first_start = jr_end;
        let first_end = ntoks;
        let von_end = von_name_ends(nd, von_start, last_end);
        Parts {
            first_start,
            first_end,
            von_start,
            von_end,
            last_end,
            jr_end,
        }
    }
}

/// Count text chars in out[start..]: special chars count as one, braces not
/// counted; returns whether there are at least `enough`.
fn enough_text_chars(out: &[u8], start: usize, enough: usize) -> bool {
    let mut num = 0usize;
    let mut i = start;
    let mut level = 0i32;
    while i < out.len() && num < enough {
        i += 1;
        let c = out[i - 1];
        if c == b'{' {
            level += 1;
            if level == 1 && i < out.len() && out[i] == b'\\' {
                i += 1;
                while i < out.len() && level > 0 {
                    if out[i] == b'}' {
                        level -= 1;
                    } else if out[i] == b'{' {
                        level += 1;
                    }
                    i += 1;
                }
            }
        } else if c == b'}' {
            if level > 0 {
                level -= 1;
            }
        }
        num += 1;
    }
    num >= enough
}

/// Emit one token (full when double_letter, else abbreviated).
fn emit_token(nd: &NameData, tok: usize, double_letter: bool, out: &mut Vec<u8>) {
    let s = nd.toks[tok];
    let e = nd.toks[tok + 1];
    if double_letter {
        out.extend_from_slice(&nd.buf[s..e]);
        return;
    }
    let mut i = s;
    while i < e {
        let c = nd.buf[i];
        if is_alpha(c) {
            out.push(c);
            return;
        }
        if c == b'{' && i + 1 < e && nd.buf[i + 1] == b'\\' {
            out.push(b'{');
            out.push(b'\\');
            i += 2;
            let mut lvl = 1i32;
            while i < e && lvl > 0 {
                let d = nd.buf[i];
                if d == b'}' {
                    lvl -= 1;
                } else if d == b'{' {
                    lvl += 1;
                }
                out.push(d);
                i += 1;
            }
            return;
        }
        i += 1;
    }
}

/// Skip over a braced group while the level is above 1; `level` is the
/// current level (>1). Advances i to just past the matching `}`.
fn skip_gt_one(sp: &[u8], level: &mut i32, i: &mut usize) {
    while *level > 1 && *i < sp.len() {
        if sp[*i] == b'}' {
            *level -= 1;
        } else if sp[*i] == b'{' {
            *level += 1;
        }
        *i += 1;
    }
}

/// format.name$ main entry.
pub fn format_name(fmt: &str, which: i64, list: &str) -> String {
    let list_b = list.as_bytes();
    let which = if which < 0 { 0 } else { which as usize };
    let (x, p) = isolate_name(list_b, which);
    let nd = copy_name(list_b, x, p.min(list_b.len()));
    let parts = find_parts(&nd);
    let sp = fmt.as_bytes();

    let mut out: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < sp.len() {
        if sp[i] == b'{' {
            i += 1;
            i = format_part(sp, i, &nd, &parts, &mut out);
        } else if sp[i] == b'}' {
            i += 1;
        } else {
            out.push(sp[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn part_range(letter: u8, p: &Parts) -> (usize, usize) {
    match letter {
        b'f' | b'F' => (p.first_start, p.first_end),
        b'v' | b'V' => (p.von_start, p.von_end),
        b'l' | b'L' => (p.von_end, p.last_end),
        b'j' | b'J' => (p.last_end, p.jr_end),
        _ => (0, 0),
    }
}

/// Process one brace-level-1 group starting just past `{`. Returns the
/// index just past the group's closing brace, appending to `out`.
fn format_part(sp: &[u8], mut i: usize, nd: &NameData, parts: &Parts, out: &mut Vec<u8>) -> usize {
    // ---- pass 1: scan the group, decide letter / double_letter / write?
    let sp_xptr1 = i;
    let mut alpha_found = false;
    let mut double_letter = false;
    let mut end_of_group = false;
    let mut to_be_written = true;
    let mut letter = b'\0';
    let mut level = 1i32;
    while !end_of_group && i < sp.len() {
        if is_alpha(sp[i]) && level == 1 {
            i += 1;
            if alpha_found {
                to_be_written = false; // second brace-level-1 letter
            } else {
                let l = sp[i - 1];
                if matches!(l, b'f' | b'F' | b'v' | b'V' | b'l' | b'L' | b'j' | b'J') {
                    letter = l;
                    let (cur, last) = part_range(l, parts);
                    if cur == last {
                        to_be_written = false;
                    }
                    if i < sp.len() && sp[i].to_ascii_lowercase() == l.to_ascii_lowercase() {
                        double_letter = true;
                    }
                    alpha_found = true;
                    if double_letter {
                        i += 1;
                    }
                } else {
                    to_be_written = false; // illegal letter
                    alpha_found = true;
                }
            }
        } else if sp[i] == b'}' {
            level -= 1;
            i += 1;
            end_of_group = true;
        } else if sp[i] == b'{' {
            level += 1;
            i += 1;
            skip_gt_one(sp, &mut level, &mut i);
        } else {
            i += 1;
        }
    }

    if !(end_of_group && to_be_written) {
        return i;
    }

    // ---- pass 2: output
    let xptr = out.len(); // ex_buf_xptr for this part
    let (mut cur_token, last_token) = part_range(letter, parts);
    i = sp_xptr1;
    level = 1;
    while level > 0 && i < sp.len() {
        if is_alpha(sp[i]) && level == 1 {
            i += 1;
            if double_letter {
                i += 1;
            }
            if i < sp.len() && sp[i] == b'{' {
                // custom inter-token string
                level += 1;
                i += 1;
                let s1 = i;
                skip_gt_one(sp, &mut level, &mut i);
                let xptr2 = i - 1; // at the group's closing brace
                while cur_token < last_token {
                    emit_token(nd, cur_token, double_letter, out);
                    cur_token += 1;
                    if cur_token < last_token {
                        out.extend_from_slice(&sp[s1..xptr2]);
                    }
                }
            } else {
                while cur_token < last_token {
                    emit_token(nd, cur_token, double_letter, out);
                    cur_token += 1;
                    if cur_token < last_token {
                        if !double_letter {
                            out.push(b'.');
                        }
                        let sep = get_sep(nd, cur_token);
                        if lex_class(sep) == LexClass::SepChar {
                            out.push(sep);
                        } else if cur_token == last_token - 1
                            || !enough_text_chars(out, xptr, LONG_TOKEN)
                        {
                            out.push(TIE);
                        } else {
                            out.push(b' ');
                        }
                    }
                }
            }
        } else if sp[i] == b'}' {
            level -= 1;
            i += 1;
            if level > 0 {
                out.push(b'}');
            }
        } else if sp[i] == b'{' {
            level += 1;
            i += 1;
            out.push(b'{');
        } else {
            out.push(sp[i]);
            i += 1;
        }
    }

    // handle a discretionary tie at the end of this name part
    if !out.is_empty() && out[out.len() - 1] == TIE {
        out.pop();
        if !out.is_empty() && out[out.len() - 1] == TIE {
            // not discretionary: the tie stays removed
        } else if !enough_text_chars(out, xptr, LONG_NAME) {
            out.push(TIE); // short name part: restore the tie
        } else {
            out.push(b' ');
        }
    }
    i
}

/// num.names$
pub fn num_names(list: &str) -> i64 {
    let b = list.as_bytes();
    let mut p = 0usize;
    let mut n = 0i64;
    while p < b.len() {
        n += 1;
        p = name_scan_for_and(b, p);
    }
    n
}
