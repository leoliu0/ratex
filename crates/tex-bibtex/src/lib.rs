//! tex-bibtex: BibTeX 0.99e in Rust.
//!
//! A port of bibtex.web with TeX Live's changes (8-bit input, `-terse`,
//! `-min-crossrefs`, exit status). `.bbl` output and the messages in the
//! `.blg` file match TeX Live's `bibtex` byte for byte; the usage
//! statistics TeX Live appends to the `.blg` are not reproduced.

mod auxfile;
mod bib;
mod bst;
mod engine;
mod exec;
mod input;
mod log;
mod text;

pub use engine::run;
