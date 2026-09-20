FdSymbol - A math symbol font
=============================

FdSymbol is a math symbol font, designed as a companion to the Fedra family
by Typotheque, but it might also fit other contemporary typefaces.

The development of this font package is hosted at GitHub:
http://github.com/ummels/fdsymbol

Usage
-----

To use this font in LaTeX, include

    \usepackage{fdsymbol}

in the preamble of your LaTeX document. See the PDF documentation for
the details.

Installation
------------

The following instructions have been written for TeX Live. If you use
a different TeX distribution, please check its documentation on how to
accomplish the required steps.

After extracting the archive into a local texmf tree, run

    texhash

to update the file database, followed by

    updmap-sys --enable Map=fdsymbol.map

to activate the map file `fdsymbol.map`.

If you prefer to install the package into a private texmf tree, replace
`updmap-sys` by `updmap-user`. Before doing so, make sure you have read
https://tug.org/texlive/scripts-sys-user.html for the caveats.

License
-------

Copyright (c) 2011-2025 by Michael Ummels <michael@ummels.de>

The font components of this software, e.g. MetaFont (.mf), TeX font metric
(.tfm), and Type 1 (.pfb) files, are licensed under the SIL Open Font
License, Version 1.1. This license is in the accompanying file OFL.txt,
and is also available with a FAQ at: http://scripts.sil.org/OFL

The LaTeX support files contained in this software may be modified
and distributed under the terms and conditions of the LaTeX Project
Public License, version 1.3c or greater (your choice).
The latest version of this license is in
  http://www.latex-project.org/lppl.txt
and version 1.3 or later is part of all distributions of LaTeX
version 2005/12/01 or later.

This work has the LPPL maintenance status `maintained'.

The Current Maintainer of this work is Michael Ummels.

This work consists of the files fdsymbol.dtx, fdsymbol.ins
and the derived files fdsymbol.pdf and fdsymbol.sty.
