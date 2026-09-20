This is the README for the ebgaramond package, version 
2024-04-23.

This package provides LaTeX, pdfLaTeX, XeLaTeX and LuaLaTeX
support for the EB Garamond family of fonts, designed by
Georg Duffner (Regular and Italic) and Octavio Pardo (Bold,
Semibold and Extrabold).

EB Garamond is a revival of the 16th century fonts designed
by Claude Garamont. The source for the letterforms is
a scan of a specimen known as the "Berner specimen",
which, composed in 1592 by Conrad Berner, son-in-law of
Christian Egenolff and his successor at the Egenolff
print office, shows Garamont's roman and Granjon's italic
fonts at different sizes. Hence the name of this project:
Egenolff-Berner Garamond.

To install this package on a TDS-compliant
TeX system download the file
"tex-archive"/install/fonts/ebgaramond.tds.zip, where the
preferred URL for "tex-archive" is http://mirror.ctan.org.
Unzip the archive at the root of an appropriate texmf tree,
likely a personal or local tree. If necessary, update the
file-name database (e.g., texhash). Update the font-map
files by enabling the Map file EBGaramond.map.

To use, add

\usepackage{ebgaramond}

to the preamble of your document. Options include:

oldstyle,osf    old-style figures
lining,nf,lf    lining figures

proportional,p  varying-width figures
tabular,t       fixed-width figures

The defaults are oldstyle and proportional.

m, medium       medium weight
sb, semibold    semibold weight
eb, extrabold   extrabold weight

The default weights are regular and bold.

Available shapes include:

it              italic
sc              small caps
scit            italic small caps

Slanted variants are not supported; the italic variants will
be automatically substituted. Font encodings supported are
OT1, T1, TS1, and LY1. 

Options scaled=<number> or scale=<number> may be used to
adjust fontsizes. The type1 option may be used by xelatex
or lualatex users who prefer to use type1 fonts or to avoid
fontspec.

Commands \oldstylenums{...} and \liningnums{...} are defined
to allow for local use of old-style figures or lining
figures, respectively. Similarly, \tabularnums{...} and
\proportionalnums{...} allow for local use of monospaced or
varying-width figures, respectively.

Superior numbers (for footnote markers) are available
using \sufigures or \textsu{...}. Inferior numbers (for
subscripts) are available using \infigures or \textinf{...}.
Swash italic glyphs (for some letters) are available using
\swshape or \textsw{...}.

Command \textin{...} produces decorative initials; currently
only the following glyphs are available: A, D, F, G, L, N,
O, Q, T and X.

Command \ebgaramond supports local use of the EB Garamond 
fonts. 

The original fonts are available from
https://github.com/octaviopardo/EBGaramond12/tree/master/fonts/otf 
and are licensed under the SIL Open Font License, (version
1.1); the text may be found in the doc directory. Font
charts may be found in the doc directory.

The type1 versions were created by cfftot1. The support
files were created using autoinst and otftotfm (version
2.108) and are licensed under the terms of the LaTeX Project
Public License. The maintainer of this package is Bob
Tennent (rdt at cs.queensu.ca)
