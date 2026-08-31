//! .bbl output buffer with bibtex's exact 79-column line-breaking.

use crate::classes::is_white;

pub const MAX_PRINT_LINE: usize = 79;
pub const MIN_PRINT_LINE: usize = 3;

pub struct OutBuf {
    buf: Vec<u8>,
    pub lines: Vec<String>,
}

impl OutBuf {
    pub fn new() -> Self {
        OutBuf {
            buf: Vec::new(),
            lines: Vec::new(),
        }
    }

    /// output_bbl_line: flush the current buffer as one line (trailing
    /// whitespace removed; a blank line is written if the buffer is empty).
    pub fn flush_line(&mut self) {
        let mut len = self.buf.len();
        while len > 0 && is_white(self.buf[len - 1]) {
            len -= 1;
        }
        self.lines
            .push(String::from_utf8_lossy(&self.buf[..len]).into_owned());
        self.buf.clear();
    }

    /// add_out_pool: append a string, breaking lines per bibtex.web.
    pub fn add_out_pool(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
        let mut unbreakable_tail = false;
        while self.buf.len() > MAX_PRINT_LINE && !unbreakable_tail {
            unbreakable_tail = !self.break_line();
        }
    }

    /// One "Break that line" iteration. Returns false if no viable break
    /// point was found (unbreakable tail).
    fn break_line(&mut self) -> bool {
        let end_ptr = self.buf.len();
        // scan backwards from out_buf[max_print_line] to min_print_line
        let mut p = MAX_PRINT_LINE;
        while !is_white(self.buf[p]) && p >= MIN_PRINT_LINE {
            p -= 1;
        }
        if p == MIN_PRINT_LINE - 1 {
            // no white_space in [min..max]: scan forward from max+1
            p = MAX_PRINT_LINE + 1;
            while p < end_ptr {
                if is_white(self.buf[p]) {
                    // point at the last of consecutive white_space
                    while p + 1 < end_ptr && is_white(self.buf[p + 1]) {
                        p += 1;
                    }
                    break;
                }
                p += 1;
            }
            if p == end_ptr {
                return false; // unbreakable tail
            }
        }
        // break at p: line is [0..p), the whitespace at p is dropped,
        // the rest slides down after a two-space indent
        let head: Vec<u8> = self.buf[..p].to_vec();
        let tail: Vec<u8> = self.buf[p + 1..end_ptr].to_vec();
        self.buf = head;
        self.flush_line();
        let mut next = vec![b' ', b' '];
        next.extend_from_slice(&tail);
        self.buf = next;
        true
    }

    /// Finish and return the full file contents.
    pub fn finish(mut self) -> String {
        if !self.buf.is_empty() {
            self.flush_line();
        }
        let mut s = String::new();
        for l in &self.lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    }
}
