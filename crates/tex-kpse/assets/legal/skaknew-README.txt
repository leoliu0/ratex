This directory contains enhanced type-1 versions of the popular skak
fonts (for diagrams and figurine notation). Plus a special version of
Eric Bentzen's ChessAlpha font for typesetting with the packages
'chessboard' and 'chessfss' by Ulrike Fischer.

Copy
*.pfb to texmf/fonts/type1
*.tfm to texmf/fonts/tfm
*.map to texmf/dvips/config
or where your system expect to find them.

Package contents
=======================================================================
README                   this file
SkakNew-Diagram.afm      Adobe font metrics file used to generate tfm's
SkakNew-Diagram.inf      font information file
SkakNew-Diagram.pfb      printer font binary file
SkakNew-Diagram.pfm      printer font metrics file
SkakNew-Diagram.tfm      TeX font metrics file
SkakNew-DiagramT.afm     Adobe font metrics file used to generate tfm's
SkakNew-DiagramT.inf     font information file
SkakNew-DiagramT.pfb     printer font binary file
SkakNew-DiagramT.pfm     printer font metrics file
SkakNew-DiagramT.tfm     TeX font metrics file
SkakNew-Figurine.afm     Adobe font metrics file
SkakNew-Figurine.inf     font information file
SkakNew-Figurine.pfb     printer font binary file
SkakNew-Figurine.pfm     printer font metrics file
SkakNew-Figurine.tfm     TeX font metrics file
SkakNew-FigurineBold.afm Adobe font metrics file
SkakNew-FigurineBold.inf font information file
SkakNew-FigurineBold.pfb printer font binary file
SkakNew-FigurineBold.pfm printer font metrics file
SkakNew-FigurineBold.tfm TeX font metrics file
AlphaDia.afm             Adobe font metrics file used to generate tfm's
AlphaDia.inf             font information file
AlphaDia.pfb             printer font binary file
AlphaDia.pfm             printer font metrics file
AlphaDia.tfm             TeX font metrics file

SkakNew.map              font mapping file for pdftex or dvips
SkakNew.pdf              documentation/installation guide
SkakNew.tex              documentation/installation guide source

fonttables.pdf           Font table showing all chars

install.vtex             installation instructions for VTeX
SkakNew.ali              needed by VTeX

Please read install.vtex for installation instructions for VTeX.
The files have been made available by Walter Schmidt.

Changes:
========
February 2009
-------------
New versions of all fonts with small corrections. OpenType versions.

January 2008
------------
Added AlphaDia.*
New versions of Skak-Diagram*.* (technical clean-up)

May 2006
--------
This is version 1.3 of SkakNew-Diagram and SkakNew-DiagramT. I have
added some character 'masks' for use with the new version of chessfss
and chessboard by Ulrike Fischer. Their main task is for separately
colouring borders, fields, character outlines, and character inlines.
For more information see the documentation of the resp. styles. PDFs
showing the font layout are supplied.
