This is the README file of the umj package for TeXLive, 2005-Aug-02.

The original font bundle is available from CTAN in

  fonts/korean/HLaTeX/uhc-myoungjo-1.0.tar.gz

It provides the font `UhcMyoungjo' which can be accessed with the CJK
package under the name `mj' (see CJK.doc and the example files of the CJK
package for more details). To reduce the number of files and the total size,
the bold version of the font has not been used; it is emulated instead.

The fontmap data has been taken from the file `uhc-base.map' in file

  fonts/korean/HLaTeX/uhc-fonts.tar.gz

also available on CTAN.

The PFB and AFM files have been modified with the script `hlatex2agl.pl' to
add glyph names compliant to the Adobe Glyph List (AGL). Additionally, the
script `sfd2uni.pl' has been used to create virtual fonts which provide
Unicode font encoding; these two scripts are part of the CJK package.


History
-------

    2005-08-02
      First version.


---End of README---
