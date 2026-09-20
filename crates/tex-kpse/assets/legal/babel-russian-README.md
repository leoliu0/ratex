----------------------------------------------------------------

# Russian language module for Babel

Released under the LaTeX Project Public License v1.3c or later.
See http://www.latex-project.org/lppl.txt

The package provides support for use of Babel in documents written in Russian
(in both traditional and ancient forms). The support is adapted for use both
under legacy TEX engines, and under X∃TEX and LuaTEX.

Current version 1.3m dated by 2021-01-10

Current Maintainer is Igor A. Kotelnikov.
Submit feature request to https://github.com/kia999/babel-russian

## 1. INSTALLATION

- Unpack babel-russian.zip.
- Run "xelatex.exe russianb.dtx" (recommended)
  or "pdflatex.exe russianb.dtx";
  alternatively run "tex.exe russianb.dtx"
  or "tex.exe russianb.ins", if you don't need documentation
- Move "russianb.ldf" to <textmf>/tex/latex/babel-russian
- Move "russianb.pdf" and README to <textmf>/doc/latex/russian-babel/
- Update filename base (see documentation for your TeX system)

## 2. USAGE

Russian language definition file can be used both with legacy 8-bit engines
(such as latex.exe or pdflatex.exe) and Unicode compilers (xelatex.exe or
lualatex.exe). The Unicode engines can be ran either in Unicode mode or 8-bit
compatibility mode, which emulates the legacy engines. The two modes differ by
a set of packages loaded in the preamble of a source TeX file. It is important
to keep recommended order of the packages loaded, especially when running
Unicode engines in a compatibility 8-bit mode.

In the examples below, it is assumed that a source file to be compile
has utf8 input encoding.

### 2.1. 8-bit mode

#### 2.1.1 PDFLATeX, LaTeX

    \usepackage[T1,T2A]{fontenc}
    \usepackage[utf8]{inputenc}
    \usepackage[english,russian]{babel}

#### 2.1.2 LuaLaTeX

    \usepackage[T1,T2A]{fontenc}
    \usepackage[lutf8]{luainputenc}
    \usepackage[english,russian]{babel}

#### 2.1.3 XeLaTeX

    \XeTeXinputencoding "bytes"
    \usepackage[utf8]{inputenc}
    \usepackage[T2A]{fontenc}
    \usepackage[english,russian]{babel}

### 2.2 Unicode mode, LuaLaTeX or XeLaTeX

    \usepackage{fontspec}
        \defaultfontfeatures{Ligatures={TeX}}
        \setmainfont{CMU Serif}
        \setsansfont{CMU Sans Serif}
        \setmonofont{CMU Typewriter Text}
    \usepackage[english,russian]{babel}

Instead of the Computer Modern Unicode (CMU) fonts loaded in this example,
you may try any True Type or Open Type fonts installed on your computer provided
that those fonts came with Russian letters.

### 2.3 Typesetting ancient book

    \usepackage[english,russian]{babel}
    \languageattribute{russian}{ancient}
or
    \usepackage[english,russian.ancient]{babel}

Using Unicode mode is strongly recommended for typesetting ancient texts.
Be sure to take X2 or OT2 font encodings when running 8-bit engine such
as latex or pdflatex.

## 3. DOCUMENTATION

See russianb.pdf for more information.

## 4. KNOWN PROBLEMS

Before switching from a legacy 8-bit engine (tex, pdftex) to an Unicode
engine (xetex, luatex) and vise versa delete all .aux, .toc, .lot, .lof
files as they might have stored incompatible internal encodings.

T2* font encodings do not have legacy Cyrillic letters `yat', which is
hard-coded in ancient caption names. Be sure to use an Unicode engine
or borrow `\cyryat` and `\CYRYAT` commands from X2 font encoding when setting
the language attribute to "ancient", for example:

    \usepackage[X2,T2A]{fontenc}
    \usepackage[utf8]{inputenc}
    \DeclareUnicodeCharacter{0462}{\CYRYAT}
    \DeclareTextSymbolDefault{\CYRYAT}{X2}
    \DeclareUnicodeCharacter{0463}{\cyryat}
    \DeclareTextSymbolDefault{\cyryat}{X2}
    \usepackage[english,russian.ancient]{babel}

Consult your font documentation for other ancient glyphs which
might be absent.

## 5. CHANGES

2021-01-10 version 1.3m

    * The macro `\cyrdash` that prints Cyrillic dash has been changed.
      Now it is alias of `\textemdash` in all encodings.

    * New Customisation section is added to the module documentation. It
      describes how to modify the `\cyrdash` macro and shorthand `"--~"` that
      is intended to print Cyrillic dash in compound names of physical
      laws, mathematical equations, company titles, e.t.c. such as the
      Ostrogradsky-Gauss theorem (thanks to Olga Lapko).

2020-10-16 version 1.3l

    * Patches for Russian language from hyperref package to babel-russian module
      (thanks to Ulrike Fischer).

2020-09-06 version 1.3k

    * Bug fixed in definition of \Russian command (thanks to Javier Bezos).

2017-08-08 version 1.3j

    * TU encoding is set as default for X∃TeX and LuaTeX.

    * \cyrdash now always prints dash 20 percents shorter than emdash.

2017-01-12 version 1.3i

    * Bug fixed in \NOD, \NOK and similar log-functions (thanks to V. Vlasov).

2016-02-18 version 1.3h

    * Bugs fixed in captions for revtex4 and revtex4-1 classes.

    * \cyrdash is now faked using \ProvideTextCommandDefault rather
      than \def.

2015-05-01 version 1.3g

    * Added support for revtex4 and revtex4-1 classes.

2014-10-21 version 1.3f

    * A documentation file russianb.pdf can now be generated by
      running pdflatex.exe over russianb.dtx although xelatex.exe
      is still recommended.

    * russianb.ins was missed in version 1.3e.

2014-10-14 version 1.3e

    * Generating all stuff from single dtx file.

2014-10-02 version 1.3d

    * Bug fix in \Proj.

2014-06-02 version 1.3c

    * Bug fix in \daterussian.

2013-04-06 version 1.3b

    * Added support for the packages \pkg{listing}, \pkg{nomencl}, and
      \pkg{nomentbl}.

2013-04-18 version 1.3a

    * Added the language attribute 'ancient' for typesetting old
      Slavonic and Church books.

2013-04-08 version 1.3

    * Updated for babel 3.9.
    * \Alph and \alph commands are not redefined any more by russianb.ldf.

2012-06-02 version 1.2a

    * russian/russianb.dtx, v1.2a : Indentation of 1st paragraph removed;
      use the indentfirst package to automatically indent first paragraph
      after sectioning commands.

2011-10-20 version 1.2 Igor A. Kotelnikov  <kia999 at mail dot ru>

    * Added support for LuaTeX and XeTeX;
      added translation for Glossary; removed \Rus, \English,
      \Eng, \cyrmath.., \latinencoding, \latintext;
      \Russian is now an alias for \selectlanguage{russian}.

Original source:  russianb.dtx,
    2008/03/21 v1.1r Russian support from the babel system.

----------------------------------------------------------------
%%
%% \CharacterTable
%%  {Upper-case    \A\B\C\D\E\F\G\H\I\J\K\L\M\N\O\P\Q\R\S\T\U\V\W\X\Y\Z
%%   Lower-case    \a\b\c\d\e\f\g\h\i\j\k\l\m\n\o\p\q\r\s\t\u\v\w\x\y\z
%%   Digits        \0\1\2\3\4\5\6\7\8\9
%%   Exclamation   \!     Double quote \"    Hash (number) \#
%%   Dollar        \$     Percent      \%    Ampersand     \&
%%   Acute accent  \'     Left paren   \(    Right paren   \)
%%   Asterisk      \*     Plus         \+    Comma         \,
%%   Minus         \-     Point        \.    Solidus       \/
%%   Colon         \:     Semicolon    \;    Less than     \<
%%   Equals        \=     Greater than \>    Question mark \?
%%   Commercial at \@     Left bracket \[    Backslash     \\
%%   Right bracket \]     Circumflex   \^    Underscore    \_
%%   Grave accent  \`     Left brace   \{    Vertical bar  \|
%%   Right brace   \}     Tilde        \~}
%%

%% Nonunicode Cyrillic Letters
%% \CYRA=А
%% \CYRB=Б
%% \CYRV=В
%% \CYRG=Г
%% \CYRGUP=Ґ
%% \CYRD=Д
%% \CYRE=Е
%% \CYRIE=Є
%% \CYRZH=Ж
%% \CYRZ=З
%% \CYRI=И
%% \CYRII=I
%% \CYRYI=Ї
%% \CYRISHRT=Й
%% \CYRK=К
%% \CYRL=Л
%% \CYRM=М
%% \CYRN=Н
%% \CYRO=О
%% \CYRP=П
%% \CYRR=Р
%% \CYRS=С
%% \CYRT=Т
%% \CYRU=У
%% \CYRF=Ф
%% \CYRH=Х
%% \CYRC=Ц
%% \CYRCH=Ч
%% \CYRSH=Ш
%% \CYRSHCH=Щ
%% \CYRYU=Ю
%% \CYRYA=Я
%% \CYRSFTSN=Ь
%%
%% \cyra=а
%% \cyrb=б
%% \cyrv=в
%% \cyrg=г
%% \cyrgup=ґ
%% \cyrd=д
%% \cyre=е
%% \cyrie=є
%% \cyrzh=ж
%% \cyrz=з
%% \cyri=и
%% \cyrii=i
%% \cyryi=ї
%% \cyrishrt=й
%% \cyrk=к
%% \cyrl=л
%% \cyrm=м
%% \cyrn=н
%% \cyro=о
%% \cyrp=п
%% \cyrr=р
%% \cyrs=с
%% \cyrt=т
%% \cyru=у
%% \cyrf=ф
%% \cyrh=х
%% \cyrc=ц
%% \cyrch=ч
%% \cyrsh=ш
%% \cyrshch=щ
%% \cyryu=ю
%% \cyrya=я
%% \cyrsftsn=ь
