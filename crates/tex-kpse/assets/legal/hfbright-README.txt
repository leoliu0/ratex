
cmbright fonts in Type1 format (Version 0.6, 2002/11/22)
========================================================

This package contains the cmbright fonts in Type1 format. Only the
fonts are included that are not part of the cm-super bundle 
(thus, math and OT1 fonts).

This package contains the ready Postscript Type1 fonts in PFB format as
used by dvips and pdflatex, as well as a map file and a config file for
dvips.

Everything I am describing is valid for the current beta version of teTeX.
If you have another distribution you might have to change several things.

If you want to built the fonts on your own look at the end of this document.

The font names and the file names begin with 'hf' for 'harders font'. This
has been done for not getting mixed up with the commercial cmbright fonts by
MicroPress. This has been suggested by Walter Schmidt, the creator of
the cmbright fonts.

The outlines were generated using mftrace and pfaedit. The only changes I
have done were the change of the font names and the other global font flags.
I have not done any hinting by hand. Thus the fonts are far from perfect.
But at least they increase quality when using acrobat reader on the screen.

Licence is LPPL. No Warrancy.

Many thanks
- to Walter Schmidt (was@vr-web.de) 
  for the permission to upload these fonts to CTAN and the help with the
  arithmetic overflow.
- to Felix Neubauer (felix.neubauer@gmx.net)
  who suggested a workaround for some fonts that did an arithmetic 
  overflow and found the problem that UniqueID was zero.
  Additionally, he has tested the fonts.
- to Ralf Stubner (ralf.stubner@physik.uni-erlangen.de)
  who told me how to create the needed encoding files and helped me with
  pfaedit. Ralf has also tested the fonts.

Manual installation
-------------------

Put the pfb files (./pfb/*.pfb) into the TeX search path for fonts 
(e.g. $TEXMFLOCAL/fonts/type1/public/hfbright/) and the dvips 
configuration files (./dvips/*) into $TEXMFLOCAL/dvips/config.

You can also add the contents of cmbright.map to psfonts.map. For the current
teTeX beta, this is done by adding 
    Map hfbright.map
to the file updmap.cfg which normally is in $TEXMFMAIN/web2c/.
Then update the font maps by calling 'updmap'.


Automatic installation (and generation)
---------------------------------------

Just execute ./install.sh.
If some files are missing, generate.sh is called before installation.
Please have a look into install.sh to be sure the right directories etc.
are used.


Generation of the Type 1 fonts
------------------------------

Just call ./generate.sh. The pfa files are written into ./pfa/, the pfb
files into ./pfb/. hfbright.map and config.hfbright are written into
./dvips/.



To Do
-----

I have ordered UniqueIDs from Adobe. I have to wait until they arrive and
then apply them to the fonts. This should not cause any trouble.


2002-11-22
Harald Harders
h.harders@tu-bs.de
