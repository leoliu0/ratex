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

/// Inspect source text for unambiguous package/primitive requirements.
pub fn detect_required_engine_from_source(source: &str) -> Option<EngineKind> {
    if let Some(engine) = detect_program_directive(source) {
        return Some(engine);
    }

    let in_preamble = true;
    let mut unicode_math_found = false;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('%') {
            continue;
        }
        if trimmed.contains("\\begin{document}") {
            break;
        }
        if in_preamble {
            if trimmed.contains("\\usepackage{luatexja}")
                || trimmed.contains("\\usepackage{luacode}")
                || trimmed.contains("\\usepackage{luatextra}")
                || trimmed.contains("\\directlua")
            {
                return Some(EngineKind::LuaTeX);
            }
            if trimmed.contains("\\usepackage{unicode-math}")
                || trimmed.contains("\\usepackage[") && trimmed.contains("]{unicode-math}")
            {
                unicode_math_found = true;
            }
        }
    }

    if unicode_math_found {
        return Some(EngineKind::LuaTeX);
    }
    None
}

/// Inspect compilation failure log/diagnostics to detect if the document requires a different engine.
pub fn detect_engine_switch_need(
    current: EngineKind,
    log: &str,
    diagnostics: &str,
) -> Option<EngineKind> {
    let combined = format!("{log}\n{diagnostics}");
    let lower = combined.to_ascii_lowercase();

    // Check for explicit engine assertions or requirements
    if lower.contains("xetex is required")
        || lower.contains("requires xetex")
        || lower.contains("xetex or luatex is required") && current == EngineKind::PdfTeX
        || lower.contains("cannot run with pdflatex")
        || lower.contains("you must use xelatex or lualatex")
    {
        if lower.contains("xecjk") || lower.contains("requires xetex") {
            return Some(EngineKind::XeTeX);
        }
        return Some(if current == EngineKind::LuaTeX {
            EngineKind::XeTeX
        } else {
            EngineKind::LuaTeX
        });
    }

    if lower.contains("luatex is required")
        || lower.contains("requires luatex")
        || lower.contains("directlua") && current != EngineKind::LuaTeX
    {
        return Some(EngineKind::LuaTeX);
    }

    None
}
