=====================================================================
The Pazo Math fonts
for mathematical typesetting with the Palatino fonts

                           Font outlines version 1.003, 17 May 2002
                           by Diego Puga <d.puga@utoronto.ca>

                           For use with PSNFSS version 9
=====================================================================

The Pazo Math fonts are a family of PostScript fonts suitable for
typesetting math in combination with the Palatino family of text
fonts.  They are required by the LaTeX macro package mathpazo.sty, now
part of the PSNFSS bundle.  The mathpazo.sty package defines the
Palatino family as the default roman font and uses the virtual
mathpazo fonts, built around the Pazo Math family, for typesetting
math in a style that suits Palatino.

The Pazo Math family is made up of five fonts provided in Type1 format
(PazoMath, PazoMath-Italic, PazoMath-Bold, PazoMath-BoldItalic, and
PazoMathBlackboardBold).  These contain, in designs that suit
Palatino, glyphs that are usually not available in Palatino and for
which Computer Modern looks odd when combined with Palatino.  These
glyphs include the uppercase Greek alphabet in upright and slanted
shapes in regular and bold weights, the lowercase Greek alphabet in
slanted shape in regular and bold weights, several math glyphs
(partialdiff, summation, product, coproduct, emptyset, infinity, and
proportional) in regular and bold weights, other glyphs (Euro and
dotlessj) in upright and slanted shapes in regular and bold weights,
and the complete uppercase latin alphabet plus number one in
blackboard bold.

The current distribution comprises the printer font binary (pfb) files
with the PostScript outlines, the Adobe font metrics (afm) files, and
the font information (inf) files for the Pazo Math family.

The TeX font metrics (tfm) files, the LaTeX macro package
mathpazo.sty, and a set of virtual math fonts that combine glyphs from
Pazo Math, Computer Modern, and Palatino to form complete math fonts
for use with TeX/LaTeX are part of the PSNFSS bundle version 9.

For those wishing to rebuild or modify the virtual fonts, the fontinst
sources of the mathpazo virtual fonts are still available with this
distribution.


Installation and usage
======================

Please see the documentation included with the PSNFSS bundle for
installation and usage intructions.  The following notes pertain files
not required by PSNFSS.

The directory inf/ contains font information files for the Pazo
Math family.  You need these files only in case you want to
install the Pazo Math fonts in Adobe Type Manager for use with
other applications (ATM will then combine afm and inf files into
pfm files).

The directory fontinst/ contains the fontinst sources of the virtual
mathpazo fonts.  You need these files only in case you want to rebuild
or modify the fonts.  If you want to rebuild the fonts, you will also
need the Adobe font metrics for the Palatino fonts (you can download
these files from
ftp://ftp.adobe.com/pub/adobe/type/win/all/afmfiles/001-050/001/ and
then rename them with the Karl Berry names: rename pob_____.afm to
pplb8a.afm, pobi____.afm to pplbi8a.afm, poi_____.afm to pplri8a.afm,
por_____.afm to pplr8a.afm). The fontinst scripts require fontinst
version 1.914 because they rely on features available only starting
from that version.


Acknowledgements
================

The fontinst scripts for the mathpazo fonts are based on scripts by
Walter Schmidt for the mathpple fonts.  Those in turn are based on
fontinst scripts written by Aloysius Helminck and distributed in
conjunction with fontinst v1.335.  The package mathpazo.sty also
builds on Walter Schmidt's mathpple.sty and has many similarities with
it.  Walter has subsequently improved mathpazo.sty and included it in
the PSNFSS bundle.  I am grateful to him for writing mathpple.sty and
for making the fontinst sources available, as well as for his many
improvements to the mathpazo.sty package.  Alan Hoenig and Martin
Osborne kindly tested an early version of the fonts.  Martin also
provided many helpful comments on the design of several glyphs. George
White suggested ending the encoding vector by `readonly def' instead
of `def' to avoid an error with versions of pdftex prior to 0.14g.
Franz Achermann reported a bad /BBox error in Acrobat with some pdf
files created with pdftex, which has now been resolved.  Pablo
Rodríguez pointed out the unusual shape of \varigma in the first
version.  Marco Kuhlmann pointed out a couple of inconsistencies in
the positioning of the double strokes in Pazo Math Blackboard Bold.
The font outlines for the Pazo Math family were designed and
hinted using FontLab (http://www.fontlab.com). Of course, NONE of the
glyphs in the Pazo Math fonts were derived from Adobe's or Linotype's
Palatino or from any other commercial font.  To maintain a family
look, some glyphs (particularly those similar to standard text glyphs,
such as the blackboard bold uppercase letters or the dotless j) were
derived from a version of URWPalladioL distributed under the GNU
General Public License. All trademarks mentioned are the property of
their respective owners.


Changes to the Type 1 fonts
===========================

Version 1.003
-------------

* Changed the positioning of the double stroke in X and Z in Pazo Math
  Blackboard Bold (suggested by Marco Kuhlmann).

Version 1.002
-------------

* Added all the remaining uppercase latin letters and number one to
  Pazo Math Blackboard Bold.

* Re-designed \Phi to make it better proportioned (suggestion
  passed on by Walter Schmidt).

* Re-designed \varsigma to give it a more recognisable shape
  (suggested by Pablo Rodríguez). Re-designed \xi and \zeta to match
  the new \varsigma.

Version 1.001
-------------

* Changed the end of the encoding vector in the .pfb files from
  `def' to `readonly def' to avoid an error with versions of pdftex
  prior to 0.14g (suggested by George White).

* Eliminated the space after `/FontBBox {' in the .pfb files
	to avoid a bad /BBox error in Acrobat with some pdf files created
	with pdftex (problem reported by Franz Achermann).

Version 1.000
-------------

* First public release.


License
=======

The Pazo Math PostScript fonts (files fplmb.afm, fplmb.inf, fplmb.pfb,
fplmbb.afm, fplmbb.inf, fplmbb.pfb, fplmbi.afm, fplmbi.inf,
fplmbi.pfb, fplmr.afm, fplmr.inf, fplmr.pfb, fplmri.afm, fplmri.inf,
and fplmri.pfb) are distributed under the GNU General Public License
(http://www.gnu.org/copyleft/gpl.txt). As a special exception,
permission is granted to include these font programs in a PostScript
or PDF file that consists of a document that contains text to be
displayed or printed using these fonts, regardless of the conditions
or license applying to the document itself.  This program is
distributed in the hope that it will be useful, but without any
warranty; without even the implied warranty of merchantability or
fitness for a particular purpose.
