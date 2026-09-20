//! Hyphenation: Liang pattern trie with `.` word-boundary marks, plus
//! exception words from `\hyphenation{...}`.
//!
//! Patterns are loaded from TeX's `hyphen.tex` (`\patterns{...}` /
//! `\hyphenation{...}` blocks); see [`Trie::load_hyphen_file`].

use crate::FxHashMap;

#[derive(Debug)]
pub struct Trie {
    /// node transitions: node_id -> (byte -> node_id)
    pub trans: Vec<FxHashMap<u8, usize>>,
    /// pattern values per node: (position, value); the value applies to the
    /// gap `position` chars after the position where the walk started
    pub values: Vec<Vec<(usize, u8)>>,
    /// exception words: lowercased letters -> sorted break points, where a
    /// point `k` means "between letter k-1 and letter k" (k letters precede)
    pub exceptions: FxHashMap<Vec<u8>, Vec<usize>>,
}

impl Trie {
    pub fn new() -> Self {
        Trie {
            trans: vec![FxHashMap::default()],
            values: vec![Vec::new()],
            exceptions: FxHashMap::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.trans.len() <= 1 && self.exceptions.is_empty()
    }

    /// insert a compiled pattern: key bytes (letters and '.' marks) with
    /// inter-letter digit values
    pub fn insert(&mut self, key: &[u8], values: &[(usize, u8)]) {
        let mut node = 0usize;
        for &b in key {
            if let Some(&next) = self.trans[node].get(&b) {
                node = next;
            } else {
                let next = self.trans.len();
                self.trans[node].insert(b, next);
                self.trans.push(FxHashMap::default());
                self.values.push(Vec::new());
                node = next;
            }
        }
        let slot = &mut self.values[node];
        for &(pos, v) in values {
            match slot.iter_mut().find(|x| x.0 == pos) {
                Some(e) => {
                    if v > e.1 {
                        e.1 = v;
                    }
                }
                None => slot.push((pos, v)),
            }
        }
    }

    /// compile one `\patterns` entry over raw bytes, e.g. `.ach4`, `a1bc3cd`, `4tion`,
    /// or TeX hex-escaped byte patterns like `.^^e0^^e11^^f0`.
    pub fn add_pattern_bytes(&mut self, pat: &[u8]) {
        let mut key: Vec<u8> = Vec::with_capacity(pat.len());
        let mut values: Vec<(usize, u8)> = Vec::new();
        let mut pos = 0usize;
        let mut i = 0usize;
        while i < pat.len() {
            let ch = pat[i];
            match ch {
                b'0'..=b'9' => {
                    if ch != b'0' {
                        values.push((pos, ch - b'0'));
                    }
                    i += 1;
                }
                b'^' if i + 3 < pat.len() && pat[i + 1] == b'^' => {
                    if let (Some(h1), Some(h2)) = (hex_val(pat[i + 2]), hex_val(pat[i + 3])) {
                        key.push((h1 << 4) | h2);
                        pos += 1;
                        i += 4;
                    } else {
                        key.push(ch);
                        pos += 1;
                        i += 1;
                    }
                }
                _ => {
                    key.push(ch);
                    pos += 1;
                    i += 1;
                }
            }
        }
        if !key.is_empty() {
            self.insert(&key, &values);
        }
    }

    /// compile one `\patterns` entry, e.g. `.ach4`, `a1bc3cd`, `4tion`.
    pub fn add_pattern(&mut self, pat: &str) {
        self.add_pattern_bytes(pat.as_bytes());
    }

    /// compile one `\hyphenation` entry over raw bytes, e.g. `ta-ble`, `ta-ble-b`.
    pub fn add_exception_bytes(&mut self, word: &[u8]) {
        let mut key: Vec<u8> = Vec::with_capacity(word.len());
        let mut points: Vec<usize> = Vec::new();
        let mut i = 0usize;
        while i < word.len() {
            let ch = word[i];
            match ch {
                b'-' => {
                    points.push(key.len());
                    i += 1;
                }
                b'^' if i + 3 < word.len() && word[i + 1] == b'^' => {
                    if let (Some(h1), Some(h2)) = (hex_val(word[i + 2]), hex_val(word[i + 3])) {
                        key.push((h1 << 4) | h2);
                        i += 4;
                    } else {
                        key.push(ch);
                        i += 1;
                    }
                }
                b'A'..=b'Z' => {
                    key.push(ch + 32);
                    i += 1;
                }
                _ => {
                    key.push(ch);
                    i += 1;
                }
            }
        }
        if key.len() >= 2 {
            self.exceptions.insert(key, points);
        }
    }

    /// compile one `\hyphenation` entry, e.g. `ta-ble`, `ta-ble-b`.
    pub fn add_exception(&mut self, word: &str) {
        self.add_exception_bytes(word.as_bytes());
    }

    /// Hyphenation points for a lowercased word. Returns sorted `k` values
    /// where point `k` sits between letter `k-1` and letter `k` (i.e. `k`
    /// letters precede the break). `left`/`right` are
    /// \lefthyphenmin/\righthyphenmin: no point with `k < left` or
    /// `k > word.len() - right` (tex.web: k ranges l_hyf..hn-r_hyf).
    pub fn hyphenate(&self, word: &[u8], left: usize, right: usize) -> Vec<usize> {
        let left = left.max(1);
        let right = right.max(1);
        let n = word.len();
        if n < left.saturating_add(right) {
            return Vec::new();
        }
        if let Some(pts) = self.exceptions.get(word) {
            return pts
                .iter()
                .copied()
                .filter(|&k| k >= left && k + right <= n)
                .collect();
        }
        // text = '.' + word + '.'; vals[i] applies before text[i]
        let mut text = Vec::with_capacity(n + 2);
        text.push(b'.');
        text.extend_from_slice(word);
        text.push(b'.');
        let m = text.len();
        let mut vals = vec![0u8; m + 1];
        for start in 0..m {
            let mut node = 0usize;
            let mut i = start;
            while i < m {
                match self.trans[node].get(&text[i]) {
                    Some(&next) => {
                        node = next;
                        i += 1;
                        for &(pos, v) in &self.values[node] {
                            let slot = start + pos;
                            if v > vals[slot] {
                                vals[slot] = v;
                            }
                        }
                    }
                    None => break,
                }
            }
        }
        let mut out = Vec::new();
        // vals[i] applies before text[i]; text = '.' + word, so a break
        // before text[i] has i-1 letters before it (point k = i-1)
        for i in (left + 1)..=(n - right + 1) {
            if vals[i] % 2 == 1 {
                out.push(i - 1);
            }
        }
        out
    }

    /// Parse a hyphen.tex-style file: `\patterns{...}` and
    /// `\hyphenation{...}` blocks, `%` comments, whitespace-separated
    /// entries. Returns (patterns added, exceptions added).
    pub fn load_hyphen_file(&mut self, path: &std::path::Path) -> std::io::Result<(usize, usize)> {
        let bytes = tex_kpse::fs::read(path)?;
        Ok(self.load_hyphen_bytes(&bytes))
    }

    /// Same as [`Trie::load_hyphen_file`] over already-read content.
    pub fn load_hyphen_str(&mut self, text: &str) -> (usize, usize) {
        self.load_hyphen_bytes(text.as_bytes())
    }

    /// Parse hyphenation patterns and exceptions from raw bytes.
    pub fn load_hyphen_bytes(&mut self, bytes: &[u8]) -> (usize, usize) {
        let mut npat = 0;
        let mut nexc = 0;
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if bytes[i..].starts_with(b"\\patterns") || bytes[i..].starts_with(b"\\hyphenation") {
                let is_pat = bytes[i + 1] == b'p';
                i += if is_pat {
                    b"\\patterns".len()
                } else {
                    b"\\hyphenation".len()
                };
                while i < bytes.len() && bytes[i] != b'{' {
                    i += 1;
                }
                if i >= bytes.len() {
                    break;
                }
                i += 1; // past '{'
                let start = i;
                let mut depth = 1;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        b'%' => {
                            while i < bytes.len() && bytes[i] != b'\n' {
                                i += 1;
                            }
                            continue;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let body = &bytes[start..i.saturating_sub(1).max(start)];
                for line in body.split(|&b| b == b'\n') {
                    let text = line.split(|&b| b == b'%').next().unwrap_or_default();
                    for tok in text
                        .split(|b| b.is_ascii_whitespace())
                        .filter(|t| !t.is_empty())
                    {
                        if is_pat {
                            self.add_pattern_bytes(tok);
                            npat += 1;
                        } else {
                            self.add_exception_bytes(tok);
                            nexc += 1;
                        }
                    }
                }
            } else {
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
            }
        }
        (npat, nexc)
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

use crate::engine::Engine;

impl Engine {
    /// Load the default English patterns (hyphen.tex) into `self.hyphen_trie`.
    /// Returns Err(io) if the file cannot be read. Meant to be called at
    /// format-build / startup time where TeX executes `\patterns`.
    pub fn load_hyphenation_file(&mut self, path: &str) -> std::io::Result<(usize, usize)> {
        self.hyphen_trie
            .load_hyphen_file(std::path::Path::new(path))
    }

    /// Language zero uses the original format slot; other languages are independent.
    pub fn trie_for_language(&self, lang: u8) -> Option<&Trie> {
        if lang == 0 {
            Some(&self.hyphen_trie)
        } else {
            self.hyphen_tries.get(&lang)
        }
    }

    /// Retrieve a mutable reference to the hyphenation trie for a language code.
    pub fn trie_for_language_mut(&mut self, lang: u8) -> &mut Trie {
        if lang == 0 {
            &mut self.hyphen_trie
        } else {
            self.hyphen_tries.entry(lang).or_insert_with(Trie::new)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trie() -> Trie {
        let mut t = Trie::new();
        let path = "/usr/share/texmf-dist/tex/generic/hyphen/hyphen.tex";
        let _ = t.load_hyphen_file(std::path::Path::new(path));
        t
    }

    #[test]
    fn considerable() {
        let t = trie();
        if t.is_empty() {
            return; // hyphen.tex unavailable
        }
        // verified against `tex \showhyphens{...}`:
        // con-sid-er-able, hy-phen-ation, al-go-rithm, ta-ble (exception)
        assert_eq!(t.hyphenate(b"considerable", 2, 3), vec![3, 6, 8]);
        assert_eq!(t.hyphenate(b"hyphenation", 2, 3), vec![2, 6]);
        assert_eq!(t.hyphenate(b"algorithm", 2, 3), vec![2, 4]);
        assert_eq!(t.hyphenate(b"table", 2, 3), vec![2]);
    }

    #[test]
    fn pattern_comments_do_not_override_breaks() {
        let mut trie = Trie::new();
        trie.load_hyphen_bytes(b"\\patterns{ab1cd % a9bcd is not a pattern\n}");
        assert_eq!(trie.hyphenate(b"abcd", 1, 1), vec![2]);
    }
}
