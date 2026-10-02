// Tests for package library and module system
use crate::*;

#[test]
fn test_package_loaded() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(type(package.loaded) == "table")
        assert(package.loaded.string ~= nil)
        assert(package.loaded.table ~= nil)
        assert(package.loaded.math ~= nil)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_package_preload() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(type(package.preload) == "table")
        
        package.preload['testmod'] = function()
            return {value = 42}
        end
        
        local mod = require('testmod')
        assert(mod.value == 42)
    "#,
    );

    if let Err(e) = &result {
        let error_msg = vm.main_state().get_error_message(*e);
        eprintln!("Error: {:?}, Message: {}", e, error_msg);
    }
    assert!(result.is_ok());
}

#[test]
fn test_package_path() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(type(package.path) == "string")
        assert(#package.path > 0)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_package_cpath() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(type(package.cpath) == "string")
        assert(#package.cpath > 0)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_package_config() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(type(package.config) == "string")
        local lines = 0
        for line in package.config:gmatch("[^\n]+") do
            lines = lines + 1
        end
        assert(lines == 5)
    "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_package_searchers() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(type(package.searchers) == "table")
        assert(type(package.searchers[1]) == "function")  -- preload searcher
        assert(type(package.searchers[2]) == "function")  -- lua file searcher
        assert(type(package.searchers[3]) == "function")  -- C module searcher
        assert(type(package.searchers[4]) == "function")  -- all-in-one C searcher
        assert(package.searchers[5] == nil)  -- we have 4 searchers total
    "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn test_package_searchpath() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local path, err = package.searchpath("string", package.path)
        -- Either finds a file or returns error message
        assert(path ~= nil or err ~= nil)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_require_preload() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        package.preload['mymodule'] = function()
            local M = {}
            M.name = "MyModule"
            M.version = "1.0"
            function M.hello()
                return "Hello!"
            end
            return M
        end
        
        local mod = require('mymodule')
        assert(mod.name == "MyModule")
        assert(mod.version == "1.0")
        assert(mod.hello() == "Hello!")
    "#,
    );

    if let Err(e) = &result {
        let error_msg = vm.main_state().get_error_message(*e);
        panic!("Error: {:?}, Message: {}", e, error_msg);
    }
    assert!(result.is_ok());
}

#[test]
fn test_require_cache() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local load_count = 0
        package.preload['cached'] = function()
            load_count = load_count + 1
            return {count = load_count}
        end
        
        local m1 = require('cached')
        local m2 = require('cached')
        
        assert(m1 == m2)
        assert(load_count == 1)
        assert(package.loaded['cached'] == m1)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_require_error() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local ok, err = pcall(require, 'nonexistent_module_xyz')
        assert(ok == false)
        assert(type(err) == "string")
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_require_return_value() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        -- Module returning nil should store true
        package.preload['nilmod'] = function()
            return nil
        end
        
        local m = require('nilmod')
        assert(m == true)
        assert(package.loaded['nilmod'] == true)
    "#,
    );

    assert!(result.is_ok());
}

/// Write `files` into a fresh directory and run `code` with package.path
/// pointing there.
fn run_with_modules(level: LuaLanguageLevel, files: &[(&str, &str)], code: &str) {
    let dir = std::env::temp_dir().join(format!(
        "tex-lua-package-{}-{}-{level:?}",
        std::process::id(),
        files.len()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let mut vm = GlobalState::new_with_language(SafeOption::default(), level);
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let prefix = format!("package.path = {:?}\n", format!("{}/?.lua", dir.display()));
    let result = vm.main_state().execute(&format!("{prefix}{code}"));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(result.is_ok(), "{level}: {result:?}");
}

#[test]
fn test_package_module_compat_53() {
    // Expected behaviour from texlua (Lua 5.3 with LUA_COMPAT_MODULE).
    run_with_modules(
        LuaLanguageLevel::Lua53,
        &[("mymod.lua", "module('mymod', package.seeall)\nfunction hello() return type(print) end\n")],
        &r#"
        assert(package.config == "@DIRSEP@\n;\n?\n!\n-\n")
        assert(package.loaders == package.searchers)
        local m = require("mymod")
        assert(m == mymod and m.hello() == "function" and m._NAME == "mymod" and m._PACKAGE == "")
        local function f()
          module("a.b.c", package.seeall)
          return _NAME, _PACKAGE, _M
        end
        local name, pkg, mod = f()
        assert(name == "a.b.c" and pkg == "a.b." and a.b.c == mod and package.loaded["a.b.c"] == mod)
        assert(select('#', require("mymod")) == 1)
        "#
        .replace("@DIRSEP@", if cfg!(windows) { "\\\\" } else { "/" }),
    );
}

#[test]
fn test_require_loader_contract() {
    for level in [LuaLanguageLevel::Lua53, LuaLanguageLevel::Lua55] {
        run_with_modules(
            level,
            &[("two.lua", "return select('#', ...), ...\n"), ("bad.lua", "x = = 1\n"), ("boom.lua", "error('boom', 0)\n")],
            r#"
            local n, name, file = require("two")
            assert(n == 2 and (name == nil or name:find("two%.lua$")), n)
            assert(package.loaded.two == 2)
            local ok, e = pcall(require, "bad")
            assert(e:find("^error loading module 'bad' from file '.-bad%.lua':\n\t.-bad%.lua:1: "), e)
            ok, e = pcall(require, "boom")
            assert(e == "boom", e)   -- loader errors propagate unchanged
            ok, e = pcall(require)
            assert(e:find("string expected, got no value", 1, true), e)
            ok, e = pcall(require, 42)
            assert(e:find("^module '42' not found:"), e)
            "#,
        );
    }
}
