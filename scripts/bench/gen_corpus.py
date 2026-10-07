#!/usr/bin/env python3
"""Generate the public TeXres-vs-TeX-Live benchmark corpus.

Deterministic: the same script always writes byte-identical files. The output
(scripts/bench/corpus/<name>/) is committed; rerun this script only to change
the corpus. All text is synthetic (random words from a fixed vocabulary, fake
author names), so there is nothing to license and nothing to fetch.

Every document contains the marker `BENCH-A` exactly once, in the middle of
the body. The one-line-edit scenario of scripts/bench_tl.py replaces it with
`BENCH-B`.
"""

import random
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent / "corpus"

WORDS = """
analysis approach assumption balance behavior benefit capital channel choice
cluster coefficient comparison condition constraint context control correlation
cost curve data decision delay demand design distribution dynamic effect
efficiency equilibrium error estimate evidence example experiment factor
feature finding flow forecast framework function gain growth hypothesis impact
income index information input interaction interval investment judgment
knowledge labor latency layer limit loss margin market measure method model
network noise observation outcome parameter pattern performance policy
population portfolio prediction pressure price probability process production
profit quality range rate regime regression response result return risk sample
scale schedule sector selection shock signal size solution source spread
stability state strategy structure supply system technique test threshold
trade trend uncertainty utility value variable variance volume weight yield
""".split()
VERBS = """
affects bounds changes conditions determines drives explains improves limits
predicts reduces reflects relates shapes stabilizes supports tracks varies
""".split()
ADJ = """
active average broad central clear common complex constant direct dynamic early
efficient empirical final general global high implicit joint large local low
marginal modest narrow observed optimal partial persistent possible primary
random robust sharp simple stable strong structural sufficient typical
""".split()
FIRST = "Anna Boris Clara David Elena Felix Greta Hugo Irene Jonas Karin Lukas Maria Nils Olga Pavel Rosa Stefan Tanya Viktor".split()
LAST = "Adler Berg Chen Dumont Eriksen Fischer Garcia Hansen Ito Jensen Keller Lopez Meyer Novak Olsen Petrov Quinn Rossi Silva Weber".split()


class Gen:
    def __init__(self, seed):
        self.r = random.Random(seed)

    def phrase(self):
        r = self.r
        return f"{r.choice(ADJ)} {r.choice(WORDS)}"

    def sentence(self):
        r = self.r
        kind = r.randrange(4)
        if kind == 0:
            s = f"The {self.phrase()} {r.choice(VERBS)} the {self.phrase()} of the {r.choice(WORDS)}"
        elif kind == 1:
            s = f"A {self.phrase()} {r.choice(VERBS)} each {r.choice(WORDS)} in the {self.phrase()}"
        elif kind == 2:
            s = f"In the {self.phrase()}, the {r.choice(WORDS)} {r.choice(VERBS)} a {self.phrase()} and the {r.choice(WORDS)}"
        else:
            s = f"We find that the {self.phrase()} {r.choice(VERBS)} the {r.choice(WORDS)}, while the {self.phrase()} {r.choice(VERBS)} the {self.phrase()}"
        return s[0].upper() + s[1:] + "."

    def para(self, n=None, cites=None, refs=None):
        r = self.r
        n = n or r.randint(4, 8)
        out = []
        for _ in range(n):
            s = self.sentence()
            if cites and r.random() < 0.35:
                s = s[:-1] + " " + r.choice(cites)() + "."
            if refs and r.random() < 0.12:
                s = s[:-1] + " (see " + r.choice(refs)() + ")."
            out.append(s)
        return " ".join(out)


def caption(g, cap):
    """A long figure or table caption: a full sentence, so that its list entry
    wraps. The text comes from the separate stream `cap`; the phrase that the
    body stream `g` used to supply here is still drawn (and dropped), so the
    body text of a document is unchanged by the caption length."""
    g.phrase()
    return cap.sentence()


def write(name, fname, text):
    d = OUT / name
    d.mkdir(parents=True, exist_ok=True)
    (d / fname).write_text(text, encoding="utf-8")


def bib(seed, n):
    g = Gen(seed)
    r = g.r
    keys, out = [], []
    for i in range(n):
        key = f"ref{i + 1:03d}"
        keys.append(key)
        au = " and ".join(
            f"{r.choice(LAST)}, {r.choice(FIRST)}" for _ in range(r.randint(1, 3))
        )
        title = " ".join(w.capitalize() for w in (r.choice(ADJ), r.choice(WORDS), "and", r.choice(WORDS)))
        year = r.randint(1985, 2025)
        kind = r.choice(["article", "article", "book", "inproceedings"])
        if kind == "article":
            out.append(
                f"@article{{{key},\n  author = {{{au}}},\n  title = {{{title}}},\n"
                f"  journal = {{Journal of {g.phrase().title()}}},\n  year = {{{year}}},\n"
                f"  volume = {{{r.randint(1, 80)}}},\n  number = {{{r.randint(1, 12)}}},\n"
                f"  pages = {{{(p := r.randint(1, 900))}--{p + r.randint(8, 40)}}},\n}}\n"
            )
        elif kind == "book":
            out.append(
                f"@book{{{key},\n  author = {{{au}}},\n  title = {{{title}}},\n"
                f"  publisher = {{{r.choice(LAST)} Press}},\n  address = {{{r.choice(LAST)}ville}},\n"
                f"  year = {{{year}}},\n}}\n"
            )
        else:
            out.append(
                f"@inproceedings{{{key},\n  author = {{{au}}},\n  title = {{{title}}},\n"
                f"  booktitle = {{Proceedings of the {r.choice(LAST)} Conference on {g.phrase().title()}}},\n"
                f"  year = {{{year}}},\n  pages = {{{(p := r.randint(1, 500))}--{p + r.randint(6, 20)}}},\n}}\n"
            )
    return keys, "\n".join(out)


def math_block(g, idx, labels):
    """One displayed-math snippet, labelled; returns (tex, label)."""
    r = g.r
    lab = f"eq:{idx}"
    k = r.randrange(6)
    a, b, c = r.choice("abcdefgh"), r.choice("pqrstu"), r.randint(2, 9)
    if k == 0:
        t = rf"\begin{{equation}}\label{{{lab}}} \sum_{{i=1}}^{{n}} {a}_i {b}^{{{c}}} = \int_0^1 f_{{{c}}}(x)\,\mathrm{{d}}x + \varepsilon_n \end{{equation}}"
    elif k == 1:
        t = rf"\begin{{align}} x_{{t+1}} &= {a} x_t + {b}\,\epsilon_t, \label{{{lab}}}\\ \operatorname{{Var}}(x_t) &= \frac{{\sigma^2}}{{1-{a}^2}} \notag \end{{align}}"
    elif k == 2:
        t = rf"\begin{{equation}}\label{{{lab}}} \mathbf{{A}} = \begin{{pmatrix}} {a}_{{11}} & {a}_{{12}} \\ {a}_{{21}} & {a}_{{22}} \end{{pmatrix}},\qquad \det \mathbf{{A}} = {a}_{{11}}{a}_{{22}} - {a}_{{12}}{a}_{{21}} \end{{equation}}"
    elif k == 3:
        t = rf"\begin{{equation}}\label{{{lab}}} \lim_{{n\to\infty}} \Bigl(1 + \frac{{{c}}}{{n}}\Bigr)^{{n}} = e^{{{c}}} \end{{equation}}"
    elif k == 4:
        t = rf"\begin{{align}} \mathbb{{E}}[{a}_t \mid \mathcal{{F}}_{{t-1}}] &= \mu + \beta {b}_{{t-1}}, \label{{{lab}}}\\ \hat\beta &= \Bigl(\sum_t {b}_t^{{2}}\Bigr)^{{-1}} \sum_t {b}_t {a}_t \nonumber \end{{align}}"
    else:
        t = rf"\begin{{equation}}\label{{{lab}}} \nabla_{{\theta}} \mathcal{{L}}(\theta) = -\frac{{1}}{{N}} \sum_{{j=1}}^{{N}} \bigl({a}_j - \theta^{{\top}} {b}_j\bigr) {b}_j,\qquad \theta \in \mathbb{{R}}^{{{c}}} \end{{equation}}"
    labels.append(lab)
    return t, lab


# ---------------------------------------------------------------- documents


def doc_article_math():
    g = Gen(101)
    labels = []
    body = []
    nsec = 14
    for s in range(1, nsec + 1):
        sec = f"sec:{s}"
        body.append(rf"\section{{{g.phrase().title()}}}\label{{{sec}}}")
        body.append(g.para(5, refs=[lambda: rf"Section~\ref{{sec:{g.r.randint(1, nsec)}}}"]))
        for j in range(3):
            m, lab = math_block(g, f"{s}.{j}", labels)
            body.append(m)
            prev = labels[g.r.randrange(len(labels))]
            body.append(
                g.para(3, refs=[lambda: rf"Eq.~\eqref{{{labels[g.r.randrange(len(labels))]}}}"])
                + rf" Compare Eq.~\eqref{{{prev}}} with Eq.~\eqref{{{lab}}}."
            )
            if j == 0:
                body.append(rf"\begin{{definition}}\label{{def:{s}}} {g.para(2)} \end{{definition}}")
            if j == 1:
                body.append(
                    rf"\begin{{theorem}}\label{{thm:{s}}} {g.para(2)} By Definition~\ref{{def:{s}}}, the bound in Eq.~\eqref{{{lab}}} holds. \end{{theorem}}"
                )
                body.append(rf"\begin{{proof}} {g.para(3)} This follows from Theorem~\ref{{thm:{s}}}. \end{{proof}}")
        if s == nsec // 2:
            body.append(r"The benchmark edit marker reads BENCH-A.")
        body.append(g.para(6))
    tex = (
        r"""\documentclass[11pt]{article}
\usepackage[margin=1in]{geometry}
\usepackage{amsmath,amssymb,amsthm}
\usepackage{hyperref}
\newtheorem{theorem}{Theorem}[section]
\newtheorem{lemma}[theorem]{Lemma}
\theoremstyle{definition}
\newtheorem{definition}[theorem]{Definition}
\title{Notes on Dynamic Estimation with Cross-References}
\author{A. Author}
\date{1 January 2026}
\begin{document}
\maketitle
\tableofcontents
"""
        + "\n\n".join(body)
        + "\n\\end{document}\n"
    )
    write("article_math", "main.tex", tex)


def cite_makers(keys, g, style):
    def one(cmd):
        return lambda: rf"\{cmd}{{{g.r.choice(keys)}}}"

    def multi(cmd):
        return lambda: rf"\{cmd}{{{','.join(g.r.sample(keys, 3))}}}"

    return [one(c) for c in style] + [multi(style[0])]


def sections_with_cites(g, nsec, makers, marker_at):
    body = []
    for s in range(1, nsec + 1):
        body.append(rf"\section{{{g.phrase().title()}}}\label{{sec:{s}}}")
        refs = [lambda: rf"Section~\ref{{sec:{g.r.randint(1, nsec)}}}"]
        for _ in range(3):
            body.append(g.para(6, cites=makers, refs=refs))
        body.append(rf"\subsection{{{g.phrase().title()}}}")
        body.append(g.para(6, cites=makers, refs=refs))
        if s == marker_at:
            body.append(r"The benchmark edit marker reads BENCH-A.")
        body.append(g.para(5, cites=makers, refs=refs))
    return body


def doc_article_biblatex():
    g = Gen(202)
    keys, b = bib(2020, 80)
    write("article_biblatex", "refs.bib", b)
    body = sections_with_cites(g, 10, cite_makers(keys, g, ["parencite", "textcite", "cite"]), 5)
    tex = (
        r"""\documentclass[11pt]{article}
\usepackage[margin=1in]{geometry}
\usepackage[backend=biber,style=authoryear]{biblatex}
\addbibresource{refs.bib}
\usepackage{hyperref}
\title{A Survey with Author--Year Citations (biblatex)}
\author{A. Author}
\date{1 January 2026}
\begin{document}
\maketitle
\tableofcontents
"""
        + "\n\n".join(body)
        + "\n\n\\printbibliography\n\\end{document}\n"
    )
    write("article_biblatex", "main.tex", tex)


def doc_article_natbib():
    g = Gen(303)
    keys, b = bib(3030, 80)
    write("article_natbib", "refs.bib", b)
    body = sections_with_cites(g, 10, cite_makers(keys, g, ["citep", "citet"]), 5)
    tex = (
        r"""\documentclass[11pt]{article}
\usepackage[margin=1in]{geometry}
\usepackage[round,authoryear]{natbib}
\usepackage{hyperref}
\title{A Survey with Author--Year Citations (natbib and BibTeX)}
\author{A. Author}
\date{1 January 2026}
\begin{document}
\maketitle
\tableofcontents
"""
        + "\n\n".join(body)
        + "\n\n\\bibliographystyle{plainnat}\n\\bibliography{refs}\n\\end{document}\n"
    )
    write("article_natbib", "main.tex", tex)


def doc_beamer():
    g = Gen(404)
    fr = []
    nsec = 6
    for s in range(1, nsec + 1):
        fr.append(rf"\section{{{g.phrase().title()}}}")
        fr.append(
            rf"\begin{{frame}}{{Outline: part {s}}}\tableofcontents[currentsection]\end{{frame}}"
        )
        for f in range(1, 7):
            title = g.phrase().title()
            kind = (s + f) % 4
            if kind == 0:
                items = "\n".join(rf"\item<{i}-> {g.sentence()}" for i in range(1, 5))
                body = rf"\begin{{itemize}}{chr(10)}{items}{chr(10)}\end{{itemize}}"
            elif kind == 1:
                body = (
                    rf"\begin{{columns}}\column{{.5\textwidth}} {g.para(2)} \column{{.5\textwidth}}"
                    rf"\begin{{block}}{{{g.phrase().title()}}} {g.sentence()} \end{{block}}\end{{columns}}"
                )
            elif kind == 2:
                m, _ = math_block(g, f"{s}.{f}", [])
                body = g.sentence() + "\n" + m.replace(r"\label{eq:" + f"{s}.{f}" + "}", "")
                body = body.replace(r"\begin{align}", r"\begin{align*}").replace(r"\end{align}", r"\end{align*}")
                body = body.replace(r"\begin{equation}", r"\begin{equation*}").replace(r"\end{equation}", r"\end{equation*}")
            else:
                rows = "\n".join(
                    rf"{g.r.choice(WORDS)} & {g.r.randint(1, 99)} & {g.r.random():.3f} \\" for _ in range(5)
                )
                body = (
                    rf"\begin{{tabular}}{{lrr}}\hline Item & Count & Share \\ \hline{chr(10)}{rows}{chr(10)}\hline\end{{tabular}}"
                    + "\n\\pause\n"
                    + g.sentence()
                )
            if s == 3 and f == 3:
                body += "\n\n" + r"The benchmark edit marker reads BENCH-A."
            fr.append(rf"\begin{{frame}}{{{title}}}{chr(10)}{body}{chr(10)}\end{{frame}}")
    tex = (
        r"""\documentclass[aspectratio=169]{beamer}
\usetheme{Madrid}
\usecolortheme{beaver}
\usepackage{amsmath,amssymb}
\title{A Beamer Deck with Overlays}
\author{A. Author}
\institute{Benchmark Institute}
\date{1 January 2026}
\begin{document}
\begin{frame}\titlepage\end{frame}
\begin{frame}{Contents}\tableofcontents\end{frame}
"""
        + "\n\n".join(fr)
        + "\n\\begin{frame}{Summary}\\begin{itemize}\\item One\\item Two\\item Three\\end{itemize}\\end{frame}\n\\end{document}\n"
    )
    write("beamer_deck", "main.tex", tex)


def doc_tikz():
    g = Gen(505)
    r = g.r
    cap = Gen(5051)
    figs = []
    for i in range(1, 9):
        # line/scatter plot from an inline table
        pts = "\n".join(
            f"{x} {round(3 * (x / 10) ** 0.5 + r.uniform(-0.3, 0.3) + i * 0.2, 3)}" for x in range(1, 61)
        )
        figs.append(
            rf"""\begin{{figure}}[ht]\centering
\begin{{tikzpicture}}
\begin{{axis}}[width=0.85\textwidth,height=6.5cm,xlabel={{$x$}},ylabel={{$y_{{{i}}}$}},grid=major,legend pos=north west]
\addplot[only marks,mark size=1.2pt,blue] table {{
{pts}
}};
\addplot[red,thick,domain=1:60,samples=80] {{3*sqrt(x/10)+{i * 0.2}}};
\addplot[green!50!black,dashed,domain=1:60,samples=80] {{{i}+sin(deg(x/6))}};
\legend{{data,fit,seasonal}}
\end{{axis}}
\end{{tikzpicture}}
\caption{{{caption(g, cap)}}}\label{{fig:plot{i}}}
\end{{figure}}"""
        )
    for i in range(1, 4):
        figs.append(
            rf"""\begin{{figure}}[ht]\centering
\begin{{tikzpicture}}
\begin{{axis}}[width=0.8\textwidth,view={{{30 + 10 * i}}}{{35}},xlabel=$x$,ylabel=$y$,zlabel=$z$]
\addplot3[surf,domain=-2:2,domain y=-2:2,samples=24] {{exp(-(x^2+y^2))*cos(deg({i}*x))}};
\end{{axis}}
\end{{tikzpicture}}
\caption{{Surface {i}. {caption(g, cap)}}}\label{{fig:surf{i}}}
\end{{figure}}"""
        )
    for i in range(1, 7):
        figs.append(
            rf"""\begin{{figure}}[ht]\centering
\begin{{tikzpicture}}[node distance=1.4cm and 1.8cm,>={{Stealth}},every node/.style={{font=\small}}]
\node[draw,rounded corners,fill=blue!10] (a) {{Input}};
\node[draw,rounded corners,fill=green!10,right=of a] (b) {{Stage {i}}};
\node[draw,diamond,aspect=2,fill=yellow!20,right=of b] (c) {{Check?}};
\node[draw,rounded corners,fill=red!10,right=of c] (d) {{Output}};
\node[draw,rounded corners,below=of c,fill=gray!15] (e) {{Retry}};
\draw[->] (a) -- (b); \draw[->] (b) -- (c); \draw[->] (c) -- node[above] {{yes}} (d);
\draw[->] (c) -- node[right] {{no}} (e); \draw[->] (e) -| (b);
\foreach \k in {{1,...,{6 + i}}} {{ \draw[fill=orange!{20 + 8 * i}] ({{\k*0.9}},-3.2) circle ({{0.15 + 0.03*\k}}); }}
\foreach \a in {{0,15,...,345}} {{ \draw[blue!60,thin] (9.6,-2.2) -- +(\a:{0.6 + 0.1 * i}); }}
\end{{tikzpicture}}
\caption{{{caption(g, cap)}}}\label{{fig:dia{i}}}
\end{{figure}}"""
        )
    names = [f"fig:plot{i}" for i in range(1, 9)] + [f"fig:surf{i}" for i in range(1, 4)] + [f"fig:dia{i}" for i in range(1, 7)]
    parts = []
    for i, f in enumerate(figs):
        if i % 3 == 0:
            parts.append(rf"\section{{{g.phrase().title()}}}")
        parts.append(g.para(3, refs=[lambda: rf"Figure~\ref{{{r.choice(names)}}}"]))
        if i == 9:
            parts.append(r"The benchmark edit marker reads BENCH-A.")
        parts.append(f)
    tex = (
        r"""\documentclass[11pt]{article}
\usepackage[margin=1in]{geometry}
\usepackage{tikz,pgfplots}
\pgfplotsset{compat=1.18}
\usetikzlibrary{arrows.meta,positioning,shapes.geometric,calc}
\title{Figures with TikZ and pgfplots}
\author{A. Author}
\date{1 January 2026}
\begin{document}
\maketitle
\listoffigures
"""
        + "\n\n".join(parts)
        + "\n\\end{document}\n"
    )
    write("tikz_pgfplots", "main.tex", tex)


UNI = [
    "Zürich, São Paulo, Kraków and Reykjavík host the survey sites; naïve façade résumé coöperate.",
    "Greek letters α, β, γ, δ and Cyrillic words (рынок, цена, риск) appear in running text.",
    "Ligatures: office, affluent, fluffy; dashes: en – em — and “curly quotes” ‘single’.",
]


def font_doc(name, engine, seed, extra_pre, extra_body, nsec):
    g = Gen(seed)
    body = []
    for s in range(1, nsec + 1):
        body.append(rf"\section{{{g.phrase().title()}}}\label{{sec:{s}}}")
        body.append(g.para(5, refs=[lambda: rf"Section~\ref{{sec:{g.r.randint(1, nsec)}}}"]))
        body.append(UNI[s % 3])
        m, lab = math_block(g, str(s), [])
        body.append(m)
        body.append(
            rf"{{\sffamily {g.para(3)}}} {{\ttfamily {g.sentence()}}} \textit{{{g.sentence()}}} \textbf{{{g.sentence()}}}"
        )
        body.append(g.para(6))
        if s == nsec // 2:
            body.append(r"The benchmark edit marker reads BENCH-A.")
        if extra_body and s % 3 == 0:
            body.append(extra_body(s))
    tex = (
        r"""\documentclass[11pt]{article}
\usepackage[margin=1in]{geometry}
\usepackage{amsmath}
\usepackage{unicode-math}
\usepackage{fontspec}
\setmainfont{texgyretermes}[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]
\setsansfont{texgyreheros}[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]
\setmonofont{texgyrecursor}[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]
\setmathfont{texgyretermes-math.otf}
"""
        + extra_pre
        + rf"""\title{{Unicode Typesetting with fontspec ({engine})}}
\author{{A. Author}}
\date{{1 January 2026}}
\begin{{document}}
\maketitle
\tableofcontents
"""
        + "\n\n".join(body)
        + "\n\\end{document}\n"
    )
    write(name, "main.tex", tex)


def doc_xelatex():
    font_doc("xelatex_fontspec", "XeLaTeX", 606, "", None, 14)


def doc_lualatex():
    lua = """-- Helper functions used by main.tex through \\directlua.
function fib(n)
  local a, b = 0, 1
  for _ = 1, n do a, b = b, a + b end
  return a
end
function primes(n)
  local out = {}
  for k = 2, n do
    local ok = true
    for d = 2, math.floor(math.sqrt(k)) do
      if math.fmod(k, d) == 0 then ok = false break end
    end
    if ok then table.insert(out, k) end
  end
  return table.concat(out, ", ")
end
"""
    write("lualatex_fontspec", "bench.lua", lua)
    pre = "\\directlua{dofile(\"bench.lua\")}\n"

    def extra(s):
        return (
            r"Fibonacci numbers from Lua: \directlua{ for i = 1, %d do tex.sprint(fib(i) .. ', ') end tex.sprint(fib(%d)) }. "
            % (10 + s, 11 + s)
            + r"Primes up to %d: \directlua{tex.sprint(primes(%d))}." % (60 + 10 * s, 60 + 10 * s)
        )

    font_doc("lualatex_fontspec", "LuaLaTeX", 707, pre, extra, 12)


def doc_long():
    g = Gen(808)
    cap = Gen(8081)
    keys, b = bib(8080, 120)
    write("long_thesis", "refs.bib", b)
    makers = cite_makers(keys, g, ["citep", "citet"])
    nch, nsec = 12, 7
    allfig, alltab, alleq = [], [], []
    chapters = []
    for c in range(1, nch + 1):
        out = [rf"\chapter{{{g.phrase().title()}}}\label{{ch:{c}}}"]
        out.append(g.para(6, cites=makers))
        for s in range(1, nsec + 1):
            out.append(rf"\section{{{g.phrase().title()}}}\label{{sec:{c}.{s}}}")
            refs = [
                lambda: rf"Chapter~\ref{{ch:{g.r.randint(1, c)}}}",
                lambda: rf"Section~\ref{{sec:{g.r.randint(1, c)}.1}}",
            ]
            if alltab:
                refs.append(lambda: rf"Table~\ref{{{g.r.choice(alltab)}}}")
            if allfig:
                refs.append(lambda: rf"Figure~\ref{{{g.r.choice(allfig)}}}")
            if alleq:
                refs.append(lambda: rf"Eq.~\eqref{{{g.r.choice(alleq)}}}")
            for _ in range(3):
                out.append(g.para(7, cites=makers, refs=refs))
            m, lab = math_block(g, f"{c}.{s}", alleq)
            out.append(m)
            out.append(g.para(5, cites=makers, refs=refs))
            if s % 2 == 1:
                tl = f"tab:{c}.{s}"
                alltab.append(tl)
                rows = "\n".join(
                    rf"{g.r.choice(WORDS)} & {g.r.randint(10, 9999)} & {g.r.random():.4f} & {g.r.choice(ADJ)} \\"
                    for _ in range(6)
                )
                out.append(
                    rf"""\begin{{table}}[tbp]\centering
\caption{{{caption(g, cap)}}}\label{{{tl}}}
\begin{{tabular}}{{lrrl}}\hline Item & Count & Share & Type \\ \hline
{rows}
\hline\end{{tabular}}
\end{{table}}"""
                )
            else:
                fl = f"fig:{c}.{s}"
                allfig.append(fl)
                bars = "".join(
                    rf"\rule{{6mm}}{{{g.r.randint(5, 40)}mm}}\hspace{{2mm}}" for _ in range(8)
                )
                out.append(
                    rf"""\begin{{figure}}[tbp]\centering
\fbox{{\parbox[b]{{0.8\textwidth}}{{\centering\vspace{{2mm}}{bars}\vspace{{2mm}}}}}}
\caption{{{caption(g, cap)}}}\label{{{fl}}}
\end{{figure}}"""
                )
            out.append(g.para(6, cites=makers, refs=refs))
            if c == 6 and s == 3:
                out.append(r"The benchmark edit marker reads BENCH-A.")
        chapters.append("\n\n".join(out))
    tex = (
        r"""\documentclass[11pt]{report}
\usepackage[margin=1in]{geometry}
\usepackage{amsmath,amssymb}
\usepackage[round,authoryear]{natbib}
\usepackage{hyperref}
\title{A Long Thesis-Like Document with Many Chapters, Floats and References}
\author{A. Author}
\date{1 January 2026}
\begin{document}
\maketitle
\tableofcontents
\listoffigures
\listoftables
"""
        + "\n\n".join(chapters)
        + "\n\n\\bibliographystyle{plainnat}\n\\bibliography{refs}\n\\end{document}\n"
    )
    write("long_thesis", "main.tex", tex)


def doc_toc_wrap_canary():
    """Regression canary: a long list-of-figures entry that wraps. TeXres once
    hyphenated it differently from TeX Live (the line breaker ignored the dot
    leaders of the entry); it is timed like every other document."""
    tex = r"""\documentclass[11pt]{article}
\usepackage[margin=1in]{geometry}
\begin{document}
\listoffigures
\begin{figure}\caption{We find that the low curve predicts the judgment, while the complex framework relates the average regression.}\end{figure}
The benchmark edit marker reads BENCH-A.
\end{document}
"""
    write("toc_wrap_canary", "main.tex", tex)


def main():
    for f in (
        doc_article_math,
        doc_article_biblatex,
        doc_article_natbib,
        doc_beamer,
        doc_tikz,
        doc_xelatex,
        doc_lualatex,
        doc_long,
        doc_toc_wrap_canary,
    ):
        f()
    print("corpus written to", OUT)
    return 0


if __name__ == "__main__":
    sys.exit(main())
