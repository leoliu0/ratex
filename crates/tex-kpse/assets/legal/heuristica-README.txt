With update 1.08 to Heuristica, the package reverts to the otf fonts as distributed by Andrey Panov, version 1.0.2, making available Cyrillic glyphs, Small Caps in regular weight, upright shape, and oldstyle fixed-width figures in all styles, with a few tables added for correct handling by otftotfm and autoinst. The font files in this package are in otf and pfb format, licensed under the SIL OFL. All other support files are subject to the LaTeX Project Public License. See 
http://www.ctan.org/license/lppl1.3
for the details of that license. 

Current version: 1.093 2024-01-30

Changes in version 1.093
Corrected code for option "looser", adding a missing \gdef. (Thanks, Andre Oxygen.)

Changes in version 1.092
Corrected an error in the last correction to the fd file. (Thanks, Frank Mittelbach.)

Changes in version 1.091
Corrected an error in the fd files. The use of \providecommand in the fd could cause problems for users who did not load heuristica.sty.

Changes in version 1.09
Added options to give the user complete control of all word-spacing parameters.

Changes in version 1.08
Reversion to Panov's glyphs, version 1.0.2, so that this and future updates are easier to manage. Those wishing to use the other, newer features (e.g., small caps in all styles) should look to Erewhon, which forks from Heuristica 1.0.1.

Changes in version 1.07
Mainly fixes to bold small caps and their kerning tables.

Changes in version 1.06
Mainly changes in bold small caps, with first attempt at kerning tables for them.

Changes in version 1.05
1. Modified superior letters so that side-bearings match those in Adobe's afms.
2. Added slots for accented small caps in bold, not yet correct glyphs.

Changes in version 1.04
1. Added superior letters to all styles and modified sups lookup table to point to the new glyphs.
2. Added minimal small caps to Heuristica-Bold.

Changes in version 1.03
1. Added a new glyph, uni0365 (superior i), to each upright shape.
2. Created a new sinf lookup for subscript figures.
3. Expanded the sups lookups in upright shapes to include all lowercase letters except q, so that superior letters become available for use in, e.g., French abbreviations.
4. Added a new \textfrac macro that makes of \textsu for the numerator and \textin for the denominator. For example, \textfrac{31}{64}.

Changes in version 1.02
1. Added option "space" to heuristica.sty, so that \usepackage[space=1.25]{heuristica} increases the interword space by 25%. (With \usepackage[space]{heuristica} you get the default value 1.2.)
2. Added a basic heuristica.fontspec so that when using fontspec, you can write
\fontspec{heuristic}
and get the basic default properties.

Changes in version 1.01
Changed the autoinst command as that in version 1.00 proved to be incompatible with font protrusion using the microtype package.
Changes in version 1.001
Fixed map file to refer to correct enc file names.

The otf fonts contain all the kerning and other tables. Should you wish to modify these yourself, the LaTeX support files may be re-created using the command

/usr/texbin/autoinst  --inferiors --noupdmap  --noswash --notitling --noornaments --target=./texmf --encoding=TS1,LY1,T2A,T2B,T2C,T1 --vendor=public --typeface=heuristica *.otf

after cding to the folder containing just the modified otf fonts and a subfolder named texmf. You should replace the newly generated heuristica.sty with the one from the distribution in order to preserve its functionality. The names of the encoding files should also be modified appropriately from those assigned by autoinst. I used the following bash shell script, which assumed you are in the directory holding the otf files and a texmf subfolder:

#!/bin/bash
pushd texmf/fonts/map/dvips/heuristica
sed -i '' 's/<\[a_/<\[zut_/g' Heuristica.map
popd
pushd texmf/fonts/enc/dvips/heuristica
for f in *.enc; do mv ${f} zut${f:1}; done
popd

Installation:

Mac OS X: To install, open a Terminal window and cd to inside the downloaded TDS folder.

Method A (recommended): type

sudo /bin/cp -Rfp * /usr/local/texlive/texmf-local

then

sudo mktexlsr

then

sudo -H updmap-sys --enable Map=Heuristica.map

Method B: If you insist on making your font installations in your home folder, potentially making your personal installation become out of sync with updates to TeXLive, use instead

(i) if you checked Update for All Users in TeXLive utility

/bin/cp -Rfp * ~/Library/texmf

then

updmap --enable Map=Heuristica.map

(ii) otherwise

/bin/cp -Rfp * ~/Library/texmf

then

sudo updmap-sys --enable Map=Heuristica.map

Linux: Very similar to Mac OS X.

MikTeX: Copy the heuristica.tds.zip to the root of the folder you use for personal additions to the TeX trees, open a command window, chdir to that folder, and type

unzip heuristica.tds.zip
initexmf --update-fndb
initexmf --edit-config-file updmap

The latter command should open updmap.cfg in your default editor, commonly Notepad. Add the line 

Map Heuristica.map 

to updmap.cfg, save and close. Then, in the command window, type

initexmf --mkmaps

You can ignore error messages that may occur here. This should result in a properly enabled heuristica.

Please send comments and bug reports or suggestions for improvement to

msharpe at ucsd dot edu