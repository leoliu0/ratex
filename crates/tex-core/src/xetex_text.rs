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

pub use crate::eqtb::{CHAR_CLASS_BOUNDARY, CHAR_CLASS_IGNORED};

/// Name of the token list holding the character put back by an
/// `\XeTeXinterchartoks` insertion (xetex.web `backed_up_char`).
const BACKED_UP_CHAR: &str = "<backed_up_char>";

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
    let ugm = eqtb.int_params[crate::prim::IntParam::XeTeXUseGlyphMetrics.idx() as usize] != 0;
    let is_word = |n: &Node, font: Option<FontId>| match n.native_word() {
        Some((f, _, _)) => font.is_none_or(|ff| ff == f),
        None => false,
    };
    let mut i = 0;
    while i < list.len() {
        if let Some((font, _, at)) = list[i].native_word() {
            // the chain of same-font words and empty discretionaries
            let mut j = i + 1;
            // TeXres's SyncTeX marks are not nodes of the TeX Live list:
            // they never end a chain and stay behind the merged word
            let mut syncs: Vec<Node> = Vec::new();
            loop {
                let mut k = j;
                while k < list.len() && matches!(&list[k], Node::Whatsit(crate::boxes::WhatIt::SyncPoint { .. }, _)) {
                    k += 1;
                }
                if k > j && k < list.len() && is_word(&list[k], Some(font)) {
                    syncs.extend(list[j..k].iter().cloned());
                    list.drain(j..k);
                    continue;
                }
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
                let nsync = syncs.len();
                list.splice(i..j, std::iter::once(merged).chain(syncs));
                i += nsync;
            }
        }
        i += 1;
    }
}

/// xetex.web `store_justified_native_glyphs`: re-measure `text` in `nf`'s
/// font and spread the difference to `target` width over the space glyphs
/// (or, with none, over all glyphs).
fn justified_word(eqtb: &Eqtb, font: FontId, text: &str, target: i32, at: bool, ugm: bool) -> Node {
    let nf = eqtb.fonts[font as usize].native.as_ref().expect("native font");
    let m = measure_native_word(nf, text, ugm);
    let n = m.glyphs.len();
    // absolute x of every glyph
    let mut xs: Vec<f64> = Vec::with_capacity(n);
    let mut x = m.glyphs.first().map_or(0, |g| g.x_offset);
    for g in &m.glyphs {
        xs.push(crate::native_font::fix2d(x));
        x += g.x_advance;
    }
    if m.width != target && n > 0 {
        let just = crate::native_font::fix2d(target - m.width);
        let space_gid = nf.map_char(' ' as u32);
        let space_count = m.glyphs.iter().filter(|g| g.glyph_id == space_gid).count();
        if space_count > 0 {
            let mut adjustment = 0.0;
            let mut space_index = 0;
            for (i, g) in m.glyphs.iter().enumerate() {
                xs[i] += adjustment;
                if g.glyph_id == space_gid {
                    space_index += 1;
                    adjustment = just * space_index as f64 / space_count as f64;
                }
            }
        } else {
            for (i, xv) in xs.iter_mut().enumerate().skip(1) {
                *xv += just * i as f64 / (n - 1) as f64;
            }
        }
    }
    let fx: Vec<i32> = xs.iter().map(|&v| crate::native_font::d2fix(v)).collect();
    let glyphs: Vec<NativeGlyph> = m
        .glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let next = fx.get(i + 1).copied().unwrap_or(target);
            NativeGlyph {
                x_advance: next - fx[i],
                x_offset: if i == 0 { fx[0] } else { 0 },
                ..g.clone()
            }
        })
        .collect();
    Node::NativeGlyphRun {
        run: Rc::new(NativeRun { font, text: Rc::from(text), glyphs, actual_text: at }),
        start: 0,
        end: n,
        width: target,
        height: m.height,
        depth: m.depth,
    }
}

/// xetex.web "Merge sequences of words using native fonts and inter-word
/// spaces into single nodes" (`hlist_out`, `\XeTeXinterwordspaceshaping>1`):
/// runs of words of one font joined by the font's normal space are replaced
/// by one justified word whose width is the run's set width. `sign`, `order`
/// and `glue_set` are the glue setting of the enclosing box.
pub fn merge_interword_runs(list: &[Node], eqtb: &Eqtb, sign: u8, order: u8, glue_set: f64) -> NodeList {
    use crate::boxes::WhatIt;
    let ugm = eqtb.int_params[crate::prim::IntParam::XeTeXUseGlyphMetrics.idx() as usize] != 0;
    let invisible = |n: &Node| {
        matches!(
            n,
            Node::Penalty(..)
                | Node::Ins { .. }
                | Node::Mark { .. }
                | Node::Adj(..)
                | Node::VAdjust(..)
                | Node::PreAdjust(..)
                | Node::Whatsit(
                    WhatIt::Write { .. }
                        | WhatIt::OpenOut { .. }
                        | WhatIt::CloseOut { .. }
                        | WhatIt::Special(..)
                        | WhatIt::Language { .. }
                        | WhatIt::SyncPoint { .. },
                    _
                )
        )
    };
    let skip_invisible = |mut q: usize| {
        while q < list.len() && invisible(&list[q]) {
            q += 1;
        }
        q
    };
    let word_of = |i: usize, font: FontId| -> bool {
        i < list.len() && list[i].native_word().is_some_and(|(f, _, _)| f == font)
    };
    let font_space = |f: FontId| -> (i32, i32, i32) {
        let p = eqtb.font_params.get(f as usize);
        let g = |i: usize| p.and_then(|p| p.get(i)).copied().unwrap_or(0);
        (g(1), g(2), g(3))
    };
    let is_font_glue = |n: &Node, f: FontId| match n {
        Node::Glue(g, _) if g.subtype == 0 => {
            let (w, st, sh) = font_space(f);
            g.width == w && g.stretch == st && g.shrink == sh && g.stretch_order == 0 && g.shrink_order == 0
        }
        _ => false,
    };
    let mut out: NodeList = Vec::with_capacity(list.len());
    let mut i = 0;
    while i < list.len() {
        let Some((font, _, at)) = list[i].native_word() else {
            out.push(list[i].clone());
            i += 1;
            continue;
        };
        let letter_space = eqtb.fonts[font as usize].native.as_ref().map_or(0, |n| n.letter_space);
        if i + 1 >= list.len() || letter_space != 0 {
            out.push(list[i].clone());
            i += 1;
            continue;
        }
        // r = i; p = last word of the run
        let mut p = i;
        let mut q = i + 1;
        loop {
            q = skip_invisible(q);
            if q >= list.len() {
                break;
            }
            if matches!(list[q], Node::Glue(g, _) if g.subtype == 0) {
                let normal = is_font_glue(&list[q], font);
                let mut r = q + 1;
                if normal {
                    r = skip_invisible(r);
                    if word_of(r, font) {
                        p = r;
                        q = r + 1;
                        continue;
                    }
                }
                // a space adjustment also licenses merging
                if matches!(list.get(r), Some(Node::SpaceAdjKern(..))) {
                    let r2 = skip_invisible(r + 1);
                    if word_of(r2, font) {
                        p = r2;
                        q = r2 + 1;
                        continue;
                    }
                }
                break;
            }
            if word_of(q, font) {
                p = q;
                q += 1;
                continue;
            }
            break;
        }
        if p == i {
            out.push(list[i].clone());
            i += 1;
            continue;
        }
        let mut text = String::new();
        let mut width = 0i64;
        let mut invisibles: Vec<Node> = Vec::new();
        for n in &list[i..=p] {
            match n {
                Node::NativeGlyphRun { run, width: w, .. } if !run.text.is_empty() => {
                    text.push_str(&run.text);
                    width += *w as i64;
                }
                Node::Glue(g, _) => {
                    text.push(' ');
                    width += g.width as i64;
                    if sign == 1 && g.stretch_order == order {
                        width += (glue_set * g.stretch as f64).round() as i64;
                    } else if sign == 2 && g.shrink_order == order {
                        width -= (glue_set * g.shrink as f64).round() as i64;
                    }
                }
                Node::Kern(k, _) | Node::ExplicitKern(k, _) | Node::AccentKern(k, _) | Node::ItalicKern(k, _) | Node::SpaceAdjKern(k, _) => {
                    width += *k as i64;
                }
                n if invisible(n) => invisibles.push(n.clone()),
                _ => {}
            }
        }
        out.push(justified_word(eqtb, font, &text, width as i32, at, ugm));
        out.extend(invisibles);
        i = p + 1;
    }
    out
}

impl Engine {
    // ---- parameters (XeCore's eqtb entries) ----

    pub(crate) fn xe_int(&self, p: XeParam) -> i32 {
        use crate::prim::IntParam as I;
        let q = match p {
            XeParam::UseGlyphMetrics => I::XeTeXUseGlyphMetrics,
            XeParam::GenerateActualText => I::XeTeXGenerateActualText,
            XeParam::DashBreakState => I::XeTeXDashBreakState,
            XeParam::InterCharTokenState => I::XeTeXInterCharTokenState,
            XeParam::TracingFonts => I::XeTeXTracingFonts,
            XeParam::InterwordSpaceShaping => I::XeTeXInterwordSpaceShaping,
            XeParam::LinebreakPenalty => I::XeTeXLinebreakPenalty,
            XeParam::TracingLostChars => I::TracingLostChars,
        };
        self.eqtb.int_params[q.idx() as usize]
    }

    fn xe_char_class(&self, c: u32) -> u16 {
        self.eqtb.char_class(c)
    }

    fn xe_linebreak_skip(&self) -> crate::boxes::Glue {
        self.eqtb.glue_params[crate::prim::GlueParam::XeTeXLinebreakSkip.idx() as usize].clone()
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
        self.eqtb.inter_char_toks(c1, c2).map(|t| t.as_ref().clone())
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

    /// tex.web's `(state=token_list) and (token_type=backed_up_char)`: the
    /// character `tok` just fetched was delivered by the list an
    /// `\XeTeXinterchartoks` insertion put it back on. Anything that backs
    /// the character up again (`\futurelet`, the optional-space scan after a
    /// dimension, ...) turns it into an ordinary `backed_up` list, which
    /// `back_input` makes by first ending the finished lists below it; that
    /// is seen here as the character having been read from `pushed`.
    fn xe_is_backed_up_char(&self, tok: crate::token::Token) -> bool {
        self.pushed_read != tok
            && matches!(
                self.input.stack.last(),
                Some(crate::input::Source::TokList { toks, pos, name: BACKED_UP_CHAR, .. }) if *pos >= toks.len()
            )
    }

    /// `check_for_inter_char_toks`: true when a token list was inserted (the
    /// character has been put back and must not be processed now).
    fn xe_check_inter_char(&mut self, scalar: u32, is_letter: bool) -> bool {
        let sc = self.xe_char_class(scalar);
        self.native_text.space_class = sc;
        if !self.xe_interchar_enabled() || sc == CHAR_CLASS_IGNORED {
            return false;
        }
        let tok = crate::token::Token::unicode_char(if is_letter { 11 } else { 12 }, scalar);
        let prev = self.native_text.prev_class;
        let toks = if prev == CHAR_CLASS_BOUNDARY {
            if self.xe_is_backed_up_char(tok) {
                None
            } else {
                self.xe_interchar_toks(CHAR_CLASS_BOUNDARY, sc)
            }
        } else {
            self.xe_interchar_toks(prev, sc)
        };
        if let Some(toks) = toks {
            self.push_tokens_named(vec![tok], BACKED_UP_CHAR);
            self.pushed_read = crate::token::Token(0);
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
    fn xe_interword_space_shaping(&mut self, f: FontId) {
        use crate::boxes::WhatIt;
        let invisible = |n: &Node| {
            matches!(
                n,
                Node::Penalty(..)
                    | Node::Ins { .. }
                    | Node::Mark { .. }
                    | Node::Adj(..)
                    | Node::VAdjust(..)
                    | Node::PreAdjust(..)
                    | Node::Whatsit(
                        WhatIt::Write { .. }
                            | WhatIt::OpenOut { .. }
                            | WhatIt::CloseOut { .. }
                            | WhatIt::Special(..)
                            | WhatIt::Language { .. }
                            | WhatIt::SyncPoint { .. },
                        _
                    )
            )
        };
        let n = self.cur_list.len();
        if n < 3 || !self.cur_list[n - 1].is_native_word() {
            return;
        }
        let tail = n - 1;
        // the most recent earlier native word
        let Some(pp) = (0..tail).rev().find(|&i| self.cur_list[i].is_native_word()) else { return };
        if self.cur_list[pp].native_word().map(|w| w.0) != Some(f) {
            return;
        }
        let mut p = pp + 1;
        while p < tail && invisible(&self.cur_list[p]) {
            p += 1;
        }
        if p >= tail || !matches!(self.cur_list[p], Node::Glue(..)) {
            return;
        }
        let mut ppp = p + 1;
        while ppp < tail && invisible(&self.cur_list[ppp]) {
            ppp += 1;
        }
        if ppp != tail {
            return;
        }
        let (w_pp, t_pp) = match &self.cur_list[pp] {
            Node::NativeGlyphRun { width, run, .. } => (*width, run.text.to_string()),
            _ => return,
        };
        let (w_tail, t_tail) = match &self.cur_list[tail] {
            Node::NativeGlyphRun { width, run, .. } => (*width, run.text.to_string()),
            _ => return,
        };
        let joined = format!("{t_pp} {t_tail}");
        let Node::NativeGlyphRun { width: w_joined, .. } =
            self.xetex_native_word(f, &joined)
        else {
            return;
        };
        let t = w_joined - w_pp - w_tail;
        let space = self.eqtb.font_params.get(f as usize).and_then(|p| p.get(1)).copied().unwrap_or(0);
        if t != space {
            let attr = self.eqtb.cur_attr;
            self.cur_list.insert(p + 1, Node::SpaceAdjKern(t - space, attr));
        }
    }
}

#[allow(unused)]
fn _engine_kind_marker(_: EngineKind) {}
