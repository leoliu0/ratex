//! `kpse`: LuaTeX's Kpathsea library (`lkpselib.c`) over the in-tree
//! resolver (`tex-kpse`): the TDS roots with their `ls-R` databases and the
//! bundled package archive. Files that live only inside the archive have
//! readable paths in its read-only virtual TDS tree. Native paths use `/`
//! on Windows; path lists use `;` there and `:` on Unix, as Kpathsea does.
//!
//! There is no `texmf.cnf`: configuration variables come from the
//! environment first (as in Kpathsea) and then from the table of
//! [`CNF_DEFAULTS`], which carries TeX Live's stock values for the
//! parameters LuaTeX scripts ask for.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString};

use crate::lua_bridge::with_engine;
use crate::lua_sys::{bytes_of, kpse_path, path_bytes, path_of, shell_escape, sys_reg, ShellEscape};

pub(crate) const PRELUDE: &str = include_str!("lua_sys_kpse.lua");

// Kpathsea's ENV_SEP; Windows consumers must not split drive letters at `:`.
const PATH_SEPARATOR: &str = if cfg!(windows) { ";" } else { ":" };

/// How a file format is searched.
#[derive(Clone, Copy)]
enum Search {
    /// One of `tex-kpse`'s formats.
    Format(tex_kpse::Format),
    /// Any file below the TDS roots, found by name.
    Any,
}

struct FormatInfo {
    name: &'static str,
    search: Search,
    /// Path variable (`TEXINPUTS`).
    var: &'static str,
    /// Default path below each TDS root: `tex/{$progname,generic,latex,}//`.
    path: &'static str,
    suffixes: &'static [&'static str],
    alt_suffixes: &'static [&'static str],
}

macro_rules! fmt {
    ($name:literal, $search:expr, $var:literal, $path:literal, [$($s:literal),*], [$($a:literal),*]) => {
        FormatInfo { name: $name, search: $search, var: $var, path: $path, suffixes: &[$($s),*], alt_suffixes: &[$($a),*] }
    };
}

use tex_kpse::Format as F;
use Search::{Any, Format};

/// The file types of `kpse.find_file`, in `lkpselib.c` order.
const FORMATS: &[FormatInfo] = &[
    fmt!("gf", Any, "GFFONTS", "fonts/gf//", [".gf"], []),
    fmt!("pk", Any, "PKFONTS", "fonts/pk//", [".pk"], []),
    fmt!("bitmap font", Any, "GLYPHFONTS", "fonts/pk//", [], []),
    fmt!("tfm", Format(F::Tfm), "TFMFONTS", "fonts/tfm//", [".tfm"], []),
    fmt!("afm", Any, "AFMFONTS", "fonts/afm//", [".afm"], []),
    fmt!("base", Any, "MFBASES", "metafont/base//", [".base"], []),
    fmt!("bib", Format(F::Bib), "BIBINPUTS", "bibtex/bib//", [".bib"], []),
    fmt!("bst", Format(F::Bst), "BSTINPUTS", "bibtex/bst//", [".bst"], []),
    fmt!("cnf", Any, "TEXMFCNF", "web2c", [".cnf"], []),
    fmt!("ls-R", Any, "TEXMFDBS", "", [], []),
    fmt!("fmt", Any, "TEXFORMATS", "web2c{/$engine,}", [".fmt", ".efmt", ".efm", ".oefmt", ".ofmt", ".yap"], []),
    fmt!("map", Format(F::Map), "TEXFONTMAPS", "fonts/map/{$progname,pdftex,dvips,}//", [".map"], []),
    fmt!("mem", Any, "MPMEMS", "metapost/base", [".mem"], []),
    fmt!("mf", Any, "MFINPUTS", "metafont//", [".mf"], []),
    fmt!("mfpool", Any, "MFPOOL", "web2c", [".pool"], []),
    fmt!("mft", Any, "MFTINPUTS", "mft//", [".mft"], []),
    fmt!("mp", Any, "MPINPUTS", "metapost//", [".mp"], []),
    fmt!("mppool", Any, "MPPOOL", "web2c", [".pool"], []),
    fmt!("MetaPost support", Any, "MPSUPPORT", "metapost/support", [], []),
    fmt!("ocp", Any, "OCPINPUTS", "omega/ocp//", [".ocp"], []),
    fmt!("ofm", Format(F::Tfm), "OFMFONTS", "fonts/{ofm,tfm}//", [".ofm", ".tfm"], []),
    fmt!("opl", Any, "OPLFONTS", "fonts/opl//", [".opl"], []),
    fmt!("otp", Any, "OTPINPUTS", "omega/otp//", [".otp"], []),
    fmt!("ovf", Format(F::Vf), "OVFFONTS", "fonts/{ovf,vf}//", [".ovf", ".vf"], []),
    fmt!("ovp", Any, "OVPFONTS", "fonts/ovp//", [".ovp"], []),
    fmt!("graphic/figure", Any, "TEXPICTS", "tex/{$progname,generic,latex,}//", [".eps", ".epsi"], [".png", ".pdf", ".jpg", ".jpeg", ".jbig2", ".jb2", ".mps"]),
    fmt!("tex", Format(F::Tex), "TEXINPUTS", "tex/{$progname,generic,latex,}//", [".tex"], [".sty", ".cls", ".fd", ".aux", ".bbl", ".def", ".clo", ".ldf"]),
    fmt!("TeX system documentation", Any, "TEXDOCS", "doc//", [], []),
    fmt!("texpool", Any, "TEXPOOL", "web2c", [".pool"], []),
    fmt!("TeX system sources", Any, "TEXSOURCES", "source//", [], []),
    fmt!("PostScript header", Any, "TEXPSHEADERS", "dvips//", [".pro"], []),
    fmt!("Troff fonts", Any, "TRFONTS", "fonts/tr//", [], []),
    fmt!("type1 fonts", Format(F::Type1), "T1FONTS", "fonts/type1//", [".pfa", ".pfb"], []),
    fmt!("vf", Format(F::Vf), "VFFONTS", "fonts/vf//", [".vf"], []),
    fmt!("dvips config", Any, "TEXCONFIG", "dvips//", [], []),
    fmt!("ist", Any, "TEXINDEXSTYLE", "makeindex//", [".ist"], []),
    fmt!("truetype fonts", Format(F::Truetype), "TTFONTS", "fonts/{truetype,opentype}//", [".ttf", ".ttc", ".TTF", ".TTC", ".dfont"], []),
    fmt!("type42 fonts", Any, "T42FONTS", "fonts/type42//", [], []),
    fmt!("web2c files", Any, "WEB2C", "web2c", [], []),
    fmt!("other text files", Any, "TEXMFSCRIPTS", "", [], []),
    fmt!("other binary files", Any, "TEXMFSCRIPTS", "", [], []),
    fmt!("misc fonts", Any, "MISCFONTS", "fonts/misc//", [], []),
    fmt!("web", Any, "WEBINPUTS", "web//", [".web", ".ch"], []),
    fmt!("cweb", Any, "CWEBINPUTS", "cweb//", [".w", ".web", ".ch"], []),
    fmt!("enc files", Format(F::Enc), "ENCFONTS", "fonts/enc//", [".enc"], []),
    fmt!("cmap files", Any, "CMAPFONTS", "fonts/cmap//", [], []),
    fmt!("subfont definition files", Any, "SFDFONTS", "fonts/sfd//", [".sfd"], []),
    fmt!("opentype fonts", Format(F::Otf), "OPENTYPEFONTS", "fonts/{opentype,truetype}//", [".otf", ".ttf", ".ttc", ".TTF", ".TTC", ".dfont"], []),
    fmt!("pdftex config", Any, "PDFTEXCONFIG", "pdftex//", [], []),
    fmt!("lig files", Any, "LIGFONTS", "fonts/lig//", [".lig"], []),
    fmt!("texmfscripts", Any, "TEXMFSCRIPTS", "scripts//", [], []),
    fmt!("lua", Format(F::Lua), "LUAINPUTS", "scripts/{$progname,$engine,}/{lua,}//:tex/{luatex,plain,generic,latex,}//", [".luatex", ".lua", ".luc", ".luctex", ".texlua", ".texluc", ".tlu"], []),
    fmt!("font feature files", Any, "FONTFEATURES", "fonts/fea//", [".fea"], []),
    fmt!("cid maps", Any, "FONTCIDMAPS", "fonts/cid//", [], []),
    fmt!("mlbib", Any, "MLBIBINPUTS", "bibtex/bib//", [".mlbib", ".bib"], []),
    fmt!("mlbst", Any, "MLBSTINPUTS", "bibtex/bst//", [".mlbst", ".bst"], []),
    fmt!("clua", Any, "CLUAINPUTS", "scripts/{$progname,$engine,}/lib{,/lua}//", [".dll", ".so"], []),
];

/// TeX Live's `texmf.cnf` values for the parameters that scripts query.
const CNF_DEFAULTS: &[(&str, &str)] = &[
    ("TEXMFHOME", "~/texmf"),
    ("TEXMFCONFIG", "~/.texlive/texmf-config"),
    ("TEXMFSYSCONFIG", "/etc/texmf"),
    ("TEXMFLOCAL", if cfg!(windows) { "/usr/local/share/texmf;/usr/share/texmf" } else { "/usr/local/share/texmf:/usr/share/texmf" }),
    ("TEXMFDIST", "/usr/share/texmf-dist"),
    ("OSFONTDIR", "/usr/share/fonts"),
    ("shell_escape", "p"),
    (
        "shell_escape_commands",
        "bibtex,bibtex8,extractbb,gregorio,kpsewhich,l3sys-query,latexminted,makeindex,memoize-extract.pl,memoize-extract.py,repstopdf,r-mpost,texosquery-jre8,",
    ),
    ("openout_any", "p"),
    ("openin_any", "a"),
    ("parse_first_line", "t"),
    ("file_line_error_style", "f"),
    ("max_print_line", "79"),
    ("error_line", "79"),
    ("half_error_line", "50"),
    ("max_strings", "500000"),
    ("strings_free", "100"),
    ("buf_size", "200000"),
    ("pool_size", "6250000"),
    ("string_vacancies", "90000"),
    ("pool_free", "47500"),
    ("main_memory", "5000000"),
    ("extra_mem_top", "0"),
    ("extra_mem_bot", "0"),
    ("font_mem_size", "8000000"),
    ("font_max", "9000"),
    ("hash_extra", "600000"),
    ("nest_size", "500"),
    ("param_size", "10000"),
    ("save_size", "100000"),
    ("stack_size", "10000"),
    ("expand_depth", "10000"),
    ("max_in_open", "15"),
    ("trie_size", "1000000"),
    ("hyph_size", "8191"),
    ("dvi_buf_size", "16384"),
    ("TEXMFOUTPUT", ""),
];

thread_local! {
    /// `texconfig.shell_escape_commands`, when a startup script set it.
    static ALLOWED: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    /// The program name of the default Kpathsea instance (`$progname`).
    static PROGRAM: RefCell<String> = RefCell::new("luahbtex".to_string());
    static PROGRAM_SET: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

fn home() -> Option<String> {
    std::env::var("HOME").ok()
}

fn exe_dirs() -> (PathBuf, PathBuf, PathBuf) {
    let exe = std::env::current_exe().ok().and_then(|p| std::fs::canonicalize(p).ok()).unwrap_or_default();
    let loc = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let dir = loc.parent().map(Path::to_path_buf).unwrap_or_default();
    let parent = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    (loc, dir, parent)
}

fn roots() -> Vec<PathBuf> {
    with_engine(|e| e.font_loader.kpse.roots().to_vec()).unwrap_or_default()
}

// ---------------------------------------------------------- variables ---

/// Raw (unexpanded) value of `name`: the environment, then the built-ins.
fn raw_var(program: &str, name: &str) -> Option<String> {
    for key in [format!("{name}_{program}"), format!("{name}.{program}"), name.to_string()] {
        if let Ok(value) = std::env::var(&key) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    match name {
        "progname" => return Some(program.to_string()),
        "engine" => return Some(crate::lua_sys_status::engine_name().to_string()),
        "SELFAUTOLOC" => return Some(kpse_path(&exe_dirs().0)),
        "SELFAUTODIR" => return Some(kpse_path(&exe_dirs().1)),
        "SELFAUTOPARENT" => return Some(kpse_path(&exe_dirs().2)),
        "SELFAUTOGRANDPARENT" => {
            let (_, _, parent) = exe_dirs();
            return Some(kpse_path(parent.parent().unwrap_or(Path::new(""))));
        }
        // The per-user cache: font databases and caches live below it.
        "TEXMFVAR" | "TEXMFSYSVAR" => {
            return Some(kpse_path(&crate::lua_sys::cache_dir().join("texmf-var")))
        }
        "TEXMFCACHE" => return Some("$TEXMFVAR".to_string()),
        // The search roots of the engine, then the bundled archive.
        "TEXMF" => {
            let mut list: Vec<String> = roots().iter().map(|r| kpse_path(r)).collect();
            list.push(tex_kpse::embedded_tree::ROOT.to_string());
            return Some(format!("{{{}}}", list.join(",")));
        }
        "shell_escape" => {
            return Some(
                match shell_escape() {
                    ShellEscape::Disabled => "f",
                    ShellEscape::Restricted => "p",
                    ShellEscape::Enabled => "t",
                }
                .to_string(),
            )
        }
        _ => {}
    }
    if let Some(fmt) = FORMATS.iter().find(|f| f.var == name) {
        return Some(default_path_template(fmt));
    }
    CNF_DEFAULTS.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
}

/// The unexpanded default search path of a format: `.` plus each TDS spec
/// below `$TEXMF`.
fn default_path_template(fmt: &FormatInfo) -> String {
    if fmt.path.is_empty() {
        return ".".to_string();
    }
    let specs: Vec<String> = fmt.path.split(':').map(|spec| format!("$TEXMF/{spec}")).collect();
    // Font searches also cover the operating system's fonts (`$OSFONTDIR//`).
    let system = if matches!(fmt.var, "OPENTYPEFONTS" | "TTFONTS" | "T1FONTS" | "AFMFONTS") {
        if cfg!(windows) { ";$OSFONTDIR//" } else { ":$OSFONTDIR//" }
    } else { "" };
    format!(".{PATH_SEPARATOR}{}{system}", specs.join(PATH_SEPARATOR))
}

/// `$VAR` / `${VAR}` expansion as `kpathsea_var_expand`.
fn expand_var(program: &str, text: &str) -> String {
    expand_var_depth(program, text, 0)
}

fn expand_var_depth(program: &str, text: &str, depth: u32) -> String {
    let b = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' {
            let ch = text[i..].chars().next().expect("char");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
        let var_char = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        if i < b.len() && b[i] == b'{' {
            let Some(close) = text[i + 1..].find('}') else {
                eprintln!("warning: kpathsea: {text}: No matching }} for ${{.");
                return out;
            };
            let name = &text[i + 1..i + 1 + close];
            i += close + 2;
            if name.bytes().all(var_char) && !name.is_empty() {
                expand_one(program, name, &mut out, depth);
            }
        } else if i < b.len() && var_char(b[i]) {
            let start = i;
            while i < b.len() && var_char(b[i]) {
                i += 1;
            }
            expand_one(program, &text[start..i], &mut out, depth);
        } else {
            let next = text[i..].chars().next().map(String::from).unwrap_or_default();
            eprintln!("warning: kpathsea: {text}: Unrecognized variable construct `${next}'.");
            out.push('$');
        }
    }
    out
}

fn expand_one(program: &str, name: &str, out: &mut String, depth: u32) {
    match raw_var(program, name) {
        Some(value) if depth < 20 => out.push_str(&expand_var_depth(program, &value, depth + 1)),
        Some(value) => out.push_str(&value),
        None => {
            out.push('$');
            out.push_str(name);
        }
    }
}

fn var_value(program: &str, name: &str) -> Option<String> {
    raw_var(program, name).map(|value| expand_var(program, &value))
}

// ---------------------------------------------------------- expansion ---

/// Brace expansion of one path element; alternatives of the first group
/// vary fastest.
fn brace_elt(elt: &str) -> Vec<String> {
    let b = elt.as_bytes();
    let Some(open) = b.iter().position(|&c| c == b'{') else {
        return vec![elt.replace('}', "")];
    };
    let mut depth = 0;
    let mut close = None;
    for (at, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(at);
                    break;
                }
            }
            _ => {}
        }
    }
    let (inner_end, rest_start) = match close {
        Some(at) => (at, at + 1),
        None => {
            eprintln!("warning: kpathsea: {elt}: Unmatched {{.");
            (b.len(), b.len())
        }
    };
    let prefix = &elt[..open];
    let inner = &elt[open + 1..inner_end];
    let postfix = &elt[rest_start..];
    // alternatives of the group, split at depth-0 commas
    let mut alternatives = Vec::new();
    let mut depth = 0;
    let mut from = 0;
    for (at, c) in inner.bytes().enumerate() {
        match c {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b',' if depth == 0 => {
                alternatives.push(&inner[from..at]);
                from = at + 1;
            }
            _ => {}
        }
    }
    alternatives.push(&inner[from..]);
    let alts: Vec<String> = alternatives.into_iter().flat_map(brace_elt).collect();
    let rest = brace_elt(postfix);
    let mut out = Vec::with_capacity(alts.len() * rest.len());
    for r in &rest {
        for a in &alts {
            out.push(format!("{prefix}{a}{r}"));
        }
    }
    out
}

/// Split at the platform path separator, not inside braces. On Unix also
/// accept `;`, as before; on Windows `:` belongs to the drive letter.
fn split_path(path: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut from = 0;
    for (at, c) in path.bytes().enumerate() {
        match c {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b';' | b':' if depth <= 0 && (c == b';' || !cfg!(windows)) => {
                parts.push(&path[from..at]);
                from = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&path[from..]);
    parts
}

fn expand_braces(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for elt in split_path(text) {
        out.extend(brace_elt(elt));
    }
    out.join(PATH_SEPARATOR)
}

fn tilde(elt: &str) -> String {
    let Some(rest) = elt.strip_prefix('~') else { return elt.to_string() };
    let (user, tail) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    if user.is_empty() {
        return match home() {
            Some(h) => format!("{h}{tail}"),
            None => elt.to_string(),
        };
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        if let Ok(name) = CString::new(user) {
            let pw = unsafe { libc::getpwnam(name.as_ptr()) };
            if !pw.is_null() {
                let dir = unsafe { std::ffi::CStr::from_ptr((*pw).pw_dir) };
                return format!("{}{tail}", dir.to_string_lossy());
            }
        }
    }
    elt.to_string()
}

/// Whether `path` is a directory: on disk or in the bundled archive.
pub(crate) fn is_dir(path: &str) -> bool {
    use tex_kpse::embedded_tree::{self, EmbeddedKind};
    if embedded_tree::is_embedded_path(path) {
        return embedded_tree::stat(path) == Some(EmbeddedKind::Directory);
    }
    Path::new(path).is_dir()
}

/// Whether `path` is a regular file: on disk or in the bundled archive.
pub(crate) fn is_file(path: &str) -> bool {
    use tex_kpse::embedded_tree::{self, EmbeddedKind};
    if embedded_tree::is_embedded_path(path) {
        return matches!(embedded_tree::stat(path), Some(EmbeddedKind::File { .. }));
    }
    Path::new(path).is_file()
}

/// Names of the subdirectories of `dir`, sorted.
fn child_directories(dir: &str) -> Vec<String> {
    if tex_kpse::embedded_tree::is_embedded_path(dir) {
        let entries = tex_kpse::embedded_tree::read_dir(dir).unwrap_or_default();
        return entries.into_iter().filter(|(_, directory)| *directory).map(|(name, _)| name).collect();
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut children: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    children.sort();
    children
}

/// Directories below (and including) `dir`, as `//` path elements list them.
fn subdirectories(dir: &str, out: &mut Vec<String>, budget: &mut usize) {
    out.push(dir.to_string());
    let base = dir.trim_end_matches('/');
    for child in child_directories(dir) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        subdirectories(&format!("{base}/{child}"), out, budget);
    }
}

/// `kpathsea_path_expand`: variables, braces and `~`, then the existing
/// directories (every subdirectory for a `//` element).
fn expand_path(program: &str, path: &str) -> String {
    let expanded = expand_var(program, path);
    let mut dirs: Vec<String> = Vec::new();
    for elt in split_path(&expanded) {
        for item in brace_elt(elt) {
            let item = tilde(item.trim_start_matches("!!"));
            let recursive = item.ends_with("//");
            let trimmed = item.trim_end_matches('/');
            let trimmed = if trimmed.is_empty() && item.starts_with('/') { "/" } else { trimmed };
            if trimmed.is_empty() {
                continue;
            }
            if !is_dir(trimmed) {
                continue;
            }
            if recursive {
                let mut budget = 100_000;
                subdirectories(trimmed, &mut dirs, &mut budget);
            } else {
                dirs.push(trimmed.to_string());
            }
        }
    }
    dirs.join(PATH_SEPARATOR)
}

/// `kpse.show_path`: the search path of a format, with variables and
/// braces expanded.
fn show_path(program: &str, fmt: &FormatInfo) -> String {
    let raw = raw_var(program, fmt.var).unwrap_or_else(|| ".".to_string());
    let expanded = expand_var(program, &raw);
    let mut out: Vec<String> = Vec::new();
    for elt in split_path(&expanded) {
        for item in brace_elt(elt) {
            out.push(item);
        }
    }
    out.join(PATH_SEPARATOR)
}

// ------------------------------------------------------------- lookup ---


fn has_suffix(name: &str, suffixes: &[&str]) -> bool {
    let lower = name.to_ascii_lowercase();
    suffixes.iter().any(|s| lower.ends_with(&s.to_ascii_lowercase()))
}

/// Candidate file names for `name` in `fmt`: with a known suffix only the
/// name, else each suffix appended and the bare name last.
fn candidates(name: &str, fmt: &FormatInfo) -> Vec<String> {
    if has_suffix(name, fmt.suffixes) || has_suffix(name, fmt.alt_suffixes) || fmt.suffixes.is_empty() {
        return vec![name.to_string()];
    }
    let mut out: Vec<String> = fmt.suffixes.iter().map(|s| format!("{name}{s}")).collect();
    out.push(name.to_string());
    out
}

fn find_one(name: &str, fmt: &FormatInfo) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if tex_kpse::embedded_tree::is_embedded_path(name) {
        return is_file(name).then(|| PathBuf::from(name));
    }
    let found = with_engine(|e| {
        let names = match fmt.search {
            Search::Format(format) => tex_kpse::Kpse::candidates(name, format),
            Search::Any => candidates(name, fmt),
        };
        if let Some(path) = e.find_job_output_file(&names) {
            return Some(path);
        }
        match fmt.search {
            Search::Format(format) => e.font_loader.kpse.find(name, format),
            Search::Any => names.iter().find_map(|c| e.font_loader.kpse.find_any(c)),
        }
    })
    .ok()
    .flatten();
    if let Some(path) = found {
        return Some(path);
    }
    let member = match fmt.search {
        Search::Format(format) => tex_kpse::Kpse::candidates(name, format)
            .into_iter()
            .find(|c| tex_kpse::has_embedded_package(c)),
        Search::Any => candidates(name, fmt).into_iter().find(|c| tex_kpse::has_embedded_package(c)),
    }?;
    tex_kpse::embedded_tree::member_path(&member).map(PathBuf::from)
}

/// `find_format`: the format a file name suggests.
fn guess_format(name: &str) -> Option<usize> {
    let special: &[(&str, &str)] = &[
        ("config.ps", "dvips config"),
        ("dvipdfmx.cfg", "other text files"),
        ("fmtutil.cnf", "web2c files"),
        ("glyphlist.txt", "map"),
        ("mktex.cnf", "web2c files"),
        ("pdfglyphlist.txt", "map"),
        ("pdftex.cfg", "pdftex config"),
        ("texmf.cnf", "cnf"),
        ("updmap.cfg", "web2c files"),
        ("XDvi", "other text files"),
    ];
    if let Some((_, fmt)) = special.iter().find(|(file, _)| file.eq_ignore_ascii_case(name)) {
        return FORMATS.iter().position(|f| f.name == *fmt);
    }
    let fmt_index = FORMATS.iter().position(|f| f.name == "fmt")?;
    for (index, fmt) in FORMATS.iter().enumerate() {
        if index == fmt_index {
            continue;
        }
        if has_suffix(name, fmt.suffixes) || has_suffix(name, fmt.alt_suffixes) {
            return Some(index);
        }
    }
    if has_suffix(name, FORMATS[fmt_index].suffixes) {
        return Some(fmt_index);
    }
    None
}

/// All files called `name` in the directories of the expanded `path`.
fn path_search(program: &str, path: &str, name: &str, all: bool, must_exist: bool) -> Vec<String> {
    let _ = must_exist;
    let expanded = expand_path(program, path);
    let mut out = Vec::new();
    for dir in expanded.split(PATH_SEPARATOR).filter(|d| !d.is_empty()) {
        let candidate = format!("{}/{name}", dir.trim_end_matches('/'));
        if is_file(&candidate) {
            out.push(candidate);
            if !all {
                break;
            }
        }
    }
    out
}

// ---------------------------------------------------------- name checks ---

/// `kpathsea_absolute_p`: a leading `/`; on Windows also `\` and a drive
/// letter (`IS_DEVICE_SEP`).
fn absolute(name: &str) -> bool {
    if cfg!(windows) {
        crate::io::win_is_absolute(&crate::io::win_form(name))
    } else {
        name.starts_with('/')
    }
}

/// `abs_fname_ok`: `name` is `dir` or below it (Windows: `IS_DIR_SEP` both
/// separators and case-insensitive names).
fn below(name: &str, dir: Option<String>) -> bool {
    match dir {
        Some(dir) if !dir.is_empty() => {
            if cfg!(windows) {
                let (name, dir) = (crate::io::win_form(name), crate::io::win_form(&dir));
                let dir = dir.trim_end_matches('/');
                return name.len() == dir.len() && name.eq_ignore_ascii_case(dir)
                    || crate::io::below_root(&name, dir, true).is_some();
            }
            name.starts_with(&dir) && (name.len() == dir.len() || name.as_bytes()[dir.len()] == b'/')
        }
        _ => false,
    }
}

/// `kpathsea_name_ok`: reading is always allowed (as of Kpathsea 2026);
/// writing follows `openout_any` (`a` anything, `r` no dot files, `p` also
/// no absolute names except below `$TEXMF_OUTPUT_DIRECTORY`, `$TEXMFOUTPUT`
/// and, when `extended`, `$TEXMFVAR`/`$TEXMFSYSVAR`, and no `..`).
fn name_ok(program: &str, name: &str, output: bool, silent: bool, extended: bool) -> bool {
    if !output {
        return true;
    }
    let var = "openout_any";
    let choice = var_value(program, var).unwrap_or_else(|| "p".to_string());
    let c = choice.chars().next().unwrap_or('p');
    if matches!(c, 'a' | 'y' | '1') {
        return true;
    }
    let expanded = expand_var(program, &tilde(name));
    let ok = 'check: {
        // dot files and dot directories
        let b = name.as_bytes();
        let mut from = 0;
        while let Some(at) = name[from..].find('.') {
            let q = from + at;
            let after = b.get(q + 1).copied().unwrap_or(0);
            let after2 = b.get(q + 2).copied().unwrap_or(0);
            if (q == 0 || b[q - 1] == b'/')
                && after != b'/'
                && !(after == b'.' && after2 == b'/')
                && !(extended && absolute(&expanded))
            {
                break 'check false;
            }
            from = q + 1;
        }
        if matches!(c, 'r' | 'n' | '0') {
            break 'check true;
        }
        if absolute(&expanded) {
            let allowed = below(&expanded, std::env::var("TEXMF_OUTPUT_DIRECTORY").ok())
                || below(&expanded, var_value(program, "TEXMFOUTPUT"))
                || (extended
                    && (below(&expanded, var_value(program, "TEXMFVAR")) || below(&expanded, var_value(program, "TEXMFSYSVAR"))));
            if !allowed {
                break 'check false;
            }
        }
        if name.starts_with("../") {
            break 'check false;
        }
        let mut search = 0;
        while let Some(at) = name[search..].find("..") {
            let q = search + at;
            if b.get(q + 2) == Some(&b'/') && q > 0 && b[q - 1] == b'/' {
                break 'check false;
            }
            search = q + 2;
        }
        true
    };
    if !ok && !silent {
        eprintln!("\n{program}: Not writing to {name} ({var} = {c}; {}extended check).", if extended { "" } else { "no " });
    }
    ok
}

// ---------------------------------------------------- shell permission ---

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Port of `shell_cmd_is_allowed`: (-1 bad quoting, 0 disallowed, 2 allowed
/// with the safely quoted command line).
fn shell_cmd_is_allowed(cmd: &str, allowed: &[String]) -> (i32, String) {
    let b = cmd.as_bytes();
    let mut c = 0;
    while c < b.len() && is_space(b[c]) {
        c += 1;
    }
    let mut d = c;
    while d < b.len() && !is_space(b[d]) {
        d += 1;
    }
    let cmdname = &cmd[c..d];
    if !allowed.iter().any(|a| a == cmdname) {
        return (0, String::new());
    }
    let mut out: Vec<u8> = Vec::new();
    let mut s = c;
    while s < b.len() && !is_space(b[s]) {
        out.push(b[s]);
        s += 1;
    }
    let mut pre = true;
    while s < b.len() {
        if b[s] == b'\'' {
            return (-1, String::new());
        }
        if b[s] == b'"' {
            if !pre {
                out.push(b'\'');
            }
            pre = false;
            out.push(b'\'');
            s += 1;
            while s < b.len() && b[s] != b'"' {
                if b[s] == b'\'' {
                    return (-1, String::new());
                }
                out.push(b[s]);
                s += 1;
            }
            if s >= b.len() {
                return (-1, String::new());
            }
            s += 1;
            if s < b.len() && !is_space(b[s]) {
                return (-1, String::new());
            }
        } else if pre && !is_space(b[s]) {
            pre = false;
            out.push(b'\'');
            out.push(b[s]);
            s += 1;
        } else if !pre && is_space(b[s]) {
            pre = true;
            out.push(b'\'');
            out.push(b[s]);
            s += 1;
        } else {
            out.push(b[s]);
            s += 1;
        }
    }
    if !pre {
        out.push(b'\'');
    }
    (2, String::from_utf8_lossy(&out).into_owned())
}

/// The commands `shell_escape_commands` allows.
pub(crate) fn allowed_commands() -> Vec<String> {
    if let Some(list) = ALLOWED.with(|a| a.borrow().clone()) {
        return list;
    }
    let list = var_value("luatex", "shell_escape_commands").unwrap_or_default();
    list.split(',').filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// `shell_cmd_is_allowed` under the active policy: (allow, command to run).
pub(crate) fn check_command(cmd: &str) -> (i32, String) {
    match shell_escape() {
        ShellEscape::Disabled => (0, String::new()),
        ShellEscape::Enabled => (1, cmd.to_string()),
        ShellEscape::Restricted => shell_cmd_is_allowed(cmd, &allowed_commands()),
    }
}

/// The directory kpathsea reports as `SELFAUTOLOC` to shell escape commands:
/// where the engine lives. TeXres is not installed beside `kpsewhich`, which
/// such tools run from there, so the directory of the TeX Live `kpsewhich`
/// on `PATH` stands in for it when the engine's own directory has none.
fn shell_tool_dir() -> PathBuf {
    let own = exe_dirs().0;
    let has_kpsewhich = |dir: &Path| dir.join(if cfg!(windows) { "kpsewhich.exe" } else { "kpsewhich" }).is_file();
    if has_kpsewhich(&own) {
        return own;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).find(|dir| has_kpsewhich(dir)).unwrap_or(own)
}

/// web2c's `runsystem` for `\write18`: the code that stands for what
/// happened (-1 bad quoting, 0 refused, 1 ran, 2 ran the safely quoted
/// command), with the command run by `/bin/sh -c` in the current directory.
/// `output_dir` is exported as `TEXMF_OUTPUT_DIRECTORY`, so a tool such as
/// `latexminted` finds the files the job wrote there.
pub(crate) fn run_system(cmd: &[u8], output_dir: Option<&std::path::Path>) -> i32 {
    let text = String::from_utf8_lossy(cmd);
    let (allow, run) = match shell_escape() {
        ShellEscape::Disabled => return 0,
        ShellEscape::Enabled => (1, text.into_owned()),
        ShellEscape::Restricted => shell_cmd_is_allowed(&text, &allowed_commands()),
    };
    if allow > 0 {
        let run: std::borrow::Cow<'_, [u8]> =
            if allow == 1 { std::borrow::Cow::Borrowed(cmd) } else { std::borrow::Cow::Owned(run.into_bytes()) };
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg(crate::lua_sys::os_str(&run));
        // kpathsea exports these for every program it starts; tools such as
        // `latexminted` locate `kpsewhich` through `SELFAUTOLOC`.
        let loc = shell_tool_dir();
        let dir = loc.parent().map(Path::to_path_buf).unwrap_or_default();
        let parent = dir.parent().map(Path::to_path_buf).unwrap_or_default();
        let grandparent = parent.parent().map(Path::to_path_buf).unwrap_or_default();
        command
            .env("SELFAUTOLOC", &loc)
            .env("SELFAUTODIR", &dir)
            .env("SELFAUTOPARENT", &parent)
            .env("SELFAUTOGRANDPARENT", &grandparent);
        if let Some(dir) = output_dir {
            let dir = if dir.is_absolute() { dir.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(dir) };
            command.env("TEXMF_OUTPUT_DIRECTORY", dir);
        }
        let _ = command.status();
    }
    allow
}

// ----------------------------------------------------------- primitives ---

fn s_of(s: &LuaString) -> String {
    String::from_utf8_lossy(&bytes_of(s)).into_owned()
}

fn opt_bytes(value: Option<String>) -> Option<LuaBytes> {
    value.map(|v| LuaBytes(v.into_bytes()))
}

pub(crate) fn register(lua: &mut Lua, s: &tex_lua::LuaTable) -> Result<(), String> {
    sys_reg!(lua, s, "kpse_format_names", || -> Vec<&'static str> { FORMATS.iter().map(|f| f.name).collect() });
    sys_reg!(lua, s, "kpse_program", || -> String { PROGRAM.with(|p| p.borrow().clone()) });
    sys_reg!(lua, s, "kpse_set_program", |name: LuaString| {
        PROGRAM.with(|p| *p.borrow_mut() = s_of(&name));
        PROGRAM_SET.with(|p| p.set(true));
    });
    sys_reg!(lua, s, "kpse_find", |name: LuaString, format: i64| -> Option<LuaBytes> {
        let fmt = FORMATS.get(format as usize)?;
        find_one(&s_of(&name), fmt).map(|p| LuaBytes(if cfg!(windows) { kpse_path(&p).into_bytes() } else { path_bytes(&p) }))
    });
    // names matching a lookup: `all` lists every hit
    sys_reg!(
        lua,
        s,
        "kpse_lookup",
        |program: LuaString, name: LuaString, format: i64, path: Option<LuaString>, all: bool, must_exist: bool, subdirs: Option<tex_lua::LuaTable>|
         -> Vec<LuaBytes> {
            let subdirs: Vec<LuaString> = subdirs.and_then(|t| t.sequence_values().ok()).unwrap_or_default();
            let program = s_of(&program);
            let name = s_of(&name);
            let mut found: Vec<String> = if let Some(path) = path {
                path_search(&program, &s_of(&path), &name, all || !subdirs.is_empty(), must_exist)
            } else {
                let index = if format >= 0 {
                    Some(format as usize)
                } else {
                    Some(guess_format(&name).unwrap_or_else(|| FORMATS.iter().position(|f| f.name == "tex").expect("tex")))
                };
                match index.and_then(|i| FORMATS.get(i)) {
                    Some(fmt) => find_one(&name, fmt).map(|p| kpse_path(&p)).into_iter().collect(),
                    None => Vec::new(),
                }
            };
            if !subdirs.is_empty() {
                let wanted: Vec<String> = subdirs.iter().map(|s| s_of(s).trim_end_matches('/').to_string()).collect();
                found.retain(|hit| {
                    let dir = hit.rsplit_once('/').map_or("", |(d, _)| d).trim_end_matches('/');
                    wanted.iter().any(|w| dir.len() >= w.len() && dir[dir.len() - w.len()..].eq_ignore_ascii_case(w))
                });
            }
            found.into_iter().map(|h| LuaBytes(h.into_bytes())).collect()
        }
    );
    sys_reg!(lua, s, "kpse_guess_format", |name: LuaString| -> i64 { guess_format(&s_of(&name)).map_or(-1, |i| i as i64) });
    sys_reg!(lua, s, "kpse_var_value", |program: LuaString, name: LuaString| -> Option<LuaBytes> {
        opt_bytes(var_value(&s_of(&program), &s_of(&name)))
    });
    sys_reg!(lua, s, "kpse_expand_var", |program: LuaString, text: LuaString| -> LuaBytes {
        LuaBytes(expand_var(&s_of(&program), &s_of(&text)).into_bytes())
    });
    sys_reg!(lua, s, "kpse_expand_path", |program: LuaString, text: LuaString| -> LuaBytes {
        LuaBytes(expand_path(&s_of(&program), &s_of(&text)).into_bytes())
    });
    sys_reg!(lua, s, "kpse_expand_braces", |text: LuaString| -> LuaBytes { LuaBytes(expand_braces(&s_of(&text)).into_bytes()) });
    sys_reg!(lua, s, "kpse_show_path", |program: LuaString, format: i64| -> Option<LuaBytes> {
        let fmt = FORMATS.get(format as usize)?;
        Some(LuaBytes(show_path(&s_of(&program), fmt).into_bytes()))
    });
    // a relative file name that TeX wrote into the job's output directories
    sys_reg!(lua, s, "job_input_path", |name: LuaString| -> Option<LuaBytes> {
        let bytes = bytes_of(&name);
        let text = std::str::from_utf8(&bytes).ok()?;
        let path = with_engine(|e| e.find_job_output_file(&[text])).ok().flatten()?;
        Some(LuaBytes(path_bytes(&path)))
    });
    sys_reg!(lua, s, "kpse_readable_file", |name: LuaString| -> Option<LuaBytes> {
        let bytes = bytes_of(&name);
        if let Ok(text) = std::str::from_utf8(&bytes) {
            if tex_kpse::embedded_tree::is_embedded_path(text) {
                return is_file(text).then_some(LuaBytes(bytes));
            }
        }
        let path = path_of(&bytes);
        match std::fs::File::open(&path) {
            Ok(_) if path.is_file() => Some(LuaBytes(bytes)),
            Ok(_) => None,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    eprintln!("{}: Permission denied", String::from_utf8_lossy(&bytes));
                }
                None
            }
        }
    });
    sys_reg!(
        lua,
        s,
        "kpse_name_ok",
        |program: LuaString, name: LuaString, output: bool, silent_extended: bool| -> bool {
            name_ok(&s_of(&program), &s_of(&name), output, silent_extended, silent_extended)
        }
    );
    sys_reg!(lua, s, "kpse_check_permission", |cmd: LuaString| -> (bool, LuaBytes) {
        let cmd = s_of(&cmd);
        match shell_escape() {
            ShellEscape::Disabled => (false, LuaBytes(b"all command execution is disabled".to_vec())),
            ShellEscape::Enabled => (true, LuaBytes(cmd.into_bytes())),
            ShellEscape::Restricted => match shell_cmd_is_allowed(&cmd, &allowed_commands()) {
                (0, _) => (false, LuaBytes(b"specific command execution disabled".to_vec())),
                (2, safe) => (true, LuaBytes(safe.into_bytes())),
                _ => (false, LuaBytes(b"bad command line quoting".to_vec())),
            },
        }
    });
    sys_reg!(lua, s, "kpse_record", |name: LuaString, output: bool| {
        let path = path_of(&bytes_of(&name));
        let _ = with_engine(|e| {
            let list = if output { &mut e.written_files } else { &mut e.loaded_files };
            if !list.contains(&path) {
                list.push(path);
            }
        });
    });
    sys_reg!(lua, s, "kpse_default_texmfcnf", || -> String {
        let (loc, dir, parent) = exe_dirs();
        let mut dirs: Vec<String> = Vec::new();
        let sub = ["", "/share/texmf-local/web2c", "/share/texmf-dist/web2c", "/share/texmf/web2c", "/texmf-local/web2c", "/texmf-dist/web2c", "/texmf/web2c"];
        for base in [&loc, &dir] {
            for suffix in sub {
                dirs.push(format!("{}{suffix}", kpse_path(base)));
            }
        }
        dirs.push(format!("{}/texmf-local/web2c", kpse_path(&parent)));
        for suffix in sub {
            dirs.push(format!("{}{suffix}", kpse_path(&parent)));
        }
        format!("{{{}}}", dirs.join(","))
    });
    Ok(())
}

/// Commands allowed under the restricted shell policy, overriding the
/// built-in `shell_escape_commands`.
pub(crate) fn set_allowed_commands(list: &str) {
    let commands = list.split(',').filter(|c| !c.is_empty()).map(str::to_string).collect();
    ALLOWED.with(|a| *a.borrow_mut() = Some(commands));
}

/// A numeric `texmf.cnf` parameter.
pub(crate) fn cnf_number(name: &str) -> i64 {
    var_value("luatex", name).and_then(|v| v.parse().ok()).unwrap_or(0)
}
