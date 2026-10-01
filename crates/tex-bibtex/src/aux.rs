//! Reading the `.aux` file(s) (bibtex.web part 9): `\citation`,
//! `\bibdata`, `\bibstyle` and `\@input`, one command per line.

use std::collections::HashSet;
use std::rc::Rc;

use tex_kpse::Format;

use crate::engine::{BibInput, Engine, Fatal};
use crate::input::{is_white, Scanner};

/// `aux_stack_size`
const AUX_STACK_SIZE: usize = 20;

struct AuxFile {
    name: Vec<u8>,
    sc: Scanner,
}

impl Engine {
    pub(crate) fn read_aux(&mut self, top_name: &[u8], bytes: Vec<u8>) -> Result<(), Fatal> {
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        seen.insert(top_name.to_vec());
        let mut bib_seen_names: HashSet<Vec<u8>> = HashSet::new();
        let mut stack = vec![AuxFile {
            name: top_name.to_vec(),
            sc: Scanner::new(bytes),
        }];
        loop {
            let depth = stack.len();
            let Some(cur) = stack.last_mut() else { break };
            cur.sc.line += 1;
            if !cur.sc.input_ln() {
                stack.pop();
                continue;
            }
            let name = cur.name.clone();
            let sc = &mut cur.sc;
            sc.p2 = 0;
            if !sc.scan1(b'{') {
                continue;
            }
            let pushed = match sc.token() {
                b"\\bibdata" => {
                    self.aux_bib_data(sc, &name, &mut bib_seen_names);
                    None
                }
                b"\\bibstyle" => {
                    self.aux_bib_style(sc, &name);
                    None
                }
                b"\\citation" => {
                    self.aux_citation(sc, &name);
                    None
                }
                b"\\@input" => self.aux_input(sc, &name, &mut seen, depth)?,
                _ => None,
            };
            if let Some(file) = pushed {
                stack.push(file);
            }
        }
        self.last_check_for_aux_errors(top_name);
        Ok(())
    }

    /// `aux_err_print`: location, the offending line, and the skip notice.
    fn aux_err(&mut self, sc: &Scanner, aux_name: &[u8]) {
        self.log.print(format!("---line {} of file ", sc.line));
        self.log.println(aux_name);
        self.print_bad_input_line(sc);
        self.log.println("I'm skipping whatever remains of this command");
    }

    /// The argument checks shared by the four commands; true if an
    /// error was reported. `list` commands allow commas.
    fn aux_arg_error(&mut self, sc: &Scanner, aux_name: &[u8], scanned: bool, list: bool) -> bool {
        let msg = if !scanned {
            "No \"}\""
        } else if is_white(sc.scan_char()) {
            "White space in argument"
        } else if sc.last > sc.p2 + 1 && (!list || sc.scan_char() == b'}') {
            "Stuff after \"}\""
        } else {
            return false;
        };
        self.log.print(msg);
        self.aux_err(sc, aux_name);
        true
    }

    fn aux_bib_data(&mut self, sc: &mut Scanner, aux_name: &[u8], seen: &mut HashSet<Vec<u8>>) {
        if self.bib_seen {
            self.log.print("Illegal, another \\bibdata command");
            self.aux_err(sc, aux_name);
            return;
        }
        self.bib_seen = true;
        while sc.scan_char() != b'}' {
            sc.p2 += 1;
            let scanned = sc.scan2_white(b'}', b',');
            if self.aux_arg_error(sc, aux_name, scanned, true) {
                return;
            }
            let name = sc.token().to_vec();
            let print_name = Engine::bib_file_name(&name);
            if !seen.insert(name.clone()) {
                self.log.print("This database file appears more than once: ");
                self.log.println(print_name);
                self.aux_err(sc, aux_name);
                return;
            }
            match self.open_input(&name, Format::Bib) {
                Some((bytes, embedded)) => self.bib_inputs.push(BibInput { name, bytes, embedded }),
                None => {
                    self.log.print("I couldn't open database file ");
                    self.log.println(print_name);
                    self.aux_err(sc, aux_name);
                    return;
                }
            }
        }
    }

    fn aux_bib_style(&mut self, sc: &mut Scanner, aux_name: &[u8]) {
        if self.bst_seen {
            self.log.print("Illegal, another \\bibstyle command");
            self.aux_err(sc, aux_name);
            return;
        }
        self.bst_seen = true;
        sc.p2 += 1;
        let scanned = sc.scan1_white(b'}');
        if self.aux_arg_error(sc, aux_name, scanned, false) {
            return;
        }
        let name = sc.token().to_vec();
        let mut print_name = name.clone();
        print_name.extend_from_slice(b".bst");
        match self.open_input(&name, Format::Bst) {
            Some((bytes, embedded)) => {
                self.bst_bytes = bytes;
                self.bst_name = Some(name);
                let print_name = Engine::announced_name(print_name, embedded);
                if self.verbose {
                    self.log.print("The style file: ");
                    self.log.println(print_name);
                } else {
                    self.log.log_only("The style file: ");
                    self.log.log_only(print_name);
                    self.log.log_only("\n");
                }
            }
            None => {
                self.log.print("I couldn't open style file ");
                self.log.println(print_name);
                self.aux_err(sc, aux_name);
            }
        }
    }

    fn aux_citation(&mut self, sc: &mut Scanner, aux_name: &[u8]) {
        self.citation_seen = true;
        while sc.scan_char() != b'}' {
            sc.p2 += 1;
            let scanned = sc.scan2_white(b'}', b',');
            if self.aux_arg_error(sc, aux_name, scanned, true) {
                return;
            }
            let key = sc.token();
            if key == b"*" {
                if self.all_entries {
                    self.log.println("Multiple inclusions of entire database");
                    self.aux_err(sc, aux_name);
                    return;
                }
                self.all_entries = true;
                self.all_marker = self.cite_list.len();
                continue;
            }
            let lc = key.to_ascii_lowercase();
            if let Some(exact) = self.lc_cite_map.get(&lc[..]) {
                if !self.cite_map.contains_key(key) {
                    let other = self.cite_list[self.cite_map[&exact[..]]].clone();
                    let mut msg = b"Case mismatch error between cite keys ".to_vec();
                    msg.extend_from_slice(key);
                    msg.extend_from_slice(b" and ");
                    msg.extend_from_slice(&other);
                    self.log.println(msg);
                    self.aux_err(sc, aux_name);
                    return;
                }
            } else {
                let key: Rc<[u8]> = Rc::from(key);
                self.cite_map.insert(key.to_vec().into(), self.cite_list.len());
                self.lc_cite_map.insert(lc.into(), key.clone());
                self.cite_list.push(key);
            }
        }
    }

    fn aux_input(
        &mut self,
        sc: &mut Scanner,
        aux_name: &[u8],
        seen: &mut HashSet<Vec<u8>>,
        depth: usize,
    ) -> Result<Option<AuxFile>, Fatal> {
        sc.p2 += 1;
        let scanned = sc.scan1_white(b'}');
        if self.aux_arg_error(sc, aux_name, scanned, false) {
            return Ok(None);
        }
        let name = sc.token().to_vec();
        if depth == AUX_STACK_SIZE {
            self.log.print(&name);
            self.log.print(": ");
            self.log
                .println(format!("Sorry---you've exceeded BibTeX's auxiliary file depth {AUX_STACK_SIZE}"));
            self.log.mark_fatal();
            return Err(Fatal);
        }
        if !name.ends_with(b".aux") {
            self.log.print(&name);
            self.log.print(" has a wrong extension");
            self.aux_err(sc, aux_name);
            return Ok(None);
        }
        if !seen.insert(name.clone()) {
            self.log.print("Already encountered file ");
            self.log.println(&name);
            self.aux_err(sc, aux_name);
            return Ok(None);
        }
        let Some(bytes) = self.open_aux(&name) else {
            self.log.print("I couldn't open auxiliary file ");
            self.log.println(&name);
            self.aux_err(sc, aux_name);
            return Ok(None);
        };
        self.log.log_only(format!("A level-{depth} auxiliary file: "));
        self.log.log_only(&name);
        self.log.log_only("\n");
        Ok(Some(AuxFile {
            name,
            sc: Scanner::new(bytes),
        }))
    }

    fn last_check_for_aux_errors(&mut self, top_name: &[u8]) {
        self.num_cites = self.cite_list.len();
        let aux_end_err = |e: &mut Engine, what: &str| {
            e.log.print(format!("I found no {what}---while reading file "));
            e.log.println(top_name);
            e.log.mark_error();
        };
        if !self.citation_seen {
            aux_end_err(self, "\\citation commands");
        } else if self.num_cites == 0 && !self.all_entries {
            aux_end_err(self, "cite keys");
        }
        if !self.bib_seen {
            aux_end_err(self, "\\bibdata command");
        } else if self.bib_inputs.is_empty() {
            aux_end_err(self, "database files");
        }
        if !self.bst_seen {
            aux_end_err(self, "\\bibstyle command");
        } else if self.bst_name.is_none() {
            aux_end_err(self, "style file");
        }
    }
}
