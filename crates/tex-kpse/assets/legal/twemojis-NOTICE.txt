# Twemojis

Simple wrapper that allows to use Twitter's emojis through LaTeX commands.
As this package works with graphics and not with fonts to achieve its goal, it *should* work on every machine with any LaTeX version without problems.

In comparison, the `emoji` package on CTAN <https://www.ctan.org/pkg/emoji> works on fonts and requires lualatex.
It is lighter and probably compiles faster if you use thousands of emojis, but it also has stricter requirements and is not as flexible.

This package really is intended to be used within Ti*k*Z images.

## Usage

This package in on [CTAN](https://www.ctan.org) as [twemojis](https://ctan.org/pkg/twemojis), so you should be able to use it like any other package, with

```latex
\usepackage{twemojis}
```

An emoji can be used via `\twemoji{...}`.
There is the alternative `\texttwemoji{...}` command which scales and sets the emoji to fit the text line.
More information can be found in the [package's documentation](https://ftp.gwdg.de/pub/ctan/macros/latex/contrib/twemojis/twemojis.pdf).

## Installation

If the package is not in you TeX distribution try [these install steps](https://tex.stackexchange.com/questions/73016/how-do-i-install-an-individual-package-on-a-linux-system).
The TL;DR for the manual installation (if everything else fails) is:
- Run `kpsewhich -var-value TEXMFLOCAL` and `kpsewhich -var-value TEXMFHOME` and check which of the resulting directories is populated.
  Lets call the resulting directory `<base dir>`.
- Create the directory `<base dir>/tex/latex/twemojis` and move the `.sty` file and the folder `pdf-twemojis` there.
- Run `mktexlsr` (you probably need root for that).
- If something doesn't work, read the long version linked above (Method 3).

## Problems/TODOs

- Currently the emojis are not text-selectable, that might change in future versions.
- Remove duplicate SVG files and point all names to the same generated PDF.

# Implementation Notice

As the emojis are PDF-based and use transparency, they include groups.
I was not able to change the `inkscape` export so that that does not happen.
Hence, `pdflatex` throws a warning about multiple groups on one PDF page.
This package disables said warning: `\pdfsuppresswarningpagegroup=1` as it is of no concern for the inputted emojis.
It **could** happen, that a PDF you input is faulty and you don't notice due to said suppression.
You can enable the warning with `\pdfsuppresswarningpagegroup=0`.

# Licenses

## Emojis

The emojis and all derived graphics belong to Twitter, Inc and other contributors (Copyright 2019).
They are licensed under CC-BY 4.0: https://creativecommons.org/licenses/by/4.0/.

Hence, attribution of the original work is needed.

### Attribution

**I'm no lawyer, so take this section with a grain of salt.**

As the emojis themselves are licensed under CC-BY they require attribution but are open to be distributed and modified anyway you like (which makes these packages possible).
I'm not sure whether the attribution in the package's source code and in this repository is enough for a file (e.g. PDF) generated with this package to be covered as well (doubt it, really).

So, I see two different possibilities to attribute the emojis in a compiled document:

1. Add attribution to the metadata of the document.
   For example with <https://www.ctan.org/pkg/hyperxmp> in PDF files.
2. Add attribution in the document directly (e.g. on the last page of a presentation).
   For example:
   ```
   Emoji graphics licensed under CC-BY 4.0: https://creativecommons.org/licenses/by/4.0/
   Copyright 2019 Twitter, Inc and other contributors
   ```
   I consider this option to be the safest.

## LaTeX Package

The LaTeX packages are licensed under the LPPL 1.3 or later License.

> Copyright (c) 2021 Jost Rossel
> This file may be distributed and/or modified under the
> conditions of the LaTeX Project Public License, either
> version 1.3 of this license or (at your option) any later
> version. The latest version of this license is in:
>
>     http://www.latex-project.org/lppl.txt
>
> and version 1.3 or later is part of all distributions of
> LaTeX version 2005/12/01 or later.
