//! Classes and packages whose verbatim material is known.
//!
//! A class or package may define environments or commands that read the
//! text after them verbatim (or with blanks, tabs, line ends, `%` or `\\`
//! read otherwise), and then a change the formatter believes invisible
//! shows in the PDF. So a project may load only the classes and packages
//! below, its own files (read for their definitions like the rest of the
//! project) and those named by `known-packages` in the settings; otherwise
//! the formatter leaves its files alone.
//!
//! The lists below hold TeX Live 2026 files whose sources, with every file
//! they load, were read for such definitions: the constructs found are in
//! the formatter's tables (`format.rs`: verbatim commands and environments,
//! environments whose arguments are verbatim, short-verb characters of
//! classes, guard words, and what a package loaded with `\usepackage`
//! changes, such as endfloat's `figure`), or none were found (a catcode
//! change only while the package is read, or around reading an auxiliary
//! file, `\tl_rescan` of tokens already read).
//!
//! [`screened`] adds the TeX Live classes and packages that, with every file
//! they load, use none of the means of reading text otherwise (category
//! codes, `\endlinechar`, verbatim and listings internals, `\scantokens`,
//! xparse `v` arguments, Lua input callbacks, ...), as checked by
//! scripts/generate_fmt_screened_packages.py.

mod screened;

/// Known classes (sorted).
#[rustfmt::skip]
const CLASSES: &[&str] = &[
    "IEEEtran", "acmart", "amsart", "amsbook", "amsproc", "article", "beamer", "book", "ctexart",
    "ctexbook", "ctexrep", "elsarticle", "extarticle", "extbook", "extreport", "l3doc", "l3in2edoc",
    "letter", "llncs", "ltnews", "ltxdoc", "ltxguide", "memoir", "minimal", "mnras", "moderncv",
    "proc", "report", "revtex4", "revtex4-1", "revtex4-2", "scrartcl", "scrbook", "scrlttr2",
    "scrreprt", "slides", "source2edoc", "standalone", "tufte-book", "tufte-handout",
];

/// Known packages (sorted).
#[rustfmt::skip]
const PACKAGES: &[&str] = &[
    "BOONDOX-cal", "CJK", "CJKfntef", "CJKulem", "CJKutf8", "MULEenc", "XCharter", "a4", "abstract",
    "academicons", "accents", "acro", "acronym", "adjcalc", "adjustbox", "ae", "aecompl",
    "afterpackage", "afterpage", "algcompatible", "algorithm", "algorithm2e", "algorithmic",
    "algorithmicx", "algpseudocode", "aliascnt", "aliasctr", "alltt", "alphalph", "amsbst",
    "amsbsy", "amscd", "amsfonts", "amsgen", "amsmath", "amsopn", "amsrefs", "amssymb", "amstex",
    "amstext", "amsthm", "amsxtra", "animate", "anyfontsize", "apacite", "appendix",
    "appendixnumberbeamer", "array", "arydshln", "ascmac", "atbegshi", "atenddvi",
    "atenddvi-2019-12-11", "attachfile", "atveryend", "authblk", "autonum", "auxhook", "babel",
    "background", "backref", "balance", "bbding", "bbm", "bbold", "bboldx",
    "beamerbaseauxtemplates", "beamerbaseboxes", "beamerbasecolor", "beamerbasecompatibility",
    "beamerbasedecode", "beamerbasefont", "beamerbaseframe", "beamerbaseframecomponents",
    "beamerbaseframesize", "beamerbaselocalstructure", "beamerbasemisc", "beamerbasemodes",
    "beamerbasenavigation", "beamerbasenotes", "beamerbaseoptions", "beamerbaseoverlay",
    "beamerbaserequires", "beamerbasesection", "beamerbasetemplates", "beamerbasethemes",
    "beamerbasetheorems", "beamerbasetitle", "beamerbasetoc", "beamerbasetranslator",
    "beamerbasetwoscreens", "beamerbaseverbatim", "beamerpatchparalist", "bera", "beramono",
    "berasans", "beraserif", "bibentry", "biblatex", "bibunits", "bidi", "bidi-perpage",
    "biditools", "bigintcalc", "bitset", "blindtext", "blkarray", "blx-case-expl3",
    "blx-case-latex2e", "bm", "bold-extra", "bookmark", "booktabs", "boxedminipage", "braket",
    "breakcites", "breakurl", "breqn", "bussproofs", "calc", "calculator", "calligra", "calrsfs",
    "cancel", "caption", "caption-light", "caption2", "caption3", "cases", "catchfile", "ccicons",
    "cclicenses", "cellspace", "censor", "centernot", "changebar", "changepage", "changes",
    "charter", "chemfig", "chemgreek", "chicago", "chngcntr", "chngpage", "circuitikz", "cite",
    "citeref", "classicthesis", "cleveref", "cmap", "collcell", "collectbox", "collref", "color",
    "colordvi", "colorprofiles", "colorspace-patches-tmp-ltx", "colortbl", "commath", "comment",
    "contour", "count1to", "courier", "csquotes", "csvsimple", "ctable", "ctablestack", "ctexhook",
    "ctexpatch", "currfile", "currfile-abspath", "cuted", "datatool", "datatool-base", "datetime",
    "datetime-defaults", "datetime2", "datetime2-calc", "dblfloatfix", "dcolumn", "defpattern",
    "delarray", "derivative", "diagbox", "diffcoeff", "dirtytalk", "doc", "doi", "draftwatermark",
    "draftwatermark-2x", "dsfont", "dsserif", "duckuments", "easytable", "ebproof", "ednmath0",
    "edtable", "elocalloc", "empheq", "endfloat", "endnotes", "enumerate", "enumitem", "environ",
    "epic", "epigraph", "epsf", "epsfig", "epstopdf", "epstopdf-base", "esint", "eso-pic", "etex",
    "etexcmds", "etoc", "etoolbox", "eucal", "eulervm", "eurosym", "euscript", "everypage",
    "everypage-1x", "everysel", "everysel-2011-10-28", "everyshi", "everyshi-2001-05-15", "expl3",
    "exscale", "extpfeil", "fancybox", "fancyhdr", "fancyvrb", "fcnumparser", "fcprefix",
    "fdsymbol", "fewerfloatpages", "figureversions", "filecontents", "filehook", "filehook-2019",
    "filehook-2020", "filehook-fink", "filehook-memoir", "filehook-scrlfile", "filemod-expmin",
    "fix-cm", "fixfoot", "fixltx2e", "flafter", "float", "floatpag", "fltrace", "flushend",
    "fmtcount", "fmtprefix", "fncychap", "fontawesome", "fontawesome5",
    "fontawesome5-generic-helper", "fontawesome5-utex-helper", "fontaxes", "fontaxes-v1", "fontenc",
    "fontsize", "fontspec", "fontspec-luatex", "fontspec-xetex", "footmisc", "footmisc-2022-02-14",
    "footnote", "forest", "forest-compat", "forloop", "fourier", "fourier-orns", "fp", "fp-addons",
    "fp-basic", "fp-eqn", "fp-eval", "fp-exp", "fp-pas", "fp-random", "fp-snap", "fp-trigo",
    "fp-upn", "framed", "frcursive", "fullpage", "fvextra", "gensymb", "geometry", "gettitlestring",
    "gincltex", "glossaries", "glossaries-extra", "glossary-hypernav", "glossary-list",
    "glossary-long", "glossary-super", "glossary-tree", "graphics", "graphicx", "graphviz",
    "grfext", "hardwrap", "helvet", "hhline", "hobsub-hyperref", "hologo", "html", "hycolor",
    "hypcap", "hypdoc", "hyperref", "hyperxmp", "icomma", "ifdraft", "ifluatex", "ifmtarg",
    "ifoddpage", "ifoption", "ifpdf", "ifplatform", "ifsym", "iftex", "ifthen", "ifvtex", "ifxetex",
    "imakeidx", "import", "inconsolata", "indentfirst", "infwarerr", "inlinedef", "inputenc",
    "intcalc", "isodate", "jmlrutils", "keyval", "kpfonts", "kpfonts-otf", "kvdefinekeys",
    "kvoptions", "kvoptions-patch", "kvsetkeys", "l3bitset", "l3color", "l3keys2e", "l3opacity",
    "lastpage", "lastpage209", "lastpage2e", "lastpageclassic", "lastpagemodern",
    "latex-lab-enumitem", "latex-lab-kernel-changes", "latex-lab-testphase-bib",
    "latex-lab-testphase-block", "latex-lab-testphase-context", "latex-lab-testphase-firstaid",
    "latex-lab-testphase-float", "latex-lab-testphase-graphic", "latex-lab-testphase-l3doc",
    "latex-lab-testphase-latest", "latex-lab-testphase-marginpar", "latex-lab-testphase-math",
    "latex-lab-testphase-minipage", "latex-lab-testphase-names", "latex-lab-testphase-new-or-1",
    "latex-lab-testphase-new-or-2", "latex-lab-testphase-sec", "latex-lab-testphase-sec-template",
    "latex-lab-testphase-table", "latex-lab-testphase-text", "latex-lab-testphase-tikz",
    "latex-lab-testphase-title", "latex-lab-testphase-toc", "latexrelease", "latexsym", "lato",
    "leftidx", "letltxmacro", "letterspace", "lettrine", "libertine", "libertinus",
    "libertinus-otf", "libertinust1math", "linegoal", "lineno", "lipsum", "listings", "listofitems",
    "lmodern", "logreq", "longtable", "lscape", "ltablex", "ltabptch", "ltcaption", "ltxcmds",
    "ltxtable", "lua-unicode-math", "luabidi", "luacode", "lualatex-math", "luamml",
    "luamml-patches-kernel", "luaotfload", "luatex85", "luatexbase", "makecell", "makeidx",
    "makerobust", "manyfoot", "marvosym", "mathabx", "mathalpha", "mathastext", "mathdots",
    "mathpartir", "mathpazo", "mathptm", "mathptmx", "mathrsfs", "mathscinet", "mathtime",
    "mathtools", "mdframed", "memhfixc", "metalogo", "mfirstuc", "mflogo", "mhchem", "mhsetup",
    "microtype", "minitoc", "minted", "mleftright", "moderncvcollection", "moderncvcompatibility",
    "moreverb", "movie15", "mparhack", "mt11p", "mtpro2", "multibib", "multicol", "multido",
    "multienum", "multirow", "mwe", "mweights", "nameref", "natbib", "nccfoots", "needspace",
    "newfloat", "newlfont", "newpxmath", "newpxtext", "newtx", "newtxmath", "newtxtext",
    "newunicodechar", "nextpage", "nicefrac", "nicematrix", "nomencl", "nowidow", "ntheorem",
    "numprint", "oldlfont", "optparams", "orcidlink", "overpic", "palatino", "paralist",
    "parseargs", "parskip", "pbalance", "pbox", "pcatcode", "pdfcolmk", "pdfcomment", "pdfescape",
    "pdflscape", "pdflscape-nometadata", "pdfmanagement", "pdfmanagement-firstaid",
    "pdfmanagement-testphase", "pdfmetadata", "pdfpages", "pdftexcmds", "perpage", "pgf",
    "pgfcalendar", "pgfcomp-version-0-65", "pgfcomp-version-1-18", "pgfcore", "pgffor", "pgfkeys",
    "pgfmath", "pgfopts", "pgfpages", "pgfplots", "pgfplotstable", "pgfrcs", "pgfsubpic", "pgfsys",
    "pgftree", "physics", "pict2e", "pifont", "placeins", "polyglossia", "preview", "proof",
    "psfrag", "pslatex", "pst-calculate", "pst-node", "pstricks", "pxfonts", "pythontex",
    "qcircuit", "qtree", "quiver", "ragged2e", "rawfonts", "realscripts", "refcheck", "refcount",
    "regexpatch", "relsize", "remreset", "rerunfilecheck", "revsymb4-1", "revsymb4-2", "rkeyval",
    "rotating", "rsfso", "sansmath", "sansmathaccent", "savesym", "scalefnt", "scalerel",
    "scontents", "scrbase", "scrextend", "scrkbase", "scrlayer", "scrlayer-scrpage", "scrlfile",
    "scrlfile-hook", "scrlfile-hook-3.34", "scrlfile-patcholdlatex", "scrlogo", "sectsty",
    "selnolig", "setspace", "sfmath", "shellesc", "shortvrb", "showexpl", "showlabels", "silence",
    "siunitx", "slashed", "somedefs", "soul", "soul-ori", "sourcecodepro", "stackengine",
    "standalone", "steinmetz", "stfloats", "stix", "stix2", "stmaryrd", "stringenc",
    "stringstrings", "subcaption", "subdepth", "subfig", "subfiles", "suffix", "supertabular",
    "svg", "svn-prov", "syntax", "tablefootnote", "tabto", "tabu", "tabularray", "tabularx",
    "tabulary", "tagpdf", "tagpdf-base", "tagpdf-debug", "tagpdf-debug-generic", "tagpdf-debug-lua",
    "tagpdf-mc-code-generic", "tagpdf-mc-code-lua", "tcolorbox", "textcase", "textcmds", "textcomp",
    "textgreek", "textpos", "tgpagella", "tgtermes", "thm-amsthm", "thm-autoref", "thm-beamer",
    "thm-kv", "thm-listof", "thm-llncs", "thm-ntheorem", "thm-patch", "thm-restate", "thmtools",
    "threeparttable", "threeparttablex", "tikz", "tikz-3dplot", "tikz-cd", "tikz-qtree",
    "tikzducks", "times", "tipa", "titlecaps", "titleps", "titlesec", "titletoc", "titling",
    "tkz-base", "tkz-euclide", "tocbasic", "tocbibind", "tocloft", "todonotes", "totcount",
    "totpages", "trace", "tracefnt", "tracklang", "translations", "translator", "transparent",
    "transparent-nometadata", "trig", "trimclip", "trimspaces", "truncate", "tweaklist", "twemojis",
    "tx-ds", "txfonts", "type1cm", "typearea", "ucs", "ucshyper", "ulem", "underscore",
    "unicode-math", "unicode-math-luatex", "unicode-math-xetex", "unique", "uniquecounter", "units",
    "upgreek", "upquote", "url", "varioref", "varwidth", "verbatim", "vplref", "vruler", "wasysym",
    "watermark", "wrapfig", "xargs", "xcharter-otf", "xcolor", "xcolor-2022-06-12",
    "xcolor-patches-tmp-ltx", "xeCJK", "xeCJK-listings", "xeCJKfntef", "xfor", "xfrac", "xifthen",
    "xkeyval", "xltabular", "xltxtra", "xparse", "xpatch", "xr", "xspace", "xstring", "xtab",
    "xtemplate", "xtemplate-2023-10-10", "xunicode", "xunicode-addon", "xurl", "xxcolor", "xy",
    "xypic", "yfonts", "zhnumber", "zi4", "zref", "zref-abspage", "zref-base", "zref-clever",
    "zref-hyperref", "zref-lastpage", "zref-savepos", "zref-user",
];

pub(crate) fn known_class(name: &str) -> bool {
    CLASSES.binary_search(&name).is_ok() || screened::CLASSES.binary_search(&name).is_ok()
}

pub(crate) fn known_package(name: &str) -> bool {
    PACKAGES.binary_search(&name).is_ok() || screened::PACKAGES.binary_search(&name).is_ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn lists_are_sorted() {
        for list in [
            super::CLASSES,
            super::PACKAGES,
            super::screened::CLASSES,
            super::screened::PACKAGES,
        ] {
            assert!(list.windows(2).all(|w| w[0] < w[1]));
        }
    }
}
