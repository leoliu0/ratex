//! XeTeX-only transcript behaviour that TeX Live's `xetex` adds to tex.web:
//! `\tracingstacklevels` (the `~`/`.` prefixes of `\tracingmacros` lines) and
//! `\showstream` (the `\show…` family writing to an open `\write` stream).
//! Both are plain parameters of the XeTeX table; Ratex does not give them to
//! any other engine (pdfTeX does not define them).

use crate::engine::{Engine, EngineKind};
use crate::eqtb::{Equiv, Macro};
use crate::prim::IntParam;
use crate::token::Token;

type CsId = u32;

impl Engine {
    fn xt_int(&self, p: IntParam) -> i64 {
        i64::from(self.eqtb.int_params[p.idx() as usize])
    }

    /// tex.web's `input_ptr` when `invocation` has just been read: the saved
    /// input levels (the file being read is level 1; a finished token list
    /// stays until the next fetch; every pending backed-up token, and the
    /// token itself when it came back from there, is a level of its own).
    /// Ratex's `<endoutput>` marker is not a level of tex.web's; its
    /// `<write>` list (`{` text `}` and a sentinel) is one while the text
    /// has tokens left or the list is the one being read, and the `\write`
    /// private stacks are levels too.
    fn tex_input_ptr(&self, invocation: CsId) -> i64 {
        use crate::input::Source;
        let levels = |stack: &[Source]| {
            stack
                .iter()
                .enumerate()
                .filter(|(i, s)| match s {
                    Source::TokList { name: "<endoutput>", .. } => false,
                    Source::TokList { name: "<write>", toks, pos, .. } => {
                        *i + 1 == stack.len() || *pos + 2 < toks.len()
                    }
                    _ => true,
                })
                .count() as i64
        };
        let mut n = levels(&self.input.stack) + self.pushed.len() as i64;
        for (parked, parked_pushed) in &self.parked_inputs {
            n += levels(parked) + parked_pushed.len() as i64;
        }
        if self.write_mode_zero {
            n += 1;
        }
        let read = self.pushed_read;
        let from_pushed = if read.is_cs() {
            read.cs_id() == invocation
        } else {
            read.0 != 0 && read.cc() == 13 && self.active_cs_lookup(read.chr()) == Some(invocation)
        };
        n + i64::from(from_pushed)
    }

    /// tex.web `print_esc` prefix.
    fn push_escape(&self, out: &mut Vec<u8>) {
        let esc = self.xt_int(IntParam::EscapeChar);
        if (0..256).contains(&esc) {
            crate::tex_bytes::push_printable(&self.xprn, out, &[esc as u8]);
        }
    }

    /// tex.web `print_cs` (a space follows a name that could run into the
    /// next letter).
    fn push_print_cs(&self, out: &mut Vec<u8>, id: CsId, trailing_space: bool) {
        let name = self.cs.name(id);
        if let Some(c) = Engine::active_cs_scalar(name) {
            match u8::try_from(c) {
                Ok(b) => crate::tex_bytes::push_printable(&self.xprn, out, &[b]),
                Err(_) => {
                    let mut buf = [0u8; 4];
                    if let Some(ch) = char::from_u32(c) {
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                }
            }
            return;
        }
        self.push_escape(out);
        if name.is_empty() {
            out.extend_from_slice(b"csname");
            self.push_escape(out);
            out.extend_from_slice(b"endcsname");
            if trailing_space {
                out.push(b' ');
            }
            return;
        }
        crate::tex_bytes::push_printable(&self.xprn, out, name);
        let mut scalars = String::from_utf8_lossy(name).chars().collect::<Vec<_>>().into_iter();
        let first = scalars.next();
        let single = scalars.next().is_none();
        if trailing_space
            && (!single || first.is_some_and(|c| self.eqtb.cat_code(u32::from(c)) == 11))
        {
            out.push(b' ');
        }
    }

    /// `token_show(ref_count)`: the parameter text, `->` and the body.
    fn push_macro_text(&self, out: &mut Vec<u8>, m: &Macro) {
        let mut raw = Vec::new();
        raw.extend_from_slice(&self.tokens_to_bytes(&m.prefix));
        for (i, delimiter) in m.params.iter().enumerate().take(usize::from(m.num_params)) {
            raw.push(b'#');
            raw.extend_from_slice((i + 1).to_string().as_bytes());
            raw.extend_from_slice(&self.tokens_to_bytes(delimiter));
        }
        raw.extend_from_slice(b"->");
        raw.extend_from_slice(&self.tokens_to_bytes(&m.body));
        crate::tex_bytes::push_printable(&self.xprn, out, &raw);
    }

    /// `\tracingmacros>0` is traced by XeTeX only.
    #[inline(always)]
    pub(crate) fn xetex_macro_trace(&self) -> bool {
        self.engine_kind == EngineKind::XeTeX
            && self.eqtb.int_params[IntParam::TracingMacros.idx() as usize] > 0
    }

    /// tex.web §401 "Show the text of the macro being expanded", with the
    /// `tex.ch` `\tracingstacklevels` prefixes: below the level a line
    /// `~` + one `.` per input level precedes the macro; at or beyond it
    /// only `~~\name` is printed (no new line first, no arguments later).
    /// Returns whether the arguments are traced too.
    pub(crate) fn trace_macro_call(&mut self, id: CsId, m: &Macro) -> bool {
        let levels = self.xt_int(IntParam::TracingStackLevels);
        let ptr = self.tex_input_ptr(id);
        self.pushed_read = Token(0);
        let within = levels <= 0 || ptr < levels;
        let mut line = Vec::new();
        if levels > 0 {
            if within {
                line.push(b'~');
                line.extend(std::iter::repeat(b'.').take(ptr as usize));
            } else {
                line.extend_from_slice(b"~~");
            }
        }
        self.push_print_cs(&mut line, id, true);
        if within {
            self.push_macro_text(&mut line, m);
        }
        let term = self.diagnostic_to_term();
        self.flush_trace_events();
        if within {
            self.tex_print_ln(term, true);
        }
        self.tex_print_printed(term, true, &line);
        self.tex_print_nl(term, true);
        // the arguments follow only at level 0 and below the level
        levels == 0 || (levels > 0 && within)
    }

    /// tex.web §400: `#n<-` and the argument just scanned.
    pub(crate) fn trace_macro_arg(&mut self, n: usize, arg: &[Token]) {
        const LIMIT: usize = 1000;
        let mut raw = self.tokens_to_bytes(arg);
        let mut line = Vec::new();
        line.push(b'#');
        line.extend_from_slice(n.to_string().as_bytes());
        line.extend_from_slice(b"<-");
        let truncated = raw.len() > LIMIT;
        raw.truncate(LIMIT);
        crate::tex_bytes::push_printable(&self.xprn, &mut line, &raw);
        if truncated {
            self.push_escape(&mut line);
            line.extend_from_slice(b"ETC.");
        }
        let term = self.diagnostic_to_term();
        self.flush_trace_events();
        self.tex_print_nl(term, true);
        self.tex_print_printed(term, true, &line);
        self.tex_print_nl(term, true);
    }

    /// tex.web §323 for `\tracingmacros>1`: a parameter token list (`\everypar`,
    /// `\output`, `\mark`, …) that starts being read.
    pub(crate) fn trace_token_list(&mut self, name: &str, toks: &[Token]) {
        let esc_name = match name {
            "<everypar>" => "everypar",
            "<everymath>" => "everymath",
            "<everydisplay>" => "everydisplay",
            "<everyhbox>" => "everyhbox",
            "<everyvbox>" => "everyvbox",
            "<everyjob>" => "everyjob",
            "<everycr>" => "everycr",
            "<everyeof>" => "everyeof",
            "<output>" => "output",
            "<mark>" => "mark",
            "<write>" => "write",
            _ => return,
        };
        // Ratex's `<output>` list has lost its braces; its `<write>` list
        // carries tex.web's `{` … `}` and the end marker around the text.
        let shown = match name {
            "<write>" if toks.len() >= 3 => &toks[1..toks.len() - 2],
            _ => toks,
        };
        let mut line = Vec::new();
        self.push_escape(&mut line);
        line.extend_from_slice(esc_name.as_bytes());
        line.extend_from_slice(b"->");
        let mut raw = self.tokens_to_bytes(shown);
        if name == "<output>" {
            raw.insert(0, b'{');
            raw.push(b'}');
        }
        crate::tex_bytes::push_printable(&self.xprn, &mut line, &raw);
        let term = self.diagnostic_to_term();
        self.flush_trace_events();
        self.tex_print_nl(term, true);
        self.tex_print_printed(term, true, &line);
        self.tex_print_nl(term, true);
    }

    // ---------------------------------------------------------------- \showstream

    /// The open `\write` stream `\showstream` selects, if any (stream 16 and
    /// above, negative values and closed streams leave `\show…` as usual).
    pub(crate) fn show_stream(&self) -> Option<usize> {
        if self.engine_kind != EngineKind::XeTeX {
            return None;
        }
        usize::try_from(self.xt_int(IntParam::ShowStream))
            .ok()
            .filter(|&i| i < 16 && self.write_streams.get(i).is_some_and(Option::is_some))
    }

    /// Write text built as tex.web prints it to `\write` stream `stream`.
    pub(crate) fn write_show_stream(&mut self, stream: usize, mut text: Vec<u8>) {
        use std::io::Write;
        self.to_external(&mut text);
        let result = self.write_streams[stream]
            .as_mut()
            .map_or(Ok(()), |file| file.write_all(&text));
        if let Err(error) = result {
            let destination = self.write_stream_paths[stream]
                .as_deref()
                .unwrap_or("<unknown>")
                .to_string();
            self.error(&format!("Cannot write output stream {stream} (`{destination}`): {error}"));
        }
    }

    /// `print_nl("> "); <text>` and the closing `print_ln`; `text` is already
    /// in printed form. With the selector on a `\write` file every `print_nl`
    /// starts a new line.
    pub(crate) fn show_stream_line(&mut self, stream: usize, text: &[u8]) {
        let mut out = Vec::with_capacity(text.len() + 4);
        out.extend_from_slice(b"\n> ");
        out.extend_from_slice(text);
        out.push(b'\n');
        self.write_show_stream(stream, out);
    }

    /// `\showthe` and `\showtokens`: `> <text>` for text Ratex holds as a string.
    pub(crate) fn show_stream_text(&mut self, stream: usize, text: &str) {
        let raw = crate::tex_bytes::text_to_bytes(text);
        let mut printed = Vec::with_capacity(raw.len());
        crate::tex_bytes::push_printable(&self.xprn, &mut printed, &raw);
        self.show_stream_line(stream, &printed);
    }

    /// `\show`: `> \name=meaning`, where the text after `macro:` starts a
    /// new line (tex.web `print_meaning`).
    pub(crate) fn show_stream_meaning(&mut self, stream: usize, t: Token) {
        let mut text = Vec::new();
        if t.is_cs() {
            self.push_print_cs(&mut text, t.cs_id(), false);
            text.push(b'=');
        }
        let meaning = self.meaning_of(t);
        let raw = crate::tex_bytes::text_to_bytes(&meaning);
        let split = if t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Macro(_))) {
            raw.windows(6).position(|w| w == b"macro:").map(|i| i + 6)
        } else {
            None
        };
        match split {
            Some(at) => {
                crate::tex_bytes::push_printable(&self.xprn, &mut text, &raw[..at]);
                text.push(b'\n');
                crate::tex_bytes::push_printable(&self.xprn, &mut text, &raw[at..]);
            }
            None => crate::tex_bytes::push_printable(&self.xprn, &mut text, &raw),
        }
        self.show_stream_line(stream, &text);
    }

    /// `\showbox`, `\showlists`, `\showgroups`, `\showifs`: the display, then
    /// `end_diagnostic(true)` and `! OK`. `lead` is the `print_nl("")` the
    /// display itself does not start with.
    pub(crate) fn show_stream_display(&mut self, stream: usize, lead: bool, display: &[u8]) {
        let mut out = Vec::with_capacity(display.len() + 8);
        if lead {
            out.push(b'\n');
        }
        out.extend_from_slice(display);
        out.extend_from_slice(b"\n\n\n! OK\n");
        self.write_show_stream(stream, out);
    }
}
