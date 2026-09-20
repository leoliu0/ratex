//! Native font model for Unicode OpenType/TrueType shaping, layout, and NFSS fontspec/xeCJK integration.

use std::rc::Rc;
use std::str::FromStr;

#[derive(Clone, Debug)]
pub struct NativeFont {
    pub program: Rc<crate::font_program::FontProgram>,
    pub script: Option<rustybuzz::Script>,
    pub language: Option<rustybuzz::Language>,
    pub features: Vec<rustybuzz::Feature>,
    pub tex_ligatures: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeFontOptions {
    pub path: Option<String>,
    pub extension: Option<String>,
    pub font_index: u32,
    pub style: Option<String>,
    pub weight: Option<u16>,
    pub italic: Option<bool>,
    pub upright_font: Option<String>,
    pub bold_font: Option<String>,
    pub italic_font: Option<String>,
    pub bold_italic_font: Option<String>,
    pub slanted_font: Option<String>,
    pub small_caps_font: Option<String>,
    pub upright_features: Vec<rustybuzz::Feature>,
    pub bold_features: Vec<rustybuzz::Feature>,
    pub italic_features: Vec<rustybuzz::Feature>,
    pub bold_italic_features: Vec<rustybuzz::Feature>,
    pub slanted_features: Vec<rustybuzz::Feature>,
    pub small_caps_features: Vec<rustybuzz::Feature>,
    pub scale: f64,
    pub script: Option<rustybuzz::Script>,
    pub language: Option<rustybuzz::Language>,
    pub features: Vec<rustybuzz::Feature>,
    pub tex_ligatures: bool,
    pub variations: Vec<(ttf_parser::Tag, f32)>,
}

impl Default for NativeFontOptions {
    fn default() -> Self {
        NativeFontOptions {
            path: None,
            extension: None,
            font_index: 0,
            style: None,
            weight: None,
            italic: None,
            upright_font: None,
            bold_font: None,
            italic_font: None,
            bold_italic_font: None,
            slanted_font: None,
            small_caps_font: None,
            upright_features: Vec::new(),
            bold_features: Vec::new(),
            italic_features: Vec::new(),
            bold_italic_features: Vec::new(),
            slanted_features: Vec::new(),
            small_caps_features: Vec::new(),
            scale: 1.0,
            script: None,
            language: None,
            features: Vec::new(),
            tex_ligatures: false,
            variations: Vec::new(),
        }
    }
}
impl NativeFontOptions {
    /// Return effective options with per-shape features (bold, italic, etc.) merged into `features`.
    pub fn effective_options(&self) -> Self {
        let mut opts = self.clone();
        if opts.italic == Some(true) && opts.weight.unwrap_or(400) >= 700 {
            if !opts.bold_italic_features.is_empty() {
                opts.features.extend(opts.bold_italic_features.clone());
            } else {
                opts.features.extend(opts.bold_features.clone());
                opts.features.extend(opts.italic_features.clone());
            }
        } else if opts.weight.unwrap_or(400) >= 700 {
            opts.features.extend(opts.bold_features.clone());
        } else if opts.italic == Some(true) {
            opts.features.extend(opts.italic_features.clone());
        } else {
            opts.features.extend(opts.upright_features.clone());
        }
        if opts.style.as_deref().is_some_and(|s| {
            s.eq_ignore_ascii_case("slanted") || s.eq_ignore_ascii_case("boldslanted")
        }) {
            opts.features.extend(opts.slanted_features.clone());
        }
        if opts
            .features
            .iter()
            .any(|f| f.tag == ttf_parser::Tag::from_bytes(b"smcp"))
        {
            opts.features.extend(opts.small_caps_features.clone());
        }
        opts
    }
}

/// Check if a TeX font name represents a native font specification rather than a classic TFM.
pub fn is_native_font_spec(name: &str) -> bool {
    let trimmed = name.trim().trim_matches('"').trim_matches('\'').trim();
    if trimmed.starts_with("ratex:") || trimmed.starts_with('[') {
        return true;
    }
    // Check if filename ends with native font extension or contains directory separators
    let clean = trimmed.split(':').next().unwrap_or(trimmed);
    let lower = clean.to_ascii_lowercase();
    lower.ends_with(".otf")
        || lower.ends_with(".ttf")
        || lower.ends_with(".ttc")
        || lower.ends_with(".otc")
        || lower.ends_with(".dfont")
        || clean.contains('/')
        || clean.contains('\\')
}

/// Parse a native font string into (selector, options).
///
/// Supported formats:
/// 1. Quoted/unquoted ratex spec: `ratex:{selector}:{options}`
/// 2. Bracketed file spec: `[file.otf]:options` or `[file.otf]`
/// 3. Plain file spec: `file.otf:options` or `file.otf`
pub fn parse_native_font_spec(name: &str) -> Result<(String, NativeFontOptions), String> {
    let s = name.trim().trim_matches('"').trim_matches('\'').trim();

    if s.starts_with("ratex:") {
        let rest = &s[6..];
        let (mut selector, options_str) = parse_ratex_braced_parts(rest)?;
        if selector.starts_with('[') && selector.ends_with(']') {
            selector = selector[1..selector.len() - 1].trim().to_string();
        }
        let options = parse_fontspec_options(&options_str)?;
        return Ok((selector, options));
    }

    if s.starts_with('[') {
        let close = s
            .find(']')
            .ok_or_else(|| "Unclosed `[` in font specification".to_string())?;
        let selector = s[1..close].trim().to_string();
        let rest = s[close + 1..].trim();
        let options_str = if rest.starts_with(':') {
            rest[1..].trim()
        } else {
            ""
        };
        let options = parse_fontspec_options(options_str)?;
        return Ok((selector, options));
    }

    if let Some((sel, opts)) = s.split_once(':') {
        let mut selector = sel.trim().to_string();
        if selector.starts_with('[') && selector.ends_with(']') {
            selector = selector[1..selector.len() - 1].trim().to_string();
        }
        let options = parse_fontspec_options(opts.trim())?;
        return Ok((selector, options));
    }

    // Bare filename or family
    let mut selector = s.to_string();
    if selector.starts_with('[') && selector.ends_with(']') {
        selector = selector[1..selector.len() - 1].trim().to_string();
    }
    Ok((selector, NativeFontOptions::default()))
}

/// Parse `ratex:{selector}:{options}` with balanced brace matching.
fn parse_ratex_braced_parts(input: &str) -> Result<(String, String), String> {
    let trimmed = input.trim();
    if trimmed.starts_with('{') {
        let (selector, after_sel) = extract_balanced_braced(trimmed)?;
        let after_colon = after_sel.trim();
        if !after_colon.starts_with(':') {
            return Ok((selector, String::new()));
        }
        let opts_part = after_colon[1..].trim();
        if opts_part.starts_with('{') {
            let (options, _) = extract_balanced_braced(opts_part)?;
            Ok((selector, options))
        } else {
            Ok((selector, opts_part.to_string()))
        }
    } else {
        // Unbraced ratex:selector:options fallback
        let parts: Vec<&str> = trimmed.splitn(2, ':').collect();
        let selector = parts[0].trim().to_string();
        let options = if parts.len() > 1 {
            parts[1].trim().to_string()
        } else {
            String::new()
        };
        Ok((selector, options))
    }
}

fn extract_balanced_braced(input: &str) -> Result<(String, &str), String> {
    let mut chars = input.char_indices();
    let (_, first) = chars.next().unwrap();
    if first != '{' {
        return Err("Expected `{`".to_string());
    }
    let mut depth = 1usize;
    let mut close_idx = None;
    for (i, c) in chars {
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                close_idx = Some(i);
                break;
            }
        }
    }
    let close = close_idx.ok_or_else(|| "Unmatched `{` in font specification".to_string())?;
    let content = input[1..close].to_string();
    let remainder = &input[close + 1..];
    Ok((content, remainder))
}

/// Parse comma-separated fontspec options, respecting balanced braces `{...}`.
pub fn parse_fontspec_options(input: &str) -> Result<NativeFontOptions, String> {
    let mut options = NativeFontOptions::default();
    if input.trim().is_empty() {
        return Ok(options);
    }

    let items = split_balanced_commas(input);
    for raw_item in items {
        let item = raw_item.trim();
        if item.is_empty() {
            continue;
        }

        // Direct OpenType feature flag: e.g. +liga, -calt, +dlig, +smcp
        if item.starts_with('+') || item.starts_with('-') {
            match rustybuzz::Feature::from_str(item) {
                Ok(feat) => {
                    options.features.push(feat);
                    continue;
                }
                Err(_) => {
                    return Err(format!("Invalid OpenType feature flag `{item}`"));
                }
            }
        }

        if let Some((raw_k, raw_v)) = item.split_once('=') {
            let key = raw_k.trim().to_ascii_lowercase();
            let val = raw_v.trim().trim_matches('"').trim_matches('\'').trim();
            let val = val
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .unwrap_or(val)
                .trim();

            match key.as_str() {
                "path" => {
                    options.path = Some(val.to_string());
                }
                "extension" | "ext" => {
                    let ext = if val.starts_with('.') {
                        val.to_string()
                    } else {
                        format!(".{}", val)
                    };
                    options.extension = Some(ext);
                }
                "fontindex" | "index" => {
                    let idx = val
                        .parse::<u32>()
                        .map_err(|_| format!("Invalid FontIndex value: `{val}`"))?;
                    options.font_index = idx;
                }
                "style" => {
                    apply_style_string(&mut options, val)?;
                    options.style = Some(val.to_owned());
                }
                "weight" => apply_weight_string(&mut options, val)?,
                "uprightfont" | "regularfont" => options.upright_font = Some(val.to_owned()),
                "boldfont" => options.bold_font = Some(val.to_owned()),
                "italicfont" => options.italic_font = Some(val.to_owned()),
                "bolditalicfont" => options.bold_italic_font = Some(val.to_owned()),
                "slantedfont" => options.slanted_font = Some(val.to_owned()),
                "italic" => match val.to_ascii_lowercase().as_str() {
                    "true" | "yes" | "on" => options.italic = Some(true),
                    "false" | "no" | "off" => options.italic = Some(false),
                    other => {
                        return Err(format!("Invalid boolean for Italic: `{other}`"));
                    }
                },
                "smallcapsfont" => {
                    options.small_caps_font = Some(val.to_string());
                }
                "uprightfeatures" | "regularfeatures" => {
                    options.upright_features = parse_feature_list(val)?;
                }
                "boldfeatures" => {
                    options.bold_features = parse_feature_list(val)?;
                }
                "italicfeatures" => {
                    options.italic_features = parse_feature_list(val)?;
                }
                "bolditalicfeatures" => {
                    options.bold_italic_features = parse_feature_list(val)?;
                }
                "slantedfeatures" => {
                    options.slanted_features = parse_feature_list(val)?;
                }
                "smallcapsfeatures" => {
                    options.small_caps_features = parse_feature_list(val)?;
                }
                "kerning" => {
                    let sub_items = split_balanced_commas(val);
                    for sub in sub_items {
                        match sub.trim().to_ascii_lowercase().as_str() {
                            "on" | "yes" | "true" => push_feature(&mut options, b"kern", 1),
                            "off" | "no" | "false" => push_feature(&mut options, b"kern", 0),
                            other => {
                                return Err(format!("Unsupported Kerning option `{other}`"));
                            }
                        }
                    }
                }
                "contextuals" => {
                    let sub_items = split_balanced_commas(val);
                    for sub in sub_items {
                        match sub.trim().to_ascii_lowercase().as_str() {
                            "on" | "yes" | "true" => push_feature(&mut options, b"calt", 1),
                            "off" | "no" | "false" => push_feature(&mut options, b"calt", 0),
                            other => {
                                return Err(format!("Unsupported Contextuals option `{other}`"));
                            }
                        }
                    }
                }
                "scale" => {
                    let scale = val.parse::<f64>().map_err(|_| {
                        format!("Invalid Scale value: `{val}`; use a positive numeric factor")
                    })?;
                    if !scale.is_finite() || scale <= 0.0 {
                        return Err(format!("Scale must be finite and positive, got {scale}"));
                    }
                    options.scale = scale;
                }
                "script" => {
                    let sc = parse_script_tag(val)?;
                    options.script = sc;
                }
                "language" | "lang" => {
                    let lang = parse_language_tag(val)?;
                    options.language = lang;
                }
                "rawfeature" | "feature" | "features" => {
                    let sub_items = split_balanced_commas(val);
                    for sub in sub_items {
                        let f_str = sub.trim();
                        if !f_str.is_empty() {
                            let feat = rustybuzz::Feature::from_str(f_str)
                                .map_err(|_| format!("Invalid RawFeature `{f_str}`"))?;
                            options.features.push(feat);
                        }
                    }
                }
                "ligatures" => {
                    let sub_items = split_balanced_commas(val);
                    for sub in sub_items {
                        match sub.trim().to_ascii_lowercase().as_str() {
                            "tex" => options.tex_ligatures = true,
                            "common" => {
                                push_feature(&mut options, b"liga", 1);
                            }
                            "nocommon" => {
                                push_feature(&mut options, b"liga", 0);
                            }
                            "discretionary" | "rare" => {
                                push_feature(&mut options, b"dlig", 1);
                            }
                            "nodiscretionary" | "norare" => {
                                push_feature(&mut options, b"dlig", 0);
                            }
                            "historic" | "historical" => {
                                push_feature(&mut options, b"hlig", 1);
                            }
                            "nohistoric" | "nohistorical" => {
                                push_feature(&mut options, b"hlig", 0);
                            }
                            "required" => {
                                push_feature(&mut options, b"rlig", 1);
                            }
                            "norequired" => {
                                push_feature(&mut options, b"rlig", 0);
                            }
                            "contextual" => {
                                push_feature(&mut options, b"clig", 1);
                            }
                            "nocontextual" => {
                                push_feature(&mut options, b"clig", 0);
                            }
                            "reset" | "off" => {
                                push_feature(&mut options, b"liga", 0);
                                push_feature(&mut options, b"clig", 0);
                                push_feature(&mut options, b"dlig", 0);
                                push_feature(&mut options, b"hlig", 0);
                                push_feature(&mut options, b"rlig", 0);
                                options.tex_ligatures = false;
                            }
                            other => {
                                return Err(format!("Unsupported Ligatures option `{other}`"));
                            }
                        }
                    }
                }
                "numbers" => {
                    let sub_items = split_balanced_commas(val);
                    for sub in sub_items {
                        match sub.trim().to_ascii_lowercase().as_str() {
                            "oldstyle" | "lowercase" => push_feature(&mut options, b"onum", 1),
                            "lining" | "uppercase" => push_feature(&mut options, b"lnum", 1),
                            "proportional" => push_feature(&mut options, b"pnum", 1),
                            "monospaced" | "tabular" => push_feature(&mut options, b"tnum", 1),
                            "slashedzero" => push_feature(&mut options, b"zero", 1),
                            "fraction" | "fractions" => push_feature(&mut options, b"frac", 1),
                            "reset" => {
                                push_feature(&mut options, b"onum", 0);
                                push_feature(&mut options, b"lnum", 0);
                                push_feature(&mut options, b"pnum", 0);
                                push_feature(&mut options, b"tnum", 0);
                                push_feature(&mut options, b"zero", 0);
                                push_feature(&mut options, b"frac", 0);
                            }
                            other => {
                                return Err(format!("Unsupported Numbers option `{other}`"));
                            }
                        }
                    }
                }
                "variation" | "variations" => {
                    let sub_items = split_balanced_commas(val);
                    for sub in sub_items {
                        if let Some((axis_k, axis_v)) = sub.split_once('=') {
                            let tag = ttf_parser::Tag::from_bytes_lossy(axis_k.trim().as_bytes());
                            let val = axis_v
                                .trim()
                                .parse::<f32>()
                                .map_err(|_| format!("Invalid variation value in `{sub}`"))?;
                            options.variations.push((tag, val));
                        } else {
                            return Err(format!("Invalid variation spec `{sub}`"));
                        }
                    }
                }
                other => {
                    // Try parsing as feature=val
                    if let Ok(val_num) = val.parse::<u32>() {
                        if other.len() <= 4 {
                            let tag = ttf_parser::Tag::from_bytes_lossy(other.as_bytes());
                            options
                                .features
                                .push(rustybuzz::Feature::new(tag, val_num, ..));
                            continue;
                        }
                    }
                    return Err(format!("Unsupported or unrecognized font option `{key}`"));
                }
            }
        } else {
            // Unrecognized option with no `=` or `+/-`
            return Err(format!("Unsupported or unrecognized font option `{item}`"));
        }
    }

    Ok(options)
}

fn split_balanced_commas(input: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;

    for (i, c) in input.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ',' if depth == 0 => {
                result.push(&input[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < input.len() {
        result.push(&input[start..]);
    }
    result
}

fn apply_style_string(options: &mut NativeFontOptions, style: &str) -> Result<(), String> {
    let s = style.to_ascii_lowercase();
    match s.as_str() {
        "regular" | "upright" | "roman" | "normal" => {
            options.weight = Some(400);
            options.italic = Some(false);
            Ok(())
        }
        "bold" => {
            options.weight = Some(700);
            options.italic = Some(false);
            Ok(())
        }
        "italic" | "oblique" | "slanted" => {
            options.weight = Some(400);
            options.italic = Some(true);
            Ok(())
        }
        "bolditalic" | "bold italic" | "boldoblique" | "bold oblique" | "boldslanted"
        | "bold slanted" => {
            options.weight = Some(700);
            options.italic = Some(true);
            Ok(())
        }
        "light" => {
            options.weight = Some(300);
            options.italic = Some(false);
            Ok(())
        }
        "lightitalic" | "light italic" => {
            options.weight = Some(300);
            options.italic = Some(true);
            Ok(())
        }
        "medium" => {
            options.weight = Some(500);
            options.italic = Some(false);
            Ok(())
        }
        "mediumitalic" | "medium italic" => {
            options.weight = Some(500);
            options.italic = Some(true);
            Ok(())
        }
        "semibold" | "demibold" => {
            options.weight = Some(600);
            options.italic = Some(false);
            Ok(())
        }
        "semibolditalic" | "semibold italic" | "demibolditalic" | "demibold italic" => {
            options.weight = Some(600);
            options.italic = Some(true);
            Ok(())
        }
        "black" | "heavy" | "extrabold" => {
            options.weight = Some(800);
            options.italic = Some(false);
            Ok(())
        }
        "blackitalic" | "black italic" | "heavyitalic" | "heavy italic" => {
            options.weight = Some(800);
            options.italic = Some(true);
            Ok(())
        }
        _ => Err(format!("Unsupported or unrecognized Style `{style}`")),
    }
}

fn apply_weight_string(options: &mut NativeFontOptions, weight: &str) -> Result<(), String> {
    let w = weight.to_ascii_lowercase();
    match w.as_str() {
        "thin" | "hairline" => {
            options.weight = Some(100);
            Ok(())
        }
        "extralight" | "ultralight" => {
            options.weight = Some(200);
            Ok(())
        }
        "light" => {
            options.weight = Some(300);
            Ok(())
        }
        "regular" | "normal" => {
            options.weight = Some(400);
            Ok(())
        }
        "medium" => {
            options.weight = Some(500);
            Ok(())
        }
        "semibold" | "demibold" => {
            options.weight = Some(600);
            Ok(())
        }
        "bold" => {
            options.weight = Some(700);
            Ok(())
        }
        "extrabold" | "ultrabold" => {
            options.weight = Some(800);
            Ok(())
        }
        "black" | "heavy" => {
            options.weight = Some(900);
            Ok(())
        }
        _ => Err(format!("Unsupported or unrecognized Weight `{weight}`")),
    }
}

/// Expand fontspec `*` wildcard in explicit font filenames to the base font selector.
pub fn expand_font_wildcard(name: &str, base: &str) -> String {
    let clean_base = base
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(base.trim());
    let clean_base = clean_base
        .strip_suffix(".otf")
        .or_else(|| clean_base.strip_suffix(".ttf"))
        .or_else(|| clean_base.strip_suffix(".OTF"))
        .or_else(|| clean_base.strip_suffix(".TTF"))
        .unwrap_or(clean_base);
    if name.contains('*') {
        name.replace('*', clean_base)
    } else {
        name.to_string()
    }
}

fn parse_feature_list(input: &str) -> Result<Vec<rustybuzz::Feature>, String> {
    let mut features = Vec::new();
    let items = split_balanced_commas(input);
    for item in items {
        let f_str = item.trim().trim_matches('"').trim_matches('\'').trim();
        let f_str = f_str
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(f_str)
            .trim();
        if f_str.is_empty() {
            continue;
        }
        let feat = rustybuzz::Feature::from_str(f_str)
            .map_err(|_| format!("Invalid feature spec `{f_str}`"))?;
        features.push(feat);
    }
    Ok(features)
}

fn parse_script_tag(val: &str) -> Result<Option<rustybuzz::Script>, String> {
    let s = val.trim().to_ascii_lowercase();
    if s.is_empty() || s == "default" {
        return Ok(None);
    }
    let script = match s.as_str() {
        "latin" | "latn" => rustybuzz::script::LATIN,
        "cjk" | "han" | "hani" | "hans" | "hant" | "chinese" => rustybuzz::script::HAN,
        "kana" | "japanese" | "hira" | "kata" => rustybuzz::script::HIRAGANA,
        "hangul" | "korean" | "hang" => rustybuzz::script::HANGUL,
        "cyrillic" | "cyrl" | "russian" => rustybuzz::script::CYRILLIC,
        "greek" | "grek" => rustybuzz::script::GREEK,
        "arabic" | "arab" => rustybuzz::script::ARABIC,
        "hebrew" | "hebr" => rustybuzz::script::HEBREW,
        "devanagari" | "deva" => rustybuzz::script::DEVANAGARI,
        _ => {
            let tag = ttf_parser::Tag::from_bytes_lossy(val.as_bytes());
            rustybuzz::Script::from_iso15924_tag(tag)
                .ok_or_else(|| format!("Invalid or unrecognized Script tag `{val}`"))?
        }
    };
    Ok(Some(script))
}

fn parse_language_tag(val: &str) -> Result<Option<rustybuzz::Language>, String> {
    let v = val.trim();
    if v.eq_ignore_ascii_case("default") || v.is_empty() {
        return Ok(None);
    }
    let l = match v.to_ascii_lowercase().as_str() {
        "japanese" | "ja" | "jan" => rustybuzz::Language::from_str("JAN").unwrap(),
        "chinese" | "zh" | "zhs" => rustybuzz::Language::from_str("ZHS").unwrap(),
        "traditional chinese" | "zht" => rustybuzz::Language::from_str("ZHT").unwrap(),
        "korean" | "ko" | "kor" => rustybuzz::Language::from_str("KOR").unwrap(),
        "english" | "en" | "eng" => rustybuzz::Language::from_str("ENG").unwrap(),
        "german" | "de" | "deu" => rustybuzz::Language::from_str("DEU").unwrap(),
        "french" | "fr" | "fra" => rustybuzz::Language::from_str("FRA").unwrap(),
        "spanish" | "es" | "esp" => rustybuzz::Language::from_str("ESP").unwrap(),
        "italian" | "it" | "ita" => rustybuzz::Language::from_str("ITA").unwrap(),
        "dutch" | "nl" | "nld" => rustybuzz::Language::from_str("NLD").unwrap(),
        "polish" | "pl" | "pol" => rustybuzz::Language::from_str("PLK").unwrap(),
        "portuguese" | "pt" | "por" => rustybuzz::Language::from_str("PTG").unwrap(),
        "russian" | "ru" | "rus" => rustybuzz::Language::from_str("RUS").unwrap(),
        "greek" | "el" | "ell" => rustybuzz::Language::from_str("ELL").unwrap(),
        "swedish" | "sv" | "sve" => rustybuzz::Language::from_str("SVE").unwrap(),
        "turkish" | "tr" | "tur" => rustybuzz::Language::from_str("TRK").unwrap(),
        other => rustybuzz::Language::from_str(other)
            .map_err(|_| format!("Invalid Language tag `{val}`"))?,
    };
    Ok(Some(l))
}

fn push_feature(options: &mut NativeFontOptions, tag_bytes: &[u8; 4], value: u32) {
    let tag = ttf_parser::Tag::from_bytes(tag_bytes);
    options
        .features
        .push(rustybuzz::Feature::new(tag, value, ..));
}

/// Check if an OpenType face supports a specific 4-byte feature tag in GSUB, GPOS, or KERN.
pub fn face_supports_feature(face: &ttf_parser::Face<'_>, tag: ttf_parser::Tag) -> bool {
    // TeX ligatures are handled by the engine layout pipeline
    if tag == ttf_parser::Tag::from_bytes(b"tlig") {
        return true;
    }
    // Kerning can be in GPOS or legacy kern table
    if tag == ttf_parser::Tag::from_bytes(b"kern") {
        if face.tables().kern.is_some() {
            return true;
        }
        if let Some(gpos) = face.tables().gpos {
            if gpos.features.find(tag).is_some() {
                return true;
            }
        }
        return false;
    }
    // GSUB table lookup
    if let Some(gsub) = face.tables().gsub {
        if gsub.features.find(tag).is_some() {
            return true;
        }
    }
    // GPOS table lookup
    if let Some(gpos) = face.tables().gpos {
        if gpos.features.find(tag).is_some() {
            return true;
        }
    }
    false
}

/// Validate that requested active features and explicit styles are supported by the face.
pub fn validate_face_features_and_style(
    face: &ttf_parser::Face<'_>,
    selector: &str,
    options: &NativeFontOptions,
) -> Result<(), String> {
    // Validate requested active OpenType features
    for feat in &options.features {
        if feat.value > 0 && !face_supports_feature(face, feat.tag) {
            return Err(format!(
                "Requested OpenType feature `{}` is not available in font `{}`",
                feat.tag, selector
            ));
        }
    }

    // Validate requested explicit styles
    if let Some(style) = &options.style {
        let s = style.to_ascii_lowercase();
        match s.as_str() {
            "bold" => {
                if !face.is_bold() && face.weight().to_number() < 600 {
                    return Err(format!(
                        "Requested style `Bold` is not available in font file `{selector}`"
                    ));
                }
            }
            "italic" | "oblique" => {
                let has_italic =
                    face.is_italic() || face.italic_angle().map(|a| a != 0.0).unwrap_or(false);
                if !has_italic {
                    return Err(format!(
                        "Requested style `Italic` is not available in font file `{selector}`"
                    ));
                }
            }
            "bolditalic" | "bold italic" | "boldoblique" | "bold oblique" => {
                let bold = face.is_bold() || face.weight().to_number() >= 600;
                let italic =
                    face.is_italic() || face.italic_angle().map(|a| a != 0.0).unwrap_or(false);
                if !bold || !italic {
                    return Err(format!(
                        "Requested style `BoldItalic` is not available in font file `{selector}`"
                    ));
                }
            }
            "slanted" | "boldslanted" => {
                let has_slant =
                    face.is_italic() || face.italic_angle().map(|a| a != 0.0).unwrap_or(false);
                if !has_slant {
                    return Err(format!(
                        "Requested style `Slanted` is not available in font file `{selector}`"
                    ));
                }
            }
            _ => {}
        }
    }

    Ok(())
}
