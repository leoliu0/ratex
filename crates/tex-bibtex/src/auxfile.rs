//! .aux file parsing: \citation, \bibdata, \bibstyle.

#[derive(Debug, Default)]
pub struct Aux {
    /// Citation keys in order of first appearance (deduped).
    pub cites: Vec<String>,
    /// Database names from \bibdata (no .bib extension).
    pub bibdata: Vec<String>,
    /// Style name from \bibstyle (no .bst extension).
    pub bibstyle: Option<String>,
    /// A `\citation{*}` was seen: include every database entry.
    pub cite_all: bool,
}

/// Extract the contents of the first brace group starting at `open` (the
/// position of the opening brace). Returns (content, index just past the
/// closing brace). Handles one level of nesting by counting braces.
fn brace_group(bytes: &[u8], open: usize) -> Option<(String, usize)> {
    let mut depth = 0usize;
    let mut i = open;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'{' {
            depth += 1;
            if depth == 1 {
                i += 1;
                continue;
            }
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some((String::from_utf8_lossy(&out).into_owned(), i + 1));
            }
        }
        if depth >= 1 {
            out.push(c);
        }
        i += 1;
    }
    None
}

pub fn parse_aux(text: &str) -> Aux {
    let mut aux = Aux::default();
    for line in text.lines() {
        let lb = line.as_bytes();
        // find a \command at the first nonblank position
        let trimmed = lb.iter().position(|&c| c != b' ' && c != b'\t');
        let Some(start) = trimmed else { continue };
        if lb[start] != b'\\' {
            continue; // @comment or junk: ignore
        }
        // read command name up to '{' or whitespace
        let mut j = start + 1;
        while j < lb.len() && lb[j] != b'{' && lb[j] != b' ' && lb[j] != b'\t' {
            j += 1;
        }
        let cmd = &line[start + 1..j];
        // skip whitespace, then expect '{'
        while j < lb.len() && (lb[j] == b' ' || lb[j] == b'\t') {
            j += 1;
        }
        if j >= lb.len() || lb[j] != b'{' {
            continue;
        }
        match cmd {
            "citation" => {
                if let Some((body, _)) = brace_group(lb, j) {
                    if body.trim() == "*" {
                        aux.cite_all = true;
                        continue;
                    }
                    for key in body.split(',') {
                        let k = key.trim();
                        if k.is_empty() {
                            continue;
                        }
                        if !aux.cites.iter().any(|c| c == k) {
                            aux.cites.push(k.to_string());
                        }
                    }
                }
            }
            "bibdata" => {
                if let Some((body, _)) = brace_group(lb, j) {
                    for name in body.split(',') {
                        let n = name.trim();
                        if !n.is_empty() {
                            aux.bibdata.push(n.to_string());
                        }
                    }
                }
            }
            "bibstyle" => {
                if let Some((body, _)) = brace_group(lb, j) {
                    let n = body.trim();
                    if !n.is_empty() {
                        aux.bibstyle = Some(n.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    aux
}
