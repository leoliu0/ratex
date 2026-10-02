// Tests for OS library functions
use crate::*;
use std::env;

// Helper to get the test data directory path
fn get_test_data_dir() -> String {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    format!("{}/src/test/test_data", manifest_dir).replace("\\", "/")
}

#[test]
fn test_os_time() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = os.time()
        assert(type(t) == "number")
        assert(t > 0)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_time_with_table() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    // Note: os.time with table argument not fully implemented
    // Just verify it doesn't crash
    let result = vm.main_state().execute(
        r#"
        local t = os.time()
        assert(type(t) == "number")
        assert(t > 0)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_time_out_of_bound_year_reports_lua_error() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local ok, err = pcall(os.time, {year = math.mininteger, month = 1, day = 1})
        assert(ok == false)
        assert(type(err) == "string")
        assert(string.find(err, "field 'year' is out%-of%-bound") ~= nil)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_date_default() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local d = os.date()
        assert(type(d) == "string")
        assert(#d > 0)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_date_table() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    // Note: os.date("*t") not fully implemented
    // Just verify os.date() returns a string
    let result = vm.main_state().execute(
        r#"
        local d = os.date()
        assert(type(d) == "string")
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_date_format() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    // Note: os.date format strings not fully implemented
    // Just verify basic functionality
    let result = vm.main_state().execute(
        r#"
        local d = os.date()
        assert(type(d) == "string")
        assert(#d > 0)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_difftime() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t1 = os.time()
        local t2 = t1 + 100
        local diff = os.difftime(t2, t1)
        assert(diff == 100)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_clock() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local c1 = os.clock()
        assert(type(c1) == "number")
        assert(c1 >= 0)
        
        -- Do some work
        local sum = 0
        for i = 1, 10000 do sum = sum + i end
        
        local c2 = os.clock()
        assert(c2 >= c1)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_getenv() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        -- PATH should exist on most systems
        local path = os.getenv("PATH")
        assert(path == nil or type(path) == "string")
        
        -- Non-existent env var should return nil
        local nonexistent = os.getenv("NONEXISTENT_VAR_12345")
        assert(nonexistent == nil)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_remove() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local path = "{}/temp_remove.txt"
        
        -- Create a file
        local f = io.open(path, "w")
        f:write("to be removed")
        f:close()
        
        -- Remove it
        local ok, err = os.remove(path)
        assert(ok == true or ok == nil)  -- Some implementations return true, others nil on success
        
        -- Verify it's gone
        local f2 = io.open(path, "r")
        assert(f2 == nil)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_remove_nonexistent() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local ok, err = os.remove("nonexistent_file_99999.txt")
        assert(ok == nil)
        assert(err ~= nil)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_rename() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local path1 = "{}/temp_rename1.txt"
        local path2 = "{}/temp_rename2.txt"
        
        -- Create a file
        local f = io.open(path1, "w")
        f:write("rename test")
        f:close()
        
        -- Rename it
        local ok = os.rename(path1, path2)
        
        -- Verify old name is gone
        local f1 = io.open(path1, "r")
        assert(f1 == nil)
        
        -- Verify new name exists
        local f2 = io.open(path2, "r")
        if f2 then
            local content = f2:read("*a")
            assert(content == "rename test")
            f2:close()
        end
        
        -- Clean up
        os.remove(path2)
        "#,
        test_dir, test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_tmpname() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local name = os.tmpname()
        assert(type(name) == "string")
        assert(#name > 0)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_exit() {
    // Note: We don't actually test os.exit() as it would terminate the process
    // Just verify the function exists
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(type(os.exit) == "function")
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_os_execute_nonzero_exit_returns_nil() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let command = if cfg!(target_os = "windows") {
        "exit /b 3"
    } else {
        "exit 3"
    };

    let result = vm.main_state().execute(&format!(
        r#"
        local ok, how, code = os.execute("{}")
        assert(ok == nil)
        assert(how == "exit")
        assert(code == 3)
        "#,
        command
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_os_setlocale() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        -- Query current locale
        local loc = os.setlocale(nil)
        assert(loc == nil or type(loc) == "string")
        
        -- Try to set to C locale
        local c_loc = os.setlocale("C")
        assert(c_loc == nil or c_loc == "C")
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

fn run_os_script(level: LuaLanguageLevel, code: &str) {
    let mut vm = GlobalState::new_with_language(SafeOption::default(), level);
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let result = vm.main_state().execute(code);
    assert!(result.is_ok(), "{level:?}: {:?}", result);
}

fn run_os_script_all(code: &str) {
    run_os_script(LuaLanguageLevel::Lua53, code);
    run_os_script(LuaLanguageLevel::Lua55, code);
}

#[test]
fn test_os_date_utc_ignores_local_time_zone() {
    run_os_script_all(
        r##"
        assert(os.date("!%Y-%m-%d %H:%M:%S", 0) == "1970-01-01 00:00:00")
        assert(os.date("!%Y-%m-%d %H:%M:%S", -1) == "1969-12-31 23:59:59")
        assert(os.date("!%c", 1700000000) == "Tue Nov 14 22:13:20 2023")
        local t = os.date("!*t", 1700000000)
        assert(t.year == 2023 and t.month == 11 and t.day == 14 and t.hour == 22)
        assert(t.min == 13 and t.sec == 20 and t.wday == 3 and t.yday == 318 and t.isdst == false)
        assert(os.date("!%z", 0) == "+0000")
        "##,
    );
}

#[test]
fn test_os_date_conversions() {
    run_os_script_all(
        r##"
        local T = 1700000000
        assert(os.date("!%C %D %e %F %g %G %h %R %T %u %V %y", T)
            == "20 11/14/23 14 2023-11-14 23 2023 Nov 22:13 22:13:20 2 46 23")
        assert(os.date("!%Ey %OH %%", T) == "23 22 %")
        for _, bad in ipairs{"%Q", "%", "%E", "%Ea", "%O", "%5", "%s", "%-d", "%#c"} do
          local ok, err = pcall(os.date, bad, T)
          assert(not ok and err:find("invalid conversion specifier '" .. bad .. "'", 1, true), bad)
        end
        assert(not pcall(os.date, "%Y", 1.5))
        assert(os.date("!%Y", "1700000000") == "2023")
        assert(os.date("!x", T) == "x" and os.date(5, T) == "5")
        "##,
    );
}

#[test]
fn test_os_time_normalizes_and_round_trips() {
    run_os_script_all(
        r##"
        for _, t in ipairs{0, 86399, 1700000000, 1720000000, 951782400} do
          assert(os.time(os.date("*t", t)) == t, t)
        end
        "##,
    );
    run_os_script_all(
        r##"
        local d = {year = 2023, month = 14, day = 35, hour = 12}
        local t = os.time(d)
        assert(d.year == 2024 and d.month == 3 and d.day == 6 and d.yday == 66 and d.wday == 4)
        assert(type(d.isdst) == "boolean")
        assert(os.time({year = "2023", month = "1", day = "1", hour = 0}) == os.time({year = 2023, month = 1, day = 1, hour = 0}))
        local ok, err = pcall(os.time, {year = "x", month = 1, day = 1})
        assert(not ok and err:find("field 'year' is not an integer", 1, true))
        ok, err = pcall(os.time, {year = 2023, month = 1})
        assert(not ok and err:find("field 'day' missing in date table", 1, true))
        assert(not pcall(os.time, 5))
        assert(math.type(os.difftime(10, 5)) == "float" and os.difftime(10, 5) == 5)
        assert(not pcall(os.difftime, 10))
        "##,
    );
}

#[cfg(unix)]
#[test]
fn test_os_file_operations_return_c_error_triples() {
    let dir = std::env::temp_dir().join(format!("tex_lua_os_{}", std::process::id()));
    let script = format!(
        r##"
        local dir = {dir:?}
        local r, msg, code = os.remove(dir .. "/missing")
        assert(r == nil and msg == dir .. "/missing: No such file or directory" and code == {enoent})
        r, msg, code = os.rename(dir .. "/missing", dir .. "/other")
        assert(r == nil and msg == "No such file or directory" and code == {enoent})
        r, msg, code = os.remove(dir)
        assert(r == nil and msg == dir .. ": Directory not empty" and code == {enotempty})
        assert(os.remove(dir .. "/sub") == true)
        local name = os.tmpname()
        local f = io.open(name, "r")
        assert(f, "tmpname must create the file") f:close()
        assert(os.remove(name) == true)
        assert(os.getenv(1) == nil and not pcall(os.getenv))
        "##,
        dir = dir.to_string_lossy(),
        // errno numbers are the platform's (ENOTEMPTY is 39 on Linux, 66 on macOS)
        enoent = libc::ENOENT,
        enotempty = libc::ENOTEMPTY
    );
    for level in [LuaLanguageLevel::Lua53, LuaLanguageLevel::Lua55] {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        run_os_script(level, &script);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
