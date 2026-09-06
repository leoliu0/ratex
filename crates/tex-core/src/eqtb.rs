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
    Simple,
    SemiSimple,
    Box,
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
        Eqtb {
            entries: Vec::new(),
            save_stack: Vec::new(),
            cur_level: LEVEL_ONE,
            cur_font_val: 0,
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
        }
    }

    // ---------- cs equivalents ----------

    #[inline(always)]
    fn ensure_entry(&mut self, id: CsId) -> &mut EqEntry {
        let idx = id as usize;
        if idx >= self.entries.len() {
            self.entries.resize(idx + 1, EqEntry { equiv: None, level: LEVEL_ONE });
        }
        &mut self.entries[idx]
    }

    #[inline(always)]
    pub fn get(&self, id: CsId) -> Option<&Equiv> {
        self.entries.get(id as usize).and_then(|e| e.equiv.as_ref())
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

    pub fn assign(&mut self, id: CsId, equiv: Equiv, global: bool) {
        let cur_level = self.cur_level;
        let idx = id as usize;
        if idx >= self.entries.len() {
            self.entries.resize(idx + 1, EqEntry { equiv: None, level: LEVEL_ONE });
        }
        if !global && self.entries[idx].level < cur_level {
            let old = self.entries[idx].equiv.clone();
            let ol = self.entries[idx].level;
            self.save_stack.push(SaveItem::Eq(id, old, ol));
        }
        self.entries[idx].equiv = Some(equiv);
        self.entries[idx].level = if global { LEVEL_ONE } else { cur_level };
    }

    pub fn undefine(&mut self, id: CsId, global: bool) {
        let cur_level = self.cur_level;
        let idx = id as usize;
        if idx >= self.entries.len() {
            self.entries.resize(idx + 1, EqEntry { equiv: None, level: LEVEL_ONE });
        }
        if !global && self.entries[idx].level < cur_level {
            let old = self.entries[idx].equiv.clone();
            let ol = self.entries[idx].level;
            self.save_stack.push(SaveItem::Eq(id, old, ol));
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
        if std::env::var_os("OBWATCH").is_some() && idx == 50 {
            let desc = match &v { Some(Node::Box { h, list, .. }) => format!("box h={:.1} n={}", *h as f64 / 65536.0, list.len()), Some(_) => "other".into(), None => "void".into() };
            eprintln!("OBWATCH assign reg50={} global={}", desc, global);
        }
        let i = idx as usize;
        Self::slot(&mut self.boxed, &mut self.box_levels, i, v, global, self.cur_level, &mut self.save_stack, |old, ol| {
            SaveItem::Box(idx, old, ol)
        });
    }
    /// tex.web begin_box/box_code: `cur_box := box(n); box(n) := null;` —
    /// the void is a normal `eq_define(box_ref)`: it pushes the old value on
    /// the save stack and records the void *at the register's current level*
    /// (`levels[idx] = cur_level`, no bump). Rust's plain `.take()` skipped
    /// both: group rollback then resurrected boxes the output routine had
    /// already consumed, which made longtable's `\copy\LT@head` material
    /// vanish when the output group closed (missing "(continued)" heads,
    /// p48/p49).
    pub fn take_box(&mut self, idx: u16) -> Option<Node> {
        let i = idx as usize;
        if self.box_levels[i] < self.cur_level {
            let old = self.boxed[i].clone();
            let ol = self.box_levels[i];
            self.save_stack.push(SaveItem::Box(idx, old, ol));
        }
        std::mem::replace(&mut self.boxed[i], None)
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
            self.save_stack.push(SaveItem::CurFont(self.cur_font_val));
        }
        self.cur_font_val = f;
    }

    pub fn assign_style_font(&mut self, style: u8, fam: u16, fid: u16, global: bool) {
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
        if crate::debug_flag("LVLTRACE") {
            eprintln!("PUSH-LVL {} ty={:?}", self.cur_level, ty);
        }
        self.save_stack.push(SaveItem::Level(self.cur_level, ty));
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
        self.pop_level_full(after_group, &mut None)
    }

    /// pop_level plus a sink for engine-side parshape restorations
    /// (par_shape lives on the Engine, not in eqtb)
    pub fn pop_level_full(
        &mut self,
        after_group: &mut Vec<Token>,
        par_shape_sink: &mut Option<(Vec<(i32, i32)>, u16)>,
    ) -> LevelType {
        let mut ty = LevelType::Group;
        if crate::debug_flag("LVLTRACE") {
            eprintln!("POP-LVL-BEFORE {}", self.cur_level);
        }
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

    /// Number of defined control sequences.
    pub(crate) fn eq_count(&self) -> usize {
        self.entries.iter().filter(|e| e.equiv.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
