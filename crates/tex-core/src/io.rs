//! I/O primitives: \input, \openout/\closeout/\write, \openin, \read,
//! \message, \special, \show, \lowercase/\uppercase, \advance arithmetic.

use crate::boxes::Node;
use crate::engine::Engine;
use crate::eqtb::Equiv;
use crate::prim::Prim;
use crate::token::Token;

const MAX_TEX_INPUT_STREAM: i32 = 15;

#[derive(Clone)]
struct ScannerDiagnosticState {
    macro_trace: Vec<crate::token::CsId>,
    token_from_file: bool,
    trace_hold: u16,
    source_cs: Option<crate::token::CsId>,
    physical_source: Option<crate::engine::PhysicalTokenSource>,
    macro_call_site: Option<crate::input::SourceMark>,
    macro_call_span: usize,
    synthetic_source: Option<(crate::token::CsId, crate::input::SourceMark, usize)>,
}

/// LaTeX's `\GenericError` still embeds instructions for TeX's interactive
/// question-and-answer loop in the `\errmessage` text. The CLI has explicit
/// interaction flags and never asks those questions, so retain the actual
/// error headline and move useful recovery advice to the diagnostic hint.
fn normalize_errmessage(text: &str) -> String {
    let trimmed = text.trim();
    let is_latex_style = is_latex_style_error(trimmed);
    if is_latex_style {
        trimmed
            .split("\n\n")
            .next()
            .unwrap_or(trimmed)
            .trim()
            .to_string()
    } else {
        trimmed.to_string()
    }
}

fn is_latex_style_error(text: &str) -> bool {
    text.starts_with("LaTeX Error:")
        || (text.starts_with("Package ") && text.contains(" Error:"))
        || (text.starts_with("Class ") && text.contains(" Error:"))
}

/// `\@missingfileerror` is the one major LaTeX error path that prints an
/// error with `\typeout` and then requests `\read-1`, rather than issuing an
/// `\errmessage`. Recover its headline before reporting that terminal input
/// is unavailable, so the missing file remains the primary, located error.
fn pending_latex_missing_file(buffer: &str) -> Option<(usize, String)> {
    const MARKER: &str = "! LaTeX Error: ";
    let start = buffer.rfind(MARKER)?;
    let tail = &buffer[start..];
    if !tail.contains(" not found.") || !tail.contains("Enter file name:") {
        return None;
    }
    let headline = tail.strip_prefix("! ")?.split("\n\n").next()?.trim();
    Some((start, headline.to_string()))
}

fn latex_missing_file_name(message: &str) -> Option<&str> {
    message
        .split_once("File `")?
        .1
        .split_once('\'')
        .map(|(name, _)| name)
}

/// Read through one physical line while retaining at most `limit` bytes.
/// The remainder is drained so a capacity error cannot leave the stream in
/// the middle of the offending line.
fn read_line_bounded(
    reader: &mut dyn std::io::BufRead,
    line: &mut Vec<u8>,
    limit: usize,
) -> std::io::Result<(bool, bool)> {
    let mut read_any = false;
    let mut overflow = false;
    loop {
        let (take, done) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                break;
            }
            read_any = true;
            let take = available
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(available.len(), |position| position + 1);
            let keep = take.min(limit.saturating_sub(line.len()));
            line.extend_from_slice(&available[..keep]);
            overflow |= keep < take;
            (take, available[take - 1] == b'\n')
        };
        reader.consume(take);
        if done {
            break;
        }
    }
    Ok((read_any, overflow))
}

impl Engine {
    fn scanner_diagnostic_state(&self) -> ScannerDiagnosticState {
        ScannerDiagnosticState {
            macro_trace: self.diagnostic_macro_trace.clone(),
            token_from_file: self.diagnostic_token_from_file,
            trace_hold: self.diagnostic_trace_hold,
            source_cs: self.diagnostic_source_cs,
            physical_source: self.diagnostic_physical_source,
            macro_call_site: self.diagnostic_macro_call_site.clone(),
            macro_call_span: self.diagnostic_macro_call_span,
            synthetic_source: self.diagnostic_synthetic_source.clone(),
        }
    }

    fn restore_scanner_diagnostic_state(&mut self, state: ScannerDiagnosticState) {
        self.diagnostic_macro_trace = state.macro_trace;
        self.diagnostic_token_from_file = state.token_from_file;
        self.diagnostic_trace_hold = state.trace_hold;
        self.diagnostic_source_cs = state.source_cs;
        self.diagnostic_physical_source = state.physical_source;
        self.diagnostic_macro_call_site = state.macro_call_site;
        self.diagnostic_macro_call_span = state.macro_call_span;
        self.diagnostic_synthetic_source = state.synthetic_source;
    }

    pub fn do_input(&mut self) {
        let included_from = self.current_token_source_mark();
        let name = self.scan_file_name();
        if name.is_empty() {
            self.error_at(
                "\\input needs a file name",
                included_from
                    .as_ref()
                    .map(crate::input::SourceMark::to_context),
            );
            return;
        }
        let _res = self.input_file_from(&name, included_from);
    }

    pub fn input_file(&mut self, name: &str) -> bool {
        let included_from = self.input.current_source_mark();
        self.input_file_from(name, included_from)
    }

    fn input_file_from(
        &mut self,
        name: &str,
        included_from: Option<crate::input::SourceMark>,
    ) -> bool {
        // Starting a file may also park pending lookahead below it. Reserve
        // both slots as one operation so recursive \input cannot trip the
        // low-level stack invariant after partially rearranging input.
        let needed = 1 + usize::from(!self.pushed.is_empty());
        if !self.input.has_stack_room(needed) {
            self.fatal_error_at(
                &format!(
                    "TeX capacity exceeded, sorry [input stack size={}]",
                    crate::input::MAX_INPUT_STACK
                ),
                included_from
                    .as_ref()
                    .map(crate::input::SourceMark::to_context),
            );
            return false;
        }
        let path = self.resolve_input_path(name);
        match path {
            Some(p) => {
                let key = p.to_string_lossy().into_owned();
                let data = match self.input.read_file(&p) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        self.error_at(
                            &format!("Cannot read {}: {}", name, e),
                            included_from
                                .as_ref()
                                .map(crate::input::SourceMark::to_context),
                        );
                        return false;
                    }
                };
                self.loaded_files.push(p.clone());
                self.term.push_str(&format!("({} ", p.display()));
                // tex.web start_input: the file sits above the current
                // token list. `pushed` is that token list, so leftovers
                // must park below the file even during \\output — else
                // hook-csname tokens sit on top of an unread .fd.
                if !self.pushed.is_empty() {
                    let mut rest = std::mem::take(&mut self.pushed);
                    rest.reverse();
                    if !self.try_push_tokens_named(rest, "<after-input>") {
                        return false;
                    }
                }
                self.input.push_file_from(key, data, included_from);
                true
            }
            None => {
                // Fall back to embedded Virtual TDS package repository
                let clean_name = std::path::Path::new(name)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name);
                let cand_names = [
                    clean_name.to_string(),
                    format!("{}.sty", clean_name),
                    format!("{}.cls", clean_name),
                ];
                let mut found_data = None;
                for cand in &cand_names {
                    let key = format!("<embedded:{cand}>");
                    if let Some(rc) = self.input.cached_file(&key) {
                        found_data = Some((key, rc));
                        break;
                    }
                    if let Some(pkg_data) = tex_kpse::get_embedded_package(cand) {
                        let rc = self.input.intern_file(key.clone(), pkg_data);
                        found_data = Some((key, rc));
                        break;
                    }
                }
                if let Some((key, data)) = found_data {
                    self.term.push_str(&format!("({key} "));
                    if !self.pushed.is_empty() {
                        let mut rest = std::mem::take(&mut self.pushed);
                        rest.reverse();
                        if !self.try_push_tokens_named(rest, "<after-input>") {
                            return false;
                        }
                    }
                    self.input.push_file_from(key, data, included_from);
                    return true;
                }
                self.error_at(
                    &format!("File `{}` not found", name),
                    included_from
                        .as_ref()
                        .map(crate::input::SourceMark::to_context),
                );
                false
            }
        }
    }

    /// Resolve `name` for \input/\openin the way web2c's open_input does:
    /// when -output-directory is set, relative names are looked up there
    /// first (kpathsea's TEXMF_OUTPUT_DIRECTORY behavior), then kpathsea's
    /// format search path (TDS, env paths, cwd, explicit paths).
    pub fn resolve_input_path(&self, name: &str) -> Option<std::path::PathBuf> {
        if name.is_empty() {
            return None;
        }
        // Format-build boot: babel's language.dat chain (ruhyph16, coptic,
        // english.ldf, ...) drags the full babel \protect machinery into
        // initex and dies ("\protect invalid in file"), and babel's
        // hyphen.cfg local-configuration pass (loaded at the end of
        // latex.ltx) trips "Missing \begin{document}" + a pending
        // \aftergroup that blocks \dump. The engine pre-loads english
        // hyphenation itself (pdflatex.rs hyphen_trie), so serve minimal
        // stand-ins instead of the babel machinery.
        if name == "language.dat" || name == "language.dat.lua" {
            let guard = std::env::temp_dir().join("tex-language-dat-guard.dat");
            let _ = std::fs::write(&guard, b"english hyphen.tex\n");
            return Some(guard);
        }
        if name == "hyphen.cfg" {
            let guard = std::env::temp_dir().join("tex-hyphen-cfg-guard.cfg");
            let _ = std::fs::write(
                &guard,
                b"\\chardef\\l@nohyphenation=255\n\\chardef\\l@english=0\n\\chardef\\l@USenglish=0\n\\def\\languagename{english}\n\\relax\n",
            );
            return Some(guard);
        }
        if name == "fontspec.sty" {
            let guard = std::env::temp_dir().join("tex-fontspec-stub.sty");
            let _ = std::fs::write(
                &guard,
                b"\\ProvidesPackage{fontspec}[2026/01/01 v2.9 Rust compatibility stub]\n\
                  \\def\\@fontspec@gobbleopt[#1]{}\n\
                  \\def\\@fontspec@cmd{\\@ifnextchar[{\\@fontspec@opt}{\\@fontspec@noopt}}\n\
                  \\def\\@fontspec@opt[#1]#2{\\@ifnextchar[{\\@fontspec@gobbleopt}{}}\n\
                  \\def\\@fontspec@noopt#1{\\@ifnextchar[{\\@fontspec@gobbleopt}{}}\n\
                  \\let\\setmainfont\\@fontspec@cmd\n\
                  \\let\\setsansfont\\@fontspec@cmd\n\
                  \\let\\setmonofont\\@fontspec@cmd\n\
                  \\def\\newfontfamily#1{\\@fontspec@cmd}\n\
                  \\def\\setfontfamily#1{\\@fontspec@cmd}\n\
                  \\def\\newfontface#1{\\@fontspec@cmd}\n\
                  \\providecommand\\addfontfeatures[2][]{}\n\
                  \\providecommand\\fontspec[2][]{}\n\
                  \\providecommand\\defaultfontfeatures[2][]{}\n\
                  \\providecommand\\emfontdeclare[1]{}\n\
                  \\providecommand\\strongfontdeclare[1]{}\n\
                  \\@ifundefined{DeclareUnicodeCharacter}{}{%\n\
                    \\DeclareUnicodeCharacter{2212}{\\ensuremath{-}}%\n\
                    \\DeclareUnicodeCharacter{2013}{--}%\n\
                    \\DeclareUnicodeCharacter{2014}{---}%\n\
                    \\DeclareUnicodeCharacter{2018}{`}%\n\
                    \\DeclareUnicodeCharacter{2019}{'}%\n\
                    \\DeclareUnicodeCharacter{201C}{``}%\n\
                    \\DeclareUnicodeCharacter{201D}{''}%\n\
                    \\DeclareUnicodeCharacter{2026}{\\dots}%\n\
                    \\DeclareUnicodeCharacter{00D7}{\\ensuremath{\\times}}%\n\
                    \\DeclareUnicodeCharacter{2264}{\\ensuremath{\\le}}%\n\
                    \\DeclareUnicodeCharacter{2265}{\\ensuremath{\\ge}}%\n\
                    \\DeclareUnicodeCharacter{2260}{\\ensuremath{\\ne}}%\n\
                    \\DeclareUnicodeCharacter{2208}{\\ensuremath{\\in}}%\n\
                    \\DeclareUnicodeCharacter{2192}{\\ensuremath{\\to}}%\n\
                    \\DeclareUnicodeCharacter{221E}{\\ensuremath{\\infty}}%\n\
                    \\DeclareUnicodeCharacter{2202}{\\ensuremath{\\partial}}%\n\
                    \\DeclareUnicodeCharacter{00B7}{\\ensuremath{\\cdot}}%\n\
                  }\n\
                  \\endinput\n",
            );
            return Some(guard);
        }
        if name == "unicode-math.sty" {
            let guard = std::env::temp_dir().join("tex-unicode-math-stub.sty");
            let _ = std::fs::write(
                &guard,
                b"\\ProvidesPackage{unicode-math}[2026/01/01 v0.9 Rust compatibility stub]\n\
                  \\RequirePackage{amsmath,amssymb}\n\
                  \\providecommand\\setmathfont[2][]{}\n\
                  \\providecommand\\unimathsetup[1]{}\n\
                  \\endinput\n",
            );
            return Some(guard);
        }
        if name == "luacode.sty" {
            let guard = std::env::temp_dir().join("tex-luacode-stub.sty");
            let _ = std::fs::write(
                &guard,
                b"\\ProvidesPackage{luacode}[2026/01/01 v1.0 Rust compatibility stub]\n\
                  \\long\\def\\luaexec#1{}\n\
                  \\def\\luacode{\\begingroup\\catcode`\\^^M=12 \\luacode@scan}\n\
                  \\def\\luacode@scan#1\\endluacode{\\endgroup}\n\
                  \\endinput\n",
            );
            return Some(guard);
        }
        if name == "luatextra.sty" {
            let guard = std::env::temp_dir().join("tex-luatextra-stub.sty");
            let _ = std::fs::write(
                &guard,
                b"\\ProvidesPackage{luatextra}[2026/01/01 v1.0 Rust compatibility stub]\n\
                  \\RequirePackage{fontspec}\n\
                  \\endinput\n",
            );
            return Some(guard);
        }
        if name == "luaotfload.sty" {
            let guard = std::env::temp_dir().join("tex-luaotfload-stub.sty");
            let _ = std::fs::write(
                &guard,
                b"\\ProvidesPackage{luaotfload}[2026/01/01 v1.0 Rust compatibility stub]\n\
                  \\endinput\n",
            );
            return Some(guard);
        }
        if !name.starts_with('/') && !self.out_dir.is_empty() {
            for cand in [
                std::path::Path::new(&self.out_dir).join(name),
                std::path::Path::new(&self.out_dir).join(format!("{name}.tex")),
            ] {
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
        if !name.starts_with('/') {
            for cand in [
                std::path::Path::new(name).to_path_buf(),
                std::path::Path::new(&format!("{name}.tex")).to_path_buf(),
            ] {
                if cand.is_file() {
                    return Some(cand);
                }
            }
            if let Some(dir) = &self.main_dir {
                for cand in [dir.join(name), dir.join(format!("{name}.tex"))] {
                    if cand.is_file() {
                        return Some(cand);
                    }
                }
            }
        }
        self.font_loader
            .kpse
            .find(name, tex_kpse::Format::Tex)
            .or_else(|| {
                let basename = std::path::Path::new(name).file_name()?.to_str()?;
                self.font_loader.kpse.find_any(basename)
            })
    }

    pub fn do_endinput(&mut self) {
        // end the current file after the current line
        for s in self.input.stack.iter_mut().rev() {
            if let crate::input::Source::File { ending, .. } = s {
                *ending = true;
                return;
            }
        }
    }
    /// tex.web §966/§967: `\patterns{...}` (INITEX only) and
    /// `\hyphenation{...}`. Letters pass through \lccode (lccode 0 drops
    /// the character); digits and '.' carry pattern values; '-' marks
    /// exception break points. Entries are separated by spaces.
    pub fn do_hyphenation_words(&mut self, is_patterns: bool) {
        if is_patterns && !self.ini_mode {
            self.error("\\patterns can be used only in INITEX mode");
            return;
        }
        self.skip_spaces_relax();
        let open = self.get_token();
        if !(open.is_char() && open.cc() == 1) {
            self.error("Missing { inserted (\\patterns or \\hyphenation)");
            return;
        }
        let toks = self.scan_balanced_raw(true);
        let mut words: Vec<String> = Vec::new();
        let mut cur = String::new();
        for t in &toks {
            if !t.is_char() {
                self.error(if is_patterns {
                    "Letter expected in \\patterns"
                } else {
                    "Letter expected in \\hyphenation"
                });
                continue;
            }
            if t.cc() == 10 {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
                continue;
            }
            let c = t.chr() as u8;
            if is_patterns && (c.is_ascii_digit() || c == b'.') {
                cur.push(c as char);
            } else if c == b'-' && !is_patterns {
                cur.push('-');
            } else {
                let lc = self.eqtb.lc_code[c as usize];
                if lc != 0 {
                    cur.push(lc as char);
                }
            }
        }
        if !cur.is_empty() {
            words.push(cur);
        }
        for w in words {
            if is_patterns {
                self.hyphen_trie.add_pattern(&w);
            } else {
                self.hyphen_trie.add_exception(&w);
            }
        }
    }

    /// tex.web §1393: `\openout`/`\closeout` create whatsit nodes
    /// (`open_node`/`close_node`); only `\immediate` executes them on the
    /// spot. Deferred ones ride the current list into the shipped box and
    /// take effect in `out_what` (§1414) at shipout time, in list order —
    /// so a `\write` after a deferred `\closeout` on the same page still
    /// reaches the still-open file.
    pub fn do_openout(&mut self, immediate: bool) {
        // \openout<n>=<file>
        let n = self.scan_int();
        self.scan_optional_equals();
        let name = self.scan_file_name();
        // web2c open_output: the -output-directory prefix applies to
        // relative names only; absolute names bypass it. Path::join so a
        // missing trailing slash cannot fuse into "dirfile.ext".
        let full = if !self.out_dir.is_empty() && !name.starts_with('/') {
            std::path::Path::new(&self.out_dir)
                .join(&name)
                .to_string_lossy()
                .into_owned()
        } else {
            name.clone()
        };
        // §1394: stream numbers <0 map to 17, >15 to 16
        let stream = if n < 0 { 17u16 } else { n.min(16) as u16 };
        if !immediate {
            self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::OpenOut {
                stream,
                path: full,
            }));
            return;
        }
        self.exec_openout(stream, &full);
    }

    /// the actual file open, shared by `\immediate\openout` and the
    /// shipout-time whatsit executor (`out_what` closes a previously open
    /// stream first, §1417)
    pub fn exec_openout(&mut self, stream: u16, full: &str) {
        // §1414: streams 16/17 (the >15 and negative aliases) are never
        // actually opened
        if stream >= 16 {
            return;
        }
        let idx = (stream as usize).min(self.write_streams.len() - 1);
        if self.write_streams[idx].take().is_some() {
            // canonical: an open on a busy stream closes the old file first
        }
        match std::fs::File::create(full) {
            Ok(f) => {
                self.input.invalidate_disk_files();
                self.write_streams[idx] = Some(f);
            }
            Err(e) => self.error(&format!("Cannot open {} for writing: {}", full, e)),
        }
    }

    pub fn do_closeout(&mut self, immediate: bool) {
        let n = self.scan_int();
        let stream = if n < 0 { 17u16 } else { n.min(16) as u16 };
        if !immediate {
            self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::CloseOut { stream }));
            return;
        }
        self.exec_closeout(stream);
    }

    pub fn exec_closeout(&mut self, stream: u16) {
        if stream >= 16 {
            return;
        }
        let idx = (stream as usize).min(self.write_streams.len() - 1);
        if let Some(f) = self.write_streams[idx].take() {
            drop(f);
        }
    }

    pub fn do_write(&mut self, immediate: bool) {
        let n = self.scan_int();
        // tex.web §1371: \write<n>{toks} collects the list RAW (scan_toks,
        // no expansion) and expands at emission like \xdef (protected macros
        // stay frozen).
        let toks = self.scan_general_text();
        // Capture after the list is scanned so a failing token within the
        // deferred expansion can be located by searching back from here.
        let source = self.input.current_source_context();
        // All non-immediate writes are structural whatsits, including the
        // log-only \write-1{} sentinel used by LaTeX's \clearpage.
        // TeX maps negative streams to 17 and streams above 15 to 16.
        if !immediate {
            self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::Write {
                stream: if n < 0 { 17 } else { n.min(16) as u16 },
                tokens: toks,
                source,
            }));
            return;
        }
        let text = self.expand_write_list(&toks, source.as_ref());
        self.write_out(if n < 0 { -1 } else { n.min(16) }, &text);
    }
    /// tex.web §1395 out_what: a Write whatsit fires at ship time, expanding
    pub fn fire_write(
        &mut self,
        stream: u16,
        tokens: &[Token],
        source: Option<&crate::input::SourceContext>,
    ) {
        let text = self.expand_write_list(tokens, source);
        self.write_out(if stream == 17 { -1 } else { stream as i32 }, &text);
    }
    /// Expand a raw `\write` token list to its emitted string. The write gets
    /// a private input stack: a delimited macro may consume the sentinel, but
    /// it must never continue into the active output routine or document.
    fn expand_write_list(
        &mut self,
        toks: &[Token],
        source: Option<&crate::input::SourceContext>,
    ) -> String {
        let saved = std::mem::take(&mut self.pushed);
        let saved_input = std::mem::take(&mut self.input.stack);
        let saved_diagnostic_state = self.scanner_diagnostic_state();
        let saved_end_occurred = self.end_occurred;
        let errors_before = self.error_count;
        let saved_source = std::mem::replace(&mut self.diagnostic_source_override, source.cloned());
        // LaTeX \set@display@protect contract: at write/emit time \protect
        // is \noexpand, so `\protect\BOOKMARK` emits `\BOOKMARK` raw
        // instead of running it (hyperref .out writes).
        let protect_saved = self.cs.lookup(b"protect").and_then(|pid| {
            let noexpand = self
                .cs
                .lookup(b"noexpand")
                .and_then(|nid| self.eqtb.get(nid).cloned())?;
            let old = self.eqtb.replace_equiv_temporarily(pid, Some(noexpand));
            Some((pid, old))
        });
        // The sentinel bounds the expansion: without it, a list whose final
        // token expands away would let get_token continue into the OUTER
        // stream and leak its tokens into the write string.
        let mut body = toks.to_vec();
        body.push(crate::page::WRITE_END_TOKEN);
        self.push_tokens_named(body, "<write>");
        let prev = self.in_expanded_scan;
        self.in_expanded_scan = true;
        let mut out: Vec<Token> = Vec::new();
        loop {
            let t = self.get_token();
            if t == crate::page::WRITE_END_TOKEN || t == crate::input::EOF_MARKER {
                break;
            }
            if out.len() >= crate::input::MAX_TOKEN_LIST_TOKENS {
                self.fatal_error_at(
                    &format!(
                        "TeX capacity exceeded, sorry [write expansion size={}]",
                        crate::input::MAX_TOKEN_LIST_TOKENS
                    ),
                    source.cloned(),
                );
                break;
            }
            out.push(t);
        }
        self.in_expanded_scan = prev;
        self.input.stack = saved_input;
        self.restore_scanner_diagnostic_state(saved_diagnostic_state);
        self.end_occurred =
            saved_end_occurred || (self.end_occurred && self.error_count > errors_before);
        self.diagnostic_source_override = saved_source;
        self.pushed = saved;
        if let Some((pid, old)) = protect_saved {
            self.eqtb.replace_equiv_temporarily(pid, old);
        }
        self.write_tokens_to_string(&out)
    }

    pub fn write_tokens_to_string(&self, toks: &[Token]) -> String {
        let mut out = Vec::new();
        for t in toks {
            if t.is_cs() {
                let name = self.cs.name(t.cs_id());
                // Active-char placeholder ids (engine::active_cs_name):
                // [0xFF,0,'A','C','T',0,c] — detokenize as the character
                // byte c, not the internal name.
                if let [0xff, 0, b'A', b'C', b'T', 0, c] = name {
                    out.push(*c);
                } else {
                    out.push(b'\\');
                    out.extend_from_slice(name);
                    out.push(b' ');
                }
            } else {
                out.push(t.chr() as u8);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    pub fn write_out(&mut self, n: i32, text: &str) {
        let line = format!("{}\n", text);
        match n {
            -1 => {
                self.append_log(&line);
            }
            -2 => {
                self.append_term(&line);
            }
            16 | 17 | 18 => {
                self.append_term(&line);
                self.append_log(&line);
            }
            _ => {
                let idx = (n as usize).min(self.write_streams.len() - 1);
                match &mut self.write_streams[idx] {
                    Some(f) => {
                        use std::io::Write;
                        let _ = f.write_all(line.as_bytes());
                    }
                    None => {
                        // tex.web §1382: a \write to a closed stream is
                        // directed to the log and the terminal. \typeout
                        // rides \write\@unused (stream 0, never opened), so
                        // this arm is what makes it visible.
                        self.append_term(&line);
                        self.append_log(&line);
                    }
                }
            }
        }
    }

    pub fn do_special(&mut self) {
        let _ = self.scan_general_text_expanded();
    }

    pub fn do_message(&mut self, err: bool) {
        let origin = self.current_token_source_mark();
        let toks = self.scan_general_text_expanded();
        if self.stopped_on_error {
            return;
        }
        let text = self.write_tokens_to_string(&toks);
        if err {
            let text = normalize_errmessage(&text);
            let previous = std::mem::replace(&mut self.diagnostic_use_err_help, true);
            let previous_trace = if is_latex_style_error(&text) {
                Some(std::mem::replace(
                    &mut self.diagnostic_trace_override,
                    Some(Vec::new()),
                ))
            } else {
                None
            };
            self.error(&text);
            if let Some(previous_trace) = previous_trace {
                self.diagnostic_trace_override = previous_trace;
            }
            self.diagnostic_use_err_help = previous;
        } else {
            let trimmed = text.trim_start();
            if trimmed.starts_with("! LaTeX Error:") && trimmed.contains(" not found.") {
                let named_source = latex_missing_file_name(trimmed).and_then(|name| {
                    self.input.find_recent_text(name.as_bytes()).or_else(|| {
                        std::path::Path::new(name)
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .and_then(|stem| self.input.find_recent_text(stem.as_bytes()))
                    })
                });
                self.pending_terminal_error_source = named_source
                    .as_ref()
                    .or(origin.as_ref())
                    .map(crate::input::SourceMark::to_context);
            }
            self.append_term(&text);
            self.append_log(&text);
        }
    }

    pub fn do_openin(&mut self) {
        let n = self.scan_int();
        self.scan_optional_equals();
        let name = self.scan_file_name();
        if !(0..=MAX_TEX_INPUT_STREAM).contains(&n) {
            self.error(&format!(
                "Bad input stream number {n} for \\openin (expected 0..={MAX_TEX_INPUT_STREAM})"
            ));
            return;
        }
        while self.read_files.len() <= n as usize {
            self.read_files.push(None);
            self.read_eof.push(true);
        }
        // kpathsea/web2c lookup: output directory first for relative
        // names, then the kpse search path (covers literal paths too)

        let path = self.resolve_input_path(&name);
        if let Some(p) = path {
            if let Ok(f) = std::fs::File::open(&p) {
                self.read_files[n as usize] = Some(Box::new(std::io::BufReader::new(f)));
                self.read_eof[n as usize] = false;
                return;
            }
        }
        // Fall back to embedded package archive
        let clean_name = std::path::Path::new(&name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&name);
        for cand in [
            clean_name.to_string(),
            format!("{clean_name}.tex"),
            format!("{clean_name}.sty"),
            format!("{clean_name}.cls"),
        ] {
            if let Some(data) = tex_kpse::get_embedded_package(&cand) {
                self.read_files[n as usize] = Some(Box::new(std::io::Cursor::new(data)));
                self.read_eof[n as usize] = false;
                return;
            }
        }
        self.read_files[n as usize] = None;
        self.read_eof[n as usize] = true;
    }

    pub fn do_closein(&mut self) {
        let n = self.scan_int();
        if !(0..=MAX_TEX_INPUT_STREAM).contains(&n) {
            self.error(&format!(
                "Bad input stream number {n} for \\closein (expected 0..={MAX_TEX_INPUT_STREAM})"
            ));
            return;
        }
        let n = n as usize;
        while self.read_files.len() <= n {
            self.read_files.push(None);
            self.read_eof.push(true);
        }
        self.read_files[n] = None;
        self.read_eof[n] = true;
    }

    pub fn do_read(&mut self, line_mode: bool) {
        let origin = self.current_token_source_mark();
        let global = self.take_global();
        let stream = self.scan_int();
        if !self.scan_keyword(b"to") {
            self.error("Missing `to' inserted for \\read");
        }
        let cs = self.scan_definable_cs();
        if !(0..=MAX_TEX_INPUT_STREAM).contains(&stream) {
            let pending = pending_latex_missing_file(&self.term)
                .or_else(|| pending_latex_missing_file(&self.log));
            if let Some((_, message)) = pending {
                if let Some((start, _)) = pending_latex_missing_file(&self.term) {
                    self.term.truncate(start);
                }
                if let Some((start, _)) = pending_latex_missing_file(&self.log) {
                    self.log.truncate(start);
                }
                let message_source = latex_missing_file_name(&message).and_then(|name| {
                    self.input.find_recent_text(name.as_bytes()).or_else(|| {
                        std::path::Path::new(name)
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .and_then(|stem| self.input.find_recent_text(stem.as_bytes()))
                    })
                });
                let source = message_source
                    .as_ref()
                    .map(crate::input::SourceMark::to_context)
                    .or_else(|| self.pending_terminal_error_source.take())
                    .or_else(|| origin.as_ref().map(crate::input::SourceMark::to_context));
                let saved_trace =
                    std::mem::replace(&mut self.diagnostic_trace_override, Some(Vec::new()));
                self.fatal_error_at(&message, source);
                self.diagnostic_trace_override = saved_trace;
            } else {
                self.fatal_error_at(
                    &format!("Terminal input is unavailable for \\read{stream}"),
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
            }
            return;
        }
        let mut toks = Vec::new();
        let mut balance = 0i32;
        loop {
            let mut line = Vec::new();
            let read = if (0..16).contains(&stream) {
                self.read_files
                    .get_mut(stream as usize)
                    .and_then(Option::as_mut)
                    .map(|reader| {
                        read_line_bounded(
                            reader.as_mut(),
                            &mut line,
                            crate::input::MAX_TOKEN_LIST_TOKENS,
                        )
                    })
            } else {
                None
            };
            let Some(read) = read else {
                self.fatal_error_at(
                    &format!("Input stream {stream} is not open for \\read"),
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                break;
            };
            let eof = match read {
                Ok((false, _)) => true,
                Ok((true, false)) => false,
                Ok((_, true)) => {
                    self.fatal_error_at(
                        &format!(
                            "TeX capacity exceeded, sorry [read line size={}]",
                            crate::input::MAX_TOKEN_LIST_TOKENS
                        ),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    self.clear_prefixes();
                    return;
                }
                Err(error) => {
                    self.error(&format!("Cannot read input stream: {error}"));
                    true
                }
            };
            if eof {
                self.read_files[stream as usize] = None;
                self.read_eof[stream as usize] = true;
                if balance != 0 {
                    self.error("File ended within \\read");
                    break;
                }
            }
            while matches!(line.last(), Some(b'\n' | b'\r' | b' ')) {
                line.pop();
            }
            let endline = self.eqtb.int_params[crate::prim::IntParam::EndLineChar.idx() as usize];
            if line_mode {
                if (0..256).contains(&endline) {
                    if line.len() >= crate::input::MAX_TOKEN_LIST_TOKENS {
                        self.fatal_error_at(
                            &format!(
                                "TeX capacity exceeded, sorry [read token list size={}]",
                                crate::input::MAX_TOKEN_LIST_TOKENS
                            ),
                            origin.as_ref().map(crate::input::SourceMark::to_context),
                        );
                        self.clear_prefixes();
                        return;
                    }
                    line.push(endline as u8);
                }
                if toks.len().saturating_add(line.len()) > crate::input::MAX_TOKEN_LIST_TOKENS {
                    self.fatal_error_at(
                        &format!(
                            "TeX capacity exceeded, sorry [read token list size={}]",
                            crate::input::MAX_TOKEN_LIST_TOKENS
                        ),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    self.clear_prefixes();
                    return;
                }
                toks.extend(
                    line.into_iter()
                        .map(|b| Token::char(if b == b' ' { 10 } else { 12 }, u32::from(b))),
                );
            } else {
                // Reuse the file tokenizer without exposing the surrounding
                // input stack or its pending expansion tokens to this read.
                if line.len() >= crate::input::MAX_TOKEN_LIST_TOKENS {
                    self.fatal_error_at(
                        &format!(
                            "TeX capacity exceeded, sorry [read token list size={}]",
                            crate::input::MAX_TOKEN_LIST_TOKENS
                        ),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    self.clear_prefixes();
                    return;
                }
                line.push(b'\n');
                let diagnostic_state = self.scanner_diagnostic_state();
                let outer = std::mem::replace(&mut self.input, crate::input::InputStack::new());
                self.input.push_file("<read>".into(), line);
                loop {
                    let mut token = self.get_next_raw();
                    if token == crate::input::EOF_MARKER {
                        break;
                    }
                    if token == crate::input::PAR_END {
                        token = Token::from_cs(self.partoken_id());
                    }
                    if token.is_char() {
                        if token.cc() == 1 {
                            balance += 1;
                        }
                        if token.cc() == 2 {
                            balance -= 1;
                        }
                    }
                    if balance < 0 {
                        balance = 0;
                        break;
                    }
                    if toks.len() >= crate::input::MAX_TOKEN_LIST_TOKENS {
                        self.input = outer;
                        self.restore_scanner_diagnostic_state(diagnostic_state);
                        self.fatal_error_at(
                            &format!(
                                "TeX capacity exceeded, sorry [read token list size={}]",
                                crate::input::MAX_TOKEN_LIST_TOKENS
                            ),
                            origin.as_ref().map(crate::input::SourceMark::to_context),
                        );
                        self.clear_prefixes();
                        return;
                    }
                    toks.push(token);
                }
                self.input = outer;
                self.restore_scanner_diagnostic_state(diagnostic_state);
            }
            if line_mode || balance == 0 || eof {
                break;
            }
        }
        let m = crate::eqtb::Macro {
            replacement: Default::default(),
            num_params: 0,
            has_param_refs: false,
            params: Vec::new(),
            body: toks.into(),
            prefix: Vec::new(),
            long: false,
            outer: false,
            protected: false,
        };
        self.eqtb
            .assign(cs, Equiv::Macro(std::rc::Rc::new(m)), global);
        self.clear_prefixes();
    }

    pub fn scan_file_name(&mut self) -> String {
        const MAX_FILE_NAME_BYTES: usize = 4096;
        let mut origin = (self.input.current_file_line() != 0)
            .then(|| self.current_token_source_mark())
            .flatten();
        self.skip_spaces_relax();
        let mut name = Vec::new();
        let t = self.get_x_raw();
        if origin.is_none() {
            origin = self
                .current_token_source_mark()
                .or_else(|| self.input.current_source_mark());
        }
        if t == crate::input::EOF_MARKER {
            return String::new();
        }
        if t.is_char() && (t.cc() == 1 || t.chr() == b'{' as u32) {
            // LaTeX \input{filename.tex} syntax
            let mut depth = 1i32;
            loop {
                let t2 = self.get_x_raw();
                if t2 == crate::input::EOF_MARKER {
                    self.fatal_error_at(
                        "File ended while scanning a braced file name; add the missing }",
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return String::new();
                }
                if t2.is_char() {
                    if t2.cc() == 1 || t2.chr() == b'{' as u32 {
                        depth += 1;
                    } else if t2.cc() == 2 || t2.chr() == b'}' as u32 {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    if name.len() == MAX_FILE_NAME_BYTES {
                        self.fatal_error_at(
                            "TeX capacity exceeded, sorry [file name exceeds 4096 bytes]",
                            origin.as_ref().map(crate::input::SourceMark::to_context),
                        );
                        return String::new();
                    }
                    name.push(t2.chr() as u8);
                } else if t2.is_cs() {
                    let additional = self.cs.name(t2.cs_id()).len();
                    if additional > MAX_FILE_NAME_BYTES.saturating_sub(name.len()) {
                        self.fatal_error_at(
                            "TeX capacity exceeded, sorry [file name exceeds 4096 bytes]",
                            origin.as_ref().map(crate::input::SourceMark::to_context),
                        );
                        return String::new();
                    }
                    name.extend_from_slice(self.cs.name(t2.cs_id()));
                }
            }
            return String::from_utf8_lossy(&name).trim().to_string();
        }
        if t.is_char() && t.chr() == b'"' as u32 {
            // pdfTeX quoted filename: "name with spaces.tex"
            loop {
                let t2 = self.get_x_raw();
                if t2 == crate::input::EOF_MARKER {
                    self.fatal_error_at(
                        "File ended while scanning a quoted file name; add the closing quote",
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return String::new();
                }
                if t2.is_char() && t2.chr() == b'"' as u32 {
                    // tex.web start_input: the one space following the closing
                    // quote terminates the filename scan and is consumed.
                    // (@filef@und = \"name\" + space; leaking it typesets a
                    // stray interword space in the using box.)
                    let t3 = self.get_x_raw();
                    if !(t3.is_char() && t3.cc() == 10) && t3 != crate::input::EOF_MARKER {
                        self.pushed.push(t3);
                    }
                    break;
                }
                if t2.is_char() {
                    if name.len() == MAX_FILE_NAME_BYTES {
                        self.fatal_error_at(
                            "TeX capacity exceeded, sorry [file name exceeds 4096 bytes]",
                            origin.as_ref().map(crate::input::SourceMark::to_context),
                        );
                        return String::new();
                    }
                    name.push(t2.chr() as u8);
                } else if t2.is_cs() {
                    let additional = self.cs.name(t2.cs_id()).len();
                    if additional > MAX_FILE_NAME_BYTES.saturating_sub(name.len()) {
                        self.fatal_error_at(
                            "TeX capacity exceeded, sorry [file name exceeds 4096 bytes]",
                            origin.as_ref().map(crate::input::SourceMark::to_context),
                        );
                        return String::new();
                    }
                    name.extend_from_slice(self.cs.name(t2.cs_id()));
                }
            }
            return String::from_utf8_lossy(&name).trim().to_string();
        }
        // standard TeX \input filename.tex (unquoted). Real TeX expands
        // macros while scanning a filename (TeXbook ch.8: \openin0=pre\foo.tex
        // finds preprobe.tex); an UNEXPANDABLE cs terminates the scan and is
        // re-read. Without expansion, \input pgflibrary\pgf@temp.code.tex
        // opens "pgflibrary" and leaks "\pgf@temp.code.tex" into the text.
        let mut cur = t;
        loop {
            if cur == crate::input::EOF_MARKER {
                break;
            }
            if cur.is_space() || (cur.is_char() && cur.cc() == 10) {
                break;
            }
            if cur.is_cs() {
                self.pushed.push(cur);
                break;
            }
            if cur.is_char() {
                let c = cur.chr() as u8;
                if c == b' ' || c == b'\t' || c == b'\r' || c == b'\n' {
                    break;
                }
                if name.len() == MAX_FILE_NAME_BYTES {
                    self.fatal_error_at(
                        "TeX capacity exceeded, sorry [file name exceeds 4096 bytes]",
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return String::new();
                }
                name.push(c);
            }
            cur = self.get_x_raw();
        }
        String::from_utf8_lossy(&name).trim().to_string()
    }

    pub fn do_show(&mut self) {
        // tex.web \show grabs the target with get_name (NON-expanding)
        let t = self.raw_token();
        let text = self.meaning_of(t);
        self.term.push_str(&format!("> {}\n", text));
        self.log.push_str(&format!("> {}\n", text));
    }

    pub fn shift_case(&mut self, toks: &mut Vec<Token>, up: bool) {
        for t in toks.iter_mut() {
            // tex.web §1289: only character tokens; CS names are unchanged.
            if t.is_cs() {
                continue;
            }
            let c = t.chr() as u8;
            let mapped = if up {
                self.eqtb.uc_code[c as usize]
            } else {
                self.eqtb.lc_code[c as usize]
            };
            // lccode/uccode 0 = leave unchanged
            if mapped != 0 {
                *t = Token::char(t.cc(), mapped as u32);
            }
        }
    }

    pub fn do_advance(&mut self) {
        // \advance<quantity> by <int/dimen/glue>
        let origin = self.current_token_source_mark();
        let global = self.take_global();
        let loc = self.scan_quantity();
        self.scan_keyword(b"by");
        let v = match loc {
            QuantityLoc::Int(_) | QuantityLoc::Count(_) => Value::Int(self.scan_int()),
            QuantityLoc::Dim(_) | QuantityLoc::Dimen(_) => Value::Dim(self.scan_dimen(false, true)),
            QuantityLoc::Glue(_) | QuantityLoc::Skip(_) => Value::Glue(self.scan_glue(false)),
            QuantityLoc::MuSkip(_) => Value::Glue(self.scan_glue(true)),
            QuantityLoc::None => return,
        };
        match loc {
            QuantityLoc::Int(p) => {
                let cur = self.int_param_value(p);
                let Some(nv) = self.checked_advance(cur, v.as_int(), false, origin.as_ref()) else {
                    return;
                };
                if p == crate::prim::IntParam::SpaceFactor {
                    self.space_factor = nv;
                } else {
                    self.eqtb.assign_int_param(p, nv, global);
                }
            }
            QuantityLoc::Count(i) => {
                let cur = self.eqtb.count[i as usize];
                if let Some(value) = self.checked_advance(cur, v.as_int(), false, origin.as_ref()) {
                    self.eqtb.assign_count(i, value, global);
                }
            }
            QuantityLoc::Dim(p) => {
                let cur = self.dim_param_value(p);
                let Some(nv) = self.checked_advance(cur, v.as_dim(), true, origin.as_ref()) else {
                    return;
                };
                if p == crate::prim::DimParam::PrevDepth {
                    self.prev_depth = nv;
                } else {
                    self.eqtb.assign_dim_param(p, nv, global);
                }
            }
            QuantityLoc::Dimen(i) => {
                let cur = self.eqtb.dimen[i as usize];
                if let Some(value) = self.checked_advance(cur, v.as_dim(), true, origin.as_ref()) {
                    self.eqtb.assign_dimen(i, value, global);
                }
            }
            QuantityLoc::Glue(p) => {
                let cur = self.eqtb.glue_params[p.idx() as usize].clone();
                if let Some(value) = self.checked_glue_advance(&cur, &v.as_glue(), origin.as_ref()) {
                    self.eqtb.assign_glue_param(p, value, global);
                }
            }
            QuantityLoc::Skip(i) => {
                let cur = self.eqtb.skip[i as usize].clone();
                if let Some(value) = self.checked_glue_advance(&cur, &v.as_glue(), origin.as_ref()) {
                    self.eqtb.assign_skip(i, value, global);
                }
            }
            QuantityLoc::MuSkip(i) => {
                let cur = self.eqtb.muskip[i as usize].clone();
                if let Some(value) = self.checked_glue_advance(&cur, &v.as_glue(), origin.as_ref()) {
                    self.eqtb.assign_muskip(i, value, global);
                }
            }
            QuantityLoc::None => {}
        }
    }

    pub fn do_arith(&mut self, op: u8) {
        // \multiply / \divide
        let origin = self.current_token_source_mark();
        let global = self.take_global();
        let loc = self.scan_quantity();
        self.scan_keyword(b"by");
        let v = match loc {
            QuantityLoc::Int(_) | QuantityLoc::Count(_) => Value::Int(self.scan_int()),
            QuantityLoc::Dim(_) | QuantityLoc::Dimen(_) => Value::Int(self.scan_int()),
            _ => Value::Int(self.scan_int()),
        };
        let n = v.as_int();
        match loc {
            QuantityLoc::Int(p) => {
                let cur = self.int_param_value(p);
                if let Some(nv) = self.checked_arith(cur, n, op, origin.as_ref()) {
                    if p == crate::prim::IntParam::SpaceFactor {
                        self.space_factor = nv;
                    } else {
                        self.eqtb.assign_int_param(p, nv, global);
                    }
                }
            }
            QuantityLoc::Count(i) => {
                let cur = self.eqtb.count[i as usize];
                if let Some(value) = self.checked_arith(cur, n, op, origin.as_ref()) {
                    self.eqtb.assign_count(i, value, global);
                }
            }
            QuantityLoc::Dim(p) => {
                let cur = self.dim_param_value(p);
                if let Some(nv) = self.checked_arith(cur, n, op, origin.as_ref()) {
                    if p == crate::prim::DimParam::PrevDepth {
                        self.prev_depth = nv;
                    } else {
                        self.eqtb.assign_dim_param(p, nv, global);
                    }
                }
            }
            QuantityLoc::Dimen(i) => {
                let cur = self.eqtb.dimen[i as usize];
                if let Some(value) = self.checked_arith(cur, n, op, origin.as_ref()) {
                    self.eqtb.assign_dimen(i, value, global);
                }
            }
            QuantityLoc::Glue(p) => {
                let mut cur = self.eqtb.glue_params[p.idx() as usize].clone();
                let Some((width, stretch, shrink)) = self.checked_glue_arith(
                    cur.width,
                    cur.stretch,
                    cur.shrink,
                    n,
                    op,
                    origin.as_ref(),
                ) else {
                    return;
                };
                cur.width = width;
                cur.stretch = stretch;
                cur.shrink = shrink;
                self.eqtb.assign_glue_param(p, cur, global);
            }
            QuantityLoc::Skip(i) => {
                let mut cur = self.eqtb.skip[i as usize].clone();
                let Some((width, stretch, shrink)) = self.checked_glue_arith(
                    cur.width,
                    cur.stretch,
                    cur.shrink,
                    n,
                    op,
                    origin.as_ref(),
                ) else {
                    return;
                };
                cur.width = width;
                cur.stretch = stretch;
                cur.shrink = shrink;
                self.eqtb.assign_skip(i, cur, global);
            }
            _ => {}
        }
    }

    fn checked_advance(
        &mut self,
        current: i32,
        increment: i32,
        dimension: bool,
        origin: Option<&crate::input::SourceMark>,
    ) -> Option<i32> {
        const MAX_DIMEN: i64 = 0x3FFF_FFFF;
        let value = i64::from(current) + i64::from(increment);
        if !(i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&value)
            || (dimension && !(-MAX_DIMEN..=MAX_DIMEN).contains(&value))
        {
            self.error_at(
                "Arithmetic overflow in \\advance; value left unchanged",
                origin.map(crate::input::SourceMark::to_context),
            );
            None
        } else {
            Some(value as i32)
        }
    }

    fn checked_glue_advance(
        &mut self,
        current: &crate::boxes::Glue,
        increment: &crate::boxes::Glue,
        origin: Option<&crate::input::SourceMark>,
    ) -> Option<crate::boxes::Glue> {
        const MAX_DIMEN: i64 = 0x3FFF_FFFF;
        let checked_sum = |left: i32, right: i32| {
            let value = i64::from(left) + i64::from(right);
            (-MAX_DIMEN..=MAX_DIMEN)
                .contains(&value)
                .then_some(value as i32)
        };
        let result = (|| {
            let mut value = current.clone();
            value.width = checked_sum(value.width, increment.width)?;
            if value.stretch_order == increment.stretch_order {
                value.stretch = checked_sum(value.stretch, increment.stretch)?;
            } else if value.stretch_order < increment.stretch_order {
                value.stretch = increment.stretch;
                value.stretch_order = increment.stretch_order;
            }
            if value.shrink_order == increment.shrink_order {
                value.shrink = checked_sum(value.shrink, increment.shrink)?;
            } else if value.shrink_order < increment.shrink_order {
                value.shrink = increment.shrink;
                value.shrink_order = increment.shrink_order;
            }
            Some(value)
        })();
        if result.is_none() {
            self.error_at(
                "Arithmetic overflow in \\advance; value left unchanged",
                origin.map(crate::input::SourceMark::to_context),
            );
        }
        result
    }

    fn checked_glue_arith(
        &mut self,
        width: i32,
        stretch: i32,
        shrink: i32,
        operand: i32,
        op: u8,
        origin: Option<&crate::input::SourceMark>,
    ) -> Option<(i32, i32, i32)> {
        Some((
            self.checked_arith(width, operand, op, origin)?,
            self.checked_arith(stretch, operand, op, origin)?,
            self.checked_arith(shrink, operand, op, origin)?,
        ))
    }

    fn checked_arith(
        &mut self,
        a: i32,
        b: i32,
        op: u8,
        origin: Option<&crate::input::SourceMark>,
    ) -> Option<i32> {
        // TeX reports arithmetic faults and leaves the quantity unchanged.
        // Silent saturation or division to zero can produce plausible but
        // incorrect output, which is much harder to debug.
        let result = if op == 1 {
            a.checked_mul(b)
        } else {
            a.checked_div(b)
        };
        if result.is_none() {
            let message = if op != 1 && b == 0 {
                "Cannot divide by zero in \\divide; value left unchanged"
            } else if op == 1 {
                "Arithmetic overflow in \\multiply; value left unchanged"
            } else {
                "Arithmetic overflow in \\divide; value left unchanged"
            };
            self.error_at(message, origin.map(crate::input::SourceMark::to_context));
        }
        result
    }

    pub fn do_setbox(&mut self) {
        let idx = self.scan_reg_num();
        self.scan_optional_equals();
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        if !t.is_cs() {
            self.pushed.push(t);
            self.error("Missing box for \\setbox");
            return;
        }
        let prim = match self.eqtb.resolve(t.cs_id()) {
            Some(Equiv::Prim(p)) => Some(*p),
            _ => None,
        };
        if let Some(prim) = prim {
            match prim {
                Prim::Box => {
                    let n = self.scan_reg_num();
                    let b = self.eqtb.take_box(n);
                    let g = self.take_global();
                    self.eqtb.assign_box(idx, b, g);
                    return;
                }
                Prim::Copy => {
                    let n = self.scan_reg_num();
                    let b = self.eqtb.boxed.get(n as usize).cloned().flatten();
                    let g = self.take_global();
                    self.eqtb.assign_box(idx, b, g);
                    return;
                }
                Prim::LastBox => {
                    let b = self.take_last_box();
                    let g = self.take_global();
                    self.eqtb.assign_box(idx, b, g);
                    return;
                }
                Prim::HBox | Prim::VBox | Prim::VTop | Prim::VCenter => {
                    self.park_setbox(idx);
                    let kind = match prim {
                        Prim::HBox => 0,
                        Prim::VBox => 1,
                        Prim::VTop => 2,
                        _ => 3,
                    };
                    self.begin_box(kind);
                    return;
                }
                Prim::VSplit => {
                    let g = self.take_global();
                    let (top, m, rest) = self.scan_vsplit();
                    if let Some(rest) = rest {
                        self.stash_vsplit_remainder(m, rest);
                    }
                    self.eqtb.assign_box(idx, top, g);
                    return;
                }
                _ => {}
            }
        }

        match self.cs.name(t.cs_id()) {
            b"box" => {
                let n = self.scan_reg_num();
                let b = self.eqtb.take_box(n);
                let g = self.take_global();
                self.eqtb.assign_box(idx, b, g);
            }
            b"copy" => {
                let n = self.scan_reg_num();
                let b = self.eqtb.boxed[n as usize].clone();
                self.eqtb.assign_box(idx, b, self.global_flag);
                self.global_flag = false;
            }
            b"lastbox" => {
                let b = self.take_last_box();
                self.eqtb.assign_box(idx, b, self.global_flag);
                self.global_flag = false;
            }
            b"hbox" | b"vbox" | b"vtop" | b"vcenter" => {
                self.park_setbox(idx);
                let kind = match self.cs.name(t.cs_id()) {
                    b"hbox" => 0,
                    b"vbox" => 1,
                    b"vtop" => 2,
                    _ => 3,
                };
                self.begin_box(kind);
            }
            b"halign" => {
                self.park_setbox(idx);
                self.begin_halign();
            }
            b"usebox" => {
                let n = self.scan_reg_num();
                let b = self.eqtb.boxed[n as usize].take();
                self.eqtb.assign_box(idx, b, self.global_flag);
                self.global_flag = false;
            }
            b"vsplit" => {
                let g = self.take_global();
                let (top, m, rest) = self.scan_vsplit();
                if let Some(rest) = rest {
                    self.stash_vsplit_remainder(m, rest);
                }
                self.eqtb.assign_box(idx, top, g);
            }
            _ => {
                self.pushed.push(t);
                self.error("Missing box for \\setbox");
            }
        }
    }

    /// scan the target of \advance/\multiply: an int/dim/glue quantity
    fn scan_quantity(&mut self) -> QuantityLoc {
        self.skip_spaces_relax();
        let t = self.get_token();
        if !t.is_cs() {
            self.pushed.push(t);
            self.error("Missing quantity for \\advance");
            return QuantityLoc::None;
        }
        let id = t.cs_id();
        match self.cur_prim {
            Some(Prim::IntP(p)) => return QuantityLoc::Int(p),
            Some(Prim::DimP(p)) => return QuantityLoc::Dim(p),
            Some(Prim::GlueP(p)) => return QuantityLoc::Glue(p),
            Some(Prim::Count) => {
                let i = self.scan_reg_num();
                return QuantityLoc::Count(i);
            }
            Some(Prim::Dimen) => {
                let i = self.scan_reg_num();
                return QuantityLoc::Dimen(i);
            }
            Some(Prim::Skip) => {
                let i = self.scan_reg_num();
                return QuantityLoc::Skip(i);
            }
            Some(Prim::MuSkip) => {
                let i = self.scan_reg_num();
                return QuantityLoc::MuSkip(i);
            }
            _ => {}
        }
        match self.eqtb.resolve(id).cloned() {
            Some(Equiv::CountReg(i)) => QuantityLoc::Count(i),
            Some(Equiv::DimenReg(i)) => QuantityLoc::Dimen(i),
            Some(Equiv::SkipReg(i)) => QuantityLoc::Skip(i),
            Some(Equiv::MuSkipReg(i)) => QuantityLoc::MuSkip(i),
            Some(Equiv::Prim(Prim::IntP(p))) => QuantityLoc::Int(p),
            Some(Equiv::Prim(Prim::DimP(p))) => QuantityLoc::Dim(p),
            Some(Equiv::Prim(Prim::GlueP(p))) => QuantityLoc::Glue(p),
            _ => {
                self.error("Unknown quantity for \\advance");
                QuantityLoc::None
            }
        }
    }

    pub fn box_to_string(&self, b: &Node) -> String {
        let mut s = String::new();
        self.box_repr(b, 0, &mut s);
        s
    }

    fn box_repr(&self, b: &Node, depth: usize, out: &mut String) {
        if depth > 4 {
            return;
        }
        if let Node::Box {
            kind,
            w,
            h,
            d,
            shift,
            list,
            ..
        } = b
        {
            let k = match kind {
                0 => "\\hbox",
                1 => "\\vbox",
                2 => "\\vtop",
                _ => "\\vcenter",
            };
            out.push_str(&format!(
                "{}({}, height {}, depth {}, width {}",
                k,
                crate::scaled::ONE / 1000, // placeholder not used
                self.scaled_to_string(*h),
                self.scaled_to_string(*d),
                self.scaled_to_string(*w),
            ));
            if *shift != 0 {
                out.push_str(&format!(", shifted {}", self.scaled_to_string(*shift)));
            }
            out.push_str(")[\n");
            for n in list {
                self.node_repr(n, depth + 1, out);
            }
            out.push_str("]\n");
        }
    }

    fn node_repr(&self, n: &Node, depth: usize, out: &mut String) {
        for _ in 0..depth {
            out.push_str("  ");
        }
        match n {
            Node::Char { c, font } => {
                out.push_str(&format!("the character {} (font {})\n", *c as char, font))
            }
            Node::Glue(g) => out.push_str(&format!("glue {}\n", self.glue_to_string(g))),
            Node::Kern(k) => out.push_str(&format!("kern {}\n", self.scaled_to_string(*k))),
            // tex.web §4416: an explicit kern is shown with a space after
            // the escape (`\kern 1.0`), an implicit one without (`\kern1.0`)
            Node::ExplicitKern(k) => out.push_str(&format!("kern {}\n", self.scaled_to_string(*k))),
            // pdftex §4302 prints margin kerns with their side annotated
            Node::MarginKern { side, width, .. } => out.push_str(&format!(
                "kern{} ({} margin)\n",
                self.scaled_to_string(*width),
                if *side == 0 { "left" } else { "right" }
            )),
            Node::Penalty(p) => out.push_str(&format!("penalty {}\n", p)),
            Node::Rule {
                width,
                height,
                depth,
            } => out.push_str(&format!(
                "rule({}+{}x{})\n",
                self.scaled_to_string(*width),
                self.scaled_to_string(*height),
                self.scaled_to_string(*depth)
            )),
            Node::Box { .. } => self.box_repr(n, depth, out),
            Node::Whatsit(_) => out.push_str("whatsit\n"),
            _ => out.push_str("node\n"),
        }
    }

    pub fn get_macro_str(&self, name: &[u8]) -> String {
        if let Some(id) = self.cs.lookup(name) {
            if let Some(crate::eqtb::Equiv::Macro(m)) = self.eqtb.resolve(id) {
                return self.tokens_to_string(&m.body);
            }
        }
        String::new()
    }
}

pub enum QuantityLoc {
    Int(crate::prim::IntParam),
    Dim(crate::prim::DimParam),
    Glue(crate::prim::GlueParam),
    Count(u16),
    Dimen(u16),
    Skip(u16),
    MuSkip(u16),
    None,
}

pub enum Value {
    Int(i32),
    Dim(i32),
    Glue(crate::boxes::Glue),
}

impl Value {
    fn as_int(&self) -> i32 {
        match self {
            Value::Int(v) => *v,
            _ => 0,
        }
    }
    fn as_dim(&self) -> i32 {
        match self {
            Value::Dim(v) => *v,
            _ => 0,
        }
    }
    fn as_glue(&self) -> crate::boxes::Glue {
        match self {
            Value::Glue(g) => g.clone(),
            _ => crate::boxes::Glue::zero(),
        }
    }
}

fn glue_plus(a: &crate::boxes::Glue, b: &crate::boxes::Glue) -> crate::boxes::Glue {
    let mut g = a.clone();
    g.width += b.width;
    if g.stretch_order == b.stretch_order {
        g.stretch += b.stretch;
    } else if g.stretch_order < b.stretch_order {
        g.stretch = b.stretch;
        g.stretch_order = b.stretch_order;
    }
    if g.shrink_order == b.shrink_order {
        g.shrink += b.shrink;
    } else if g.shrink_order < b.shrink_order {
        g.shrink = b.shrink;
        g.shrink_order = b.shrink_order;
    }
    g
}

#[cfg(test)]
mod tests {
    use super::read_line_bounded;

    #[test]
    fn bounded_line_reader_drains_the_overflowing_line() {
        let mut reader = std::io::Cursor::new(b"abcdef\nz\n");
        let mut line = Vec::new();
        assert_eq!(
            read_line_bounded(&mut reader, &mut line, 3).unwrap(),
            (true, true)
        );
        assert_eq!(line, b"abc");

        line.clear();
        assert_eq!(
            read_line_bounded(&mut reader, &mut line, 3).unwrap(),
            (true, false)
        );
        assert_eq!(line, b"z\n");
    }
}
