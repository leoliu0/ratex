// Package library
// Implements: config, cpath, loaded, loadlib, path, preload, searchers, searchpath

use crate::LuaLanguageLevel;
use crate::lib_registry::LibraryModule;
use crate::lua_value::{LuaValue, UpvalueStore};
use crate::lua_vm::{LuaResult, LuaState};
use crate::stdlib::lauxlib;

pub fn create_package_lib() -> LibraryModule {
    crate::lib_module!("package", {
        "loadlib" => package_loadlib,
        "searchpath" => package_searchpath,
    })
    .with_initializer(init_package_fields)
}

// Initialize package library fields (called after module is loaded)
pub fn init_package_fields(l: &mut LuaState) -> LuaResult<()> {
    // Get package table (should already exist from module creation)
    let package_table = l
        .get_global_value("package")?
        .ok_or_else(|| l.error("package table not found".to_string()))?;

    if !package_table.is_table() {
        return Err(l.error("package must be a table".to_string()));
    };

    // Create all keys
    let loaded_key = l.create_string("loaded")?;
    let preload_key = l.create_string("preload")?;
    let path_key = l.create_string("path")?;
    let cpath_key = l.create_string("cpath")?;
    let config_key = l.create_string("config")?;
    let searchers_key = l.create_string("searchers")?;

    // Create all values
    let loaded_table = l.create_table(0, 0)?;
    let preload_table = l.create_table(0, 0)?;
    let no_environment = l
        .global_state_mut()
        .registry_get("LUA_NOENV")?
        .is_some_and(|value| value.is_truthy());
    let (path_default, cpath_default, version_suffix) = match l.global_state().version {
        LuaLanguageLevel::Lua53 => (
            "/usr/local/share/lua/5.3/?.lua;/usr/local/share/lua/5.3/?/init.lua;/usr/local/lib/lua/5.3/?.lua;/usr/local/lib/lua/5.3/?/init.lua;./?.lua;./?/init.lua",
            "/usr/local/lib/lua/5.3/?.so;/usr/local/lib/lua/5.3/loadall.so;./?.so",
            "_5_3",
        ),
        _ => (
            "/usr/local/share/lua/5.5/?.lua;/usr/local/share/lua/5.5/?/init.lua;/usr/local/lib/lua/5.5/?.lua;/usr/local/lib/lua/5.5/?/init.lua;./?.lua;./?/init.lua",
            "/usr/local/lib/lua/5.5/?.so;/usr/local/lib/lua/5.5/loadall.so;./?.so",
            "_5_5",
        ),
    };
    let cpath_default = if cfg!(windows) {
        cpath_default.replace(".so", ".dll")
    } else {
        cpath_default.to_owned()
    };
    let path =
        package_path_from_environment("LUA_PATH", version_suffix, path_default, no_environment);
    let cpath =
        package_path_from_environment("LUA_CPATH", version_suffix, &cpath_default, no_environment);
    let path_value = l.create_string(&path)?;
    let cpath_value = l.create_string(&cpath)?;

    #[cfg(windows)]
    let config_str = "\\\n;\n?\n!\n-\n";
    #[cfg(not(windows))]
    let config_str = "/\n;\n?\n!\n-\n";
    let config_value = l.create_string(config_str)?;

    // Create searchers array
    let searchers_table_value = l.create_table(4, 0)?;
    let searchers_table = searchers_table_value.as_table_mut().unwrap();

    // Fill searchers array
    searchers_table.raw_seti(1, LuaValue::cfunction(searcher_preload));
    searchers_table.raw_seti(2, LuaValue::cfunction(searcher_lua));
    searchers_table.raw_seti(3, LuaValue::cfunction(searcher_c));
    searchers_table.raw_seti(4, LuaValue::cfunction(searcher_c_all_in_one));

    // Set all fields in package table
    l.raw_set(&package_table, loaded_key, loaded_table);
    l.raw_set(&package_table, preload_key, preload_table);
    l.raw_set(&package_table, path_key, path_value);
    l.raw_set(&package_table, cpath_key, cpath_value);
    l.raw_set(&package_table, config_key, config_value);
    l.raw_set(&package_table, searchers_key, searchers_table_value);
    if l.global_state().version == LuaLanguageLevel::Lua53 {
        // LUA_COMPAT_MODULE (TeX Live scripts still call module())
        let loaders_key = l.create_string("loaders")?;
        l.raw_set(&package_table, loaders_key, searchers_table_value);
        let seeall_key = l.create_string("seeall")?;
        l.raw_set(&package_table, seeall_key, LuaValue::cfunction(ll_seeall));
        l.set_global_value("module", LuaValue::cfunction(ll_module))?;
    }

    // Add package itself to package.loaded (normally lib_registry does this,
    // but package.loaded doesn't exist yet when the package module is first loaded)
    let package_mod_key = l.create_string("package")?;
    l.raw_set(&loaded_table, package_mod_key, package_table);

    // Store loaded table and package table in registry for use by require
    // This matches standard Lua's LUA_LOADED_TABLE ("_LOADED") and upvalue approach
    let vm = l.global_state_mut();
    vm.registry_set("_LOADED", loaded_table)?;
    vm.registry_set("_PRELOAD", preload_table)?;
    // Store the original package table so require can find searchers
    // even if the global 'package' is reassigned
    vm.registry_set("_PACKAGE", package_table)?;

    Ok(())
}

fn package_path_from_environment(
    name: &str,
    suffix: &str,
    default: &str,
    no_environment: bool,
) -> String {
    let versioned = format!("{name}{suffix}");
    let path = (!no_environment)
        .then(|| std::env::var_os(&versioned).or_else(|| std::env::var_os(name)))
        .flatten();
    let Some(path) = path else {
        return default.to_owned();
    };
    let path = path.to_string_lossy();
    if path.contains(";;") {
        path.replace(";;", &format!(";{default};"))
    } else {
        path.into_owned()
    }
}

// Helper to get the original package table from registry
fn get_package_from_registry(l: &mut LuaState) -> LuaResult<LuaValue> {
    let vm = l.global_state_mut();
    vm.registry_get("_PACKAGE")?
        .ok_or_else(|| vm.error("package table not found".to_string()))
}

fn is_lua53(l: &LuaState) -> bool {
    l.global_state().language() == LuaLanguageLevel::Lua53
}

/// A searcher's "not found" entry: Lua 5.3 searchers start each entry with
/// "\n\t"; from 5.4 on, `require` adds the separator itself.
fn not_found_entry(lua53: bool, entry: &str) -> String {
    if lua53 { format!("\n\t{entry}") } else { entry.to_owned() }
}

/// `package.<field>` of the package table, which must be a string.
fn package_string_field(l: &mut LuaState, field: &str) -> LuaResult<String> {
    let package = get_package_from_registry(l)?;
    let key = l.create_string(field)?;
    match package.as_table().and_then(|table| table.raw_get(&key)) {
        Some(value) if value.is_string() || value.is_number() => {
            Ok(String::from_utf8_lossy(&lauxlib::tolstring(l, &value)?).into_owned())
        }
        _ => Err(lauxlib::lual_error(l, format!("'package.{field}' must be a string"))),
    }
}

// Searcher 1: package.preload
fn searcher_preload(l: &mut LuaState) -> LuaResult<usize> {
    let name = String::from_utf8_lossy(&lauxlib::check_lstring(l, 1)?).into_owned();
    let preload = l.global_state_mut().registry_get("_PRELOAD")?.unwrap_or_default();
    let Some(preload_table) = preload.as_table() else {
        return Err(lauxlib::lual_error(l, "'package.preload' must be a table"));
    };
    let key = l.create_string(&name)?;
    let loader = preload_table.raw_get(&key).unwrap_or_default();
    let lua53 = is_lua53(l);
    if loader.is_nil() {
        let message = not_found_entry(lua53, &format!("no field package.preload['{name}']"));
        let message = l.create_string(&message)?;
        l.push_value(message)?;
        return Ok(1);
    }
    l.push_value(loader)?;
    if lua53 {
        return Ok(1);
    }
    let data = l.create_string(":preload:")?;
    l.push_value(data)?;
    Ok(2)
}

// Searcher 2: package.path
fn searcher_lua(l: &mut LuaState) -> LuaResult<usize> {
    let name = String::from_utf8_lossy(&lauxlib::check_lstring(l, 1)?).into_owned();
    let path = package_string_field(l, "path")?;
    let lua53 = is_lua53(l);
    let filename = match search_path(&name, &path, ".", std::path::MAIN_SEPARATOR_STR, lua53) {
        Ok(filename) => filename,
        Err(message) => {
            let message = l.create_string(&message)?;
            l.push_value(message)?;
            return Ok(1);
        }
    };
    // checkload: the loaded chunk is the loader, the file name its data
    match load_lua_file(l, &filename) {
        Ok(function) => {
            l.push_value(function)?;
            let filename = l.create_string(&filename)?;
            l.push_value(filename)?;
            Ok(2)
        }
        Err(message) => Err(lauxlib::lual_error(
            l,
            format!("error loading module '{name}' from file '{filename}':\n\t{message}"),
        )),
    }
}

/// `luaL_loadfile` with the global environment.
fn load_lua_file(l: &mut LuaState, filename: &str) -> Result<LuaValue, String> {
    let proto = l.load_proto_from_file(filename).map_err(|error| l.get_error_message(error))?;
    let global = l.global_state().global;
    let function = l.create_upvalue_closed(global).and_then(|env| {
        l.global_state_mut()
            .create_function(proto, UpvalueStore::from_single(env))
    });
    function.map_err(|error| l.get_error_message(error))
}

/// `luaL_pushmodule` (LUA_COMPAT_MODULE): LOADED[modname], or the global
/// table path `modname` (created if missing), registered in LOADED.
fn push_module(l: &mut LuaState, modname: &str) -> LuaResult<LuaValue> {
    let loaded = l.global_state_mut().registry_get("_LOADED")?.unwrap_or_default();
    let key = l.create_string(modname)?;
    if let Some(module) = l.table_get(&loaded, &key)?.filter(|value| value.is_table()) {
        return Ok(module);
    }
    // luaL_findtable(_G, modname)
    let mut table = l.global_state().global;
    for part in modname.split('.') {
        let part_key = l.create_string(part)?;
        let field = table.as_table().and_then(|t| t.raw_get(&part_key)).unwrap_or_default();
        table = if field.is_nil() {
            let created = l.create_table(0, 1)?;
            l.table_set(&table, part_key, created)?;
            created
        } else if field.is_table() {
            field
        } else {
            return Err(lauxlib::lual_error(l, format!("name conflict for module '{modname}'")));
        };
    }
    l.table_set(&loaded, key, table)?;
    Ok(table)
}

/// module(name [, ...]) (Lua 5.3 with LUA_COMPAT_MODULE)
fn ll_module(l: &mut LuaState) -> LuaResult<usize> {
    let modname = String::from_utf8_lossy(&lauxlib::check_lstring(l, 1)?).into_owned();
    let options = l.get_args();
    let module = push_module(l, &modname)?;
    let name_key = l.create_string("_NAME")?;
    if l.table_get(&module, &name_key)?.is_none_or(|value| value.is_nil()) {
        // modinit
        let m_key = l.create_string("_M")?;
        l.table_set(&module, m_key, module)?;
        let name_value = l.create_string(&modname)?;
        l.table_set(&module, name_key, name_value)?;
        let package = modname.rfind('.').map_or("", |dot| &modname[..=dot]);
        let package_key = l.create_string("_PACKAGE")?;
        let package_value = l.create_string(package)?;
        l.table_set(&module, package_key, package_value)?;
    }
    // set_env: the calling Lua function's first upvalue becomes the module
    let depth = l.call_depth();
    let caller = (depth >= 2).then(|| l.get_frame_func(depth - 2)).flatten();
    let Some(caller) = caller.filter(|function| function.as_lua_function().is_some()) else {
        return Err(lauxlib::lual_error(l, "'module' not called from a Lua function"));
    };
    if let Some(function) = caller.as_lua_function()
        && let Some(&upvalue) = function.upvalues().first()
    {
        upvalue.as_mut_ref().data.set_value(module);
        if let Some(gc_ptr) = module.as_gc_ptr() {
            l.gc_barrier(upvalue, gc_ptr);
        }
    }
    // dooptions
    for option in options.into_iter().skip(1) {
        if option.is_function() {
            l.call(option, vec![module])?;
        }
    }
    l.push_value(module)?;
    Ok(1)
}

/// package.seeall(module) (Lua 5.3 with LUA_COMPAT_MODULE)
fn ll_seeall(l: &mut LuaState) -> LuaResult<usize> {
    let module = l.get_arg(1).unwrap_or_default();
    if !module.is_table() {
        return Err(lauxlib::typeerror(l, 1, "table"));
    }
    let metatable = match module.as_table().and_then(|t| t.get_metatable()) {
        Some(metatable) => metatable,
        None => {
            let metatable = l.create_table(0, 1)?;
            if let Some(table) = module.as_table_mut() {
                table.set_metatable(Some(metatable));
            }
            if let Some(gc_ptr) = module.as_gc_ptr() {
                l.gc_barrier_back(gc_ptr);
            }
            metatable
        }
    };
    let index_key = l.create_string("__index")?;
    let global = l.global_state().global;
    l.table_set(&metatable, index_key, global)?;
    Ok(0)
}

// Searcher 3: Search package.cpath for C modules.
fn searcher_c(l: &mut LuaState) -> LuaResult<usize> {
    let modname = String::from_utf8_lossy(&lauxlib::check_lstring(l, 1)?).into_owned();
    let cpath = package_string_field(l, "cpath")?;
    let lua53 = is_lua53(l);
    let path = match search_path(&modname, &cpath, ".", std::path::MAIN_SEPARATOR_STR, lua53) {
        Ok(path) => path,
        Err(message) => {
            let message = l.create_string(&message)?;
            l.push_value(message)?;
            return Ok(1);
        }
    };
    let loader = load_native_module_function(l, &path, &modname).map_err(|error| {
        l.error(format!(
            "error loading module '{modname}' from file '{path}':\n\t{}",
            error.message()
        ))
    })?;
    l.push_value(loader)?;
    let path = l.create_string(&path)?;
    l.push_value(path)?;
    Ok(2)
}

// Searcher 4: Search the root library for a submodule entry point.
fn searcher_c_all_in_one(l: &mut LuaState) -> LuaResult<usize> {
    let modname = String::from_utf8_lossy(&lauxlib::check_lstring(l, 1)?).into_owned();
    let Some(root_name) = modname.split('.').next().filter(|root| *root != modname) else {
        return Ok(0);
    };
    let cpath = package_string_field(l, "cpath")?;
    let lua53 = is_lua53(l);
    let Ok(path) = search_path(root_name, &cpath, ".", std::path::MAIN_SEPARATOR_STR, lua53) else {
        return Ok(0);
    };
    let loader = match load_native_module_function(l, &path, &modname) {
        Ok(loader) => loader,
        Err(NativeLoadError::Symbol(_)) => {
            let message = not_found_entry(lua53, &format!("no module '{modname}' in file '{path}'"));
            let message = l.create_string(&message)?;
            l.push_value(message)?;
            return Ok(1);
        }
        Err(error) => {
            return Err(l.error(format!(
                "error loading module '{modname}' from file '{path}':\n\t{}",
                error.message()
            )));
        }
    };
    l.push_value(loader)?;
    let path = l.create_string(&path)?;
    l.push_value(path)?;
    Ok(2)
}

fn native_open_symbols(module_name: &str) -> Vec<String> {
    let module_name = module_name.replace('.', "_");
    if let Some((prefix, suffix)) = module_name.split_once('-') {
        vec![format!("luaopen_{prefix}"), format!("luaopen_{suffix}")]
    } else {
        vec![format!("luaopen_{module_name}")]
    }
}

#[derive(Debug)]
enum NativeLoadError {
    Open(String),
    Symbol(String),
}

impl NativeLoadError {
    fn message(&self) -> &str {
        match self {
            Self::Open(message) | Self::Symbol(message) => message,
        }
    }
}

fn load_native_module_function(
    l: &mut LuaState,
    path: &str,
    module_name: &str,
) -> Result<LuaValue, NativeLoadError> {
    let mut last_error = None;
    for symbol in native_open_symbols(module_name) {
        match load_native_function(l, path, &symbol) {
            Ok(function) => return Ok(function),
            Err(error @ NativeLoadError::Open(_)) => return Err(error),
            Err(error @ NativeLoadError::Symbol(_)) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| NativeLoadError::Symbol("module has no open function".to_string())))
}

#[cfg(not(target_arch = "wasm32"))]
fn load_native_function(
    l: &mut LuaState,
    path: &str,
    symbol: &str,
) -> Result<LuaValue, NativeLoadError> {
    use libloading::{Library, Symbol};
    use std::ffi::c_void;

    let library_index = l
        .global_state()
        .native_libraries
        .iter()
        .position(|(loaded_path, _)| loaded_path == path);
    let library_index = if let Some(index) = library_index {
        index
    } else {
        let library = unsafe { Library::new(path) }
            .map_err(|error| NativeLoadError::Open(error.to_string()))?;
        let libraries = &mut l.global_state_mut().native_libraries;
        libraries.push((path.to_owned(), library));
        libraries.len() - 1
    };
    if symbol == "*" {
        return Ok(LuaValue::boolean(true));
    }
    type OpenFunction = unsafe extern "C" fn(*mut crate::c_api::lua_State) -> std::ffi::c_int;
    let callback = {
        let library = &l.global_state().native_libraries[library_index].1;
        unsafe {
            let entry: Symbol<'_, OpenFunction> = library
                .get(symbol.as_bytes())
                .map_err(|error| NativeLoadError::Symbol(error.to_string()))?;
            *entry as *const () as *mut c_void
        }
    };
    crate::c_api::external_c_function(l, callback)
        .map_err(|error| NativeLoadError::Symbol(l.get_error_msg(error)))
}

#[cfg(target_arch = "wasm32")]
fn load_native_function(
    _l: &mut LuaState,
    _path: &str,
    _symbol: &str,
) -> Result<LuaValue, NativeLoadError> {
    Err(NativeLoadError::Open(
        "dynamic libraries are not supported on this platform".to_string(),
    ))
}
/// C: searchpath. Returns the first readable file, or the "no file" list.
fn search_path(name: &str, path: &str, sep: &str, rep: &str, lua53: bool) -> Result<String, String> {
    let name = if sep.is_empty() { name.to_owned() } else { name.replace(sep, rep) };
    let mut tried = Vec::new();
    for template in path.split(';').filter(|template| !template.is_empty()) {
        let filename = template.replace('?', &name);
        if std::fs::File::open(&filename).is_ok() {
            return Ok(filename);
        }
        tried.push(format!("no file '{filename}'"));
    }
    if lua53 {
        Err(tried.iter().map(|entry| format!("\n\t{entry}")).collect())
    } else {
        Err(tried.join("\n\t"))
    }
}

fn package_loadlib(l: &mut LuaState) -> LuaResult<usize> {
    let path = l
        .get_arg(1)
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| l.error("bad argument #1 to 'loadlib' (string expected)".to_string()))?;
    let symbol = l
        .get_arg(2)
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| l.error("bad argument #2 to 'loadlib' (string expected)".to_string()))?;

    match load_native_function(l, &path, &symbol) {
        Ok(loader) => {
            l.push_value(loader)?;
            Ok(1)
        }
        Err(error) => {
            l.push_value(LuaValue::nil())?;
            let message = l.create_string(error.message())?;
            l.push_value(message)?;
            let stage = l.create_string(match error {
                NativeLoadError::Open(_) => "open",
                NativeLoadError::Symbol(_) => "init",
            })?;
            l.push_value(stage)?;
            Ok(3)
        }
    }
}

/// package.searchpath(name, path [, sep [, rep]])
fn package_searchpath(l: &mut LuaState) -> LuaResult<usize> {
    let name = String::from_utf8_lossy(&lauxlib::check_lstring(l, 1)?).into_owned();
    let path = String::from_utf8_lossy(&lauxlib::check_lstring(l, 2)?).into_owned();
    let sep = lauxlib::opt_lstring(l, 3)?.map_or_else(|| ".".to_owned(), |s| String::from_utf8_lossy(&s).into_owned());
    let rep = lauxlib::opt_lstring(l, 4)?.map_or_else(
        || std::path::MAIN_SEPARATOR_STR.to_owned(),
        |s| String::from_utf8_lossy(&s).into_owned(),
    );
    let lua53 = is_lua53(l);
    match search_path(&name, &path, &sep, &rep, lua53) {
        Ok(filename) => {
            let filename = l.create_string(&filename)?;
            l.push_value(filename)?;
            Ok(1)
        }
        Err(message) => {
            l.push_value(LuaValue::nil())?;
            let message = l.create_string(&message)?;
            l.push_value(message)?;
            Ok(2)
        }
    }
}
