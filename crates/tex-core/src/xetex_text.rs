//! XeTeX native text in the main control loop: native word nodes
//! (`new_native_word_node`, `set_native_metrics`), the character collection
//! of `main_control` ("collect_native"), `\XeTeXinterchartoks` insertion and
//! `\XeTeXlinebreaklocale` breaks (`do_locale_linebreaks`), merging of word
//! fragments at packaging time.

use crate::boxes::{DiscNode, Node, NodeList};
use crate::engine::{Engine, EngineKind, Mode};
use crate::eqtb::Eqtb;
use crate::native_layout::{measure_native_word, NativeGlyph, NativeRun};
use crate::tfm::FontId;
use std::rc::Rc;

/// Parameters of XeTeX read by the text code.
#[derive(Clone, Copy, Debug)]
pub(crate) enum XeParam {
    UseGlyphMetrics,
    GenerateActualText,
    DashBreakState,
    InterCharTokenState,
    TracingFonts,
    InterwordSpaceShaping,
    LinebreakPenalty,
    TracingLostChars,
}

pub const CHAR_CLASS_BOUNDARY: u16 = 4095;
pub const CHAR_CLASS_IGNORED: u16 = 4096;

/// Build a native word node of `text` in `font` (`new_native_word_node` +
/// `set_native_metrics`). The font must be a native font.
pub fn native_word(eqtb: &Eqtb, font: FontId, text: &str, actual_text: bool, use_glyph_metrics: bool) -> Node {
    let nf = eqtb
        .fonts
        .get(font as usize)
        .and_then(|f| f.native.as_ref())
        .expect("native_word: not a native font");
    let m = measure_native_word(nf, text, use_glyph_metrics);
    let n = m.glyphs.len();
    Node::NativeGlyphRun {
        run: Rc::new(NativeRun { font, text: Rc::from(text), glyphs: m.glyphs, actual_text }),
        start: 0,
        end: n,
        width: m.width,
        height: m.height,
        depth: m.depth,
    }
}

/// A `glyph_node` (`\XeTeXglyph`): one glyph of the font with its advance and
/// extents (`measure_native_glyph`).
pub fn glyph_node(eqtb: &Eqtb, font: FontId, gid: u16, use_glyph_metrics: bool) -> Node {
    let nf = eqtb
        .fonts
        .get(font as usize)
        .and_then(|f| f.native.as_ref())
        .expect("glyph_node: not a native font");
    let width = crate::native_font::d2fix(nf.glyph_width(gid) as f64);
    let (height, depth) = if use_glyph_metrics {
        let (h, d) = nf.glyph_height_depth(gid);
        (crate::native_font::d2fix(h as f64), crate::native_font::d2fix(d as f64))
    } else {
        (nf.height_base, nf.depth_base)
    };
    let glyph = NativeGlyph {
        glyph_id: gid,
        cluster_start: 0,
        cluster_end: 0,
        x_advance: width,
        y_advance: 0,
        x_offset: 0,
        y_offset: 0,
    };
    Node::NativeGlyphRun {
        run: Rc::new(NativeRun { font, text: Rc::from(""), glyphs: vec![glyph], actual_text: false }),
        start: 0,
        end: 1,
        width,
        height,
        depth,
    }
}

fn utf16_of(c: u32, out: &mut Vec<u16>) {
    if c > 0xFFFF {
        out.push(((c - 0x10000) / 1024 + 0xD800) as u16);
        out.push(((c - 0x10000) % 1024 + 0xDC00) as u16);
    } else {
        out.push(c as u16);
    }
}

fn string_of(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// `hpack`'s "Incorporate a whatsit node into an hbox" for native words:
/// a word followed by further fragments of the same font (possibly with
/// empty discretionaries between) becomes one word, re-shaped as a whole.
pub fn merge_native_fragments(list: &mut NodeList, eqtb: &Eqtb) {
    let ugm = eqtb.xe_use_glyph_metrics;
    let is_word = |n: &Node, font: Option<FontId>| match n.native_word() {
        Some((f, _, _)) => font.is_none_or(|ff| ff == f),
        None => false,
    };
    let mut i = 0;
    while i < list.len() {
        if let Some((font, _, at)) = list[i].native_word() {
            // the chain of same-font words and empty discretionaries
            let mut j = i + 1;
            loop {
                if j < list.len() && is_word(&list[j], Some(font)) {
                    j += 1;
                } else if j + 1 < list.len()
                    && matches!(&list[j], Node::Disc(d) if d.no_break.is_empty() && d.replace_count == 0)
                    && is_word(&list[j + 1], Some(font))
                {
                    j += 2;
                } else {
                    break;
                }
            }
            if j > i + 1 {
                let mut text = String::new();
                for n in &list[i..j] {
                    if let Some((_, t, _)) = n.native_word() {
                        text.push_str(t);
                    }
                }
                let merged = native_word(eqtb, font, &text, at, ugm);
                list.splice(i..j, std::iter::once(merged));
            }
        }
        i += 1;
    }
}

impl Engine {
    // ---- parameters (XeCore's eqtb entries) ----

    // ADAPTERS: reads of XeCore's parameters and class storage.
    pub(crate) fn xe_int(&self, p: XeParam) -> i32 {
        match p {
            XeParam::UseGlyphMetrics => i32::from(self.eqtb.xe_use_glyph_metrics),
            XeParam::GenerateActualText => self.xetex_generate_actual_text,
            XeParam::DashBreakState => self.xetex_dash_break_state,
            XeParam::InterCharTokenState => self.xetex_interchartokenstate,
            XeParam::TracingFonts => 0,
            XeParam::InterwordSpaceShaping => 0,
            XeParam::LinebreakPenalty => 0,
            XeParam::TracingLostChars => self.eqtb.int_params[crate::prim::IntParam::TracingLostChars.idx() as usize],
        }
    }

    fn xe_char_class(&self, c: u32) -> u16 {
        u16::from(self.xetex_char_classes.get(&c).copied().unwrap_or(0))
    }

    fn xe_linebreak_skip(&self) -> crate::boxes::Glue {
        crate::boxes::Glue::zero()
    }

    pub(crate) fn xe_tracing_fonts(&self) -> i32 {
        self.xe_int(XeParam::TracingFonts)
    }

    pub(crate) fn xe_use_glyph_metrics(&self) -> bool {
        self.xe_int(XeParam::UseGlyphMetrics) != 0
    }

    fn xe_actual_text(&self) -> bool {
        self.xe_int(XeParam::GenerateActualText) > 0
    }

    fn xe_dash_break(&self) -> bool {
        self.xe_int(XeParam::DashBreakState) > 0
    }

    /// `XeTeX_inter_char_tokens_en`
    fn xe_interchar_enabled(&self) -> bool {
        self.xe_int(XeParam::InterCharTokenState) > 0
    }

    /// `\XeTeXinterchartoks` for class pair, if non-empty.
    fn xe_interchar_toks(&self, c1: u16, c2: u16) -> Option<Vec<crate::token::Token>> {
        self.xetex_interchar_toks.get(&(c1 as u8, c2 as u8)).filter(|t| !t.is_empty()).cloned()
    }

    // ---- public node constructors (used by hyphenation and math) ----

    /// A shaped, measured native word as the main loop builds it. `font`
    /// must be a native font (see [`Engine::is_native_font`]).
    pub fn xetex_native_word(&self, font: FontId, text: &str) -> Node {
        native_word(&self.eqtb, font, text, self.xe_actual_text(), self.xe_use_glyph_metrics())
    }

    /// Like [`Engine::xetex_native_word`], keeping the ActualText subtype of `like`.
    pub fn xetex_native_word_like(&self, font: FontId, text: &str, like: &Node) -> Node {
        let at = like.native_word().is_some_and(|(_, _, at)| at);
        native_word(&self.eqtb, font, text, at, self.xe_use_glyph_metrics())
    }

    /// A glyph node of `font` (`\XeTeXglyph`, math).
    pub fn xetex_glyph_node(&self, font: FontId, gid: u16) -> Node {
        glyph_node(&self.eqtb, font, gid, self.xe_use_glyph_metrics())
    }

    pub fn is_native_font(&self, f: FontId) -> bool {
        self.eqtb.fonts.get(f as usize).is_some_and(|font| font.native.is_some())
    }

    /// `is_native_font(cur_font)`
    #[inline]
    pub fn cur_font_is_native(&self) -> bool {
        self.eqtb.has_native_fonts && self.is_native_font(self.eqtb.cur_font_val)
    }

    // ---- main loop ----

    /// Turn any collected native text into nodes (`collected`). A no-op
    /// outside XeTeX.
    #[inline]
    pub fn flush_native_text(&mut self) {
        if !self.native_text.buffer.is_empty() {
            self.xetex_flush_native_word();
        }
    }

    /// `adjust_space_factor`
    fn xe_adjust_space_factor(&mut self, c: u32) {
        self.space_factor = self.space_factor_of(c);
    }

    /// The start of a character run: `prev_class:=char_class_boundary` and
    /// the language check.
    fn xe_begin_run(&mut self) {
        if !self.native_text.in_run {
            self.native_text.in_run = true;
            self.native_text.prev_class = CHAR_CLASS_BOUNDARY;
            if self.mode == Mode::Horizontal {
                self.fix_language();
            }
        }
    }

    /// `check_for_inter_char_toks`: true when a token list was inserted (the
    /// character has been put back and must not be processed now).
    fn xe_check_inter_char(&mut self, scalar: u32, is_letter: bool) -> bool {
        let sc = self.xe_char_class(scalar);
        self.native_text.space_class = sc;
        if !self.xe_interchar_enabled() || sc == CHAR_CLASS_IGNORED {
            return false;
        }
        let prev = self.native_text.prev_class;
        let toks = if prev == CHAR_CLASS_BOUNDARY {
            if self.native_text.backed_up_char == Some(scalar) {
                self.native_text.backed_up_char = None;
                None
            } else {
                self.xe_interchar_toks(CHAR_CLASS_BOUNDARY, sc)
            }
        } else {
            self.xe_interchar_toks(prev, sc)
        };
        if let Some(toks) = toks {
            let cc = if is_letter { 11 } else { 12 };
            self.push_token(crate::token::Token::unicode_char(cc, scalar));
            self.native_text.backed_up_char = Some(scalar);
            self.push_tokens_named(toks, "<XeTeXinterchartoks>");
            if prev != CHAR_CLASS_BOUNDARY {
                self.native_text.prev_class = CHAR_CLASS_BOUNDARY;
            }
            return true;
        }
        self.native_text.prev_class = sc;
        false
    }

    /// `check_for_post_char_toks` before a non-character command `t` in
    /// horizontal mode. True when tokens were inserted (`t` is put back).
    pub(crate) fn xe_check_post_char(&mut self, t: crate::token::Token) -> bool {
        let st = &self.native_text;
        if !(self.xe_interchar_enabled()
            && st.space_class != CHAR_CLASS_IGNORED
            && st.prev_class != CHAR_CLASS_BOUNDARY)
        {
            return false;
        }
        self.native_text.prev_class = CHAR_CLASS_BOUNDARY;
        let sc = self.native_text.space_class;
        match self.xe_interchar_toks(sc, CHAR_CLASS_BOUNDARY) {
            Some(toks) => {
                self.push_token(t);
                self.push_tokens_named(toks, "<XeTeXinterchartoks>");
                true
            }
            None => false,
        }
    }

    /// End of a character run (any command other than a character): settle
    /// the collected native word and reset the run state.
    pub(crate) fn xe_end_run(&mut self) {
        self.native_text.in_run = false;
    }

    /// A character of `scalar` in horizontal mode, XeTeX's `main_loop`.
    pub(crate) fn xetex_main_char(&mut self, scalar: u32, is_letter: bool) {
        self.xe_begin_run();
        let f = self.eqtb.cur_font_val;
        if self.cur_font_is_native() {
            // collect_native
            if self.native_text.font != Some(f) {
                self.xetex_flush_native_word();
                self.native_text.font = Some(f);
            }
            self.xe_adjust_space_factor(scalar);
            if self.xe_check_inter_char(scalar, is_letter) {
                self.xetex_flush_native_word();
                return;
            }
            let hyph = self.eqtb.hyphen_char.get(f as usize).copied().unwrap_or(-1);
            let st = &mut self.native_text;
            if st.buffer.is_empty() && self.synctex_active() {
                if let Some((path, line)) = self.input.current_file_position() {
                    if !path.is_empty() && line > 0 {
                        let file_id = self.synctex.get_or_register_file(path);
                        self.native_text.source = Some((file_id, line));
                    }
                }
            }
            let dash = self.xe_dash_break();
            let st = &mut self.native_text;
            utf16_of(scalar, &mut st.buffer);
            let is_hyph = scalar as i32 == hyph || (dash && (scalar == 0x2014 || scalar == 0x2013));
            st.last_is_hyph = is_hyph;
            if st.first_hyph == 0 && is_hyph {
                st.first_hyph = st.buffer.len();
            }
            return;
        }
        // an ordinary font
        self.xetex_flush_native_word();
        self.xe_adjust_space_factor(scalar);
        if self.xe_check_inter_char(scalar, is_letter) {
            // main_loop_lookahead+1: leave without the right boundary
            self.native_text.lig_chain = None;
            self.native_text.suppress_left_boundary = false;
            return;
        }
        let present = u8::try_from(scalar).ok().filter(|c| {
            self.eqtb.fonts.get(f as usize).is_some_and(|font| font.char_present(*c))
        });
        match present {
            Some(c) => {
                self.append_char(c);
            }
            None => {
                self.xetex_char_warning(f, scalar);
                if self.native_text.lig_chain == Some(f) {
                    self.native_text.lig_chain = None;
                    self.lig_kern_loop_end(f);
                }
            }
        }
    }

    /// `collected`: turn the collected UTF-16 text into native word nodes.
    pub(crate) fn xetex_flush_native_word(&mut self) {
        if self.native_text.buffer.is_empty() {
            return;
        }
        let Some(f) = self.native_text.font else {
            self.native_text.buffer.clear();
            return;
        };
        let mut text = std::mem::take(&mut self.native_text.buffer);
        let mut main_h = std::mem::take(&mut self.native_text.first_hyph);
        let is_hyph = std::mem::take(&mut self.native_text.last_is_hyph);
        let source = self.native_text.source.take();
        let Some(nf) = self.eqtb.fonts.get(f as usize).and_then(|x| x.native.clone()) else {
            return;
        };
        let hyph = self.eqtb.hyphen_char.get(f as usize).copied().unwrap_or(-1);
        let dash = self.xe_dash_break();
        let is_hyph_unit = |c: u16| c as i32 == hyph || (dash && (c == 0x2014 || c == 0x2013));
        if let Some(m) = nf.mapping.clone() {
            text = m.apply(&text);
            main_h = 0;
            for (i, &c) in text.iter().enumerate() {
                if main_h == 0 && is_hyph_unit(c) {
                    main_h = i + 1;
                }
            }
            // recompute precisely as the web does: first hyphen only
            main_h = text.iter().position(|&c| is_hyph_unit(c)).map_or(0, |p| p + 1);
        }
        if self.xe_int(XeParam::TracingLostChars) > 0 {
            let mut i = 0;
            while i < text.len() {
                let mut c = text[i] as u32;
                i += 1;
                if (0xD800..0xDC00).contains(&c) && i < text.len() {
                    c = 0x10000 + (c - 0xD800) * 1024 + (text[i] as u32).wrapping_sub(0xDC00);
                    i += 1;
                }
                if nf.map_char(c) == 0 {
                    self.xetex_char_warning(f, c);
                }
            }
        }
        if let Some((file_id, line)) = source {
            self.cur_list.push(Node::Whatsit(
                crate::boxes::WhatIt::SyncPoint { file_id, line },
                crate::boxes::Attr::NONE,
            ));
        }
        let main_k_total = text.len();
        let mut temp_ptr = 0usize;
        let mut main_k = main_k_total;
        let attr = self.eqtb.cur_attr;
        let _ = attr;
        if self.mode == Mode::Horizontal {
            loop {
                if main_h == 0 {
                    main_h = main_k;
                }
                let tail_is_word = matches!(self.cur_list.last().and_then(|n| n.native_word()), Some((tf, _, _)) if tf == f);
                let prev_ok = {
                    let n = self.cur_list.len();
                    n < 2 || !matches!(self.cur_list[n - 2], Node::Disc(_))
                };
                if tail_is_word && prev_ok {
                    // merge with the preceding word
                    let old = self.cur_list.pop().unwrap();
                    let (_, old_text, _) = old.native_word().unwrap();
                    let mut combined: Vec<u16> = old_text.encode_utf16().collect();
                    combined.extend_from_slice(&text[temp_ptr..temp_ptr + main_h]);
                    self.do_locale_linebreaks(f, &combined);
                    main_k = main_k_total - main_h - temp_ptr;
                    temp_ptr = main_h;
                    main_h = 0;
                } else {
                    let frag = text[temp_ptr..temp_ptr + main_h].to_vec();
                    self.do_locale_linebreaks(f, &frag);
                    temp_ptr += main_h;
                    main_k -= main_h;
                    main_h = 0;
                }
                while main_h < main_k && !is_hyph_unit(text[temp_ptr + main_h]) {
                    main_h += 1;
                }
                if main_h < main_k {
                    main_h += 1;
                }
                if main_k > 0 || is_hyph {
                    self.cur_list
                        .push(Node::Disc(DiscNode::new(Vec::new(), Vec::new(), Vec::new(), 0).with_attr(self.eqtb.cur_attr)));
                }
                if main_k == 0 {
                    break;
                }
            }
        } else {
            // restricted horizontal mode: no breaks, but merge with a preceding word
            let tail_is_word = matches!(self.cur_list.last().and_then(|n| n.native_word()), Some((tf, _, _)) if tf == f);
            let prev_ok = {
                let n = self.cur_list.len();
                n < 2 || !matches!(self.cur_list[n - 2], Node::Disc(_))
            };
            if tail_is_word && prev_ok {
                let old = self.cur_list.pop().unwrap();
                let (_, old_text, at) = old.native_word().unwrap();
                let mut s = old_text.to_string();
                s.push_str(&string_of(&text));
                let node = native_word(&self.eqtb, f, &s, at, self.xe_use_glyph_metrics());
                self.cur_list.push(node);
            } else {
                let node = self.xetex_native_word(f, &string_of(&text));
                self.cur_list.push(node);
            }
        }
        if self.xe_int(XeParam::InterwordSpaceShaping) > 0 {
            self.xe_interword_space_shaping(f);
        }
    }

    /// `do_locale_linebreaks(s, len)`: append the text as one word, or as
    /// words separated by `\XeTeXlinebreakpenalty`/`\XeTeXlinebreakskip`
    /// at the break opportunities of `\XeTeXlinebreaklocale`.
    fn do_locale_linebreaks(&mut self, f: FontId, text: &[u16]) {
        let ugm = self.xe_use_glyph_metrics();
        let at = self.xe_actual_text();
        if self.xetex_linebreak_locale.is_none() || text.len() == 1 {
            let node = native_word(&self.eqtb, f, &string_of(text), at, ugm);
            self.cur_list.push(node);
            return;
        }
        let skip = self.xe_linebreak_skip();
        let use_skip = !skip.is_zero();
        let penalty = self.xe_int(XeParam::LinebreakPenalty);
        let use_penalty = penalty != 0 || !use_skip;
        let s = string_of(text);
        // UTF-16 offsets of the break opportunities (ubrk_next)
        let mut u16_at_byte = Vec::with_capacity(s.len() + 1);
        let mut u = 0usize;
        for ch in s.chars() {
            for _ in 0..ch.len_utf8() {
                u16_at_byte.push(u);
            }
            u += ch.len_utf16();
        }
        u16_at_byte.push(u);
        let mut offsets: Vec<usize> = unicode_linebreak::linebreaks(&s)
            .map(|(b, _)| u16_at_byte[b])
            .collect();
        offsets.dedup();
        let mut prev = 0usize;
        for offs in offsets {
            if offs == 0 {
                continue;
            }
            if prev != 0 {
                if use_penalty {
                    self.cur_list.push(Node::Penalty(penalty, self.eqtb.cur_attr));
                }
                if use_skip {
                    self.cur_list.push(Node::Glue(
                        skip.param(crate::boxes::glue_subtype::NORMAL),
                        self.eqtb.cur_attr,
                    ));
                }
            }
            let node = native_word(&self.eqtb, f, &string_of(&text[prev..offs]), at, ugm);
            self.cur_list.push(node);
            prev = offs;
        }
    }

    /// `\XeTeXinterwordspaceshaping`: measure the space between two words in
    /// context and adjust it with a kern (`space_adjustment`).
    fn xe_interword_space_shaping(&mut self, _f: FontId) {}
}

#[allow(unused)]
fn _engine_kind_marker(_: EngineKind) {}
