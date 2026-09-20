Copyright 1993 Gilles F. Robert.
You may freely use, modify and/or distribute this file.
This statement added 2008/11/14 by Clea F. Rees on the basis of the email exchange archived at http://lists.debian.org/debian-tex-maint/2008/05/msg00040.html.

%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%

A mathematician often needs special symbols to designate sets such as the
integers, the reals etc..

For the time being, the only fonts providing these special symbols have been:
 - the AMS fonts msbm (older version msym), which are intended for use with
Times and aren't truly what a mathematician expects (the stress is on both
stems instead of being on only the left one).
 - Alan Jeffrey's bbold fonts, which are sans-serif ones, with (almost) no
possibility of design variations for, say, a bold variant.
That was all, and in particular nothing for use with Computer Modern.

I was already working at that time on the project of changing the codes of
romanu.mf, romanl.mf and other files to add a 'Blackboard' variant to
Computer Modern.

                YOU HAVE IT ALL THERE !!!

The whole package contains five 'main' files:
 - blbbase.mf (the 'base') contains the supplementary definitions needed
for double stems etc..
 - blbord.mf (the 'driver') is fairly simple and only makes the necessary
calls.
 - blbordu.mf (based on romanu.mf): programs for uppercase letters.
 - blbordl.mf (based on romanl.mf): programs for lowercase letters.
 - blbordsp.mf (based on romand.mf and punct.mf): programs for the digits~1
and~2 and for parentheses and brackets.

There is also a whole lot of parameter files that are (almost) the same as
those provided by DEK.

For most of them, the only differences are
- change 'cmbase' for 'blbbase' on line~2
- change 'generate roman' for 'generate blbord' on last line
- add two extra parameters (interspace# and cap_interspace#).

A LaTeX(2e) package written by Torsten Hilbrich for using these fonts is also 
available nearby, exactly in ctan/macros/latex/contrib/supported/bbm/

I sincerely hope you'll enjoy using these fonts; if you get into problems
while using them, I should be reachable at

 _ _       Gilles F. ROBERT
|          Unit\'e de Math\'ematiques Pures et Appliqu\'ees
|  _ _ _   \'Ecole Normale Sup\'erieure
|   |   |  46, All\'ee d'Italie
|_ _|_ _|  69364 Lyon Cedex (FRANCE)
    |  \
    |   \  e-mail : gfrobert@umpa.ens-lyon.fr

%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%
