//! tex-format: the LaTeX source formatter behind `texres fmt`.
//!
//! [`format_source`] indents environment bodies, brace groups and list items,
//! normalizes blanks and blank lines, and optionally wraps prose and aligns
//! `&` columns. It never changes what TeX reads: every result is compared
//! with its input token by token (see `tokens`), and verbatim material is
//! copied byte for byte. [`cli::run`] is the command-line front end.

pub mod cli;
mod config;
mod diff;
mod format;
mod sections;
mod tokens;

pub use config::{find_config, Config, CONFIG_FILE_NAME, DEFAULT_ALIGN_ENVS};
pub use diff::unified_diff;
pub use format::{format_source, ArgSpec, Delim, Extras, FormatError, SourceKind};
pub use sections::{FileKind, SectionFacts};
