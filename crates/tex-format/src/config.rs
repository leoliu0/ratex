//! Formatter settings and the `.texresfmt.toml` reader.
//!
//! The file holds top-level `key = value` pairs only: integers, booleans,
//! strings and arrays of strings, with `#` comments. Unknown keys are errors,
//! so a misspelled setting is reported instead of silently ignored.

use std::path::{Path, PathBuf};

/// Name of the per-project settings file, looked up from each formatted
/// file's directory towards the filesystem root.
pub const CONFIG_FILE_NAME: &str = ".texresfmt.toml";

/// Environments whose `&` columns are aligned when `align-columns` is on.
pub const DEFAULT_ALIGN_ENVS: &[&str] = &[
    "tabular",
    "tabular*",
    "tabularx",
    "tabulary",
    "longtable",
    "array",
    "align",
    "align*",
    "alignat",
    "alignat*",
    "flalign",
    "flalign*",
    "aligned",
    "alignedat",
    "split",
    "eqnarray",
    "eqnarray*",
    "matrix",
    "pmatrix",
    "bmatrix",
    "Bmatrix",
    "vmatrix",
    "Vmatrix",
    "smallmatrix",
    "cases",
    "dcases",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Spaces per indentation level.
    pub indent_width: usize,
    /// Tab stops used when tabs inside a line become spaces.
    pub tab_width: usize,
    /// Longest run of blank lines kept (at least 1: a blank line ends a paragraph).
    pub max_blank_lines: usize,
    /// Put a blank line before `\part` ... `\subsubsection` at the top level.
    pub blank_line_before_sections: bool,
    /// Start every `\item` on its own line.
    pub one_item_per_line: bool,
    /// Break long prose lines at spaces.
    pub wrap: bool,
    /// Target width for `wrap`.
    pub line_width: usize,
    /// Pad table and alignment cells so the `&` line up.
    pub align_columns: bool,
    /// Environments whose body is not indented.
    pub no_indent_envs: Vec<String>,
    /// Environments added to the built-in verbatim list.
    pub verbatim_envs: Vec<String>,
    /// Commands (without backslash) whose argument is kept verbatim.
    pub verbatim_commands: Vec<String>,
    /// Environments added to the built-in list where lines are never wrapped.
    pub no_wrap_envs: Vec<String>,
    /// Environments whose `&` columns are aligned.
    pub align_envs: Vec<String>,
    /// Classes and packages, besides the built-in list, that may be loaded:
    /// any verbatim environments or commands they define are listed in
    /// `verbatim_envs` and `verbatim_commands`.
    pub known_packages: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            indent_width: 2,
            tab_width: 4,
            max_blank_lines: 1,
            blank_line_before_sections: true,
            one_item_per_line: true,
            wrap: false,
            line_width: 80,
            align_columns: false,
            no_indent_envs: vec!["document".to_string()],
            verbatim_envs: Vec::new(),
            verbatim_commands: Vec::new(),
            no_wrap_envs: Vec::new(),
            align_envs: DEFAULT_ALIGN_ENVS.iter().map(|s| s.to_string()).collect(),
            known_packages: Vec::new(),
        }
    }
}

enum Value {
    Int(i64),
    Bool(bool),
    /// No setting takes a string; kept to report the type.
    Str,
    List(Vec<String>),
}

impl Value {
    fn describe(&self) -> &'static str {
        match self {
            Value::Int(_) => "an integer",
            Value::Bool(_) => "a boolean",
            Value::Str => "a string",
            Value::List(_) => "an array",
        }
    }
}

impl Config {
    /// Parses settings from `.texresfmt.toml` text on top of the defaults.
    pub fn from_toml(text: &str) -> Result<Config, String> {
        let mut config = Config::default();
        let mut lines = text.lines().enumerate().peekable();
        while let Some((index, line)) = lines.next() {
            let line_no = index + 1;
            let line = strip_comment(line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                return Err(format!(
                    "line {line_no}: tables are not supported; use top-level keys"
                ));
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("line {line_no}: expected `key = value`"))?;
            let key = key.trim().trim_matches('"');
            let mut value_text = value.trim().to_string();
            // A multi-line array continues until its closing bracket.
            if value_text.starts_with('[') {
                while !array_closed(&value_text) {
                    let Some((_, next)) = lines.next() else {
                        return Err(format!("line {line_no}: unterminated array for `{key}`"));
                    };
                    value_text.push(' ');
                    value_text.push_str(strip_comment(next).trim());
                }
            }
            let value = parse_value(&value_text).map_err(|e| format!("line {line_no}: {e}"))?;
            config
                .set(key, value)
                .map_err(|e| format!("line {line_no}: {e}"))?;
        }
        Ok(config)
    }

    fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
        fn int(key: &str, value: Value, min: i64, max: i64) -> Result<usize, String> {
            match value {
                Value::Int(n) if (min..=max).contains(&n) => Ok(n as usize),
                Value::Int(n) => Err(format!("`{key}` must be between {min} and {max}, not {n}")),
                other => Err(format!(
                    "`{key}` must be an integer, not {}",
                    other.describe()
                )),
            }
        }
        fn boolean(key: &str, value: Value) -> Result<bool, String> {
            match value {
                Value::Bool(b) => Ok(b),
                other => Err(format!(
                    "`{key}` must be true or false, not {}",
                    other.describe()
                )),
            }
        }
        fn list(key: &str, value: Value) -> Result<Vec<String>, String> {
            match value {
                Value::List(items) => Ok(items),
                other => Err(format!(
                    "`{key}` must be an array of strings, not {}",
                    other.describe()
                )),
            }
        }
        match key {
            "indent-width" => self.indent_width = int(key, value, 0, 16)?,
            "tab-width" => self.tab_width = int(key, value, 1, 16)?,
            "max-blank-lines" => self.max_blank_lines = int(key, value, 1, 100)?,
            "blank-line-before-sections" => self.blank_line_before_sections = boolean(key, value)?,
            "one-item-per-line" => self.one_item_per_line = boolean(key, value)?,
            "wrap" => self.wrap = boolean(key, value)?,
            "line-width" => self.line_width = int(key, value, 20, 10_000)?,
            "align-columns" => self.align_columns = boolean(key, value)?,
            "no-indent-envs" => self.no_indent_envs = list(key, value)?,
            "verbatim-envs" => self.verbatim_envs = list(key, value)?,
            "verbatim-commands" => {
                self.verbatim_commands = list(key, value)?
                    .into_iter()
                    .map(|name| name.trim_start_matches('\\').to_string())
                    .collect()
            }
            "no-wrap-envs" => self.no_wrap_envs = list(key, value)?,
            "align-envs" => self.align_envs = list(key, value)?,
            "known-packages" => self.known_packages = list(key, value)?,
            _ => return Err(format!("unknown setting `{key}`")),
        }
        Ok(())
    }

    /// The settings as `.texresfmt.toml` text (used by `--print-config`).
    pub fn to_toml(&self) -> String {
        fn list(items: &[String]) -> String {
            let quoted: Vec<String> = items.iter().map(|s| quote(s)).collect();
            format!("[{}]", quoted.join(", "))
        }
        format!(
            "indent-width = {}\n\
             tab-width = {}\n\
             max-blank-lines = {}\n\
             blank-line-before-sections = {}\n\
             one-item-per-line = {}\n\
             wrap = {}\n\
             line-width = {}\n\
             align-columns = {}\n\
             no-indent-envs = {}\n\
             verbatim-envs = {}\n\
             verbatim-commands = {}\n\
             no-wrap-envs = {}\n\
             align-envs = {}\n\
             known-packages = {}\n",
            self.indent_width,
            self.tab_width,
            self.max_blank_lines,
            self.blank_line_before_sections,
            self.one_item_per_line,
            self.wrap,
            self.line_width,
            self.align_columns,
            list(&self.no_indent_envs),
            list(&self.verbatim_envs),
            list(&self.verbatim_commands),
            list(&self.no_wrap_envs),
            list(&self.align_envs),
            list(&self.known_packages),
        )
    }

    /// Reads a settings file.
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Config::from_toml(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// The nearest `.texresfmt.toml` in `dir` or one of its ancestors.
pub fn find_config(dir: &Path) -> Option<PathBuf> {
    let mut current = Some(dir);
    while let Some(dir) = current {
        let candidate = dir.join(CONFIG_FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Byte offset of the first `stop` character outside a basic (`"..."`) or
/// literal (`'...'`) string.
fn find_outside_strings(line: &str, stop: char) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            _ if escaped => escaped = false,
            (Some('"'), '\\') => escaped = true,
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c == stop => return Some(i),
            _ => {}
        }
    }
    None
}

/// Removes a `#` comment that is not inside a string.
fn strip_comment(line: &str) -> &str {
    find_outside_strings(line, '#').map_or(line, |i| &line[..i])
}

fn array_closed(text: &str) -> bool {
    find_outside_strings(text, ']').is_some()
}

fn parse_value(text: &str) -> Result<Value, String> {
    match text {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if text.starts_with('"') || text.starts_with('\'') {
        let (_, rest) = parse_string(text)?;
        if !rest.trim().is_empty() {
            return Err(format!("unexpected text after string: `{}`", rest.trim()));
        }
        return Ok(Value::Str);
    }
    if let Some(inner) = text.strip_prefix('[') {
        let mut items = Vec::new();
        let mut rest = inner.trim_start();
        loop {
            if let Some(after) = rest.strip_prefix(']') {
                if !after.trim().is_empty() {
                    return Err(format!("unexpected text after array: `{}`", after.trim()));
                }
                return Ok(Value::List(items));
            }
            let (item, after) =
                parse_string(rest).map_err(|_| "arrays may only hold strings".to_string())?;
            items.push(item);
            rest = after.trim_start();
            if let Some(after) = rest.strip_prefix(',') {
                rest = after.trim_start();
            } else if !rest.starts_with(']') {
                return Err("expected `,` or `]` in array".to_string());
            }
        }
    }
    let digits = text.replace('_', "");
    digits
        .parse::<i64>()
        .map(Value::Int)
        .map_err(|_| format!("cannot read value `{text}`"))
}

/// Parses a basic (`"..."`) or literal (`'...'`) string; returns it and the rest.
fn parse_string(text: &str) -> Result<(String, &str), String> {
    let mut chars = text.char_indices();
    let quote = match chars.next() {
        Some((_, q @ ('"' | '\''))) => q,
        _ => return Err("expected a string".to_string()),
    };
    let mut out = String::new();
    let mut escaped = false;
    for (i, c) in chars {
        if escaped {
            match c {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => return Err(format!("unsupported escape `\\{other}`")),
            }
            escaped = false;
        } else if c == '\\' && quote == '"' {
            escaped = true;
        } else if c == quote {
            return Ok((out, &text[i + 1..]));
        } else {
            out.push(c);
        }
    }
    Err("unterminated string".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let config = Config::default();
        assert_eq!(Config::from_toml(&config.to_toml()).unwrap(), config);
    }

    #[test]
    fn reads_all_value_kinds() {
        let config = Config::from_toml(
            "# project settings\n\
             indent-width = 4 # spaces\n\
             wrap = true\n\
             line-width = 100\n\
             verbatim-envs = [\n  \"pre\", # html-ish\n  'code#1',\n]\n\
             verbatim-commands = [\"\\\\shellcmd\"]\n",
        )
        .unwrap();
        assert_eq!(config.indent_width, 4);
        assert!(config.wrap);
        assert_eq!(config.line_width, 100);
        assert_eq!(config.verbatim_envs, ["pre", "code#1"]);
        assert_eq!(config.verbatim_commands, ["shellcmd"]);
    }

    #[test]
    fn rejects_unknown_keys_and_bad_values() {
        assert!(Config::from_toml("indent = 2")
            .unwrap_err()
            .contains("unknown setting `indent`"));
        assert!(Config::from_toml("wrap = 1")
            .unwrap_err()
            .contains("true or false"));
        assert!(Config::from_toml("max-blank-lines = 0").is_err());
        assert!(Config::from_toml("[format]\nwrap = true").is_err());
        assert!(Config::from_toml("no-indent-envs = [1]").is_err());
    }
}
