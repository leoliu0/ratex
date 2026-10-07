//! Writing the formatted index.

use crate::page::Page;
use crate::scan::{Entry, Range};
use crate::sort::{group_of, Group};
use crate::style::Style;

pub(crate) struct Output {
    pub(crate) bytes: Vec<u8>,
    pub(crate) lines: usize,
    pub(crate) warnings: usize,
}

/// Formatting parameters that do not come from the style file.
pub(crate) struct Layout<'a> {
    pub(crate) style: &'a Style,
    pub(crate) no_ranges: bool,
    pub(crate) start_page: Option<&'a [u8]>,
    pub(crate) input_names: &'a [String],
    pub(crate) output_name: &'a str,
}

struct Writer<'a> {
    bytes: Vec<u8>,
    /// Columns used on the current output line.
    column: usize,
    lines: usize,
    warnings: usize,
    layout: &'a Layout<'a>,
    log: &'a mut dyn FnMut(String),
}

impl Writer<'_> {
    fn push(&mut self, text: &[u8]) {
        match text.iter().rposition(|&byte| byte == b'\n') {
            Some(last) => {
                self.lines += text.iter().filter(|&&byte| byte == b'\n').count();
                self.column = text.len() - last - 1;
            }
            None => self.column += text.len(),
        }
        self.bytes.extend_from_slice(text);
    }

    fn warn(&mut self, entry: &Entry, message: &str) {
        self.warnings += 1;
        let name = self.layout.input_names.get(entry.file).map_or("", String::as_str);
        (self.log)(format!(
            "## Warning (input = {name}, line = {}; output = {}, line = {}):\n   -- {message}\n",
            entry.line,
            self.layout.output_name,
            self.lines + 1
        ));
    }
}

fn wrap_encap(style: &Style, encap: &[u8], text: &[u8]) -> Vec<u8> {
    if encap.is_empty() {
        return text.to_vec();
    }
    let mut out = Vec::new();
    out.extend_from_slice(&style.encap_prefix);
    out.extend_from_slice(encap);
    out.extend_from_slice(&style.encap_infix);
    out.extend_from_slice(text);
    out.extend_from_slice(&style.encap_suffix);
    out
}

fn join(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// A stretch of pages printed as one unit.
struct Segment<'e> {
    start: &'e Entry,
    end: &'e Entry,
    encap: &'e [u8],
    /// Written with explicit range operators.
    explicit: bool,
    /// Single pages merged into this segment.
    pages: usize,
}

fn explicit_segment<'e>(start: &'e Entry, end: &'e Entry, encap: &'e [u8]) -> Segment<'e> {
    if std::ptr::eq(start, end) {
        // A range operator without a second page is an ordinary page.
        return single(start, encap);
    }
    Segment { start, end, encap, explicit: true, pages: 0 }
}

fn single<'e>(entry: &'e Entry, encap: &'e [u8]) -> Segment<'e> {
    Segment { start: entry, end: entry, encap, explicit: false, pages: 1 }
}

/// Renders the page list of one index item as printable tokens.
fn page_tokens(writer: &mut Writer, entries: &[&Entry]) -> Vec<Vec<u8>> {
    let style = writer.layout.style;
    let count = entries.len();
    for pair in entries.windows(2) {
        if pair[0].page.compare(&pair[1].page).is_eq() && pair[0].encap != pair[1].encap {
            writer.warn(pair[1], "Conflicting entries: multiple encaps for the same page under same key.");
        }
    }
    let open_mark = char::from(style.range_open);
    let close_mark = char::from(style.range_close);

    // Explicit ranges first: each becomes one segment.
    let mut segments: Vec<Segment> = Vec::new();
    let mut at = 0;
    while at < count {
        let entry = entries[at];
        match entry.range {
            Range::Open => {
                let mut start = at;
                // Pages are checked against the latest opener, which an extra
                // opening operator replaces without changing the printed start.
                let mut anchor = at;
                let mut last = at;
                let mut closed = false;
                let mut next = at + 1;
                while next < count {
                    let inner = entries[next];
                    if inner.range == Range::Open {
                        writer.warn(inner, &format!("Extra range opening operator {open_mark}."));
                        anchor = next;
                        next += 1;
                        continue;
                    }
                    if let Some(problem) = range_problem(&entries[anchor].page, &inner.page) {
                        // The range so far ends; the offending page begins a new one.
                        writer.warn(inner, problem);
                        segments.push(explicit_segment(entries[start], entries[last], &entries[start].encap));
                        start = next;
                        anchor = next;
                        last = next;
                        next += 1;
                        if inner.range == Range::Close {
                            closed = true;
                            break;
                        }
                        continue;
                    }
                    match inner.range {
                        Range::Close => {
                            if !inner.encap.is_empty() && inner.encap != entries[start].encap {
                                writer.warn(
                                    inner,
                                    &format!(
                                        "Range closing operator has an inconsistent encapsulator {}.",
                                        String::from_utf8_lossy(&inner.encap)
                                    ),
                                );
                            }
                            last = next;
                            closed = true;
                            next += 1;
                            break;
                        }
                        Range::Open => unreachable!("handled above"),
                        Range::None => {
                            if !inner.encap.is_empty() && inner.encap != entries[start].encap {
                                // A differently emphasized page stays a page of its own.
                                writer.warn(
                                    inner,
                                    &format!(
                                        "Inconsistent page encapsulator {} within range.",
                                        String::from_utf8_lossy(&inner.encap)
                                    ),
                                );
                                segments.push(single(inner, &inner.encap));
                            }
                            last = next;
                        }
                    }
                    next += 1;
                }
                if !closed {
                    writer.warn(entry, &format!("Unmatched range opening operator {open_mark}."));
                }
                segments.push(explicit_segment(entries[start], entries[last], &entries[start].encap));
                at = next;
            }
            Range::Close => {
                writer.warn(entry, &format!("Unmatched range closing operator {close_mark}."));
                segments.push(single(entry, &entry.encap));
                at += 1;
            }
            Range::None => {
                segments.push(single(entry, &entry.encap));
                at += 1;
            }
        }
    }

    // Consecutive segments with the same encapsulator form one run.
    let mut runs: Vec<Segment> = Vec::new();
    for segment in segments {
        if let Some(run) = runs.last_mut() {
            let same_page = run.end.page.compare(&segment.start.page).is_eq();
            let follows = !writer.layout.no_ranges
                && run.end.page.precedes_consecutively(&segment.start.page);
            if run.encap == segment.encap && (same_page || follows) {
                run.end = segment.end;
                run.explicit |= segment.explicit;
                run.pages += segment.pages;
                continue;
            }
        }
        runs.push(segment);
    }

    runs.iter()
        .map(|run| {
            let first = &run.start.page.text;
            let last = &run.end.page.text;
            let covered = run.start.page.pages_until(&run.end.page);
            let text = if first == last {
                first.clone()
            } else {
                match covered {
                    Some(2) if !style.suffix_2p.is_empty() => join(&[first, &style.suffix_2p]),
                    Some(2) if !run.explicit => join(&[first, &style.delim_n, last]),
                    Some(3) if !style.suffix_3p.is_empty() => join(&[first, &style.suffix_3p]),
                    Some(1..=3) => join(&[first, &style.delim_r, last]),
                    _ if !style.suffix_mp.is_empty() => join(&[first, &style.suffix_mp]),
                    _ => join(&[first, &style.delim_r, last]),
                }
            };
            wrap_encap(style, run.encap, &text)
        })
        .collect()
}

fn range_problem(first: &Page, last: &Page) -> Option<&'static str> {
    if first.fields.len() != last.fields.len()
        || first.fields.iter().zip(&last.fields).any(|(a, b)| a.kind != b.kind)
    {
        return Some("Illegal range formation: starting & ending pages are of different types.");
    }
    let leading = first.fields.len() - 1;
    if first.fields[..leading] != last.fields[..leading] {
        return Some("Illegal range formation: starting & ending pages cross chap/sec breaks.");
    }
    None
}

/// An item: consecutive entries with identical keys.
fn same_item(a: &Entry, b: &Entry) -> bool {
    a.levels == b.levels && (0..a.levels).all(|l| a.sf[l] == b.sf[l] && a.af[l] == b.af[l])
}

/// Number of leading levels where the two items print identically.
fn common_levels(prev: &Entry, current: &Entry) -> usize {
    let limit = prev.levels.min(current.levels);
    (0..limit)
        .take_while(|&l| prev.sf[l] == current.sf[l] && prev.af[l] == current.af[l])
        .count()
}

pub(crate) fn generate(
    entries: &[Entry],
    layout: &Layout,
    log: &mut dyn FnMut(String),
) -> Output {
    let style = layout.style;
    let mut writer = Writer { bytes: Vec::new(), column: 0, lines: 0, warnings: 0, layout, log };
    writer.push(&style.preamble);
    if let Some(page) = layout.start_page {
        writer.push(&style.setpage_prefix);
        writer.push(page);
        writer.push(&style.setpage_suffix);
    }
    let mut previous: Option<(&Entry, usize)> = None; // first entry of the previous item, its entry count
    let mut previous_group: Option<Group> = None;
    let mut start = 0;
    while start < entries.len() {
        let mut end = start + 1;
        while end < entries.len() && same_item(&entries[start], &entries[end]) {
            end += 1;
        }
        let item: Vec<&Entry> = entries[start..end].iter().collect();
        let head = item[0];
        start = end;

        let group = group_of(&head.sf[0]);
        if previous_group != Some(group) {
            if previous_group.is_some() {
                writer.push(&style.group_skip);
            }
            if style.headings_flag != 0 {
                let positive = style.headings_flag > 0;
                let text: Vec<u8> = match group {
                    Group::Symbols if positive => style.symhead_positive.clone(),
                    Group::Symbols => style.symhead_negative.clone(),
                    Group::Numbers if positive => style.numhead_positive.clone(),
                    Group::Numbers => style.numhead_negative.clone(),
                    Group::Byte(byte) if byte.is_ascii_lowercase() && positive => {
                        vec![byte.to_ascii_uppercase()]
                    }
                    Group::Byte(byte) => vec![byte],
                };
                writer.push(&style.heading_prefix);
                writer.push(&text);
                writer.push(&style.heading_suffix);
            }
            previous_group = Some(group);
        }

        let common = previous.map_or(0, |(prev, _)| common_levels(prev, head));
        for level in common..head.levels {
            let last_level = level + 1 == head.levels;
            let prefix = if level == 0 {
                &style.item_0
            } else if level > common {
                // The line before this one is the parent, printed without pages.
                if level == 1 { &style.item_x1 } else { &style.item_x2 }
            } else {
                match previous {
                    Some((prev, count)) if prev.levels == level => {
                        if count == 1 {
                            if level == 1 { &style.item_01 } else { &style.item_12 }
                        } else if level == 1 {
                            &style.item_1
                        } else {
                            &style.item_2
                        }
                    }
                    _ => {
                        if level == 1 { &style.item_1 } else { &style.item_2 }
                    }
                }
            };
            writer.push(prefix);
            writer.push(if head.af[level].is_empty() { &head.sf[level] } else { &head.af[level] });
            if !last_level {
                continue;
            }
            let delimiter = match level {
                0 => &style.delim_0,
                1 => &style.delim_1,
                _ => &style.delim_2,
            };
            writer.push(delimiter);
            let tokens = page_tokens(&mut writer, &item);
            for (index, token) in tokens.iter().enumerate() {
                if index > 0 {
                    writer.push(&style.delim_n);
                }
                if writer.column + token.len() >= style.line_max {
                    writer.push(b"\n");
                    writer.push(&style.indent_space);
                    // An indented line counts as `indent_length` columns wide.
                    writer.column = style.indent_length + 1;
                }
                writer.push(token);
            }
            writer.push(&style.delim_t);
        }
        previous = Some((head, item.len()));
    }
    writer.push(&style.postamble);
    Output { bytes: writer.bytes, lines: writer.lines, warnings: writer.warnings }
}
