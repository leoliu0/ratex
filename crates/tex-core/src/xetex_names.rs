//! The primitive names TeX Live 2026 `xetex -ini -etex` defines (the way
//! `fmtutil` builds `xelatex.fmt`), probed with `\ifdefined` over every
//! string of the binary and every name Ratex knows. Apart from `\ `, `\/`
//! and `\-` (see [`XETEX_SYMBOL_NAMES`]) they are all letters.

/// Letter-only primitive names (sorted).
pub(crate) static XETEX_PRIMITIVE_NAMES: &[&[u8]] = &[
    b"TeXXeTstate", b"Uchar", b"Ucharcat", b"Udelcode", b"Udelcodenum", b"Udelimiter",
    b"Umathaccent", b"Umathchar", b"Umathchardef", b"Umathcharnum", b"Umathcharnumdef",
    b"Umathcode", b"Umathcodenum", b"Uradical", b"XeTeXOTcountfeatures",
    b"XeTeXOTcountlanguages", b"XeTeXOTcountscripts", b"XeTeXOTfeaturetag",
    b"XeTeXOTlanguagetag", b"XeTeXOTscripttag", b"XeTeXcharclass", b"XeTeXcharglyph",
    b"XeTeXcountfeatures", b"XeTeXcountglyphs", b"XeTeXcountselectors", b"XeTeXcountvariations",
    b"XeTeXdashbreakstate", b"XeTeXdefaultencoding", b"XeTeXdelcode", b"XeTeXdelcodenum",
    b"XeTeXdelimiter", b"XeTeXfeaturecode", b"XeTeXfeaturename", b"XeTeXfindfeaturebyname",
    b"XeTeXfindselectorbyname", b"XeTeXfindvariationbyname", b"XeTeXfirstfontchar",
    b"XeTeXfonttype", b"XeTeXgenerateactualtext", b"XeTeXglyph", b"XeTeXglyphbounds",
    b"XeTeXglyphindex", b"XeTeXglyphname", b"XeTeXhyphenatablelength", b"XeTeXinputencoding",
    b"XeTeXinputnormalization", b"XeTeXinterchartokenstate", b"XeTeXinterchartoks",
    b"XeTeXinterwordspaceshaping", b"XeTeXisdefaultselector", b"XeTeXisexclusivefeature",
    b"XeTeXlastfontchar", b"XeTeXlinebreaklocale", b"XeTeXlinebreakpenalty",
    b"XeTeXlinebreakskip", b"XeTeXmathaccent", b"XeTeXmathchar", b"XeTeXmathchardef",
    b"XeTeXmathcharnum", b"XeTeXmathcharnumdef", b"XeTeXmathcode", b"XeTeXmathcodenum",
    b"XeTeXpdffile", b"XeTeXpdfpagecount", b"XeTeXpicfile", b"XeTeXprotrudechars",
    b"XeTeXradical", b"XeTeXrevision", b"XeTeXselectorcode", b"XeTeXselectorname",
    b"XeTeXtracingfonts", b"XeTeXupwardsmode", b"XeTeXuseglyphmetrics", b"XeTeXvariation",
    b"XeTeXvariationdefault", b"XeTeXvariationmax", b"XeTeXvariationmin", b"XeTeXvariationname",
    b"XeTeXversion", b"above", b"abovedisplayshortskip", b"abovedisplayskip",
    b"abovewithdelims", b"accent", b"adjdemerits", b"advance", b"afterassignment",
    b"aftergroup", b"atop", b"atopwithdelims", b"badness", b"baselineskip", b"batchmode",
    b"beginL", b"beginR", b"begingroup", b"belowdisplayshortskip", b"belowdisplayskip",
    b"binoppenalty", b"botmark", b"botmarks", b"box", b"boxmaxdepth", b"brokenpenalty",
    b"catcode", b"char", b"chardef", b"cleaders", b"closein", b"closeout", b"clubpenalties",
    b"clubpenalty", b"copy", b"count", b"countdef", b"cr", b"crcr", b"creationdate", b"csname",
    b"currentgrouplevel", b"currentgrouptype", b"currentifbranch", b"currentiflevel",
    b"currentiftype", b"day", b"deadcycles", b"def", b"defaulthyphenchar", b"defaultskewchar",
    b"delcode", b"delimiter", b"delimiterfactor", b"delimitershortfall", b"detokenize",
    b"dimen", b"dimendef", b"dimexpr", b"discretionary", b"displayindent", b"displaylimits",
    b"displaystyle", b"displaywidowpenalties", b"displaywidowpenalty", b"displaywidth",
    b"divide", b"doublehyphendemerits", b"dp", b"dump", b"eTeXrevision", b"eTeXversion",
    b"edef", b"elapsedtime", b"else", b"emergencystretch", b"end", b"endL", b"endR",
    b"endcsname", b"endgroup", b"endinput", b"endlinechar", b"eqno", b"errhelp", b"errmessage",
    b"errorcontextlines", b"errorstopmode", b"escapechar", b"everycr", b"everydisplay",
    b"everyeof", b"everyhbox", b"everyjob", b"everymath", b"everypar", b"everyvbox",
    b"exhyphenpenalty", b"expandafter", b"expanded", b"fam", b"fi", b"filedump", b"filemoddate",
    b"filesize", b"finalhyphendemerits", b"firstmark", b"firstmarks", b"floatingpenalty",
    b"font", b"fontchardp", b"fontcharht", b"fontcharic", b"fontcharwd", b"fontdimen",
    b"fontname", b"futurelet", b"gdef", b"global", b"globaldefs", b"glueexpr", b"glueshrink",
    b"glueshrinkorder", b"gluestretch", b"gluestretchorder", b"gluetomu", b"halign",
    b"hangafter", b"hangindent", b"hbadness", b"hbox", b"hfil", b"hfill", b"hfilneg", b"hfuzz",
    b"hoffset", b"holdinginserts", b"hrule", b"hsize", b"hskip", b"hss", b"ht", b"hyphenation",
    b"hyphenchar", b"hyphenpenalty", b"if", b"ifcase", b"ifcat", b"ifcsname", b"ifdefined",
    b"ifdim", b"ifeof", b"iffalse", b"iffontchar", b"ifhbox", b"ifhmode", b"ifincsname",
    b"ifinner", b"ifmmode", b"ifnum", b"ifodd", b"ifprimitive", b"iftrue", b"ifvbox",
    b"ifvmode", b"ifvoid", b"ifx", b"ignoreprimitiveerror", b"ignorespaces", b"immediate",
    b"indent", b"input", b"inputlineno", b"insert", b"insertpenalties", b"interactionmode",
    b"interlinepenalties", b"interlinepenalty", b"jobname", b"kern", b"language", b"lastbox",
    b"lastkern", b"lastlinefit", b"lastnodetype", b"lastpenalty", b"lastskip", b"lccode",
    b"leaders", b"left", b"lefthyphenmin", b"leftmarginkern", b"leftskip", b"leqno", b"let",
    b"limits", b"linepenalty", b"lineskip", b"lineskiplimit", b"long", b"looseness", b"lower",
    b"lowercase", b"lpcode", b"mag", b"mark", b"marks", b"mathaccent", b"mathbin", b"mathchar",
    b"mathchardef", b"mathchoice", b"mathclose", b"mathcode", b"mathinner", b"mathop",
    b"mathopen", b"mathord", b"mathpunct", b"mathrel", b"mathsurround", b"maxdeadcycles",
    b"maxdepth", b"mdfivesum", b"meaning", b"medmuskip", b"message", b"middle", b"mkern",
    b"month", b"moveleft", b"moveright", b"mskip", b"muexpr", b"multiply", b"muskip",
    b"muskipdef", b"mutoglue", b"newlinechar", b"noalign", b"noboundary", b"noexpand",
    b"noindent", b"nolimits", b"nonscript", b"nonstopmode", b"normaldeviate",
    b"nulldelimiterspace", b"nullfont", b"number", b"numexpr", b"omit", b"openin", b"openout",
    b"or", b"outer", b"output", b"outputpenalty", b"over", b"overfullrule", b"overline",
    b"overwithdelims", b"pagedepth", b"pagediscards", b"pagefilllstretch", b"pagefillstretch",
    b"pagefilstretch", b"pagegoal", b"pageshrink", b"pagestretch", b"pagetotal", b"par",
    b"parfillskip", b"parindent", b"parshape", b"parshapedimen", b"parshapeindent",
    b"parshapelength", b"parskip", b"partokencontext", b"partokenname", b"patterns", b"pausing",
    b"pdflastxpos", b"pdflastypos", b"pdfpageheight", b"pdfpagewidth", b"pdfsavepos",
    b"penalty", b"postdisplaypenalty", b"predisplaydirection", b"predisplaypenalty",
    b"predisplaysize", b"pretolerance", b"prevdepth", b"prevgraf", b"primitive", b"protected",
    b"radical", b"raise", b"randomseed", b"read", b"readline", b"relax", b"relpenalty",
    b"resettimer", b"right", b"righthyphenmin", b"rightmarginkern", b"rightskip",
    b"romannumeral", b"rpcode", b"savinghyphcodes", b"savingvdiscards", b"scantokens",
    b"scriptfont", b"scriptscriptfont", b"scriptscriptstyle", b"scriptspace", b"scriptstyle",
    b"scrollmode", b"setbox", b"setlanguage", b"setrandomseed", b"sfcode", b"shellescape",
    b"shipout", b"show", b"showbox", b"showboxbreadth", b"showboxdepth", b"showgroups",
    b"showifs", b"showlists", b"showstream", b"showthe", b"showtokens", b"skewchar", b"skip",
    b"skipdef", b"spacefactor", b"spaceskip", b"span", b"special", b"splitbotmark",
    b"splitbotmarks", b"splitdiscards", b"splitfirstmark", b"splitfirstmarks", b"splitmaxdepth",
    b"splittopskip", b"strcmp", b"string", b"suppressfontnotfounderror", b"synctex", b"tabskip",
    b"textfont", b"textstyle", b"the", b"thickmuskip", b"thinmuskip", b"time", b"toks",
    b"toksdef", b"tolerance", b"topmark", b"topmarks", b"topskip", b"tracingassigns",
    b"tracingcommands", b"tracinggroups", b"tracingifs", b"tracinglostchars", b"tracingmacros",
    b"tracingnesting", b"tracingonline", b"tracingoutput", b"tracingpages",
    b"tracingparagraphs", b"tracingrestores", b"tracingscantokens", b"tracingstacklevels",
    b"tracingstats", b"uccode", b"uchyph", b"underline", b"unexpanded", b"unhbox", b"unhcopy",
    b"uniformdeviate", b"unkern", b"unless", b"unpenalty", b"unskip", b"unvbox", b"unvcopy",
    b"uppercase", b"vadjust", b"valign", b"vbadness", b"vbox", b"vcenter", b"vfil", b"vfill",
    b"vfilneg", b"vfuzz", b"voffset", b"vrule", b"vsize", b"vskip", b"vsplit", b"vss", b"vtop",
    b"wd", b"widowpenalties", b"widowpenalty", b"write", b"xdef", b"xleaders", b"xspaceskip",
    b"year",
];

/// The control-symbol primitives `\ `, `\/` and `\-`.
pub(crate) static XETEX_SYMBOL_NAMES: &[&[u8]] = &[b" ", b"/", b"-"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, EngineKind};
    use crate::eqtb::Equiv;

    /// The primitives the XeTeX engine defines are exactly the ones
    /// `xetex -ini -etex` defines (probed with `\ifdefined` over every
    /// string of the binary).
    #[test]
    fn xetex_primitive_table_is_texlives() {
        let mut eng = Engine::new_with_kind(EngineKind::XeTeX, true);
        eng.init_primitives();
        let defined: std::collections::BTreeSet<Vec<u8>> = eng
            .cs
            .all_ids()
            .filter(|&id| id != eng.ids.frozen_primitive)
            .filter(|&id| matches!(eng.eqtb.get(id), Some(Equiv::Prim(_))))
            .map(|id| eng.cs.name(id).to_vec())
            .collect();
        let expected: std::collections::BTreeSet<Vec<u8>> = XETEX_PRIMITIVE_NAMES
            .iter()
            .chain(XETEX_SYMBOL_NAMES)
            .map(|n| n.to_vec())
            .collect();
        let name = |n: &Vec<u8>| String::from_utf8_lossy(n).into_owned();
        let extra: Vec<_> = defined.difference(&expected).map(name).collect();
        assert!(extra.is_empty(), "defined but not in TeX Live's XeTeX: {extra:?}");
        let mut missing: Vec<_> = expected.difference(&defined).map(name).collect();
        // `\nullfont` is a font identifier, defined together with the null font
        missing.retain(|n| n != "nullfont");
        assert!(missing.is_empty(), "TeX Live's XeTeX defines, this one does not: {missing:?}");
    }
}
