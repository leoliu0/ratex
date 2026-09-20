MnSymbol is a symbol font designed to be used in conjunction with Adobe
MinionPro.

All files in this archive are in the public domain.

WARNING
=======

This version includes a bugfix that causes incompatibilities with
previous versions. The symbols

  \...harpoonup and \...harpoondown

are now called

  \...harpoonccw and \...harpooncw

(This was needed for compatibility with amsfonts.sty.)

INSTALLATION
============

To install the fonts:

(0) If you have already a version of MnSymbol installed, delete the old
TFM and PK files.

(1) Run

  $ cd tex
  $ latex MnSymbol.ins

and copy the resulting file MnSymbol.sty to

   .../texmf/tex/latex/MnSymbol/MnSymbol.sty

(2) Copy all files in the "source" directory into the directory

   .../texmf/fonts/source/public/MnSymbol/

(3) Copy the documentation (MnSymbol.ps, MnSymbol.pdf, README) into the
directory

   .../texmf/doc/latex/MnSymbol/

(4) To install the PostScript fonts (courtesy of Michael Zedler) copy

  o enc/MnSymbol.map into .../texmf/fonts/map/dvips/MnSymbol
  o enc/*.enc        into .../texmf/fonts/enc/dvips/MnSymbol
  o pfb/*.pfb        into .../texmf/fonts/type1/public/MnSymbol

(These fonts are not strictly necessary but recommended.)

(5) You can copy the files in the "tfm" directory to

   .../texmf/fonts/tfm/public/MnSymbol

This is optional since these files can be generated automatically.
But it saves some time.

(6) Regenerate the file database:

  $ mktexlsr

If you installed the PostScript fonts you also need to run updmap or
updmap-sys to make them available:

  $ updmap-sys --enable MixedMap MnSymbol.map

(7) Happy TeXing.

Achim Blumensath
