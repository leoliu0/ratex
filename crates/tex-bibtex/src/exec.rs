//! Executing the style file (bibtex.web parts 13 and 14): the literal
//! stack, `EXECUTE`/`ITERATE`/`REVERSE`/`SORT`, `.bbl` output and the
//! built-in functions. All strings are byte strings, as in BibTeX.

use std::rc::Rc;

use crate::engine::{
    Builtin, Engine, EntryType, Fatal, FnId, FnKind, Lit, Op, Str, ENT_STR_SIZE, GLOB_STR_SIZE,
    MAX_PRINT_LINE, MIN_PRINT_LINE,
};
use crate::input::{is_alpha, is_white, lex_class, SEP, WHITE};
use crate::text;

type R = Result<(), Fatal>;

#[derive(Clone, Copy)]
enum Want {
    Int,
    Str,
    Fn,
}

impl Engine {
    // ------------------------------------------------------------------
    // messages
    // ------------------------------------------------------------------

    fn for_entry_newline(&mut self) {
        if self.mess_with_entries {
            let cite = self.cite_list[self.cite_ptr].clone();
            self.log.print(" for entry ");
            self.log.print(&cite[..]);
        }
        self.log.print("\n");
    }

    /// `bst_ex_warn_print` (an error).
    fn bst_ex_warn_print(&mut self) {
        self.for_entry_newline();
        self.log.print("while executing-");
        self.bst_ln_num_print();
        self.log.mark_error();
    }

    /// `bst_ex_warn`.
    fn bst_ex_warn(&mut self, msg: impl AsRef<[u8]>) {
        self.log.print(msg);
        self.bst_ex_warn_print();
    }

    /// `bst_mild_ex_warn_print` (a warning).
    fn bst_mild_ex_warn_print(&mut self) {
        self.for_entry_newline();
        self.log.print("while executing");
        self.bst_ln_num_print();
        self.log.mark_warning();
    }

    fn cant_mess_with_entries(&mut self) {
        self.bst_ex_warn("You can't mess with entries here");
    }

    /// `braces_unbalanced_complaint`.
    fn braces_unbalanced(&mut self, s: &[u8]) {
        self.log.print("Warning--\"");
        self.log.print(s);
        self.log.print("\" isn't a brace-balanced string");
        self.bst_mild_ex_warn_print();
    }

    /// `decr_brace_level`.
    fn decr_brace_level(&mut self, s: &[u8]) {
        if self.brace_level == 0 {
            self.braces_unbalanced(s);
        } else {
            self.brace_level -= 1;
        }
    }

    /// `check_brace_level`.
    fn check_brace_level(&mut self, s: &[u8]) {
        if self.brace_level > 0 {
            self.braces_unbalanced(s);
        }
    }

    /// `print_stk_lit`.
    fn print_stk_lit(&mut self, lit: &Lit) {
        match lit {
            Lit::Int(i) => self.log.print(format!("{i} is an integer literal")),
            Lit::Str(s) => {
                self.log.print("\"");
                self.log.print(&s[..]);
                self.log.print("\" is a string literal");
            }
            Lit::Fn(f) => {
                let name = self.fns[*f as usize].name.clone();
                self.log.print("`");
                self.log.print(&name[..]);
                self.log.print("' is a function literal");
            }
            Lit::Missing(f) => {
                let name = self.fns[*f as usize].name.clone();
                self.log.print("`");
                self.log.print(&name[..]);
                self.log.print("' is a missing field");
            }
            Lit::Empty => {}
        }
    }

    /// `print_lit`.
    fn print_lit(&mut self, lit: &Lit) {
        match lit {
            Lit::Int(i) => self.log.println(i.to_string()),
            Lit::Str(s) => self.log.println(&s[..]),
            Lit::Fn(f) | Lit::Missing(f) => {
                let name = self.fns[*f as usize].name.clone();
                self.log.println(&name[..]);
            }
            Lit::Empty => {}
        }
    }

    /// `print_wrong_stk_lit`.
    fn wrong_lit(&mut self, lit: &Lit, want: Want) {
        if matches!(lit, Lit::Empty) {
            return;
        }
        self.print_stk_lit(lit);
        self.log.print(match want {
            Want::Int => ", not an integer,",
            Want::Str => ", not a string,",
            Want::Fn => ", not a function,",
        });
        self.bst_ex_warn_print();
    }

    // ------------------------------------------------------------------
    // the literal stack
    // ------------------------------------------------------------------

    #[inline]
    fn push(&mut self, l: Lit) {
        self.stack.push(l);
    }

    #[inline]
    fn push_int(&mut self, i: i32) {
        self.stack.push(Lit::Int(i));
    }

    #[inline]
    fn push_str(&mut self, s: Str) {
        self.stack.push(Lit::Str(s));
    }

    fn push_bytes(&mut self, b: &[u8]) {
        self.stack.push(Lit::Str(Rc::from(b)));
    }

    fn push_null(&mut self) {
        let s = self.null_str.clone();
        self.stack.push(Lit::Str(s));
    }

    fn pop(&mut self) -> Lit {
        match self.stack.pop() {
            Some(l) => l,
            None => {
                self.bst_ex_warn("You can't pop an empty literal stack");
                Lit::Empty
            }
        }
    }

    fn pop_top_and_print(&mut self) {
        match self.pop() {
            Lit::Empty => self.log.println("Empty literal"),
            l => self.print_lit(&l),
        }
    }

    fn pop_whole_stack(&mut self) {
        while !self.stack.is_empty() {
            self.pop_top_and_print();
        }
    }

    fn check_command_execution(&mut self) {
        if !self.stack.is_empty() {
            self.log.println(format!("ptr={}, stack=", self.stack.len()));
            self.pop_whole_stack();
            self.bst_ex_warn("---the literal stack isn't empty");
        }
    }

    // ------------------------------------------------------------------
    // commands
    // ------------------------------------------------------------------

    pub(crate) fn cmd_execute(&mut self, f: FnId) -> R {
        self.stack.clear();
        self.mess_with_entries = false;
        self.execute_fn(f)?;
        self.check_command_execution();
        Ok(())
    }

    pub(crate) fn cmd_iterate(&mut self, f: FnId, reverse: bool) -> R {
        self.stack.clear();
        self.mess_with_entries = true;
        for k in 0..self.num_cites {
            let k = if reverse { self.num_cites - 1 - k } else { k };
            self.cite_ptr = self.sorted_cites[k];
            self.execute_fn(f)?;
            self.check_command_execution();
        }
        Ok(())
    }

    /// `SORT`: by `sort.key$`, ties broken by cite-list position (the
    /// order `less_than` imposes on BibTeX's quicksort).
    pub(crate) fn cmd_sort(&mut self) {
        let ns = self.num_ent_strs;
        let sk = self.sort_key_num;
        let strs = &self.entry_strs;
        self.sorted_cites
            .sort_unstable_by(|&a, &b| strs[a * ns + sk].cmp(&strs[b * ns + sk]).then(a.cmp(&b)));
    }

    // ------------------------------------------------------------------
    // execution
    // ------------------------------------------------------------------

    pub(crate) fn execute_fn(&mut self, f: FnId) -> R {
        match &self.fns[f as usize].kind {
            FnKind::Builtin(b) => {
                let b = *b;
                self.builtin(b)?;
            }
            FnKind::Wiz(body) => {
                let body = body.clone();
                for op in body.iter() {
                    match op {
                        Op::Call(g) => self.execute_fn(*g)?,
                        Op::Quote(g) => self.push(Lit::Fn(*g)),
                        Op::Int(i) => self.push_int(*i),
                        Op::Str(s) => self.push_str(s.clone()),
                    }
                }
            }
            FnKind::Field(i) => {
                let i = *i;
                if !self.mess_with_entries {
                    self.cant_mess_with_entries();
                } else {
                    match &self.field_info[self.cite_ptr * self.num_fields + i] {
                        Some(v) => {
                            let v = v.clone();
                            self.push_str(v);
                        }
                        None => self.push(Lit::Missing(f)),
                    }
                }
            }
            FnKind::IntEntry(i) => {
                let i = *i;
                if !self.mess_with_entries {
                    self.cant_mess_with_entries();
                } else {
                    let v = self.entry_ints[self.cite_ptr * self.num_ent_ints + i];
                    self.push_int(v);
                }
            }
            FnKind::StrEntry(i) => {
                let i = *i;
                if !self.mess_with_entries {
                    self.cant_mess_with_entries();
                } else {
                    let v = self.entry_strs[self.cite_ptr * self.num_ent_strs + i].clone();
                    self.push_str(v);
                }
            }
            FnKind::IntGlobal(i) => {
                let v = self.glb_ints[*i];
                self.push_int(v);
            }
            FnKind::StrGlobal(i) => {
                let v = self.glb_strs[*i].clone();
                self.push_str(v);
            }
        }
        Ok(())
    }

    fn pop_two_ints(&mut self) -> Option<(i32, i32)> {
        let a = self.pop();
        let b = self.pop();
        match (&a, &b) {
            (Lit::Int(x), Lit::Int(y)) => Some((*y, *x)),
            (Lit::Int(_), _) => {
                self.wrong_lit(&b, Want::Int);
                None
            }
            _ => {
                self.wrong_lit(&a, Want::Int);
                None
            }
        }
    }

    fn pop_str(&mut self) -> Option<Str> {
        match self.pop() {
            Lit::Str(s) => Some(s),
            other => {
                self.wrong_lit(&other, Want::Str);
                None
            }
        }
    }

    fn builtin(&mut self, b: Builtin) -> R {
        match b {
            Builtin::Eq => self.x_equals(),
            Builtin::Gt => match self.pop_two_ints() {
                Some((second, first)) => self.push_int((second > first) as i32),
                None => self.push_int(0),
            },
            Builtin::Lt => match self.pop_two_ints() {
                Some((second, first)) => self.push_int((second < first) as i32),
                None => self.push_int(0),
            },
            Builtin::Plus => match self.pop_two_ints() {
                Some((second, first)) => self.push_int(second.wrapping_add(first)),
                None => self.push_int(0),
            },
            Builtin::Minus => match self.pop_two_ints() {
                Some((second, first)) => self.push_int(second.wrapping_sub(first)),
                None => self.push_int(0),
            },
            Builtin::Concat => {
                let a = self.pop();
                let c = self.pop();
                match (&a, &c) {
                    (Lit::Str(a), Lit::Str(c)) => {
                        if a.is_empty() {
                            let c = c.clone();
                            self.push_str(c);
                        } else if c.is_empty() {
                            let a = a.clone();
                            self.push_str(a);
                        } else {
                            let mut s = Vec::with_capacity(a.len() + c.len());
                            s.extend_from_slice(c);
                            s.extend_from_slice(a);
                            self.push_str(Rc::from(s));
                        }
                    }
                    (Lit::Str(_), _) => {
                        self.wrong_lit(&c, Want::Str);
                        self.push_null();
                    }
                    _ => {
                        self.wrong_lit(&a, Want::Str);
                        self.push_null();
                    }
                }
            }
            Builtin::Gets => self.x_gets(),
            Builtin::AddPeriod => match self.pop_str() {
                Some(s) => match text::add_period(&s) {
                    Some(t) => self.push_bytes(&t),
                    None => self.push_str(s),
                },
                None => self.push_null(),
            },
            Builtin::CallType => {
                if !self.mess_with_entries {
                    self.cant_mess_with_entries();
                } else {
                    match self.type_list[self.cite_ptr] {
                        EntryType::Undefined => self.execute_fn(self.b_default)?,
                        EntryType::Empty => {}
                        EntryType::Type(f) => self.execute_fn(f)?,
                    }
                }
            }
            Builtin::ChangeCase => self.x_change_case(),
            Builtin::ChrToInt => match self.pop_str() {
                Some(s) if s.len() == 1 => self.push_int(s[0] as i32),
                Some(s) => {
                    self.log.print("\"");
                    self.log.print(&s[..]);
                    self.bst_ex_warn("\" isn't a single character");
                    self.push_int(0);
                }
                None => self.push_int(0),
            },
            Builtin::Cite => {
                if !self.mess_with_entries {
                    self.cant_mess_with_entries();
                } else {
                    let c = self.cite_list[self.cite_ptr].clone();
                    self.push_str(c);
                }
            }
            Builtin::Duplicate => {
                let l = self.pop();
                self.push(l.clone());
                self.push(l);
            }
            Builtin::Empty => {
                let v = match self.pop() {
                    Lit::Str(s) => s.iter().all(|&c| is_white(c)) as i32,
                    Lit::Missing(_) => 1,
                    Lit::Empty => 0,
                    other => {
                        self.print_stk_lit(&other);
                        self.bst_ex_warn(", not a string or missing field,");
                        0
                    }
                };
                self.push_int(v);
            }
            Builtin::FormatName => self.x_format_name(),
            Builtin::If => {
                let f1 = self.pop();
                let f2 = self.pop();
                let c = self.pop();
                match (&f1, &f2, &c) {
                    (Lit::Fn(f1), Lit::Fn(f2), Lit::Int(c)) => {
                        let f = if *c > 0 { *f2 } else { *f1 };
                        self.execute_fn(f)?;
                    }
                    (Lit::Fn(_), Lit::Fn(_), _) => self.wrong_lit(&c, Want::Int),
                    (Lit::Fn(_), _, _) => self.wrong_lit(&f2, Want::Fn),
                    _ => self.wrong_lit(&f1, Want::Fn),
                }
            }
            Builtin::IntToChr => match self.pop() {
                Lit::Int(i) if (0..=127).contains(&i) => self.push_bytes(&[i as u8]),
                Lit::Int(i) => {
                    self.bst_ex_warn(format!("{i} isn't valid ASCII"));
                    self.push_null();
                }
                other => {
                    self.wrong_lit(&other, Want::Int);
                    self.push_null();
                }
            },
            Builtin::IntToStr => match self.pop() {
                Lit::Int(i) => self.push_bytes(i.to_string().as_bytes()),
                other => {
                    self.wrong_lit(&other, Want::Int);
                    self.push_null();
                }
            },
            Builtin::Missing => {
                let l = self.pop();
                if !self.mess_with_entries {
                    self.cant_mess_with_entries();
                } else {
                    match l {
                        Lit::Missing(_) => self.push_int(1),
                        Lit::Str(_) => self.push_int(0),
                        Lit::Empty => self.push_int(0),
                        other => {
                            self.print_stk_lit(&other);
                            self.bst_ex_warn(", not a string or missing field,");
                            self.push_int(0);
                        }
                    }
                }
            }
            Builtin::Newline => self.output_bbl_line(),
            Builtin::NumNames => match self.pop_str() {
                Some(s) => {
                    let n = self.num_names(&s);
                    self.push_int(n);
                }
                None => self.push_int(0),
            },
            Builtin::Pop => {
                self.pop();
            }
            Builtin::Preamble => {
                let s: Vec<u8> = self.preambles.iter().flat_map(|p| p.iter().copied()).collect();
                self.push_bytes(&s);
            }
            Builtin::Purify => match self.pop_str() {
                Some(s) => self.push_bytes(&text::purify(&s)),
                None => self.push_null(),
            },
            Builtin::Quote => self.push_bytes(b"\""),
            Builtin::Skip => {}
            Builtin::Stack => self.pop_whole_stack(),
            Builtin::Substring => {
                let len = self.pop();
                let start = self.pop();
                let s = self.pop();
                match (&len, &start, &s) {
                    (Lit::Int(len), Lit::Int(start), Lit::Str(s)) => match text::substring(s, *start, *len) {
                        Some(sub) => self.push_bytes(sub),
                        None => {
                            let s = s.clone();
                            self.push_str(s);
                        }
                    },
                    (Lit::Int(_), Lit::Int(_), _) => {
                        self.wrong_lit(&s, Want::Str);
                        self.push_null();
                    }
                    (Lit::Int(_), _, _) => {
                        self.wrong_lit(&start, Want::Int);
                        self.push_null();
                    }
                    _ => {
                        self.wrong_lit(&len, Want::Int);
                        self.push_null();
                    }
                }
            }
            Builtin::Swap => {
                let a = self.pop();
                let c = self.pop();
                self.push(a);
                self.push(c);
            }
            Builtin::TextLength => match self.pop_str() {
                Some(s) => self.push_int(text::text_length(&s) as i32),
                None => self.push_null(),
            },
            Builtin::TextPrefix => {
                let n = self.pop();
                let s = self.pop();
                match (&n, &s) {
                    (Lit::Int(n), Lit::Str(s)) => {
                        if *n <= 0 {
                            self.push_null();
                        } else {
                            self.push_bytes(&text::text_prefix(s, *n as usize));
                        }
                    }
                    (Lit::Int(_), _) => {
                        self.wrong_lit(&s, Want::Str);
                        self.push_null();
                    }
                    _ => {
                        self.wrong_lit(&n, Want::Int);
                        self.push_null();
                    }
                }
            }
            Builtin::Top => self.pop_top_and_print(),
            Builtin::Type => {
                if !self.mess_with_entries {
                    self.cant_mess_with_entries();
                } else {
                    match self.type_list[self.cite_ptr] {
                        EntryType::Type(f) => {
                            let name = self.fns[f as usize].name.clone();
                            self.push_str(name);
                        }
                        _ => self.push_null(),
                    }
                }
            }
            Builtin::Warning => {
                if let Some(s) = self.pop_str() {
                    self.log.print("Warning--");
                    self.log.println(&s[..]);
                    self.log.mark_warning();
                }
            }
            Builtin::While => {
                let body = self.pop();
                let test = self.pop();
                match (&body, &test) {
                    (Lit::Fn(body), Lit::Fn(test)) => loop {
                        self.execute_fn(*test)?;
                        match self.pop() {
                            Lit::Int(c) if c > 0 => self.execute_fn(*body)?,
                            Lit::Int(_) => break,
                            other => {
                                self.wrong_lit(&other, Want::Int);
                                break;
                            }
                        }
                    },
                    (Lit::Fn(_), _) => self.wrong_lit(&test, Want::Fn),
                    _ => self.wrong_lit(&body, Want::Fn),
                }
            }
            Builtin::Width => match self.pop_str() {
                Some(s) => {
                    let w = self.width(&s);
                    self.push_int(w);
                }
                None => self.push_int(0),
            },
            Builtin::Write => {
                if let Some(s) = self.pop_str() {
                    self.add_out_pool(&s);
                }
            }
        }
        Ok(())
    }

    fn x_equals(&mut self) {
        let a = self.pop();
        let b = self.pop();
        let v = match (&a, &b) {
            (Lit::Int(x), Lit::Int(y)) => x == y,
            (Lit::Str(x), Lit::Str(y)) => x == y,
            _ if std::mem::discriminant(&a) != std::mem::discriminant(&b) => {
                if !matches!(a, Lit::Empty) && !matches!(b, Lit::Empty) {
                    self.print_stk_lit(&a);
                    self.log.print(", ");
                    self.print_stk_lit(&b);
                    self.log.print("\n");
                    self.bst_ex_warn("---they aren't the same literal types");
                }
                false
            }
            _ => {
                if !matches!(a, Lit::Empty) {
                    self.print_stk_lit(&a);
                    self.bst_ex_warn(", not an integer or a string,");
                }
                false
            }
        };
        self.push_int(v as i32);
    }

    /// `:=`.
    fn x_gets(&mut self) {
        let target = self.pop();
        let value = self.pop();
        let Lit::Fn(f) = target else {
            self.wrong_lit(&target, Want::Fn);
            return;
        };
        let kind = self.fns[f as usize].kind.clone();
        if !self.mess_with_entries && matches!(kind, FnKind::IntEntry(_) | FnKind::StrEntry(_)) {
            self.cant_mess_with_entries();
            return;
        }
        match kind {
            FnKind::IntEntry(i) => match value {
                Lit::Int(v) => self.entry_ints[self.cite_ptr * self.num_ent_ints + i] = v,
                other => self.wrong_lit(&other, Want::Int),
            },
            FnKind::StrEntry(i) => match value {
                Lit::Str(s) => {
                    let s = self.truncate_assigned(s, ENT_STR_SIZE, "entry");
                    self.entry_strs[self.cite_ptr * self.num_ent_strs + i] = s;
                }
                other => self.wrong_lit(&other, Want::Str),
            },
            FnKind::IntGlobal(i) => match value {
                Lit::Int(v) => self.glb_ints[i] = v,
                other => self.wrong_lit(&other, Want::Int),
            },
            FnKind::StrGlobal(i) => match value {
                Lit::Str(s) => {
                    let s = self.truncate_assigned(s, GLOB_STR_SIZE, "global");
                    self.glb_strs[i] = s;
                }
                other => self.wrong_lit(&other, Want::Str),
            },
            _ => {
                self.log.print(format!("You can't assign to type {}", self.fn_class_name(f)));
                self.bst_ex_warn(", a nonvariable function class");
            }
        }
    }

    /// `bst_string_size_exceeded`: entry and global string variables hold
    /// at most `ent_str_size`/`glob_str_size` bytes.
    fn truncate_assigned(&mut self, s: Str, cap: usize, what: &str) -> Str {
        if s.len() <= cap {
            return s;
        }
        self.log.print(format!("Warning--you've exceeded {cap}, the {what}-string-size,"));
        self.bst_mild_ex_warn_print();
        self.log.println("*Please notify the bibstyle designer*");
        Rc::from(&s[..cap])
    }

    // ------------------------------------------------------------------
    // .bbl output
    // ------------------------------------------------------------------

    /// `output_bbl_line`.
    fn output_bbl_line(&mut self) {
        if !self.out_buf.is_empty() {
            let mut n = self.out_buf.len();
            while n > 0 && is_white(self.out_buf[n - 1]) {
                n -= 1;
            }
            if n == 0 {
                self.out_buf.clear();
                return;
            }
            self.bbl.extend_from_slice(&self.out_buf[..n]);
        }
        self.bbl.push(b'\n');
        self.out_buf.clear();
    }

    /// `add_out_pool`: append and break lines longer than
    /// `max_print_line` at white space, indenting continuations by two.
    fn add_out_pool(&mut self, s: &[u8]) {
        self.out_buf.extend_from_slice(s);
        while self.out_buf.len() > MAX_PRINT_LINE {
            let end = self.out_buf.len();
            let mut p = MAX_PRINT_LINE;
            while !is_white(self.out_buf[p]) && p >= MIN_PRINT_LINE {
                p -= 1;
            }
            if p == MIN_PRINT_LINE - 1 {
                // <Break that unbreakably long line>
                p = MAX_PRINT_LINE + 1;
                while p < end && !is_white(self.out_buf[p]) {
                    p += 1;
                }
                if p == end {
                    return;
                }
                while p + 1 < end && is_white(self.out_buf[p + 1]) {
                    p += 1;
                }
            }
            let rest = self.out_buf.split_off(p + 1);
            self.out_buf.truncate(p);
            self.output_bbl_line();
            self.out_buf.extend_from_slice(b"  ");
            self.out_buf.extend_from_slice(&rest);
        }
    }

    // ------------------------------------------------------------------
    // change.case$ and width$
    // ------------------------------------------------------------------

    fn x_change_case(&mut self) {
        let spec = self.pop();
        let s = self.pop();
        let (spec, s) = match (spec, s) {
            (Lit::Str(spec), Lit::Str(s)) => (spec, s),
            (Lit::Str(_), s) => {
                self.wrong_lit(&s, Want::Str);
                return self.push_null();
            }
            (spec, _) => {
                self.wrong_lit(&spec, Want::Str);
                return self.push_null();
            }
        };
        let conv = match (spec.len(), spec.first()) {
            (1, Some(b't' | b'T')) => text::Case::Title,
            (1, Some(b'l' | b'L')) => text::Case::Lower,
            (1, Some(b'u' | b'U')) => text::Case::Upper,
            _ => {
                self.log.print(&spec[..]);
                self.bst_ex_warn(" is an illegal case-conversion string");
                text::Case::Bad
            }
        };
        let mut buf = s.to_vec();
        let mut prev_colon = false;
        self.brace_level = 0;
        let mut i = 0;
        while i < buf.len() {
            let c = buf[i];
            if c == b'{' {
                self.brace_level += 1;
                let special = self.brace_level == 1
                    && i + 4 <= buf.len()
                    && buf[i + 1] == b'\\'
                    && !(conv == text::Case::Title
                        && (i == 0 || (prev_colon && is_white(buf[i - 1]))));
                if special {
                    i = text::convert_special(&mut buf, i, conv, &mut self.brace_level);
                }
                prev_colon = false;
            } else if c == b'}' {
                self.decr_brace_level(&s);
                prev_colon = false;
            } else if self.brace_level == 0 {
                match conv {
                    text::Case::Title => {
                        if !(i == 0 || (prev_colon && is_white(buf[i - 1]))) {
                            buf[i] = buf[i].to_ascii_lowercase();
                        }
                        if buf[i] == b':' {
                            prev_colon = true;
                        } else if !is_white(buf[i]) {
                            prev_colon = false;
                        }
                    }
                    text::Case::Lower => buf[i] = buf[i].to_ascii_lowercase(),
                    text::Case::Upper => buf[i] = buf[i].to_ascii_uppercase(),
                    text::Case::Bad => {}
                }
            }
            i += 1;
        }
        self.check_brace_level(&s);
        self.push_bytes(&buf);
    }

    fn width(&mut self, s: &[u8]) -> i32 {
        let mut w: i32 = 0;
        self.brace_level = 0;
        let mut i = 0;
        let n = s.len();
        while i < n {
            let c = s[i];
            if c == b'{' {
                self.brace_level += 1;
                if self.brace_level == 1 && i + 1 < n && s[i + 1] == b'\\' {
                    // <Determine the width of this special character>
                    i += 1;
                    while i < n && self.brace_level > 0 {
                        i += 1;
                        let xs = i;
                        while i < n && is_alpha(s[i]) {
                            i += 1;
                        }
                        if i < n && i == xs {
                            i += 1;
                        } else if let Some(k) = text::control_seq(&s[xs..i]) {
                            w += text::special_width(k, s[xs]);
                        }
                        while i < n && is_white(s[i]) {
                            i += 1;
                        }
                        while i < n && self.brace_level > 0 && s[i] != b'\\' {
                            match s[i] {
                                b'}' => self.brace_level -= 1,
                                b'{' => self.brace_level += 1,
                                c => w += text::char_width(c),
                            }
                            i += 1;
                        }
                    }
                    i -= 1;
                } else {
                    w += text::char_width(b'{');
                }
            } else if c == b'}' {
                self.decr_brace_level(s);
                w += text::char_width(b'}');
            } else {
                w += text::char_width(c);
            }
            i += 1;
        }
        self.check_brace_level(s);
        w
    }

    // ------------------------------------------------------------------
    // num.names$ and format.name$
    // ------------------------------------------------------------------

    /// `name_scan_for_and`: advance `*p` past the next brace-level-0
    /// " and " (or to the end).
    fn name_scan_for_and(&mut self, b: &[u8], p: &mut usize) {
        self.brace_level = 0;
        let mut preceding_white = false;
        let n = b.len();
        while *p < n {
            match b[*p] {
                b'a' | b'A' => {
                    *p += 1;
                    if preceding_white
                        && *p + 3 <= n
                        && matches!(b[*p], b'n' | b'N')
                        && matches!(b[*p + 1], b'd' | b'D')
                        && is_white(b[*p + 2])
                    {
                        *p += 2;
                        break;
                    }
                    preceding_white = false;
                }
                b'{' => {
                    self.brace_level += 1;
                    *p += 1;
                    while self.brace_level > 0 && *p < n {
                        match b[*p] {
                            b'}' => self.brace_level -= 1,
                            b'{' => self.brace_level += 1,
                            _ => {}
                        }
                        *p += 1;
                    }
                    preceding_white = false;
                }
                b'}' => {
                    self.decr_brace_level(b);
                    *p += 1;
                    preceding_white = false;
                }
                c => {
                    *p += 1;
                    preceding_white = is_white(c);
                }
            }
        }
        self.check_brace_level(b);
    }

    fn num_names(&mut self, s: &[u8]) -> i32 {
        let mut p = 0;
        let mut count = 0;
        while p < s.len() {
            self.name_scan_for_and(s, &mut p);
            count += 1;
        }
        count
    }

    fn x_format_name(&mut self) {
        let fmt = self.pop();
        let which = self.pop();
        let list = self.pop();
        let (fmt, which, list) = match (fmt, which, list) {
            (Lit::Str(f), Lit::Int(w), Lit::Str(l)) => (f, w, l),
            (Lit::Str(_), Lit::Int(_), l) => {
                self.wrong_lit(&l, Want::Str);
                return self.push_null();
            }
            (Lit::Str(_), w, _) => {
                self.wrong_lit(&w, Want::Int);
                return self.push_null();
            }
            (f, _, _) => {
                self.wrong_lit(&f, Want::Str);
                return self.push_null();
            }
        };
        let out = self.format_name(&fmt, which, &list);
        self.push_bytes(&out);
    }

    fn format_name(&mut self, fmt: &[u8], which: i32, list: &[u8]) -> Vec<u8> {
        let len = list.len();
        // <Isolate the desired name>
        let mut ptr = 0usize;
        let mut xptr = 0usize;
        let mut num_names = 0;
        while num_names < which && ptr < len {
            num_names += 1;
            xptr = ptr;
            self.name_scan_for_and(list, &mut ptr);
        }
        if ptr < len {
            ptr -= 4;
        }
        if num_names < which {
            if which == 1 {
                self.log.print("There is no name in \"");
            } else {
                self.log.print(format!("There aren't {which} names in \""));
            }
            self.log.print(list);
            self.bst_ex_warn("\"");
        }
        // <Remove leading and trailing junk, complaining if necessary>
        while ptr > xptr {
            match lex_class(list[ptr - 1]) {
                WHITE | SEP => ptr -= 1,
                _ if list[ptr - 1] == b',' => {
                    self.log.print(format!("Name {which} in \""));
                    self.log.print(list);
                    self.log.print("\" has a comma at the end");
                    self.bst_ex_warn_print();
                    ptr -= 1;
                }
                _ => break,
            }
        }
        // <Copy name and count commas to determine syntax>
        let mut nb: Vec<u8> = Vec::with_capacity(ptr.saturating_sub(xptr));
        let mut tok: Vec<usize> = Vec::new();
        let mut sep: Vec<u8> = Vec::new();
        let mut num_commas = 0;
        let (mut comma1, mut comma2) = (0usize, 0usize);
        let mut token_starting = true;
        let set_sep = |sep: &mut Vec<u8>, k: usize, c: u8| {
            if sep.len() <= k {
                sep.resize(k + 1, b' ');
            }
            sep[k] = c;
        };
        while xptr < ptr {
            let c = list[xptr];
            match c {
                b',' => {
                    if num_commas == 2 {
                        self.log.print(format!("Too many commas in name {which} of \""));
                        self.log.print(list);
                        self.log.print("\"");
                        self.bst_ex_warn_print();
                    } else {
                        num_commas += 1;
                        if num_commas == 1 {
                            comma1 = tok.len();
                        } else {
                            comma2 = tok.len();
                        }
                        set_sep(&mut sep, tok.len(), b',');
                    }
                    xptr += 1;
                    token_starting = true;
                }
                b'{' => {
                    self.brace_level += 1;
                    if token_starting {
                        tok.push(nb.len());
                    }
                    nb.push(c);
                    xptr += 1;
                    while self.brace_level > 0 && xptr < ptr {
                        match list[xptr] {
                            b'}' => self.brace_level -= 1,
                            b'{' => self.brace_level += 1,
                            _ => {}
                        }
                        nb.push(list[xptr]);
                        xptr += 1;
                    }
                    token_starting = false;
                }
                b'}' => {
                    if token_starting {
                        tok.push(nb.len());
                    }
                    self.log.print(format!("Name {which} of \""));
                    self.log.print(list);
                    self.bst_ex_warn("\" isn't brace balanced");
                    xptr += 1;
                    token_starting = false;
                }
                _ => match lex_class(c) {
                    WHITE => {
                        if !token_starting {
                            set_sep(&mut sep, tok.len(), b' ');
                        }
                        xptr += 1;
                        token_starting = true;
                    }
                    SEP => {
                        if !token_starting {
                            set_sep(&mut sep, tok.len(), c);
                        }
                        xptr += 1;
                        token_starting = true;
                    }
                    _ => {
                        if token_starting {
                            tok.push(nb.len());
                        }
                        nb.push(c);
                        xptr += 1;
                        token_starting = false;
                    }
                },
            }
        }
        let num_tokens = tok.len();
        tok.push(nb.len());
        if sep.len() <= num_tokens {
            sep.resize(num_tokens + 1, b' ');
        }
        let names = Names { buf: nb, tok, sep };
        // <Find the parts of the name>
        let parts = names.parts(num_commas, comma1, comma2);
        self.figure_out_formatted_name(fmt, &names, &parts)
    }

    fn brace_lvl_one_letters_complaint(&mut self, fmt: &[u8]) {
        self.log.print("The format string \"");
        self.log.print(fmt);
        self.bst_ex_warn("\" has an illegal brace-level-1 letter");
    }

    /// `figure_out_the_formatted_name`.
    fn figure_out_formatted_name(&mut self, fmt: &[u8], names: &Names, parts: &Parts) -> Vec<u8> {
        let at = |i: usize| fmt.get(i).copied().unwrap_or(0);
        let end = fmt.len();
        let mut out: Vec<u8> = Vec::new();
        let mut level = 0i32;
        let mut sp = 0usize;
        while sp < end {
            match fmt[sp] {
                b'{' => {
                    level += 1;
                    sp += 1;
                    // <Format this part of the name>
                    let group = sp;
                    let mut alpha_found = false;
                    let mut double_letter = false;
                    let mut end_of_group = false;
                    let mut to_be_written = true;
                    let (mut cur, mut last) = (0isize, 0isize);
                    while !end_of_group && sp < end {
                        let c = fmt[sp];
                        if is_alpha(c) {
                            sp += 1;
                            if alpha_found {
                                self.brace_lvl_one_letters_complaint(fmt);
                                to_be_written = false;
                            } else {
                                let range = match c {
                                    b'f' | b'F' => Some((parts.first_start, parts.first_end)),
                                    b'v' | b'V' => Some((parts.von_start, parts.von_end)),
                                    b'l' | b'L' => Some((parts.von_end, parts.last_end)),
                                    b'j' | b'J' => Some((parts.last_end, parts.jr_end)),
                                    _ => None,
                                };
                                match range {
                                    Some((a, b)) => {
                                        cur = a;
                                        last = b;
                                        if cur == last {
                                            to_be_written = false;
                                        }
                                        if at(sp).to_ascii_lowercase() == c.to_ascii_lowercase() {
                                            double_letter = true;
                                        }
                                    }
                                    None => {
                                        self.brace_lvl_one_letters_complaint(fmt);
                                        to_be_written = false;
                                    }
                                }
                                if double_letter {
                                    sp += 1;
                                }
                            }
                            alpha_found = true;
                        } else if c == b'}' {
                            level -= 1;
                            sp += 1;
                            end_of_group = true;
                        } else if c == b'{' {
                            level += 1;
                            sp += 1;
                            skip_brace_group(fmt, &mut sp, &mut level);
                        } else {
                            sp += 1;
                        }
                    }
                    if end_of_group && to_be_written {
                        self.format_part(fmt, group, names, cur, last, double_letter, &mut out);
                    }
                }
                b'}' => {
                    self.braces_unbalanced(fmt);
                    sp += 1;
                }
                c => {
                    out.push(c);
                    sp += 1;
                }
            }
        }
        if level > 0 {
            self.braces_unbalanced(fmt);
        }
        out
    }

    /// `<Finally format this part of the name>` (second pass).
    #[allow(clippy::too_many_arguments)]
    fn format_part(
        &mut self,
        fmt: &[u8],
        group: usize,
        names: &Names,
        mut cur: isize,
        last: isize,
        double_letter: bool,
        out: &mut Vec<u8>,
    ) {
        let at = |i: usize| fmt.get(i).copied().unwrap_or(0);
        let part_start = out.len();
        let mut sp = group;
        let mut level = 1i32;
        while level > 0 {
            let c = at(sp);
            if is_alpha(c) && level == 1 {
                sp += 1;
                if double_letter {
                    sp += 1;
                }
                let mut use_default = true;
                let (mut x1, mut x2) = (sp, sp);
                if at(sp) == b'{' {
                    use_default = false;
                    level += 1;
                    sp += 1;
                    x1 = sp;
                    skip_brace_group(fmt, &mut sp, &mut level);
                    x2 = sp - 1;
                }
                while cur < last {
                    let t = names.token(cur);
                    if double_letter {
                        out.extend_from_slice(t);
                    } else {
                        abbreviate(t, out);
                    }
                    cur += 1;
                    if cur < last {
                        if use_default {
                            if !double_letter {
                                out.push(b'.');
                            }
                            let s = names.sep_before(cur);
                            if lex_class(s) == SEP {
                                out.push(s);
                            } else if cur == last - 1 || !self.enough_text_chars(out, part_start, 3) {
                                out.push(b'~');
                            } else {
                                out.push(b' ');
                            }
                        } else {
                            out.extend_from_slice(&fmt[x1.min(fmt.len())..x2.min(fmt.len())]);
                        }
                    }
                }
                if !use_default {
                    sp = x2 + 1;
                }
            } else if c == b'}' {
                level -= 1;
                sp += 1;
                if level > 0 {
                    out.push(b'}');
                }
            } else if c == b'{' {
                level += 1;
                sp += 1;
                out.push(b'{');
            } else {
                out.push(c);
                sp += 1;
            }
        }
        // <Handle a discretionary tie>
        if out.last() == Some(&b'~') {
            out.pop();
            if out.last() == Some(&b'~') {
            } else if !self.enough_text_chars(out, part_start, 3) {
                out.push(b'~');
            } else {
                out.push(b' ');
            }
        }
    }

    /// `enough_text_chars`: does `out[from..]` hold `enough` text
    /// characters (a special character counts as one)?
    fn enough_text_chars(&mut self, out: &[u8], from: usize, enough: usize) -> bool {
        let mut num = 0;
        let mut y = from;
        while y < out.len() && num < enough {
            y += 1;
            if out[y - 1] == b'{' {
                self.brace_level += 1;
                if self.brace_level == 1 && y < out.len() && out[y] == b'\\' {
                    y += 1;
                    while y < out.len() && self.brace_level > 0 {
                        match out[y] {
                            b'}' => self.brace_level -= 1,
                            b'{' => self.brace_level += 1,
                            _ => {}
                        }
                        y += 1;
                    }
                }
            } else if out[y - 1] == b'}' {
                self.brace_level -= 1;
            }
            num += 1;
        }
        num >= enough
    }
}

/// `skip_stuff_at_sp_brace_level_greater_than_one`.
fn skip_brace_group(fmt: &[u8], sp: &mut usize, level: &mut i32) {
    while *level > 1 && *sp < fmt.len() {
        match fmt[*sp] {
            b'}' => *level -= 1,
            b'{' => *level += 1,
            _ => {}
        }
        *sp += 1;
    }
}

/// `<Finally output an abbreviated token>`.
fn abbreviate(t: &[u8], out: &mut Vec<u8>) {
    let mut p = 0;
    while p < t.len() {
        if is_alpha(t[p]) {
            out.push(t[p]);
            return;
        }
        if t[p] == b'{' && p + 1 < t.len() && t[p + 1] == b'\\' {
            out.extend_from_slice(b"{\\");
            p += 2;
            let mut lvl = 1;
            while p < t.len() && lvl > 0 {
                match t[p] {
                    b'}' => lvl -= 1,
                    b'{' => lvl += 1,
                    _ => {}
                }
                out.push(t[p]);
                p += 1;
            }
            return;
        }
        p += 1;
    }
}

/// A name split into tokens (`name_buf`, `name_tok`, `name_sep_char`).
struct Names {
    buf: Vec<u8>,
    /// token start offsets plus an end marker
    tok: Vec<usize>,
    /// `sep[k]`: the character separating token k from token k-1
    sep: Vec<u8>,
}

struct Parts {
    first_start: isize,
    first_end: isize,
    von_start: isize,
    von_end: isize,
    last_end: isize,
    jr_end: isize,
}

impl Names {
    fn num_tokens(&self) -> isize {
        self.tok.len() as isize - 1
    }

    /// Token `k`; out-of-range indexes (which bibtex.web reads as stale
    /// memory for names like `, X`) yield an empty token.
    fn token(&self, k: isize) -> &[u8] {
        if k < 0 || k >= self.num_tokens() {
            return &[];
        }
        let k = k as usize;
        &self.buf[self.tok[k]..self.tok[k + 1]]
    }

    fn sep_before(&self, k: isize) -> u8 {
        if k < 0 {
            return b' ';
        }
        self.sep.get(k as usize).copied().unwrap_or(b' ')
    }

    /// `von_token_found` for token `k`.
    fn von_token(&self, k: isize) -> bool {
        let t = self.token(k);
        let mut p = 0;
        let end = t.len();
        let mut lvl = 0;
        while p < end {
            let c = t[p];
            if c.is_ascii_uppercase() {
                return false;
            } else if c.is_ascii_lowercase() {
                return true;
            } else if c == b'{' {
                lvl += 1;
                p += 1;
                if p + 2 < end && t[p] == b'\\' {
                    // <Check the special character (and return)>
                    p += 1;
                    let xs = p;
                    while p < end && is_alpha(t[p]) {
                        p += 1;
                    }
                    if let Some(k) = text::control_seq(&t[xs..p]) {
                        return k.is_lower_case();
                    }
                    while p < end && lvl > 0 {
                        let c = t[p];
                        if c.is_ascii_uppercase() {
                            return false;
                        } else if c.is_ascii_lowercase() {
                            return true;
                        } else if c == b'}' {
                            lvl -= 1;
                        } else if c == b'{' {
                            lvl += 1;
                        }
                        p += 1;
                    }
                    return false;
                }
                while lvl > 0 && p < end {
                    match t[p] {
                        b'}' => lvl -= 1,
                        b'{' => lvl += 1,
                        _ => {}
                    }
                    p += 1;
                }
            } else {
                p += 1;
            }
        }
        false
    }

    /// `von_name_ends_and_last_name_starts_stuff`.
    fn von_end(&self, von_start: isize, last_end: isize) -> isize {
        let mut von_end = last_end - 1;
        while von_end > von_start {
            if self.von_token(von_end - 1) {
                return von_end;
            }
            von_end -= 1;
        }
        von_end
    }

    /// `<Find the parts of the name>`.
    fn parts(&self, num_commas: u32, comma1: usize, comma2: usize) -> Parts {
        let n = self.num_tokens();
        match num_commas {
            0 => {
                let last_end = n;
                let mut von_start = 0;
                let mut von_end = None;
                while von_start < last_end - 1 {
                    if self.von_token(von_start) {
                        von_end = Some(self.von_end(von_start, last_end));
                        break;
                    }
                    von_start += 1;
                }
                let von_end = match von_end {
                    Some(v) => v,
                    None => {
                        while von_start > 0 {
                            let s = self.sep_before(von_start);
                            if lex_class(s) != SEP || s == b'~' {
                                break;
                            }
                            von_start -= 1;
                        }
                        von_start
                    }
                };
                Parts {
                    first_start: 0,
                    first_end: von_start,
                    von_start,
                    von_end,
                    last_end,
                    jr_end: last_end,
                }
            }
            _ => {
                let last_end = comma1 as isize;
                let jr_end = if num_commas == 1 { last_end } else { comma2 as isize };
                Parts {
                    first_start: jr_end,
                    first_end: n,
                    von_start: 0,
                    von_end: self.von_end(0, last_end),
                    last_end,
                    jr_end,
                }
            }
        }
    }
}
