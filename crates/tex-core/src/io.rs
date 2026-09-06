//! I/O primitives: \input, \openout/\closeout/\write, \openin, \read,
//! \message, \special, \show, \lowercase/\uppercase, \advance arithmetic.

use crate::boxes::Node;
use crate::engine::{Engine, Mode};
use crate::eqtb::Equiv;
use crate::prim::Prim;
use crate::token::Token;

impl Engine {
    pub fn do_input(&mut self) {
        let name = self.scan_file_name();
        if name.is_empty() {
            self.error("\\input needs a file name");
            return;
        }
        let res = self.input_file(&name);
    }

    pub fn input_file(&mut self, name: &str) -> bool {
        if std::env::var("IOTRACE").is_ok() && name.ends_with(".aux") {
            let sz = std::fs::metadata(name).map(|m| m.len()).unwrap_or(99999);
            eprintln!("IO-AUX-READ {} size={}", name, sz);
        }
        let path = self.resolve_input_path(name);
        match path {
            Some(p) => match std::fs::read(&p) {
                Ok(data) => {
                    self.loaded_files.push(p.clone());
                    self.term.push_str(&format!("({} ", p.display()));
                    // tex.web start_input: the file sits above the current
                    // token list. `pushed` is that token list, so leftovers
                    // must park below the file even during \\output — else
                    // hook-csname tokens sit on top of an unread .fd.
                    if !self.pushed.is_empty() {
                        let mut rest = std::mem::take(&mut self.pushed);
                        rest.reverse();
                        self.input.push_toks(rest, "<after-input>");
                    }
                    self.input.push_file(p.display().to_string(), data);
                    true
                }
                Err(e) => {
                    self.error(&format!("Cannot read {}: {}", name, e));
                    false
                }
            },
            None => {
                // Fall back to embedded Virtual TDS package repository
                let clean_name = std::path::Path::new(name).file_name().and_then(|s| s.to_str()).unwrap_or(name);
                let cand_names = [clean_name.to_string(), format!("{}.sty", clean_name), format!("{}.cls", clean_name)];
                let mut found_data = None;
                for cand in &cand_names {
                    if let Some(pkg_data) = tex_kpse::get_embedded_package(cand) {
                        found_data = Some((cand.clone(), pkg_data.to_vec()));
                        break;
                    }
                }
                if let Some((cand_name, data)) = found_data {
                    self.term.push_str(&format!("(<embedded:{}> ", cand_name));
                    if !self.pushed.is_empty() {
                        let mut rest = std::mem::take(&mut self.pushed);
                        rest.reverse();
                        self.input.push_toks(rest, "<after-input>");
                    }
                    self.input.push_file(format!("<embedded:{}>", cand_name), data);
                    return true;
                }
                self.error(&format!("File `{}` not found", name));
                false
            }
        }
    }

    /// Resolve `name` for \input/\openin the way web2c's open_input does:
    /// when -output-directory is set, relative names are looked up there
    /// first (kpathsea's TEXMF_OUTPUT_DIRECTORY behavior), then kpathsea's
    /// format search path (TDS, env paths, cwd, explicit paths).
    fn resolve_input_path(&self, name: &str) -> Option<std::path::PathBuf> {
        if name.is_empty() {
            return None;
        }
        // LaTeX's \@missingfileerror give-up path (no terminal to answer the
        // "Enter file name:" prompt) re-requests the degenerate name ".tex".
        // Serve a placeholder file so batch/nonstop runs abort the
        // missing-input attempt once and continue instead of looping on the
        // unanswerable prompt (web2c TeX emergency-stops here; interactive
        // sessions prompt for a replacement).
        if name == ".tex" {
            let guard = std::env::temp_dir().join("tex-missingfile-guard.tex");
            let _ = std::fs::write(&guard, b"\\relax\n");
            return Some(guard);
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
            let _ = std::fs::write(&guard, b"\\relax\n");
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
            for cand in [std::path::Path::new(name).to_path_buf(), std::path::Path::new(&format!("{name}.tex")).to_path_buf()] {
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
        self.font_loader.kpse.find(name, tex_kpse::Format::Tex)
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

    pub fn do_openout(&mut self) {
        if std::env::var("IOTRACE").is_ok() {
            eprintln!("IO-OPENOUT at line {}", self.input.current_file_line());
        }
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
        match std::fs::File::create(&full) {
            Ok(f) => {
                let idx = (n as usize).min(self.write_streams.len() - 1);
                self.write_streams[idx] = Some(f);
            }
            Err(e) => self.error(&format!("Cannot open {} for writing: {}", full, e)),
        }
    }

    pub fn do_closeout(&mut self) {
        let n = self.scan_int();
        let idx = (n as usize).min(self.write_streams.len() - 1);
        if let Some(f) = self.write_streams[idx].take() {
            drop(f);
        }
    }

    pub fn do_write(&mut self, immediate: bool) {
        if crate::debug_flag("DEFTRACE") {
            eprintln!("DOWRITE top_pushed={}", self.pushed.len());
        }
        let n = self.scan_int();
        // tex.web §1371: \write<n>{toks} collects the list RAW (scan_toks,
        // no expansion) and expands at emission like \xdef (protected macros
        // stay frozen).
        let toks = self.scan_general_text();
        // tex.web §1395: plain \write to a FILE stream queues a whatsit and
        // expands at SHIPOUT, so \thepage resolves to the page that actually
        // ships the node. Terminal/log streams (and \immediate\write) emit
        // now: their visible ordering is cosmetic and pdfTeX users expect it.
        if !immediate && n >= 0 && n <= 15 {
            let toks = toks;
            self.append_whatsit(Node::Whatsit(crate::boxes::WhatIt::Write {
                stream: n as u16,
                tokens: toks,
            }));
            return;
        }
        let text = self.expand_write_list(&toks);
        self.write_out(n, &text);
    }

    /// tex.web §1395 out_what: a Write whatsit fires at ship time, expanding
    /// its token list with the page counter of the page being shipped.
    pub fn fire_write(&mut self, stream: u16, tokens: &[Token]) {
        let text = self.expand_write_list(tokens);
        self.write_out(stream as i32, &text);
    }

    /// Expand a raw \\write token list to its emitted string: standalone
    /// toklist source, edef expansion rules, outer `pushed` parked so it
    /// cannot leak into the output.
    fn expand_write_list(&mut self, toks: &[Token]) -> String {
        let saved = std::mem::take(&mut self.pushed);
        let stack_depth = self.input.stack.len();
        // LaTeX \set@display@protect contract: at write/emit time \protect
        // is \noexpand, so `\protect\BOOKMARK` emits `\BOOKMARK` raw
        // instead of running it (hyperref .out writes).
        let protect_saved = self.cs.lookup(b"protect").map(|pid| {
            let old = self.eqtb.get(pid).cloned();
            if let Some(nid) = self.cs.lookup(b"noexpand") {
                if let Some(eq) = self.eqtb.get(nid).cloned() {
                    self.eqtb.assign(pid, eq, false);
                }
            }
            old
        });
        // The sentinel bounds the expansion: without it, a list whose final
        // token expands away would let get_token continue into the OUTER
        // stream and leak its tokens into the write string.
        let mut body = toks.to_vec();
        body.push(crate::page::WRITE_END_TOKEN);
        self.input.push_toks(body, "<write>");
        let prev = self.in_expanded_scan;
        self.in_expanded_scan = true;
        let mut out: Vec<Token> = Vec::new();
        loop {
            let t = self.get_token();
            if t == crate::page::WRITE_END_TOKEN || t == crate::input::EOF_MARKER {
                break;
            }
            out.push(t);
        }
        self.in_expanded_scan = prev;
        self.input.stack.truncate(stack_depth);
        self.pushed = saved;
        if let Some(pid) = self.cs.lookup(b"protect") {
            if let Some(Some(old)) = &protect_saved {
                self.eqtb.assign(pid, old.clone(), false);
            }
        }
        self.write_tokens_to_string(&out)
    }

    pub fn write_tokens_to_string(&self, toks: &[Token]) -> String {
        let mut out = String::new();
        for t in toks {
            if t.is_cs() {
                let name = self.cs.name(t.cs_id());
                // Active-char placeholder ids (engine::active_cs_name):
                // [0xFF,0,'A','C','T',0,c] — detokenize as the character
                // byte c, not the internal name.
                if name.len() == 7 && name[0] == 0xFF && &name[2..5] == b"ACT" && name[5] == 0 {
                    out.push(name[6] as char);
                } else {
                    out.push('\\');
                    out.push_str(&String::from_utf8_lossy(name));
                    out.push(' ');
                }
            } else {
                out.push(t.chr() as u8 as char);
            }
        }
        out
    }

    pub fn write_out(&mut self, n: i32, text: &str) {
        let line = format!("{}\n", text);
        match n {
            -1 => { self.log.push_str(&line);
                    if crate::debug_flag("DEFTRACE") {
                        eprintln!("LOG: {}", text); } }
            -2 => { self.term.push_str(&line); }
            16 | 17 | 18 => { self.term.push_str(&line); self.log.push_str(&line); }
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
                        self.term.push_str(&line);
                        self.log.push_str(&line);
                    }
                }
            }
        }
    }

    pub fn do_special(&mut self) {
        let _ = self.scan_general_text_expanded();
    }

    pub fn do_message(&mut self, err: bool) {
        let toks = self.scan_general_text_expanded();
        let text = self.write_tokens_to_string(&toks);
        if err {
            // tex.web print_err → print_nl("! "): the error starts on a
            // fresh line, never glued to unterminated "(file" output.
            self.term_print_nl(&format!("! {}\n", text));
        } else {
            self.term.push_str(&format!("{}\n", text));
            self.log.push_str(&format!("{}\n", text));
        }
    }

    pub fn do_openin(&mut self) {
        let n = self.scan_int();
        self.scan_optional_equals();
        let name = self.scan_file_name();
        while self.read_files.len() <= n as usize {
            self.read_files.push(None);
            self.read_eof.push(true);
        }
        // kpathsea/web2c lookup: output directory first for relative
        // names, then the kpse search path (covers literal paths too)
        if crate::debug_flag("IFTRACE") {
            eprintln!("OPENIN {} -> {:?}", name, self.resolve_input_path(&name));
        }
        let path = match self.resolve_input_path(&name) {
            Some(p) => p,
            None => {
                self.read_files[n as usize] = None;
                self.read_eof[n as usize] = true;
                return;
            }
        };
        match std::fs::File::open(&path) {
            Ok(f) => {
                self.read_files[n as usize] = Some(std::io::BufReader::new(f));
                self.read_eof[n as usize] = false;
            }
            Err(_) => {
                self.read_files[n as usize] = None;
                self.read_eof[n as usize] = true;
            }
        }
    }

    pub fn do_closein(&mut self) {
        let n = self.scan_int();
        let n = n.max(0) as usize;
        while self.read_files.len() <= n {
            self.read_files.push(None);
            self.read_eof.push(true);
        }
        self.read_files[n] = None;
        self.read_eof[n] = true;
    }

    pub fn do_read(&mut self, line_mode: bool) {
        let n = self.scan_int();
        self.scan_optional_equals();
        // tex.web: keyword `to`, then the target cs. Spaces are ignored.
        let mut t = self.raw_token();
        while !t.is_cs() && t.cc() == 10 {
            t = self.raw_token();
        }
        if !t.is_cs() && t.chr() == u32::from(b't') {
            let mut o = self.raw_token();
            while !o.is_cs() && o.cc() == 10 {
                o = self.raw_token();
            }
            if o.is_cs() || o.chr() != u32::from(b'o') {
                self.pushed.push(o);
            }
        } else {
            self.pushed.push(t);
        }
        let cs = self.scan_definable_cs();
        // tex.web: a negative stream number reads the TERMINAL, never the
        // numeric stream 0 (which LaTeX keeps for \@inputcheck). The engine
        // has no interactive terminal, so a terminal read is always at EOF;
        // web2c treats that as fatal ("! Emergency stop.") instead of
        // returning lines — LaTeX's \@missingfileerror retry loop depends on
        // this to abort a missing-\input instead of spinning forever.
        let is_terminal = n < 0;
        let mut stream_eof = false;
        let n = n.max(0) as usize;
        while self.read_files.len() <= n {
            self.read_files.push(None);
            self.read_eof.push(true);
        }
        use std::io::BufRead;
        let line: Option<String> = if is_terminal {
            None
        } else {
            match &mut self.read_files[n] {
            Some(reader) => {
                let mut buf = String::new();
                match reader.read_line(&mut buf) {
                    Ok(0) | Err(_) => {
                        self.read_eof[n] = true;
                        stream_eof = true;
                        None
                    }
                    Ok(_) => {
                        if buf.ends_with('\n') {
                            buf.pop();
                        }
                        if buf.ends_with('\r') {
                            buf.pop();
                        }
                        Some(buf)
                    }
                }
            }
            None => {
                self.read_eof[n] = true;
                stream_eof = true;
                None
            }
            }
        };
        let toks: Vec<Token> = match line {
            Some(l) => {
                if l.is_empty() && !line_mode {
                    vec![Token::from_cs(self.cs.lookup(b"par").unwrap_or(0))]
                } else {
                    let mut toks = Vec::new();
                    for b in l.bytes() {
                        let cat = if line_mode {
                            if b == b' ' { 10 } else { 12 }
                        } else {
                            self.eqtb.cat[b as usize]
                        };
                        toks.push(Token::char(cat, b as u32));
                    }
                    // tex.web \\read: endlinechar (usually ^^M cat 5) becomes a
                    // space. expl3 \\ior_get + "#9 ~ \\q_stop" needs that space.
                    if !line_mode {
                        toks.push(Token::space());
                    }
                    toks
                }
            }
            None if is_terminal => {
                // web2c: terminal read with exhausted input is fatal. Print
                // like a real error (fresh line, current-line context) and
                // abort the run; assigning the target an empty body keeps
                // the \read assignment itself well-formed.
                self.term_print_nl("! Emergency stop.\n");
                if let Some(crate::input::Source::File { line_buf, line_no, .. }) = self.input.stack.last() {
                    if let Some(buf) = line_buf {
                        let text = String::from_utf8_lossy(buf);
                        let text = text.trim_end_matches(['\n', '\r']);
                        self.term.push_str(&format!("l.{} {}\n", line_no, text));
                    }
                }
                self.error_count += 1;
                self.end_occurred = true;
                Vec::new()
            }
            None if stream_eof => Vec::new(),
            None => vec![Token::from_cs(self.cs.lookup(b"par").unwrap_or(0))],
        };
        if line_mode || crate::debug_flag("IORTRACE") {
            static RN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if RN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
                eprintln!(
                    "READLINE n={} mode={} cs=\\{} ntoks={} body=[{}]",
                    n,
                    line_mode,
                    String::from_utf8_lossy(self.cs.name(cs)),
                    toks.len(),
                    self.tokens_to_string(&toks.iter().take(40).cloned().collect::<Vec<_>>())
                );
            }
        }
        let m = crate::eqtb::Macro { num_params: 0, params: Vec::new(), body: toks, prefix: Vec::new(), long: false, outer: false, protected: false };
        self.eqtb.assign(cs, Equiv::Macro(std::rc::Rc::new(m)), self.global_flag);
        self.global_flag = false;
    }

    pub fn scan_file_name(&mut self) -> String {
        self.skip_spaces_relax();
        let mut name = Vec::new();
        let t = self.get_x_raw();
        if t == crate::input::EOF_MARKER {
            return String::new();
        }
        if t.is_char() && (t.cc() == 1 || t.chr() == b'{' as u32) {
            // LaTeX \input{filename.tex} syntax
            let mut depth = 1i32;
            loop {
                let t2 = self.get_x_raw();
                if t2 == crate::input::EOF_MARKER {
                    break;
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
                    name.push(t2.chr() as u8);
                } else if t2.is_cs() {
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
                    break;
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
                    name.push(t2.chr() as u8);
                } else if t2.is_cs() {
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
        let loc = self.scan_quantity();
        self.scan_keyword(b"by");
        let v = match loc {
            QuantityLoc::Int(_) | QuantityLoc::Count(_) => Value::Int(self.scan_int()),
            QuantityLoc::Dim(_) | QuantityLoc::Dimen(_) => Value::Dim(self.scan_dimen(false, false)),
            QuantityLoc::Glue(_) | QuantityLoc::Skip(_) => Value::Glue(self.scan_glue(false)),
            QuantityLoc::MuSkip(_) => Value::Glue(self.scan_glue(true)),
            QuantityLoc::None => return,
        };
        match loc {
            QuantityLoc::Int(p) => {
                let cur = self.int_param_value(p);
                self.eqtb.assign_int_param(p, cur.wrapping_add(v.as_int()), self.global_flag);
            }
            QuantityLoc::Count(i) => {
                let cur = self.eqtb.count[i as usize];
                self.eqtb.assign_count(i, cur.wrapping_add(v.as_int()), self.global_flag);
            }
            QuantityLoc::Dim(p) => {
                let cur = self.dim_param_value(p);
                let nv = cur.wrapping_add(v.as_dim());
                if p == crate::prim::DimParam::PrevDepth {
                    self.prev_depth = nv;
                } else {
                    self.eqtb.assign_dim_param(p, nv, self.global_flag);
                }
            }
            QuantityLoc::Dimen(i) => {
                let cur = self.eqtb.dimen[i as usize];
                self.eqtb.assign_dimen(i, cur.wrapping_add(v.as_dim()), self.global_flag);
            }
            QuantityLoc::Glue(p) => {
                let mut cur = self.eqtb.glue_params[p.idx() as usize].clone();
                cur = glue_plus(&cur, &v.as_glue());
                self.eqtb.assign_glue_param(p, cur, self.global_flag);
            }
            QuantityLoc::Skip(i) => {
                let mut cur = self.eqtb.skip[i as usize].clone();
                cur = glue_plus(&cur, &v.as_glue());
                self.eqtb.assign_skip(i, cur, self.global_flag);
            }
            QuantityLoc::MuSkip(i) => {
                let mut cur = self.eqtb.muskip[i as usize].clone();
                cur = glue_plus(&cur, &v.as_glue());
                self.eqtb.assign_muskip(i, cur, self.global_flag);
            }
            QuantityLoc::None => {}
        }
        self.global_flag = false;
    }

    pub fn do_arith(&mut self, op: u8) {
        // \multiply / \divide
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
                self.eqtb.assign_int_param(p, self.arith(cur, n, op), self.global_flag);
            }
            QuantityLoc::Count(i) => {
                let cur = self.eqtb.count[i as usize];
                self.eqtb.assign_count(i, self.arith(cur, n, op), self.global_flag);
            }
            QuantityLoc::Dim(p) => {
                let cur = self.dim_param_value(p);
                let nv = self.arith(cur, n, op);
                if p == crate::prim::DimParam::PrevDepth {
                    self.prev_depth = nv;
                } else {
                    self.eqtb.assign_dim_param(p, nv, self.global_flag);
                }
            }
            QuantityLoc::Dimen(i) => {
                let cur = self.eqtb.dimen[i as usize];
                self.eqtb.assign_dimen(i, self.arith(cur, n, op), self.global_flag);
            }
            QuantityLoc::Glue(p) => {
                let mut cur = self.eqtb.glue_params[p.idx() as usize].clone();
                for x in [&mut cur.width, &mut cur.stretch, &mut cur.shrink] {
                    *x = self.arith(*x, n, op);
                }
                self.eqtb.assign_glue_param(p, cur, self.global_flag);
            }
            QuantityLoc::Skip(i) => {
                let mut cur = self.eqtb.skip[i as usize].clone();
                for x in [&mut cur.width, &mut cur.stretch, &mut cur.shrink] {
                    *x = self.arith(*x, n, op);
                }
                self.eqtb.assign_skip(i, cur, self.global_flag);
            }
            _ => {}
        }
        self.global_flag = false;
    }

    fn arith(&self, a: i32, b: i32, op: u8) -> i32 {
        // tex.web: \\multiply/\\divide are integer ops, not scaled\\_mult
        // (\\@settopoint does \\divide#1\\p@\\multiply#1\\p@).
        match op {
            1 => {
                let v = a as i128 * b as i128;
                v.clamp(i32::MIN as i128, i32::MAX as i128) as i32
            }
            _ => {
                if b == 0 {
                    0
                } else {
                    a / b
                }
            }
        }
    }

    pub fn do_setbox(&mut self) {
        let idx = self.scan_reg_num();
        self.scan_optional_equals();
        // <box spec>: \box<n> | \hbox.. | \vbox.. | \vtop.. | \copy<n> | \lastbox
        self.skip_spaces_relax();
        let t = self.get_token();
        if !t.is_cs() {
            self.pushed.push(t);
            self.error("Missing box for \\setbox");
            return;
        }
        // l3 aliases (\tex_lastbox:D etc.) resolve to the same primitives;
        // dispatch on meaning, fall back to raw name for \copy/\usebox.
        if let Some(prim) = self.cur_prim {
            match prim {
                Prim::Box => {
                    let n = self.scan_reg_num();
                    let b = self.eqtb.boxed.get(n as usize).cloned().flatten();
                    self.eqtb.assign_box(n, None, true);
                    self.eqtb.assign_box(idx, b, self.global_flag);
                    self.global_flag = false;
                    return;
                }
                Prim::Copy => {
                    let n = self.scan_reg_num();
                    let b = self.eqtb.boxed.get(n as usize).cloned().flatten();
                    self.eqtb.assign_box(idx, b, self.global_flag);
                    self.global_flag = false;
                    return;
                }
                Prim::LastBox => {
                    let b = self.take_last_box();
                    self.eqtb.assign_box(idx, b, self.global_flag);
                    self.global_flag = false;
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
                    let (top, m, rest) = self.scan_vsplit();
                    if let Some(rest) = rest {
                        self.stash_vsplit_remainder(m, rest);
                    }
                    self.eqtb.assign_box(idx, top, self.global_flag);
                    self.global_flag = false;
                    return;
                }
                _ => {}
            }
        }

        match self.cs.name(t.cs_id()) {
            b"box" => {
                let n = self.scan_reg_num();
                let b = self.eqtb.boxed[n as usize].take();
                self.eqtb.assign_box(idx, b, self.global_flag);
                self.global_flag = false;
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
                // \setbox<n>=\vsplit<m> to <dimen>: split box m; the top
                // part lands in box n, the remainder returns to box m
                let (top, m, rest) = self.scan_vsplit();
                if let Some(rest) = rest {
                    self.stash_vsplit_remainder(m, rest);
                }
                self.eqtb.assign_box(idx, top, self.global_flag);
                self.global_flag = false;
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
        if let Node::Box { kind, w, h, d, shift, list, .. } = b {
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
            Node::Char { c, font } => out.push_str(&format!("the character {} (font {})\n", *c as char, font)),
            Node::Glue(g) => out.push_str(&format!("glue {}\n", self.glue_to_string(g))),
            Node::Kern(k) => out.push_str(&format!("kern {}\n", self.scaled_to_string(*k))),
            Node::Penalty(p) => out.push_str(&format!("penalty {}\n", p)),
            Node::Rule { width, height, depth } => out.push_str(&format!(
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

use crate::prim::{DimParam, GlueParam, IntParam};
