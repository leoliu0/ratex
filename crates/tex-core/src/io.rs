//! I/O primitives: \input, \openout/\closeout/\write, \openin, \read,
//! \message, \special, \show, \lowercase/\uppercase, \advance arithmetic.

use crate::boxes::Node;
use crate::engine::Engine;
use crate::eqtb::Equiv;
use crate::prim::{IntParam, Prim};
use crate::token::{Token, CAT_LETTER};
use tex_kpse::fs::PathExt;

const MAX_TEX_INPUT_STREAM: i32 = 15;
/// tex.web §484 fatal_error text (TeX prints it as the help of `Emergency stop`).
const TERMINAL_READ_IN_NONSTOP_MODE: &str =
    "Emergency stop: cannot \\read from terminal in nonstop modes";

/// pdfTeX's `\pdfescapestring`, `\pdfescapename` and `\pdfescapehex`
/// (utils.c escapestring/escapename/escapehex).
pub(crate) fn pdf_escape(primitive: Prim, bytes: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        match primitive {
            Prim::PdfEscapeString => {
                if !(b'!'..=b'~').contains(&b) {
                    out.extend_from_slice(format!("\\{b:03o}").as_bytes());
                } else {
                    if matches!(b, b'(' | b')' | b'\\') {
                        out.push(b'\\');
                    }
                    out.push(b);
                }
            }
            Prim::PdfEscapeName => {
                if b == 0 {
                    continue;
                }
                if !(b'!'..=b'~').contains(&b) || b"#%()/<>[]{}".contains(&b) {
                    out.extend_from_slice(&[b'#', HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]);
                } else {
                    out.push(b);
                }
            }
            _ => out.extend_from_slice(&[HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]),
        }
    }
    out
}

/// kpathsea's `kpathsea_name_ok` for writing (TeX Live 2026, non-extended,
/// Unix rules): `openout_any` `a` allows everything; `r` refuses dotfiles
/// (`.rhosts`, `dir/.ssh`, `..x`) and `p` (the default) also refuses
/// absolute names outside the output directories and every `../` step.
/// `rel` is the document-requested part of the name; `absolute` tells
/// whether it escapes every permitted output root.
fn out_name_ok(rel: &str, absolute: bool, choice: &str) -> bool {
    let first = choice.as_bytes().first().copied().unwrap_or(b'p');
    if matches!(first, b'a' | b'y' | b'1') {
        return true;
    }
    let b = rel.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c != b'.' || (i > 0 && b[i - 1] != b'/') {
            continue;
        }
        let next = b.get(i + 1).copied();
        let dot_dot_dir = next == Some(b'.') && b.get(i + 2) == Some(&b'/');
        if next != Some(b'/') && !dot_dot_dir {
            return false;
        }
    }
    if matches!(first, b'r' | b'n' | b'0') {
        return true;
    }
    if absolute || rel.starts_with("../") {
        return false;
    }
    !rel.contains("/../")
}

#[derive(Clone)]
struct ScannerDiagnosticState {
    macro_trace: Vec<crate::token::CsId>,
    macro_trace_truncated: bool,
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

/// A file located by `Engine::find_input_file`.
pub(crate) enum FoundInputFile {
    Path(std::path::PathBuf),
    /// Contents of a compatibility input or embedded-archive member; these
    /// have no file-system timestamp.
    Bytes(Vec<u8>),
}

/// Engine-owned package adapters and small bootstrap inputs.
/// They are immutable virtual files: keeping them in `InputStack`'s byte
/// cache avoids fixed names and repeated writes in the process temp folder.
fn compatibility_input(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        // pdftexconfig.tex (TeX Live keeps \pdfcompresslevel=9; this engine
        // trades a little size for speed)
        "pdflatex.ini" => br"\pdfoutput=1
\pdfpageheight=297 true mm
\pdfpagewidth=210 true mm
\pdfminorversion=7
\pdfobjcompresslevel=2
\pdfcompresslevel=3
\pdfdecimaldigits=3
\pdfpkresolution=600
\pdfhorigin=1 true in
\pdfvorigin=1 true in
\input latex.ltx
\endinput
",
        "xelatex.ini" => br"\begingroup
  \catcode`\{=1
  \catcode`\}=2
  \catcode`\#=6
  \csname protected\endcsname\gdef\pdfmapfile#1{\special{pdf:mapfile #1}}
  \csname protected\endcsname\gdef\pdfmapline#1{\special{pdf:mapline #1}}
\endgroup
\input latex.ltx
\endinput
",
        "lualatex.ini" => br"\input luatexconfig.tex
\begingroup
  \catcode`\{=1
  \catcode`\}=2
  \global\everyjob{\directlua{require('lualatexquotejobname.lua')}}
\endgroup
\input latex.ltx
\endinput
",
        "luatexconfig.tex" => br"\begingroup
  \catcode`\{=1
  \catcode`\}=2
  \catcode`\#=6
  \globaldefs=1
  \pdfoutput=1
  \pdfpageheight=297 true mm
  \pdfpagewidth=210 true mm
  \pdfminorversion=7
  \pdfobjcompresslevel=2
  \pdfhorigin=1 true in
  \pdfvorigin=1 true in
  \pdfcompresslevel=9
  \globaldefs=0
\endgroup
\endinput
",
        "lualatexquotejobname.lua" => br#"local jobname_cache = {}
if callback and callback.register then
    callback.register('process_jobname', function(jobname)
        local cached = jobname_cache[jobname]
        if cached ~= nil then return cached end
        local clean, n_quotes = jobname:gsub([["]], [[]])
        if n_quotes % 2 ~= 0 then
            texio.write_nl('! Unbalanced quotes in jobname: ' .. jobname)
        end
        if jobname:find(' ') then
            clean = '"' .. clean .. '"'
        end
        jobname_cache[jobname] = clean
        return clean
    end)
end
"#,
        "graphics.cfg" => br"\ProvidesFile{graphics.cfg}[2026/01/01 v1.0 Ratex graphics configuration]
\ExecuteOptions{pdftex}
\AtEndOfPackage{
  \@ifundefined{Gin@extensions}{}{
    \edef\Gin@extensions{\Gin@extensions,.svg,.SVG}
    \@namedef{Gin@rule@.svg}#1{{png}{.svg}{#1}}
    \@namedef{Gin@rule@.SVG}#1{{png}{.SVG}{#1}}
  }
}
\endinput
",
        "fontspec.sty" => include_bytes!("../assets/ratex-fontspec.sty"),
        "xeCJK.sty" => include_bytes!("../assets/ratex-xeCJK.sty"),
        "tuenc.def" => include_bytes!("../assets/ratex-tuenc.def"),
        "UTF8.chr" => include_bytes!("../assets/ratex-UTF8.chr"),
        _ => return None,
    })
}

impl Engine {
    pub fn record_loaded_bytes(&mut self, path: &std::path::Path, bytes: &[u8]) {
        let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
        let mut h2: u64 = 0x9e37_79b9_7f4a_7c15;
        for (index, byte) in bytes.iter().enumerate() {
            h1 ^= u64::from(*byte);
            h1 = h1.wrapping_mul(0x1000_0000_01b3);
            h2 = (h2 + u64::from(*byte) + index as u64).wrapping_mul(0x1000_0000_01b3);
        }
        self.loaded_file_digests
            .push((path.to_path_buf(), bytes.len() as u64, h1 ^ h2));
    }

    fn scanner_diagnostic_state(&self) -> ScannerDiagnosticState {
        ScannerDiagnosticState {
            macro_trace: self.diagnostic_macro_trace.clone(),
            macro_trace_truncated: self.diagnostic_macro_trace_truncated,
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
        self.diagnostic_macro_trace_truncated = state.macro_trace_truncated;
        self.diagnostic_trace_hold = state.trace_hold;
        self.diagnostic_source_cs = state.source_cs;
        self.diagnostic_physical_source = state.physical_source;
        self.diagnostic_macro_call_site = state.macro_call_site;
        self.diagnostic_macro_call_span = state.macro_call_span;
        self.diagnostic_synthetic_source = state.synthetic_source;
        self.diagnostic_sources_live = self.diagnostic_physical_source.is_some()
            || self.diagnostic_synthetic_source.is_some();
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
        // names keep the bytes TeX read (see `tex_bytes`); lookups use the
        // UTF-8 view the search path code works with
        let raw_name = name;
        let name = &*crate::tex_bytes::text_to_display(raw_name);
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
        let path = if raw_name == name {
            self.resolve_input_path(name)
        } else {
            self.resolve_raw_input_path(raw_name)
                .or_else(|| self.resolve_input_path(name))
        };
        if let Some(bytes) = path.is_none().then(|| compatibility_input(name)).flatten() {
            let key = format!("<compat:{name}>");
            let data = self
                .input
                .cached_file(&key)
                .unwrap_or_else(|| self.input.intern_file(key.clone(), bytes.to_vec()));
            self.print_file_open(key.as_bytes());
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
        match path {
            Some(p) => {
                let key = p.to_string_lossy().into_owned();
                let data = match self.input.read_file(&p) {
                    Ok(bytes) => {
                        let is_pdftex_def = p
                            .file_name()
                            .and_then(|f| f.to_str())
                            .map_or(false, |s| s == "pdftex.def");
                        if is_pdftex_def {
                            let mut b = bytes.to_vec();
                            b.extend_from_slice(b"\n\\@namedef{Gin@rule@.svg}#1{{png}{.svg}{#1}}\n\\@namedef{Gin@rule@.SVG}#1{{png}{.SVG}{#1}}\n\\edef\\Gin@extensions{\\Gin@extensions,.svg,.SVG}\n");
                            std::rc::Rc::from(b.into_boxed_slice())
                        } else {
                            bytes
                        }
                    }
                    Err(e) => {
                        // tex.web §537 start_input: an input that cannot be
                        // opened goes to prompt_file_name, which is fatal
                        // without a terminal to supply another name.
                        self.fatal_error_at(
                            &format!("Cannot read {}: {}", name, e),
                            included_from
                                .as_ref()
                                .map(crate::input::SourceMark::to_context),
                        );
                        return false;
                    }
                };
                self.loaded_files.push(p.clone());
                self.record_loaded_bytes(&p, &data);
                #[cfg(unix)]
                let shown = std::os::unix::ffi::OsStrExt::as_bytes(p.as_os_str()).to_vec();
                #[cfg(not(unix))]
                let shown = p.to_string_lossy().into_owned().into_bytes();
                self.print_file_open(&shown);
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
                let data = self.from_external(data);
                self.input.push_file_from(key, data, included_from);
                true
            }
            None => {
                // LaTeX reads a job/include `.aux` before opening the stream
                // that will replace it. A first build therefore needs an
                // empty input, while an unrelated same-named aux in the
                // invocation directory must not be used.
                if self.allow_missing_main_aux && name == format!("{}.aux", self.job_name) {
                    let key = format!("<empty-aux:{name}>");
                    let data = self
                        .input
                        .cached_file(&key)
                        .unwrap_or_else(|| self.input.intern_file(key.clone(), Vec::new()));
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
                // Fall back to embedded Virtual TDS package repository
                let found_data = tex_kpse::get_embedded_tex_input(name).map(|(name, bytes)| {
                    let key = format!("<embedded:{name}>");
                    let data = self
                        .input
                        .cached_file(&key)
                        .unwrap_or_else(|| self.input.intern_file(key.clone(), bytes));
                    (key, data)
                });
                if let Some((key, data)) = found_data {
                    self.print_file_open(key.as_bytes());
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
                self.fatal_error_at(
                    &format!("File `{}` not found", name),
                    included_from
                        .as_ref()
                        .map(crate::input::SourceMark::to_context),
                );
                false
            }
        }
    }

    /// A file whose name holds bytes that are not valid UTF-8 (TeX reads
    /// 8-bit names): the exact name beside the job's files.
    fn resolve_raw_input_path(&mut self, raw_name: &str) -> Option<std::path::PathBuf> {
        let dirs = [
            self.aux_dir.clone(),
            (!self.out_dir.is_empty()).then(|| std::path::PathBuf::from(&self.out_dir)),
            Some(self.main_dir.clone().unwrap_or_default()),
        ];
        let with_ext = format!("{raw_name}.tex");
        for dir in dirs.into_iter().flatten() {
            for candidate in [raw_name, with_ext.as_str()] {
                let path = dir.join(crate::tex_bytes::text_to_path(candidate));
                if path.tex_is_file() {
                    let absolute = tex_kpse::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                    self.loaded_files.push(absolute);
                    return Some(path);
                }
            }
        }
        None
    }

    /// Resolve `name` for \input/\openin the way web2c's open_input does:
    /// when -output-directory is set, relative names are looked up there
    /// first (kpathsea's TEXMF_OUTPUT_DIRECTORY behavior), then kpathsea's
    /// format search path (TDS, env paths, cwd, explicit paths).
    pub fn resolve_input_path(&mut self, name: &str) -> Option<std::path::PathBuf> {
        self.resolve_input_path_in(name, true)
    }

    /// `lax` additionally accepts a file of that basename anywhere in the
    /// TeX tree (fonts, maps, ...), which `\input` tolerates but
    /// kpse_find_tex's TEXINPUTS search never does.
    fn resolve_input_path_in(&mut self, name: &str, lax: bool) -> Option<std::path::PathBuf> {
        if name.is_empty() {
            return None;
        }
        let requested = std::path::Path::new(name);
        let absolute = |path: std::path::PathBuf| {
            tex_kpse::fs::canonicalize(&path).unwrap_or_else(|_| {
                if path.is_absolute() {
                    path
                } else {
                    tex_kpse::fs::current_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                        .join(path)
                }
            })
        };
        if requested.is_absolute() {
            if requested.tex_is_file() {
                self.loaded_files.push(absolute(requested.to_path_buf()));
                return Some(requested.to_path_buf());
            }
            self.missing_files.push(requested.to_path_buf());
            return None;
        }
        if !requested.is_absolute() {
            if let Some(dir) = self.aux_dir.clone() {
                for cand in [dir.join(name), dir.join(format!("{name}.tex"))] {
                    if cand.tex_is_file() {
                        self.loaded_files.push(absolute(cand.clone()));
                        return Some(cand);
                    }
                    self.missing_files.push(absolute(cand));
                }
            }
            if !self.out_dir.is_empty() {
                for cand in [
                    std::path::Path::new(&self.out_dir).join(name),
                    std::path::Path::new(&self.out_dir).join(format!("{name}.tex")),
                ] {
                    if cand.tex_is_file() {
                        self.loaded_files.push(absolute(cand.clone()));
                        return Some(cand);
                    }
                    self.missing_files.push(absolute(cand));
                }
            }
        }
        if self.aux_dir.is_some()
            && self.allow_missing_main_aux
            && name == format!("{}.aux", self.job_name)
        {
            return None;
        }
        if !requested.is_absolute() {
            if let Some(dir) = self.main_dir.clone() {
                for cand in [dir.join(name), dir.join(format!("{name}.tex"))] {
                    if cand.tex_is_file() {
                        self.loaded_files.push(absolute(cand.clone()));
                        return Some(cand);
                    }
                    self.missing_files.push(absolute(cand));
                }
            } else {
                for cand in [
                    std::path::Path::new(name).to_path_buf(),
                    std::path::Path::new(&format!("{name}.tex")).to_path_buf(),
                ] {
                    if cand.tex_is_file() {
                        self.loaded_files.push(absolute(cand.clone()));
                        return Some(cand);
                    }
                    self.missing_files.push(absolute(cand));
                }
            }
        }
        // A missing main LaTeX aux is valid first-pass state. Avoid allowing
        // kpathsea to substitute an unrelated same-named aux from the
        // invocation directory after the managed aux/output and source-local
        // probes above. Other explicit inputs retain ordinary TeX semantics.
        if self.allow_missing_main_aux && name == format!("{}.aux", self.job_name) {
            return None;
        }
        // Project/managed files above retain precedence. Engine adapters win
        // over installed legacy shims and engine-specific upstream packages.
        if compatibility_input(name).is_some() {
            return None;
        }
        let kpse = &self.font_loader.kpse;
        let resolved = if lax {
            kpse.find(name, tex_kpse::Format::Tex).or_else(|| {
                let basename = std::path::Path::new(name).file_name()?.to_str()?;
                kpse.find_any(basename)
            })
        } else {
            kpse.find_in_format_tree(name, tex_kpse::Format::Tex)
        };
        // A lower-priority system or embedded TeX input remains valid only
        // while every earlier search-path candidate stays absent. Stop the
        // dependency trace at the selected path so irrelevant lower roots do
        // not disable caching.
        self.font_loader
            .record_lookup_dependency(name, tex_kpse::Format::Tex, resolved.as_deref());
        if let Some(path) = &resolved {
            self.loaded_files.push(absolute(path.clone()));
        }
        if crate::debug_flag("lookups") {
            let explanation = self
                .font_loader
                .kpse
                .explain_lookup(name, tex_kpse::Format::Tex);
            let msg = format!(
                "[kpse:lookup] {} ({:?}) -> {:?} via {:?} (searched {} roots)\n",
                explanation.name,
                explanation.format,
                explanation.resolved,
                explanation.source_kind,
                explanation.searched_roots
            );
            self.append_log(&msg);
        }
        resolved
    }

    /// web2c `find_input_file` for \pdffilesize, \pdffilemoddate,
    /// \pdfmdfivesum file and \pdffiledump: the name loses every `"` (and
    /// nothing else, so surrounding spaces stay significant), then is found
    /// as kpse_find_tex finds it: the output directory and TEXINPUTS path,
    /// the built-in compatibility inputs, and the embedded TeX tree. Unlike
    /// `\input`, a TFM, encoding or map file elsewhere in the TeX tree is
    /// not found.
    pub(crate) fn find_input_file(&mut self, name: &str) -> Option<FoundInputFile> {
        let name = name.replace('"', "");
        if name.is_empty() {
            return None;
        }
        if let Some(path) = self.resolve_input_path_in(&name, false) {
            return Some(FoundInputFile::Path(path));
        }
        if let Some(data) = compatibility_input(&name) {
            return Some(FoundInputFile::Bytes(data.to_vec()));
        }
        tex_kpse::get_embedded_tex_tree_input(&name).map(|(_, data)| FoundInputFile::Bytes(data))
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
        if !self.scan_left_brace() {
            return;
        }
        let origin = self.current_token_source_mark();
        let language = self.eqtb.int_params[crate::prim::IntParam::Language.idx() as usize];
        let language = u8::try_from(language).unwrap_or(0);
        let mut word = Vec::new();
        loop {
            // TeX expands macros while scanning patterns and exceptions.
            // Authentic pattern files use active accents and macro wrappers.
            let token = self.get_x_raw();
            if self.stopped_on_error {
                return;
            }
            if token == crate::input::EOF_MARKER {
                self.fatal_error_at(
                    "File ended while scanning hyphenation patterns or exceptions",
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                return;
            }
            let closing = token.is_char() && token.cc() == 2;
            if token.is_space() || closing {
                if !word.is_empty() {
                    if is_patterns {
                        self.trie_for_language_mut(language)
                            .add_pattern_bytes(&word);
                    } else {
                        self.trie_for_language_mut(language)
                            .add_exception_bytes(&word);
                    }
                    word.clear();
                }
                if closing {
                    if is_patterns
                        && self.eqtb.int_params
                            [crate::prim::IntParam::SavingHyphCodes.idx() as usize]
                            > 0
                    {
                        let mut codes = Box::new([0; 256]);
                        codes.copy_from_slice(&self.eqtb.lc_code[..256]);
                        self.hyphen_codes.insert(language, codes);
                    }
                    return;
                }
                continue;
            }
            if !token.is_char() {
                self.error(if is_patterns {
                    "Letter expected in \\patterns"
                } else {
                    "Letter expected in \\hyphenation"
                });
                continue;
            }
            let c = token.chr() as u8;
            if is_patterns && (c.is_ascii_digit() || c == b'.') {
                word.push(c);
            } else if c == b'-' && !is_patterns {
                word.push(b'-');
            } else {
                let lc = if is_patterns {
                    self.eqtb.lc_code[c as usize]
                } else {
                    self.hyphen_codes
                        .get(&language)
                        .map_or(self.eqtb.lc_code[c as usize], |codes| codes[c as usize])
                };
                if lc != 0 {
                    word.push(lc);
                }
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
        let source = self
            .current_token_source_mark()
            .as_ref()
            .map(crate::input::SourceMark::to_context);
        let n = self.scan_int();
        self.scan_optional_equals();
        let name = self.scan_file_name();
        if self.stopped_on_error {
            return;
        }
        if name.is_empty() {
            self.error_at("\\openout needs an output file name", source);
            return;
        }
        // tex.web §1374: `if cur_ext="" then cur_ext:=".tex"`. The
        // extension starts at the last dot of the final path component.
        let mut name = name;
        if !name.rsplit('/').next().unwrap_or("").contains('.') {
            name.push_str(".tex");
        }
        // web2c open_output: the -output-directory prefix applies to
        // relative names only; absolute names bypass it. Path::join so a
        // missing trailing slash cannot fuse into "dirfile.ext".
        let requested = std::path::Path::new(&name);
        let create_parent = self.aux_dir.is_some()
            && !requested.is_absolute()
            && !requested
                .components()
                .any(|component| component == std::path::Component::ParentDir);
        let full_path = if requested.is_absolute() {
            requested.to_path_buf()
        } else if let Some(dir) = &self.aux_dir {
            dir.join(requested)
        } else if !self.out_dir.is_empty() {
            std::path::Path::new(&self.out_dir).join(requested)
        } else {
            requested.to_path_buf()
        };
        let full = full_path.to_string_lossy().into_owned();
        // §1394: stream numbers <0 map to 17, >15 to 16
        let stream = if n < 0 { 17u16 } else { n.min(16) as u16 };
        if !immediate {
            self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::OpenOut {
                stream,
                path: full,
                create_parent,
                source,
            }));
            return;
        }
        self.exec_openout(stream, &full, create_parent, source.as_ref());
    }

    /// Apply TeX Live's `openout_any` policy to a resolved `\openout` path.
    /// The output directories (`-output-directory`/aux directory, plus
    /// kpathsea's `TEXMF_OUTPUT_DIRECTORY` and `TEXMFOUTPUT`) are trusted
    /// prefixes; only the document-requested remainder is checked, as
    /// kpathsea checks the name before web2c prefixes the output directory.
    fn openout_allowed(&self, full: &str) -> bool {
        let choice = std::env::var("openout_any").unwrap_or_default();
        let choice = if choice.is_empty() { "p" } else { choice.as_str() };
        let env_roots = ["TEXMF_OUTPUT_DIRECTORY", "TEXMFOUTPUT"].map(|v| std::env::var(v).ok());
        let aux = self.aux_dir.as_ref().map(|dir| dir.to_string_lossy().into_owned());
        let roots = [aux.as_deref(), Some(self.out_dir.as_str())]
            .into_iter()
            .chain(env_roots.iter().map(Option::as_deref))
            .flatten()
            .map(|root| root.trim_end_matches('/'))
            .filter(|root| !root.is_empty());
        for root in roots {
            if let Some(rest) = full.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
                return out_name_ok(rest, false, choice);
            }
        }
        out_name_ok(full, std::path::Path::new(full).is_absolute(), choice)
    }

    /// the actual file open, shared by `\immediate\openout` and the
    /// shipout-time whatsit executor (`out_what` closes a previously open
    /// stream first, §1417)
    pub fn exec_openout(
        &mut self,
        stream: u16,
        full: &str,
        create_parent: bool,
        source: Option<&crate::input::SourceContext>,
    ) {
        // §1414: streams 16/17 (the >15 and negative aliases) are never
        // actually opened
        if stream >= 16 {
            return;
        }
        if !self.openout_allowed(full) {
            let choice = std::env::var("openout_any").ok().filter(|c| !c.is_empty());
            let choice = choice.as_deref().unwrap_or("p");
            let note = format!("\nNot writing to {full} (openout_any = {choice}).\n");
            self.append_term(&note);
            self.append_log(&note);
            self.fatal_error_at(&format!("I can't write on file `{full}`"), source.cloned());
            return;
        }
        let idx = (stream as usize).min(self.write_streams.len() - 1);
        if self.write_streams[idx].take().is_some() {
            // canonical: an open on a busy stream closes the old file first
        }
        self.write_stream_paths[idx] = None;
        if create_parent {
            let parent = crate::tex_bytes::text_to_path(full);
            let parent = parent
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""));
            if !parent.as_os_str().is_empty() {
                if let Err(error) = tex_kpse::fs::create_dir_all(parent) {
                    self.fatal_error_at(
                        &format!(
                            "Cannot create output directory `{}` for \\openout{stream}: {error}",
                            parent.display()
                        ),
                        source.cloned(),
                    );
                    return;
                }
            }
        }
        let path = crate::tex_bytes::text_to_path(full);
        match tex_kpse::fs::File::create(&path) {
            Ok(f) => {
                self.input.invalidate_disk_files();
                self.write_streams[idx] = Some(f);
                self.write_stream_paths[idx] = Some(full.to_string());
                self.written_files.push(path);
                self.print_openout_note(stream, full);
            }
            // tex.web §1374: a stream that cannot be opened goes to
            // prompt_file_name, which is fatal without a terminal.
            Err(error) => self.fatal_error_at(
                &format!("Cannot open output file `{full}` for \\openout{stream}: {error}"),
                source.cloned(),
            ),
        }
    }

    pub fn do_closeout(&mut self, immediate: bool) {
        let source = self
            .current_token_source_mark()
            .as_ref()
            .map(crate::input::SourceMark::to_context);
        let n = self.scan_int();
        let stream = if n < 0 { 17u16 } else { n.min(16) as u16 };
        if !immediate {
            self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::CloseOut {
                stream,
                source,
            }));
            return;
        }
        self.exec_closeout(stream, source.as_ref());
    }

    pub fn exec_closeout(&mut self, stream: u16, source: Option<&crate::input::SourceContext>) {
        if stream >= 16 {
            return;
        }
        let idx = (stream as usize).min(self.write_streams.len() - 1);
        let path = self.write_stream_paths[idx].take();
        if let Some(mut file) = self.write_streams[idx].take() {
            use std::io::Write;
            if let Err(error) = file.flush() {
                let destination = path.as_deref().unwrap_or("<unknown>");
                self.error_at(
                    &format!(
                        "Cannot flush output stream {stream} (`{destination}`) while closing it: {error}"
                    ),
                    source.cloned(),
                );
            }
        }
    }

    pub fn do_write(&mut self, immediate: bool) {
        let write = self.cur_cs;
        let source = self
            .current_token_source_mark()
            .as_ref()
            .map(crate::input::SourceMark::to_context);
        let n = self.scan_int();
        // tex.web §1371: \write<n>{toks} collects the list RAW (scan_toks,
        // no expansion) and expands at emission like \xdef (protected macros
        // stay frozen).
        let toks = self.scan_general_text_of(write);
        // Retain the command site because deferred expansion and file I/O can
        // happen after this input file and its macro stack have disappeared.
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
        if !self.stopped_on_error {
            self.write_out(if n < 0 { -1 } else { n.min(16) }, &text, source.as_ref());
        }
    }
    /// tex.web §1395 out_what: a Write whatsit fires at ship time, expanding
    pub fn fire_write(
        &mut self,
        stream: u16,
        tokens: &[Token],
        source: Option<&crate::input::SourceContext>,
    ) {
        let text = self.expand_write_list(tokens, source);
        if self.stopped_on_error {
            return;
        }
        self.write_out(if stream == 17 { -1 } else { stream as i32 }, &text, source);
    }
    /// Expand a raw `\write` token list to its emitted string. The write gets
    /// a private input stack: a delimited macro may consume the sentinel, but
    /// it must never continue into the active output routine or document.
    fn expand_write_list(
        &mut self,
        toks: &[Token],
        source: Option<&crate::input::SourceContext>,
    ) -> Vec<u8> {
        let saved = std::mem::take(&mut self.pushed);
        let saved_input = std::mem::take(&mut self.input.stack);
        let saved_diagnostic_state = self.scanner_diagnostic_state();
        let saved_end_occurred = self.end_occurred;
        let errors_before = self.error_count;
        let saved_source = std::mem::replace(&mut self.diagnostic_source_override, source.cloned());
        // tex.web §1371: the text is scanned like general text of \write.
        let write = self.cs.lookup(b"write");
        let saved_outer_scan =
            self.outer_scan.replace((crate::expand::OuterScan::Text, write));

        // tex.web §1371-§1372: the text is expanded as `{` text `}` \endwrite,
        // so a macro argument cannot run past the text; the sentinel stands
        // for the frozen outer \endwrite. An expansion yielding an extra `}`
        // ends the text early (the rest is dropped with "Unbalanced write
        // command"); an extra `{` runs into \endwrite, which is fatal.
        let mut body = Vec::with_capacity(toks.len() + 3);
        body.push(Token::char(1, u32::from(b'{')));
        body.extend_from_slice(toks);
        body.push(Token::char(2, u32::from(b'}')));
        body.push(crate::page::WRITE_END_TOKEN);
        self.push_tokens_named(body, "<write>");
        let prev = self.in_expanded_scan;
        self.in_expanded_scan = true;
        let saved_mode_zero = std::mem::replace(&mut self.write_mode_zero, true);
        let mut out: Vec<Token> = Vec::new();
        let mut depth = 0usize;
        loop {
            let t = self.get_token();
            if t == crate::page::WRITE_END_TOKEN || t == crate::input::EOF_MARKER {
                if depth > 0 {
                    self.fatal_error_at("Unbalanced write command", source.cloned());
                }
                break;
            }
            if t.is_left_brace() {
                depth += 1;
                if depth == 1 {
                    continue;
                }
            } else if t.is_right_brace() && depth > 0 {
                depth -= 1;
                if depth == 0 {
                    // §1372: the balanced text must be followed by \endwrite;
                    // the text scan (and its \outer check) is over. TeX reads
                    // the follower and skips to \endwrite with the
                    // non-expanding get_token, so nothing there is expanded.
                    self.outer_scan = saved_outer_scan;
                    let next = self.raw_token();
                    if next != crate::page::WRITE_END_TOKEN && next != crate::input::EOF_MARKER {
                        self.error("Unbalanced write command");
                        loop {
                            let skipped = self.raw_token();
                            if skipped == crate::page::WRITE_END_TOKEN
                                || skipped == crate::input::EOF_MARKER
                            {
                                break;
                            }
                        }
                    }
                    break;
                }
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
        self.write_mode_zero = saved_mode_zero;
        self.input.stack = saved_input;
        self.restore_scanner_diagnostic_state(saved_diagnostic_state);
        self.end_occurred =
            saved_end_occurred || (self.end_occurred && self.error_count > errors_before);
        self.diagnostic_source_override = saved_source;
        self.outer_scan = saved_outer_scan;
        self.pushed = saved;
        self.token_list_bytes(&out)
    }

    pub fn write_tokens_to_string(&self, toks: &[Token]) -> String {
        String::from_utf8_lossy(&self.token_list_bytes(toks)).into_owned()
    }

    /// The text TeX prints for a token list as part of an error message:
    /// unprintable bytes appear in `^^` notation (§59).
    pub(crate) fn print_tokens_to_string(&self, toks: &[Token]) -> String {
        let bytes = self.token_list_bytes(toks);
        let mut printed = Vec::with_capacity(bytes.len());
        crate::tex_bytes::push_printable(&self.xprn, &mut printed, &bytes);
        crate::tex_bytes::bytes_to_text(&printed)
    }

    /// The bytes TeX's `show_token_list` would produce for an expanded
    /// token list (one byte per 8-bit character).
    pub(crate) fn token_list_bytes(&self, toks: &[Token]) -> Vec<u8> {
        let mut out = Vec::new();
        for t in toks {
            if t.is_cs() {
                let name = self.cs.name(t.cs_id());
                // Active-character placeholder ids detokenize to their source
                // bytes, not to the internal collision-proof name.
                if let Some((bytes, len)) = Self::active_cs_source_bytes(name) {
                    out.extend_from_slice(&bytes[..len]);
                } else {
                    let esc = self.eqtb.int_params[IntParam::EscapeChar.idx() as usize];
                    if (0..256).contains(&esc) {
                        out.push(esc as u8);
                    }
                    out.extend_from_slice(name);
                    if name.len() > 1
                        || name
                            .first()
                            .is_some_and(|&c| self.eqtb.cat[c as usize] == CAT_LETTER)
                    {
                        out.push(b' ');
                    }
                }
            } else {
                // tex.web show_token_list: a mac_param character is doubled
                if t.cc() == 6 {
                    t.append_character_bytes(&mut out);
                }
                t.append_character_bytes(&mut out);
            }
        }
        out
    }

    /// tex.web §1370 write_out for the characters `raw` of a `\write`.
    pub fn write_out(&mut self, n: i32, raw: &[u8], source: Option<&crate::input::SourceContext>) {
        match n {
            -1 => {
                self.tex_print_nl(false, true);
                self.tex_print_chars(false, true, raw);
                self.tex_print_ln(false, true);
            }
            -2 => {
                self.tex_print_nl(true, false);
                self.tex_print_chars(true, false, raw);
                self.tex_print_ln(true, false);
            }
            16 | 17 | 18 => {
                self.tex_print_nl(true, true);
                self.tex_print_chars(true, true, raw);
                self.tex_print_ln(true, true);
            }
            _ => {
                let idx = (n as usize).min(self.write_streams.len() - 1);
                if self.write_streams[idx].is_some() {
                    // print(c) for a \write file: the new-line character
                    // ends the line, unprintable bytes use `^^` notation
                    let nl = self.new_line_char();
                    let mut line = Vec::with_capacity(raw.len() + 1);
                    for &byte in raw {
                        if i32::from(byte) == nl {
                            line.push(b'\n');
                        } else {
                            crate::tex_bytes::push_printable(&self.xprn, &mut line, &[byte]);
                        }
                    }
                    line.push(b'\n');
                    self.to_external(&mut line);
                    use std::io::Write;
                    let result = self.write_streams[idx]
                        .as_mut()
                        .map_or(Ok(()), |file| file.write_all(&line));
                    if let Err(error) = result {
                        let destination = self.write_stream_paths[idx]
                            .as_deref()
                            .unwrap_or("<unknown>");
                        self.error_at(
                            &format!("Cannot write output stream {n} (`{destination}`): {error}"),
                            source.cloned(),
                        );
                    }
                } else {
                    // tex.web §1382: a \write to a closed stream is
                    // directed to the log and the terminal. \typeout
                    // rides \write\@unused (stream 0, never opened), so
                    // this arm is what makes it visible.
                    self.tex_print_nl(true, true);
                    self.tex_print_chars(true, true, raw);
                    self.tex_print_ln(true, true);
                }
            }
        }
    }

    pub fn do_special(&mut self) {
        let toks = self.scan_general_text_expanded();
        let s = self.tokens_to_text(&toks);
        self.cur_list.push(crate::boxes::Node::Whatsit(crate::boxes::WhatIt::Special(s)));
    }

    pub fn do_message(&mut self, err: bool) {
        let origin = self.current_token_source_mark();
        let toks = self.scan_general_text_expanded();
        if self.stopped_on_error {
            return;
        }
        let text = self.print_tokens_to_string(&toks);
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
            self.error_at(
                &text,
                origin.as_ref().map(crate::input::SourceMark::to_context),
            );
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
            let raw = self.token_list_bytes(&toks);
            self.tex_message(&raw);
        }
    }

    pub fn do_openin(&mut self) {
        let n = self.scan_int();
        self.scan_optional_equals();
        let name = crate::tex_bytes::text_to_display(&self.scan_file_name()).into_owned();
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
        if let Some(data) = path.is_none().then(|| compatibility_input(&name)).flatten() {
            let is_empty = data.is_empty();
            self.read_files[n as usize] = Some(Box::new(std::io::Cursor::new(data)));
            self.read_eof[n as usize] = is_empty;
            return;
        }
        if let Some(p) = path {
            if let Ok(mut bytes) = tex_kpse::fs::read(&p) {
                if name == "pdftex.def" {
                    let rule_bytes = b"\n\\@namedef{Gin@rule@.svg}#1{{png}{.svg}{#1}}\n\\@namedef{Gin@rule@.SVG}#1{{png}{.SVG}{#1}}\n";
                    bytes.extend_from_slice(rule_bytes);
                }
                self.record_loaded_bytes(&p, &bytes);
                let is_empty = bytes.is_empty();
                self.read_files[n as usize] = Some(Box::new(std::io::Cursor::new(bytes)));
                self.read_eof[n as usize] = is_empty;
                return;
            }
        }
        // Fall back to embedded package archive
        if let Some((_name, data)) = tex_kpse::get_embedded_tex_input(&name) {
            let is_empty = data.is_empty();
            self.read_files[n as usize] = Some(Box::new(std::io::Cursor::new(data)));
            self.read_eof[n as usize] = is_empty;
            return;
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

    /// tex.web §484: in batch and nonstop modes a terminal \read is fatal.
    fn terminal_read_forbidden(&self) -> bool {
        matches!(
            self.interaction_mode,
            crate::engine::InteractionMode::Batch | crate::engine::InteractionMode::Nonstop
        )
    }

    pub fn do_read(&mut self, line_mode: bool) {
        let origin = self.current_token_source_mark();
        let global = self.take_assignment_prefixes("\\read");
        let stream = self.scan_int();
        if !self.scan_keyword(b"to") {
            self.error("Missing `to' inserted for \\read");
        }
        let cs = self.scan_definable_cs();
        if self.stopped_on_error {
            self.clear_prefixes();
            return;
        }
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
                let message = if self.terminal_read_forbidden() {
                    TERMINAL_READ_IN_NONSTOP_MODE.to_string()
                } else {
                    format!("Terminal input is unavailable for \\read{stream}")
                };
                self.fatal_error_at(
                    &message,
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
                // tex.web §484: a closed stream reads from the terminal
                let message = if self.terminal_read_forbidden() {
                    TERMINAL_READ_IN_NONSTOP_MODE.to_string()
                } else {
                    format!("Input stream {stream} is not open for \\read")
                };
                self.fatal_error_at(
                    &message,
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                // A failed read must not replace the requested control
                // sequence with an empty macro, especially in error-stop
                // mode where execution has already halted.
                self.clear_prefixes();
                return;
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
                    self.error_at(
                        &format!("Cannot read input stream {stream}: {error}"),
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    true
                }
            };
            if eof {
                self.read_files[stream as usize] = None;
                self.read_eof[stream as usize] = true;
                if balance != 0 {
                    self.error_at(
                        "File ended within \\read",
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
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
                let saved_source_override = std::mem::replace(
                    &mut self.diagnostic_source_override,
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
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
                        self.diagnostic_source_override = saved_source_override;
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
                self.diagnostic_source_override = saved_source_override;
                if self.stopped_on_error {
                    self.clear_prefixes();
                    return;
                }
            }
            if line_mode || balance == 0 || eof {
                break;
            }
        }
        if self.stopped_on_error {
            self.clear_prefixes();
            return;
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

    /// `\jobname` as web2c prints it: quoted when it contains a space, so
    /// `\jobname.aux` scans back as one file name.
    pub(crate) fn quoted_job_name(&self) -> String {
        if self.job_name.contains(' ') {
            format!("\"{}\"", self.job_name)
        } else {
            self.job_name.clone()
        }
    }

    /// web2c's scan_file_name ends a file name at a space read from a file
    /// line that is exhausted (`state<>token_list` and `loc>limit`): the
    /// end-of-line space stops even a quoted name, so `\input "foo` at the
    /// end of a line looks for `foo`.
    pub(crate) fn file_name_line_ended(&self, token: Token) -> bool {
        token.is_char()
            && token.chr() == u32::from(b' ')
            && matches!(
                self.input.stack.last(),
                Some(crate::input::Source::File { line_buf, line_pos, .. })
                    if line_buf.as_ref().is_none_or(|line| *line_pos >= line.len())
            )
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
                    let additional = if t2.is_unicode_char() {
                        char::from_u32(t2.chr()).map_or(0, char::len_utf8)
                    } else {
                        1
                    };
                    if additional > MAX_FILE_NAME_BYTES.saturating_sub(name.len()) {
                        self.fatal_error_at(
                            "TeX capacity exceeded, sorry [file name exceeds 4096 bytes]",
                            origin.as_ref().map(crate::input::SourceMark::to_context),
                        );
                        return String::new();
                    }
                    t2.append_character_bytes(&mut name);
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
            return crate::tex_bytes::bytes_to_text(&name).trim().to_string();
        }
        // standard TeX \input filename.tex (unquoted). Real TeX expands
        // macros while scanning a filename (TeXbook ch.8: \openin0=pre\foo.tex
        // finds preprobe.tex); an UNEXPANDABLE cs terminates the scan and is
        // re-read. Without expansion, \input pgflibrary\pgf@temp.code.tex
        // opens "pgflibrary" and leaks "\pgf@temp.code.tex" into the text.
        // web2c more_name: a `"` toggles quoting and is not part of the
        // name; a space ends the name only outside quotes, so
        // `\input "main file".aux` reads `main file.aux`.
        let mut quoted = false;
        let mut cur = t;
        loop {
            if cur == crate::input::EOF_MARKER {
                break;
            }
            if self.file_name_line_ended(cur) {
                break;
            }
            if cur.is_char() && cur.chr() == u32::from(b'"') {
                quoted = !quoted;
                cur = self.get_x_raw();
                continue;
            }
            if !quoted && (cur.is_space() || (cur.is_char() && cur.cc() == 10)) {
                break;
            }
            if cur.is_cs() {
                self.push_token(cur);
                break;
            }
            if cur.is_char() {
                let c = cur.chr();
                if !quoted && matches!(c, 32 | 9 | 13 | 10) {
                    break;
                }
                let additional = if cur.is_unicode_char() {
                    char::from_u32(c).map_or(0, char::len_utf8)
                } else {
                    1
                };
                if additional > MAX_FILE_NAME_BYTES.saturating_sub(name.len()) {
                    self.fatal_error_at(
                        "TeX capacity exceeded, sorry [file name exceeds 4096 bytes]",
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    return String::new();
                }
                cur.append_character_bytes(&mut name);
            }
            cur = self.get_x_raw();
        }
        crate::tex_bytes::bytes_to_text(&name).trim().to_string()
    }

    pub fn do_show(&mut self) {
        // tex.web \show grabs the target with get_name (NON-expanding)
        let source = self.current_token_source_mark();
        let t = self.raw_token();
        if t == crate::input::EOF_MARKER {
            self.fatal_error_at(
                "File ended after \\show; add the token or control sequence to inspect",
                source.as_ref().map(crate::input::SourceMark::to_context),
            );
            return;
        }
        let target = if t.is_cs() {
            self.display_cs(t.cs_id())
        } else {
            self.diagnostic_tokens_to_string(&[t], 64)
        };
        let meaning = self.bounded_show_meaning(t);
        self.report_inspection("\\show", format!("{target} = {meaning}"), source);
    }

    fn bounded_show_meaning(&self, token: Token) -> String {
        const PART_LIMIT: usize = 256;
        const BODY_LIMIT: usize = 5 * 1024;
        let Some(Equiv::Macro(definition)) = token
            .is_cs()
            .then(|| self.eqtb.resolve(token.cs_id()))
            .flatten()
        else {
            let meaning = self.meaning_of(token);
            let raw = crate::tex_bytes::text_to_bytes(&meaning);
            let mut shown = Vec::with_capacity(raw.len());
            crate::tex_bytes::push_printable(&self.xprn, &mut shown, &raw);
            return crate::tex_bytes::bytes_to_text(&shown);
        };

        let mut text = String::with_capacity(256);
        if definition.protected {
            text.push_str("\\protected ");
        }
        if definition.long {
            text.push_str("\\long ");
        }
        if definition.outer {
            text.push_str("\\outer ");
        }
        text.push_str("macro:");
        text.push_str(&self.diagnostic_tokens_to_string(&definition.prefix, PART_LIMIT));
        for (index, delimiter) in definition.params.iter().enumerate() {
            text.push('#');
            text.push_str(&(index + 1).to_string());
            text.push_str(&self.diagnostic_tokens_to_string(delimiter, PART_LIMIT));
        }
        text.push_str(" -> ");
        text.push_str(&self.diagnostic_tokens_to_string(&definition.body, BODY_LIMIT));
        text
    }

    pub fn shift_case(&mut self, toks: &mut Vec<Token>, up: bool) {
        for t in toks.iter_mut() {
            // tex.web §1289: only character tokens; CS names are unchanged.
            if t.is_cs() {
                continue;
            }
            let mapped = self.eqtb.case_code(t.chr(), up);
            // lccode/uccode 0 = leave unchanged
            if mapped != 0 {
                *t = if t.is_unicode_char() {
                    Token::unicode_char(t.cc(), mapped)
                } else {
                    Token::char(t.cc(), mapped)
                };
            }
        }
    }

    pub fn do_advance(&mut self) {
        // \advance<quantity> by <int/dimen/glue>
        let origin = self.current_token_source_mark();
        let global = self.take_assignment_prefixes("\\advance");
        let loc = self.scan_quantity("\\advance");
        if matches!(loc, QuantityLoc::None) {
            return;
        }
        self.scan_keyword(b"by");
        let v = match loc {
            QuantityLoc::Int(_) | QuantityLoc::Count(_) => Value::Int(self.scan_int()),
            QuantityLoc::Dim(_) | QuantityLoc::Dimen(_) => Value::Dim(self.scan_dimen(false, true)),
            QuantityLoc::Glue(p) if p.is_mu() => Value::Glue(self.scan_glue(true)),
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
                let nv = self.recover_linebreak_int_parameter(
                    p,
                    nv,
                    origin.as_ref().map(crate::input::SourceMark::to_context),
                );
                self.eqtb.assign_int_param(p, nv, global);
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
                self.eqtb.assign_dim_param(p, nv, global);
            }
            QuantityLoc::Dimen(i) => {
                let cur = self.eqtb.dimen[i as usize];
                if let Some(value) = self.checked_advance(cur, v.as_dim(), true, origin.as_ref()) {
                    self.eqtb.assign_dimen(i, value, global);
                }
            }
            QuantityLoc::Glue(p) => {
                let cur = self.eqtb.glue_params[p.idx() as usize].clone();
                if let Some(value) = self.checked_glue_advance(&cur, &v.as_glue(), origin.as_ref())
                {
                    self.eqtb.assign_glue_param(p, value, global);
                }
            }
            QuantityLoc::Skip(i) => {
                let cur = self.eqtb.skip[i as usize].clone();
                if let Some(value) = self.checked_glue_advance(&cur, &v.as_glue(), origin.as_ref())
                {
                    self.eqtb.assign_skip(i, value, global);
                }
            }
            QuantityLoc::MuSkip(i) => {
                let cur = self.eqtb.muskip[i as usize].clone();
                if let Some(value) = self.checked_glue_advance(&cur, &v.as_glue(), origin.as_ref())
                {
                    self.eqtb.assign_muskip(i, value, global);
                }
            }
            QuantityLoc::None => {}
        }
    }

    pub fn do_arith(&mut self, op: u8) {
        // \multiply / \divide
        let origin = self.current_token_source_mark();
        let operation = if op == 1 { "\\multiply" } else { "\\divide" };
        let global = self.take_assignment_prefixes(operation);
        let loc = self.scan_quantity(operation);
        if matches!(loc, QuantityLoc::None) {
            return;
        }
        self.scan_keyword(b"by");
        let n = self.scan_int();
        match loc {
            QuantityLoc::Int(p) => {
                let cur = self.int_param_value(p);
                if let Some(nv) = self.checked_arith(cur, n, op, false, origin.as_ref()) {
                    let nv = self.recover_linebreak_int_parameter(
                        p,
                        nv,
                        origin.as_ref().map(crate::input::SourceMark::to_context),
                    );
                    self.eqtb.assign_int_param(p, nv, global);
                }
            }
            QuantityLoc::Count(i) => {
                let cur = self.eqtb.count[i as usize];
                if let Some(value) = self.checked_arith(cur, n, op, false, origin.as_ref()) {
                    self.eqtb.assign_count(i, value, global);
                }
            }
            QuantityLoc::Dim(p) => {
                let cur = self.dim_param_value(p);
                if let Some(nv) = self.checked_arith(cur, n, op, true, origin.as_ref()) {
                    self.eqtb.assign_dim_param(p, nv, global);
                }
            }
            QuantityLoc::Dimen(i) => {
                let cur = self.eqtb.dimen[i as usize];
                if let Some(value) = self.checked_arith(cur, n, op, true, origin.as_ref()) {
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
            QuantityLoc::MuSkip(i) => {
                let mut cur = self.eqtb.muskip[i as usize];
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
                self.eqtb.assign_muskip(i, cur, global);
            }
            _ => {}
        }
    }

    fn checked_advance(
        &mut self,
        current: i32,
        increment: i32,
        _dimension: bool,
        _origin: Option<&crate::input::SourceMark>,
    ) -> Option<i32> {
        // tex.web §23137: \advance uses wrapping 32-bit addition without overflow checks.
        Some(current.wrapping_add(increment))
    }

    fn checked_glue_advance(
        &mut self,
        current: &crate::boxes::Glue,
        increment: &crate::boxes::Glue,
        _origin: Option<&crate::input::SourceMark>,
    ) -> Option<crate::boxes::Glue> {
        // tex.web §23143-23156: \advance on glue specs uses wrapping addition without overflow checks.
        let mut value = current.clone();
        value.width = value.width.wrapping_add(increment.width);
        if increment.stretch != 0 {
            if value.stretch_order == increment.stretch_order {
                value.stretch = value.stretch.wrapping_add(increment.stretch);
            } else if value.stretch_order < increment.stretch_order {
                value.stretch = increment.stretch;
                value.stretch_order = increment.stretch_order;
            }
        }
        if increment.shrink != 0 {
            if value.shrink_order == increment.shrink_order {
                value.shrink = value.shrink.wrapping_add(increment.shrink);
            } else if value.shrink_order < increment.shrink_order {
                value.shrink = increment.shrink;
                value.shrink_order = increment.shrink_order;
            }
        }
        Some(value)
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
            self.checked_arith(width, operand, op, true, origin)?,
            self.checked_arith(stretch, operand, op, true, origin)?,
            self.checked_arith(shrink, operand, op, true, origin)?,
        ))
    }

    fn checked_arith(
        &mut self,
        a: i32,
        b: i32,
        op: u8,
        dimension: bool,
        origin: Option<&crate::input::SourceMark>,
    ) -> Option<i32> {
        // TeX reports arithmetic faults and leaves the quantity unchanged.
        // Silent saturation or division to zero can produce plausible but
        // incorrect output, which is much harder to debug.
        let result = if op == 1 {
            a.checked_mul(b)
        } else {
            a.checked_div(b)
        }
        .filter(|value| {
            (op != 1 || *value >= -i32::MAX)
                && (!dimension || i64::from(*value).abs() <= 0x3FFF_FFFF)
        });
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
        // Consume assignment prefixes before any scanner can recover early.
        // A constructed box completes asynchronously, so hand the captured
        // globality to park_setbox immediately before opening its body.
        let global = self.take_assignment_prefixes("\\setbox");
        let idx = self.scan_reg_num();
        self.scan_optional_equals();
        self.skip_spaces_relax();
        let t = self.get_x_raw();
        if !t.is_cs() {
            self.push_token(t);
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
                    self.eqtb.assign_box(idx, b, global);
                    return;
                }
                Prim::Copy => {
                    let n = self.scan_reg_num();
                    let b = self.eqtb.boxed.get(n as usize).cloned().flatten();
                    self.eqtb.assign_box(idx, b, global);
                    return;
                }
                Prim::LastBox => {
                    let b = self.take_last_box();
                    self.eqtb.assign_box(idx, b, global);
                    return;
                }
                Prim::HBox | Prim::VBox | Prim::VTop | Prim::VCenter => {
                    self.park_setbox_with_global(idx, global);
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
                    let (top, m, rest) = self.scan_vsplit();
                    if let Some(rest) = rest {
                        self.stash_vsplit_remainder(m, rest);
                    }
                    self.eqtb.assign_box(idx, top, global);
                    return;
                }
                _ => {}
            }
        }

        match self.cs.name(t.cs_id()) {
            b"box" => {
                let n = self.scan_reg_num();
                let b = self.eqtb.take_box(n);
                self.eqtb.assign_box(idx, b, global);
            }
            b"copy" => {
                let n = self.scan_reg_num();
                let b = self.eqtb.boxed[n as usize].clone();
                self.eqtb.assign_box(idx, b, global);
            }
            b"lastbox" => {
                let b = self.take_last_box();
                self.eqtb.assign_box(idx, b, global);
            }
            b"hbox" | b"vbox" | b"vtop" | b"vcenter" => {
                self.park_setbox_with_global(idx, global);
                let kind = match self.cs.name(t.cs_id()) {
                    b"hbox" => 0,
                    b"vbox" => 1,
                    b"vtop" => 2,
                    _ => 3,
                };
                self.begin_box(kind);
            }
            b"halign" => {
                self.park_setbox_with_global(idx, global);
                self.begin_halign();
            }
            b"usebox" => {
                let n = self.scan_reg_num();
                let b = self.eqtb.boxed[n as usize].take();
                self.eqtb.assign_box(idx, b, global);
            }
            b"vsplit" => {
                let (top, m, rest) = self.scan_vsplit();
                if let Some(rest) = rest {
                    self.stash_vsplit_remainder(m, rest);
                }
                self.eqtb.assign_box(idx, top, global);
            }
            _ => {
                self.push_token(t);
                self.error("Missing box for \\setbox");
            }
        }
    }

    /// tex.web §1237: the target of \advance/\multiply/\divide is the next
    /// expanded token; only assign_int/dimen/glue/mu_glue and register
    /// commands qualify. Anything else is consumed with "You can't use `x'
    /// after \advance" and nothing changes.
    fn scan_quantity(&mut self, operation: &str) -> QuantityLoc {
        use crate::prim::{DimParam, IntParam};
        let t = self.get_x_raw();
        let loc = match self.cur_prim {
            Some(Prim::IntP(p)) => match p {
                // set_aux, set_prev_graf, set_page_int, set_interaction and
                // last_item commands that Ratex stores as integer parameters
                IntParam::SpaceFactor
                | IntParam::PrevGraf
                | IntParam::DeadCycles
                | IntParam::InsertPenalties
                | IntParam::InteractionMode
                | IntParam::ErrorStopMode
                | IntParam::ScrollMode
                | IntParam::NonStopMode
                | IntParam::BatchMode
                | IntParam::InputLineNo
                | IntParam::Badness
                | IntParam::EtxVersion
                | IntParam::PdfTexVersion
                | IntParam::PdfPageCount
                | IntParam::CurrentGroupLevel
                | IntParam::CurrentGroupType
                | IntParam::CurrentIfLevel
                | IntParam::CurrentIfType
                | IntParam::CurrentIfBranch
                | IntParam::LastNodeType
                | IntParam::PartokenNameCs => QuantityLoc::None,
                _ => QuantityLoc::Int(p),
            },
            Some(Prim::DimP(p)) => match p {
                // set_aux and set_page_dimen commands
                DimParam::PrevDepth
                | DimParam::PageGoal
                | DimParam::PageTotal
                | DimParam::PageDepth
                | DimParam::PageStretch
                | DimParam::PageFilStretch
                | DimParam::PageFillStretch
                | DimParam::PageFilllStretch
                | DimParam::PageShrink => QuantityLoc::None,
                _ => QuantityLoc::Dim(p),
            },
            Some(Prim::GlueP(p)) => QuantityLoc::Glue(p),
            Some(Prim::Count) => QuantityLoc::Count(self.scan_reg_num()),
            Some(Prim::Dimen) => QuantityLoc::Dimen(self.scan_reg_num()),
            Some(Prim::Skip) => QuantityLoc::Skip(self.scan_reg_num()),
            Some(Prim::MuSkip) => QuantityLoc::MuSkip(self.scan_reg_num()),
            Some(_) => QuantityLoc::None,
            None if t.is_cs() => {
                match self.eqtb.resolve(t.cs_id()) {
                    Some(Equiv::CountReg(i)) => QuantityLoc::Count(*i),
                    Some(Equiv::DimenReg(i)) => QuantityLoc::Dimen(*i),
                    Some(Equiv::SkipReg(i)) => QuantityLoc::Skip(*i),
                    Some(Equiv::MuSkipReg(i)) => QuantityLoc::MuSkip(*i),
                    _ => QuantityLoc::None,
                }
            }
            None => QuantityLoc::None,
        };
        if matches!(loc, QuantityLoc::None) {
            // print_cmd_chr: a \noexpand-marked token is relax/no_expand_flag
            let what = if t.is_cs() && self.no_expand_tok == Some(t) {
                let esc = self.eqtb.int_params[crate::prim::IntParam::EscapeChar.idx() as usize];
                let mut s = String::new();
                if (0..=255).contains(&esc) {
                    s.push(char::from(esc as u8));
                }
                s.push_str("relax");
                s
            } else {
                self.meaning_of(t)
            };
            self.error(&format!("You can't use `{what}' after {operation}"));
        }
        loc
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
            Node::MuGlue(g) => out.push_str(&format!("math glue {}\n", self.mu_glue_to_string(g))),
            Node::Kern(k) => out.push_str(&format!("kern {}\n", self.scaled_to_string(*k))),
            // tex.web §4416: an explicit kern is shown with a space after
            // the escape (`\kern 1.0`), an implicit one without (`\kern1.0`)
            Node::ExplicitKern(k) => out.push_str(&format!("kern {}\n", self.scaled_to_string(*k))),
            Node::AccentKern(k) => out.push_str(&format!(
                "kern {} (for accent)\n",
                self.scaled_to_string(*k)
            )),
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

#[cfg(test)]
mod tests {
    use super::read_line_bounded;
    use crate::engine::{Engine, InteractionMode};

    fn run(source: String) -> Engine {
        run_in("", source)
    }

    /// Run with `out_dir` as the `-output-directory` equivalent.
    fn run_in(out_dir: &str, source: String) -> Engine {
        let mut engine = Engine::new(true);
        engine.init_primitives();
        engine.add_nullfont();
        engine.out_dir = out_dir.to_string();
        engine.eqtb.cat[b'{' as usize] = 1;
        engine.eqtb.cat[b'}' as usize] = 2;
        engine.set_interaction_mode(InteractionMode::Nonstop);
        engine
            .input
            .push_file("output-error.tex".to_string(), source.into_bytes());
        engine.run();
        engine
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tex-core-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn openout_name_policy_matches_kpathsea_paranoid_mode() {
        for (name, absolute, ok) in [
            ("x.txt", false, true),
            ("./y.txt", false, true),
            ("sub/x..y.txt", false, true),
            ("/etc/x.txt", true, false),
            ("../x.txt", false, false),
            ("sub/../x.txt", false, false),
            ("sub/..", false, false),
            (".bashrc", false, false),
            (".tex", false, false),
            ("sub/.ssh/config", false, false),
            ("..x/z.txt", false, false),
        ] {
            assert_eq!(super::out_name_ok(name, absolute, "p"), ok, "{name}");
        }
        assert!(super::out_name_ok("../x.txt", false, "r"));
        assert!(!super::out_name_ok(".profile", false, "r"));
        assert!(super::out_name_ok("/etc/.profile", true, "a"));
    }

    #[test]
    fn openout_refuses_paths_escaping_the_output_directory() {
        let dir = scratch_dir("openout-policy");
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let out_dir = out.to_string_lossy().replace('\\', "/");
        let outside = dir.join("victim.txt").to_string_lossy().replace('\\', "/");
        for name in [outside.as_str(), "../victim.txt", ".victimrc", "sub/../../victim.txt"] {
            for deferred in [false, true] {
                let source = if deferred {
                    format!("\\setbox0=\\vbox{{\\openout4={name}\n}}\\shipout\\box0\n\\end\n")
                } else {
                    format!("\\immediate\\openout4={name}\n\\message{{after}}\\end\n")
                };
                let engine = run_in(&out_dir, source);
                assert!(engine.stopped_on_error, "{name}: fatal like TeX's\n{}", engine.term);
                assert!(
                    engine
                        .diagnostics
                        .iter()
                        .any(|d| d.message.starts_with("I can't write on file")),
                    "{name}: {}",
                    engine.term
                );
                assert!(engine.term.contains("(openout_any = p)"), "{}", engine.term);
                assert!(!engine.term.contains("after"), "{}", engine.term);
            }
        }
        assert!(!dir.join("victim.txt").exists());
        assert!(!out.join(".victimrc").exists());
        // Absolute names inside the output directory stay writable, as with
        // TeX Live's TEXMF_OUTPUT_DIRECTORY.
        let inside = format!("{out_dir}/inside.txt");
        let engine = run_in(&out_dir, format!("\\immediate\\openout4={inside}\n\\end\n"));
        assert!(!engine.stopped_on_error, "{}", engine.term);
        assert!(out.join("inside.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn openout_without_extension_writes_a_tex_file() {
        let dir = scratch_dir("openout-ext");
        let out_dir = dir.to_string_lossy().replace('\\', "/");
        let engine = run_in(
            &out_dir,
            "\\immediate\\openout4=plain\\immediate\\write4{x}\\immediate\\closeout4\n\
             \\immediate\\openout5=x..y\\immediate\\closeout5\n\\end\n"
                .to_string(),
        );
        assert!(!engine.stopped_on_error, "{}", engine.term);
        assert_eq!(std::fs::read(dir.join("plain.tex")).unwrap(), b"x\n");
        assert!(!dir.join("plain").exists());
        assert!(dir.join("x..y").exists(), "an existing extension is kept");
        std::fs::remove_dir_all(dir).unwrap();
    }

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

    #[test]
    fn immediate_and_deferred_openout_failures_keep_the_command_source() {
        let dir = scratch_dir("missing-openout-parent");
        let out_dir = dir.to_string_lossy().replace('\\', "/");
        let output = format!("{out_dir}/missing/result.out");
        let relative = "missing/result.out";
        let cases = [
            (
                format!("\\message{{before}}\n\\immediate\\openout4={relative}\n\\end\n"),
                2,
                11,
            ),
            (
                format!("\\setbox0=\\vbox{{\n\\openout4={relative}\n}}\n\\shipout\\box0\n\\end\n"),
                2,
                1,
            ),
        ];

        for (input, expected_line, expected_column) in cases {
            let engine = run_in(&out_dir, input);
            let diagnostic = engine
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.message.starts_with("Cannot open output file"))
                .expect("openout diagnostic");
            assert!(
                diagnostic.message.contains(&output),
                "{}",
                diagnostic.message
            );
            assert!(diagnostic.message.contains("for \\openout4"));
            let source = diagnostic.primary.as_ref().expect("openout source");
            assert_eq!(
                (source.name.as_str(), source.line, source.column),
                ("output-error.tex", expected_line, expected_column)
            );
            assert_eq!(
                diagnostic.help.as_deref(),
                Some("check the path and permissions for the file named in this error")
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn immediate_and_deferred_write_failures_name_the_stream_and_write_source() {
        if !std::path::Path::new("/dev/full").exists() {
            return;
        }
        // A symlink inside the output directory reaches the full device
        // without naming an absolute path, which openout_any=p refuses.
        let dir = scratch_dir("write-full");
        std::os::unix::fs::symlink("/dev/full", dir.join("full.out")).unwrap();
        let out_dir = dir.to_string_lossy().replace('\\', "/");
        let cases = [
            (
                "\\immediate\\openout3=full.out\n\\immediate\\write3{payload}\n\\end\n"
                    .to_string(),
                2,
                11,
            ),
            (
                "\\immediate\\openout3=full.out\n\\setbox0=\\vbox{\n\\write3{payload}\n}\n\\shipout\\box0\n\\end\n"
                    .to_string(),
                3,
                1,
            ),
        ];
        let destination = format!("Cannot write output stream 3 (`{out_dir}/full.out`):");

        for (input, expected_line, expected_column) in cases {
            let engine = run_in(&out_dir, input);
            let diagnostic = engine
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.message.starts_with("Cannot write output stream"))
                .expect("write diagnostic");
            assert!(
                diagnostic.message.starts_with(&destination),
                "{}",
                diagnostic.message
            );
            let source = diagnostic.primary.as_ref().expect("write source");
            assert_eq!(
                (source.name.as_str(), source.line, source.column),
                ("output-error.tex", expected_line, expected_column)
            );
            assert_eq!(
                diagnostic.help.as_deref(),
                Some(
                    "check the destination path, permissions, and available disk space, then compile again"
                )
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn assignment_commands_consume_definition_prefixes_without_leaking_them() {
        let engine = run("\\count0=6\n\
             \\long\\advance\\count0 by 1\n\
             \\outer\\multiply\\count0 by 2\n\
             \\protected\\divide\\count0 by 7\n\
             \\long\\setbox0=\\hbox{}\n\
             \\outer\\wd0=1pt\n\
             \\protected\\fontdimen1\\nullfont=2pt\n\
             \\long\\partokenname\\par\n\
             \\def\\probe{}\n\
             \\end\n"
            .to_string());

        assert_eq!(engine.eqtb.count[0], 2);
        assert_eq!(engine.box_reg_dimen(0, 0), 65_536);
        let probe = engine.cs.lookup(b"probe").expect("probe definition");
        let crate::eqtb::Equiv::Macro(definition) =
            engine.eqtb.resolve(probe).expect("probe meaning")
        else {
            panic!("probe is not a macro");
        };
        assert!(!definition.long);
        assert!(!definition.outer);
        assert!(!definition.protected);
        assert!(!engine.global_flag);
        assert!(!engine.long_flag);
        assert!(!engine.outer_flag);
        assert!(!engine.protected_flag);

        for command in [
            "\\advance",
            "\\multiply",
            "\\divide",
            "\\setbox",
            "\\wd",
            "\\fontdimen",
            "\\partokenname",
        ] {
            let diagnostic = engine
                .diagnostics
                .iter()
                .find(|diagnostic| {
                    diagnostic.message.contains("You can't use")
                        && diagnostic.message.contains(command)
                })
                .unwrap_or_else(|| panic!("missing illegal-prefix diagnostic for {command}"));
            assert!(diagnostic.primary.is_some(), "{command} lacks a source");
        }
    }

    #[test]
    fn setbox_keeps_captured_globality_through_a_valid_box_body() {
        let engine = run("{\\global\\setbox0=\\hbox{global}}\n\
             {\\globaldefs=-1 \\global\\setbox1=\\hbox{local}}\n\
             \\end\n"
            .to_string());

        assert!(engine.eqtb.boxed[0].is_some());
        assert!(engine.eqtb.boxed[1].is_none());
        assert_eq!(engine.error_count, 0, "{}", engine.term);
    }

    #[test]
    fn rejected_and_malformed_prefixed_commands_clear_global_state() {
        let engine = run("{\\global\\afterassignment A\\count10=1}\n\
             {\\global\\aftergroup B\\count11=1}\n\
             {\\global\\copy2\\count12=1}\n\
             {\\global\\box2\\count13=1}\n\
             \\chardef\\letter=65 {\\global\\letter\\count14=1}\n\
             \\mathchardef\\symbol=42 {\\global\\symbol\\count15=1}\n\
             {\\global\\partokenname A\\count16=1}\n\
             {\\global\\setbox3=Q\\count17=1}\n\
             \\end\n"
            .to_string());

        for register in 10..=17 {
            assert_eq!(
                engine.eqtb.count[register], 0,
                "prefix leaked into count{register}"
            );
        }
        for command in [
            "\\afterassignment",
            "\\aftergroup",
            "\\copy",
            "\\box",
            "\\char\"41",
            "\\mathchar\"2A",
        ] {
            assert!(
                engine
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(command)),
                "missing diagnostic for {command}: {}",
                engine.term
            );
        }
    }

    /// Expected output from `pdftex -ini` (TeX Live 2026): the text is read
    /// as `{<text>}\endwrite`, so an extra `}` from expansion ends it.
    #[test]
    fn write_with_an_extra_close_brace_stops_like_tex() {
        let engine = run("\\def\\x{\\iffalse{\\fi}}\\immediate\\write16{A:a\\x d}\\end\n".into());
        assert!(engine.term.contains("A:a\n"), "{}", engine.term);
        assert!(!engine.term.contains("A:a}"), "{}", engine.term);
        assert_eq!(engine.error_count, 1, "{}", engine.term);
    }

    /// pdftex: after an extra `}` ends the text, TeX skips to \endwrite with
    /// the non-expanding get_token, so an undefined control sequence there is
    /// never expanded: "Unbalanced write command" is the only error.
    #[test]
    fn unbalanced_write_recovery_skips_without_expanding() {
        let engine = run("\\def\\x{\\iffalse{\\fi}}\
                          \\immediate\\write16{A:a\\x B\\nosuchmacro C}\\end\n"
            .into());
        assert!(engine.term.contains("A:a\n"), "{}", engine.term);
        assert_eq!(engine.error_count, 1, "{}", engine.term);
    }

    #[test]
    fn write_with_an_extra_open_brace_is_fatal() {
        let engine = run("\\def\\y{c{\\iffalse}\\fi}\\immediate\\write16{B:a\\y d}\\end\n".into());
        assert!(engine.stopped_on_error, "{}", engine.term);
        assert!(!engine.term.contains("B:a"), "{}", engine.term);
    }

    /// pdftex: `\edef\z{a\c}` gives `macro:->a ` and `\def\w{q{r\c}` gives
    /// `macro:->q{r } `, each forbidden `\outer` occurrence storing a space.
    #[test]
    fn outer_macros_end_definitions_like_tex() {
        let engine = run("\\outer\\def\\c{}\\edef\\z{a\\c}\\message{[\\meaning\\z]}\
                          \\def\\w{q{r\\c}\\message{[\\meaning\\w]}\\end\n"
            .into());
        assert!(engine.term.contains("[macro:->a ]"), "{}", engine.term);
        assert!(engine.term.contains("[macro:->q{r } ]"), "{}", engine.term);
        let forbidden = engine
            .diagnostics
            .iter()
            .filter(|d| d.message.starts_with("Forbidden control sequence found while scanning definition"))
            .count();
        assert_eq!(forbidden, 3, "{}", engine.term);
    }

    #[test]
    fn jobname_with_a_space_expands_quoted() {
        let mut engine = crate::engine::Engine::new(true);
        engine.init_primitives();
        engine.add_nullfont();
        engine.eqtb.cat[b'{' as usize] = 1;
        engine.eqtb.cat[b'}' as usize] = 2;
        engine.set_interaction_mode(InteractionMode::Nonstop);
        engine.job_name = "main file".to_string();
        engine.input.push_file(
            "j.tex".to_string(),
            b"\\message{[\\jobname]}\\end\n".to_vec(),
        );
        engine.run();
        assert!(engine.term.contains("[\"main file\"]"), "{}", engine.term);
        assert_eq!(engine.job_name, "main file");
    }

    /// Byte-for-byte pdfTeX `\pdfescapestring`, `\pdfescapename` and
    /// `\pdfescapehex` results (TeX Live 2026).
    #[test]
    fn pdf_escapes_match_pdftex() {
        use crate::prim::Prim;
        assert_eq!(super::pdf_escape(Prim::PdfEscapeString, b"a b(c)\\"), b"a\\040b\\(c\\)\\\\");
        assert_eq!(
            super::pdf_escape(Prim::PdfEscapeName, b"a b#/()<>[]{}%\xe9\x00"),
            b"a#20b#23#2F#28#29#3C#3E#5B#5D#7B#7D#25#E9"
        );
        assert_eq!(super::pdf_escape(Prim::PdfEscapeHex, b"AZ\n"), b"415A0A");
    }
}
