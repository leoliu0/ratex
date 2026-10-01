//! pdfTeX's font map database (mapfile.c): an ordered list of map-file and
//! map-line layers, each applied with its `+`/`=`/`-` mode. Each layer
//! indexes its entry lines by TFM name on first use; only requested fonts
//! undergo line parsing.
use crate::fontload::{parse_map_line, MapEntry};
use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

/// How a map item treats entries for TFM names that are already mapped
/// (mapfile.c `FM_DUPIGNORE` / `FM_REPLACE` / `FM_DELETE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapMode {
    /// `+` and unprefixed items: insert unless the TFM is already mapped;
    /// within one item the first entry wins.
    DupIgnore,
    /// `=`: replace an earlier entry unless its font is already in use;
    /// within one item the last entry wins.
    Replace,
    /// `-`: delete an earlier entry unless its font is already in use.
    Delete,
}

struct Layer {
    text: Rc<str>,
    mode: MapMode,
    /// TFM names whose entries were in use when this replace/delete layer
    /// was added: mapfile.c `avl_do_entry` leaves them untouched.
    frozen: crate::FxHashSet<Box<str>>,
    index: OnceCell<LayerIndex>,
}

/// Byte offsets of the entry lines of a layer's text, in text order.
#[derive(Default)]
struct LayerIndex {
    /// Lines whose TFM name is their first word, by that name.
    by_name: crate::FxHashMap<Box<str>, smallvec::SmallVec<[u32; 1]>>,
    /// Lines whose TFM name only a full parse reveals (a quote precedes or
    /// adjoins the first word).
    other: Vec<u32>,
}

impl LayerIndex {
    fn build(text: &str) -> Self {
        let mut index = LayerIndex::default();
        let mut offset = 0;
        for raw in text.split_inclusive('\n') {
            let start = offset as u32;
            offset += raw.len();
            let line = raw.trim();
            if line.is_empty() || line.starts_with(['%', '*', '#', ';']) {
                continue;
            }
            match plain_map_tfm(line) {
                Some(tfm) => index.by_name.entry(tfm.into()).or_default().push(start),
                None => index.other.push(start),
            }
        }
        index
    }
}

/// The TFM name `parse_map_line` takes from `line` when it is the first
/// word and no quote precedes or adjoins it.
fn plain_map_tfm(line: &str) -> Option<&str> {
    let head = line.split('"').next().unwrap_or(line);
    let tfm = head.split_whitespace().next()?;
    let end = tfm.as_ptr() as usize - head.as_ptr() as usize + tfm.len();
    (end < head.len() || head.len() == line.len()).then_some(tfm)
}

/// pdfTeX diagnostics owed for one added layer, in entry order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LayerReport {
    /// `fontmap entry for `x' already exists, duplicates ignored`
    pub duplicates: Vec<String>,
    /// `fontmap entry for `x' has been used, replace/delete not allowed`
    pub in_use: Vec<String>,
}

#[derive(Default)]
pub struct FontMap {
    entries: RefCell<crate::FxHashMap<Box<str>, Option<MapEntry>>>,
    layers: Vec<Layer>,
}

impl FontMap {
    pub fn get(&self, name: &str) -> Option<MapEntry> {
        if let Some(cached) = self.entries.borrow().get(name) {
            return cached.clone();
        }
        let entry = self.resolve(name);
        self.entries
            .borrow_mut()
            .insert(name.into(), entry.clone());
        entry
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Replay the layers in order for one TFM name.
    fn resolve(&self, name: &str) -> Option<MapEntry> {
        let mut entry = None;
        for layer in &self.layers {
            match layer.mode {
                MapMode::DupIgnore => {
                    if entry.is_none() {
                        entry = layer.find_entry(name, true);
                    }
                }
                MapMode::Replace => {
                    if !layer.frozen.contains(name) {
                        if let Some(found) = layer.find_entry(name, false) {
                            entry = Some(found);
                        }
                    }
                }
                MapMode::Delete => {
                    if !layer.frozen.contains(name) && map_text_names(&layer.text).any(|n| n == name)
                    {
                        entry = None;
                    }
                }
            }
        }
        entry
    }

    /// Append a map file or map line. `check` enables pdfTeX's per-entry
    /// duplicate diagnostics (the default `pdftex.map` is written
    /// duplicate-free by updmap, so it is added without the full parse).
    pub fn add_layer(
        &mut self,
        text: String,
        mode: MapMode,
        in_use: &dyn Fn(&str) -> bool,
        check: bool,
    ) -> LayerReport {
        let mut report = LayerReport::default();
        let mut frozen = crate::FxHashSet::default();
        if check || mode != MapMode::DupIgnore {
            let mut seen = crate::FxHashSet::default();
            for line in text.lines().map(str::trim) {
                if line.is_empty() || line.starts_with(['%', '*', '#', ';']) {
                    continue;
                }
                // a deletion only needs the TFM name (`\pdfmapline{-cmr10}`)
                let tfm = if mode == MapMode::Delete {
                    match line.split_whitespace().next() {
                        Some(name) => name.to_string(),
                        None => continue,
                    }
                } else {
                    match parse_map_line(line) {
                        Some(entry) => entry.tfm,
                        None => continue,
                    }
                };
                let earlier = seen.contains(tfm.as_str()) || self.get(&tfm).is_some();
                match mode {
                    MapMode::DupIgnore => {
                        if earlier {
                            report.duplicates.push(tfm.clone());
                        }
                    }
                    MapMode::Replace | MapMode::Delete => {
                        if earlier && in_use(&tfm) {
                            report.in_use.push(tfm.clone());
                            frozen.insert(tfm.clone().into_boxed_str());
                        }
                    }
                }
                seen.insert(tfm);
            }
        }
        self.entries.borrow_mut().clear();
        self.layers.push(Layer {
            text: text.into(),
            mode,
            frozen,
            index: OnceCell::new(),
        });
        report
    }
}

/// TFM names (first items) of the entry lines of a map text.
fn map_text_names(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(['%', '*', '#', ';']))
        .filter_map(|line| line.split_whitespace().next())
}

impl Layer {
    /// The first (`first`) or last entry for `name` in this layer.
    fn find_entry(&self, name: &str, first: bool) -> Option<MapEntry> {
        let index = self.index.get_or_init(|| LayerIndex::build(&self.text));
        let named = index.by_name.get(name).map_or(&[][..], |lines| &lines[..]);
        let (mut named, mut other) = (named.iter().peekable(), index.other.iter().peekable());
        let mut found = None;
        loop {
            // Visit both line lists in text order.
            let start = match (named.peek(), other.peek()) {
                (Some(&&a), Some(&&b)) if a < b => named.next(),
                (Some(_), None) => named.next(),
                _ => other.next(),
            };
            let Some(&start) = start else {
                break;
            };
            let rest = &self.text[start as usize..];
            let line = rest[..rest.find('\n').map_or(rest.len(), |end| end + 1)].trim();
            if !line.contains(name) {
                continue;
            }
            if let Some(entry) = parse_map_line(line) {
                if entry.tfm == name {
                    found = Some(entry);
                    if first {
                        break;
                    }
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazy_map_matches_eager_parser_and_mutations() {
        let chunks = [
            "foo",
            "bar",
            "\"x y\"",
            "\"x",
            "y\"",
            "<[test.enc",
            ".167SlantFont",
            "\t",
            "\u{2003}",
            "a\"b\"",
            "\"q\"z",
        ];
        let mut text = String::from("% comment\n* comment\nvalid Font <valid.pfb\nvalid\n");
        for a in chunks {
            for b in chunks {
                for c in chunks {
                    text.push_str(&format!("{a} {b} {c}\n"));
                }
            }
        }
        // mapfile.c inserts in FM_DUPIGNORE mode: the first entry wins.
        let mut eager = crate::FxHashMap::default();
        for line in text.lines().map(str::trim) {
            if line.starts_with(['%', '*']) {
                continue;
            }
            if let Some(e) = parse_map_line(line) {
                eager.entry(e.tfm.clone()).or_insert(e);
            }
        }
        // An `=` layer of the same text: the last entry wins.
        let mut last = crate::FxHashMap::default();
        for line in text.lines().map(str::trim) {
            if line.starts_with(['%', '*']) {
                continue;
            }
            if let Some(e) = parse_map_line(line) {
                last.insert(e.tfm.clone(), e);
            }
        }
        let mut replaced = FontMap::default();
        replaced.add_layer(text.clone(), MapMode::Replace, &|_: &str| false, false);
        for (key, value) in last {
            assert_eq!(replaced.get(&key).as_ref(), Some(&value));
        }
        let never_used = |_: &str| false;
        let mut lazy = FontMap::default();
        lazy.add_layer(text, MapMode::DupIgnore, &never_used, false);
        for (key, value) in eager {
            assert_eq!(lazy.get(&key).as_ref(), Some(&value));
        }
        let report = lazy.add_layer(
            "valid Ignored <new.pfb\nfresh Fresh <fresh.pfb\nfresh Again <again.pfb\n".into(),
            MapMode::DupIgnore,
            &never_used,
            true,
        );
        assert_eq!(report.duplicates, ["valid", "fresh"]);
        assert_eq!(lazy.get("valid").unwrap().fontname, "Font");
        assert_eq!(lazy.get("fresh").unwrap().fontname, "Fresh");
        lazy.add_layer("valid Replacement <new.pfb\n".into(), MapMode::Replace, &never_used, true);
        assert_eq!(lazy.get("valid").unwrap().fontname, "Replacement");
        lazy.add_layer("valid\n".into(), MapMode::Delete, &never_used, true);
        assert!(lazy.get("valid").is_none());
        let used = |name: &str| name == "fresh";
        let report = lazy.add_layer("fresh Other <o.pfb\n".into(), MapMode::Replace, &used, true);
        assert_eq!(report.in_use, ["fresh"]);
        assert_eq!(lazy.get("fresh").unwrap().fontname, "Fresh");
    }
}
