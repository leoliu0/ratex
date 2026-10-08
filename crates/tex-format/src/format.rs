//! The formatter: one pass over the source lines with a small lexer that
//! tracks groups, environments, list items, math and verbatim material.
//!
//! What it changes, and why TeX cannot tell:
//! - indentation: TeX skips blanks at the start of every line;
//! - trailing blanks: TeX drops trailing spaces itself, and a trailing tab
//!   reads like the line end that follows it (kept after `\` and `^`);
//! - tabs inside a line become spaces (both are blanks, runs read as one);
//! - a blank run before `\item` (or, when wrapping, any breakable blank run)
//!   becomes a line end, which TeX reads as the same single space;
//! - runs of blank lines shrink (each blank line is a `\par`; repeated
//!   `\par` does nothing more), and a blank line may go before a top-level
//!   `\section` when its definition is known to start with `\par` (see
//!   `sections`);
//! - with `align-columns`, blanks around `&` in alignments (ignored by the
//!   cell templates).
//!
//! Lines inside verbatim environments, multi-line verbatim arguments
//! (`\url`, `\index`, ...) and groups that change the category code of
//! blanks or line ends are copied byte for byte.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::rc::Rc;

use crate::config::Config;
use crate::sections::{self, FileKind, SectionFacts};
use crate::tokens::{self, Allowances};

/// How a verbatim-like command takes its argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delim {
    /// Only `{...}` arguments.
    Braces,
    /// `{...}` or `|...|` with any delimiter character for the last argument.
    Either,
    /// Only a delimiter character (`\verb|...|`).
    Only,
    /// Everything up to the next `;` outside braces, over several lines if
    /// need be (pgfplots' `\addplot ... table {...};`).
    Statement,
    /// The rest of the line, and of the lines after it while a group opened
    /// in it stays open (commands whose arguments are not understood).
    Rest,
}

/// The arguments of a command that reads them verbatim (or with special
/// category codes): an optional `*`, an optional `[...]`, then `args`
/// arguments, the last of which may be delimited by any character when
/// `delim` allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArgSpec {
    pub star: bool,
    pub optional: bool,
    pub args: u8,
    pub delim: Delim,
}

const fn spec(optional: bool, args: u8, delim: Delim) -> ArgSpec {
    ArgSpec {
        star: false,
        optional,
        args,
        delim,
    }
}

impl ArgSpec {
    /// The same arguments after an optional `*`.
    const fn starred(self) -> Self {
        ArgSpec { star: true, ..self }
    }
}

/// Commands whose arguments TeX does not read with the usual category codes:
/// the kernel's, and those of the packages in `packages` (see there).
const VERBATIM_COMMANDS: &[(&str, ArgSpec)] = &[
    ("verb", spec(false, 1, Delim::Only).starred()),
    ("Verb", spec(true, 1, Delim::Either).starred()),
    ("spverb", spec(false, 1, Delim::Only).starred()),
    ("lstinline", spec(true, 1, Delim::Either)),
    ("mintinline", spec(true, 2, Delim::Either)),
    ("url", spec(false, 1, Delim::Either)),
    ("nolinkurl", spec(false, 1, Delim::Either)),
    ("path", spec(false, 1, Delim::Either)),
    ("href", spec(true, 1, Delim::Braces)),
    ("index", spec(true, 1, Delim::Braces)),
    ("sindex", spec(true, 1, Delim::Braces)),
    ("glossary", spec(false, 1, Delim::Braces)),
    ("nomenclature", spec(true, 2, Delim::Braces)),
    // minted
    ("mint", spec(true, 2, Delim::Either)),
    // fancyvrb, fancybox
    ("SaveVerb", spec(true, 2, Delim::Either).starred()),
    ("SaveMVerb", spec(true, 2, Delim::Either).starred()),
    ("SaveGVerb", spec(true, 2, Delim::Either).starred()),
    // pgfplots inline tables: line ends and tabs separate rows and cells.
    ("pgfplotstableread", spec(true, 1, Delim::Braces).starred()),
    ("pgfplotstabletypeset", spec(true, 1, Delim::Braces)),
    ("pgfplotstablesort", spec(true, 2, Delim::Braces)),
    ("addplot", spec(false, 1, Delim::Statement)),
    // hyperref, breakurl, url-based commands of classes
    ("hyperref", spec(false, 1, Delim::Braces)),
    ("hyperimage", spec(false, 1, Delim::Braces)),
    ("burl", spec(false, 1, Delim::Either)),
    ("burlalt", spec(false, 2, Delim::Either)),
    ("urlalt", spec(false, 2, Delim::Either)),
    ("URL", spec(false, 1, Delim::Only).starred()),
    ("doi", spec(true, 1, Delim::Either)),
    ("email", spec(true, 1, Delim::Either)),
    ("homepage", spec(true, 1, Delim::Braces)),
    // biblatex
    ("addbibresource", spec(true, 1, Delim::Braces)),
    ("addglobalbib", spec(true, 1, Delim::Braces)),
    ("addsectionbib", spec(true, 1, Delim::Braces)),
    // amsrefs: line ends in the fields are `\par`.
    ("bib", spec(false, 3, Delim::Braces)),
    // attachfile
    ("attachfile", spec(true, 1, Delim::Braces)),
    ("textattachfile", spec(true, 1, Delim::Braces)),
    // graphviz: line ends of the graph are written out.
    ("digraph", spec(true, 2, Delim::Braces)),
    ("neatograph", spec(true, 2, Delim::Braces)),
    // titleps: line ends are ignored in page styles.
    ("newpagestyle", spec(false, 1, Delim::Rest)),
    ("renewpagestyle", spec(false, 1, Delim::Rest)),
    // tcolorbox, scontents, xstring, beamer, mdwtools' syntax
    ("tcboxverb", spec(true, 1, Delim::Either)),
    ("Scontents", spec(true, 1, Delim::Either).starred()),
    ("verbtocs", spec(false, 2, Delim::Either)),
    ("defverb", spec(false, 2, Delim::Only).starred()),
    ("syntax", spec(false, 1, Delim::Braces)),
    // pythontex's default families
    ("py", spec(true, 1, Delim::Either)),
    ("pyb", spec(true, 1, Delim::Either)),
    ("pyc", spec(true, 1, Delim::Either)),
    ("pys", spec(true, 1, Delim::Either)),
    ("pyv", spec(true, 1, Delim::Either)),
    ("pycon", spec(true, 1, Delim::Either)),
    ("pyconv", spec(true, 1, Delim::Either)),
    ("sympy", spec(true, 1, Delim::Either)),
    ("sympyb", spec(true, 1, Delim::Either)),
    ("sympyc", spec(true, 1, Delim::Either)),
    ("sympys", spec(true, 1, Delim::Either)),
    ("sympyv", spec(true, 1, Delim::Either)),
    ("sympycon", spec(true, 1, Delim::Either)),
    ("sympyconv", spec(true, 1, Delim::Either)),
    ("pylab", spec(true, 1, Delim::Either)),
    ("pylabb", spec(true, 1, Delim::Either)),
    ("pylabc", spec(true, 1, Delim::Either)),
    ("pylabs", spec(true, 1, Delim::Either)),
    ("pylabv", spec(true, 1, Delim::Either)),
    ("pylabcon", spec(true, 1, Delim::Either)),
    ("pylabconv", spec(true, 1, Delim::Either)),
    ("pygment", spec(false, 2, Delim::Either)),
    ("pythontexcustomc", spec(true, 2, Delim::Either)),
    ("setpythontexcustomcode", spec(false, 2, Delim::Braces)),
];

/// Control words after which blanks or line ends may stop meaning what they
/// usually mean; the rest of the enclosing group is kept byte for byte.
const GUARD_WORDS: &[&str] = &[
    "obeyspaces",
    "obeylines",
    "obeywhitespace",
    "obeycr",
    "@vobeyspaces",
    "endlinechar",
    "dospecials",
    "@sanitize",
    "@makeother",
    "verbatim",
    "Verbatim",
    "lstlisting",
    "alltt",
    // `%` ignored (doc, showexpl), tabs as data (datatool), catcode tables
    // (luatexbase, LuaTeX), mathtools' internal syntax, pgfplots' verbatim.
    "MakePercentIgnore",
    "DTLsettabseparator",
    "BeginCatcodeRegime",
    "catcodetable",
    "MHInternalSyntaxOn",
    "beginpgfplotsverbatim",
];

/// Environments that are read verbatim, matched by their exact name.
const VERBATIM_ENVS: &[&str] = &[
    "LTXexample",
    "scontents",
    "algoendfloat",
    "tcbwritetemp",
    "syntax",
    "NOTE",
    "grammar",
    "syntdiag",
    "syntdiag*",
    "tabu*",
    "longtabu*",
    "CCSXML",
    "acks",
    "screenonly",
    "printonly",
    "anonsuppress",
    "Bg5text",
    "Bg5+text",
    "HKtext",
    "GBKtext",
    "SJIStext",
];

/// Environments whose arguments right after `\begin{...}` are verbatim
/// (l3doc's `O{} +v`).
const VERBATIM_ARGUMENT_ENVS: &[(&str, ArgSpec)] = &[
    ("function", spec(true, 1, Delim::Either)),
    ("variable", spec(true, 1, Delim::Either)),
    ("macro", spec(true, 1, Delim::Either)),
];

/// Environments where lines are never wrapped (math, tables, pictures).
const NO_WRAP_ENVS: &[&str] = &[
    "equation",
    "align",
    "alignat",
    "flalign",
    "gather",
    "multline",
    "eqnarray",
    "displaymath",
    "math",
    "split",
    "aligned",
    "alignedat",
    "gathered",
    "multlined",
    "cases",
    "dcases",
    "array",
    "matrix",
    "pmatrix",
    "bmatrix",
    "Bmatrix",
    "vmatrix",
    "Vmatrix",
    "smallmatrix",
    "tabular",
    "tabularx",
    "tabulary",
    "longtable",
    "supertabular",
    "tabbing",
    "tblr",
    "longtblr",
    "NiceTabular",
    "tikzpicture",
    "pgfpicture",
    "picture",
    "axis",
    "semilogxaxis",
    "semilogyaxis",
    "loglogaxis",
    "polaraxis",
    "groupplot",
    "tikzcd",
    "forest",
    "circuitikz",
    "algorithmic",
];

/// Environment names (lower case) read verbatim by their packages, matched
/// generously: treating an ordinary environment as verbatim only means its
/// body is left alone.
fn builtin_verbatim_env(name: &str) -> bool {
    if VERBATIM_ENVS.contains(&name) {
        return true;
    }
    let lower = name.trim_end_matches('*').to_ascii_lowercase();
    const CONTAINS: &[&str] = &[
        "verb",
        "listing",
        "minted",
        "comment",
        "filecontents",
        "alltt",
        "code",
        "python",
        "sympy",
        "pylab",
        "sage",
        "gnuplot",
        "lilypond",
        "dot2tex",
        "markdown",
        "plantuml",
        "html",
    ];
    const PREFIXES: &[&str] = &["lua", "asy", "py", "jl", "sinput", "soutput", "knitr"];
    CONTAINS.iter().any(|p| lower.contains(p)) || PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// Verbatim-like commands and environments a project defines itself.
#[derive(Clone, Debug, Default)]
pub struct Extras {
    pub verbatim_envs: HashSet<String>,
    pub verbatim_commands: HashMap<String, ArgSpec>,
    /// Characters made into `\verb` delimiters by `\MakeShortVerb` and friends.
    pub short_verb: Vec<u8>,
    /// Commands defined to work like `\MakeShortVerb`.
    pub short_verb_definers: HashSet<String>,
    /// Commands whose definitions change category codes of blanks or line
    /// ends (`\obeylines`, `\catcode`, ...): after one, the rest of the
    /// enclosing group is kept as is.
    pub guard_commands: HashSet<String>,
    /// Classes, packages, input files and sectioning definitions, which
    /// decide where a blank line may go before a sectioning command.
    pub sections: SectionFacts,
}

impl Extras {
    /// Collects definitions of verbatim environments, verbatim commands and
    /// short-verb characters from LaTeX source. Comments and verbatim text
    /// that is only displayed (`verbatim`, `\verb|...|`, short-verb text)
    /// are skipped: a `\usepackage` there loads nothing.
    pub fn scan(&mut self, text: &str) {
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let Some(off) = b[i..]
                .iter()
                .position(|&c| c == b'\\' || c == b'%' || self.short_verb.contains(&c))
            else {
                break;
            };
            let at = i + off;
            let line_end = memchr(b'\n', &b[at..]).map_or(b.len(), |p| at + p);
            match b[at] {
                b'%' => {
                    i = line_end;
                    continue;
                }
                b'\\' => {}
                c => {
                    // Short-verb text, to the same character on the line.
                    i = memchr(c, &b[at + 1..line_end]).map_or(line_end, |p| at + p + 2);
                    continue;
                }
            }
            let start = at + 1;
            let mut end = start;
            while end < b.len() && is_letter(b[end]) {
                end += 1;
            }
            if end == start {
                // A control symbol such as `\%`.
                i = (start + 1).min(b.len());
                continue;
            }
            i = end;
            let name = &text[start..end];
            if name == "begin" {
                if let Some((env, after)) = env_name(text, end) {
                    if DISPLAYED_VERBATIM_ENVS.contains(&env) {
                        let close = format!("\\end{{{env}}}");
                        i = text[after..].find(&close).map_or(b.len(), |p| after + p + close.len());
                        continue;
                    }
                }
            }
            if let Some(arg_spec) = self.verbatim_command(name) {
                if matches!(arg_spec.delim, Delim::Only | Delim::Either) {
                    let mut span = Span {
                        spec: arg_spec,
                        done: 0,
                        depth: 0,
                    };
                    let line = &b[..line_end];
                    i = scan_span(line, end, &mut span).unwrap_or(line_end).max(end);
                    continue;
                }
            }
            self.sections.note(text, start - 1, end, name);
            match name {
                "lstnewenvironment"
                | "DefineVerbatimEnvironment"
                | "CustomVerbatimEnvironment"
                | "RecustomVerbatimEnvironment"
                | "newtcblisting"
                | "renewtcblisting"
                | "DeclareTCBListing"
                | "NewTCBListing"
                | "RenewTCBListing"
                | "ProvideTCBListing"
                | "excludecomment"
                | "includecomment"
                | "specialcomment"
                | "processcomment"
                | "newverbatim" => {
                    if let Some(env) = first_braced_arg(text, end) {
                        self.verbatim_envs.insert(env.to_string());
                    }
                }
                "newminted" => {
                    if let Some(opt) = optional_arg(text, end) {
                        self.verbatim_envs.insert(opt.to_string());
                    } else if let Some(lang) = first_braced_arg(text, end) {
                        self.verbatim_envs.insert(format!("{lang}code"));
                    }
                }
                "newmintinline" | "newmint" => {
                    let suffix = if name == "newmintinline" {
                        "inline"
                    } else {
                        ""
                    };
                    let command = optional_arg(text, end).map(str::to_string).or_else(|| {
                        first_braced_arg(text, end).map(|lang| format!("{lang}{suffix}"))
                    });
                    if let Some(command) = command {
                        self.verbatim_commands
                            .insert(command, spec(true, 1, Delim::Either));
                    }
                }
                "DeclareUrlCommand" => {
                    if let Some((command, _)) = defined_command(text, end) {
                        self.verbatim_commands
                            .insert(command.to_string(), spec(false, 1, Delim::Either));
                    }
                }
                "NewDocumentCommand"
                | "RenewDocumentCommand"
                | "DeclareDocumentCommand"
                | "ProvideDocumentCommand" => {
                    // An xparse `v` argument is read verbatim.
                    if let Some((command, after)) = defined_command(text, end) {
                        if let Some(arg_spec) = balanced_group(text, after).and_then(xparse_verbatim) {
                            self.verbatim_commands.insert(command.to_string(), arg_spec);
                        }
                    }
                }
                "newenvironment"
                | "renewenvironment"
                | "NewDocumentEnvironment"
                | "RenewDocumentEnvironment"
                | "DeclareDocumentEnvironment"
                | "ProvideDocumentEnvironment" => {
                    if let Some(env) = first_braced_arg(text, end) {
                        let parts = if name.contains("Document") { 4 } else { 3 };
                        let window = definition(text, end, parts);
                        const MARKERS: &[&str] = &[
                            "erbatim",
                            "lstlisting",
                            "minted",
                            "\\comment",
                            "alltt",
                            "tcblisting",
                            "VerbatimOut",
                        ];
                        // xparse argument specification: `c` collects the
                        // body verbatim (siunitx's LaTeXdemo), `v` reads a
                        // verbatim argument.
                        let verbatim_spec = name.contains("Document")
                            && text[end..]
                                .find('}')
                                .and_then(|p| balanced_group(text, end + p + 1))
                                .is_some_and(|s| {
                                    s.split(['{', '}'])
                                        .step_by(2)
                                        .any(|outside| outside.contains(['c', 'v']))
                                });
                        if verbatim_spec
                            || MARKERS.iter().any(|m| window.contains(m))
                            || GUARD_MARKERS.iter().any(|m| window.contains(m))
                        {
                            self.verbatim_envs.insert(env.to_string());
                        }
                    }
                }
                "newcommand"
                | "renewcommand"
                | "providecommand"
                | "DeclareRobustCommand"
                | "def"
                | "gdef"
                | "edef"
                | "xdef"
                | "let"
                | "NewCommandCopy"
                | "RenewCommandCopy"
                | "DeclareCommandCopy"
                | "LetLtxMacro" => {
                    let Some((command, after)) = defined_command(text, end) else {
                        continue;
                    };
                    let alias = matches!(name, "let" | "LetLtxMacro") || name.ends_with("Copy");
                    if alias {
                        // `\let\code\verb`, `\let\code=\verb`,
                        // `\NewCommandCopy{\code}{\verb}`: a copy reads its
                        // arguments as the original does.
                        let b = text.as_bytes();
                        let mut j = skip_filler(b, after);
                        if b.get(j) == Some(&b'=') {
                            j = skip_filler(b, j + 1);
                        }
                        if b.get(j) == Some(&b'{') {
                            j = skip_filler(b, j + 1);
                        }
                        if b.get(j) != Some(&b'\\') {
                            continue;
                        }
                        let mut k = j + 1;
                        while k < b.len() && is_letter(b[k]) {
                            k += 1;
                        }
                        let original = &text[j + 1..k];
                        let arg_spec = VERBATIM_COMMANDS
                            .iter()
                            .find(|(n, _)| *n == original)
                            .map(|(_, s)| *s)
                            .or_else(|| self.verbatim_commands.get(original).copied());
                        if let Some(arg_spec) = arg_spec {
                            self.verbatim_commands.insert(command.to_string(), arg_spec);
                        }
                        if GUARD_WORDS.contains(&original)
                            || original == "catcode"
                            || self.guard_commands.contains(original)
                        {
                            self.guard_commands.insert(command.to_string());
                        }
                        continue;
                    }
                    const VERBATIM_MARKERS: &[&str] = &[
                        "\\verb",
                        "\\Verb",
                        "\\SaveVerb",
                        "\\lstinline",
                        "\\mint",
                        "\\url",
                        "\\Url",
                        "\\nolinkurl",
                        "\\path",
                        "\\hyper@normalise",
                    ];
                    let window = definition(text, end, 2);
                    if VERBATIM_MARKERS.iter().any(|m| window.contains(m)) {
                        self.verbatim_commands
                            .insert(command.to_string(), spec(true, 1, Delim::Either));
                    }
                    if GUARD_MARKERS.iter().any(|m| window.contains(m)) {
                        self.guard_commands.insert(command.to_string());
                    }
                }
                "MakeShortVerb"
                | "DefineShortVerb"
                | "lstMakeShortInline"
                | "MakeSpecialShortVerb"
                | "shortverb" => self.add_short_verb(text, end),
                _ if self.short_verb_definers.contains(name) => self.add_short_verb(text, end),
                "CustomVerbatimCommand" | "RecustomVerbatimCommand" => {
                    // fancyvrb: `\CustomVerbatimCommand{\new}{Verb}{options}`
                    // works like its base command.
                    let Some((command, after)) = defined_command(text, end) else {
                        continue;
                    };
                    match first_braced_arg(text, after) {
                        Some("DefineShortVerb") => {
                            self.short_verb_definers.insert(command.to_string());
                        }
                        Some(base) => {
                            if let Some(arg_spec) = builtin_verbatim_command(base) {
                                self.verbatim_commands.insert(command.to_string(), arg_spec);
                            }
                        }
                        None => {}
                    }
                }
                "DeclareTotalTCBox" | "NewTotalTCBox" | "RenewTotalTCBox" | "ProvideTotalTCBox"
                | "DeclareTCBox" | "NewTCBox" | "RenewTCBox" | "ProvideTCBox" => {
                    // tcolorbox: `\NewTCBox[init]{\cmd}{xparse spec}{options}`.
                    let mut j = skip_filler(b, end);
                    if b.get(j) == Some(&b'*') {
                        j = skip_filler(b, j + 1);
                    }
                    if b.get(j) == Some(&b'[') {
                        j = text[j..].find(']').map_or(b.len(), |p| j + p + 1);
                    }
                    if let Some((command, after)) = defined_command(text, j) {
                        if let Some(arg_spec) = balanced_group(text, after).and_then(xparse_verbatim) {
                            self.verbatim_commands.insert(command.to_string(), arg_spec);
                        }
                    }
                }
                "newtcolorbox" | "renewtcolorbox" | "NewTColorBox" | "RenewTColorBox"
                | "DeclareTColorBox" | "ProvideTColorBox" => {
                    // Options that write or swallow the body verbatim.
                    if let Some(env) = first_braced_arg(text, end) {
                        let window = definition(text, end, 4);
                        if TCOLORBOX_VERBATIM_KEYS.iter().any(|k| window.contains(k)) {
                            self.verbatim_envs.insert(env.to_string());
                        }
                    }
                }
                "makepythontexfamily" => {
                    if let Some(family) = first_braced_arg(text, end) {
                        for suffix in ["", "b", "c", "s", "v"] {
                            self.verbatim_commands
                                .insert(format!("{family}{suffix}"), spec(true, 1, Delim::Either));
                        }
                        for suffix in ["code", "block", "verbatim", "sub", "sole", "console"] {
                            self.verbatim_envs.insert(format!("{family}{suffix}"));
                        }
                    }
                }
                "documentclass" | "LoadClass" | "LoadClassWithOptions" => {
                    // Classes that make `|` (and `"`) short verbs.
                    let chars: &[u8] = match first_braced_arg(text, end) {
                        Some("ltxdoc" | "ltxguide" | "source2edoc") => b"|",
                        Some("l3doc" | "l3in2edoc") => b"|\"",
                        _ => b"",
                    };
                    for &c in chars {
                        if !self.short_verb.contains(&c) {
                            self.short_verb.push(c);
                        }
                    }
                }
                "usepackage" | "RequirePackage" | "RequirePackageWithOptions" => {
                    let names = balanced_group(text, skip_options(b, end)).unwrap_or("");
                    for package in names.split(',').map(|n| n.split('%').next().unwrap_or("").trim()) {
                        match package {
                            // Delayed floats are copied line by line until a
                            // line that is exactly `\end{figure}`.
                            "endfloat" => {
                                for env in ["figure", "figure*", "table", "table*"] {
                                    self.verbatim_envs.insert(env.to_string());
                                }
                            }
                            // `\label` reads its argument with `\@sanitize`.
                            "showlabels" => {
                                self.verbatim_commands
                                    .insert("label".to_string(), spec(false, 1, Delim::Braces));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn add_short_verb(&mut self, text: &str, end: usize) {
        if let Some(c) = short_verb_char(text, end) {
            if !self.short_verb.contains(&c) {
                self.short_verb.push(c);
            }
        }
    }

    /// How the command `name` reads its arguments, if verbatim.
    fn verbatim_command(&self, name: &str) -> Option<ArgSpec> {
        builtin_verbatim_command(name).or_else(|| self.verbatim_commands.get(name).copied())
    }
}

/// Keys of tcolorbox that write the body to a file or swallow it verbatim.
const TCOLORBOX_VERBATIM_KEYS: &[&str] = &["saveto", "savelowerto", "void"];

/// Environments whose verbatim body is only displayed, never read as TeX:
/// definitions and `\usepackage` there are examples.
const DISPLAYED_VERBATIM_ENVS: &[&str] = &[
    "verbatim",
    "verbatim*",
    "Verbatim",
    "Verbatim*",
    "BVerbatim",
    "BVerbatim*",
    "LVerbatim",
    "LVerbatim*",
    "lstlisting",
    "minted",
    "macrocode",
    "macrocode*",
];

fn builtin_verbatim_command(name: &str) -> Option<ArgSpec> {
    VERBATIM_COMMANDS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
}

/// Skips blanks, line ends, comments and `[...]` groups after `i`.
fn skip_options(b: &[u8], mut i: usize) -> usize {
    loop {
        i = skip_filler(b, i);
        if b.get(i) != Some(&b'[') {
            return i;
        }
        i = memchr(b']', &b[i..]).map_or(b.len(), |p| i + p + 1);
    }
}

/// Inside a definition, these make the defined command or environment change
/// how blanks or line ends are read.
const GUARD_MARKERS: &[&str] = &[
    "\\obeyspaces",
    "\\obeylines",
    "\\obeywhitespace",
    "\\@vobeyspaces",
    "\\catcode",
    "\\endlinechar",
    "\\dospecials",
    "\\@sanitize",
    "\\@makeother",
];

/// The text of a definition from `i`: `count` items, each a braced group or a
/// control sequence, skipping blanks, `*`, `[...]`, comments and `#1`-style
/// parameters in between. At most 4000 bytes.
fn definition(text: &str, i: usize, count: usize) -> &str {
    let b = text.as_bytes();
    let limit = (i + 4000).min(b.len());
    let mut j = i;
    let mut found = 0;
    while j < limit && found < count {
        match b[j] {
            b' ' | b'\t' | b'\n' | b'\r' | b'*' => j += 1,
            b'%' => {
                while j < limit && b[j] != b'\n' {
                    j += 1;
                }
            }
            b'[' => {
                while j < limit && b[j] != b']' {
                    j += 1;
                }
                j += 1;
            }
            b'#' => j += 2,
            b'{' => {
                let mut depth = 0usize;
                while j < limit {
                    match b[j] {
                        b'\\' => j += 1,
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                j += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                found += 1;
            }
            b'\\' => {
                j += 1;
                if j < limit && is_letter(b[j]) {
                    while j < limit && is_letter(b[j]) {
                        j += 1;
                    }
                } else {
                    j += 1;
                }
                found += 1;
            }
            _ => break,
        }
    }
    &text[i..floor_char_boundary(text, j.min(limit))]
}

/// How a command with this xparse argument specification reads its `v`
/// (verbatim) argument, if it has one. With only `m`, `o`, `O{..}` and `s`
/// before it, those are skipped as the command reads them; after any other
/// argument type the rest of the line is kept.
fn xparse_verbatim(spec_text: &str) -> Option<ArgSpec> {
    let b = spec_text.as_bytes();
    // The index after the token (`\name` or one character) at `i`.
    let token = |i: usize| -> usize {
        let i = skip_filler(b, i);
        if b.get(i) != Some(&b'\\') {
            return (i + 1).min(b.len());
        }
        let mut k = i + 1;
        while k < b.len() && is_letter(b[k]) {
            k += 1;
        }
        if k == i + 1 {
            k += 1;
        }
        k.min(b.len())
    };
    // The index after the `{...}` group at `i` (or `i` without one).
    let group = |i: usize| -> usize {
        let i = skip_filler(b, i);
        balanced_group(spec_text, i).map_or(i, |g| i + g.len() + 2)
    };
    let (mut braced, mut optional, mut star, mut regular) = (0u8, false, false, true);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        i += 1;
        match c {
            b' ' | b'\t' | b'\n' | b'\r' | b'+' | b'!' | b'~' => {}
            b'%' => i = skip_filler(b, i - 1),
            b'>' | b'=' => i = group(i),
            b'm' => braced = braced.saturating_add(1),
            // Read before the first `{...}` argument only.
            b'o' | b'O' | b's' => {
                regular &= braced == 0;
                if c == b's' {
                    star = true;
                } else {
                    optional = true;
                }
                if c == b'O' {
                    i = group(i);
                }
            }
            b'v' => {
                return Some(if regular {
                    ArgSpec {
                        star,
                        optional,
                        args: braced.saturating_add(1),
                        delim: Delim::Either,
                    }
                } else {
                    spec(false, 1, Delim::Rest)
                });
            }
            b'r' | b'd' => {
                regular = false;
                i = token(token(i));
            }
            b'R' | b'D' => {
                regular = false;
                i = group(token(token(i)));
            }
            b't' => {
                regular = false;
                i = token(i);
            }
            b'e' => {
                regular = false;
                i = group(i);
            }
            b'E' => {
                regular = false;
                i = group(group(i));
            }
            _ => regular = false,
        }
    }
    None
}

pub(crate) fn memchr(needle: u8, hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&c| c == needle)
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn is_letter(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'@'
}

fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn skip_blanks(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_blank(b[i]) {
        i += 1;
    }
    i
}

/// Skips what TeX skips between a command and its arguments: blanks, line
/// ends and comments.
fn skip_filler(b: &[u8], mut i: usize) -> usize {
    loop {
        while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if b.get(i) != Some(&b'%') {
            return i;
        }
        while i < b.len() && b[i] != b'\n' {
            i += 1;
        }
    }
}

/// `[name]` after position `i` (an optional star first).
fn optional_arg(text: &str, i: usize) -> Option<&str> {
    let b = text.as_bytes();
    let mut j = skip_filler(b, i);
    if b.get(j) == Some(&b'*') {
        j = skip_filler(b, j + 1);
    }
    if b.get(j) != Some(&b'[') {
        return None;
    }
    let close = text[j..].find(']')? + j;
    let inner = text[j + 1..close].trim();
    (!inner.is_empty() && !inner.contains(['{', '\\', '\n'])).then_some(inner)
}

/// The first `{name}` after position `i`, skipping a star and one `[...]`
/// group.
fn first_braced_arg(text: &str, i: usize) -> Option<&str> {
    let b = text.as_bytes();
    let mut j = skip_filler(b, i);
    if b.get(j) == Some(&b'*') {
        j = skip_filler(b, j + 1);
    }
    if b.get(j) == Some(&b'[') {
        j = text[j..].find(']')? + j + 1;
        j = skip_filler(b, j);
    }
    if b.get(j) != Some(&b'{') {
        return None;
    }
    let close = text[j..].find('}')? + j;
    let inner = text[j + 1..close].trim();
    (!inner.is_empty() && !inner.contains(['{', '\n', '%'])).then_some(inner)
}

/// The contents of the `{...}` group (nested braces allowed) at `i`.
fn balanced_group(text: &str, i: usize) -> Option<&str> {
    let b = text.as_bytes();
    let start = skip_filler(b, i);
    if b.get(start) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    for (k, &c) in b.iter().enumerate().skip(start).take(400) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start + 1..k]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The command name defined at `i` (`{\name}`, `\name`, or `*{\name}`) and
/// the index after it (after the `}` when braced).
fn defined_command(text: &str, i: usize) -> Option<(&str, usize)> {
    let b = text.as_bytes();
    let mut j = skip_filler(b, i);
    if b.get(j) == Some(&b'*') {
        j = skip_filler(b, j + 1);
    }
    let braced = b.get(j) == Some(&b'{');
    if braced {
        j = skip_filler(b, j + 1);
    }
    if b.get(j) != Some(&b'\\') {
        return None;
    }
    let start = j + 1;
    let mut end = start;
    while end < b.len() && is_letter(b[end]) {
        end += 1;
    }
    if end == start {
        return None;
    }
    let mut after = end;
    if braced {
        let k = skip_filler(b, end);
        if b.get(k) == Some(&b'}') {
            after = k + 1;
        }
    }
    Some((&text[start..end], after))
}

/// The character of `\MakeShortVerb{\|}`, `\MakeShortVerb*\|`,
/// `\lstMakeShortInline[opts]|` and similar.
fn short_verb_char(text: &str, i: usize) -> Option<u8> {
    let b = text.as_bytes();
    let mut j = skip_filler(b, i);
    if b.get(j) == Some(&b'*') {
        j = skip_filler(b, j + 1);
    }
    if b.get(j) == Some(&b'[') {
        j = skip_filler(b, text[j..].find(']')? + j + 1);
    }
    if b.get(j) == Some(&b'{') {
        j = skip_filler(b, j + 1);
    }
    if b.get(j) == Some(&b'\\') {
        j += 1;
    }
    let c = *b.get(j)?;
    (c.is_ascii_graphic() && !c.is_ascii_alphanumeric() && !matches!(c, b'{' | b'}' | b'\\' | b'%'))
        .then_some(c)
}

/// An error that makes the formatter leave a file unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// Formatting would change how TeX reads the file from this input line.
    Changed { line: usize },
    /// The project loads a class or package that may read text verbatim
    /// in ways the formatter does not know (`None`: its name is computed).
    UnknownPackage { class: bool, name: Option<String> },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Changed { line } => write!(
                f,
                "line {line}: formatting would change how TeX reads the file there, so it was left \
                 unchanged (please report this as a texres bug)"
            ),
            FormatError::UnknownPackage { class, name } => {
                let what = match name {
                    Some(name) if *class => format!("the class `{name}`"),
                    Some(name) => format!("the package `{name}`"),
                    None => "a class or package whose name is computed".to_string(),
                };
                write!(
                    f,
                    "left unchanged: the project loads {what}, which texres fmt does not know; \
                     it may read text verbatim. If it does not, or once its verbatim environments \
                     and commands are listed in `verbatim-envs` and `verbatim-commands`, add it \
                     to `known-packages` in .texresfmt.toml"
                )
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// How LaTeX reads a file: packages and classes are loaded with `@` a
/// letter; in documents it is one only after `\makeatletter`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceKind {
    #[default]
    Document,
    Package,
}

impl SourceKind {
    /// `.sty` and `.cls` files are packages; everything else is a document.
    pub fn of(path: &Path) -> Self {
        let package = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("sty") || e.eq_ignore_ascii_case("cls"));
        if package {
            SourceKind::Package
        } else {
            SourceKind::Document
        }
    }
}

/// What TeX reads with other category codes: built-in, project-defined and
/// configured verbatim commands, environments and short-verb characters.
/// Shared by the formatter and the safety check.
#[derive(Clone, Copy)]
pub(crate) struct Lexicon<'a> {
    pub cfg: &'a Config,
    pub extras: &'a Extras,
}

impl Lexicon<'_> {
    pub fn is_verbatim_env(&self, name: &str) -> bool {
        builtin_verbatim_env(name)
            || self.extras.verbatim_envs.contains(name)
            || self.cfg.verbatim_envs.iter().any(|e| e == name)
    }

    /// Whether the body of `\begin{name}` followed by `rest` (the rest of
    /// its line) is verbatim: a verbatim environment, or one given
    /// tcolorbox's `saveto`, `savelowerto` or `void` key.
    pub fn is_verbatim_instance(&self, name: &str, rest: &str) -> bool {
        self.is_verbatim_env(name)
            || (rest.trim_start_matches([' ', '\t']).starts_with('[')
                && TCOLORBOX_VERBATIM_KEYS.iter().any(|k| rest.contains(k)))
    }

    /// How the arguments right after `\begin{name}` are read, if verbatim.
    pub fn verbatim_env_args(&self, name: &str) -> Option<ArgSpec> {
        VERBATIM_ARGUMENT_ENVS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| *s)
    }

    pub fn verbatim_command(&self, name: &str) -> Option<ArgSpec> {
        if let Some(s) = builtin_verbatim_command(name) {
            return Some(s);
        }
        if !self.extras.verbatim_commands.is_empty() {
            if let Some(s) = self.extras.verbatim_commands.get(name) {
                return Some(*s);
            }
        }
        self.cfg
            .verbatim_commands
            .iter()
            .any(|c| c == name)
            .then_some(spec(true, 1, Delim::Either))
    }

    /// Whether `c` delimits `\verb`-like text where the formatter's lexer
    /// reaches it (not blanks, `%`, `\`, braces, brackets, `$` or `&`).
    pub fn short_verb(&self, c: u8) -> bool {
        !matches!(
            c,
            b' ' | b'\t' | b'%' | b'\\' | b'{' | b'}' | b'[' | b']' | b'$' | b'&'
        ) && self.extras.short_verb.contains(&c)
    }
}

/// Formats LaTeX source. The result is checked against the input with TeX's
/// tokenizer; if they would read differently, nothing is changed and an
/// error is returned.
pub fn format_source(
    source: &str,
    config: &Config,
    extras: &Extras,
    kind: SourceKind,
) -> Result<String, FormatError> {
    let par_sections = if config.blank_line_before_sections {
        extras.sections.par_sections()
    } else {
        0
    };
    let formatted = Formatter::new(config, extras, source, kind, par_sections).run(source);
    if formatted == source {
        return Ok(formatted);
    }
    if let Some(unknown) = extras.sections.unvetted(&config.known_packages) {
        return Err(FormatError::UnknownPackage {
            class: unknown.is_some_and(|(kind, _)| kind == FileKind::Class),
            name: unknown.map(|(_, name)| name.to_string()),
        });
    }
    let allow = Allowances {
        par_sections: tokens::section_tokens(par_sections),
        spaces_around_ampersands: config.align_columns,
    };
    let lex = Lexicon {
        cfg: config,
        extras,
    };
    match tokens::first_difference(source, &formatted, &allow, lex, kind) {
        None => Ok(formatted),
        Some((line, _)) => Err(FormatError::Changed { line }),
    }
}

#[derive(Clone, Debug)]
enum Kind {
    Brace,
    Bracket,
    Env {
        name: Rc<str>,
        align: Option<usize>,
        fragile: bool,
    },
    Item,
    /// `\begingroup ... \endgroup`
    Semi,
    /// `\[ ... \]`
    Display,
    /// `\( ... \)`
    Inline,
}

#[derive(Clone, Debug)]
struct Frame {
    kind: Kind,
    indent: bool,
    nowrap: bool,
    /// `$` and `$$` state when the frame opened, restored when it closes.
    saved_math: (bool, bool),
    /// Whether `@` was a letter when the frame opened; the end of a group
    /// restores it (`{\makeatletter ...}`, `\begin{x}\makeatletter\end{x}`).
    saved_at: bool,
}

impl Frame {
    /// Frames that are TeX groups (their end ends local assignments).
    fn is_group(&self) -> bool {
        !matches!(self.kind, Kind::Bracket | Kind::Item)
    }
}

/// A protected argument that continues on the next line.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Span {
    pub spec: ArgSpec,
    pub done: u8,
    pub depth: usize,
}

#[derive(Clone, Debug)]
pub(crate) enum Raw {
    /// Inside a verbatim environment until this `\end{...}`.
    Env(Rc<str>),
    /// Inside a verbatim argument.
    Span(Span),
}

#[derive(Clone, Debug, Default)]
struct State {
    stack: Vec<Frame>,
    raw: Option<Raw>,
    /// Lines are kept while the number of open groups is at least this.
    guard: Option<usize>,
    level: usize,
    groups: usize,
    nowrap: usize,
    dollar: bool,
    ddollar: bool,
    in_preamble: bool,
    /// Whether `@` is a letter: in packages, and after `\makeatletter`.
    at_letter: bool,
    next_align_id: usize,
}

#[derive(Clone, Copy, Debug)]
struct Run {
    start: usize,
    end: usize,
    breakable: bool,
}

/// What the lexer learned about one output line.
#[derive(Debug, Default)]
struct Scan {
    /// Copy the line unchanged.
    keep: bool,
    indent: usize,
    content_start: usize,
    /// From here to the end of the line nothing may change.
    raw_from: Option<usize>,
    has_comment: bool,
    /// Whether a `\par` right after this line could not be taken as a macro
    /// argument (see `ends_safely`); `None` for a line without tokens.
    ends_safely: Option<bool>,
    /// Blank runs inside the line that may be rewritten.
    runs: Vec<Run>,
    /// Split before an `\item`: the line ends here and the next starts at `rest`.
    stop: Option<usize>,
    rest: usize,
    section: bool,
    opened: bool,
    align: Option<(usize, Vec<usize>)>,
}

#[derive(Debug)]
struct Line {
    text: String,
    has_comment: bool,
    opened: bool,
    indent_len: usize,
    align: Option<(usize, Vec<usize>)>,
}

#[derive(Debug)]
enum Out {
    Blank,
    Kept(String),
    Line(Line),
}

/// Alignment environments whose cells are all math, where blanks are ignored.
const MATH_ALIGN_ENVS: &[&str] = &[
    "align",
    "alignat",
    "flalign",
    "aligned",
    "alignedat",
    "split",
    "eqnarray",
    "array",
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

struct Formatter<'a> {
    cfg: &'a Config,
    extras: &'a Extras,
    st: State,
    out: Vec<Out>,
    /// Stack depth of the first indenting frame opened on the current line.
    line_mark: Option<usize>,
    /// For each alignment environment instance: whether its cells are math.
    math_aligns: HashMap<usize, bool>,
    /// Whether the last line with tokens ends so that a following `\par`
    /// cannot be a macro argument; only then may blank lines be dropped or
    /// added after it.
    prev_safe: bool,
    /// Sectioning commands (bits, see `sections`) known to start with
    /// `\par`: only these get a blank line before them.
    par_sections: u8,
}

impl<'a> Formatter<'a> {
    fn new(
        cfg: &'a Config,
        extras: &'a Extras,
        source: &str,
        kind: SourceKind,
        par_sections: u8,
    ) -> Self {
        let st = State {
            in_preamble: source.contains("\\begin{document}"),
            at_letter: kind == SourceKind::Package,
            ..State::default()
        };
        Formatter {
            cfg,
            extras,
            st,
            out: Vec::new(),
            line_mark: None,
            prev_safe: false,
            math_aligns: HashMap::new(),
            par_sections,
        }
    }

    fn run(mut self, source: &str) -> String {
        if source.is_empty() {
            return String::new();
        }
        let eol = match source.find('\n') {
            Some(p) if p > 0 && source.as_bytes()[p - 1] == b'\r' => "\r\n",
            _ => "\n",
        };
        let body = source.strip_suffix('\n').unwrap_or(source);
        // Inside `% texres-fmt: off` ... `% texres-fmt: on`, and on lines
        // marked `skip`, lines are lexed (to keep track of groups and
        // environments) but copied unchanged.
        let mut off = false;
        let mut skip_next = false;
        for line in body.split('\n') {
            let in_raw = self.st.raw.is_some() || self.st.guard.is_some();
            let text = line.strip_suffix('\r').unwrap_or(line);
            let marker = if in_raw { None } else { marker(text) };
            let keep = off || skip_next || marker.is_some();
            skip_next = false;
            match marker {
                Some(Marker::Off) => off = true,
                Some(Marker::On) => off = false,
                Some(Marker::Skip { own_line }) => skip_next = own_line,
                None => {}
            }
            if keep {
                self.keep_line(line, text);
                continue;
            }
            if !in_raw && text.bytes().all(is_blank) {
                self.st.dollar = false;
                self.st.ddollar = false;
                self.push_blank();
                continue;
            }
            self.format_line(line, text);
        }
        if self.cfg.align_columns {
            self.align_columns();
        }
        let mut result = String::with_capacity(source.len() + source.len() / 8);
        for out in &self.out {
            match out {
                Out::Blank => result.push_str(eol),
                Out::Kept(text) => {
                    result.push_str(text);
                    result.push('\n');
                }
                Out::Line(line) => {
                    result.push_str(&line.text);
                    result.push_str(eol);
                }
            }
        }
        result
    }

    fn push_blank(&mut self) {
        let blanks = self
            .out
            .iter()
            .rev()
            .take_while(|o| matches!(o, Out::Blank))
            .count();
        // After `\fbox` (say) the first blank line is its argument and the
        // second ends the paragraph: there every blank line stays.
        if blanks < self.cfg.max_blank_lines || !self.prev_safe {
            self.out.push(Out::Blank);
        }
    }

    /// Copies a source line unchanged, lexing it so that the state stays
    /// right for the lines after it.
    fn keep_line(&mut self, original: &str, text: &str) {
        let in_raw = self.st.raw.is_some() || self.st.guard.is_some();
        if !in_raw && text.bytes().all(is_blank) {
            self.st.dollar = false;
            self.st.ddollar = false;
        } else {
            let mut rest = text;
            loop {
                let scan = self.scan_line(rest, None);
                match scan.stop {
                    Some(_) if !scan.keep => rest = &rest[scan.rest..],
                    _ => break,
                }
            }
        }
        self.out.push(Out::Kept(original.to_string()));
        self.prev_safe = false;
    }

    /// Formats one source line, which may become several output lines.
    fn format_line(&mut self, original: &str, text: &str) {
        let mut rest = text;
        let mut first = true;
        loop {
            let snapshot = (self.cfg.wrap && self.may_be_too_wide(rest)).then(|| self.st.clone());
            let mut scan = self.scan_line(rest, None);
            if scan.keep {
                // Only whole source lines are kept (a continuation piece
                // never starts inside raw material).
                let kept = if first { original } else { rest };
                self.out.push(Out::Kept(kept.to_string()));
                self.prev_safe = false;
                return;
            }
            let piece_end = scan.stop.unwrap_or(rest.len());
            let mut built = self.build(&rest[..piece_end], &scan);
            let mut next = scan.stop.map(|_| scan.rest);
            if let Some(snapshot) = snapshot {
                if let Some(run) = self.choose_break(&built, &scan) {
                    self.st = snapshot;
                    scan = self.scan_line(rest, Some(run.start));
                    built = self.build(&rest[..run.start], &scan);
                    next = Some(run.end);
                }
            }
            self.emit(built, &scan);
            first = false;
            match next {
                Some(n) => rest = &rest[n..],
                None => return,
            }
        }
    }

    /// Whether `rest` might come out wider than the line width: its
    /// indentation is at most the current level, and a tab widens to at
    /// most a tab stop.
    fn may_be_too_wide(&self, rest: &str) -> bool {
        let content = rest.trim_start_matches([' ', '\t']);
        let tabs = content.bytes().filter(|&c| c == b'\t').count();
        self.st.level * self.cfg.indent_width + content.len() + tabs * self.cfg.tab_width
            > self.cfg.line_width
    }

    fn emit(&mut self, built: Built, scan: &Scan) {
        if scan.section {
            let after_text = self.prev_safe
                && matches!(self.out.last(), Some(Out::Line(prev)) if !prev.has_comment && !prev.opened);
            if after_text {
                self.out.push(Out::Blank);
            }
        }
        self.out.push(Out::Line(Line {
            text: built.text,
            has_comment: scan.has_comment,
            opened: scan.opened,
            indent_len: built.indent_len,
            align: scan.align.as_ref().map(|(id, _)| (*id, built.amps)),
        }));
        if let Some(safe) = scan.ends_safely {
            self.prev_safe = safe;
        }
    }

    fn choose_break(&self, built: &Built, scan: &Scan) -> Option<Run> {
        if built.width <= self.cfg.line_width {
            return None;
        }
        let fitting = built
            .breaks
            .iter()
            .rev()
            .find(|(col, _)| *col <= self.cfg.line_width);
        let chosen = fitting.or_else(|| built.breaks.first())?;
        Some(scan.runs[chosen.1])
    }

    // ----- stack -----

    fn in_math(&self) -> bool {
        self.st.dollar
            || self.st.ddollar
            || self
                .st
                .stack
                .iter()
                .any(|f| matches!(f.kind, Kind::Display | Kind::Inline))
    }

    fn push(&mut self, kind: Kind, indent: bool, nowrap: bool) {
        let indent = indent && !self.line_mark.is_some_and(|m| m < self.st.stack.len());
        if indent {
            self.line_mark = Some(self.st.stack.len());
            self.st.level += 1;
        }
        let frame = Frame {
            kind,
            indent,
            nowrap,
            saved_math: (self.st.dollar, self.st.ddollar),
            saved_at: self.st.at_letter,
        };
        if frame.is_group() {
            self.st.groups += 1;
        }
        if nowrap {
            self.st.nowrap += 1;
        }
        self.st.stack.push(frame);
    }

    fn pop(&mut self) -> Frame {
        let frame = self.st.stack.pop().expect("pop on empty stack");
        if frame.indent {
            self.st.level -= 1;
        }
        if frame.nowrap {
            self.st.nowrap -= 1;
        }
        if frame.is_group() {
            self.st.groups -= 1;
            if !matches!(frame.kind, Kind::Display | Kind::Inline) {
                (self.st.dollar, self.st.ddollar) = frame.saved_math;
            }
            self.st.at_letter = frame.saved_at;
            if self.st.guard.is_some_and(|g| self.st.groups < g) {
                self.st.guard = None;
            }
        }
        if self.line_mark.is_some_and(|m| self.st.stack.len() <= m) {
            self.line_mark = None;
        }
        frame
    }

    /// Pops through the innermost frame matching `pred`, unless a frame for
    /// which `barrier` holds comes first. Returns the popped matching frame.
    fn pop_to(
        &mut self,
        pred: impl Fn(&Frame) -> bool,
        barrier: impl Fn(&Frame) -> bool,
    ) -> Option<Frame> {
        let index = self.st.stack.iter().rposition(|f| pred(f) || barrier(f))?;
        if !pred(&self.st.stack[index]) {
            return None;
        }
        while self.st.stack.len() > index + 1 {
            self.pop();
        }
        Some(self.pop())
    }

    fn top(&self) -> Option<&Frame> {
        self.st.stack.last()
    }

    fn can_break(&self) -> bool {
        self.cfg.wrap && !self.st.in_preamble && self.st.nowrap == 0 && !self.in_math()
    }

    /// Skips the verbatim arguments read as `spec` says from `from` in the
    /// line `b` (of a command or environment at `cs_start`). Returns where
    /// lexing resumes, or `None` when they continue on the next line.
    fn verbatim_span(
        &mut self,
        b: &[u8],
        from: usize,
        cs_start: usize,
        spec: ArgSpec,
        sc: &mut Scan,
    ) -> Option<usize> {
        let mut span = Span {
            spec,
            done: 0,
            depth: 0,
        };
        match scan_span(b, from, &mut span) {
            Some(e) => {
                // Keep the rest of the line when the span runs to its end
                // (its trailing blanks may be verbatim) or holds a `%`: read
                // with ordinary category codes, everything after that `%`
                // would be a comment, so the safety check could not compare
                // it.
                let to_end = e >= b.len() && span.done < span.spec.args;
                if (to_end || b[from..e.max(from)].contains(&b'%')) && sc.raw_from.is_none() {
                    sc.raw_from = Some(cs_start);
                }
                Some(e)
            }
            None => {
                if sc.raw_from.is_none() {
                    sc.raw_from = Some(cs_start);
                }
                self.st.raw = Some(Raw::Span(span));
                None
            }
        }
    }

    fn lex(&self) -> Lexicon<'a> {
        Lexicon {
            cfg: self.cfg,
            extras: self.extras,
        }
    }

    fn is_nowrap_env(&self, name: &str) -> bool {
        let base = name.trim_end_matches('*');
        NO_WRAP_ENVS.contains(&base)
            || self.cfg.no_wrap_envs.iter().any(|e| e == name)
            || self.cfg.align_envs.iter().any(|e| e == name)
    }

    /// Whether a blank line may go before the sectioning command `name`:
    /// its definition starts with `\par`, and it is used at the top level.
    fn section_allowed(&self, name: &str) -> bool {
        sections::has(self.par_sections, name)
            && !self.in_math()
            && self
                .st
                .stack
                .iter()
                .all(|f| matches!(f.kind, Kind::Env { .. }) && !f.indent)
    }

    // ----- lexer -----

    /// Lexes one output line, updating the state. With `stop_at`, lexing
    /// ends there (a wrap point).
    fn scan_line(&mut self, line: &str, stop_at: Option<usize>) -> Scan {
        let b = line.as_bytes();
        let mut sc = Scan::default();
        self.line_mark = None;
        let start_len = self.st.stack.len();
        let mut min_len = start_len;
        let mut i = 0;

        // A line that starts inside raw material is kept whole.
        if self.st.raw.is_some() || self.st.guard.is_some() {
            sc.keep = true;
            match self.st.raw.take() {
                Some(Raw::Env(end)) => match line.find(&*end) {
                    Some(p) => i = p + end.len(),
                    None => {
                        self.st.raw = Some(Raw::Env(end));
                        return sc;
                    }
                },
                Some(Raw::Span(mut span)) => match scan_span(b, 0, &mut span) {
                    Some(e) => i = e,
                    None => {
                        self.st.raw = Some(Raw::Span(span));
                        return sc;
                    }
                },
                None => {}
            }
        } else {
            i = skip_blanks(b, 0);
        }
        sc.content_start = i;

        let mut indent: Option<usize> = None;
        // Alignment row: (env id, stack depth) and whether it is still valid.
        let mut row: Option<(usize, usize)> = None;
        let mut row_ok = true;
        let mut amps = Vec::new();
        // Kind of the previous token, for `[` at the end of a line.
        let mut after_command = false;

        macro_rules! content {
            () => {
                if indent.is_none() {
                    indent = Some(self.st.level);
                    if let Some(Frame {
                        kind: Kind::Env {
                            align: Some(id), ..
                        },
                        ..
                    }) = self.top()
                    {
                        row = Some((*id, self.st.stack.len()));
                    }
                }
            };
        }
        macro_rules! track_min {
            () => {
                min_len = min_len.min(self.st.stack.len());
                if row.is_some_and(|(_, depth)| self.st.stack.len() < depth) {
                    row_ok = false;
                }
            };
        }

        while i < b.len() {
            if stop_at.is_some_and(|s| i >= s) {
                break;
            }
            let c = b[i];
            let modifiable = !sc.keep && sc.raw_from.is_none();
            match c {
                b' ' | b'\t' => {
                    let start = i;
                    i = skip_blanks(b, i);
                    if modifiable && i < b.len() && b[start - 1] != b'^' {
                        let breakable = b[i] != b'%' && self.can_break();
                        sc.runs.push(Run {
                            start,
                            end: i,
                            breakable,
                        });
                    }
                    continue;
                }
                b'%' => {
                    content!();
                    sc.has_comment = true;
                    break;
                }
                b'\\' => {
                    let cs_start = i;
                    i += 1;
                    if i >= b.len() {
                        content!();
                        break;
                    }
                    if !is_letter(b[i]) {
                        let sym = b[i];
                        i += utf8_len(sym);
                        after_command = false;
                        match sym {
                            b']' if matches!(self.top().map(|f| &f.kind), Some(Kind::Display)) => {
                                self.pop();
                                track_min!();
                            }
                            b')' if matches!(self.top().map(|f| &f.kind), Some(Kind::Inline)) => {
                                self.pop();
                                track_min!();
                            }
                            b'[' => {
                                content!();
                                self.push(Kind::Display, true, true);
                            }
                            b'(' => {
                                content!();
                                self.push(Kind::Inline, false, true);
                            }
                            _ => content!(),
                        }
                        continue;
                    }
                    let name_start = i;
                    while i < b.len() && is_letter(b[i]) {
                        i += 1;
                    }
                    let mut name = &line[name_start..i];
                    // Where `@` may not be a letter, `\Q@code@` can be `\Q`
                    // reading `@code@` verbatim. A document may still be read
                    // with `@` a letter (`\input` after `\makeatletter`), so
                    // the verbatim reading, which changes nothing, wins.
                    if !self.st.at_letter {
                        if let Some(p) = name.find('@').filter(|&p| p > 0) {
                            if self.lex().verbatim_command(&name[..p]).is_some() {
                                name = &name[..p];
                                i = name_start + p;
                            }
                        }
                    }
                    // `\left[`, `\big[` ... are math delimiters, not optional arguments.
                    after_command = !matches!(name, "left" | "right" | "middle")
                        && !name.starts_with("big")
                        && !name.starts_with("Big");
                    match name {
                        "begin" => {
                            content!();
                            let Some((env, after)) = env_name(line, i) else {
                                continue;
                            };
                            i = after;
                            after_command = false;
                            if self.lex().is_verbatim_instance(env, &line[after..]) {
                                let end: Rc<str> = format!("\\end{{{env}}}").into();
                                if sc.raw_from.is_none() {
                                    sc.raw_from = Some(after);
                                }
                                match line[after..].find(&*end) {
                                    Some(p) => i = after + p + end.len(),
                                    None => {
                                        self.st.raw = Some(Raw::Env(end));
                                        break;
                                    }
                                }
                                continue;
                            }
                            if env == "document" {
                                self.st.in_preamble = false;
                            }
                            let align = (self.cfg.align_columns
                                && self.cfg.align_envs.iter().any(|e| e == env))
                            .then(|| {
                                self.st.next_align_id += 1;
                                let id = self.st.next_align_id;
                                let base = env.trim_end_matches('*');
                                self.math_aligns.insert(id, MATH_ALIGN_ENVS.contains(&base));
                                id
                            });
                            let fragile = env == "frame"
                                && (line[after..].contains("fragile")
                                    || line[after..].contains("containsverbatim"));
                            let indent_body = !self.cfg.no_indent_envs.iter().any(|e| e == env);
                            let nowrap = fragile || self.is_nowrap_env(env);
                            self.push(
                                Kind::Env {
                                    name: env.into(),
                                    align,
                                    fragile,
                                },
                                indent_body,
                                nowrap,
                            );
                            if let Some(arg_spec) = self.lex().verbatim_env_args(env) {
                                match self.verbatim_span(b, after, cs_start, arg_spec, &mut sc) {
                                    Some(e) => i = e,
                                    None => break,
                                }
                            }
                        }
                        "end" => {
                            let Some((env, after)) = env_name(line, i) else {
                                content!();
                                continue;
                            };
                            i = after;
                            after_command = false;
                            let popped = self.pop_to(
                                |f| matches!(&f.kind, Kind::Env { name, .. } if &**name == env),
                                |f| matches!(f.kind, Kind::Brace),
                            );
                            track_min!();
                            let fragile = matches!(
                                popped,
                                Some(Frame {
                                    kind: Kind::Env { fragile: true, .. },
                                    ..
                                })
                            );
                            if fragile && indent.is_none() {
                                // beamer finds the end of a fragile frame by
                                // comparing whole lines: keep this one as is.
                                sc.keep = true;
                            }
                            if popped.is_none() {
                                content!();
                            }
                        }
                        "item" => {
                            let split = self.cfg.one_item_per_line
                                && modifiable
                                && indent.is_some()
                                && sc.runs.last().is_some_and(|r| r.end == cs_start)
                                && matches!(
                                    self.top().map(|f| &f.kind),
                                    Some(Kind::Item | Kind::Env { .. })
                                )
                                && !self.in_math();
                            if split {
                                let run = sc.runs.pop().expect("checked above");
                                sc.stop = Some(run.start);
                                sc.rest = cs_start;
                                break;
                            }
                            if matches!(self.top().map(|f| &f.kind), Some(Kind::Item)) {
                                self.pop();
                                track_min!();
                            }
                            content!();
                            if matches!(self.top().map(|f| &f.kind), Some(Kind::Env { .. })) {
                                self.push(Kind::Item, true, false);
                            }
                        }
                        "multicolumn" => {
                            content!();
                            row_ok = false;
                        }
                        "makeatletter" | "makeatother" => {
                            content!();
                            self.st.at_letter = name == "makeatletter";
                        }
                        "begingroup" => {
                            content!();
                            self.push(Kind::Semi, false, false);
                        }
                        "endgroup" => {
                            content!();
                            self.pop_to(
                                |f| matches!(f.kind, Kind::Semi),
                                |f| matches!(f.kind, Kind::Brace | Kind::Env { .. }),
                            );
                            track_min!();
                        }
                        _ => {
                            content!();
                            if sc.content_start == cs_start
                                && self.section_allowed(name)
                                && is_section_call(b, i)
                            {
                                sc.section = true;
                            }
                            if let Some(arg_spec) = self.lex().verbatim_command(name) {
                                match self.verbatim_span(b, i, cs_start, arg_spec, &mut sc) {
                                    Some(e) => i = e,
                                    None => break,
                                }
                                after_command = false;
                            } else if GUARD_WORDS.contains(&name)
                                || self.extras.guard_commands.contains(name)
                                || (name == "catcode" && catcode_guard(b, i))
                            {
                                if self.st.guard.is_none() {
                                    self.st.guard = Some(self.st.groups);
                                }
                                if sc.raw_from.is_none() {
                                    sc.raw_from = Some(cs_start);
                                }
                            }
                        }
                    }
                }
                b'{' => {
                    content!();
                    i += 1;
                    after_command = false;
                    self.push(Kind::Brace, true, false);
                }
                b'}' => {
                    i += 1;
                    after_command = true;
                    if self
                        .pop_to(|f| matches!(f.kind, Kind::Brace), |_| false)
                        .is_none()
                    {
                        content!();
                    }
                    track_min!();
                }
                b'[' => {
                    content!();
                    i += 1;
                    // A wrap point ends the line as much as its real end.
                    let line_end = stop_at.map_or(b.len(), |s| s.min(b.len()));
                    if after_command && modifiable && line_ends_open(&b[..line_end], i) {
                        self.push(Kind::Bracket, true, false);
                    }
                    after_command = false;
                }
                b']' => {
                    i += 1;
                    after_command = true;
                    if matches!(self.top().map(|f| &f.kind), Some(Kind::Bracket)) {
                        self.pop();
                        track_min!();
                    } else {
                        content!();
                    }
                }
                b'$' => {
                    content!();
                    after_command = false;
                    if b.get(i + 1) == Some(&b'$') && !self.st.dollar {
                        self.st.ddollar = !self.st.ddollar;
                        i += 2;
                    } else {
                        self.st.dollar = !self.st.dollar;
                        i += 1;
                    }
                }
                b'&' => {
                    content!();
                    after_command = false;
                    if row.is_some_and(|(_, depth)| depth == self.st.stack.len()) {
                        amps.push(i);
                    } else {
                        row_ok = false;
                    }
                    i += 1;
                }
                _ => {
                    content!();
                    after_command = false;
                    if self.lex().short_verb(c) {
                        match memchr(c, &b[i + 1..]) {
                            Some(p) => {
                                if b[i + 1..i + 1 + p].contains(&b'%') && sc.raw_from.is_none() {
                                    sc.raw_from = Some(i);
                                }
                                i += p + 2;
                            }
                            None => {
                                if sc.raw_from.is_none() {
                                    sc.raw_from = Some(i);
                                }
                                break;
                            }
                        }
                    } else {
                        // Skip the run of characters that need no attention.
                        i += 1;
                        while i < b.len()
                            && !matches!(
                                b[i],
                                b' ' | b'\t'
                                    | b'%'
                                    | b'\\'
                                    | b'{'
                                    | b'}'
                                    | b'['
                                    | b']'
                                    | b'$'
                                    | b'&'
                            )
                            && !self.lex().short_verb(b[i])
                        {
                            i += 1;
                        }
                    }
                }
            }
        }
        sc.indent = indent.unwrap_or(self.st.level);
        sc.ends_safely = if sc.keep || sc.raw_from.is_some() {
            Some(false)
        } else {
            // Lexing stopped at a comment, a wrap point or the end of the
            // line; an `\item` split ends the line before its blank run.
            let end = sc.stop.unwrap_or(i).min(b.len());
            let lexed = line[sc.content_start.min(end)..end].trim_end_matches([' ', '\t']);
            (!lexed.is_empty()).then(|| ends_safely(lexed))
        };
        sc.opened = self.st.stack.len() > min_len;
        if let Some((id, depth)) = row {
            if row_ok
                && !sc.keep
                && sc.raw_from.is_none()
                && sc.stop.is_none()
                && stop_at.is_none()
                && depth == self.st.stack.len()
                && !amps.is_empty()
            {
                sc.align = Some((id, amps));
            }
        }
        sc
    }

    // ----- output -----

    fn build(&self, piece: &str, sc: &Scan) -> Built {
        let indent_len = sc.indent * self.cfg.indent_width;
        let mut text = String::with_capacity(indent_len + piece.len());
        text.extend(std::iter::repeat_n(' ', indent_len));
        let mut col = indent_len;
        let end = if sc.raw_from.is_some() {
            piece.len()
        } else {
            trimmed_end(piece)
        };
        let mut breaks = Vec::new();
        let mut amps_out = Vec::new();
        let amps: &[usize] = sc.align.as_ref().map_or(&[], |(_, a)| a);
        let mut amp_iter = amps.iter().peekable();
        let mut pos = sc.content_start.min(end);
        let mut copy = |text: &mut String, col: &mut usize, from: usize, to: usize| {
            while let Some(&&a) = amp_iter.peek() {
                if a >= to {
                    break;
                }
                amps_out.push(text.len() + (a - from));
                amp_iter.next();
            }
            let segment = &piece[from..to];
            text.push_str(segment);
            *col += segment.chars().count();
        };
        for (index, run) in sc.runs.iter().enumerate() {
            if run.start < pos || run.end > end {
                continue;
            }
            copy(&mut text, &mut col, pos, run.start);
            if run.breakable {
                breaks.push((col, index));
            }
            for &c in &piece.as_bytes()[run.start..run.end] {
                if c == b'\t' {
                    let n = self.cfg.tab_width - col % self.cfg.tab_width;
                    text.extend(std::iter::repeat_n(' ', n));
                    col += n;
                } else {
                    text.push(' ');
                    col += 1;
                }
            }
            pos = run.end;
        }
        copy(&mut text, &mut col, pos, end);
        Built {
            text,
            width: col,
            breaks,
            amps: amps_out,
            indent_len,
        }
    }

    /// Pads the cells of alignment rows so the `&` line up per environment.
    fn align_columns(&mut self) {
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for (index, out) in self.out.iter().enumerate() {
            if let Out::Line(Line {
                align: Some((id, amps)),
                ..
            }) = out
            {
                if !amps.is_empty() {
                    groups.entry(*id).or_default().push(index);
                }
            }
        }
        for (id, rows) in groups {
            let math = self.math_aligns.get(&id).copied().unwrap_or(false);
            let mut cells: Vec<(usize, Vec<String>)> = Vec::new();
            let mut widths: Vec<usize> = Vec::new();
            for &index in &rows {
                let Out::Line(line) = &self.out[index] else {
                    continue;
                };
                let (_, amps) = line.align.as_ref().expect("grouped rows have amps");
                let mut parts = Vec::with_capacity(amps.len() + 1);
                let mut from = line.indent_len;
                let mut safe = true;
                for &a in amps {
                    let cell = line.text[from..a].trim_matches(|c| c == ' ');
                    // In a text table the template's \unskip removes one
                    // blank before `&`; a blank added after material that
                    // ends in glue (`~`, `\quad`, `\hspace{..}`) would leave
                    // that glue in place. Rows needing one are not aligned.
                    if !math
                        && !line.text[..a].ends_with(' ')
                        && !cell.is_empty()
                        && !ends_in_glyph(cell)
                    {
                        safe = false;
                    }
                    parts.push(cell.to_string());
                    from = a + 1;
                }
                if !safe {
                    continue;
                }
                parts.push(line.text[from..].trim_start_matches(' ').to_string());
                for (j, part) in parts[..amps.len()].iter().enumerate() {
                    let w = part.chars().count();
                    if widths.len() <= j {
                        widths.push(w);
                    } else {
                        widths[j] = widths[j].max(w);
                    }
                }
                cells.push((index, parts));
            }
            for (index, parts) in cells {
                let Out::Line(line) = &mut self.out[index] else {
                    continue;
                };
                let mut text = line.text[..line.indent_len].to_string();
                let last = parts.len() - 1;
                for (j, part) in parts.iter().enumerate() {
                    if j < last {
                        text.push_str(part);
                        if widths[j] > 0 {
                            let pad = widths[j] - part.chars().count();
                            text.extend(std::iter::repeat_n(' ', pad + 1));
                        }
                        text.push('&');
                    } else if !part.is_empty() {
                        text.push(' ');
                        text.push_str(part);
                    }
                    if j + 1 < last {
                        text.push(' ');
                    }
                }
                line.text = text;
            }
        }
    }
}

/// Whether lexed line text ends in a token after which a `\par` cannot be a
/// macro argument: an inactive character, `}`, `$` or `&`, not a control
/// sequence (`\fbox`, `\\`), `{`, or a possibly active character (`~`, `"`).
/// Matches `tokens::ends_safely`.
fn ends_safely(text: &str) -> bool {
    let b = text.as_bytes();
    let Some(&last) = b.last() else {
        return false;
    };
    let before = |end: usize| b[..end].iter().rev().take_while(|&&c| c == b'\\').count();
    if last.is_ascii_alphabetic() {
        // A control word: letters preceded by an odd number of backslashes.
        let start = b.iter().rposition(|&c| !is_letter(c)).map_or(0, |p| p + 1);
        return before(start) % 2 == 0;
    }
    if before(b.len() - 1) % 2 == 1 {
        return false; // a control symbol such as `\\` or `\%`
    }
    !matches!(last, b'\\' | b'{' | b'~' | b'^' | b'_' | b'#' | b'"' | b'@')
}

/// Whether text ends in a character that typesets as a glyph (so a blank
/// after it is the last glue in the cell), not in a command, group or
/// possibly active character (`~`, babel shorthands) that might add glue.
fn ends_in_glyph(text: &str) -> bool {
    let Some(&last) = text.as_bytes().last() else {
        return false;
    };
    if last.is_ascii_alphanumeric() {
        // A letter may end a control word (`\quad`).
        let word = text.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '@');
        let backslashes = word.bytes().rev().take_while(|&c| c == b'\\').count();
        return !(last.is_ascii_alphabetic() && backslashes % 2 == 1);
    }
    matches!(
        last,
        b'.' | b',' | b')' | b'*' | b'+' | b'-' | b'/' | b'<' | b'>' | b'=' | b'$'
    ) && !text[..text.len() - 1].ends_with('\\')
}

/// A formatter directive in a comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Marker {
    Off,
    On,
    /// `skip`: on a line of its own it covers the next line too.
    Skip {
        own_line: bool,
    },
}

/// The directive a line ends with: `% texres-fmt: off`, `on` or `skip`, or
/// tex-fmt's `% tex-fmt: ...` spelling of the same.
fn marker(text: &str) -> Option<Marker> {
    let pos = text.rfind('%')?;
    let backslashes = text[..pos]
        .bytes()
        .rev()
        .take_while(|&c| c == b'\\')
        .count();
    if backslashes % 2 == 1 {
        return None;
    }
    let comment = text[pos + 1..].trim();
    let word = comment
        .strip_prefix("texres-fmt:")
        .or_else(|| comment.strip_prefix("tex-fmt:"))?
        .trim();
    match word {
        "off" => Some(Marker::Off),
        "on" => Some(Marker::On),
        "skip" => Some(Marker::Skip {
            own_line: text[..pos].bytes().all(is_blank),
        }),
        _ => None,
    }
}

struct Built {
    text: String,
    width: usize,
    /// Breakable blank runs: output column of the run and its index in `Scan::runs`.
    breaks: Vec<(usize, usize)>,
    amps: Vec<usize>,
    indent_len: usize,
}

fn utf8_len(lead: u8) -> usize {
    match lead {
        0xF0..=0xFF => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

/// End of `s` without trailing blanks, unless they follow a `\` (a control
/// space or `\^^I`) or a `^` (part of `^^` notation): those stay.
fn trimmed_end(s: &str) -> usize {
    let t = s.trim_end_matches([' ', '\t']);
    if t.len() == s.len() {
        return s.len();
    }
    let backslashes = t.bytes().rev().take_while(|&c| c == b'\\').count();
    if backslashes % 2 == 1 || t.ends_with('^') {
        s.len()
    } else {
        t.len()
    }
}

/// `{name}` after `\begin` or `\end` at `i`: the name and the index after `}`.
pub(crate) fn env_name(line: &str, i: usize) -> Option<(&str, usize)> {
    let b = line.as_bytes();
    let j = skip_blanks(b, i);
    if b.get(j) != Some(&b'{') {
        return None;
    }
    let close = j + 1 + line[j + 1..].find('}')?;
    let name = &line[j + 1..close];
    let valid = !name.is_empty()
        && !name
            .bytes()
            .any(|c| matches!(c, b'{' | b'\\' | b'%' | b' ' | b'\t'));
    valid.then_some((name, close + 1))
}

/// Whether a sectioning command at `i` (after its name) is a real use:
/// an optional `*`, then `[` or `{`.
fn is_section_call(b: &[u8], i: usize) -> bool {
    let mut j = i;
    if b.get(j) == Some(&b'*') {
        j += 1;
    }
    let j = skip_blanks(b, j);
    matches!(b.get(j), Some(b'{' | b'['))
}

/// Whether only blanks or a comment follow position `i`.
fn line_ends_open(b: &[u8], i: usize) -> bool {
    let j = skip_blanks(b, i);
    j == b.len() || b[j] == b'%'
}

/// Characters that end an argument or start a command, never a `\verb` delimiter.
fn is_closing(c: u8) -> bool {
    matches!(c, b'}' | b']' | b')' | b'\\' | b'%' | b',' | b'=' | b';')
}

/// Scans the arguments of a verbatim-like command from `i`. Returns the end
/// of the span, or `None` when a braced argument continues on the next line.
pub(crate) fn scan_span(b: &[u8], mut i: usize, span: &mut Span) -> Option<usize> {
    if matches!(span.spec.delim, Delim::Statement | Delim::Rest) {
        return scan_statement(b, i, span);
    }
    loop {
        if span.depth > 0 {
            while i < b.len() {
                match b[i] {
                    b'\\' => i += 1,
                    b'{' => span.depth += 1,
                    b'}' => {
                        span.depth -= 1;
                        if span.depth == 0 {
                            i += 1;
                            span.done += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            if span.depth > 0 {
                return None;
            }
            if span.done >= span.spec.args {
                return Some(i.min(b.len()));
            }
            continue;
        }
        let mut j = skip_blanks(b, i);
        if span.spec.star && span.done == 0 && b.get(j) == Some(&b'*') {
            j = skip_blanks(b, j + 1);
        }
        if j >= b.len() {
            return Some(i);
        }
        let last = span.done + 1 == span.spec.args;
        match b[j] {
            // `\verb` takes any delimiter; `{\verb}` or `\let\x\verb\relax`
            // mention it without using it.
            c if span.spec.delim == Delim::Only && !matches!(c, b'}' | b'\\') => {
                return Some(memchr(c, &b[j + 1..]).map_or(b.len(), |p| j + p + 2));
            }
            // Before the last argument a control sequence is an argument
            // (`\pgfplotstablesort[...]\result{...}`).
            b'\\' if !last => {
                let mut k = j + 1;
                if b.get(k).is_some_and(|&c| is_letter(c)) {
                    while k < b.len() && is_letter(b[k]) {
                        k += 1;
                    }
                } else {
                    k += 1;
                }
                span.done += 1;
                i = k.min(b.len());
            }
            // For other commands a closing bracket, `\`, `%` or punctuation
            // after the name means it is mentioned, not used: `{\url}`.
            c if is_closing(c) => return Some(i),
            b'[' if span.spec.optional && span.done == 0 => {
                let mut depth = 0usize;
                let mut k = j + 1;
                loop {
                    match b.get(k) {
                        None => return Some(b.len()),
                        Some(b'{') => depth += 1,
                        Some(b'}') => depth = depth.saturating_sub(1),
                        Some(b']') if depth == 0 => break,
                        _ => {}
                    }
                    k += 1;
                }
                i = k + 1;
            }
            b'{' => {
                span.depth = 1;
                i = j + 1;
            }
            c if last && span.spec.delim == Delim::Either && !c.is_ascii_alphanumeric() => {
                return Some(memchr(c, &b[j + 1..]).map_or(b.len(), |p| j + p + 2));
            }
            _ => return Some(i),
        }
    }
}

/// Scans a statement up to its `;` outside braces (`\addplot ... ;`), or
/// for `Delim::Rest` to the end of the line, over several lines while
/// braces are open; a `}` that closes an enclosing group ends either
/// (`{\addplot}` mentions the command).
fn scan_statement(b: &[u8], mut i: usize, span: &mut Span) -> Option<usize> {
    let statement = span.spec.delim == Delim::Statement;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'%' if statement => return None,
            b'{' => span.depth += 1,
            b'}' if span.depth == 0 => {
                span.done = span.spec.args;
                return Some(i);
            }
            b'}' => span.depth -= 1,
            b';' if statement && span.depth == 0 => {
                span.done = span.spec.args;
                return Some(i + 1);
            }
            _ => {}
        }
        i += 1;
    }
    // The rest of the line, unless a group stays open.
    (!statement && span.depth == 0).then_some(b.len())
}

/// Whether `\catcode` at `i` may change the category of a blank or the
/// line end (codes 9, 13 and 32, or anything not written as a constant).
fn catcode_guard(b: &[u8], i: usize) -> bool {
    let j = skip_blanks(b, i);
    let Some(&first) = b.get(j) else { return true };
    let code: Option<u32> = match first {
        b'`' => {
            let k = j + 1;
            match b.get(k) {
                Some(b'\\') => match b.get(k + 1) {
                    Some(b'^') if b.get(k + 2) == Some(&b'^') => caret_code(&b[k + 3..]),
                    Some(&c) => Some(u32::from(c)),
                    None => None,
                },
                Some(b'^') if b.get(k + 1) == Some(&b'^') => caret_code(&b[k + 2..]),
                Some(&c) => Some(u32::from(c)),
                None => None,
            }
        }
        b'0'..=b'9' => {
            let digits: String = b[j..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .map(|&c| c as char)
                .collect();
            digits.parse().ok()
        }
        b'"' => {
            let digits: String = b[j + 1..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .map(|&c| c as char)
                .collect();
            u32::from_str_radix(&digits, 16).ok()
        }
        b'\'' => {
            let digits: String = b[j + 1..]
                .iter()
                .take_while(|c| (b'0'..=b'7').contains(c))
                .map(|&c| c as char)
                .collect();
            u32::from_str_radix(&digits, 8).ok()
        }
        _ => None,
    };
    !matches!(code, Some(c) if c != 9 && c != 13 && c != 32 && c != 10)
}

/// The character denoted by the text after `^^`.
fn caret_code(b: &[u8]) -> Option<u32> {
    let hex = |c: u8| matches!(c, b'0'..=b'9' | b'a'..=b'f');
    match b {
        [x, y, ..] if hex(*x) && hex(*y) => {
            u32::from_str_radix(std::str::from_utf8(&b[..2]).ok()?, 16).ok()
        }
        [c, ..] if *c < 128 => Some(if *c < 64 {
            u32::from(*c) + 64
        } else {
            u32::from(*c) - 64
        }),
        _ => None,
    }
}
