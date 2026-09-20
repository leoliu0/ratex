This is the README for the cabin package, version 2023-09-25.

This package provides LaTeX, pdfLaTeX, XeLaTeX and LuaLaTeX support for
the Cabin and Cabin Condensed families of sans serif fonts, designed by 
Pablo Impallari.

Cabin is a humanist sans with four weights and true italics and small
capitals. According to the designer, Cabin was inspired by Edward
Johnston's and Eric Gill's typefaces, with a touch of modernism. Cabin
incorporates modern proportions, optical adjustments, and some elements
of the geometric sans. A compatible condensed family CabinCondensed is
also available.

To install this package on a TDS-compliant TeX system download the
file "tex-archive"/install/fonts/cabin.tds.zip, where the preferred
URL for "tex-archive" is http://mirror.ctan.org. Unzip the archive
at the root of an appropriate texmf tree, likely a personal or local
tree. If necessary, update the file-name database (e.g., texhash).
Update the font-map files by enabling the Map file cabin.map.

To use, add

\usepackage{cabin}

to the preamble of your document. This makes Cabin the default sans
family. To also set Cabin as the main text font, use

\usepackage[sfdefault]{cabin}

LuaLaTeX and xeLaTeX users who might prefer type1 fonts or who wish
to avoid fontspec may use the type1 option.

Other options include:

bold            
semibold        

regular         
medium          

condensed       

Defaults are regular and bold (uncondensed). 

Small caps are available in non-condensed series.

Options scaled=<number> or scale=<number> may be used to adjust
fontsizes to match a serifed font.

Slanted variants are not supported; the designed italic
variants will be automatically substituted. The only figure
style supported is tabular-lining. Font encodings supported
are OT1, T1, TS1, LY1.

Macros \cabin and \cabincondensed select the Cabin and CabinCondensed
font family, respectively.

The original fonts are available at
https://github.com/impallari/Cabin/ and are licensed under
the SIL Open Font License, (version 1.1); the text may be
found in the doc directory. The type1 versions were created
using cfftot1. The support files were created using autoinst
and are licensed under the terms of the LaTeX Project Public
License. The maintainer of this package is Bob Tennent (rdt
at cs.queensu.ca)
