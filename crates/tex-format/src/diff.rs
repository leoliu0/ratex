//! Unified line diffs (Myers' algorithm with linear-space bisection).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// A unified diff (3 lines of context) from `old` to `new`, or an empty
/// string when they are equal.
pub fn unified_diff(old: &str, new: &str, old_name: &str, new_name: &str) -> String {
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    let mut ops = Vec::with_capacity(a.len().max(b.len()));
    diff(&a, &b, 0, 0, &mut ops);
    if ops.iter().all(|op| matches!(op, Op::Equal(..))) {
        return String::new();
    }
    // Within each block of changes, list deletions before insertions.
    let mut i = 0;
    while i < ops.len() {
        if matches!(ops[i], Op::Equal(..)) {
            i += 1;
            continue;
        }
        let end = ops[i..]
            .iter()
            .position(|op| matches!(op, Op::Equal(..)))
            .map_or(ops.len(), |p| i + p);
        ops[i..end].sort_by_key(|op| matches!(op, Op::Insert(_)));
        i = end;
    }
    let mut out = format!("--- {old_name}\n+++ {new_name}\n");
    const CONTEXT: usize = 3;
    let changed: Vec<usize> = (0..ops.len())
        .filter(|&i| !matches!(ops[i], Op::Equal(..)))
        .collect();
    let mut start = 0;
    while start < changed.len() {
        let mut end = start;
        while end + 1 < changed.len() && changed[end + 1] - changed[end] <= 2 * CONTEXT + 1 {
            end += 1;
        }
        let from = changed[start].saturating_sub(CONTEXT);
        let to = (changed[end] + CONTEXT + 1).min(ops.len());
        write_hunk(&mut out, &ops[from..to], &a, &b);
        start = end + 1;
    }
    out
}

fn write_hunk(out: &mut String, ops: &[Op], a: &[&str], b: &[&str]) {
    // Line numbers where the hunk starts on each side.
    let mut old_start = None;
    let mut new_start = None;
    let (mut old_count, mut new_count) = (0, 0);
    for op in ops {
        match *op {
            Op::Equal(i, j) => {
                old_start.get_or_insert(i);
                new_start.get_or_insert(j);
                old_count += 1;
                new_count += 1;
            }
            Op::Delete(i) => {
                old_start.get_or_insert(i);
                old_count += 1;
            }
            Op::Insert(j) => {
                new_start.get_or_insert(j);
                new_count += 1;
            }
        }
    }
    // A side with no lines in the hunk is reported at the line before it.
    let old_pos = position(ops, true);
    let new_pos = position(ops, false);
    let range = |start: Option<usize>, pos: usize, count: usize| match (start, count) {
        (Some(s), 1) => format!("{}", s + 1),
        (Some(s), n) => format!("{},{n}", s + 1),
        (None, _) => format!("{pos},0"),
    };
    out.push_str(&format!(
        "@@ -{} +{} @@\n",
        range(old_start, old_pos, old_count),
        range(new_start, new_pos, new_count)
    ));
    for op in ops {
        let (sign, line) = match *op {
            Op::Equal(i, _) => (' ', a[i]),
            Op::Delete(i) => ('-', a[i]),
            Op::Insert(j) => ('+', b[j]),
        };
        out.push(sign);
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
}

/// For a hunk without lines on one side: the number of lines before it there.
fn position(ops: &[Op], old: bool) -> usize {
    for op in ops {
        match (*op, old) {
            (Op::Equal(i, _), true) | (Op::Delete(i), true) => return i,
            (Op::Equal(_, j), false) | (Op::Insert(j), false) => return j,
            _ => {}
        }
    }
    0
}

fn diff(a: &[&str], b: &[&str], a0: usize, b0: usize, ops: &mut Vec<Op>) {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    ops.extend((0..prefix).map(|i| Op::Equal(a0 + i, b0 + i)));
    let (a, b) = (&a[prefix..], &b[prefix..]);
    let (a0, b0) = (a0 + prefix, b0 + prefix);
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[..a.len() - suffix], &b[..b.len() - suffix]);
    if am.is_empty() {
        ops.extend((0..bm.len()).map(|j| Op::Insert(b0 + j)));
    } else if bm.is_empty() {
        ops.extend((0..am.len()).map(|i| Op::Delete(a0 + i)));
    } else {
        match bisect(am, bm) {
            Some((x, y)) => {
                diff(&am[..x], &bm[..y], a0, b0, ops);
                diff(&am[x..], &bm[y..], a0 + x, b0 + y, ops);
            }
            None => {
                ops.extend((0..am.len()).map(|i| Op::Delete(a0 + i)));
                ops.extend((0..bm.len()).map(|j| Op::Insert(b0 + j)));
            }
        }
    }
    let (ae, be) = (a0 + am.len(), b0 + bm.len());
    ops.extend((0..suffix).map(|i| Op::Equal(ae + i, be + i)));
}

/// Finds the middle snake of the shortest edit script; returns a split point.
fn bisect(a: &[&str], b: &[&str]) -> Option<(usize, usize)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max_d = (n + m + 1) / 2;
    let offset = max_d;
    let len = (2 * max_d + 2) as usize;
    let mut v1 = vec![-1isize; len];
    let mut v2 = vec![-1isize; len];
    v1[(offset + 1) as usize] = 0;
    v2[(offset + 1) as usize] = 0;
    let delta = n - m;
    let front = delta % 2 != 0;
    let (mut k1start, mut k1end, mut k2start, mut k2end) = (0, 0, 0, 0);
    for d in 0..max_d {
        let mut k1 = -d + k1start;
        while k1 <= d - k1end {
            let k1_off = (offset + k1) as usize;
            let mut x1 = if k1 == -d || (k1 != d && v1[k1_off - 1] < v1[k1_off + 1]) {
                v1[k1_off + 1]
            } else {
                v1[k1_off - 1] + 1
            };
            let mut y1 = x1 - k1;
            while x1 < n && y1 < m && a[x1 as usize] == b[y1 as usize] {
                x1 += 1;
                y1 += 1;
            }
            v1[k1_off] = x1;
            if x1 > n {
                k1end += 2;
            } else if y1 > m {
                k1start += 2;
            } else if front {
                let k2_off = offset + delta - k1;
                if k2_off >= 0 && (k2_off as usize) < len && v2[k2_off as usize] != -1 {
                    let x2 = n - v2[k2_off as usize];
                    if x1 >= x2 {
                        return Some((x1 as usize, y1 as usize));
                    }
                }
            }
            k1 += 2;
        }
        let mut k2 = -d + k2start;
        while k2 <= d - k2end {
            let k2_off = (offset + k2) as usize;
            let mut x2 = if k2 == -d || (k2 != d && v2[k2_off - 1] < v2[k2_off + 1]) {
                v2[k2_off + 1]
            } else {
                v2[k2_off - 1] + 1
            };
            let mut y2 = x2 - k2;
            while x2 < n && y2 < m && a[(n - x2 - 1) as usize] == b[(m - y2 - 1) as usize] {
                x2 += 1;
                y2 += 1;
            }
            v2[k2_off] = x2;
            if x2 > n {
                k2end += 2;
            } else if y2 > m {
                k2start += 2;
            } else if !front {
                let k1_off = offset + delta - k2;
                if k1_off >= 0 && (k1_off as usize) < len && v1[k1_off as usize] != -1 {
                    let x1 = v1[k1_off as usize];
                    let y1 = offset + x1 - k1_off;
                    if x1 >= n - x2 {
                        return Some((x1 as usize, y1 as usize));
                    }
                }
            }
            k2 += 2;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies a unified diff produced by `unified_diff` to `old`.
    fn apply(old: &str, patch: &str) -> String {
        let a: Vec<&str> = old.split_inclusive('\n').collect();
        let mut out = String::new();
        let mut next = 0;
        let mut lines = patch.lines().skip(2).peekable();
        while let Some(header) = lines.next() {
            let old_range = header.split(' ').nth(1).unwrap().trim_start_matches('-');
            let (start, count) = match old_range.split_once(',') {
                Some((s, c)) => (s.parse::<usize>().unwrap(), c.parse::<usize>().unwrap()),
                None => (old_range.parse().unwrap(), 1),
            };
            let first = if count == 0 { start } else { start - 1 };
            for line in &a[next..first] {
                out.push_str(line);
            }
            next = first;
            while let Some(line) = lines.peek() {
                if line.starts_with("@@") {
                    break;
                }
                let line = lines.next().unwrap();
                match line.as_bytes()[0] {
                    b' ' => {
                        out.push_str(a[next]);
                        next += 1;
                    }
                    b'-' => next += 1,
                    b'+' => {
                        out.push_str(&line[1..]);
                        out.push('\n');
                    }
                    _ => {}
                }
            }
        }
        for line in &a[next..] {
            out.push_str(line);
        }
        out
    }

    #[test]
    fn equal_texts_have_no_diff() {
        assert_eq!(unified_diff("a\nb\n", "a\nb\n", "a", "b"), "");
    }

    #[test]
    fn diff_applies_back() {
        let cases = [
            ("a\nb\nc\n", "a\nB\nc\n"),
            ("", "x\ny\n"),
            ("x\ny\n", ""),
            (
                "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n",
                "1\n2x\n3\n4\n5\n6\n7\n8\n9\n10\n11x\n12\n",
            ),
            ("a\nb\nc\nd\ne\n", "e\nd\nc\nb\na\n"),
            ("  a\n b\nc\n", "a\nb\nc\nd\n"),
        ];
        for (old, new) in cases {
            let patch = unified_diff(old, new, "a", "b");
            assert_eq!(apply(old, &patch), new, "patch:\n{patch}");
        }
    }

    #[test]
    fn hunk_header_format() {
        let patch = unified_diff("a\nb\nc\n", "a\nX\nc\n", "old", "new");
        assert_eq!(patch, "--- old\n+++ new\n@@ -1,3 +1,3 @@\n a\n-b\n+X\n c\n");
    }
}
