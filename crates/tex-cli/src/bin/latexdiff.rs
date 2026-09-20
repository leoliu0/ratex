//! Built-in latexdiff algorithm and CLI wrapper for Ratex.
//!
//! Computes token/word-level diffs between two LaTeX documents and marks additions
//! and deletions with standard LaTeX diff markup (`\DIFadd{...}`, `\DIFdel{...}`).
//! Also provides fallback execution to system `latexdiff` when available.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Standard LaTeX diff preamble injected into the generated diff file.
pub const LATEXDIFF_PREAMBLE: &str = r#"%DIF PREAMBLE EXTENSION - RATE DIFF
\providecommand{\DIFadd}[1]{{\protect\color{blue}\uwave{#1}}}
\providecommand{\DIFdel}[1]{{\protect\color{red}\sout{#1}}}
\providecommand{\DIFaddbegin}{}
\providecommand{\DIFaddend}{}
\providecommand{\DIFdelbegin}{}
\providecommand{\DIFdelend}{}
%DIF END PREAMBLE EXTENSION
"#;

#[derive(Debug, PartialEq, Eq)]
pub enum DiffToken {
    Word(String),
    Command(String),
    GroupOpen,
    GroupClose,
    Whitespace(String),
    Newline,
    Comment(String),
}

/// Tokenize LaTeX source into atomic chunks for diffing.
pub fn tokenize_latex(src: &str) -> Vec<DiffToken> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        let c = chars[i];
        if c == '\n' {
            tokens.push(DiffToken::Newline);
            i += 1;
        } else if c.is_whitespace() {
            let mut ws = String::new();
            while i < n && chars[i].is_whitespace() && chars[i] != '\n' {
                ws.push(chars[i]);
                i += 1;
            }
            tokens.push(DiffToken::Whitespace(ws));
        } else if c == '%' {
            let mut comment = String::new();
            while i < n && chars[i] != '\n' {
                comment.push(chars[i]);
                i += 1;
            }
            tokens.push(DiffToken::Comment(comment));
        } else if c == '\\' {
            let mut cmd = String::from('\\');
            i += 1;
            if i < n && !chars[i].is_alphabetic() {
                cmd.push(chars[i]);
                i += 1;
            } else {
                while i < n && chars[i].is_alphabetic() {
                    cmd.push(chars[i]);
                    i += 1;
                }
            }
            tokens.push(DiffToken::Command(cmd));
        } else if c == '{' {
            tokens.push(DiffToken::GroupOpen);
            i += 1;
        } else if c == '}' {
            tokens.push(DiffToken::GroupClose);
            i += 1;
        } else {
            let mut word = String::new();
            while i < n
                && !chars[i].is_whitespace()
                && chars[i] != '\\'
                && chars[i] != '{'
                && chars[i] != '}'
                && chars[i] != '%'
            {
                word.push(chars[i]);
                i += 1;
            }
            tokens.push(DiffToken::Word(word));
        }
    }
    tokens
}

impl DiffToken {
    pub fn to_string_repr(&self) -> String {
        match self {
            DiffToken::Word(w) => w.clone(),
            DiffToken::Command(c) => c.clone(),
            DiffToken::GroupOpen => "{".to_string(),
            DiffToken::GroupClose => "}".to_string(),
            DiffToken::Whitespace(w) => w.clone(),
            DiffToken::Newline => "\n".to_string(),
            DiffToken::Comment(c) => c.clone(),
        }
    }
}

/// Compute LCS (Longest Common Subsequence) diff between old and new tokens.
pub fn diff_latex_tokens(old_tokens: &[DiffToken], new_tokens: &[DiffToken]) -> String {
    let m = old_tokens.len();
    let n = new_tokens.len();

    // Fast path: if files are identical
    if old_tokens == new_tokens {
        return old_tokens.iter().map(|t| t.to_string_repr()).collect();
    }

    // Dynamic programming table for LCS
    let mut dp = vec![vec![0u32; n + 1]; m + 1];
    for i in 0..m {
        for j in 0..n {
            if old_tokens[i] == new_tokens[j] {
                dp[i + 1][j + 1] = dp[i][j] + 1;
            } else {
                dp[i + 1][j + 1] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }

    // Backtrack to build diff
    let mut i = m;
    let mut j = n;
    let mut ops = Vec::new();

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_tokens[i - 1] == new_tokens[j - 1] {
            ops.push((0, &old_tokens[i - 1])); // Unchanged
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops.push((1, &new_tokens[j - 1])); // Added
            j -= 1;
        } else if i > 0 && (j == 0 || dp[i][j - 1] < dp[i - 1][j]) {
            ops.push((-1, &old_tokens[i - 1])); // Deleted
            i -= 1;
        }
    }
    ops.reverse();

    // Render output
    let mut out = String::new();
    let mut has_preamble_injected = false;

    let mut k = 0;
    while k < ops.len() {
        let (op, tok) = ops[k];

        // Check if we reached \begin{document} to inject preamble if needed
        if !has_preamble_injected {
            if let DiffToken::Command(cmd) = tok {
                if cmd == "\\begin" {
                    out.push_str(LATEXDIFF_PREAMBLE);
                    has_preamble_injected = true;
                }
            }
        }

        match op {
            0 => {
                out.push_str(&tok.to_string_repr());
                k += 1;
            }
            1 => {
                // Collect contiguous additions
                let mut added_text = String::new();
                while k < ops.len() && ops[k].0 == 1 {
                    added_text.push_str(&ops[k].1.to_string_repr());
                    k += 1;
                }
                out.push_str(&format!("\\DIFadd{{{added_text}}}"));
            }
            -1 => {
                // Collect contiguous deletions
                let mut deleted_text = String::new();
                while k < ops.len() && ops[k].0 == -1 {
                    deleted_text.push_str(&ops[k].1.to_string_repr());
                    k += 1;
                }
                out.push_str(&format!("\\DIFdel{{{deleted_text}}}"));
            }
            _ => unreachable!(),
        }
    }

    if !has_preamble_injected {
        // Fallback: prepend preamble if \begin{document} wasn't found
        format!("{LATEXDIFF_PREAMBLE}\n{out}")
    } else {
        out
    }
}

/// Run latexdiff on two files, writing output or returning stdout string.
pub fn run_latexdiff(old_path: &Path, new_path: &Path) -> Result<String, String> {
    // If system latexdiff is present, try it first for maximum LaTeX package macro coverage.
    // Guard against re-invoking ourselves if ratex/latexdiff binary is on PATH.
    if std::env::var("RATEX_LATEXDIFF_NESTED").is_err() {
        if let Ok(output) = Command::new("latexdiff")
            .env("RATEX_LATEXDIFF_NESTED", "1")
            .arg(old_path)
            .arg(new_path)
            .output()
        {
            if output.status.success() {
                return Ok(String::from_utf8_lossy(&output.stdout).to_string());
            }
        }
    }
    // Fall back to native Ratex latexdiff algorithm
    let old_src = fs::read_to_string(old_path)
        .map_err(|e| format!("Cannot read old file {}: {e}", old_path.display()))?;
    let new_src = fs::read_to_string(new_path)
        .map_err(|e| format!("Cannot read new file {}: {e}", new_path.display()))?;

    let old_tokens = tokenize_latex(&old_src);
    let new_tokens = tokenize_latex(&new_src);
    Ok(diff_latex_tokens(&old_tokens, &new_tokens))
}

/// CLI entry point for `ratex latexdiff old.tex new.tex [output.tex]`
pub fn latexdiff_main(args: &[String]) -> i32 {
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "-v" | "-version" | "--version"))
    {
        println!("latexdiff (Ratex {})", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if args.len() < 2 || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("Usage: ratex latexdiff [OPTIONS] <old.tex> <new.tex> [output.tex]");
        eprintln!("       latexdiff <old.tex> <new.tex> [output.tex]");
        eprintln!("\nComputes token-level visual diff with \\DIFadd and \\DIFdel markup.");
        return if args.iter().any(|a| a == "-h" || a == "--help") {
            0
        } else {
            1
        };
    }

    let non_flags: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if non_flags.len() < 2 {
        eprintln!("Error: two input files (old and new) are required");
        return 1;
    }

    let old_file = Path::new(non_flags[0]);
    let new_file = Path::new(non_flags[1]);
    let out_file = non_flags.get(2).map(|s| Path::new(s.as_str()));

    match run_latexdiff(old_file, new_file) {
        Ok(diff_text) => {
            if let Some(dest) = out_file {
                if let Err(e) = fs::write(dest, diff_text) {
                    eprintln!("Error writing {}: {e}", dest.display());
                    return 1;
                }
                println!("Wrote diff to {}", dest.display());
            } else {
                print!("{diff_text}");
            }
            0
        }
        Err(e) => {
            eprintln!("Error: {e}");
            1
        }
    }
}
#[allow(dead_code)]
pub(crate) fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(latexdiff_main(&args));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latex_tokenizer() {
        let src = "Hello \\textbf{world} % comment\nNext line";
        let tokens = tokenize_latex(src);
        assert!(!tokens.is_empty());
        assert_eq!(tokens[0], DiffToken::Word("Hello".into()));
        assert_eq!(tokens[1], DiffToken::Whitespace(" ".into()));
        assert_eq!(tokens[2], DiffToken::Command("\\textbf".into()));
        assert_eq!(tokens[3], DiffToken::GroupOpen);
        assert_eq!(tokens[4], DiffToken::Word("world".into()));
        assert_eq!(tokens[5], DiffToken::GroupClose);
    }

    #[test]
    fn test_diff_word_replacement() {
        let old = "\\begin{document}\nHello old world.\n\\end{document}";
        let new = "\\begin{document}\nHello new world.\n\\end{document}";
        let old_toks = tokenize_latex(old);
        let new_toks = tokenize_latex(new);
        let diff = diff_latex_tokens(&old_toks, &new_toks);
        assert!(diff.contains("\\DIFdel{old}"));
        assert!(diff.contains("\\DIFadd{new}"));
        assert!(diff.contains(LATEXDIFF_PREAMBLE));
    }

    #[test]
    fn test_latexdiff_main_flags() {
        assert_eq!(latexdiff_main(&["--help".to_string()]), 0);
        assert_eq!(latexdiff_main(&["-h".to_string()]), 0);
        assert_eq!(latexdiff_main(&[]), 1);
        assert_eq!(latexdiff_main(&["only_one.tex".to_string()]), 1);
    }

    #[test]
    fn test_latexdiff_main_file_diff() {
        use std::time::SystemTime;
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("latexdiff_test_{nonce}"));
        fs::create_dir_all(&temp_dir).unwrap();

        let old_file = temp_dir.join("old.tex");
        let new_file = temp_dir.join("new.tex");
        let out_file = temp_dir.join("diff.tex");

        fs::write(
            &old_file,
            "\\begin{document}\nOriginal sentence.\n\\end{document}",
        )
        .unwrap();
        fs::write(
            &new_file,
            "\\begin{document}\nModified sentence.\n\\end{document}",
        )
        .unwrap();

        let exit_code = latexdiff_main(&[
            old_file.to_str().unwrap().to_string(),
            new_file.to_str().unwrap().to_string(),
            out_file.to_str().unwrap().to_string(),
        ]);
        assert_eq!(exit_code, 0);
        assert!(out_file.exists());

        let diff_content = fs::read_to_string(&out_file).unwrap();
        assert!(diff_content.contains("\\DIFdel"));
        assert!(diff_content.contains("\\DIFadd"));

        // Also verify native diff on the file contents
        let old_toks = tokenize_latex(&fs::read_to_string(&old_file).unwrap());
        let new_toks = tokenize_latex(&fs::read_to_string(&new_file).unwrap());
        let native_diff = diff_latex_tokens(&old_toks, &new_toks);
        assert!(native_diff.contains("\\DIFdel{Original}"));
        assert!(native_diff.contains("\\DIFadd{Modified}"));
        assert!(native_diff.contains(LATEXDIFF_PREAMBLE));
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
