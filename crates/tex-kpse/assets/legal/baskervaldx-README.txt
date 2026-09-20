Baskervaldx
----------

Baskervaldx is a Baskerville-like font package consisting of:

1. the Baskervaldx fonts (OpenType and PostScript Type 1) by Michael Sharpe, based on
the BaskervalADF Std fonts developed by Hirwen Harendel, Arkandis Digital Foundry (ADF);
the Baskervaldx fonts are  released under the GPL terms set out in the files COPYING and 
NOTICE.txt;

2. (La)TeX support by Michael Sharpe released under the LaTeX Project Public License. See 
http://www.ctan.org/license/lppl1.3
for the details of that license.

Current version: 1.08 2025/01/07

Changes in version 1.08
Added missing fd files t1minbaskervaldx.fd, ot1minbaskervaldx.fd and ly1minbaskervaldx.fd, needed only for mathematical processing using newtx. (Newtx.sty contains changes to properly support math in baskervaldx.)


Changes in version 1.078
Added text items to c2sc table and removed symbols from smcp.

Changes in version 1.077
Corrected a typo in the last line of Baskervaldx.map. (Thanks Clerk Ma.)

Changes in version 1.076
Re-added the missing components (tfm, vf, enc, map entries) required so that the "swash" versions function as documented.

Changes in version 1.075
1. Corrected the order of the GPOS tables so that proper kerning takes place in the text fonts.
2. Added OT1 encoded fonts.
3. Reworked \textcircled so that is functional once more.

Changes in version 1.074
Removed from the otf files the rlig lookups that made substitutions of, e.g., \oe for oe, in opentype tex.

Changes in version 1.073
Modified Baskervaldx.sty to comply with changes to latex with respect to mweights and fontaxes packages.

Changes in version 1.072
Modified the sty file so that Baskervaldx text figure styles are selected always in the \AtEndPreamble hook. (Formerly, using babel would prevent this.)


Changes in version 1.071
Modified some metadata in the otf files, correcting some misbehaviors in some programs and platforms.

Changes in version 1.07
1. Corrected definition of \tosfstyle in Baskervaldx.sty.
2. Changed usage of option osf to match that used in newtxtext, testing for babel and using instead \useosf in that case.

Changes in version 1.05a
Corrected name of afm folder in tds and revised xheights in two afm files.

Changes in version 1.05
Changes to italic and bold italic to improve rendering of accents.

Changes in version 1.04
Many corrections in bold math italics, following similar corrections in newtx, version 1.27. That version (or higher) of newtx is required for full functionality of this package.

Changes in version 1.03
Many adjustments to math font metrics to work around problems with J, j and f.

Changes in version 1.02
Corrected missing \fi in T1Baskervaldx-TLF.fd.

Changes in version 1.01
1. Corrected OtherBlues in Baskervald-Bold.{otf,pfb}.
2. Added files to support baskervaldx option in newtxmath, substituting Baskervaldx math italic for Times math italic.

Changes from BaskervaldADF Std
------------------------------
0. Regular and bold weights only. 
1. Roman xheights increased, ascenders decreased.
2. Adjusted most symbols used in mathematical typesetting, as the originals do not seem
to have been designed to accommodate the needs of TeX.
3. Added fixed width lining and oldstyle figures, plus proportional oldstyle figures in
each of the four styles.
4. Adjusted metrics and shapes of ligatures for more consistent spacing.
5. Adjusted metrics and kerning of most uppercase letters so they are less packed.
6. Added small caps in all four styles.
7. Added many accented glyphs so that the only characters missing in T1 encoding are Eng and eng.

(La)TeX Support
---------------

For details, see Baskervaldx-doc.pdf.

Contact Details
---------------

msharpe at ucsd dot edu