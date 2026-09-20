With the exception of the PostScript fonts, this material is subject to the LaTeX Project Public License. See http://www.ctan.org/help/Catalogue/licenses.lppl.html for the details of that license.

The PostScript fonts in this package were derived from the STIX OpenType collection, with regular and bold weights of calligraphic, fraktur and double-struck (aka blackboard bold.) As such, they must be released under the SIL OPEN FONT LICENSE Version 1.1, whose details may be viewed in the file

STIX_Font_License_2010.pdf

distributed with the STIX fonts.

The package contains virtual fonts based on the PostScript fonts with metrics adjusted to suit my taste in width, accent position and subscript position. There are also less oblique variants of the calligraphic faces. The current version, 1.01, dated 2015-06-14, corrects a flaw in uboondox-cal.fd, which now once again loads as expected.

Current version 1.02d 2017-02-25

Changes in version 1.02d
Corrected date format in sty files.

Changes in version 1.02c
Corrected BOONDOX-uprscr.sty

Changes in version 1.02b
Corrected another error in uboondoxuprscr.fd.

Changes in version 1.02a
Corrected an error in uboondoxuprscr.fd.

Changes in version 1.02
Added a new font, called by BOONDOX-uprscr.sty, which takes the glyphs from STIX-MathScr-*.pfb, unslants them and reshapes the outlines where needed.  

Changes in version 1.01
Added the newer version of the bold double-struck STIX fonts (version 1.1) so that BOONDOX-b-ds now has full alphabetic coverage.

Install the TDS in the usual way. For example, if you have administrative privileges under OS X, cd to the top level of the downloaded folder (where you see subfolders doc, fonts, tex) and type

sudo cp -r * /usr/local/texlive/texmf-local
sudo mktexlsr
sudo -H updmap-sys --enable Map=boondox.map

after which you should have access to the boondox alphabets.

With MikTeX, copy boondox.zip to the root of the folder you use for personal additions to the TeX trees, open a command window, chdir to that folder, and type

unzip boondox
initexmf --update-fndb
initexmf --edit-config-file updmap

The latter command should open updmap.cfg in your default editor, commonly Notepad. Add the line 

Map boondox.map 

to updmap.cfg, save and close. Then, in the command window, type

initexmf --mkmaps

to complete the installation.

Michael Sharpe
msharpe at ucsd dot edu
