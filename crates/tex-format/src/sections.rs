//! Which sectioning commands may get a blank line before them.
//!
//! A blank line is a `\par`, and a second `\par` does nothing more. So a blank
//! line before `\section` changes nothing exactly when `\section` itself
//! begins by ending the paragraph, as LaTeX's `\@startsection` does
//! (`\if@noskipsec \leavevmode \fi \par`). A class or a project may define the
//! command otherwise: a CV class whose `\subsubsection` starts with
//! `\linebreak` fails when the paragraph has already ended. The blank line is
//! therefore added only when every definition in play is known:
//!
//! - the document class is one of [`VETTED_CLASSES`], whose definitions of
//!   the listed commands start with `\par` (directly or via `\@startsection`);
//!   a class in the project's own files counts only as a layer over one of
//!   them, its own definitions being checked like the rest of the project;
//! - every package is either in [`SAFE_PACKAGES`] or a file of the project;
//! - no project file redefines, patches or hooks into the command or into
//!   `\@startsection`, unless the new definition itself starts with `\par`
//!   or `\@startsection`;
//! - every file the project `\input`s or `\include`s was found and read, and
//!   no `\DocumentMetadata` (which may load new sectioning code) is used.
//!
//! [`SAFE_PACKAGES`] lists common packages whose TeX Live 2025 sources,
//! together with every file they load, neither define nor patch nor hook
//! into `\part`, `\section`, `\subsection`, `\subsubsection` or
//! `\@startsection` (packages that only use them, such as natbib's
//! `\bibsection`, and patches after the leading `\par`, such as parskip's,
//! were checked by reading them). titlesec and placeins are left out.

use std::collections::BTreeSet;

pub(crate) const SECTIONS: &[&str] = &["part", "chapter", "section", "subsection", "subsubsection"];

const PART: u8 = 1;
const SECTION: u8 = 4;
const SUBSECTION: u8 = 8;
const SUBSUBSECTION: u8 = 16;
/// Not a command that gets a blank line, but redefining it changes all of them.
const STARTSECTION: u8 = 32;
const STANDARD: u8 = SECTION | SUBSECTION | SUBSUBSECTION;

/// The bit of a sectioning command (or `\@startsection`), 0 for other names.
fn bit(name: &str) -> u8 {
    match name {
        "part" => PART,
        "chapter" => 2,
        "section" => SECTION,
        "subsection" => SUBSECTION,
        "subsubsection" => SUBSUBSECTION,
        "@startsection" => STARTSECTION,
        _ => 0,
    }
}

/// Classes whose listed sectioning commands start by ending the paragraph.
/// `\chapter` (and `\part` in books) start with `\clearpage`, which tests
/// the mode first, so they are never listed.
const VETTED_CLASSES: &[(&str, u8)] = &[
    ("amsart", PART | STANDARD),
    ("amsbook", STANDARD),
    ("amsproc", PART | STANDARD),
    ("article", PART | STANDARD),
    ("book", STANDARD),
    ("elsarticle", STANDARD),
    ("llncs", STANDARD),
    ("report", STANDARD),
];

/// Packages that leave the start of the sectioning commands alone (sorted).
#[rustfmt::skip]
const SAFE_PACKAGES: &[&str] = &[
    "BOONDOX-cal", "CJKutf8", "XCharter", "a4", "abstract", "academicons", "accents", "acronym",
    "adjustbox", "ae", "aecompl", "afterpackage", "afterpage", "algorithm", "algorithm2e",
    "algorithmic", "algorithmicx", "algpseudocode", "aliascnt", "alltt", "amsbsy", "amscd",
    "amsfonts", "amsmath", "amsopn", "amsrefs", "amssymb", "amstext", "amsthm", "amsxtra",
    "anyfontsize", "appendix", "appendixnumberbeamer", "array", "arydshln", "atbegshi", "authblk",
    "auxhook", "babel", "background", "backref", "balance", "bbding", "bbm", "bbold", "bera",
    "beramono", "biblatex", "bitset", "blindtext", "bm", "bold-extra", "bookmark", "booktabs",
    "boxedminipage", "braket", "breakcites", "breakurl", "bussproofs", "calc", "calculator",
    "calligra", "calrsfs", "cancel", "caption", "caption2", "cases", "ccicons", "cclicenses",
    "changebar", "changepage", "changes", "charter", "chemfig", "chngcntr", "chngpage",
    "circuitikz", "cite", "citeref", "cleveref", "cmap", "collcell", "color", "colordvi",
    "colortbl", "comment", "courier", "csquotes", "csvsimple", "ctable", "cuted", "datatool",
    "datetime", "datetime2", "dblfloatfix", "dcolumn", "diagbox", "dirtytalk", "doc", "doi",
    "draftwatermark", "dsfont", "duckuments", "empheq", "enumerate", "enumitem", "environ",
    "epigraph", "epsf", "epsfig", "epstopdf", "esint", "eso-pic", "etex", "etexcmds", "etoolbox",
    "eucal", "eulervm", "eurosym", "euscript", "everypage", "expl3", "exscale", "extpfeil",
    "fancybox", "fancyhdr", "fancyvrb", "fdsymbol", "fewerfloatpages", "fix-cm", "fixfoot",
    "fixltx2e", "float", "fltrace", "flushend", "fncychap", "fontawesome", "fontawesome5",
    "fontenc", "fontspec", "footmisc", "footnote", "forest", "forloop", "fourier", "fp", "framed",
    "frcursive", "fullpage", "gensymb", "geometry", "glossaries", "graphics", "graphicx",
    "graphviz", "helvet", "hhline", "hobsub-hyperref", "hologo", "hycolor", "hypcap", "hyperref",
    "ifluatex", "ifpdf", "ifsym", "iftex", "ifthen", "ifvtex", "ifxetex", "imakeidx", "import",
    "inconsolata", "indentfirst", "infwarerr", "inputenc", "intcalc", "keyval", "kpfonts",
    "kvdefinekeys", "kvoptions", "kvsetkeys", "l3keys2e", "lastpage", "latexrelease", "latexsym",
    "lato", "leftidx", "letltxmacro", "libertine", "lineno", "lipsum", "listings", "lmodern",
    "longtable", "lscape", "ltxcmds", "lua-unicode-math", "luacode", "makecell", "makeidx",
    "manyfoot", "marvosym", "mathabx", "mathalpha", "mathdots", "mathpartir", "mathpazo",
    "mathptm", "mathptmx", "mathrsfs", "mathtools", "mdframed", "memhfixc", "mflogo", "mhchem",
    "microtype", "minted", "mleftright", "moreverb", "mparhack", "multibib", "multicol",
    "multienum", "multirow", "mwe", "nameref", "natbib", "needspace", "newfloat", "newlfont",
    "newtxmath", "newtxtext", "newunicodechar", "nicefrac", "nomencl", "oldlfont", "orcidlink",
    "overpic", "palatino", "paralist", "parskip", "pbox", "pdfescape", "pdflscape",
    "pdfmanagement", "pdfpages", "pdftexcmds", "pgf", "pgfplots", "pgfplotstable", "physics",
    "pifont", "polyglossia", "proof", "psfrag", "pst-node", "pstricks", "pxfonts", "qtree",
    "ragged2e", "rawfonts", "refcount", "relsize", "rotating", "rsfso", "sansmath", "sectsty",
    "setspace", "sfmath", "shortvrb", "showlabels", "siunitx", "soul", "sourcecodepro",
    "standalone", "steinmetz", "stfloats", "stmaryrd", "stringenc", "stringstrings", "subcaption",
    "subdepth", "subfig", "subfiles", "syntax", "tablefootnote", "tabto", "tabu", "tabularx",
    "tabulary", "tcolorbox", "textcase", "textcomp", "textgreek", "textpos", "tgpagella",
    "tgtermes", "thm-restate", "thmtools", "threeparttable", "threeparttablex", "tikz",
    "tikz-3dplot", "tikz-cd", "tikz-qtree", "times", "titletoc", "titling", "tocbibind", "tocloft",
    "todonotes", "totcount", "totpages", "translations", "translator", "twemojis", "txfonts",
    "ulem", "unicode-math", "units", "upgreek", "url", "varioref", "verbatim", "wasysym",
    "watermark", "wrapfig", "xargs", "xcolor", "xeCJK", "xfrac", "xifthen", "xkeyval", "xparse",
    "xr", "xspace", "xstring", "xtab", "xy", "xypic", "yfonts", "zi4", "zref", "zref-clever",
];

/// Control words that may come right before a sectioning command that is
/// being used, not defined (`\clearpage\section{...}`).
const HARMLESS_BEFORE: &[&str] = &[
    "par",
    "clearpage",
    "cleardoublepage",
    "newpage",
    "pagebreak",
    "appendix",
    "bigskip",
    "medskip",
    "smallskip",
    "vfill",
    "vfil",
    "mainmatter",
    "frontmatter",
    "backmatter",
    "relax",
    "noindent",
    "maketitle",
    "tableofcontents",
];

/// Commands that define a control sequence given by name: `\@namedef{section}`.
const NAME_DEFINERS: &[&str] = &[
    "@namedef", "csdef", "csgdef", "csedef", "csxdef", "cslet", "csletcs", "csundef", "cspreto",
    "csappto", "csgpreto", "csgappto",
];

/// Commands that define the control sequence after them, whose new
/// definition is checked for a leading `\par` or `\@startsection`.
const DEFINERS: &[&str] = &[
    "def",
    "gdef",
    "edef",
    "xdef",
    "newcommand",
    "renewcommand",
    "providecommand",
    "DeclareRobustCommand",
];

/// What kind of file a name refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    Class,
    Package,
    Input,
}

impl FileKind {
    /// File names to try for `name`, in order.
    pub fn candidates(self, name: &str) -> Vec<String> {
        match self {
            FileKind::Class => vec![format!("{name}.cls")],
            FileKind::Package => vec![format!("{name}.sty")],
            FileKind::Input if name.ends_with(".tex") => vec![name.to_string()],
            FileKind::Input => vec![format!("{name}.tex"), name.to_string()],
        }
    }
}

/// What the project's sources say about its sectioning commands.
#[derive(Clone, Debug, Default)]
pub struct SectionFacts {
    classes: BTreeSet<String>,
    packages: BTreeSet<String>,
    inputs: BTreeSet<String>,
    /// Files found among the project's sources (and read).
    found: BTreeSet<(FileKind, String)>,
    /// Files looked for and not found.
    missing: BTreeSet<(FileKind, String)>,
    /// Commands the project redefines or patches.
    redefined: u8,
    /// Something makes the definitions unknowable.
    unknown: bool,
    /// A class or package is loaded whose name is not written out.
    unknown_load: bool,
}

impl SectionFacts {
    /// Records what the control word `name`, which ends at `end`, says.
    /// `start` is the position of its backslash.
    pub(crate) fn note(&mut self, text: &str, start: usize, end: usize, name: &str) {
        match name {
            "documentclass" | "LoadClass" | "LoadClassWithOptions" => {
                self.add_names(text, end, FileKind::Class);
            }
            "usepackage" | "RequirePackage" | "RequirePackageWithOptions" => {
                self.add_names(text, end, FileKind::Package);
            }
            "input" | "include" | "subfile" | "InputIfFileExists" | "includestandalone" => {
                match file_arg(text, end, name == "input") {
                    Some(arg) => self.add(FileKind::Input, arg),
                    None => self.unknown = true,
                }
            }
            "import" | "subimport" | "inputfrom" | "subinputfrom" | "includefrom"
            | "subincludefrom" => {
                let dir = braced(text, end);
                let file = dir.and_then(|(_, e)| braced(text, e));
                match (dir, file) {
                    (Some((d, _)), Some((f, _))) => self.add(FileKind::Input, &format!("{d}{f}")),
                    _ => self.unknown = true,
                }
            }
            "DocumentMetadata" => self.unknown = true,
            "csname" => {
                let b = text.as_bytes();
                let s = skip_space(b, end);
                let e = word_end(b, s);
                let n = bit(&text[s..e]);
                if n != 0 && text[skip_space(b, e)..].starts_with("\\endcsname") {
                    self.redefined |= n;
                }
            }
            "AddToHook" | "AddToHookNext" | "AddToHookWithArguments" => {
                if let Some((arg, _)) = braced(text, end) {
                    if let Some(rest) = arg.trim().strip_prefix("cmd/") {
                        self.redefined |= bit(rest.split('/').next().unwrap_or(""));
                    }
                }
            }
            _ if NAME_DEFINERS.contains(&name) => {
                if let Some((arg, _)) = braced(text, end) {
                    self.redefined |= bit(arg.trim());
                }
            }
            _ => {
                let n = bit(name);
                if n != 0 && redefines(text, start, end) {
                    self.redefined |= n;
                }
            }
        }
    }

    fn add_names(&mut self, text: &str, end: usize, kind: FileKind) {
        let Some((list, _)) = braced(text, after_options(text.as_bytes(), end)) else {
            self.unknown = true;
            return;
        };
        for line in list.split('\n') {
            let line = line.split('%').next().unwrap_or("");
            for name in line.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                self.add(kind, name);
            }
        }
    }

    fn add(&mut self, kind: FileKind, name: &str) {
        if name.contains(['\\', '#', '{', '}']) {
            self.unknown = true;
            self.unknown_load |= kind != FileKind::Input;
            return;
        }
        let set = match kind {
            FileKind::Class => &mut self.classes,
            FileKind::Package => &mut self.packages,
            FileKind::Input => &mut self.inputs,
        };
        if !set.contains(name) {
            set.insert(name.to_string());
        }
    }

    /// The first class or package loaded whose verbatim material is not
    /// known: not in `packages`' lists, not one of the project's files and
    /// not named in `also_known`. `Some(None)` when a name is not written
    /// out (`\usepackage{\name}`).
    pub(crate) fn unvetted(&self, also_known: &[String]) -> Option<Option<(FileKind, &str)>> {
        if self.unknown_load {
            return Some(None);
        }
        let vetted = |kind: FileKind, name: &String| {
            let known = match kind {
                FileKind::Class => crate::packages::known_class(name),
                _ => crate::packages::known_package(name),
            };
            known
                || also_known.contains(name)
                || self.found.contains(&(kind, name.clone()))
        };
        let classes = self.classes.iter().map(|c| (FileKind::Class, c));
        let packages = self.packages.iter().map(|p| (FileKind::Package, p));
        classes
            .chain(packages)
            .find(|(kind, name)| !vetted(*kind, name))
            .map(|(kind, name)| Some((kind, name.as_str())))
    }

    /// Files to look for among the project's sources: inputs, and classes
    /// and packages not known otherwise, not yet found or missed.
    pub fn wanted(&self) -> Vec<(FileKind, String)> {
        let classes = self
            .classes
            .iter()
            .filter(|c| vetted_class(c).is_none())
            .map(|c| (FileKind::Class, c));
        let packages = self
            .packages
            .iter()
            .filter(|p| SAFE_PACKAGES.binary_search(&p.as_str()).is_err())
            .map(|p| (FileKind::Package, p));
        let inputs = self.inputs.iter().map(|i| (FileKind::Input, i));
        classes
            .chain(packages)
            .chain(inputs)
            .map(|(k, n)| (k, n.clone()))
            .filter(|key| !self.found.contains(key) && !self.missing.contains(key))
            .collect()
    }

    /// Records that a wanted file was found (and its text scanned) or not.
    pub fn resolve(&mut self, kind: FileKind, name: String, found: bool) {
        if found {
            self.found.insert((kind, name));
        } else {
            self.missing.insert((kind, name));
        }
    }

    /// The bits of the sectioning commands known to start with `\par`.
    pub(crate) fn par_sections(&self) -> u8 {
        if self.unknown || !self.missing.is_empty() || self.redefined & STARTSECTION != 0 {
            return 0;
        }
        let found = |kind: FileKind, name: &String| self.found.contains(&(kind, name.clone()));
        if !self.inputs.iter().all(|i| found(FileKind::Input, i)) {
            return 0;
        }
        let packages_known = self.packages.iter().all(|p| {
            SAFE_PACKAGES.binary_search(&p.as_str()).is_ok() || found(FileKind::Package, p)
        });
        if !packages_known {
            return 0;
        }
        let mut mask = None;
        for class in &self.classes {
            match vetted_class(class) {
                Some(m) => mask = Some(mask.unwrap_or(u8::MAX) & m),
                None if found(FileKind::Class, class) => {}
                None => return 0,
            }
        }
        mask.unwrap_or(0) & !self.redefined
    }
}

/// Names of the sectioning commands in `mask`.
pub(crate) fn names(mask: u8) -> impl Iterator<Item = &'static str> {
    SECTIONS.iter().copied().filter(move |s| mask & bit(s) != 0)
}

pub(crate) fn has(mask: u8, name: &str) -> bool {
    mask & bit(name) != 0
}

fn vetted_class(name: &str) -> Option<u8> {
    VETTED_CLASSES
        .iter()
        .find(|(c, _)| *c == name)
        .map(|(_, m)| *m)
}

fn is_letter(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'@'
}

fn skip_space(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

fn word_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_letter(b[i]) {
        i += 1;
    }
    i
}

/// Skips blanks, line ends, comments and any `[...]` groups (with nested
/// braces) after `i`.
fn after_options(b: &[u8], mut i: usize) -> usize {
    loop {
        i = skip_space(b, i);
        match b.get(i) {
            Some(b'%') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            Some(b'[') => {
                let mut depth = 0usize;
                while i < b.len() {
                    match b[i] {
                        b'{' => depth += 1,
                        b'}' => depth = depth.saturating_sub(1),
                        b']' if depth == 0 => break,
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
            }
            _ => return i,
        }
    }
}

/// The `{...}` group after blanks at `i` (nested braces allowed, at most
/// 4000 bytes) and the index after it.
fn braced(text: &str, i: usize) -> Option<(&str, usize)> {
    let b = text.as_bytes();
    let start = skip_space(b, i);
    if b.get(start) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut k = start;
    while k < b.len().min(start + 4000) {
        match b[k] {
            b'\\' => k += 1,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[start + 1..k], k + 1));
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// The file name after `\input` and friends: `{name}`, or for the primitive
/// `\input name` up to a blank or line end.
fn file_arg(text: &str, i: usize, bare: bool) -> Option<&str> {
    if let Some((arg, _)) = braced(text, i) {
        return Some(arg.trim());
    }
    let b = text.as_bytes();
    let start = skip_space(b, i);
    let mut end = start;
    while end < b.len() && !matches!(b[end], b' ' | b'\t' | b'\n' | b'\r' | b'%' | b'\\' | b'}') {
        end += 1;
    }
    (bare && end > start).then(|| &text[start..end])
}

/// Moves back from `j` over blanks, line ends and the comments that end
/// the lines before.
fn back(b: &[u8], mut j: usize) -> usize {
    loop {
        while j > 0 && matches!(b[j - 1], b' ' | b'\t' | b'\r') {
            j -= 1;
        }
        if j == 0 || b[j - 1] != b'\n' {
            return j;
        }
        j -= 1;
        let line = b[..j]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |p| p + 1);
        let mut k = line;
        while k < j {
            match b[k] {
                b'\\' => k += 2,
                b'%' => {
                    j = k;
                    break;
                }
                _ => k += 1,
            }
        }
    }
}

/// Whether the sectioning command at `start..end` is being defined or
/// patched rather than used: preceded (across blanks, comments, a `{` and
/// a `*`) by a control word other than a harmless one, unless that is a
/// definition whose body starts with `\par` or `\@startsection`. Inside
/// the body of a sectioning command (`\section{\@startsection...}`) it is
/// a use.
fn redefines(text: &str, start: usize, end: usize) -> bool {
    let b = text.as_bytes();
    let mut j = back(b, start);
    let mut braced_name = false;
    if j > 0 && b[j - 1] == b'{' {
        braced_name = true;
        j = back(b, j - 1);
    }
    if j > 0 && b[j - 1] == b'*' {
        j = back(b, j - 1);
    }
    let word_start = {
        let mut k = j;
        while k > 0 && (is_letter(b[k - 1]) || matches!(b[k - 1], b'_' | b':')) {
            k -= 1;
        }
        k
    };
    if word_start == j || word_start == 0 || b[word_start - 1] != b'\\' {
        // After text, a `}` or a control symbol: a use.
        return false;
    }
    let word = &text[word_start..j];
    if braced_name && bit(word) != 0 {
        return false;
    }
    if !braced_name && HARMLESS_BEFORE.contains(&word) {
        return false;
    }
    if DEFINERS.contains(&word) {
        return !body_starts_with_par(b, end, braced_name);
    }
    true
}

/// Whether the definition after a defined name (ending at `i`) starts with
/// `\par` or `\@startsection`: skips `}`, `[n]` and `[default]` groups and
/// `#1`-style parameters, then looks inside the body past blanks and
/// comments.
fn body_starts_with_par(b: &[u8], i: usize, braced_name: bool) -> bool {
    let mut j = skip_space(b, i);
    if braced_name {
        if b.get(j) != Some(&b'}') {
            return false;
        }
        j += 1;
    }
    loop {
        j = after_options(b, j);
        match b.get(j) {
            Some(b'#') => j += 2,
            Some(b'{') => break,
            _ => return false,
        }
    }
    j = after_options(b, j + 1);
    let starts =
        |w: &[u8]| b[j..].starts_with(w) && !b.get(j + w.len()).copied().is_some_and(is_letter);
    starts(b"\\par") || starts(b"\\@startsection")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(text: &str) -> SectionFacts {
        let mut facts = SectionFacts::default();
        let b = text.as_bytes();
        let mut i = 0;
        while let Some(off) = b[i..].iter().position(|&c| c == b'\\') {
            let start = i + off;
            let end = word_end(b, start + 1);
            i = end.max(start + 1);
            if end > start + 1 {
                facts.note(text, start, end, &text[start + 1..end]);
            }
        }
        facts
    }

    #[test]
    fn packages_are_sorted() {
        assert!(SAFE_PACKAGES.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn known_classes_and_packages() {
        let all = PART | STANDARD;
        assert_eq!(facts("\\documentclass{article}").par_sections(), all);
        assert_eq!(
            facts("\\documentclass[a4paper]{book}").par_sections(),
            STANDARD
        );
        let doc = "\\documentclass[11pt]{article}\n\\usepackage[utf8]{inputenc}\n\
                   \\usepackage{amsmath,%\n  hyperref}\n\\section{A}\n\\clearpage\\section{B}\n";
        assert_eq!(facts(doc).par_sections(), all);
        assert_eq!(facts("").par_sections(), 0);
        assert_eq!(facts("\\documentclass{moderncv}").par_sections(), 0);
        assert_eq!(
            facts("\\documentclass{article}\\usepackage{titlesec}").par_sections(),
            0
        );
        assert_eq!(
            facts("\\documentclass{article}\\usepackage{\\mine}").par_sections(),
            0
        );
        assert_eq!(
            facts("\\DocumentMetadata{}\\documentclass{article}").par_sections(),
            0
        );
    }

    #[test]
    fn redefinitions_remove_commands() {
        let with = |s: &str| facts(&format!("\\documentclass{{article}}\n{s}")).par_sections();
        assert_eq!(
            with("\\renewcommand{\\subsubsection}[1]{%\n  \\linebreak #1}"),
            PART | SECTION | SUBSECTION
        );
        assert_eq!(
            with("\\let\\section\\relax"),
            PART | SUBSECTION | SUBSUBSECTION
        );
        assert_eq!(
            with("\\renewcommand*\\section{\\@startsection{section}{1}{0pt}{1ex}{1ex}{}}"),
            PART | STANDARD
        );
        assert_eq!(with("\\def\\section#1{\\par #1}"), PART | STANDARD);
        assert_eq!(
            with("\\pretocmd{\\section}{\\clearpage}{}{}"),
            PART | SUBSECTION | SUBSUBSECTION
        );
        assert_eq!(
            with("\\expandafter\\def\\csname section\\endcsname{x}"),
            PART | SUBSECTION | SUBSUBSECTION
        );
        assert_eq!(
            with("\\@namedef{subsection}{x}"),
            PART | SECTION | SUBSUBSECTION
        );
        assert_eq!(
            with("\\AddToHook{cmd/section/before}{x}"),
            PART | SUBSECTION | SUBSUBSECTION
        );
        assert_eq!(
            with("\\cs_set:Npn \\section {x}"),
            PART | SUBSECTION | SUBSUBSECTION
        );
        assert_eq!(with("\\def\\@startsection#1{x}"), 0);
        assert_eq!(with("\\let\\oldsection=\\section"), PART | STANDARD);
        assert_eq!(
            with("\\setcounter{section}{2}\\appendix\\section{A}"),
            PART | STANDARD
        );
    }

    #[test]
    fn files_must_be_found() {
        let mut f = facts("\\documentclass{wgu-cv}\n\\input{body}\n\\usepackage{mine}");
        assert_eq!(f.par_sections(), 0);
        let mut wanted = f.wanted();
        wanted.sort();
        assert_eq!(
            wanted,
            [
                (FileKind::Class, "wgu-cv".to_string()),
                (FileKind::Package, "mine".to_string()),
                (FileKind::Input, "body".to_string()),
            ]
        );
        for (kind, name) in wanted {
            f.resolve(kind, name, true);
        }
        // A project class alone vouches for nothing.
        assert_eq!(f.par_sections(), 0);
        f.add(FileKind::Class, "article");
        assert_eq!(f.par_sections(), PART | STANDARD);
        f.resolve(FileKind::Input, "other".to_string(), false);
        assert_eq!(f.par_sections(), 0);
    }
}
