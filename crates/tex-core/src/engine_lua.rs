//! LuaTeX runtime engine bridge and LuaTeX primitive integration.
//!
//! Provides the LuaTeX Lua 5.3 environment including the standard `tex`,
//! `token`, `node`, `callback`, `status`, `lua`, `texio`, and `kpse` modules.

use std::collections::VecDeque;

use tex_lua::{Lua, LuaApi, SafeOption, Stdlib};

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

        // 13. mplib (lmplib.c) over tex-mplib
        crate::lua_mplib::install(&mut self.lua)?;
        // 14. the visible environment of a LuaTeX run
        self.lua
            .load(FINALIZE_ENVIRONMENT)
            .set_name("=[ratex environment]")
            .exec()
            .map_err(|e| format!("environment: {}", self.lua.get_error_message(e).message()))?;
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

/// What luatex's `luainit.c`/`luastuff.c` leave visible once all libraries
/// are open: `package.loaded` knows the libraries, `ffi` is a stub preload,
/// `package.loaders` is `package.searchers`, the userdata and library
/// metatables carry their `__name`, and `debug` shrinks to `traceback`.
const FINALIZE_ENVIRONMENT: &str = r##"
local debug, package, getmetatable, rawget, type, ipairs, pairs =
      debug, package, getmetatable, rawget, type, ipairs, pairs
local reg = debug.getregistry()

-- luaL_newmetatable convention for the host userdata (reported by getmetatable)
reg["luatex.token"] = {
  __name = "luatex.token",
  __eq = function(a, b) return a == b end,
  __gc = function() end,
  __index = function(t, k) return t[k] end,
  __tostring = function(t) return tostring(t) end,
}
-- the one node field the host cannot return directly: a mark's token table
local node_getfield, node_setfield = node.getfield, node.setfield
reg["luatex.node"] = {
  __name = "luatex.node",
  __eq = function(a, b) return a == b end,
  __index = function(n, k) if k == "mark" then return node_getfield(n, k) end end,
  __newindex = function(n, k, v) if k == "mark" then return node_setfield(n, k, v) end end,
  __tostring = function(n) return tostring(n) end,
}

local function named(t, name)
  local mt = type(t) == "table" and getmetatable(t) or nil
  if type(mt) == "table" then mt.__name = name end
end
named(font and font.fonts, "tex.fonts")
named(status, "tex.stats")
named(lua.bytecode, "tex.bytecode")
named(tex, "tex.meta")
for _, k in ipairs{ "attribute", "box", "catcode", "count", "delcode", "dimen", "glue", "lccode", "lists",
                    "mathcode", "muglue", "muskip", "nest", "sfcode", "skip", "toks", "uccode" } do
  named(rawget(tex, k), "tex." .. k)
end

local loaded = package.loaded
for _, k in ipairs{ "bit32", "callback", "font", "img", "kpse", "lang", "lfs", "lua", "mplib", "node", "pdf",
                    "pdfe", "pdfscanner", "status", "tex", "texio", "token", "vf" } do
  local v = rawget(_G, k)
  if v ~= nil and loaded[k] == nil then loaded[k] = v end
end
-- luainit.c: the ffi loader of a LuaTeX without ffi support
package.preload.ffi = function(...) error((...), 0) end
package.loaders = package.searchers

local keep = debug.traceback
for k in pairs(debug) do
  if k ~= "traceback" then debug[k] = nil end
end
debug.traceback = keep
"##;
