#!/usr/bin/env python3
"""Generate the synthetic half of the 100-document TeXres benchmark corpus.

Deterministic: rerunning writes byte-identical files under
scripts/bench/corpus100/<name>/ plus corpus100/manifest.json (committed).
All text is synthetic (random words from a fixed vocabulary), so there is
nothing to license. The public half (real arXiv / GitHub sources) is described
by public_manifest.json and fetched by fetch_public.py.

Every document contains the marker `BENCH-A` exactly once, mid-body; the
one-line-edit scenario of scripts/bench100.py replaces it with `BENCH-B`.

    python3 scripts/bench/gen_corpus100.py
"""

import argparse
import json
import random
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from gen_corpus import ADJ, VERBS, WORDS, Gen, bib, math_block  # noqa: E402

OUT = Path(__file__).resolve().parent / "corpus100"
SEED_OFFSET = 0
MANIFEST = {}
MARK = r"The benchmark edit marker reads BENCH-A."

FLAG = {"pdf": "-pdf", "xe": "-xelatex", "lua": "-lualatex"}

# ------------------------------------------------------------- foreign text

LANG_TEXT = {
    "german": [
        "Die effiziente Verteilung bestimmt den Wert des Marktes.",
        "Wir zeigen, dass die Nachfrage mit dem Preis sinkt.",
        "Der durchschnittliche Ertrag hängt von der Größe der Stichprobe ab.",
        "Außerdem verändert die Unsicherheit die Entscheidung der Haushalte.",
        "Schließlich folgt aus der Annahme eine strenge Schranke für den Fehler.",
        "Über die Zeit stabilisiert sich das Gleichgewicht der Wirtschaft.",
    ],
    "french": [
        "La distribution efficace détermine la valeur du marché.",
        "Nous montrons que la demande diminue avec le prix.",
        "Le rendement moyen dépend de la taille de l'échantillon.",
        "De plus, l'incertitude modifie la décision des ménages.",
        "Enfin, l'hypothèse entraîne une borne stricte sur l'erreur.",
        "À long terme, l'équilibre de l'économie se stabilise.",
    ],
    "spanish": [
        "La distribución eficiente determina el valor del mercado.",
        "Mostramos que la demanda disminuye con el precio.",
        "El rendimiento medio depende del tamaño de la muestra.",
        "Además, la incertidumbre cambia la decisión de los hogares.",
        "Finalmente, el supuesto implica una cota estricta del error.",
        "Con el tiempo, el equilibrio de la economía se estabiliza.",
    ],
    "polish": [
        "Efektywny rozkład określa wartość rynku.",
        "Pokazujemy, że popyt maleje wraz z ceną.",
        "Średni zwrot zależy od wielkości próby.",
        "Ponadto niepewność zmienia decyzję gospodarstw domowych.",
        "Wreszcie założenie prowadzi do ścisłego ograniczenia błędu.",
        "Z czasem równowaga gospodarki się stabilizuje.",
    ],
    "russian": [
        "Эффективное распределение определяет стоимость рынка.",
        "Мы показываем, что спрос уменьшается с ростом цены.",
        "Средняя доходность зависит от размера выборки.",
        "Кроме того, неопределённость меняет решение домохозяйств.",
        "Наконец, предположение влечёт строгую границу ошибки.",
        "Со временем равновесие экономики стабилизируется.",
    ],
    "greek": [
        "Η αποδοτική κατανομή καθορίζει την αξία της αγοράς.",
        "Δείχνουμε ότι η ζήτηση μειώνεται με την τιμή.",
        "Η μέση απόδοση εξαρτάται από το μέγεθος του δείγματος.",
        "Επιπλέον, η αβεβαιότητα αλλάζει την απόφαση των νοικοκυριών.",
        "Τέλος, η υπόθεση συνεπάγεται αυστηρό φράγμα του σφάλματος.",
        "Με τον χρόνο η ισορροπία της οικονομίας σταθεροποιείται.",
    ],
    "italian": [
        "La distribuzione efficiente determina il valore del mercato.",
        "Mostriamo che la domanda diminuisce con il prezzo.",
        "Il rendimento medio dipende dalla dimensione del campione.",
        "Inoltre, l'incertezza modifica la decisione delle famiglie.",
        "Infine, l'ipotesi implica un limite stretto sull'errore.",
        "Nel tempo, l'equilibrio dell'economia si stabilizza.",
    ],
}
CJK = [
    "我们发现边际成本稳定了市场。",
    "有效的分配决定了市场的价值。",
    "需求随着价格的上升而下降。",
    "平均收益取决于样本的大小。",
    "此外，不确定性改变了家庭的决策。",
    "最后，这一假设给出了误差的严格上界。",
    "随着时间的推移，经济的均衡趋于稳定。",
    "实验结果表明该方法优于传统的回归模型。",
]

FONTS = {
    "termes": ("texgyretermes", "texgyreheros", "texgyrecursor", "texgyretermes-math.otf"),
    "pagella": ("texgyrepagella", "texgyreheros", "texgyrecursor", "texgyrepagella-math.otf"),
    "bonum": ("texgyrebonum", "texgyreheros", "texgyrecursor", "texgyrebonum-math.otf"),
    "schola": ("texgyreschola", "texgyreheros", "texgyrecursor", "texgyreschola-math.otf"),
}
FSOPT = "[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]"


def font_preamble(fam, math=True):
    m, s, t, mm = FONTS[fam]
    out = [
        r"\usepackage{fontspec}",
        rf"\setmainfont{{{m}}}{FSOPT}",
        rf"\setsansfont{{{s}}}{FSOPT}",
        rf"\setmonofont{{{t}}}{FSOPT}",
    ]
    if math:
        out.insert(0, r"\usepackage{unicode-math}")
        out.append(rf"\setmathfont{{{mm}}}")
    return "\n".join(out)


CMU_FONTS = r"""\usepackage{fontspec}
\setmainfont{cmunrm.otf}[BoldFont=cmunbx.otf,ItalicFont=cmunti.otf,BoldItalicFont=cmunbi.otf]
\setsansfont{cmunss.otf}
\setmonofont{cmuntt.otf}"""

LM_FONTS = r"""\usepackage{fontspec}
\setmainfont{lmroman10-regular.otf}[BoldFont=lmroman10-bold.otf,ItalicFont=lmroman10-italic.otf,BoldItalicFont=lmroman10-bolditalic.otf]
\setsansfont{lmsans10-regular.otf}[BoldFont=lmsans10-bold.otf,ItalicFont=lmsans10-oblique.otf]
\setmonofont{lmmono10-regular.otf}[ItalicFont=lmmono10-italic.otf]"""

# ------------------------------------------------------------ block builders


def fig_example(g, i, cref=False):
    img = g.r.choice(["example-image-a", "example-image-b", "example-image-c"])
    return (
        rf"\begin{{figure}}[tbp]\centering\includegraphics[width=.6\linewidth]{{{img}}}"
        rf"\caption{{{g.phrase().capitalize()} by {g.r.choice(WORDS)}.}}\label{{fig:{i}}}\end{{figure}}"
    )


def fig_sub(g, i):
    return (
        r"\begin{figure}[tbp]\centering"
        r"\begin{subfigure}{.45\linewidth}\includegraphics[width=\linewidth]{example-image-a}"
        rf"\caption{{{g.phrase().capitalize()}}}\label{{fig:{i}a}}\end{{subfigure}}\hfill"
        r"\begin{subfigure}{.45\linewidth}\includegraphics[width=\linewidth]{example-image-b}"
        rf"\caption{{{g.phrase().capitalize()}}}\label{{fig:{i}b}}\end{{subfigure}}"
        rf"\caption{{{g.phrase().capitalize()} in two panels.}}\label{{fig:{i}}}\end{{figure}}"
    )


def fig_tikz(g, i):
    r = g.r
    n = r.randint(4, 7)
    nodes = "\n".join(
        rf"\node[draw,circle,fill=blue!{r.randint(10, 40)}] (n{k}) at ({r.randint(0, 6)},{r.randint(0, 3)}) {{{k}}};"
        for k in range(n)
    )
    edges = "\n".join(
        rf"\draw[->,thick] (n{a}) to[bend left={r.randint(5, 30)}] (n{b});"
        for a, b in [(r.randrange(n), r.randrange(n)) for _ in range(n + 2)]
        if a != b
    )
    return (
        r"\begin{figure}[tbp]\centering\begin{tikzpicture}[scale=.8]"
        + "\n"
        + nodes
        + "\n"
        + edges
        + "\n"
        + rf"\end{{tikzpicture}}\caption{{{g.phrase().capitalize()} network.}}\label{{fig:{i}}}\end{{figure}}"
    )


def fig_plot(g, i):
    r = g.r
    a, b = r.randint(1, 4), r.randint(1, 5)
    return (
        r"\begin{figure}[tbp]\centering\begin{tikzpicture}\begin{axis}[width=.8\linewidth,height=5.5cm,grid=major,"
        rf"xlabel={{$x$}},ylabel={{$f(x)$}},legend pos=north west]"
        "\n"
        rf"\addplot[blue,thick,domain=0:6,samples=60]{{{a}*sin(deg(x)) + {b}*x/3}};"
        "\n"
        rf"\addplot[red,dashed,domain=0:6,samples=60]{{exp(-x/{a + 1})*{b}}};"
        "\n"
        r"\legend{signal,decay}\end{axis}\end{tikzpicture}"
        rf"\caption{{{g.phrase().capitalize()} response.}}\label{{fig:{i}}}\end{{figure}}"
    )


def tbl_booktabs(g, i, cols=4, rows=8):
    r = g.r
    head = " & ".join(r.choice(WORDS).capitalize() for _ in range(cols))
    body = "\n".join(
        " & ".join([r.choice(WORDS)] + [f"{r.random() * 100:.2f}" for _ in range(cols - 1)]) + r" \\"
        for _ in range(rows)
    )
    return (
        r"\begin{table}[tbp]\centering"
        rf"\caption{{{g.phrase().capitalize()} summary.}}\label{{tab:{i}}}"
        rf"\begin{{tabular}}{{l{'r' * (cols - 1)}}}\toprule {head} \\ \midrule"
        f"\n{body}\n"
        r"\bottomrule\end{tabular}\end{table}"
    )


def tbl_long(g, i, rows=60):
    r = g.r
    body = "\n".join(
        rf"{k} & {r.choice(WORDS)} {r.choice(WORDS)} & {r.random() * 1000:.3f} & {r.randint(1, 9999)} \\"
        for k in range(1, rows + 1)
    )
    return (
        rf"\begin{{longtable}}{{rllr}}\caption{{{g.phrase().capitalize()} listing.}}\label{{tab:{i}}}\\"
        r"\toprule No. & Item & Value & Count \\ \midrule \endfirsthead"
        r"\toprule No. & Item & Value & Count \\ \midrule \endhead"
        r"\bottomrule \endfoot"
        f"\n{body}\n"
        r"\end{longtable}"
    )


def tbl_tabularx(g, i):
    r = g.r
    rows = "\n".join(rf"{r.choice(WORDS)} & {g.sentence()} & {r.randint(1, 99)} \\ \hline" for _ in range(6))
    return (
        r"\begin{table}[tbp]\centering"
        rf"\caption{{{g.phrase().capitalize()} descriptions.}}\label{{tab:{i}}}"
        r"\begin{tabularx}{\linewidth}{|l|X|r|}\hline Name & Description & N \\ \hline"
        f"\n{rows}\n"
        r"\end{tabularx}\end{table}"
    )


def tbl_siunitx(g, i):
    r = g.r
    rows = "\n".join(
        rf"{r.choice(WORDS)} & {r.random() * 1e4:.3f} & \SI{{{r.random() * 90:.2f}}}{{\metre\per\second}} & \qty{{{r.randint(1, 500)}}}{{\kilo\gram}} \\"
        for _ in range(7)
    )
    return (
        r"\begin{table}[tbp]\centering"
        rf"\caption{{{g.phrase().capitalize()} measurements.}}\label{{tab:{i}}}"
        r"\begin{tabular}{lS[table-format=4.3]ll}\toprule Item & {Value} & Speed & Mass \\ \midrule"
        f"\n{rows}\n"
        r"\bottomrule\end{tabular}\end{table}"
    )


def listing(g, i):
    r = g.r
    lines = "\n".join(
        f"    total{k} = total{k - 1} + {r.choice(WORDS)}[{k}] * {r.randint(2, 9)}  # {g.phrase()}" for k in range(1, 12)
    )
    return (
        r"\begin{lstlisting}[language=Python,caption={" + g.phrase().capitalize() + r" routine.},label=lst:" + str(i) + "]\n"
        f"def compute_{r.choice(WORDS)}({r.choice(WORDS)}):\n    total0 = 0\n{lines}\n    return total11\n"
        r"\end{lstlisting}"
    )


def algo2e(g, i):
    r = g.r
    return (
        rf"\begin{{algorithm}}[tbp]\caption{{{g.phrase().capitalize()} procedure.}}\label{{alg:{i}}}"
        rf"\KwData{{{r.choice(WORDS)} $x$ and tolerance $\epsilon$}}\KwResult{{{r.choice(WORDS)} $\hat\theta$}}"
        r"initialize $\theta\leftarrow 0$\;"
        rf"\While{{$\|g(\theta)\|>\epsilon$}}{{compute the {r.choice(WORDS)}\;"
        rf"\eIf{{$\theta<{r.randint(1, 9)}$}}{{update $\theta\leftarrow\theta+\eta g$\;}}{{shrink $\eta\leftarrow\eta/2$\;}}"
        r"\ForEach{block $b$}{accumulate the $b$-th term\;}}"
        r"\Return{$\theta$}\end{algorithm}"
    )


def theorem_block(g, key, builtin=False):
    t = (
        rf"\begin{{definition}}\label{{def:{key}}} {g.para(2)} \end{{definition}}"
        "\n"
        rf"\begin{{theorem}}\label{{thm:{key}}} {g.para(2)} By Definition~\ref{{def:{key}}} the bound holds. \end{{theorem}}"
        "\n"
        rf"\begin{{proof}} {g.para(3)} This follows from Theorem~\ref{{thm:{key}}}. \end{{proof}}"
    )
    return t


def foot_para(g, nfoot=6):
    out = []
    for _ in range(nfoot):
        s = g.sentence()
        out.append(s[:-1] + r"\footnote{" + g.sentence() + " " + g.sentence() + "}.")
    return " ".join(out)


def itemize(g):
    k = g.r.choice(["itemize", "enumerate", "description"])
    items = "\n".join((rf"\item[{g.r.choice(WORDS)}] " if k == "description" else r"\item ") + g.sentence() for _ in range(4))
    return rf"\begin{{{k}}}{chr(10)}{items}{chr(10)}\end{{{k}}}"


def lang_para(lang, r, n=6):
    pool = LANG_TEXT[lang]
    return " ".join(r.choice(pool) for _ in range(n))


def cjk_para(r, n=6):
    return "".join(r.choice(CJK) for _ in range(n))


# ----------------------------------------------------------- body assembler


def make_body(g, nsec, feats, *, cites=None, chapters=0, marker=True, cleveref=False, lang=None, cjk=False,
              tail_para=5, theorem=True, idx=False, gloss=0):
    """Return list of body chunks. feats: dict feature -> every-k sections."""
    r = g.r
    body = []
    labels = []
    total = nsec
    figs = 0
    refs = [lambda: rf"Section~\ref{{sec:{r.randint(1, total)}}}"]
    if cleveref:
        refs = [lambda: rf"\cref{{sec:{r.randint(1, total)}}}"]
    mid = max(1, total // 2)
    per_chap = max(1, total // chapters) if chapters else 0
    for s in range(1, total + 1):
        if chapters and (s - 1) % per_chap == 0 and (s - 1) // per_chap < chapters:
            body.append(rf"\chapter{{{g.phrase().title()}}}\label{{chap:{(s - 1) // per_chap + 1}}}")
        body.append(rf"\section{{{g.phrase().title()}}}\label{{sec:{s}}}")
        if idx:
            body.append(rf"\index{{{g.r.choice(WORDS)}}}\index{{{g.r.choice(WORDS)}!{g.r.choice(ADJ)}}}")

        def para(n=None):
            if lang:
                return lang_para(lang, r, n or 6)
            if cjk:
                return cjk_para(r, n or 5) + "\n\n" + g.para(n or 3, cites=cites, refs=refs)
            return g.para(n, cites=cites, refs=refs)

        body.append(para(6))
        if gloss:
            body.append(rf"\gls{{g{r.randint(1, gloss)}}} and \gls{{g{r.randint(1, gloss)}}} appear here, with \glspl{{g{r.randint(1, gloss)}}}.")
        if feats.get("math") and s % feats["math"] == 0:
            m, lab = math_block(g, f"{s}", labels)
            body.append(m)
            ref = rf"\cref{{{lab}}}" if cleveref else rf"Eq.~\eqref{{{lab}}}"
            body.append(f"Compare {ref} with the earlier results. " + para(3))
        if feats.get("theorem") and theorem and s % feats["theorem"] == 0:
            body.append(theorem_block(g, s))
        for key, fn in (
            ("fig", lambda: fig_example(g, f"f{s}")),
            ("sub", lambda: fig_sub(g, f"s{s}")),
            ("tikz", lambda: fig_tikz(g, f"t{s}")),
            ("plot", lambda: fig_plot(g, f"p{s}")),
            ("tab", lambda: tbl_booktabs(g, f"b{s}")),
            ("long", lambda: tbl_long(g, f"l{s}")),
            ("tabx", lambda: tbl_tabularx(g, f"x{s}")),
            ("si", lambda: tbl_siunitx(g, f"u{s}")),
            ("lst", lambda: listing(g, s)),
            ("algo", lambda: algo2e(g, s)),
            ("list", lambda: itemize(g)),
        ):
            if feats.get(key) and s % feats[key] == 0:
                body.append(fn())
                if key in ("fig", "sub", "tikz", "plot"):
                    lab = {"fig": "f", "sub": "s", "tikz": "t", "plot": "p"}[key] + str(s)
                    body.append((rf"As shown in \cref{{fig:{lab}}}, " if cleveref else rf"See Figure~\ref{{fig:{lab}}}. ") + para(2))
                if key in ("tab", "long", "tabx", "si"):
                    lab = {"tab": "b", "long": "l", "tabx": "x", "si": "u"}[key] + str(s)
                    body.append((rf"\cref{{tab:{lab}}} lists the values. " if cleveref else rf"Table~\ref{{tab:{lab}}} lists the values. ") + para(2))
        if feats.get("foot") and s % feats["foot"] == 0:
            body.append(foot_para(g, feats.get("nfoot", 6)))
        if feats.get("siunit") and s % feats["siunit"] == 0:
            body.append(
                rf"The sample reached \SI{{{r.random() * 100:.2f}}}{{\metre\per\second}} at \qty{{{r.randint(280, 330)}}}{{\kelvin}} "
                rf"with a tolerance of \num{{{r.random():.4f}e-{r.randint(2, 6)}}}, i.e.\ \SI{{{r.randint(1, 99)}}}{{\percent}} of the range \SIrange{{{r.randint(1, 5)}}}{{{r.randint(6, 12)}}}{{\volt}}."
            )
        if feats.get("sub2") and s % feats["sub2"] == 0:
            body.append(rf"\subsection{{{g.phrase().title()}}}" + "\n" + para(5))
            body.append(rf"\subsubsection{{{g.phrase().title()}}}" + "\n" + para(4))
        if marker and s == mid:
            body.append(MARK)
        body.append(para(tail_para))
    return body


def bib_tail(mode):
    kind = mode[0]
    if kind == "biblatex":
        return r"\printbibliography" + "\n"
    if kind == "natbib" or kind == "bibtex":
        return rf"\bibliographystyle{{{mode[1]}}}" + "\n" + r"\bibliography{refs}" + "\n"
    return ""


def cite_fns(g, keys, cmds):
    def one(c):
        return lambda: rf"\{c}{{{g.r.choice(keys)}}}"

    out = [one(c) for c in cmds]
    out.append(lambda: rf"\{cmds[0]}{{{','.join(g.r.sample(keys, 3))}}}")
    return out


FEAT_PKGS = {
    "math": ["amsmath", "amssymb", "mathtools"][:2],
    "theorem": ["amsmath", "amssymb"],
    "fig": ["graphicx"],
    "sub": ["graphicx", "subcaption"],
    "tikz": ["tikz"],
    "plot": ["pgfplots"],
    "tab": ["booktabs"],
    "long": ["booktabs", "longtable"],
    "tabx": ["tabularx", "array"],
    "si": ["booktabs", "siunitx"],
    "siunit": ["siunitx"],
    "lst": ["listings"],
    "algo": ["algorithm2e"],
}


def auto_packages(pre, feats, cleveref):
    have = set()
    for m in re.finditer(r"\\(?:usepackage|RequirePackage)(?:\[[^\]]*\])?\{([^}]*)\}", pre):
        have.update(x.strip() for x in m.group(1).split(","))
    need = []
    if "unicode-math" in pre:
        have.add("amssymb")
    for k, v in feats.items():
        if k in FEAT_PKGS and v:
            for pk in FEAT_PKGS[k]:
                if pk not in have and pk not in need:
                    need.append(pk)
    lines = []
    for pk in need:
        if pk == "pgfplots":
            lines.append("\\usepackage{pgfplots}\\pgfplotsset{compat=1.18}")
        elif pk == "algorithm2e":
            lines.append("\\usepackage[ruled,vlined]{algorithm2e}")
        else:
            lines.append(f"\\usepackage{{{pk}}}")
    return "\n".join(lines)


def emit(name, engine, tex, files=None, tags=None, note=""):
    d = OUT / name
    d.mkdir(parents=True, exist_ok=True)
    (d / "main.tex").write_text(tex, encoding="utf-8")
    for fn, text in (files or {}).items():
        (d / fn).write_text(text, encoding="utf-8")
    MANIFEST[name] = {"engine": engine, "flag": FLAG[engine], "main": "main.tex", "source": "generated", "tags": tags or [], "note": note}


THM_AMS = (
    r"\usepackage{amsmath,amssymb,amsthm}" "\n"
    r"\newtheorem{theorem}{Theorem}[section]" "\n"
    r"\newtheorem{lemma}[theorem]{Lemma}" "\n"
    r"\theoremstyle{definition}" "\n"
    r"\newtheorem{definition}[theorem]{Definition}"
)


def std_front(title, author="A. Author", toc=True, abstract=True):
    s = rf"\title{{{title}}}" "\n" rf"\author{{{author}}}" "\n" r"\date{1 January 2026}" "\n" r"\begin{document}" "\n" r"\maketitle" "\n"
    if abstract:
        s += r"\begin{abstract}" + "\n" + "This synthetic benchmark document exercises the engine with typical content." + "\n" r"\end{abstract}" + "\n"
    if toc:
        s += r"\tableofcontents" + "\n"
    return s


def build(name, engine, *, cls="article", opts="11pt", pre="", front=None, nsec=8, feats=None, bibmode=None, nbib=60,
          seed=None, title=None, tags=None, cleveref=False, lang=None, cjk=False, chapters=0, idx=False, gloss=0,
          tail="", citecmds=("parencite", "textcite", "cite"), theorem=True, toc=True, abstract=True, note="",
          tail_para=5, seed_bib=None):
    g = Gen((seed if seed is not None else sum(map(ord, name)) * 7919) + SEED_OFFSET)
    files = {}
    cites = None
    if bibmode:
        keys, b = bib((seed_bib or (sum(map(ord, name)) + 11)) + SEED_OFFSET, nbib)
        files["refs.bib"] = b
        cmds = citecmds
        cites = cite_fns(g, keys, list(cmds))
    pre = auto_packages(pre, feats or {}, cleveref) + "\n" + pre
    body = make_body(g, nsec, feats or {}, cites=cites, chapters=chapters, cleveref=cleveref, lang=lang, cjk=cjk,
                     idx=idx, gloss=gloss, theorem=theorem, tail_para=tail_para)
    if front is None:
        front = std_front(title or name.replace("_", " ").title(), toc=toc, abstract=abstract and not chapters)
    tex = rf"\documentclass[{opts}]{{{cls}}}" "\n" + pre.strip() + "\n" + front + "\n\n".join(body) + "\n\n" + tail
    if bibmode:
        tex += bib_tail(bibmode)
    tex += "\\end{document}\n"
    emit(name, engine, tex, files, tags, note)


# =============================================================== the corpus

GEO = r"\usepackage[margin=1in]{geometry}"


def define_docs():
    # ---------------------------------------------------------------- pdfLaTeX
    build("pdf_cleveref_hyperref", "pdf", pre=GEO + "\n" + THM_AMS + "\n\\usepackage{hyperref}\n\\usepackage[capitalize]{cleveref}",
          nsec=30, feats={"math": 1, "theorem": 2, "fig": 3, "tab": 4}, cleveref=True, tags=["article", "cleveref", "hyperref", "amsthm"])
    build("pdf_biblatex_numeric", "pdf", pre=GEO + "\n\\usepackage[backend=biber,style=numeric-comp,sorting=nyt]{biblatex}\n\\addbibresource{refs.bib}\n\\usepackage{hyperref}",
          nsec=25, feats={"math": 3, "tab": 4}, bibmode=("biblatex",), tags=["article", "biblatex", "biber", "numeric-comp"])
    build("pdf_biblatex_authortitle", "pdf", pre=GEO + "\n\\usepackage[backend=biber,style=authortitle]{biblatex}\n\\addbibresource{refs.bib}",
          nsec=7, feats={"tab": 3}, bibmode=("biblatex",), citecmds=("autocite", "textcite", "cite"), nbib=40, tags=["article", "biblatex", "biber", "authortitle"])
    build("pdf_biblatex_ieee", "pdf", cls="article", opts="10pt,twocolumn",
          pre="\\usepackage[margin=.8in]{geometry}\n\\usepackage[backend=biber,style=ieee]{biblatex}\n\\addbibresource{refs.bib}",
          nsec=24, feats={"math": 2, "fig": 4}, bibmode=("biblatex",), citecmds=("cite",), nbib=50, tags=["article", "biblatex", "biber", "ieee", "twocolumn"])
    build("pdf_biblatex_chicago", "pdf",
          pre=GEO + "\n\\usepackage[backend=biber,style=chicago-authordate]{biblatex}\n\\addbibresource{refs.bib}",
          nsec=30, feats={}, bibmode=("biblatex",), nbib=50, tags=["article", "biblatex", "biber", "chicago-authordate"])
    build("pdf_natbib_numbers", "pdf", pre=GEO + "\n\\usepackage[numbers,sort&compress]{natbib}",
          nsec=40, feats={"math": 2, "tab": 3}, bibmode=("natbib", "plainnat"), citecmds=("citep", "citet"), tags=["article", "natbib", "bibtex"])
    build("pdf_report_longtable", "pdf", cls="report", opts="11pt",
          pre=GEO + "\n\\usepackage{booktabs,longtable,tabularx,array}", nsec=12, chapters=4,
          feats={"long": 1, "tabx": 2, "tab": 3}, tags=["report", "longtable", "tabularx", "booktabs"], toc=True, tail_para=3)
    build("pdf_memoir_book", "pdf", cls="memoir", opts="11pt,oneside",
          pre="\\usepackage{amsmath,amssymb}\n\\usepackage{graphicx}\n\\chapterstyle{veelo}\n\\pagestyle{headings}",
          nsec=160, chapters=10, feats={"math": 3, "fig": 5}, tags=["memoir", "book"], tail_para=8)
    build("pdf_scrbook_large", "pdf", cls="scrbook", opts="11pt,a4paper,twoside",
          pre="\\usepackage{amsmath,amssymb,amsthm}\n\\usepackage{graphicx,booktabs}\n\\newtheorem{theorem}{Theorem}[chapter]\n\\newtheorem{lemma}[theorem]{Lemma}\n\\theoremstyle{definition}\n\\newtheorem{definition}[theorem]{Definition}",
          nsec=300, chapters=12, feats={"math": 2, "theorem": 4, "fig": 6, "tab": 7, "foot": 5}, tags=["scrbook", "book", "KOMA"], tail_para=9,
          note="~300 pages")
    build("pdf_scrartcl_floats", "pdf", cls="scrartcl", opts="11pt,a4paper",
          pre="\\usepackage{graphicx,booktabs,subcaption}\n\\usepackage{float}", nsec=40,
          feats={"sub": 1, "fig": 2, "tab": 2}, tags=["scrartcl", "KOMA", "subcaption", "float-heavy"], tail_para=3)
    build("pdf_amsart", "pdf", cls="amsart", opts="11pt",
          pre="\\usepackage{amsmath,amssymb}\n\\newtheorem{theorem}{Theorem}[section]\n\\newtheorem{lemma}[theorem]{Lemma}\n\\theoremstyle{definition}\n\\newtheorem{definition}[theorem]{Definition}",
          front="\\title{On Stable Estimators of Marginal Structure}\n\\author{A. Author}\n\\address{Benchmark Institute}\n\\date{1 January 2026}\n\\begin{document}\n\\begin{abstract}A synthetic abstract.\\end{abstract}\n\\maketitle\n",
          nsec=50, feats={"math": 1, "theorem": 1}, tags=["amsart", "amsthm"])
    build("pdf_revtex4_2", "pdf", cls="revtex4-2", opts="aps,prd,twocolumn,amsmath,superscriptaddress",
          pre="\\usepackage{graphicx}\n\\newtheorem{theorem}{Theorem}",
          front="\\begin{document}\n\\title{Dynamic Response of a Structured Sample}\n\\author{A. Author}\n\\affiliation{Benchmark Institute}\n\\date{1 January 2026}\n\\begin{abstract}A synthetic abstract.\\end{abstract}\n\\maketitle\n",
          nsec=20, feats={"math": 1, "fig": 3}, bibmode=None, tags=["revtex4-2", "twocolumn"], theorem=False)
    build("pdf_elsarticle", "pdf", cls="elsarticle", opts="preprint,12pt,authoryear",
          pre="\\usepackage{amsmath,amssymb}\n\\usepackage{natbib}\n\\usepackage{booktabs}\n\\journal{Journal of Benchmarks}",
          front="\\begin{document}\n\\begin{frontmatter}\n\\title{Estimation under Structural Uncertainty}\n\\author{A. Author}\n\\address{Benchmark Institute}\n\\begin{abstract}A synthetic abstract.\\end{abstract}\n\\begin{keyword}benchmark \\sep synthetic\\end{keyword}\n\\end{frontmatter}\n",
          nsec=30, feats={"math": 2, "tab": 3}, bibmode=("bibtex", "elsarticle-harv"), citecmds=("citep", "citet"), theorem=False,
          tags=["elsarticle", "bibtex", "elsarticle-harv"])
    build("pdf_ieeetran_conf", "pdf", cls="IEEEtran", opts="conference",
          pre="\\usepackage{amsmath,amssymb}\n\\usepackage{graphicx}\n\\usepackage{cite}",
          front="\\title{Latency Bounds for Adaptive Scheduling}\n\\author{\\IEEEauthorblockN{A. Author}\\IEEEauthorblockA{Benchmark Institute}}\n\\begin{document}\n\\maketitle\n\\begin{abstract}A synthetic abstract.\\end{abstract}\n",
          nsec=20, feats={"math": 2, "fig": 3}, bibmode=("bibtex", "IEEEtran"), citecmds=("cite",), theorem=False, tags=["IEEEtran", "bibtex", "twocolumn"])
    build("pdf_llncs", "pdf", cls="llncs", opts="runningheads",
          pre="\\usepackage{amsmath,amssymb}\n\\usepackage{graphicx}",
          front="\\title{Verifying Scheduling Invariants}\n\\author{A. Author}\n\\institute{Benchmark Institute}\n\\begin{document}\n\\maketitle\n\\begin{abstract}A synthetic abstract.\\end{abstract}\n",
          nsec=10, feats={"math": 2, "theorem": 3, "fig": 4}, bibmode=("bibtex", "splncs04"), citecmds=("cite",), toc=False,
          tags=["llncs", "bibtex", "splncs04"])
    build("pdf_standalone_tikz", "pdf", cls="standalone", opts="border=5pt,tikz",
          pre="\\usepackage{pgfplots}\\pgfplotsset{compat=1.18}", front="\\begin{document}\n",
          nsec=0, tags=["standalone", "tikz"], tail=r"""\begin{tikzpicture}
\foreach \i in {0,...,11} {\draw[fill=blue!\i0] (\i*30:2) circle (.4);}
\draw[thick,->] (0,0) -- (3,0) node[right]{$x$};
\node at (0,-3) {BENCH-A};
\end{tikzpicture}
""")
    build("pdf_tikz_diagrams", "pdf", pre=GEO + "\n\\usepackage{tikz}\n\\usetikzlibrary{arrows.meta,positioning,shapes}",
          nsec=40, feats={"tikz": 1}, tags=["article", "tikz"], tail_para=3)
    build("pdf_pgfplots_gallery", "pdf", pre=GEO + "\n\\usepackage{pgfplots}\n\\pgfplotsset{compat=1.18}",
          nsec=30, feats={"plot": 1}, tags=["article", "pgfplots"], tail_para=2)
    build("pdf_siunitx_tables", "pdf", pre=GEO + "\n\\usepackage{siunitx}\n\\usepackage{booktabs}",
          nsec=30, feats={"si": 1, "siunit": 1}, tags=["article", "siunitx", "booktabs"], tail_para=3)
    build("pdf_listings_algorithm2e", "pdf", pre=GEO + "\n\\usepackage{listings}\n\\usepackage[ruled,vlined,linesnumbered]{algorithm2e}\n\\usepackage{xcolor}\n\\lstset{basicstyle=\\ttfamily\\small,keywordstyle=\\color{blue},commentstyle=\\color{gray},numbers=left}",
          nsec=40, feats={"lst": 1, "algo": 2}, tags=["article", "listings", "algorithm2e", "xcolor"], tail_para=3)
    build("pdf_glossaries", "pdf", pre=GEO + "\n\\usepackage{glossaries}\n\\makenoidxglossaries\n" + "\n".join(
        rf"\newglossaryentry{{g{k}}}{{name={{{w}}},description={{The {w} of the system, defined for the benchmark.}},plural={{{w}s}}}}" for k, w in enumerate(
            ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta"], 1)),
          nsec=8, gloss=8, feats={}, tail="\\printnoidxglossary\n", tags=["article", "glossaries"])
    build("pdf_makeindex", "pdf", cls="book", opts="11pt", pre="\\usepackage{makeidx}\n\\makeindex",
          nsec=30, chapters=6, idx=True, feats={"tab": 5}, tail="\\printindex\n", tags=["book", "makeidx", "makeindex"], tail_para=7)
    build("pdf_imakeidx", "pdf", pre=GEO + "\n\\usepackage[makeindex]{imakeidx}\n\\makeindex[intoc]",
          nsec=40, idx=True, feats={}, tail="\\printindex\n", tags=["article", "imakeidx", "makeindex"])
    build("pdf_footnote_heavy", "pdf", pre=GEO + "\n\\usepackage[bottom]{footmisc}", nsec=40,
          feats={"foot": 1, "nfoot": 12}, tags=["article", "footmisc", "footnote-heavy"], tail_para=3)
    build("pdf_float_heavy", "pdf", pre=GEO + "\n\\usepackage{graphicx,booktabs,subcaption}\n\\usepackage[section]{placeins}",
          nsec=20, feats={"fig": 1, "sub": 1, "tab": 1}, tags=["article", "float-heavy", "subcaption"], tail_para=2)
    build("pdf_microtype", "pdf", cls="article", opts="10pt,twocolumn",
          pre="\\usepackage[margin=.8in]{geometry}\n\\usepackage{mathpazo}\n\\usepackage[protrusion=true,expansion=true]{microtype}\n\\usepackage{amsmath}",
          nsec=90, feats={"math": 3}, tags=["article", "microtype", "twocolumn"], tail_para=6)
    build("pdf_hyperref_toc", "pdf", cls="book", opts="11pt",
          pre="\\usepackage[colorlinks,linkcolor=blue,bookmarksnumbered,pdfusetitle]{hyperref}\n\\usepackage{amsmath}\n\\setcounter{tocdepth}{3}",
          nsec=60, chapters=10, feats={"math": 2, "sub2": 1}, tags=["book", "hyperref", "bookmarks"], tail_para=4)
    build("pdf_tcolorbox", "pdf", pre=GEO + "\n\\usepackage[most]{tcolorbox}\n\\usepackage{xcolor}\n\\newtcolorbox{note}[1]{colback=blue!5,colframe=blue!50!black,title=#1}",
          nsec=20, feats={"list": 1, "tab": 3}, tags=["article", "tcolorbox", "xcolor"],
          tail="")
    build("pdf_letter", "pdf", cls="letter", opts="11pt", pre="\\usepackage[margin=1in]{geometry}",
          front="\\signature{A. Author}\n\\address{Benchmark Institute \\\\ 1 Main Street}\n\\begin{document}\n\\begin{letter}{Dr. B. Recipient \\\\ Another Institute}\n\\opening{Dear Dr. Recipient,}\n",
          nsec=0, tags=["letter"], tail=MARK + " " + "We write about the ongoing synthetic benchmark and its results. " * 12 + "\n\\closing{Sincerely,}\n\\end{letter}\n".replace("\\end{letter}\n", "\\end{letter}\n"))
    build("pdf_beamer_metropolis", "pdf", cls="beamer", opts="aspectratio=169",
          pre="\\usetheme{metropolis}\n\\usepackage{amsmath}",
          front="\\title{A Metropolis Deck}\n\\author{A. Author}\n\\date{1 January 2026}\n\\begin{document}\n\\begin{frame}\\titlepage\\end{frame}\n",
          nsec=0, tags=["beamer", "metropolis"],
          tail="\n".join(
              rf"\begin{{frame}}{{{Gen(k).phrase().title()}}}{chr(10)}\begin{{itemize}}{chr(10)}"
              + "\n".join(rf"\item<{i}-> {Gen(k * 10 + i).sentence()}" for i in range(1, 4))
              + (("\n" + MARK) if k == 12 else "")
              + rf"{chr(10)}\end{{itemize}}{chr(10)}\end{{frame}}"
              for k in range(1, 25)) + "\n")
    build("pdf_beamer_boadilla", "pdf", cls="beamer", opts="11pt",
          pre="\\usetheme{Boadilla}\n\\usecolortheme{seahorse}\n\\usepackage{tikz}\n\\usepackage{pgfplots}\\pgfplotsset{compat=1.18}",
          front="\\title{Figures in Frames}\n\\author{A. Author}\n\\date{1 January 2026}\n\\begin{document}\n\\begin{frame}\\titlepage\\end{frame}\n",
          nsec=0, tags=["beamer", "Boadilla", "pgfplots"],
          tail="\n".join(
              rf"\begin{{frame}}{{Plot {k}}}{chr(10)}\begin{{tikzpicture}}\begin{{axis}}[width=.7\textwidth,height=.6\textheight]\addplot[blue,domain=0:{k + 3},samples=50]{{sin(deg(x))*{k}}};\end{{axis}}\end{{tikzpicture}}"
              + (("\n" + MARK) if k == 6 else "") + rf"{chr(10)}\end{{frame}}"
              for k in range(1, 13)) + "\n")

    # ------------------------------------------------------------------ XeLaTeX
    for fam in ("termes", "pagella", "bonum", "schola"):
        build(f"xe_fontspec_{fam}", "xe", pre=GEO + "\n\\usepackage{amsmath}\n" + font_preamble(fam),
              nsec={"pagella": 13, "schola": 30}.get(fam, 7), feats={"math": 2, "tab": 4, "foot": 3}, tags=["article", "xelatex", "fontspec", "unicode-math", fam])
    build("xe_lm_fontspec", "xe", pre=GEO + "\n" + LM_FONTS, nsec=8, feats={"tab": 3}, tags=["article", "xelatex", "fontspec", "latin-modern"])
    build("xe_book_memoir", "xe", cls="memoir", opts="11pt",
          pre="\\usepackage{amsmath}\n" + font_preamble("pagella", math=False) + "\n\\chapterstyle{bianchi}",
          nsec=24, chapters=6, feats={"math": 3, "fig": 5}, tags=["memoir", "xelatex", "fontspec"], tail_para=7)
    build("xe_biblatex", "xe", pre=GEO + "\n" + font_preamble("termes", math=False) + "\n\\usepackage[backend=biber,style=authoryear]{biblatex}\n\\addbibresource{refs.bib}",
          nsec=8, feats={"foot": 4}, bibmode=("biblatex",), tags=["article", "xelatex", "biblatex", "biber"])
    build("xe_tikz_pgfplots", "xe", pre=GEO + "\n" + font_preamble("termes", math=False) + "\n\\usepackage{tikz,pgfplots}\n\\pgfplotsset{compat=1.18}\n\\usetikzlibrary{arrows.meta}",
          nsec=8, feats={"tikz": 2, "plot": 1}, tags=["article", "xelatex", "tikz", "pgfplots"], tail_para=3)
    build("xe_siunitx", "xe", pre=GEO + "\n" + font_preamble("pagella", math=False) + "\n\\usepackage{siunitx,booktabs}",
          nsec=7, feats={"si": 1, "siunit": 1}, tags=["article", "xelatex", "siunitx"], tail_para=3)
    build("xe_beamer", "xe", cls="beamer", opts="aspectratio=169",
          pre="\\usetheme{Madrid}\n" + font_preamble("pagella", math=False),
          front="\\title{Beamer under XeLaTeX}\n\\author{A. Author}\n\\date{1 January 2026}\n\\begin{document}\n\\begin{frame}\\titlepage\\end{frame}\n",
          nsec=0, tags=["beamer", "xelatex", "fontspec"],
          tail="\n".join(
              rf"\begin{{frame}}{{{Gen(k).phrase().title()}}}{chr(10)}{Gen(k + 100).para(3)}"
              + (("\n" + MARK) if k == 10 else "") + rf"{chr(10)}\end{{frame}}" for k in range(1, 21)) + "\n")
    for lang, pg_name, fam in (("german", "german", "termes"), ("french", "french", "pagella"), ("spanish", "spanish", "termes"),
                                ("polish", "polish", "bonum"), ("russian", "russian", "cmu"), ("greek", "greek", "cmu")):
        fonts = CMU_FONTS if fam == "cmu" else font_preamble(fam, math=False)
        extra = ""
        if lang == "russian":
            extra = "\\newfontfamily\\cyrillicfont{cmunrm.otf}[BoldFont=cmunbx.otf,ItalicFont=cmunti.otf,BoldItalicFont=cmunbi.otf]\n\\newfontfamily\\cyrillicfontsf{cmunss.otf}\n"
        if lang == "greek":
            extra = "\\newfontfamily\\greekfont{cmunrm.otf}[BoldFont=cmunbx.otf,ItalicFont=cmunti.otf,BoldItalicFont=cmunbi.otf]\n\\newfontfamily\\greekfontsf{cmunss.otf}\n"
        build(f"xe_polyglossia_{lang}", "xe", pre=GEO + "\n" + fonts + f"\n\\usepackage{{polyglossia}}\n\\setmainlanguage{{{pg_name}}}\n" + extra,
              nsec={"german": 40, "greek": 20}.get(lang, 8), lang=lang, feats={"tab": 3, "foot": 3}, tags=["article", "xelatex", "polyglossia", lang])
    build("xe_polyglossia_multilingual", "xe", pre=GEO + "\n" + LM_FONTS + "\n\\usepackage{polyglossia}\n\\setmainlanguage{english}\n\\setotherlanguages{german,french,spanish}",
          nsec=6, feats={}, tags=["article", "xelatex", "polyglossia", "multilingual"],
          tail="\\begin{german}\n" + lang_para("german", random.Random(5), 8) + "\n\\end{german}\n\n\\begin{french}\n" + lang_para("french", random.Random(6), 8) + "\n\\end{french}\n\n\\begin{spanish}\n" + lang_para("spanish", random.Random(7), 8) + "\n\\end{spanish}\n\n")
    build("xe_xecjk_chinese", "xe", pre=GEO + "\n" + font_preamble("termes", math=False) + "\n\\usepackage{xeCJK}\n\\setCJKmainfont{FandolSong-Regular.otf}[BoldFont=FandolSong-Bold.otf,ItalicFont=FandolKai-Regular.otf]\n\\setCJKsansfont{FandolHei-Regular.otf}",
          nsec=40, cjk=True, feats={"tab": 3}, tags=["article", "xelatex", "xeCJK", "CJK"], tail_para=3)
    build("xe_ctex_article", "xe", cls="ctexart", opts="fontset=fandol,11pt", pre=GEO,
          nsec=30, cjk=True, feats={"tab": 3, "foot": 3}, tags=["ctexart", "xelatex", "ctex", "CJK"],
          front="\\title{经济学中的稳定估计}\n\\author{作者}\n\\date{2026年1月1日}\n\\begin{document}\n\\maketitle\n\\tableofcontents\n", tail_para=3)
    build("xe_ctex_book", "xe", cls="ctexbook", opts="fontset=fandol,11pt", pre=GEO,
          nsec=24, chapters=6, cjk=True, feats={"fig": 4}, tags=["ctexbook", "xelatex", "ctex", "CJK"],
          front="\\title{市场与估计}\n\\author{作者}\n\\date{2026年1月1日}\n\\begin{document}\n\\maketitle\n\\tableofcontents\n", tail_para=3)

    build("xe_scrbook_fontspec", "xe", cls="scrbook", opts="11pt,a4paper", pre=font_preamble("bonum", math=False) + "\n\\usepackage{amsmath}",
          nsec=48, chapters=8, feats={"math": 3, "tab": 5, "fig": 6}, tags=["scrbook", "KOMA", "xelatex", "fontspec"], tail_para=6)
    build("xe_report_tables", "xe", cls="report", opts="11pt", pre=GEO + "\n" + font_preamble("termes", math=False) + "\n\\usepackage{longtable}",
          nsec=12, chapters=4, feats={"long": 1, "tabx": 2}, tags=["report", "xelatex", "longtable", "tabularx"], tail_para=3)
    build("xe_natbib_bibtex", "xe", pre=GEO + "\n" + font_preamble("pagella", math=False) + "\n\\usepackage[authoryear,round]{natbib}",
          nsec=14, feats={"tab": 4}, bibmode=("natbib", "plainnat"), citecmds=("citep", "citet"), tags=["article", "xelatex", "natbib", "bibtex"])
    build("xe_float_heavy", "xe", pre=GEO + "\n" + font_preamble("termes", math=False) + "\n\\usepackage{subcaption,booktabs}",
          nsec=16, feats={"fig": 1, "sub": 1, "tab": 2}, tags=["article", "xelatex", "float-heavy", "subcaption"], tail_para=2)

    # ------------------------------------------------------------------ LuaLaTeX
    # (document bodies avoid \begin{luacode}: see PERFORMANCE.md; lua_luacode_env keeps it as a known case)
    for fam in ("termes", "pagella"):
        build(f"lua_fontspec_{fam}", "lua", pre=GEO + "\n\\usepackage{amsmath}\n" + font_preamble(fam), nsec=25 if fam == "termes" else 7,
              feats={"math": 2, "tab": 4}, tags=["article", "lualatex", "fontspec", "unicode-math", fam])
    build("lua_lm_fontspec", "lua", pre=GEO + "\n" + LM_FONTS, nsec=8, feats={"tab": 3, "foot": 4}, tags=["article", "lualatex", "fontspec", "latin-modern"])
    build("lua_directlua", "lua", pre=GEO + "\n" + font_preamble("termes", math=False) + "\n\\directlua{function fibn(n) local a,b=0,1 for i=1,n do a,b=b,a+b end return a end}",
          nsec=8, feats={}, tags=["article", "lualatex", "directlua"],
          tail="Fibonacci via Lua: \\directlua{for i=1,40 do tex.sprint(fibn(i)..', ') end}.\n\n")
    build("lua_luacode_env", "lua", pre=GEO + "\n" + font_preamble("termes", math=False) + "\n\\usepackage{luacode}",
          nsec=6, feats={}, tags=["article", "lualatex", "luacode"], note="uses \\begin{luacode}; TeXres 0.7.2 failed on it",
          tail="\\begin{luacode}\nfor i=1,10 do tex.sprint(i*i..' ') end\n\\end{luacode}\n\n")
    build("lua_polyglossia_german", "lua", pre=GEO + "\n" + font_preamble("termes", math=False) + "\n\\usepackage{polyglossia}\n\\setmainlanguage{german}",
          nsec=7, lang="german", feats={"tab": 3}, tags=["article", "lualatex", "polyglossia", "german"])
    build("lua_polyglossia_french", "lua", pre=GEO + "\n" + font_preamble("pagella", math=False) + "\n\\usepackage{polyglossia}\n\\setmainlanguage{french}",
          nsec=7, lang="french", feats={"foot": 3}, tags=["article", "lualatex", "polyglossia", "french"])
    build("lua_biblatex", "lua", pre=GEO + "\n" + font_preamble("termes", math=False) + "\n\\usepackage[backend=biber,style=numeric]{biblatex}\n\\addbibresource{refs.bib}",
          nsec=8, feats={"math": 3}, bibmode=("biblatex",), tags=["article", "lualatex", "biblatex", "biber"])
    build("lua_tikz_pgfplots", "lua", pre=GEO + "\n" + font_preamble("pagella", math=False) + "\n\\usepackage{tikz,pgfplots}\n\\pgfplotsset{compat=1.18}",
          nsec=7, feats={"tikz": 2, "plot": 1}, tags=["article", "lualatex", "tikz", "pgfplots"], tail_para=3)
    build("lua_beamer", "lua", cls="beamer", opts="aspectratio=169",
          pre="\\usetheme{CambridgeUS}\n" + font_preamble("termes", math=False),
          front="\\title{Beamer under LuaLaTeX}\n\\author{A. Author}\n\\date{1 January 2026}\n\\begin{document}\n\\begin{frame}\\titlepage\\end{frame}\n",
          nsec=0, tags=["beamer", "lualatex", "fontspec"],
          tail="\n".join(
              rf"\begin{{frame}}{{{Gen(k).phrase().title()}}}{chr(10)}{Gen(k + 200).para(3)}"
              + (("\n" + MARK) if k == 10 else "") + rf"{chr(10)}\end{{frame}}" for k in range(1, 21)) + "\n")
    build("lua_book", "lua", cls="book", opts="11pt", pre="\\usepackage{amsmath}\n" + font_preamble("pagella", math=False),
          nsec=36, chapters=9, feats={"math": 3, "fig": 5, "tab": 6}, tags=["book", "lualatex", "fontspec"], tail_para=7)
    build("lua_microtype", "lua", cls="article", opts="10pt,twocolumn",
          pre="\\usepackage[margin=.8in]{geometry}\n" + font_preamble("termes", math=False) + "\n\\usepackage[protrusion=true,expansion=true]{microtype}",
          nsec=100, feats={}, tags=["article", "lualatex", "microtype", "twocolumn"], tail_para=6)
    build("lua_siunitx", "lua", pre=GEO + "\n" + font_preamble("bonum", math=False) + "\n\\usepackage{siunitx,booktabs}",
          nsec=7, feats={"si": 1, "siunit": 1}, tags=["article", "lualatex", "siunitx"], tail_para=3)
    build("lua_scrartcl_unicode", "lua", cls="scrartcl", opts="11pt,a4paper", pre=font_preamble("schola"),
          nsec=9, feats={"math": 2, "sub2": 2}, tags=["scrartcl", "KOMA", "lualatex", "unicode-math"])
    build("lua_polyglossia_russian", "lua", pre=GEO + "\n" + CMU_FONTS + "\n\\usepackage{polyglossia}\n\\setmainlanguage{russian}\n\\newfontfamily\\cyrillicfont{cmunrm.otf}[BoldFont=cmunbx.otf,ItalicFont=cmunti.otf,BoldItalicFont=cmunbi.otf]\n\\newfontfamily\\cyrillicfontsf{cmunss.otf}",
          nsec=7, lang="russian", feats={"tab": 3}, tags=["article", "lualatex", "polyglossia", "russian"])


def main():
    global OUT, SEED_OFFSET
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=Path, default=OUT, help="output directory (default: scripts/bench/corpus100)")
    ap.add_argument("--seed-offset", type=int, default=0,
                    help="shift every random seed: same document types and packages, different text and data "
                         "(scripts/build_pgo.sh trains on such a variant, never on the measured documents)")
    args = ap.parse_args()
    OUT, SEED_OFFSET = args.out, args.seed_offset
    define_docs()
    names = sorted(MANIFEST)
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "manifest.json").write_text(json.dumps({n: MANIFEST[n] for n in names}, indent=1) + "\n")
    eng = {}
    for n in names:
        eng[MANIFEST[n]["engine"]] = eng.get(MANIFEST[n]["engine"], 0) + 1
    print(len(names), "documents", eng)


if __name__ == "__main__":
    main()
