//! Formatting rules and safety cases. Every case is also checked for
//! idempotence: formatting the result again must not change it.

use tex_format::{format_source, Config, Extras, FileKind, SourceKind};

fn fmt_kind(src: &str, config: &Config, extras: &Extras, kind: SourceKind) -> String {
    let once = format_source(src, config, extras, kind)
        .unwrap_or_else(|e| panic!("{e}\n--- input ---\n{src}"));
    let twice = format_source(&once, config, extras, kind).unwrap();
    assert_eq!(
        once, twice,
        "not idempotent\n--- first ---\n{once}\n--- second ---\n{twice}"
    );
    once
}

fn fmt_with(src: &str, config: &Config, extras: &Extras) -> String {
    fmt_kind(src, config, extras, SourceKind::Document)
}

fn fmt(src: &str) -> String {
    fmt_with(src, &Config::default(), &Extras::default())
}

fn config(toml: &str) -> Config {
    Config::from_toml(toml).unwrap()
}

fn extras_from(text: &str) -> Extras {
    let mut extras = Extras::default();
    extras.scan(text);
    extras
}

#[test]
fn indents_environments_but_not_document() {
    let src = "\\begin{document}\n\\begin{center}\nText\n\\begin{tabular}{ll}\na & b\n\\end{tabular}\n\\end{center}\n\\end{document}\n";
    let want = "\\begin{document}\n\\begin{center}\n  Text\n  \\begin{tabular}{ll}\n    a & b\n  \\end{tabular}\n\\end{center}\n\\end{document}\n";
    assert_eq!(fmt(src), want);
}

#[test]
fn indents_list_items_and_their_continuations() {
    let src = "\\begin{itemize}\n\\item one\ncontinued\n\\item two\n\\begin{enumerate}\n\\item inner\n\\end{enumerate}\n\\end{itemize}\n";
    let want = "\\begin{itemize}\n  \\item one\n    continued\n  \\item two\n    \\begin{enumerate}\n      \\item inner\n    \\end{enumerate}\n\\end{itemize}\n";
    assert_eq!(fmt(src), want);
}

#[test]
fn indents_brace_group_continuations() {
    let src =
        "\\newcommand{\\foo}{%\nbar\n\\textbf{baz\nqux}\n}\n\\caption{A long\ncaption} after\n";
    let want = "\\newcommand{\\foo}{%\n  bar\n  \\textbf{baz\n    qux}\n}\n\\caption{A long\n  caption} after\n";
    assert_eq!(fmt(src), want);
}

#[test]
fn several_groups_opened_on_one_line_indent_once() {
    let src = "\\foo{\\bar{%\nx\n}}\n";
    assert_eq!(fmt(src), "\\foo{\\bar{%\n  x\n}}\n");
    let src = "\\begin{figure}\\begin{center}\nx\n\\end{center}\\end{figure}\n";
    assert_eq!(
        fmt(src),
        "\\begin{figure}\\begin{center}\n  x\n\\end{center}\\end{figure}\n"
    );
}

#[test]
fn closing_then_opening_on_one_line() {
    let src = "\\newenvironment{x}{%\na\n}{%\nb\n}\n";
    assert_eq!(fmt(src), "\\newenvironment{x}{%\n  a\n}{%\n  b\n}\n");
}

#[test]
fn optional_argument_lists_are_indented() {
    let src = "\\usepackage[\nmargin=1in,\nletterpaper\n]{geometry}\n";
    assert_eq!(
        fmt(src),
        "\\usepackage[\n  margin=1in,\n  letterpaper\n]{geometry}\n"
    );
}

#[test]
fn math_delimiters_do_not_open_groups() {
    let src = "\\begin{equation}\nx = \\left[\na\n\\right)\n\\end{equation}\n";
    assert_eq!(
        fmt(src),
        "\\begin{equation}\n  x = \\left[\n  a\n  \\right)\n\\end{equation}\n"
    );
}

#[test]
fn display_math_body_is_indented() {
    assert_eq!(fmt("\\[\na + b\n\\]\n"), "\\[\n  a + b\n\\]\n");
}

#[test]
fn splits_items_onto_their_own_lines() {
    let src = "\\begin{itemize} \\item a \\item b\n\\end{itemize}\n";
    assert_eq!(
        fmt(src),
        "\\begin{itemize}\n  \\item a\n  \\item b\n\\end{itemize}\n"
    );
}

#[test]
fn does_not_split_items_without_a_blank_before_them() {
    // Splitting there would add a space token.
    let src = "\\begin{itemize}\\item a\\item b\n\\end{itemize}\n";
    assert_eq!(
        fmt(src),
        "\\begin{itemize}\\item a\\item b\n\\end{itemize}\n"
    );
}

#[test]
fn item_split_can_be_turned_off() {
    let src = "\\begin{itemize}\n\\item a \\item b\n\\end{itemize}\n";
    let out = fmt_with(
        src,
        &config("one-item-per-line = false"),
        &Extras::default(),
    );
    assert_eq!(
        out,
        "\\begin{itemize}\n  \\item a \\item b\n\\end{itemize}\n"
    );
}

#[test]
fn normalizes_blanks_and_line_ends() {
    let src = "a  \t\nb\t\n\n\n\n\tc\td\n\n\n";
    assert_eq!(fmt(src), "a\nb\n\nc   d\n\n");
    assert_eq!(fmt("no newline"), "no newline\n");
    assert_eq!(fmt(""), "");
}

#[test]
fn keeps_one_trailing_blank_line() {
    // At the end of an \input file a blank line is a \par.
    assert_eq!(fmt("text\n\n\n"), "text\n\n");
}

#[test]
fn max_blank_lines_is_configurable() {
    let out = fmt_with(
        "a\n\n\n\n\nb\n",
        &config("max-blank-lines = 2"),
        &Extras::default(),
    );
    assert_eq!(out, "a\n\n\nb\n");
}

#[test]
fn keeps_blanks_that_belong_to_control_symbols() {
    // `\` + tab is \^^I; stripping the tab would make it \^^M.
    assert_eq!(fmt("a\\\t\nb\n"), "a\\\t\nb\n");
    assert_eq!(fmt("a\\ \\\t x\n"), "a\\ \\\t x\n");
}

#[test]
fn indent_width_is_configurable() {
    let out = fmt_with(
        "\\begin{center}\nx\n\\end{center}\n",
        &config("indent-width = 4"),
        &Extras::default(),
    );
    assert_eq!(out, "\\begin{center}\n    x\n\\end{center}\n");
}

#[test]
fn no_indent_envs_are_configurable() {
    let cfg = config("no-indent-envs = [\"document\", \"frame\"]");
    let out = fmt_with(
        "\\begin{frame}\nx\n\\end{frame}\n",
        &cfg,
        &Extras::default(),
    );
    assert_eq!(out, "\\begin{frame}\nx\n\\end{frame}\n");
}

#[test]
fn verbatim_environments_are_untouched() {
    let src = "\\begin{itemize}\n\\item x\n\\begin{verbatim}\n   keep\t  \n\n\n\n  {\n\\end{verbatim}\n\\end{itemize}\n";
    let want = "\\begin{itemize}\n  \\item x\n    \\begin{verbatim}\n   keep\t  \n\n\n\n  {\n\\end{verbatim}\n\\end{itemize}\n";
    assert_eq!(fmt(src), want);
}

#[test]
fn listing_environments_are_untouched() {
    for env in [
        "lstlisting",
        "minted",
        "Verbatim",
        "comment",
        "filecontents*",
        "alltt",
        "luacode*",
        "pycode",
    ] {
        let src = format!("\\begin{{center}}\n\\begin{{{env}}}[x]\n  a   \n\t\tb\n\\end{{{env}}}\n\\end{{center}}\n");
        let want = format!("\\begin{{center}}\n  \\begin{{{env}}}[x]\n  a   \n\t\tb\n\\end{{{env}}}\n\\end{{center}}\n");
        assert_eq!(fmt(&src), want, "{env}");
    }
}

#[test]
fn inline_verbatim_hides_structure() {
    // The `{` inside \verb is not a group and its `%` is not a comment; the
    // rest of such a line is kept as is.
    let src = "\\begin{center}\nx \\verb|{%| y \\lstinline{a{b}}\nz\n\\end{center}\n";
    let want = "\\begin{center}\n  x \\verb|{%| y \\lstinline{a{b}}\n  z\n\\end{center}\n";
    assert_eq!(fmt(src), want);
    let src = "\\begin{itemize}\n\\item \\verb|{| a \\item b\nc\n\\end{itemize}\n";
    let want = "\\begin{itemize}\n  \\item \\verb|{| a\n  \\item b\n    c\n\\end{itemize}\n";
    assert_eq!(fmt(src), want);
}

#[test]
fn multi_line_verbatim_arguments_are_untouched() {
    let src = "\\begin{center}\nsee \\url{http://a.example/\n   b}  \nmore\n\\end{center}\n";
    let want = "\\begin{center}\n  see \\url{http://a.example/\n   b}  \n  more\n\\end{center}\n";
    assert_eq!(fmt(src), want);
    // \index reads its argument with spaces as ordinary characters.
    let src = "\\begin{center}\nx\\index{a\n    b}\n\\end{center}\n";
    assert_eq!(
        fmt(src),
        "\\begin{center}\n  x\\index{a\n    b}\n\\end{center}\n"
    );
}

#[test]
fn comments_keep_their_content() {
    let src = "\\begin{center}\n%   \\begin{itemize}  {   \nx % a\t\tb\n\\end{center}\n";
    let want = "\\begin{center}\n  %   \\begin{itemize}  {\n  x % a\t\tb\n\\end{center}\n";
    assert_eq!(fmt(src), want);
}

#[test]
fn lines_ending_in_percent_are_never_joined() {
    let cfg = config("wrap = true\nline-width = 20");
    let src = "abc%\ndef\n";
    assert_eq!(fmt_with(src, &cfg, &Extras::default()), src);
}

#[test]
fn obeylines_groups_are_kept_verbatim() {
    let src = "\\begin{center}\n{\\obeylines\n  a  \n\n\n      b\n}\nafter\n\\end{center}\n";
    let want = "\\begin{center}\n  {\\obeylines\n  a  \n\n\n      b\n}\n  after\n\\end{center}\n";
    assert_eq!(fmt(src), want);
}

#[test]
fn catcode_changes_of_blanks_guard_the_rest_of_the_group() {
    let src = "{\\catcode`\\ =12\n   x   y\n}\n{\\catcode`\\@=11\n   z\n}\n";
    let want = "{\\catcode`\\ =12\n   x   y\n}\n{\\catcode`\\@=11\n  z\n}\n";
    assert_eq!(fmt(src), want);
}

#[test]
fn commands_defined_with_obeylines_guard_too() {
    let src = "\\newcommand{\\poem}{\\obeylines\\obeyspaces}\n\\begin{document}\n{\\poem\n   a\n}\n\\end{document}\n";
    let extras = extras_from(src);
    assert_eq!(fmt_with(src, &Config::default(), &extras), src);
}

/// Formats `body` as part of a project whose main file has `preamble`.
fn fmt_in(preamble: &str, body: &str) -> String {
    fmt_with(body, &Config::default(), &extras_from(preamble))
}

#[test]
fn blank_line_before_sections() {
    let article = "\\documentclass{article}\n\\usepackage{amsmath,hyperref}\n";
    assert_eq!(
        fmt_in(article, "text\n\\section{A}\nmore\n"),
        "text\n\n\\section{A}\nmore\n"
    );
    assert_eq!(
        fmt_in(article, "text\n\\subsection*{A}\n"),
        "text\n\n\\subsection*{A}\n"
    );
    // Not after a comment, a line ending in %, or a group opener.
    for src in [
        "% banner\n\\section{A}\n",
        "text%\n\\section{A}\n",
        "\\begin{document}\n\\section{A}\n",
        "\\begin{frame}\nx\n\\section{A}\n\\end{frame}\n",
        "\\newcommand{\\x}{\nx\n\\section{A}}\n",
        "\\let\\oldsection\\section\n\\section\\foo\n",
    ] {
        assert_eq!(fmt_in(article, src).matches("\n\n").count(), 0, "{src}");
    }
    let out = fmt_with(
        "text\n\\section{A}\n",
        &config("blank-line-before-sections = false"),
        &extras_from(article),
    );
    assert_eq!(out, "text\n\\section{A}\n");
}

#[test]
fn no_blank_line_before_sections_of_unknown_definition() {
    // Regression (tex-fmt's cv test): the class starts `\subsubsection` with
    // `\linebreak`, which fails once a blank line has ended the paragraph.
    let cls = "\\LoadClass{article}\n\\renewcommand{\\subsubsection}[1]{%\n  \\linebreak\n  #1}\n";
    let src = "\\documentclass{cv}\n\\begin{document}\n{Jul 2024}\n\\subsubsection{Cambridge}\n\
               text\n\\section{B}\n\\end{document}\n";
    let mut extras = extras_from(src);
    assert_eq!(
        extras.sections.wanted(),
        [(FileKind::Class, "cv".to_string())]
    );
    extras.scan(cls);
    extras
        .sections
        .resolve(FileKind::Class, "cv".to_string(), true);
    assert_eq!(
        fmt_with(src, &Config::default(), &extras),
        "\\documentclass{cv}\n\\begin{document}\n{Jul 2024}\n\\subsubsection{Cambridge}\n\
         text\n\n\\section{B}\n\\end{document}\n"
    );
    // Classes and packages that are not known, files that were not read
    // and no class at all: no blank line anywhere.
    for preamble in [
        "",
        "\\documentclass{moderncv}",
        "\\documentclass{article}\\usepackage{titlesec}",
        "\\documentclass{article}\\input{macros}",
        "\\documentclass{article}\\let\\section\\relax",
    ] {
        assert_eq!(
            fmt_in(preamble, "text\n\\section{A}\n"),
            "text\n\\section{A}\n"
        );
    }
    // A project redefinition that itself starts with `\par` is fine.
    let ok = "\\documentclass{article}\\renewcommand\\section{\\par\\bigskip\\textbf}";
    assert_eq!(fmt_in(ok, "text\n\\section{A}\n"), "text\n\n\\section{A}\n");
}

#[test]
fn fragile_frame_end_lines_are_kept() {
    let src = "\\begin{document}\n\\begin{frame}[fragile]\nx\n\\begin{verbatim}\nv\n\\end{verbatim}\n\\end{frame}\n\\end{document}\n";
    let want = "\\begin{document}\n\\begin{frame}[fragile]\n  x\n  \\begin{verbatim}\nv\n\\end{verbatim}\n\\end{frame}\n\\end{document}\n";
    assert_eq!(fmt(src), want);
}

#[test]
fn crlf_line_ends_are_preserved() {
    assert_eq!(
        fmt("\\begin{center}\r\nx  \r\n\\end{center}\r\n"),
        "\\begin{center}\r\n  x\r\n\\end{center}\r\n"
    );
}

#[test]
fn wraps_long_prose_lines_when_enabled() {
    let cfg = config("wrap = true\nline-width = 30");
    let src = "\\begin{document}\nThe quick brown fox jumps over the lazy dog and keeps running far away.\n\\end{document}\n";
    let out = fmt_with(src, &cfg, &Extras::default());
    assert_eq!(
        out,
        "\\begin{document}\nThe quick brown fox jumps over\nthe lazy dog and keeps running\nfar away.\n\\end{document}\n"
    );
    assert!(out.lines().all(|l| l.len() <= 30));
}

#[test]
fn wrapping_skips_math_comments_and_verbatim() {
    let cfg = config("wrap = true\nline-width = 20");
    let src = "\\begin{document}\n$a + b + c + d + e + f + g$\n\\begin{equation}\n  a + b + c + d + e + f + g + h\n\\end{equation}\n% a comment that is quite long indeed\n\\verb|a b c d e f g h i j k l m n|\n\\end{document}\n";
    assert_eq!(fmt_with(src, &cfg, &Extras::default()), src);
    // The blank after the closing `$` is outside math and may break.
    let out = fmt_with(
        "\\begin{document}\n$a + b + c + d + e + f$ x\n\\end{document}\n",
        &cfg,
        &Extras::default(),
    );
    assert_eq!(
        out,
        "\\begin{document}\n$a + b + c + d + e + f$\nx\n\\end{document}\n"
    );
}

#[test]
fn wrapping_indents_continuations_inside_groups() {
    let cfg = config("wrap = true\nline-width = 24");
    let src = "\\begin{document}\nText\\footnote{one two three four five six}.\n\\end{document}\n";
    let out = fmt_with(src, &cfg, &Extras::default());
    assert_eq!(
        out,
        "\\begin{document}\nText\\footnote{one two\n  three four five six}.\n\\end{document}\n"
    );
}

#[test]
fn wrapping_never_happens_in_the_preamble() {
    let cfg = config("wrap = true\nline-width = 20");
    let src = "\\pgfplotstableread{a b c d e f g h i j k l m}\\data\n\\begin{document}\n\\end{document}\n";
    assert_eq!(fmt_with(src, &cfg, &Extras::default()), src);
}

#[test]
fn aligns_columns_when_enabled() {
    let cfg = config("align-columns = true");
    let src = "\\begin{tabular}{lll}\na&bb&c\\\\\n\\hline\nlonger & x & \\\\ % note\n\\multicolumn{2}{c}{m} & z \\\\\n\\end{tabular}\n";
    let want = "\\begin{tabular}{lll}\n  a      & bb & c\\\\\n  \\hline\n  longer & x  & \\\\ % note\n  \\multicolumn{2}{c}{m} & z \\\\\n\\end{tabular}\n";
    assert_eq!(fmt_with(src, &cfg, &Extras::default()), want);
    let src = "\\begin{align}\nx &= 1 \\\\\nyyy &= 22\n\\end{align}\n";
    let want = "\\begin{align}\n  x   & = 1 \\\\\n  yyy & = 22\n\\end{align}\n";
    assert_eq!(fmt_with(src, &cfg, &Extras::default()), want);
}

#[test]
fn alignment_never_adds_a_blank_after_glue_in_text_tables() {
    // Regression (arXiv revtex paper): `Energy~&` padded to `Energy~ &` kept
    // the `~` glue that \unskip used to remove, widening the column.
    let cfg = config("align-columns = true");
    let src = "\\begin{tabular}{lll}\nEnergy~&~x & y\\\\\n30 GeV & 1.114 & 7\\\\\n\\end{tabular}\n";
    let want =
        "\\begin{tabular}{lll}\n  Energy~&~x & y\\\\\n  30 GeV & 1.114 & 7\\\\\n\\end{tabular}\n";
    assert_eq!(fmt_with(src, &cfg, &Extras::default()), want);
    // In math alignments blanks never matter.
    let src = "\\begin{align}\nx^{2}&=1\\\\\ny~&=22\n\\end{align}\n";
    let want = "\\begin{align}\n  x^{2} & =1\\\\\n  y~    & =22\n\\end{align}\n";
    assert_eq!(fmt_with(src, &cfg, &Extras::default()), want);
}

#[test]
fn alignment_is_off_by_default() {
    let src = "\\begin{tabular}{ll}\na&bb\\\\\nlonger & x\n\\end{tabular}\n";
    assert_eq!(
        fmt(src),
        "\\begin{tabular}{ll}\n  a&bb\\\\\n  longer & x\n\\end{tabular}\n"
    );
}

#[test]
fn project_definitions_extend_the_verbatim_lists() {
    let defs = "\\lstnewenvironment{code}{}{}\n\\newenvironment{myverb}{\\verbatim}{\\endverbatim}\n\\newminted{python}{}\n\\newcommand{\\cmd}{\\lstinline}\n\\MakeShortVerb{\\|}\n";
    let extras = extras_from(defs);
    for env in ["code", "myverb", "pythoncode"] {
        assert!(extras.verbatim_envs.contains(env), "{env}");
    }
    assert!(extras.verbatim_commands.contains_key("cmd"));
    assert_eq!(extras.short_verb, b"|");
    let src =
        "\\begin{center}\n\\begin{myverb}\n  a\n\\end{myverb}\nx |{| \\cmd{b  {}\n\\end{center}\n";
    let want = "\\begin{center}\n  \\begin{myverb}\n  a\n\\end{myverb}\n  x |{| \\cmd{b  {}\n\\end{center}\n";
    assert_eq!(fmt_with(src, &Config::default(), &extras), want);
}

#[test]
fn configured_verbatim_names_are_respected() {
    let cfg = config("verbatim-envs = [\"shell\"]\nverbatim-commands = [\"\\\\shellcmd\"]");
    let src = "\\begin{center}\n\\begin{shell}\n  $ ls\n\\end{shell}\n\\shellcmd{a\n   b}\n\\end{center}\n";
    let want = "\\begin{center}\n  \\begin{shell}\n  $ ls\n\\end{shell}\n  \\shellcmd{a\n   b}\n\\end{center}\n";
    assert_eq!(fmt_with(src, &cfg, &Extras::default()), want);
}

#[test]
fn unbalanced_input_does_not_panic() {
    for src in [
        "}}}\n]\n\\end{x}\n",
        "{{{\n\\begin{a}\n",
        "\\verb",
        "\\begin{verbatim}\nnever ends\n",
        "\\url{\n",
        "\\",
        "$$\n$\n",
    ] {
        fmt(src);
    }
}

#[test]
fn latin1_text_passes_through() {
    let src: String = [b'\\', b'b', b'f', b'{', 0xE9, b'\n', b'x', b'}', b'\n']
        .iter()
        .map(|&b| char::from(b))
        .collect();
    assert_eq!(fmt(&src), "\\bf{\u{e9}\n  x}\n");
}

#[test]
fn mentioning_a_verbatim_command_does_not_start_verbatim() {
    // Regression (lkmpg): `{\sh}` in a definition of the verbatim command \sh
    // used `}` as a \verb delimiter and unbalanced the groups.
    let src = "\\NewDocumentCommand{\\sh}{v}{\\texttt{#1}}\n\\newmintinline[sh]{bash}{}\n\\let\\x\\verb\nnext\n";
    let extras = extras_from(src);
    assert!(extras.verbatim_commands.contains_key("sh"));
    assert_eq!(fmt_with(src, &Config::default(), &extras), src);
}

#[test]
fn blank_lines_after_a_command_are_kept() {
    // Regression (arXiv 1903.00050): `\hfill\fbox` followed by three blank
    // lines takes the first \par as \fbox's argument; collapsing the blank
    // lines removed the paragraph break that the others made.
    let src = "set.\\hfill\\fbox\n\n\n\nLet $X$ be\n";
    assert_eq!(fmt(src), src);
    let article = "\\documentclass{article}";
    let src = "a \\\\\n\n\n\\section{B}\n";
    assert_eq!(fmt_in(article, src), src);
    // No blank line is put between a command and a \section either.
    let src = "\\fbox\n\\section{B}\n";
    assert_eq!(fmt_in(article, src), src);
    // After text, `}` or `$` the extra blank lines go.
    assert_eq!(fmt("text.\n\n\n\nx\n"), "text.\n\nx\n");
    assert_eq!(fmt("\\label{a}\n\n\n\nx\n"), "\\label{a}\n\nx\n");
    assert_eq!(
        fmt("\\fbox % c\n% only a comment\n\n\n\nx\n"),
        "\\fbox % c\n% only a comment\n\n\n\nx\n"
    );
    // Leading blank lines of a file follow whatever came before \input.
    assert_eq!(fmt("\n\n\nx\n"), "\n\n\nx\n");
}

#[test]
fn xparse_verbatim_body_environments_are_untouched() {
    // Regression (siunitx manual): LaTeXdemo prints its body verbatim.
    let defs =
        "\\NewDocumentEnvironment { LaTeXdemo } { O { code~and~example } c }\n  { }\n  { }\n";
    let extras = extras_from(defs);
    assert!(extras.verbatim_envs.contains("LaTeXdemo"));
    let src =
        "\\begin{center}\n\\begin{LaTeXdemo}\n\\num{1} \\\\\n\\end{LaTeXdemo}\n\\end{center}\n";
    let want =
        "\\begin{center}\n  \\begin{LaTeXdemo}\n\\num{1} \\\\\n\\end{LaTeXdemo}\n\\end{center}\n";
    assert_eq!(fmt_with(src, &Config::default(), &extras), want);
}

#[test]
fn at_delimited_arguments_of_verbatim_aliases_are_untouched() {
    // Regression (arXiv 1902.10231): with `\newcommand{\Q}{\lstinline}`,
    // `\Q@{a = b; c}@` was lexed as the control word `\Q@` (as if `@` were a
    // letter), so wrapping broke the line inside the verbatim argument.
    let extras = extras_from("\\newcommand{\\Q}{\\lstinline}\n");
    let cfg = config("wrap = true\nline-width = 40");
    let src = "\\begin{document}\nCall it at the end: \\Q@{this.h = h; this.path = path; base();}@. Then \\Q@[Pure] bool Equal(Point that)@ is fine.\n\\end{document}\n";
    let out = fmt_with(src, &cfg, &extras);
    assert!(out.contains("\\Q@{this.h = h; this.path = path; base();}@."));
    assert!(out.contains("\\Q@[Pure] bool Equal(Point that)@"));
    assert!(out
        .lines()
        .filter(|l| !l.contains("\\Q@"))
        .all(|l| l.len() <= 40));
    // Where `@` is a letter (packages, after \makeatletter) `\Q@x` is another
    // control word and the line is formatted as usual.
    let src = "\\def\\Q@x#1{%\nfoo bar baz qux quux corge grault garply waldo\n}\n";
    let want = "\\def\\Q@x#1{%\n  foo bar baz qux quux corge grault\n  garply waldo\n}\n";
    assert_eq!(fmt_kind(src, &cfg, &extras, SourceKind::Package), want);
    let doc = |body: &str| {
        format!("\\begin{{document}}\n\\makeatletter\n{body}\\makeatother\n\\end{{document}}\n")
    };
    assert_eq!(fmt_with(&doc(src), &cfg, &extras), doc(want));
}

#[test]
fn off_on_and_skip_comments_keep_lines() {
    for prefix in ["texres-fmt", "tex-fmt"] {
        let src = format!(
            "\\begin{{itemize}}\n\\item a\n% {prefix}: off\n\\item   b  \n   \\begin{{center}}\n\n\n\nx\n% {prefix}: on\n\\end{{center}}\n\\item c\n\\end{{itemize}}\n"
        );
        // The region is copied as it is; indentation after it follows the
        // environments opened inside it.
        let want = format!(
            "\\begin{{itemize}}\n  \\item a\n% {prefix}: off\n\\item   b  \n   \\begin{{center}}\n\n\n\nx\n% {prefix}: on\n    \\end{{center}}\n  \\item c\n\\end{{itemize}}\n"
        );
        assert_eq!(fmt(&src), want, "{prefix}");
        // `skip` at the end of a line keeps that line; on a line of its own
        // it keeps the next line too.
        let src = format!(
            "\\begin{{center}}\nkeep   this  % {prefix}: skip\nindent this\n  % {prefix}: skip\nkeep    this\nindent this\n\\end{{center}}\n"
        );
        let want = format!(
            "\\begin{{center}}\nkeep   this  % {prefix}: skip\n  indent this\n  % {prefix}: skip\nkeep    this\n  indent this\n\\end{{center}}\n"
        );
        assert_eq!(fmt(&src), want, "{prefix}");
    }
    // An unterminated `off` runs to the end of the file; an escaped `\%` or a
    // directive inside verbatim text is no directive.
    let src = "% texres-fmt: off\n\\begin{center}\nx\n\\end{center}\n";
    assert_eq!(fmt(src), src);
    let src = "\\begin{center}\nx \\% texres-fmt: off\n\\end{center}\n";
    assert_eq!(
        fmt(src),
        "\\begin{center}\n  x \\% texres-fmt: off\n\\end{center}\n"
    );
    let src = "\\begin{verbatim}\n% texres-fmt: off\n\\end{verbatim}\n\\begin{center}\nx\n\\end{center}\n";
    assert_eq!(
        fmt(src),
        "\\begin{verbatim}\n% texres-fmt: off\n\\end{verbatim}\n\\begin{center}\n  x\n\\end{center}\n"
    );
}
