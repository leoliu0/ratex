//! The equivalent table (`eqtb`), register files, code tables, parameter
//! tables, and the save/undo stack implementing TeX's grouping semantics.
//!
//! Levels are 1-based (`level_one = 1`). Assignment with `\global` stores at
//! level 1 and saves nothing. Non-global assignment of a slot whose level is
//! below the current level pushes an undo record `(old value, old level)`;
//! group close (`pop_level`) applies undo records back to the boundary.

use std::collections::HashMap;
use std::rc::Rc;

use crate::boxes::{Glue, Node};
use crate::prim::{DimParam, GlueParam, IntParam, Prim, ToksParam};
use crate::tfm::Font;
use crate::token::{CsId, Token};

pub const LEVEL_ONE: u16 = 1;
pub const NUM_REGISTERS: usize = 512;

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
    /// delimiter token lists per parameter (empty vec = undelimited)
    pub params: Vec<Vec<Token>>,
    /// parameter text before the first # (matched literally, discarded)
    pub prefix: Vec<Token>,
    pub body: Vec<Token>,
    pub long: bool,
    pub outer: bool,
    pub protected: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LevelType {
    Group,
    MacroCall,
    NoLine,
    Balanced,
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
    StyleFont(u8, u8, u16, u16), // (0=textfont,1=scriptfont,2=ssfont, fam, fontid, level)
    FontParam(u16, usize, i32, u16), // font, param index (0-based), old, level
    HyphenChar(u16, i32, u16),
    SkewChar(u16, i32, u16),
}

struct EqEntry {
    equiv: Option<Equiv>,
    level: u16,
}

pub struct Eqtb {
    map: HashMap<CsId, EqEntry>,
    pub save_stack: Vec<SaveItem>,
    pub cur_level: u16,

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

    /// style_fonts[style][fam] -> font id (0 = none); style: 0=text 1=script 2=ss
    pub style_fonts: [[u16; 17]; 3],
    pub style_font_levels: [[u16; 17]; 3],

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
        // tex.web iniTeX default: mathcode(c) = 7*256 + c for visible chars
        for c in 32..256u16 {
            math_code[c as usize] = (7 << 8) | c;
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
        del_code[b'|' as usize] = make_del_code(7, 0x7C, 7, 0x7C);
        let mut lc_code = [0u8; 256];
        let mut sf_code = [1000u16; 256];
        let mut uc_code = [0u8; 256];
        // tex.web §1252 INITEX defaults
        for c in b'a'..=b'z' {
            lc_code[c as usize] = c;
            uc_code[c as usize] = c - 32;
            sf_code[c as usize] = 999;
        }
        for c in b'A'..=b'Z' {
            lc_code[c as usize] = c + 32;
            uc_code[c as usize] = c;
            sf_code[c as usize] = 999;
        }
        Eqtb {
            map: HashMap::new(),
            save_stack: Vec::new(),
            cur_level: LEVEL_ONE,
            int_params: vec![0; crate::prim::NUM_INT_PARAMS],
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
            style_fonts: [[0; 17]; 3],
            style_font_levels: [[LEVEL_ONE; 17]; 3],
            fonts: Vec::new(),
            font_params: Vec::new(),
            font_param_levels: Vec::new(),
            hyphen_char: Vec::new(),
            hyphen_char_levels: Vec::new(),
            skew_char: Vec::new(),
            skew_char_levels: Vec::new(),
            font_cs: Vec::new(),
        }
    }

    // ---------- cs equivalents ----------

    pub fn get(&self, id: CsId) -> Option<&Equiv> {
        self.map.get(&id).and_then(|e| e.equiv.as_ref())
    }

    /// follow \let aliases to the effective meaning
    pub fn resolve(&self, mut id: CsId) -> Option<&Equiv> {
        for _ in 0..1024 {
            match self.get(id) {
                Some(Equiv::Alias(next)) => id = *next,
                other => return other,
            }
        }
        None
    }

    pub fn assign(&mut self, id: CsId, equiv: Equiv, global: bool) {
        let entry = self.map.entry(id).or_insert(EqEntry { equiv: None, level: LEVEL_ONE });
        if !global && entry.level < self.cur_level {
            let old = entry.equiv.take();
            let ol = entry.level;
            self.save_stack.push(SaveItem::Eq(id, old, ol));
        }
        entry.equiv = Some(equiv);
        entry.level = if global { LEVEL_ONE } else { self.cur_level };
    }

    pub fn undefine(&mut self, id: CsId, global: bool) {
        let entry = self.map.entry(id).or_insert(EqEntry { equiv: None, level: LEVEL_ONE });
        if !global && entry.level < self.cur_level && entry.equiv.is_some() {
            let old = entry.equiv.take();
            let ol = entry.level;
            self.save_stack.push(SaveItem::Eq(id, old, ol));
        }
        entry.equiv = None;
        entry.level = if global { LEVEL_ONE } else { self.cur_level };
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
        let i = p.idx() as usize;
        Self::slot(&mut self.int_params, &mut self.int_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::IntParam(p.idx(), old, ol)
        });
    }
    pub fn assign_dim_param(&mut self, p: DimParam, v: i32, global: bool) {
        let i = p.idx() as usize;
        Self::slot(&mut self.dim_params, &mut self.dim_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::DimParam(p.idx(), old, ol)
        });
    }
    pub fn assign_glue_param(&mut self, p: GlueParam, v: Glue, global: bool) {
        let i = p.idx() as usize;
        Self::slot(&mut self.glue_params, &mut self.glue_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::GlueParam(p.idx(), old, ol)
        });
    }
    pub fn assign_toks_param(&mut self, p: ToksParam, v: Rc<Vec<Token>>, global: bool) {
        let i = p.idx() as usize;
        Self::slot(&mut self.tok_params, &mut self.tok_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::ToksParam(p.idx(), old, ol)
        });
    }
    pub fn assign_count(&mut self, idx: u16, v: i32, global: bool) {
        let i = idx as usize;
        Self::slot(&mut self.count, &mut self.count_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::Count(idx, old, ol)
        });
    }
    pub fn assign_dimen(&mut self, idx: u16, v: i32, global: bool) {
        let i = idx as usize;
        Self::slot(&mut self.dimen, &mut self.dimen_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::Dimen(idx, old, ol)
        });
    }
    pub fn assign_skip(&mut self, idx: u16, v: Glue, global: bool) {
        let i = idx as usize;
        Self::slot(&mut self.skip, &mut self.skip_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::Skip(idx, old, ol)
        });
    }
    pub fn assign_muskip(&mut self, idx: u16, v: Glue, global: bool) {
        let i = idx as usize;
        Self::slot(&mut self.muskip, &mut self.muskip_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::MuSkip(idx, old, ol)
        });
    }
    pub fn assign_toks_reg(&mut self, idx: u16, v: Rc<Vec<Token>>, global: bool) {
        let i = idx as usize;
        Self::slot(&mut self.toks, &mut self.toks_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::Toks(idx, old, ol)
        });
    }
    pub fn assign_box(&mut self, idx: u16, v: Option<Node>, global: bool) {
        let i = idx as usize;
        Self::slot(&mut self.boxed, &mut self.box_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::Box(idx, old, ol)
        });
    }
    pub fn assign_cat(&mut self, c: u8, v: u8, global: bool) {
        Self::slot(&mut self.cat, &mut self.cat_levels, c as usize, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::Cat(c, old, ol)
        });
    }
    pub fn assign_math_code(&mut self, c: u8, v: u16, global: bool) {
        Self::slot(&mut self.math_code, &mut self.math_levels, c as usize, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::MathCode(c, old, ol)
        });
    }
    pub fn assign_del_code(&mut self, c: u8, v: i32, global: bool) {
        Self::slot(&mut self.del_code, &mut self.del_levels, c as usize, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::DelCode(c, old, ol)
        });
    }
    pub fn assign_lc_code(&mut self, c: u8, v: u8, global: bool) {
        Self::slot(&mut self.lc_code, &mut self.lc_levels, c as usize, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::LcCode(c, old, ol)
        });
    }
    pub fn assign_sf_code(&mut self, c: u8, v: u16, global: bool) {
        Self::slot(&mut self.sf_code, &mut self.sf_levels, c as usize, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::SfCode(c, old, ol)
        });
    }
    pub fn assign_uc_code(&mut self, c: u8, v: u8, global: bool) {
        Self::slot(&mut self.uc_code, &mut self.uc_levels, c as usize, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::UcCode(c, old, ol)
        });
    }
    pub fn assign_style_font(&mut self, style: u8, fam: u8, fid: u16, global: bool) {
        let old = self.style_fonts[style as usize][fam as usize];
        let ol = self.style_font_levels[style as usize][fam as usize];
        if !global && ol < self.cur_level {
            self.save_stack.push(SaveItem::StyleFont(style, fam, old, ol));
        }
        self.style_fonts[style as usize][fam as usize] = fid;
        self.style_font_levels[style as usize][fam as usize] = if global { LEVEL_ONE } else { self.cur_level };
    }
    pub fn assign_font_param(&mut self, font: u16, idx: usize, v: i32, global: bool) {
        while self.font_params[font as usize].len() <= idx {
            self.font_params[font as usize].push(0);
            self.font_param_levels[font as usize].push(LEVEL_ONE);
        }
        if !global && self.font_param_levels[font as usize][idx] < self.cur_level {
            let old = self.font_params[font as usize][idx];
            let ol = self.font_param_levels[font as usize][idx];
            self.save_stack.push(SaveItem::FontParam(font, idx, old, ol));
        }
        self.font_params[font as usize][idx] = v;
        self.font_param_levels[font as usize][idx] = if global { LEVEL_ONE } else { self.cur_level };
    }
    pub fn assign_hyphen_char(&mut self, font: u16, v: i32, global: bool) {
        Self::slot(&mut self.hyphen_char, &mut self.hyphen_char_levels, font as usize, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::HyphenChar(font, old, ol)
        });
    }
    pub fn assign_skew_char(&mut self, font: u16, v: i32, global: bool) {
        Self::slot(&mut self.skew_char, &mut self.skew_char_levels, font as usize, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::SkewChar(font, old, ol)
        });
    }

    // ---------- groups ----------

    pub fn push_level(&mut self, ty: LevelType) {
        self.cur_level += 1;
        self.save_stack.push(SaveItem::Level(self.cur_level, ty));
    }

    /// close the current level; returns its type
    pub fn pop_level(&mut self) -> LevelType {
        if self.cur_level <= LEVEL_ONE || self.save_stack.is_empty() {
            return LevelType::Group;
        }
        let mut ty = LevelType::Group;
        while let Some(item) = self.save_stack.pop() {
            match item {
                SaveItem::Level(l, t) => {
                    debug_assert_eq!(l, self.cur_level, "unbalanced save stack");
                    ty = t;
                    break;
                }
                SaveItem::Eq(id, old, ol) => {
                    let e = self.map.entry(id).or_insert(EqEntry { equiv: None, level: LEVEL_ONE });
                    e.equiv = old;
                    e.level = ol;
                }
                SaveItem::IntParam(i, v, l) => {
                    self.int_params[i as usize] = v;
                    self.int_levels[i as usize] = l;
                }
                SaveItem::DimParam(i, v, l) => {
                    self.dim_params[i as usize] = v;
                    self.dim_levels[i as usize] = l;
                }
                SaveItem::GlueParam(i, v, l) => {
                    self.glue_params[i as usize] = v;
                    self.glue_levels[i as usize] = l;
                }
                SaveItem::ToksParam(i, v, l) => {
                    self.tok_params[i as usize] = v;
                    self.tok_levels[i as usize] = l;
                }
                SaveItem::Count(i, v, l) => {
                    self.count[i as usize] = v;
                    self.count_levels[i as usize] = l;
                }
                SaveItem::Dimen(i, v, l) => {
                    self.dimen[i as usize] = v;
                    self.dimen_levels[i as usize] = l;
                }
                SaveItem::Skip(i, v, l) => {
                    self.skip[i as usize] = v;
                    self.skip_levels[i as usize] = l;
                }
                SaveItem::MuSkip(i, v, l) => {
                    self.muskip[i as usize] = v;
                    self.muskip_levels[i as usize] = l;
                }
                SaveItem::Toks(i, v, l) => {
                    self.toks[i as usize] = v;
                    self.toks_levels[i as usize] = l;
                }
                SaveItem::Box(i, v, l) => {
                    self.boxed[i as usize] = v;
                    self.box_levels[i as usize] = l;
                }
                SaveItem::Cat(c, v, l) => {
                    self.cat[c as usize] = v;
                    self.cat_levels[c as usize] = l;
                }
                SaveItem::MathCode(c, v, l) => {
                    self.math_code[c as usize] = v;
                    self.math_levels[c as usize] = l;
                }
                SaveItem::DelCode(c, v, l) => {
                    self.del_code[c as usize] = v;
                    self.del_levels[c as usize] = l;
                }
                SaveItem::LcCode(c, v, l) => {
                    self.lc_code[c as usize] = v;
                    self.lc_levels[c as usize] = l;
                }
                SaveItem::SfCode(c, v, l) => {
                    self.sf_code[c as usize] = v;
                    self.sf_levels[c as usize] = l;
                }
                SaveItem::UcCode(c, v, l) => {
                    self.uc_code[c as usize] = v;
                    self.uc_levels[c as usize] = l;
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
        self.cur_level -= 1;
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
}
