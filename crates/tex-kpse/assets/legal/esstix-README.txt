The files in this TDS are the ESSTIX fonts in PostScript format, plus virtual fonts and  LaTeX support files allowing some of them to be used as calligraphic, fraktur and double-struck (blackboard bold) math alphabets. The support files were put together by Michael Sharpe (msharpe at ucsd dot edu) and are released under the LPPL. The fonts themselves were an unfinished product of Elsevier whose original documentation stated: ``These fonts may be used and distributed freely by and to everyone.'' Elsevier subsequently donated the collection to the STIX project, to which it was a precursor. The ESSTIX fonts were never officially released and STIX views the collection as deprecated. Nonetheless, the collection has interesting parts that I believe should not become extinct. Like the entire STIX project, the fonts are covered by the SIL OPEN FONT LICENSE Version 1.1, whose details may be viewed in the file

STIX_Font_License_2010.pdf

distributed with the STIX fonts. 

The original ESSTIX pfb font files suffer from validation problems, having improper values for some of the parameters (Bluevalues, Bluescale) used for automatic hinting. I believe I have fixed those problems for ESSTIX13, ESSTIX14 and ESSTIX15, the only fonts on which this package depends. Take care before using any of the others.

The packages esstixbb, esstixcal and esstixfrak load the fonts as targets for the macros mathbb, mathcal and mathfrak. They may be loaded instead using the mathalfa package.

The zip file should be extracted to a temporary folder named esstixTDS in your personal area.

Install the TDS in the usual way. For example, if you have administrative privileges under OS X, cd to the top level of the folder esstixTDS and type

sudo cp -r * /usr/local/texlive/texmf-local
sudo mktexlsr
sudo -H updmap-sys --enable Map=ESSTIX.map

after which you should have access to the three ESSTIX alphabets via the mathalfa package or one of the three esstix* packages described above.

With MikTeX, copy esstixTDS.zip to the root of the folder you use for personal additions to the TeX trees, open a command window, chdir to that folder, and type

unzip esstixTDS
initexmf --update-fndb
initexmf --edit-config-file updmap

The latter command should open updmap.cfg in your default editor, commonly Notepad. Add the line 

Map ESSTIX.map 

to updmap.cfg, save and close. Then, in the command window, type

initexmf --mkmaps

You can ignore error messages that may occur here. This should result in a properly enabled ESSTIX.