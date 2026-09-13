#![allow(dead_code)]
#![allow(unused_assignments)]

//! tex-core: a pure-Rust TeX engine (pdfTeX-compatible core).

pub mod align;
pub mod boxes;
pub mod build;
pub mod control;
pub mod diagnostics;
pub mod engine;
pub mod eqtb;
pub mod expand;
pub mod fontiface;
pub mod fontload;
pub mod fonts;
pub mod format;
pub mod hyphen;
pub mod input;
pub mod io;
pub mod linebreak;
pub mod maincontrol;
pub mod math;
pub mod page;
pub mod pdf_fonts;
pub mod pdf_images;
pub mod pdffile;
pub mod pdfout;
pub mod pdfrender;
pub mod pdftex;
pub mod prim;
pub mod scaled;
pub mod scan;
pub mod scanner;
pub mod tfm;
pub mod token;

pub use engine::Engine;
#[inline]
pub fn debug_flag(name: &str) -> bool {
    use std::sync::OnceLock;
    static FLAGS: OnceLock<Vec<String>> = OnceLock::new();
    let flags = FLAGS.get_or_init(|| {
        std::env::var("TEXDEBUG")
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    });
    flags.iter().any(|f| f == name)
}

/// Fast, non-cryptographic hasher used by compiler and lexer symbol tables.
#[derive(Default)]
pub struct FxHasher(u64);

impl std::hash::Hasher for FxHasher {
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline(always)]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(5) ^ (b as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
    #[inline(always)]
    fn write_u8(&mut self, i: u8) {
        self.0 = self.0.rotate_left(5) ^ (i as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    #[inline(always)]
    fn write_u32(&mut self, i: u32) {
        self.0 = self.0.rotate_left(5) ^ (i as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    #[inline(always)]
    fn write_usize(&mut self, i: usize) {
        self.0 = self.0.rotate_left(5) ^ (i as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

pub type FxBuildHasher = std::hash::BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;
pub type FxHashSet<K> = std::collections::HashSet<K, FxBuildHasher>;

pub mod fontmap;

mod pdfcompress;

mod pdfcompact;
