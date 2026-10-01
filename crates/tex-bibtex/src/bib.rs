//! The `READ` command: reading the `.bib` database(s) and building the
//! entry list (bibtex.web part 12), including cross-reference handling.

use std::rc::Rc;

use crate::engine::{Engine, EntryType, Fatal, FnKind, Str};
use crate::input::{is_white, IdScan, Scanner};

#[derive(Clone, Copy, PartialEq, Eq)]
enum BibCommand {
    Preamble,
    String,
}

/// Per-entry scanning state (bibtex.web globals used while reading).
struct Rd {
    sc: Scanner,
    /// `Some` while processing an `@preamble`/`@string` command
    command: Option<BibCommand>,
    right_outer_delim: u8,
    right_str_delim: u8,
    store_entry: bool,
    store_field: bool,
    field_idx: usize,
    field_fn: u32,
    entry_cite_ptr: usize,
    cur_macro: Box<[u8]>,
    /// `field_vl_str`
    field: Vec<u8>,
}

/// `return false` from a scanning function after an error was reported.
type Ok_ = bool;

impl Engine {
    pub(crate) fn read_database(&mut self) -> Result<(), Fatal> {
        // <Final initialization for .bib processing>
        let n = self.num_cites;
        self.type_list = vec![EntryType::Empty; n];
        self.entry_exists = vec![false; n];
        self.cite_info = vec![None; n];
        self.xref_count = vec![0; n];
        self.field_info = vec![None; n * self.num_fields];
        self.old_num_cites = n;
        if self.all_entries {
            for p in self.all_marker..n {
                self.cite_info[p] = Some(self.cite_list[p].clone());
            }
            self.cite_ptr = self.all_marker;
        } else {
            self.cite_ptr = n;
        }
        self.read_performed = true;
        let inputs = std::mem::take(&mut self.bib_inputs);
        for (i, input) in inputs.into_iter().enumerate() {
            let print_name =
                Engine::announced_name(Engine::bib_file_name(&input.name), input.embedded);
            let line = format!("Database file #{}: ", i + 1);
            if self.verbose {
                self.log.print(line);
                self.log.println(&print_name);
            } else {
                self.log.log_only(line);
                self.log.log_only(&print_name);
                self.log.log_only("\n");
            }
            self.bib_name = input.name;
            self.bib_line = 0;
            let mut rd = Rd {
                sc: Scanner::new(input.bytes),
                command: None,
                right_outer_delim: b'}',
                right_str_delim: b'}',
                store_entry: false,
                store_field: false,
                field_idx: 0,
                field_fn: 0,
                entry_cite_ptr: 0,
                cur_macro: Box::default(),
                field: Vec::new(),
            };
            while !rd.sc.src.at_eof() {
                self.bib_command_or_entry(&mut rd)?;
            }
        }
        self.reading_completed = true;
        self.finish_reading()
    }

    /// Make sure cite slot `k` exists in every per-cite array.
    fn ensure_cite_slot(&mut self, k: usize) {
        if k >= self.cite_list.len() {
            self.cite_list.resize(k + 1, self.null_str.clone());
        }
        if k >= self.type_list.len() {
            self.type_list.resize(k + 1, EntryType::Empty);
            self.entry_exists.resize(k + 1, false);
            self.cite_info.resize(k + 1, None);
            self.xref_count.resize(k + 1, 0);
            self.field_info.resize((k + 1) * self.num_fields, None);
        }
    }

    /// `add_database_cite`: put `key` (lower case `lc`) at `cite_ptr`.
    fn add_database_cite(&mut self, key: Str, lc: &[u8]) {
        let new = self.cite_ptr;
        self.ensure_cite_slot(new);
        self.cite_list[new] = key.clone();
        self.cite_map.insert(key.to_vec().into(), new);
        self.lc_cite_map.insert(lc.into(), key);
        self.cite_ptr += 1;
    }

    // ------------------------------------------------------------------
    // error messages
    // ------------------------------------------------------------------

    /// `bib_ln_num_print`.
    fn bib_ln_num_print(&mut self) {
        let name = Engine::bib_file_name(&self.bib_name);
        self.log.print(format!("--line {} of file ", self.bib_line));
        self.log.println(name);
    }

    /// `bib_err`: report a serious error and abandon this entry/command.
    fn bib_err(&mut self, rd: &Rd, msg: impl AsRef<[u8]>) -> Ok_ {
        self.log.print(msg);
        self.log.print("-");
        self.bib_ln_num_print();
        self.print_bad_input_line(&rd.sc);
        self.log.print("I'm skipping whatever remains of this ");
        self.log.println(if rd.command.is_some() { "command" } else { "entry" });
        false
    }

    /// `bib_warn_newline`.
    fn bib_warn_newline(&mut self, msg: impl AsRef<[u8]>) {
        self.log.println(msg);
        self.bib_ln_num_print();
        self.log.mark_warning();
    }

    fn macro_name_warning(&mut self, rd: &Rd, what: &str) {
        self.log.print("Warning--string name \"");
        self.log.print(rd.sc.token());
        self.log.print("\" is ");
        self.bib_warn_newline(what);
    }

    /// `eat_bib_white_space`.
    fn eat_bib_white(&mut self, rd: &mut Rd) -> bool {
        while !rd.sc.scan_white_space() {
            if !rd.sc.input_ln() {
                return false;
            }
            self.bib_line += 1;
            rd.sc.p2 = 0;
        }
        true
    }

    /// `eat_bib_white_and_eof_check`.
    fn eat_bib_white_check(&mut self, rd: &mut Rd) -> Ok_ {
        self.eat_bib_white(rd) || self.bib_err(rd, "Illegal end of database file")
    }

    fn bib_identifier_scan_check(&mut self, rd: &Rd, result: IdScan, what: &str) -> Ok_ {
        match result {
            IdScan::WhiteAdjacent | IdScan::SpecifiedCharAdjacent => true,
            IdScan::Null => self.bib_err(rd, format!("You're missing {what}")),
            IdScan::OtherCharAdjacent => {
                let mut msg = vec![b'"', rd.sc.scan_char()];
                msg.extend_from_slice(b"\" immediately follows ");
                msg.extend_from_slice(what.as_bytes());
                self.bib_err(rd, msg)
            }
        }
    }

    fn bib_one_of_two_expected(&mut self, rd: &Rd, c1: u8, c2: u8) -> Ok_ {
        let msg = [b"I was expecting a `".as_slice(), &[c1], b"' or a `", &[c2], b"'"].concat();
        self.bib_err(rd, msg)
    }

    /// Read the left outer delimiter of an entry or command.
    fn scan_outer_delim(&mut self, rd: &mut Rd) -> Ok_ {
        rd.right_outer_delim = match rd.sc.scan_char() {
            b'{' => b'}',
            b'(' => b')',
            _ => return self.bib_one_of_two_expected(rd, b'{', b'('),
        };
        rd.sc.p2 += 1;
        true
    }

    // ------------------------------------------------------------------
    // entries and commands
    // ------------------------------------------------------------------

    fn bib_command_or_entry(&mut self, rd: &mut Rd) -> Result<(), Fatal> {
        rd.command = None;
        while !rd.sc.scan1(b'@') {
            if !rd.sc.input_ln() {
                return Ok(());
            }
            self.bib_line += 1;
            rd.sc.p2 = 0;
        }
        rd.sc.p2 += 1;
        if !self.eat_bib_white_check(rd) {
            return Ok(());
        }
        let r = rd.sc.scan_identifier(b'{', b'(', b'(');
        if !self.bib_identifier_scan_check(rd, r, "an entry type") {
            return Ok(());
        }
        rd.sc.lower_case_token();
        let mut entry_type = None;
        match rd.sc.token() {
            b"comment" => return Ok(()),
            b"preamble" => {
                rd.command = Some(BibCommand::Preamble);
                self.bib_preamble(rd);
                return Ok(());
            }
            b"string" => {
                rd.command = Some(BibCommand::String);
                self.bib_string(rd);
                return Ok(());
            }
            name => {
                if let Some(id) = self.lookup_fn(name) {
                    if matches!(self.fns[id as usize].kind, FnKind::Wiz(_)) {
                        entry_type = Some(id);
                    }
                }
            }
        }
        if !self.eat_bib_white_check(rd) || !self.scan_outer_delim(rd) || !self.eat_bib_white_check(rd) {
            return Ok(());
        }
        if rd.right_outer_delim == b')' {
            rd.sc.scan1_white(b',');
        } else {
            rd.sc.scan2_white(b',', b'}');
        }
        if !self.database_key_of_interest(rd, entry_type)? {
            return Ok(());
        }
        if !self.eat_bib_white_check(rd) {
            return Ok(());
        }
        // <Scan the entry's list of fields>
        while rd.sc.scan_char() != rd.right_outer_delim {
            if rd.sc.scan_char() != b',' {
                self.bib_one_of_two_expected(rd, b',', rd.right_outer_delim);
                return Ok(());
            }
            rd.sc.p2 += 1;
            if !self.eat_bib_white_check(rd) {
                return Ok(());
            }
            if rd.sc.scan_char() == rd.right_outer_delim {
                break;
            }
            if !self.field_name(rd) || !self.eat_bib_white_check(rd) {
                return Ok(());
            }
            if !self.scan_and_store_field_value(rd)? {
                return Ok(());
            }
        }
        rd.sc.p2 += 1;
        Ok(())
    }

    fn bib_preamble(&mut self, rd: &mut Rd) {
        if !self.eat_bib_white_check(rd) || !self.scan_outer_delim(rd) || !self.eat_bib_white_check(rd) {
            return;
        }
        rd.store_field = true;
        if !matches!(self.scan_and_store_field_value(rd), Ok(true)) {
            return;
        }
        if rd.sc.scan_char() != rd.right_outer_delim {
            let msg = [b"Missing \"".as_slice(), &[rd.right_outer_delim], b"\" in preamble command"].concat();
            self.bib_err(rd, msg);
            return;
        }
        rd.sc.p2 += 1;
    }

    fn bib_string(&mut self, rd: &mut Rd) {
        if !self.eat_bib_white_check(rd) || !self.scan_outer_delim(rd) || !self.eat_bib_white_check(rd) {
            return;
        }
        let r = rd.sc.scan_identifier(b'=', b'=', b'=');
        if !self.bib_identifier_scan_check(rd, r, "a string name") {
            return;
        }
        rd.sc.lower_case_token();
        rd.cur_macro = rd.sc.token().into();
        // the macro's own name is its value in case of error
        self.macros.insert(rd.cur_macro.clone(), Rc::from(&rd.cur_macro[..]));
        if !self.eat_bib_white_check(rd) {
            return;
        }
        if rd.sc.scan_char() != b'=' {
            self.bib_err(rd, "I was expecting an \"=\"");
            return;
        }
        rd.sc.p2 += 1;
        if !self.eat_bib_white_check(rd) {
            return;
        }
        rd.store_field = true;
        if !matches!(self.scan_and_store_field_value(rd), Ok(true)) {
            return;
        }
        if rd.sc.scan_char() != rd.right_outer_delim {
            let msg = [b"Missing \"".as_slice(), &[rd.right_outer_delim], b"\" in string command"].concat();
            self.bib_err(rd, msg);
            return;
        }
        rd.sc.p2 += 1;
    }

    /// `<Check for a database key of interest>` and `<Make sure this entry
    /// is ok before proceeding>`; false if the entry was abandoned.
    fn database_key_of_interest(&mut self, rd: &mut Rd, entry_type: Option<u32>) -> Result<bool, Fatal> {
        let key: Vec<u8> = rd.sc.token().to_vec();
        let lc = key.to_ascii_lowercase();
        let found = self.lc_cite_map.contains_key(&lc[..]);
        if found {
            let exact = self.lc_cite_map[&lc[..]].clone();
            rd.entry_cite_ptr = self.cite_map[&exact[..]];
            let p = rd.entry_cite_ptr;
            let mut first_time = false;
            if !self.all_entries || p < self.all_marker || p >= self.old_num_cites {
                if self.type_list[p] == EntryType::Empty {
                    // <Make sure this entry's database key is on cite_list>
                    if !self.all_entries && p >= self.old_num_cites && !self.cite_map.contains_key(&key[..]) {
                        let k: Str = Rc::from(&key[..]);
                        self.lc_cite_map.insert(lc.clone().into(), k.clone());
                        self.cite_map.insert(key.clone().into(), p);
                        self.cite_list[p] = k;
                    }
                    first_time = true;
                }
            } else if !self.entry_exists[p] {
                let saved = self.cite_info[p].as_deref().unwrap_or_default().to_ascii_lowercase();
                if !self.lc_cite_map.contains_key(&saved[..]) {
                    return Err(self.confusion("A cite key disappeared"));
                }
                if saved == lc {
                    first_time = true;
                }
            }
            if !first_time {
                if self.type_list[p] == EntryType::Empty {
                    return Err(self.confusion("The cite list is messed up"));
                }
                self.bib_err(rd, "Repeated entry");
                return Ok(false);
            }
        }
        rd.store_entry = true;
        if self.all_entries {
            // <Put this cite key in its place>
            let cite_key: Option<Str> = if found {
                if rd.entry_cite_ptr < self.all_marker {
                    None
                } else {
                    self.entry_exists[rd.entry_cite_ptr] = true;
                    Some(self.lc_cite_map[&lc[..]].clone())
                }
            } else {
                if self.cite_map.contains_key(&key[..]) {
                    return Err(self.confusion("Cite hash error"));
                }
                Some(Rc::from(&key[..]))
            };
            if let Some(k) = cite_key {
                rd.entry_cite_ptr = self.cite_ptr;
                self.add_database_cite(k, &lc);
            }
        } else if !found {
            rd.store_entry = false;
        }
        if rd.store_entry {
            let p = rd.entry_cite_ptr;
            match entry_type {
                Some(id) => self.type_list[p] = EntryType::Type(id),
                None => {
                    self.type_list[p] = EntryType::Undefined;
                    self.log.print("Warning--entry type for \"");
                    self.log.print(&key);
                    self.bib_warn_newline("\" isn't style-file defined");
                }
            }
        }
        Ok(true)
    }

    /// `<Get the next field name>`.
    fn field_name(&mut self, rd: &mut Rd) -> Ok_ {
        let r = rd.sc.scan_identifier(b'=', b'=', b'=');
        if !self.bib_identifier_scan_check(rd, r, "a field name") {
            return false;
        }
        rd.store_field = false;
        if rd.store_entry {
            rd.sc.lower_case_token();
            if let Some(id) = self.lookup_fn(rd.sc.token()) {
                if let FnKind::Field(idx) = self.fns[id as usize].kind {
                    rd.store_field = true;
                    rd.field_idx = idx;
                    rd.field_fn = id;
                }
            }
        }
        if !self.eat_bib_white_check(rd) {
            return false;
        }
        if rd.sc.scan_char() != b'=' {
            return self.bib_err(rd, "I was expecting an \"=\"");
        }
        rd.sc.p2 += 1;
        true
    }

    // ------------------------------------------------------------------
    // field values
    // ------------------------------------------------------------------

    /// `scan_and_store_the_field_value_and_eat_white`.
    fn scan_and_store_field_value(&mut self, rd: &mut Rd) -> Result<bool, Fatal> {
        rd.field.clear();
        if !self.scan_field_token(rd) {
            return Ok(false);
        }
        while rd.sc.scan_char() == b'#' {
            rd.sc.p2 += 1;
            if !self.eat_bib_white_check(rd) || !self.scan_field_token(rd) {
                return Ok(false);
            }
        }
        if rd.store_field {
            self.store_field_value(rd)?;
        }
        Ok(true)
    }

    /// `scan_a_field_token_and_eat_white`.
    fn scan_field_token(&mut self, rd: &mut Rd) -> Ok_ {
        match rd.sc.scan_char() {
            b'{' => {
                rd.right_str_delim = b'}';
                if !self.scan_balanced_braces(rd) {
                    return false;
                }
            }
            b'"' => {
                rd.right_str_delim = b'"';
                if !self.scan_balanced_braces(rd) {
                    return false;
                }
            }
            b'0'..=b'9' => {
                rd.sc.scan_nonneg_integer();
                if rd.store_field {
                    let (p1, p2) = (rd.sc.p1, rd.sc.p2);
                    rd.field.extend_from_slice(&rd.sc.buf[p1..p2]);
                }
            }
            _ => {
                let r = rd.sc.scan_identifier(b',', rd.right_outer_delim, b'#');
                if !self.bib_identifier_scan_check(rd, r, "a field part") {
                    return false;
                }
                if rd.store_field {
                    rd.sc.lower_case_token();
                    let value = self.macros.get(rd.sc.token()).cloned();
                    let own = rd.command == Some(BibCommand::String) && *rd.sc.token() == *rd.cur_macro;
                    if own {
                        self.macro_name_warning(rd, "used in its own definition");
                    }
                    match value {
                        None => self.macro_name_warning(rd, "undefined"),
                        Some(v) if !own => copy_macro(&mut rd.field, &v),
                        Some(_) => {}
                    }
                }
            }
        }
        self.eat_bib_white_check(rd)
    }

    /// `check_for_and_compress_bib_white_space`.
    fn compress_white_check(&mut self, rd: &mut Rd) -> Ok_ {
        if !is_white(rd.sc.scan_char()) && rd.sc.p2 != rd.sc.last {
            return true;
        }
        rd.field.push(b' ');
        while !rd.sc.scan_white_space() {
            if !rd.sc.input_ln() {
                return self.bib_err(rd, "Illegal end of database file");
            }
            self.bib_line += 1;
            rd.sc.p2 = 0;
        }
        true
    }

    /// `scan_balanced_braces`.
    fn scan_balanced_braces(&mut self, rd: &mut Rd) -> Ok_ {
        rd.sc.p2 += 1;
        if !self.compress_white_check(rd) {
            return false;
        }
        let n = rd.field.len();
        if n > 1 && rd.field[n - 1] == b' ' && rd.field[n - 2] == b' ' {
            rd.field.pop();
        }
        let delim = rd.right_str_delim;
        let mut level = 0u32;
        if rd.store_field {
            while rd.sc.scan_char() != delim {
                match rd.sc.scan_char() {
                    b'{' => {
                        level += 1;
                        rd.field.push(b'{');
                        rd.sc.p2 += 1;
                        if !self.compress_white_check(rd) {
                            return false;
                        }
                        loop {
                            let c = rd.sc.scan_char();
                            rd.field.push(c);
                            rd.sc.p2 += 1;
                            if !self.compress_white_check(rd) {
                                return false;
                            }
                            match c {
                                b'}' => {
                                    level -= 1;
                                    if level == 0 {
                                        break;
                                    }
                                }
                                b'{' => level += 1,
                                _ => {}
                            }
                        }
                    }
                    b'}' => return self.bib_err(rd, "Unbalanced braces"),
                    c => {
                        rd.field.push(c);
                        rd.sc.p2 += 1;
                        if !self.compress_white_check(rd) {
                            return false;
                        }
                    }
                }
            }
        } else {
            while rd.sc.scan_char() != delim {
                match rd.sc.scan_char() {
                    b'{' => {
                        level += 1;
                        rd.sc.p2 += 1;
                        if !self.eat_bib_white_check(rd) {
                            return false;
                        }
                        while level > 0 {
                            match rd.sc.scan_char() {
                                b'}' => {
                                    level -= 1;
                                    rd.sc.p2 += 1;
                                    if !self.eat_bib_white_check(rd) {
                                        return false;
                                    }
                                }
                                b'{' => {
                                    level += 1;
                                    rd.sc.p2 += 1;
                                    if !self.eat_bib_white_check(rd) {
                                        return false;
                                    }
                                }
                                _ => {
                                    rd.sc.p2 += 1;
                                    if !rd.sc.scan2(b'}', b'{') && !self.eat_bib_white_check(rd) {
                                        return false;
                                    }
                                }
                            }
                        }
                    }
                    b'}' => return self.bib_err(rd, "Unbalanced braces"),
                    _ => {
                        rd.sc.p2 += 1;
                        if !rd.sc.scan3(delim, b'{', b'}') && !self.eat_bib_white_check(rd) {
                            return false;
                        }
                    }
                }
            }
        }
        rd.sc.p2 += 1;
        true
    }

    /// `<Store the field value string>`.
    fn store_field_value(&mut self, rd: &mut Rd) -> Result<(), Fatal> {
        let mut end = rd.field.len();
        let mut start = 0;
        if rd.command.is_none() {
            if end > 0 && rd.field[end - 1] == b' ' {
                end -= 1;
            }
            if end > 0 && rd.field[0] == b' ' {
                start = 1;
            }
        }
        let value: Str = Rc::from(&rd.field[start..end.max(start)]);
        match rd.command {
            Some(BibCommand::Preamble) => self.preambles.push(value),
            Some(BibCommand::String) => {
                self.macros.insert(rd.cur_macro.clone(), value);
            }
            None => {
                let ptr = rd.entry_cite_ptr * self.num_fields + rd.field_idx;
                if self.field_info[ptr].is_some() {
                    let cite = self.cite_list[rd.entry_cite_ptr].clone();
                    let field_name = self.fns[rd.field_fn as usize].name.clone();
                    self.log.print("Warning--I'm ignoring ");
                    self.log.print(&cite[..]);
                    self.log.print("'s extra \"");
                    self.log.print(field_name);
                    self.bib_warn_newline("\" field");
                } else {
                    self.field_info[ptr] = Some(value.clone());
                    if rd.field_idx == self.crossref_num && !self.all_entries {
                        self.add_crossref(value)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `<Add or update a cross reference on cite_list if necessary>`.
    fn add_crossref(&mut self, value: Str) -> Result<(), Fatal> {
        let lc = value.to_ascii_lowercase();
        if let Some(exact) = self.lc_cite_map.get(&lc[..]) {
            let idx = self.cite_map[&exact[..]];
            if idx >= self.old_num_cites {
                self.xref_count[idx] += 1;
            }
        } else {
            if self.cite_map.contains_key(&value[..]) {
                return Err(self.confusion("Cite hash error"));
            }
            let idx = self.cite_ptr;
            self.add_database_cite(value, &lc);
            self.xref_count[idx] = 1;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // <Final initialization for processing the entries>
    // ------------------------------------------------------------------

    /// `find_cite_locs_for_this_cite_key`: (exact spelling found, the
    /// official spelling of the lower-case match).
    fn find_cite_locs(&self, key: &[u8]) -> (bool, Option<Str>) {
        let exact = self.cite_map.contains_key(key);
        let lc = key.to_ascii_lowercase();
        (exact, self.lc_cite_map.get(&lc[..]).cloned())
    }

    fn finish_reading(&mut self) -> Result<(), Fatal> {
        self.num_cites = self.cite_ptr;
        let nf = self.num_fields;
        let xr = self.crossref_num;
        // <Add cross-reference information>
        for cp in 0..self.num_cites {
            let Some(v) = self.field_info[cp * nf + xr].clone() else { continue };
            let (_, Some(official)) = self.find_cite_locs(&v) else { continue };
            let parent = self.cite_map[&official[..]];
            self.field_info[cp * nf + xr] = Some(official);
            for f in 1..nf {
                if self.field_info[cp * nf + f].is_none() {
                    self.field_info[cp * nf + f] = self.field_info[parent * nf + f].clone();
                }
            }
        }
        // <Subtract cross-reference information>
        for cp in 0..self.num_cites {
            let Some(v) = self.field_info[cp * nf + xr].clone() else { continue };
            let (exact, official) = self.find_cite_locs(&v);
            let parent = match official {
                None => {
                    if exact {
                        return Err(self.confusion("Cite hash error"));
                    }
                    None
                }
                Some(_) if !exact => return Err(self.confusion("Cite hash error")),
                Some(_) => Some(self.cite_map[&v[..]]),
            };
            match parent {
                Some(p) if self.type_list[p] != EntryType::Empty => {
                    if self.field_info[p * nf + xr].is_some() {
                        self.log.print("Warning--you've nested cross references");
                        let parent_key = self.cite_list[p].clone();
                        self.bad_cross_reference_print(cp, &parent_key);
                        self.log.println("\", which also refers to something");
                        self.log.mark_warning();
                    }
                    if !self.all_entries && p >= self.old_num_cites && self.xref_count[p] < self.min_crossrefs {
                        self.field_info[cp * nf + xr] = None;
                    }
                }
                _ => {
                    self.log.print("A bad cross reference-");
                    self.bad_cross_reference_print(cp, &v);
                    self.log.println("\", which doesn't exist");
                    self.log.mark_error();
                    self.field_info[cp * nf + xr] = None;
                }
            }
        }
        // <Remove missing entries or those cross referenced too few times>
        let mut xptr = 0;
        for cp in 0..self.num_cites {
            if self.type_list[cp] == EntryType::Empty {
                let key = self.cite_list[cp].clone();
                self.print_missing_entry(&key);
            } else if self.all_entries || cp < self.old_num_cites || self.xref_count[cp] >= self.min_crossrefs {
                if cp > xptr {
                    let key = self.cite_list[cp].clone();
                    if !self.cite_map.contains_key(&key[..]) {
                        return Err(self.confusion("Cite hash error"));
                    }
                    self.cite_map.insert(key.to_vec().into(), xptr);
                    self.cite_list[xptr] = key;
                    self.type_list[xptr] = self.type_list[cp];
                    for f in 0..nf {
                        self.field_info[xptr * nf + f] = self.field_info[cp * nf + f].take();
                    }
                }
                xptr += 1;
            }
        }
        self.num_cites = xptr;
        if self.all_entries {
            for cp in self.all_marker..self.old_num_cites {
                if !self.entry_exists[cp] {
                    let key = self.cite_info[cp].clone().unwrap_or_else(|| self.null_str.clone());
                    self.print_missing_entry(&key);
                }
            }
        }
        self.cite_list.truncate(xptr);
        self.type_list.truncate(xptr);
        self.field_info.truncate(xptr * nf);
        self.entry_ints = vec![0; self.num_ent_ints * xptr];
        self.entry_strs = vec![self.null_str.clone(); self.num_ent_strs * xptr];
        self.sorted_cites = (0..xptr).collect();
        Ok(())
    }

    fn bad_cross_reference_print(&mut self, cp: usize, target: &[u8]) {
        let cite = self.cite_list[cp].clone();
        self.log.print("--entry \"");
        self.log.print(&cite[..]);
        self.log.println("\"");
        self.log.print("refers to entry \"");
        self.log.print(target);
    }

    fn print_missing_entry(&mut self, key: &[u8]) {
        self.log.print("Warning--I didn't find a database entry for \"");
        self.log.print(key);
        self.log.println("\"");
        self.log.mark_warning();
    }
}

/// `<Copy the macro string to field_vl_str>`: compress white space.
fn copy_macro(field: &mut Vec<u8>, v: &[u8]) {
    let mut i = 0;
    if field.is_empty() && !v.is_empty() && is_white(v[0]) {
        field.push(b' ');
        i = 1;
        while i < v.len() && is_white(v[i]) {
            i += 1;
        }
    }
    while i < v.len() {
        if !is_white(v[i]) {
            field.push(v[i]);
        } else if field.last() != Some(&b' ') {
            field.push(b' ');
        }
        i += 1;
    }
}
