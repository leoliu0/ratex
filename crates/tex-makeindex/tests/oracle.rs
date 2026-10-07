//! Outputs of TeX Live's `makeindex` 2.18 (generated with `makeindex -q`) for
//! a few inputs; the port must reproduce them byte for byte.

use std::cell::RefCell;
use std::collections::HashMap;

use tex_makeindex::Host;

struct MemoryHost {
    files: RefCell<HashMap<String, Vec<u8>>>,
}

impl Host for MemoryHost {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
    fn write(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.files.borrow_mut().insert(path.to_string(), bytes.to_vec());
        Ok(())
    }
    fn exists(&self, path: &str) -> bool {
        self.files.borrow().contains_key(path)
    }
    fn find_style(&self, name: &str) -> Option<(String, Vec<u8>)> {
        self.files.borrow().get(name).map(|bytes| (name.to_string(), bytes.clone()))
    }
    fn read_stdin(&self) -> std::io::Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

fn check(name: &str) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    let read = |file: String| std::fs::read(format!("{dir}{file}")).unwrap();
    let host = MemoryHost { files: RefCell::new(HashMap::new()) };
    host.write(&format!("{name}.idx"), &read(format!("{name}.idx"))).unwrap();
    if let Ok(style) = std::fs::read(format!("{dir}{name}.ist")) {
        host.write(&format!("{name}.ist"), &style).unwrap();
    }
    let mut args: Vec<String> = String::from_utf8(read(format!("{name}.args")))
        .unwrap()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    args.insert(0, "-q".to_string());
    args.push(format!("{name}.idx"));
    assert_eq!(tex_makeindex::run_cli(&args, &host), 0);
    let produced = host.files.borrow().get(&format!("{name}.ind")).cloned().unwrap();
    let expected = read(format!("{name}.ind"));
    assert_eq!(
        String::from_utf8_lossy(&produced),
        String::from_utf8_lossy(&expected),
        "{name}"
    );
}

#[test]
fn headings_sublevels_sort_keys_ranges_and_page_types() {
    check("basic");
}

#[test]
fn letter_ordering_and_blank_compression() {
    check("letter");
}

#[test]
fn no_implicit_ranges() {
    check("noranges");
}

#[test]
fn long_page_lists_wrap() {
    check("wrap");
}

#[test]
fn style_file_headings_items_delimiters_and_suffixes() {
    check("styled");
}
