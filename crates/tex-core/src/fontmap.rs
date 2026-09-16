//! A map file is scanned on demand; only requested fonts undergo line parsing.
use crate::fontload::{parse_map_line, MapEntry};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
pub struct FontMap {
    entries: RefCell<crate::FxHashMap<Box<str>, Option<MapEntry>>>,
    removed: RefCell<crate::FxHashSet<Box<str>>>,
    files: Vec<Rc<str>>,
}

impl FontMap {
    pub fn get(&self, name: &str) -> Option<MapEntry> {
        if self.removed.borrow().contains(name) {
            return None;
        }
        if let Some(cached) = self.entries.borrow().get(name) {
            return cached.clone();
        }
        // Scan raw files in reverse order (most recently added file wins)
        for file in self.files.iter().rev() {
            if let Some(entry) = find_map_entry_in_text(file, name) {
                self.entries
                    .borrow_mut()
                    .insert(name.into(), Some(entry.clone()));
                return Some(entry);
            }
        }
        self.entries.borrow_mut().insert(name.into(), None);
        None
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn clear(&mut self) {
        self.entries.borrow_mut().clear();
        self.removed.borrow_mut().clear();
        self.files.clear();
    }

    pub fn remove(&mut self, name: &str) {
        self.entries.borrow_mut().remove(name);
        self.removed.borrow_mut().insert(name.into());
    }

    pub fn insert(&mut self, name: String, entry: MapEntry) {
        self.removed.borrow_mut().remove(name.as_str());
        self.entries
            .borrow_mut()
            .insert(name.into_boxed_str(), Some(entry));
    }
    pub fn extend_file(&mut self, text: String) {
        self.entries.borrow_mut().clear();
        self.files.push(text.into());
    }
}

fn find_map_entry_in_text(text: &str, name: &str) -> Option<MapEntry> {
    let mut last_match = None;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(['%', '*']) {
            continue;
        }
        if line.starts_with(name) {
            let after = &line[name.len()..];
            if after.is_empty() || after.starts_with([' ', '\t', '"']) {
                if let Some(entry) = parse_map_line(line) {
                    if entry.tfm == name {
                        last_match = Some(entry);
                    }
                }
            }
        } else if line.contains(name) {
            if let Some(entry) = parse_map_line(line) {
                if entry.tfm == name {
                    last_match = Some(entry);
                }
            }
        }
    }
    last_match
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
        ];
        let mut text = String::from("% comment\n* comment\nvalid Font <valid.pfb\nvalid\n");
        for a in chunks {
            for b in chunks {
                for c in chunks {
                    text.push_str(&format!("{a} {b} {c}\n"));
                }
            }
        }
        let mut eager = crate::FxHashMap::default();
        for line in text.lines().map(str::trim) {
            if line.starts_with(['%', '*']) {
                continue;
            }
            if let Some(e) = parse_map_line(line) {
                eager.insert(e.tfm.clone(), e);
            }
        }
        let mut lazy = FontMap::default();
        lazy.extend_file(text);
        for (key, value) in eager {
            assert_eq!(lazy.get(&key).as_ref(), Some(&value));
        }
        lazy.extend_file("valid Replacement <new.pfb\n".into());
        assert_eq!(lazy.get("valid").unwrap().fontname, "Replacement");
        lazy.remove("valid");
        assert!(lazy.get("valid").is_none());
        let entry = parse_map_line("valid Explicit <explicit.pfb").unwrap();
        lazy.insert(entry.tfm.clone(), entry);
        assert_eq!(lazy.get("valid").unwrap().fontname, "Explicit");
        lazy.clear();
        assert!(lazy.get("valid").is_none());
    }
}
