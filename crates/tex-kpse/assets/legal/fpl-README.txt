======================================================================

                FPL Fonts -- SC/OsF for URW Palladio L

======================================================================
                                                            2018-12-11
                                                          Ralf Stubner 
                                                 <ralf.stubner@web.de>

The FPL Fonts consist of the following four fonts:

fplrc8a.pfb   TeXPalladioL-SC
fplrij8a.pfb  TeXPalladioL-ItalicOsF
fplbj8a.pfb   TeXPalladioL-BoldOsF
fplbij8a.pfb  TeXPalladioL-BoldItalicOsF

The FPL Fonts provide a set of SC/OsF fonts for URW Palladio L which are
compatible with respect to metrics with the Palatino SC/OsF fonts from
Adobe. Note that it is not my aim to exactly reproduce the outlines of
the original Adobe fonts. The SC and OsF in the FPL Fonts were designed
with the glyphs from URW Palladio L as starting point. For some glyphs
(eg 'o') I got the best result by scaling and boldening. For others (eg
'h') shifting selected portions of the character gave more satisfying
results. All this was done using the free font editor FontForge
<URL:https://fontforge.github.io/>. The kerning data in these fonts comes
from Walter Schmidt's improved Palatino metrics. Please report any issues
at <URL:https://github.com/rstub/fpl/issues>.


History
-------

Version 1.003 2018-12-11
        * Regenerate the fonts with current FontForge resolving
	  PDF/A compatibility problem.
	* Use the same dual licensing as the original URW fonts, c.f.
          <URL:http://www.tug.org/fonts/lppl-urw.txt>

Version 1.002 2005-02-13 
        * no div operator (compatibility problems)

Version 1.001 2005-02-05
        * fixed lcaron in SC font
        * no references (compatibility problems)
        * no UniqueID
        * OtherSubrs from URW fonts

Version 1.000 2004-08-02
	First official release


Deficiencies
------------

On the character level only automatic hinting as done by FontForge has
been applied to the SC and OsF glyphs.


Usage with LaTeX
----------------

As the FPL Fonts have the same metrics as their Adobe counterparts, the
improved Palatino metrics by Walter Schmidt can be used. They are part
of PSNFSS 9.2, which can be found on CTAN:macros/latex/required/psnfss/.
Therefore, it is best to update your TeX distribution to a version which
provides PSNFSS 9.2. The usage with LaTeX is documented in psnfss2e.pdf.
Please follow the installation instruction from PSNFSS, if you want to
upgrade PSNFSS only. 


License
-------

Copyright (URW)++,Copyright 1999 by (URW)++ Design & Development
Copyright 2004, 2005 Ralf Stubner

See the file COPYING (GNU General Public License) for license
conditions. As a special exception, permission is granted to include
this font program in a Postscript or PDF file that consists of a
document that contains text to be displayed or printed using this font,
regardless of the conditions or license applying to the document itself.

Alternativley you can use these fonts subject to the LaTeX Project Public
License <URL:http://www.latex-project.org/lppl>, either version 1 or (at
your option) any later version.

== The End ==
