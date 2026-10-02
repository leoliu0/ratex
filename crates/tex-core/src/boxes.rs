//! Node lists, glue, boxes, and packing algorithms (hpack/vpack).

use crate::fonts::FontResolver;
use crate::scaled::{self, ONE};
use crate::tfm::FontId;

pub struct EqtbFonts<'a>(pub &'a crate::eqtb::Eqtb);

impl<'a> crate::fonts::FontResolver for EqtbFonts<'a> {
    fn char_width(&self, f: FontId, c: u8) -> i32 {
        self.0
            .fonts
            .get(f as usize)
            .map(|x| x.char_width(c))
            .unwrap_or(0)
    }
    fn char_height(&self, f: FontId, c: u8) -> i32 {
        self.0
            .fonts
            .get(f as usize)
            .map(|x| x.char_height(c))
            .unwrap_or(0)
    }
    fn char_depth(&self, f: FontId, c: u8) -> i32 {
        self.0
            .fonts
            .get(f as usize)
            .map(|x| x.char_depth(c))
            .unwrap_or(0)
    }
    fn char_italic(&self, f: FontId, c: u8) -> i32 {
        self.0
            .fonts
            .get(f as usize)
            .map(|x| x.char_italic(c))
            .unwrap_or(0)
    }
}

pub fn eqtb_fonts<'a>(eqtb: &'a crate::eqtb::Eqtb) -> EqtbFonts<'a> {
    EqtbFonts(eqtb)
}

pub const GLUE_FIL: u8 = 1;
pub const GLUE_FILL: u8 = 2;
pub const GLUE_FILLL: u8 = 3;

pub const HBOX: u8 = 0;
pub const VBOX: u8 = 1;
pub const VTOP: u8 = 2;

/// e-TeX `box_lr` values (etex.ch "reversed"/"dlist").
pub const BOX_LR_REVERSED: u8 = 1;
pub const BOX_LR_DLIST: u8 = 2;

/// Math-node kinds of `Node::MathKern`: e-TeX's math-node subtype plus one
/// (kind 0 is an unconverted `\mkern`). etex.ch: before=0, after=1,
/// begin_M=2, end_M=3, begin_L=6, end_L=7, begin_R=10, end_R=11.
pub const MATH_ON: u8 = 1;
pub const MATH_OFF: u8 = 2;
pub const BEGIN_M: u8 = 3;
pub const END_M: u8 = 4;
pub const BEGIN_L: u8 = 7;
pub const END_L: u8 = 8;
pub const BEGIN_R: u8 = 11;
pub const END_R: u8 = 12;
/// Math-node kinds at or above this are TeXXeT text-direction nodes
/// (e-TeX `subtype>=L_code`).
pub const LR_KIND_MIN: u8 = 5;

/// etex.ch `end_LR`: an end node (`\mathoff`, `\endM`, `\endL`, `\endR`).
#[inline]
pub fn math_end_lr(kind: u8) -> bool {
    kind & 1 == 0
}

/// etex.ch `end_LR_type`: the end-node kind matching node `kind`.
#[inline]
pub fn math_end_lr_type(kind: u8) -> u8 {
    4 * ((kind - 1) / 4) + 4
}

/// etex.ch `LR_dir`: 0 left-to-right, 1 right-to-left.
#[inline]
pub fn math_lr_dir(kind: u8) -> u8 {
    (kind - 1) / 8
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MathStyle {
    Display,
    Text,
    Script,
    ScriptScript,
    CrampedDisplay,
    CrampedText,
    CrampedScript,
    CrampedScriptScript,
}

/// The accent characters of a luatex `accent_noad` (texnodes.h
/// `top_accent_chr`, `bot_accent_chr`, `overlay_accent_chr`, `accentfraction`
/// and the subtype). A missing accent is `None`; `subtype` is 0 (both
/// stretchable), 1 (top fixed), 2 (bottom fixed) or 3 (both fixed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccentSpec {
    pub top: Option<(u8, u32)>,
    pub bottom: Option<(u8, u32)>,
    pub overlay: Option<(u8, u32)>,
    pub subtype: u8,
    pub fraction: i32,
}

/// The options of a luatex fence noad (`delimiterheight`, `delimiterdepth`,
/// `delimiterclass`, `delimiteroptions`): `\Uleft height 10pt axis class 4 ...`.
/// `class` is -1 unless a `class` keyword was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FenceOpts {
    pub height: i32,
    pub depth: i32,
    pub class: i32,
    pub options: u16,
}

impl FenceOpts {
    pub const NONE: FenceOpts = FenceOpts { height: 0, depth: 0, class: -1, options: 0 };
}

impl Default for FenceOpts {
    fn default() -> Self {
        FenceOpts::NONE
    }
}

/// A luatex delimiter field (texnodes.h `small_fam`/`small_char`/
/// `large_fam`/`large_char`): the "small" and "large" starting characters of
/// a variable-size delimiter. The characters are Unicode code points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Delim {
    pub small_fam: u8,
    pub small_char: u32,
    pub large_fam: u8,
    pub large_char: u32,
}

impl Delim {
    /// The delimiter of a tex.web 27-bit delimiter code (the class bits
    /// above bit 23 are dropped).
    pub fn from_code(code: i32) -> Delim {
        let c = code as u32;
        Delim {
            small_fam: ((c >> 20) & 0xF) as u8,
            small_char: (c >> 12) & 0xFF,
            large_fam: ((c >> 8) & 0xF) as u8,
            large_char: c & 0xFF,
        }
    }

    /// `small_fam = small_char = large_fam = large_char = 0`
    pub fn is_null(&self) -> bool {
        self.small_fam == 0 && self.small_char == 0 && self.large_fam == 0 && self.large_char == 0
    }
}

/// luatex `noad_option_*` bits (texnodes.h): every option carries the
/// `SET` bit, and some options contain the bits of others (`LEFT` contains
/// `EXACT`), so tests compare masked values as luatex does.
pub mod noad_option {
    pub const SET: u16 = 0x08;
    pub const AXIS: u16 = 0x02 + 0x08;
    pub const NO_AXIS: u16 = 0x04 + 0x08;
    pub const EXACT: u16 = 0x10 + 0x08;
    pub const LEFT: u16 = 0x11 + 0x08;
    pub const MIDDLE: u16 = 0x12 + 0x08;
    pub const RIGHT: u16 = 0x14 + 0x08;
    pub const NO_SUB_SCRIPT: u16 = 0x21 + 0x08;
    pub const NO_SUPER_SCRIPT: u16 = 0x22 + 0x08;
    pub const NO_SCRIPT: u16 = 0x23 + 0x08;
    pub const NO_RULE: u16 = 0x24 + 0x08;

    /// `(options & bit) == bit`
    pub fn has(options: u16, bit: u16) -> bool {
        options & bit == bit
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Glue {
    pub width: i32,
    pub stretch: i32,
    pub shrink: i32,
    pub stretch_order: u8,
    pub shrink_order: u8,
    /// tex.web glue-node subtype (§149): 0 for normal glue, `n+1` for glue
    /// taken from glue parameter `n` (see [`glue_subtype`]); box displays
    /// print it as `\glue(\baselineskip)`. Meaningless for eqtb values.
    pub subtype: u8,
    /// Identity of the glue specification TeX shares between copies
    /// ([`Glue::NO_SPEC`], [`Glue::ZERO_SPEC`] or an id the engine minted for
    /// a scanned or registered spec). Copies of a value keep it, which is
    /// what e-TeX's `\tracingassigns` pointer comparison sees: assigning a
    /// spec to the register that already holds it is a reassignment.
    pub spec: u32,
}

/// tex.web glue-node subtypes: `skip_param_code + 1`.
pub mod glue_subtype {
    pub const NORMAL: u8 = 0;
    pub const LINE_SKIP: u8 = 1;
    pub const BASELINE_SKIP: u8 = 2;
    pub const PAR_SKIP: u8 = 3;
    pub const ABOVE_DISPLAY_SKIP: u8 = 4;
    pub const BELOW_DISPLAY_SKIP: u8 = 5;
    pub const ABOVE_DISPLAY_SHORT_SKIP: u8 = 6;
    pub const BELOW_DISPLAY_SHORT_SKIP: u8 = 7;
    pub const LEFT_SKIP: u8 = 8;
    pub const RIGHT_SKIP: u8 = 9;
    pub const TOP_SKIP: u8 = 10;
    pub const SPLIT_TOP_SKIP: u8 = 11;
    pub const TAB_SKIP: u8 = 12;
    pub const SPACE_SKIP: u8 = 13;
    pub const XSPACE_SKIP: u8 = 14;
    pub const PAR_FILL_SKIP: u8 = 15;
    pub const THIN_MU_SKIP: u8 = 16;
    pub const MED_MU_SKIP: u8 = 17;
    pub const THICK_MU_SKIP: u8 = 18;
    /// luatex `cond_math_glue` (`\nonscript`)
    pub const NONSCRIPT: u8 = 19;
    /// tex.web print_skip_param names, indexed by `subtype - 1`.
    pub const NAMES: [&str; 19] = [
        "lineskip",
        "baselineskip",
        "parskip",
        "abovedisplayskip",
        "belowdisplayskip",
        "abovedisplayshortskip",
        "belowdisplayshortskip",
        "leftskip",
        "rightskip",
        "topskip",
        "splittopskip",
        "tabskip",
        "spaceskip",
        "xspaceskip",
        "parfillskip",
        "thinmuskip",
        "medmuskip",
        "thickmuskip",
        "nonscript",
    ];
}

impl Glue {
    pub fn zero() -> Glue {
        Glue::new(0)
    }
    /// A spec nobody else shares: never the same spec as any other value.
    pub const NO_SPEC: u32 = 0;
    /// TeX's shared `zero_glue`: every all-zero glue parameter or register
    /// is (`trap_zero_glue`), so glue copied from one is too. `short_display`
    /// prints no space for such glue.
    pub const ZERO_SPEC: u32 = 1;
    /// The first id the engine mints for a shared spec.
    pub const FIRST_SPEC: u32 = 2;
    pub const ZERO_GLUE: Glue = Glue {
        width: 0,
        stretch: 0,
        shrink: 0,
        stretch_order: 0,
        shrink_order: 0,
        subtype: glue_subtype::NORMAL,
        spec: Glue::ZERO_SPEC,
    };
    pub fn new(w: i32) -> Glue {
        Glue {
            width: w,
            stretch: 0,
            shrink: 0,
            stretch_order: 0,
            shrink_order: 0,
            subtype: glue_subtype::NORMAL,
            spec: Glue::NO_SPEC,
        }
    }
    pub fn fil(order: u8, w: i32) -> Glue {
        Glue {
            stretch: ONE,
            stretch_order: order,
            ..Glue::new(w)
        }
    }
    /// A newly created spec `width plus stretch minus shrink`.
    pub fn spec(width: i32, stretch: i32, stretch_order: u8, shrink: i32, shrink_order: u8) -> Glue {
        Glue {
            width,
            stretch,
            shrink,
            stretch_order,
            shrink_order,
            ..Glue::new(0)
        }
    }
    pub fn is_zero(&self) -> bool {
        self.width == 0 && self.stretch == 0 && self.shrink == 0
    }
    /// The spec is TeX's shared `zero_glue`.
    #[inline]
    pub fn is_zero_glue(&self) -> bool {
        self.spec == Glue::ZERO_SPEC
    }
    /// tex.web new_param_glue: this eqtb glue value as the node of glue
    /// parameter `subtype`; the node shares the parameter's spec.
    pub fn param(self, subtype: u8) -> Glue {
        Glue { subtype, ..self }
    }
    /// A value read from a glue register or parameter: TeX shares the eqtb
    /// spec (eqtb values are interned at assignment, so an all-zero one is
    /// `zero_glue`).
    pub fn eqtb_value(self) -> Glue {
        Glue {
            subtype: glue_subtype::NORMAL,
            ..self
        }
    }
    /// The same values as a newly created spec (`new_spec`).
    pub fn fresh(self) -> Glue {
        Glue {
            subtype: glue_subtype::NORMAL,
            spec: Glue::NO_SPEC,
            ..self
        }
    }
}

/// pdfTeX `pdf_colorstack_cmd`: set and push carry data, pop and current
/// re-emit the stack's current value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorStackCmd {
    Set,
    Push,
    Pop,
    Current,
}

/// LuaTeX boundary node subtypes (texnodes.h `boundary_subtypes`).
pub const BOUNDARY_USER: u8 = 1;
pub const BOUNDARY_PROTRUSION: u8 = 2;
pub const BOUNDARY_WORD: u8 = 3;

#[derive(Clone, Debug)]
pub enum WhatIt {
    /// LuaTeX boundary node (`\boundary`, `\wordboundary`,
    /// `\protrusionboundary`): no output, separates characters.
    Boundary {
        kind: u8,
        value: i32,
    },
    /// LuaTeX `\latelua{...}` (`func` 0) / `\lateluafunction n`: Lua that
    /// runs when the page is shipped out.
    LateLua {
        code: Vec<u8>,
        func: i32,
    },
    /// LuaTeX dir node (`\textdir`): `cancel` ends the direction `dir`.
    Dir {
        dir: u8,
        cancel: bool,
        level: u16,
    },
    PdfLiteral {
        origin: u8,
        data: String,
    },
    /// `\pdfcolorstack <stack> <cmd> [{data}]`, executed at shipout
    PdfColorStack {
        stack: i32,
        cmd: ColorStackCmd,
        data: String,
    },
    PdfRefXImage {
        obj: i32,
        w: i32,
        h: i32,
        d: i32,
    },
    PdfRefXForm {
        obj: i32,
        w: i32,
        h: i32,
        d: i32,
    },
    /// PDF graphics-state operations execute later at shipout. Keep a cheap
    /// source bookmark so any delayed diagnostic still names the command
    /// that created the node.
    PdfSave {
        source: Option<crate::input::SourceMark>,
    },
    PdfRestore {
        source: Option<crate::input::SourceMark>,
    },
    PdfSetMatrix {
        matrix: String,
        source: Option<crate::input::SourceMark>,
    },
    Write {
        stream: u16,
        tokens: Vec<crate::token::Token>,
        source: Option<Box<crate::input::SourceContext>>,
    },
    /// tex.web §1393 `open_node`: a non-immediate `\openout` is queued as a
    /// whatsit and takes effect in list order at shipout (`out_what` @1414)
    OpenOut {
        stream: u16,
        /// (path, shown): the resolved output path, and tex.web
        /// `print_file_name(open_name,open_area,open_ext)`: the name as
        /// scanned (no `.tex` appended), quoted when it has spaces. Boxed to
        /// keep the hot `Node` enum compact.
        names: Box<(String, String)>,
        /// Managed auxiliary directories mirror nested `\\include` paths.
        /// Traditional and absolute `\\openout` paths do not create parents.
        create_parent: bool,
        source: Option<Box<crate::input::SourceContext>>,
    },
    /// tex.web §1393 `close_node`: a non-immediate `\closeout` is queued as
    /// a whatsit on the current list and only takes effect when the list is
    /// shipped (`out_what` @1414) — writes issued before that shipout still
    /// land in the file
    CloseOut {
        stream: u16,
        source: Option<Box<crate::input::SourceContext>>,
    },
    /// Source position captured while building the list, before page shipout.
    SyncPoint {
        file_id: u32,
        line: u32,
    },
    PdfDest {
        id: crate::pdfout::DestId,
        kind: u8,
        params: [i32; 4],
    },
    PdfAnnot {
        attr: String,
        wd: i32,
        ht: i32,
        dp: i32,
    },
    /// `\pdfstartlink`; `wd`/`ht`/`dp` are `RULE_FILL` when running.
    PdfStartLink {
        attr: String,
        uri: Option<String>,
        name: Option<String>,
        wd: i32,
        ht: i32,
        dp: i32,
    },
    PdfEndLink,
    /// tex.web §1341 `language_node` (`\setlanguage`, `fix_language`):
    /// line_break hyphenates the following words with this language and
    /// these `\lefthyphenmin`/`\righthyphenmin` values.
    Language {
        lang: u8,
        lhm: u8,
        rhm: u8,
    },
    Special(String),
    SavePos {
        obj: i32,
    },
    User(i32),
    CjkText(Option<char>),
    /// `\pdfinterwordspaceon` (true) / `\pdfinterwordspaceoff` (false).
    PdfInterwordSpace(bool),
    /// `\pdffakespace`: a space glyph from `\pdfspacefont` at shipout.
    PdfFakeSpace,
    /// `\pdfrunninglinkon` (true) / `\pdfrunninglinkoff` (false).
    PdfRunningLink(bool),
    /// `\pdfsnaprefpoint`: the reference point for `\pdfsnapy`.
    PdfSnapRefPoint,
    /// `\pdfsnapy <glue>`: snap the vertical position to a grid.
    PdfSnapY(crate::boxes::Glue),
    /// `\pdfsnapycomp <ratio>`: compensate the next `\pdfsnapy` (0..1000).
    PdfSnapYComp(i32),
}

/// leader kinds (tex.web subtypes a_leaders/c_leaders/x_leaders)
pub const LEADERS_A: u8 = 0;
pub const LEADERS_C: u8 = 1;
pub const LEADERS_X: u8 = 2;

/// the repeated object of a leader node: a rule or a packed box
#[derive(Clone, Debug)]
pub enum LeaderBody {
    Rule { width: i32, height: i32, depth: i32 },
    Box(Box<Node>),
}

/// (width, height, depth) of a leader body
pub fn leader_dims(body: &LeaderBody) -> (i32, i32, i32) {
    match body {
        LeaderBody::Rule {
            width,
            height,
            depth,
        } => (*width, *height, *depth),
        LeaderBody::Box(b) => match &**b {
            Node::Box { w, h, d, .. } => (*w, *h, *d),
            Node::Rule {
                width,
                height,
                depth, .. } => (*width, *height, *depth),
            _ => (0, 0, 0),
        },
    }
}

#[derive(Clone, Debug)]
pub struct DiscNode {
    pub pre_break: NodeList,
    pub post_break: NodeList,
    pub no_break: NodeList,
    pub replace_count: usize,
    /// LuaTeX disc subtype (`discretionary` 0, `explicit` 1, `automatic` 2,
    /// `regular` 3, `first` 4, `second` 5)
    pub subtype: u8,
    /// LuaTeX `penalty` field; [`DISC_PENALTY_TEX`] applies tex.web's rule
    /// (`\hyphenpenalty` with a pre-break text, `\exhyphenpenalty` without)
    pub penalty: i32,
    pub attr: Attr,
}

/// [`DiscNode::penalty`] of a discretionary that follows tex.web's rule.
pub const DISC_PENALTY_TEX: i32 = i32::MIN;

impl DiscNode {
    pub fn with_attr(mut self, attr: Attr) -> Self {
        self.attr = attr;
        self
    }

    pub fn new(pre_break: NodeList, post_break: NodeList, no_break: NodeList, replace_count: usize) -> Self {
        DiscNode { pre_break, post_break, no_break, replace_count, subtype: 0, penalty: DISC_PENALTY_TEX, attr: Attr::NONE }
    }
}

/// Stable identity for a math atom that may need to report a missing glyph
/// after TeX has selected the conversion style and font. Source marks live in
/// an engine-side arena keyed by this id, keeping the hot `Node` enum compact.
/// The id also prevents a measuring pass from reporting an atom twice.
#[derive(Clone, Debug, Default)]
pub struct MathDiagnosticOrigin {
    pub(crate) id: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SpanId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructureTag {
    Heading(u8),
    Paragraph,
    Formula,
    Table,
    Figure,
}

impl StructureTag {
    pub fn tag_name(&self) -> &'static str {
        match self {
            StructureTag::Paragraph => "/P",
            StructureTag::Heading(1) => "/H1",
            StructureTag::Heading(2) => "/H2",
            StructureTag::Heading(3) => "/H3",
            StructureTag::Heading(4) => "/H4",
            StructureTag::Heading(5) => "/H5",
            StructureTag::Heading(_) => "/H6",
            StructureTag::Formula => "/Formula",
            StructureTag::Table => "/Table",
            StructureTag::Figure => "/Figure",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DisplayItem {
    GlyphRun {
        font: crate::tfm::FontId,
        x_bp: f64,
        y_bp: f64,
        glyphs: Vec<u8>,
        tag: Option<StructureTag>,
        span: Option<SpanId>,
    },
    NativeGlyphRun {
        run: std::rc::Rc<crate::native_layout::NativeRun>,
        start: usize,
        end: usize,
        x_bp: f64,
        y_bp: f64,
        tag: Option<StructureTag>,
        span: Option<SpanId>,
    },
    Rule {
        x_bp: f64,
        y_bp: f64,
        width_bp: f64,
        height_bp: f64,
    },
    Image {
        obj_id: u64,
        x_bp: f64,
        y_bp: f64,
        width_bp: f64,
        height_bp: f64,
    },
    Link {
        rect_bp: [f64; 4],
        dest: String,
    },
}
impl DisplayItem {
    pub fn glyph_run(font: crate::tfm::FontId, x_bp: f64, y_bp: f64, glyphs: Vec<u8>) -> Self {
        Self::GlyphRun {
            font,
            x_bp,
            y_bp,
            glyphs,
            tag: None,
            span: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

impl DisplayList {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn push(&mut self, item: DisplayItem) {
        self.items.push(item);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn has_structure_tags(&self) -> bool {
        self.items.iter().any(|item| {
            matches!(
                item,
                DisplayItem::GlyphRun { tag: Some(_), .. }
                    | DisplayItem::NativeGlyphRun { tag: Some(_), .. }
            )
        })
    }
    pub fn tag_range(&mut self, start: usize, end: usize, tag: StructureTag) {
        let bound = end.min(self.items.len());
        for item in self.items[start..bound].iter_mut() {
            match item {
                DisplayItem::GlyphRun { tag: item_tag, .. }
                | DisplayItem::NativeGlyphRun { tag: item_tag, .. } => {
                    *item_tag = Some(tag);
                }
                _ => {}
            }
        }
    }
}

/// A Unicode glyph of a Lua-defined font (LuaTeX `glyph_node`). The engine's
/// [`Node::Char`] stays an 8-bit TFM character; glyphs of fonts whose
/// `Font::lua` is set (and everything Lua code builds with `node.new`) use
/// this node. Metrics come from the font at use time, as in LuaTeX.
#[derive(Clone, Debug)]
pub struct LuaGlyph {
    pub c: u32,
    pub font: FontId,
    /// hyphenation language and minimal left/right fragments
    pub lang: u16,
    pub left: u8,
    pub right: u8,
    pub uchyph: u8,
    pub xoffset: i32,
    pub yoffset: i32,
    pub expansion_factor: i32,
    pub data: i32,
    /// LuaTeX glyph subtype (`GLYPH_CHARACTER`, `GLYPH_LIGATURE`, ...)
    pub subtype: u8,
    /// the components of a ligature
    pub components: NodeList,
    pub attr: Attr,
}

/// Handle of an interned LuaTeX attribute list (`Eqtb::attr_lists`); 0 is
/// no list. Every node a LuaTeX document can hang `node.set_attribute` on
/// carries one (luatex `node_attr`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Attr(pub u32);

impl Attr {
    pub const NONE: Attr = Attr(0);
}

#[derive(Clone, Debug)]
pub enum Node {
    Char {
        c: u8,
        font: FontId,
        attr: Attr,
    },
    /// a glyph of a Lua font (see [`LuaGlyph`])
    LuaGlyph(Box<LuaGlyph>),
    NativeGlyphRun {
        run: std::rc::Rc<crate::native_layout::NativeRun>,
        start: usize,
        end: usize,
        width: i32,
        height: i32,
        depth: i32,
    },
    /// `letters` = the component letters that formed the glyph (hyphenation
    /// needs them: a break point may fall inside the ligature); `subtype`
    /// is tex.web's ligature subtype (§143): +2 when the left boundary
    /// took part, +1 when the right boundary did.
    Ligature {
        c: u8,
        font: FontId,
        lig_width: i32,
        lig_height: i32,
        lig_depth: i32,
        letters: [u8; 3],
        n_letters: u8,
        subtype: u8,
        attr: Attr,
    },
    Glue(Glue, Attr),
    Kern(i32, Attr),
    ExplicitKern(i32, Attr),
    /// tex.web `acc_kern` (subtype 2): the two kerns `\accent` puts around
    /// the accent. Unlike a normal kern it ends a hyphenation word and is
    /// never stretched by font expansion; unlike an explicit kern it is not
    /// a legal breakpoint and is not discarded at a line break.
    AccentKern(i32, Attr),
    /// LuaTeX `italic_kern` (kern subtype 3): italic correction kerns
    /// that math conversion inserts
    ItalicKern(i32, Attr),
    /// pdfTeX `margin_kern_node`: a kern of width `-w` placed at the very
    /// start (or just before the trailing `\rightskip`) of a line box to let
    /// the marginal character `c` protrude `w` into the margin when
    /// `\pdfprotrudechars > 0`. `side` is 0 for left, 1 for right.
    MarginKern {
        side: u8,
        width: i32,
        c: u8,
        font: FontId,
        attr: Attr,
    },
    Penalty(i32, Attr),
    Rule {
        width: i32,
        height: i32,
        depth: i32,
        attr: Attr,
    },
    Leaders {
        glue: Glue,
        kind: u8,
        body: LeaderBody,
        attr: Attr,
    },
    Disc(DiscNode),
    Box {
        kind: u8,
        w: i32,
        h: i32,
        d: i32,
        shift: i32,
        list: NodeList,
        glue_sign: u8,
        glue_order: u8,
        glue_set: f64,
        /// e-TeX `box_lr` (the hlist subtype): 0, [`BOX_LR_REVERSED`] once
        /// ship_out reversed the list, [`BOX_LR_DLIST`] for display math.
        lr: u8,
        /// LuaTeX `box_dir` (`\boxdir`): 0 TLT, 1 TRT, 2 LTL, 3 RTT.
        dir: u8,
        attr: Attr,
    },
    Mark {
        class: i32,
        tokens: Vec<crate::token::Token>,
        attr: Attr,
    },
    Ins {
        num: u16,
        height: i32,
        depth: i32,
        cost: i32,
        /// the LOCAL `\splittopskip` captured before unsave (tex.web
        /// @21193 `q:=split_top_skip; add_glue_ref(q)` / @21201
        /// `split_top_ptr(tail):=q`); Glue is Copy, matching the by-value
        /// ownership every other glue site in the tree uses
        split_top_skip: Glue,
        /// the LOCAL `\splitmaxdepth` captured before unsave (tex.web
        /// @21194 `d:=split_max_depth` / @21201 `depth(tail):=d`)
        split_max_depth: i32,
        box_node: Box<Node>,
        attr: Attr,
    },
    Adj(i32, Attr),
    Whatsit(WhatIt, Attr),
    // math nodes (converted to boxes before shipping):
    Style(MathStyle, Attr),
    NonScript,
    MuGlue(Glue, Attr),
    Choice,
    ChoiceAlt {
        body: NodeList,
        attr: Attr,
    },
    MathChar {
        fam: u8,
        c: u32,
        class: u8,
        origin: MathDiagnosticOrigin,
        attr: Attr,
    },
    Frac {
        num: NodeList,
        den: NodeList,
        thickness: i32,
        left: Option<Delim>,
        right: Option<Delim>,
        /// `\Uskewed` / `\Uskewedwithdelims`: the delimiter between the
        /// numerator and the denominator (texmath.c `middle_delimiter`)
        middle: Option<Delim>,
        /// luatex `fractionoptions` (`noad_option_*`, see [`noad_option`])
        options: u16,
        origin: MathDiagnosticOrigin,
        attr: Attr,
    },
    /// luatex `radical_noad`: `\radical` (`subtype` 0), `\Uradical` (1),
    /// `\Uroot` (2), `\Uunderdelimiter` (3), `\Uoverdelimiter` (4),
    /// `\Udelimiterunder` (5), `\Udelimiterover` (6), `\Uhextensible` (7).
    Radical {
        body: NodeList,
        delim: Delim,
        subtype: u8,
        /// `radicalwidth` (`width=` keyword)
        width: i32,
        /// luatex `radicaloptions` (`noad_option_*`)
        options: u16,
        /// the root degree of `\Uroot`
        degree: Option<NodeList>,
        origin: MathDiagnosticOrigin,
        attr: Attr,
    },
    Scripts {
        nucleus: NodeList,
        sup: Option<NodeList>,
        sub: Option<NodeList>,
        attr: Attr,
    },
    /// A delimiter marker of a flat mlist: `size` 0 is a `\left` (open
    /// boundary), 1 a `\right`, 2 a plain delimiter atom, 3 a `\middle`
    /// and 4 luatex's `no_noad_side` fence (`\Uvextensible`).
    DelimBox {
        small: (u8, u32),
        large: (u8, u32),
        size: u8,
        /// `\Uleft`/`\Umiddle`/`\Uright`/`\Uvextensible` options
        fence: FenceOpts,
        origin: MathDiagnosticOrigin,
        attr: Attr,
    },
    OpLimits {
        op: NodeList,
        above: Option<NodeList>,
        below: Option<NodeList>,
        attr: Attr,
    },
    /// `\mkern` (kind 0, mlists only) or a math node (kind = e-TeX math
    /// subtype + 1, see [`MATH_ON`]..[`END_R`]); the i32 is the width.
    MathKern(i32, u8, Attr),
    /// luatex `accent_noad`: `\mathaccent` and `\Umathaccent`.
    Accent {
        spec: AccentSpec,
        body: NodeList,
        origin: MathDiagnosticOrigin,
        attr: Attr,
    },
    Overline {
        body: NodeList,
        under: bool,
        /// the already-packed bar-and-body box the conversion yields; `body`
        /// is the original field, kept for `\showlists` (tex.web §692)
        packed: Box<Node>,
        attr: Attr,
    },
    VCenter {
        box_node: Box<Node>,
    },
    InsDisc,
    Empty,
    VAdjust(NodeList, Attr),
    /// pdfTeX `\vadjust pre{...}`: material that migrates to the vertical
    /// list in front of the line (row, display) containing it
    PreAdjust(NodeList, Attr),
}

impl Node {
    /// The node's attribute list ([`Attr::NONE`] for the variants LuaTeX
    /// gives none).
    pub fn attr(&self) -> Attr {
        match self {
            Node::Char { attr, .. }
            | Node::Ligature { attr, .. }
            | Node::MarginKern { attr, .. }
            | Node::Rule { attr, .. }
            | Node::Leaders { attr, .. }
            | Node::Box { attr, .. }
            | Node::Mark { attr, .. }
            | Node::Ins { attr, .. }
            | Node::ChoiceAlt { attr, .. }
            | Node::MathChar { attr, .. }
            | Node::Frac { attr, .. }
            | Node::Radical { attr, .. }
            | Node::Scripts { attr, .. }
            | Node::DelimBox { attr, .. }
            | Node::OpLimits { attr, .. }
            | Node::Accent { attr, .. }
            | Node::Overline { attr, .. } => *attr,
            Node::Glue(_, a)
            | Node::Kern(_, a)
            | Node::ExplicitKern(_, a)
            | Node::AccentKern(_, a)
            | Node::ItalicKern(_, a)
            | Node::Penalty(_, a)
            | Node::Adj(_, a)
            | Node::Whatsit(_, a)
            | Node::Style(_, a)
            | Node::MuGlue(_, a)
            | Node::VAdjust(_, a)
            | Node::PreAdjust(_, a)
            | Node::MathKern(_, _, a) => *a,
            Node::LuaGlyph(g) => g.attr,
            Node::Disc(d) => d.attr,
            _ => Attr::NONE,
        }
    }

    /// Give the node attribute list `a` (a no-op for the variants without).
    pub fn set_attr(&mut self, a: Attr) {
        match self {
            Node::Char { attr, .. }
            | Node::Ligature { attr, .. }
            | Node::MarginKern { attr, .. }
            | Node::Rule { attr, .. }
            | Node::Leaders { attr, .. }
            | Node::Box { attr, .. }
            | Node::Mark { attr, .. }
            | Node::Ins { attr, .. }
            | Node::ChoiceAlt { attr, .. }
            | Node::MathChar { attr, .. }
            | Node::Frac { attr, .. }
            | Node::Radical { attr, .. }
            | Node::Scripts { attr, .. }
            | Node::DelimBox { attr, .. }
            | Node::OpLimits { attr, .. }
            | Node::Accent { attr, .. }
            | Node::Overline { attr, .. } => *attr = a,
            Node::Glue(_, x)
            | Node::Kern(_, x)
            | Node::ExplicitKern(_, x)
            | Node::AccentKern(_, x)
            | Node::ItalicKern(_, x)
            | Node::Penalty(_, x)
            | Node::Adj(_, x)
            | Node::Whatsit(_, x)
            | Node::Style(_, x)
            | Node::MuGlue(_, x)
            | Node::VAdjust(_, x)
            | Node::PreAdjust(_, x)
            | Node::MathKern(_, _, x) => *x = a,
            Node::LuaGlyph(g) => g.attr = a,
            Node::Disc(d) => d.attr = a,
            _ => {}
        }
    }
}

pub type NodeList = Vec<Node>;

/// (width, height, depth) of a glyph of a Lua font as hpack counts them
/// (texnodes.c `glyph_width`, `glyph_height`, `glyph_depth` with
/// `\glyphdimensionsmode` 0): the character record's metrics, the height
/// raised and the depth lowered by the vertical offset `y`. Zero when the
/// font lacks the character.
pub fn lua_glyph_whd(fonts: &[std::rc::Rc<crate::tfm::Font>], font: i32, c: i32, y: i32) -> (i32, i32, i32) {
    let Some(f) = usize::try_from(font).ok().and_then(|f| fonts.get(f)) else {
        return (0, 0, 0);
    };
    let (w, h, d) = if f.lua.is_some() {
        match u32::try_from(c).ok().and_then(|c| f.lua_char(c)) {
            Some(ci) => (ci.width, ci.height, ci.depth),
            None => return (0, 0, 0),
        }
    } else if (0..256).contains(&c) {
        let c = c as u8;
        (f.char_width(c), f.char_height(c), f.char_depth(c))
    } else {
        return (0, 0, 0);
    };
    (w, (h + y).max(0), if y > 0 { d - y } else { d }.max(0))
}

/// [`lua_glyph_whd`] of a glyph node.
pub fn lua_glyph_dims(eqtb: &crate::eqtb::Eqtb, g: &LuaGlyph) -> (i32, i32, i32) {
    lua_glyph_whd(&eqtb.fonts, i32::from(g.font), g.c as i32, g.yoffset)
}

/// dimensions of a single node in a horizontal list
fn single_dims(n: &Node, eqtb: &crate::eqtb::Eqtb) -> (i32, i32, i32) {
    match n {
        Node::Char { c, font, .. } => (
            eqtb_fonts(eqtb).char_width(*font, *c),
            eqtb_fonts(eqtb).char_height(*font, *c),
            eqtb_fonts(eqtb).char_depth(*font, *c),
        ),
        Node::LuaGlyph(g) => lua_glyph_dims(eqtb, g),
        Node::Ligature {
            lig_width,
            lig_height,
            lig_depth,
            ..
        } => (*lig_width, *lig_height, *lig_depth),
        Node::Glue(g, _) => (g.width, 0, 0),
        Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) => (*k, 0, 0),
        // tex.web math_node: width = \mathsurround (math-on 1 / math-off 2);
        // an unconverted \mkern (kind 0) has no width yet
        Node::MathKern(k, MATH_ON.., _) => (*k, 0, 0),
        Node::MarginKern { width, .. } => (*width, 0, 0),
        Node::Penalty(_, _) => (0, 0, 0),
        Node::Rule {
            width,
            height,
            depth, .. } => (*width, *height, *depth),
        Node::Box { w, h, d, shift, .. } => (*w, (*h - *shift).max(0), (*d + *shift).max(0)),
        Node::Mark { .. }
        | Node::Style(_, _)
        | Node::Adj(_, _)
        | Node::Choice
        | Node::ChoiceAlt { .. } => (0, 0, 0),
        Node::Scripts { nucleus, .. } => hlist_dims(nucleus, eqtb),
        Node::Frac { num, den, .. } => {
            let (wn, hn, _) = hlist_dims(num, eqtb);
            let (wd, hd, _) = hlist_dims(den, eqtb);
            (wn.max(wd), hn + hd, 0)
        }
        Node::Radical { body, .. } => hlist_dims(body, eqtb),
        Node::Overline { packed, .. } => single_dims(packed, eqtb),
        Node::OpLimits { op, .. } => hlist_dims(op, eqtb),
        Node::VCenter { box_node } => single_dims(box_node, eqtb),
        Node::Whatsit(WhatIt::PdfRefXImage { w, h, d, .. }, _)
        | Node::Whatsit(WhatIt::PdfRefXForm { w, h, d, .. }, _) => (*w, *h, *d),
        Node::DelimBox { .. }
        | Node::Accent { .. }
        | Node::Whatsit(_, _)
        | Node::Ins { .. }
        | Node::Disc(_)
        | Node::MathChar { .. } => (0, 0, 0),
        Node::VAdjust(_, _)
        | Node::PreAdjust(_, _)
        | Node::InsDisc
        | Node::Empty
        | Node::MathKern(_, _, _)
        | Node::NonScript
        | Node::MuGlue(_, _) => (0, 0, 0),
        Node::Leaders { glue, body, .. } => {
            let (_, bh, bd) = leader_dims(body);
            (glue.width, bh, bd)
        }
        Node::NativeGlyphRun {
            width,
            height,
            depth,
            ..
        } => (*width, *height, *depth),
    }
}

/// natural dimensions of a horizontal list. Returns (width, height, max_depth)
pub fn hlist_dims(list: &[Node], eqtb: &crate::eqtb::Eqtb) -> (i32, i32, i32) {
    let mut w = 0i64;
    let mut h = 0i64;
    let mut d = 0i64;
    let mut i = 0usize;
    while i < list.len() {
        let n = &list[i];
        if let Node::Disc(dc) = n {
            for n2 in &dc.no_break {
                let (w2, h2, d2) = single_dims(n2, eqtb);
                w += w2 as i64;
                h = h.max(h2 as i64);
                d = d.max(d2 as i64);
            }
            i += 1 + dc.replace_count;
            continue;
        }
        if matches!(n, Node::Choice) {
            i += 1;
            continue;
        }
        let (w2, h2, d2) = single_dims(n, eqtb);
        w += w2 as i64;
        h = h.max(h2 as i64);
        d = d.max(d2 as i64);
        i += 1;
    }
    (w as i32, h as i32, d as i32)
}

/// natural dimensions of a vertical list: (width, height, depth), per
/// tex.web vpackage §13178: glue/kern reset the running depth, box width
/// competes as width+shift, and penalty/mark/ins/whatsit items do nothing.
pub fn vlist_dims(list: &[Node], eqtb: &crate::eqtb::Eqtb) -> (i32, i32, i32) {
    let _ = eqtb;
    let (mut x, mut d, mut w) = (0i64, 0i64, 0i64);
    for n in list {
        match n {
            Node::Box {
                w: bw,
                h: bh,
                d: bd,
                shift,
                ..
            } => {
                x += d + *bh as i64;
                d = *bd as i64;
                w = w.max(*bw as i64 + *shift as i64);
            }
            Node::Rule {
                height,
                depth,
                width, .. } => {
                x += d + *height as i64;
                d = *depth as i64;
                w = w.max(*width as i64);
            }
            Node::Glue(g, _) => {
                x += d + g.width as i64;
                d = 0;
            }
            Node::MarginKern { width, .. } => {
                x += d + *width as i64;
                d = 0;
            }
            Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) => {
                x += d + *k as i64;
                d = 0;
            }
            Node::Leaders { body, .. } => {
                x += d;
                d = 0;
                let (lw, _, _) = leader_dims(body);
                w = w.max(lw as i64);
            }
            Node::Whatsit(WhatIt::PdfRefXImage {
                w: iw,
                h: ih,
                d: id,
                ..
            }, _)
            | Node::Whatsit(WhatIt::PdfRefXForm {
                w: iw,
                h: ih,
                d: id,
                ..
            }, _) => {
                x += d + *ih as i64;
                d = *id as i64;
                w = w.max(*iw as i64);
            }
            Node::NativeGlyphRun {
                width,
                height,
                depth,
                ..
            } => {
                x += d + *height as i64;
                d = *depth as i64;
                w = w.max(*width as i64);
            }
            // penalty, mark, ins, whatsit, math-only nodes: do_nothing
            _ => {}
        }
    }
    (w as i32, x as i32, d as i32)
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// choose glue sign/order/ratio per tex.web hpack §649: check the highest
/// nonzero stretch (or shrink) order first — filll, fill, fil, normal.
/// If that total is 0 the sign reverts to normal with ratio 0.
pub fn compute_glue_set(
    target: i64,
    natural: i64,
    stretch: [i64; 4],
    shrink: [i64; 4],
) -> (u8, u8, f64) {
    let x = target - natural;
    if x == 0 {
        return (0, 0, 0.0);
    }
    let pick = |v: &[i64; 4]| -> usize {
        if v[3] != 0 {
            3
        } else if v[2] != 0 {
            2
        } else if v[1] != 0 {
            1
        } else {
            0
        }
    };
    if x > 0 {
        let o = pick(&stretch);
        if stretch[o] != 0 {
            (1, o as u8, x as f64 / stretch[o] as f64)
        } else {
            (0, 0, 0.0)
        }
    } else {
        let o = pick(&shrink);
        if shrink[o] != 0 {
            (2, o as u8, (-x) as f64 / shrink[o] as f64)
        } else {
            (0, 0, 0.0)
        }
    }
}

pub struct PackResult {
    pub node: Node,
    /// tex.web `last_badness`: 0 unless the pack was under/tight/overfull
    pub badness: i32,
    /// requested − natural along the packing axis ("x" in tex.web hpack)
    pub delta: i64,
    pub stretch: [i64; 4],
    pub shrink: [i64; 4],
    pub sign: u8,
    pub order: u8,
    /// etex.ch `LR_problems` found by hpack's TeXXeT check:
    /// 10000 × missing end nodes + unmatched end nodes
    pub lr_problems: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackRecord {
    pub badness: i32,
    pub delta: i64,
    pub stretch: [i64; 4],
    pub shrink: [i64; 4],
    pub sign: u8,
    pub order: u8,
}

impl PackResult {
    pub fn record(&self) -> PackRecord {
        PackRecord {
            badness: self.badness,
            delta: self.delta,
            stretch: self.stretch,
            shrink: self.shrink,
            sign: self.sign,
            order: self.order,
        }
    }
}

pub(crate) fn glue_sums(list: &[Node]) -> ([i64; 4], [i64; 4]) {
    let mut stretch = [0i64; 4];
    let mut shrink = [0i64; 4];
    for n in list {
        let g = match n {
            Node::Glue(g, _) => g,
            Node::Leaders { glue: g, .. } => g,
            _ => continue,
        };
        stretch[g.stretch_order as usize] += g.stretch as i64;
        shrink[g.shrink_order as usize] += g.shrink as i64;
    }
    (stretch, shrink)
}

/// shared tail of hpack/vpackage: glue decision, capped shrink, last_badness,
/// and the overfull-rule marker for overfull hboxes (tex.web §656-659).
fn finish_glue(
    mut list: NodeList,
    target: i64,
    natural: i64,
    stretch: [i64; 4],
    shrink: [i64; 4],
    horizontal: bool,
    eqtb: &crate::eqtb::Eqtb,
) -> (NodeList, u8, u8, f64, i32, i64) {
    use crate::prim::DimParam;
    let x = target - natural;
    let nonempty = !list.is_empty();
    let (sign, order, mut set) = compute_glue_set(target, natural, stretch, shrink);
    let mut bad = 0i32;
    if x > 0 {
        // stretching: badness only meaningful in the normal order
        if order == 0 && nonempty {
            bad = scaled::badness(clamp_i32(x), clamp_i32(stretch[0]));
        }
    } else if x < 0 {
        if order == 0 {
            if (shrink[0] as i64) < -x {
                // overfull: clamp the glue ratio to exactly 1
                set = 1.0;
                if nonempty {
                    bad = 1_000_000;
                    let excess = -x - shrink[0];
                    let fuzz = eqtb.dim_params[DimParam::Hfuzz.idx() as usize] as i64;
                    let rule_w = eqtb.dim_params[DimParam::OverfullRule.idx() as usize] as i64;
                    if horizontal && rule_w > 0 && excess > fuzz {
                        // tex.web §666: new_rule keeps running height and
                        // depth, so the marker spans the whole line
                        list.push(Node::Rule {
                            width: rule_w as i32,
                            height: crate::build::RULE_FILL,
                            depth: crate::build::RULE_FILL, attr: eqtb.cur_attr,
                        });
                    }
                }
            } else if nonempty {
                bad = scaled::badness(clamp_i32(-x), clamp_i32(shrink[0]));
            }
        }
    }
    (list, sign, order, set, bad, x)
}

/// \hbox packing; `additional` selects tex.web's m=additional (\hbox spread).
pub fn hpack_add(
    list: NodeList,
    w: Option<i32>,
    additional: bool,
    kind: u8,
    eqtb: &crate::eqtb::Eqtb,
) -> PackResult {
    let (nat_w, h, d) = hlist_dims(&list, eqtb);
    let nat = nat_w as i64;
    let (stretch, shrink) = glue_sums(&list);
    let mut target = w.map(|v| v as i64).unwrap_or(nat);
    if additional {
        target = nat + target;
    }
    let (mut list, sign, order, set, bad, delta) =
        finish_glue(list, target, nat, stretch, shrink, true, eqtb);
    // etex.ch: `exit: if TeXXeT_en then <Check for LR anomalies at the end
    // of hpack>` — after the glue decision and any overfull rule
    let lr_problems =
        if eqtb.int_params[crate::prim::IntParam::TeXXeTEnabled.idx() as usize] > 0 {
            crate::texxet::hpack_lr_check(&mut list)
        } else {
            0
        };
    PackResult {
        node: Node::Box {
            kind,
            w: target as i32,
            h,
            d,
            shift: 0,
            list,
            glue_sign: sign,
            glue_order: order,
            glue_set: set,
            lr: 0,
            dir: 0, attr: eqtb.cur_attr,
        },
        badness: bad,
        delta,
        stretch,
        shrink,
        sign,
        order,
        lr_problems,
    }
}

/// \hpack packing with exact (or natural) target
pub fn hpack(list: NodeList, w: Option<i32>, kind: u8, eqtb: &crate::eqtb::Eqtb) -> PackResult {
    hpack_add(list, w, false, kind, eqtb)
}

/// \hpack that also performs tex.web's adjustment migration (§12900-12903,
/// §12956-12957, §13006-13016): this is the `adjust_tail<>null` hpack case —
/// the display-math finish (§22507) and adjusted-hbox group (§21057). Every
/// top-level `Ins`/`Mark` node is REMOVED from the hlist and appended to the
/// returned migration list, and every `VAdjust` node is replaced in place by
/// the contents of its vlist (canonical `adjust_ptr` splice, §13008-13012).
/// Callers splice the migrated nodes into the enclosing vertical list right
/// after the box (tex.web §22611, §20897-20902); the material of
/// `\vadjust pre` (pdftex.web `pre_adjust_tail`) follows all the rest, as
/// pdftex.web <Append the glue or equation number following the display>.
pub fn hpack_migrate(
    list: NodeList,
    w: Option<i32>,
    kind: u8,
    eqtb: &crate::eqtb::Eqtb,
) -> (PackResult, NodeList) {
    let mut migrated: NodeList = Vec::new();
    let mut pre: NodeList = Vec::new();
    let mut kept: NodeList = Vec::with_capacity(list.len());
    for n in list {
        match n {
            Node::Ins { .. } | Node::Mark { .. } => migrated.push(n),
            // §13008-13012: an adjust_node's own vlist joins the adjustment
            // list and the node is freed — it never stays in the hlist
            Node::VAdjust(inner, _) => migrated.extend(inner),
            Node::PreAdjust(inner, _) => pre.extend(inner),
            other => kept.push(other),
        }
    }
    migrated.append(&mut pre);
    (hpack(kept, w, kind, eqtb), migrated)
}

/// \vbox/\vtop packing with explicit max depth (tex.web vpackage's `l`):
/// \vbox passes box_max_depth, \vsplit split_max_depth, the page builder
/// page_max_depth. Excess depth moves into the natural height *before*
/// glue setting.
pub fn vpack_add_md(
    list: NodeList,
    h: Option<i32>,
    additional: bool,
    kind: u8,
    eqtb: &crate::eqtb::Eqtb,
    max_depth: i32,
) -> PackResult {
    // tex.web vpackage §668-669: a rule with running (null) width does not
    // compete for the box width and STAYS running; ship_out resolves it to
    // the enclosing box's width (§633), so `\unvcopy` of the list into a
    // wider box draws it wider.
    let (w, nat_h, nat_d) = vlist_dims(&list, eqtb);
    let (mut x, mut d) = (nat_h as i64, nat_d as i64);
    let (stretch, shrink) = glue_sums(&list);
    if d > max_depth as i64 {
        x += d - max_depth as i64;
        d = max_depth as i64;
    }
    let mut target = h.map(|v| v as i64).unwrap_or(x);
    if additional {
        target = x + target;
    }
    let (list, sign, order, set, bad, delta) =
        finish_glue(list, target, x, stretch, shrink, false, eqtb);
    PackResult {
        node: Node::Box {
            kind,
            w,
            h: target as i32,
            d: d as i32,
            shift: 0,
            list,
            glue_sign: sign,
            glue_order: order,
            glue_set: set,
            lr: 0,
            dir: 0, attr: eqtb.cur_attr,
        },
        badness: bad,
        delta,
        stretch,
        shrink,
        sign,
        order,
        lr_problems: 0,
    }
}

/// vpack with no depth clamp (tex.web's max_dimen `l`)
pub fn vpack(list: NodeList, h: Option<i32>, kind: u8, eqtb: &crate::eqtb::Eqtb) -> PackResult {
    vpack_add_md(list, h, false, kind, eqtb, i32::MAX)
}

/// \vtop: vpackage to the target, then readjust per tex.web package() §1074:
/// height becomes the *raw* height of the first box/rule item (0 if the list
/// is empty or starts with anything else), and the depth absorbs the rest.
pub fn vtop_md(
    list: NodeList,
    h: Option<i32>,
    additional: bool,
    eqtb: &crate::eqtb::Eqtb,
    max_depth: i32,
) -> PackResult {
    let mut res = vpack_add_md(list, h, additional, VTOP, eqtb, max_depth);
    if let Node::Box {
        h: hh, d: dd, list, ..
    } = &mut res.node
    {
        // tex.web §1087: The height of a \vtop box is inherited from the
        // FIRST item on its list, if that item is an hlist, vlist, or rule;
        // otherwise the \vtop height is zero.
        let first_h = match list.first() {
            Some(Node::Box { h: bh, .. }) => *bh,
            Some(Node::Rule { height, .. }) => *height,
            Some(Node::NativeGlyphRun { height, .. }) => *height,
            _ => 0,
        };
        *dd = *dd - first_h + *hh;
        *hh = first_h;
    }
    res
}

/// \vtop with box_max_depth (the tex.web package() default)
pub fn vtop(list: NodeList, h: Option<i32>, eqtb: &crate::eqtb::Eqtb) -> PackResult {
    use crate::prim::DimParam;
    let md = eqtb.dim_params[DimParam::BoxMaxDepth.idx() as usize];
    vtop_md(list, h, false, eqtb, md)
}

/// leader replication positions along one axis, tex.web hlist_out/vlist_out
/// §12456/§12625: returns (positions of each copy, lx spacing, final cur).
/// `left_edge` is the containing box's content origin; inputs/outputs in the
/// caller's units (sp at shipout) with tex.web's ±10 rounding compensation.
pub fn leader_layout(
    kind: u8,
    leader_wd: i64,
    total_w: i64,
    left_edge: i64,
    mut cur: i64,
) -> (Vec<i64>, i64, i64) {
    let mut lx = 0i64;
    let mut out = Vec::new();
    if leader_wd <= 0 || total_w <= 0 {
        return (out, lx, cur);
    }
    let rule_wd = total_w + 10; // compensate for floating-point rounding
    let edge = cur + rule_wd;
    if kind == LEADERS_A {
        let save = cur;
        cur = left_edge + leader_wd * ((cur - left_edge) / leader_wd);
        if cur < save {
            cur += leader_wd;
        }
    } else {
        let lq = rule_wd / leader_wd;
        let lr = rule_wd % leader_wd;
        if kind == LEADERS_C {
            cur += lr / 2;
        } else {
            lx = lr / (lq + 1);
            cur += (lr - (lq - 1) * lx) / 2;
        }
    }
    while cur + leader_wd <= edge {
        out.push(cur);
        cur += leader_wd + lx;
    }
    (out, lx, edge - 10)
}

// ---------- pdfTeX font expansion packing (pdftex.web hpack m=2/3) ----------

/// `divide_scaled(s, m, dd)` + the `scaled_out` side effect (pdftex.web
/// §15825): integer division with `dd` extra decimal digits, half-up on the
/// remainder; `scaled_out` = the quotient*divisor rounded onto the 10^-dd
/// raster. Used only for the expansion-ratio computation, where the result
/// is the ratio (a per-mille integer), not a scaled quantity.
pub(crate) fn divide_scaled(s: i64, m: i64, dd: u32) -> (i64, i64) {
    let (mut s, mut m) = (s, m);
    let mut sign = 1i64;
    if s < 0 {
        sign = -sign;
        s = -s;
    }
    if m < 0 {
        sign = -sign;
        m = -m;
    }
    let mut q = s / m;
    let mut r = s % m;
    for _ in 0..dd {
        q = 10 * q + (10 * r) / m;
        r = (10 * r) % m;
    }
    if 2 * r >= m {
        q += 1;
        r -= m;
    }
    let ten_pow = 10i64.pow(dd);
    let scaled_out = sign * (s - r / ten_pow);
    (sign * q, scaled_out)
}

/// `get_ef_code(f, c)`
#[inline]
pub fn char_ef_code(eqtb: &crate::eqtb::Eqtb, f: FontId, c: u8) -> i32 {
    eqtb.expand.get(f as usize).map_or(1000, |x| x.ef_code(c))
}

/// `char_stretch(f, c)`: extra room this character gains at the font's
/// maximum stretch (scaled by its efcode).
pub fn char_stretch(eqtb: &crate::eqtb::Eqtb, f: FontId, c: u8) -> i32 {
    let x = match eqtb.expand.get(f as usize) {
        Some(x) => x,
        None => return 0,
    };
    if x.stretch == 0 {
        return 0;
    }
    let ef = x.ef_code(c);
    if ef <= 0 {
        return 0;
    }
    let kf = match eqtb.fonts.get(x.stretch as usize) {
        Some(k) => k,
        None => return 0,
    };
    let dw = kf.char_width(c) - eqtb.fonts[f as usize].char_width(c);
    if dw > 0 {
        crate::tfm::round_xn_over_d(dw, ef, 1000)
    } else {
        0
    }
}

/// `char_shrink(f, c)`
pub fn char_shrink(eqtb: &crate::eqtb::Eqtb, f: FontId, c: u8) -> i32 {
    let x = match eqtb.expand.get(f as usize) {
        Some(x) => x,
        None => {
            return 0;
        }
    };
    if x.shrink == 0 {
        return 0;
    }
    let ef = x.ef_code(c);
    if ef <= 0 {
        return 0;
    }
    let kf = match eqtb.fonts.get(x.shrink as usize) {
        Some(k) => k,
        None => return 0,
    };
    let dw = eqtb.fonts[f as usize].char_width(c) - kf.char_width(c);
    if dw > 0 {
        crate::tfm::round_xn_over_d(dw, ef, 1000)
    } else {
        0
    }
}

/// `get_kern(f, lc, rc)`: the first kern amount in `lc`'s lig/kern program
/// that fires before `rc` (walks the TFM program like build.rs's finder,
/// but without the stop-on-first-match shortcut: pdftex scans for kerns).
pub fn get_kern(eqtb: &crate::eqtb::Eqtb, f: FontId, lc: u8, rc: u8) -> i32 {
    let font = match eqtb.fonts.get(f as usize) {
        Some(ft) => ft,
        None => return 0,
    };
    let ci = match font.chars.get(lc as usize) {
        Some(ci) => ci,
        None => return 0,
    };
    if ci.tag != crate::tfm::TAG_LIG {
        return 0;
    }
    let mut k = ci.remainder as usize;
    if let Some(first) = font.lig_kern.get(k) {
        if first.skip > 128 {
            k = 256 * first.op as usize + first.rem as usize;
        }
    }
    let mut jumps = 0;
    loop {
        let step = match font.lig_kern.get(k) {
            Some(s) => s,
            None => return 0,
        };
        if step.next_char == rc && step.skip <= 128 && step.op >= 128 {
            let idx = ((step.op as usize) - 128) * 256 + step.rem as usize;
            return font.kerns.get(idx).copied().unwrap_or(0);
        }
        if step.skip == 0 {
            k += 1;
        } else if step.stop {
            return 0;
        } else {
            k += step.skip as usize + 1;
        }
        jumps += 1;
        if jumps > 512 || k >= font.lig_kern.len() {
            return 0;
        }
    }
}

/// `kern_stretch(p)`: stretch of the implicit kern node `p` sitting between
/// characters `lc` (prev) and `rc` (next) of font `f`.
pub fn kern_stretch(eqtb: &crate::eqtb::Eqtb, f: FontId, lc: u8, rc: u8, cur_kern: i32) -> i32 {
    let x = match eqtb.expand.get(f as usize) {
        Some(x) => x,
        None => return 0,
    };
    if x.stretch == 0 {
        return 0;
    }
    let ef = x.ef_code(lc);
    let d = get_kern(eqtb, x.stretch, lc, rc);
    crate::tfm::round_xn_over_d(d - cur_kern, ef, 1000)
}

/// `kern_shrink(p)`: how much the implicit kern `p` (currently `cur_kern`)
/// can *reduce* the line when the font is swapped to its max-shrink variant.
/// pdftex.web §20779-20806: `round_xn_over_d(width(p) - d, ef, 1000)` —
/// opposite orientation to `kern_stretch`, and signed: kerns that *grow*
/// in the shrunk font contribute negative shrink to `total_font_shrink`.
pub fn kern_shrink(eqtb: &crate::eqtb::Eqtb, f: FontId, lc: u8, rc: u8, cur_kern: i32) -> i32 {
    let x = match eqtb.expand.get(f as usize) {
        Some(x) => x,
        None => return 0,
    };
    if x.shrink == 0 {
        return 0;
    }
    let ef = x.ef_code(lc);
    let d = get_kern(eqtb, x.shrink, lc, rc);
    crate::tfm::round_xn_over_d(cur_kern - d, ef, 1000)
}

/// Is font `f` expandable at all (a configured step and a stretch or shrink
/// limit)? `check_expand_pars`' first test.
pub fn font_expand_enabled(eqtb: &crate::eqtb::Eqtb, f: FontId) -> bool {
    match eqtb.expand.get(f as usize) {
        Some(x) => x.step != 0 && (x.stretch != 0 || x.shrink != 0),
        None => false,
    }
}

/// `do_subst_font` for one char/ligature position: swap the font id to the
/// variant expanded by `ratio * ef / 1000` (clamped inside `expand_font`).
fn do_subst_font(eng: &mut crate::engine::Engine, f: &mut FontId, c: u8, ex_ratio: i32) {
    let ef = char_ef_code(&eng.eqtb, *f, c);
    if ef == 0 {
        return;
    }
    let k = {
        let x = match eng.eqtb.expand.get(*f as usize) {
            Some(x) => x,
            None => return,
        };
        let (st, sh) = (x.stretch, x.shrink);
        if st != 0 && ex_ratio > 0 {
            let maxr = eng.eqtb.expand.get(st as usize).map_or(0, |x| x.ratio);
            let e = ext_xn_over_d(ex_ratio as i64 * ef as i64, maxr as i64, 1_000_000);
            eng.expand_font(*f, e)
        } else if sh != 0 && ex_ratio < 0 {
            let maxr = eng.eqtb.expand.get(sh as usize).map_or(0, |x| x.ratio);
            let e = ext_xn_over_d(ex_ratio as i64 * ef as i64, -maxr as i64, 1_000_000);
            eng.expand_font(*f, e)
        } else {
            return;
        }
    };
    if k != *f {
        *f = k;
    }
}
fn subst_node_font(eng: &mut crate::engine::Engine, node: &mut Node, ratio: i32) {
    match node {
        Node::Char { c, font, .. } => do_subst_font(eng, font, *c, ratio),
        Node::Ligature {
            c,
            font,
            lig_width,
            lig_height,
            lig_depth,
            ..
        } => {
            let before = *font;
            do_subst_font(eng, font, *c, ratio);
            if *font != before {
                let nf = &eng.eqtb.fonts[*font as usize];
                *lig_width = nf.char_width(*c);
                *lig_height = nf.char_height(*c);
                *lig_depth = nf.char_depth(*c);
            }
        }
        Node::Disc(dc) => {
            for node in &mut dc.no_break {
                subst_node_font(eng, node, ratio);
            }
        }
        _ => {}
    }
}

/// `ext_xn_over_d(x, n, d)`: 64-bit-safe round(x*n/d) (pdftex change file).
fn ext_xn_over_d(x: i64, n: i64, d: i64) -> i32 {
    let neg = (x < 0) ^ (n < 0);
    let (x, n) = (x.abs(), n.abs());
    let q = (x * n) / d;
    let r = (x * n) % d;
    let u = if 2 * r >= d { q + 1 } else { q };
    (if neg { -u } else { u }).clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// pdfTeX `hpack(p, w, cal_expand_ratio)` + `subst_ex_font` fused: measure
/// the line's expandable stretch/shrink, compute the expansion ratio from
/// the leftover after normal glue, then repack with substituted fonts.
/// Falls back to plain `hpack` when expansion cannot apply (parameter
/// `pdfadjustspacing` <= 0, no expandable font, or the glue absorbs the
/// difference on its own).
pub fn hpack_expand(
    eng: &mut crate::engine::Engine,
    list: NodeList,
    w: i32,
    kind: u8,
) -> PackResult {
    if eng.eqtb.int_params[crate::prim::IntParam::PdfAdjustSpacing.idx() as usize] <= 0 {
        return hpack(list, Some(w), kind, &eng.eqtb);
    }
    // ---- pass 1: cal_expand_ratio ----
    let (nat_w, _, _) = hlist_dims(&list, &eng.eqtb);
    let (stretch, shrink) = glue_sums(&list);
    let x = w as i64 - nat_w as i64;
    let mut ratio: i32 = 0;
    let mut font_stretch: i64 = 0;
    let mut font_shrink: i64 = 0;
    if x > 0 {
        let no_inf = stretch[1] == 0 && stretch[2] == 0 && stretch[3] == 0;
        if no_inf {
            collect_char_stretch(eng, &list, &mut font_stretch, true);
            if font_stretch > 0 {
                let (q, _) = divide_scaled(x, font_stretch, 3);
                ratio = q.clamp(-1000, 1000) as i32;
            }
        }
    } else if x < 0 {
        let no_inf = shrink[1] == 0 && shrink[2] == 0 && shrink[3] == 0;
        if no_inf {
            collect_char_stretch(eng, &list, &mut font_shrink, false);
            if font_shrink > 0 {
                let (q, _) = divide_scaled(x, font_shrink, 3);
                ratio = q.clamp(-1000, 1000) as i32;
            }
        }
    }
    if ratio == 0 {
        return hpack(list, Some(w), kind, &eng.eqtb);
    }
    // ---- pass 2: subst_ex_font ----
    // pdftex rewrites the font ids of char/ligature nodes (and the
    // ligature's own dimensions, which our packing reads from the node) and
    // then, for an implicit kern whose neighbours are char/ligature nodes of
    // one expandable font, recomputes the kern from the expanded font's
    // lig/kern program (`width(p) := get_kern(font(prev_char_p), l, r)`).
    // The kern only moves when `kern_stretch`/`kern_shrink` is nonzero —
    // the same test that fed `font_stretch` in pass 1.
    let mut list = list;
    for node in &mut list {
        subst_node_font(eng, node, ratio);
    }
    for i in 1..list.len().saturating_sub(1) {
        if !matches!(list[i], Node::Kern(_, _)) {
            continue;
        }
        let Some((f, lc)) = char_or_lig(&list[i - 1]) else {
            continue;
        };
        let Some((rf, rc)) = char_or_lig(&list[i + 1]) else {
            continue;
        };
        if f != rf {
            continue;
        }
        let expanded = match eng.eqtb.expand.get(f as usize) {
            Some(x) if ratio > 0 => x.stretch,
            Some(x) if ratio < 0 => x.shrink,
            _ => 0,
        };
        if expanded == 0 {
            continue;
        }
        let (cur, attr) = match &list[i] {
            Node::Kern(k, a) => (*k, *a),
            _ => (0, Attr::NONE),
        };
        let nonzero = if ratio > 0 {
            kern_stretch(&eng.eqtb, f, lc, rc, cur) != 0
        } else {
            kern_shrink(&eng.eqtb, f, lc, rc, cur) != 0
        };
        if nonzero {
            list[i] = Node::Kern(get_kern(&eng.eqtb, expanded, lc, rc), attr);
        }
    }
    hpack(list, Some(w), kind, &eng.eqtb)
}

/// The character code of a char/ligature node along with its font id — the
/// pair pdftex reads for `kern_stretch`'s neighbours.
fn char_or_lig(n: &Node) -> Option<(FontId, u8)> {
    match n {
        Node::Char { c, font, .. } => Some((*font, *c)),
        Node::Ligature { c, font, .. } => Some((*font, *c)),
        _ => None,
    }
}

/// Sum `char_stretch`/`char_shrink` over characters and `kern_stretch`/
/// `kern_shrink` over implicit kerns (pdftex hpack cal_expand_ratio pass).
fn collect_char_stretch(
    eng: &crate::engine::Engine,
    list: &NodeList,
    total: &mut i64,
    stretch: bool,
) {
    let eqtb = &eng.eqtb;
    for i in 0..list.len() {
        match &list[i] {
            Node::Char { c, font, .. } | Node::Ligature { c, font, .. } => {
                *total += if stretch {
                    char_stretch(eqtb, *font, *c) as i64
                } else {
                    char_shrink(eqtb, *font, *c) as i64
                };
            }
            Node::Kern(d, _) => {
                // `kern_stretch(p)`: only when the kern sits directly
                // between two char/ligature nodes of the same expandable
                // font (pdftex's prev_char_p/link(p) guard)
                let prev = match i.checked_sub(1).map(|j| &list[j]) {
                    Some(Node::Char { c, font, .. } | Node::Ligature { c, font, .. }) => (*font, *c),
                    _ => continue,
                };
                let next = match list.get(i + 1) {
                    Some(Node::Char { c, font, .. } | Node::Ligature { c, font, .. }) => (*font, *c),
                    _ => continue,
                };
                if prev.0 != next.0 {
                    continue;
                }
                *total += if stretch {
                    kern_stretch(eqtb, prev.0, prev.1, next.1, *d) as i64
                } else {
                    kern_shrink(eqtb, prev.0, prev.1, next.1, *d) as i64
                };
            }
            Node::Disc(dc) => collect_char_stretch(eng, &dc.no_break, total, stretch),
            _ => {}
        }
    }
}

/// Split a vertical list at `target` natural height (tex.web vsplit).
/// Glue at the break is discarded. Height accounts for inter-box depth.
pub fn split_vlist(list: &[Node], target: i64) -> (NodeList, NodeList) {
    let mut height = 0i64;
    let mut depth = 0i64;
    let mut seen_box = false;
    let mut split_at: Option<usize> = None;
    for (i, node) in list.iter().enumerate() {
        match node {
            Node::Box { h, d, .. }
            | Node::Rule {
                height: h,
                depth: d,
                ..
            }
            | Node::Ins {
                height: h,
                depth: d,
                ..
            }
            | Node::NativeGlyphRun {
                height: h,
                depth: d,
                ..
            } => {
                let h = *h as i64;
                if seen_box && height + depth + h > target {
                    split_at = Some(i);
                    break;
                }
                height += depth + h;
                depth = *d as i64;
                seen_box = true;
            }
            Node::Glue(g, _) => {
                let w = g.width as i64;
                if seen_box && height + depth + w > target {
                    split_at = Some(i);
                    break;
                }
                height += depth + w;
                depth = 0;
            }
            Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) => {
                let w = *k as i64;
                if seen_box && height + depth + w > target {
                    split_at = Some(i);
                    break;
                }
                height += depth + w;
                depth = 0;
            }
            Node::Penalty(p, _) if *p <= crate::scaled::EJECT_PENALTY && seen_box => {
                split_at = Some(i);
                break;
            }
            _ => {}
        }
    }
    match split_at {
        None => (list.to_vec(), Vec::new()),
        Some(i) => {
            let at_glue = matches!(list[i], Node::Glue(_, _));
            let mut rest = list[i..].to_vec();
            if at_glue && !rest.is_empty() {
                rest.remove(0);
            }
            (list[..i].to_vec(), rest)
        }
    }
}
