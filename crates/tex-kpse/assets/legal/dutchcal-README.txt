The files in this TDS are variants of the math calligraphic font ESSTIX13, plus virtual fonts and  LaTeX support files enabling their use in regular and bold weights as math alphabets. The support files were put together by Michael Sharpe (msharpe at ucsd dot edu) and are released under the LPPL.  Like the entire STIX project, of which ESSTIX13 is part, the fonts are covered by the SIL OPEN FONT LICENSE Version 1.1, whose details may be viewed in the file

STIX_Font_License_2010.pdf

distributed with the STIX fonts. 

The regular weight is essentially the same as ESSTIX13, but with minor adjustments and reworked hinting. The bold version was derived from a slightly modified version of the regular weight using FontForge to embolden it by 20em. (The slight modification to the upper loop of lower case 'f' was useful in avoiding a self intersection.)

Because of the changes, a name change was required. Given that the originals were a creation of Elsevier, Amsterdam, I hope the name DutchCalligraphic (DutchCal for short) is acceptable.

The package dutchcal loads the fonts as targets for the macros \mathcal and \mathbcal. They may be loaded instead using the mathalfa package, whose documentation contains samples of the fonts and comparisons to other math calligraphic fonts.

