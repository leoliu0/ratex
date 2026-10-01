//! TeX82 language whatsits (tex.web §1362-§1377): `\setlanguage`, the
//! automatic `fix_language` whatsit when `\language` differs from the
//! paragraph's current language, and the per-paragraph start values that
//! `line_break` hyphenates with.

use crate::boxes::{Node, WhatIt};
use crate::engine::{Engine, Mode};
use crate::prim::IntParam;

/// tex.web `cur_lang`, `l_hyf`, `r_hyf`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LangState {
    pub lang: u8,
    pub lhm: u8,
    pub rhm: u8,
}

/// What new_graf records for one paragraph: its `prev_graf` encoding of the
/// starting language state, and the enclosing paragraph's `clang`.
pub(crate) struct ParLang {
    pub(crate) start: LangState,
    pub(crate) outer_clang: u8,
}

/// tex.web `set_cur_lang` for a `\language` or `\setlanguage` value.
fn lang_number(v: i32) -> u8 {
    if (1..=255).contains(&v) {
        v as u8
    } else {
        0
    }
}

/// tex.web §1091 `norm_min`.
fn norm_min(h: i32) -> u8 {
    h.clamp(1, 63) as u8
}

impl Engine {
    fn hyphen_minima(&self) -> (u8, u8) {
        (
            norm_min(self.eqtb.int_params[IntParam::LeftHyphenMin.idx() as usize]),
            norm_min(self.eqtb.int_params[IntParam::RightHyphenMin.idx() as usize]),
        )
    }

    /// The language state the current parameters describe.
    pub(crate) fn current_language(&self) -> LangState {
        let (lhm, rhm) = self.hyphen_minima();
        LangState {
            lang: lang_number(self.eqtb.int_params[IntParam::Language.idx() as usize]),
            lhm,
            rhm,
        }
    }

    /// tex.web §1091 new_graf / §1200 resume_after_display:
    /// `set_cur_lang; clang:=cur_lang; prev_graf:=...`.
    pub(crate) fn begin_paragraph_language(&mut self) {
        let start = self.current_language();
        self.par_langs.push(ParLang {
            start,
            outer_clang: self.clang,
        });
        self.clang = start.lang;
    }

    /// The paragraph's nest level is popped (line_break or an empty
    /// paragraph); the enclosing paragraph's `clang` is current again.
    pub(crate) fn end_paragraph_language(&mut self) {
        if let Some(p) = self.par_langs.pop() {
            self.clang = p.outer_clang;
        }
    }

    /// tex.web §1376 fix_language, run before a character starts a chain in
    /// unrestricted horizontal mode.
    #[inline]
    pub(crate) fn fix_language(&mut self) {
        let l = lang_number(self.eqtb.int_params[IntParam::Language.idx() as usize]);
        if l != self.clang {
            self.append_language_whatsit(l);
        }
    }

    fn append_language_whatsit(&mut self, lang: u8) {
        let (lhm, rhm) = self.hyphen_minima();
        self.clang = lang;
        self.cur_list
            .push(Node::Whatsit(WhatIt::Language { lang, lhm, rhm }, crate::boxes::Attr::NONE));
    }

    /// tex.web §1377 <Implement \setlanguage>.
    pub(crate) fn set_language(&mut self, id: crate::token::CsId) {
        if !self.mode.is_h() {
            self.report_illegal_case(id);
            return;
        }
        let v = self.scan_int();
        let saved = self.clang;
        self.append_language_whatsit(lang_number(v));
        // clang belongs to the paragraph only in unrestricted horizontal
        // mode; an \hbox's own clang is never read
        if self.mode != Mode::Horizontal {
            self.clang = saved;
        }
    }
}
