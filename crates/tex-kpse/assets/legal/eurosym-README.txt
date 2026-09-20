+------------------------------------------------------+
| The Euro Symbol Package for LaTeX by Henrik Theiling |
| Version 1.4                                          |
| <henrik@theiling.de>, http://www.theiling.de/        |
+------------------------------------------------------+

Hi!

This is the Euro symbol package I created.  It provides a new symbol
to be used for the new European currency symbol, the Euro.  The
specifications were taken from the c't magazine 11/98 p.211 from from
Encyclopaedia Britannica, Book of the Year 2002.

There is a style file (sty/eurosym.sty), some Metafont source files
(src/*.mf) and a LaTeX file (doc/testeuro.tex).  For installation, put
this somewere where LaTeX can find it.

Further documentation can be found in doc/testeuro.dvi.  You can only
see the Euro symbols in the text if you have installed the package
successfully, though.

System Wide Installation Done by the Superuser
----------------------------------------------
An example follows.  Substitute $TEXMF with something that
matches your installation (search for texmf on your system
if you don't know where it is).

If you are the superuser, move the eurosym/src directory to
$TEXMF/fonts/source/eurosym, the eurosym/tfm directory to
$TEXMF/fonts/tfm/eurosym and the eurosym/sty directory to
$TEXMF/tex/latex/eurosym.

Use Installation
----------------
Choose a directory for this installation, say $HOME/lib/tex/eurosym
and make sure that the programmes `latex', `dvips', `xdvi', and
possible other TeX related programmes find the files by ensuring
that the environment variables TEXINPUTS, MFINPUT and TFMFONTS
point to $HOME/lib/tex// (double slash at the end to ensure
sub-directory recursion) as well.  See the file doc/runlatex for
an example.

Testing
-------
Go to the doc subdirectory, compile testeuro.tex with latex and read
it with xdvi.  Ignore some warnings about underfull hboxes which
occur because there is a three column part of text which TeX can't
typeset perfectly.  Also ignore the warnings about unavailable shapes
of cmr.

TODO
----
  Implement the OCR Euro symbols. (long term project...)


BUGS
----
- testeuro.tex creates warnings about unavailable shapes of cmr
  although I tried my best to make the scope of the \olshape
  command include the \officialeuro{} command only.

- FIXED:
  at least on my machine, dvips and xdvi produce checksum mismatches
  for some unknown reason.

- FIXED:
  problems with names: `eurosymbol' and `eurosym' were both used.

**Henrik
