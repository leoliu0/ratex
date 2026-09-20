# stix2-type1, v2.0.0a
Type 1 versions of STIX 2.0.0 for legacy TeX engines

If you are using XeTeX or luaTeX, you do not need the files in this
directory.  See the stix2-otf package instead.

The stix2 package provides support for versions of TeX that are
limited to TFM files, Type 1 PostScript fonts, and 8-bit font
encodings.  Version 2.0.0 of the STIX fonts are being released in this
format in hopes of easing the transition from legacy TeX engines to
modern fully Unicode-compatible systems.  The Type 1 versions are
merely a repackaging of the original OpenType versions and should not
be viewed as independent entities.  Some glyphs that are traditionally
available in TeX math fonts are not available in version 2.0.0 of STIX
Two.  In such cases, we have chosen to omit them from the stix2
package rather than create incompatibilities between the OpenType and
Type 1 versions.

Note that while the OpenType versions of these fonts are under active
development, **no further updates** are planned to the Type 1
distribution; future efforts by the STI Pub companies will focus on
improving the OpenType fonts.  However, patches to the existing Type 1
distribution are welcome and, pending review, will be incorporated
into the distribution.  Any such patches should be submitted through
the github repo at

    https://github.com/stipub/stixfonts

# Changelog

v2.0.0a fixes a problem in the TeX font metrics file for stix2-mathcal
(https://github.com/stipub/stixfonts/issues/108).  The only changes
outside of this README file were to the files stix2-mathcal.tfm and
stix2-mathcal.pl.

v2.0.1 fixes one bug (https://github.com/stipub/stixfonts/issues/144)
and makes some small updates to the documentation.

v2.0.2 fixes a bug (actually, two bugs) in the previous patch.
