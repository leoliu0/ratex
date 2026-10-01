//! Shipping the glyphs of Lua fonts (luatex `pdf_place_glyph`,
//! `do_vf_packet`): real glyphs of OpenType/TrueType fonts go to the PDF as
//! two-byte CIDs through the native glyph machinery, glyphs of TFM-named and
//! Type 1 Lua fonts through the classic one-byte path, virtual characters
//! run their command list.

use std::rc::Rc;

use super::*;
use crate::lua_font::{LuaFont, LuaPdfKind, VfCommand};
use crate::tfm::round_xn_over_d as round_xn;

/// The text a glyph stands for in the ToUnicode CMap (luatex
/// `write_cid_tounicode`): the `tounicode` string when the font asks for
/// it, the character code otherwise.
fn glyph_text(lf: &LuaFont, tounicode: Option<&[u8]>, c: u32) -> String {
    if lf.tounicode != 0 {
        let Some(hex) = tounicode else { return String::new() };
        let units: Vec<u16> = hex
            .chunks(4)
            .filter_map(|chunk| u16::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok())
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        char::from_u32(c).map(String::from).unwrap_or_default()
    }
}

impl RenderCtx<'_> {
    /// Output glyph `c` of the Lua font `f` with its origin at (`x`, `y`)
    /// (`y` is the baseline, downwards) displaced by (`xoff`, `yoff`), the
    /// glyph being expanded by `ex` per mille. Returns the advance in sp.
    pub(crate) fn emit_lua_glyph(&mut self, f: u16, c: u32, x: i64, y: i64, xoff: i32, yoff: i32, ex: i32) -> i64 {
        let Some(font) = self.eng.eqtb.fonts.get(f as usize).cloned() else {
            return 0;
        };
        let Some(lf) = font.lua.clone() else {
            return 0;
        };
        let Some(ci) = lf.chars.get(&c) else {
            return 0;
        };
        let mut width = ci.width;
        if ex != 0 && width != 0 {
            width = round_xn(width, 1000 + ex, 1000);
        }
        let (px, py) = (x + i64::from(xoff), y - i64::from(yoff));
        if ci.commands.is_some() {
            self.lua_vf_packet(f, c, ex, px, py);
        } else {
            self.lua_real_glyph(f, &lf, c, px, py, ex);
        }
        self.eng.lua_fonts.used.insert(f);
        i64::from(width)
    }

    /// A character that is not made of virtual-font commands.
    fn lua_real_glyph(&mut self, f: u16, lf: &Rc<LuaFont>, c: u32, x: i64, y: i64, ex: i32) {
        match lf.pdf_kind() {
            LuaPdfKind::Legacy => {
                if let Ok(byte) = u8::try_from(c) {
                    self.emit_char_sp(f, byte, x, y, ex);
                }
            }
            LuaPdfKind::Cid => self.lua_cid_glyph(f, lf, c, x, y, ex),
        }
    }

    fn lua_cid_glyph(&mut self, fid: u16, lf: &Rc<LuaFont>, c: u32, x: i64, y: i64, ex: i32) {
        let at_size_sp = self.eng.eqtb.fonts.get(fid as usize).map_or(0, |font| i64::from(font.at_size));
        if at_size_sp <= 0 {
            return;
        }
        let Some(ci) = lf.chars.get(&c) else { return };
        let gid = match self.eng.lua_glyph_index(fid, lf, c) {
            Ok(gid) => gid,
            Err(message) => {
                self.eng.error(&message);
                return;
            }
        };
        let text = glyph_text(lf, ci.tounicode.as_deref(), c);
        let run = Rc::new(crate::native_layout::NativeRun {
            font: fid,
            text: Rc::from(text.as_str()),
            glyphs: vec![crate::native_layout::NativeGlyph {
                glyph_id: gid,
                cluster_start: 0,
                cluster_end: text.len() as u32,
                x_advance: ci.width,
                y_advance: 0,
                x_offset: 0,
                y_offset: 0,
            }],
        });
        self.display_list.push(crate::boxes::DisplayItem::NativeGlyphRun {
            run,
            start: 0,
            end: 1,
            x_bp: sp_to_bp(x),
            y_bp: self.y_pdf(sp_to_bp(y)),
            tag: None,
            span: None,
        });
        let (_, m) = divide_scaled(at_size_sp, ONE_HUNDRED_BP_SP, 6);
        let (binding, code) = self.eng.pdf_doc.get_or_alloc_native_code(fid as usize, gid, &text);
        let ratio = ex;
        self.begin_hex_string(x, y, fid, binding, ratio);
        use std::fmt::Write;
        let _ = write!(&mut self.content, "{code:04X}");
        let nom_sp = self.native_glyph_nom_advance_sp(fid, gid);
        let (_, nom_out) = if self.cur_tm_a == 0 {
            divide_scaled(nom_sp, m, 4)
        } else {
            let (_, out) = divide_scaled(round_xn_over_d(nom_sp, 1000, 1000 + self.cur_tm_a as i64), m, 4);
            (0, out)
        };
        self.delta_h += nom_out;
    }

    /// luatex `do_vf_packet`: run the commands of virtual character `c` of
    /// font `vf_f`; (`x`, `y`) is the character's origin.
    fn lua_vf_packet(&mut self, vf_f: u16, c: u32, ex: i32, x: i64, y: i64) {
        let Some(vf_font) = self.eng.eqtb.fonts.get(vf_f as usize).cloned() else { return };
        let Some(lf) = vf_font.lua.clone() else { return };
        let Some(commands) = lf.chars.get(&c).and_then(|ci| ci.commands.clone()) else { return };
        let mut st = VfState { vf_f, c, ex, x, y, fs: lf.size, local_font: 0, h: 0, v: 0, stack: Vec::new(), in_lua: false };
        for cmd in &commands {
            if !self.vf_command(&mut st, cmd) {
                return;
            }
        }
    }

    /// Execute one virtual-font command; `false` stops the packet.
    fn vf_command(&mut self, st: &mut VfState, cmd: &VfCommand) -> bool {
        match cmd {
            VfCommand::Font(id) => st.local_font = *id,
            VfCommand::Push => st.stack.push((st.h, st.v)),
            VfCommand::Pop => return self.vf_pop(st),
            VfCommand::Char(k) => self.vf_char(st, *k),
            VfCommand::Rule(rh, rw) => {
                let size_v = crate::lua_font_vf::store_scaled_f(*rh, st.fs);
                let size_h = crate::lua_font_vf::store_scaled_f(*rw, st.fs);
                self.vf_rule(st, size_h, size_v);
            }
            VfCommand::Right(r) => {
                let mut i = crate::lua_font_vf::store_scaled_f(*r, st.fs);
                if st.ex != 0 && i != 0 {
                    i = round_xn(i, 1000 + st.ex, 1000);
                }
                st.h += i64::from(i);
            }
            VfCommand::Down(d) => st.v += i64::from(crate::lua_font_vf::store_scaled_f(*d, st.fs)),
            VfCommand::Pdf { mode, data } => {
                self.lua_literal(*mode, &String::from_utf8_lossy(data), st.x + st.h, st.y + st.v);
            }
            VfCommand::PdfMode(mode) => self.lua_literal(*mode, "", st.x + st.h, st.y + st.v),
            VfCommand::Special(data) => {
                let text = String::from_utf8_lossy(data).into_owned();
                self.handle_special(&text, st.x + st.h, st.y + st.v);
            }
            VfCommand::Nop | VfCommand::Scale(_) => {}
            VfCommand::Lua(function) => self.vf_lua(st, function),
            VfCommand::Node(_) | VfCommand::Image(_) => {
                self.eng.warning_at("virtual font node and image commands are not supported at shipout", None);
            }
        }
        true
    }

    fn vf_pop(&mut self, st: &mut VfState) -> bool {
        match st.stack.pop() {
            Some((h, v)) => {
                st.h = h;
                st.v = v;
                true
            }
            None => {
                self.eng.error("vf: packet_stack_level underflow");
                false
            }
        }
    }

    fn vf_rule(&mut self, st: &mut VfState, mut size_h: i32, size_v: i32) {
        if st.ex != 0 && size_h > 0 {
            size_h = round_xn(size_h, 1000 + st.ex, 1000);
        }
        if size_h > 0 && size_v > 0 {
            self.emit_rect_sp(st.x + st.h, st.y + st.v, i64::from(size_h), i64::from(size_v));
        }
        st.h += i64::from(size_h);
    }

    /// The `char` command: typeset `k` of the current local font.
    fn vf_char(&mut self, st: &mut VfState, k: u32) {
        let (px, py) = (st.x + st.h, st.y + st.v);
        let local_font = st.local_font;
        let target = self.eng.eqtb.fonts.get(local_font as usize).cloned();
        let Some(tf) = target.as_ref().and_then(|t| t.lua.clone()) else {
            // a TFM font used as a local font of a Lua font
            if let Ok(byte) = u8::try_from(k) {
                let adv = self.font_char_advance_sp(local_font, byte);
                self.emit_char_sp(local_font, byte, px, py, st.ex);
                st.h += adv;
            }
            return;
        };
        let Some(tci) = tf.chars.get(&k) else {
            self.eng.warning_at(
                &format!(
                    "Missing character: There is no {} ({:#x}) in font {}!",
                    char::from_u32(k).unwrap_or('?'),
                    k,
                    String::from_utf8_lossy(&tf.name)
                ),
                None,
            );
            return;
        };
        if tci.commands.is_some() && !(k == st.c && local_font == st.vf_f) {
            self.lua_vf_packet(local_font, k, st.ex, px, py);
        } else {
            self.lua_real_glyph(local_font, &tf, k, px, py, st.ex);
            self.eng.lua_fonts.used.insert(local_font);
        }
        let mut w = tci.width;
        if st.ex != 0 && w != 0 {
            w = round_xn(w, 1000 + st.ex, 1000);
        }
        st.h += i64::from(w);
    }

    /// `{"lua", function}`: call `function(font, char)` with the `vf`
    /// library bound to this packet.
    fn vf_lua(&mut self, st: &mut VfState, function: &tex_lua::Value) {
        let Some(function) = function.as_function() else { return };
        st.in_lua = true;
        let ctx: *mut RenderCtx<'static> = (self as *mut RenderCtx<'_>).cast();
        let previous = VF_CALL.with(|slot| slot.replace(Some((ctx, st as *mut VfState))));
        let (vf_f, c) = (i64::from(st.vf_f), i64::from(st.c));
        let eng: *mut Engine = &mut *self.eng;
        // SAFETY: the engine outlives this call; Lua reaches it through
        // `lua_run`'s active-engine pointer exactly as for other callbacks.
        use tex_lua::LuaApi;
        let result = unsafe { &mut *eng }.lua_run(|lua| {
            function
                .call::<_, ()>((vf_f, c))
                .map_err(|e| lua.lua.get_error_message(e).message().to_string())
        });
        VF_CALL.with(|slot| slot.set(previous));
        st.in_lua = false;
        if let Err(message) = result {
            self.eng.error(&format!("LuaTeX error {message}"));
        }
    }

    /// A `pdf` virtual-font literal in the given luatex literal mode.
    fn lua_literal(&mut self, mode: u8, data: &str, x: i64, y: i64) {
        use crate::lua_font::*;
        let origin = match mode {
            PDF_SET_ORIGIN => 0,
            PDF_DIRECT_PAGE => 2,
            _ => 1,
        };
        self.emit_whatsit_sp(
            &crate::boxes::WhatIt::PdfLiteral { origin, data: data.to_string() },
            x,
            y,
        );
    }
}

/// The running state of one virtual character (luatex `vf_struct`).
pub(crate) struct VfState {
    vf_f: u16,
    c: u32,
    ex: i32,
    x: i64,
    y: i64,
    fs: i32,
    local_font: u16,
    h: i64,
    v: i64,
    stack: Vec<(i64, i64)>,
    in_lua: bool,
}

thread_local! {
    /// The packet whose `{"lua", f}` command is running.
    static VF_CALL: std::cell::Cell<Option<(*mut RenderCtx<'static>, *mut VfState)>> =
        const { std::cell::Cell::new(None) };
}

/// Run `f` on the virtual character that is executing a `lua` command
/// (the `vf` library); an error outside virtual fonts, like luatex.
pub(crate) fn with_vf_packet<R>(
    name: &str,
    f: impl FnOnce(&mut RenderCtx<'_>, &mut VfState) -> R,
) -> Result<R, String> {
    let Some((ctx, st)) = VF_CALL.with(std::cell::Cell::get) else {
        return Err(format!("vf: vf.{name}() outside virtual font"));
    };
    // SAFETY: both pointers were stored by `vf_lua`, which keeps the context
    // and the packet state alive and unused for the duration of the call.
    Ok(f(unsafe { &mut *ctx }, unsafe { &mut *st }))
}

impl VfState {
    /// `vf.fontid`.
    pub(crate) fn set_font(&mut self, f: u16) {
        self.local_font = f;
    }
}

impl RenderCtx<'_> {
    pub(crate) fn vf_lib_char(&mut self, st: &mut VfState, k: u32) {
        self.vf_char(st, k);
    }

    pub(crate) fn vf_lib_push(&mut self, st: &mut VfState) {
        st.stack.push((st.h, st.v));
    }

    pub(crate) fn vf_lib_pop(&mut self, st: &mut VfState) -> Result<(), String> {
        if self.vf_pop(st) {
            Ok(())
        } else {
            Err("vf: packet_stack_level underflow".to_string())
        }
    }

    pub(crate) fn vf_lib_right(&mut self, st: &mut VfState, i: i32) {
        let mut i = i;
        if st.ex != 0 && i != 0 {
            i = round_xn(i, 1000 + st.ex, 1000);
        }
        st.h += i64::from(crate::lua_font_vf::store_scaled_f(i, st.fs));
    }

    pub(crate) fn vf_lib_down(&mut self, st: &mut VfState, i: i32) {
        st.v += i64::from(crate::lua_font_vf::store_scaled_f(i, st.fs));
    }

    pub(crate) fn vf_lib_rule(&mut self, st: &mut VfState, h: i32, v: i32) {
        let mut size_h = h;
        if st.ex != 0 && size_h > 0 {
            size_h = round_xn(size_h, 1000 + st.ex, 1000);
        }
        let size_h = crate::lua_font_vf::store_scaled_f(size_h, st.fs);
        let size_v = crate::lua_font_vf::store_scaled_f(v, st.fs);
        if size_h > 0 && size_v > 0 {
            self.emit_rect_sp(st.x + st.h, st.y + st.v, i64::from(size_h), i64::from(size_v));
        }
        st.h += i64::from(size_h);
    }

    pub(crate) fn vf_lib_special(&mut self, st: &mut VfState, data: &[u8]) {
        let text = String::from_utf8_lossy(data).into_owned();
        self.handle_special(&text, st.x + st.h, st.y + st.v);
    }

    pub(crate) fn vf_lib_pdf(&mut self, st: &mut VfState, data: &[u8]) {
        self.lua_literal(crate::lua_font::PDF_SET_ORIGIN, &String::from_utf8_lossy(data), st.x + st.h, st.y + st.v);
    }
}
