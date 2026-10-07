//! Index style (`.ist`) files: makeindex's defaults and `scanst.c`.

use crate::stream::{Stream, EOF};
use crate::Transcript;

pub(crate) const ROML: usize = 0;
pub(crate) const ROMU: usize = 1;
pub(crate) const ARAB: usize = 2;
pub(crate) const ALPL: usize = 3;
pub(crate) const ALPU: usize = 4;

const ROMAN_LOWER_OFFSET: i32 = 10000;
const ROMAN_UPPER_OFFSET: i32 = 10000;
const ARABIC_OFFSET: i32 = 10000;
const ALPHA_LOWER_OFFSET: i32 = 26;
const ALPHA_UPPER_OFFSET: i32 = 26;

/// `ARRAY_MAX`: the longest string a style attribute may hold.
const ARRAY_MAX: usize = 1024;
/// `STRING_MAX`: the longest specifier name.
const STRING_MAX: usize = 999;
const PAGETYPE_MAX: usize = 5;

/// Every attribute makeindex knows, with TeX Live's defaults, plus the line
/// counts makeindex keeps for the multi-line ones.
#[derive(Clone, Debug)]
pub struct Style {
    pub keyword: Vec<u8>,
    pub arg_open: u8,
    pub arg_close: u8,
    pub range_open: u8,
    pub range_close: u8,
    pub level: u8,
    pub actual: u8,
    pub encap: u8,
    pub quote: u8,
    pub escape: u8,
    pub page_compositor: Vec<u8>,
    pub page_precedence: Vec<u8>,
    /// Value added to each numeral type (`ROML` .. `ALPU`) when ordering.
    pub(crate) page_offset: [i32; PAGETYPE_MAX],
    pub preamble: Vec<u8>,
    pub postamble: Vec<u8>,
    pub(crate) prelen: usize,
    pub(crate) postlen: usize,
    pub setpage_prefix: Vec<u8>,
    pub setpage_suffix: Vec<u8>,
    pub(crate) setpagelen: usize,
    pub group_skip: Vec<u8>,
    pub(crate) skiplen: usize,
    pub headings_flag: i32,
    pub heading_prefix: Vec<u8>,
    pub heading_suffix: Vec<u8>,
    pub(crate) headprelen: usize,
    pub(crate) headsuflen: usize,
    pub symhead_positive: Vec<u8>,
    pub symhead_negative: Vec<u8>,
    pub numhead_positive: Vec<u8>,
    pub numhead_negative: Vec<u8>,
    /// `item_0`, `item_1`, `item_2`.
    pub(crate) item_r: [Vec<u8>; 3],
    /// `item_01`, `item_12` (index 0 unused).
    pub(crate) item_u: [Vec<u8>; 3],
    /// `item_x1`, `item_x2` (index 0 unused).
    pub(crate) item_x: [Vec<u8>; 3],
    pub(crate) ilen_r: [usize; 3],
    pub(crate) ilen_u: [usize; 3],
    pub(crate) ilen_x: [usize; 3],
    /// `delim_0`, `delim_1`, `delim_2`.
    pub(crate) delim_p: [Vec<u8>; 3],
    pub delim_n: Vec<u8>,
    pub delim_r: Vec<u8>,
    pub delim_t: Vec<u8>,
    pub suffix_2p: Vec<u8>,
    pub suffix_3p: Vec<u8>,
    pub suffix_mp: Vec<u8>,
    pub encap_prefix: Vec<u8>,
    pub encap_infix: Vec<u8>,
    pub encap_suffix: Vec<u8>,
    pub line_max: i32,
    pub indent_space: Vec<u8>,
    pub indent_length: i32,
}

impl Default for Style {
    fn default() -> Self {
        let s = |text: &str| text.as_bytes().to_vec();
        let item0 = s("\n  \\item ");
        let item1 = s("\n    \\subitem ");
        let item2 = s("\n      \\subsubitem ");
        Style {
            keyword: s("\\indexentry"),
            arg_open: b'{',
            arg_close: b'}',
            range_open: b'(',
            range_close: b')',
            level: b'!',
            actual: b'@',
            encap: b'|',
            quote: b'"',
            escape: b'\\',
            page_compositor: s("-"),
            page_precedence: s("rnaRA"),
            page_offset: [
                0,
                ROMAN_LOWER_OFFSET,
                ROMAN_LOWER_OFFSET + ARABIC_OFFSET,
                ROMAN_LOWER_OFFSET + ARABIC_OFFSET + ALPHA_LOWER_OFFSET,
                ROMAN_LOWER_OFFSET + ARABIC_OFFSET + ALPHA_LOWER_OFFSET + ROMAN_UPPER_OFFSET,
            ],
            preamble: s("\\begin{theindex}\n"),
            postamble: s("\n\n\\end{theindex}\n"),
            prelen: 1,
            postlen: 3,
            setpage_prefix: s("\n  \\setcounter{page}{"),
            setpage_suffix: s("}\n"),
            setpagelen: 2,
            group_skip: s("\n\n  \\indexspace\n"),
            skiplen: 3,
            headings_flag: 0,
            heading_prefix: Vec::new(),
            heading_suffix: Vec::new(),
            headprelen: 0,
            headsuflen: 0,
            symhead_positive: s("Symbols"),
            symhead_negative: s("symbols"),
            numhead_positive: s("Numbers"),
            numhead_negative: s("numbers"),
            item_r: [item0, item1.clone(), item2.clone()],
            item_u: [Vec::new(), item1.clone(), item2.clone()],
            item_x: [Vec::new(), item1, item2],
            ilen_r: [1, 1, 1],
            ilen_u: [0, 1, 1],
            ilen_x: [0, 1, 1],
            delim_p: [s(", "), s(", "), s(", ")],
            delim_n: s(", "),
            delim_r: s("--"),
            delim_t: Vec::new(),
            suffix_2p: Vec::new(),
            suffix_3p: Vec::new(),
            suffix_mp: Vec::new(),
            encap_prefix: s("\\"),
            encap_infix: s("{"),
            encap_suffix: s("}"),
            line_max: 72,
            indent_space: s("\t\t"),
            indent_length: 16,
        }
    }
}

fn count_lfd(text: &[u8]) -> usize {
    text.iter().filter(|&&byte| byte == b'\n').count()
}

/// Reads `%d` the way `fscanf` does: blanks (newlines too, which the style
/// line count never sees), an optional sign, digits. On a mismatch the target
/// keeps its value.
fn scan_number(stream: &mut Stream) -> Option<i32> {
    while stream.peek_raw().is_some_and(|byte| byte.is_ascii_whitespace() || byte == 0x0b) {
        stream.raw_getc();
    }
    let mut negative = false;
    if let Some(sign @ (b'+' | b'-')) = stream.peek_raw() {
        negative = sign == b'-';
        stream.raw_getc();
    }
    let mut value: i64 = 0;
    let mut digits = 0;
    while let Some(byte) = stream.peek_raw().filter(u8::is_ascii_digit) {
        stream.raw_getc();
        value = value.saturating_mul(10).saturating_add(i64::from(byte - b'0'));
        digits += 1;
    }
    if digits == 0 {
        return None;
    }
    let value = if negative { -value } else { value };
    Some(value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
}

/// State of one `scan_sty` run.
struct StyleScanner<'a, 'b> {
    stream: Stream<'a>,
    file: &'a str,
    lc: usize,
    tc: usize,
    ec: usize,
    put_dot: bool,
    /// `scan_string`'s buffer, which keeps its contents between calls.
    clone: Vec<u8>,
    log: &'b mut Transcript,
}

enum Spec {
    Done,
    Found(Vec<u8>),
    /// `scan_spec` returned -1 (premature end): the caller still processes it.
    Premature(Vec<u8>),
}

impl StyleScanner<'_, '_> {
    fn error(&mut self, message: &str) {
        self.error_bytes(message.as_bytes());
    }

    fn error_bytes(&mut self, message: &[u8]) {
        self.log.error_line();
        self.log.ilg(format!("** Input style error (file = {}, line = {}):\n   -- ", self.file, self.lc).as_bytes());
        self.log.ilg(message);
        self.ec += 1;
        self.put_dot = false;
    }

    fn skip_line(&mut self) {
        loop {
            let c = self.stream.getc();
            if c == i32::from(b'\n') || c == EOF {
                break;
            }
        }
        self.lc += 1;
    }

    fn next_nonblank(&mut self) -> i32 {
        loop {
            let c = self.stream.getc();
            match c {
                EOF => return EOF,
                0x0a => self.lc += 1,
                0x20 | 0x09 => {}
                _ => return c,
            }
        }
    }

    fn scan_spec(&mut self) -> Spec {
        let mut c;
        loop {
            c = self.next_nonblank();
            if c == EOF {
                return Spec::Done;
            } else if c == i32::from(b'%') {
                self.skip_line();
            } else {
                break;
            }
        }
        let mut spec = vec![(c as u8).to_ascii_lowercase()];
        let mut i = 0usize;
        loop {
            let within = i < STRING_MAX;
            i += 1;
            if !within {
                break;
            }
            c = self.stream.getc();
            if c == 0x20 || c == 0x09 || c == 0x0a || c == EOF {
                break;
            }
            spec.push((c as u8).to_ascii_lowercase());
        }
        if i < STRING_MAX {
            if c == EOF {
                let message = [&b"No attribute for specifier "[..], &spec, b" (premature EOF)\n"].concat();
                self.error_bytes(&message);
                return Spec::Premature(spec);
            }
            if c == 0x0a {
                self.lc += 1;
            }
            Spec::Found(spec)
        } else {
            let message = [&b"Specifier "[..], &spec, format!(" too long (max {STRING_MAX}).\n").as_bytes()].concat();
            self.error_bytes(&message);
            Spec::Done
        }
    }

    /// `scan_string`: a `"..."` value; `target` keeps its value on errors.
    fn scan_string(&mut self, target: &mut Vec<u8>) -> bool {
        let c = self.next_nonblank();
        if c == i32::from(b'"') {
            let mut i = 0usize;
            loop {
                let c = self.stream.getc();
                let byte = if c == EOF {
                    // The message shows makeindex's buffer, which still holds
                    // the tail of the previous string past what was read.
                    let message = [&b"No closing delimiter in "[..], self.clone_text(), b".\n"].concat();
                    self.error_bytes(&message);
                    return false;
                } else if c == i32::from(b'"') {
                    self.store(i, 0);
                    *target = self.clone[..i].to_vec();
                    return true;
                } else if c == i32::from(b'\\') {
                    match self.stream.getc() {
                        0x74 => b'\t',
                        0x6e => b'\n',
                        other => other as u8,
                    }
                } else {
                    if c == 0x0a {
                        self.lc += 1;
                    }
                    if i >= ARRAY_MAX {
                        self.skip_line();
                        let message = [
                            &b"Attribute string "[..],
                            self.clone_text(),
                            format!(" too long (max {ARRAY_MAX}).\n").as_bytes(),
                        ]
                        .concat();
                        self.error_bytes(&message);
                        return false;
                    }
                    c as u8
                };
                self.store(i, byte);
                i += 1;
            }
        } else if c == i32::from(b'%') {
            self.skip_line();
            true
        } else {
            self.skip_line();
            self.error("No opening delimiter.\n");
            false
        }
    }

    fn store(&mut self, index: usize, byte: u8) {
        if self.clone.len() <= index {
            self.clone.resize(index + 1, 0);
        }
        self.clone[index] = byte;
    }

    /// The string buffer up to its first NUL.
    fn clone_text(&self) -> &[u8] {
        let end = self.clone.iter().position(|&byte| byte == 0).unwrap_or(self.clone.len());
        &self.clone[..end]
    }

    /// `scan_char`: a `'c'` value.
    fn scan_char(&mut self, target: &mut u8) {
        let c = self.next_nonblank();
        if c == i32::from(b'\'') {
            let mut clone = self.stream.getc();
            match clone {
                0x27 => {
                    self.skip_line();
                    self.error("Premature closing delimiter.\n");
                    return;
                }
                0x0a | EOF => {
                    if clone == 0x0a {
                        self.lc += 1;
                    }
                    self.error("No character (premature EOF).\n");
                    return;
                }
                0x5c => clone = self.stream.getc(),
                _ => {}
            }
            if self.stream.getc() == i32::from(b'\'') {
                *target = clone as u8;
            } else {
                self.error("No closing delimiter or too many letters.\n");
            }
        } else if c == i32::from(b'%') {
            self.skip_line();
        } else {
            self.skip_line();
            self.error("No opening delimiter.\n");
        }
    }
}

impl Style {
    /// makeindex's `scan_sty`: applies the style file `bytes` (named `file`
    /// in messages), reporting to the transcript.
    pub(crate) fn scan(&mut self, bytes: &[u8], file: &str, log: &mut Transcript) {
        log.message(format!("Scanning style file {file}").as_bytes());
        let mut scanner =
            StyleScanner { stream: Stream::new(bytes), file, lc: 0, tc: 0, ec: 0, put_dot: false, clone: Vec::new(), log };
        loop {
            let spec = match scanner.scan_spec() {
                Spec::Done => break,
                Spec::Found(spec) | Spec::Premature(spec) => spec,
            };
            scanner.tc += 1;
            scanner.put_dot = true;
            self.apply(&spec, &mut scanner);
            if scanner.put_dot {
                scanner.log.idx_dot = true;
                scanner.log.message(b".");
            }
        }
        self.process_precedence(&mut scanner);
        if self.quote == self.escape {
            let message = [&b"Quote and escape symbols must be distinct (both `"[..], &[self.quote], b"' now).\n"].concat();
            scanner.error_bytes(&message);
            self.quote = b'"';
            self.escape = b'\\';
        }
        let (redefined, ignored) = (scanner.tc - scanner.ec, scanner.ec);
        scanner
            .log
            .message(format!("done ({redefined} attributes redefined, {ignored} ignored).\n").as_bytes());
    }

    fn apply(&mut self, spec: &[u8], scanner: &mut StyleScanner) {
        macro_rules! string {
            ($field:expr) => {{
                scanner.scan_string(&mut $field);
            }};
            ($field:expr, $count:expr) => {{
                scanner.scan_string(&mut $field);
                $count = count_lfd(&$field);
            }};
        }
        match spec {
            b"preamble" => string!(self.preamble, self.prelen),
            b"postamble" => string!(self.postamble, self.postlen),
            b"group_skip" => string!(self.group_skip, self.skiplen),
            b"headings_flag" => {
                if let Some(value) = scan_number(&mut scanner.stream) {
                    self.headings_flag = value;
                }
            }
            b"heading_prefix" => string!(self.heading_prefix, self.headprelen),
            b"heading_suffix" => string!(self.heading_suffix, self.headsuflen),
            b"symhead_positive" => string!(self.symhead_positive),
            b"symhead_negative" => string!(self.symhead_negative),
            b"numhead_positive" => string!(self.numhead_positive),
            b"numhead_negative" => string!(self.numhead_negative),
            b"setpage_prefix" => string!(self.setpage_prefix, self.setpagelen),
            b"setpage_suffix" => string!(self.setpage_suffix, self.setpagelen),
            b"item_0" => string!(self.item_r[0], self.ilen_r[0]),
            b"item_1" => string!(self.item_r[1], self.ilen_r[1]),
            b"item_2" => string!(self.item_r[2], self.ilen_r[2]),
            b"item_01" => string!(self.item_u[1], self.ilen_u[1]),
            b"item_12" => string!(self.item_u[2], self.ilen_u[2]),
            b"item_x1" => string!(self.item_x[1], self.ilen_x[1]),
            b"item_x2" => string!(self.item_x[2], self.ilen_x[2]),
            b"encap_prefix" => string!(self.encap_prefix),
            b"encap_infix" => string!(self.encap_infix),
            b"encap_suffix" => string!(self.encap_suffix),
            b"delim_0" => string!(self.delim_p[0]),
            b"delim_1" => string!(self.delim_p[1]),
            b"delim_2" => string!(self.delim_p[2]),
            b"delim_n" => string!(self.delim_n),
            b"delim_r" => string!(self.delim_r),
            b"delim_t" => string!(self.delim_t),
            b"suffix_2p" => string!(self.suffix_2p),
            b"suffix_3p" => string!(self.suffix_3p),
            b"suffix_mp" => string!(self.suffix_mp),
            b"line_max" => {
                // An unreadable number leaves fscanf's target uninitialized;
                // makeindex's stack happens to hold the previous value.
                let value = scan_number(&mut scanner.stream).unwrap_or(self.line_max);
                if value > 0 {
                    self.line_max = value;
                } else {
                    scanner.error(&format!("line_max must be positive (got {value})"));
                }
            }
            b"indent_length" => {
                let value = scan_number(&mut scanner.stream).unwrap_or(self.indent_length);
                if value >= 0 {
                    self.indent_length = value;
                } else {
                    scanner.error(&format!("indent_length must be nonnegative (got {value})"));
                }
            }
            b"indent_space" => string!(self.indent_space),
            b"page_compositor" => string!(self.page_compositor),
            b"page_precedence" => string!(self.page_precedence),
            b"keyword" => string!(self.keyword),
            b"arg_open" => scanner.scan_char(&mut self.arg_open),
            b"arg_close" => scanner.scan_char(&mut self.arg_close),
            b"level" => scanner.scan_char(&mut self.level),
            b"range_open" => scanner.scan_char(&mut self.range_open),
            b"range_close" => scanner.scan_char(&mut self.range_close),
            b"quote" => scanner.scan_char(&mut self.quote),
            b"actual" => scanner.scan_char(&mut self.actual),
            b"encap" => scanner.scan_char(&mut self.encap),
            b"escape" => scanner.scan_char(&mut self.escape),
            _ => {
                scanner.next_nonblank();
                scanner.skip_line();
                let message = [&b"Unknown specifier "[..], spec, b".\n"].concat();
                scanner.error_bytes(&message);
                scanner.put_dot = false;
            }
        }
    }

    /// `process_precedence`: lays the numeral types out in the order of
    /// `page_precedence`.
    fn process_precedence(&mut self, scanner: &mut StyleScanner) {
        let precedence = self.page_precedence.clone();
        let mut seen = [false; PAGETYPE_MAX];
        let mut i = 0;
        while i < PAGETYPE_MAX && i < precedence.len() {
            let letter = precedence[i];
            let (kind, reported) = match letter {
                b'r' => (ROML, b'r'),
                b'R' => (ROMU, b'R'),
                b'n' => (ARAB, b'n'),
                // makeindex names the upper-case type for a repeated `a`.
                b'a' => (ALPL, b'A'),
                b'A' => (ALPU, b'A'),
                other => {
                    scanner.skip_line();
                    let message = [&b"Unknow type `"[..], &[other], b"' in page precedence specification.\n"].concat();
                    scanner.error_bytes(&message);
                    return;
                }
            };
            if seen[kind] {
                scanner.skip_line();
                let message = [
                    &b"Multiple instances of type `"[..],
                    &[reported],
                    b"' in page precedence specification `",
                    &precedence,
                    b"'.\n",
                ]
                .concat();
                scanner.error_bytes(&message);
                return;
            }
            seen[kind] = true;
            i += 1;
        }
        if i < precedence.len() {
            scanner.skip_line();
            scanner.error("Page precedence specification string too long.\n");
            return;
        }
        let mut last = i;
        if last == 0 {
            // makeindex reads uninitialized slots here; keep the offsets.
            return;
        }
        let kind_of = |letter: u8| match letter {
            b'r' => ROML,
            b'R' => ROMU,
            b'n' => ARAB,
            b'a' => ALPL,
            _ => ALPU,
        };
        let width_after = |kind: usize| match kind {
            ROML => ROMAN_LOWER_OFFSET,
            ROMU => ROMAN_UPPER_OFFSET,
            ARAB => ARABIC_OFFSET,
            // Both alphabetic types advance by the lower-case width when
            // listed; only the fill-in below uses ALPHA_UPPER_OFFSET.
            _ => ALPHA_LOWER_OFFSET,
        };
        let mut order = [0i32; PAGETYPE_MAX + 1];
        let mut kinds = [0usize; PAGETYPE_MAX + 1];
        kinds[0] = kind_of(precedence[0]);
        order[0] = width_after(kinds[0]);
        for index in 1..last {
            kinds[index] = kind_of(precedence[index]);
            order[index] = order[index - 1] + width_after(kinds[index]);
        }
        let mut offset = [-1i32; PAGETYPE_MAX];
        offset[kinds[0]] = 0;
        for index in 1..last {
            offset[kinds[index]] = order[index - 1];
        }
        for (kind, slot) in offset.iter_mut().enumerate() {
            if *slot == -1 {
                let width = match kinds[last - 1] {
                    ROML => ROMAN_LOWER_OFFSET,
                    ROMU => ROMAN_UPPER_OFFSET,
                    ARAB => ARABIC_OFFSET,
                    ALPL => ALPHA_LOWER_OFFSET,
                    _ => ALPHA_UPPER_OFFSET,
                };
                order[last] = order[last - 1] + width;
                kinds[last] = kind;
                *slot = order[last];
                last += 1;
            }
        }
        self.page_offset = offset;
    }
}
