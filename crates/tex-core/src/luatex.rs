//! LuaTeX's primitive model.
//!
//! `luatex --ini` defines only the primitives of the `tex` group plus
//! `\directlua`; every other primitive lives in LuaTeX's primitive table
//! and becomes a control sequence through `tex.enableprimitives(prefix,
//! names)` (ltexlib.c), usually with names from `tex.extraprimitives`.
//! Ratex keeps that table per engine ([`LuaPrimitive`] entries for the
//! names it implements); LuaTeX names of pdfTeX/e-TeX primitives resolve to
//! the implementations registered under the pdfTeX names.

use crate::engine::{Engine, EngineKind};
use crate::eqtb::Equiv;
use crate::prim::{DimParam, IntParam, Prim};
use crate::token::Token;

/// `tex.extraprimitives` group bits (ltexlib.c `tex_command` ...).
pub(crate) const TEX: u8 = 1;
pub(crate) const CORE: u8 = 2;
pub(crate) const ETEX: u8 = 4;
pub(crate) const LUATEX: u8 = 8;

/// A LuaTeX primitive ratex implements: its `tex.extraprimitives` group,
/// LuaTeX name and meaning.
#[derive(Clone, Debug)]
pub struct LuaPrimitive {
    pub group: u8,
    pub name: &'static [u8],
    pub equiv: Equiv,
}

/// LuaTeX-only primitives with their own implementation.
static LUATEX_ONLY: &[(&[u8], Prim)] = &[
    (b"luatexversion", Prim::LuaTeXVersion),
    (b"luatexrevision", Prim::LuaTeXRevision),
    (b"luatexbanner", Prim::LuaTeXBanner),
    (b"directlua", Prim::DirectLua),
    (b"luafunction", Prim::LuaFunction),
    (b"luafunctioncall", Prim::LuaFunctionCall),
    (b"luadef", Prim::LuaDef),
    (b"luabytecode", Prim::LuaBytecode),
    (b"luabytecodecall", Prim::LuaBytecodeCall),
    (b"catcodetable", Prim::CatCodeTable),
    (b"initcatcodetable", Prim::InitCatCodeTable),
    (b"savecatcodetable", Prim::SaveCatCodeTable),
    (b"attribute", Prim::Attribute),
    (b"attributedef", Prim::AttributeDef),
    (b"Ustack", Prim::Ustack),
    (b"Ustartmath", Prim::Ustartmath),
    (b"Ustopmath", Prim::Ustopmath),
    (b"pdfvariable", Prim::PdfVariable),
    (b"pdffeedback", Prim::PdfFeedback),
    (b"pdfextension", Prim::PdfExtension),
    (b"dvivariable", Prim::DviVariable),
    (b"dvifeedback", Prim::DviFeedback),
    (b"dviextension", Prim::DviExtension),
    (b"glet", Prim::GLet),
    (b"hpack", Prim::HPack),
    (b"vpack", Prim::VPack),
    (b"tpack", Prim::TPack),
    (b"eTeXminorversion", Prim::EtxMinorVersion),
    (b"eTeXVersion", Prim::EtxVersionString),
    (b"gluestretchorder", Prim::LuaGlueStretchOrder),
    (b"glueshrinkorder", Prim::LuaGlueShrinkOrder),
    (b"toksapp", Prim::ToksApp),
    (b"tokspre", Prim::ToksPre),
    (b"etoksapp", Prim::EToksApp),
    (b"etokspre", Prim::EToksPre),
    (b"gtoksapp", Prim::GToksApp),
    (b"gtokspre", Prim::GToksPre),
    (b"xtoksapp", Prim::XToksApp),
    (b"xtokspre", Prim::XToksPre),
    (b"csstring", Prim::CsString),
    (b"begincsname", Prim::BeginCsName),
    (b"letcharcode", Prim::LetCharCode),
    (b"formatname", Prim::FormatName),
    (b"luaescapestring", Prim::LuaEscapeString),
    (b"deferred", Prim::Deferred),
    (b"boundary", Prim::Boundary),
    (b"wordboundary", Prim::WordBoundary),
    (b"protrusionboundary", Prim::ProtrusionBoundary),
    (b"Uleft", Prim::ULeft),
    (b"Umiddle", Prim::UMiddle),
    (b"Uright", Prim::URight),
    (b"outputmode", Prim::IntP(IntParam::PdfOutput)),
    (b"exhyphenchar", Prim::IntP(IntParam::ExHyphenChar)),
    (b"firstvalidlanguage", Prim::IntP(IntParam::FirstValidLanguage)),
    (b"showstream", Prim::IntP(IntParam::ShowStream)),
    (b"automatichyphenmode", Prim::IntP(IntParam::AutomaticHyphenMode)),
    (b"automatichyphenpenalty", Prim::IntP(IntParam::AutomaticHyphenPenalty)),
    (b"breakafterdirmode", Prim::IntP(IntParam::BreakAfterDirMode)),
    (b"compoundhyphenmode", Prim::IntP(IntParam::CompoundHyphenMode)),
    (b"discretionaryligaturemode", Prim::IntP(IntParam::DiscretionaryLigatureMode)),
    (b"exceptionpenalty", Prim::IntP(IntParam::ExceptionPenalty)),
    (b"explicithyphenpenalty", Prim::IntP(IntParam::ExplicitHyphenPenalty)),
    (b"fixupboxesmode", Prim::IntP(IntParam::FixupBoxesMode)),
    (b"glyphdimensionsmode", Prim::IntP(IntParam::GlyphDimensionsMode)),
    (b"hyphenationbounds", Prim::IntP(IntParam::HyphenationBounds)),
    (b"hyphenpenaltymode", Prim::IntP(IntParam::HyphenPenaltyMode)),
    (b"localbrokenpenalty", Prim::IntP(IntParam::LocalBrokenPenalty)),
    (b"localinterlinepenalty", Prim::IntP(IntParam::LocalInterLinePenalty)),
    (b"luacopyinputnodes", Prim::IntP(IntParam::LuaCopyInputNodes)),
    (b"mathdefaultsmode", Prim::IntP(IntParam::MathDefaultsMode)),
    (b"mathdelimitersmode", Prim::IntP(IntParam::MathDelimitersMode)),
    (b"mathdisplayskipmode", Prim::IntP(IntParam::MathDisplaySkipMode)),
    (b"mathemptydisplaymode", Prim::IntP(IntParam::MathEmptyDisplayMode)),
    (b"matheqdirmode", Prim::IntP(IntParam::MathEqDirMode)),
    (b"matheqnogapstep", Prim::IntP(IntParam::MathEqnoGapStep)),
    (b"mathflattenmode", Prim::IntP(IntParam::MathFlattenMode)),
    (b"mathitalicsmode", Prim::IntP(IntParam::MathItalicsMode)),
    (b"mathnolimitsmode", Prim::IntP(IntParam::MathNoLimitsMode)),
    (b"mathpenaltiesmode", Prim::IntP(IntParam::MathPenaltiesMode)),
    (b"mathrulesfam", Prim::IntP(IntParam::MathRulesFam)),
    (b"mathrulesmode", Prim::IntP(IntParam::MathRulesMode)),
    (b"mathrulethicknessmode", Prim::IntP(IntParam::MathRuleThicknessMode)),
    (b"mathscriptboxmode", Prim::IntP(IntParam::MathScriptBoxMode)),
    (b"mathscriptcharmode", Prim::IntP(IntParam::MathScriptCharMode)),
    (b"mathscriptsmode", Prim::IntP(IntParam::MathScriptsMode)),
    (b"mathsurroundmode", Prim::IntP(IntParam::MathSurroundMode)),
    (b"nokerns", Prim::IntP(IntParam::NoKerns)),
    (b"noligs", Prim::IntP(IntParam::NoLigs)),
    (b"nospaces", Prim::IntP(IntParam::NoSpaces)),
    (b"outputbox", Prim::IntP(IntParam::OutputBox)),
    (b"prebinoppenalty", Prim::IntP(IntParam::PreBinOpPenalty)),
    (b"predisplaygapfactor", Prim::IntP(IntParam::PreDisplayGapFactor)),
    (b"prerelpenalty", Prim::IntP(IntParam::PreRelPenalty)),
    (b"shapemode", Prim::IntP(IntParam::ShapeMode)),
    (b"suppressfontnotfounderror", Prim::IntP(IntParam::SuppressFontNotFoundError)),
    (b"suppressifcsnameerror", Prim::IntP(IntParam::SuppressIfCsnameError)),
    (b"suppresslongerror", Prim::IntP(IntParam::SuppressLongError)),
    (b"suppressmathparerror", Prim::IntP(IntParam::SuppressMathParError)),
    (b"suppressoutererror", Prim::IntP(IntParam::SuppressOuterError)),
    (b"suppressprimitiveerror", Prim::IntP(IntParam::SuppressPrimitiveError)),
    (b"variablefam", Prim::IntP(IntParam::VariableFam)),
    (b"textdirection", Prim::IntP(IntParam::TextDirection)),
    (b"pardirection", Prim::IntP(IntParam::ParDirection)),
    (b"bodydirection", Prim::IntP(IntParam::BodyDirection)),
    (b"linedirection", Prim::IntP(IntParam::LineDirection)),
    (b"mathdirection", Prim::IntP(IntParam::MathDirection)),
    (b"pagedirection", Prim::IntP(IntParam::PageDirection)),
    (b"pagetopoffset", Prim::DimP(DimParam::PageTopOffset)),
    (b"pageleftoffset", Prim::DimP(DimParam::PageLeftOffset)),
    (b"pagebottomoffset", Prim::DimP(DimParam::PageBottomOffset)),
    (b"pagerightoffset", Prim::DimP(DimParam::PageRightOffset)),
    (b"mathsurroundskip", Prim::GlueP(crate::prim::GlueParam::MathSurroundSkip)),
];

pub(crate) use crate::uprim::mp::*;

/// The meaning of a LuaTeX-only primitive name (everything not registered
/// under a pdfTeX or e-TeX name).
fn luatex_only(name: &[u8]) -> Option<Prim> {
    if let Some(&(_, p)) = LUATEX_ONLY.iter().find(|(n, _)| *n == name) {
        return Some(p);
    }
    if let Some(&(_, u)) = crate::uprim::UPRIMS.iter().find(|(n, _)| *n == name) {
        return Some(Prim::U(u));
    }
    crate::uprim::UMATH_NAMES
        .iter()
        .position(|n| *n == name)
        .map(|id| Prim::UMath(id as u8))
}

/// LuaTeX names of primitives ratex registers under their pdfTeX or e-TeX
/// name (LuaTeX name, ratex name).
static ALIASES: &[(&[u8], &[u8])] = &[
    (b"adjustspacing", b"pdfadjustspacing"),
    (b"protrudechars", b"pdfprotrudechars"),
    (b"pagewidth", b"pdfpagewidth"),
    (b"pageheight", b"pdfpageheight"),
    (b"pxdimen", b"pdfpxdimen"),
    (b"insertht", b"pdfinsertht"),
    (b"uniformdeviate", b"pdfuniformdeviate"),
    (b"normaldeviate", b"pdfnormaldeviate"),
    (b"randomseed", b"pdfrandomseed"),
    (b"setrandomseed", b"pdfsetrandomseed"),
    (b"savepos", b"pdfsavepos"),
    (b"lastxpos", b"pdflastxpos"),
    (b"lastypos", b"pdflastypos"),
    (b"saveboxresource", b"pdfxform"),
    (b"useboxresource", b"pdfrefxform"),
    (b"lastsavedboxresourceindex", b"pdflastxform"),
    (b"saveimageresource", b"pdfximage"),
    (b"useimageresource", b"pdfrefximage"),
    (b"lastsavedimageresourceindex", b"pdflastximage"),
    (b"lastsavedimageresourcepages", b"pdflastximagepages"),
    (b"ifabsnum", b"ifpdfabsnum"),
    (b"ifabsdim", b"ifpdfabsdim"),
    (b"primitive", b"pdfprimitive"),
    (b"ifprimitive", b"ifpdfprimitive"),
    (b"explicitdiscretionary", b"-"),
    (b"draftmode", b"pdfdraftmode"),
    (b"expandglyphsinfont", b"pdffontexpand"),
    (b"ignoreligaturesinfont", b"pdfnoligatures"),
    (b"copyfont", b"pdfcopyfont"),
    (b"eTeXgluestretchorder", b"gluestretchorder"),
    (b"eTeXglueshrinkorder", b"glueshrinkorder"),
];

/// `\pdfvariable` keys (textoken.c `do_variable_pdf`, in its scan order)
/// and the ratex primitive holding each backend parameter.
pub(crate) static PDF_VARIABLES: &[(&[u8], &[u8])] = &[
    (b"compresslevel", b"pdfcompresslevel"),
    (b"decimaldigits", b"pdfdecimaldigits"),
    (b"imageresolution", b"pdfimageresolution"),
    (b"pkresolution", b"pdfpkresolution"),
    (b"uniqueresname", b"pdfuniqueresname"),
    (b"majorversion", b"pdfmajorversion"),
    (b"minorversion", b"pdfminorversion"),
    (b"pagebox", b"pdfpagebox"),
    (b"inclusionerrorlevel", b"pdfinclusionerrorlevel"),
    (b"ignoreunknownimages", b"\xFF\x00LUA-ignoreunknownimages"),
    (b"gamma", b"pdfgamma"),
    (b"imageapplygamma", b"pdfimageapplygamma"),
    (b"imagegamma", b"pdfimagegamma"),
    (b"imagehicolor", b"pdfimagehicolor"),
    (b"imageaddfilename", b"\xFF\x00LUA-imageaddfilename"),
    (b"objcompresslevel", b"pdfobjcompresslevel"),
    (b"inclusioncopyfonts", b"pdfinclusioncopyfonts"),
    (b"gentounicode", b"pdfgentounicode"),
    (b"pkfixeddpi", b"\xFF\x00LUA-pkfixeddpi"),
    (b"suppressoptionalinfo", b"pdfsuppressptexinfo"),
    (b"omitcidset", b"\xFF\x00LUA-omitcidset"),
    (b"recompress", b"\xFF\x00LUA-recompress"),
    (b"omitcharset", b"pdfomitcharset"),
    (b"omitinfodict", b"pdfomitinfodict"),
    (b"omitmediabox", b"\xFF\x00LUA-omitmediabox"),
    (b"linking", b"\xFF\x00LUA-linking"),
    (b"omitprocset", b"pdfomitprocset"),
    (b"ptexprefix", b"pdfptexuseunderscore"),
    (b"horigin", b"pdfhorigin"),
    (b"vorigin", b"pdfvorigin"),
    (b"threadmargin", b"pdfthreadmargin"),
    (b"destmargin", b"pdfdestmargin"),
    (b"linkmargin", b"pdflinkmargin"),
    (b"xformmargin", b"\xFF\x00LUA-xformmargin"),
    (b"pageattr", b"pdfpageattr"),
    (b"pageresources", b"pdfpageresources"),
    (b"pagesattr", b"pdfpagesattr"),
    (b"xformattr", b"\xFF\x00LUA-xformattr"),
    (b"xformresources", b"\xFF\x00LUA-xformresources"),
    (b"pkmode", b"pdfpkmode"),
    (b"trailerid", b"pdftrailerid"),
];

/// LuaTeX-only backend parameters: hidden names for [`PDF_VARIABLES`].
fn lua_backend_param(name: &[u8]) -> Option<Prim> {
    Some(match name.strip_prefix(b"\xFF\x00LUA-")? {
        b"ignoreunknownimages" => Prim::IntP(IntParam::PdfIgnoreUnknownImages),
        b"imageaddfilename" => Prim::IntP(IntParam::PdfImageAddFilename),
        b"pkfixeddpi" => Prim::IntP(IntParam::PdfPkFixedDpi),
        b"omitcidset" => Prim::IntP(IntParam::PdfOmitCidSet),
        b"recompress" => Prim::IntP(IntParam::PdfRecompress),
        b"omitmediabox" => Prim::IntP(IntParam::PdfOmitMediaBox),
        b"linking" => Prim::IntP(IntParam::PdfLinking),
        b"xformmargin" => Prim::DimP(DimParam::PdfXFormMargin),
        b"xformattr" => Prim::ToksP(crate::prim::ToksParam::PdfXFormAttr),
        b"xformresources" => Prim::ToksP(crate::prim::ToksParam::PdfXFormResources),
        _ => return None,
    })
}

/// `\pdfextension` keys (extensions.c `do_extension_pdf`) with the pdfTeX
/// command performing each, and keyword text to pass on to it.
pub(crate) static PDF_EXTENSIONS: &[(&[u8], &[u8], &[u8])] = &[
    (b"literal", b"pdfliteral", b""),
    (b"lateliteral", b"pdfliteral", b"shipout"),
    (b"dest", b"pdfdest", b""),
    (b"annot", b"pdfannot", b""),
    (b"save", b"pdfsave", b""),
    (b"restore", b"pdfrestore", b""),
    (b"setmatrix", b"pdfsetmatrix", b""),
    (b"obj", b"pdfobj", b""),
    (b"refobj", b"pdfrefobj", b""),
    (b"colorstack", b"pdfcolorstack", b""),
    (b"startlink", b"pdfstartlink", b""),
    (b"endlink", b"pdfendlink", b""),
    (b"startthread", b"pdfstartthread", b""),
    (b"endthread", b"pdfendthread", b""),
    (b"thread", b"pdfthread", b""),
    (b"outline", b"pdfoutline", b""),
    (b"glyphtounicode", b"pdfglyphtounicode", b""),
    (b"catalog", b"pdfcatalog", b""),
    (b"fontattr", b"pdffontattr", b""),
    (b"mapfile", b"pdfmapfile", b""),
    (b"mapline", b"pdfmapline", b""),
    (b"includechars", b"pdfincludechars", b""),
    (b"info", b"pdfinfo", b""),
    (b"names", b"pdfnames", b""),
    (b"trailer", b"pdftrailer", b""),
];

/// `\pdffeedback` keys (textoken.c `do_feedback_pdf`): pdfTeX command and
/// whether its value is an internal integer (expanded through `\number`).
pub(crate) static PDF_FEEDBACKS: &[(&[u8], &[u8], bool)] = &[
    (b"lastlink", b"pdflastlink", true),
    (b"lastobj", b"pdflastobj", true),
    (b"lastannot", b"pdflastannot", true),
    (b"retval", b"pdfretval", true),
    (b"xformname", b"pdfxformname", false),
    (b"creationdate", b"pdfcreationdate", false),
    (b"fontname", b"pdffontname", false),
    (b"fontobjnum", b"pdffontobjnum", false),
    (b"fontsize", b"pdffontsize", false),
    (b"pageref", b"pdfpageref", false),
    (b"colorstackinit", b"pdfcolorstackinit", false),
];

/// Name prefix of the hidden control sequences `\pdfvariable`,
/// `\pdfextension` and `\pdffeedback` insert: their meanings are the
/// pdfTeX implementations of the backend parameters and commands.
pub(crate) const BACKEND_PREFIX: &[u8] = b"\xFF\x00BKD";

impl Engine {
    /// Resolve the LuaTeX primitive table against the primitives
    /// `init_primitives` registered (before any LuaTeX undefinitions).
    pub(crate) fn resolve_lua_primitives(&mut self) -> (Vec<LuaPrimitive>, Vec<(Vec<u8>, Equiv)>) {
        let lookup = |e: &Engine, name: &[u8]| -> Option<Equiv> {
            if let Some(p) = lua_backend_param(name) {
                return Some(Equiv::Prim(p));
            }
            match e.eqtb.get(e.cs.lookup(name)?) {
                Some(eq @ (Equiv::Prim(_) | Equiv::FontRef(_))) => Some(eq.clone()),
                _ => None,
            }
        };
        let mut table = Vec::with_capacity(LUATEX_PRIMITIVES.len());
        for &(group, name) in LUATEX_PRIMITIVES {
            let equiv = if name == b"alignmark" {
                // chr '#' of `mac_param_cmd` (commands.c)
                Some(Equiv::CharTok(Token::char(6, u32::from(b'#')).0))
            } else if name == b"aligntab" {
                Some(Equiv::CharTok(Token::char(4, u32::from(b'&')).0))
            } else if let Some(p) = luatex_only(name) {
                Some(Equiv::Prim(p))
            } else if let Some(&(_, target)) = ALIASES.iter().find(|(n, _)| *n == name) {
                lookup(self, target)
            } else {
                lookup(self, name)
            };
            if let Some(equiv) = equiv {
                table.push(LuaPrimitive { group, name, equiv });
            }
        }
        let mut backend = Vec::new();
        let targets = PDF_VARIABLES.iter().map(|(_, t)| *t)
            .chain(PDF_EXTENSIONS.iter().map(|(_, t, _)| *t))
            .chain(PDF_FEEDBACKS.iter().map(|(_, t, _)| *t))
            .chain([b"number" as &[u8], b"special"]);
        for target in targets {
            if let Some(equiv) = lookup(self, target) {
                let mut hidden = BACKEND_PREFIX.to_vec();
                hidden.extend_from_slice(target);
                if !backend.iter().any(|(h, _)| *h == hidden) {
                    backend.push((hidden, equiv));
                }
            }
        }
        (table, backend)
    }

    /// Switch a freshly initialised engine to LuaTeX's primitive model:
    /// only the `tex` group and `\directlua` stay defined (plus ratex's own
    /// extensions), primitives print by their LuaTeX names and the LuaTeX
    /// INITEX parameter values apply.
    pub fn init_luatex_primitives(&mut self) {
        if self.engine_kind != EngineKind::LuaTeX {
            return;
        }
        let (table, backend) = self.resolve_lua_primitives();
        for id in self.cs.all_ids() {
            let name = self.cs.name(id);
            if name.starts_with(b"Ratex") || name.starts_with(b"ratex") {
                continue;
            }
            if matches!(self.eqtb.get(id), Some(Equiv::Prim(_))) {
                self.eqtb.undefine(id, true);
            }
        }
        for entry in &table {
            if entry.group == TEX || entry.name == b"directlua" {
                let id = self.cs.intern(entry.name);
                self.eqtb.assign(id, entry.equiv.clone(), true);
            }
        }
        self.install_lua_primitive_table(table, backend);
        let ints = &mut self.eqtb.int_params;
        for (p, v) in [
            (IntParam::PdfOutput, 0),
            (IntParam::PdfMajorVersion, 1),
            (IntParam::PdfMinorVersion, 0),
            (IntParam::PdfCompressLevel, 0),
            (IntParam::PdfObjCompressLevel, 0),
            (IntParam::PdfDecimalDigits, 0),
            (IntParam::PdfPkResolution, 0),
            (IntParam::ExHyphenChar, 45),
            (IntParam::FirstValidLanguage, 0),
            (IntParam::ShowStream, -1),
            (IntParam::CompoundHyphenMode, 1),
            (IntParam::MathDefaultsMode, 1),
            (IntParam::MathEqnoGapStep, 1000),
            (IntParam::MathFlattenMode, 1),
            (IntParam::MathScriptBoxMode, 1),
            (IntParam::MathScriptCharMode, 1),
            (IntParam::OutputBox, 255),
            (IntParam::PreBinOpPenalty, 10000),
            (IntParam::PreRelPenalty, 10000),
            (IntParam::PreDisplayGapFactor, 2000),
            (IntParam::VariableFam, -1),
        ] {
            ints[p.idx() as usize] = v;
        }
        let dims = &mut self.eqtb.dim_params;
        dims[DimParam::PdfHOrigin.idx() as usize] = 0;
        dims[DimParam::PdfVOrigin.idx() as usize] = 0;
        dims[DimParam::PdfPxDimen.idx() as usize] = 65781;
        for p in [DimParam::PageTopOffset, DimParam::PageLeftOffset, DimParam::PageBottomOffset, DimParam::PageRightOffset] {
            dims[p.idx() as usize] = 4736287;
        }
    }

    /// Store a resolved table: hidden backend control sequences get their
    /// meanings and primitives print by their LuaTeX names (the last name
    /// wins, so `\-` shows as `\explicitdiscretionary`).
    pub(crate) fn install_lua_primitive_table(
        &mut self,
        table: Vec<LuaPrimitive>,
        backend: Vec<(Vec<u8>, Equiv)>,
    ) {
        for (name, equiv) in backend {
            let id = self.cs.intern(&name);
            self.eqtb.assign(id, equiv, true);
        }
        // `\primitive` (`prim_lookup`) knows every LuaTeX primitive,
        // enabled or not.
        self.primitive_table.clear();
        for entry in &table {
            if let Equiv::Prim(p) = entry.equiv {
                self.primitive_names.insert(p.code(), entry.name);
                self.primitive_table.insert(entry.name.into(), p);
            }
        }
        self.lua_primitives = table;
    }

    /// The meaning of LuaTeX primitive `name` (`prim_lookup`), defined or not.
    pub(crate) fn lua_primitive_lookup(&self, name: &[u8]) -> Option<&LuaPrimitive> {
        self.lua_primitives.iter().find(|p| p.name == name)
    }

    /// `tex.enableprimitives(prefix, names)` (ltexlib.c): define
    /// `prefix..name` (or `name` itself when it already starts with
    /// `prefix`) globally for each primitive name, unless that control
    /// sequence is already defined; names that are not primitives are
    /// skipped.
    pub(crate) fn lua_enable_primitives(&mut self, prefix: &[u8], names: &[Vec<u8>]) {
        for name in names {
            let Some(equiv) = self.lua_primitive_lookup(name).map(|p| p.equiv.clone()) else {
                continue;
            };
            let full = if name.starts_with(prefix) {
                name.clone()
            } else {
                [prefix, name.as_slice()].concat()
            };
            let id = self.cs.intern(&full);
            if self.eqtb.get(id).is_none() {
                self.eqtb.assign(id, equiv, true);
            }
        }
    }

    /// `tex.extraprimitives(groups)` / `tex.primitives()`: the implemented
    /// primitive names of the groups in `mask`, in LuaTeX's table order.
    pub(crate) fn lua_primitive_names(&self, mask: u8) -> Vec<&'static [u8]> {
        self.lua_primitives
            .iter()
            .filter(|p| p.group & mask != 0)
            .map(|p| p.name)
            .collect()
    }

    /// The hidden control sequence whose meaning is the ratex primitive
    /// registered as `target` (see [`BACKEND_PREFIX`]).
    pub(crate) fn backend_cs(&self, target: &[u8]) -> Option<crate::token::CsId> {
        let mut hidden = BACKEND_PREFIX.to_vec();
        hidden.extend_from_slice(target);
        self.cs.lookup(&hidden)
    }
}

impl Engine {
    fn output_mode(&self) -> i32 {
        self.eqtb.int_params[IntParam::PdfOutput.idx() as usize]
    }

    fn backend_warning(&mut self, backend: &str, what: &str) {
        self.warning_at(&format!("({backend} backend): unexpected use of \\{what}"), None);
    }

    /// `\pdfvariable <key>` (textoken.c `do_variable_pdf`): insert the
    /// backend parameter so it can be read or assigned; an unknown key
    /// warns and leaves the input alone.
    pub(crate) fn expand_pdf_variable(&mut self) {
        for &(key, target) in PDF_VARIABLES {
            if self.scan_keyword(key) {
                if let Some(id) = self.backend_cs(target) {
                    self.push_token(Token::from_cs(id));
                }
                return;
            }
        }
        self.backend_warning("pdf", "pdfvariable");
    }

    /// `\pdffeedback <key>` (textoken.c `do_feedback`): only in PDF mode.
    pub(crate) fn expand_pdf_feedback(&mut self) {
        if self.output_mode() <= 0 {
            self.error("unexpected use of \\pdffeedback");
            return;
        }
        if self.scan_keyword(b"version") {
            self.exp_string(b"140");
            return;
        }
        if self.scan_keyword(b"revision") {
            self.exp_string(b"0");
            return;
        }
        for &(key, target, integer) in PDF_FEEDBACKS {
            if self.scan_keyword(key) {
                let mut toks = Vec::with_capacity(2);
                if integer {
                    toks.extend(self.backend_cs(b"number").map(Token::from_cs));
                }
                toks.extend(self.backend_cs(target).map(Token::from_cs));
                self.push_tokens(toks);
                return;
            }
        }
        self.backend_warning("pdf", "pdffeedback");
    }

    /// `\dvifeedback`: LuaTeX has no DVI feedback keys.
    pub(crate) fn expand_dvi_feedback(&mut self) {
        if self.output_mode() == 0 {
            self.backend_warning("dvi", "dvifeedback");
        } else {
            self.error("unexpected use of \\dvifeedback");
        }
    }

    /// `\pdfextension <key> ...` (extensions.c `do_extension_pdf`): the
    /// pdfTeX command for the key reads the rest; nothing happens outside
    /// PDF mode.
    pub(crate) fn do_pdf_extension(&mut self) {
        if self.output_mode() <= 0 {
            return;
        }
        for &(key, target, keyword) in PDF_EXTENSIONS {
            if self.scan_keyword(key) {
                let deferred = std::mem::take(&mut self.lua_deferred);
                let mut toks: Vec<Token> = self.backend_cs(target).map(Token::from_cs).into_iter().collect();
                if key == b"literal" && deferred && !self.scan_keyword(b"shipout") {
                    toks.extend(b"shipout ".iter().map(|&c| Token::char(if c == b' ' { 10 } else { 11 }, u32::from(c))));
                }
                toks.extend(keyword.iter().map(|&c| Token::char(11, u32::from(c))));
                if !keyword.is_empty() {
                    toks.push(Token::char(10, u32::from(b' ')));
                }
                self.push_tokens(toks);
                return;
            }
        }
        self.lua_deferred = false;
        self.error("unexpected use of \\pdfextension");
    }

    /// `\dviextension` (extensions.c `do_extension_dvi`): `literal` and
    /// `lateliteral` are `\special`s, in DVI mode only.
    pub(crate) fn do_dvi_extension(&mut self) {
        self.lua_deferred = false;
        if self.output_mode() != 0 {
            return;
        }
        if self.scan_keyword(b"literal") {
            let _ = self.scan_keyword(b"shipout");
        } else if !self.scan_keyword(b"lateliteral") {
            self.error("unexpected use of \\dviextension");
            return;
        }
        if let Some(id) = self.backend_cs(b"special") {
            self.push_token(Token::from_cs(id));
        }
    }

    /// LuaTeX `\deferred`: the next extension command is a shipout-time one
    /// (writes, opens and closes already are).
    pub(crate) fn do_deferred(&mut self) {
        let t = self.get_x_raw();
        if t.is_cs() {
            if let Some(Equiv::Prim(Prim::PdfExtension | Prim::DviExtension)) = self.eqtb.resolve(t.cs_id()) {
                self.lua_deferred = true;
            }
        }
        self.push_token(t);
    }

    /// LuaTeX `\csstring`: `\string` without the escape character.
    pub(crate) fn expand_csstring(&mut self, id: crate::token::CsId) {
        let idx = IntParam::EscapeChar.idx() as usize;
        let escape = std::mem::replace(&mut self.eqtb.int_params[idx], -1);
        let _ = self.expand_prim(Prim::String, id);
        self.eqtb.int_params[idx] = escape;
    }

    /// LuaTeX `\luaescapestring {<text>}` (lua_str_toks): the expanded text
    /// with `\`, `"` and `'` escaped by a backslash and LF/CR written as
    /// `\n`/`\r`.
    pub(crate) fn expand_lua_escape_string(&mut self) {
        let toks = self.scan_general_text_expanded();
        let text = self.print_tokens_to_string(&toks);
        let mut out = Vec::with_capacity(text.len());
        for &b in text.as_bytes() {
            match b {
                b'\\' | b'"' | b'\'' => out.extend_from_slice(&[b'\\', b]),
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\r' => out.extend_from_slice(b"\\r"),
                _ => out.push(b),
            }
        }
        self.exp_string(&out);
    }

    /// LuaTeX `\toksapp` & co. (textoken.c `combine_the_toks`): append or
    /// prepend a braced text (expanded for the `e`/`x` forms) or another
    /// register to a token register; the `g`/`x` forms assign globally
    /// unless the register was set at the current group level, which is
    /// updated in place.
    pub(crate) fn combine_the_toks(&mut self, p: Prim) {
        use Prim::*;
        let append = matches!(p, ToksApp | EToksApp | GToksApp | XToksApp);
        let expand = matches!(p, EToksApp | EToksPre | XToksApp | XToksPre);
        let global = matches!(p, GToksApp | GToksPre | XToksApp | XToksPre);
        let target = self.scan_toks_register();
        let source: Vec<Token> = loop {
            let t = self.get_x_raw();
            if t.is_char() && t.cc() == 10 {
                continue;
            }
            if self.token_is_left_brace(t) {
                self.push_token(t);
                break if expand {
                    self.scan_general_text_expanded()
                } else {
                    self.scan_general_text()
                };
            }
            self.push_token(t);
            let n = self.scan_toks_register();
            break self.eqtb.toks[n as usize].as_ref().clone();
        };
        if source.is_empty() {
            return;
        }
        let old = &self.eqtb.toks[target as usize];
        let mut combined = Vec::with_capacity(old.len() + source.len());
        if append {
            combined.extend_from_slice(old);
            combined.extend_from_slice(&source);
        } else {
            combined.extend_from_slice(&source);
            combined.extend_from_slice(old);
        }
        let in_place = self.eqtb.toks_levels[target as usize] == self.eqtb.cur_level;
        self.eqtb.assign_toks_reg(target, std::rc::Rc::new(combined), global && !in_place);
    }

    /// A token register: a `\toksdef` token or a register number.
    fn scan_toks_register(&mut self) -> u16 {
        let t = self.get_x_raw();
        if t.is_cs() {
            match self.eqtb.resolve(t.cs_id()) {
                Some(&Equiv::ToksReg(n)) => return n,
                Some(Equiv::Prim(Prim::Toks)) => return self.scan_reg_num(),
                _ => {}
            }
        }
        self.push_token(t);
        self.scan_reg_num()
    }

    /// LuaTeX `\boundary`, `\wordboundary`, `\protrusionboundary`
    /// (maincontrol.c `append_boundary`): a boundary node in any mode.
    pub(crate) fn append_boundary(&mut self, p: Prim) {
        let (kind, value) = match p {
            Prim::Boundary => (crate::boxes::BOUNDARY_USER, self.scan_int()),
            Prim::ProtrusionBoundary => (crate::boxes::BOUNDARY_PROTRUSION, self.scan_int()),
            _ => (crate::boxes::BOUNDARY_WORD, 0),
        };
        self.flush_native_text();
        self.cur_list.push(crate::boxes::Node::Whatsit(crate::boxes::WhatIt::Boundary { kind, value }));
    }

    /// LuaTeX `\letcharcode <number> <token>`: `\let` the active character.
    pub(crate) fn do_letcharcode(&mut self) {
        let global = self.take_global();
        let n = self.scan_int();
        match u32::try_from(n).ok().filter(|&c| c > 0 && char::from_u32(c).is_some()) {
            Some(c) => {
                let target = self.active_cs_id(c);
                self.let_target(target, global);
            }
            None => {
                self.error("invalid number for \\letcharcode");
                self.clear_prefixes();
            }
        }
    }

    /// LuaTeX `\formatname`: the name of the loaded format.
    pub(crate) fn expand_format_name(&mut self) {
        let name = self.format_name.clone();
        self.exp_string(name.as_bytes());
    }
}

/// LuaTeX 1.24's primitive table in `tex.primitives()` order, with the
/// group `tex.extraprimitives` reports for each name.
pub(crate) static LUATEX_PRIMITIVES: &[(u8, &[u8])] = &[
    (TEX, b"vskip"), (LUATEX, b"Umathcloseopspacing"), (ETEX, b"unless"), (ETEX, b"botmarks"),
    (LUATEX, b"textdir"), (TEX, b"write"), (TEX, b"vsize"), (LUATEX, b"Umathordpunctspacing"),
    (ETEX, b"currentiftype"), (LUATEX, b"Udelimiterunder"), (LUATEX, b"variablefam"),
    (TEX, b"boundary"), (TEX, b"unhcopy"), (ETEX, b"pagediscards"), (LUATEX, b"mathsurroundmode"),
    (TEX, b"output"), (TEX, b"-"), (TEX, b"/"), (ETEX, b"ignoreprimitiveerror"),
    (LUATEX, b"Uskewedwithdelims"), (TEX, b"unskip"), (LUATEX, b"bodydirection"), (TEX, b"unvbox"),
    (LUATEX, b"Umathopenpunctspacing"), (TEX, b"boxmaxdepth"), (TEX, b"muskipdef"),
    (TEX, b"string"), (LUATEX, b"pagebottomoffset"), (ETEX, b"partokencontext"),
    (LUATEX, b"luabytecodecall"), (LUATEX, b"mathsurroundskip"), (TEX, b"toksdef"),
    (ETEX, b"displaywidowpenalties"), (LUATEX, b"endlocalcontrol"),
    (LUATEX, b"Umathordinnerspacing"), (LUATEX, b"Umathbinclosespacing"),
    (TEX, b"floatingpenalty"), (TEX, b"righthyphenmin"), (TEX, b"voffset"), (LUATEX, b"toksapp"),
    (LUATEX, b"rightghost"), (ETEX, b"fontcharic"), (ETEX, b"fontchardp"), (TEX, b"escapechar"),
    (ETEX, b"fontcharht"), (LUATEX, b"Umathlimitbelowbgap"), (TEX, b"topmark"),
    (ETEX, b"fontcharwd"), (LUATEX, b"Umathopeninnerspacing"), (ETEX, b"partokenname"),
    (LUATEX, b"textdirection"), (TEX, b"splitfirstmark"), (TEX, b"vsplit"), (TEX, b"everydisplay"),
    (TEX, b"badness"), (LUATEX, b"tokspre"), (TEX, b"xleaders"), (TEX, b"textfont"),
    (TEX, b"showlists"), (TEX, b"language"), (LUATEX, b"Umathnolimitsubfactor"),
    (TEX, b"mathchoice"), (LUATEX, b"Uoverdelimiter"), (LUATEX, b"Umathpunctpunctspacing"),
    (LUATEX, b"Umathclosepunctspacing"), (LUATEX, b"mathdisplayskipmode"),
    (LUATEX, b"saveimageresource"), (LUATEX, b"mathrulesfam"), (LUATEX, b"Umathrelordspacing"),
    (TEX, b"topskip"), (TEX, b"abovedisplayshortskip"), (TEX, b"underline"),
    (LUATEX, b"Umathsupbottommin"), (LUATEX, b"Umathlimitbelowkern"), (TEX, b"tracinglostchars"),
    (LUATEX, b"copyfont"), (LUATEX, b"pagedirection"), (LUATEX, b"Umathstackdenomdown"),
    (LUATEX, b"localrightbox"), (LUATEX, b"Umathfractionrule"), (TEX, b"pagefillstretch"),
    (TEX, b"unvcopy"), (ETEX, b"widowpenalties"), (TEX, b"splitbotmark"),
    (LUATEX, b"Umathcharfam"), (TEX, b"finalhyphendemerits"), (LUATEX, b"Umathcloseinnerspacing"),
    (LUATEX, b"Umathopenrelspacing"), (TEX, b"atopwithdelims"), (ETEX, b"tracingifs"),
    (LUATEX, b"Uhextensible"), (LUATEX, b"Umathsupsubbottommax"), (TEX, b"pretolerance"),
    (LUATEX, b"leftmarginkern"), (ETEX, b"iffontchar"), (LUATEX, b"Umathcloserelspacing"),
    (LUATEX, b"linedirection"), (TEX, b"fi"), (TEX, b"dp"), (ETEX, b"eTeXVersion"),
    (TEX, b"setlanguage"), (TEX, b"ht"), (TEX, b"mathchardef"), (LUATEX, b"ifincsname"),
    (TEX, b"nulldelimiterspace"), (TEX, b"or"), (TEX, b"wd"), (LUATEX, b"Umathcharnum"),
    (LUATEX, b"Umathinnerordspacing"), (LUATEX, b"synctex"), (LUATEX, b"luabytecode"),
    (LUATEX, b"formatname"), (TEX, b"pagegoal"), (TEX, b"advance"), (LUATEX, b"letterspacefont"),
    (LUATEX, b"boxdirection"), (LUATEX, b"pdfextension"), (ETEX, b"protected"), (TEX, b"chardef"),
    (TEX, b"catcode"), (TEX, b"mathchar"), (LUATEX, b"discretionaryligaturemode"),
    (LUATEX, b"Umathrelinnerspacing"), (ETEX, b"topmarks"), (ETEX, b"showgroups"),
    (TEX, b"scriptscriptfont"), (TEX, b"mathcode"), (TEX, b"leftskip"),
    (LUATEX, b"Umathsubtopmax"), (ETEX, b"glueexpr"), (LUATEX, b"randomseed"),
    (TEX, b"pageshrink"), (ETEX, b"splitfirstmarks"), (LUATEX, b"suppressoutererror"),
    (ETEX, b"predisplaydirection"), (LUATEX, b"Umathsubsupshiftdown"), (TEX, b"pagefilstretch"),
    (LUATEX, b"Umathopbinspacing"), (TEX, b"delcode"), (LUATEX, b"Umathordbinspacing"),
    (TEX, b"fontname"), (LUATEX, b"Umathrelopspacing"), (TEX, b"brokenpenalty"),
    (LUATEX, b"Umathopenbinspacing"), (LUATEX, b"suppressprimitiveerror"),
    (LUATEX, b"Umathoverdelimiterbgap"), (LUATEX, b"localleftbox"), (LUATEX, b"alignmark"),
    (LUATEX, b"Uunderdelimiter"), (LUATEX, b"hyphenationmin"), (LUATEX, b"Umathclosebinspacing"),
    (LUATEX, b"Umathcodenum"), (LUATEX, b"dvifeedback"), (TEX, b"lastkern"),
    (LUATEX, b"outputmode"), (TEX, b"belowdisplayshortskip"), (TEX, b"tolerance"),
    (TEX, b"mathopen"), (LUATEX, b"luafunction"), (TEX, b"exhyphenpenalty"),
    (LUATEX, b"compoundhyphenmode"), (TEX, b"maxdepth"), (LUATEX, b"Umathpunctopenspacing"),
    (LUATEX, b"luacopyinputnodes"), (LUATEX, b"Umathconnectoroverlapmin"), (TEX, b"futurelet"),
    (TEX, b"abovewithdelims"), (LUATEX, b"crampedscriptscriptstyle"), (LUATEX, b"csstring"),
    (TEX, b"hangindent"), (LUATEX, b"Umathradicaldegreeafter"), (ETEX, b"everyeof"),
    (TEX, b"lastskip"), (ETEX, b"eTeXversion"), (LUATEX, b"uniformdeviate"), (TEX, b"linepenalty"),
    (LUATEX, b"luatexversion"), (TEX, b"everyjob"), (TEX, b"xspaceskip"), (TEX, b"globaldefs"),
    (LUATEX, b"Umathfractionnumup"), (LUATEX, b"rightmarginkern"), (TEX, b"everypar"),
    (LUATEX, b"Umathopclosespacing"), (TEX, b"scriptfont"), (ETEX, b"clubpenalties"),
    (LUATEX, b"mathrulesmode"), (TEX, b"delimiter"), (LUATEX, b"explicithyphenpenalty"),
    (LUATEX, b"Umathordclosespacing"), (ETEX, b"savingvdiscards"), (ETEX, b"splitbotmarks"),
    (LUATEX, b"Umathoverdelimitervgap"), (LUATEX, b"etokspre"), (TEX, b"afterassignment"),
    (TEX, b"firstmark"), (LUATEX, b"expanded"), (LUATEX, b"suppressmathparerror"),
    (LUATEX, b"Udelcode"), (LUATEX, b"bodydir"), (TEX, b"wordboundary"), (ETEX, b"showtokens"),
    (LUATEX, b"immediateassigned"), (ETEX, b"tracingassigns"), (LUATEX, b"shapemode"),
    (ETEX, b"dimexpr"), (TEX, b"lineskiplimit"), (TEX, b"lineskip"), (TEX, b"def"),
    (ETEX, b"parshapedimen"), (LUATEX, b"attribute"), (ETEX, b"readline"), (TEX, b"fam"),
    (TEX, b"day"), (TEX, b"iffalse"), (TEX, b"textstyle"), (TEX, b"end"), (TEX, b"mag"),
    (TEX, b"box"), (TEX, b"belowdisplayskip"), (LUATEX, b"Umathsubshiftdrop"),
    (LUATEX, b"Umathsubshiftdown"), (TEX, b"ifx"), (LUATEX, b"matheqnogapstep"),
    (LUATEX, b"Umathpunctrelspacing"), (TEX, b"let"), (TEX, b"errmessage"),
    (LUATEX, b"lastsavedimageresourceindex"), (TEX, b"exhyphenchar"), (TEX, b"hss"),
    (LUATEX, b"mathemptydisplaymode"), (TEX, b"expandafter"),
    (LUATEX, b"lastsavedimageresourcepages"), (LUATEX, b"mathoption"), (TEX, b"the"),
    (TEX, b"displaywidth"), (LUATEX, b"Umathradicaldegreeraise"), (LUATEX, b"fixupboxesmode"),
    (TEX, b"Uright"), (TEX, b"mathsurround"), (TEX, b"pagedepth"), (LUATEX, b"adjustspacing"),
    (TEX, b"looseness"), (LUATEX, b"Umathsupshiftdrop"), (LUATEX, b"Umathcharslot"),
    (TEX, b"leaders"), (LUATEX, b"Umathcloseclosespacing"), (LUATEX, b"luatexrevision"),
    (TEX, b"vss"), (LUATEX, b"insertht"), (LUATEX, b"localinterlinepenalty"),
    (LUATEX, b"useboxresource"), (TEX, b"ifhmode"), (LUATEX, b"explicitdiscretionary"),
    (LUATEX, b"Umathchar"), (TEX, b"botmark"), (LUATEX, b"Udelimiterover"), (LUATEX, b"Ustack"),
    (LUATEX, b"Umathcode"), (LUATEX, b"mathdelimitersmode"), (LUATEX, b"saveboxresource"),
    (LUATEX, b"Udelcodenum"), (LUATEX, b"gtoksapp"), (ETEX, b"tracingscantokens"),
    (LUATEX, b"suppresslongerror"), (TEX, b"displaystyle"), (LUATEX, b"ignoreligaturesinfont"),
    (TEX, b"accent"), (TEX, b"immediate"), (LUATEX, b"Umathaxis"),
    (LUATEX, b"Umathfractionnumvgap"), (TEX, b"ifmmode"), (LUATEX, b"gtokspre"),
    (TEX, b"parshape"), (LUATEX, b"Umathskewedfractionhgap"), (TEX, b"deferred"),
    (LUATEX, b"Umathrelclosespacing"), (TEX, b"meaning"), (LUATEX, b"Umathpunctbinspacing"),
    (TEX, b"abovedisplayskip"), (TEX, b"medmuskip"), (TEX, b"emergencystretch"),
    (LUATEX, b"Ustopdisplaymath"), (LUATEX, b"quitvmode"), (TEX, b"rightskip"),
    (TEX, b"mathclose"), (TEX, b"hangafter"), (TEX, b"hoffset"), (LUATEX, b"crampedscriptstyle"),
    (LUATEX, b"letcharcode"), (LUATEX, b"setrandomseed"), (LUATEX, b"hyphenationbounds"),
    (LUATEX, b"crampedtextstyle"), (LUATEX, b"pagedir"), (LUATEX, b"Umathbinrelspacing"),
    (TEX, b"aftergroup"), (LUATEX, b"Umathopordspacing"), (LUATEX, b"dvivariable"),
    (LUATEX, b"attributedef"), (LUATEX, b"mathdirection"), (LUATEX, b"Umathordordspacing"),
    (LUATEX, b"pdffeedback"), (TEX, b"cleaders"), (TEX, b"romannumeral"), (TEX, b"hbadness"),
    (TEX, b"mathbin"), (LUATEX, b"Umathskewedfractionvgap"), (LUATEX, b"Umathopenordspacing"),
    (TEX, b"showboxbreadth"), (LUATEX, b"mathitalicsmode"), (TEX, b"ifvmode"), (TEX, b"jobname"),
    (LUATEX, b"mathdir"), (TEX, b"vbadness"), (TEX, b"patterns"), (TEX, b"nonstopmode"),
    (TEX, b"errhelp"), (TEX, b"predisplaypenalty"), (LUATEX, b"outputbox"),
    (LUATEX, b"Umathcloseordspacing"), (LUATEX, b"Umathnolimitsupfactor"), (TEX, b"endlinechar"),
    (TEX, b"mathinner"), (TEX, b"lastbox"), (TEX, b"showboxdepth"), (LUATEX, b"pagewidth"),
    (TEX, b"postdisplaypenalty"), (LUATEX, b"Ustopmath"), (LUATEX, b"aligntab"), (TEX, b"mathrel"),
    (TEX, b"holdinginserts"), (TEX, b"radical"), (TEX, b"mathord"), (LUATEX, b"prehyphenchar"),
    (LUATEX, b"dviextension"), (TEX, b"pagetotal"), (LUATEX, b"luafunctioncall"),
    (LUATEX, b"Umathpunctopspacing"), (TEX, b"everycr"), (LUATEX, b"breakafterdirmode"),
    (TEX, b"adjdemerits"), (LUATEX, b"Umathsubsupvgap"), (LUATEX, b"luaescapestring"),
    (LUATEX, b"prerelpenalty"), (TEX, b"halign"), (LUATEX, b"begincsname"),
    (TEX, b"defaultskewchar"), (ETEX, b"tracingnesting"), (TEX, b"errorcontextlines"),
    (LUATEX, b"Umathradicalrule"), (TEX, b"splitmaxdepth"), (TEX, b"Uleft"), (TEX, b"ifcase"),
    (LUATEX, b"Umathunderbarrule"), (TEX, b"noindent"), (LUATEX, b"postexhyphenchar"),
    (LUATEX, b"Umathradicaldegreebefore"), (TEX, b"tracingmacros"), (TEX, b"moveright"),
    (TEX, b"predisplaysize"), (LUATEX, b"Umathstacknumup"), (TEX, b"tracingrestores"),
    (TEX, b"message"), (TEX, b"ifhbox"), (TEX, b"deadcycles"), (LUATEX, b"normaldeviate"),
    (TEX, b"interlinepenalty"), (TEX, b"mathpunct"), (TEX, b"lccode"), (ETEX, b"ifdefined"),
    (TEX, b"noboundary"), (TEX, b"displayindent"), (LUATEX, b"Umathbinopspacing"),
    (TEX, b"nonscript"), (LUATEX, b"xtoksapp"), (TEX, b"everyhbox"), (LUATEX, b"boxdir"),
    (LUATEX, b"Ustartdisplaymath"), (LUATEX, b"savecatcodetable"),
    (LUATEX, b"Umathbinpunctspacing"), (LUATEX, b"eTeXglueshrinkorder"),
    (LUATEX, b"mathscriptboxmode"), (LUATEX, b"tagcode"), (TEX, b"global"), (LUATEX, b"Uroot"),
    (LUATEX, b"lastsavedboxresourceindex"), (TEX, b"penalty"), (TEX, b"tracingcommands"),
    (TEX, b"everymath"), (LUATEX, b"Unosuperscript"), (TEX, b"nolimits"), (TEX, b"noalign"),
    (LUATEX, b"Umathoperatorsize"), (CORE, b"directlua"), (LUATEX, b"xtokspre"),
    (TEX, b"inputlineno"), (LUATEX, b"Uradical"), (TEX, b"pagestretch"), (TEX, b"parskip"),
    (TEX, b"indent"), (TEX, b"dimendef"), (LUATEX, b"mathstyle"), (LUATEX, b"Umathopopenspacing"),
    (TEX, b"widowpenalty"), (TEX, b"ifvbox"), (TEX, b"above"), (LUATEX, b"Umathordopenspacing"),
    (LUATEX, b"automatichyphenpenalty"), (LUATEX, b"Umathbininnerspacing"), (TEX, b"spaceskip"),
    (TEX, b"middle"), (LUATEX, b"Umathinnerrelspacing"), (LUATEX, b"clearmarks"),
    (LUATEX, b"Umathoverbarvgap"), (LUATEX, b"fontid"), (TEX, b"displaylimits"), (TEX, b"pausing"),
    (LUATEX, b"Umathopenopenspacing"), (LUATEX, b"immediateassignment"), (TEX, b"everyvbox"),
    (TEX, b"iftrue"), (TEX, b"moveleft"), (TEX, b"mathop"), (LUATEX, b"Umathunderdelimiterbgap"),
    (LUATEX, b"Umathoverbarrule"), (TEX, b"endcsname"), (LUATEX, b"setfontid"),
    (LUATEX, b"crampeddisplaystyle"), (LUATEX, b"ifabsdim"), (LUATEX, b"Umathlimitabovebgap"),
    (LUATEX, b"Umathcharclass"), (TEX, b"dimen"), (LUATEX, b"Umathstackvgap"),
    (LUATEX, b"Umathinneropspacing"), (ETEX, b"currentifbranch"), (LUATEX, b"Umathrelbinspacing"),
    (TEX, b"ifcat"), (TEX, b"clubpenalty"), (TEX, b"splittopskip"),
    (LUATEX, b"Umathcloseopenspacing"), (LUATEX, b"ifcondition"), (TEX, b"doublehyphendemerits"),
    (TEX, b"ifdim"), (LUATEX, b"pardir"), (TEX, b"limits"), (TEX, b"ifeof"), (ETEX, b"firstmarks"),
    (TEX, b"ignorespaces"), (LUATEX, b"initcatcodetable"), (TEX, b"insert"),
    (TEX, b"delimitershortfall"), (ETEX, b"lastnodetype"), (TEX, b"ifodd"), (LUATEX, b"nokerns"),
    (LUATEX, b"pageleftoffset"), (TEX, b"insertpenalties"), (TEX, b"tracingpages"),
    (TEX, b"hpack"), (LUATEX, b"luadef"), (TEX, b"vadjust"), (LUATEX, b"tracingfonts"),
    (LUATEX, b"nospaces"), (TEX, b"tracingonline"), (LUATEX, b"Umathrelopenspacing"),
    (TEX, b"count"), (LUATEX, b"Umathlimitabovekern"), (TEX, b"ifnum"), (LUATEX, b"Udelimiter"),
    (LUATEX, b"savepos"), (TEX, b"edef"), (LUATEX, b"nohrule"), (TEX, b"char"),
    (TEX, b"begingroup"), (TEX, b"sfcode"), (TEX, b"tracingparagraphs"), (TEX, b"hyphenation"),
    (ETEX, b"marks"), (LUATEX, b"localbrokenpenalty"), (LUATEX, b"Umathfractiondelsize"),
    (LUATEX, b"exceptionpenalty"), (TEX, b"hfuzz"), (TEX, b"openout"),
    (LUATEX, b"automaticdiscretionary"), (ETEX, b"currentgrouplevel"), (TEX, b"leqno"),
    (LUATEX, b"gleaders"), (LUATEX, b"Umathunderdelimitervgap"), (LUATEX, b"Umathinnerbinspacing"),
    (TEX, b"hyphenpenalty"), (TEX, b"vcenter"), (TEX, b"hfil"), (TEX, b"thickmuskip"),
    (TEX, b"maxdeadcycles"), (TEX, b"mkern"), (TEX, b"hbox"), (TEX, b"overfullrule"),
    (LUATEX, b"noligs"), (TEX, b"else"), (TEX, b"hsize"), (TEX, b"raise"), (TEX, b"thinmuskip"),
    (TEX, b"spacefactor"), (TEX, b"input"), (TEX, b"hrule"), (TEX, b"left"), (TEX, b"eqno"),
    (TEX, b"parfillskip"), (TEX, b"font"), (TEX, b"valign"), (TEX, b"dump"), (TEX, b"relax"),
    (LUATEX, b"hyphenpenaltymode"), (LUATEX, b"draftmode"), (TEX, b"prevdepth"), (TEX, b"read"),
    (TEX, b"shipout"), (TEX, b"batchmode"), (TEX, b"right"), (LUATEX, b"automatichyphenmode"),
    (TEX, b"setbox"), (LUATEX, b"prebinoppenalty"), (TEX, b"baselineskip"),
    (LUATEX, b"Usubscript"), (TEX, b"special"), (TEX, b"mskip"), (LUATEX, b"Umathcharnumdef"),
    (TEX, b"endgroup"), (TEX, b"uchyph"), (TEX, b"binoppenalty"), (LUATEX, b"rpcode"),
    (ETEX, b"interlinepenalties"), (TEX, b"endinput"), (TEX, b"omit"), (TEX, b"pagefilllstretch"),
    (ETEX, b"muexpr"), (TEX, b"overwithdelims"), (ETEX, b"unexpanded"), (TEX, b"newlinechar"),
    (TEX, b"vfilneg"), (TEX, b"time"), (TEX, b"tpack"), (TEX, b"skip"), (TEX, b"vfill"),
    (TEX, b"span"), (TEX, b"prevgraf"), (TEX, b"over"), (TEX, b"show"), (TEX, b"vbox"),
    (TEX, b"tracingstats"), (TEX, b"year"), (LUATEX, b"mathpenaltiesmode"), (ETEX, b"ifcsname"),
    (TEX, b"defaulthyphenchar"), (TEX, b"nullfont"), (ETEX, b"parshapeindent"),
    (LUATEX, b"mathscriptcharmode"), (TEX, b"muskip"), (TEX, b"vpack"), (TEX, b"toks"),
    (LUATEX, b"mathdefaultsmode"), (LUATEX, b"Umathaccent"), (LUATEX, b"pagetopoffset"),
    (TEX, b"outer"), (ETEX, b"showifs"), (LUATEX, b"matheqdirmode"), (TEX, b"multiply"),
    (LUATEX, b"pageheight"), (TEX, b"tracingoutput"), (TEX, b"firstvalidlanguage"),
    (LUATEX, b"catcodetable"), (TEX, b"parindent"), (ETEX, b"parshapelength"),
    (TEX, b"protrusionboundary"), (TEX, b"displaywidowpenalty"), (TEX, b"unhbox"),
    (TEX, b"lefthyphenmin"), (TEX, b"vtop"), (TEX, b"mathaccent"),
    (LUATEX, b"Umathspaceafterscript"), (LUATEX, b"predisplaygapfactor"), (LUATEX, b"primitive"),
    (LUATEX, b"Umathinneropenspacing"), (LUATEX, b"Uskewed"), (LUATEX, b"pxdimen"),
    (TEX, b"vfuzz"), (LUATEX, b"glyphdimensionsmode"), (TEX, b"overline"),
    (LUATEX, b"Umathopenopspacing"), (TEX, b"unkern"), (ETEX, b"splitdiscards"),
    (ETEX, b"gluetomu"), (ETEX, b"mutoglue"), (LUATEX, b"eTeXgluestretchorder"),
    (ETEX, b"glueshrink"), (ETEX, b"gluestretch"), (ETEX, b"glueshrinkorder"),
    (ETEX, b"gluestretchorder"), (ETEX, b"numexpr"), (LUATEX, b"ifabsnum"),
    (LUATEX, b"scantextokens"), (ETEX, b"scantokens"), (ETEX, b"interactionmode"),
    (ETEX, b"detokenize"), (TEX, b"showstream"), (ETEX, b"currentiflevel"),
    (ETEX, b"currentgrouptype"), (LUATEX, b"mathrulethicknessmode"), (LUATEX, b"mathnolimitsmode"),
    (LUATEX, b"mathflattenmode"), (LUATEX, b"mathscriptsmode"), (LUATEX, b"suppressifcsnameerror"),
    (LUATEX, b"suppressfontnotfounderror"), (ETEX, b"savinghyphcodes"), (ETEX, b"lastlinefit"),
    (ETEX, b"tracinggroups"), (ETEX, b"eTeXrevision"), (ETEX, b"eTeXminorversion"),
    (LUATEX, b"pardirection"), (LUATEX, b"pdfvariable"), (LUATEX, b"lateluafunction"),
    (LUATEX, b"latelua"), (LUATEX, b"useimageresource"), (LUATEX, b"pagerightoffset"),
    (LUATEX, b"linedir"), (TEX, b"closeout"), (TEX, b"showthe"), (TEX, b"showbox"),
    (TEX, b"uppercase"), (TEX, b"lowercase"), (TEX, b"closein"), (TEX, b"openin"),
    (TEX, b"errorstopmode"), (TEX, b"scrollmode"), (LUATEX, b"efcode"), (LUATEX, b"lpcode"),
    (TEX, b"skewchar"), (TEX, b"hyphenchar"), (LUATEX, b"hjcode"), (LUATEX, b"preexhyphenchar"),
    (LUATEX, b"posthyphenchar"), (LUATEX, b"Umathinnerinnerspacing"),
    (LUATEX, b"Umathinnerpunctspacing"), (LUATEX, b"Umathinnerclosespacing"),
    (LUATEX, b"Umathpunctinnerspacing"), (LUATEX, b"Umathpunctclosespacing"),
    (LUATEX, b"Umathpunctordspacing"), (LUATEX, b"Umathopenclosespacing"),
    (LUATEX, b"Umathrelpunctspacing"), (LUATEX, b"Umathrelrelspacing"),
    (LUATEX, b"Umathbinopenspacing"), (LUATEX, b"Umathbinbinspacing"),
    (LUATEX, b"Umathbinordspacing"), (LUATEX, b"Umathopinnerspacing"),
    (LUATEX, b"Umathoppunctspacing"), (LUATEX, b"Umathoprelspacing"),
    (LUATEX, b"Umathopopspacing"), (LUATEX, b"Umathordrelspacing"), (LUATEX, b"Umathordopspacing"),
    (LUATEX, b"Umathsupshiftup"), (LUATEX, b"Umathlimitbelowvgap"),
    (LUATEX, b"Umathlimitabovevgap"), (LUATEX, b"Umathfractiondenomdown"),
    (LUATEX, b"Umathfractiondenomvgap"), (LUATEX, b"Umathradicalvgap"),
    (LUATEX, b"Umathradicalkern"), (LUATEX, b"Umathunderbarvgap"), (LUATEX, b"Umathunderbarkern"),
    (LUATEX, b"Umathoverbarkern"), (LUATEX, b"Umathquad"), (TEX, b"uccode"), (TEX, b"skipdef"),
    (TEX, b"countdef"), (LUATEX, b"Umathchardef"), (TEX, b"glet"), (TEX, b"xdef"), (TEX, b"gdef"),
    (TEX, b"long"), (LUATEX, b"Uvextensible"), (TEX, b"Umiddle"), (TEX, b"atop"),
    (LUATEX, b"Unosubscript"), (LUATEX, b"Usuperscript"), (TEX, b"scriptscriptstyle"),
    (TEX, b"scriptstyle"), (LUATEX, b"Ustartmath"), (TEX, b"discretionary"), (TEX, b"unpenalty"),
    (TEX, b"copy"), (TEX, b"lower"), (TEX, b"kern"), (TEX, b"vfil"), (TEX, b"hfilneg"),
    (TEX, b"hfill"), (TEX, b"hskip"), (TEX, b"crcr"), (TEX, b"cr"), (LUATEX, b"ifprimitive"),
    (TEX, b"ifvoid"), (TEX, b"ifinner"), (TEX, b"if"), (LUATEX, b"Uchar"),
    (LUATEX, b"luatexbanner"), (TEX, b"number"), (LUATEX, b"lastypos"), (LUATEX, b"lastxpos"),
    (TEX, b"lastpenalty"), (TEX, b"par"), (LUATEX, b"novrule"), (TEX, b"vrule"),
    (LUATEX, b"etoksapp"), (TEX, b"noexpand"), (TEX, b"mark"), (LUATEX, b"leftghost"),
    (TEX, b"fontdimen"), (LUATEX, b"expandglyphsinfont"), (TEX, b"divide"),
    (LUATEX, b"lastnamedcs"), (TEX, b"csname"), (TEX, b" "), (TEX, b"scriptspace"),
    (LUATEX, b"protrudechars"), (TEX, b"outputpenalty"), (TEX, b"month"),
    (TEX, b"delimiterfactor"), (TEX, b"relpenalty"), (TEX, b"tabskip"),
];
