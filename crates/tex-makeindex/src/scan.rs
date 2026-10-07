//! Reading `.idx` files: makeindex's `scanid.c`.

use crate::stream::{Stream, EOF};
use crate::style::{Style, ALPL, ALPU, ARAB, ROML, ROMU};
use crate::Transcript;

/// `FIELD_MAX`: item, subitem, subsubitem.
pub(crate) const FIELD_MAX: usize = 3;
/// Page type of an entry not yet decoded (`EMPTY`).
const EMPTY: i16 = -9999;
/// Page type marking an entry that repeats an earlier one (`DUPLICATE`).
pub(crate) const DUPLICATE: i16 = 9999;
/// Group of keys that start with a symbol (`SYMBOL`); pure numbers have their
/// value as group.
pub(crate) const SYMBOL: i32 = -1;
/// Group of keys that start with a letter (`ALPHA`).
pub(crate) const ALPHA: i32 = -2;

const ARGUMENT_MAX: usize = 10240;
const ARRAY_MAX: usize = 1024;
const NUMBER_MAX: usize = 99;
const ARABIC_MAX: usize = 99;
const ROMAN_MAX: usize = 99;
const PAGEFIELD_MAX: usize = 10;
/// One progress dot per this many entries.
pub(crate) const DOT_MAX: usize = 1000;

const LFD: i32 = b'\n' as i32;
const TAB: i32 = b'\t' as i32;
const SPC: i32 = b' ' as i32;

/// An accepted index entry (makeindex's `FIELD`).
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    /// Sort keys per level.
    pub(crate) sf: [Vec<u8>; FIELD_MAX],
    /// Printed texts per level; empty when the sort key is printed.
    pub(crate) af: [Vec<u8>; FIELD_MAX],
    pub(crate) group: i32,
    /// The page as written.
    pub(crate) lpg: Vec<u8>,
    /// Numeric value of each page field.
    pub(crate) npg: Vec<i32>,
    /// Numeral type of the first page field, or `DUPLICATE`.
    pub(crate) ty: i16,
    /// The encapsulator, with a leading range operator if any.
    pub(crate) encap: Vec<u8>,
    /// Sizes of the zeroed buffers makeindex allocates for `sf`, `af` and
    /// `encap` (what lies beyond them shows in `-l` comparisons).
    pub(crate) sf_cap: [usize; FIELD_MAX],
    pub(crate) af_cap: [usize; FIELD_MAX],
    pub(crate) encap_cap: usize,
    /// Index of the input file.
    pub(crate) file: usize,
    /// Input line.
    pub(crate) lc: i32,
}

/// A character read as `int` compares equal to a style character only when
/// the (signed) `char` is not negative.
fn is(c: i32, ch: u8) -> bool {
    ch < 0x80 && c == i32::from(ch)
}

fn at(text: &[u8], index: usize) -> u8 {
    text.get(index).copied().unwrap_or(0)
}

fn tail(text: &[u8], from: usize) -> &[u8] {
    text.get(from..).unwrap_or(&[])
}

/// A C string ends at its first NUL.
fn c_str(text: &[u8]) -> &[u8] {
    match text.iter().position(|&byte| byte == 0) {
        Some(end) => &text[..end],
        None => text,
    }
}

fn strspn(text: &[u8], set: &[u8]) -> usize {
    text.iter().take_while(|byte| set.contains(byte)).count()
}

/// `group_type`: the value of a key made of digits only, else `SYMBOL` or
/// `ALPHA` by its first byte.
pub(crate) fn group_type(key: &[u8]) -> i32 {
    if key.iter().all(u8::is_ascii_digit) {
        if key.is_empty() {
            return 0;
        }
        // sscanf's `%d` goes through strtol and truncates to int.
        let mut value: i64 = 0;
        for &digit in key {
            value = match value.checked_mul(10).and_then(|v| v.checked_add(i64::from(digit - b'0'))) {
                Some(value) => value,
                None => return -1,
            };
        }
        return value as i32;
    }
    let first = key[0];
    let symbol = (b'!'..=b'@').contains(&first)
        || (b'['..=b'`').contains(&first)
        || (b'{'..=b'~').contains(&first);
    if symbol { SYMBOL } else { ALPHA }
}

fn is_roman_lower(byte: u8) -> bool {
    b"ivxlcdm".contains(&byte)
}

fn is_roman_upper(byte: u8) -> bool {
    b"IVXLCDM".contains(&byte)
}

fn roman_value(byte: u8) -> i32 {
    match byte.to_ascii_lowercase() {
        b'i' => 1,
        b'v' => 5,
        b'x' => 10,
        b'l' => 50,
        b'c' => 100,
        b'd' => 500,
        b'm' => 1000,
        _ => 0,
    }
}

fn alpha_value(byte: u8) -> i32 {
    if byte.is_ascii_alphabetic() { i32::from(byte.to_ascii_lowercase() - b'a') } else { 0 }
}

/// One input file being read.
struct FileScan<'a> {
    stream: Stream<'a>,
    name: &'a str,
    /// Lines read so far (`idx_lc`).
    lc: i32,
    /// Entries started (`idx_tc`).
    tc: usize,
    /// Entries rejected (`idx_ec`).
    ec: usize,
    key: Vec<u8>,
    no: Vec<u8>,
}

impl FileScan<'_> {
    fn error(&mut self, log: &mut Transcript, message: &[u8]) {
        log.error_line();
        log.ilg(format!("!! Input index error (file = {}, line = {}):\n   -- ", self.name, self.lc).as_bytes());
        log.ilg(message);
        self.ec += 1;
    }

    fn flush_to_eol(&mut self) {
        loop {
            let c = self.stream.getc();
            if c == LFD || c == EOF {
                break;
            }
        }
    }

    /// `IDX_SKIPLINE`.
    fn skip_line(&mut self) {
        self.flush_to_eol();
        self.lc += 1;
    }
}

/// Settings and state shared by all input files.
pub(crate) struct Scanner<'s> {
    style: &'s Style,
    compress_blanks: bool,
    german_sort: bool,
    /// The type each page field position last had (`type_guess`), which
    /// decides letters valid both as Roman numerals and as letters.
    type_guess: [i32; PAGEFIELD_MAX],
    /// The `level`, `actual` and `encap` characters and the byte after them.
    /// TeX Live's makeindex reads and writes `type_guess[PAGEFIELD_MAX]`,
    /// one past the end, while rejecting a page number with too many
    /// fields; in its binary those four bytes are these characters, so the
    /// write changes how all later keys are split.
    chars: [u8; 4],
    pub(crate) entries: Vec<Entry>,
    /// Entries started in all files (`idx_tt`).
    pub(crate) total: usize,
    /// Entries rejected in all files (`idx_et`).
    pub(crate) rejected: usize,
}

impl<'s> Scanner<'s> {
    pub(crate) fn new(style: &'s Style, compress_blanks: bool, german_sort: bool) -> Self {
        Scanner {
            style,
            compress_blanks,
            german_sort,
            type_guess: [i32::from(EMPTY); PAGEFIELD_MAX],
            chars: [style.level, style.actual, style.encap, 0],
            entries: Vec::new(),
            total: 0,
            rejected: 0,
        }
    }

    /// `type_guess[index]`.
    fn guess(&self, index: usize) -> i32 {
        match index {
            index if index < PAGEFIELD_MAX => self.type_guess[index],
            PAGEFIELD_MAX => i32::from_le_bytes(self.chars),
            _ => i32::from(EMPTY),
        }
    }

    fn set_guess(&mut self, index: usize, value: i32) {
        match index {
            index if index < PAGEFIELD_MAX => self.type_guess[index] = value,
            PAGEFIELD_MAX => self.chars = value.to_le_bytes(),
            _ => {}
        }
    }

    /// `scan_idx`.
    pub(crate) fn scan_file(&mut self, bytes: &[u8], file: usize, name: &str, log: &mut Transcript) {
        log.message(format!("Scanning input file {name}...").as_bytes());
        log.idx_dc = 0;
        let mut f = FileScan { stream: Stream::new(bytes), name, lc: 0, tc: 0, ec: 0, key: Vec::new(), no: Vec::new() };
        let style = self.style;
        let mut keyword: Vec<u8> = Vec::new();
        let mut arg_count: i32 = -1;
        loop {
            let c = f.stream.getc();
            match c {
                EOF => {
                    if arg_count == 2 {
                        f.lc += 1;
                        if self.make_key(&mut f, file, log) {
                            log.dot(DOT_MAX);
                        }
                        arg_count = -1;
                    } else {
                        if arg_count > -1 {
                            f.lc += 1;
                            f.error(log, b"Missing arguments -- need two (premature EOF).\n");
                        }
                        break;
                    }
                }
                LFD => {
                    f.lc += 1;
                    if arg_count == 2 {
                        if self.make_key(&mut f, file, log) {
                            log.dot(DOT_MAX);
                        }
                        arg_count = -1;
                    } else if arg_count > -1 {
                        f.error(log, b"Missing arguments -- need two (premature LFD).\n");
                        arg_count = -1;
                    }
                }
                TAB | SPC => {}
                _ => match arg_count {
                    -1 => {
                        keyword.clear();
                        keyword.push(c as u8);
                        arg_count = 0;
                        f.tc += 1;
                    }
                    0 => {
                        if is(c, style.arg_open) {
                            arg_count = 1;
                            if c_str(&keyword) == style.keyword.as_slice() {
                                if !self.scan_arg1(&mut f, log) {
                                    arg_count = -1;
                                }
                            } else {
                                f.skip_line();
                                arg_count = -1;
                                let message = [b"Unknown index keyword ", c_str(&keyword), b".\n"].concat();
                                f.error(log, &message);
                            }
                        } else if keyword.len() < ARRAY_MAX {
                            keyword.push(c as u8);
                        } else {
                            f.skip_line();
                            arg_count = -1;
                            let message = [
                                b"Index keyword ",
                                c_str(&keyword),
                                format!(" too long (max {ARRAY_MAX}).\n").as_bytes(),
                            ]
                            .concat();
                            f.error(log, &message);
                        }
                    }
                    1 => {
                        if is(c, style.arg_open) {
                            arg_count = 2;
                            if !self.scan_arg2(&mut f, log) {
                                arg_count = -1;
                            }
                        } else {
                            f.skip_line();
                            arg_count = -1;
                            let message = [
                                &b"No opening delimiter for second argument (illegal character `"[..],
                                &[c as u8],
                                b"').\n",
                            ]
                            .concat();
                            f.error(log, &message);
                        }
                    }
                    2 => {
                        f.skip_line();
                        arg_count = -1;
                        let message = [
                            &b"No closing delimiter for second argument (illegal character `"[..],
                            &[c as u8],
                            b"').\n",
                        ]
                        .concat();
                        f.error(log, &message);
                    }
                    _ => {}
                },
            }
        }
        self.total += f.tc;
        self.rejected += f.ec;
        log.message(format!("done ({} entries accepted, {} rejected).\n", f.tc - f.ec, f.ec).as_bytes());
    }

    /// `scan_arg1`: the first argument, into `f.key`.
    fn scan_arg1(&mut self, f: &mut FileScan, log: &mut Transcript) -> bool {
        let style = self.style;
        f.key.clear();
        let mut depth = 0usize;
        let mut a = f.stream.getc();
        if self.compress_blanks {
            while a == SPC || a == TAB {
                a = f.stream.getc();
            }
        }
        while f.key.len() < ARGUMENT_MAX && a != EOF {
            if is(a, style.quote) || is(a, style.escape) {
                // The next character is taken literally; both are kept.
                f.key.push(a as u8);
                a = f.stream.getc();
                f.key.push(a as u8);
            } else if is(a, style.arg_open) {
                f.key.push(a as u8);
                depth += 1;
            } else if is(a, style.arg_close) {
                if depth == 0 {
                    if self.compress_blanks && f.key.last() == Some(&b' ') {
                        f.key.pop();
                    }
                    return true;
                }
                f.key.push(a as u8);
                depth -= 1;
            } else {
                match a {
                    LFD => {
                        f.lc += 1;
                        f.error(log, b"Incomplete first argument (premature LFD).\n");
                        return false;
                    }
                    TAB | SPC if self.compress_blanks => {
                        if f.key.last().is_some_and(|&last| last != b' ' && last != b'\t') {
                            f.key.push(b' ');
                        }
                    }
                    _ => f.key.push(a as u8),
                }
            }
            a = f.stream.getc();
        }
        f.flush_to_eol();
        f.lc += 1;
        f.error(log, format!("First argument too long (max {ARGUMENT_MAX}).\n").as_bytes());
        false
    }

    /// `scan_arg2`: the page number, into `f.no`.
    fn scan_arg2(&mut self, f: &mut FileScan, log: &mut Transcript) -> bool {
        f.no.clear();
        let mut a = f.stream.getc();
        while a == SPC || a == TAB {
            a = f.stream.getc();
        }
        let mut hit_blank = false;
        while f.no.len() < NUMBER_MAX {
            if is(a, self.style.arg_close) {
                return true;
            }
            match a {
                LFD => {
                    f.lc += 1;
                    f.error(log, b"Incomplete second argument (premature LFD).\n");
                    return false;
                }
                TAB | SPC => hit_blank = true,
                _ => {
                    if hit_blank {
                        f.flush_to_eol();
                        f.lc += 1;
                        f.error(log, b"Illegal space within numerals in second argument.\n");
                        return false;
                    }
                    f.no.push(a as u8);
                }
            }
            a = f.stream.getc();
        }
        f.flush_to_eol();
        f.lc += 1;
        f.error(log, format!("Second argument too long (max {NUMBER_MAX}).\n").as_bytes());
        false
    }

    /// `make_key`: turns the two arguments into an entry.
    fn make_key(&mut self, f: &mut FileScan, file: usize, log: &mut Transcript) -> bool {
        let mut entry = Entry {
            sf: Default::default(),
            af: Default::default(),
            group: 0,
            lpg: Vec::new(),
            npg: Vec::new(),
            ty: EMPTY,
            encap: Vec::new(),
            sf_cap: [1; FIELD_MAX],
            af_cap: [1; FIELD_MAX],
            encap_cap: 1,
            file,
            lc: 0,
        };
        if !self.scan_key(f, &mut entry, log) {
            return false;
        }
        entry.group = group_type(&entry.sf[0]);
        let no = c_str(&f.no).to_vec();
        entry.lpg = no.clone();
        if !self.scan_no(f, log, &no, &mut entry.npg, &mut entry.ty) {
            return false;
        }
        entry.lc = f.lc;
        self.entries.push(entry);
        true
    }

    /// `scan_key`: splits the first argument into levels, sort keys, printed
    /// texts and the encapsulator.
    fn scan_key(&mut self, f: &mut FileScan, data: &mut Entry, log: &mut Transcript) -> bool {
        let key = c_str(&f.key).to_vec();
        let mut i = 0usize;
        let mut n = 0usize;
        let mut second_round = false;
        let last = FIELD_MAX - 1;
        loop {
            let byte = at(&key, n);
            if byte == 0 {
                break;
            }
            let cap = key.len() + 1;
            if byte == self.chars[2] {
                n += 1;
                data.encap_cap = cap;
                match self.scan_field(f, log, &key, &mut n, false, false, false) {
                    Some(field) => {
                        data.encap = field;
                        break;
                    }
                    None => return false,
                }
            }
            if byte == self.chars[1] {
                n += 1;
                data.af_cap[i] = cap;
                match self.scan_field(f, log, &key, &mut n, i != last, true, false) {
                    Some(field) => data.af[i] = field,
                    None => return false,
                }
            } else {
                if second_round {
                    i += 1;
                    n += 1;
                }
                data.sf_cap[i] = cap;
                match self.scan_field(f, log, &key, &mut n, i != last, true, true) {
                    Some(field) => data.sf[i] = field,
                    None => return false,
                }
                second_round = true;
                if self.german_sort && data.sf[i].contains(&b'"') {
                    let (sort, actual) = search_quote(&data.sf[i]);
                    data.af_cap[i] = if actual.is_empty() { 1 } else { data.sf[i].len() + 1 };
                    data.sf[i] = sort;
                    data.af[i] = actual;
                }
            }
        }
        let null = |data: &Entry| {
            if data.sf[0].is_empty() {
                return true;
            }
            for level in 1..FIELD_MAX - 1 {
                if data.sf[level].is_empty() && (!data.af[level].is_empty() || !data.sf[level + 1].is_empty()) {
                    return true;
                }
            }
            data.sf[FIELD_MAX - 1].is_empty() && !data.af[FIELD_MAX - 1].is_empty()
        };
        if null(data) {
            f.error(log, b"Illegal null field.\n");
            return false;
        }
        true
    }

    /// `scan_field`: one field of the first argument, from `key[n]` up to the
    /// first delimiter that may end it.
    #[allow(clippy::too_many_arguments)]
    fn scan_field(
        &self,
        f: &mut FileScan,
        log: &mut Transcript,
        key: &[u8],
        n: &mut usize,
        ck_level: bool,
        ck_encap: bool,
        ck_actual: bool,
    ) -> Option<Vec<u8>> {
        let style = self.style;
        let [level, actual, encap, _] = self.chars;
        let mut field: Vec<u8> = Vec::new();
        if self.compress_blanks && matches!(at(key, *n), b' ' | b'\t') {
            *n += 1;
        }
        loop {
            let mut escapes = 0usize;
            while at(key, *n) == style.escape {
                escapes += 1;
                field.push(at(key, *n));
                *n += 1;
            }
            let byte = at(key, *n);
            if byte == style.quote {
                if escapes % 2 == 0 {
                    *n += 1;
                    field.push(at(key, *n));
                } else {
                    field.push(byte);
                }
            } else if (ck_level && byte == level)
                || (ck_encap && byte == encap)
                || (ck_actual && byte == actual)
                || byte == 0
            {
                if !field.is_empty() && self.compress_blanks && field.last() == Some(&b' ') {
                    field.pop();
                }
                let end = c_str(&field).len();
                field.truncate(end);
                return Some(field);
            } else {
                field.push(byte);
                let extra = if !ck_level && byte == level {
                    Some(level)
                } else if !ck_encap && byte == encap {
                    Some(encap)
                } else if !ck_actual && byte == actual {
                    Some(actual)
                } else {
                    None
                };
                if let Some(extra) = extra {
                    let message = [
                        &b"Extra `"[..],
                        &[extra],
                        format!("' at position {} of first argument.\n", *n + 1).as_bytes(),
                    ]
                    .concat();
                    f.error(log, &message);
                    return None;
                }
            }
            if *n >= key.len() {
                // A quote at the very end took the terminator literally.
                let end = c_str(&field).len();
                field.truncate(end);
                return Some(field);
            }
            *n += 1;
        }
    }

    fn enter(&self, f: &mut FileScan, log: &mut Transcript, no: &[u8], npg: &mut Vec<i32>, value: i32) -> bool {
        if npg.len() >= PAGEFIELD_MAX {
            let message =
                [b"Page number ", no, format!(" has too many fields (max. {PAGEFIELD_MAX}).").as_bytes()].concat();
            f.error(log, &message);
            return false;
        }
        npg.push(value);
        true
    }

    /// `scan_no`: decodes the page number `no` (the rest of it, for later
    /// fields) into `npg`; `ty` receives the type of this field.
    fn scan_no(&mut self, f: &mut FileScan, log: &mut Transcript, no: &[u8], npg: &mut Vec<i32>, ty: &mut i16) -> bool {
        let precedence = &self.style.page_precedence;
        let has = |letter: u8| precedence.contains(&letter);
        let first = at(no, 0);
        let count = npg.len();
        let (roml, romu, arab, alpl, alpu, empty) =
            (ROML as i32, ROMU as i32, ARAB as i32, ALPL as i32, ALPU as i32, i32::from(EMPTY));
        let old = self.guess(count);
        let new = if first.is_ascii_digit() {
            Some(arab)
        } else if is_roman_lower(first) && has(b'r') && has(b'a') {
            let run = strspn(no, b"ivxlcdm");
            if run > 1 {
                Some(roml)
            } else if run == 1 && old != roml && old != alpl {
                Some(if strspn(no, b"ivx") == 1 { roml } else { alpl })
            } else {
                None
            }
        } else if is_roman_upper(first) && has(b'R') && has(b'A') {
            let run = strspn(no, b"IVXLCDM");
            if run > 1 {
                Some(romu)
            } else if run == 1 && old != romu && old != alpu {
                Some(if strspn(no, b"IVX") == 1 { romu } else { alpu })
            } else {
                None
            }
        } else if is_roman_lower(first) && has(b'r') {
            Some(roml)
        } else if is_roman_upper(first) && has(b'R') {
            Some(romu)
        } else if first.is_ascii_lowercase() && has(b'a') {
            Some(alpl)
        } else if first.is_ascii_uppercase() && has(b'A') {
            Some(alpu)
        } else {
            Some(empty)
        };
        if let Some(value) = new {
            self.set_guess(count, value);
        }
        let guess = self.guess(count);

        if first.is_ascii_digit() {
            *ty = ARAB as i16;
            self.scan_arabic(f, log, no, npg)
        } else if is_roman_lower(first) && has(b'r') && (!has(b'a') || guess == roml) {
            *ty = ROML as i16;
            self.scan_roman(f, log, no, npg, ROML)
        } else if is_roman_upper(first) && has(b'R') && (!has(b'A') || guess == romu) {
            *ty = ROMU as i16;
            self.scan_roman(f, log, no, npg, ROMU)
        } else if first.is_ascii_lowercase() && has(b'a') {
            *ty = ALPL as i16;
            self.scan_alpha(f, log, no, npg, ALPL)
        } else if first.is_ascii_uppercase() && has(b'A') {
            *ty = ALPU as i16;
            self.scan_alpha(f, log, no, npg, ALPU)
        } else {
            let message = [b"Illegal page number ", no, b" or page_precedence ", c_str(precedence), b".\n"].concat();
            f.error(log, &message);
            false
        }
    }

    fn is_compositor(&self, no: &[u8], index: usize) -> bool {
        tail(no, index).starts_with(&self.style.page_compositor)
    }

    /// The fields after a compositor; their types do not matter.
    fn scan_rest(&mut self, f: &mut FileScan, log: &mut Transcript, no: &[u8], npg: &mut Vec<i32>) -> bool {
        let mut ignored = 0i16;
        self.scan_no(f, log, no, npg, &mut ignored)
    }

    fn scan_arabic(&mut self, f: &mut FileScan, log: &mut Transcript, no: &[u8], npg: &mut Vec<i32>) -> bool {
        let mut i = 0usize;
        while at(no, i) != 0 && i <= ARABIC_MAX && !self.is_compositor(no, i) {
            if !at(no, i).is_ascii_digit() {
                let message =
                    [format!("Illegal Arabic digit: position {} in ", i + 1).as_bytes(), no, b".\n"].concat();
                f.error(log, &message);
                return false;
            }
            i += 1;
        }
        if i > ARABIC_MAX {
            let message = [b"Arabic page number ", no, format!(" too big (max {ARABIC_MAX} digits).\n").as_bytes()].concat();
            f.error(log, &message);
            return false;
        }
        let value = no[..i].iter().fold(0i32, |value, &digit| {
            value.wrapping_mul(10).wrapping_add(i32::from(digit)).wrapping_sub(48)
        });
        if !self.enter(f, log, no, npg, value.wrapping_add(self.style.page_offset[ARAB])) {
            return false;
        }
        if self.is_compositor(no, i) {
            let rest = tail(no, i + self.style.page_compositor.len());
            return self.scan_rest(f, log, rest, npg);
        }
        true
    }

    fn scan_roman(&mut self, f: &mut FileScan, log: &mut Transcript, no: &[u8], npg: &mut Vec<i32>, kind: usize) -> bool {
        let valid = if kind == ROML { is_roman_lower } else { is_roman_upper };
        let mut i = 0usize;
        let mut inp = 0i32;
        let mut prev = 0i32;
        while at(no, i) != 0 && i < ROMAN_MAX && !self.is_compositor(no, i) {
            let byte = at(no, i);
            if !valid(byte) {
                let message =
                    [format!("Illegal Roman number: position {} in ", i + 1).as_bytes(), no, b".\n"].concat();
                f.error(log, &message);
                return false;
            }
            let mut new = roman_value(byte);
            if prev == 0 {
                prev = new;
            } else {
                if prev < new {
                    prev = new - prev;
                    new = 0;
                }
                inp += prev;
                prev = new;
            }
            i += 1;
        }
        if i == ROMAN_MAX {
            let message = [b"Roman page number ", no, format!(" too big (max {ROMAN_MAX} digits).\n").as_bytes()].concat();
            f.error(log, &message);
            return false;
        }
        inp += prev;
        if !self.enter(f, log, no, npg, inp.wrapping_add(self.style.page_offset[kind])) {
            return false;
        }
        if self.is_compositor(no, i) {
            let rest = tail(no, i + self.style.page_compositor.len());
            return self.scan_rest(f, log, rest, npg);
        }
        true
    }

    fn scan_alpha(&mut self, f: &mut FileScan, log: &mut Transcript, no: &[u8], npg: &mut Vec<i32>, kind: usize) -> bool {
        if !self.enter(f, log, no, npg, alpha_value(at(no, 0)).wrapping_add(self.style.page_offset[kind])) {
            return false;
        }
        if self.is_compositor(no, 1) {
            let rest = tail(no, 1 + self.style.page_compositor.len());
            return self.scan_rest(f, log, rest, npg);
        }
        true
    }
}

/// `search_quote` (option `-g`): `"a`, `"o`, `"u` and `"s` sort as `ae`,
/// `oe`, `ue` and `ss`; the original text is printed. Returns the sort key
/// and the printed text (empty when nothing was replaced).
fn search_quote(key: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut sort = key.to_vec();
    let mut found = false;
    let mut at_quote = sort.iter().position(|&byte| byte == b'"');
    while let Some(index) = at_quote {
        let replacement: Option<&[u8; 2]> = match sort.get(index + 1) {
            Some(b'a') => Some(b"ae"),
            Some(b'A') => Some(b"Ae"),
            Some(b'o') => Some(b"oe"),
            Some(b'O') => Some(b"Oe"),
            Some(b'u') => Some(b"ue"),
            Some(b'U') => Some(b"Ue"),
            Some(b's') => Some(b"ss"),
            _ => None,
        };
        if let Some(pair) = replacement {
            found = true;
            sort[index] = pair[0];
            sort[index + 1] = pair[1];
        }
        at_quote = sort[index + 1..].iter().position(|&byte| byte == b'"').map(|offset| index + 1 + offset);
    }
    let actual = if found { key.to_vec() } else { Vec::new() };
    (sort, actual)
}
