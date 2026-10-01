//! tex.web §57-§71 printing to the terminal and the transcript: the
//! `term_offset`/`file_offset` column counters, `max_print_line` wrapping,
//! `print_nl`/`print_ln`, the `\newlinechar` rule and `^^` notation, all on
//! bytes (an 8-bit character counts as the 1, 3 or 4 bytes it prints as).
//!
//! Only text that TeX itself prints goes through here (`\message`, `\write`
//! to the terminal and log, box displays). Ratex's structured diagnostics
//! are free-form blocks that go through `append_log`/`append_term`, which
//! merely keep the column counters in step.

use crate::engine::{Engine, InteractionMode};
use crate::prim::IntParam;
use crate::tex_bytes::{bytes_to_text, push_printable, Xprn};

/// tex.web `max_print_line` (texmf.cnf default).
pub(crate) const MAX_PRINT_LINE: usize = 79;

/// One output stream's pending text and column.
pub(crate) struct Lane {
    pub(crate) out: Vec<u8>,
    pub(crate) offset: usize,
}

impl Lane {
    fn new(offset: usize) -> Lane {
        Lane { out: Vec::new(), offset }
    }

    /// tex.web print_char for a byte that is not the new-line character.
    fn put(&mut self, byte: u8) {
        self.out.push(byte);
        self.offset += 1;
        if self.offset == MAX_PRINT_LINE {
            self.out.push(b'\n');
            self.offset = 0;
        }
    }

    /// tex.web print_ln.
    pub(crate) fn ln(&mut self) {
        self.out.push(b'\n');
        self.offset = 0;
    }

    /// tex.web print for a fixed string: print_char for each byte, so the
    /// new-line character (`nl`) ends the line and `\n` stands for print_ln.
    pub(crate) fn text(&mut self, s: &str, nl: i32) {
        for &byte in s.as_bytes() {
            if byte == b'\n' || i32::from(byte) == nl {
                self.ln();
            } else {
                self.put(byte);
            }
        }
    }

    /// tex.web print(c) for each character code of `raw`: the new-line
    /// character ends the line, any other character prints as itself or in
    /// `^^` notation according to `xprn`.
    pub(crate) fn chars(&mut self, raw: &[u8], xprn: &Xprn, nl: i32) {
        let mut shown = Vec::with_capacity(4);
        for &byte in raw {
            if i32::from(byte) == nl {
                self.ln();
                continue;
            }
            shown.clear();
            push_printable(xprn, &mut shown, &[byte]);
            for &b in &shown {
                self.put(b);
            }
        }
    }

    /// Text already in its printed form (a box display): `\n` is print_ln.
    pub(crate) fn printed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                self.ln();
            } else {
                self.put(byte);
            }
        }
    }
}

impl Engine {
    pub(crate) fn new_line_char(&self) -> i32 {
        self.eqtb.int_params[IntParam::NewLineChar.idx() as usize]
    }

    /// Run `body` once per selected stream with that stream's column and
    /// append what it printed. `term` is ignored in batch mode (tex.web
    /// selector `log_only`).
    pub(crate) fn print_to(&mut self, term: bool, log: bool, body: &dyn Fn(&mut Lane)) {
        if term && self.interaction_mode != InteractionMode::Batch {
            let mut lane = Lane::new(self.term_offset);
            body(&mut lane);
            self.term_pad = false;
            self.append_term(&bytes_to_text(&lane.out));
            self.term_offset = lane.offset;
        }
        if log {
            let mut lane = Lane::new(self.file_offset);
            body(&mut lane);
            self.log_pad = false;
            self.append_log(&bytes_to_text(&lane.out));
            self.file_offset = lane.offset;
        }
    }

    /// tex.web print_nl(""): start a fresh line on every selected stream
    /// unless all of them are at the start of one.
    pub(crate) fn tex_print_nl(&mut self, term: bool, log: bool) {
        let term_active = term && self.interaction_mode != InteractionMode::Batch;
        if (term_active && self.term_offset > 0) || (log && self.file_offset > 0) {
            self.print_to(term, log, &|lane| lane.ln());
        }
    }

    /// tex.web print_ln.
    pub(crate) fn tex_print_ln(&mut self, term: bool, log: bool) {
        self.print_to(term, log, &|lane| lane.ln());
    }

    /// tex.web print for a fixed string.
    pub(crate) fn tex_print_str(&mut self, term: bool, log: bool, s: &str) {
        let nl = self.new_line_char();
        self.print_to(term, log, &|lane| lane.text(s, nl));
    }

    /// tex.web slow_print for the characters `raw` (one byte per character).
    pub(crate) fn tex_print_chars(&mut self, term: bool, log: bool, raw: &[u8]) {
        let nl = self.new_line_char();
        let xprn = self.xprn;
        self.print_to(term, log, &|lane| lane.chars(raw, &xprn, nl));
    }

    /// Text built in its printed form (box displays and their headers).
    pub(crate) fn tex_print_printed(&mut self, term: bool, log: bool, bytes: &[u8]) {
        self.print_to(term, log, &|lane| lane.printed(bytes));
    }

    /// tex.web §1280 issue_message for `\message`. web2c builds the message
    /// string with `message_printing` set, so characters are already in
    /// `^^` notation (only the new-line character stays itself) when its
    /// length is compared with the line width.
    pub(crate) fn tex_message(&mut self, raw: &[u8]) {
        let nl = self.new_line_char();
        let mut s = Vec::with_capacity(raw.len());
        for &byte in raw {
            if i32::from(byte) == nl {
                s.push(byte);
            } else {
                push_printable(&self.xprn, &mut s, &[byte]);
            }
        }
        let term_active = self.interaction_mode != InteractionMode::Batch;
        let term_offset = if term_active { self.term_offset } else { 0 };
        if term_offset + s.len() > MAX_PRINT_LINE - 2 {
            self.tex_print_ln(true, true);
        } else if term_offset > 0 || self.file_offset > 0 {
            self.tex_print_str(true, true, " ");
        }
        self.tex_print_chars(true, true, &s);
    }
}

/// Keep a column counter in step with text appended without TeX's wrapping.
pub(crate) fn advance_offset(offset: usize, text: &str) -> usize {
    match text.as_bytes().iter().rposition(|&b| b == b'\n') {
        Some(newline) => crate::tex_bytes::printed_len(&text[newline + 1..]),
        None => offset + crate::tex_bytes::printed_len(text),
    }
}

impl Engine {
    /// tex.web §537 start_input: `(name`, after a separating space or a
    /// line break. Ratex keeps the space after the name pending until more
    /// text follows, so that a following `print_nl` does not leave it dangling.
    pub(crate) fn print_file_open(&mut self, name: &[u8]) {
        let term_offset = if self.interaction_mode == InteractionMode::Batch {
            0
        } else {
            self.term_offset
        };
        if term_offset + name.len() > MAX_PRINT_LINE - 2 {
            self.tex_print_ln(true, true);
        } else if term_offset > 0 || self.file_offset > 0 {
            self.tex_print_str(true, true, " ");
        }
        self.tex_print_str(true, true, "(");
        self.tex_print_chars(true, true, name);
        self.term_pad = self.interaction_mode != InteractionMode::Batch;
        self.log_pad = true;
    }

    /// web2c `xchr`: the external codes for internal bytes written to a
    /// file, the terminal or the transcript.
    pub(crate) fn to_external(&self, bytes: &mut [u8]) {
        if let Some(tcx) = &self.tcx {
            for byte in bytes {
                *byte = tcx.xchr[usize::from(*byte)];
            }
        }
    }
}

impl Engine {
    /// web2c `-translate-file`: the TCX tables replace the compiled-in
    /// (or format-dumped) ones.
    pub fn set_tcx(&mut self, tcx: crate::tex_bytes::Tcx) {
        self.xprn = tcx.xprn;
        self.tcx = if tcx.is_identity() { None } else { Some(Box::new(tcx)) };
    }

    /// web2c `-8bit`: every byte prints as itself.
    pub fn set_eight_bit(&mut self) {
        self.xprn = crate::tex_bytes::eight_bit_xprn();
    }

    /// web2c `xord`: translate text read from a file to internal codes.
    pub(crate) fn from_external(&self, data: std::rc::Rc<[u8]>) -> std::rc::Rc<[u8]> {
        match &self.tcx {
            Some(tcx) => data.iter().map(|&b| tcx.xord[usize::from(b)]).collect(),
            None => data,
        }
    }
}

impl Engine {
    /// tex.web §1374: the transcript line announcing an opened `\write`
    /// file, `\openout3 = `name'.`, followed by a blank line. The terminal
    /// sees it only when `\tracingonline>0`.
    pub(crate) fn print_openout_note(&mut self, stream: u16, full: &str) {
        let mut name = full;
        let roots = [
            self.aux_dir.as_ref().map(|dir| dir.to_string_lossy().into_owned()),
            Some(self.out_dir.clone()),
        ];
        for root in roots.iter().flatten() {
            let root = root.trim_end_matches('/');
            if let Some(rest) = full.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
                if !root.is_empty() {
                    name = rest;
                    break;
                }
            }
        }
        let raw = crate::tex_bytes::text_to_bytes(name);
        let mut shown = Vec::with_capacity(raw.len() + 2);
        // web2c print_file_name quotes a name that contains a space
        let quoted = raw.contains(&b' ');
        if quoted {
            shown.push(b'"');
        }
        shown.extend_from_slice(&raw);
        if quoted {
            shown.push(b'"');
        }
        let term = self.diagnostic_to_term();
        self.tex_print_nl(term, true);
        self.tex_print_str(term, true, &format!("\\openout{stream} = `"));
        self.tex_print_chars(term, true, &shown);
        self.tex_print_str(term, true, "'.");
        self.tex_print_nl(term, true);
        self.tex_print_ln(term, true);
    }
}
