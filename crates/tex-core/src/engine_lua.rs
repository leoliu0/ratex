//! LuaTeX runtime engine bridge and LuaTeX primitive integration.
//!
//! Provides the LuaTeX Lua 5.3 environment including the standard `tex`,
//! `token`, `node`, `callback`, `status`, `lua`, `texio`, and `kpse` modules.

use std::collections::VecDeque;

use tex_lua::{Lua, LuaApi, LuaResult, SafeOption, Stdlib, Value};

use crate::engine::Engine;
use crate::token::Token;

/// `tex.print` & co. without a catcode table argument read their lines
/// with the current catcode table (luatex `DEFAULT_CAT_TABLE`).
pub const DEFAULT_CAT_TABLE: i32 = -1;
/// `tex.write` lines are read with "string" catcodes: spaces are spacers,
/// everything else is other (luatex `NO_CAT_TABLE`).
pub const NO_CAT_TABLE: i32 = -2;

/// One item printed by `tex.print`/`sprint`/`write`/`cprint`/`tprint`
/// (luatex ltexlib.c `rope`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaLine {
    /// The bytes of a printed string.
    pub text: Vec<u8>,
    /// A printed token object; `text` is empty then.
    pub token: Option<Token>,
    /// `sprint`-style partial line: no `\endlinechar`, trailing spaces and
    /// the scanner state are kept.
    pub partial: bool,
    /// Catcode regime: [`DEFAULT_CAT_TABLE`], [`NO_CAT_TABLE`], a catcode
    /// table id, or `-0xFF - c` for the fixed catcode `c` (`cprint`).
    pub cattable: i32,
}

/// The lines of a Lua pseudo file still to be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LuaLines {
    pub lines: VecDeque<LuaLine>,
}

pub struct LuaEngine {
    pub lua: Lua,
}

impl LuaEngine {
    pub fn new() -> Result<Self, String> {
        let mut lua = Lua::new_lua53(SafeOption::default());
        lua.open_stdlib(Stdlib::All)
            .map_err(|e| format!("failed to open stdlib: {e:?}"))?;

        let mut engine = Self { lua };
        engine.init_modules()?;
        Ok(engine)
    }

    fn init_modules(&mut self) -> Result<(), String> {
        // `status`, `kpse`, `lfs`, `os`/`io` additions and the rest of the
        // system libraries are installed by `lua_sys` after the bridge.
        let lua_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("lua table creation failed: {e:?}"))?;
        self.lua
            .set_global("lua", lua_tbl)
            .map_err(|e| format!("failed to set lua table: {e:?}"))?;

        // 3. texio table (filled by `lua_bridge`)
        let texio_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("texio table creation failed: {e:?}"))?;
        self.lua
            .set_global("texio", texio_tbl)
            .map_err(|e| format!("failed to set texio table: {e:?}"))?;

        // 4. tex table; tex.print & co. are installed by `lua_bridge`.
        let tex_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("tex table creation failed: {e:?}"))?;

        // tex.count & co. are installed by `lua_bridge`.

        self.lua
            .set_global("tex", tex_tbl)
            .map_err(|e| format!("failed to set tex table: {e:?}"))?;

        // 8. node and node.direct (lnodelib.c)
        crate::lua_node_lib::install_library(&mut self.lua)?;
        self.lua
            .execute("if os then os.gettimeofday = function() return os.time() end end")
            .map_err(|e| format!("failed to initialize os: {e:?}"))?;
        // 9. font library (luatex lfontlib.c)
        crate::lua_font_lib::install(&mut self.lua)?;

        // 10. fontloader and luaharfbuzz (lua_font_hb.rs)
        crate::lua_font_hb::install(&mut self.lua)?;
        crate::lua_lpeg::install(&mut self.lua)?;
        crate::lua_bridge::install(&mut self.lua)?;
        crate::lua_sys::install(&mut self.lua)?;

        // 13. mplib module backed by native Rust tex-mplib
        let mplib_raw_exec_fn = self
            .lua
            .create_function(|code: String| -> LuaResult<(i64, String, String, String, String, String)> {
                let mut session = tex_mplib::MpSession::new(tex_mplib::MpConfig::default());
                let res = session.execute(&code);
                let (ps, svg, bbox, charcode, num_objs) = if let Some(fig) = res.fig.first() {
                    (
                        fig.to_postscript(),
                        fig.to_svg(),
                        fig.bounding_box,
                        fig.charcode as i64,
                        fig.objects.len() as i64,
                    )
                } else {
                    (String::new(), String::new(), (0.0, 0.0, 0.0, 0.0), 0, 0)
                };
                let meta = format!(
                    "{:.4} {:.4} {:.4} {:.4} {} {}",
                    bbox.0, bbox.1, bbox.2, bbox.3, charcode, num_objs
                );
                Ok((res.status as i64, res.log, res.term, ps, svg, meta))
            })
            .unwrap();
        self.lua.set_global("__mplib_raw_execute", mplib_raw_exec_fn).unwrap();

        self.lua.execute(r#"
            mplib = {}
            function mplib.version()
                return "3.00"
            end
            function mplib.new(params)
                local sess = { finished = false }
                function sess:execute(code)
                    if self.finished then
                        return { status = 2, log = "Session finished\n", term = "Session finished\n", fig = {} }
                    end
                    local status, log, term, ps, svg, meta = __mplib_raw_execute(code)
                    local llx, lly, urx, ury, charcode, num_objects = meta:match("([^%s]+)%s+([^%s]+)%s+([^%s]+)%s+([^%s]+)%s+([^%s]+)%s+([^%s]+)")
                    llx = tonumber(llx) or 0
                    lly = tonumber(lly) or 0
                    urx = tonumber(urx) or 0
                    ury = tonumber(ury) or 0
                    charcode = tonumber(charcode) or 0
                    num_objects = tonumber(num_objects) or 0
                    local figs = {}
                    if ps and #ps > 0 then
                        local fig = {
                            charcode = function() return charcode end,
                            boundingbox = function() return { llx, lly, urx, ury } end,
                            width = function() return math.max(0, urx - llx) end,
                            height = function() return math.max(0, ury) end,
                            depth = function() return math.max(0, -lly) end,
                            postscript = function() return ps end,
                            svg = function() return svg end,
                            objects = function()
                                local objs = {}
                                for i = 1, num_objects do
                                    objs[i] = { type = "stroke" }
                                end
                                return objs
                            end
                        }
                        figs[1] = fig
                    end
                    return {
                        status = status,
                        log = log,
                        term = term,
                        fig = figs,
                    }
                end
                function sess:finish()
                    self.finished = true
                    return { status = 0, log = "", term = "", fig = {} }
                end
                return sess
            end
        "#).map_err(|e| format!("failed to initialize mplib: {e:?}"))?;
        Ok(())
    }

    /// Run Lua source `code` as a chunk named `name`.
    pub fn execute(&mut self, code: &[u8], name: &str) -> Result<(), String> {
        let result = match std::str::from_utf8(code) {
            Ok(code) => self.lua.load(code).set_name(name).exec(),
            Err(_) => {
                let chunk = self.lua.create_bytes(code).map_err(|e| format!("{e:?}"))?;
                self.lua
                    .load("local code, name = ... return assert(load(code, name))()")
                    .call::<_, ()>((chunk, name.to_string()))
            }
        };
        result.map_err(|e| format!("Lua error: {}", self.lua.get_error_message(e).message()))
    }
}

impl Engine {
    /// luatex `lua_string_start`: what Lua printed (tex.print & co.)
    /// becomes a pseudo file that is read before anything Lua put back
    /// with `token.put_next`.
    pub(crate) fn flush_lua_output(&mut self) {
        if self.lua_print_queue.is_empty() {
            return;
        }
        let lines = LuaLines { lines: std::mem::take(&mut self.lua_print_queue).into() };
        if self.ensure_input_stack_room(1) {
            self.input.push_lua_lines(lines);
        }
    }

    /// ltexlib.c `luac_store` for a string: `cattable` is checked here as
    /// `do_luacprint` does (an invalid table means the current one).
    pub(crate) fn lua_print_text(&mut self, text: &[u8], partial: bool, cattable: i32) {
        let cattable = self.lua_print_cattable(cattable);
        self.lua_print_queue.push(LuaLine { text: text.to_vec(), token: None, partial, cattable });
    }

    /// ltexlib.c `luac_store` for a token object.
    pub(crate) fn lua_print_token(&mut self, token: Token, partial: bool, cattable: i32) {
        let cattable = self.lua_print_cattable(cattable);
        self.lua_print_queue.push(LuaLine { text: Vec::new(), token: Some(token), partial, cattable });
    }

    fn lua_print_cattable(&self, cattable: i32) -> i32 {
        if cattable >= 0 && !self.eqtb.cat_table_valid(cattable) {
            DEFAULT_CAT_TABLE
        } else {
            cattable
        }
    }

    /// luatex textoken.c `do_get_cat_code` for a Lua pseudo-file line read
    /// with catcode regime `regime` (never [`DEFAULT_CAT_TABLE`]).
    pub(crate) fn lua_line_cat_code(&self, regime: i32, character: u32) -> u8 {
        if regime == NO_CAT_TABLE {
            if character == u32::from(b' ') {
                crate::token::CAT_SPACE
            } else {
                crate::token::CAT_OTHER
            }
        } else if regime >= 0 {
            self.eqtb.cat_code_in(regime, character)
        } else {
            (-regime - 0xFF) as u8
        }
    }

    /// ltexiolib.c `texio.write`/`write_nl` for one string: `target` 1 is
    /// the log only, 2 the terminal only, anything else both. Bytes equal
    /// to `\newlinechar` end the line; `nl` starts a new line first unless
    /// the output is already at the start of one (`print_nlp`).
    pub(crate) fn lua_texio_print(&mut self, target: i64, nl: bool, bytes: &[u8]) {
        let newline = self.eqtb.int_params[crate::prim::IntParam::NewLineChar.idx() as usize];
        let text: Vec<u8> = bytes
            .iter()
            .map(|&b| if i32::from(b) == newline { b'\n' } else { b })
            .collect();
        let log_text = String::from_utf8_lossy(&text).into_owned();
        // texio.setescape: the terminal shows control characters as ^^ notation
        let text = if self.lua_tex.texio_noescape {
            log_text.clone()
        } else {
            let mut s = String::with_capacity(log_text.len());
            for c in log_text.chars() {
                if (c as u32) < 32 && c != '\t' && c != '\n' {
                    s.push_str("^^");
                    s.push(((c as u8) + 64) as char);
                } else {
                    s.push(c);
                }
            }
            s
        };
        if target != 1 {
            if nl && !self.term.is_empty() && !self.term.ends_with('\n') {
                self.append_term("\n");
            }
            self.append_term(&text);
        }
        if target != 2 {
            if nl && !self.log.is_empty() && !self.log.ends_with('\n') {
                self.append_log("\n");
            }
            self.append_log(&log_text);
        }
    }
}
