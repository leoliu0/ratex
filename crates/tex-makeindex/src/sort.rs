//! The ordering of index entries.

use std::cmp::Ordering;

use crate::scan::Entry;

/// How sort keys are compared.
#[derive(Clone, Copy)]
pub(crate) struct Collation {
    /// `-l`: blanks do not take part in the comparison.
    pub(crate) letter_ordering: bool,
    /// `-g`: quote + letter spells an umlaut.
    pub(crate) german: bool,
    pub(crate) quote: u8,
}

/// Classes of the first byte of a key. Symbols (including keys that start
/// with a digit but are not numbers) come first, then numbers, then bytes up
/// to and including the blank, then letters, then 8-bit bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Rank(u8, u8);

fn rank(key: &[u8]) -> Rank {
    let Some(&first) = key.first() else {
        return Rank(0, 0);
    };
    if key.iter().all(u8::is_ascii_digit) {
        return Rank(1, 0);
    }
    match first {
        b if b.is_ascii_digit() => Rank(0, 2),
        b if b.is_ascii_alphabetic() => Rank(3, b.to_ascii_lowercase()),
        b if b <= b' ' || b == 0x7f => Rank(2, b),
        b if b >= 0x80 => Rank(4, b),
        _ => Rank(0, 1),
    }
}

/// The group an entry belongs to in the output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Group {
    Symbols,
    Numbers,
    /// A letter (lower case), a control byte or blank, or an 8-bit byte.
    Byte(u8),
}

pub(crate) fn group_of(key: &[u8]) -> Group {
    match rank(key) {
        Rank(0, _) => Group::Symbols,
        Rank(1, _) => Group::Numbers,
        Rank(_, byte) => Group::Byte(byte),
    }
}

fn compare_digits(a: &[u8], b: &[u8]) -> Ordering {
    let strip = |digits: &[u8]| -> usize { digits.iter().take_while(|&&d| d == b'0').count() };
    let (a, b) = (&a[strip(a)..], &b[strip(b)..]);
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn compare_text(a: &[u8], b: &[u8], fold: bool, letter_ordering: bool) -> Ordering {
    let (mut i, mut j) = (0usize, 0usize);
    loop {
        if letter_ordering {
            if a.get(i) == Some(&b' ') {
                i += 1;
            }
            if b.get(j) == Some(&b' ') {
                j += 1;
            }
        }
        match (a.get(i), b.get(j)) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&x), Some(&y)) => {
                let (x, y) = if fold { (x.to_ascii_lowercase(), y.to_ascii_lowercase()) } else { (x, y) };
                if x != y {
                    return x.cmp(&y);
                }
            }
        }
        i += 1;
        j += 1;
    }
    a.cmp(b)
}

fn german_form(key: &[u8], quote: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(key.len());
    let mut at = 0;
    while at < key.len() {
        if key[at] == quote {
            match key.get(at + 1) {
                Some(b'a') => out.extend_from_slice(b"ae"),
                Some(b'o') => out.extend_from_slice(b"oe"),
                Some(b'u') => out.extend_from_slice(b"ue"),
                Some(b's') => out.extend_from_slice(b"ss"),
                Some(b'A') => out.extend_from_slice(b"Ae"),
                Some(b'O') => out.extend_from_slice(b"Oe"),
                Some(b'U') => out.extend_from_slice(b"Ue"),
                Some(&other) => out.push(other),
                None => {}
            }
            at += 2;
        } else {
            out.push(key[at]);
            at += 1;
        }
    }
    out
}

pub(crate) fn compare_keys(a: &[u8], b: &[u8], collation: &Collation) -> Ordering {
    if collation.german {
        let (a, b) = (german_form(a, collation.quote), german_form(b, collation.quote));
        return compare_plain(&a, &b, collation);
    }
    compare_plain(a, b, collation)
}

fn compare_plain(a: &[u8], b: &[u8], collation: &Collation) -> Ordering {
    let (rank_a, rank_b) = (rank(a), rank(b));
    if rank_a != rank_b {
        return rank_a.cmp(&rank_b);
    }
    match rank_a.0 {
        1 => compare_digits(a, b),
        0 => compare_text(a, b, false, collation.letter_ordering),
        _ => compare_text(a, b, true, collation.letter_ordering),
    }
}

/// Orders two entries by key at each level, then by page. Entries that tie
/// keep their input order (the sort is stable).
pub(crate) fn compare_entries(a: &Entry, b: &Entry, collation: &Collation) -> Ordering {
    compare_item_keys(a, b, collation).then_with(|| a.page.compare(&b.page))
}

/// Among entries of one page, those that do not open or close a range are
/// ordered by encapsulator; the others keep their places.
pub(crate) fn order_same_page(entries: &mut [Entry], collation: &Collation) {
    let mut start = 0;
    while start < entries.len() {
        let mut end = start + 1;
        while end < entries.len()
            && compare_entries(&entries[start], &entries[end], collation) == Ordering::Equal
        {
            end += 1;
        }
        if end - start > 1 {
            let positions: Vec<usize> = (start..end)
                .filter(|&index| entries[index].range == crate::scan::Range::None)
                .collect();
            let mut plain: Vec<Entry> = positions.iter().map(|&index| entries[index].clone()).collect();
            plain.sort_by(|a, b| a.encap.cmp(&b.encap));
            for (position, entry) in positions.into_iter().zip(plain) {
                entries[position] = entry;
            }
        }
        start = end;
    }
}

pub(crate) fn compare_item_keys(a: &Entry, b: &Entry, collation: &Collation) -> Ordering {
    for level in 0..3 {
        match (level < a.levels, level < b.levels) {
            (false, false) => break,
            (false, true) => return Ordering::Less,
            (true, false) => return Ordering::Greater,
            (true, true) => {}
        }
        let ordering = compare_keys(&a.sf[level], &b.sf[level], collation)
            .then_with(|| compare_keys(&a.af[level], &b.af[level], collation));
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}
