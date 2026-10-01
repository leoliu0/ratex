// Tests for string library functions
use crate::*;

#[test]
fn test_string_gsub_function() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        -- Test 1: Simple function replacement
        local s1, n1 = string.gsub("hello world", "%w+", function(w)
            return string.upper(w)
        end)
        assert(s1 == "HELLO WORLD")
        assert(n1 == 2)

        -- Test 2: Function with captures
        local s2, n2 = string.gsub("foo=123 bar=456", "(%w+)=(%d+)", function(k, v)
            return k .. ":" .. (tonumber(v) * 2)
        end)
        assert(s2 == "foo:246 bar:912")
        assert(n2 == 2)

        -- Test 3: Function returning nil keeps original
        local s3, n3 = string.gsub("keep drop keep", "%w+", function(w)
            if w == "drop" then return nil end
            return w
        end)
        assert(s3 == "keep drop keep")
        assert(n3 == 3)

        -- Test 4: Function returning number
        local s4, n4 = string.gsub("a b c", "%w", function(c)
            return string.byte(c)
        end)
        assert(s4 == "97 98 99")
        assert(n4 == 3)

        -- Test 5: Limit replacements
        local s5, n5 = string.gsub("aaaa", "a", function() return "b" end, 2)
        assert(s5 == "bbaa")
        assert(n5 == 2)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_len() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(string.len("hello") == 5)
        assert(string.len("") == 0)
        assert(#"hello" == 5)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_sub() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(string.sub("hello", 2, 4) == "ell")
        assert(string.sub("hello", 2) == "ello")
        assert(string.sub("hello", -2) == "lo")
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_upper_lower() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(string.upper("hello") == "HELLO")
        assert(string.lower("WORLD") == "world")
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_rep() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(string.rep("ab", 3) == "ababab")
        assert(string.rep("x", 0) == "")
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_reverse() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(string.reverse("hello") == "olleh")
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_byte_char() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(string.byte("A") == 65)
        assert(string.char(65) == "A")
        assert(string.char(65, 66, 67) == "ABC")
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_format() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(string.format("%d", 42) == "42")
        assert(string.format("%s", "hello") == "hello")
        assert(string.format("%d %s", 10, "test") == "10 test")
        assert(string.format("%s", 1.0) == "1.0")
        assert(string.format("%s", 1.25) == "1.25")
        assert(string.format("%f", 1.5) == "1.500000")
        assert(string.format("%q", 1.5) == "0x1.8p+0")
        local quoted = string.format("%q", string.char(255, 0, 49))
        assert(quoted == '"\255\\0001"')
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_find() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local i, j = string.find("hello world", "world")
        assert(i == 7)
        assert(j == 11)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_match() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local m = string.match("hello 123", "%d+")
        assert(m == "123")
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_gmatch() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local words = {}
        for w in string.gmatch("one two three", "%w+") do
            table.insert(words, w)
        end
        assert(words[1] == "one")
        assert(words[2] == "two")
        assert(words[3] == "three")
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_gsub() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local s, n = string.gsub("hello world", "l", "L")
        assert(s == "heLLo worLd")
        assert(n == 3)
        
        local s2, n2 = string.gsub("hello", "l", "L", 1)
        assert(s2 == "heLlo")
        assert(n2 == 1)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_pack_unpack() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local packed = string.pack("bhi", 127, 32767, 2147483647)
        assert(type(packed) == "string")
        
        local b, h, i = string.unpack("bhi", packed)
        assert(b == 127)
        assert(h == 32767)
        assert(i == 2147483647)
    "#,
    );

    if let Err(e) = &result {
        eprintln!("Error: {}", e);
    }
    assert!(result.is_ok());
}

#[test]
fn test_string_packsize() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        assert(string.packsize("b") == 1)
        assert(string.packsize("h") == 2)
        assert(string.packsize("i") == 4)
        assert(string.packsize("bhi") == 7)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_string_pack_short_byte_string_equality() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local lstr = "\1\2\3\4\5\6\7\8"
        local lnum = 0x0807060504030201

        for i = 1, 8 do
            local n = lnum & (~(-1 << (i * 8)))
            local s = string.sub(lstr, 1, i)
            assert(string.pack("<i" .. i, n) == s)
            assert(string.pack(">i" .. i, n) == s:reverse())
        end
    "#,
    );

    if let Err(e) = &result {
        eprintln!("Error: {}", e);
    }
    assert!(result.is_ok());
}

fn run_lua(level: LuaLanguageLevel, source: &str) {
    let mut vm = GlobalState::new_with_language(SafeOption::default(), level);
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let state = vm.main_state();
    if let Err(error) = state.execute(source) {
        let message = state.get_error_msg(error);
        panic!("{level}: {message}");
    }
}

fn run_both(source: &str) {
    run_lua(LuaLanguageLevel::Lua53, source);
    run_lua(LuaLanguageLevel::Lua55, source);
}

#[test]
fn test_format_matches_c_printf() {
    // Expected values come from texlua (Lua 5.3) and lua 5.5 on glibc.
    run_both(
        r#"
        local f = string.format
        local function eq(a, b) if a ~= b then error(("%q ~= %q"):format(a, b), 2) end end
        eq(f("%e", 0), "0.000000e+00")
        eq(f("%22.14e", 3.14159265358979), "  3.14159265358979e+00")
        eq(f("%-18.7e|", 0), "0.0000000e+00     |")
        eq(f("%010.2f", -2.5), "-000002.50")
        eq(f("%.0f %.0f", 0.5, 2.5), "0 2")
        eq(f("%g %g %g %g", 1e20, 100000, 1e-5, 0.0001), "1e+20 100000 1e-05 0.0001")
        eq(f("%.3g|%.3g|%.10g", 0.0001234, 99950, 1/3), "0.000123|1e+05|0.3333333333")
        eq(f("%.14g", 0.1), "0.1")
        eq(f("%.17g", 0.03), "0.029999999999999999")
        eq(f("%5.1f|%-8.3f|", 3.14159, 2), "  3.1|2.000   |")
        eq(f("%a %a %A", 1, 5e-324, 1e300), "0x1p+0 0x0.0000000000001p-1022 0X1.7E43C8800759CP+996")
        eq(f("%.0a %.1a", 1.5, 1.96875), "0x2p+0 0x2.0p+0")
        eq(f("%.3d|%05d|%+d|% d", 7, -42, 3, 3), "007|-0042|+3| 3")
        eq(f("%x %X %o %#x %#o", 255, 255, 8, 255, 8), "ff FF 10 0xff 010")
        eq(f("%.5x|%-6.2x|", 7, 255), "00007|ff    |")
        eq(f("%x", -1), "ffffffffffffffff")
        eq(f("%5s|%-5s|%.2s", "ab", "ab", "abc"), "   ab|ab   |ab")
        eq(f("%c%c", 256 + 65, 66), "AB")
        eq(f("%d %s", 3.0, 1e15), "3 1e+15")
        eq(f("%q", math.mininteger), "0x8000000000000000")
        eq(f("%q", "\r\n\0001\200"), '"\\13\\\n\\0001\200"')
        eq(f("%s", setmetatable({}, {__tostring = function() return 42 end})), "42")
        local ok, err = pcall(f, "%d", 3.5)
        assert(not ok and err:find("number has no integer representation", 1, true))
        ok, err = pcall(f, "%F", 1)
        assert(not ok)
        ok, err = pcall(f, "%100d", 1)
        assert(not ok)
    "#,
    );
}

#[test]
fn test_format_dialect_differences() {
    run_lua(
        LuaLanguageLevel::Lua53,
        r#"
        assert(string.format("%#d|%05s|%.3c|%5q", 7, "ab", 65, "x") == '7|   ab|A|"x"')
        assert(string.format("%q %q %q", 1/0, -1/0, 1.5) == "inf -inf 0x1.8p+0")
        assert(select(2, pcall(string.format, "%------d", 1)):find("repeated flags", 1, true))
        assert(select(2, pcall(string.format, "%p", {})):find("invalid option '%p'", 1, true))
    "#,
    );
    run_lua(
        LuaLanguageLevel::Lua55,
        r#"
        for _, spec in ipairs{"%#d", "%05s", "%.3c", "%5.2p"} do
            local ok, err = pcall(string.format, spec, 1)
            assert(not ok and err:find("invalid conversion specification", 1, true), spec)
        end
        assert(select(2, pcall(string.format, "%5q", "x")):find("cannot have modifiers", 1, true))
        assert(string.format("%q %q %q", 1/0, -1/0, 0/0) == "1e9999 -1e9999 (0/0)")
        assert(string.format("%------d", 1) == "1")
        assert(string.format("%p", 1) == "(null)")
    "#,
    );
}

#[test]
fn test_tostring_uses_dialect_float_format() {
    run_lua(
        LuaLanguageLevel::Lua53,
        r#"
        assert(tostring(0.1 + 0.2) == "0.3")
        assert(tostring(2^63) == "9.2233720368548e+18")
        assert(1e100 .. "" == "1e+100")
        assert(table.concat({1.0, 2, -0.0, 0.5}, ",") == "1.0,2,-0.0,0.5")
    "#,
    );
    run_lua(
        LuaLanguageLevel::Lua55,
        r#"
        assert(tostring(0.1 + 0.2) == "0.30000000000000004")
        assert(tostring(1e15) == "1e+15")
        assert(tostring(2^63) == "9.2233720368547758e+18")
        assert(table.concat({1.0, 1e100}, ",") == "1.0,1e+100")
    "#,
    );
}

#[test]
fn test_pattern_anchors_and_gmatch() {
    run_both(
        r#"
        local s, n = string.gsub("hello", "^", "x")
        assert(s == "xhello" and n == 1)
        s, n = string.gsub("aaa", "^a", "b")
        assert(s == "baa" and n == 1)
        local count = 0
        for _ in ("aaa"):gmatch("^a") do count = count + 1 end
        assert(count == 0) -- '^' is not an anchor in gmatch
        count = 0
        for _ in ("a^a"):gmatch("^a") do count = count + 1 end
        assert(count == 1)
        assert(("x\vy"):find("%s") == 2)
        assert(("abc"):find("x[") == nil) -- malformed part never reached
    "#,
    );
    run_lua(
        LuaLanguageLevel::Lua53,
        r#"
        local t = {}
        for w in string.gmatch("abcabc", "a", 2) do t[#t + 1] = w end
        assert(#t == 2) -- Lua 5.3 has no init argument
    "#,
    );
    run_lua(
        LuaLanguageLevel::Lua55,
        r#"
        local t = {}
        for w in string.gmatch("abcabc", "a", 2) do t[#t + 1] = w end
        assert(#t == 1)
    "#,
    );
}

#[test]
fn test_pattern_and_replacement_errors() {
    run_both(
        r#"
        local function err(f, ...)
            local ok, e = pcall(f, ...)
            assert(not ok)
            return e
        end
        assert(err(string.find, "a", "[%") == "malformed pattern (missing ']')")
        assert(err(string.find, "xa", "x[") == "malformed pattern (missing ']')")
        assert(err(string.match, ("a"):rep(40), ("(a)"):rep(33)) == "too many captures")
        assert(err(string.gsub, "hello", "l", "a%") == "invalid use of '%' in replacement string")
        assert(err(string.gsub, "hello", "l", "%2") == "invalid capture index %2")
        assert(err(string.gsub, "hello", "l", "L", 1.5):find("number has no integer representation", 1, true))
        local t = {}
        local ok, e = pcall(string.gsub, "abc", "b", function() error(t) end)
        assert(not ok and e == t) -- errors in the replacement propagate unchanged
        local r, n = string.gsub("hello", "l", "L", -1)
        assert(r == "hello" and n == 0)
        r, n = string.gsub("abc", "b", 42)
        assert(r == "a42c" and n == 1)
        r = string.gsub("abc", "%w", {a = 1, b = false})
        assert(r == "1bc")
        local where = err(function() string.rep() end)
        assert(where:find("^.-:%d+: bad argument #1 to 'rep' %(string expected, got no value%)$"), where)
    "#,
    );
}

#[test]
fn test_string_functions_accept_numbers_and_bytes() {
    run_both(
        r#"
        assert(string.len(123) == 3)
        assert(string.lower(1.5) == "1.5")
        assert(string.upper("\233a") == "\233A")
        assert(string.reverse("\200\1") == "\1\200")
        assert(string.rep(1, 3, 0) == "10101")
        assert(("x"):rep(2.0) == "xx")
        assert(select('#', string.byte("hello", -3, -1)) == 3)
        assert(string.sub("hello", math.mininteger, math.maxinteger) == "hello")
        assert(tonumber(" -ff ", 16) == -255 and tonumber("- ff", 16) == nil)
        assert(tonumber("1e1", 10) == nil and tonumber("0x1_0") == nil)
    "#,
    );
}
