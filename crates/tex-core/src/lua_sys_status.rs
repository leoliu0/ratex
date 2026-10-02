//! `status` (`lstatslib.c`), `texconfig`, the `lua` table and `arg`.

use std::cell::Cell;

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString};

use crate::lua_bridge::with_engine;
use crate::lua_sys::{safer_option, shell_escape, sys_reg, ShellEscape};

pub(crate) const PRELUDE: &str = include_str!("lua_sys_status.lua");

thread_local! {
    static RESET_AT: Cell<usize> = const { Cell::new(0) };
    static EXIT_CODE: Cell<Option<i32>> = const { Cell::new(None) };
}

/// `status.luatex_engine`.
pub(crate) fn engine_name() -> &'static str {
    crate::engine_mode::EngineKind::LuaTeX.program_name()
}

/// The exit code set by `status.setexitcode`, if any (LuaTeX's
/// `defaultexitcode`, used when the run itself succeeded).
pub fn default_exit_code() -> Option<i32> {
    EXIT_CODE.with(Cell::get)
}

fn locale(var: &str) -> String {
    for key in ["LC_ALL", var, "LANG"] {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    "C".to_string()
}

enum Field {
    Int(i64),
    Str(String),
    Bool(bool),
    Nil,
}

/// Names of `status.list()`, in `lstatslib.c` order.
const NAMES: &[&str] = &[
    "output_active", "best_page_break", "filename", "inputid", "linenumber", "lasterrorstring",
    "lastluaerrorstring", "lastwarningtag", "lastwarningstring", "lasterrorcontext", "pdf_gone", "pdf_ptr",
    "dvi_gone", "dvi_ptr", "total_pages", "output_file_name", "log_name", "banner", "luatex_version",
    "luatex_revision", "development_id", "luatex_hashtype", "luatex_hashchars", "luatex_engine", "ini_version",
    "shell_escape", "safer_option", "luadebug_option", "kpse_used", "output_directory", "var_used", "dyn_used",
    "str_ptr", "init_str_ptr", "max_strings", "pool_ptr", "init_pool_ptr", "pool_size", "var_mem_max",
    "node_mem_usage", "fix_mem_max", "fix_mem_min", "fix_mem_end", "cs_count", "hash_size", "hash_extra",
    "font_ptr", "max_in_stack", "max_nest_stack", "max_param_stack", "max_buf_stack", "max_save_stack",
    "stack_size", "nest_size", "param_size", "buf_size", "save_size", "input_ptr", "obj_ptr", "obj_tab_size",
    "pdf_os_cntr", "pdf_os_objidx", "pdf_dest_names_ptr", "dest_names_size", "pdf_mem_ptr", "pdf_mem_size",
    "largest_used_mark", "luabytecodes", "luabytecode_bytes", "luastate_bytes", "callbacks", "indirect_callbacks",
    "saved_callbacks", "late_callbacks", "direct_callbacks", "function_callbacks", "lc_ctype", "lc_collate",
    "lc_numeric",
];

fn cnf_int(name: &str) -> i64 {
    crate::lua_sys_kpse::cnf_number(name)
}

fn diagnostic_message(severity: crate::diagnostics::DiagnosticSeverity) -> Field {
    let from = RESET_AT.with(Cell::get);
    with_engine(|e| {
        e.diagnostics
            .iter()
            .skip(from)
            .rev()
            .find(|d| d.severity == severity)
            .map(|d| Field::Str(d.message.clone()))
    })
    .ok()
    .flatten()
    .unwrap_or(Field::Nil)
}

fn field(name: &str) -> Option<Field> {
    use crate::diagnostics::DiagnosticSeverity::{Error, Warning};
    let engine = |f: fn(&mut crate::engine::Engine) -> i64| Field::Int(with_engine(|e| f(e)).unwrap_or(0));
    Some(match name {
        "output_active" => Field::Bool(with_engine(|e| e.output_depth > 0).unwrap_or(false)),
        "best_page_break" => Field::Nil,
        "filename" => Field::Str(with_engine(|e| e.input.current_file_name()).unwrap_or_default()),
        "inputid" => engine(|e| e.input.stack.len() as i64),
        "linenumber" => engine(|e| i64::from(e.input.current_file_line())),
        "lasterrorstring" => diagnostic_message(Error),
        "lastluaerrorstring" | "lastwarningtag" | "lasterrorcontext" => Field::Nil,
        "lastwarningstring" => diagnostic_message(Warning),
        // pdf.c: the named-destination table starts with room for 1000 entries
        "dest_names_size" => Field::Int(1000),
        "pdf_gone" | "pdf_ptr" | "dvi_gone" | "dvi_ptr" | "total_pages" | "obj_ptr" | "obj_tab_size" | "pdf_os_cntr"
        | "pdf_os_objidx" | "pdf_dest_names_ptr" | "pdf_mem_ptr" | "pdf_mem_size" => Field::Int(0),
        "output_file_name" => Field::Nil,
        "log_name" => Field::Str(
            with_engine(|e| {
                let job = if e.job_name.is_empty() { "texput" } else { e.job_name.as_str() };
                format!("{job}.log")
            })
            .unwrap_or_else(|_| "texput.log".to_string()),
        ),
        "banner" => Field::Str("This is LuaTeX, Version 1.24.0".to_string()),
        "luatex_version" => Field::Int(124),
        "luatex_revision" => Field::Str("0".to_string()),
        "development_id" => Field::Int(7724),
        "luatex_hashtype" => Field::Str("lua".to_string()),
        "luatex_hashchars" => Field::Int(6),
        "luatex_engine" => Field::Str(engine_name().to_string()),
        "ini_version" => Field::Bool(with_engine(|e| e.ini_mode).unwrap_or(false)),
        "shell_escape" => Field::Int(match shell_escape() {
            ShellEscape::Disabled => 0,
            ShellEscape::Enabled => 1,
            ShellEscape::Restricted => 2,
        }),
        "safer_option" => Field::Int(i64::from(safer_option())),
        "luadebug_option" => Field::Int(i64::from(shell_escape() == ShellEscape::Enabled)),
        "kpse_used" => Field::Int(1),
        "output_directory" => with_engine(|e| e.out_dir.clone())
            .ok()
            .filter(|d| !d.is_empty())
            .map_or(Field::Nil, Field::Str),
        "cs_count" => engine(|e| e.cs.len() as i64),
        "hash_size" => Field::Int(65536),
        "hash_extra" => Field::Int(cnf_int("hash_extra")),
        "font_ptr" => engine(|e| e.eqtb.fonts.len().saturating_sub(1) as i64),
        "max_strings" => Field::Int(cnf_int("max_strings")),
        "pool_size" => Field::Int(cnf_int("pool_size")),
        "stack_size" => Field::Int(cnf_int("stack_size")),
        "nest_size" => Field::Int(cnf_int("nest_size")),
        "param_size" => Field::Int(cnf_int("param_size")),
        "buf_size" => Field::Int(cnf_int("buf_size")),
        "save_size" => Field::Int(cnf_int("save_size")),
        "input_ptr" => engine(|e| e.input.stack.len() as i64),
        "luabytecodes" => Field::Int(-1),
        "lc_ctype" => Field::Str(locale("LC_CTYPE")),
        "lc_collate" => Field::Str(locale("LC_COLLATE")),
        "lc_numeric" => Field::Str(locale("LC_NUMERIC")),
        "node_mem_usage" => Field::Str(String::new()),
        "var_used" | "dyn_used" | "str_ptr" | "init_str_ptr" | "pool_ptr" | "init_pool_ptr" | "var_mem_max" | "fix_mem_max"
        | "fix_mem_min" | "fix_mem_end" | "max_in_stack" | "max_nest_stack" | "max_param_stack" | "max_buf_stack"
        | "max_save_stack" | "largest_used_mark" | "luabytecode_bytes" | "luastate_bytes" | "callbacks"
        | "indirect_callbacks" | "saved_callbacks" | "late_callbacks" | "direct_callbacks" | "function_callbacks" => {
            Field::Int(0)
        }
        _ => return None,
    })
}

pub(crate) fn register(lua: &mut Lua, s: &tex_lua::LuaTable) -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let table = lua.create_table().map_err(|e| format!("{e:?}"))?;
    for (i, a) in args.iter().enumerate() {
        table.set(i as i64, a.as_str()).map_err(|e| format!("{e:?}"))?;
    }
    lua.set_global("arg", table).map_err(|e| format!("{e:?}"))?;
    // luatex's `luatex_core_version` (luatex.c), a Lua number.
    lua.set_global("LUATEXCOREVERSION", 1.18_f64).map_err(|e| format!("{e:?}"))?;
    sys_reg!(lua, s, "status_names", || -> Vec<&'static str> { NAMES.to_vec() });
    // (kind, integer, string): 0 unknown, 1 integer, 2 string, 3 boolean, 4 nil
    sys_reg!(lua, s, "status_field", |name: LuaString| -> (i64, i64, Option<LuaBytes>) {
        let name = String::from_utf8_lossy(&crate::lua_sys::bytes_of(&name)).into_owned();
        match field(&name) {
            None => (0, 0, None),
            Some(Field::Int(i)) => (1, i, None),
            Some(Field::Str(v)) => (2, 0, Some(LuaBytes(v.into_bytes()))),
            Some(Field::Bool(b)) => (3, i64::from(b), None),
            Some(Field::Nil) => (4, 0, None),
        }
    });
    sys_reg!(lua, s, "status_resetmessages", || {
        let at = with_engine(|e| e.diagnostics.len()).unwrap_or(0);
        RESET_AT.with(|r| r.set(at));
    });
    sys_reg!(lua, s, "status_setexitcode", |code: i64| {
        EXIT_CODE.with(|c| c.set(Some(code as i32)));
    });
    sys_reg!(lua, s, "lua_calllevel", || -> i64 { crate::lua_bridge::lua_call_level() });
    Ok(())
}

/// Settings a startup script left in `texconfig` that LuaTeX reads once
/// after that script ran (`luainit.c`): `shell_escape` (`t`, `y`, `1`, `p`),
/// `shell_escape_commands` and `SOURCE_DATE_EPOCH`/`start_time`.
/// Applies the shell policy and returns the start time, if one was set.
pub fn apply_texconfig(lua: &mut Lua) -> Option<i64> {
    let config: tex_lua::LuaTable = lua.get_global("texconfig").ok().flatten()?;
    if let Ok(mode) = config.get::<String>("shell_escape") {
        match mode.chars().next() {
            Some('t' | 'y' | '1') => crate::lua_sys::set_shell_escape(ShellEscape::Enabled),
            Some('p') => crate::lua_sys::set_shell_escape(ShellEscape::Restricted),
            _ => {}
        }
    }
    if let Ok(list) = config.get::<String>("shell_escape_commands") {
        crate::lua_sys_kpse::set_allowed_commands(&list);
    }
    let start = config.get::<i64>("start_time").ok().filter(|t| *t >= 0);
    start.or_else(|| config.get::<i64>("SOURCE_DATE_EPOCH").ok().filter(|t| *t >= 0))
}
