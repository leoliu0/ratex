#![allow(dead_code)]
#![allow(unused_assignments)]

//! tex-core: a pure-Rust TeX engine (pdfTeX-compatible core).

pub mod align;
pub mod boxes;
pub mod build;
mod clock;
pub mod control;
pub mod diagnostics;
pub mod driver;
pub mod engine;
pub mod engine_mode;
pub mod engine_lua;
pub mod engine_xetex;
pub mod eqtb;
pub mod expand;
pub mod font_program;
pub mod fontiface;
pub mod fontload;
pub mod fonts;
pub mod format;
pub mod hyphen;
pub mod input;
pub mod io;
pub mod language;
pub mod linebreak;
pub mod luatex;
pub mod lua_font;
mod lua_font_hb;
mod lua_font_lib;
mod lua_font_vf;
mod lua_callbacks;
mod lua_ligkern;
mod lua_bridge;
mod lua_sys;
pub use lua_sys::set_cache_dir;
mod lua_sys_embedded;
mod lua_sys_fio;
mod lua_sys_hash;
mod lua_sys_kpse;
mod lua_sys_lfs;
mod lua_sys_mime;
mod lua_sys_os;
mod lua_sys_status;
mod lua_sys_unicode;
mod lua_sys_zlib;
mod lua_cmds;
pub mod uprim;
mod uprims;
mod lua_img;
mod lua_lang;
mod lua_pdf;
mod lua_tex;
mod lua_texnodes;
mod lua_ud;
mod lua_lpeg;
pub mod lua_node;
mod lua_node_conv;
pub mod lua_node_tables;
mod lua_node_lib;
mod lua_node_ops;
mod lua_node_pack;
pub mod maincontrol;
pub mod math;
pub mod native_font;
pub mod native_layout;
pub mod page;
mod pdf_encodings;
pub mod pdf_fonts;
pub mod pdf_images;
pub mod pdf_svg;
pub mod pdffile;
pub mod pdfout;
pub mod pdfrender;
pub mod pdftex;
pub mod prim;
mod random;
pub mod scaled;
pub mod scan;
pub mod scanner;
mod show_box;
mod show_state;
pub mod synctex;
pub mod tfm;
mod texxet;
mod trace;
pub mod tex_bytes;
mod tex_print;
pub mod token;
mod writet1;

pub use engine::Engine;
pub use pdffile::PdfEncryptConfig;
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
