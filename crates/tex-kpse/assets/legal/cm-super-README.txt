=========================================================================
                     CM-Super font package
                     version 0.3.4 (July 9, 2008)

  Copyright (c) 2001, 2002, 2008 Vladimir Volovich <vvv@vsu.ru>.

  This package is free software; you can redistribute it and/or modify
  it under the terms of the GNU General Public License as published by
  the Free Software Foundation; either version 2 of the License, or
  (at your option) any later version.

  As a special exception, permission is granted to include these font
  programs in a Postscript or PDF file that consists of a document that
  contains text to be displayed or printed using these fonts, regardless
  of the conditions or license applying to the document itself.

  This package is distributed in the hope that it will be useful,
  but WITHOUT ANY WARRANTY; without even the implied warranty of
  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
  GNU General Public License for more details.

  You should have received a copy of the GNU General Public License
  along with this program; if not, write to the Free Software
  Foundation, Inc., 59 Temple Place, Suite 330, Boston, MA  02111-1307  USA

The CM-Super package contains Type 1 fonts converted from METAFONT
fonts and covers entire EC/TC, EC Concrete, EC Bright and LH fonts
(Computer Modern font families). All European and Cyrillic writings
are covered. Each Type 1 font program contains ALL glyphs from the
following standard LaTeX font encodings: T1, TS1, T2A, T2B, T2C, X2,
and also Adobe StandardEncoding (585 glyphs per non-SC font and 468
glyphs per SC font), and could be reencoded to any of these encodings
using standard dvips or pdftex facilities (the corresponding support
files are also included).

See the file INSTALL for installation instructions for teTeX, TeX Live,
MikTeX or VTeX/Free systems.

See the file FAQ for answers to some frequently asked questions.

PLEASE read the files README, INSTALL, and FAQ carefully before
reporting problems.

The goal was to provide full support for a wide number of fonts used
in LaTeX, and at the same time minimize the number and total size of
font files. Each Type 1 font shares all common glyphs which are present
in supported encodings. E.g. Latin letters, Russian letters, accents
and a number of other glyphs which are present in more than one font
encoding are stored only once in Type 1 fonts.

These Type 1 fonts could be used as a drop-in replacement for LaTeX's
EC/TC, EC Concrete, EC Bright and LH fonts to create Postscript and
PDF documents with vector fonts (if you used METAFONT versions of
EC/TC, EC Concrete, EC Bright or LH fonts, no changes to your LaTeX
documents are required to use these Type 1 fonts once you have them
installed). You will still need the METAFONT sources of these fonts to
be able to generate needed TFM metric files (once you have them
generated, METAFONT will not be used). You could also prepare EPS or
PDF illustrations in your favorite applications with texts written
using these fonts, and then include them in your LaTeX documents.
These fonts could be used not only in TeX-related applications, but
also as all other Type 1 text fonts, e.g. in Windows (and you will be
able to typeset multilingual texts using them).

Font file names correspond to the scheme used in EC fonts, with the
first two letters "sf" ("super font") instead of "ec".  E.g., sfrm1000.pfb
contains all glyphs from:
ecrm1000, tcrm1000, larm1000, lbrm1000, lcrm1000, rxrm1000.

The third and fourth letters in the font name correspond to font shape.
The list of provided font shapes is included below:

  rm:  Computer Modern Roman
  sl:  Computer Modern Slanted
  ti:  Computer Modern Italic
  cc:  Computer Modern Caps and Small Caps
  ui:  Computer Modern Unslanted Italic
  sc:  Computer Modern Slanted Caps and Small Caps
  ci:  Computer Modern Classical Serif Italic
  bx:  Computer Modern Bold Extended
  bl:  Computer Modern Bold Extended Slanted
  bi:  Computer Modern Bold Extended Italic
  xc:  Computer Modern Bold Extended Caps and Small Caps
  oc:  Computer Modern Bold Extended Slanted Caps and Small Caps
  rb:  Computer Modern Roman Bold
  bm:  Computer Modern Roman Bold Variant
  ss:  Computer Modern Sans Serif
  si:  Computer Modern Sans Serif Slanted
  sx:  Computer Modern Sans Serif Bold Extended
  so:  Computer Modern Sans Serif Bold Extended Slanted
  tt:  Computer Modern Typewriter
  st:  Computer Modern Typewriter Slanted
  it:  Computer Modern Typewriter Italic
  tc:  Computer Modern Typewriter Caps and Small Caps
  vt:  Computer Modern Variable Width Typewriter
  vi:  Computer Modern Variable Width Typewriter Italic
  dh:  Computer Modern Dunhill Roman
  fb:  Computer Modern Fibonacci Medium
  fs:  Computer Modern Fibonacci Slanted
  ff:  Computer Modern Funny Roman
  fi:  Computer Modern Funny Italic

Each font shape comes in 14 font sizes ranging from 5pt to 35.83pt (or
11 font sizes for typewriter fonts ranging from 8pt to 35.83pt).

Also, the following 13 one-sized font shapes are included:

  sflq8:     Computer Modern SliTeX Sans Serif Quotation
  sfli8:     Computer Modern SliTeX Sans Serif Quotation Inclined
  sflb8:     Computer Modern SliTeX Sans Serif Quotation Bold
  sflo8:     Computer Modern SliTeX Sans Serif Quotation Bold Oblique
  sfltt8:    Computer Modern LaTeX Typewriter
  isflq8:    Computer Modern SliTeX Sans Serif Quotation Invisible
  isfli8:    Computer Modern SliTeX Sans Serif Quotation Inclined Invisible
  isflb8:    Computer Modern SliTeX Sans Serif Quotation Bold Invisible
  isflo8:    Computer Modern SliTeX Sans Serif Quotation Bold Oblique Invisible
  isfltt8:   Computer Modern LaTeX Typewriter Invisible
  sfsq8:     Computer Modern Sans Serif Quotation
  sfqi8:     Computer Modern Sans Serif Quotation Inclined
  sfssdc10:  Computer Modern Sans Serif Demi Condensed

Also, the following 14 fonts from Computer Modern Concrete family are
included (font file names correspond to the scheme used in EC Concrete
fonts):

  sform5 .. sform10:  Computer Modern Concrete Roman
  sfosl5 .. sfosl10:  Computer Modern Concrete Slanted
  sfoti10:            Computer Modern Concrete Italic
  sfocc10:            Computer Modern Concrete Caps and Small Caps

Also, the following 19 fonts from Computer Modern Bright family are
included (font file names correspond to the scheme used in European
Computer Modern Bright fonts):

  sfbmr{8,9,10,17}:  Computer Modern Bright Roman
  sfbmo{8,9,10,17}:  Computer Modern Bright Oblique
  sfbsr{8,9,10,17}:  Computer Modern Bright Semibold
  sfbso{8,9,10,17}:  Computer Modern Bright Semibold Oblique
  sfbbx10:           Computer Modern Bright Bold Extended
  sfbtl10:           Computer Modern Typewriter Light
  sfbto10:           Computer Modern Typewriter Light Oblique

Fonts were created using TeXtrace (based on AutoTrace and
Ghostscript), t1utils and a bunch of Perl scripts, and were optimized
and hinted using FontLab 3.1. The set of UniqueID values was
registered at Adobe. We use AGL compliant glyph names when possible
(there are some glyphs which are neither present in AGL nor in
Unicode).

It should also be noted that the fonts use precise (non-integer) glyph
widths which better match the TFM widths than just rounding to the
nearest integer. These widths are generated using the best
approximation (based on continued fractions) with the denominator not
exceeding 107 to fit in 1 byte in CharString.  Apparently, such subtle
technique was used first in BSR/Y&Y CM fonts.

I'd like to thank Peter Szabo for TeXtrace, Martin Weber for
AutoTrace, and FontLab Ltd. for providing a copy of FontLab.

It should be noted that while creating these fonts we intentionally
and on principle used only automatic methods which do not require font
designers talents. The aim was to use TOTALLY automatic conversion of
METAFONT fonts to Type 1 format, automatic optimization and hinting,
with the best achievable quality of final Type 1 fonts, to be able to
re-generate the fonts if necessary (e.g. when a new version of
original METAFONT fonts will be released).  Undoubtedly, there are
fields for improvement of this approach, which we will use in future
versions of the fonts, but even now the fonts seem to look and print
quite good (we hope :-).

It appears that careless approach to FontLab's optimization and
auto-hinting facilities could lead to loss of quality of the original
font (some glyph shapes could be broken), so we used the most precise
optimization, and hope that optimized and hinted fonts are indeed
better than original traced fonts (also, they are significantly
smaller in size). So far, we did not find any bugs in optimized fonts.

There are 434 Type 1 outline fonts (*.pfb) in the CM-Super font set,
and they cover 2536 TeX fonts!

Note that a small number of (fortunately, rarely used) fonts are not
included yet because of the bugs in EC font drivers which prevented
their generation. We plan to provide these missing fonts soon.
(Currently there are 25 missing Type 1 font files.)

The CM-Super package could be freely downloaded from
CTAN:fonts/ps-type1/cm-super
and also from
ftp://ftp.vsu.ru/pub/tex/font-packs/cm-super/
The total size of all PFB files is about 57 Mb
(AFM and INF files are provided, too, for use with non-TeX applications).

Although this package is provided AS IS, WITHOUT WARRANTY (as
explained in the file COPYING), we welcome your comments and bug
reports which should be sent to the email address given above.

Happy TeXing!
=========================================================================
