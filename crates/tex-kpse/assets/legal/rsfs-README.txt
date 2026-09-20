RSFS - Ralph Smith's Formal Script Symbol Fonts
------------------------------------------------

Version 1.0,  Released October 1991

This package contains METAFONT sources for fonts of uppercase
script letters for use as symbols in scientific and mathematical
typesetting.  For example, script D, N, and S are often used in
functional analysis for domain, nullspace, and Schwartz class.
Script L and H are the standard symbols for lagrangian and
hamiltonian densities in quantum field theory.  These formal
script symbols may be contrasted with informal scripts such
as the one designed by H. Zapf and used as `calligraphic'
symbols in the TeX math symbol font.
I have tried to be fairly conservative, and closely imitated
a formal script type designed by R. Hunter Middleton.  This is
similar to Typo Script and Marina, as well as the font used
by the typesetters at Springer Verlag.  All are based on the
so-called Spencerian or Copperplate hand lettering which
prevailed in the eighteenth century.

These programs were not designed with much `meta-ness' in mind.
Although formal script is subject to many stylistic variations,
radical changes in weight or slant violate my sense of taste.
This is a first revision, and certainly does not reflect much
expertise in METAFONT programming. It has been checked for
legibility and spacing.

I have tested these fonts on some 300dpi printers (ink-jet and laser),
but not at other resolutions.  I suspect they won't look very good at
low resolution (because of the instrinsic detail).  Comments from users
of the fonts are welcome; please describe your printer and metafont
parameters when relevant.  I will probably revise the sources and hope
to add lowercase letters to make a text font.

You are welcome to use and distribute these files; if you modify them,
please change the name but give credit to the original author!

			- Ralph Smith
			  Physics 0319
			  Univ. of California -- San Diego
			  9500 Gilman Drive, La Jolla CA 92093-0319
			  
			  E-mail: rasmith@ucsd.edu


Notes on the files:

README - this file
rsfsX.mf (X = 5,7,10) - set sizes for Xpt font and calls driver
script.mf - driver
scriptu.mf - character programs
scrload.tex -  TeX code to make these symbols available with
		a \scr macro (to be used like \cal)

To compile a font, run Metafont with CMBASE:  at the MF prompt, type

    &cm \mode=localfont; input rsfs10

Then run gftopk, put the PK and TFM files in the appropriate places,
and TeX away.

