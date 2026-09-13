//! Engine implementation of FontResolver plus small dispatch shims and
//! ligature step types.

use crate::engine::Engine;
use crate::fonts::FontResolver;
use crate::prim::Prim;
use crate::tfm::FontId;

pub struct LigKernStep {
    pub is_kern: bool,
    pub kern_amount: i32,
    pub lig_char: u8,
    pub keep_left: bool,
    pub keep_right: bool,
    pub iterate: bool,
}

impl FontResolver for Engine {
    fn char_width(&self, f: FontId, c: u8) -> i32 {
        self.eqtb
            .fonts
            .get(f as usize)
            .map(|f| f.char_width(c))
            .unwrap_or(0)
    }
    fn char_height(&self, f: FontId, c: u8) -> i32 {
        self.eqtb
            .fonts
            .get(f as usize)
            .map(|f| f.char_height(c))
            .unwrap_or(0)
    }
    fn char_depth(&self, f: FontId, c: u8) -> i32 {
        self.eqtb
            .fonts
            .get(f as usize)
            .map(|f| f.char_depth(c))
            .unwrap_or(0)
    }
    fn char_italic(&self, f: FontId, c: u8) -> i32 {
        self.eqtb
            .fonts
            .get(f as usize)
            .map(|f| f.char_italic(c))
            .unwrap_or(0)
    }
}

impl Engine {
    /// public wrapper for expansion dispatch from active chars
    pub fn expand_prim_pub(
        &mut self,
        p: Prim,
        id: crate::token::CsId,
    ) -> Option<crate::token::Token> {
        self.expand_prim(p, id)
    }

    /// dispatch a primitive from non-main-loop contexts
    pub fn dispatch_cs(&mut self, p: Prim, id: crate::token::CsId) {
        if !self.try_assignment(p, id) {
            self.main_dispatch(p, id);
        }
    }
}
