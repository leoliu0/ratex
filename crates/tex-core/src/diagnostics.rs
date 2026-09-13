//! Human-readable, structured diagnostics for TeX errors.

use crate::engine::Engine;
use crate::input::{Source, SourceContext, SourceMark};
use crate::prim::{IntParam, ToksParam};

const MAX_CONTEXT_FRAMES: usize = 20;
const DEFAULT_CONTEXT_FRAMES: usize = 5;
const MAX_SOURCE_COLUMNS: usize = 120;
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_HELP_BYTES: usize = 4 * 1024;
const MAX_CONTROL_SEQUENCE_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub primary: Option<SourceContext>,
    pub highlight_len: usize,
    /// Macro calls ordered from the user-facing outer call to the innermost.
    /// A literal `…` entry marks frames omitted to honor `\\errorcontextlines`.
    pub expansion: Vec<String>,
    /// Parent files ordered from the direct includer outwards.
    pub included_from: Vec<SourceContext>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn render(&self) -> String {
        let mut out = String::new();
        let message = bounded_text(&self.message, MAX_MESSAGE_BYTES).replace('\n', "\n  | ");
        out.push_str(match self.severity {
            DiagnosticSeverity::Error => "! ",
            DiagnosticSeverity::Warning => "warning: ",
        });
        out.push_str(&message);
        out.push('\n');

        if let Some(primary) = &self.primary {
            out.push_str(&format!(
                "  --> {}:{}:{}\n",
                bounded_inline(&primary.name, 4096),
                primary.line,
                primary.column
            ));
            if !primary.text.is_empty() {
                let (line, caret, width) = source_window(
                    &primary.text,
                    primary.display_column.saturating_sub(1),
                    self.highlight_len,
                );
                let gutter = primary.line.to_string().len();
                out.push_str(&format!("{:gutter$} |\n", ""));
                out.push_str(&format!("{} | {}\n", primary.line, line));
                out.push_str(&format!(
                    "{:gutter$} | {}{}\n",
                    "",
                    " ".repeat(caret),
                    "^".repeat(width.max(1))
                ));
            }
        }

        if !self.expansion.is_empty() {
            out.push_str("  = while expanding: ");
            out.push_str(
                &self
                    .expansion
                    .iter()
                    .map(|name| bounded_inline(name, MAX_CONTROL_SEQUENCE_BYTES + 8))
                    .collect::<Vec<_>>()
                    .join(" -> "),
            );
            out.push('\n');
        }
        for parent in &self.included_from {
            out.push_str(&format!(
                "  = included from {}:{}:{}\n",
                bounded_inline(&parent.name, 4096),
                parent.line,
                parent.column
            ));
        }
        if let Some(help) = &self.help {
            out.push_str("  = help: ");
            let help = bounded_text(help.trim(), MAX_HELP_BYTES).replace('\n', "\n  =       ");
            out.push_str(help.trim());
            out.push('\n');
        }
        out
    }
}

impl Engine {
    /// Clear transient diagnostic state before processing a new top-level
    /// document with a reused format engine.
    pub fn reset_job_diagnostics(&mut self) {
        self.error_count = 0;
        self.stopped_on_error = false;
        self.explicit_end_seen = false;
        self.diagnostics.clear();
        self.diagnostic_output.clear();
        self.diagnostic_source_override = None;
        self.diagnostic_trace_override = None;
        self.pending_terminal_error_source = None;
        self.diagnostic_macro_trace.clear();
        self.diagnostic_token_from_file = false;
        self.diagnostic_trace_hold = 0;
        self.diagnostic_source_cs = None;
        self.diagnostic_physical_source = None;
        self.diagnostic_macro_call_site = None;
        self.diagnostic_macro_call_span = 1;
        self.diagnostic_synthetic_source = None;
        self.diagnostic_group_openings.clear();
        self.diagnostic_use_err_help = false;
        self.definable_cs_recovery_count = 0;
    }

    pub(crate) fn enter_macro_diagnostic(
        &mut self,
        owner: crate::token::CsId,
        invocation: crate::token::CsId,
    ) {
        const MAX_TRACE: usize = 20;
        let synthetic = self
            .diagnostic_synthetic_source
            .take()
            .filter(|(token, _, _)| *token == invocation);
        let physical = self.physical_source_for_cs(invocation);
        // Reset the outer call only for a token whose exact physical spelling
        // matches this invocation. Scanning a macro's physical argument can
        // leave `diagnostic_token_from_file` set while an internal wrapper is
        // expanded; treating that wrapper as a new source call shifted carets
        // into `\hspace{...}` and discarded the useful `\hspace` frame.
        if synthetic.is_some() || physical.is_some() {
            self.diagnostic_macro_trace.clear();
            if let Some((_, mark, span)) = synthetic {
                self.diagnostic_macro_call_site = Some(mark);
                self.diagnostic_macro_call_span = span.max(1);
            } else if let Some((mark, span)) = physical {
                self.diagnostic_macro_call_site = Some(mark);
                self.diagnostic_macro_call_span = span.max(1);
            } else if self.diagnostic_macro_trace.is_empty() {
                let mut call_site = self.input.current_source_mark();
                let span = self.diagnostic_cs_source_width(invocation);
                if let Some(mark) = &mut call_site {
                    mark.rewind(span);
                }
                self.diagnostic_macro_call_site = call_site;
                self.diagnostic_macro_call_span = span.max(1);
            }
        }
        for id in [invocation, owner] {
            if self.diagnostic_macro_trace.last() != Some(&id) {
                if self.diagnostic_macro_trace.len() == MAX_TRACE {
                    // Entry zero names the physical call highlighted by the
                    // source excerpt. Keep it and discard the oldest inner
                    // frame so a deep trace cannot disagree with its caret.
                    self.diagnostic_macro_trace.remove(1);
                }
                self.diagnostic_macro_trace.push(id);
            }
        }
        self.diagnostic_token_from_file = false;
    }

    fn physical_source_for_cs(&self, id: crate::token::CsId) -> Option<(SourceMark, usize)> {
        let source = self
            .diagnostic_physical_source
            .filter(|source| source.semantic_cs == Some(id))?;
        let mark =
            self.input
                .source_mark_at(source.source_index, source.line, source.byte_column)?;
        Some((mark, source.span))
    }

    fn current_physical_source(&self) -> Option<(SourceMark, usize)> {
        let source = self.diagnostic_physical_source?;
        let current_cs = self
            .diagnostic_source_cs
            .or_else(|| self.cur_tok.is_cs().then(|| self.cur_tok.cs_id()));
        let matches = current_cs.map_or_else(
            || source.token == self.cur_tok,
            |id| source.semantic_cs == Some(id),
        );
        if !matches {
            return None;
        }
        let mark =
            self.input
                .source_mark_at(source.source_index, source.line, source.byte_column)?;
        Some((mark, source.span))
    }

    /// Capture the current source position and move it back to the token that
    /// began the current operation whenever it is still visible on the line.
    pub(crate) fn current_token_source_mark(&self) -> Option<SourceMark> {
        let current_cs = self
            .diagnostic_source_cs
            .or_else(|| self.cur_tok.is_cs().then(|| self.cur_tok.cs_id()));
        if let Some((token, mark, _)) = &self.diagnostic_synthetic_source {
            if current_cs == Some(*token) {
                return Some(mark.clone());
            }
        }
        if !self.diagnostic_macro_trace.is_empty() {
            if let Some(mark) = &self.diagnostic_macro_call_site {
                return Some(mark.clone());
            }
        }
        if let Some((mark, _)) = self.current_physical_source() {
            return Some(mark);
        }
        let mut mark = self.input.current_source_mark()?;
        if let Some(id) = self
            .diagnostic_source_cs
            .filter(|id| (*id as usize) < self.cs.len())
        {
            mark.rewind(self.diagnostic_cs_source_width(id));
        } else if self.cur_tok.is_cs() && (self.cur_tok.cs_id() as usize) < self.cs.len() {
            mark.rewind(self.diagnostic_cs_source_width(self.cur_tok.cs_id()));
        } else if self.cur_tok.is_char() {
            mark.rewind(1);
        }
        Some(mark)
    }

    pub(crate) fn record_group_opening(&mut self) {
        if let Some(source) = self.current_token_source_mark() {
            self.diagnostic_group_openings
                .push((self.eqtb.cur_level, source));
        }
    }

    pub(crate) fn push_group_level(&mut self, kind: crate::eqtb::LevelType) {
        self.eqtb.push_level(kind);
        self.record_group_opening();
    }

    pub(crate) fn forget_group_opening(&mut self, level: u16) {
        if self
            .diagnostic_group_openings
            .last()
            .is_some_and(|(opening_level, _)| *opening_level == level)
        {
            self.diagnostic_group_openings.pop();
        } else {
            self.diagnostic_group_openings
                .retain(|(opening_level, _)| *opening_level != level);
        }
    }

    pub fn error_at(&mut self, message: &str, source: Option<SourceContext>) {
        let saved = std::mem::replace(&mut self.diagnostic_source_override, source);
        self.error(message);
        self.diagnostic_source_override = saved;
    }

    pub fn fatal_error_at(&mut self, message: &str, source: Option<SourceContext>) {
        let saved = std::mem::replace(&mut self.diagnostic_source_override, source);
        self.fatal_error(message);
        self.diagnostic_source_override = saved;
    }

    pub fn current_diagnostic_line(&self) -> u32 {
        self.diagnostic_source_override
            .as_ref()
            .map_or_else(|| self.input.current_file_line(), |source| source.line)
    }

    pub(crate) fn make_error_diagnostic(&self, message: &str) -> Diagnostic {
        self.make_diagnostic(message, DiagnosticSeverity::Error)
    }

    fn make_diagnostic(&self, message: &str, severity: DiagnosticSeverity) -> Diagnostic {
        let configured_context = self.eqtb.int_params[IntParam::ErrorContextLines.idx() as usize];
        // Current LaTeX formats use -1 as their inherited/default value. A
        // literal clamp to zero silently hid every macro and include note in
        // the normal CLI experience. Keep an explicit zero as the opt-out,
        // while negative defaults select a small, bounded useful trace.
        let context_limit = if configured_context < 0 {
            DEFAULT_CONTEXT_FRAMES
        } else {
            (configured_context as usize).min(MAX_CONTEXT_FRAMES)
        };
        let macro_source = if self.diagnostic_source_override.is_none()
            && !self.diagnostic_macro_trace.is_empty()
        {
            self.diagnostic_macro_call_site
                .as_ref()
                .map(SourceMark::to_context)
        } else {
            None
        };
        let synthetic_source = if self.diagnostic_source_override.is_none()
            && self.diagnostic_macro_trace.is_empty()
        {
            self.diagnostic_synthetic_source
                .as_ref()
                .map(|(_, mark, _)| mark.to_context())
        } else {
            None
        };
        // Keep the exact span even when the caller has already converted this
        // same physical mark into a SourceContext for `error_at`.
        let physical = self.current_physical_source();
        let physical_context = physical.as_ref().map(|(mark, _)| mark.to_context());
        let physical_span = physical.as_ref().and_then(|(_, span)| {
            self.diagnostic_source_override
                .as_ref()
                .map_or(Some(*span), |source| {
                    physical_context.as_ref().and_then(|physical| {
                        (source.name == physical.name
                            && source.line == physical.line
                            && source.column == physical.column)
                            .then_some(*span)
                    })
                })
        });
        let physical_source = if self.diagnostic_source_override.is_none()
            && self.diagnostic_macro_trace.is_empty()
        {
            physical_context
        } else {
            None
        };
        let has_exact_source = self.diagnostic_source_override.is_some()
            || macro_source.is_some()
            || synthetic_source.is_some()
            || physical_source.is_some();
        let contexts = self
            .diagnostic_source_override
            .clone()
            .or(macro_source)
            .or(synthetic_source)
            .or(physical_source)
            .map_or_else(
                || self.input.source_contexts(),
                |source| {
                    crate::input::InputStack::source_context_chain(source, MAX_CONTEXT_FRAMES + 1)
                },
            );

        let trace = if let Some(trace) = &self.diagnostic_trace_override {
            trace.clone()
        } else if self.diagnostic_macro_trace.is_empty() {
            self.input
                .stack
                .iter()
                .filter_map(|source| match source {
                    Source::TokList {
                        owner: Some(owner), ..
                    } => Some(*owner),
                    _ => None,
                })
                .collect::<Vec<_>>()
        } else {
            self.diagnostic_macro_trace.clone()
        };
        let mut full_expansion = Vec::new();
        let trace_len = trace.len();
        for (index, id) in trace.into_iter().enumerate() {
            let name = self.display_cs(id).trim_end().to_string();
            let visible_endpoint =
                (index == 0 || index + 1 == trace_len) && name != "␠" && name != "⇥";
            if (visible_endpoint || !trace_noise(&name)) && full_expansion.last() != Some(&name) {
                full_expansion.push(name);
            }
        }

        let mut primary = contexts.first().cloned();
        let mut highlight_len = if !self.diagnostic_macro_trace.is_empty() {
            self.diagnostic_macro_call_span
        } else {
            self.diagnostic_synthetic_source
                .as_ref()
                .map_or_else(|| physical_span.unwrap_or(1), |(_, _, span)| *span)
        };
        if !has_exact_source {
            if let Some(source) = &mut primary {
                // Macro traces point to the outer user call. For direct undefined
                // commands the name in the message is reliable; `cur_tok` is not,
                // because deferred writes and scanners may have advanced it.
                let candidate = full_expansion.first().cloned().or_else(|| {
                    message
                        .strip_prefix("Undefined control sequence ")
                        .and_then(|rest| rest.split_whitespace().next())
                        .map(str::to_string)
                });
                if let Some(candidate) = candidate {
                    let searchable = candidate.strip_suffix('…').unwrap_or(&candidate);
                    if locate_candidate(source, searchable) {
                        highlight_len = searchable.len();
                    }
                }
            }
        }

        // `\\errorcontextlines` controls rendered notes, never the primary
        // call-site lookup above.
        let expansion = select_expansion_context(full_expansion, context_limit);

        let included_from = if context_limit == 0 {
            Vec::new()
        } else {
            contexts
                .iter()
                .skip(1)
                .cloned()
                // Include ancestry answers a different question from macro
                // ancestry. Give it its own bounded budget so LaTeX wrapper
                // macros cannot hide the file that included the failing one.
                .take(context_limit)
                .collect()
        };
        let custom_help = if self.diagnostic_use_err_help {
            let tokens = &self.eqtb.tok_params[ToksParam::ErrHelp.idx() as usize];
            self.diagnostic_tokens_to_string(tokens, MAX_HELP_BYTES)
                .trim()
                .to_string()
        } else {
            String::new()
        };
        let help = if custom_help.is_empty() || legacy_interactive_help(&custom_help) {
            default_help(message)
        } else {
            Some(clean_help(&custom_help))
        };

        Diagnostic {
            severity,
            message: bounded_text(message, MAX_MESSAGE_BYTES),
            primary,
            highlight_len,
            expansion,
            included_from,
            help,
        }
    }

    fn warning_at(&mut self, message: &str, source: Option<SourceContext>) {
        let saved = std::mem::replace(&mut self.diagnostic_source_override, source);
        let diagnostic = self.make_diagnostic(message, DiagnosticSeverity::Warning);
        self.diagnostic_source_override = saved;
        let rendered = diagnostic.render();
        self.diagnostic_print_nl(&rendered);
        self.diagnostics.push(diagnostic);
    }

    /// Diagnose structural input that reaches raw EOF. Canonical TeX treats
    /// the same state after an explicit `\end` as a non-fatal transcript
    /// warning and still writes the completed output.
    pub fn finish_job_diagnostics(&mut self) {
        if self.stopped_on_error || (self.ini_mode && self.format_done) {
            return;
        }
        let open_groups = self
            .eqtb
            .save_stack
            .iter()
            .filter_map(|item| match item {
                crate::eqtb::SaveItem::Level(level, kind) => Some((*level, *kind)),
                _ => None,
            })
            .collect::<Vec<_>>();

        if self.explicit_end_seen {
            if !self.if_stack.is_empty() {
                let count = self.if_stack.len();
                let state = self.if_stack.last().expect("nonempty conditional stack");
                let condition = if state.loc_cs == 0 {
                    "\\if".to_string()
                } else {
                    self.display_cs(state.loc_cs)
                };
                let opened = if state.loc_file.is_empty() {
                    format!("line {}", state.loc_line)
                } else {
                    format!("{}:{}", state.loc_file, state.loc_line)
                };
                let source = state.loc.as_ref().map(SourceMark::to_context);
                self.warning_at(
                    &format!(
                        "Unclosed conditional{} at \\end (missing \\fi); {condition} opened at {opened}",
                        if count == 1 {
                            String::new()
                        } else {
                            format!("s ({count})")
                        }
                    ),
                    source,
                );
            }
            if !open_groups.is_empty() {
                let opening = open_groups.iter().find_map(|(level, _)| {
                    self.diagnostic_group_openings
                        .iter()
                        .find(|(opening_level, _)| opening_level == level)
                        .map(|(_, source)| source.to_context())
                });
                let description = if self.scanner_status == crate::engine::ScannerStatus::Aligning {
                    "Unfinished alignment at \\end; add the missing \\cr and }"
                } else if self.mode.is_m() {
                    "Unclosed math formula at \\end; add the matching $"
                } else {
                    match open_groups[0].1 {
                        crate::eqtb::LevelType::SemiSimple => {
                            "Unclosed \\begingroup at \\end; add the missing \\endgroup"
                        }
                        crate::eqtb::LevelType::Box => {
                            "Unclosed box or alignment at \\end; add the missing }"
                        }
                        _ => "Unclosed group at \\end; add the missing }",
                    }
                };
                let nested = open_groups.len().saturating_sub(1);
                let message = if nested == 0 {
                    description.to_string()
                } else {
                    format!(
                        "{description} ({nested} nested group{} also open)",
                        if nested == 1 { "" } else { "s" }
                    )
                };
                self.warning_at(&message, opening);
            }
            return;
        }

        if !self.if_stack.is_empty() {
            let count = self.if_stack.len();
            let openings = self
                .if_stack
                .iter()
                .rev()
                .take(3)
                .map(|state| {
                    if state.loc_file.is_empty() {
                        format!("line {}", state.loc_line)
                    } else {
                        format!("{}:{}", state.loc_file, state.loc_line)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let opening = self
                .if_stack
                .last()
                .and_then(|state| state.loc.as_ref().map(SourceMark::to_context));
            self.error_at(
                &format!(
                    "Unclosed conditional{} (missing \\fi); opened at {}",
                    if count == 1 {
                        String::new()
                    } else {
                        format!("s ({count})")
                    },
                    openings
                ),
                opening,
            );
            if self.stopped_on_error {
                return;
            }
        }
        if !open_groups.is_empty() {
            let opening = open_groups.iter().find_map(|(level, _)| {
                self.diagnostic_group_openings
                    .iter()
                    .find(|(opening_level, _)| opening_level == level)
                    .map(|(_, source)| source.to_context())
            });
            let description = if self.scanner_status == crate::engine::ScannerStatus::Aligning {
                "Unfinished alignment; add the missing \\cr and }"
            } else if self.mode.is_m() {
                "Unclosed math formula; add the matching $"
            } else {
                match open_groups[0].1 {
                    crate::eqtb::LevelType::SemiSimple => {
                        "Unclosed \\begingroup; add the missing \\endgroup"
                    }
                    crate::eqtb::LevelType::Box => "Unclosed box or alignment; add the missing }",
                    _ => "Unclosed group; add the missing }",
                }
            };
            let nested = open_groups.len().saturating_sub(1);
            let message = if nested == 0 {
                description.to_string()
            } else {
                format!(
                    "{description} ({nested} nested group{} also open)",
                    if nested == 1 { "" } else { "s" }
                )
            };
            self.error_at(&message, opening);
            if self.stopped_on_error {
                return;
            }
        }
        if !self.if_stack.is_empty() || !open_groups.is_empty() {
            self.stopped_on_error = true;
            self.end_occurred = true;
        } else {
            self.fatal_error("Emergency stop: no legal \\end found");
        }
    }

    pub(crate) fn display_cs(&self, id: u32) -> String {
        let name = self.cs.name(id);
        if let Some(active) = active_character(name) {
            return match active {
                b' ' => "␠".to_string(),
                b'\t' => "⇥".to_string(),
                _ => char::from(active).to_string(),
            };
        }
        let shown = &name[..name.len().min(MAX_CONTROL_SEQUENCE_BYTES)];
        let mut result = format!("\\{}", String::from_utf8_lossy(shown));
        if shown.len() < name.len() {
            result.push('…');
        }
        bounded_inline(&result, MAX_CONTROL_SEQUENCE_BYTES + 8)
    }

    pub(crate) fn diagnostic_cs_source_width(&self, id: u32) -> usize {
        let name = self.cs.name(id);
        if active_character(name).is_some() {
            1
        } else {
            name.len().saturating_add(1)
        }
    }

    pub(crate) fn diagnostic_tokens_to_string(
        &self,
        toks: &[crate::token::Token],
        limit: usize,
    ) -> String {
        let mut bytes = Vec::with_capacity(limit.min(256));
        for token in toks {
            if bytes.len() >= limit {
                break;
            }
            if token.is_cs() {
                bytes.push(b'\\');
                let name = self.cs.name(token.cs_id());
                let room = limit.saturating_sub(bytes.len());
                bytes.extend_from_slice(&name[..name.len().min(room)]);
                if name.len() > 1 && bytes.len() < limit {
                    bytes.push(b' ');
                }
            } else {
                bytes.push(token.chr() as u8);
            }
        }
        let mut result = String::from_utf8_lossy(&bytes).into_owned();
        if bytes.len() == limit && !toks.is_empty() {
            result.push('…');
        }
        result
    }
}

fn active_character(name: &[u8]) -> Option<u8> {
    (name.len() == 7 && name[..6] == [0xff, 0, b'A', b'C', b'T', 0]).then(|| name[6])
}

fn trace_noise(name: &str) -> bool {
    name == "␠"
        || name == "⇥"
        || name.starts_with("\\reserved@")
        || name.starts_with("\\@if")
        || matches!(
            name,
            "\\@swaptwoargs" | "\\@input@file@exists@with@hooks" | "\\@hspace" | "\\@latex@error"
        )
}

fn select_expansion_context(full: Vec<String>, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    if full.len() <= limit {
        return full;
    }

    // The primary excerpt points to the outer call, while the newest frames
    // explain how execution reached the failure. The marker is not itself a
    // context frame, so a limit of two still shows both useful endpoints.
    let tail_start = full.len() - limit.saturating_sub(1);
    let mut shown = Vec::with_capacity(limit.saturating_add(1));
    shown.push(full[0].clone());
    shown.push("…".to_string());
    shown.extend(full.into_iter().skip(tail_start));
    shown
}

fn locate_candidate(source: &mut SourceContext, candidate: &str) -> bool {
    if candidate.is_empty() {
        return false;
    }
    let before = source
        .display_column
        .saturating_sub(1)
        .min(source.text.len());
    if !source.text.is_char_boundary(before) {
        return false;
    }
    let column = if source.text[before..].starts_with(candidate) {
        before
    } else if let Some(column) = source.text[..before].rfind(candidate) {
        column
    } else {
        return false;
    };
    let omitted = source.column.saturating_sub(source.display_column);
    source.column = omitted + column + 1;
    source.display_column = column + 1;
    true
}

fn bounded_text(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end.saturating_add(64));
    for ch in text[..end].chars() {
        if ch == '\n' || ch == '\t' || !ch.is_control() {
            out.push(ch);
        } else {
            out.push('�');
        }
    }
    if end < text.len() {
        out.push_str(&format!("… <{} bytes omitted>", text.len() - end));
    }
    out
}

fn bounded_inline(text: &str, limit: usize) -> String {
    bounded_text(text, limit)
        .chars()
        .map(|ch| if ch.is_control() { '�' } else { ch })
        .collect()
}

fn legacy_interactive_help(help: &str) -> bool {
    let lower = help.to_ascii_lowercase();
    lower.contains("<return>")
        || lower.contains("type  i ")
        || lower.contains("type i ")
        || lower.contains("your command was ignored")
}

fn clean_help(help: &str) -> String {
    help.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let rest = text.split_once(start)?.1;
    Some(rest.split_once(end)?.0)
}

fn default_help(message: &str) -> Option<String> {
    let static_help = if message.starts_with("Undefined control sequence") {
        Some("check the command spelling; if a package defines it, load that package before use")
    } else if message.contains("not found") || message.starts_with("Cannot read") {
        Some("check the file name and path, and make sure the file is visible in the TeX search paths")
    } else if message.starts_with("Cannot open") {
        Some("check the path and permissions for the file named in this error")
    } else if message.starts_with("Unclosed conditional") {
        Some("add the missing `\\fi` for the conditional opened at the reported location")
    } else if message.contains("Runaway")
        || message.starts_with("File ended")
        || message.starts_with("Unclosed")
        || message.starts_with("Unfinished alignment")
    {
        Some("look before this location for an unmatched `{`, a missing delimiter, or an unfinished environment")
    } else if message.contains("Missing $ inserted") {
        Some("balance `$...$` or `\\(...\\)` math delimiters near this location")
    } else if message.starts_with("Missing control sequence") {
        Some("supply a command name such as `\\name` after the definition or assignment command")
    } else if message.starts_with("Missing { inserted") {
        Some("add the opening `{` required by this command or group")
    } else if message.contains("Missing number") {
        Some("provide an integer or dimension where this command expects one")
    } else if message.starts_with("Number too big") {
        Some("use an integer no larger than 2147483647; TeX clamped this value to its maximum")
    } else if message.starts_with("Dimension too large") {
        Some("use a dimension no larger than 16383.99998pt in magnitude; TeX clamped this value")
    } else if message.starts_with("Cannot divide by zero") {
        Some("use a nonzero divisor; the target register or dimension was left unchanged")
    } else if message.starts_with("Arithmetic overflow") {
        Some("reduce the operands; the result is outside TeX's signed 32-bit numeric range")
    } else if message.contains("Illegal unit of measure") {
        Some("add a TeX dimension unit such as `pt`, `mm`, `cm`, or `em`")
    } else if message.contains("Too many }")
        || message.contains("Extra }")
        || message.contains("Extra \\endgroup")
    {
        Some("remove the extra closing brace, or add the corresponding opening brace earlier")
    } else if message.starts_with("Primitive not implemented")
        || message.contains("not implemented")
    {
        Some("this command is not supported by this engine yet; rewrite that construct or use another TeX engine")
    } else if message.starts_with("TeX capacity exceeded") {
        Some("check for recursive macros or runaway input before increasing an engine limit")
    } else if message.starts_with("Bad input stream number") {
        Some("TeX input streams are numbered from 0 through 15")
    } else if message.starts_with("Terminal input is unavailable") {
        Some("read from a file-backed stream instead of requesting interactive terminal input")
    } else if message.starts_with("Input stream ") && message.contains(" is not open for \\read") {
        Some("open this stream with `\\openin` before reading it, and check `\\ifeof` before each read")
    } else if message.starts_with("Parameters must be numbered consecutively")
        || message.starts_with("Illegal parameter number")
        || message.starts_with("Illegal parameter reference")
    {
        Some("number macro parameters in order as `#1`, `#2`, and so on, and use only those numbers in the replacement text")
    } else if message.contains("doesn't match its definition")
        || message.starts_with("Paragraph ended before")
        || message.starts_with("Argument of")
    {
        Some("check this macro call for missing, extra, or incorrectly delimited arguments")
    } else if message.contains("alignment tab")
        || message.starts_with("Misplaced \\cr")
        || message.starts_with("Misplaced \\omit")
        || message.starts_with("\\span outside alignment")
    {
        Some("check the surrounding alignment for an extra `&`, a missing `\\cr`, or an unmatched brace")
    } else if message.starts_with("Output routine didn't use all of \\box255") {
        Some("make the output routine ship or explicitly empty `\\box255` before it returns")
    } else if message.starts_with("Output loop") {
        Some("check that the output routine consumes `\\box255` and makes progress")
    } else if message.starts_with("You can't use") || message.contains(" outside alignment") {
        Some("move this command into the TeX mode or environment where it is valid")
    } else if message.starts_with("Text line contains an invalid character") {
        Some("remove the reported byte or save the input in the encoding expected by this engine")
    } else if message.starts_with("Missing \\endcsname") {
        Some("finish the control-sequence name with `\\endcsname`")
    } else if message.contains("perhaps a missing \\item") {
        Some("add `\\item` before the list contents, or move the text outside the list environment")
    } else if message.contains("can be used only in preamble") {
        Some("move this command before `\\begin{document}`")
    } else if message.contains("There's no line here to end") {
        Some("remove the extra line break command, or place it after text in a paragraph")
    } else if message.contains("already defined") {
        Some("rename the command or use the appropriate renewal command when replacing an existing definition")
    } else if message.contains("Option clash for package") {
        Some("load the package only once and combine its options at the first load site")
    } else if message.contains("Unknown option") {
        Some("check the option spelling and whether this package or document class supports it")
    } else if message.contains("Too deeply nested") {
        Some("reduce the nesting depth or split the nested environments into smaller parts")
    } else if message.starts_with("Emergency stop: no legal \\end found") {
        Some("finish the document with `\\end` (plain TeX) or `\\end{document}` (LaTeX)")
    } else {
        None
    };
    if let Some(help) = static_help {
        return Some(help.to_string());
    }

    if message.contains("ended by \\end{") {
        let opened = between(message, "\\begin{", "}")?;
        let closed = between(message, "ended by \\end{", "}")?;
        return Some(format!(
            "change `\\end{{{closed}}}` to `\\end{{{opened}}}`, or change the opening environment so the names match"
        ));
    }
    if let Some(environment) = between(message, "Environment ", " undefined") {
        return Some(format!(
            "check the spelling of `{environment}` and load the package that defines this environment"
        ));
    }
    None
}

fn char_display_width(ch: char) -> usize {
    let cp = ch as u32;
    if matches!(
        cp,
        0x0300..=0x036f
            | 0x1ab0..=0x1aff
            | 0x1dc0..=0x1dff
            | 0x20d0..=0x20ff
            | 0xfe20..=0xfe2f
    ) {
        0
    } else if matches!(
        cp,
        0x1100..=0x115f
            | 0x2329..=0x232a
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f300..=0x1faff
            | 0x20000..=0x3fffd
    ) {
        2
    } else {
        1
    }
}

fn expand_tabs_and_controls(text: &str, byte_column: usize) -> (Vec<(char, usize)>, usize) {
    let mut rendered = Vec::new();
    let mut caret = 0usize;
    let mut seen_column = false;
    let mut cells = 0usize;
    for (offset, ch) in text.char_indices() {
        if !seen_column && offset >= byte_column {
            caret = rendered.len();
            seen_column = true;
        }
        if ch == '\t' {
            let spaces = 4 - (cells % 4);
            rendered.extend(std::iter::repeat((' ', 1)).take(spaces));
            cells += spaces;
        } else if ch.is_control() {
            rendered.push(('�', 1));
            cells += 1;
        } else {
            let width = char_display_width(ch);
            rendered.push((ch, width));
            cells += width;
        }
    }
    if !seen_column {
        caret = rendered.len();
    }
    (rendered, caret)
}

fn source_window(text: &str, byte_column: usize, highlight_len: usize) -> (String, usize, usize) {
    let (chars, caret_index) = expand_tabs_and_controls(text, byte_column);
    let mut width = text
        .get(byte_column..byte_column.saturating_add(highlight_len))
        .map_or(1, |s| {
            s.chars().map(char_display_width).sum::<usize>().max(1)
        });
    let total_cells = chars.iter().map(|(_, cells)| cells).sum::<usize>();
    if total_cells <= MAX_SOURCE_COLUMNS {
        let caret = chars[..caret_index].iter().map(|(_, cells)| cells).sum();
        return (chars.into_iter().map(|(ch, _)| ch).collect(), caret, width);
    }

    let mut start = caret_index.saturating_sub(MAX_SOURCE_COLUMNS / 3);
    let mut end = (start + MAX_SOURCE_COLUMNS).min(chars.len());
    start = end.saturating_sub(MAX_SOURCE_COLUMNS);
    while chars[start..end]
        .iter()
        .map(|(_, cells)| cells)
        .sum::<usize>()
        + usize::from(start > 0)
        + usize::from(end < chars.len())
        > MAX_SOURCE_COLUMNS
    {
        if end > caret_index.saturating_add(1) {
            end -= 1;
        } else if start < caret_index {
            start += 1;
        } else {
            break;
        }
    }
    let left_ellipsis = start > 0;
    let right_ellipsis = end < chars.len();
    let mut shown = Vec::with_capacity(MAX_SOURCE_COLUMNS);
    if left_ellipsis {
        shown.push('…');
    }
    shown.extend(chars[start..end].iter().map(|(ch, _)| *ch));
    if right_ellipsis {
        shown.push('…');
    }
    let caret = chars[start..caret_index]
        .iter()
        .map(|(_, cells)| cells)
        .sum::<usize>()
        + usize::from(left_ellipsis);
    let shown_cells = chars[start..end]
        .iter()
        .map(|(_, cells)| cells)
        .sum::<usize>()
        + usize::from(left_ellipsis)
        + usize::from(right_ellipsis);
    width = width.min(shown_cells.saturating_sub(caret).max(1));
    (shown.into_iter().collect(), caret, width)
}

#[cfg(test)]
mod tests {
    use super::{select_expansion_context, source_window};

    #[test]
    fn expansion_context_limits_keep_the_outer_call_and_newest_frames() {
        let frames = ["outer", "middle", "inner"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        assert!(select_expansion_context(frames.clone(), 0).is_empty());
        assert_eq!(select_expansion_context(frames.clone(), 1), ["outer", "…"]);
        assert_eq!(select_expansion_context(frames, 2), ["outer", "…", "inner"]);
    }

    #[test]
    fn source_window_keeps_caret_visible_and_expands_tabs() {
        let long = format!("{}\t\\broken{}", "x".repeat(100), "y".repeat(100));
        let at = long.find("\\broken").unwrap();
        let (shown, caret, width) = source_window(&long, at, "\\broken".len());
        assert!(shown.chars().count() <= 120);
        assert_eq!(
            shown.chars().skip(caret).take(width).collect::<String>(),
            "\\broken"
        );
    }
}
