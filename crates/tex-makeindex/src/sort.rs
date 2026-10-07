//! The ordering of index entries: makeindex's `sortid.c` and `qsort.c`.

use crate::locale::Locale;
use crate::scan::{group_type, Entry, DUPLICATE, FIELD_MAX, SYMBOL};

/// One progress dot per this many comparisons.
pub(crate) const CMP_MAX: usize = 1500;

/// How keys compare.
pub(crate) struct Collation<'l> {
    /// `-l`: blanks do not count.
    pub(crate) letter_ordering: bool,
    /// `-g`: German ordering.
    pub(crate) german_sort: bool,
    /// `-L`/`-T`: the environment's collation (`strcoll`).
    pub(crate) locale: Option<&'l Locale>,
    pub(crate) range_open: u8,
    pub(crate) range_close: u8,
}

/// `TOLOWER` in the "C" locale.
fn to_lower(byte: u8) -> i32 {
    i32::from(byte.to_ascii_lowercase())
}

fn at(text: &[u8], index: usize) -> u8 {
    text.get(index).copied().unwrap_or(0)
}

/// A string in a zeroed buffer of `cap` bytes.
#[derive(Clone, Copy)]
pub(crate) struct Buffer<'a> {
    pub(crate) text: &'a [u8],
    pub(crate) cap: usize,
}

impl Buffer<'_> {
    /// The byte at `index`, also past the terminator. `compare_string` with
    /// `-l` can step over the terminator of both strings and read beyond
    /// the buffer, into what glibc's malloc left there: in a reused small
    /// chunk the first bytes hold a heap pointer (non-zero in the low five
    /// bytes), the rest are zero.
    fn at(&self, index: usize) -> u8 {
        match self.text.get(index) {
            Some(&byte) => byte,
            None if index < self.cap || index >= 5 => 0,
            None => 0x55,
        }
    }
}

/// `strcmp`, with bytes compared unsigned.
fn strcmp(a: &[u8], b: &[u8]) -> i32 {
    let mut index = 0;
    loop {
        let (x, y) = (at(a, index), at(b, index));
        if x != y || x == 0 {
            return i32::from(x) - i32::from(y);
        }
        index += 1;
    }
}

impl Collation<'_> {
    fn strcoll(&self, a: &[u8], b: &[u8]) -> i32 {
        match self.locale {
            Some(locale) => locale.strcoll(a, b),
            None => strcmp(a, b),
        }
    }

    /// `compare_one`: two sort keys or two printed texts.
    fn compare_one(&self, x: Buffer, y: Buffer) -> i32 {
        let (bx, by) = (x, y);
        let (x, y) = (x.text, y.text);
        match (x.is_empty(), y.is_empty()) {
            (true, true) => return 0,
            (true, false) => return -1,
            (false, true) => return 1,
            _ => {}
        }
        let m = group_type(x);
        let n = group_type(y);
        if m >= 0 && n >= 0 {
            return m.wrapping_sub(n);
        }
        if m >= 0 {
            return if self.german_sort || n == SYMBOL { 1 } else { -1 };
        }
        if n >= 0 {
            return if self.german_sort || m == SYMBOL { -1 } else { 1 };
        }
        if m == SYMBOL && n == SYMBOL {
            return self.check_mixsym(x, y);
        }
        if m == SYMBOL {
            return -1;
        }
        if n == SYMBOL {
            return 1;
        }
        self.compare_string(bx, by)
    }

    /// `check_mixsym`: keys starting with a digit follow other symbols.
    fn check_mixsym(&self, x: &[u8], y: &[u8]) -> i32 {
        let m = at(x, 0).is_ascii_digit();
        let n = at(y, 0).is_ascii_digit();
        if m && !n {
            return 1;
        }
        if !m && n {
            return -1;
        }
        self.strcoll(x, y)
    }

    /// `compare_string`: case-insensitive first, then exact.
    pub(crate) fn compare_string(&self, x: Buffer, y: Buffer) -> i32 {
        let (a, b) = (x.text, y.text);
        if self.locale.is_some() {
            return self.strcoll(a, b);
        }
        let (mut i, mut j) = (0usize, 0usize);
        while x.at(i) != 0 || y.at(j) != 0 {
            if x.at(i) == 0 {
                return -1;
            }
            if y.at(j) == 0 {
                return 1;
            }
            if self.letter_ordering {
                if x.at(i) == b' ' {
                    i += 1;
                }
                if y.at(j) == b' ' {
                    j += 1;
                }
            }
            let (al, bl) = (to_lower(x.at(i)), to_lower(y.at(j)));
            if al != bl {
                return al - bl;
            }
            i += 1;
            j += 1;
        }
        if self.german_sort { new_strcmp_german(a, b) } else { strcmp(a, b) }
    }

    fn is_range(&self, encap: &[u8]) -> bool {
        let first = at(encap, 0);
        first == self.range_open || first == self.range_close
    }

    /// `compare`: keys level by level, then pages. Marks the second of two
    /// identical entries `DUPLICATE`.
    pub(crate) fn compare(&self, entries: &mut [Entry], a: usize, b: usize) -> i32 {
        for level in 0..FIELD_MAX {
            let (x, y) = (&entries[a], &entries[b]);
            let dif = self.compare_one(
                Buffer { text: &x.sf[level], cap: x.sf_cap[level] },
                Buffer { text: &y.sf[level], cap: y.sf_cap[level] },
            );
            if dif != 0 {
                return dif;
            }
            let dif = self.compare_one(
                Buffer { text: &x.af[level], cap: x.af_cap[level] },
                Buffer { text: &y.af[level], cap: y.af_cap[level] },
            );
            if dif != 0 {
                return dif;
            }
        }
        self.compare_page(entries, a, b)
    }

    /// `compare_page`.
    fn compare_page(&self, entries: &mut [Entry], a: usize, b: usize) -> i32 {
        let (x, y) = (&entries[a], &entries[b]);
        let (count_a, count_b) = (x.npg.len(), y.npg.len());
        let mut m = 0i32;
        let mut i = 0usize;
        while i < count_a && i < count_b {
            m = x.npg[i].wrapping_sub(y.npg[i]);
            if m != 0 {
                break;
            }
            i += 1;
        }
        if m != 0 {
            return m;
        }
        if i == count_a && i == count_b {
            // Identical pages: the input order decides between range
            // operators (so that a range ending and one starting on the same
            // page stay apart), the encapsulator between other entries.
            if self.is_range(&x.encap) && self.is_range(&y.encap) {
                m = x.lc.wrapping_sub(y.lc);
            } else if x.encap == y.encap {
                if x.ty != DUPLICATE && y.ty != DUPLICATE {
                    entries[b].ty = DUPLICATE;
                }
            } else if self.is_range(&x.encap) || self.is_range(&y.encap) {
                m = x.lc.wrapping_sub(y.lc);
            } else {
                m = self.compare_string(
                    Buffer { text: &x.encap, cap: x.encap_cap },
                    Buffer { text: &y.encap, cap: y.encap_cap },
                );
            }
        } else if i == count_a && i < count_b {
            m = -1;
        } else if i < count_a && i == count_b {
            m = 1;
        }
        m
    }
}

/// `new_strcmp(..., GERMAN)`: of two keys that differ only in case, the one
/// with the upper-case letter comes second.
fn new_strcmp_german(a: &[u8], b: &[u8]) -> i32 {
    let mut index = 0;
    while at(a, index) == at(b, index) {
        if at(a, index) == 0 {
            return 0;
        }
        index += 1;
    }
    if at(a, index).is_ascii_uppercase() { 1 } else { -1 }
}

/// `qqsort`: Nelson Beebe's quicksort, whose exact sequence of comparisons
/// decides which of two identical entries is dropped and how many
/// comparisons the transcript reports. `cmp` receives two elements of `keys`.
pub(crate) fn qqsort(keys: &mut [usize], cmp: &mut dyn FnMut(usize, usize) -> i32) {
    const THRESH: isize = 4;
    let n = keys.len() as isize;
    if n <= 1 {
        return;
    }
    let max = n;
    let hi = if n >= THRESH {
        qst(keys, 0, max, cmp);
        THRESH
    } else {
        max
    };
    let k = |index: isize| index as usize;
    // The smallest of the first THRESH elements becomes a sentinel.
    let mut j = 0;
    let mut lo = 0;
    loop {
        lo += 1;
        if lo >= hi {
            break;
        }
        if cmp(keys[k(j)], keys[k(lo)]) > 0 {
            j = lo;
        }
    }
    if j != 0 {
        keys.swap(0, k(j));
    }
    // Insertion sort.
    let mut min = 0;
    loop {
        min += 1;
        if min >= max {
            break;
        }
        let mut hi = min;
        loop {
            hi -= 1;
            if hi < 0 || cmp(keys[k(hi)], keys[k(min)]) <= 0 {
                break;
            }
        }
        hi += 1;
        if hi != min {
            keys[k(hi)..=k(min)].rotate_right(1);
        }
    }
}

/// `qst`: partitions `keys[base..max]` until the pieces are below the
/// insertion threshold.
fn qst(keys: &mut [usize], mut base: isize, mut max: isize, cmp: &mut dyn FnMut(usize, usize) -> i32) {
    const THRESH: isize = 4;
    const MTHRESH: isize = 6;
    let k = |index: isize| index as usize;
    let mut lo = max - base;
    loop {
        let mut mid = base + (lo >> 1);
        let mut i = mid;
        if lo >= MTHRESH {
            let jj = base;
            let mut j = if cmp(keys[k(jj)], keys[k(i)]) > 0 { jj } else { i };
            let tmp = max - 1;
            if cmp(keys[k(j)], keys[k(tmp)]) > 0 {
                j = if j == jj { i } else { jj };
                if cmp(keys[k(j)], keys[k(tmp)]) < 0 {
                    j = tmp;
                }
            }
            if j != i {
                keys.swap(k(i), k(j));
            }
        }
        i = base;
        let mut j = max - 1;
        loop {
            while i < mid && cmp(keys[k(i)], keys[k(mid)]) <= 0 {
                i += 1;
            }
            let mut target = None;
            while j > mid {
                if cmp(keys[k(mid)], keys[k(j)]) <= 0 {
                    j -= 1;
                    continue;
                }
                let after = i + 1;
                let jj = j;
                if i == mid {
                    mid = j;
                } else {
                    j -= 1;
                }
                target = Some((jj, after));
                break;
            }
            let (jj, after) = match target {
                Some(found) => found,
                None => {
                    if i == mid {
                        break;
                    }
                    let jj = mid;
                    mid = i;
                    j -= 1;
                    (jj, i)
                }
            };
            keys.swap(k(i), k(jj));
            i = after;
        }
        let j = mid;
        let i = mid + 1;
        let left = j - base;
        let right = max - i;
        if left <= right {
            if left >= THRESH {
                qst(keys, base, j, cmp);
            }
            base = i;
            lo = right;
        } else {
            if right >= THRESH {
                qst(keys, i, max, cmp);
            }
            max = j;
            lo = left;
        }
        if lo < THRESH {
            break;
        }
    }
}
