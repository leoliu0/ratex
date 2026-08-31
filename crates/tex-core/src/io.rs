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
        self.input_file(&name);
    }

    pub fn input_file(&mut self, name: &str) -> bool {
        let path = self.resolve_input_path(name);
        match path {
            Some(p) => match std::fs::read(&p) {
                Ok(data) => {
                    self.term.push_str(&format!("({} ", p.display()));
                    self.input.push_file(p.display().to_string(), data);
                    true
                }
                Err(e) => {
                    self.error(&format!("Cannot read {}: {}", name, e));
                    false
                }
            },
            None => {
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
        if !name.starts_with('/') && !self.out_dir.is_empty() {
            let cand = std::path::Path::new(&self.out_dir).join(name);
            if cand.is_file() {
                return Some(cand);
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

    pub fn do_openout(&mut self) {
        // \openout<n>=<file>
        let n = self.scan_int();
        self.scan_optional_equals();
        let name = self.scan_file_name();
        // web2c open_output: the -output-directory prefix applies to
        // relative names only; absolute names bypass it
        let full = if !self.out_dir.is_empty() && !name.starts_with('/') {
            format!("{}{}", self.out_dir, name)
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

    pub fn do_write(&mut self) {
        if std::env::var("DEFTRACE").map(|v|v=="1").unwrap_or(false) {
            eprintln!("DOWRITE top_pushed={}", self.pushed.len());
        }
        let n = self.scan_int();
        // \write<n>{tokens}: expand tokens now (TeX expands at shipout; LaTeX
        // expects expansion at the point of \write for most usage)
        let toks = self.scan_general_text_expanded();
        let text = self.write_tokens_to_string(&toks);
        self.write_out(n, &text);
    }

    pub fn write_tokens_to_string(&self, toks: &[Token]) -> String {
        let mut out = String::new();
        for t in toks {
            if t.is_cs() {
                out.push('\\');
                out.push_str(&String::from_utf8_lossy(self.cs.name(t.cs_id())));
                out.push(' ');
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
                    if std::env::var("DEFTRACE").map(|v|v=="1").unwrap_or(false) {
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
            self.term.push_str(&format!("! {}\n", text));
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
        if std::env::var("IFTRACE").map(|v|v=="1").unwrap_or(false) {
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
                self.read_files[n as usize] = Some(f);
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
        // tex.web syntax: `\read<n> to <cs>` — skip the literal keyword `to`
        // (tex.web scans it off before the target control sequence).
        let mut t = self.raw_token();
        while !t.is_cs() && t.cc() == 10 {
            t = self.raw_token();
        }
        if !t.is_cs() && t.chr() == u32::from(b't') {
            let mut t1 = self.raw_token();
            while !t1.is_cs() && t1.cc() == 10 {
                t1 = self.raw_token();
            }
            if t1.is_cs() {
                // `\read0 to\cs`: the cs directly follows; hand it back
                self.pushed.push(t1);
            }
        } else {
            // no `to` keyword: give the token back to scan_definable_cs
            self.pushed.push(t);
        }
        let cs = self.scan_definable_cs();
        let n = n.max(0) as usize;
        while self.read_files.len() <= n {
            self.read_files.push(None);
            self.read_eof.push(true);
        }
        use std::io::BufRead;
use std::io::BufReader;
        let line: Option<String> = match &mut self.read_files[n] {
            Some(f) => {
                let mut buf = String::new();
                let mut reader = std::io::BufReader::new(&mut *f);
                match reader.read_line(&mut buf) {
                    Ok(0) | Err(_) => {
                        self.read_eof[n] = true;
                        None
                    }
                    Ok(_) => Some(if line_mode { buf } else { buf.trim_end().to_string() }),
                }
            }
            None => {
                // read from terminal input: not supported; treat as EOF
                self.read_eof[n] = true;
                None
            }
        };
        let toks: Vec<Token> = match line {
            Some(l) => {
                let mut toks = Vec::new();
                for b in l.bytes() {
                    let cat = self.eqtb.cat[b as usize];
                    toks.push(Token::char(cat, b as u32));
                }
                toks
            }
            None => vec![Token::from_cs(self.cs.lookup(b"par").unwrap_or(0))],
        };
        // tex.web read_toks uses scan_toks(false,false): a plain (non-\long)
        // macro, so that \ifx against an \edef'd macro of the same text is true
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
        // standard TeX \input filename.tex (unquoted). tex.web scan_file_name:
        // cs tokens TERMINATE the name (and are reread); only chars accumulate.
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
            cur = self.raw_token();
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
            if !t.is_cs() {
                let c = t.chr() as u8;
                let mapped = if up {
                    self.eqtb.uc_code[c as usize]
                } else {
                    self.eqtb.lc_code[c as usize]
                };
                // tex.web §1289: lccode/uccode 0 = leave unchanged
                if mapped != 0 {
                    *t = Token::char(t.cc(), mapped as u32);
                }
            } else {
                let name = self.cs.name(t.cs_id());
                if name.len() == 1 {
                    let c = name[0];
                    let mapped = if up {
                        self.eqtb.uc_code[c as usize]
                    } else {
                        self.eqtb.lc_code[c as usize]
                    };
                    if mapped != 0 && mapped != c {
                        let new_id = self.cs.intern(&[mapped]);
                        *t = Token::from_cs(new_id);
                    }
                }
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
                let cur = self.eqtb.dim_params[p.idx() as usize];
                self.eqtb.assign_dim_param(p, cur.wrapping_add(v.as_dim()), self.global_flag);
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
                let cur = self.eqtb.dim_params[p.idx() as usize];
                self.eqtb.assign_dim_param(p, self.arith(cur, n, op), self.global_flag);
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
        match op {
            1 => crate::scaled::mult(a, b),
            _ => {
                if b == 0 {
                    0
                } else {
                    crate::scaled::x_over_y(a, b)
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
                self.setbox_target = Some(idx);
                let kind = match self.cs.name(t.cs_id()) {
                    b"hbox" => 0,
                    b"vbox" => 1,
                    b"vtop" => 2,
                    _ => 3,
                };
                self.begin_box(kind);
            }
            b"halign" => {
                // \setbox<n>=\halign{...}: the alignment result lands in box n
                self.setbox_target = Some(idx);
                self.begin_halign();
            }
            b"usebox" => {
                let n = self.scan_reg_num();
                let b = self.eqtb.boxed[n as usize].take();
                self.eqtb.assign_box(idx, b, self.global_flag);
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
