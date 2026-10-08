//! BibTeX database formatting.
//!
//! Each entry is rewritten as
//!
//! ```text
//! @article{key,
//!   author = {...},
//!   title  = {...},
//! }
//! ```
//!
//! with one field per line, the `=` aligned and a trailing comma, and
//! entries are separated by one blank line. Only blanks between tokens
//! change, which BibTeX and Biber skip: entry types, keys, field names and
//! field values (braces, quotes, `#` concatenations and line breaks inside
//! them) are copied byte for byte. `@string`, `@preamble` and `@comment`,
//! text outside entries, and any entry that does not follow the plain
//! grammar (a `%` line inside it, say) are kept as they are. The result is
//! read back and compared with the input before it is used.

use std::fmt;

use crate::config::Config;

/// An error that leaves a database unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BibError {
    /// Line in the input of the entry that could not be read.
    pub line: usize,
    pub message: &'static str,
}

impl fmt::Display for BibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}: {}; file left unchanged",
            self.line, self.message
        )
    }
}

impl std::error::Error for BibError {}

/// One piece of a database, as positions in the text.
#[derive(Debug, PartialEq, Eq)]
enum Item<'a> {
    /// Text outside entries (BibTeX ignores it, but it is kept).
    Text(&'a str),
    /// `@string`, `@preamble`, `@comment` or an entry kept as written.
    Kept(&'a str),
    Entry(Entry<'a>),
}

#[derive(Debug, PartialEq, Eq)]
struct Entry<'a> {
    kind: &'a str,
    open: u8,
    key: &'a str,
    fields: Vec<(&'a str, &'a str)>,
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

/// Characters of entry types, keys, field names, numbers and macro names.
fn is_ident(c: u8) -> bool {
    !is_space(c)
        && !matches!(
            c,
            b',' | b'#' | b'=' | b'{' | b'}' | b'(' | b')' | b'"' | b'%' | b'@'
        )
}

struct Parser<'a> {
    text: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn skip_space(&mut self) {
        while self.i < self.b.len() && is_space(self.b[self.i]) {
            self.i += 1;
        }
    }

    fn ident(&mut self) -> Option<&'a str> {
        let start = self.i;
        while self.i < self.b.len() && is_ident(self.b[self.i]) {
            self.i += 1;
        }
        (self.i > start).then(|| &self.text[start..self.i])
    }

    /// The end of the `{...}` group starting at `i` (after its `}`).
    fn braced_end(&self, mut i: usize) -> Option<usize> {
        let mut depth = 0usize;
        while i < self.b.len() {
            match self.b[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// The end of the `"..."` string starting at `i` (braces inside must
    /// balance; a `"` inside braces does not end it).
    fn quoted_end(&self, mut i: usize) -> Option<usize> {
        let mut depth = 0usize;
        i += 1;
        while i < self.b.len() {
            match self.b[i] {
                b'{' => depth += 1,
                b'}' => depth = depth.checked_sub(1)?,
                b'"' if depth == 0 => return Some(i + 1),
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// The end of the entry whose delimiter `open` is at `i`, found the way
    /// BibTeX does: the matching `}`, or the first `)` outside braces.
    fn entry_end(&self, i: usize, open: u8) -> Option<usize> {
        if open == b'{' {
            return self.braced_end(i);
        }
        let mut depth = 0usize;
        for (k, &c) in self.b.iter().enumerate().skip(i + 1) {
            match c {
                b'{' => depth += 1,
                b'}' => depth = depth.checked_sub(1)?,
                b')' if depth == 0 => return Some(k + 1),
                _ => {}
            }
        }
        None
    }

    /// A field value: parts joined by `#`, copied as written.
    fn value(&mut self) -> Option<&'a str> {
        let start = self.i;
        loop {
            let end = match self.b.get(self.i)? {
                b'{' => self.braced_end(self.i)?,
                b'"' => self.quoted_end(self.i)?,
                _ => {
                    self.ident()?;
                    self.i
                }
            };
            self.i = end;
            let after = self.i;
            self.skip_space();
            if self.b.get(self.i) != Some(&b'#') {
                self.i = after;
                return Some(&self.text[start..after]);
            }
            self.i += 1;
            self.skip_space();
        }
    }

    /// The fields of an entry after its key, up to and including `close`.
    fn fields(&mut self, close: u8) -> Option<Vec<(&'a str, &'a str)>> {
        let mut fields = Vec::new();
        loop {
            // After the key or a field: `,` or the end.
            self.skip_space();
            match *self.b.get(self.i)? {
                c if c == close => {
                    self.i += 1;
                    return Some(fields);
                }
                b',' => self.i += 1,
                _ => return None,
            }
            self.skip_space();
            if *self.b.get(self.i)? == close {
                self.i += 1;
                return Some(fields);
            }
            let name = self.ident()?;
            self.skip_space();
            if *self.b.get(self.i)? != b'=' {
                return None;
            }
            self.i += 1;
            self.skip_space();
            fields.push((name, self.value()?));
        }
    }

    /// The entry whose `@` is at `self.i`, if it follows the plain grammar.
    fn entry(&mut self, kind: &'a str, open: u8) -> Option<Entry<'a>> {
        let close = if open == b'{' { b'}' } else { b')' };
        self.i += 1;
        self.skip_space();
        let key = self.ident()?;
        let fields = self.fields(close)?;
        (!fields.is_empty()).then_some(Entry {
            kind,
            open,
            key,
            fields,
        })
    }
}

fn line_of(text: &str, pos: usize) -> usize {
    text.as_bytes()[..pos]
        .iter()
        .filter(|&&c| c == b'\n')
        .count()
        + 1
}

/// Splits a database into items the way BibTeX reads it.
fn parse(text: &str) -> Result<Vec<Item<'_>>, BibError> {
    let mut p = Parser {
        text,
        b: text.as_bytes(),
        i: 0,
    };
    let mut items = Vec::new();
    let mut text_start = 0;
    while let Some(off) = p.b[p.i..].iter().position(|&c| c == b'@') {
        let at = p.i + off;
        p.i = at + 1;
        p.skip_space();
        let Some(kind) = p.ident() else {
            continue;
        };
        p.skip_space();
        let open = match p.b.get(p.i) {
            Some(&c @ (b'{' | b'(')) => c,
            // Not an entry: BibTeX skips it as part of the text outside.
            _ => continue,
        };
        let open_at = p.i;
        let Some(end) = p.entry_end(open_at, open) else {
            return Err(BibError {
                line: line_of(text, at),
                message: "this entry does not end",
            });
        };
        if at > text_start {
            items.push(Item::Text(&text[text_start..at]));
        }
        // BibTeX reads `%@article{...}` as an entry, Biber may take the `%`
        // as a comment: such an entry is left as written.
        let line_start = text[..at].rfind('\n').map_or(0, |p| p + 1);
        let commented = text[line_start..at].contains('%');
        let special = ["string", "preamble", "comment"]
            .iter()
            .any(|s| kind.eq_ignore_ascii_case(s));
        let entry = if special || commented {
            None
        } else {
            p.entry(kind, open)
        };
        match entry {
            Some(entry) if p.i == end => items.push(Item::Entry(entry)),
            _ => items.push(Item::Kept(&text[at..end])),
        }
        p.i = end;
        text_start = end;
    }
    if text_start < text.len() {
        items.push(Item::Text(&text[text_start..]));
    }
    Ok(items)
}

fn is_blank_text(s: &str) -> bool {
    s.bytes().all(is_space)
}

/// Formats a BibTeX database. The result is read back and compared with
/// the input; if they differ in anything BibTeX reads, nothing is changed.
pub fn format_bib(source: &str, config: &Config) -> Result<String, BibError> {
    let items = parse(source)?;
    let eol = match source.find('\n') {
        Some(p) if p > 0 && source.as_bytes()[p - 1] == b'\r' => "\r\n",
        _ => "\n",
    };
    let indent = " ".repeat(config.indent_width);
    let mut out = String::with_capacity(source.len() + source.len() / 4);
    let is_entry = |i: Option<&Item<'_>>| matches!(i, Some(Item::Entry(_)));
    for (index, item) in items.iter().enumerate() {
        let prev = index.checked_sub(1).map(|k| &items[k]);
        let next = items.get(index + 1);
        let entry_side = is_entry(prev) || is_entry(Some(item)) || is_entry(next);
        match item {
            Item::Text(t) if is_blank_text(t) => {
                // Next to an entry one blank line; none at the start; a line
                // end at the end; elsewhere (between `@string`s, say) as is.
                if prev.is_none() {
                } else if next.is_none() {
                    out.push_str(eol);
                } else if entry_side {
                    out.push_str(eol);
                    out.push_str(eol);
                } else {
                    out.push_str(t);
                }
                continue;
            }
            Item::Kept(_) | Item::Entry(_)
                if entry_side && prev.is_some_and(|p| !matches!(p, Item::Text(_))) =>
            {
                // Written right after the previous one, on the same line.
                out.push_str(eol);
                out.push_str(eol);
            }
            _ => {}
        }
        match item {
            Item::Text(t) | Item::Kept(t) => out.push_str(t),
            Item::Entry(e) => {
                let close = if e.open == b'{' { '}' } else { ')' };
                out.push('@');
                out.push_str(e.kind);
                out.push(e.open as char);
                out.push_str(e.key);
                out.push(',');
                out.push_str(eol);
                let width = e
                    .fields
                    .iter()
                    .map(|(n, _)| n.chars().count())
                    .max()
                    .unwrap_or(0);
                for (name, value) in &e.fields {
                    out.push_str(&indent);
                    out.push_str(name);
                    out.extend(std::iter::repeat_n(' ', width - name.chars().count()));
                    out.push_str(" = ");
                    out.push_str(value);
                    out.push(',');
                    out.push_str(eol);
                }
                out.push(close);
            }
        }
    }
    if items.last().is_some_and(|i| !matches!(i, Item::Text(_))) {
        out.push_str(eol);
    }
    match first_difference(&items, &out) {
        None => Ok(out),
        Some(line) => Err(BibError {
            line,
            message: "formatting would change how BibTeX reads the file here (please report \
                      this as a texres bug)",
        }),
    }
}

/// Reads `after` back and compares it with the items of the input: entries
/// and kept text must be identical, text outside entries may differ only
/// in blanks. Returns the line in the input where they first differ.
fn first_difference(before: &[Item<'_>], after: &str) -> Option<usize> {
    let Ok(again) = parse(after) else {
        return Some(1);
    };
    let mut line = 1;
    let mut a = before.iter().peekable();
    let mut b = again.iter().peekable();
    loop {
        // Blank text outside entries does not count.
        while a
            .next_if(|i| matches!(i, Item::Text(t) if is_blank_text(t)))
            .is_some()
        {}
        while b
            .next_if(|i| matches!(i, Item::Text(t) if is_blank_text(t)))
            .is_some()
        {}
        match (a.next(), b.next()) {
            (None, None) => return None,
            (Some(x), Some(y)) if x == y => {
                line += match x {
                    Item::Text(t) | Item::Kept(t) => t.matches('\n').count(),
                    Item::Entry(_) => 0,
                };
            }
            _ => return Some(line),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        let once = format_bib(src, &Config::default()).unwrap();
        assert_eq!(format_bib(&once, &Config::default()).unwrap(), once);
        once
    }

    #[test]
    fn entries_get_one_field_per_line() {
        let src = "% my refs\n\n\n@Article{ knuth84 ,author={Donald E. Knuth}, title = \"Literate {P}rogramming\",\n year=1984,\n  journal = cj # \" 27\"}\n@misc(x, note = {a\n   b})";
        let want = "% my refs\n\n\n@Article{knuth84,\n  author  = {Donald E. Knuth},\n  title   = \"Literate {P}rogramming\",\n  year    = 1984,\n  journal = cj # \" 27\",\n}\n\n@misc(x,\n  note = {a\n   b},\n)\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn strings_comments_and_odd_entries_are_kept() {
        let src = "@string{cj = \"Comp.  J.\"}\n@preamble{ \"\\newcommand{\\x}{y}\" }\n@comment{ @article{a, b = c} }\n@article{k,\n% note\n  title = {T}}\n@book{e}\nmail me@example.org\n";
        assert_eq!(fmt(src), src);
        let src = "%@article{x, title={a}}\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn values_are_copied_exactly() {
        let src = "@article{k,\r\n title = {A {B} \"c\"\r\n  d},url={http://x.org/a%20b},\r\n}\r\n";
        let want = "@article{k,\r\n  title = {A {B} \"c\"\r\n  d},\r\n  url   = {http://x.org/a%20b},\r\n}\r\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn unterminated_entries_leave_the_file_alone() {
        let err = format_bib("@article{k,\n title = {x}\n", &Config::default()).unwrap_err();
        assert_eq!(err.line, 1);
    }
}
