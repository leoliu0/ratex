// Tests for UTF-8 library functions
use crate::*;

#[test]
fn test_utf8_len() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(utf8.len("hello") == 5)
        assert(utf8.len("") == 0)
        assert(utf8.len("世界") == 2)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_utf8_char() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(utf8.char(65) == "A")
        assert(utf8.char(65, 66, 67) == "ABC")
        assert(utf8.char(0x4E16, 0x754C) == "世界")
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_utf8_codes() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local s = "ABC"
        local codes = {}
        for p, c in utf8.codes(s) do
            table.insert(codes, c)
        end
        assert(codes[1] == 65)
        assert(codes[2] == 66)
        assert(codes[3] == 67)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_utf8_codepoint() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(utf8.codepoint("A") == 65)
        local a, b, c = utf8.codepoint("ABC", 1, 3)
        assert(a == 65)
        assert(b == 66)
        assert(c == 67)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_utf8_offset() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local s = "hello"
        assert(utf8.offset(s, 2) == 2)
        assert(utf8.offset(s, 5) == 5)
        assert(utf8.offset(s, -1) == 5)
    "#,
    );

    if let Err(e) = &result {
        eprintln!("Error: {}", e);
    }
    assert!(result.is_ok());
}

#[test]
fn test_utf8_charpattern() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(type(utf8.charpattern) == "string")
        assert(#utf8.charpattern > 0)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_utf8_multibyte() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local s = "Hello 世界"
        local len = utf8.len(s)
        assert(len == 8)
        
        local count = 0
        for p, c in utf8.codes(s) do
            count = count + 1
        end
        assert(count == 8)
    "#,
    );

    assert!(result.is_ok());
}

fn run_level(level: crate::LuaLanguageLevel, code: &str) {
    let mut vm = GlobalState::new_with_language(SafeOption::default(), level);
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let result = vm.main_state().execute(code);
    assert!(result.is_ok(), "{level}: {result:?}");
}

#[test]
fn test_utf8_matches_lutf8lib_53() {
    // Expected values from texlua (Lua 5.3.6).
    run_level(
        crate::LuaLanguageLevel::Lua53,
        r#"
        local sur = "\xED\xA0\x80"
        assert(utf8.len(sur) == 1 and utf8.codepoint(sur) == 0xD800)
        local n, p = utf8.len("\xF4\x90\x80\x80") assert(n == nil and p == 1)
        local ok, e = pcall(utf8.char, 0x110000)
        assert(e == "bad argument #1 to 'utf8.char' (value out of range)", e)
        ok, e = pcall(utf8.char, "x")
        assert(e == "bad argument #1 to 'utf8.char' (number expected, got string)", e)
        ok, e = pcall(utf8.len, "abc", 5)
        assert(e == "bad argument #2 to 'utf8.len' (initial position out of string)", e)
        local f, s, i = utf8.codes("a\u{4E2D}b")
        assert(i == 0 and select('#', f(s, 0)) == 2 and select(2, f(s, 1)) == 0x4E2D)
        assert(f(s, 2) == 5 and f("xyz", 0) == 1 and f(s, 5) == nil)
        ok, e = pcall(function() for _ in utf8.codes("ab\xff") do end end)
        assert(e:find(":%d+: invalid UTF%-8 code$"), e)
        "#,
    );
}

#[test]
fn test_utf8_matches_lutf8lib_55() {
    // Expected values from lua 5.5.1.
    run_level(
        crate::LuaLanguageLevel::Lua55,
        r#"
        local sur = "\xED\xA0\x80"
        assert(utf8.len(sur) == nil and utf8.len(sur, 1, -1, true) == 1)
        local r = {}
        for p, c in utf8.codes("\xF4\x90\x80\x80", true) do r[#r + 1] = c end
        assert(r[1] == 0x110000)
        local ok, e = pcall(function() for _ in utf8.codes(sur) do end end)
        assert(e:find("invalid UTF%-8 code$"), e)
        ok, e = pcall(utf8.codes, "\x80")
        assert(e == "bad argument #1 to 'utf8.codes' (invalid UTF-8 code)", e)
        assert(utf8.char(0x7FFFFFFF) == "\xFD\xBF\xBF\xBF\xBF\xBF")
        local f, s = utf8.codes("a\u{4E2D}b")
        assert(f(s, 2) == 5 and f(s, 1) == 2)
        "#,
    );
}
