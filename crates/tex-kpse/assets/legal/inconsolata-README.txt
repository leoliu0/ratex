The PostScript fonts in this distribution were derived from Inconsolata-Regular.ttf, v.1.013, and Inconsolata-Bold.ttf, v.1.014, downloaded from googlefonts.com. Both were converted to PostScript format and nine glyphs were added to each, providing upright single and double quotes, a more shapely lower-case l series, and horizontal arrows. As Inconsolata-Regular.ttf is offered under the SIL font licence, so is the derived font Inconsolatazi4-Regular.pfb. Inconsolata-Bold.ttf was released under the Apache 2 licence, and so therefore is the derived font Inconsolatazi4-Bold.pfb.

The TeX font metrics, virtual fonts and font definition files for Inconsolata,
i.e., the other files in the archive Inconsolata.zip, may be distributed and/or modified
under the conditions of the LaTeX Project Public License, either version 1.3 of
this license or (at your option) any later version.  The latest version of this
license is in http://www.latex-project.org/lppl.txt and version 1.3 or later is
part of all distributions of LaTeX version 2003/12/01 or later. Those files and
LaTeX support file zi4.sty are

  Copyright (c) 2013--2019 Michael Sharpe

Files provided "as is", with no warranties.

Current version: 1.121

Changes in version 1.121
Corrected an error in i4-ot1-0.enc. (Thanks, Takuo Watanabe.)

Changes in version 1.12
1. Added standard text ligatures, excepting f-ligatures, to improve rendering of text blocks other than verbatim. 
2. Changed the sty file to be better support the use of Inconsolata and Inconsolata Narrow as limited text fonts.
3. Documentation additions and correction.

Changes in version 1.114
Corrected error in the fd files that did not permit multiple calls to \usefont under some circumstances. (Thanks  Haruhiko Okumura.) 

Changes in version 1.113
Corrected three glyphs that had overlapping strokes.

Changes in version 1.112
Corrected a bug in sty file handling of scale[d] option. (Thanks Isaac Sánchez Barrera.)

Changes in version 1.111
Corrected internal filename for zi4.sty.

Changes in version 1.11
1. Added narrow versions of the Inconsolata fonts, reducing widths by 10%. The narrow versions may be better for printing code.
2. Added a number of options to allow control over hyphenation and word spacing.
3. Some small changes to glyph shapes.
4. Added documentation for the above.

Changes in version 1.10
Fixed names of Stylistic Sets and added to documentation.

Changes in version 1.09
Restored the ss01--ss03 lookup tables that had gone missing.

Changes in version 1.08
Changed "euro" to "Euro" in encoding files, regenerated tfms and map file.

Changes in version 1.071
Modified some metadata in the font files.

Changes in version 1.07
1. Changed glyph name zdot to zdotaccent.
2. Corrected unicode values for Zcaron and zcaron.

Changes in version 1.06
Changed glyph name asciigrave.Var to grave.Var, rightarrow.Var to rightarrow, leftarrow.Var to leftarrow, and modified the encoding files accordingly. The result appears to be that the variant forms now not only look correct when rendered, but copy properly from pdf to text. 

Changes in version 1.05
(1) Modifications to all fd files to remove error that became visible only when used without the sty file.
(2) Added a brief .fontspec file so that, when using the fonts with fontspec, you can write
\fontspec{inconsolata}
to load the basic fontnames.

Changes in version 1.04
Modified all fonts so that glyph names of the variant forms permit correct copying from pdf to ASCII.

Changes in version 1.03
(1) Added ss01, ss02 and ss03 tables to the otf versions, providing options for glyph variants under fontspec.
(2) Corrections to some of the fd files affecting bold weight. 

Changes in version 1.02
(1) The otf versions of the fonts were modified so their names and properties are better-behaved than the original ones. (Thanks to Greg' Ar Tourter for very useful feedback.) 

Changes in version 1.01
(1) Added the ts1 tfms and fd that were inadvertently omitted from version 1.0.
(2) Added inconsolata.sty to doc/fonts/inconsolata. It is essentially a copy of zi4.sty, and should be copied to your personal texmf tree if you wish to retain the old name.
(3) Added the otf versions of the fonts to the distribution.

Installation:

Mac OS X: To install, open a Terminal window and cd to inside the downloaded TDS folder.

Method A (recommended): type

sudo mkdir -p /usr/local/texlive/texmf-local/web2c
sudo cp -Rfp * /usr/local/texlive/texmf-local
sudo echo Map zi4.map >> /usr/local/texlive/texmf-local/web2c/updmap.cfg
sudo mktexlsr
sudo -H updmap-sys

Method B: If you insist on making your font installations in your home folder, potentially making your personal installation become out of sync with updates to TeXLive, use instead

(i) if you checked Update for All Users in TeXLive utility

cp -Rfp * ~/Library/texmf

then

updmap --enable Map=zi4.map

(ii) otherwise

cp -Rfp * ~/Library/texmf

then

sudo updmap-sys --enable Map=zi4.map

Linux: Very similar to Mac OS X.

MikTeX: Copy the Inconsolata-zi4.tds.zip to the root of the folder you use for personal additions to the TeX trees, open a command window, chdir to that folder, and type

unzip Inconsolata-zi4.tds
initexmf --update-fndb
initexmf --edit-config-file updmap

The latter command should open updmap.cfg in your default editor, commonly Notepad. Add the line 

Map zi4.map 

to updmap.cfg, save and close. Then, in the command window, type

initexmf --mkmaps

You can ignore error messages that may occur here. This should result in a properly enabled garamondx.

Please send comments and bug reports or suggestions for improvement to

msharpe at ucsd dot edu