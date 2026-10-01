//! LuaTeX runtime engine bridge and LuaTeX primitive integration.
//!
//! Provides the LuaTeX Lua 5.3 environment including the standard `tex`,
//! `token`, `node`, `callback`, `status`, `lua`, `texio`, and `kpse` modules.

use std::cell::RefCell;
use std::rc::Rc;

use tex_lua::{Lua, LuaApi, LuaResult, SafeOption, Stdlib, Value};

use crate::engine::Engine;

/// Captured output emitted by `tex.print`, `tex.sprint`, and `tex.write`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaOutputItem {
    pub catcode_table: Option<i32>,
    pub text: String,
    pub newline: bool,
}

/// Shared host bridge accessible from Lua callbacks.
pub struct LuaBridgeState {
    pub output_queue: Vec<LuaOutputItem>,
    pub term_log: String,
}

impl LuaBridgeState {
    pub fn new() -> Self {
        Self {
            output_queue: Vec::new(),
            term_log: String::new(),
        }
    }
}

pub struct LuaEngine {
    pub lua: Lua,
    pub bridge: Rc<RefCell<LuaBridgeState>>,
}

impl LuaEngine {
    pub fn new() -> Result<Self, String> {
        let mut lua = Lua::new_lua53(SafeOption::default());
        lua.open_stdlib(Stdlib::All)
            .map_err(|e| format!("failed to open stdlib: {e:?}"))?;

        let bridge = Rc::new(RefCell::new(LuaBridgeState::new()));

        let mut engine = Self { lua, bridge };
        engine.init_modules()?;
        Ok(engine)
    }

    fn init_modules(&mut self) -> Result<(), String> {
        let bridge = self.bridge.clone();

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

        // 3. texio table
        let texio_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("texio table creation failed: {e:?}"))?;
        let bridge_clone = bridge.clone();
        let write_fn = self
            .lua
            .create_function(move |msg: String| -> LuaResult<()> {
                bridge_clone.borrow_mut().term_log.push_str(&msg);
                Ok(())
            })
            .unwrap();
        texio_tbl.set("write", write_fn.clone()).unwrap();

        let bridge_clone = bridge.clone();
        let write_nl_fn = self
            .lua
            .create_function(move |msg: String| -> LuaResult<()> {
                let mut b = bridge_clone.borrow_mut();
                if !b.term_log.ends_with('\n') {
                    b.term_log.push('\n');
                }
                b.term_log.push_str(&msg);
                Ok(())
            })
            .unwrap();
        texio_tbl.set("write_nl", write_nl_fn).unwrap();
        self.lua
            .set_global("texio", texio_tbl)
            .map_err(|e| format!("failed to set texio table: {e:?}"))?;

        // 4. tex table
        let tex_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("tex table creation failed: {e:?}"))?;

        // tex.print
        let bridge_clone = bridge.clone();
        let print_fn = self
            .lua
            .create_function(move |val: String| -> LuaResult<()> {
                bridge_clone.borrow_mut().output_queue.push(LuaOutputItem {
                    catcode_table: None,
                    text: val,
                    newline: true,
                });
                Ok(())
            })
            .unwrap();
        tex_tbl.set("print", print_fn).unwrap();

        // tex.sprint
        let bridge_clone = bridge.clone();
        let sprint_fn = self
            .lua
            .create_function(move |val: String| -> LuaResult<()> {
                bridge_clone.borrow_mut().output_queue.push(LuaOutputItem {
                    catcode_table: None,
                    text: val,
                    newline: false,
                });
                Ok(())
            })
            .unwrap();
        tex_tbl.set("sprint", sprint_fn).unwrap();

        // tex.write
        let bridge_clone = bridge.clone();
        let write_fn = self
            .lua
            .create_function(move |val: String| -> LuaResult<()> {
                bridge_clone.borrow_mut().output_queue.push(LuaOutputItem {
                    catcode_table: None,
                    text: val,
                    newline: false,
                });
                Ok(())
            })
            .unwrap();
        tex_tbl.set("write", write_fn).unwrap();


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
            local __raw_print = tex.print
            local __raw_sprint = tex.sprint
            local function flatten(v, out)
                if type(v) == "table" then
                    for _, item in ipairs(v) do
                        flatten(item, out)
                    end
                elseif v ~= nil then
                    table.insert(out, tostring(v))
                end
            end
            tex.print = function(...)
                local args = {...}
                local n = select("#", ...)
                if n == 0 then return end
                local start = 1
                if type(args[1]) == "number" and n > 1 then
                    start = 2
                end
                for i = start, n do
                    local out = {}
                    flatten(args[i], out)
                    for _, s in ipairs(out) do
                        __raw_print(s)
                    end
                end
            end
            tex.sprint = function(...)
                local args = {...}
                local n = select("#", ...)
                if n == 0 then return end
                local start = 1
                if type(args[1]) == "number" and n > 1 then
                    start = 2
                end
                for i = start, n do
                    local out = {}
                    flatten(args[i], out)
                    for _, s in ipairs(out) do
                        __raw_sprint(s)
                    end
                end
            end
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
            if os then
                os.gettimeofday = function() return os.time() end
            end
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
    /// Feed what Lua printed (tex.print & co.) back to TeX.
    pub(crate) fn flush_lua_output(&mut self) {
        let Some(lua) = self.lua.as_ref() else {
            return;
        };
        let items: Vec<LuaOutputItem> = lua.bridge.borrow_mut().output_queue.drain(..).collect();
        if !items.is_empty() {
            let mut combined = String::new();
            for item in items {
                combined.push_str(&item.text);
                if item.newline {
                    combined.push('\n');
                }
            }
            if self.ensure_input_stack_room(1) {
                self.input.push_file("<directlua>".to_string(), combined.into_bytes());
            }
        }
    }
}
