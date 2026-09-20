Haralambous Broken Typefaces PostScript Fonts
(Adobe Type 1 format)

Copyright (c) 2002 Torsten Bronger, torsten.bronger@gmx.de
2002/05/23 Ver. 1.0


This PostScript Type 1 implementation of the fonts yfrak, yswab and
ygoth, originally created by Yannis Haralambous using METAFONT, is
freely available for general use.

The conversion was done with textrace, finetuning and hinting with
pfaedit.

This distribution does not contain the TFM files that are necessary to
use the fonts with TeX; the TFM files can be obtained from CTAN, in
fonts/gothic/{yfrak,yswab,ygoth}, where you also can find the Metafont
sources of the original glyphs.

Notice that the AFM files included in this distribution contain no
ligature information, therefore they cannot be used to generate proper
TFM files.

The fragment for the dvips mapfile is
yfrak yfrak-Regular < yfrak.pfb
yswab yswab-Regular < yswab.pfb
ygoth ygoth-Regular < ygoth.pfb


Although the PFB files may still need some finetuning, the font
metrics are frozen since 1990 and thus the use with TeX is totally
unaffected by future improvements of the PFB files.
