//! Character classes and the line-oriented scanner shared by the `.aux`,
//! `.bst` and `.bib` readers (bibtex.web parts 3 and 7, with TeX Live's
//! 8-bit changes: bytes 128-255 are letters and legal identifier bytes,
//! carriage return is white space).

pub const WHITE: u8 = 1;
pub const ALPHA: u8 = 2;
pub const NUMERIC: u8 = 3;
pub const SEP: u8 = 4;
pub const OTHER: u8 = 5;
pub const ILLEGAL: u8 = 0;

const fn make_lex_class() -> [u8; 256] {
    let mut t = [OTHER; 256];
    let mut i = 0;
    while i < 0x20 {
        t[i] = ILLEGAL;
        i += 1;
    }
    t[0x7f] = ILLEGAL;
    t[b'\t' as usize] = WHITE;
    t[b'\r' as usize] = WHITE;
    t[b' ' as usize] = WHITE;
    t[b'~' as usize] = SEP;
    t[b'-' as usize] = SEP;
    let mut i = b'0';
    while i <= b'9' {
        t[i as usize] = NUMERIC;
        i += 1;
    }
    let mut i = 0;
    while i < 26 {
        t[b'A' as usize + i] = ALPHA;
        t[b'a' as usize + i] = ALPHA;
        i += 1;
    }
    let mut i = 0x80;
    while i < 0x100 {
        t[i] = ALPHA;
        i += 1;
    }
    t
}

const fn make_id_class() -> [bool; 256] {
    let mut t = [true; 256];
    let mut i = 0;
    while i < 0x20 {
        t[i] = false;
        i += 1;
    }
    let illegal = b" \t\"#%'(),={}";
    let mut i = 0;
    while i < illegal.len() {
        t[illegal[i] as usize] = false;
        i += 1;
    }
    t
}

static LEX_CLASS: [u8; 256] = make_lex_class();
static LEGAL_ID: [bool; 256] = make_id_class();

#[inline]
pub fn lex_class(c: u8) -> u8 {
    LEX_CLASS[c as usize]
}

#[inline]
pub fn is_white(c: u8) -> bool {
    LEX_CLASS[c as usize] == WHITE
}

#[inline]
pub fn is_alpha(c: u8) -> bool {
    LEX_CLASS[c as usize] == ALPHA
}

#[inline]
pub fn legal_id_char(c: u8) -> bool {
    LEGAL_ID[c as usize]
}

/// bibtex.web's `lower_case`: ASCII letters only.
pub fn lower_case(b: &mut [u8]) {
    b.make_ascii_lowercase();
}

/// Outcome of `scan_identifier`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IdScan {
    Null,
    SpecifiedCharAdjacent,
    OtherCharAdjacent,
    WhiteAdjacent,
}

/// A source file read one line at a time, like web2c's `input_ln`: a line
/// ends at `\n` or `\r` (so CRLF yields an extra empty line, as in TeX
/// Live) and trailing white space is dropped.
pub struct Source {
    bytes: Vec<u8>,
    pos: usize,
}

impl Source {
    pub fn new(bytes: Vec<u8>) -> Self {
        Source { bytes, pos: 0 }
    }

    pub fn at_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }
}

/// The scanning buffer: `buffer[0..last)` holds the current line,
/// `p1..p2` delimits the current token and `p2` is the scan position.
pub struct Scanner {
    pub src: Source,
    pub buf: Vec<u8>,
    pub last: usize,
    pub p1: usize,
    pub p2: usize,
    pub line: usize,
}

impl Scanner {
    pub fn new(bytes: Vec<u8>) -> Self {
        Scanner {
            src: Source::new(bytes),
            buf: vec![0],
            last: 0,
            p1: 0,
            p2: 0,
            line: 0,
        }
    }

    /// `input_ln`: load the next line into the buffer; false at end of
    /// file, where (as in web2c) `last` becomes 0 but the buffer keeps the
    /// previous line for error messages.
    pub fn input_ln(&mut self) -> bool {
        self.last = 0;
        let src = &self.src;
        if src.pos >= src.bytes.len() {
            return false;
        }
        self.buf.clear();
        let rest = &src.bytes[src.pos..];
        let len = rest
            .iter()
            .position(|&c| c == b'\n' || c == b'\r')
            .unwrap_or(rest.len());
        let mut end = len;
        while end > 0 && is_white(rest[end - 1]) {
            end -= 1;
        }
        self.buf.extend_from_slice(&rest[..end]);
        self.last = end;
        // sentinel: `scan_char` at `last` matches nothing of interest
        self.buf.push(0);
        self.src.pos += len + 1;
        true
    }

    #[inline]
    pub fn scan_char(&self) -> u8 {
        self.buf[self.p2]
    }

    #[inline]
    pub fn token(&self) -> &[u8] {
        &self.buf[self.p1..self.p2]
    }

    pub fn lower_case_token(&mut self) {
        let (p1, p2) = (self.p1, self.p2);
        lower_case(&mut self.buf[p1..p2]);
    }

    pub fn scan1(&mut self, c1: u8) -> bool {
        self.p1 = self.p2;
        while self.p2 < self.last && self.buf[self.p2] != c1 {
            self.p2 += 1;
        }
        self.p2 < self.last
    }

    pub fn scan1_white(&mut self, c1: u8) -> bool {
        self.p1 = self.p2;
        while self.p2 < self.last && !is_white(self.buf[self.p2]) && self.buf[self.p2] != c1 {
            self.p2 += 1;
        }
        self.p2 < self.last
    }

    pub fn scan2(&mut self, c1: u8, c2: u8) -> bool {
        self.p1 = self.p2;
        while self.p2 < self.last && self.buf[self.p2] != c1 && self.buf[self.p2] != c2 {
            self.p2 += 1;
        }
        self.p2 < self.last
    }

    pub fn scan2_white(&mut self, c1: u8, c2: u8) -> bool {
        self.p1 = self.p2;
        while self.p2 < self.last {
            let c = self.buf[self.p2];
            if c == c1 || c == c2 || is_white(c) {
                break;
            }
            self.p2 += 1;
        }
        self.p2 < self.last
    }

    pub fn scan3(&mut self, c1: u8, c2: u8, c3: u8) -> bool {
        self.p1 = self.p2;
        while self.p2 < self.last {
            let c = self.buf[self.p2];
            if c == c1 || c == c2 || c == c3 {
                break;
            }
            self.p2 += 1;
        }
        self.p2 < self.last
    }

    pub fn scan_alpha(&mut self) -> bool {
        self.p1 = self.p2;
        while self.p2 < self.last && is_alpha(self.buf[self.p2]) {
            self.p2 += 1;
        }
        self.p2 > self.p1
    }

    pub fn scan_identifier(&mut self, c1: u8, c2: u8, c3: u8) -> IdScan {
        self.p1 = self.p2;
        if lex_class(self.scan_char()) != NUMERIC {
            while self.p2 < self.last && legal_id_char(self.buf[self.p2]) {
                self.p2 += 1;
            }
        }
        let c = self.scan_char();
        if self.p2 == self.p1 {
            IdScan::Null
        } else if is_white(c) || self.p2 == self.last {
            IdScan::WhiteAdjacent
        } else if c == c1 || c == c2 || c == c3 {
            IdScan::SpecifiedCharAdjacent
        } else {
            IdScan::OtherCharAdjacent
        }
    }

    /// `scan_nonneg_integer`; the value wraps like a 32-bit Pascal integer.
    pub fn scan_nonneg_integer(&mut self) -> Option<i32> {
        self.p1 = self.p2;
        let mut v: i32 = 0;
        while self.p2 < self.last && lex_class(self.buf[self.p2]) == NUMERIC {
            v = v.wrapping_mul(10).wrapping_add((self.buf[self.p2] - b'0') as i32);
            self.p2 += 1;
        }
        (self.p2 > self.p1).then_some(v)
    }

    /// `scan_integer`: optional minus sign, then digits.
    pub fn scan_integer(&mut self) -> Option<i32> {
        self.p1 = self.p2;
        let negative = self.scan_char() == b'-';
        if negative {
            self.p2 += 1;
        }
        let mut v: i32 = 0;
        let digits_start = self.p2;
        while self.p2 < self.last && lex_class(self.buf[self.p2]) == NUMERIC {
            v = v.wrapping_mul(10).wrapping_add((self.buf[self.p2] - b'0') as i32);
            self.p2 += 1;
        }
        if self.p2 == digits_start {
            return None;
        }
        Some(if negative { v.wrapping_neg() } else { v })
    }

    pub fn scan_white_space(&mut self) -> bool {
        while self.p2 < self.last && is_white(self.buf[self.p2]) {
            self.p2 += 1;
        }
        self.p2 < self.last
    }

    /// `print_bad_input_line`: the line split at the scan position.
    /// Returns the text (the caller prints it and marks the error).
    pub fn bad_input_line(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 * self.last + 48);
        out.extend_from_slice(b" : ");
        for &c in &self.buf[..self.p2] {
            out.push(if is_white(c) { b' ' } else { c });
        }
        out.extend_from_slice(b"\n : ");
        out.resize(out.len() + self.p2, b' ');
        for &c in self.buf.get(self.p2..self.last).unwrap_or_default() {
            out.push(if is_white(c) { b' ' } else { c });
        }
        out.push(b'\n');
        if self.buf[..self.p2].iter().all(|&c| is_white(c)) {
            out.extend_from_slice(b"(Error may have been on previous line)\n");
        }
        out
    }
}
