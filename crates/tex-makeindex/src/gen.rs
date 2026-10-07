//! Writing the formatted index: makeindex's `genind.c`.

use crate::locale::Locale;
use crate::scan::{Entry, ALPHA, DOT_MAX, DUPLICATE, FIELD_MAX, SYMBOL};
use crate::style::Style;
use crate::Transcript;

/// Settings of one `gen_ind` run that do not come from the style.
pub(crate) struct Layout<'a> {
    pub(crate) style: &'a Style,
    /// `-r` not given.
    pub(crate) merge_page: bool,
    pub(crate) german_sort: bool,
    pub(crate) thai_sort: bool,
    /// `-p`: the page to start at (already resolved for `even`/`odd`/`any`).
    pub(crate) start_page: Option<Vec<u8>>,
    pub(crate) input_names: &'a [String],
    pub(crate) output_name: &'a str,
    /// The environment's `LC_CTYPE`, used for the letters of new groups.
    pub(crate) locale: Option<&'a Locale>,
}

/// `TOLOWER`/`TOUPPER` in the "C" locale.
fn c_lower(byte: u8) -> u8 {
    byte.to_ascii_lowercase()
}

fn c_upper(byte: u8) -> u8 {
    byte.to_ascii_uppercase()
}

struct Gen<'a, 'l> {
    layout: &'a Layout<'a>,
    style: &'a Style,
    entries: &'a [Entry],
    log: &'l mut Transcript,
    out: Vec<u8>,
    curr: Option<usize>,
    prev: Option<usize>,
    begin: usize,
    the_end: usize,
    range_ptr: usize,
    level: usize,
    prev_level: usize,
    /// The current entry's encapsulator without its range operator.
    encap: Vec<u8>,
    prev_encap: Option<Vec<u8>>,
    in_range: bool,
    encap_range: bool,
    buff: Vec<u8>,
    /// The output line under construction.
    line: Vec<u8>,
    /// Output lines so far (`ind_lc`).
    lc: usize,
    /// Warnings (`ind_ec`).
    ec: usize,
    indent: usize,
}

/// `page_diff`: the distance between the last fields of two pages of the
/// same shape, -1 otherwise.
fn page_diff(a: &Entry, b: &Entry) -> i32 {
    if a.npg.len() != b.npg.len() {
        return -1;
    }
    let count = a.npg.len();
    if count == 0 || a.npg[..count - 1] != b.npg[..count - 1] {
        return if count == 0 { 0 } else { -1 };
    }
    b.npg[count - 1].wrapping_sub(a.npg[count - 1])
}

impl Gen<'_, '_> {
    fn entry(&self, index: usize) -> &Entry {
        &self.entries[index]
    }

    fn curr(&self) -> &Entry {
        &self.entries[self.curr.expect("an entry is current")]
    }

    fn prev(&self) -> &Entry {
        &self.entries[self.prev.or(self.curr).expect("an entry was seen")]
    }

    fn put(&mut self, text: &[u8]) {
        self.out.extend_from_slice(text);
    }

    fn putln(&mut self, text: &[u8]) {
        self.out.extend_from_slice(text);
        self.out.push(b'\n');
        self.lc += 1;
    }

    fn warn(&mut self, message: &[u8]) {
        let (file, line) = {
            let curr = self.curr();
            (curr.file, curr.lc)
        };
        let name = self.layout.input_names.get(file).map_or("", String::as_str);
        self.log.error_line();
        self.log.ilg(
            format!(
                "## Warning (input = {name}, line = {line}; output = {}, line = {}):\n   -- ",
                self.layout.output_name,
                self.lc + 1
            )
            .as_bytes(),
        );
        self.log.ilg(message);
        self.ec += 1;
    }

    /// `SAVE`.
    fn save(&mut self) {
        let curr = self.curr.expect("an entry is current");
        self.begin = curr;
        self.the_end = curr;
        self.prev_encap = Some(self.encap.clone());
    }

    fn run(&mut self) {
        let style = self.style;
        self.log.message(format!("Generating output file {}...", self.layout.output_name).as_bytes());
        self.put(&style.preamble);
        self.lc += style.prelen;
        if let Some(page) = self.layout.start_page.clone() {
            self.put(&style.setpage_prefix);
            self.put(&page);
            self.put(&style.setpage_suffix);
            self.lc += style.setpagelen;
        }
        self.log.idx_dc = 0;
        for n in 0..self.entries.len() {
            if self.entries[n].ty != DUPLICATE {
                self.make_entry(n);
                self.log.dot(DOT_MAX);
            }
        }
        if self.curr.is_some() {
            if self.in_range {
                self.curr = Some(self.range_ptr);
                let message = [&b"Unmatched range opening operator "[..], &[style.range_open], b".\n"].concat();
                self.warn(&message);
            }
            self.prev = self.curr;
            self.flush_line(true);
        }
        self.put(&style.delim_t);
        self.put(&style.postamble);
        let lines = self.lc + style.postlen;
        let plural = if self.ec == 1 { "warning" } else { "warnings" };
        self.log.message(format!("done ({lines} lines written, {} {plural}).\n", self.ec).as_bytes());
    }

    fn make_entry(&mut self, n: usize) {
        let style = self.style;
        let first = self.curr.is_none();
        self.prev = self.curr;
        self.curr = Some(n);
        let encap = &self.entries[n].encap;
        let lead = encap.first().copied().unwrap_or(0);
        self.encap =
            if lead == style.range_open || lead == style.range_close { encap[1..].to_vec() } else { encap.clone() };

        if first {
            self.prev_level = 0;
            self.level = 0;
            // The first group letter is converted in the "C" locale.
            let letter = self.entries[n].sf[0].first().copied().unwrap_or(0);
            self.put_header(letter, false);
            self.make_item(&[]);
        } else {
            self.prev_level = self.level;
            let (curr, prev) = (self.curr(), self.prev());
            let level = (0..FIELD_MAX)
                .find(|&level| curr.sf[level] != prev.sf[level] || curr.af[level] != prev.af[level])
                .unwrap_or(FIELD_MAX);
            self.level = level;
            if level < FIELD_MAX {
                self.new_entry();
            } else if !(lead == style.range_open && self.in_range) {
                self.old_entry();
            }
        }

        let encap = self.entries[n].encap.clone();
        let prev_encap = self.prev_encap.clone().unwrap_or_default();
        if lead == style.range_open {
            if self.in_range {
                let message = [&b"Extra range opening operator "[..], &[style.range_open], b".\n"].concat();
                self.warn(&message);
            } else {
                self.in_range = true;
                self.range_ptr = n;
            }
        } else if lead == style.range_close {
            if self.in_range {
                self.in_range = false;
                let closing = &encap[1..];
                if !closing.is_empty() && prev_encap != closing {
                    let message =
                        [&b"Range closing operator has an inconsistent encapsulator "[..], closing, b".\n"].concat();
                    self.warn(&message);
                }
            } else {
                let message = [&b"Unmatched range closing operator "[..], &[style.range_close], b".\n"].concat();
                self.warn(&message);
            }
        } else if !encap.is_empty() && encap != prev_encap && self.in_range {
            let message = [&b"Inconsistent page encapsulator "[..], &encap, b" within range.\n"].concat();
            self.warn(&message);
        }
    }

    /// `make_item`: starts the output line of a new item, printing the
    /// lines of any parent levels that are new as well.
    fn make_item(&mut self, term: &[u8]) {
        let style = self.style;
        let curr = self.curr.expect("an entry is current");
        let level = self.level;
        let text = |entry: &Entry, level: usize| -> Vec<u8> {
            if entry.af[level].is_empty() { entry.sf[level].clone() } else { entry.af[level].clone() }
        };
        let (item, lines) = if level > self.prev_level {
            (&style.item_u[level], style.ilen_u[level])
        } else {
            (&style.item_r[level], style.ilen_r[level])
        };
        self.line = [term, item, &text(self.entry(curr), level)].concat();
        self.lc += lines;
        let mut i = level + 1;
        while i < FIELD_MAX && !self.entry(curr).sf[i].is_empty() {
            let line = std::mem::take(&mut self.line);
            self.put(&line);
            self.line = [&style.item_x[i][..], &text(self.entry(curr), i)].concat();
            self.lc += style.ilen_x[i];
            self.level = i;
            i += 1;
        }
        self.indent = 0;
        let delimiter = style.delim_p[self.level].clone();
        self.line.extend_from_slice(&delimiter);
        self.save();
    }

    /// `first_letter`: the byte that decides the group of a key.
    fn first_letter(&self, term: &[u8]) -> u8 {
        let at = |index: usize| term.get(index).copied().unwrap_or(0);
        if self.layout.thai_sort {
            // Thai leading vowels (TIS-620) do not count.
            return if matches!(at(0), 0xe0..=0xe4) || at(0) == 0 { at(1) } else { at(0) };
        }
        match self.layout.locale {
            Some(locale) => locale.to_lower(at(0)),
            None => c_lower(at(0)),
        }
    }

    fn new_entry(&mut self) {
        let style = self.style;
        if self.in_range {
            let current = self.curr;
            self.curr = Some(self.range_ptr);
            let message = [&b"Unmatched range opening operator "[..], &[style.range_open], b".\n"].concat();
            self.warn(&message);
            self.in_range = false;
            self.curr = current;
        }
        self.flush_line(true);

        let (curr, prev) = (self.curr(), self.prev());
        let mut letter = None;
        let new_group = (curr.group != ALPHA && curr.group != prev.group && prev.group == SYMBOL)
            || (curr.group == ALPHA && {
                let first = self.first_letter(&curr.sf[0]);
                letter = Some(first);
                first != self.first_letter(&prev.sf[0])
            })
            || (self.layout.german_sort && curr.group != ALPHA && prev.group == ALPHA);
        if new_group {
            self.put(&style.delim_t);
            self.put(&style.group_skip);
            self.lc += style.skiplen;
            self.put_header(letter.unwrap_or(0xff), true);
            self.make_item(&[]);
        } else {
            let term = style.delim_t.clone();
            self.make_item(&term);
        }
    }

    fn old_entry(&mut self) {
        let style = self.style;
        let curr_index = self.curr.expect("an entry is current");
        let diff = page_diff(self.entry(self.the_end), self.entry(curr_index));
        let same_type = self.prev().ty == self.curr().ty;
        let same_encap = self.prev_encap.as_ref().is_some_and(|prev| *prev == self.encap);
        if same_type
            && diff != -1
            && ((diff == 0 && same_encap) || (self.layout.merge_page && diff == 1 && same_encap) || self.in_range)
        {
            self.the_end = curr_index;
            let curr = self.curr();
            let lead = curr.encap.first().copied().unwrap_or(0);
            if self.in_range
                && lead != 0
                && lead != style.range_close
                && self.prev_encap.as_deref() != Some(curr.encap.as_slice())
            {
                self.buff = [&style.encap_prefix[..], &curr.encap, &style.encap_infix, &curr.lpg, &style.encap_suffix]
                    .concat();
                self.wrap_line(false);
            }
            if self.in_range {
                self.encap_range = true;
            }
        } else {
            self.flush_line(false);
            if diff == 0 && same_type {
                self.warn(b"Conflicting entries: multiple encaps for the same page under same key.\n");
            } else if self.in_range && !same_type {
                self.warn(b"Illegal range formation: starting & ending pages are of different types.\n");
            } else if self.in_range && diff == -1 {
                self.warn(b"Illegal range formation: starting & ending pages cross chap/sec breaks.\n");
            }
            self.save();
        }
    }

    /// `put_header`: the group heading. `in_locale` tells whether the letter
    /// is converted under the environment's `LC_CTYPE`.
    fn put_header(&mut self, letter: u8, in_locale: bool) {
        let style = self.style;
        if style.headings_flag == 0 {
            return;
        }
        self.put(&style.heading_prefix);
        self.lc += style.headprelen;
        let positive = style.headings_flag > 0;
        match self.curr().group {
            SYMBOL => {
                let text = if positive { &style.symhead_positive } else { &style.symhead_negative };
                self.put(text);
            }
            ALPHA => {
                let locale = if in_locale { self.layout.locale } else { None };
                let byte = match (locale, positive) {
                    (Some(locale), true) => locale.to_upper(letter),
                    (Some(locale), false) => locale.to_lower(letter),
                    (None, true) => c_upper(letter),
                    (None, false) => c_lower(letter),
                };
                self.put(&[byte]);
            }
            _ => {
                let text = if positive { &style.numhead_positive } else { &style.numhead_negative };
                self.put(text);
            }
        }
        self.put(&style.heading_suffix);
        self.lc += style.headsuflen;
    }

    /// `flush_line`: prints the pages collected since `begin`.
    fn flush_line(&mut self, print: bool) {
        let style = self.style;
        let entries = self.entries;
        let prev = self.prev.or(self.curr).expect("an entry was seen");
        let (begin, the_end, prev) = (&entries[self.begin], &entries[self.the_end], &entries[prev]);
        if page_diff(begin, the_end) != 0 {
            let threshold = if style.suffix_2p.is_empty() { 1 } else { 0 };
            if self.encap_range || page_diff(begin, prev) > threshold {
                let diff = page_diff(begin, the_end);
                self.buff = if diff == 1 && !style.suffix_2p.is_empty() {
                    [&begin.lpg[..], &style.suffix_2p].concat()
                } else if diff == 2 && !style.suffix_3p.is_empty() {
                    [&begin.lpg[..], &style.suffix_3p].concat()
                } else if diff >= 2 && !style.suffix_mp.is_empty() {
                    [&begin.lpg[..], &style.suffix_mp].concat()
                } else {
                    [&begin.lpg[..], &style.delim_r, &the_end.lpg].concat()
                };
                self.encap_range = false;
            } else {
                self.buff = [&begin.lpg[..], &style.delim_n, &the_end.lpg].concat();
            }
        } else {
            self.encap_range = false;
            self.buff = begin.lpg.clone();
        }
        if let Some(prev_encap) = self.prev_encap.as_ref().filter(|encap| !encap.is_empty()) {
            self.buff =
                [&style.encap_prefix[..], prev_encap, &style.encap_infix, &self.buff, &style.encap_suffix].concat();
        }
        self.wrap_line(print);
    }

    fn wrap_line(&mut self, print: bool) {
        let style = self.style;
        let length = self.line.len() + self.buff.len() + self.indent;
        let too_long = length as i64 > i64::from(style.line_max);
        let indent = usize::try_from(style.indent_length).unwrap_or(0);
        if print {
            let line = std::mem::take(&mut self.line);
            if too_long {
                self.putln(&line);
                self.put(&style.indent_space);
                self.indent = indent;
            } else {
                self.put(&line);
            }
            self.line = line;
            let buff = std::mem::take(&mut self.buff);
            self.put(&buff);
            self.buff = buff;
        } else if too_long {
            let line = std::mem::take(&mut self.line);
            self.putln(&line);
            self.line = [&style.indent_space[..], &self.buff, &style.delim_n].concat();
            self.indent = indent;
        } else {
            self.buff.extend_from_slice(&style.delim_n);
            self.line.extend_from_slice(&self.buff);
        }
    }
}

/// Output of `gen_ind`.
pub(crate) struct Output {
    pub(crate) bytes: Vec<u8>,
}

/// `gen_ind`: writes the sorted `entries`.
pub(crate) fn generate(entries: &[Entry], layout: &Layout, log: &mut Transcript) -> Output {
    let mut gen = Gen {
        layout,
        style: layout.style,
        entries,
        log,
        out: Vec::new(),
        curr: None,
        prev: None,
        begin: 0,
        the_end: 0,
        range_ptr: 0,
        level: 0,
        prev_level: 0,
        encap: Vec::new(),
        prev_encap: None,
        in_range: false,
        encap_range: false,
        buff: Vec::new(),
        line: Vec::new(),
        lc: 0,
        ec: 0,
        indent: 0,
    };
    gen.run();
    Output { bytes: gen.out }
}

/// `insert_page` for `-p even`/`odd`/`any`: the page after `last`, made
/// even or odd as asked.
pub(crate) fn next_page(last: &[u8], even_odd: i32) -> Vec<u8> {
    let mut page = last.to_vec();
    if page.is_empty() {
        return page;
    }
    let j = page.len() - 1;
    // The trailing run of digits.
    let mut i = j;
    while i > 0 && page[i].is_ascii_digit() {
        i -= 1;
    }
    if !page[i].is_ascii_digit() {
        i += 1;
    }
    let digits = page.get(i..).unwrap_or(&[]);
    let mut value = digits.iter().fold(0i32, |value, &digit| {
        value.wrapping_mul(10).wrapping_add(i32::from(digit)).wrapping_sub(48)
    });
    value = value.wrapping_add(1);
    if (even_odd == 1 && value % 2 == 0) || (even_odd == 2 && value % 2 == 1) {
        value += 1;
    }
    page.truncate(i.min(page.len()));
    page.extend_from_slice(value.to_string().as_bytes());
    page
}
