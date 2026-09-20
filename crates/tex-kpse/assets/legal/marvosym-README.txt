MarVoSym is Martin Vogel's Symbols font.  It contains many useful
symbols, especially for office and civil engineering purposes.

This marvosym package contains additional support files for TeX; any
questions or comments about it should go to its current maintainer,
not Martin.

Original TrueType font creator: Martin Vogel
Type 1 derivative, TeX macros, documentation:
    Thomas Henlich (original package)
    Mojca Miklavec (updates; current maintainer)

With special thanks to:
    Karl Berry (many useful comments)
    Heiko Oberdiek (created symbol list)

Contact info, urls: see OFL.txt.

Licensing: fonts under OFL; TeX support files under LPPL.

Installation: for TeX, your TeX distribution should already provide the
package.  If you need to install it yourself, you can download it from
CTAN (http://ctan.org/pkg/marvosym) and then see
http://tug.org/fonts/fontinstall.html.

To install for non-TeX (native) use on Windows, copy the .ttf
into your system font directory or otherwise follow Windows procedures.


File list:

doc/
    fonts/marvosym/
        README                       This file
        FONTLOG.txt                  Detailed information about font
        Makefile                     Makefile
        marvodoc.tex                 Documentation source
        marvodoc.pdf                 Formatted documentation
        marvosym-doc.tex             Symbol list (source)
        marvosym-doc.pdf             Symbol list (reproduction of original documentation)
        OFL.txt                      Licence
        OFL-FAQ.txt                  Licence FAQ

fonts/
    afm/public/marvosym/
        marvosym.afm                 Adobe metric file for marvosym
    map/dvips/marvosym/
        marvosym.map                 Map file for pdftex, dvips, ...
    tfm/public/marvosym/
        umvs.tfm                     TeX font metrics for marvosym.
    truetype/public/marvosym/
        marvosym.ttf                 Original TrueType font
    type1/ public/marvosym/
        marvosym.pfb                 Type 1 derivative

source/
    fonts/marvosym/
        generate_marvosym_derivs.sh  Shell script to generate afm, pfb
                                     and tfm files from ttf
        patch_marvosym_afm.sed       Patch for font description in afm file
        patch_marvosym_pfb.sed       Patch for font description in pfb file

tex/
    latex/marvosym/
        marvosym.sty                 The marvosym style file for LaTeX2e
        umvs.fd                      L2E Font description file for marvosym
