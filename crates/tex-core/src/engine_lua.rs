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

        // 8. node table and node.direct
        let node_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("node table creation failed: {e:?}"))?;
        let direct_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("node.direct table creation failed: {e:?}"))?;

        let types_tbl = self.lua.create_table().unwrap();
        types_tbl.set(0i64, "hlist").unwrap();
        types_tbl.set(1i64, "vlist").unwrap();
        types_tbl.set(2i64, "rule").unwrap();
        types_tbl.set(3i64, "ins").unwrap();
        types_tbl.set(4i64, "mark").unwrap();
        types_tbl.set(5i64, "adjust").unwrap();
        types_tbl.set(7i64, "disc").unwrap();
        types_tbl.set(8i64, "whatsit").unwrap();
        types_tbl.set(10i64, "glue").unwrap();
        types_tbl.set(11i64, "kern").unwrap();
        types_tbl.set(12i64, "penalty").unwrap();
        types_tbl.set(29i64, "glyph").unwrap();
        node_tbl.set("types", types_tbl.clone()).unwrap();
        direct_tbl.set("types", types_tbl).unwrap();

        let id_fn = self
            .lua
            .create_function(|name: String| -> LuaResult<Option<i64>> {
                let id = match name.as_str() {
                    "hlist" => 0,
                    "vlist" => 1,
                    "rule" => 2,
                    "ins" => 3,
                    "mark" => 4,
                    "adjust" => 5,
                    "disc" => 7,
                    "whatsit" => 8,
                    "glue" => 10,
                    "kern" => 11,
                    "penalty" => 12,
                    "glyph" => 29,
                    _ => return Ok(None),
                };
                Ok(Some(id))
            })
            .unwrap();
        node_tbl.set("id", id_fn.clone()).unwrap();
        direct_tbl.set("id", id_fn).unwrap();

        let type_fn = self
            .lua
            .create_function(|id_or_node: Value| -> LuaResult<Option<String>> {
                let id = if let Some(i) = id_or_node.as_integer() {
                    i
                } else {
                    -1
                };
                let name = match id {
                    0 => "hlist",
                    1 => "vlist",
                    2 => "rule",
                    3 => "ins",
                    4 => "mark",
                    5 => "adjust",
                    7 => "disc",
                    8 => "whatsit",
                    10 => "glue",
                    11 => "kern",
                    12 => "penalty",
                    29 => "glyph",
                    _ => return Ok(None),
                };
                Ok(Some(name.to_string()))
            })
            .unwrap();
        node_tbl.set("type", type_fn.clone()).unwrap();
        direct_tbl.set("type", type_fn).unwrap();

        // node.has_attribute / set_attribute / get_attribute
        let has_attr_fn = self
            .lua
            .create_function(|_node: Value, _id: i64| -> LuaResult<Option<i64>> {
                Ok(None)
            })
            .unwrap();
        node_tbl.set("has_attribute", has_attr_fn.clone()).unwrap();
        node_tbl.set("get_attribute", has_attr_fn.clone()).unwrap();
        direct_tbl.set("has_attribute", has_attr_fn.clone()).unwrap();
        direct_tbl.set("get_attribute", has_attr_fn).unwrap();

        let set_attr_fn = self
            .lua
            .create_function(|_node: Value, _id: i64, _val: Option<i64>| -> LuaResult<()> {
                Ok(())
            })
            .unwrap();
        node_tbl.set("set_attribute", set_attr_fn.clone()).unwrap();
        direct_tbl.set("set_attribute", set_attr_fn).unwrap();

        // node.dimensions
        let dimensions_fn = self
            .lua
            .create_function(|_node: Value| -> LuaResult<(i64, i64, i64)> {
                Ok((0, 0, 0))
            })
            .unwrap();
        node_tbl.set("dimensions", dimensions_fn.clone()).unwrap();
        direct_tbl.set("dimensions", dimensions_fn).unwrap();

        node_tbl.set("direct", direct_tbl.clone()).unwrap();
        self.lua
            .set_global("node", node_tbl)
            .map_err(|e| format!("failed to set node table: {e:?}"))?;
        self.lua.execute(r#"
            node.flush_list = function(n) end
            node.free = function(n) end
            node.mlist_to_hlist = function(head, display_type, need_penalties) return head end
            node.subtype = function(name) return 1 end
            node.subtypes = function(id) return {} end
            node.direct.mlist_to_hlist = node.mlist_to_hlist
            node.direct.flush_list = node.flush_list
            node.direct.free = node.free
            node.direct.subtype = node.subtype
            node.direct.subtypes = node.subtypes
            node.direct.new = function(id, subtype)
                return { id = id, subtype = subtype }
            end
            node.direct.setfield = function(n, field, val)
                if type(n) == "table" then n[field] = val end
            end
            node.direct.getfield = function(n, field)
                if type(n) == "table" then return n[field] end
                return nil
            end
            node.direct.setwhatsitfield = node.direct.setfield
            node.direct.getwhatsitfield = node.direct.getfield
            node.direct.write = function(n) end
            node.write = function(n) end
        "#).unwrap();
        // 9. font table
        self.lua.execute(r#"
            local __font_store = {}
            font = {
                define = function(n, tbl)
                    __font_store[n] = tbl
                    return n
                end,
                getfont = function(n)
                    return __font_store[n]
                end,
                setfont = function(n, tbl)
                    __font_store[n] = tbl
                end,
                id = function(name)
                    return 0
                end,
            }
        "#).unwrap();

        // 10. Native fontloader and luaharfbuzz modules via Lua script
        self.lua.execute(r#"
            fontloader = {}
            function fontloader.info(filename)
                local resolved = kpse.find_file(filename) or filename
                local name = filename:match("([^/]+)$") or filename
                name = name:gsub("%.%a+$", "")
                return {
                    fontname = name,
                    familyname = name,
                    fullname = name,
                    units_per_em = 1000,
                    version = "1.0",
                }
            end
            function fontloader.open(filename)
                local info = fontloader.info(filename)
                return {
                    fontname = info.fontname,
                    fullname = info.fullname,
                    familyname = info.familyname,
                    units_per_em = 1000,
                    ascent = 800,
                    descent = -200,
                    glyphcnt = 256,
                    glyphs = {},
                }
            end
            function fontloader.close(font)
                return true
            end

            luaharfbuzz = {
                version = function() return "14.4.0" end
            }
            local Buffer = {}
            Buffer.__index = Buffer
            function Buffer.new()
                return setmetatable({ text = "", glyphs = {} }, Buffer)
            end
            function Buffer:add_utf8(text)
                self.text = self.text .. text
            end
            function Buffer:guess_segment_properties()
            end
            function Buffer:set_direction(dir)
            end
            function Buffer:set_script(script)
            end
            function Buffer:set_language(lang)
            end
            function Buffer:get_glyph_infos_and_positions()
                local res = {}
                for i = 1, #self.text do
                    table.insert(res, {
                        codepoint = string.byte(self.text, i),
                        cluster = i - 1,
                        x_advance = 655360,
                        y_advance = 0,
                        x_offset = 0,
                        y_offset = 0,
                    })
                end
                return res
            end
            luaharfbuzz.Buffer = Buffer

            local Face = {}
            Face.__index = Face
            function Face.new(path)
                return setmetatable({ path = path }, Face)
            end
            luaharfbuzz.Face = Face

            local Font = {}
            Font.__index = Font
            function Font.new(face)
                return setmetatable({ face = face }, Font)
            end
            function Font:shape(buffer)
            end
            luaharfbuzz.Font = Font
            package.loaded["luaharfbuzz"] = luaharfbuzz
            package.loaded["fontloader"] = fontloader
        "#).map_err(|e| format!("failed to initialize fontloader/luaharfbuzz: {e:?}"))?;
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
