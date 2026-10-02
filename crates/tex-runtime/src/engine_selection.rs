use tex_core::engine::EngineKind;

/// Detect standard TeX magic comments/program directives in a file's initial lines.
/// Examples:
///   `% !TeX program = xelatex`
///   `% !TEX TS-program = lualatex`
///   `%&pdflatex`
pub fn detect_program_directive(source: &str) -> Option<EngineKind> {
    for line in source.lines().take(25) {
        let trimmed = line.trim();
        if !trimmed.starts_with('%') {
            if !trimmed.is_empty() {
                break;
            }
            continue;
        }
        let comment = trimmed.trim_start_matches('%').trim();
        let lower = comment.to_ascii_lowercase();
        if lower.starts_with('&') {
            let prog = lower.trim_start_matches('&').trim();
            if prog.starts_with("xelatex") || prog.starts_with("xetex") {
                return Some(EngineKind::XeTeX);
            } else if prog.starts_with("lualatex") || prog.starts_with("luatex") {
                return Some(EngineKind::LuaTeX);
            } else if prog.starts_with("pdflatex") || prog.starts_with("pdftex") {
                return Some(EngineKind::PdfTeX);
            }
        }
        if lower.starts_with("!tex program") || lower.starts_with("!tex ts-program") {
            if let Some((_, prog)) = lower.split_once('=') {
                let prog = prog.trim();
                if prog.contains("xelatex") || prog.contains("xetex") {
                    return Some(EngineKind::XeTeX);
                } else if prog.contains("lualatex") || prog.contains("luatex") {
                    return Some(EngineKind::LuaTeX);
                } else if prog.contains("pdflatex") || prog.contains("pdftex") {
                    return Some(EngineKind::PdfTeX);
                }
            }
        }
    }
    None
}

/// Packages that only work under LuaTeX.
const LUATEX_PACKAGES: &[&str] = &["luatexja", "luacode", "luatextra"];

/// Packages and classes which TeX Live compiles only with a Unicode engine
/// (XeTeX or LuaTeX). A document that requires one and no LuaTeX-only package
/// runs under XeTeX, as `texmk` runs it as `xelatex` (keep the lists in sync
/// with `UNICODE_ENGINE_PACKAGES`/`UNICODE_ENGINE_CLASSES` in `texmk.rs`).
const UNICODE_ENGINE_PACKAGES: &[&str] = &["ctex", "xeCJK", "fontspec", "unicode-math", "polyglossia"];
const UNICODE_ENGINE_CLASSES: &[&str] = &["ctexart", "ctexbook", "ctexrep", "ctexbeamer"];

/// The text of a line before its first unescaped `%`.
fn strip_comment(line: &str) -> &str {
    let mut escaped = false;
    for (k, &b) in line.as_bytes().iter().enumerate() {
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == b'%' {
            return &line[..k];
        }
    }
    line
}

/// Arguments of every `\command[...]{a,b}` occurrence in `text`, split on commas.
fn command_arguments<'a>(text: &'a str, command: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(command) {
        rest = &rest[at + command.len()..];
        // A longer control word (`\usepackagefoo`) is not this command.
        if rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '@') {
            continue;
        }
        let mut tail = rest.trim_start();
        if tail.starts_with('[') {
            match tail.find(']') {
                Some(end) => tail = tail[end + 1..].trim_start(),
                None => break,
            }
        }
        if let Some(body) = tail.strip_prefix('{') {
            if let Some(end) = body.find('}') {
                out.extend(body[..end].split(',').map(str::trim));
            }
        }
    }
    out
}

/// Inspect the preamble for packages and primitives that require LuaTeX or a
/// Unicode engine, after any explicit program directive.
pub fn detect_required_engine_from_source(source: &str) -> Option<EngineKind> {
    if let Some(engine) = detect_program_directive(source) {
        return Some(engine);
    }
    let mut preamble = String::new();
    for line in source.lines() {
        let code = strip_comment(line);
        if let Some(at) = code.find("\\begin{document}") {
            preamble.push_str(&code[..at]);
            break;
        }
        preamble.push_str(code);
        preamble.push('\n');
    }
    let packages = |names: &[&str]| {
        ["\\usepackage", "\\RequirePackage"]
            .iter()
            .flat_map(|cmd| command_arguments(&preamble, cmd))
            .any(|package| names.contains(&package))
    };
    if packages(LUATEX_PACKAGES) || preamble.contains("\\directlua") {
        return Some(EngineKind::LuaTeX);
    }
    let unicode_class = command_arguments(&preamble, "\\documentclass")
        .iter()
        .any(|class| UNICODE_ENGINE_CLASSES.contains(class));
    (unicode_class || packages(UNICODE_ENGINE_PACKAGES)).then_some(EngineKind::XeTeX)
}

/// Inspect a failed pass's log and diagnostics for an engine requirement the
/// library can meet by switching engines: a package that names XeTeX (alone
/// or beside LuaTeX) moves a pdfTeX run to XeTeX, one that needs LuaTeX moves
/// a pdfTeX or XeTeX run to LuaTeX. A LuaTeX run never switches.
pub fn detect_engine_switch_need(
    current: EngineKind,
    log: &str,
    diagnostics: &str,
) -> Option<EngineKind> {
    if current == EngineKind::LuaTeX {
        return None;
    }
    let combined = format!("{log}\n{diagnostics}").to_ascii_lowercase();
    let mentions = |signals: &[&str]| signals.iter().any(|signal| combined.contains(signal));
    if current == EngineKind::PdfTeX
        && mentions(&[
            "xetex or luatex is required",
            "requires either xetex or luatex",
            "you must use xelatex or lualatex",
            "cannot run with pdflatex",
            "requires xetex",
            "xetex is required",
        ])
    {
        return Some(EngineKind::XeTeX);
    }
    mentions(&["luatex is required", "requires luatex", "directlua"]).then_some(EngineKind::LuaTeX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(src: &str) -> Option<EngineKind> {
        detect_required_engine_from_source(src)
    }

    #[test]
    fn lua_only_packages_select_luatex_in_any_preamble_form() {
        let doc = |pre: &str| format!("{pre}\n\\begin{{document}}x\\end{{document}}");
        assert_eq!(detect(&doc(r"\documentclass{article}\usepackage[x]{amsmath,luacode}")), Some(EngineKind::LuaTeX));
        assert_eq!(detect(&doc("\\documentclass{article}\n\\usepackage[opt]\n  { luatexja }")), Some(EngineKind::LuaTeX));
        assert_eq!(detect(&doc(r"\documentclass{article}\RequirePackage{luatextra}")), Some(EngineKind::LuaTeX));
        assert_eq!(detect(&doc(r"\documentclass{article}\directlua{tex.print(1)}")), Some(EngineKind::LuaTeX));
    }

    #[test]
    fn unicode_engine_packages_and_classes_select_xetex_like_texmk() {
        for pre in [
            r"\documentclass{ctexart}",
            r"\documentclass[a4paper]{ctexbook}",
            r"\documentclass{article}\usepackage{ctex}",
            r"\documentclass{article}\usepackage[no-math]{fontspec}",
            r"\documentclass{article}\usepackage{xeCJK}",
            r"\documentclass{article}\usepackage{amsmath,unicode-math}",
            r"\documentclass{article}\RequirePackage{polyglossia}",
        ] {
            assert_eq!(detect(&format!("{pre}\\begin{{document}}x\\end{{document}}")), Some(EngineKind::XeTeX), "{pre}");
        }
        // a LuaTeX-only package wins over a package both Unicode engines run
        assert_eq!(
            detect("\\documentclass{article}\\usepackage{fontspec,luacode}\\begin{document}\\end{document}"),
            Some(EngineKind::LuaTeX)
        );
    }

    #[test]
    fn comments_body_text_and_similar_names_do_not_select_an_engine() {
        assert_eq!(detect("\\documentclass{article}\n% \\usepackage{luacode}\n\\begin{document}\\end{document}"), None);
        assert_eq!(detect("\\documentclass{article}\\usepackage{amsmath} % luacode later\n\\begin{document}\\end{document}"), None);
        assert_eq!(detect("\\documentclass{article}\n\\begin{document}\\usepackage{luacode}\\end{document}"), None);
        assert_eq!(detect("\\documentclass{article}\\usepackage{luacodex}\\usepackage@x{luacode}\\begin{document}\\end{document}"), None);
        assert_eq!(detect("\\documentclass{article}\\usepackage{amsmath}\\begin{document}100\\% luacode\\end{document}"), None);
        assert_eq!(detect("\\documentclass{article}\n% \\usepackage{fontspec}\n\\begin{document}\\end{document}"), None);
        assert_eq!(detect("\\documentclass{article}\\usepackage{fontspecx,ctex-xecjk}\\usepackage@x{fontspec}\\begin{document}\\end{document}"), None);
        assert_eq!(detect("\\documentclass{ctexartx}\\begin{document}\\usepackage{xeCJK}\\end{document}"), None);
    }

    #[test]
    fn directives_win_over_packages() {
        assert_eq!(detect("% !TeX program = lualatex\n\\documentclass{article}"), Some(EngineKind::LuaTeX));
        assert_eq!(detect("%&pdflatex\n\\documentclass{article}\\usepackage{luacode}"), Some(EngineKind::PdfTeX));
    }

    #[test]
    fn engine_switches_follow_the_engine_a_package_names() {
        let from_pdftex = |log: &str| detect_engine_switch_need(EngineKind::PdfTeX, log, "");
        // TeX Live's fontspec error under pdflatex names both Unicode engines; XeTeX is texmk's choice.
        assert_eq!(
            from_pdftex("! Package fontspec Error: The fontspec package requires either XeTeX or LuaTeX."),
            Some(EngineKind::XeTeX)
        );
        assert_eq!(from_pdftex("! Package foo Error: XeTeX or LuaTeX is required"), Some(EngineKind::XeTeX));
        assert_eq!(from_pdftex("Package bar Error: this requires XeTeX"), Some(EngineKind::XeTeX));
        assert_eq!(from_pdftex("! Package baz Error: LuaTeX is required"), Some(EngineKind::LuaTeX));
        assert_eq!(from_pdftex("! Undefined control sequence. \\directlua"), Some(EngineKind::LuaTeX));
        assert_eq!(from_pdftex("! Undefined control sequence. \\foo"), None);
        // an XeTeX run only moves on to LuaTeX; a LuaTeX run never switches
        assert_eq!(
            detect_engine_switch_need(EngineKind::XeTeX, "! Package foo Error: LuaTeX is required", ""),
            Some(EngineKind::LuaTeX)
        );
        assert_eq!(detect_engine_switch_need(EngineKind::XeTeX, "this requires XeTeX", ""), None);
        assert_eq!(detect_engine_switch_need(EngineKind::LuaTeX, "luatex is required", ""), None);
    }
}
