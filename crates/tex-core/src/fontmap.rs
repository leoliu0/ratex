//! A map file is indexed once; only selected fonts need full option parsing.
use crate::fontload::{parse_map_line, MapEntry};
use std::cell::OnceCell;
use std::ops::Range;
use std::rc::Rc;

enum Entry {
    Parsed(MapEntry),
    Deferred {
        text: Rc<str>,
        line: Range<usize>,
        parsed: OnceCell<MapEntry>,
    },
}

impl Entry {
    fn get(&self) -> &MapEntry {
        match self {
            Self::Parsed(entry) => entry,
            Self::Deferred { text, line, parsed } => parsed.get_or_init(|| {
                // Indexing verifies the two bare words required by the parser.
                parse_map_line(&text[line.clone()]).expect("validated map entry")
            }),
        }
    }
}

#[derive(Default)]
pub struct FontMap {
    entries: crate::FxHashMap<Box<str>, Entry>,
}

impl FontMap {
    pub fn get(&self, name: &str) -> Option<&MapEntry> {
        self.entries.get(name).map(Entry::get)
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn remove(&mut self, name: &str) {
        self.entries.remove(name);
    }

    pub fn insert(&mut self, name: String, entry: MapEntry) {
        self.entries
            .insert(name.into_boxed_str(), Entry::Parsed(entry));
    }

    pub fn extend_file(&mut self, text: String) {
        let text: Rc<str> = text.into();
        let mut offset = 0;
        for raw in text.split_inclusive('\n') {
            let start = offset;
            offset += raw.len();
            let line = raw.trim();
            if line.is_empty() || line.starts_with(['%', '*']) {
                continue;
            }
            let mut words = line.split_whitespace();
            let first = words.next().unwrap();
            let second = words.next();
            if !first.contains('"') && second.is_some_and(|s| !s.contains('"')) {
                self.entries.insert(
                    first.into(),
                    Entry::Deferred {
                        text: text.clone(),
                        line: start..offset,
                        parsed: OnceCell::new(),
                    },
                );
            } else if let Some(entry) = parse_map_line(line) {
                // Unusual joined quotes use the original parser, including
                // malformed lines which must not hide an earlier valid entry.
                self.insert(entry.tfm.clone(), entry);
            }
        }
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
        assert_eq!(lazy.entries.len(), eager.len());
        for (key, value) in eager {
            assert_eq!(lazy.get(&key), Some(&value));
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
