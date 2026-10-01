//! The equivalent table (`eqtb`), register files, code tables, parameter
//! tables, and the save/undo stack implementing TeX's grouping semantics.
//!
//! Levels are 1-based (`level_one = 1`). Assignment with `\global` stores at
//! level 1 and saves nothing. Non-global assignment of a slot whose level is
//! below the current level pushes an undo record `(old value, old level)`;
//! group close (`pop_level`) applies undo records back to the boundary.

use std::rc::Rc;

use crate::boxes::{Glue, Node};
use crate::prim::{DimParam, GlueParam, IntParam, Prim, ToksParam};
use crate::tfm::Font;
use crate::token::{CsId, Token, CAT_OTHER};

pub const LEVEL_ONE: u16 = 1;
pub const MAX_GROUP_LEVEL: u16 = u16::MAX;
pub const NUM_REGISTERS: usize = 32768;
pub const MAX_SAVE_STACK: usize = 100_000;

/// What a control sequence can mean.
#[derive(Clone, Debug)]
pub enum Equiv {
    CountReg(u16),
    DimenReg(u16),
    SkipReg(u16),
    MuSkipReg(u16),
    ToksReg(u16),
    BoxReg(u16),
    /// `\chardef`: value is the character code
    CharDef(u32),
    /// `\let\x={`: the cs stands for a character token (raw token bits)
    CharTok(u32),
    MathCharDef(u16),
    FontRef(u16),
    /// \let alias: follows the target dynamically (TeX semantics)
    Alias(CsId),
    Prim(Prim),
    Macro(Rc<Macro>),
}

impl Equiv {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Equiv::CountReg(_) => "CountReg",
            Equiv::DimenReg(_) => "DimenReg",
            Equiv::SkipReg(_) => "SkipReg",
            Equiv::MuSkipReg(_) => "MuSkipReg",
            Equiv::ToksReg(_) => "ToksReg",
            Equiv::BoxReg(_) => "BoxReg",
            Equiv::CharDef(_) => "CharDef",
            Equiv::CharTok(_) => "CharTok",
            Equiv::MathCharDef(_) => "MathCharDef",
            Equiv::FontRef(_) => "FontRef",
            Equiv::Alias(_) => "Alias",
            Equiv::Prim(_) => "Prim",
            Equiv::Macro(_) => "Macro",
        }
    }

    /// e-TeX eq_define's `eq_type(p)=t and equiv(p)=e`: the same meaning
    /// object (a macro's token list by identity, everything else by value).
    pub(crate) fn same(a: Option<&Equiv>, b: Option<&Equiv>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(Equiv::Macro(p)), Some(Equiv::Macro(q))) => Rc::ptr_eq(p, q),
            (Some(Equiv::Prim(p)), Some(Equiv::Prim(q))) => p == q,
            (Some(Equiv::CountReg(p)), Some(Equiv::CountReg(q)))
            | (Some(Equiv::DimenReg(p)), Some(Equiv::DimenReg(q)))
            | (Some(Equiv::SkipReg(p)), Some(Equiv::SkipReg(q)))
            | (Some(Equiv::MuSkipReg(p)), Some(Equiv::MuSkipReg(q)))
            | (Some(Equiv::ToksReg(p)), Some(Equiv::ToksReg(q)))
            | (Some(Equiv::BoxReg(p)), Some(Equiv::BoxReg(q)))
            | (Some(Equiv::MathCharDef(p)), Some(Equiv::MathCharDef(q)))
            | (Some(Equiv::FontRef(p)), Some(Equiv::FontRef(q))) => p == q,
            (Some(Equiv::CharDef(p)), Some(Equiv::CharDef(q)))
            | (Some(Equiv::CharTok(p)), Some(Equiv::CharTok(q)))
            | (Some(Equiv::Alias(p)), Some(Equiv::Alias(q))) => p == q,
            _ => false,
        }
    }
}

/// e-TeX's reassignment test for glue: TeX shares one spec between copies,
/// and every all-zero value is the shared `zero_glue` (trap_zero_glue), so
/// only zero glue is reliably the same spec.
#[inline]
fn same_glue(a: &Glue, b: &Glue) -> bool {
    a.is_zero() && b.is_zero()
}

/// e-TeX's reassignment test for token lists: the same list, or both empty.
#[inline]
fn same_toks(a: &Rc<Vec<Token>>, b: &Rc<Vec<Token>>) -> bool {
    Rc::ptr_eq(a, b) || (a.is_empty() && b.is_empty())
}

#[derive(Clone, Debug)]
pub struct Macro {
    pub num_params: u8,
    pub has_param_refs: bool,
    /// delimiter token lists per parameter (empty vec = undelimited)
    pub params: Vec<Vec<Token>>,
    /// parameter text before the first # (matched literally, discarded)
    pub prefix: Vec<Token>,
    pub body: Rc<[Token]>,
    pub long: bool,
    pub outer: bool,
    pub protected: bool,
    /// Derived replacement plan; excluded from format serialization and \ifx.
    pub replacement: std::cell::RefCell<Option<MacroReplacement>>,
}

#[derive(Clone, Debug)]
pub struct MacroReplacement {
    pub(crate) body: Rc<[Token]>,
    pub(crate) references: Rc<[(usize, usize)]>,
}
impl Macro {
    pub(crate) fn ensure_replacement_plan(&self) -> Rc<[(usize, usize)]> {
        let mut cached = self.replacement.borrow_mut();
        if cached
            .as_ref()
            .is_none_or(|plan| !Rc::ptr_eq(&plan.body, &self.body))
        {
            let is_reference = |token: &Token| (0x4000_0001..0x8000_0000).contains(&token.0);
            let count = self.body.iter().filter(|token| is_reference(token)).count();
            let mut found = self
                .body
                .iter()
                .enumerate()
                .filter(|(_, token)| is_reference(token))
                .map(|(position, token)| (position, (token.0 & 0x3FFF_FFFF).wrapping_sub(1) as usize));
            // A counted (trusted-length) source fills the slice in place.
            let references: Rc<[(usize, usize)]> =
                (0..count).map(|_| found.next().expect("counted reference")).collect();
            *cached = Some(MacroReplacement {
                body: self.body.clone(),
                references: references.clone(),
            });
            references
        } else {
            cached.as_ref().unwrap().references.clone()
        }
    }

    /// Length of the replacement for `args` under this macro's replacement
    /// plan `references`, or None beyond `limit`.
    pub(crate) fn replacement_length(
        &self,
        references: &[(usize, usize)],
        args: &crate::input::MacroArgs,
        limit: usize,
    ) -> Option<usize> {
        let mut length = self.body.len();
        for &(_, parameter) in references {
            // A reference without a supplied argument contributes nothing.
            let arg_len = args.get(parameter).map_or(0, <[Token]>::len);
            let next = length.checked_sub(1)?.checked_add(arg_len)?;
            if next > limit {
                return None;
            }
            length = next;
        }
        (length <= limit).then_some(length)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LevelType {
    Group,
    Simple,
    SemiSimple,
    Box,
    MacroCall,
    NoLine,
    Balanced,
    MathShift,
    MathLeft,
    MathGroup,
}

/// tex.web box_context encodings (`box_flag` and friends): a context below
/// `BOX_FLAG` is a shift, above it a `\setbox`, `\shipout` or leaders box.
pub const BOX_FLAG: i32 = 1 << 30;
pub const GLOBAL_BOX_FLAG: i32 = BOX_FLAG + 32768;
pub const SHIP_OUT_FLAG: i32 = BOX_FLAG + 65536;
pub const LEADER_FLAG: i32 = SHIP_OUT_FLAG + 1;

/// tex.web's `cur_group` codes, as `\currentgrouptype` reports them.
pub mod group_code {
    pub const BOTTOM: u8 = 0;
    pub const SIMPLE: u8 = 1;
    pub const HBOX: u8 = 2;
    pub const ADJUSTED_HBOX: u8 = 3;
    pub const VBOX: u8 = 4;
    pub const VTOP: u8 = 5;
    pub const ALIGN: u8 = 6;
    pub const NO_ALIGN: u8 = 7;
    pub const OUTPUT: u8 = 8;
    pub const MATH: u8 = 9;
    pub const DISC: u8 = 10;
    pub const INSERT: u8 = 11;
    pub const VCENTER: u8 = 12;
    pub const MATH_CHOICE: u8 = 13;
    pub const SEMI_SIMPLE: u8 = 14;
    pub const MATH_SHIFT: u8 = 15;
    pub const MATH_LEFT: u8 = 16;
}

/// What e-TeX keeps in the save stack next to a level boundary (`saved(-2)`
/// to `saved(-4)`): the group code and the box information `\showgroups`
/// reads back.
#[derive(Clone, Copy, Debug)]
pub struct GroupMeta {
    pub code: u8,
    /// tex.web `box_context` (`saved(-4)`): 0, a shift, `box_flag + n`, ...
    pub context: i32,
    /// `saved(-2)`: the box dimension, the insertion class or the number of
    /// discretionary/\mathchoice parts already finished.
    pub spec: i32,
    /// `saved(-3)`: true for `exactly`, false for `additional`.
    pub exactly: bool,
}

impl GroupMeta {
    pub const fn new(code: u8) -> Self {
        GroupMeta {
            code,
            context: 0,
            spec: 0,
            exactly: true,
        }
    }

    /// The group a plain level kind opens.
    pub const fn of(kind: LevelType) -> Self {
        Self::new(match kind {
            LevelType::SemiSimple => group_code::SEMI_SIMPLE,
            LevelType::MathShift => group_code::MATH_SHIFT,
            LevelType::MathLeft => group_code::MATH_LEFT,
            LevelType::MathGroup => group_code::MATH,
            LevelType::Box => group_code::HBOX,
            _ => group_code::SIMPLE,
        })
    }
}

/// One open group: its metadata, the input line it began on (`saved(-1)`)
/// and the save-stack position of its boundary (`cur_boundary`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct GroupRec {
    pub meta: GroupMeta,
    pub line: i32,
    pub boundary: usize,
}

/// tex.web print_group's description of a group (without a leading
/// `entering`/`leaving`).
pub(crate) fn group_description(code: u8, level: u16, line: i32, entered: bool) -> String {
    let name = match code {
        group_code::BOTTOM => return "bottom level".to_string(),
        group_code::SIMPLE => "simple",
        group_code::SEMI_SIMPLE => "semi simple",
        group_code::HBOX => "hbox",
        group_code::ADJUSTED_HBOX => "adjusted hbox",
        group_code::VBOX => "vbox",
        group_code::VTOP => "vtop",
        group_code::ALIGN => "align",
        group_code::NO_ALIGN => "no align",
        group_code::OUTPUT => "output",
        group_code::DISC => "disc",
        group_code::INSERT => "insert",
        group_code::VCENTER => "vcenter",
        group_code::MATH => "math",
        group_code::MATH_CHOICE => "math choice",
        group_code::MATH_SHIFT => "math shift",
        _ => "math left",
    };
    let mut text = format!("{name} group (level {level})");
    if line != 0 {
        text.push_str(if entered {
            " entered at line "
        } else {
            " at line "
        });
        text.push_str(&line.to_string());
    }
    text
}

#[derive(Clone, Debug)]
pub enum SaveItem {
    Level(u16, LevelType),
    Eq(CsId, Option<Equiv>, u16),
    IntParam(u16, i32, u16),
    DimParam(u16, i32, u16),
    GlueParam(u16, Glue, u16),
    ToksParam(u16, Rc<Vec<Token>>, u16),
    Count(u16, i32, u16),
    Dimen(u16, i32, u16),
    Skip(u16, Glue, u16),
    MuSkip(u16, Glue, u16),
    Toks(u16, Rc<Vec<Token>>, u16),
    Box(u16, Option<Node>, u16),
    Cat(u8, u8, u16),
    MathCode(u8, u16, u16),
    DelCode(u8, i32, u16),
    LcCode(u8, u8, u16),
    SfCode(u8, u16, u16),
    UcCode(u8, u8, u16),
    UnicodeCase(bool, u32, Option<(u32, u16)>),
    UnicodeCat(u32, Option<(u8, u16)>),
    UnicodeMath(u32, Option<(u32, u16)>),
    UnicodeDel(u32, Option<(i64, u16)>),
    UnicodeSf(u32, Option<(u16, u16)>),
    StyleFont(u8, u16, u16, u16), // (0=textfont,1=scriptfont,2=ssfont, fam, fontid, level)
    FontParam(u16, usize, i32, u16), // font, param index (0-based), old, level
    HyphenChar(u16, i32, u16),
    SkewChar(u16, i32, u16),
    /// previous current font and its level (tex.web cur_font_loc is an eqtb
    /// entry, so a font selection inside a group is restored at \endgroup)
    CurFont(u16, u16),
    /// engine-side \parshape value before a local assignment/clear
    /// (tex.web level-tracks par_shape_ptr through eq_define)
    ParShape(Vec<(i32, i32)>, u16),
    /// Previous e-TeX penalty-array value before a local assignment.
    PenaltyShape(u8, Rc<[i32]>, u16),
    AfterGroup(Token),
}

/// An eqtb location named by e-TeX's assignment and restore tracing
/// (tex.web show_eqtb regions; e-TeX show_sa for registers above 255).
#[derive(Clone, Copy, Debug)]
pub(crate) enum TraceSlot {
    Eq(CsId),
    IntParam(u16),
    DimParam(u16),
    GlueParam(u16),
    ToksParam(u16),
    Count(u16),
    Dimen(u16),
    Skip(u16),
    MuSkip(u16),
    Toks(u16),
    Box(u16),
    Cat(u32),
    MathCode(u32),
    DelCode(u32),
    LcCode(u32),
    UcCode(u32),
    SfCode(u32),
    CurFont,
    /// (style 0=text 1=script 2=scriptscript, family)
    StyleFont(u8, u16),
    ParShape,
    /// e-TeX penalty array index (Prim::penalty_shape_index)
    PenaltyShape(u8),
}

/// The value of a [`TraceSlot`] when the event happened.
#[derive(Clone, Debug)]
pub(crate) enum TraceValue {
    Int(i64),
    Glue(Glue),
    Toks(Rc<Vec<Token>>),
    /// A box register's value and the `depth_threshold`/`breadth_max` its
    /// display uses; `true` when it is shown with show_box (a register above
    /// 255 lives in an e-TeX sparse array) rather than as an eqtb entry.
    Box(Option<Node>, i64, i64, bool),
    Eq(Option<Equiv>),
    Font(u16),
    /// Entry count and the first two values of a shape array.
    Shape(i32, i32, i32),
}

/// One transcript line queued by the table and printed by the engine before
/// its next transcript output, so lines keep their order relative to every
/// other message.
#[derive(Clone, Debug)]
pub(crate) enum TraceEvent {
    /// `{<verb> <eqtb entry>}` (e-TeX restore_trace / show_sa).
    Eqtb {
        verb: &'static str,
        slot: TraceSlot,
        value: TraceValue,
        /// `\escapechar` at the time of the event.
        escape: i32,
        online: bool,
    },
    /// A rendered `{...}` line (group tracing).
    Text { text: String, online: bool },
}

#[derive(Clone)]
struct EqEntry {
    equiv: Option<Equiv>,
    level: u16,
}

pub struct Eqtb {
    entries: Vec<EqEntry>,
    pub save_stack: Vec<SaveItem>,
    pub cur_level: u16,
    group_level_capacity_exceeded: bool,
    pending_interaction_mode: Option<i32>,
    /// Set once any control sequence has been given an \outer macro meaning
    /// and never cleared: until then no token can be \outer, so scanners
    /// skip the per-token meaning lookup of tex.web §336.
    outer_macros: bool,

    /// tex.web cur_font_loc: current font, group-scoped via SaveItem::CurFont
    pub cur_font_val: u16,
    cur_font_level: u16,
    /// Pending `\tracingassigns`/`\tracingrestores` lines, drained by the
    /// engine in order before any other transcript output.
    pub(crate) trace_events: Vec<TraceEvent>,
    /// The open groups, innermost last (e-TeX's group stack in the save
    /// stack).
    pub(crate) groups: Vec<GroupRec>,

    pub int_params: Vec<i32>,
    pub int_levels: Vec<u16>,
    pub dim_params: Vec<i32>,
    pub dim_levels: Vec<u16>,
    pub glue_params: Vec<Glue>,
    pub glue_levels: Vec<u16>,
    pub tok_params: Vec<Rc<Vec<Token>>>,
    pub tok_levels: Vec<u16>,

    pub count: Vec<i32>,
    pub count_levels: Vec<u16>,
    pub dimen: Vec<i32>,
    pub dimen_levels: Vec<u16>,
    pub skip: Vec<Glue>,
    pub skip_levels: Vec<u16>,
    pub muskip: Vec<Glue>,
    pub muskip_levels: Vec<u16>,
    pub toks: Vec<Rc<Vec<Token>>>,
    pub toks_levels: Vec<u16>,
    pub boxed: Vec<Option<Node>>,
    pub box_levels: Vec<u16>,

    pub cat: Vec<u8>,
    pub cat_levels: Vec<u16>,
    pub math_code: Vec<u16>,
    pub math_levels: Vec<u16>,
    pub del_code: Vec<i32>,
    pub del_levels: Vec<u16>,
    pub lc_code: Vec<u8>,
    pub lc_levels: Vec<u16>,
    pub sf_code: Vec<u16>,
    pub sf_levels: Vec<u16>,
    pub uc_code: Vec<u8>,
    pub uc_levels: Vec<u16>,
    /// Sparse Unicode overrides; the byte tables remain the pdfTeX hot path.
    pub unicode_cat_codes: crate::FxHashMap<u32, (u8, u16)>,
    pub unicode_math_codes: crate::FxHashMap<u32, (u32, u16)>,
    pub unicode_del_codes: crate::FxHashMap<u32, (i64, u16)>,
    pub unicode_sf_codes: crate::FxHashMap<u32, (u16, u16)>,
    pub unicode_case_codes: crate::FxHashMap<(bool, u32), (u32, u16)>,

    /// style_fonts[style][fam] -> font id (0 = none); style: 0=text 1=script 2=ss
    pub style_fonts: [[u16; 256]; 3],
    pub style_font_levels: [[u16; 256]; 3],

    pub fonts: Vec<Rc<Font>>,
    /// mutable copy of font params (\fontdimen), per font
    pub font_params: Vec<Vec<i32>>,
    pub font_param_levels: Vec<Vec<u16>>,
    pub hyphen_char: Vec<i32>,
    pub hyphen_char_levels: Vec<u16>,
    pub skew_char: Vec<i32>,
    pub skew_char_levels: Vec<u16>,
    /// control sequence each font was loaded as (\the\font)
    pub font_cs: Vec<CsId>,
    /// pdfTeX per-font expansion/letterspacing state (pdffontexpand,
    /// efcode/lpcode/rpcode/tagcode/kn*code, stretch/shrink font links).
    pub expand: Vec<FontExpand>,
}

/// The eight shared per-font code tables (`pdf_font_*_base` pointers),
/// cloned as a unit when an expanded variant aliases its base font.
pub type CodeTables = (
    Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    Option<Rc<std::cell::RefCell<[i32; 256]>>>,
);

#[derive(Clone)]
pub struct FontExpand {
    pub step: i32,
    pub auto_expand: bool,
    pub stretch: u16,
    pub shrink: u16,
    pub elink: u16,
    pub blink: u16,
    pub ratio: i32,
    pub ef: Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    pub lp: Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    pub rp: Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    pub kn_bs: Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    pub st_bs: Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    pub sh_bs: Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    pub kn_bc: Option<Rc<std::cell::RefCell<[i32; 256]>>>,
    pub kn_ac: Option<Rc<std::cell::RefCell<[i32; 256]>>>,
}

impl Default for FontExpand {
    fn default() -> Self {
        FontExpand {
            step: 0,
            auto_expand: false,
            stretch: 0,
            shrink: 0,
            elink: 0,
            blink: 0,
            ratio: 0,
            ef: None,
            lp: None,
            rp: None,
            kn_bs: None,
            st_bs: None,
            sh_bs: None,
            kn_bc: None,
            kn_ac: None,
        }
    }
}

impl FontExpand {
    #[inline]
    pub fn ef_code(&self, c: u8) -> i32 {
        self.ef.as_ref().map_or(1000, |t| t.borrow()[c as usize])
    }
    #[inline]
    pub fn lp_code(&self, c: u8) -> i32 {
        self.lp.as_ref().map_or(0, |t| t.borrow()[c as usize])
    }
    #[inline]
    pub fn rp_code(&self, c: u8) -> i32 {
        self.rp.as_ref().map_or(0, |t| t.borrow()[c as usize])
    }
    #[inline]
    pub fn kn_bs_code(&self, c: u8) -> i32 {
        self.kn_bs.as_ref().map_or(0, |t| t.borrow()[c as usize])
    }
    #[inline]
    pub fn st_bs_code(&self, c: u8) -> i32 {
        self.st_bs.as_ref().map_or(0, |t| t.borrow()[c as usize])
    }
    #[inline]
    pub fn sh_bs_code(&self, c: u8) -> i32 {
        self.sh_bs.as_ref().map_or(0, |t| t.borrow()[c as usize])
    }
    #[inline]
    pub fn kn_bc_code(&self, c: u8) -> i32 {
        self.kn_bc.as_ref().map_or(0, |t| t.borrow()[c as usize])
    }
    #[inline]
    pub fn kn_ac_code(&self, c: u8) -> i32 {
        self.kn_ac.as_ref().map_or(0, |t| t.borrow()[c as usize])
    }
    pub fn set_ef_code(&mut self, c: u8, v: i32) {
        let t = self
            .ef
            .get_or_insert_with(|| Rc::new(std::cell::RefCell::new([1000i32; 256])));
        t.borrow_mut()[c as usize] = v.clamp(0, 1000);
    }
    pub fn set_lp_code(&mut self, c: u8, v: i32) {
        let t = self
            .lp
            .get_or_insert_with(|| Rc::new(std::cell::RefCell::new([0i32; 256])));
        t.borrow_mut()[c as usize] = v.clamp(-1000, 1000);
    }
    pub fn set_rp_code(&mut self, c: u8, v: i32) {
        let t = self
            .rp
            .get_or_insert_with(|| Rc::new(std::cell::RefCell::new([0i32; 256])));
        t.borrow_mut()[c as usize] = v.clamp(-1000, 1000);
    }
    pub fn set_kn_bs_code(&mut self, c: u8, v: i32) {
        let t = self
            .kn_bs
            .get_or_insert_with(|| Rc::new(std::cell::RefCell::new([0i32; 256])));
        t.borrow_mut()[c as usize] = v.clamp(-1000, 1000);
    }
    pub fn set_st_bs_code(&mut self, c: u8, v: i32) {
        let t = self
            .st_bs
            .get_or_insert_with(|| Rc::new(std::cell::RefCell::new([0i32; 256])));
        t.borrow_mut()[c as usize] = v.clamp(-1000, 1000);
    }
    pub fn set_sh_bs_code(&mut self, c: u8, v: i32) {
        let t = self
            .sh_bs
            .get_or_insert_with(|| Rc::new(std::cell::RefCell::new([0i32; 256])));
        t.borrow_mut()[c as usize] = v.clamp(-1000, 1000);
    }
    pub fn set_kn_bc_code(&mut self, c: u8, v: i32) {
        let t = self
            .kn_bc
            .get_or_insert_with(|| Rc::new(std::cell::RefCell::new([0i32; 256])));
        t.borrow_mut()[c as usize] = v.clamp(-1000, 1000);
    }
    pub fn set_kn_ac_code(&mut self, c: u8, v: i32) {
        let t = self
            .kn_ac
            .get_or_insert_with(|| Rc::new(std::cell::RefCell::new([0i32; 256])));
        t.borrow_mut()[c as usize] = v.clamp(-1000, 1000);
    }
    pub fn clone_tables(x: &FontExpand) -> CodeTables {
        (
            x.ef.clone(),
            x.lp.clone(),
            x.rp.clone(),
            x.kn_bs.clone(),
            x.st_bs.clone(),
            x.sh_bs.clone(),
            x.kn_bc.clone(),
            x.kn_ac.clone(),
        )
    }
    pub fn set_shared_tables(&mut self, t: CodeTables) {
        let (ef, lp, rp, kn_bs, st_bs, sh_bs, kn_bc, kn_ac) = t;
        self.ef = ef;
        self.lp = lp;
        self.rp = rp;
        self.kn_bs = kn_bs;
        self.st_bs = st_bs;
        self.sh_bs = sh_bs;
        self.kn_bc = kn_bc;
        self.kn_ac = kn_ac;
    }
}
/// 27-bit delcode layout: small_fam<<20 | small_char<<12 | big_fam<<8 | big_char
pub fn make_del_code(small_fam: u32, small_char: u32, big_fam: u32, big_char: u32) -> i32 {
    ((small_fam << 20) | (small_char << 12) | (big_fam << 8) | big_char) as i32
}

impl Eqtb {
    pub fn new(ini: bool) -> Self {
        let cat = if ini {
            crate::token::CatTable::initex().0
        } else {
            crate::token::CatTable::new().0
        };
        let mut math_code = [0u16; 256];
        // tex.web §4838 INITEX defaults: mathcode(k)=k (class 0, fam 0) for
        // every char; digits get k+var_code (class 7, fam 0); letters get
        // k+var_code+0x100 (class 7, fam 1). The var_code class makes the
        // family follow \fam; the fam field is the fallback.
        for c in 0..256u16 {
            math_code[c as usize] = c;
        }
        for c in b'0'..=b'9' {
            math_code[c as usize] = 0x7000 | c as u16;
        }
        for c in b'A'..=b'Z' {
            math_code[c as usize] = 0x7100 | c as u16;
        }
        for c in b'a'..=b'z' {
            math_code[c as usize] = 0x7100 | c as u16;
        }
        let mut del_code = [-1i32; 256];
        for (c, d) in [
            (b'(', b'('),
            (b')', b')'),
            (b'[', b'['),
            (b']', b']'),
            (b'<', 0x3Cu8),
            (b'>', 0x3E),
        ] {
            del_code[c as usize] = make_del_code(7, d as u32, 7, d as u32);
        }
        del_code[b'.' as usize] = 0; // tex.web §240: period is null delimiter
        del_code[b'|' as usize] = make_del_code(7, 0x7C, 7, 0x7C);
        let mut lc_code = [0u8; 256];
        let mut sf_code = [1000u16; 256];
        let mut uc_code = [0u8; 256];
        // tex.web §1252 INITEX: sf_code = 999 for UPPERCASE letters only;
        // lowercase keeps the default 1000. Setting a-z to 999 shrank every
        // interword glue by 0.1% and flipped marginal line breaks.
        for c in b'a'..=b'z' {
            lc_code[c as usize] = c;
            uc_code[c as usize] = c - 32;
        }
        for c in b'A'..=b'Z' {
            lc_code[c as usize] = c + 32;
            uc_code[c as usize] = c;
            sf_code[c as usize] = 999;
        }
        let mut int_params = vec![0; crate::prim::NUM_INT_PARAMS];
        // e-TeX's \interactionmode mirrors TeX's runtime interaction state.
        // A fresh engine starts in error-stop mode (numeric value 3).
        int_params[IntParam::InteractionMode.idx() as usize] = 3;
        Eqtb {
            entries: Vec::new(),
            save_stack: Vec::new(),
            cur_level: LEVEL_ONE,
            group_level_capacity_exceeded: false,
            outer_macros: false,
            pending_interaction_mode: None,
            cur_font_val: 0,
            cur_font_level: LEVEL_ONE,
            trace_events: Vec::new(),
            groups: Vec::new(),
            int_params,
            int_levels: vec![LEVEL_ONE; crate::prim::NUM_INT_PARAMS],
            dim_params: vec![0; crate::prim::NUM_DIM_PARAMS],
            dim_levels: vec![LEVEL_ONE; crate::prim::NUM_DIM_PARAMS],
            glue_params: vec![Glue::zero(); crate::prim::NUM_GLUE_PARAMS],
            glue_levels: vec![LEVEL_ONE; crate::prim::NUM_GLUE_PARAMS],
            tok_params: vec![Rc::new(Vec::new()); crate::prim::NUM_TOKS_PARAMS],
            tok_levels: vec![LEVEL_ONE; crate::prim::NUM_TOKS_PARAMS],
            count: vec![0; NUM_REGISTERS],
            count_levels: vec![LEVEL_ONE; NUM_REGISTERS],
            dimen: vec![0; NUM_REGISTERS],
            dimen_levels: vec![LEVEL_ONE; NUM_REGISTERS],
            skip: vec![Glue::zero(); NUM_REGISTERS],
            skip_levels: vec![LEVEL_ONE; NUM_REGISTERS],
            muskip: vec![Glue::zero(); NUM_REGISTERS],
            muskip_levels: vec![LEVEL_ONE; NUM_REGISTERS],
            toks: vec![Rc::new(Vec::new()); NUM_REGISTERS],
            toks_levels: vec![LEVEL_ONE; NUM_REGISTERS],
            boxed: vec![None; NUM_REGISTERS],
            box_levels: vec![LEVEL_ONE; NUM_REGISTERS],
            cat: cat.to_vec(),
            cat_levels: vec![LEVEL_ONE; 256],
            math_code: math_code.to_vec(),
            math_levels: vec![LEVEL_ONE; 256],
            del_code: del_code.to_vec(),
            del_levels: vec![LEVEL_ONE; 256],
            lc_code: lc_code.to_vec(),
            lc_levels: vec![LEVEL_ONE; 256],
            sf_code: sf_code.to_vec(),
            sf_levels: vec![LEVEL_ONE; 256],
            uc_code: uc_code.to_vec(),
            uc_levels: vec![LEVEL_ONE; 256],
            unicode_case_codes: crate::FxHashMap::default(),
            unicode_cat_codes: crate::FxHashMap::default(),
            unicode_math_codes: crate::FxHashMap::default(),
            unicode_del_codes: crate::FxHashMap::default(),
            unicode_sf_codes: crate::FxHashMap::default(),
            style_fonts: [[0; 256]; 3],
            style_font_levels: [[LEVEL_ONE; 256]; 3],
            fonts: Vec::new(),
            font_params: Vec::new(),
            font_param_levels: Vec::new(),
            hyphen_char: Vec::new(),
            hyphen_char_levels: Vec::new(),
            skew_char: Vec::new(),
            skew_char_levels: Vec::new(),
            font_cs: Vec::new(),
            expand: Vec::new(),
        }
    }

    // ---------- cs equivalents ----------

    #[inline(always)]
    fn ensure_entry(&mut self, id: CsId) -> &mut EqEntry {
        let idx = id as usize;
        if idx >= self.entries.len() {
            self.entries.resize(
                idx + 1,
                EqEntry {
                    equiv: None,
                    level: LEVEL_ONE,
                },
            );
        }
        &mut self.entries[idx]
    }

    #[inline(always)]
    pub fn get(&self, id: CsId) -> Option<&Equiv> {
        self.entries.get(id as usize).and_then(|e| e.equiv.as_ref())
    }
    #[inline(always)]
    pub(crate) fn definition_level(&self, id: CsId) -> Option<u16> {
        self.entries.get(id as usize).map(|e| e.level)
    }

    /// follow \let aliases to the effective meaning
    #[inline(always)]
    pub fn resolve(&self, mut id: CsId) -> Option<&Equiv> {
        for _ in 0..1024 {
            match self.get(id) {
                Some(Equiv::Alias(next)) => id = *next,
                other => return other,
            }
        }
        None
    }
    #[inline]
    pub fn push_save(&mut self, item: SaveItem) {
        self.save_stack.push(item);
    }

    /// Allow one entry beyond the logical TeX limit so callers can finish the
    /// current operation with a structurally valid save stack. Main control
    /// turns this monotonic condition into a fatal diagnostic immediately
    /// after the operation completes.
    #[inline]
    pub(crate) fn save_stack_capacity_exceeded(&self) -> bool {
        self.save_stack.len() > MAX_SAVE_STACK
    }

    #[inline]
    pub(crate) fn group_level_capacity_exceeded(&self) -> bool {
        self.group_level_capacity_exceeded
    }

    /// Keep the readable e-TeX parameter aligned with a mode selected by the
    /// command line or by \batchmode/\nonstopmode/\scrollmode/\errorstopmode.
    pub(crate) fn set_runtime_interaction_mode(&mut self, value: i32) {
        let i = IntParam::InteractionMode.idx() as usize;
        self.int_params[i] = value;
        self.int_levels[i] = LEVEL_ONE;
        self.pending_interaction_mode = None;
    }

    pub(crate) fn take_pending_interaction_mode(&mut self) -> Option<i32> {
        self.pending_interaction_mode.take()
    }

    pub fn assign(&mut self, id: CsId, equiv: Equiv, global: bool) {
        if matches!(&equiv, Equiv::Macro(m) if m.outer) {
            self.outer_macros = true;
        }
        self.define_eq(id, Some(equiv), global);
    }

    pub fn undefine(&mut self, id: CsId, global: bool) {
        self.define_eq(id, None, global);
    }

    /// tex.web eq_define / geq_define for a control sequence.
    fn define_eq(&mut self, id: CsId, equiv: Option<Equiv>, global: bool) {
        let idx = id as usize;
        let cur_level = self.cur_level;
        if idx >= self.entries.len() {
            self.entries.resize(
                idx + 1,
                EqEntry {
                    equiv: None,
                    level: LEVEL_ONE,
                },
            );
        }
        let same = Equiv::same(self.entries[idx].equiv.as_ref(), equiv.as_ref());
        if !self.begin_assign(global, same, TraceSlot::Eq(id)) {
            return;
        }
        let entry = &mut self.entries[idx];
        if !global && entry.level < cur_level {
            let old = std::mem::replace(&mut entry.equiv, equiv);
            let ol = entry.level;
            entry.level = cur_level;
            self.push_save(SaveItem::Eq(id, old, ol));
        } else {
            entry.equiv = equiv;
            entry.level = if global { LEVEL_ONE } else { cur_level };
        }
        self.end_assign(TraceSlot::Eq(id));
    }

    // ---------- e-TeX assignment tracing ----------

    /// The start of e-TeX's eq_define/eq_word_define (local) or
    /// geq_define/geq_word_define (global). A local assignment of the value
    /// the slot already holds is a reassignment that changes nothing (not
    /// even the save stack); returns false then.
    #[inline]
    fn begin_assign(&mut self, global: bool, same: bool, slot: TraceSlot) -> bool {
        let reassign = same && !global;
        if self.int_params[IntParam::TracingAssigns as usize] > 0 {
            let verb = if reassign {
                "reassigning"
            } else if global {
                "globally changing"
            } else {
                "changing"
            };
            self.trace(verb, slot);
        }
        !reassign
    }

    #[inline]
    fn end_assign(&mut self, slot: TraceSlot) {
        if self.int_params[IntParam::TracingAssigns as usize] > 0 {
            self.trace("into", slot);
        }
    }

    #[inline]
    fn tracing_restores(&self) -> bool {
        self.int_params[IntParam::TracingRestores as usize] > 0
    }

    /// Queue `{verb slot=value}` with the slot's current value.
    #[cold]
    pub(crate) fn trace(&mut self, verb: &'static str, slot: TraceSlot) {
        let value = self.trace_value(slot);
        self.trace_with(verb, slot, value);
    }

    #[cold]
    pub(crate) fn trace_with(&mut self, verb: &'static str, slot: TraceSlot, value: TraceValue) {
        let escape = self.int_params[IntParam::EscapeChar as usize];
        let online = self.int_params[IntParam::TracingOnline as usize] > 0;
        self.trace_events.push(TraceEvent::Eqtb {
            verb,
            slot,
            value,
            escape,
            online,
        });
    }

    /// Queue a rendered `{...}` line.
    #[cold]
    pub(crate) fn trace_text(&mut self, text: String) {
        let online = self.int_params[IntParam::TracingOnline as usize] > 0;
        self.trace_events.push(TraceEvent::Text { text, online });
    }

    /// `group_trace`: `{entering ...}` or `{leaving ...}`.
    #[cold]
    fn trace_group(&mut self, leaving: bool, code: u8, level: u16, line: i32) {
        let text = format!(
            "{{{} {}}}",
            if leaving { "leaving" } else { "entering" },
            group_description(code, level, line, leaving)
        );
        self.trace_text(text);
    }

    /// Whether the sparse-table entry for `character` was assigned in a
    /// group (so unsave restores it rather than retaining a global value).
    fn sparse_is_local<T>(values: &crate::FxHashMap<u32, (T, u16)>, character: u32) -> bool {
        values
            .get(&character)
            .is_some_and(|value| value.1 > LEVEL_ONE)
    }

    fn trace_value(&self, slot: TraceSlot) -> TraceValue {
        match slot {
            TraceSlot::Eq(id) => TraceValue::Eq(self.get(id).cloned()),
            TraceSlot::IntParam(i) => TraceValue::Int(self.int_params[i as usize].into()),
            TraceSlot::DimParam(i) => TraceValue::Int(self.dim_params[i as usize].into()),
            TraceSlot::GlueParam(i) => TraceValue::Glue(self.glue_params[i as usize]),
            TraceSlot::ToksParam(i) => TraceValue::Toks(self.tok_params[i as usize].clone()),
            TraceSlot::Count(i) => TraceValue::Int(self.count[i as usize].into()),
            TraceSlot::Dimen(i) => TraceValue::Int(self.dimen[i as usize].into()),
            TraceSlot::Skip(i) => TraceValue::Glue(self.skip[i as usize]),
            TraceSlot::MuSkip(i) => TraceValue::Glue(self.muskip[i as usize]),
            TraceSlot::Toks(i) => TraceValue::Toks(self.toks[i as usize].clone()),
            TraceSlot::Box(i) => {
                let value = self.boxed[i as usize].clone();
                if i < 256 {
                    TraceValue::Box(value, 0, 1, false)
                } else {
                    let breadth = self.int_params[IntParam::ShowBoxBreadth as usize] as i64;
                    TraceValue::Box(
                        value,
                        (self.int_params[IntParam::ShowBoxDepth as usize] as i64).min(10_000),
                        if breadth <= 0 { 5 } else { breadth },
                        true,
                    )
                }
            }
            TraceSlot::Cat(c) => TraceValue::Int(self.cat_code(c).into()),
            TraceSlot::MathCode(c) => TraceValue::Int(self.math_code_for(c).into()),
            TraceSlot::DelCode(c) => TraceValue::Int(self.delimiter_code_for(c)),
            TraceSlot::LcCode(c) => TraceValue::Int(self.case_code(c, false).into()),
            TraceSlot::UcCode(c) => TraceValue::Int(self.case_code(c, true).into()),
            TraceSlot::SfCode(c) => TraceValue::Int(self.space_factor_code(c).into()),
            TraceSlot::CurFont => TraceValue::Font(self.cur_font_val),
            TraceSlot::StyleFont(s, f) => {
                TraceValue::Font(self.style_fonts[s as usize][f as usize])
            }
            // Shapes live on the engine, which passes their values itself.
            TraceSlot::ParShape | TraceSlot::PenaltyShape(_) => TraceValue::Shape(0, 0, 0),
        }
    }

    /// e-TeX restore_trace after unsave restored (`restored`) or kept the
    /// global value of `slot`.
    #[inline]
    fn trace_restore(&mut self, restored: bool, slot: TraceSlot) {
        if self.tracing_restores() {
            self.trace(if restored { "restoring" } else { "retaining" }, slot);
        }
    }

    // ---------- generic level-aware slots ----------

    #[inline]
    fn slot<T>(
        vals: &mut Vec<T>,
        levels: &mut [u16],
        idx: usize,
        v: T,
        global: bool,
        cur_level: u16,
        stack: &mut Vec<SaveItem>,
        mk: impl Fn(T, u16) -> SaveItem,
    ) {
        if !global && levels[idx] < cur_level {
            let old = std::mem::replace(&mut vals[idx], v);
            stack.push(mk(old, levels[idx]));
        } else {
            vals[idx] = v;
        }
        levels[idx] = if global { LEVEL_ONE } else { cur_level };
    }

    fn sparse_slot<T: Copy>(
        values: &mut crate::FxHashMap<u32, (T, u16)>,
        character: u32,
        value: T,
        global: bool,
        cur_level: u16,
        stack: &mut Vec<SaveItem>,
        save: impl FnOnce(Option<(T, u16)>) -> SaveItem,
    ) {
        let old = values.get(&character).copied();
        let old_level = old.map_or(LEVEL_ONE, |(_, level)| level);
        if !global && old_level < cur_level {
            stack.push(save(old));
        }
        values.insert(
            character,
            (value, if global { LEVEL_ONE } else { cur_level }),
        );
    }

    fn restore_sparse<T>(
        values: &mut crate::FxHashMap<u32, (T, u16)>,
        character: u32,
        old: Option<(T, u16)>,
    ) {
        if values
            .get(&character)
            .is_some_and(|value| value.1 > LEVEL_ONE)
        {
            if let Some(value) = old {
                values.insert(character, value);
            } else {
                values.remove(&character);
            }
        }
    }

    pub fn assign_int_param(&mut self, p: IntParam, v: i32, global: bool) {
        if p == IntParam::InteractionMode {
            self.pending_interaction_mode = Some(v);
            if !(0..=3).contains(&v) {
                // e-TeX rejects the assignment and keeps the previous mode.
                // The Engine consumes the attempted value at the dispatch
                // boundary and reports it with the current source location.
                return;
            }
        }
        let i = p.idx() as usize;
        // e-TeX changes interaction mode immediately and globally, even in a
        // group and even when \globaldefs is negative.
        let global = global || p == IntParam::InteractionMode;
        // Pseudo-parameters outside eqtb (\spacefactor, \interactionmode,
        // \prevgraf, ...) are neither reassignments nor traced.
        let slot = p.in_eqtb().then_some(TraceSlot::IntParam(p.idx()));
        if let Some(slot) = slot {
            if !self.begin_assign(global, self.int_params[i] == v, slot) {
                return;
            }
        }
        Self::slot(
            &mut self.int_params,
            &mut self.int_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::IntParam(p.idx(), old, ol),
        );
        if let Some(slot) = slot {
            self.end_assign(slot);
        }
    }
    pub fn assign_dim_param(&mut self, p: DimParam, v: i32, global: bool) {
        let i = p.idx() as usize;
        let slot = p.in_eqtb().then_some(TraceSlot::DimParam(p.idx()));
        if let Some(slot) = slot {
            if !self.begin_assign(global, self.dim_params[i] == v, slot) {
                return;
            }
        }
        Self::slot(
            &mut self.dim_params,
            &mut self.dim_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::DimParam(p.idx(), old, ol),
        );
        if let Some(slot) = slot {
            self.end_assign(slot);
        }
    }
    pub fn assign_glue_param(&mut self, p: GlueParam, v: Glue, global: bool) {
        let i = p.idx() as usize;
        let slot = TraceSlot::GlueParam(p.idx());
        if !self.begin_assign(global, same_glue(&self.glue_params[i], &v), slot) {
            return;
        }
        Self::slot(
            &mut self.glue_params,
            &mut self.glue_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::GlueParam(p.idx(), old, ol),
        );
        self.end_assign(slot);
    }
    pub fn assign_toks_param(&mut self, p: ToksParam, v: Rc<Vec<Token>>, global: bool) {
        let i = p.idx() as usize;
        let slot = TraceSlot::ToksParam(p.idx());
        if !self.begin_assign(global, same_toks(&self.tok_params[i], &v), slot) {
            return;
        }
        Self::slot(
            &mut self.tok_params,
            &mut self.tok_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::ToksParam(p.idx(), old, ol),
        );
        self.end_assign(slot);
    }
    pub fn assign_count(&mut self, idx: u16, v: i32, global: bool) {
        let i = idx as usize;
        if !self.begin_assign(global, self.count[i] == v, TraceSlot::Count(idx)) {
            return;
        }
        Self::slot(
            &mut self.count,
            &mut self.count_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::Count(idx, old, ol),
        );
        self.end_assign(TraceSlot::Count(idx));
    }
    pub fn assign_dimen(&mut self, idx: u16, v: i32, global: bool) {
        let i = idx as usize;
        if !self.begin_assign(global, self.dimen[i] == v, TraceSlot::Dimen(idx)) {
            return;
        }
        Self::slot(
            &mut self.dimen,
            &mut self.dimen_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::Dimen(idx, old, ol),
        );
        self.end_assign(TraceSlot::Dimen(idx));
    }
    pub fn assign_skip(&mut self, idx: u16, v: Glue, global: bool) {
        let i = idx as usize;
        if !self.begin_assign(global, same_glue(&self.skip[i], &v), TraceSlot::Skip(idx)) {
            return;
        }
        Self::slot(
            &mut self.skip,
            &mut self.skip_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::Skip(idx, old, ol),
        );
        self.end_assign(TraceSlot::Skip(idx));
    }
    pub fn assign_muskip(&mut self, idx: u16, v: Glue, global: bool) {
        let i = idx as usize;
        if !self.begin_assign(global, same_glue(&self.muskip[i], &v), TraceSlot::MuSkip(idx)) {
            return;
        }
        Self::slot(
            &mut self.muskip,
            &mut self.muskip_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::MuSkip(idx, old, ol),
        );
        self.end_assign(TraceSlot::MuSkip(idx));
    }
    pub fn assign_toks_reg(&mut self, idx: u16, v: Rc<Vec<Token>>, global: bool) {
        let i = idx as usize;
        if !self.begin_assign(global, same_toks(&self.toks[i], &v), TraceSlot::Toks(idx)) {
            return;
        }
        Self::slot(
            &mut self.toks,
            &mut self.toks_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::Toks(idx, old, ol),
        );
        self.end_assign(TraceSlot::Toks(idx));
    }
    pub fn assign_box(&mut self, idx: u16, v: Option<Node>, global: bool) {
        let i = idx as usize;
        // a box is a fresh node list, so only void replaces void unchanged
        let same = self.boxed[i].is_none() && v.is_none();
        if !self.begin_assign(global, same, TraceSlot::Box(idx)) {
            return;
        }
        Self::slot(
            &mut self.boxed,
            &mut self.box_levels,
            i,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::Box(idx, old, ol),
        );
        self.end_assign(TraceSlot::Box(idx));
    }
    /// Replace a box register's value without changing its assignment level.
    /// TeX uses this for consuming boxes and for storing a \vsplit remainder.
    pub(crate) fn replace_box_value(&mut self, idx: u16, value: Option<Node>) -> Option<Node> {
        std::mem::replace(&mut self.boxed[idx as usize], value)
    }
    pub fn take_box(&mut self, idx: u16) -> Option<Node> {
        // tex.web's box(n):=null changes only the value. Retaining the
        // assignment level lets group unwinding restore an outer box after a
        // locally assigned inner box is consumed.
        self.replace_box_value(idx, None)
    }
    /// tex.web's `box(n):=p` for boxes the page builder stores directly
    /// (insertions, `\box255`): a global store that is not an `eq_define`,
    /// so `\tracingassigns` does not report it.
    pub(crate) fn set_box_untraced(&mut self, idx: u16, v: Option<Node>) {
        self.boxed[idx as usize] = v;
        self.box_levels[idx as usize] = LEVEL_ONE;
    }
    pub fn assign_cat(&mut self, c: u8, v: u8, global: bool) {
        let slot = TraceSlot::Cat(c.into());
        if !self.begin_assign(global, self.cat[c as usize] == v, slot) {
            return;
        }
        Self::slot(
            &mut self.cat,
            &mut self.cat_levels,
            c as usize,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::Cat(c, old, ol),
        );
        self.end_assign(slot);
    }
    pub fn assign_math_code(&mut self, c: u8, v: u16, global: bool) {
        let slot = TraceSlot::MathCode(c.into());
        if !self.begin_assign(global, self.math_code[c as usize] == v, slot) {
            return;
        }
        Self::slot(
            &mut self.math_code,
            &mut self.math_levels,
            c as usize,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::MathCode(c, old, ol),
        );
        self.end_assign(slot);
    }
    pub fn assign_del_code(&mut self, c: u8, v: i32, global: bool) {
        let slot = TraceSlot::DelCode(c.into());
        if !self.begin_assign(global, self.del_code[c as usize] == v, slot) {
            return;
        }
        Self::slot(
            &mut self.del_code,
            &mut self.del_levels,
            c as usize,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::DelCode(c, old, ol),
        );
        self.end_assign(slot);
    }
    pub fn assign_lc_code(&mut self, c: u8, v: u8, global: bool) {
        let slot = TraceSlot::LcCode(c.into());
        if !self.begin_assign(global, self.lc_code[c as usize] == v, slot) {
            return;
        }
        Self::slot(
            &mut self.lc_code,
            &mut self.lc_levels,
            c as usize,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::LcCode(c, old, ol),
        );
        self.end_assign(slot);
    }
    pub fn assign_sf_code(&mut self, c: u8, v: u16, global: bool) {
        let slot = TraceSlot::SfCode(c.into());
        if !self.begin_assign(global, self.sf_code[c as usize] == v, slot) {
            return;
        }
        Self::slot(
            &mut self.sf_code,
            &mut self.sf_levels,
            c as usize,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::SfCode(c, old, ol),
        );
        self.end_assign(slot);
    }
    pub fn assign_uc_code(&mut self, c: u8, v: u8, global: bool) {
        let slot = TraceSlot::UcCode(c.into());
        if !self.begin_assign(global, self.uc_code[c as usize] == v, slot) {
            return;
        }
        Self::slot(
            &mut self.uc_code,
            &mut self.uc_levels,
            c as usize,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::UcCode(c, old, ol),
        );
        self.end_assign(slot);
    }

    pub fn cat_code(&self, character: u32) -> u8 {
        self.unicode_cat_codes
            .get(&character)
            .map(|&(value, _)| value)
            .or_else(|| self.cat.get(character as usize).copied())
            .unwrap_or(CAT_OTHER)
    }

    pub fn assign_cat_code(&mut self, character: u32, value: u8, global: bool) {
        if let Ok(character) = u8::try_from(character) {
            self.assign_cat(character, value, global);
            return;
        }
        let slot = TraceSlot::Cat(character);
        if !self.begin_assign(global, self.cat_code(character) == value, slot) {
            return;
        }
        Self::sparse_slot(
            &mut self.unicode_cat_codes,
            character,
            value,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old| SaveItem::UnicodeCat(character, old),
        );
        self.end_assign(slot);
    }

    pub fn math_code_for(&self, character: u32) -> u32 {
        self.unicode_math_codes
            .get(&character)
            .map(|&(value, _)| value)
            .or_else(|| {
                self.math_code
                    .get(character as usize)
                    .copied()
                    .map(u32::from)
            })
            .unwrap_or(0)
    }

    pub fn assign_math_code_for(&mut self, character: u32, value: u32, global: bool) {
        if let (Ok(character), Ok(value)) = (u8::try_from(character), u16::try_from(value)) {
            if !self.unicode_math_codes.contains_key(&u32::from(character)) {
                self.assign_math_code(character, value, global);
                return;
            }
        }
        let slot = TraceSlot::MathCode(character);
        if !self.begin_assign(global, self.math_code_for(character) == value, slot) {
            return;
        }
        Self::sparse_slot(
            &mut self.unicode_math_codes,
            character,
            value,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old| SaveItem::UnicodeMath(character, old),
        );
        self.end_assign(slot);
    }

    pub fn delimiter_code_for(&self, character: u32) -> i64 {
        self.unicode_del_codes
            .get(&character)
            .map(|&(value, _)| value)
            .or_else(|| {
                self.del_code
                    .get(character as usize)
                    .copied()
                    .map(i64::from)
            })
            .unwrap_or(-1)
    }

    pub fn assign_delimiter_code_for(&mut self, character: u32, value: i64, global: bool) {
        if let (Ok(character), Ok(value)) = (u8::try_from(character), i32::try_from(value)) {
            if !self.unicode_del_codes.contains_key(&u32::from(character)) {
                self.assign_del_code(character, value, global);
                return;
            }
        }
        let slot = TraceSlot::DelCode(character);
        if !self.begin_assign(global, self.delimiter_code_for(character) == value, slot) {
            return;
        }
        Self::sparse_slot(
            &mut self.unicode_del_codes,
            character,
            value,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old| SaveItem::UnicodeDel(character, old),
        );
        self.end_assign(slot);
    }

    pub fn space_factor_code(&self, character: u32) -> u16 {
        self.unicode_sf_codes
            .get(&character)
            .map(|&(value, _)| value)
            .or_else(|| self.sf_code.get(character as usize).copied())
            .unwrap_or(1000)
    }

    pub fn assign_space_factor_code(&mut self, character: u32, value: u16, global: bool) {
        if let Ok(character) = u8::try_from(character) {
            self.assign_sf_code(character, value, global);
            return;
        }
        let slot = TraceSlot::SfCode(character);
        if !self.begin_assign(global, self.space_factor_code(character) == value, slot) {
            return;
        }
        Self::sparse_slot(
            &mut self.unicode_sf_codes,
            character,
            value,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old| SaveItem::UnicodeSf(character, old),
        );
        self.end_assign(slot);
    }

    pub fn case_code(&self, character: u32, uppercase: bool) -> u32 {
        if !self.unicode_case_codes.is_empty() {
            if let Some(&(value, _)) = self.unicode_case_codes.get(&(uppercase, character)) {
                return value;
            }
        }
        let table = if uppercase {
            &self.uc_code
        } else {
            &self.lc_code
        };
        table.get(character as usize).copied().unwrap_or(0) as u32
    }

    pub fn assign_case_code(&mut self, character: u32, value: u32, uppercase: bool, global: bool) {
        if character < 256 {
            let byte = u8::try_from(value).unwrap_or(0);
            if uppercase {
                self.assign_uc_code(character as u8, byte, global);
            } else {
                self.assign_lc_code(character as u8, byte, global);
            }
        }
        let key = (uppercase, character);
        let old = self.unicode_case_codes.get(&key).copied();
        if character < 256 && value < 256 && old.is_none() {
            return;
        }
        if !global
            && self.cur_level > LEVEL_ONE
            && old.map(|(_, level)| level) != Some(self.cur_level)
        {
            self.push_save(SaveItem::UnicodeCase(uppercase, character, old));
        }
        self.unicode_case_codes.insert(
            key,
            (value, if global { LEVEL_ONE } else { self.cur_level }),
        );
    }
    /// tex.web set_font: `define(cur_font_loc, data, cur_chr)` — a font
    /// selection is a group-scoped assignment; \globaldefs>0 forces it
    /// global, <0 forces it local ("Adjust for the setting of \globaldefs").
    pub fn define_cur_font(&mut self, f: u16, global: bool) {
        let gd = self.int_params[crate::prim::IntParam::GlobalDefs.idx() as usize];
        let mut global = global;
        if gd != 0 {
            if gd < 0 {
                global = false;
            } else {
                global = true;
            }
        }
        if !self.begin_assign(global, self.cur_font_val == f, TraceSlot::CurFont) {
            return;
        }
        if !global && self.cur_font_level < self.cur_level {
            // tex.web cur_font_loc: the save happens only when the entry is
            // group-scoped — a top-level selection must not leave a pending
            // save item (it would block \dump forever).
            self.push_save(SaveItem::CurFont(self.cur_font_val, self.cur_font_level));
        }
        self.cur_font_val = f;
        self.cur_font_level = if global { LEVEL_ONE } else { self.cur_level };
        self.end_assign(TraceSlot::CurFont);
    }

    pub fn assign_style_font(&mut self, style: u8, fam: u16, fid: u16, global: bool) {
        let (s, fm) = (style as usize, fam as usize);
        let slot = TraceSlot::StyleFont(style, fam);
        if !self.begin_assign(global, self.style_fonts[s][fm] == fid, slot) {
            return;
        }
        let old = self.style_fonts[s][fm];
        let ol = self.style_font_levels[s][fm];
        if !global && ol < self.cur_level {
            self.push_save(SaveItem::StyleFont(style, fam, old, ol));
        }
        self.style_fonts[s][fm] = fid;
        self.style_font_levels[s][fm] = if global { LEVEL_ONE } else { self.cur_level };
        self.end_assign(slot);
    }
    pub fn assign_font_param(&mut self, font: u16, idx: usize, v: i32, global: bool) {
        if self.font_params[font as usize].len() <= idx {
            self.font_params[font as usize].resize(idx + 1, 0);
            self.font_param_levels[font as usize].resize(idx + 1, LEVEL_ONE);
        }
        if !global && self.font_param_levels[font as usize][idx] < self.cur_level {
            let old = self.font_params[font as usize][idx];
            let ol = self.font_param_levels[font as usize][idx];
            self.push_save(SaveItem::FontParam(font, idx, old, ol));
        }
        self.font_params[font as usize][idx] = v;
        self.font_param_levels[font as usize][idx] =
            if global { LEVEL_ONE } else { self.cur_level };
    }
    pub fn assign_hyphen_char(&mut self, font: u16, v: i32, global: bool) {
        Self::slot(
            &mut self.hyphen_char,
            &mut self.hyphen_char_levels,
            font as usize,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::HyphenChar(font, old, ol),
        );
    }
    pub fn assign_skew_char(&mut self, font: u16, v: i32, global: bool) {
        Self::slot(
            &mut self.skew_char,
            &mut self.skew_char_levels,
            font as usize,
            v,
            global,
            self.cur_level,
            &mut self.save_stack,
            |old, ol| SaveItem::SkewChar(font, old, ol),
        );
    }

    // ---------- groups ----------

    pub fn push_level(&mut self, ty: LevelType) {
        self.push_level_with(ty, GroupMeta::of(ty), 0);
    }

    /// tex.web new_save_level with e-TeX's line number and group
    /// information; `line` is the current input line.
    pub fn push_level_with(&mut self, ty: LevelType, meta: GroupMeta, line: i32) {
        let Some(next_level) = self.cur_level.checked_add(1) else {
            self.group_level_capacity_exceeded = true;
            return;
        };
        if self.int_params[IntParam::TracingGroups as usize] > 0 {
            self.trace_group(false, meta.code, self.cur_level, line);
        }
        self.groups.push(GroupRec {
            meta,
            line,
            boundary: self.save_stack.len(),
        });
        self.cur_level = next_level;

        self.push_save(SaveItem::Level(self.cur_level, ty));
    }
    pub fn cur_group_type(&self) -> Option<LevelType> {
        for item in self.save_stack.iter().rev() {
            if let SaveItem::Level(_, t) = item {
                return Some(*t);
            }
        }
        None
    }

    /// tex.web `cur_group`.
    pub(crate) fn cur_group_code(&self) -> u8 {
        self.groups.last().map_or(group_code::BOTTOM, |g| g.meta.code)
    }

    /// tex.web `cur_boundary` of the innermost group; 0 is the bottom level.
    pub(crate) fn cur_boundary(&self) -> usize {
        self.groups.last().map_or(0, |g| g.boundary + 1)
    }

    /// The boundary of the group enclosing the innermost one
    /// (`save_index(cur_boundary)`).
    pub(crate) fn outer_boundary(&self) -> usize {
        match self.groups.len() {
            0 | 1 => 0,
            n => self.groups[n - 2].boundary + 1,
        }
    }

    pub fn pop_level(&mut self, after_group: &mut Vec<Token>) -> LevelType {
        self.pop_level_full(after_group, &mut None, &mut Vec::new())
    }

    /// Queue `{restoring|retaining ...}` for an engine-side shape (`\parshape`
    /// or an e-TeX penalty array); the engine decides which verb applies and
    /// supplies the value.
    #[inline]
    fn trace_shape(&mut self, slot: TraceSlot) {
        if self.tracing_restores() {
            self.trace_with("", slot, TraceValue::Shape(0, 0, 0));
        }
    }

    /// Pop one group and return restorations for state stored on `Engine`.
    pub fn pop_level_full(
        &mut self,
        after_group: &mut Vec<Token>,
        par_shape_sink: &mut Option<(Vec<(i32, i32)>, u16)>,
        penalty_shape_sink: &mut Vec<(u8, Rc<[i32]>, u16)>,
    ) -> LevelType {
        let mut ty = LevelType::Group;

        while let Some(item) = self.save_stack.pop() {
            match item {
                SaveItem::AfterGroup(tok) => {
                    after_group.push(tok);
                }
                SaveItem::CurFont(old, ol) => {
                    let restored = self.cur_font_level > LEVEL_ONE;
                    if restored {
                        self.cur_font_val = old;
                        self.cur_font_level = ol;
                    }
                    self.trace_restore(restored, TraceSlot::CurFont);
                }
                SaveItem::ParShape(old, lvl) => {
                    // engine decides whether to restore (it tracks the
                    // level field; a later global assign suppresses it,
                    // same as the param arms' `> LEVEL_ONE` check)
                    *par_shape_sink = Some((old, lvl));
                    self.trace_shape(TraceSlot::ParShape);
                }
                SaveItem::PenaltyShape(kind, old, level) => {
                    penalty_shape_sink.push((kind, old, level));
                    self.trace_shape(TraceSlot::PenaltyShape(kind));
                }
                SaveItem::Level(lvl, t) => {
                    self.cur_level = lvl - 1;
                    ty = t;
                    if let Some(group) = self.groups.pop() {
                        if self.int_params[IntParam::TracingGroups as usize] > 0 {
                            self.trace_group(true, group.meta.code, lvl - 1, group.line);
                        }
                    }
                    break;
                }
                SaveItem::Eq(id, old, ol) => {
                    let e = self.ensure_entry(id);
                    let restored = e.level > LEVEL_ONE;
                    if restored {
                        e.equiv = old;
                        e.level = ol;
                    }
                    self.trace_restore(restored, TraceSlot::Eq(id));
                }
                SaveItem::IntParam(i, v, l) => {
                    let restored = self.int_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.int_params[i as usize] = v;
                        self.int_levels[i as usize] = l;
                        if i == IntParam::InteractionMode.idx() {
                            self.pending_interaction_mode = Some(v);
                        }
                    }
                    if IntParam::from_idx(i).is_some_and(IntParam::in_eqtb) {
                        self.trace_restore(restored, TraceSlot::IntParam(i));
                    }
                }
                SaveItem::DimParam(i, v, l) => {
                    let restored = self.dim_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.dim_params[i as usize] = v;
                        self.dim_levels[i as usize] = l;
                    }
                    if DimParam::from_idx(i).is_some_and(DimParam::in_eqtb) {
                        self.trace_restore(restored, TraceSlot::DimParam(i));
                    }
                }
                SaveItem::GlueParam(i, v, l) => {
                    let restored = self.glue_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.glue_params[i as usize] = v;
                        self.glue_levels[i as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::GlueParam(i));
                }
                SaveItem::ToksParam(i, v, l) => {
                    let restored = self.tok_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.tok_params[i as usize] = v;
                        self.tok_levels[i as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::ToksParam(i));
                }
                SaveItem::Count(i, v, l) => {
                    let restored = self.count_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.count[i as usize] = v;
                        self.count_levels[i as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::Count(i));
                }
                SaveItem::Dimen(i, v, l) => {
                    let restored = self.dimen_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.dimen[i as usize] = v;
                        self.dimen_levels[i as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::Dimen(i));
                }
                SaveItem::Skip(i, v, l) => {
                    let restored = self.skip_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.skip[i as usize] = v;
                        self.skip_levels[i as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::Skip(i));
                }
                SaveItem::MuSkip(i, v, l) => {
                    let restored = self.muskip_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.muskip[i as usize] = v;
                        self.muskip_levels[i as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::MuSkip(i));
                }
                SaveItem::Toks(i, v, l) => {
                    let restored = self.toks_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.toks[i as usize] = v;
                        self.toks_levels[i as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::Toks(i));
                }
                SaveItem::Box(i, v, l) => {
                    // tex.web §6076-6092 ("unless eqtb[p] holds a global
                    // value"): a box register left at level_one by
                    // \global\setbox survives the group; restoring
                    // unconditionally clobbered it with the pre-group
                    // value, voiding microtype's \MT@tempbox lastbox.
                    let restored = self.box_levels[i as usize] > LEVEL_ONE;
                    if restored {
                        self.boxed[i as usize] = v;
                        self.box_levels[i as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::Box(i));
                }
                SaveItem::Cat(c, v, l) => {
                    let restored = self.cat_levels[c as usize] > LEVEL_ONE;
                    if restored {
                        self.cat[c as usize] = v;
                        self.cat_levels[c as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::Cat(c.into()));
                }
                SaveItem::MathCode(c, v, l) => {
                    let restored = self.math_levels[c as usize] > LEVEL_ONE;
                    if restored {
                        self.math_code[c as usize] = v;
                        self.math_levels[c as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::MathCode(c.into()));
                }
                SaveItem::DelCode(c, v, l) => {
                    let restored = self.del_levels[c as usize] > LEVEL_ONE;
                    if restored {
                        self.del_code[c as usize] = v;
                        self.del_levels[c as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::DelCode(c.into()));
                }
                SaveItem::LcCode(c, v, l) => {
                    let restored = self.lc_levels[c as usize] > LEVEL_ONE;
                    if restored {
                        self.lc_code[c as usize] = v;
                        self.lc_levels[c as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::LcCode(c.into()));
                }
                SaveItem::SfCode(c, v, l) => {
                    let restored = self.sf_levels[c as usize] > LEVEL_ONE;
                    if restored {
                        self.sf_code[c as usize] = v;
                        self.sf_levels[c as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::SfCode(c.into()));
                }
                SaveItem::UcCode(c, v, l) => {
                    let restored = self.uc_levels[c as usize] > LEVEL_ONE;
                    if restored {
                        self.uc_code[c as usize] = v;
                        self.uc_levels[c as usize] = l;
                    }
                    self.trace_restore(restored, TraceSlot::UcCode(c.into()));
                }
                SaveItem::UnicodeCase(uppercase, character, old) => {
                    let key = (uppercase, character);
                    let restored = self
                        .unicode_case_codes
                        .get(&key)
                        .is_some_and(|&(_, level)| level > LEVEL_ONE);
                    if restored {
                        if let Some(value) = old {
                            self.unicode_case_codes.insert(key, value);
                        } else {
                            self.unicode_case_codes.remove(&key);
                        }
                    }
                    self.trace_restore(
                        restored,
                        if uppercase {
                            TraceSlot::UcCode(character)
                        } else {
                            TraceSlot::LcCode(character)
                        },
                    );
                }
                SaveItem::UnicodeCat(character, old) => {
                    let restored = Self::sparse_is_local(&self.unicode_cat_codes, character);
                    Self::restore_sparse(&mut self.unicode_cat_codes, character, old);
                    self.trace_restore(restored, TraceSlot::Cat(character));
                }
                SaveItem::UnicodeMath(character, old) => {
                    let restored = Self::sparse_is_local(&self.unicode_math_codes, character);
                    Self::restore_sparse(&mut self.unicode_math_codes, character, old);
                    self.trace_restore(restored, TraceSlot::MathCode(character));
                }
                SaveItem::UnicodeDel(character, old) => {
                    let restored = Self::sparse_is_local(&self.unicode_del_codes, character);
                    Self::restore_sparse(&mut self.unicode_del_codes, character, old);
                    self.trace_restore(restored, TraceSlot::DelCode(character));
                }
                SaveItem::UnicodeSf(character, old) => {
                    let restored = Self::sparse_is_local(&self.unicode_sf_codes, character);
                    Self::restore_sparse(&mut self.unicode_sf_codes, character, old);
                    self.trace_restore(restored, TraceSlot::SfCode(character));
                }
                SaveItem::StyleFont(style, fam, v, l) => {
                    let (s, f) = (style as usize, fam as usize);
                    let restored = self.style_font_levels[s][f] > LEVEL_ONE;
                    if restored {
                        self.style_fonts[s][f] = v;
                        self.style_font_levels[s][f] = l;
                    }
                    self.trace_restore(restored, TraceSlot::StyleFont(style, fam));
                }
                SaveItem::FontParam(f, i, v, l) => {
                    self.font_params[f as usize][i] = v;
                    self.font_param_levels[f as usize][i] = l;
                }
                SaveItem::HyphenChar(f, v, l) => {
                    self.hyphen_char[f as usize] = v;
                    self.hyphen_char_levels[f as usize] = l;
                }
                SaveItem::SkewChar(f, v, l) => {
                    self.skew_char[f as usize] = v;
                    self.skew_char_levels[f as usize] = l;
                }
            }
        }
        ty
    }

    // ---------- value getters ----------

    pub fn int_of(&self, e: &Equiv) -> Option<i32> {
        match e {
            Equiv::CountReg(i) => Some(self.count[*i as usize]),
            Equiv::CharDef(v) => Some(*v as i32),
            Equiv::MathCharDef(v) => Some(*v as i32),
            _ => None,
        }
    }

    // ---------- format dump support (crate::format) ----------

    /// All defined control sequences: `(cs, equivalent, save level)`.
    pub(crate) fn eqs(&self) -> Vec<(CsId, Option<&Equiv>, u16)> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.equiv.is_some() || e.level != LEVEL_ONE)
            .map(|(id, e)| (id as CsId, e.equiv.as_ref(), e.level))
            .collect()
    }

    /// Restore one control-sequence entry from a format dump.
    pub(crate) fn restore_eq(&mut self, id: CsId, equiv: Option<Equiv>, level: u16) {
        if matches!(&equiv, Some(Equiv::Macro(m)) if m.outer) {
            self.outer_macros = true;
        }
        let e = self.ensure_entry(id);
        e.equiv = equiv;
        e.level = level;
    }

    /// Clear all control-sequence entries before restoring from a format dump.
    pub(crate) fn clear_entries(&mut self) {
        self.entries.clear();
    }

    /// False while no \outer macro has ever been defined.
    #[inline(always)]
    pub(crate) fn has_outer_macros(&self) -> bool {
        self.outer_macros
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_level_overflow_is_latched_without_wrapping_or_mutating_the_save_stack() {
        let mut eq = Eqtb::new(false);
        eq.cur_level = MAX_GROUP_LEVEL;
        let save_len = eq.save_stack.len();

        eq.push_level(LevelType::Simple);

        assert_eq!(eq.cur_level, MAX_GROUP_LEVEL);
        assert_eq!(eq.save_stack.len(), save_len);
        assert!(eq.group_level_capacity_exceeded());
    }

    #[test]
    fn invalid_interaction_mode_assignment_preserves_the_previous_value() {
        let mut eq = Eqtb::new(false);
        let index = IntParam::InteractionMode.idx() as usize;
        assert_eq!(eq.int_params[index], 3);

        eq.assign_int_param(IntParam::InteractionMode, 7, true);

        assert_eq!(eq.int_params[index], 3);
        assert_eq!(eq.take_pending_interaction_mode(), Some(7));
    }

    #[test]
    fn interaction_mode_assignment_is_global_even_inside_a_group() {
        let mut eq = Eqtb::new(false);
        let index = IntParam::InteractionMode.idx() as usize;
        eq.push_level(LevelType::Simple);

        eq.assign_int_param(IntParam::InteractionMode, 0, false);
        eq.pop_level(&mut Vec::new());

        assert_eq!(eq.int_params[index], 0);
        assert_eq!(eq.int_levels[index], LEVEL_ONE);
        assert_eq!(eq.take_pending_interaction_mode(), Some(0));
    }

    #[test]
    fn save_stack_can_cross_its_logical_limit_without_panicking() {
        let mut eq = Eqtb::new(true);
        eq.save_stack
            .resize(MAX_SAVE_STACK, SaveItem::AfterGroup(Token::space()));

        eq.push_save(SaveItem::AfterGroup(Token::letter(b'x')));

        assert_eq!(eq.save_stack.len(), MAX_SAVE_STACK + 1);
        assert!(eq.save_stack_capacity_exceeded());
    }

    #[test]
    fn replacement_plan_preserves_repeated_parameters_and_rebuilds_after_body_change() {
        fn expand(m: &Macro, args: &crate::input::MacroArgs) -> Vec<Token> {
            let mut frame = crate::input::MacroFrame::new(
                m.body.clone(),
                m.ensure_replacement_plan(),
                args.clone(),
                None,
                0,
            );
            std::iter::from_fn(|| frame.next_token()).collect()
        }
        let mut m = Macro {
            num_params: 2,
            has_param_refs: true,
            params: vec![vec![], vec![]],
            prefix: vec![],
            body: vec![
                Token::letter(b'A'),
                Token(0x4000_0002),
                Token(0x4000_0001),
                Token(0x4000_0002),
                Token(0x4000_0009),
            ]
            .into(),
            long: true,
            outer: false,
            protected: false,
            replacement: Default::default(),
        };
        let mut args = crate::input::MacroArgs::with_buffer(Vec::new());
        args.buffer().push(Token::letter(b'X'));
        args.finish_arg();
        args.buffer()
            .extend([Token::letter(b'Y'), Token::letter(b'Z')]);
        args.finish_arg();
        // A reference without a supplied argument contributes nothing.
        assert_eq!(
            expand(&m, &args),
            vec![
                Token::letter(b'A'),
                Token::letter(b'Y'),
                Token::letter(b'Z'),
                Token::letter(b'X'),
                Token::letter(b'Y'),
                Token::letter(b'Z'),
            ]
        );
        assert_eq!(m.replacement_length(&m.ensure_replacement_plan(), &args, usize::MAX), Some(6));
        Rc::make_mut(&mut m.body)[0] = Token::letter(b'B');
        assert_eq!(expand(&m, &args)[0], Token::letter(b'B'));
        m.body = vec![Token(0x4000_0001)].into();
        assert_eq!(expand(&m, &args), vec![Token::letter(b'X')]);

        m.body = vec![Token(0x4000_0002), Token(0x4000_0002)].into();
        let plan = m.ensure_replacement_plan();
        assert_eq!(m.replacement_length(&plan, &args, 3), None);
        assert_eq!(m.replacement_length(&plan, &args, 4), Some(4));
    }

    fn prim_of(eq: &Eqtb, id: CsId) -> Option<Prim> {
        match eq.get(id) {
            Some(Equiv::Prim(p)) => Some(*p),
            None => None,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn style_font_high_family_local_assignment_restores() {
        // modern LaTeX + newtxmath allocate math families beyond the
        // classic 0..=16; \textfont 200 in a group must restore on \egroup
        let mut eq = Eqtb::new(true);
        eq.assign_style_font(0, 200, 7, true);
        assert_eq!(eq.style_fonts[0][200], 7);
        assert_eq!(eq.style_fonts[0][16], 0);

        eq.push_level(LevelType::Simple);
        eq.assign_style_font(1, 255, 9, false);
        assert_eq!(eq.style_fonts[1][255], 9);
        let mut after = Vec::new();
        eq.pop_level(&mut after);
        assert_eq!(eq.style_fonts[1][255], 0);
        assert_eq!(eq.style_font_levels[1][255], LEVEL_ONE);
        assert!(after.is_empty());
    }

    #[test]
    fn local_assign_prim_pop_restores_previous_prim() {
        let mut eq = Eqtb::new(true);
        let a = 100u32;
        eq.assign(a, Equiv::Prim(Prim::IfNum), true);
        assert_eq!(prim_of(&eq, a), Some(Prim::IfNum));

        eq.push_level(LevelType::Group);
        eq.assign(a, Equiv::Prim(Prim::Relax), false);
        assert_eq!(prim_of(&eq, a), Some(Prim::Relax));

        let mut after = Vec::new();
        eq.pop_level(&mut after);
        assert_eq!(prim_of(&eq, a), Some(Prim::IfNum));
    }

    #[test]
    fn local_undefine_pop_restores_prim() {
        let mut eq = Eqtb::new(true);
        let b = 101u32;
        eq.assign(b, Equiv::Prim(Prim::Def), true);

        eq.push_level(LevelType::Group);
        eq.undefine(b, false);
        assert_eq!(prim_of(&eq, b), None);

        let mut after = Vec::new();
        eq.pop_level(&mut after);
        assert_eq!(prim_of(&eq, b), Some(Prim::Def));
    }

    #[test]
    fn two_local_assigns_same_group_restore_original() {
        let mut eq = Eqtb::new(true);
        let c = 102u32;
        eq.assign(c, Equiv::Prim(Prim::Let), true);

        eq.push_level(LevelType::Group);
        eq.assign(c, Equiv::Prim(Prim::Def), false);
        eq.assign(c, Equiv::Prim(Prim::Relax), false);
        assert_eq!(prim_of(&eq, c), Some(Prim::Relax));

        let mut after = Vec::new();
        eq.pop_level(&mut after);
        assert_eq!(prim_of(&eq, c), Some(Prim::Let));
    }

    #[test]
    fn local_undefine_of_already_undefined_no_panic_no_invention() {
        let mut eq = Eqtb::new(true);
        let d = 103u32;

        eq.push_level(LevelType::Group);
        eq.undefine(d, false);
        assert_eq!(prim_of(&eq, d), None);
        let mut after = Vec::new();
        eq.pop_level(&mut after);
        assert_eq!(prim_of(&eq, d), None);

        eq.push_level(LevelType::Group);
        eq.assign(d, Equiv::Prim(Prim::Relax), false);
        assert_eq!(prim_of(&eq, d), Some(Prim::Relax));
        let mut after2 = Vec::new();
        eq.pop_level(&mut after2);
        assert_eq!(prim_of(&eq, d), None);
    }

    #[test]
    fn global_undefine_is_not_restored_by_pop() {
        let mut eq = Eqtb::new(true);
        let e = 104u32;
        eq.assign(e, Equiv::Prim(Prim::Let), true);

        eq.undefine(e, true);
        assert_eq!(prim_of(&eq, e), None);

        eq.push_level(LevelType::Group);
        eq.assign(e, Equiv::Prim(Prim::Def), false);
        let mut after = Vec::new();
        eq.pop_level(&mut after);
        assert_eq!(prim_of(&eq, e), None);
    }

    #[test]
    fn taking_a_box_preserves_its_assignment_scope() {
        let mut eq = Eqtb::new(true);
        eq.assign_box(255, Some(Node::Penalty(1)), true);
        eq.push_level(LevelType::Simple);
        eq.assign_box(255, Some(Node::Penalty(2)), false);

        assert!(matches!(eq.take_box(255), Some(Node::Penalty(2))));
        let mut after = Vec::new();
        eq.pop_level(&mut after);

        assert!(matches!(eq.boxed[255].as_ref(), Some(Node::Penalty(1))));
        assert_eq!(eq.box_levels[255], LEVEL_ONE);

        eq.push_level(LevelType::Simple);
        assert!(matches!(eq.take_box(255), Some(Node::Penalty(1))));
        eq.pop_level(&mut after);

        assert!(eq.boxed[255].is_none());
        assert_eq!(eq.box_levels[255], LEVEL_ONE);
    }
    #[test]
    fn unicode_code_tables_restore_local_assignments_and_keep_globals() {
        let mut eq = Eqtb::new(true);
        let character = '界' as u32;

        assert_eq!(eq.cat_code(character), CAT_OTHER);
        assert_eq!(eq.math_code_for(character), 0);
        assert_eq!(eq.delimiter_code_for(character), -1);
        assert_eq!(eq.space_factor_code(character), 1000);

        eq.assign_cat_code(character, 11, true);
        eq.assign_math_code_for(character, 0x0123_4567, true);
        eq.assign_delimiter_code_for(character, 0x0123_4567_89ab, true);
        eq.assign_space_factor_code(character, 2000, true);

        eq.push_level(LevelType::Simple);
        eq.assign_cat_code(character, 13, false);
        eq.assign_math_code_for(character, 7, false);
        eq.assign_delimiter_code_for(character, 8, false);
        eq.assign_space_factor_code(character, 900, false);
        assert_eq!(eq.cat_code(character), 13);
        assert_eq!(eq.math_code_for(character), 7);

        let mut after = Vec::new();
        eq.pop_level(&mut after);
        assert_eq!(eq.cat_code(character), 11);
        assert_eq!(eq.math_code_for(character), 0x0123_4567);
        assert_eq!(eq.delimiter_code_for(character), 0x0123_4567_89ab);
        assert_eq!(eq.space_factor_code(character), 2000);

        eq.push_level(LevelType::Simple);
        eq.assign_cat_code(character, 12, false);
        eq.assign_cat_code(character, 10, true);
        eq.pop_level(&mut after);
        assert_eq!(eq.cat_code(character), 10);
    }
}
