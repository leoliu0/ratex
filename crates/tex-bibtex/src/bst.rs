//! Reading and executing the `.bst` file (bibtex.web parts 10 and 11).
//! Commands are executed as soon as they are read, so error messages and
//! output interleave exactly as in BibTeX.

use std::rc::Rc;

use crate::engine::{Engine, Fatal, FnId, FnKind, Op};
use crate::input::{is_white, IdScan, Scanner};

/// How a `.bst` command ended abnormally.
pub(crate) enum Stop {
    /// error reported; continue with the next command
    Skip,
    /// end of the style file reached during error recovery
    Done,
    Fatal,
}

impl From<Fatal> for Stop {
    fn from(_: Fatal) -> Self {
        Stop::Fatal
    }
}

type R<T = ()> = Result<T, Stop>;

#[derive(Clone, Copy)]
enum ListKind {
    Field,
    IntEntry,
    StrEntry,
    IntGlobal,
    StrGlobal,
}

impl Engine {
    pub(crate) fn read_bst(&mut self) -> Result<(), Fatal> {
        let mut sc = Scanner::new(std::mem::take(&mut self.bst_bytes));
        sc.p2 = sc.last;
        loop {
            if !self.eat_bst_white(&mut sc) {
                return Ok(());
            }
            match self.bst_command(&mut sc) {
                Ok(()) | Err(Stop::Skip) => {}
                Err(Stop::Done) => return Ok(()),
                Err(Stop::Fatal) => return Err(Fatal),
            }
        }
    }

    /// `bst_ln_num_print`.
    pub(crate) fn bst_ln_num_print(&mut self) {
        let name = self.bst_file_name();
        self.log.print(format!("--line {} of file ", self.bst_line));
        self.log.println(name);
    }

    /// `eat_bst_white_space`: skip white space and `%` comments.
    fn eat_bst_white(&mut self, sc: &mut Scanner) -> bool {
        loop {
            if sc.scan_white_space() && sc.scan_char() != b'%' {
                self.bst_line = sc.line;
                return true;
            }
            if !sc.input_ln() {
                self.bst_line = sc.line;
                return false;
            }
            sc.line += 1;
            sc.p2 = 0;
        }
    }

    /// `bst_err`: report, then skip to a blank line (end of command).
    fn bst_err(&mut self, sc: &mut Scanner, msg: impl AsRef<[u8]>) -> Stop {
        self.log.print(msg);
        self.bst_err_print_and_look_for_blank_line(sc)
    }

    fn bst_err_print_and_look_for_blank_line(&mut self, sc: &mut Scanner) -> Stop {
        self.log.print("-");
        self.bst_ln_num_print();
        self.print_bad_input_line(sc);
        while sc.last != 0 {
            if !sc.input_ln() {
                return Stop::Done;
            }
            sc.line += 1;
        }
        sc.p2 = sc.last;
        Stop::Skip
    }

    fn eat_bst_white_check(&mut self, sc: &mut Scanner, cmd: &str) -> R {
        if self.eat_bst_white(sc) {
            return Ok(());
        }
        self.log.print("Illegal end of style file in command: ");
        Err(self.bst_err(sc, cmd))
    }

    fn bst_identifier_scan(&mut self, sc: &mut Scanner, cmd: &str) -> R {
        match sc.scan_identifier(b'}', b'%', b'%') {
            IdScan::WhiteAdjacent | IdScan::SpecifiedCharAdjacent => Ok(()),
            IdScan::Null => {
                let msg = format!("\"{}\" begins identifier, command: {cmd}", sc.scan_char() as char);
                Err(self.bst_err_bytes(sc, msg))
            }
            IdScan::OtherCharAdjacent => {
                let msg = format!(
                    "\"{}\" immediately follows identifier, command: {cmd}",
                    sc.scan_char() as char
                );
                Err(self.bst_err_bytes(sc, msg))
            }
        }
    }

    fn bst_err_bytes(&mut self, sc: &mut Scanner, msg: String) -> Stop {
        // messages quote raw input bytes; `as char` above maps bytes
        // >= 128 to other code points, so rebuild them byte-exactly
        let bytes: Vec<u8> = msg.chars().map(|c| c as u32 as u8).collect();
        self.bst_err(sc, bytes)
    }

    fn bst_left_brace(&mut self, sc: &mut Scanner, cmd: &str) -> R {
        if sc.scan_char() != b'{' {
            self.log.print("\"{\" is missing in command: ");
            return Err(self.bst_err(sc, cmd));
        }
        sc.p2 += 1;
        Ok(())
    }

    fn bst_right_brace(&mut self, sc: &mut Scanner, cmd: &str) -> R {
        if sc.scan_char() != b'}' {
            self.log.print("\"}\" is missing in command: ");
            return Err(self.bst_err(sc, cmd));
        }
        sc.p2 += 1;
        Ok(())
    }

    /// Insert a new function name, complaining if it already exists.
    fn insert_new_fn(&mut self, sc: &mut Scanner, kind: FnKind) -> R<FnId> {
        sc.lower_case_token();
        if let Some(id) = self.lookup_fn(sc.token()) {
            let mut msg = self.fns[id as usize].name.to_vec();
            msg.extend_from_slice(b" is already a type \"");
            msg.extend_from_slice(self.fn_class_name(id).as_bytes());
            self.log.print(msg);
            self.log.println("\" function name");
            return Err(self.bst_err_print_and_look_for_blank_line(sc));
        }
        let name = sc.token().to_vec();
        Ok(self.define(&name, kind))
    }

    fn bst_command(&mut self, sc: &mut Scanner) -> R {
        if !sc.scan_alpha() {
            let msg = format!("\"{}\" can't start a style-file command", sc.scan_char() as char);
            return Err(self.bst_err_bytes(sc, msg));
        }
        sc.lower_case_token();
        match sc.token() {
            b"entry" => self.bst_entry(sc),
            b"execute" => self.bst_execute(sc, "execute"),
            b"function" => self.bst_function(sc),
            b"integers" => self.bst_list_command(sc, "integers", ListKind::IntGlobal),
            b"iterate" => self.bst_execute(sc, "iterate"),
            b"macro" => self.bst_macro(sc),
            b"read" => self.bst_read(sc),
            b"reverse" => self.bst_execute(sc, "reverse"),
            b"sort" => {
                if !self.read_seen {
                    return Err(self.bst_err(sc, "Illegal, sort command before read command"));
                }
                self.cmd_sort();
                Ok(())
            }
            b"strings" => self.bst_list_command(sc, "strings", ListKind::StrGlobal),
            _ => {
                let tok = sc.token().to_vec();
                self.log.print(tok);
                Err(self.bst_err(sc, " is an illegal style-file command"))
            }
        }
    }

    /// One brace-delimited list of new names (`ENTRY` parts, `INTEGERS`,
    /// `STRINGS`).
    fn bst_name_list(&mut self, sc: &mut Scanner, cmd: &str, kind: ListKind) -> R {
        self.bst_left_brace(sc, cmd)?;
        self.eat_bst_white_check(sc, cmd)?;
        while sc.scan_char() != b'}' {
            self.bst_identifier_scan(sc, cmd)?;
            let new_kind = match kind {
                ListKind::Field => FnKind::Field(self.num_fields),
                ListKind::IntEntry => FnKind::IntEntry(self.num_ent_ints),
                ListKind::StrEntry => FnKind::StrEntry(self.num_ent_strs),
                ListKind::IntGlobal => FnKind::IntGlobal(self.glb_ints.len()),
                ListKind::StrGlobal => FnKind::StrGlobal(self.glb_strs.len()),
            };
            self.insert_new_fn(sc, new_kind)?;
            match kind {
                ListKind::Field => self.num_fields += 1,
                ListKind::IntEntry => self.num_ent_ints += 1,
                ListKind::StrEntry => self.num_ent_strs += 1,
                ListKind::IntGlobal => self.glb_ints.push(0),
                ListKind::StrGlobal => self.glb_strs.push(self.null_str.clone()),
            }
            self.eat_bst_white_check(sc, cmd)?;
        }
        sc.p2 += 1;
        Ok(())
    }

    fn bst_list_command(&mut self, sc: &mut Scanner, cmd: &str, kind: ListKind) -> R {
        self.eat_bst_white_check(sc, cmd)?;
        self.bst_name_list(sc, cmd, kind)
    }

    fn bst_entry(&mut self, sc: &mut Scanner) -> R {
        if self.entry_seen {
            return Err(self.bst_err(sc, "Illegal, another entry command"));
        }
        self.entry_seen = true;
        self.eat_bst_white_check(sc, "entry")?;
        self.bst_name_list(sc, "entry", ListKind::Field)?;
        self.eat_bst_white_check(sc, "entry")?;
        if self.num_fields == 1 {
            self.log.print("Warning--I didn't find any fields");
            self.bst_ln_num_print();
            self.log.mark_warning();
        }
        self.bst_name_list(sc, "entry", ListKind::IntEntry)?;
        self.eat_bst_white_check(sc, "entry")?;
        self.bst_name_list(sc, "entry", ListKind::StrEntry)
    }

    /// `EXECUTE`, `ITERATE` and `REVERSE`.
    fn bst_execute(&mut self, sc: &mut Scanner, cmd: &str) -> R {
        if !self.read_seen {
            let msg = format!("Illegal, {cmd} command before read command");
            return Err(self.bst_err(sc, msg));
        }
        self.eat_bst_white_check(sc, cmd)?;
        self.bst_left_brace(sc, cmd)?;
        self.eat_bst_white_check(sc, cmd)?;
        self.bst_identifier_scan(sc, cmd)?;
        sc.lower_case_token();
        let Some(id) = self.lookup_fn(sc.token()) else {
            let tok = sc.token().to_vec();
            self.log.print(tok);
            return Err(self.bst_err(sc, " is an unknown function"));
        };
        if !matches!(self.fns[id as usize].kind, FnKind::Builtin(_) | FnKind::Wiz(_)) {
            let tok = sc.token().to_vec();
            self.log.print(tok);
            self.log.print(format!(" has bad function type {}", self.fn_class_name(id)));
            return Err(self.bst_err_print_and_look_for_blank_line(sc));
        }
        self.eat_bst_white_check(sc, cmd)?;
        self.bst_right_brace(sc, cmd)?;
        match cmd {
            "execute" => self.cmd_execute(id)?,
            "iterate" => self.cmd_iterate(id, false)?,
            _ => self.cmd_iterate(id, true)?,
        }
        Ok(())
    }

    fn bst_function(&mut self, sc: &mut Scanner) -> R {
        self.eat_bst_white_check(sc, "function")?;
        self.bst_left_brace(sc, "function")?;
        self.eat_bst_white_check(sc, "function")?;
        self.bst_identifier_scan(sc, "function")?;
        let wiz = self.insert_new_fn(sc, FnKind::Wiz(Rc::from(Vec::new())))?;
        if &*self.fns[wiz as usize].name == b"default.type" {
            self.b_default = wiz;
        }
        self.eat_bst_white_check(sc, "function")?;
        self.bst_right_brace(sc, "function")?;
        self.eat_bst_white_check(sc, "function")?;
        self.bst_left_brace(sc, "function")?;
        self.scan_fn_def(sc, wiz)
    }

    /// `skip_token_print`: finish a bad function token.
    fn skip_token(&mut self, sc: &mut Scanner) {
        self.log.print("-");
        self.bst_ln_num_print();
        self.log.mark_error();
        sc.scan2_white(b'}', b'%');
    }

    fn skip_unknown_or_recursive(&mut self, sc: &mut Scanner, id: Option<FnId>, wiz: FnId) -> Option<FnId> {
        match id {
            None => {
                let tok = sc.token().to_vec();
                self.log.print(tok);
                self.log.print(" is an unknown function");
                self.skip_token(sc);
                None
            }
            Some(id) if id == wiz => {
                let tok = sc.token().to_vec();
                self.log.println("Curse you, wizard, before you recurse me:");
                self.log.print("function ");
                self.log.print(tok);
                self.log.println(" is illegal in its own definition");
                self.skip_token(sc);
                None
            }
            some => some,
        }
    }

    /// True (after complaining) if a literal is followed by junk.
    fn bad_after_literal(&mut self, sc: &mut Scanner) -> bool {
        let c = sc.scan_char();
        if !is_white(c) && sc.p2 < sc.last && c != b'}' && c != b'%' {
            self.log.print([b'"', c]);
            self.log.print("\" can't follow a literal");
            self.skip_token(sc);
            return true;
        }
        false
    }

    /// `scan_fn_def`: read a function body up to its closing brace.
    fn scan_fn_def(&mut self, sc: &mut Scanner, wiz: FnId) -> R {
        self.eat_bst_white_check(sc, "function")?;
        let mut ops: Vec<Op> = Vec::new();
        while sc.scan_char() != b'}' {
            match sc.scan_char() {
                b'#' => {
                    sc.p2 += 1;
                    match sc.scan_integer() {
                        None => {
                            self.log.print("Illegal integer in integer literal");
                            self.skip_token(sc);
                        }
                        Some(v) => {
                            if !self.bad_after_literal(sc) {
                                ops.push(Op::Int(v));
                            }
                        }
                    }
                }
                b'"' => {
                    sc.p2 += 1;
                    if !sc.scan1(b'"') {
                        self.log.print("No `\"' to end string literal");
                        self.skip_token(sc);
                    } else {
                        let s: Rc<[u8]> = Rc::from(sc.token());
                        sc.p2 += 1;
                        if !self.bad_after_literal(sc) {
                            ops.push(Op::Str(s));
                        }
                    }
                }
                b'\'' => {
                    sc.p2 += 1;
                    sc.scan2_white(b'}', b'%');
                    sc.lower_case_token();
                    let id = self.lookup_fn(sc.token());
                    if let Some(id) = self.skip_unknown_or_recursive(sc, id, wiz) {
                        ops.push(Op::Quote(id));
                    }
                }
                b'{' => {
                    let name = format!("'{}", self.impl_fn_num);
                    self.impl_fn_num += 1;
                    let id = self.define(name.as_bytes(), FnKind::Wiz(Rc::from(Vec::new())));
                    ops.push(Op::Quote(id));
                    sc.p2 += 1;
                    self.scan_fn_def(sc, id)?;
                }
                _ => {
                    sc.scan2_white(b'}', b'%');
                    sc.lower_case_token();
                    let id = self.lookup_fn(sc.token());
                    if let Some(id) = self.skip_unknown_or_recursive(sc, id, wiz) {
                        ops.push(Op::Call(id));
                    }
                }
            }
            self.eat_bst_white_check(sc, "function")?;
        }
        self.fns[wiz as usize].kind = FnKind::Wiz(Rc::from(ops));
        sc.p2 += 1;
        Ok(())
    }

    fn bst_macro(&mut self, sc: &mut Scanner) -> R {
        if self.read_seen {
            return Err(self.bst_err(sc, "Illegal, macro command after read command"));
        }
        self.eat_bst_white_check(sc, "macro")?;
        self.bst_left_brace(sc, "macro")?;
        self.eat_bst_white_check(sc, "macro")?;
        self.bst_identifier_scan(sc, "macro")?;
        sc.lower_case_token();
        let name: Box<[u8]> = sc.token().into();
        if self.macros.contains_key(&name) {
            let tok = sc.token().to_vec();
            self.log.print(tok);
            return Err(self.bst_err(sc, " is already defined as a macro"));
        }
        // the macro's own name is its value until the definition is read
        self.macros.insert(name.clone(), Rc::from(&name[..]));
        self.eat_bst_white_check(sc, "macro")?;
        self.bst_right_brace(sc, "macro")?;
        self.eat_bst_white_check(sc, "macro")?;
        self.bst_left_brace(sc, "macro")?;
        self.eat_bst_white_check(sc, "macro")?;
        if sc.scan_char() != b'"' {
            return Err(self.bst_err(sc, "A macro definition must be \"-delimited"));
        }
        sc.p2 += 1;
        if !sc.scan1(b'"') {
            return Err(self.bst_err(sc, "There's no `\"' to end macro definition"));
        }
        self.macros.insert(name, Rc::from(sc.token()));
        sc.p2 += 1;
        self.eat_bst_white_check(sc, "macro")?;
        self.bst_right_brace(sc, "macro")
    }

    fn bst_read(&mut self, sc: &mut Scanner) -> R {
        if self.read_seen {
            return Err(self.bst_err(sc, "Illegal, another read command"));
        }
        self.read_seen = true;
        if !self.entry_seen {
            return Err(self.bst_err(sc, "Illegal, read command before entry command"));
        }
        self.read_database()?;
        Ok(())
    }
}
