//! e-TeX's table-driven transcript tracing: the `{changing ...}`,
//! `{into ...}`, `{reassigning ...}`, `{restoring ...}` and `{retaining ...}`
//! lines of `\tracingassigns` and `\tracingrestores` (tex.web show_eqtb,
//! e-TeX restore_trace/show_sa) and the pre-rendered `{entering ...}` and
//! `{leaving ...}` lines of `\tracinggroups`.
//!
//! The equivalent table queues [`TraceEvent`]s while it changes; the engine
//! prints them, in order, before any other transcript output (see
//! `Engine::append_log`), so they interleave with every other message
//! exactly as if each had been printed at the moment of the change.
use crate::engine::Engine;
use crate::eqtb::{Equiv, TraceEvent, TraceSlot, TraceValue};
use crate::input::Source;
use crate::prim::{GlueParam, IntParam, Prim};
use crate::show_box::BoxDisplay;
use crate::token::Token;

/// tex.web print of `bytes` for a trace line: printable ASCII as itself,
/// other bytes in `^^` notation.
fn push_printable(out: &mut Vec<u8>, bytes: &[u8]) {
    crate::tex_bytes::push_printable(&crate::tex_bytes::default_xprn(), out, bytes);
}

/// `show_token_list(p,null,32)`: at most this many characters are printed
/// before `\ETC.`.
const TOKEN_LIST_LIMIT: usize = 32;

/// tex.web print_cmd_chr(if_test, kind) without the escape character.
pub(crate) fn if_name(kind: u8) -> &'static str {
    const NAMES: [&str; 24] = [
        "if",
        "ifcat",
        "ifnum",
        "ifdim",
        "ifodd",
        "ifvmode",
        "ifhmode",
        "ifmmode",
        "ifinner",
        "ifvoid",
        "ifhbox",
        "ifvbox",
        "ifx",
        "ifeof",
        "iftrue",
        "iffalse",
        "ifcase",
        "ifdefined",
        "ifcsname",
        "iffontchar",
        "ifincsname",
        "ifpdfprimitive",
        "ifpdfabsnum",
        "ifpdfabsdim",
    ];
    NAMES.get(usize::from(kind)).copied().unwrap_or("if")
}

/// What e-TeX records when an input file is opened (`grp_stack[in_open]`
/// and `if_stack[in_open]`): the innermost group boundary and the number of
/// open conditionals.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FileNest {
    pub boundary: usize,
    pub if_depth: usize,
    /// The file is a `\scantokens` pseudo-file opened under
    /// `\tracingscantokens`, which prints `( ` and `)` like a file.
    pub scan_tokens: bool,
}

impl Engine {
    /// Print the queued trace events (each a `begin_diagnostic;
    /// print_char("{"); ...; print_char("}"); end_diagnostic(false)`).
    #[cold]
    pub(crate) fn print_trace_events(&mut self) {
        let events = std::mem::take(&mut self.eqtb.trace_events);
        for event in events {
            let (line, online) = match event {
                TraceEvent::Text { text, online } => (text, online),
                TraceEvent::Eqtb {
                    verb,
                    slot,
                    value,
                    escape,
                    online,
                } => (self.render_eqtb_event(verb, slot, &value, escape), online),
            };
            // print_char("{") ... print_char("}") then print_nl(""): the
            // line starts wherever the transcript is, ends the line, and
            // keeps `file_offset`/`term_offset` for the next `\message`.
            self.tex_print_printed(online, true, line.as_bytes());
            self.tex_print_nl(online, true);
        }
    }

    fn render_eqtb_event(
        &self,
        verb: &str,
        slot: TraceSlot,
        value: &TraceValue,
        escape: i32,
    ) -> String {
        let mut out = Vec::new();
        out.push(b'{');
        out.extend_from_slice(verb.as_bytes());
        out.push(b' ');
        self.show_eqtb(&mut out, slot, value, escape);
        out.push(b'}');
        String::from_utf8_lossy(&out).into_owned()
    }

    fn print_escape(out: &mut Vec<u8>, escape: i32) {
        if (0..256).contains(&escape) {
            push_printable(out, &[escape as u8]);
        }
    }

    /// `print_esc(name)` with the escape character of the event.
    fn print_esc_bytes(out: &mut Vec<u8>, escape: i32, name: &[u8]) {
        Self::print_escape(out, escape);
        push_printable(out, name);
    }

    fn print_name(&self, out: &mut Vec<u8>, escape: i32, code: u16, unknown: &str) {
        match self.primitive_names.get(&code) {
            Some(name) => Self::print_esc_bytes(out, escape, name),
            None => out.extend_from_slice(unknown.as_bytes()),
        }
    }

    /// tex.web sprint_cs.
    fn sprint_cs(&self, out: &mut Vec<u8>, id: u32, escape: i32) {
        let name = self.cs.name(id);
        if let Some((bytes, len)) = Self::active_cs_source_bytes(name) {
            out.extend_from_slice(&bytes[..len]);
        } else if name.is_empty() {
            Self::print_esc_bytes(out, escape, b"csname");
            Self::print_esc_bytes(out, escape, b"endcsname");
        } else {
            Self::print_esc_bytes(out, escape, name);
        }
    }

    fn token_piece(&self, token: Token, escape: i32) -> Vec<u8> {
        let mut piece = Vec::new();
        push_printable(&mut piece, &self.tokens_to_bytes_esc(&[token], escape));
        piece
    }

    /// `show_token_list(.., null, 32)` over already rendered tokens.
    fn print_limited(out: &mut Vec<u8>, escape: i32, pieces: impl Iterator<Item = Vec<u8>>) {
        let mut tally = 0;
        for piece in pieces {
            if tally >= TOKEN_LIST_LIMIT {
                Self::print_esc_bytes(out, escape, b"ETC.");
                return;
            }
            tally += piece.len();
            out.extend_from_slice(&piece);
        }
    }

    fn print_token_list_limited(&self, out: &mut Vec<u8>, escape: i32, tokens: &[Token]) {
        Self::print_limited(out, escape, tokens.iter().map(|&t| self.token_piece(t, escape)));
    }

    /// print_cmd_chr of an equivalent, then `:` and the replacement text for
    /// a macro.
    fn print_equiv(&self, out: &mut Vec<u8>, equiv: &Equiv, escape: i32) {
        match equiv {
            Equiv::Macro(m) => {
                if m.protected {
                    Self::print_esc_bytes(out, escape, b"protected");
                }
                if m.long {
                    Self::print_esc_bytes(out, escape, b"long");
                }
                if m.outer {
                    Self::print_esc_bytes(out, escape, b"outer");
                }
                if m.protected || m.long || m.outer {
                    out.push(b' ');
                }
                out.extend_from_slice(b"macro:");
                let mut pieces: Vec<Vec<u8>> = Vec::new();
                pieces.extend(m.prefix.iter().map(|&t| self.token_piece(t, escape)));
                for (i, delimiter) in m.params.iter().enumerate() {
                    pieces.push(format!("#{}", i + 1).into_bytes());
                    pieces.extend(delimiter.iter().map(|&t| self.token_piece(t, escape)));
                }
                pieces.push(b"->".to_vec());
                pieces.extend(m.body.iter().map(|&t| self.token_piece(t, escape)));
                Self::print_limited(out, escape, pieces.into_iter());
            }
            Equiv::Prim(p) => {
                Self::print_esc_bytes(out, escape, self.prim_name(*p).as_bytes());
            }
            Equiv::CharDef(c) => {
                Self::print_esc_bytes(out, escape, b"char");
                out.extend_from_slice(format!("\"{c:X}").as_bytes());
            }
            Equiv::MathCharDef(c) => {
                Self::print_esc_bytes(out, escape, b"mathchar");
                out.extend_from_slice(format!("\"{c:X}").as_bytes());
            }
            Equiv::FontRef(f) => {
                out.extend_from_slice(b"select font ");
                out.extend_from_slice(self.font_display_name(*f).as_bytes());
            }
            Equiv::CountReg(i) => self.print_register(out, escape, b"count", *i),
            Equiv::DimenReg(i) => self.print_register(out, escape, b"dimen", *i),
            Equiv::SkipReg(i) => self.print_register(out, escape, b"skip", *i),
            Equiv::MuSkipReg(i) => self.print_register(out, escape, b"muskip", *i),
            Equiv::ToksReg(i) => self.print_register(out, escape, b"toks", *i),
            Equiv::BoxReg(i) => self.print_register(out, escape, b"box", *i),
            Equiv::AttributeReg(i) => self.print_register(out, escape, b"attribute", *i),
            Equiv::UMathCharDef(v) if self.engine_kind == crate::engine::EngineKind::XeTeX => {
                Self::print_esc_bytes(out, escape, crate::xemath_prims::umathchardef_meaning(*v).as_bytes());
            }
            Equiv::UMathCharDef(v) => {
                let (class, family, slot) = crate::uprims::decode_umath_num(*v);
                Self::print_esc_bytes(out, escape, b"Umathchar");
                out.extend_from_slice(format!("\"{class:X}\"{family:02X}\"{slot:06X}").as_bytes());
            }
            Equiv::LuaCall { slot, protected } => {
                out.extend_from_slice(if *protected { b"luacall " } else { b"expandable luacall " });
                out.extend_from_slice(slot.to_string().as_bytes());
            }
            Equiv::CharTok(v) => {
                out.extend_from_slice(self.meaning_of(Token(*v)).as_bytes());
            }
            Equiv::Alias(id) => self.sprint_cs(out, *id, escape),
        }
    }

    fn print_register(&self, out: &mut Vec<u8>, escape: i32, name: &[u8], index: u16) {
        Self::print_esc_bytes(out, escape, name);
        out.extend_from_slice(index.to_string().as_bytes());
    }

    fn print_font_id(&self, out: &mut Vec<u8>, escape: i32, font: u16) {
        let cs = self.eqtb.font_cs.get(font as usize).copied().unwrap_or(0);
        self.sprint_cs_plain(out, cs, escape);
    }

    /// `print_esc(font_id_text(f))`: the name verbatim, whatever it is.
    fn sprint_cs_plain(&self, out: &mut Vec<u8>, id: u32, escape: i32) {
        Self::print_esc_bytes(out, escape, self.cs.name(id));
    }

    fn print_spec_bytes(&self, out: &mut Vec<u8>, glue: &crate::boxes::Glue, unit: &str) {
        let mut d = BoxDisplay::new(self);
        d.print_spec(glue, unit);
        out.extend_from_slice(&d.out);
    }

    /// tex.web show_eqtb (all six regions), e-TeX show_sa for the registers
    /// above 255.
    fn show_eqtb(&self, out: &mut Vec<u8>, slot: TraceSlot, value: &TraceValue, escape: i32) {
        let int = |v: &TraceValue| match v {
            TraceValue::Int(n) => *n,
            _ => 0,
        };
        let scaled = |out: &mut Vec<u8>, n: i64| {
            out.extend_from_slice(crate::build::print_scaled(n).as_bytes());
            out.extend_from_slice(b"pt");
        };
        match slot {
            TraceSlot::Eq(id) => {
                self.sprint_cs(out, id, escape);
                out.push(b'=');
                match value {
                    TraceValue::Eq(Some(equiv)) => self.print_equiv(out, equiv, escape),
                    _ => out.extend_from_slice(b"undefined"),
                }
            }
            TraceSlot::IntParam(i) => {
                self.print_name(out, escape, 0x1000 | i, "[unknown integer parameter!]");
                out.push(b'=');
                out.extend_from_slice(int(value).to_string().as_bytes());
            }
            TraceSlot::DimParam(i) => {
                self.print_name(out, escape, 0x2000 | i, "[unknown dimen parameter!]");
                out.push(b'=');
                scaled(out, int(value));
            }
            TraceSlot::GlueParam(i) => {
                self.print_name(out, escape, 0x3000 | i, "[unknown glue parameter!]");
                out.push(b'=');
                if let TraceValue::Glue(g) = value {
                    let mu = i >= GlueParam::ThinMuSkip.idx();
                    self.print_spec_bytes(out, g, if mu { "mu" } else { "pt" });
                }
            }
            TraceSlot::ToksParam(i) => {
                self.print_name(out, escape, 0x4000 | i, "[unknown toks parameter!]");
                out.push(b'=');
                if let TraceValue::Toks(t) = value {
                    if i == crate::prim::ToksParam::Output.idx() && !t.is_empty() {
                        // tex.web §1226 stores a non-empty \output enclosed in braces
                        let mut pieces = vec![b"{".to_vec()];
                        pieces.extend(t.iter().map(|&tok| self.token_piece(tok, escape)));
                        pieces.push(b"}".to_vec());
                        Self::print_limited(out, escape, pieces.into_iter());
                    } else {
                        self.print_token_list_limited(out, escape, t);
                    }
                }
            }
            TraceSlot::Count(i) => {
                self.print_register(out, escape, b"count", i);
                out.push(b'=');
                out.extend_from_slice(int(value).to_string().as_bytes());
            }
            TraceSlot::Dimen(i) => {
                self.print_register(out, escape, b"dimen", i);
                out.push(b'=');
                scaled(out, int(value));
            }
            TraceSlot::Skip(i) | TraceSlot::MuSkip(i) => {
                let mu = matches!(slot, TraceSlot::MuSkip(_));
                self.print_register(out, escape, if mu { b"muskip" } else { b"skip" }, i);
                out.push(b'=');
                if let TraceValue::Glue(g) = value {
                    self.print_spec_bytes(out, g, if mu { "mu" } else { "pt" });
                }
            }
            TraceSlot::Toks(i) => {
                self.print_register(out, escape, b"toks", i);
                out.push(b'=');
                if let TraceValue::Toks(t) = value {
                    self.print_token_list_limited(out, escape, t);
                }
            }
            TraceSlot::Box(i) => {
                self.print_register(out, escape, b"box", i);
                out.push(b'=');
                match value {
                    TraceValue::Box(None) => out.extend_from_slice(b"void"),
                    TraceValue::Box(Some(node)) => {
                        let mut d = BoxDisplay::with_limits(self, 0, 1, escape);
                        d.show_node_list(std::slice::from_ref(node));
                        out.extend_from_slice(&d.out);
                    }
                    _ => {}
                }
            }
            TraceSlot::Cat(c) => self.print_code(out, escape, b"catcode", c, int(value)),
            TraceSlot::LcCode(c) => self.print_code(out, escape, b"lccode", c, int(value)),
            TraceSlot::UcCode(c) => self.print_code(out, escape, b"uccode", c, int(value)),
            TraceSlot::SfCode(c) => self.print_code(out, escape, b"sfcode", c, int(value)),
            TraceSlot::MathCode(c) => self.print_code(out, escape, b"mathcode", c, int(value)),
            TraceSlot::DelCode(c) => self.print_code(out, escape, b"delcode", c, int(value)),
            TraceSlot::CurFont => {
                out.extend_from_slice(b"current font=");
                if let TraceValue::Font(f) = value {
                    self.print_font_id(out, escape, *f);
                }
            }
            TraceSlot::StyleFont(style, family) => {
                let name: &[u8] = match style {
                    0 => b"textfont",
                    1 => b"scriptfont",
                    _ => b"scriptscriptfont",
                };
                self.print_register(out, escape, name, family);
                out.push(b'=');
                if let TraceValue::Font(f) = value {
                    self.print_font_id(out, escape, *f);
                }
            }
            TraceSlot::ParShape | TraceSlot::PenaltyShape(_) => {
                let (name, penalties): (&[u8], bool) = match slot {
                    TraceSlot::PenaltyShape(0) => (b"interlinepenalties", true),
                    TraceSlot::PenaltyShape(1) => (b"clubpenalties", true),
                    TraceSlot::PenaltyShape(2) => (b"widowpenalties", true),
                    TraceSlot::PenaltyShape(_) => (b"displaywidowpenalties", true),
                    _ => (b"parshape", false),
                };
                Self::print_esc_bytes(out, escape, name);
                out.push(b'=');
                if let TraceValue::Shape(n, first, _) = value {
                    if *n == 0 {
                        out.push(b'0');
                    } else if penalties {
                        out.extend_from_slice(format!("{n} {first}").as_bytes());
                        if *n > 1 {
                            Self::print_esc_bytes(out, escape, b"ETC.");
                        }
                    } else {
                        out.extend_from_slice(n.to_string().as_bytes());
                    }
                }
            }
        }
    }

    fn print_code(&self, out: &mut Vec<u8>, escape: i32, name: &[u8], c: u32, value: i64) {
        Self::print_esc_bytes(out, escape, name);
        out.extend_from_slice(c.to_string().as_bytes());
        out.push(b'=');
        out.extend_from_slice(value.to_string().as_bytes());
    }

    // ---------- \tracingifs ----------

    /// tex.web print_cmd_chr(if_test, chr): the conditional's name, with the
    /// `\unless` prefix e-TeX adds.
    fn print_if_name(out: &mut Vec<u8>, escape: i32, kind: u8, unless: bool) {
        if unless {
            Self::print_esc_bytes(out, escape, b"unless");
        }
        Self::print_esc_bytes(out, escape, if_name(kind).as_bytes());
    }

    /// `begin_diagnostic; print_nl("{")` and the `mode: ` prefix of
    /// show_cur_cmd_chr.
    fn begin_command_trace(&mut self) -> Vec<u8> {
        let mut out = vec![b'{'];
        if self.shown_mode != Some(self.mode) {
            out.extend_from_slice(self.mode.name().as_bytes());
            out.extend_from_slice(b": ");
            self.shown_mode = Some(self.mode);
        }
        out
    }

    /// `end_diagnostic(false)` for a line built by `begin_command_trace`.
    fn end_command_trace(&mut self, mut out: Vec<u8>) {
        out.extend_from_slice(b"}\n");
        let text = String::from_utf8_lossy(&out).into_owned();
        let online = self.diagnostic_to_term();
        self.print_nl_diagnostic(&text, online);
    }

    /// e-TeX show_cur_cmd_chr for a conditional that is starting
    /// (`{\ifx: (level 1) entered on line 3}`), called before it is pushed.
    #[cold]
    pub(crate) fn show_if_start(&mut self, p: Prim) {
        let escape = self.eqtb.int_params[IntParam::EscapeChar as usize];
        let mut out = self.begin_command_trace();
        Self::print_if_name(&mut out, escape, Self::if_code(p), self.unless_next);
        out.extend_from_slice(b": ");
        let level = self.if_stack.len() + 1;
        let line = self.input.current_file_line();
        out.extend_from_slice(format!("(level {level})").as_bytes());
        if line != 0 {
            out.extend_from_slice(format!(" entered on line {line}").as_bytes());
        }
        self.end_command_trace(out);
    }

    /// e-TeX show_cur_cmd_chr for `\fi`, `\else` or `\or`
    /// (`{\else: \ifx (level 1) entered on line 3}`).
    #[cold]
    pub(crate) fn show_if_delimiter(&mut self, delimiter: Prim) {
        if self.eqtb.int_params[IntParam::TracingIfs as usize] <= 0 {
            return;
        }
        let escape = self.eqtb.int_params[IntParam::EscapeChar as usize];
        let mut out = self.begin_command_trace();
        let name: &[u8] = match delimiter {
            Prim::Fi => b"fi",
            Prim::Or => b"or",
            _ => b"else",
        };
        Self::print_esc_bytes(&mut out, escape, name);
        out.extend_from_slice(b": ");
        let (kind, unless, line) = self
            .if_stack
            .last()
            .map_or((0, false, 0), |s| (s.kind, s.unless, s.loc_line));
        Self::print_if_name(&mut out, escape, kind, unless);
        out.push(b' ');
        out.extend_from_slice(format!("(level {})", self.if_stack.len()).as_bytes());
        if line != 0 {
            out.extend_from_slice(format!(" entered on line {line}").as_bytes());
        }
        self.end_command_trace(out);
    }

    // ---------- \tracingnesting ----------

    /// Record the group and conditional nesting of the file that was just
    /// pushed (e-TeX begin_file_reading's `grp_stack`/`if_stack`).
    pub(crate) fn mark_file_nesting(&mut self) {
        let record = FileNest {
            boundary: self.eqtb.cur_boundary(),
            if_depth: self.if_stack.len(),
            scan_tokens: false,
        };
        if let Some(Source::File { tracked, .. }) = self.input.stack.last_mut() {
            *tracked = true;
            self.file_nests.push(record);
        }
    }

    /// e-TeX pseudo_start's "Initiate input from new pseudo file": the file
    /// is tracked like a real one, and `\tracingscantokens>0` prints `( `
    /// (and `)` when it ends) as for an input file.
    pub(crate) fn mark_scan_tokens_file(&mut self) {
        self.mark_file_nesting();
        if self.eqtb.int_params[IntParam::TracingScanTokens as usize] > 0 {
            if !self.log.is_empty() && !self.log.ends_with('\n') {
                self.append_log(" ");
            }
            if !self.term.is_empty() && !self.term.ends_with('\n') {
                self.append_term(" ");
            }
            self.append_log("( ");
            self.append_term("( ");
            if let Some(nest) = self.file_nests.last_mut() {
                nest.scan_tokens = true;
            }
        }
    }

    /// A `Warning: ...` line of `\tracingnesting`; with `\tracingnesting>1`
    /// it carries the input context.
    fn nesting_warning(&mut self, text: &str) {
        let source = if self.eqtb.int_params[IntParam::TracingNesting as usize] > 1 {
            self.input.current_source_context()
        } else {
            None
        };
        self.warning_at(text, source);
    }

    /// e-TeX group_warning, called when the group opened at `boundary`
    /// (described by `code`, `level`, `line`) ends.
    pub(crate) fn group_warning(&mut self, boundary: usize, outer: usize, group: (u8, u16, i32)) {
        let mut warn = false;
        for nest in self.file_nests.iter_mut().rev() {
            if nest.boundary != boundary {
                break;
            }
            nest.boundary = outer;
            warn = true;
        }
        if warn && self.eqtb.int_params[IntParam::TracingNesting as usize] > 0 {
            let text = format!(
                "end of {} of a different file",
                crate::eqtb::group_description(group.0, group.1, group.2, true)
            );
            self.nesting_warning(&text);
        }
    }

    /// e-TeX if_warning, called when the innermost conditional ends.
    pub(crate) fn if_warning(&mut self) {
        let depth = self.if_stack.len();
        let mut warn = false;
        for nest in self.file_nests.iter_mut().rev() {
            if nest.if_depth != depth {
                break;
            }
            nest.if_depth = depth - 1;
            warn = true;
        }
        if warn && self.eqtb.int_params[IntParam::TracingNesting as usize] > 0 {
            let escape = self.eqtb.int_params[IntParam::EscapeChar as usize];
            let state = &self.if_stack[depth - 1];
            let (kind, unless, line) = (state.kind, state.unless, state.loc_line);
            let mut out = Vec::new();
            out.extend_from_slice(b"end of ");
            Self::print_if_name(&mut out, escape, kind, unless);
            if line != 0 {
                out.extend_from_slice(format!(" entered on line {line}").as_bytes());
            }
            out.extend_from_slice(b" of a different file");
            let text = String::from_utf8_lossy(&out).into_owned();
            self.nesting_warning(&text);
        }
    }

    /// The end of an input file: e-TeX file_warning for the groups and
    /// conditionals it left open, then the pseudo-file's closing `)` under
    /// `\tracingscantokens`.
    pub(crate) fn finish_tracked_file(&mut self) {
        let Some(nest) = self.file_nests.pop() else {
            return;
        };
        if self.eqtb.int_params[IntParam::TracingNesting as usize] > 0 {
            self.file_warning(&nest);
        }
        if nest.scan_tokens {
            self.append_log(")");
            self.append_term(")");
        }
    }

    fn file_warning(&mut self, nest: &FileNest) {
        let escape = self.eqtb.int_params[IntParam::EscapeChar as usize];
        let mut texts: Vec<String> = Vec::new();
        // groups opened since the file began, innermost first
        for (index, group) in self.eqtb.groups.iter().enumerate().rev() {
            if group.boundary + 1 == nest.boundary {
                break;
            }
            let level = (index + 1) as u16;
            texts.push(format!(
                "end of file when {} is incomplete",
                crate::eqtb::group_description(group.meta.code, level, group.line, true)
            ));
        }
        // conditionals started since the file began, innermost first
        for state in self.if_stack[nest.if_depth.min(self.if_stack.len())..].iter().rev() {
            let mut out = Vec::new();
            out.extend_from_slice(b"end of file when ");
            Self::print_if_name(&mut out, escape, state.kind, state.unless);
            if state.in_else {
                Self::print_esc_bytes(&mut out, escape, b"else");
            }
            if state.loc_line != 0 {
                out.extend_from_slice(format!(" entered on line {}", state.loc_line).as_bytes());
            }
            out.extend_from_slice(b" is incomplete");
            texts.push(String::from_utf8_lossy(&out).into_owned());
        }
        for text in texts {
            self.nesting_warning(&text);
        }
    }
}
