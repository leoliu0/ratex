//! tex.web box displays: `show_box`/`show_node_list` (§173-§198, with the
//! e-TeX and pdfTeX node kinds), `short_display` (§175) with pdfTeX's
//! `print_font_identifier`, the mlist cases (§690-§698), `show_activities`
//! (§218-§219, §986) and the `begin_diagnostic`/`end_diagnostic` transcript
//! discipline (§245) the box-displaying commands share.

mod items;
mod nest;

use crate::boxes::{glue_subtype, Glue, LeaderBody, Node, WhatIt};
use crate::build::RULE_FILL;
use crate::engine::{Engine, Mode};
use crate::prim::IntParam;
use crate::tfm::FontId;
use crate::tex_bytes::push_printable;
use crate::token::Token;

/// `Node::Box::kind` of an alignment column shown as tex.web's unset node
/// (only in the copy of a preamble list made for a packing report).
const UNSET_KIND: u8 = 0xFF;

/// tex.web keeps the nesting prefix in the string pool and bounds
/// `depth_threshold` by the pool space left; this is that bound.
const MAX_DEPTH_THRESHOLD: i64 = 10_000;
/// `print_mark` shows at most `max_print_line-10` characters.
const MARK_LIMIT: usize = 69;
/// tex.web `default_code` for a fraction rule thickness.
const DEFAULT_CODE: i32 = 0x4000_0000;

/// One display in progress: the text plus tex.web's `cur_length` prefix.
pub(crate) struct BoxDisplay<'a> {
    e: &'a Engine,
    pub(crate) out: Vec<u8>,
    prefix: Vec<u8>,
    depth_threshold: i64,
    breadth_max: i64,
    /// `\escapechar` for the control sequence names this display prints
    escape: i32,
    /// tex.web `font_in_short_display` (`null_font` is font 0)
    font_in_short_display: Option<FontId>,
}

/// Ratex-internal bookkeeping nodes that tex.web lists do not contain.
fn invisible(n: &Node) -> bool {
    matches!(
        n,
        Node::Whatsit(WhatIt::SyncPoint { .. } | WhatIt::CjkText(_)) | Node::Empty | Node::InsDisc
    )
}

impl<'a> BoxDisplay<'a> {
    /// tex.web show_box's `depth_threshold`/`breadth_max` setup.
    pub(crate) fn new(e: &'a Engine) -> Self {
        let int = |p: IntParam| e.eqtb.int_params[p.idx() as usize] as i64;
        let breadth = int(IntParam::ShowBoxBreadth);
        BoxDisplay {
            e,
            out: Vec::new(),
            prefix: Vec::new(),
            depth_threshold: int(IntParam::ShowBoxDepth).min(MAX_DEPTH_THRESHOLD),
            breadth_max: if breadth <= 0 { 5 } else { breadth },
            escape: int(IntParam::EscapeChar) as i32,
            font_in_short_display: Some(0),
        }
    }

    /// A display with explicit `depth_threshold` and `breadth_max`, as
    /// show_eqtb sets them for a box register.
    pub(crate) fn with_limits(
        e: &'a Engine,
        depth_threshold: i64,
        breadth_max: i64,
        escape: i32,
    ) -> Self {
        BoxDisplay {
            depth_threshold,
            breadth_max,
            escape,
            ..Self::new(e)
        }
    }

    pub(crate) fn print(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
    }

    pub(crate) fn print_ln(&mut self) {
        self.out.push(b'\n');
    }

    /// tex.web print_nl relative to this display's own text.
    pub(crate) fn print_nl(&mut self, s: &str) {
        if self.out.last().is_some_and(|&b| b != b'\n') {
            self.print_ln();
        }
        self.print(s);
    }

    pub(crate) fn print_int(&mut self, n: i64) {
        self.print(&n.to_string());
    }

    pub(crate) fn print_scaled(&mut self, v: i32) {
        self.print(&crate::build::print_scaled(v as i64));
    }

    pub(crate) fn print_esc(&mut self, s: &str) {
        let esc = self.escape;
        if (0..256).contains(&esc) {
            push_printable(&self.e.xprn, &mut self.out, &[esc as u8]);
        }
        self.print(s);
    }

    fn print_ascii(&mut self, c: u8) {
        if i32::from(c) == self.e.new_line_char() {
            self.out.push(b'\n');
        } else {
            push_printable(&self.e.xprn, &mut self.out, &[c]);
        }
    }

    /// a character code of a Lua font: UTF-8 beyond the 8-bit range
    fn print_unicode(&mut self, c: u32) {
        match u8::try_from(c) {
            Ok(b) => self.print_ascii(b),
            Err(_) => {
                let mut buf = [0u8; 4];
                match char::from_u32(c) {
                    Some(ch) => self.out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes()),
                    None => self.print(&format!("[{c:X}]")),
                }
            }
        }
    }

    fn print_rule_dimen(&mut self, d: i32) {
        if d == RULE_FILL {
            self.out.push(b'*');
        } else {
            self.print_scaled(d);
        }
    }

    fn print_glue(&mut self, d: i32, order: u8, unit: &str) {
        self.print_scaled(d);
        if order > 3 {
            self.print("foul");
        } else if order > 0 {
            self.print("fil");
            for _ in 1..order {
                self.out.push(b'l');
            }
        } else {
            self.print(unit);
        }
    }

    pub(crate) fn print_spec(&mut self, g: &Glue, unit: &str) {
        self.print_scaled(g.width);
        self.print(unit);
        if g.stretch != 0 {
            self.print(" plus ");
            self.print_glue(g.stretch, g.stretch_order, unit);
        }
        if g.shrink != 0 {
            self.print(" minus ");
            self.print_glue(g.shrink, g.shrink_order, unit);
        }
    }

    /// pdfTeX print_font_identifier.
    fn print_font_identifier(&mut self, f: FontId) {
        let eqtb = &self.e.eqtb;
        let blink = eqtb.expand.get(f as usize).map_or(0, |x| x.blink);
        let shown = if blink == 0 { f } else { blink };
        let cs = eqtb.font_cs.get(shown as usize).copied().unwrap_or(0);
        let esc = self.escape;
        if (0..256).contains(&esc) {
            push_printable(&self.e.xprn, &mut self.out, &[esc as u8]);
        }
        push_printable(&self.e.xprn, &mut self.out, self.e.cs.name(cs));
        let font = eqtb.fonts.get(f as usize);
        if eqtb.int_params[IntParam::PdfTracingFonts.idx() as usize] > 0 {
            self.print(" (");
            if let Some(font) = font {
                self.print(&font.tfm_name);
                if font.at_size != font.dsize {
                    self.out.push(b'@');
                    self.print_scaled(font.at_size);
                    self.print("pt");
                }
            }
            self.out.push(b')');
        } else {
            let ratio = eqtb.expand.get(f as usize).map_or(0, |x| x.ratio);
            if ratio != 0 {
                self.print(" (");
                if ratio > 0 {
                    self.out.push(b'+');
                }
                self.print_int(ratio as i64);
                self.out.push(b')');
            }
        }
    }

    fn print_font_and_char(&mut self, f: FontId, c: u32) {
        if (f as usize) < self.e.eqtb.fonts.len() {
            self.print_font_identifier(f);
        } else {
            self.out.push(b'*');
        }
        self.out.push(b' ');
        self.print_unicode(c);
    }

    fn print_token_list(&mut self, tokens: &[Token]) {
        self.out.push(b'{');
        let start = self.out.len();
        let mut rest = tokens.iter();
        for t in rest.by_ref() {
            if self.out.len() - start >= MARK_LIMIT {
                self.print_esc("ETC.");
                break;
            }
            let bytes = self.e.tokens_to_bytes(std::slice::from_ref(t));
            push_printable(&self.e.xprn, &mut self.out, &bytes);
        }
        self.out.push(b'}');
    }

    /// print_mark for data Ratex keeps as text rather than tokens.
    fn print_text_mark(&mut self, text: &str) {
        self.out.push(b'{');
        let bytes = &*crate::tex_bytes::text_to_bytes(text);
        let start = self.out.len();
        for (i, &b) in bytes.iter().enumerate() {
            if self.out.len() - start >= MARK_LIMIT {
                if i < bytes.len() {
                    self.print_esc("ETC.");
                }
                break;
            }
            push_printable(&self.e.xprn, &mut self.out, &[b]);
        }
        self.out.push(b'}');
    }

    /// tex.web short_display.
    pub(crate) fn short_display(&mut self, list: &[Node]) {
        let mut i = 0;
        while i < list.len() {
            let n = &list[i];
            i += 1;
            match n {
                Node::Char { c, font } => self.short_char(*font, u32::from(*c)),
                Node::LuaGlyph(g) => {
                    if g.components.is_empty() {
                        self.short_char(g.font, g.c);
                    } else {
                        self.short_display(&g.components);
                    }
                }
                Node::Ligature {
                    font,
                    letters,
                    n_letters,
                    ..
                } => {
                    for &c in &letters[..(*n_letters as usize).min(3)] {
                        self.short_char(*font, u32::from(c));
                    }
                }
                Node::Box { .. }
                | Node::Ins { .. }
                | Node::Mark { .. }
                | Node::Adj(_)
                | Node::VAdjust(_)
                | Node::PreAdjust(_) => self.print("[]"),
                Node::Whatsit(_) if !invisible(n) => self.print("[]"),
                Node::Rule { .. } => self.out.push(b'|'),
                Node::Glue(g) | Node::Leaders { glue: g, .. } => {
                    if !g.is_zero_glue() {
                        self.out.push(b' ');
                    }
                }
                Node::MathKern(_, kind) if *kind >= crate::boxes::MATH_ON => {
                    if *kind >= crate::boxes::BEGIN_L {
                        self.print("[]");
                    } else {
                        self.out.push(b'$');
                    }
                }
                Node::Disc(d) => {
                    self.short_display(&d.pre_break);
                    self.short_display(&d.post_break);
                    i += d.replace_count;
                }
                Node::NativeGlyphRun { run, .. } => {
                    if self.font_in_short_display != Some(run.font) {
                        self.print_font_identifier(run.font);
                        self.out.push(b' ');
                        self.font_in_short_display = Some(run.font);
                    }
                    self.print(&run.text);
                }
                _ => {}
            }
        }
    }

    fn short_char(&mut self, font: FontId, c: u32) {
        if self.font_in_short_display != Some(font) {
            if (font as usize) < self.e.eqtb.fonts.len() {
                self.print_font_identifier(font);
            } else {
                self.out.push(b'*');
            }
            self.out.push(b' ');
            self.font_in_short_display = Some(font);
        }
        self.print_unicode(c);
    }

    /// tex.web show_box: the list `p`, then print_ln.
    pub(crate) fn show_box(&mut self, list: &[Node]) {
        self.show_node_list(list);
        self.print_ln();
    }

    fn node_list_display(&mut self, list: &[Node]) {
        self.prefix.push(b'.');
        self.show_node_list(list);
        self.prefix.pop();
    }

    pub(crate) fn show_node_list(&mut self, list: &[Node]) {
        if self.prefix.len() as i64 > self.depth_threshold {
            if list.iter().any(|n| !invisible(n)) {
                self.print(" []");
            }
            return;
        }
        let mut n = 0i64;
        // A scanned \discretionary keeps its replacement text inside the
        // node; TeX stores it as the `replace_count` nodes that follow.
        let flat = list.iter().flat_map(|n| {
            let embedded: &[Node] = match n {
                Node::Disc(d) if d.replace_count == 0 => &d.no_break,
                _ => &[],
            };
            std::iter::once(n).chain(embedded)
        });
        for node in flat.filter(|n| !invisible(n)) {
            self.print_ln();
            self.out.extend_from_slice(&self.prefix);
            n += 1;
            if n > self.breadth_max {
                self.print("etc.");
                return;
            }
            self.display_node(node);
        }
    }

    fn display_node(&mut self, node: &Node) {
        match node {
            Node::Char { c, font } => self.print_font_and_char(*font, u32::from(*c)),
            Node::LuaGlyph(g) => {
                self.print_font_and_char(g.font, g.c);
                if u16::from(g.subtype) & crate::lua_node::GLYPH_LIGATURE != 0 {
                    self.print(" (ligature ");
                    if u16::from(g.subtype) & crate::lua_node::GLYPH_LEFT != 0 {
                        self.out.push(b'|');
                    }
                    self.font_in_short_display = Some(g.font);
                    self.short_display(&g.components);
                    if u16::from(g.subtype) & crate::lua_node::GLYPH_RIGHT != 0 {
                        self.out.push(b'|');
                    }
                    self.out.push(b')');
                }
            }
            Node::Box {
                kind,
                w,
                h,
                d,
                shift,
                list,
                glue_sign,
                glue_order,
                glue_set,
                lr,
                ..
            } => {
                self.print_esc(match *kind {
                    crate::boxes::HBOX => "h",
                    UNSET_KIND => "unset",
                    _ => "v",
                });
                self.print("box(");
                self.print_scaled(*h);
                self.out.push(b'+');
                self.print_scaled(*d);
                self.print(")x");
                self.print_scaled(*w);
                let g = *glue_set;
                if g != 0.0 && *glue_sign != 0 {
                    self.print(", glue set ");
                    if *glue_sign == 2 {
                        self.print("- ");
                    }
                    if g.abs() > 20000.0 {
                        self.print(if g > 0.0 { ">" } else { "< -" });
                        self.print_glue(20000 * 65536, *glue_order, "");
                    } else {
                        self.print_glue((65536.0 * g).round() as i32, *glue_order, "");
                    }
                }
                if *shift != 0 {
                    self.print(", shifted ");
                    self.print_scaled(*shift);
                }
                if self.e.engine_kind == crate::engine::EngineKind::LuaTeX {
                    self.print(", direction TLT");
                }
                // etex.ch "Display if this box is never to be reversed"
                if *kind == crate::boxes::HBOX && *lr == crate::boxes::BOX_LR_DLIST {
                    self.print(", display");
                }
                self.node_list_display(list);
            }
            Node::Rule {
                width,
                height,
                depth,
            } => self.display_rule(*width, *height, *depth),
            Node::Ins {
                num,
                height,
                depth,
                cost,
                split_top_skip,
                split_max_depth,
                box_node,
                ..
            } => {
                self.print_esc("insert");
                self.print_int(*num as i64);
                self.print(", natural size ");
                self.print_scaled(height.wrapping_add(*depth));
                self.print("; split(");
                self.print_spec(split_top_skip, "");
                self.out.push(b',');
                self.print_scaled(*split_max_depth);
                self.print("); float cost ");
                self.print_int(*cost as i64);
                match &**box_node {
                    Node::Box { list, .. } => self.node_list_display(list),
                    other => self.node_list_display(std::slice::from_ref(other)),
                }
            }
            Node::Whatsit(w) => self.display_whatsit(w),
            Node::Glue(g) => {
                self.print_esc("glue");
                if g.subtype != glue_subtype::NORMAL {
                    self.out.push(b'(');
                    let name = glue_subtype::NAMES
                        .get(g.subtype as usize - 1)
                        .copied()
                        .unwrap_or("?");
                    self.print_esc(name);
                    self.out.push(b')');
                }
                self.out.push(b' ');
                self.print_spec(g, "");
            }
            Node::Leaders { glue, kind, body } => {
                self.print_esc("");
                match *kind {
                    crate::boxes::LEADERS_C => self.out.push(b'c'),
                    crate::boxes::LEADERS_X => self.out.push(b'x'),
                    _ => {}
                }
                self.print("leaders ");
                self.print_spec(glue, "");
                match body {
                    LeaderBody::Box(b) => self.node_list_display(std::slice::from_ref(&**b)),
                    LeaderBody::Rule {
                        width,
                        height,
                        depth,
                    } => {
                        let rule = Node::Rule {
                            width: *width,
                            height: *height,
                            depth: *depth,
                        };
                        self.node_list_display(std::slice::from_ref(&rule));
                    }
                }
            }
            Node::MarginKern { side, width, .. } => {
                self.print_esc("kern");
                self.print_scaled(*width);
                self.print(if *side == 0 {
                    " (left margin)"
                } else {
                    " (right margin)"
                });
            }
            Node::Kern(k) => {
                self.print_esc("kern");
                self.print_scaled(*k);
                if self.e.engine_kind == crate::engine::EngineKind::LuaTeX {
                    self.print(" (font)");
                }
            }
            Node::ExplicitKern(k) => {
                self.print_esc("kern");
                self.out.push(b' ');
                self.print_scaled(*k);
            }
            Node::ItalicKern(k) => {
                self.print_esc("kern");
                self.print_scaled(*k);
                self.print(" (italic)");
            }
            Node::AccentKern(k) => {
                self.print_esc("kern");
                self.out.push(b' ');
                self.print_scaled(*k);
                self.print(" (for accent)");
            }
            Node::MathKern(k, 0) => {
                self.print_esc("mkern");
                self.print_scaled(*k);
                self.print("mu");
            }
            Node::MathKern(k, kind) => {
                use crate::boxes::{math_end_lr, BEGIN_L, BEGIN_R, MATH_OFF};
                if *kind > MATH_OFF {
                    self.print_esc(if math_end_lr(*kind) { "end" } else { "begin" });
                    self.out.push(if *kind >= BEGIN_R {
                        b'R'
                    } else if *kind >= BEGIN_L {
                        b'L'
                    } else {
                        b'M'
                    });
                } else {
                    self.print_esc(if *kind == MATH_OFF { "mathoff" } else { "mathon" });
                    if *k != 0 {
                        self.print(", surrounded ");
                        self.print_scaled(*k);
                    }
                }
            }
            Node::Ligature {
                c,
                font,
                letters,
                n_letters,
                subtype,
                ..
            } => {
                self.print_font_and_char(*font, u32::from(*c));
                self.print(" (ligature ");
                if *subtype > 1 {
                    self.out.push(b'|');
                }
                self.font_in_short_display = Some(*font);
                for &l in &letters[..(*n_letters as usize).min(3)] {
                    self.short_char(*font, u32::from(l));
                }
                if subtype % 2 == 1 {
                    self.out.push(b'|');
                }
                self.out.push(b')');
            }
            Node::Penalty(p) => {
                self.print_esc("penalty ");
                self.print_int(*p as i64);
            }
            Node::Disc(d) if self.e.engine_kind == crate::engine::EngineKind::LuaTeX => {
                // texnodes.c show_box: `\discretionary (penalty n)` with the
                // three lists marked `<`, `>` and `=`
                let penalty = if d.penalty != crate::boxes::DISC_PENALTY_TEX {
                    d.penalty
                } else if d.pre_break.is_empty() {
                    self.e.eqtb.int_params[crate::prim::IntParam::ExHyphenPenalty.idx() as usize]
                } else {
                    self.e.eqtb.int_params[crate::prim::IntParam::HyphenPenalty.idx() as usize]
                };
                self.print_esc("discretionary");
                self.print(" (penalty ");
                self.print_int(i64::from(penalty));
                self.out.push(b')');
                for (mark, list) in [(b'<', &d.pre_break), (b'>', &d.post_break), (b'=', &d.no_break)] {
                    if !list.is_empty() {
                        let len = self.prefix.len();
                        self.prefix.extend_from_slice(&[b'.', mark, b' ']);
                        self.show_node_list(list);
                        self.prefix.truncate(len);
                    }
                }
            }
            Node::Disc(d) => {
                self.print_esc("discretionary");
                let replaced = if d.replace_count > 0 {
                    d.replace_count
                } else {
                    d.no_break.len()
                };
                if replaced > 0 {
                    self.print(" replacing ");
                    self.print_int(replaced as i64);
                }
                self.node_list_display(&d.pre_break);
                self.prefix.push(b'|');
                self.show_node_list(&d.post_break);
                self.prefix.pop();
            }
            Node::Mark { class, tokens } => {
                self.print_esc("mark");
                if *class != 0 {
                    self.out.push(b's');
                    self.print_int(*class as i64);
                }
                self.print_token_list(tokens);
            }
            Node::Adj(_) => self.print_esc("vadjust"),
            Node::VAdjust(list) => {
                self.print_esc("vadjust");
                self.node_list_display(list);
            }
            // pdftex.web: `print_esc("vadjust"); if adjust_pre(p)<>0 then print(" pre ")`
            Node::PreAdjust(list) => {
                self.print_esc("vadjust");
                self.print(" pre ");
                self.node_list_display(list);
            }
            Node::NativeGlyphRun { run, .. } => {
                self.print_font_identifier(run.font);
                self.out.push(b' ');
                self.print(&run.text);
            }
            // §690-§698: the cases of show_box that arise in mlists only
            Node::Style(s) => self.print_style(*s),
            Node::NonScript => {
                self.print_esc("glue(");
                self.print_esc("nonscript)");
            }
            Node::MuGlue(g) => {
                self.print_esc("glue(");
                self.print_esc("mskip) ");
                self.print_spec(g, "mu");
            }
            // noads are displayed through the mlist view (items.rs)
            Node::Choice
            | Node::ChoiceAlt { .. }
            | Node::MathChar { .. }
            | Node::Scripts { .. }
            | Node::OpLimits { .. }
            | Node::Frac { .. }
            | Node::Radical { .. }
            | Node::Accent { .. }
            | Node::Overline { .. }
            | Node::VCenter { .. }
            | Node::DelimBox { .. } => self.display_node_as_items(node),
            Node::InsDisc | Node::Empty => {}
        }
    }

    fn display_rule(&mut self, width: i32, height: i32, depth: i32) {
        self.print_esc("rule(");
        self.print_rule_dimen(height);
        self.out.push(b'+');
        self.print_rule_dimen(depth);
        self.print(")x");
        self.print_rule_dimen(width);
    }

    fn rule_spec(&mut self, w: i32, h: i32, d: i32) {
        self.out.push(b'(');
        self.print_rule_dimen(h);
        self.out.push(b'+');
        self.print_rule_dimen(d);
        self.print(")x");
        self.print_rule_dimen(w);
    }

    fn print_write_whatsit(&mut self, name: &str, stream: u16) {
        self.print_esc(name);
        match stream {
            0..=15 => self.print_int(stream as i64),
            16 => self.out.push(b'*'),
            _ => self.out.push(b'-'),
        }
    }

    /// tex.web §1356 + pdftex.web "Display the whatsit node".
    fn display_whatsit(&mut self, w: &WhatIt) {
        match w {
            WhatIt::OpenOut { stream, names, .. } => {
                self.print_write_whatsit("openout", *stream);
                self.out.push(b'=');
                self.print(&names.1);
            }
            WhatIt::Write { stream, tokens, .. } => {
                self.print_write_whatsit("write", *stream);
                self.print_token_list(tokens);
            }
            WhatIt::CloseOut { stream, .. } => self.print_write_whatsit("closeout", *stream),
            WhatIt::Special(s) => {
                self.print_esc("special");
                self.print_text_mark(s);
            }
            WhatIt::PdfLiteral { origin, data } => {
                self.print_esc("pdfliteral");
                match origin {
                    1 => self.print(" direct"),
                    2 => self.print(" page"),
                    _ => {}
                }
                self.print_text_mark(data);
            }
            WhatIt::PdfColorStack { stack, cmd, data } => {
                use crate::boxes::ColorStackCmd;
                self.print_esc("pdfcolorstack ");
                self.print_int(*stack as i64);
                match cmd {
                    ColorStackCmd::Set => self.print(" set "),
                    ColorStackCmd::Push => self.print(" push "),
                    ColorStackCmd::Pop => self.print(" pop"),
                    ColorStackCmd::Current => self.print(" current"),
                }
                if matches!(cmd, ColorStackCmd::Set | ColorStackCmd::Push) {
                    self.print_text_mark(data);
                }
            }
            WhatIt::PdfSetMatrix { matrix, .. } => {
                self.print_esc("pdfsetmatrix");
                self.print_text_mark(matrix);
            }
            WhatIt::PdfSave { .. } => self.print_esc("pdfsave"),
            WhatIt::PdfRestore { .. } => self.print_esc("pdfrestore"),
            WhatIt::PdfRefXImage { w, h, d, .. } => {
                self.print_esc("pdfrefximage");
                self.rule_spec(*w, *h, *d);
            }
            WhatIt::PdfRefXForm { w, h, d, .. } => {
                self.print_esc("pdfrefxform");
                self.rule_spec(*w, *h, *d);
            }
            WhatIt::PdfAnnot { attr, wd, ht, dp } => {
                self.print_esc("pdfannot");
                self.rule_spec(*wd, *ht, *dp);
                self.print_text_mark(attr);
            }
            WhatIt::PdfStartLink {
                attr,
                uri,
                name,
                wd,
                ht,
                dp,
            } => {
                self.print_esc("pdfstartlink");
                self.rule_spec(*wd, *ht, *dp);
                if !attr.is_empty() {
                    self.print(" attr");
                    self.print_text_mark(attr);
                }
                self.print(" action");
                if let Some(name) = name {
                    self.print(" goto name");
                    self.print_text_mark(name);
                } else if let Some(uri) = uri {
                    self.print(" user");
                    self.print_text_mark(&format!("/S /URI /URI ({uri})"));
                }
            }
            WhatIt::PdfEndLink => self.print_esc("pdfendlink"),
            WhatIt::PdfDest { id, kind, params } => {
                self.print_esc("pdfdest");
                match id {
                    crate::pdfout::DestId::Name(n) => {
                        self.print(" name");
                        self.print_text_mark(n);
                    }
                    crate::pdfout::DestId::Num(n) => {
                        self.print(" num");
                        self.print_int(*n as i64);
                    }
                }
                self.out.push(b' ');
                match kind {
                    0 => {
                        self.print("xyz");
                        if params[2] != crate::pdfout::PDF_POS_CURRENT {
                            self.print(" zoom");
                            self.print_int(params[2] as i64);
                        }
                    }
                    1 => self.print("fit"),
                    2 => self.print("fith"),
                    3 => self.print("fitv"),
                    4 => self.print("fitb"),
                    5 => self.print("fitbh"),
                    6 => self.print("fitbv"),
                    _ => {
                        self.print("fitr");
                        self.rule_spec(params[0], params[1], params[2]);
                    }
                }
            }
            WhatIt::SavePos { .. } => self.print_esc("pdfsavepos"),
            WhatIt::User(_) => self.print("whatsit?"),
            WhatIt::Language { lang, lhm, rhm } => {
                self.print_esc("setlanguage");
                self.print_int(*lang as i64);
                self.print(" (hyphenmin ");
                self.print_int(*lhm as i64);
                self.out.push(b',');
                self.print_int(*rhm as i64);
                self.out.push(b')');
            }
            WhatIt::PdfInterwordSpace(true) => self.print_esc("pdfinterwordspaceon"),
            WhatIt::PdfInterwordSpace(false) => self.print_esc("pdfinterwordspaceoff"),
            WhatIt::PdfFakeSpace => self.print_esc("pdffakespace"),
            WhatIt::PdfRunningLink(true) => self.print_esc("pdfrunninglinkon"),
            WhatIt::PdfRunningLink(false) => self.print_esc("pdfrunninglinkoff"),
            WhatIt::PdfSnapRefPoint => self.print_esc("pdfsnaprefpoint"),
            WhatIt::PdfSnapY(g) => {
                self.print_esc("pdfsnapy");
                self.out.push(b' ');
                self.print_spec(g, "");
                self.out.push(b' ');
                self.print_spec(&Glue::zero(), "");
            }
            WhatIt::PdfSnapYComp(r) => {
                self.print_esc("pdfsnapycomp");
                self.out.push(b' ');
                self.print_int(*r as i64);
            }
            #[allow(unreachable_patterns)]
            _ => self.print("whatsit?"),
        }
    }

    fn print_style(&mut self, s: crate::boxes::MathStyle) {
        use crate::boxes::MathStyle;
        self.print_esc(match s {
            MathStyle::Display => "displaystyle",
            MathStyle::Text => "textstyle",
            MathStyle::Script => "scriptstyle",
            MathStyle::ScriptScript => "scriptscriptstyle",
            MathStyle::CrampedDisplay => "crampeddisplaystyle",
            MathStyle::CrampedText => "crampedtextstyle",
            MathStyle::CrampedScript => "crampedscriptstyle",
            MathStyle::CrampedScriptScript => "crampedscriptscriptstyle",
        });
    }

    fn print_fam_and_char(&mut self, fam: u8, c: u32) {
        self.print_esc("fam");
        self.print_int(fam as i64);
        self.out.push(b' ');
        if self.e.engine_kind == crate::engine::EngineKind::LuaTeX {
            // texmath.c print_fam_and_char: `print(math_character(p))`
            // writes the code point as UTF-8 (printing.c `print`)
            if i64::from(c) == i64::from(self.e.new_line_char()) {
                self.out.push(b'\n');
            } else if let Some(ch) = char::from_u32(c) {
                let mut buf = [0u8; 4];
                self.out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
        } else {
            self.print_ascii(c as u8);
        }
    }
}

impl Engine {
    /// tex.web's `line` for `mode_line`: the current file's line number.
    pub(crate) fn nest_line(&self) -> i32 {
        self.input.current_file_line() as i32
    }

    /// The tex.web `mode_line` of the current semantic level.
    pub(crate) fn mode_line(&self) -> i32 {
        let n = self.saved_lists.len();
        if self.output_tail.is_some() && self.output_nest_mark.0 == n {
            return self.output_nest_mark.1;
        }
        self.saved_lists.last().map_or(0, |frame| frame.5)
    }

    /// Append `text` (one trace line, without its line end) to the transcript
    /// and, when `to_term`, to the terminal, starting it on a fresh line of
    /// each (tex.web `print_nl`) and ending the line as `end_diagnostic(false)`.
    pub(crate) fn print_nl_diagnostic(&mut self, text: &str, to_term: bool) {
        self.flush_trace_events();
        self.tex_print_nl(to_term, true);
        self.tex_print_printed(to_term, true, text.trim_end_matches('\n').as_bytes());
        self.tex_print_nl(to_term, true);
    }

    /// tex.web end_diagnostic(true): `print_nl(""); print_ln`.
    fn end_diagnostic(&mut self, term: bool) {
        self.tex_print_nl(term, true);
        self.tex_print_ln(term, true);
    }

    /// tex.web begin_diagnostic: the terminal sees diagnostics only when
    /// `\tracingonline>0`.
    pub(crate) fn diagnostic_to_term(&self) -> bool {
        self.eqtb.int_params[IntParam::TracingOnline.idx() as usize] > 0
    }

    /// tex.web §1121: after "Improper discretionary list", the deleted
    /// part of the sublist is displayed.
    pub(crate) fn show_deleted_disc_list(&mut self, deleted: &[Node]) {
        let mut d = BoxDisplay::new(self);
        d.print("The following discretionary sublist has been deleted:");
        d.show_box(deleted);
        let out = d.out;
        self.emit_box_diagnostic(out);
    }

    /// `begin_diagnostic; <display>; end_diagnostic(true)` for a display
    /// built by `body` (which starts with tex.web print_nl semantics).
    pub(crate) fn emit_box_diagnostic(&mut self, display: Vec<u8>) {
        let term = self.diagnostic_to_term();
        self.tex_print_nl(term, true);
        self.tex_print_printed(term, true, &display);
        self.end_diagnostic(term);
    }

    /// tex.web §1296 `\showbox`: `> \box<n>=` and the box display.
    pub(crate) fn show_box_register(&mut self, register: u16) {
        let mut d = BoxDisplay::new(self);
        d.print_nl("> ");
        d.print_esc("box");
        d.print_int(register as i64);
        d.out.push(b'=');
        match self.eqtb.boxed[register as usize].as_ref() {
            None => d.print("void"),
            Some(b) => d.show_box(std::slice::from_ref(b)),
        }
        let out = d.out;
        self.emit_box_diagnostic(out);
    }

    /// tex.web §638 `\tracingoutput`: the shipped box, after the
    /// `Completed box being shipped out [<counts>]` line.
    pub(crate) fn show_shipped_box(&mut self, b: &Node) {
        // print_nl(""); print_ln; print("Completed box being shipped out")
        self.tex_print_nl(true, true);
        self.tex_print_ln(true, true);
        self.tex_print_str(true, true, "Completed box being shipped out");
        let mut j = 9;
        while j > 0 && self.eqtb.count[j] == 0 {
            j -= 1;
        }
        let mut head = String::new();
        for k in 0..=j {
            head.push_str(&self.eqtb.count[k].to_string());
            if k < j {
                head.push('.');
            }
        }
        head.push(']');
        let near_edge = self.term_offset > crate::tex_print::MAX_PRINT_LINE - 9;
        if near_edge {
            self.tex_print_ln(true, true);
        } else if self.term_offset > 0 || self.file_offset > 0 {
            self.tex_print_str(true, true, " ");
        }
        self.tex_print_str(true, true, "[");
        self.tex_print_str(true, true, &head);
        let mut d = BoxDisplay::new(self);
        d.show_box(std::slice::from_ref(b));
        let out = d.out;
        self.emit_box_diagnostic_inline(out);
    }

    /// tex.web hpack/vpack common_ending for an under/over-full box `r`:
    /// the header line (also the structured warning's message), for an
    /// hbox the short display of its list, and the box display.
    pub(crate) fn show_pack_report(&mut self, header: &str, r: &Node) {
        let mut d = BoxDisplay::new(self);
        // print_ln; print_nl(header)
        d.print(header);
        let hbox = matches!(r, Node::Box { kind, .. } if *kind == crate::boxes::HBOX);
        if hbox {
            d.print_ln();
            d.font_in_short_display = Some(0);
            if let Node::Box { list, .. } = r {
                d.short_display(list);
            }
            d.print_ln();
        } else if !self.in_output {
            d.print_ln();
        }
        let head = std::mem::take(&mut d.out);
        if self.pack_begin_line < 0 {
            // tex.web §804: the preamble list holds unset nodes, one per column
            let mut preamble = r.clone();
            if let Node::Box { list, .. } = &mut preamble {
                for n in list.iter_mut() {
                    if let Node::Box { kind, .. } = n {
                        *kind = UNSET_KIND;
                    }
                }
            }
            d.show_box(std::slice::from_ref(&preamble));
        } else {
            d.show_box(std::slice::from_ref(r));
        }
        let display = d.out;
        // print_ln; the header goes to the transcript; the terminal follows
        // Ratex's structured-warning policy for box reports
        self.tex_print_ln(false, true);
        self.tex_print_printed(false, true, &head);
        self.emit_box_diagnostic_inline(display);
    }

    /// `begin_diagnostic; show_box; end_diagnostic(true)` continuing the
    /// current transcript line (no print_nl).
    fn emit_box_diagnostic_inline(&mut self, display: Vec<u8>) {
        let term = self.diagnostic_to_term();
        self.tex_print_printed(term, true, &display);
        self.end_diagnostic(term);
    }

    /// tex.web §218 show_activities (for `\showlists`).
    pub(crate) fn show_activities(&self) -> Vec<u8> {
        let mut d = BoxDisplay::new(self);
        // print_nl(""); print_ln
        d.print_ln();
        let mut levels = self.nest_levels();
        let mut par_index = self.par_page_lists.len();
        let mut above_is_paragraph = false;
        // tex.web keeps the paragraph's starting language in the level's
        // prev_graf (new_graf §1091) and `clang` in its aux
        let mut par_lang = self.par_langs.len();
        let mut clang_above: Option<u8> = None;
        for p in (0..levels.len()).rev() {
            let level = &mut levels[p];
            d.print_nl("### ");
            print_mode(&mut d, level.mode);
            d.print(" entered at line ");
            d.print_int(level.line.unsigned_abs() as i64);
            let mut clang = 0u8;
            if level.mode == Mode::Horizontal {
                let pl = if par_lang > 0 {
                    par_lang -= 1;
                    self.par_langs.get(par_lang)
                } else {
                    None
                };
                clang = clang_above.unwrap_or(self.clang);
                clang_above = pl.map(|pl| pl.outer_clang);
                let start = pl.map_or_else(|| self.current_language(), |pl| pl.start);
                if (start.lhm, start.rhm, start.lang) != (2, 3, 0) {
                    d.print(" (language");
                    d.print_int(i64::from(start.lang));
                    d.print(":hyphenmin");
                    d.print_int(i64::from(start.lhm));
                    d.out.push(b',');
                    d.print_int(i64::from(start.rhm));
                    d.out.push(b')');
                }
            }
            if level.line < 0 {
                d.print(" (\\output routine)");
            }
            let contributions: &[Node] = match level.mode {
                Mode::Vertical => &self.page_list[self.page_processed.min(self.page_list.len())..],
                _ => &[],
            };
            if p == 0 {
                self.show_page_status(&mut d);
                if !contributions.is_empty() {
                    d.print_nl("### recent contributions:");
                }
            }
            if let Some(items) = level.math.as_ref() {
                d.show_items_box(items);
            } else {
                let list: &[Node] = match level.mode {
                    Mode::Vertical => contributions,
                    Mode::InternalVertical if above_is_paragraph && par_index > 0 => {
                        par_index -= 1;
                        &self.par_page_lists[par_index]
                    }
                    _ => level.list,
                };
                if level.mode == Mode::Vertical && above_is_paragraph && par_index > 0 {
                    par_index -= 1;
                }
                if level.prefix.is_empty() {
                    d.show_box(list);
                } else {
                    let mut items = std::mem::take(&mut level.prefix);
                    items.extend(nest::plain_items(list));
                    d.show_items_box(&items);
                }
            }
            above_is_paragraph = level.mode == Mode::Horizontal;
            match level.mode {
                Mode::Vertical | Mode::InternalVertical => {
                    d.print_nl("prevdepth ");
                    if level.prev_depth <= self.ignore_depth() {
                        d.print("ignored");
                    } else {
                        d.print_scaled(level.prev_depth);
                    }
                    if level.prev_graf != 0 {
                        d.print(", prevgraf ");
                        d.print_int(level.prev_graf as i64);
                        d.print(" line");
                        if level.prev_graf != 1 {
                            d.out.push(b's');
                        }
                    }
                }
                Mode::Horizontal | Mode::RestrictedHorizontal => {
                    d.print_nl("spacefactor ");
                    d.print_int(level.space_factor as i64);
                    if level.mode == Mode::Horizontal && clang > 0 {
                        d.print(", current language ");
                        d.print_int(i64::from(clang));
                    }
                }
                Mode::Math | Mode::DisplayMath => {
                    if let Some(frac) = level.incompleat.as_ref() {
                        d.print(if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                            "this will be denominator of:"
                        } else {
                            "this will begin denominator of:"
                        });
                        d.show_items_box(std::slice::from_ref(frac));
                    }
                }
            }
        }
        d.out
    }

    /// e-TeX show_save_groups (`\showgroups`).
    pub(crate) fn show_save_groups(&self) -> Vec<u8> {
        use crate::eqtb::{
            group_code as gc, group_description, BOX_FLAG, GLOBAL_BOX_FLAG, LEADER_FLAG,
            SHIP_OUT_FLAG,
        };
        const VMODE: i32 = 1;
        const HMODE: i32 = 102;
        const MMODE: i32 = 203;
        let mut d = BoxDisplay::new(self);
        // print_nl(""); print_ln
        d.print_ln();
        let levels = self.nest_levels();
        let mode_of = |index: usize| -> i32 {
            match levels[index].mode {
                Mode::Vertical => VMODE,
                Mode::InternalVertical => -VMODE,
                Mode::Horizontal => HMODE,
                Mode::RestrictedHorizontal => -HMODE,
                Mode::DisplayMath => MMODE,
                Mode::Math => -MMODE,
            }
        };
        let groups = &self.eqtb.groups;
        let mut p = levels.len() - 1;
        let mut a: i32 = 1;
        let mut remaining = groups.len();
        loop {
            d.print_nl("### ");
            if remaining == 0 {
                d.print("bottom level");
                break;
            }
            let group = groups[remaining - 1];
            let meta = group.meta;
            d.print(&group_description(meta.code, remaining as u16, group.line, true));
            let mut m;
            loop {
                m = mode_of(p);
                if p > 0 {
                    p -= 1;
                } else {
                    m = VMODE;
                }
                if m != HMODE {
                    break;
                }
            }
            d.print(" (");
            // where the arms below continue: the box context, `found1`
            // (name and packaging), `found2` (the brace) and `found` (the
            // closing parenthesis)
            enum Next {
                Context(&'static str),
                Found1(&'static str),
                Found2,
                Found,
            }
            let next = match meta.code {
                gc::SIMPLE => {
                    p += 1;
                    Next::Found2
                }
                gc::HBOX | gc::ADJUSTED_HBOX => Next::Context("hbox"),
                gc::VBOX => Next::Context("vbox"),
                gc::VTOP => Next::Context("vtop"),
                gc::ALIGN => {
                    if a == 0 {
                        a = 1;
                        Next::Found1(if m == -VMODE { "halign" } else { "valign" })
                    } else {
                        if a == 1 {
                            d.print("align entry");
                        } else {
                            d.print_esc("cr");
                        }
                        // tex.web: the entry's row level is not a group
                        // of its own (`if p>=a then p:=p-a`).
                        if let Some(q) = p.checked_add_signed(-(a as isize)) {
                            p = q;
                        }
                        a = 0;
                        Next::Found
                    }
                }
                gc::NO_ALIGN => {
                    p += 1;
                    a = -1;
                    d.print_esc("noalign");
                    Next::Found2
                }
                gc::OUTPUT => {
                    d.print_esc("output");
                    Next::Found
                }
                gc::MATH => Next::Found2,
                gc::DISC | gc::MATH_CHOICE => {
                    d.print_esc(if meta.code == gc::DISC {
                        "discretionary"
                    } else {
                        "mathchoice"
                    });
                    for i in 1..=3 {
                        if i <= meta.spec {
                            d.print("{}");
                        }
                    }
                    Next::Found2
                }
                gc::INSERT => {
                    // pdftex.web's begin_insert_or_adjust keeps the class in
                    // saved(0) and the `\vadjust pre` flag in saved(1), so
                    // show_save_groups' saved(-2) is that flag: `\insert1`
                    // for `\vadjust pre`, `\insert0` for every other insert
                    // group (the meta's spec holds the flag).
                    d.print_esc("insert");
                    d.print_int(i64::from(meta.spec));
                    Next::Found2
                }
                gc::VCENTER => Next::Found1("vcenter"),
                gc::SEMI_SIMPLE => {
                    p += 1;
                    d.print_esc("begingroup");
                    Next::Found
                }
                gc::MATH_SHIFT => {
                    if m == MMODE {
                        d.print("$");
                        d.print("$");
                        Next::Found
                    } else if mode_of(p) == MMODE {
                        d.print_esc(if meta.spec == 1 { "leqno" } else { "eqno" });
                        Next::Found
                    } else {
                        d.print("$");
                        Next::Found
                    }
                }
                _ => {
                    d.print_esc(if meta.spec == 1 { "middle" } else { "left" });
                    Next::Found
                }
            };
            let (name, show_context) = match next {
                Next::Context(name) => (Some(name), true),
                Next::Found1(name) => (Some(name), false),
                Next::Found2 => (None, false),
                Next::Found => {
                    d.print(")");
                    remaining -= 1;
                    continue;
                }
            };
            if show_context && meta.context != 0 {
                let i = meta.context;
                if i < BOX_FLAG {
                    let horizontal = mode_of(p).abs() == VMODE;
                    d.print_esc(match (horizontal, i > 0) {
                        (true, true) => "moveright",
                        (true, false) => "moveleft",
                        (false, true) => "lower",
                        (false, false) => "raise",
                    });
                    d.print_scaled(i.abs());
                    d.print("pt");
                } else if i < SHIP_OUT_FLAG {
                    let mut register = i;
                    if i >= GLOBAL_BOX_FLAG {
                        d.print_esc("global");
                        register -= GLOBAL_BOX_FLAG - BOX_FLAG;
                    }
                    d.print_esc("setbox");
                    d.print_int((register - BOX_FLAG) as i64);
                    d.print("=");
                } else {
                    d.print_esc(match i - LEADER_FLAG {
                        -1 => "shipout",
                        0 => "leaders",
                        1 => "cleaders",
                        _ => "xleaders",
                    });
                }
            }
            if let Some(name) = name {
                d.print_esc(name);
                if meta.spec != 0 {
                    d.print(" ");
                    d.print(if meta.exactly { "to" } else { "spread" });
                    d.print_scaled(meta.spec);
                    d.print("pt");
                }
            }
            d.print("{)");
            remaining -= 1;
        }
        d.out
    }

    /// e-TeX's `\showifs` display.
    pub(crate) fn show_ifs(&self) -> Vec<u8> {
        let mut d = BoxDisplay::new(self);
        // print_nl(""); print_ln
        d.print_ln();
        if self.if_stack.is_empty() {
            d.print_nl("### ");
            d.print("no active conditionals");
        } else {
            for (index, state) in self.if_stack.iter().enumerate().rev() {
                d.print_nl("### level ");
                d.print_int((index + 1) as i64);
                d.print(": ");
                if state.unless {
                    d.print_esc("unless");
                }
                d.print_esc(crate::trace::if_name(state.kind));
                if state.in_else {
                    d.print_esc("else");
                }
                if state.loc_line != 0 {
                    d.print(" entered on line ");
                    d.print_int(state.loc_line as i64);
                }
            }
        }
        d.out
    }

    fn show_page_status(&self, d: &mut BoxDisplay<'_>) {
        let page = &self.page_list[..self.page_processed.min(self.page_list.len())];
        if page.is_empty() {
            return;
        }
        d.print_nl("### current page:");
        if self.in_output {
            d.print(" (held over for next output)");
        }
        d.show_box(page);
        if !self.page_goal_set {
            return;
        }
        d.print_nl("total height ");
        d.print_scaled(self.page_total as i32);
        for (i, unit) in ["", "fil", "fill", "filll"].iter().enumerate() {
            if self.page_stretch[i] != 0 {
                d.print(" plus ");
                d.print_scaled(self.page_stretch[i] as i32);
                d.print(unit);
            }
        }
        if self.page_shrink[0] != 0 {
            d.print(" minus ");
            d.print_scaled(self.page_shrink[0] as i32);
        }
        d.print_nl(" goal height ");
        d.print_scaled(self.page_goal as i32);
        for r in &self.page_insertions {
            d.print_ln();
            d.print_esc("insert");
            d.print_int(r.num as i64);
            d.print(" adds ");
            let count = self.eqtb.count[r.num as usize];
            let t = if count == 1000 {
                r.height_raw
            } else {
                (r.height_raw / 1000) * count as i64
            };
            d.print_scaled(t as i32);
            if r.split_up {
                let broken = r.broken_ord.unwrap_or(usize::MAX);
                let t = page
                    .iter()
                    .filter(|n| matches!(n, Node::Ins { .. }))
                    .take(broken.saturating_add(1))
                    .filter(|n| matches!(n, Node::Ins { num, .. } if *num == r.num))
                    .count();
                d.print(", #");
                d.print_int(t as i64);
                d.print(" might split");
            }
        }
    }
}

/// tex.web print_mode.
fn print_mode(d: &mut BoxDisplay<'_>, mode: Mode) {
    d.print(match mode {
        Mode::Vertical => "vertical",
        Mode::Horizontal => "horizontal",
        Mode::DisplayMath => "display math",
        Mode::InternalVertical => "internal vertical",
        Mode::RestrictedHorizontal => "restricted horizontal",
        Mode::Math => "math",
    });
    d.print(" mode");
}
