//! LuaTeX runtime engine bridge and LuaTeX primitive integration.
//!
//! Provides the LuaTeX Lua 5.3 environment including the standard `tex`,
//! `token`, `node`, `callback`, `status`, `lua`, `texio`, and `kpse` modules.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tex_lua::{Lua, LuaApi, LuaResult, LuaValue, SafeOption, Stdlib};

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
    pub callbacks: HashMap<String, i64>,
    pub function_table: HashMap<i64, i64>,
    pub bytecode_table: HashMap<i64, Vec<u8>>,
    pub saved_counts: HashMap<i32, i32>,
    pub saved_dimens: HashMap<i32, i32>,
    pub saved_toks: HashMap<i32, String>,
    pub saved_named_counts: HashMap<String, i32>,
    pub requested_primitives: Vec<(String, Vec<String>)>,
}

impl LuaBridgeState {
    pub fn new() -> Self {
        Self {
            output_queue: Vec::new(),
            term_log: String::new(),
            callbacks: HashMap::new(),
            function_table: HashMap::new(),
            bytecode_table: HashMap::new(),
            saved_counts: HashMap::new(),
            saved_dimens: HashMap::new(),
            saved_toks: HashMap::new(),
            saved_named_counts: HashMap::new(),
            requested_primitives: Vec::new(),
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
            local __functions_table = {}
            lua.get_functions_table = function() return __functions_table end
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

        // tex.enableprimitives
        let bridge_clone = self.bridge.clone();
        let enable_fn = self
            .lua
            .create_function(move |prefix: String, table: LuaValue| -> LuaResult<()> {
                let mut prims = Vec::new();
                if let Some(tbl) = table.as_table() {
                    for i in 1..=tbl.len() {
                        if let Some(val) = tbl.raw_geti(i as i64) {
                            if let Some(s) = val.as_str() {
                                prims.push(s.to_string());
                            }
                        }
                    }
                }
                bridge_clone.borrow_mut().requested_primitives.push((prefix, prims));
                Ok(())
            })
            .unwrap();
        tex_tbl.set("enableprimitives", enable_fn).unwrap();

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
        // tex.hashtokens defined in Lua below

        // Register tables (count, dimen, toks)
        let count_tbl = self.lua.create_table().unwrap();
        let count_meta = self.lua.create_table().unwrap();
        let c_clone = bridge.clone();
        let count_index = self
            .lua
            .create_function(move |_tbl: LuaValue, key: LuaValue| -> LuaResult<i64> {
                let b = c_clone.borrow();
                if let Some(idx) = key.as_integer() {
                    let val = b.saved_counts.get(&(idx as i32)).copied().unwrap_or(0);
                    Ok(val as i64)
                } else if let Some(s) = key.as_str() {
                    let val = b.saved_named_counts.get(s).copied().unwrap_or(0);
                    Ok(val as i64)
                } else {
                    Ok(0)
                }
            })
            .unwrap();
        count_meta.set("__index", count_index).unwrap();

        let c_clone = bridge.clone();
        let count_newindex = self
            .lua
            .create_function(move |_tbl: LuaValue, key: LuaValue, val: i64| -> LuaResult<()> {
                let mut b = c_clone.borrow_mut();
                if let Some(idx) = key.as_integer() {
                    b.saved_counts.insert(idx as i32, val as i32);
                } else if let Some(s) = key.as_str() {
                    b.saved_named_counts.insert(s.to_string(), val as i32);
                }
                Ok(())
            })
            .unwrap();
        count_meta.set("__newindex", count_newindex).unwrap();
        count_tbl.set_metatable(Some(&count_meta)).unwrap();
        tex_tbl.set("count", count_tbl).unwrap();

        let dimen_tbl = self.lua.create_table().unwrap();
        let dimen_meta = self.lua.create_table().unwrap();
        let d_clone = bridge.clone();
        let dimen_index = self
            .lua
            .create_function(move |_tbl: LuaValue, idx: i64| -> LuaResult<i64> {
                let b = d_clone.borrow();
                let val = b.saved_dimens.get(&(idx as i32)).copied().unwrap_or(0);
                Ok(val as i64)
            })
            .unwrap();
        dimen_meta.set("__index", dimen_index).unwrap();

        let d_clone = bridge.clone();
        let dimen_newindex = self
            .lua
            .create_function(move |_tbl: LuaValue, idx: i64, val: i64| -> LuaResult<()> {
                d_clone.borrow_mut().saved_dimens.insert(idx as i32, val as i32);
                Ok(())
            })
            .unwrap();
        dimen_meta.set("__newindex", dimen_newindex).unwrap();
        dimen_tbl.set_metatable(Some(&dimen_meta)).unwrap();
        tex_tbl.set("dimen", dimen_tbl).unwrap();

        let toks_tbl = self.lua.create_table().unwrap();
        let toks_meta = self.lua.create_table().unwrap();
        let t_clone = bridge.clone();
        let toks_index = self
            .lua
            .create_function(move |_tbl: LuaValue, idx: i64| -> LuaResult<String> {
                let b = t_clone.borrow();
                let val = b.saved_toks.get(&(idx as i32)).cloned().unwrap_or_default();
                Ok(val)
            })
            .unwrap();
        toks_meta.set("__index", toks_index).unwrap();

        let t_clone = bridge.clone();
        let toks_newindex = self
            .lua
            .create_function(move |_tbl: LuaValue, idx: i64, val: String| -> LuaResult<()> {
                t_clone.borrow_mut().saved_toks.insert(idx as i32, val);
                Ok(())
            })
            .unwrap();
        toks_meta.set("__newindex", toks_newindex).unwrap();
        toks_tbl.set_metatable(Some(&toks_meta)).unwrap();
        tex_tbl.set("toks", toks_tbl).unwrap();

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
            tex.setcount = function(scope, name, val)
                if val == nil then
                    val = name
                    name = scope
                end
                tex.count[name] = val
            end
            tex.getcount = function(name)
                return tex.count[name] or 0
            end
            tex.inputlineno = 1
            tex.hashtokens = function() return {} end
        "##).unwrap();
        // 5. kpse table
        let kpse_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("kpse table creation failed: {e:?}"))?;
        let find_file_fn = self
            .lua
            .create_function(|name: String, _fmt: Option<String>| -> LuaResult<Option<String>> {
                let resolver = tex_kpse::Kpse::new();
                if let Some(path) = resolver.find_any(&name) {
                    Ok(Some(path.to_string_lossy().into_owned()))
                } else {
                    Ok(None)
                }
            })
            .unwrap();
        kpse_tbl.set("find_file", find_file_fn.clone()).unwrap();
        kpse_tbl.set("lookup", find_file_fn).unwrap();
        let version_fn = self
            .lua
            .create_function(|| -> LuaResult<String> {
                Ok("kpathsea version 6.3.2".to_string())
            })
            .unwrap();
        kpse_tbl.set("version", version_fn).unwrap();
        self.lua
            .set_global("kpse", kpse_tbl)
            .map_err(|e| format!("failed to set kpse table: {e:?}"))?;
        self.lua.execute(r#"
            if package and package.searchers then
                table.insert(package.searchers, 2, function(name)
                    local filename = name:gsub("%.", "/") .. ".lua"
                    local found = kpse.find_file(filename, "lua")
                        or kpse.find_file(name .. ".lua", "lua")
                        or kpse.find_file(name, "lua")
                    if found then
                        local chunk, err = loadfile(found, "t", _G)
                        if chunk then
                            return chunk
                        else
                            error("error loading module '" .. name .. "' from file '" .. found .. "':\n\t" .. tostring(err))
                        end
                    end
                    return "\n\t[kpse] no file '" .. filename .. "' in kpathsea database"
                end)
            end
        "#).map_err(|e| format!("failed to register kpse package searcher: {e:?}"))?;

        // 6. token table
        let token_tbl = self
            .lua
            .create_table()
            .map_err(|e| format!("token table creation failed: {e:?}"))?;
        let is_token_fn = self
            .lua
            .create_function(|val: LuaValue| -> LuaResult<bool> {
                Ok(val.is_table() || val.is_userdata())
            })
            .unwrap();
        token_tbl.set("is_token", is_token_fn).unwrap();
        // token.create defined in Lua below
        self.lua
            .set_global("token", token_tbl)
            .map_err(|e| format!("failed to set token table: {e:?}"))?;
        self.lua.execute(r#"
            local cmd_map = {
                undefined_cs = 0,
                char_given = 1,
                math_given = 2,
                get_font = 3,
                letter = 11,
                other_char = 12,
                left_brace = 1,
                right_brace = 2,
                math_shift = 3,
                spacer = 10,
            }
            local cmd_counter = 200
            setmetatable(cmd_map, {
                __index = function(t, k)
                    cmd_counter = cmd_counter + 1
                    t[k] = cmd_counter
                    return cmd_counter
                end
            })
            token.command_id = function(name)
                return cmd_map[name] or 0
            end
            token.commands = function()
                return cmd_map
            end
            token.create = function(val, cmd)
                return {
                    id = 0,
                    tok = 0,
                    mode = 0,
                    cmdname = "undefined_cs",
                    command = cmd or cmd_map.undefined_cs,
                    index = 0,
                }
            end
            token.new = token.create
            token.set_lua = function(name, id, ...) end
            token.setlua = token.set_lua
            token.biggest_char = function() return 1114111 end
            token.get_mode = function(tok) return (type(tok) == "table" and tok.mode) or 0 end
            token.get_index = function(tok) return (type(tok) == "table" and tok.index) or 0 end
            token.is_defined = function(s, b) return true end
            token.set_char = function(s, n) end
            token.put_next = function(...) end
            token.putnext = token.put_next
            token.scan_string = function() return "" end
            token.scan_int = function() return 0 end
            token.scan_csname = function() return "" end
            token.scan_keyword = function() return false end
            token.scan_argument = function() return "" end
            token.get_next = function() return token.create("") end
            token.get_macro = function(s) return "" end
            token.set_macro = function(s, v) end
            tex.chardef = token.set_char
            tex.runtoks = function(fn) if type(fn) == "function" then fn() end end
        "#).unwrap();
        // 7. callback table
        self.lua.execute(r#"
            local __callbacks = {}
            callback = {
                register = function(name, func)
                    if func == nil or func == false then
                        __callbacks[name] = nil
                    else
                        __callbacks[name] = func
                    end
                end,
                find = function(name)
                    return __callbacks[name]
                end,
                list = function()
                    local t = {}
                    for k, v in pairs(__callbacks) do t[k] = true end
                    return t
                end,
            }
        "#).unwrap();

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
            .create_function(|id_or_node: LuaValue| -> LuaResult<Option<String>> {
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
            .create_function(|_node: LuaValue, _id: i64| -> LuaResult<Option<i64>> {
                Ok(None)
            })
            .unwrap();
        node_tbl.set("has_attribute", has_attr_fn.clone()).unwrap();
        node_tbl.set("get_attribute", has_attr_fn.clone()).unwrap();
        direct_tbl.set("has_attribute", has_attr_fn.clone()).unwrap();
        direct_tbl.set("get_attribute", has_attr_fn).unwrap();

        let set_attr_fn = self
            .lua
            .create_function(|_node: LuaValue, _id: i64, _val: Option<i64>| -> LuaResult<()> {
                Ok(())
            })
            .unwrap();
        node_tbl.set("set_attribute", set_attr_fn.clone()).unwrap();
        direct_tbl.set("set_attribute", set_attr_fn).unwrap();

        // node.dimensions
        let dimensions_fn = self
            .lua
            .create_function(|_node: LuaValue| -> LuaResult<(i64, i64, i64)> {
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

            luatexbase = {
                add_to_callback = function(...) end,
                remove_from_callback = function(...) end,
                create_callback = function(...) end,
                call_callback = function(...) end,
                attributes = {},
                registernumber = function(...) return 0 end,
                new_attribute = function(...) return 0 end,
                new_whatsit = function(...) return 0 end,
                new_user_whatsit = function(...) return 0 end,
                provides_module = function(...) end,
            }
            package.loaded["luatexbase"] = luatexbase
            package.loaded["ltluatex"] = luatexbase
            package.loaded["lualatexquotejobname"] = {}
            package.loaded["lualatexquotejobname.lua"] = {}
            package.loaded["l3backend-luatex"] = {}
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

    /// Execute Lua code string and return any emitted TeX tokens/text.
    pub fn execute(&mut self, code: &str) -> Result<Vec<LuaOutputItem>, String> {
        self.bridge.borrow_mut().output_queue.clear();
        if let Err(e) = self.lua.execute(code) {
            let full = self.lua.get_error_message(e);
            return Err(format!("Lua error: {}", full.message()));
        }
        let output = self.bridge.borrow_mut().output_queue.drain(..).collect();
        Ok(output)
    }

    /// Sync register values from the TeX engine to the Lua environment.
    pub fn sync_from_engine(&mut self, eng: &Engine) {
        let mut b = self.bridge.borrow_mut();
        for i in 0..1024 {
            let val = eng.eqtb.count.get(i).copied().unwrap_or(0);
            b.saved_counts.insert(i as i32, val);
        }
        for i in 0..1024 {
            let val = eng.eqtb.dimen.get(i).copied().unwrap_or(0);
            b.saved_dimens.insert(i as i32, val);
        }
        for id in eng.cs.all_ids() {
            if let Some(crate::eqtb::Equiv::CountReg(idx)) = eng.eqtb.resolve(id) {
                let name = String::from_utf8_lossy(eng.cs.name(id)).into_owned();
                let val = eng.eqtb.count.get(*idx as usize).copied().unwrap_or(0);
                b.saved_named_counts.insert(name, val);
            }
        }
        let line = eng.input.current_file_line() as i64;
        let _ = self.lua.execute(&format!("if tex then tex.inputlineno = {line} end"));
    }

    /// Sync changed register values back to the TeX engine.
    pub fn sync_to_engine(&self, eng: &mut Engine) {
        let mut b = self.bridge.borrow_mut();
        for (&idx, &val) in &b.saved_counts {
            if let Some(slot) = eng.eqtb.count.get_mut(idx as usize) {
                *slot = val;
            }
        }
        for (&idx, &val) in &b.saved_dimens {
            if let Some(slot) = eng.eqtb.dimen.get_mut(idx as usize) {
                *slot = val;
            }
        }
        for (name, &val) in &b.saved_named_counts {
            let id = eng.cs.intern(name.as_bytes());
            let count_idx = match eng.eqtb.resolve(id) {
                Some(crate::eqtb::Equiv::CountReg(idx)) => Some(*idx),
                _ => None,
            };
            if let Some(idx) = count_idx {
                if let Some(slot) = eng.eqtb.count.get_mut(idx as usize) {
                    *slot = val;
                }
            }
        }
        let requested = std::mem::take(&mut b.requested_primitives);
        for (prefix, prims) in requested {
            if prims.is_empty() {
                for id in eng.cs.all_ids() {
                    let name = eng.cs.name(id).to_vec();
                    if let Some(equiv) = eng.eqtb.get(id).cloned() {
                        let mut new_name = prefix.as_bytes().to_vec();
                        new_name.extend_from_slice(&name);
                        let new_id = eng.cs.intern(&new_name);
                        eng.eqtb.assign(new_id, equiv, true);
                    }
                }
            } else {
                for prim_name in prims {
                    let orig_id = eng.cs.lookup(prim_name.as_bytes()).unwrap_or_else(|| eng.cs.intern(prim_name.as_bytes()));
                    if let Some(equiv) = eng.eqtb.get(orig_id).cloned() {
                        let mut new_name = prefix.as_bytes().to_vec();
                        new_name.extend_from_slice(prim_name.as_bytes());
                        let new_id = eng.cs.intern(&new_name);
                        eng.eqtb.assign(new_id, equiv, true);
                    }
                }
            }
        }
    }
}
impl Engine {
    /// Registers all LuaTeX primitives when `engine_kind == EngineKind::LuaTeX`.
    pub fn init_luatex_primitives(&mut self) {
        if self.engine_kind != crate::engine::EngineKind::LuaTeX {
            return;
        }

        let def = |name: &'static [u8], p: crate::prim::Prim, e: &mut Engine| {
            let id = e.cs.intern(name);
            e.eqtb.assign(id, crate::eqtb::Equiv::Prim(p), false);
        };

        def(b"luatexversion", crate::prim::Prim::LuaTeXVersion, self);
        def(b"luatexrevision", crate::prim::Prim::LuaTeXRevision, self);
        def(b"luatexbanner", crate::prim::Prim::LuaTeXBanner, self);
        def(b"outputmode", crate::prim::Prim::OutputMode, self);
        def(b"directlua", crate::prim::Prim::DirectLua, self);
        def(b"tex_luatexversion:D", crate::prim::Prim::LuaTeXVersion, self);
        def(b"tex_directlua:D", crate::prim::Prim::DirectLua, self);
        def(b"catcodetable", crate::prim::Prim::CatCodeTable, self);
        def(b"initcatcodetable", crate::prim::Prim::InitCatCodeTable, self);
        def(b"savecatcodetable", crate::prim::Prim::SaveCatCodeTable, self);
        def(b"attribute", crate::prim::Prim::Attribute, self);
        def(b"attributedef", crate::prim::Prim::AttributeDef, self);
        def(b"Ustack", crate::prim::Prim::Ustack, self);
        def(b"Umathfractiondelsize", crate::prim::Prim::Umathfractiondelsize, self);
        def(b"Umathstacknumup", crate::prim::Prim::Umathstacknumup, self);
        def(b"Umathstackdenomdown", crate::prim::Prim::Umathstackdenomdown, self);
        def(b"Umathstackvgap", crate::prim::Prim::Umathstackvgap, self);
        def(b"Ustartmath", crate::prim::Prim::Ustartmath, self);
        def(b"Ustopmath", crate::prim::Prim::Ustopmath, self);
    }
}
