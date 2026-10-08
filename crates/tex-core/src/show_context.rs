//! TeX's `show_context` (tex.web 311-318) over the engine's input stack, as
//! luatex's `set_last_error_context` runs it: newline character 10, so every
//! level starts on a new line, and the result goes to
//! `status.lasterrorcontext` / the `show_error_hook` callback.
//!
//! The engine keeps the sources of the stack in its own shapes: macro calls
//! with parameters are `MacroFrame`s that deliver the arguments lazily, a
//! single backed-up token lives in `Engine::pushed`, and the private input of
//! a `\write` expansion parks the stack it hides in `Engine::parked_inputs`.
//! `levels` turns them back into the levels `show_context` walks.

use std::borrow::Cow;
use std::rc::Rc;

use crate::engine::Engine;
use crate::eqtb::Equiv;
use crate::input::Source;
use crate::prim::IntParam;
use crate::token::{CsId, Token};

const ERROR_LINE: usize = 79;
const HALF_ERROR_LINE: usize = 50;

/// `token_type` of a token list level (`print_token_list_type`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ListType {
    Parameter,
    Template,
    BackedUp,
    Inserted,
    Output,
    EveryPar,
    EveryMath,
    EveryDisplay,
    EveryHbox,
    EveryVbox,
    EveryJob,
    EveryCr,
    Mark,
    EveryEof,
    Write,
    InterCharToks,
}

impl ListType {
    fn header(self, read: bool) -> &'static str {
        match self {
            ListType::Parameter => "<argument> ",
            ListType::Template => "<template> ",
            ListType::BackedUp if read => "<recently read> ",
            ListType::BackedUp => "<to be read again> ",
            ListType::Inserted => "<inserted text> ",
            ListType::Output => "<output> ",
            ListType::EveryPar => "<everypar> ",
            ListType::EveryMath => "<everymath> ",
            ListType::EveryDisplay => "<everydisplay> ",
            ListType::EveryHbox => "<everyhbox> ",
            ListType::EveryVbox => "<everyvbox> ",
            ListType::EveryJob => "<everyjob> ",
            ListType::EveryCr => "<everycr> ",
            ListType::Mark => "<mark> ",
            ListType::EveryEof => "<everyeof> ",
            ListType::Write => "<write> ",
            ListType::InterCharToks => "<XeTeXinterchartoks> ",
        }
    }

    /// The type of a token list pushed under `name`; `None` for lists that
    /// are no input level of TeX's.
    fn of_name(name: &str) -> Option<ListType> {
        Some(match name {
            "<align-u>" | "<align-cell>" => ListType::Template,
            "<everypar>" => ListType::EveryPar,
            "<everymath>" => ListType::EveryMath,
            "<everydisplay>" => ListType::EveryDisplay,
            "<everyhbox>" => ListType::EveryHbox,
            "<everyvbox>" => ListType::EveryVbox,
            "<everyjob>" => ListType::EveryJob,
            "<everycr>" => ListType::EveryCr,
            "<everyeof>" => ListType::EveryEof,
            "<mark>" => ListType::Mark,
            "<output>" => ListType::Output,
            "<write>" => ListType::Write,
            "<XeTeXinterchartoks>" => ListType::InterCharToks,
            "<inserted>" => ListType::Inserted,
            "<endoutput>" => return None,
            _ => ListType::BackedUp,
        })
    }
}

/// What TeX inserted before the token an error was found at, when the
/// engine has not put it on its input yet or not as an inserted list.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Insertion {
    None,
    /// the newest backed-up token is the inserted text
    TopPushed,
    /// an inserted `\par`
    Par,
    /// an inserted `$`
    Dollar,
}

/// A macro level the expansion shortcuts never put on the input stack.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SkippedLevel {
    /// a parameterless macro whose body is one token, read
    Body(CsId),
    /// a macro whose body is one parameter, with the one-token argument it
    /// passed on, read
    Argument(CsId, Token),
}

/// One level of the input stack as `show_context` sees it.
enum Level<'a> {
    /// A line of an input file or pseudo file.
    File(&'a Source),
    /// A token list; `loc` is the index of the next token to read.
    List {
        ty: ListType,
        toks: Cow<'a, [Token]>,
        loc: usize,
        /// the last token stands for the frozen `\endtemplate`
        end_template: bool,
    },
    /// A macro: the definition text and how far its body has been read.
    Macro { cs: Option<CsId>, body: &'a Rc<[Token]>, loc: usize },
}

/// A level rendered into the pseudoprinted pieces `show_context` cuts.
struct Rendered {
    /// what precedes the pseudoprinted text on the first line
    header: Vec<u8>,
    /// `tally` of the header (only `print_char` counts when printing into a string)
    l: usize,
    text: Vec<u8>,
    /// `first_count`: the text read so far
    first: usize,
}

impl Engine {
    /// `show_context` as a string: what luatex stores in
    /// `status.lasterrorcontext` and the `show_error_hook` callback reads.
    ///
    /// `insertion`: how TeX's `ins_error` inserted text before the token the
    /// error was found at. `printed`: the text goes to the terminal and the
    /// transcript, where `tally` counts the whole location header of a
    /// level, not only what `print_char` printed into a string.
    pub(crate) fn show_context_string(&self, insertion: Insertion, printed: bool) -> String {
        let levels = self.context_levels(insertion);
        let limit = i64::from(self.eqtb.int_params[IntParam::ErrorContextLines.idx() as usize]);
        let mut out: Vec<u8> = Vec::new();
        let mut nn: i64 = -1;
        let count = levels.len();
        for (index, level) in levels.iter().enumerate() {
            let top = index == 0;
            let is_file = matches!(level, Level::File(_));
            let bottom_line = is_file && (index + 1 == count || !is_pseudo_file(level));
            if top || bottom_line || nn < limit {
                let read = matches!(level, Level::List { ty: ListType::BackedUp, toks, loc, .. } if *loc >= toks.len());
                if top || is_file || !read {
                    let mut rendered = self.render_level(level);
                    if printed {
                        rendered.l = self.code_units(&rendered.header[1..]);
                    }
                    rendered.emit(&mut out);
                    nn += 1;
                }
            } else if nn == limit {
                out.extend_from_slice(b"\n...");
                nn += 1;
            }
            if bottom_line {
                break;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// The levels of the input stack from the top down.
    fn context_levels(&self, insertion: Insertion) -> Vec<Level<'_>> {
        let mut levels = Vec::new();
        let synthetic = match insertion {
            Insertion::Par => Some(Token::from_cs(self.partoken_id())),
            Insertion::Dollar if !self.pushed.last().is_some_and(|t| t.is_char() && t.cc() == 3) => {
                Some(Token::char(3, u32::from(b'$')))
            }
            _ => None,
        };
        if let Some(t) = synthetic {
            levels.push(Level::List { ty: ListType::Inserted, toks: Cow::Owned(vec![t]), loc: 0, end_template: false });
        }
        let inserted = insertion == Insertion::TopPushed
            || (insertion == Insertion::Dollar && synthetic.is_none());
        let parked = self.parked_inputs.iter().rev();
        let current = std::iter::once((&self.input.stack, &self.pushed)).chain(parked.map(|p| (&p.0, &p.1)));
        for (depth, (stack, pushed)) in current.enumerate() {
            // a macro level a shortcut skipped, until something is read
            // from the input below it or backed up
            let skipped = match self.skipped_level {
                Some((level, signature)) if depth == 0 && pushed.is_empty() && self.input.signature() == signature => {
                    Some(level)
                }
                _ => None,
            };
            // the backed-up token read last is still a level of TeX's, with
            // nothing left to read, until something else is read (the
            // skipped level, newer, ended it)
            let recent = match self.recent_pushed {
                Some((t, signature))
                    if depth == 0 && skipped.is_none() && pushed.is_empty() && self.input.signature() == signature =>
                {
                    Some(t)
                }
                _ => None,
            };
            // back_input first ends the finished lists below the token it backs up
            let mut top = stack.len();
            if !pushed.is_empty() || recent.is_some() {
                while top > 0 && self.list_finished(&stack[top - 1]) {
                    top -= 1;
                }
            }
            if let Some(t) = recent {
                levels.push(Level::List { ty: ListType::BackedUp, toks: Cow::Owned(vec![t]), loc: 1, end_template: false });
            }
            levels.extend(pushed.iter().rev().enumerate().map(|(i, &t)| Level::List {
                ty: if inserted && depth == 0 && i == 0 { ListType::Inserted } else { ListType::BackedUp },
                toks: Cow::Owned(vec![t]),
                loc: 0,
                end_template: false,
            }));
            if let Some(level) = skipped {
                self.push_skipped_levels(level, &mut levels);
            }
            for source in stack[..top].iter().rev() {
                self.push_levels(source, &mut levels);
            }
        }
        levels
    }

    /// Whether `source` is a token list with nothing left to read.
    fn list_finished(&self, source: &Source) -> bool {
        match source {
            Source::TokList { toks, pos, name, .. } => *pos >= toks.len() && *name != "<align-cell>",
            Source::MacroFrame(frame) => {
                let (body, body_loc, arg) = frame.context_view();
                body_loc >= body.len() && arg.is_none_or(|(toks, loc)| loc >= toks.len())
            }
            Source::File { .. } => false,
        }
    }

    fn push_skipped_levels<'a>(&'a self, level: SkippedLevel, levels: &mut Vec<Level<'a>>) {
        let (id, argument) = match level {
            SkippedLevel::Body(id) => (id, None),
            SkippedLevel::Argument(id, t) => (id, Some(t)),
        };
        if let Some(t) = argument {
            levels.push(Level::List { ty: ListType::Parameter, toks: Cow::Owned(vec![t]), loc: 1, end_template: false });
        }
        if let Some(Equiv::Macro(m)) = self.eqtb.get(id) {
            levels.push(Level::Macro { cs: Some(id), body: &m.body, loc: m.body.len() });
        }
    }

    fn push_levels<'a>(&'a self, source: &'a Source, levels: &mut Vec<Level<'a>>) {
        match source {
            Source::File { .. } => levels.push(Level::File(source)),
            Source::MacroFrame(frame) => {
                let (body, body_loc, arg) = frame.context_view();
                if let Some((toks, loc)) = arg {
                    levels.push(Level::List {
                        ty: ListType::Parameter,
                        toks: Cow::Borrowed(toks),
                        loc,
                        end_template: false,
                    });
                }
                levels.push(Level::Macro { cs: frame.owner, body, loc: body_loc });
            }
            Source::TokList { toks, pos, name, owner, .. } => {
                if *name == "<macro>" {
                    let cs = *owner;
                    match toks {
                        crate::input::TokTokens::Rc(body) => {
                            levels.push(Level::Macro { cs, body, loc: *pos });
                        }
                        // a macro that only passes on one parameter: the
                        // list holds that parameter, the body is `#n`
                        crate::input::TokTokens::Vec(args) => {
                            levels.push(Level::List {
                                ty: ListType::Parameter,
                                toks: Cow::Borrowed(args),
                                loc: *pos,
                                end_template: false,
                            });
                            if let Some(Equiv::Macro(m)) = cs.and_then(|id| self.eqtb.get(id)) {
                                levels.push(Level::Macro { cs, body: &m.body, loc: m.body.len() });
                            }
                        }
                    }
                    return;
                }
                if *name == "<write>" {
                    // `{`, the text, `}` and the frozen `\endwrite` are one list here
                    let text = toks.len().saturating_sub(3);
                    let read = *pos;
                    if read > text + 1 {
                        levels.push(Level::List {
                            ty: ListType::Inserted,
                            toks: Cow::Borrowed(&toks[1 + text..]),
                            loc: read - (1 + text),
                            end_template: false,
                        });
                    } else {
                        levels.push(Level::List {
                            ty: ListType::Write,
                            toks: Cow::Borrowed(&toks[1..1 + text]),
                            loc: read.saturating_sub(1),
                            end_template: false,
                        });
                        levels.push(Level::List {
                            ty: ListType::Inserted,
                            toks: Cow::Borrowed(&toks[1 + text..]),
                            loc: 0,
                            end_template: false,
                        });
                    }
                    return;
                }
                if *name == "<output>" {
                    // the output routine's text, braces included
                    let mut braced = Vec::with_capacity(toks.len() + 2);
                    braced.push(Token::char(1, u32::from(b'{')));
                    braced.extend_from_slice(toks);
                    braced.push(Token::char(2, u32::from(b'}')));
                    levels.push(Level::List { ty: ListType::Output, toks: Cow::Owned(braced), loc: *pos + 1, end_template: false });
                    return;
                }
                if let Some(ty) = ListType::of_name(name) {
                    levels.push(Level::List {
                        ty,
                        toks: Cow::Borrowed(toks),
                        loc: *pos,
                        end_template: *name == "<align-cell>",
                    });
                }
            }
        }
    }

    fn render_level(&self, level: &Level<'_>) -> Rendered {
        let esc = self.eqtb.int_params[IntParam::EscapeChar.idx() as usize];
        match level {
            Level::File(source) => self.render_file(source),
            Level::List { ty, toks, loc, end_template } => {
                let read = *loc >= toks.len();
                let mut text = Vec::new();
                let mut first = None;
                for (i, &t) in toks.iter().enumerate() {
                    if i == *loc {
                        first = Some(text.len());
                    }
                    if *end_template && i + 1 == toks.len() {
                        push_escaped(&mut text, esc, b"endtemplate ");
                    } else {
                        self.push_token_text(&mut text, esc, t);
                    }
                }
                let first = first.unwrap_or(text.len());
                let mut header = vec![b'\n'];
                header.extend_from_slice(ty.header(read).as_bytes());
                Rendered { header, l: 1, text, first }
            }
            Level::Macro { cs, body, loc } => {
                let mut header = vec![b'\n'];
                let mut l = 1;
                if let Some(id) = cs {
                    l += self.push_cs_header(&mut header, esc, *id);
                }
                let mut text = Vec::new();
                if let Some(Equiv::Macro(m)) = cs.and_then(|id| self.eqtb.get(id)) {
                    if Rc::ptr_eq(&m.body, body) {
                        for &t in &m.prefix {
                            self.push_token_text(&mut text, esc, t);
                        }
                        for (i, delimiter) in m.params.iter().enumerate().take(usize::from(m.num_params)) {
                            text.extend_from_slice(format!("#{}", i + 1).as_bytes());
                            for &t in delimiter {
                                self.push_token_text(&mut text, esc, t);
                            }
                        }
                    }
                }
                text.extend_from_slice(b"->");
                let mut first = None;
                for (i, &t) in body.iter().enumerate() {
                    if i == *loc {
                        first = Some(text.len());
                    }
                    self.push_token_text(&mut text, esc, t);
                }
                let first = first.unwrap_or(text.len());
                Rendered { header, l, text, first }
            }
        }
    }

    /// `print_cs` of a macro name into `out` (after the line break); returns
    /// the characters `tally` counts: the escape character and the space
    /// (names are appended without `print_char`).
    fn push_cs_header(&self, out: &mut Vec<u8>, esc: i32, id: CsId) -> usize {
        let name = self.cs.name(id);
        if let Some((bytes, len)) = Self::active_cs_source_bytes(name) {
            out.extend_from_slice(&bytes[..len]);
            return len;
        }
        let mut counted = 0;
        if (0..0x11_0000).contains(&esc) {
            let before = out.len();
            push_char(out, esc as u32);
            counted += out.len() - before;
        }
        out.extend_from_slice(name);
        let letter = name.len() == 1 && self.eqtb.cat[usize::from(name[0])] == crate::token::CAT_LETTER;
        if name.len() != 1 || letter {
            out.push(b' ');
            counted += 1;
        }
        counted
    }

    fn push_token_text(&self, out: &mut Vec<u8>, esc: i32, t: Token) {
        if t == crate::page::WRITE_END_TOKEN {
            push_escaped(out, esc, b"endwrite ");
        } else {
            out.extend_from_slice(&self.tokens_to_bytes_esc(&[t], esc));
        }
    }

    fn render_file(&self, source: &Source) -> Rendered {
        let Source::File { line_no, data, line_start, line_buf, line_pos, line_end_len, .. } = source else {
            unreachable!("render_file takes a file level")
        };
        let end_line_char = self.eqtb.int_params[IntParam::EndLineChar.idx() as usize];
        let (text, loc): (Vec<u8>, usize) = match line_buf {
            Some(buf) => {
                // PSEUDO_PRINT_THE_LINE leaves out a last character that is
                // the current \endlinechar
                let mut end = buf.len();
                let tail = usize::from(*line_end_len);
                if tail > 0 {
                    let mut encoded = [0u8; 4];
                    let wanted: &[u8] = match u32::try_from(end_line_char).ok().and_then(char::from_u32) {
                        Some(c) => c.encode_utf8(&mut encoded).as_bytes(),
                        None => &[],
                    };
                    if &buf[end - tail..] == wanted {
                        end -= tail;
                    }
                } else if (0..128).contains(&end_line_char) && buf.last() == Some(&(end_line_char as u8)) {
                    end -= 1;
                }
                (buf[..end].to_vec(), (*line_pos).min(end))
            }
            None => {
                let (end, _) = crate::input::physical_line_bounds(data, *line_start);
                let line = data[(*line_start).min(end)..end].to_vec();
                let len = line.len();
                (line, len)
            }
        };
        let mut header = b"\nl.".to_vec();
        let number = line_no.to_string();
        header.extend_from_slice(number.as_bytes());
        header.push(b' ');
        Rendered { header, l: 2 + number.len(), text, first: loc }
    }
}

impl Rendered {
    /// The two lines `show_context` prints from the pseudoprinted text.
    fn emit(&self, out: &mut Vec<u8>) {
        let first = self.first.min(self.text.len());
        let tally = self.text.len();
        let trick_count = ERROR_LINE.max(first + 1 + ERROR_LINE - HALF_ERROR_LINE);
        let second = tally.min(trick_count) - first;
        out.extend_from_slice(&self.header);
        let (from, n) = if self.l + first <= HALF_ERROR_LINE {
            (0, self.l + first)
        } else {
            out.extend_from_slice(b"...");
            (self.l + first - HALF_ERROR_LINE + 3, HALF_ERROR_LINE)
        };
        push_valid_utf8(out, &self.text, from, first);
        out.push(b'\n');
        out.extend(std::iter::repeat(b' ').take(n));
        let to = if second + n <= ERROR_LINE { first + second } else { first + (ERROR_LINE - n - 3) };
        push_valid_utf8(out, &self.text, first, to);
        if second + n > ERROR_LINE {
            out.extend_from_slice(b"...");
        }
    }
}

/// `print_valid_utf8` over `text[from..to]`: stray continuation bytes and
/// invalid lead bytes are dropped, a lead byte brings its continuation bytes.
fn push_valid_utf8(out: &mut Vec<u8>, text: &[u8], from: usize, to: usize) {
    for q in from..to {
        let c = text[q];
        let extra = match c {
            0..=127 => 0,
            128..=193 => continue,
            194..=223 => 1,
            224..=239 => 2,
            240..=244 => 3,
            _ => continue,
        };
        out.push(c);
        out.extend(text.iter().skip(q + 1).take(extra));
    }
}

fn is_pseudo_file(level: &Level<'_>) -> bool {
    matches!(level, Level::File(Source::File { name, .. })
        if matches!(name.as_str(), "<scantokens>" | "<scantextokens>" | "<directlua>"))
}

fn push_char(out: &mut Vec<u8>, c: u32) {
    match char::from_u32(c) {
        Some(c) => out.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes()),
        None => out.push(c as u8),
    }
}

fn push_escaped(out: &mut Vec<u8>, esc: i32, name: &[u8]) {
    if (0..0x11_0000).contains(&esc) {
        push_char(out, esc as u32);
    }
    out.extend_from_slice(name);
}

#[cfg(test)]
mod tests {
    use crate::engine::{Engine, InteractionMode};

    fn transcript(source: &str) -> (String, i32) {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.add_nullfont();
        engine.eqtb.cat[b'{' as usize] = 1;
        engine.eqtb.cat[b'}' as usize] = 2;
        engine.eqtb.cat[b'#' as usize] = 6;
        engine.set_interaction_mode(InteractionMode::Nonstop);
        engine.input.push_file("ctx.tex".to_string(), source.as_bytes().to_vec());
        engine.run();
        (engine.log.clone(), engine.error_count)
    }

    /// The expansion shortcuts for a macro whose body is one token and for a
    /// macro that passes on a one-token argument put no list on the input
    /// stack; the error context still shows the levels TeX has. Expected
    /// lines from `pdftex -ini` (TeX Live 2026).
    #[test]
    fn single_token_macro_levels_appear_in_error_context() {
        let (log, errors) = transcript(
            "\\errorcontextlines=5\n\
             \\def\\a{\\undefined}\n\
             \\a\n\
             \\def\\m#1{#1}\n\
             \\m{\\undefined}\n\
             \\def\\c{x}\n\
             \\count1=\\c\\relax\n\
             \\def\\d{\\a}\n\
             \\d\n\
             \\def\\e{\\undefined}\\def\\f{\\e}\\def\\g{\\f}\n\
             \\g x\n\
             \\def\\h{\\c}\n\
             \\count1=\\h\\relax\n\
             \\m\\undefined\n\
             \\def\\k#1#2{#2}\\k{a}\\undefined\n\
             \\end\n",
        );
        assert_eq!(errors, 8, "{log}");
        for expected in [
            "! Undefined control sequence.\n\\a ->\\undefined \n                \nl.3 \\a\n      \n",
            "! Undefined control sequence.\n<argument> \\undefined \n                      \n\\m #1->#1\n         \nl.5 \\m{\\undefined}\n",
            "! Missing number, treated as zero.\n<to be read again> \n                   x\nl.7 \\count1=\\c\n              \\relax\n",
            "! Undefined control sequence.\n\\a ->\\undefined \n                \nl.9 \\d\n      \n",
            "! Undefined control sequence.\n\\e ->\\undefined \n                \nl.11 \\g\n        x\n",
            "! Missing number, treated as zero.\n<to be read again> \n                   x\nl.13 \\count1=\\h\n               \\relax\n",
            "! Undefined control sequence.\n<argument> \\undefined \n                      \n\\m #1->#1\n         \nl.14 \\m\\undefined\n",
            "! Undefined control sequence.\n<argument> \\undefined \n                      \n\\k #1#2->#2\n           \nl.15 \\def\\k#1#2{#2}\\k{a}\\undefined\n",
        ] {
            assert!(log.contains(expected), "missing:\n{expected}\nlog:\n{log}");
        }
    }
}
