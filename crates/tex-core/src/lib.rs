//! tex-core: a pure-Rust TeX engine (pdfTeX-compatible core).

pub mod boxes;
pub mod engine;
pub mod build;
pub mod control;
pub mod maincontrol;
pub mod align;
pub mod fontload;
pub mod linebreak;
pub mod math;
pub mod page;
pub mod io;
pub mod expand;
pub mod scan;
pub mod eqtb;
pub mod format;
pub mod fontiface;
pub mod fonts;
pub mod hyphen;
pub mod input;
pub mod pdfout;
pub mod pdffile;
pub mod pdf_fonts;
pub mod pdftex;
pub mod pdfrender;
pub mod prim;
pub mod scaled;
pub mod scanner;
pub mod tfm;
pub mod token;

pub use engine::Engine;
/// Cached debug-flag lookup for the `eprintln!` probes scattered through the
/// engine. `std::env::var` costs ~100-300ns per call and we were paying it on
/// every token push (PUSHWATCH et al.) — that alone dominated boot time.
/// A flag is true iff the variable is set to a non-empty value other than "0".
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
