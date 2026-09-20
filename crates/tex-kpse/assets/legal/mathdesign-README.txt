The Math Design fonts
~~~~~~~~~~~~~~~~~~~~~

Free mathematical fonts that match with existing text fonts.

By now, three fonts families are available:

* mdput family     corresponds to    Adobe Utopia      text fonts
* mdugm family     corresponds to    URW Garamond      text fonts
* mdbch family     corresponds to    Bitstream Charter text fonts

The following commercial families are also supported:

* mdpgd family     corresponds to    Adobe Garamond Pro     text fonts
* mdpus family     corresponds to    Adobe Utopia Std       text fonts
* mdici family     corresponds to    ITC Charter            text fonts

The fonts provided by the Math Design project are free; you can
redistribute it and/or modify it under the terms of the GNU General
Public License as published by the Free Software Foundation; either
version 2 of the License, or (at your option) any later version.

NOTES ON UPGRADING
~~~~~~~~~~~~~~~~~~

When you install a new version of the Math Design fonts, be sure to
delete all the pk files that has been automatically created by xdvi or
yap. If you forget to do that, any document read with xdvi or yap will
look ugly. Anyway, pdf versions and printing will be fine.

Each new version of the fonts introduces small changes in font
metrics. This can lead to important change in existing document,
including unexpected line and page breaks.

SMALL CAPITALS
~~~~~~~~~~~~~~

To obtain small capitals with the charter and utopia version of the
fonts, you must:

1) Obtain the corresponding commercial fonts from your favorite font
   seller. This is the font you'll need :

   Charter : Charter Small Cap         (bchrc8a.pfb)
             Charter Bold Small Cap    (bchbc8a.pfb)

   Utopia  : Utopia Expert Regular     (putr8x.pfb)
             Utopia Expert Bold        (putb8x.pfb)

   You need the Windows Postscript versions of the fonts.

2) Rename the preceding font files. I have indicated in
   parenthesis the new name of each file.

3) Put the renamed file somewhere tex will be able to find them.

   $TEXMF/fonts/type1/bitsrea/charter

   or

   $TEXMF/fonts/type1/adobe/utopia

   should be fine.

4) Refresh your texmf file database, by running an utility like
   "mktexlsr" or "texconfig rehash".

5) That's it ! Now use the mathdesign package with the option
   "expert". Small caps and oldstyle figure are available with the
   command \textsc{...}.

OBTAINING TEXT FONTS
~~~~~~~~~~~~~~~~~~~~

You can find these free text fonts  on CTAN:

CTAN directory of Adobe Utopia      : fonts/utopia
CTAN directory of URW Garamond      : nonfree/fonts/urw/garamond
CTAN directory of Bitstream Charter : fonts/charter

You have to obtain the commercial text fonts from your favorite
reseller. Only the opentype version of these fonts are supported. You
will need the following files:

* For Adobe Garamond Pro
     AGaramondPro-Regular.otf
     AGaramondPro-Italic.otf
     AGaramondPro-Semibold.otf
     AGaramondPro-SemiboldItalic.otf
* For Adobe Utopia Std
     UtopiaStd-Regular.otf
     UtopiaStd-Italic.otf
     UtopiaStd-Semibold.otf
     UtopiaStd-SemiboldIt.otf
* For ITC Charter
     CharterITCStd-Regular.otf
     CharterITCStd-Italic.otf
     CharterITCStd-Bold.otf
     CharterITCStd-BoldItalic.otf

Put them in your TEXMF tree in the directory

    ${TEXMF}/fonts/opentype/<supplier>/<fontname>

(Supplier is adobe or itc. The fontname is basically what you want.)

You don't need to rename the files. The 'expert' options is useless,
since this fonts are provided with small caps and oldstyle figures.


CHANGES
~~~~~~~

v 2.31 * removed Windows-type end of line in text files
       * URW Garamond is now a "non-free" fonts

v 2.3  * distribution splitted in "free" and "non-free"
       * fontdimen corrected (for fractions)
       * infinity symbol redesigned
       * afm files no longer distributed

v 2.25 * \nexists is now correct
       * typo corrected in mathdesign.sty

v 2.22 * Double bracket corrected
       * Punctuation in math mode is now in roman shape

v 2.21 * Incorrect ffi and ffl ligatures in some small caps fonts

v 2.2  * Glyph blackboard bold 1 added.
       * text dot accent is now correct

v 2.16 * Missing files included

v 2.15 * Weight of mdugm family slightly changed
       * Blackboard glyphs corrected

v 2.1  * Bracket and double brackets modified
       * Change of name of some internal files

v 2.0  * The commercial Adobe Garamond font is now supported.
       * The commercial Adobe Utopia Std font is now supported.
       * The commercial ITC Charter font is now supported.
       * New option 'greekfamily' added. The greek letters can be taken
        from  GFS Didot and  GFS Bodoni.
       * the \varkappa \varkappaup and \varkappait gives the expected results          * many glyphs revised
       * the 'scaled' option is no longer supported
       * the fontsite Garamond font is no longer supported.

v 1.57 * New option 'lowercase' added
       * Oriented contour integral signs are now corrects

v 1.56 * The kerning between L and ' in charter has been fixed
       * New option 'scaled' added

v 1.55 * The emplacement of some files has changed
       * 'faked' small caps are now avalaible

v 1.5  * Even more spacing corrections
       * A config file mathdesign.cfg is now avalaible
       * A new \figurecircled command is defined
       * \circledS gives the expected result
       * \bigvee and \bigwedge are now correct
       * \heart and \diamond are now outlined
       * New "expert" option added

v 1.41 * Corrected a bug in option parsing of mathdesign.sty

v 1.4  * Dimensions of large delimiters are now correct
       * Indices and exposant of script letters correctly placed
       * Overshoot of greek letters corrected (muchas gracias a Ricard
         Torres por haberme señalado el problema)
       * Slanted font have now correct names in map files (merci à
         Jean-Michel Sarlat)
       * Symbols added (\smallin, various multiple integrals)

v 1.31 * Style files don't use \AtBeginDocument any longer (except for
         \euro).

v 1.3  * The text fonts of Charter and Utopia have been slightly reduced
       * Support of sans serif and monospaced fonts simplified
       * Many approachs corrected
       * Some kerning correction in Charter
       * Option 'amssymb' deprecated (the ams symbol are now loaded
         when needed)

v 1.2  * New family added: Bitstream Charter
       * Options of the package use now the keyval syntax

v 1.1  * Many small corrections in glyph design
       * Approachs of Garamond have been tightened *a lot*
       * Arrows shape of Garamond changed

v 1.0  * Initial release


REQUIREMENTS
~~~~~~~~~~~~

A recent TeX distribution (tetex v. >3.0 or miktex v. >2.0) is
strongly recommended: the installation process will be *much*
easier.

The text fonts must be already installed on your system. If these
fonts are free, you can download them from CTAN. If not, you must get
them from the supplier.

What you need precisely are the *.pfb files. Install them on your
texmf tree in the directory

    $TEXMF/fonts/type1/supplier/name

where $TEXMF is the path of your texmf root, supplier is the name of
the supplier of the font (e.g. adobe) and name is the name of
the font (e.g. utopia).

If you use some recent distribution (e.g. MikTeX, tetex, etc.)
the free fonts are probably already installed on your system.


INSTALLATION
~~~~~~~~~~~~

If you use a recent TeX distribution (miktex, tetex, ...), you should
use the tools furnished with the distribution.

If you want to install the package by yourself:

0) First, you must choose the texmf tree where you want to install the
   Math Design fonts. This tree is probably some local or personal tree
   (for example: "c:/localtexmf" with miktex >2.3).

1) Download the archive mathdesign-tds.zip and unzip it in this
   directory. The mathdesign-tds.zip file is located in the
   install/fonts of any ctan repository.

2) Now refresh your texmf file database, by running an utility
   like "mktexlsr" or "texconfig rehash".

3) Update the configuration files of your favourite drivers
   (e.g. dvips, xdvi, yap, pdftex, etc.).

   On recent distributions, a script called 'updmap' does all the job
   for you.

   Add the following lines at the end of the file 'updmap.cfg':

       Map mdput.map
       Map mdugm.map
       Map mdbch.map
       Map mdpus.map
       Map mdpgd.map
       Map mdici.map

   and then run 'updmap' from a command line.


ENJOY
~~~~~

To use the package, simply add the following line in the preamble of
your source file:

\usepackage[supplier-typeface]{mathdesign}

Where supplier-typeface is one the following:

    adobe-utopia
    urw-garamond
    bitsream-charter

    adobe-garamond
    adobe-utopiastd
    itc-charter

Please read the documentation for more options and commands.



Thank you for using the Math Design fonts and happy TeXing :-)

                    Paul Pichaureau, paul.pichaureau@alcandre.net


                                                    Paris, Easter 2005
