//! Unicode semantics of Biber 2.22's bundled Perl 5.38.2 (Unicode 15.0.0).
//! All membership and mappings come from the PAR tables, not Rust's Unicode version.
#[path = "perl_unicode_tables.rs"]
mod tables;
use std::sync::LazyLock;

/// Property range tables; table `i` is `ranges[starts[i]..starts[i + 1]]`.
struct Ranges { ranges: Vec<(u32, u32)>, starts: Vec<usize> }
static RANGES: LazyLock<Ranges> = LazyLock::new(|| {
    let data = crate::blob::inflate(include_bytes!("perl_unicode_tables.bin"));
    let mut r = crate::blob::Reader::new(&data);
    let count = r.len();
    let (mut ranges, mut starts) = (Vec::new(), Vec::with_capacity(count + 1));
    starts.push(0);
    for _ in 0..count {
        let n = r.len();
        ranges.extend((0..n).map(|_| (r.u32(), r.u32())));
        starts.push(ranges.len());
    }
    Ranges { ranges, starts }
});
fn table(i: usize) -> &'static [(u32, u32)] { &RANGES.ranges[RANGES.starts[i]..RANGES.starts[i + 1]] }

static NAME_DATA: LazyLock<Vec<u8>> = LazyLock::new(|| crate::blob::inflate(include_bytes!("perl_unicode_names.bin")));
/// Sorted (name, value) pairs borrowing from `NAME_DATA`.
static NAMES: LazyLock<Vec<(&'static str, &'static str)>> = LazyLock::new(|| {
    let mut r = crate::blob::Reader::new(&NAME_DATA);
    (0..r.len()).map(|_| (r.str(), r.str())).collect()
});

pub fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {
    let point = c as u32;
    let index = ranges.partition_point(|&(_, end)| end < point);
    ranges.get(index).is_some_and(|&(start, _)| start <= point)
}

pub fn is_word(c: char) -> bool { in_ranges(table(tables::WORD), c) }
pub fn is_space(c: char) -> bool { in_ranges(table(tables::SPACE), c) }
pub fn needs_quoting(c: char) -> bool { in_ranges(table(tables::PERL_QUOTEMETA), c) }

fn alias_index(aliases: &[(&str, usize)], name: &str) -> Option<usize> {
    aliases.binary_search_by(|&(key, _)| key.cmp(name)).ok().map(|i| aliases[i].1)
}

fn normalize_property(name: &str) -> String {
    let mut key: String = name.chars().filter(|c| !c.is_ascii_whitespace() && *c != '_').map(|c| c.to_ascii_lowercase()).collect();
    if let Some(index) = key.find(['=', ':']) {
        let property = key[..index].replace('-', "");
        let canonical = tables::PROPERTY_NAMES.binary_search_by(|&(p, _)| p.cmp(&property)).ok().map(|i| tables::PROPERTY_NAMES[i].1).unwrap_or(&property);
        let value = &key[index + 1..];
        let value = if canonical == "nv" { value.to_owned() } else { value.replace('-', "") };
        key = format!("{canonical}={value}");
    } else {
        key.retain(|c| c != '-');
    }
    key
}

pub fn property_ranges(name: &str) -> Result<&'static [(u32, u32)], String> {
    property_ranges_casei(name, false)
}

pub fn property_ranges_casei(name: &str, casei: bool) -> Result<&'static [(u32, u32)], String> {
    // Internal Perl extensions deliberately use stricter underscore matching.
    let strict = name.trim().to_ascii_lowercase();
    let key = normalize_property(name);
    let index = if casei { alias_index(tables::CASE_PROPERTIES, &key) } else { None }
        .or_else(|| alias_index(tables::PROPERTIES, &strict))
        .or_else(|| alias_index(tables::PROPERTIES, &key));
    index.map(table).ok_or_else(|| format!("Unknown Perl Unicode property {name:?}"))
}

pub fn property_contains(name: &str, c: char) -> Result<bool, String> {
    Ok(in_ranges(property_ranges(name)?, c))
}

/// All nonidentity full folds, sorted by source character. Compile class closures
/// over these entries instead of enumerating every Unicode scalar in a range.
pub fn nonidentity_folds() -> &'static [(char, &'static [char])] { tables::CF }

fn mapping(table: &'static [(char, &'static [char])], c: char) -> Option<&'static [char]> {
    table.binary_search_by_key(&c, |&(source, _)| source).ok().map(|i| table[i].1)
}

#[derive(Clone)]
pub struct FoldChars {
    mapped: std::slice::Iter<'static, char>,
    identity: Option<char>,
}

impl Iterator for FoldChars {
    type Item = char;
    fn next(&mut self) -> Option<char> { self.identity.take().or_else(|| self.mapped.next().copied()) }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let size = self.mapped.len() + usize::from(self.identity.is_some());
        (size, Some(size))
    }
}
impl ExactSizeIterator for FoldChars {}

fn case_chars(table: &'static [(char, &'static [char])], c: char) -> FoldChars {
    match mapping(table, c) {
        Some(chars) => FoldChars { mapped: chars.iter(), identity: None },
        None => FoldChars { mapped: [].iter(), identity: Some(c) },
    }
}
pub fn fold_chars(c: char) -> FoldChars { case_chars(tables::CF, c) }
pub fn upper_chars(c: char) -> FoldChars { case_chars(tables::UC, c) }
pub fn lower_chars(c: char) -> FoldChars { case_chars(tables::LC, c) }
pub fn title_chars(c: char) -> FoldChars { case_chars(tables::TC, c) }

pub fn append_fold(out: &mut String, c: char) { out.extend(fold_chars(c)); }
pub fn append_upper(out: &mut String, c: char) { out.extend(upper_chars(c)); }
pub fn append_lower(out: &mut String, c: char) { out.extend(lower_chars(c)); }
pub fn append_title(out: &mut String, c: char) { out.extend(title_chars(c)); }
pub fn full_fold(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() { append_fold(&mut out, c); }
    out
}

/// Match a complete folded literal or backreference at a UTF-8 byte boundary.
/// Never accept half of a source character's fold ("s" must not match "ß").
/// Neither side is copied or allocated; the result is a byte endpoint.
pub fn match_folded_restricted(literal: &str, haystack: &str, start: usize, ascii_restrict: bool) -> Option<usize> {
    match_folded_mode(literal,haystack,start,ascii_restrict,false)
}
/// C/POSIX /li uses native ASCII folds below 256, Unicode above 255,
/// and prohibits a case-insensitive match across that locale boundary.
pub fn match_folded_locale(literal:&str,haystack:&str,start:usize)->Option<usize> {
    match_folded_mode(literal,haystack,start,false,true)
}
fn locale_fold(c:char,locale:bool)->FoldChars {
    if locale&&c as u32<=255 {FoldChars {mapped:[].iter(),identity:Some(c.to_ascii_lowercase())}}
    else{fold_chars(c)}
}
fn match_folded_mode(literal:&str,haystack:&str,start:usize,ascii_restrict:bool,locale:bool)->Option<usize> {
    let suffix = haystack.get(start..)?;
    let mut target = suffix.char_indices();
    let mut folded_target = FoldChars { mapped: [].iter(), identity: None };
    let mut target_region = false;
    let mut end = start;
    for source in literal.chars() {
        for wanted in locale_fold(source,locale) {
            let actual = match folded_target.next() {
                Some(c) => c,
                None => {
                    let (offset, c) = target.next()?;
                    target_region = if locale {c as u32<=255}else{c.is_ascii()};
                    end = start + offset + c.len_utf8();
                    folded_target = locale_fold(c,locale);
                    folded_target.next()?
                }
            };
            let source_region=if locale {source as u32<=255}else{source.is_ascii()};
            if actual != wanted || ((ascii_restrict||locale) && source_region != target_region) { return None; }
        }
    }
    if folded_target.len() == 0 { Some(end) } else { None }
}

pub fn named_sequence(name: &str) -> Option<&'static str> {
    NAMES.binary_search_by(|&(key, _)| key.cmp(name)).ok().map(|i| NAMES[i].1)
}

pub fn named_char(name: &str) -> Option<char> {
    if let Some(sequence) = named_sequence(name) {
        let mut chars = sequence.chars();
        let c = chars.next()?;
        return chars.next().is_none().then_some(c);
    }
    let (base, hex) = name.rsplit_once('-')?;
    if !(4..=6).contains(&hex.len()) || !hex.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)) { return None; }
    let point = u32::from_str_radix(hex, 16).ok()?;
    let c = char::from_u32(point)?;
    tables::ALGORITHMIC_NAMES.iter().any(|&(prefix, ranges)| prefix == base && in_ranges(ranges, c)).then_some(c)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gcb { Other, Control, Cr, Lf, Extend, Zwj, Ri, Prepend, SpacingMark, L, V, T, Lv, Lvt }
fn gcb(c: char) -> Gcb {
    use Gcb::*;
    for (kind, i) in [
        (Control, tables::GCB_CONTROL), (Cr, tables::GCB_CR), (Lf, tables::GCB_LF),
        (Extend, tables::GCB_EXTEND), (Zwj, tables::GCB_ZWJ), (Ri, tables::GCB_REGIONALINDICATOR),
        (Prepend, tables::GCB_PREPEND), (SpacingMark, tables::GCB_SPACINGMARK),
        (L, tables::GCB_L), (V, tables::GCB_V), (T, tables::GCB_T), (Lv, tables::GCB_LV), (Lvt, tables::GCB_LVT),
    ] {
        if in_ranges(table(i), c) { return kind; }
    }
    Other
}

/// Perl \X: Unicode 15.0 extended grapheme cluster, including GB11 emoji ZWJ
/// and odd/even regional-indicator pairing. Matching starts a fresh cluster.
pub fn grapheme_end(value: &str, start: usize) -> Option<usize> {
    use Gcb::*;
    let mut chars = value.get(start..)?.char_indices();
    let (_, first) = chars.next()?;
    let mut previous = gcb(first);
    let mut end = start + first.len_utf8();
    let mut ri_count = usize::from(previous == Ri);
    let mut ep_extend = in_ranges(table(tables::EXTPICT), first);
    let mut zwj_after_ep = false;
    for (offset, c) in chars {
        let next = gcb(c);
        let pictographic = in_ranges(table(tables::EXTPICT), c);
        let joined = if previous == Cr && next == Lf { true }
            else if matches!(previous, Control | Cr | Lf) || matches!(next, Control | Cr | Lf) { false }
            else { (previous == L && matches!(next, L | V | Lv | Lvt))
                || (matches!(previous, Lv | V) && matches!(next, V | T))
                || (matches!(previous, Lvt | T) && next == T)
                || matches!(next, Extend | Zwj | SpacingMark)
                || previous == Prepend
                || (previous == Zwj && zwj_after_ep && pictographic)
                || (previous == Ri && next == Ri && ri_count % 2 == 1) };
        if !joined { break; }
        zwj_after_ep = next == Zwj && ep_extend;
        ep_extend = pictographic || (next == Extend && ep_extend);
        ri_count = if next == Ri { ri_count + 1 } else { 0 };
        previous = next;
        end = start + offset + c.len_utf8();
    }
    Some(end)
}

/// Successive Perl `\X` clusters of `value`; also `split /\b{gcb}/`.
pub fn graphemes(value: &str) -> impl Iterator<Item = &str> {
    let mut start = 0;
    std::iter::from_fn(move || {
        let end = grapheme_end(value, start)?;
        let cluster = &value[start..end];
        start = end;
        Some(cluster)
    })
}

fn break_value(table: &[(u32, u32, &'static str)], c: char) -> &'static str {
    let point = c as u32;
    let i = table.partition_point(|&(_, end, _)| end < point);
    table.get(i).filter(|&&(start, _, _)| start <= point).map_or("Other", |&(_, _, kind)| kind)
}
fn wb(c: char) -> &'static str { break_value(tables::WB_RANGES, c) }
fn sb(c: char) -> &'static str { break_value(tables::SB_RANGES, c) }
fn wb_ignored(kind: &str) -> bool { matches!(kind, "Extend" | "Format" | "ZWJ") }
fn sb_ignored(kind: &str) -> bool { matches!(kind, "Extend" | "Format") }
fn wb_letter(kind: &str) -> bool { matches!(kind, "ALetter" | "ExtPict_LE" | "Hebrew_Letter") }
fn wb_space(kind: &str) -> bool { matches!(kind, "CR" | "LF" | "Newline" | "Perl_Tailored_HSpace") }

fn word_boundary(value: &str, pos: usize) -> bool {
    if value.is_empty() { return false; }
    if pos == 0 || pos == value.len() { return true; }
    let mut left = value[..pos].chars().rev();
    let mut right = value[pos..].chars();
    let mut before = wb(left.next().unwrap());
    let after = wb(right.next().unwrap());
    // Perl's WB3 tailoring keeps ALL adjacent whitespace together, including
    // newline combinations; an HSpace before HSpace+Extend is the exception.
    if wb_space(before) && wb_space(after) {
        return before == "Perl_Tailored_HSpace" && after == "Perl_Tailored_HSpace"
            && right.next().is_some_and(|c| matches!(wb(c), "Extend" | "Format"));
    }
    if before == "ZWJ" && matches!(after, "ExtPict_XX" | "ExtPict_LE") { return false; }
    if wb_space(before) || wb_space(after) {
        if before == "Perl_Tailored_HSpace" && wb_ignored(after) { return false; }
        return true;
    }
    if wb_ignored(after) { return false; }
    if wb_ignored(before) {
        before = left.find_map(|c| (!wb_ignored(wb(c))).then(|| wb(c))).unwrap_or("EDGE");
        if before == "EDGE" { return true; }
        if wb_space(before) { return true; }
    }
    if wb_letter(before) && wb_letter(after) { return false; }
    if before == "Hebrew_Letter" && after == "Single_Quote" { return false; }
    let previous = || left.clone().find_map(|c| (!wb_ignored(wb(c))).then(|| wb(c))).unwrap_or("EDGE");
    let next = || right.clone().find_map(|c| (!matches!(wb(c), "Extend" | "Format")).then(|| wb(c))).unwrap_or("EDGE");
    if before == "Hebrew_Letter" && after == "Double_Quote" && next() == "Hebrew_Letter" { return false; }
    if before == "Double_Quote" && after == "Hebrew_Letter" && previous() == "Hebrew_Letter" { return false; }
    if wb_letter(before) && matches!(after, "MidLetter" | "MidNumLet" | "Single_Quote") && wb_letter(next()) { return false; }
    if matches!(before, "MidLetter" | "MidNumLet" | "Single_Quote") && wb_letter(after) && wb_letter(previous()) { return false; }
    if before == "Numeric" && matches!(after, "MidNum" | "MidNumLet" | "Single_Quote") && next() == "Numeric" { return false; }
    if matches!(before, "MidNum" | "MidNumLet" | "Single_Quote") && after == "Numeric" && previous() == "Numeric" { return false; }
    if (before == "Numeric" && (after == "Numeric" || wb_letter(after)))
        || (wb_letter(before) && after == "Numeric") || (before == "Katakana" && after == "Katakana") { return false; }
    if (after == "ExtendNumLet" && (wb_letter(before) || matches!(before, "Numeric" | "Katakana" | "ExtendNumLet")))
        || (before == "ExtendNumLet" && (wb_letter(after) || matches!(after, "Numeric" | "Katakana"))) { return false; }
    if before == "Regional_Indicator" && after == "Regional_Indicator" {
        let preceding = left.filter(|&c| !wb_ignored(wb(c))).take_while(|&c| wb(c) == "Regional_Indicator").count();
        return (preceding + 1) % 2 == 0;
    }
    true
}

fn sentence_boundary(value: &str, pos: usize) -> bool {
    if value.is_empty() { return false; }
    if pos == 0 || pos == value.len() { return true; }
    let mut left = value[..pos].chars().rev();
    let mut before = sb(left.next().unwrap());
    let after = sb(value[pos..].chars().next().unwrap());
    if before == "CR" && after == "LF" { return false; }
    if matches!(before, "Sep" | "CR" | "LF") { return true; }
    if sb_ignored(after) { return false; }
    if sb_ignored(before) {
        let mut backup = left.clone();
        let prior = backup.find_map(|c| (!sb_ignored(sb(c))).then(|| sb(c))).unwrap_or("EDGE");
        if !matches!(prior, "EDGE" | "Sep" | "CR" | "LF") { before = prior; left = backup; }
    }
    if before == "ATerm" && after == "Numeric" { return false; }
    if before == "ATerm" && after == "Upper" && left.clone().find_map(|c| (!sb_ignored(sb(c))).then(|| sb(c))).is_some_and(|kind| matches!(kind, "Upper" | "Lower")) { return false; }
    let mut has_space = false;
    while before == "Sp" {
        has_space = true;
        before = left.find_map(|c| (!sb_ignored(sb(c))).then(|| sb(c))).unwrap_or("EDGE");
    }
    while before == "Close" { before = left.find_map(|c| (!sb_ignored(sb(c))).then(|| sb(c))).unwrap_or("EDGE"); }
    if !matches!(before, "STerm" | "ATerm") { return false; }
    if before == "ATerm" {
        let later = value[pos..].chars().map(sb).find(|kind| matches!(*kind, "OLetter" | "Upper" | "Lower" | "Sep" | "CR" | "LF" | "STerm" | "ATerm"));
        if later == Some("Lower") { return false; }
    }
    if matches!(after, "SContinue" | "STerm" | "ATerm") { return false; }
    if !has_space && matches!(after, "Close" | "Sp" | "Sep" | "CR" | "LF") { return false; }
    !matches!(after, "Sp" | "Sep" | "CR" | "LF")
}

pub fn is_boundary(kind: &str, value: &str, pos: usize) -> Result<bool, String> {
    if !value.is_char_boundary(pos) { return Ok(false); }
    match kind {
        "wb" => Ok(word_boundary(value, pos)),
        "sb" => Ok(sentence_boundary(value, pos)),
        "gcb" | "g" => {
            if value.is_empty() { return Ok(false); }
            if pos == 0 || pos == value.len() { return Ok(true); }
            let mut start = 0;
            while let Some(end) = grapheme_end(value, start) {
                if end >= pos { return Ok(end == pos); }
                start = end;
            }
            Ok(false)
        }
        _ => Err(format!("Unknown Perl Unicode boundary {kind:?}")),
    }
}

/// Perl's tailored Script_Extensions intersection (Han/Jpan/Kore/Hanb included),
/// with one decimal digit set and the single-unknown-codepoint exception.
pub fn is_script_run(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else { return true; };
    if chars.clone().next().is_none() { return true; }
    let mut possible = [u64::MAX; 4];
    let mut zero = None;
    for c in std::iter::once(first).chain(chars) {
        let point = c as u32;
        let i = tables::SCRIPT_RANGES.partition_point(|&(_, end, _, _)| end < point);
        let Some(&(start, _, kind, mask)) = tables::SCRIPT_RANGES.get(i) else { return false; };
        if start > point || kind == 0 { return false; }
        if kind == 2 {
            for i in 0..4 { possible[i] &= mask[i]; }
            if possible == [0; 4] { return false; }
        }
        let digit = table(tables::DIGIT);
        let i = digit.partition_point(|&(_, end)| end < point);
        if let Some(&(start, _)) = digit.get(i) {
            if start <= point {
                if zero.is_some_and(|old| old != start) { return false; }
                zero = Some(start);
            }
        }
    }
    true
}
