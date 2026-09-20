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
use crate::token::{CsId, Token};

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
            let references: Rc<[(usize, usize)]> = self
                .body
                .iter()
                .enumerate()
                .filter_map(|(position, token)| {
                    (0x4000_0001..0x8000_0000)
                        .contains(&token.0)
                        .then_some((position, (token.0 & 0x3FFF_FFFF).wrapping_sub(1) as usize))
                })
                .collect::<Vec<_>>()
                .into();
            *cached = Some(MacroReplacement {
                body: self.body.clone(),
                references: references.clone(),
            });
            references
        } else {
            cached.as_ref().unwrap().references.clone()
        }
    }

    pub(crate) fn replacement_length(
        &self,
        args: &[smallvec::SmallVec<[Token; 16]>],
        limit: usize,
    ) -> Option<usize> {
        let references = self.ensure_replacement_plan();
        let mut length = self.body.len();
        for &(_, parameter) in references.iter() {
            if let Some(arg) = args.get(parameter) {
                let next = length.checked_sub(1)?.checked_add(arg.len())?;
                if next > limit {
                    return None;
                }
                length = next;
            }
        }
        if length > limit {
            None
        } else {
            Some(length)
        }
    }

    pub(crate) fn append_replacement(
        &self,
        args: &[smallvec::SmallVec<[Token; 16]>],
        output: &mut Vec<Token>,
        limit: usize,
    ) -> bool {
        let mut cached = self.replacement.borrow_mut();
        if cached
            .as_ref()
            .is_none_or(|plan| !Rc::ptr_eq(&plan.body, &self.body))
        {
            let references: Rc<[(usize, usize)]> = self
                .body
                .iter()
                .enumerate()
                .filter_map(|(position, token)| {
                    (0x4000_0001..0x8000_0000)
                        .contains(&token.0)
                        .then_some((position, (token.0 & 0x3FFF_FFFF).wrapping_sub(1) as usize))
                })
                .collect::<Vec<_>>()
                .into();
            *cached = Some(MacroReplacement {
                body: self.body.clone(),
                references,
            });
        }
        let plan = cached.as_ref().unwrap();
        let references = plan.references.clone();
        drop(cached);
        let mut length = self.body.len();
        for &(_, parameter) in references.iter() {
            if let Some(arg) = args.get(parameter) {
                let Some(next) = length
                    .checked_sub(1)
                    .and_then(|length| length.checked_add(arg.len()))
                else {
                    return false;
                };
                if next > limit {
                    return false;
                }
                length = next;
            }
        }
        if length > limit {
            return false;
        }
        output.reserve(length);
        let mut start = 0;
        for &(position, parameter) in references.iter() {
            if let Some(arg) = args.get(parameter) {
                output.extend_from_slice(&self.body[start..position]);
                output.extend_from_slice(arg);
                start = position + 1;
            }
        }
        output.extend_from_slice(&self.body[start..]);
        true
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
    StyleFont(u8, u16, u16, u16), // (0=textfont,1=scriptfont,2=ssfont, fam, fontid, level)
    FontParam(u16, usize, i32, u16), // font, param index (0-based), old, level
    HyphenChar(u16, i32, u16),
    SkewChar(u16, i32, u16),
    /// previous current font (tex.web cur_font_loc is an eqtb entry, so a
    /// font selection inside a group is restored at \endgroup)
    CurFont(u16),
    /// engine-side \parshape value before a local assignment/clear
    /// (tex.web level-tracks par_shape_ptr through eq_define)
    ParShape(Vec<(i32, i32)>, u16),
    /// Previous e-TeX penalty-array value before a local assignment.
    PenaltyShape(u8, Rc<[i32]>, u16),
    /// pdfTeX stores \pdfpageattr / \pdfpagesattr / \pdfpageresources as
    /// eqtb token-list variables: a local assignment pushes the previous
    /// tokens + level here and \endgroup rolls it back (otherwise a
    /// landscape \pdfpageattr{/Rotate 90} leaks to every later page).
    /// kind: 0 = pageattr, 1 = pagesattr, 2 = pageresources.
    PdfPageVar(u8, Rc<Vec<Token>>, u16),
    AfterGroup(Token),
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

    /// tex.web cur_font_loc: current font, group-scoped via SaveItem::CurFont
    pub cur_font_val: u16,

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
    /// Unicode overrides, keyed by (uppercase, scalar); the byte tables stay hot.
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
            pending_interaction_mode: None,
            cur_font_val: 0,
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
        if !global && self.entries[idx].level < cur_level {
            let old = self.entries[idx].equiv.clone();
            let ol = self.entries[idx].level;
            self.push_save(SaveItem::Eq(id, old, ol));
        }
        self.entries[idx].equiv = Some(equiv);
        self.entries[idx].level = if global { LEVEL_ONE } else { cur_level };
    }

    /// Replace only the current meaning without recording a TeX assignment.
    /// This is for tightly scoped engine internals such as write expansion;
    /// the caller must restore the returned meaning before normal execution
    /// resumes. The definition level and save stack remain untouched.
    pub(crate) fn replace_equiv_temporarily(
        &mut self,
        id: CsId,
        equiv: Option<Equiv>,
    ) -> Option<Equiv> {
        std::mem::replace(&mut self.ensure_entry(id).equiv, equiv)
    }

    pub fn undefine(&mut self, id: CsId, global: bool) {
        let cur_level = self.cur_level;
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
        if !global && self.entries[idx].level < cur_level {
            let old = self.entries[idx].equiv.clone();
            let ol = self.entries[idx].level;
            self.push_save(SaveItem::Eq(id, old, ol));
        }
        self.entries[idx].equiv = None;
        self.entries[idx].level = if global { LEVEL_ONE } else { cur_level };
    }

    // ---------- generic level-aware slots ----------

    #[inline]
    fn slot<T: Clone>(
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
            let old = vals[idx].clone();
            let ol = levels[idx];
            stack.push(mk(old, ol));
        }
        vals[idx] = v;
        levels[idx] = if global { LEVEL_ONE } else { cur_level };
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
    }
    pub fn assign_dim_param(&mut self, p: DimParam, v: i32, global: bool) {
        let i = p.idx() as usize;
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
    }
    pub fn assign_glue_param(&mut self, p: GlueParam, v: Glue, global: bool) {
        let i = p.idx() as usize;
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
    }
    pub fn assign_toks_param(&mut self, p: ToksParam, v: Rc<Vec<Token>>, global: bool) {
        let i = p.idx() as usize;
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
    }
    pub fn assign_count(&mut self, idx: u16, v: i32, global: bool) {
        let i = idx as usize;
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
    }
    pub fn assign_dimen(&mut self, idx: u16, v: i32, global: bool) {
        let i = idx as usize;
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
    }
    pub fn assign_skip(&mut self, idx: u16, v: Glue, global: bool) {
        let i = idx as usize;
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
    }
    pub fn assign_muskip(&mut self, idx: u16, v: Glue, global: bool) {
        let i = idx as usize;
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
    }
    pub fn assign_toks_reg(&mut self, idx: u16, v: Rc<Vec<Token>>, global: bool) {
        let i = idx as usize;
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
    }
    pub fn assign_box(&mut self, idx: u16, v: Option<Node>, global: bool) {
        let i = idx as usize;
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
    pub fn assign_cat(&mut self, c: u8, v: u8, global: bool) {
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
    }
    pub fn assign_math_code(&mut self, c: u8, v: u16, global: bool) {
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
    }
    pub fn assign_del_code(&mut self, c: u8, v: i32, global: bool) {
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
    }
    pub fn assign_lc_code(&mut self, c: u8, v: u8, global: bool) {
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
    }
    pub fn assign_sf_code(&mut self, c: u8, v: u16, global: bool) {
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
    }
    pub fn assign_uc_code(&mut self, c: u8, v: u8, global: bool) {
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
        if !global && self.cur_level > 1 {
            // tex.web cur_font_loc: the save happens only when the entry is
            // group-scoped — a top-level selection must not leave a pending
            // save item (it would block \dump forever).
            self.push_save(SaveItem::CurFont(self.cur_font_val));
        }
        self.cur_font_val = f;
    }

    pub fn assign_style_font(&mut self, style: u8, fam: u16, fid: u16, global: bool) {
        let old = self.style_fonts[style as usize][fam as usize];
        let ol = self.style_font_levels[style as usize][fam as usize];
        if !global && ol < self.cur_level {
            self.push_save(SaveItem::StyleFont(style, fam, old, ol));
        }
        self.style_fonts[style as usize][fam as usize] = fid;
        self.style_font_levels[style as usize][fam as usize] =
            if global { LEVEL_ONE } else { self.cur_level };
    }
    pub fn assign_font_param(&mut self, font: u16, idx: usize, v: i32, global: bool) {
        while self.font_params[font as usize].len() <= idx {
            self.font_params[font as usize].push(0);
            self.font_param_levels[font as usize].push(LEVEL_ONE);
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
        let Some(next_level) = self.cur_level.checked_add(1) else {
            self.group_level_capacity_exceeded = true;
            return;
        };
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
    pub fn pop_level(&mut self, after_group: &mut Vec<Token>) -> LevelType {
        self.pop_level_full(after_group, &mut None, &mut Vec::new())
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
                SaveItem::CurFont(old) => {
                    self.cur_font_val = old;
                }
                SaveItem::ParShape(old, lvl) => {
                    // engine decides whether to restore (it tracks the
                    // level field; a later global assign suppresses it,
                    // same as the param arms' `> LEVEL_ONE` check)
                    *par_shape_sink = Some((old, lvl));
                }
                SaveItem::PenaltyShape(kind, old, level) => {
                    penalty_shape_sink.push((kind, old, level));
                }
                SaveItem::PdfPageVar(_kind, _old, _lvl) => {
                    // pdfpageattr / pdfpagesattr / pdfpageresources restoration
                }
                SaveItem::Level(lvl, t) => {
                    self.cur_level = lvl - 1;
                    ty = t;
                    break;
                }
                SaveItem::Eq(id, old, ol) => {
                    let e = self.ensure_entry(id);
                    if e.level > LEVEL_ONE {
                        e.equiv = old;
                        e.level = ol;
                    }
                }
                SaveItem::IntParam(i, v, l) => {
                    if self.int_levels[i as usize] > LEVEL_ONE {
                        self.int_params[i as usize] = v;
                        self.int_levels[i as usize] = l;
                        if i == IntParam::InteractionMode.idx() {
                            self.pending_interaction_mode = Some(v);
                        }
                    }
                }
                SaveItem::DimParam(i, v, l) => {
                    if self.dim_levels[i as usize] > LEVEL_ONE {
                        self.dim_params[i as usize] = v;
                        self.dim_levels[i as usize] = l;
                    }
                }
                SaveItem::GlueParam(i, v, l) => {
                    if self.glue_levels[i as usize] > LEVEL_ONE {
                        self.glue_params[i as usize] = v;
                        self.glue_levels[i as usize] = l;
                    }
                }
                SaveItem::ToksParam(i, v, l) => {
                    if self.tok_levels[i as usize] > LEVEL_ONE {
                        self.tok_params[i as usize] = v;
                        self.tok_levels[i as usize] = l;
                    }
                }
                SaveItem::Count(i, v, l) => {
                    if self.count_levels[i as usize] > LEVEL_ONE {
                        self.count[i as usize] = v;
                        self.count_levels[i as usize] = l;
                    }
                }
                SaveItem::Dimen(i, v, l) => {
                    if self.dimen_levels[i as usize] > LEVEL_ONE {
                        self.dimen[i as usize] = v;
                        self.dimen_levels[i as usize] = l;
                    }
                }
                SaveItem::Skip(i, v, l) => {
                    if self.skip_levels[i as usize] > LEVEL_ONE {
                        self.skip[i as usize] = v;
                        self.skip_levels[i as usize] = l;
                    }
                }
                SaveItem::MuSkip(i, v, l) => {
                    if self.muskip_levels[i as usize] > LEVEL_ONE {
                        self.muskip[i as usize] = v;
                        self.muskip_levels[i as usize] = l;
                    }
                }
                SaveItem::Toks(i, v, l) => {
                    if self.toks_levels[i as usize] > LEVEL_ONE {
                        self.toks[i as usize] = v;
                        self.toks_levels[i as usize] = l;
                    }
                }
                SaveItem::Box(i, v, l) => {
                    // tex.web §6076-6092 ("unless eqtb[p] holds a global
                    // value"): a box register left at level_one by
                    // \global\setbox survives the group; restoring
                    // unconditionally clobbered it with the pre-group
                    // value, voiding microtype's \MT@tempbox lastbox.
                    if self.box_levels[i as usize] > LEVEL_ONE {
                        self.boxed[i as usize] = v;
                        self.box_levels[i as usize] = l;
                    }
                }
                SaveItem::Cat(c, v, l) => {
                    if self.cat_levels[c as usize] > LEVEL_ONE {
                        self.cat[c as usize] = v;
                        self.cat_levels[c as usize] = l;
                    }
                }
                SaveItem::MathCode(c, v, l) => {
                    if self.math_levels[c as usize] > LEVEL_ONE {
                        self.math_code[c as usize] = v;
                        self.math_levels[c as usize] = l;
                    }
                }
                SaveItem::DelCode(c, v, l) => {
                    if self.del_levels[c as usize] > LEVEL_ONE {
                        self.del_code[c as usize] = v;
                        self.del_levels[c as usize] = l;
                    }
                }
                SaveItem::LcCode(c, v, l) => {
                    if self.lc_levels[c as usize] > LEVEL_ONE {
                        self.lc_code[c as usize] = v;
                        self.lc_levels[c as usize] = l;
                    }
                }
                SaveItem::SfCode(c, v, l) => {
                    if self.sf_levels[c as usize] > LEVEL_ONE {
                        self.sf_code[c as usize] = v;
                        self.sf_levels[c as usize] = l;
                    }
                }
                SaveItem::UcCode(c, v, l) => {
                    if self.uc_levels[c as usize] > LEVEL_ONE {
                        self.uc_code[c as usize] = v;
                        self.uc_levels[c as usize] = l;
                    }
                }
                SaveItem::UnicodeCase(uppercase, character, old) => {
                    let key = (uppercase, character);
                    if self
                        .unicode_case_codes
                        .get(&key)
                        .is_some_and(|&(_, level)| level > LEVEL_ONE)
                    {
                        if let Some(value) = old {
                            self.unicode_case_codes.insert(key, value);
                        } else {
                            self.unicode_case_codes.remove(&key);
                        }
                    }
                }
                SaveItem::StyleFont(style, fam, v, l) => {
                    self.style_fonts[style as usize][fam as usize] = v;
                    self.style_font_levels[style as usize][fam as usize] = l;
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
        let e = self.ensure_entry(id);
        e.equiv = equiv;
        e.level = level;
    }

    /// Clear all control-sequence entries before restoring from a format dump.
    pub(crate) fn clear_entries(&mut self) {
        self.entries.clear();
    }

    /// Number of defined control sequences.
    pub(crate) fn eq_count(&self) -> usize {
        self.entries.iter().filter(|e| e.equiv.is_some()).count()
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
        let args = [
            smallvec::smallvec![Token::letter(b'X')],
            smallvec::smallvec![Token::letter(b'Y'), Token::letter(b'Z')],
        ];
        let mut output = Vec::new();
        assert!(m.append_replacement(&args, &mut output, usize::MAX));
        assert_eq!(
            output,
            vec![
                Token::letter(b'A'),
                Token::letter(b'Y'),
                Token::letter(b'Z'),
                Token::letter(b'X'),
                Token::letter(b'Y'),
                Token::letter(b'Z'),
                Token(0x4000_0009)
            ]
        );
        Rc::make_mut(&mut m.body)[0] = Token::letter(b'B');
        output.clear();
        assert!(m.append_replacement(&args, &mut output, usize::MAX));
        assert_eq!(output[0], Token::letter(b'B'));
        m.body = vec![Token(0x4000_0001)].into();
        output.clear();
        assert!(m.append_replacement(&args, &mut output, usize::MAX));
        assert_eq!(output, vec![Token::letter(b'X')]);

        m.body = vec![Token(0x4000_0002), Token(0x4000_0002)].into();
        output.clear();
        assert!(!m.append_replacement(&args, &mut output, 3));
        assert!(
            output.is_empty(),
            "an oversized replacement is not allocated"
        );
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
}
