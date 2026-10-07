//! Reading `.idx` files: one `\indexentry{key}{page}` per line.

use crate::page::{Page, PageParser};
use crate::style::Style;
use crate::Options;

/// Whether an entry opens or closes an explicit page range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Range {
    Open,
    None,
    Close,
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    /// Sort keys, one per level (at most three).
    pub(crate) sf: [Vec<u8>; 3],
    /// Printed texts, one per level; empty when the sort key is printed.
    pub(crate) af: [Vec<u8>; 3],
    pub(crate) levels: usize,
    pub(crate) encap: Vec<u8>,
    pub(crate) range: Range,
    pub(crate) page: Page,
    /// Input line.
    pub(crate) line: usize,
    pub(crate) file: usize,
}

pub(crate) struct ScanResult {
    pub(crate) accepted: usize,
    pub(crate) rejected: usize,
}

struct Level {
    sf: Vec<u8>,
    af: Vec<u8>,
    has_actual: bool,
}

impl Level {
    fn new() -> Level {
        Level { sf: Vec::new(), af: Vec::new(), has_actual: false }
    }

    fn text(&mut self) -> &mut Vec<u8> {
        if self.has_actual {
            &mut self.af
        } else {
            &mut self.sf
        }
    }
}

struct Key {
    levels: Vec<Level>,
    encap: Vec<u8>,
}

fn is_blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r')
}

/// Scans the first argument starting after its opening delimiter. Returns the
/// parsed key and the index just past the closing delimiter.
fn scan_key(line: &[u8], mut at: usize, style: &Style) -> Result<(Key, usize), String> {
    let origin = at;
    let mut levels = vec![Level::new()];
    let mut encap = Vec::new();
    let mut in_encap = false;
    let mut depth = 0usize;
    let mut quoted = false;
    loop {
        let Some(&byte) = line.get(at) else {
            return Err("Incomplete first argument (premature LFD).".to_string());
        };
        at += 1;
        let push = |levels: &mut Vec<Level>, encap: &mut Vec<u8>, byte: u8| {
            if in_encap {
                encap.push(byte);
            } else {
                levels.last_mut().expect("a level is always open").text().push(byte);
            }
        };
        if quoted {
            push(&mut levels, &mut encap, byte);
            quoted = false;
            continue;
        }
        if byte == style.escape {
            push(&mut levels, &mut encap, byte);
            if line.get(at) == Some(&style.quote) {
                push(&mut levels, &mut encap, style.quote);
                at += 1;
            }
            continue;
        }
        if byte == style.quote {
            quoted = true;
            continue;
        }
        if byte == style.arg_open {
            depth += 1;
            push(&mut levels, &mut encap, byte);
            continue;
        }
        if byte == style.arg_close {
            if depth == 0 {
                break;
            }
            depth -= 1;
            push(&mut levels, &mut encap, byte);
            continue;
        }
        if in_encap {
            push(&mut levels, &mut encap, byte);
            continue;
        }
        if byte == style.level {
            if levels.len() == 3 {
                return Err(format!(
                    "Extra `{}' at position {} of first argument.",
                    char::from(byte),
                    at - origin
                ));
            }
            levels.push(Level::new());
        } else if byte == style.actual && !levels.last().is_some_and(|level| level.has_actual) {
            levels.last_mut().expect("a level is always open").has_actual = true;
        } else if byte == style.encap {
            in_encap = true;
        } else {
            push(&mut levels, &mut encap, byte);
        }
    }
    Ok((Key { levels, encap }, at))
}

fn compress_blanks(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut pending = false;
    for &byte in text {
        if byte == b' ' || byte == b'\t' {
            pending = !out.is_empty();
        } else {
            if pending {
                out.push(b' ');
                pending = false;
            }
            out.push(byte);
        }
    }
    out
}

/// Scans one input file, appending accepted entries.
pub(crate) fn scan_file(
    bytes: &[u8],
    file_index: usize,
    file_name: &str,
    style: &Style,
    options: &Options,
    parser: &mut PageParser,
    entries: &mut Vec<Entry>,
    messages: &mut dyn FnMut(String),
) -> ScanResult {
    let mut result = ScanResult { accepted: 0, rejected: 0 };
    let mut line_number = 0usize;
    for line in bytes.split(|&byte| byte == b'\n') {
        line_number += 1;
        let mut error = |message: &str| {
            messages(format!(
                "!! Input index error (file = {file_name}, line = {line_number}):\n   -- {message}\n"
            ));
        };
        let start = line.iter().position(|&byte| !is_blank(byte)).unwrap_or(line.len());
        if start == line.len() {
            continue;
        }
        // The keyword runs up to the first argument delimiter after its first
        // character.
        let Some(open) = line
            .iter()
            .skip(start + 1)
            .position(|&byte| byte == style.arg_open)
            .map(|index| index + start + 1)
        else {
            error("Missing arguments -- need two (premature LFD).");
            result.rejected += 1;
            continue;
        };
        let mut word = &line[start..open];
        while let Some((&last, head)) = word.split_last() {
            if is_blank(last) {
                word = head;
            } else {
                break;
            }
        }
        if word != style.keyword.as_slice() {
            error(&format!("Unknown index keyword {}.", String::from_utf8_lossy(word)));
            result.rejected += 1;
            continue;
        }
        let (key, after) = match scan_key(line, open + 1, style) {
            Ok(parsed) => parsed,
            Err(message) => {
                error(&message);
                result.rejected += 1;
                continue;
            }
        };
        let mut at = after;
        while line.get(at).is_some_and(|&byte| is_blank(byte)) {
            at += 1;
        }
        match line.get(at) {
            None => {
                error("Missing arguments -- need two (premature LFD).");
                result.rejected += 1;
                continue;
            }
            Some(&byte) if byte != style.arg_open => {
                error(&format!(
                    "No opening delimiter for second argument (illegal character `{}').",
                    char::from(byte)
                ));
                result.rejected += 1;
                continue;
            }
            Some(_) => {}
        }
        at += 1;
        let page_start = at;
        let mut bad = None;
        loop {
            match line.get(at) {
                None => {
                    bad = Some("Incomplete second argument (premature LFD).".to_string());
                    break;
                }
                Some(&byte) if byte == style.arg_close => break,
                Some(&byte) if byte == style.arg_open => {
                    bad = Some(format!(
                        "No closing delimiter for second argument (illegal character `{}').",
                        char::from(byte)
                    ));
                    break;
                }
                Some(_) => at += 1,
            }
        }
        if bad.is_none() {
            if let Some(&byte) = line[at + 1..].iter().find(|&&byte| !is_blank(byte)) {
                bad = Some(format!(
                    "No closing delimiter for second argument (illegal character `{}').",
                    char::from(byte)
                ));
            }
        }
        if let Some(message) = bad {
            error(&message);
            result.rejected += 1;
            continue;
        }
        let raw_page = &line[page_start..at];
        let trimmed: &[u8] = {
            let from = raw_page.iter().position(|&byte| !is_blank(byte)).unwrap_or(raw_page.len());
            let to = raw_page.iter().rposition(|&byte| !is_blank(byte)).map_or(from, |index| index + 1);
            &raw_page[from..to]
        };
        if trimmed.iter().any(|&byte| byte == b' ' || byte == b'\t') {
            error("Illegal space within numerals in second argument.");
            result.rejected += 1;
            continue;
        }
        // Null fields: no level may be empty except a trailing one.
        let mut levels = key.levels;
        if levels.len() > 1
            && levels.last().is_some_and(|level| level.sf.is_empty() && level.af.is_empty())
        {
            levels.pop();
        }
        if levels.iter().any(|level| level.sf.is_empty()) {
            error("Illegal null field.");
            result.rejected += 1;
            continue;
        }
        let page = match parser.parse(trimmed) {
            Ok(page) => page,
            Err(message) => {
                error(&message);
                result.rejected += 1;
                continue;
            }
        };
        let mut entry = Entry {
            sf: Default::default(),
            af: Default::default(),
            levels: levels.len(),
            encap: key.encap,
            range: Range::None,
            page,
            line: line_number,
            file: file_index,
        };
        for (index, level) in levels.into_iter().enumerate() {
            if options.compress_blanks {
                entry.sf[index] = compress_blanks(&level.sf);
                entry.af[index] = compress_blanks(&level.af);
            } else {
                entry.sf[index] = level.sf;
                entry.af[index] = level.af;
            }
        }
        match entry.encap.first() {
            Some(&byte) if byte == style.range_open => {
                entry.range = Range::Open;
                entry.encap.remove(0);
            }
            Some(&byte) if byte == style.range_close => {
                entry.range = Range::Close;
                entry.encap.remove(0);
            }
            _ => {}
        }
        entries.push(entry);
        result.accepted += 1;
    }
    result
}
