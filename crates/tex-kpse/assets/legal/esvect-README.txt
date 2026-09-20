%
% esvect
%
%

Change: november 2013. Rename \vecteur internal macro into \ESV@vecteur in order to avoid conflit with tdsfrmath.sty package.

Change : december 2001. Add Type1 PostScript font. Conversion from 
METAFONT files made by Olivier Crouzet. Thank's a lot !

Run Latex on esvect.ins. You got the files:

-- mathvect.mf, vecsym.mf, vsymbol.mf,  vect5.mf,
   vect6.mf, vect7.mf, vect8.mf, vect9.mf, vect10.mf;
-- esvect.sty;
-- uesvect.fd.

Run METAFONT on vect5.mf, vect6.mf, vect7.mf, vect8.mf, vect9.mf and vect10.mf
to obtain .tfm files :

	mf \mode=localfont; input vect5.mf

Put *.mf files on your MFINPUT directory.
Put *.tfm files on your TFM directory.
Put esvect.sty and uesvect.fd on your TEXINPUT directory.

Then if you run Latex on esvect.dtx, you obtain the documentation.

You will find ready to use *.mf, *.tfm, *.sty and *.fd files in 
the mfinput, mf and texinput directories.

If you want to use Type1 PostScript font, put *.pfb files in a directory
where dvips can find them, and add the content of the esvect.map file
to your psnfss.map file.

e.saudrais@wanadoo.fr
http://phylam.org