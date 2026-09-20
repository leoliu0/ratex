//! Input sources: files (line-based) and token lists (macros, args, registers).

use crate::token::{CsId, Token};
use std::collections::HashMap;
use std::rc::Rc;

pub const EOF_MARKER: Token = Token(0xFFFF_FFFF);
pub const PAR_END: Token = Token(0xFFFF_FFFE); // blank line -> \par
/// TeX82 bounds nested input sources (`stack_size`) instead of allowing a
/// recursive macro/input cycle to consume the host. Keep the same hard limit.
pub const MAX_INPUT_STACK: usize = 5_000;
/// Upper bound for one scanned macro argument or definition body. This is
/// deliberately comparable to modern TeX's main-memory capacity, while
/// preventing a malformed expansion cycle from growing a Vec without bound.
pub const MAX_TOKEN_LIST_TOKENS: usize = 5_000_000;
/// A diagnostic keeps only a local window of a physical source line. This
/// prevents one malformed megabyte-long line from being copied several times
/// while still leaving ample context for the 120-column renderer.
const MAX_DIAGNOSTIC_SOURCE_BYTES: usize = 4 * 1024;

/// A source position captured for a user-facing diagnostic. Columns are
/// one-based byte positions, matching TeX's byte-oriented input scanner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceContext {
    pub name: String,
    pub line: u32,
    pub column: usize,
    pub text: String,
    /// One-based byte position within the captured `text` window.
    pub(crate) display_column: usize,
    /// Immutable include chain. Source text for parents remains lazy until a
    /// diagnostic is rendered.
    included_from: Option<Rc<SourceMark>>,
}

/// Cheap source bookmark used on successful hot paths. The source excerpt is
/// materialized only if an error later needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub struct SourceMark {
    name: Rc<str>,
    data: Rc<[u8]>,
    line: u32,
    /// Byte offset of `line` in `data`. Keeping this in the bookmark avoids
    /// rescanning the file from byte zero whenever a diagnostic is rendered.
    line_start: usize,
    byte_column: usize,
    included_from: Option<Rc<SourceMark>>,
}

impl SourceMark {
    pub(crate) fn to_context(&self) -> SourceContext {
        let bytes = if self.line == 0 {
            &[][..]
        } else {
            let (end, _) = physical_line_bounds(&self.data, self.line_start);
            &self.data[self.line_start.min(end)..end]
        };
        let mut context = InputStack::context_from_line(
            self.name.to_string(),
            self.line,
            bytes,
            self.byte_column.min(bytes.len()),
        );
        context.included_from = self.included_from.clone();
        context
    }

    pub(crate) fn rewind(&mut self, bytes: usize) {
        self.byte_column = self.byte_column.saturating_sub(bytes);
    }
}

#[derive(Clone, Debug)]
pub enum Source {
    File {
        name: String,
        diagnostic_name: Rc<str>,
        data: Rc<[u8]>,
        /// Location in the parent source that opened this file.
        included_from: Option<Rc<SourceMark>>,
        pos: usize,
        line_no: u32,
        /// Byte offset of the current physical line in `data`.
        line_start: usize,
        /// tokenizer state: 0 = new line, 1 = mid line, 2 = skip spaces
        state: u8,
        /// set by \endinput: stop at end of current line
        ending: bool,
        /// true when the source has returned EOF already
        done: bool,
        /// pending ParEnd to deliver once
        pending_par: bool,
        at_eof: bool,
        line_buf: Option<Vec<u8>>,
        line_pos: usize,
        line_reload: bool,
    },
    TokList {
        toks: TokTokens,
        pos: usize,
        name: &'static str,
        owner: Option<CsId>,
        /// Number of macro ancestry entries that belong to this list.
        trace_depth: u8,
    },
    MacroFrame(MacroFrame),
}
#[derive(Clone, Debug)]
pub struct MacroFrame {
    pub body: std::rc::Rc<[Token]>,
    pub args: smallvec::SmallVec<[smallvec::SmallVec<[Token; 16]>; 9]>,
    pub references: std::rc::Rc<[(usize, usize)]>,
    pub ref_idx: usize,
    pub body_pos: usize,
    pub arg_pos: usize,
    pub name: &'static str,
    pub owner: Option<CsId>,
    pub trace_depth: u8,
    pub delivered_brace_balance: i32,
}

impl MacroFrame {
    #[inline(always)]
    pub fn is_exhausted(&self) -> bool {
        self.body_pos >= self.body.len()
    }

    #[inline(always)]
    pub fn next_token(&mut self) -> Option<Token> {
        while self.ref_idx < self.references.len() {
            let (param_pos, param_idx) = self.references[self.ref_idx];
            if self.body_pos < param_pos {
                let tok = self.body[self.body_pos];
                self.body_pos += 1;
                self.track_brace(tok);
                return Some(tok);
            }
            if let Some(arg) = self.args.get(param_idx) {
                if self.arg_pos < arg.len() {
                    let tok = arg[self.arg_pos];
                    self.arg_pos += 1;
                    self.track_brace(tok);
                    return Some(tok);
                }
            }
            self.body_pos += 1;
            self.arg_pos = 0;
            self.ref_idx += 1;
        }
        if self.body_pos < self.body.len() {
            let tok = self.body[self.body_pos];
            self.body_pos += 1;
            self.track_brace(tok);
            Some(tok)
        } else {
            None
        }
    }

    #[inline(always)]
    fn track_brace(&mut self, tok: Token) {
        if tok.is_char() {
            let cc = tok.cc();
            if cc == 1 {
                self.delivered_brace_balance += 1;
            } else if cc == 2 {
                self.delivered_brace_balance -= 1;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum TokTokens {
    Rc(std::rc::Rc<[Token]>),
    Vec(Vec<Token>),
}

impl std::ops::Deref for TokTokens {
    type Target = [Token];
    #[inline(always)]
    fn deref(&self) -> &[Token] {
        match self {
            TokTokens::Rc(r) => r,
            TokTokens::Vec(v) => v.as_slice(),
        }
    }
}

impl From<std::rc::Rc<[Token]>> for TokTokens {
    #[inline(always)]
    fn from(r: std::rc::Rc<[Token]>) -> Self {
        TokTokens::Rc(r)
    }
}

impl From<Vec<Token>> for TokTokens {
    #[inline(always)]
    fn from(v: Vec<Token>) -> Self {
        TokTokens::Vec(v)
    }
}

impl From<&[Token]> for TokTokens {
    #[inline(always)]
    fn from(s: &[Token]) -> Self {
        TokTokens::Vec(s.to_vec())
    }
}

pub struct InputStack {
    pub stack: Vec<Source>,
    file_bytes: HashMap<String, Rc<[u8]>>,
    disk_stamps: HashMap<String, (std::time::SystemTime, u64)>,
    /// The scanner normally pops a file before an EOF error is diagnosed.
    /// Retain its final position so runaway definitions and arguments do not
    /// degrade to the unhelpful "line 0" location.
    last_finished_file: Option<SourceContext>,
}

/// Return the content end and next-line offset for a physical line.
/// TeX accepts LF, CRLF, and legacy CR-only files.
pub(crate) fn physical_line_bounds(data: &[u8], start: usize) -> (usize, usize) {
    let start = start.min(data.len());
    let Some(offset) = data[start..]
        .iter()
        .position(|&byte| byte == b'\n' || byte == b'\r')
    else {
        return (data.len(), data.len());
    };
    let end = start + offset;
    let next = if data[end] == b'\r' && data.get(end + 1) == Some(&b'\n') {
        end + 2
    } else {
        end + 1
    };
    (end, next)
}
impl InputStack {
    pub fn new() -> Self {
        InputStack {
            stack: Vec::new(),
            file_bytes: HashMap::new(),
            disk_stamps: HashMap::new(),
            last_finished_file: None,
        }
    }

    /// Clear active sources and any EOF location retained for diagnostics.
    pub fn clear_sources(&mut self) {
        self.stack.clear();
        self.last_finished_file = None;
    }

    fn raw_line_at(data: &[u8], line_no: u32) -> (usize, &[u8]) {
        if line_no == 0 {
            return (0, &[]);
        }
        let mut start = 0usize;
        let mut current = 1u32;
        while current < line_no && start < data.len() {
            let (end, next) = physical_line_bounds(data, start);
            if end == next {
                return (data.len(), &[]);
            }
            start = next;
            current += 1;
        }
        let (end, _) = physical_line_bounds(data, start);
        (start, &data[start..end])
    }

    fn raw_line(data: &[u8], line_no: u32) -> &[u8] {
        Self::raw_line_at(data, line_no).1
    }

    fn context_for_at(source: &Source, byte_column: Option<usize>) -> Option<SourceContext> {
        let Source::File {
            name,
            diagnostic_name: _,
            data,
            included_from,
            line_no,
            line_start,
            line_buf,
            line_pos,
            ..
        } = source
        else {
            return None;
        };
        let bytes = line_buf.as_deref().unwrap_or_else(|| {
            if *line_no == 0 {
                &[][..]
            } else {
                let (end, _) = physical_line_bounds(data, *line_start);
                &data[(*line_start).min(end)..end]
            }
        });
        let byte_column = byte_column
            .unwrap_or_else(|| {
                if line_buf.is_some() {
                    *line_pos
                } else {
                    bytes.len()
                }
            })
            .min(bytes.len());
        let mut context = Self::context_from_line(name.clone(), *line_no, bytes, byte_column);
        context.included_from = included_from.clone();
        Some(context)
    }

    fn context_from_line(
        name: String,
        line: u32,
        bytes: &[u8],
        byte_column: usize,
    ) -> SourceContext {
        let mut start = byte_column.saturating_sub(MAX_DIAGNOSTIC_SOURCE_BYTES / 2);
        let end = (start + MAX_DIAGNOSTIC_SOURCE_BYTES).min(bytes.len());
        start = end.saturating_sub(MAX_DIAGNOSTIC_SOURCE_BYTES);
        let display_column = String::from_utf8_lossy(&bytes[start..byte_column]).len() + 1;
        SourceContext {
            name,
            line,
            column: byte_column + 1,
            text: String::from_utf8_lossy(&bytes[start..end]).into_owned(),
            display_column,
            included_from: None,
        }
    }

    fn context_for(source: &Source) -> Option<SourceContext> {
        Self::context_for_at(source, None)
    }

    pub(crate) fn source_context_at(
        &self,
        index: usize,
        byte_column: usize,
    ) -> Option<SourceContext> {
        self.stack
            .get(index)
            .and_then(|source| Self::context_for_at(source, Some(byte_column)))
    }

    /// Materialize an exact physical scanner coordinate only when a
    /// diagnostic needs it. Unlike `current_source_mark`, this remains
    /// correct after the scanner has consumed the rest of the token or moved
    /// to another line in the same still-open source.
    pub(crate) fn source_mark_at(
        &self,
        index: usize,
        line: u32,
        byte_column: usize,
    ) -> Option<SourceMark> {
        let Source::File {
            diagnostic_name,
            data,
            included_from,
            line_no,
            line_start,
            ..
        } = self.stack.get(index)?
        else {
            return None;
        };
        let line_start = if line == *line_no {
            *line_start
        } else {
            Self::raw_line_at(data, line).0
        };
        Some(SourceMark {
            name: diagnostic_name.clone(),
            data: data.clone(),
            line,
            line_start,
            byte_column,
            included_from: included_from.clone(),
        })
    }

    pub(crate) fn source_line_at(&self, index: usize) -> Option<u32> {
        match self.stack.get(index)? {
            Source::File { line_no, .. } => Some(*line_no),
            Source::TokList { .. } | Source::MacroFrame(_) => None,
        }
    }

    pub(crate) fn current_source_mark(&self) -> Option<SourceMark> {
        self.stack.iter().rev().find_map(|source| match source {
            Source::File {
                diagnostic_name,
                data,
                line_no,
                line_start,
                line_buf,
                line_pos,
                included_from,
                ..
            } => Some(SourceMark {
                name: diagnostic_name.clone(),
                data: data.clone(),
                line: *line_no,
                line_start: *line_start,
                byte_column: if line_buf.is_some() {
                    *line_pos
                } else if *line_no == 0 {
                    0
                } else {
                    let (end, _) = physical_line_bounds(data, *line_start);
                    end.saturating_sub(*line_start)
                },
                included_from: included_from.clone(),
            }),
            _ => None,
        })
    }

    /// Find the most recent physical spelling of text in an active source.
    /// This is used for errors whose TeX macro protocol reports the failure
    /// only after it has read ahead (notably LaTeX's missing-file prompt).
    /// Searching happens only on that error path, outside the scanner's hot
    /// loop, and returns a precise byte-oriented location.
    pub(crate) fn find_recent_text(&self, needle: &[u8]) -> Option<SourceMark> {
        if needle.is_empty() {
            return None;
        }
        self.stack.iter().rev().find_map(|source| {
            let Source::File {
                diagnostic_name,
                data,
                included_from,
                pos,
                line_no,
                line_buf,
                line_pos,
                ..
            } = source
            else {
                return None;
            };
            let consumed_end = if line_buf.is_some() && *line_no > 0 {
                let mut line_start = 0usize;
                for _ in 1..*line_no {
                    let (end, next) = physical_line_bounds(data, line_start);
                    if end == next {
                        break;
                    }
                    line_start = next;
                }
                line_start.saturating_add(*line_pos).min(data.len())
            } else {
                (*pos).min(data.len())
            };
            let consumed = &data[..consumed_end];
            let index = consumed
                .windows(needle.len())
                .rposition(|bytes| bytes == needle)?;
            let mut line_start = 0usize;
            let mut line = 1u32;
            while line_start < index {
                let (end, next) = physical_line_bounds(data, line_start);
                if index <= end || end == next {
                    break;
                }
                line_start = next;
                line = line.saturating_add(1);
            }
            Some(SourceMark {
                name: diagnostic_name.clone(),
                data: data.clone(),
                line,
                line_start,
                byte_column: index - line_start,
                included_from: included_from.clone(),
            })
        })
    }

    /// Current file followed by the exact locations that included it. If
    /// scanning reached global EOF, retain the last file's final position.
    pub fn source_contexts(&self) -> Vec<SourceContext> {
        self.current_source_context()
            .map_or_else(Vec::new, |source| Self::source_context_chain(source, 21))
    }

    /// Materialize only the current source. Hot-path provenance capture for
    /// deferred writes does not need to walk the include chain.
    pub fn current_source_context(&self) -> Option<SourceContext> {
        self.stack
            .iter()
            .rev()
            .find_map(Self::context_for)
            .or_else(|| self.last_finished_file.clone())
    }

    pub(crate) fn source_context_chain(
        mut source: SourceContext,
        limit: usize,
    ) -> Vec<SourceContext> {
        let mut contexts = Vec::with_capacity(limit.min(8));
        for _ in 0..limit {
            let parent = source.included_from.clone();
            contexts.push(source);
            let Some(parent) = parent else {
                break;
            };
            source = parent.to_context();
        }
        contexts
    }

    /// Remove an exhausted file while retaining its final source position.
    pub(crate) fn finish_file(&mut self, index: usize) {
        let source = if index + 1 == self.stack.len() {
            self.stack.pop()
        } else {
            Some(self.stack.remove(index))
        };
        if let Some(source) = source {
            if let Some(context) = Self::context_for(&source) {
                self.last_finished_file = Some(context);
            }
        }
    }

    #[inline]
    fn ensure_stack_room(&self) {
        assert!(
            self.stack.len() < MAX_INPUT_STACK,
            "TeX capacity exceeded, sorry [input stack size={MAX_INPUT_STACK}]"
        );
    }

    /// Test a compound input-stack push before mutating any of its sources.
    /// Engine entry points use this to turn user-triggered exhaustion into a
    /// normal fatal diagnostic; the assertion in the low-level push remains
    /// an internal invariant for direct callers.
    #[inline]
    pub(crate) fn has_stack_room(&self, needed: usize) -> bool {
        self.stack.len().saturating_add(needed) <= MAX_INPUT_STACK
    }

    pub fn cached_file(&self, key: &str) -> Option<Rc<[u8]>> {
        self.file_bytes.get(key).cloned()
    }

    pub fn read_file(&mut self, path: &std::path::Path) -> std::io::Result<Rc<[u8]>> {
        let key = path.to_string_lossy().into_owned();
        let meta = tex_kpse::fs::metadata(path)?;
        let stamp = meta.modified().ok().map(|mtime| (mtime, meta.len()));
        if stamp.is_some() && self.disk_stamps.get(&key).copied() == stamp {
            if let Some(bytes) = self.file_bytes.get(&key) {
                return Ok(bytes.clone());
            }
        }
        let bytes: Rc<[u8]> = tex_kpse::fs::read(path)?.into();
        self.file_bytes.insert(key.clone(), bytes.clone());
        if let Some(stamp) = stamp {
            self.disk_stamps.insert(key, stamp);
        }
        Ok(bytes)
    }

    /// A TeX output can alias an input under a different spelling. Drop all
    /// disk entries at openout rather than retaining stale aliases; active
    /// input sources retain their own Rc and embedded packages stay cached.
    pub fn invalidate_disk_files(&mut self) {
        for (key, _) in self.disk_stamps.drain() {
            self.file_bytes.remove(&key);
        }
    }

    pub fn intern_file(&mut self, key: String, data: Vec<u8>) -> Rc<[u8]> {
        if let Some(rc) = self.file_bytes.get(&key) {
            return rc.clone();
        }
        let rc: Rc<[u8]> = Rc::from(data);
        self.file_bytes.insert(key, rc.clone());
        rc
    }

    pub fn push_file(&mut self, name: String, data: impl Into<Rc<[u8]>>) {
        let included_from = self.current_source_mark();
        self.push_file_from(name, data, included_from);
    }

    pub(crate) fn push_file_from(
        &mut self,
        name: String,
        data: impl Into<Rc<[u8]>>,
        included_from: Option<SourceMark>,
    ) {
        self.ensure_stack_room();
        let diagnostic_name: Rc<str> = Rc::from(name.as_str());
        self.stack.push(Source::File {
            name,
            diagnostic_name,
            data: data.into(),
            included_from: included_from.map(Rc::new),
            pos: 0,
            line_no: 0,
            line_start: 0,
            state: 0,
            ending: false,
            done: false,
            pending_par: false,
            at_eof: false,
            line_buf: None,
            line_pos: 0,
            line_reload: true,
        });
    }
    pub fn push_toks(&mut self, toks: impl Into<TokTokens>, name: &'static str) {
        self.push_toks_owned(toks, name, None, 0);
    }

    pub(crate) fn push_toks_owned(
        &mut self,
        toks: impl Into<TokTokens>,
        name: &'static str,
        owner: Option<CsId>,
        trace_depth: u8,
    ) {
        self.ensure_stack_room();
        let toks = toks.into();
        assert!(
            toks.len() <= MAX_TOKEN_LIST_TOKENS,
            "TeX capacity exceeded, sorry [token list size={MAX_TOKEN_LIST_TOKENS}]"
        );
        self.stack.push(Source::TokList {
            toks,
            pos: 0,
            name,
            owner,
            trace_depth,
        });
    }

    pub fn current_file_name(&self) -> String {
        for s in self.stack.iter().rev() {
            if let Source::File { name, .. } = s {
                return name.clone();
            }
        }
        self.last_finished_file
            .as_ref()
            .map(|context| context.name.clone())
            .unwrap_or_default()
    }

    pub fn current_file_line(&self) -> u32 {
        for s in self.stack.iter().rev() {
            if let Source::File { line_no, .. } = s {
                return *line_no;
            }
        }
        self.last_finished_file
            .as_ref()
            .map_or(0, |context| context.line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_input_shares_bytes() {
        let blob = vec![b'a'; 1_000_000];
        let mut s = InputStack::new();
        let key = "big.tex".to_string();
        let rc = s.intern_file(key.clone(), blob);
        for _ in 0..4_000 {
            s.push_file(key.clone(), rc.clone());
        }
        // Each source owns the shared bytes; every nested include bookmark
        // after the first owns one more reference for its parent location.
        assert_eq!(Rc::strong_count(&rc), 2 + 4_000 + 3_999);
        assert_eq!(s.file_bytes.len(), 1);
    }

    #[test]
    fn disk_cache_shares_unchanged_reads_and_refreshes_after_writes() {
        let path = std::env::temp_dir().join(format!("tex-input-cache-{}.tex", std::process::id()));
        std::fs::write(&path, b"old").unwrap();
        let mut input = InputStack::new();
        let old = input.read_file(&path).unwrap();
        assert!(Rc::ptr_eq(&old, &input.read_file(&path).unwrap()));
        std::fs::write(&path, b"new contents").unwrap();
        assert_eq!(&*input.read_file(&path).unwrap(), b"new contents");
        input.invalidate_disk_files();
        std::fs::write(&path, b"same length!").unwrap();
        assert_eq!(&*input.read_file(&path).unwrap(), b"same length!");
        assert_eq!(&*old, b"old", "active input retains its original bytes");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn recent_text_uses_tex_line_endings_for_the_reported_location() {
        let bytes: Rc<[u8]> = Rc::from(&b"first\rneedle rest\rlast"[..]);
        let mut input = InputStack::new();
        input.stack.push(Source::File {
            name: "legacy-cr.tex".into(),
            diagnostic_name: Rc::from("legacy-cr.tex"),
            data: bytes,
            included_from: None,
            pos: 0,
            line_no: 2,
            line_start: b"first\r".len(),
            state: 1,
            ending: false,
            done: false,
            pending_par: false,
            at_eof: false,
            line_buf: Some(b"needle rest".to_vec()),
            line_pos: b"needle".len(),
            line_reload: false,
        });

        let context = input
            .find_recent_text(b"needle")
            .expect("consumed text")
            .to_context();
        assert_eq!(context.name, "legacy-cr.tex");
        assert_eq!(context.line, 2);
        assert_eq!(context.column, 1);
        assert_eq!(context.text, "needle rest");
    }

    #[test]
    fn unopened_file_mark_does_not_claim_the_first_line() {
        let mut input = InputStack::new();
        input.push_file("pending.tex".to_string(), b"first line\nsecond".to_vec());

        let context = input.current_source_mark().expect("file mark").to_context();

        assert_eq!(context.line, 0);
        assert_eq!(context.column, 1);
        assert!(context.text.is_empty());
    }
}
