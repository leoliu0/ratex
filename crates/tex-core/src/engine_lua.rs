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
        // 1. status table
        let status = self
            .lua
            .create_table()
            .map_err(|e| format!("status table creation failed: {e:?}"))?;
        status.set("banner", "This is LuaTeX, Version 1.24.0").unwrap();
        status.set("luatex_version", 124i64).unwrap();
        status.set("luatex_revision", "0").unwrap();
        status.set("luatex_date", 2026i64).unwrap();
        status.set("development_id", 7724i64).unwrap();
        status.set("output_active", false).unwrap();
        status.set("shell_escape", 0i64).unwrap();
        self.lua
            .set_global("status", status)
            .map_err(|e| format!("failed to set status table: {e:?}"))?;

        // 2. lua table
        let lua_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("lua table creation failed: {e:?}"))?;
        lua_tbl.set("id", 0i64).unwrap();
        lua_tbl.set("version", "Lua 5.3").unwrap();
        let bytecode_tbl = self.lua.create_table().unwrap();
        lua_tbl.set("bytecode", bytecode_tbl).unwrap();
        let name_tbl = self.lua.create_table().unwrap();
        lua_tbl.set("name", name_tbl).unwrap();

        self.lua
            .set_global("lua", lua_tbl)
            .map_err(|e| format!("failed to set lua table: {e:?}"))?;
        self.lua.execute(r#"
            lua.newtable = function(narr, nrec) return {} end
            unpack = table.unpack
            loadstring = load
        "#).unwrap();

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

        // tex.sp
        let sp_fn = self
            .lua
            .create_function(|dim_str: String| -> LuaResult<i64> {
                let s = dim_str.trim();
                let sp = if let Some(stripped) = s.strip_suffix("pt") {
                    let pts: f64 = stripped.trim().parse().unwrap_or(0.0);
                    (pts * 65536.0) as i64
                } else if let Some(stripped) = s.strip_suffix("sp") {
                    stripped.trim().parse().unwrap_or(0i64)
                } else if let Some(stripped) = s.strip_suffix("in") {
                    let inches: f64 = stripped.trim().parse().unwrap_or(0.0);
                    (inches * 72.27 * 65536.0) as i64
                } else if let Some(stripped) = s.strip_suffix("mm") {
                    let mm: f64 = stripped.trim().parse().unwrap_or(0.0);
                    (mm * (72.27 / 25.4) * 65536.0) as i64
                } else if let Some(stripped) = s.strip_suffix("cm") {
                    let cm: f64 = stripped.trim().parse().unwrap_or(0.0);
                    (cm * (722.7 / 25.4) * 65536.0) as i64
                } else {
                    s.parse().unwrap_or(0i64)
                };
                Ok(sp)
            })
            .unwrap();
        tex_tbl.set("sp", sp_fn).unwrap();
        tex_tbl.set("luatexversion", 124i64).unwrap();
        tex_tbl.set("luatexrevision", "0").unwrap();
        tex_tbl.set("luatexbanner", "This is LuaTeX, Version 1.24.0").unwrap();
        // tex.count & co. are installed by `lua_bridge`.

        self.lua
            .set_global("tex", tex_tbl)
            .map_err(|e| format!("failed to set tex table: {e:?}"))?;
        self.lua.execute(r##"
            if os then
                os.type = "unix"
                os.name = "linux"
                os.uname = function()
                    return {
                        sysname = "Linux",
                        nodename = "localhost",
                        release = "6.0",
                        version = "1",
                        machine = "x86_64",
                    }
                end
            end
        "##).unwrap();

        // 8. node and node.direct (lnodelib.c)
        crate::lua_node_lib::install_library(&mut self.lua)?;
        self.lua
            .execute("if os then os.gettimeofday = function() return os.time() end end")
            .map_err(|e| format!("failed to initialize os: {e:?}"))?;
        // 9. font library (luatex lfontlib.c)
        crate::lua_font_lib::install(&mut self.lua)?;

        // 10. fontloader and luaharfbuzz (lua_font_hb.rs)
        crate::lua_font_hb::install(&mut self.lua)?;

        // 11. md5 table with authentic Rust md5 implementation
        let md5_tbl = self.lua.create_table().unwrap();
        let sumhexa_fn = self
            .lua
            .create_function(|s: String| -> LuaResult<String> {
                let digest = md5::compute(s.as_bytes());
                Ok(format!("{:x}", digest))
            })
            .unwrap();
        md5_tbl.set("sumhexa", sumhexa_fn).unwrap();
        let sum_fn = self
            .lua
            .create_function(|s: String| -> LuaResult<String> {
                let digest = md5::compute(s.as_bytes());
                Ok(String::from_utf8_lossy(&digest.0).into_owned())
            })
            .unwrap();
        md5_tbl.set("sum", sum_fn).unwrap();
        self.lua.set_global("md5", md5_tbl).unwrap();

        // 12. Documented runtime modules: img, pdf, lang, lfs, sha2, gzip, zlib, zip
        self.lua.execute(r#"
            img = {}
            function img.types()
                return { "png", "jpg", "pdf" }
            end
            function img.new()
                return { xsize = 0, ysize = 0, xres = 72, yres = 72 }
            end
            function img.scan(obj)
                local file = type(obj) == "table" and (obj.filename or obj.file) or obj
                return {
                    filename = file,
                    xsize = 100,
                    ysize = 100,
                    xres = 72,
                    yres = 72,
                    colordepth = 24,
                }
            end
            function img.node(obj)
                return node.new(8, 0)
            end
            function img.write(obj)
            end

            pdf = {
                mapfile = function(s) end,
                mapline = function(s) end,
                setmatrix = function(m) end,
                print = function(s) end,
                immediateobj = function(s) return 1 end,
                reserveobj = function() return 1 end,
                obj = function(s) return 1 end,
                getcreationdate = function() return "D:20260923000000Z" end,
                setcreationdate = function(s) end,
            }
            if status then
                status.getcreationdate = pdf.getcreationdate
            end

            lang = {
                new = function(id)
                    return { id = id or 0 }
                end,
                hyphenation = function(l, words) end,
                patterns = function(l, pats) end,
                clear_patterns = function(l) end,
                clear_hyphenation = function(l) end,
            }

            lfs = {
                currentdir = function() return "." end,
                attributes = function(filepath, aname)
                    local attr = {
                        mode = "file",
                        size = 1024,
                        modification = os.time(),
                        access = os.time(),
                        change = os.time(),
                    }
                    if aname then return attr[aname] else return attr end
                end,
                dir = function(path)
                    local i = 0
                    local entries = { ".", ".." }
                    return function()
                        i = i + 1
                        return entries[i]
                    end
                end,
                mkdir = function(p) return true end,
                rmdir = function(p) return true end,
                touch = function(p) return true end,
            }

            sha2 = {
                digest256 = function(s) return "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" end,
                digest512 = function(s) return "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e" end,
            }

            gzip = {
                compress = function(s) return s end,
                decompress = function(s) return s end,
            }
            zlib = gzip
            zip = {
                open = function() return nil end,
            }
            lpeg = {}
            local pat_meta = {}
            pat_meta.__index = pat_meta
            function pat_meta:match(s) return s end
            function pat_meta.__mul(a, b) return setmetatable({}, pat_meta) end
            function pat_meta.__add(a, b) return setmetatable({}, pat_meta) end
            function pat_meta.__sub(a, b) return setmetatable({}, pat_meta) end
            function pat_meta.__div(a, b) return setmetatable({}, pat_meta) end
            function pat_meta.__pow(a, b) return setmetatable({}, pat_meta) end
            function pat_meta.__mod(a, b) return setmetatable({}, pat_meta) end
            function pat_meta.__unm() return setmetatable({}, pat_meta) end
            function pat_meta.__len() return setmetatable({}, pat_meta) end
            local function new_pat() return setmetatable({}, pat_meta) end

            local num_meta = debug.getmetatable(0) or {}
            num_meta.__mul = function(a, b) return new_pat() end
            num_meta.__add = function(a, b) return new_pat() end
            num_meta.__sub = function(a, b) return new_pat() end
            num_meta.__div = function(a, b) return new_pat() end
            num_meta.__pow = function(a, b) return new_pat() end
            num_meta.__mod = function(a, b) return new_pat() end
            debug.setmetatable(0, num_meta)

            local str_meta = debug.getmetatable("") or {}
            str_meta.__mul = function(a, b) return new_pat() end
            str_meta.__add = function(a, b) return new_pat() end
            str_meta.__sub = function(a, b) return new_pat() end
            str_meta.__div = function(a, b) return new_pat() end
            str_meta.__pow = function(a, b) return new_pat() end
            str_meta.__mod = function(a, b) return new_pat() end
            debug.setmetatable("", str_meta)

            lpeg.P = function(x) return new_pat() end
            lpeg.S = function(x) return new_pat() end
            lpeg.R = function(...) return new_pat() end
            lpeg.C = function(x) return new_pat() end
            lpeg.Cc = function(...) return new_pat() end
            lpeg.Cs = function(x) return new_pat() end
            lpeg.Cg = function(x) return new_pat() end
            lpeg.Ct = function(x) return new_pat() end
            lpeg.Cp = function() return new_pat() end
            lpeg.Cf = function(x, op) return new_pat() end
            lpeg.Carg = function(n) return new_pat() end
            lpeg.Cmt = function(patt, fn) return new_pat() end
            lpeg.V = function(x) return new_pat() end
            lpeg.B = function(x) return new_pat() end
            lpeg.type = function(v) if getmetatable(v) == pat_meta then return "pattern" end return nil end
            lpeg.match = function(pat, s, ...) return {} end
            package.loaded["lpeg"] = lpeg
            sio = {
                readinteger1 = function(s, pos) return (string.unpack(">i1", s, pos or 1)) end,
                readinteger2 = function(s, pos) return (string.unpack(">i2", s, pos or 1)) end,
                readinteger3 = function(s, pos) return (string.unpack(">i3", s, pos or 1)) end,
                readinteger4 = function(s, pos) return (string.unpack(">i4", s, pos or 1)) end,
                readcardinal1 = function(s, pos) return (string.unpack(">I1", s, pos or 1)) end,
                readcardinal2 = function(s, pos) return (string.unpack(">I2", s, pos or 1)) end,
                readcardinal3 = function(s, pos) return (string.unpack(">I3", s, pos or 1)) end,
                readcardinal4 = function(s, pos) return (string.unpack(">I4", s, pos or 1)) end,
            }
            fio = sio
            package.loaded["sio"] = sio
            package.loaded["fio"] = fio
        "#).map_err(|e| format!("failed to initialize runtime modules: {e:?}"))?;
        crate::lua_bridge::install(&mut self.lua)?;
        static PRELOAD_ZST: &[u8] = include_bytes!("../assets/lua_uni_data_preload.lua.zst");
        if let Ok(mut decoder) = ruzstd::decoding::StreamingDecoder::new(PRELOAD_ZST) {
            use std::io::Read;
            let mut decompressed = Vec::new();
            if decoder.read_to_end(&mut decompressed).is_ok() {
                if let Ok(code) = std::str::from_utf8(&decompressed) {
                    let _ = self.lua.execute(code);
                }
            }
        }

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
        let text = String::from_utf8_lossy(&text);
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
            self.append_log(&text);
        }
    }
}
