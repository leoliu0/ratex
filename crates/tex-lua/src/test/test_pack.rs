// Tests for string.pack / string.unpack / string.packsize
use crate::*;

fn run(level: LuaLanguageLevel, code: &str) {
    let mut vm = GlobalState::new_with_language(SafeOption::default(), level);
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let result = vm.main_state().execute(code);
    assert!(result.is_ok(), "{level:?}: {:?}", result);
}

fn run_all(code: &str) {
    run(LuaLanguageLevel::Lua53, code);
    run(LuaLanguageLevel::Lua55, code);
}

#[test]
fn test_pack_range_checks() {
    run_all(
        r#"
        local function err(fmt, v)
          local ok, e = pcall(string.pack, fmt, v)
          return not ok and e:match("%((.-)%)$")
        end
        assert(err("b", 128) == "integer overflow" and err("b", -129) == "integer overflow")
        assert(err("B", 256) == "unsigned overflow" and err("B", -1) == "unsigned overflow")
        assert(err("h", 32768) == "integer overflow" and err("H", 65536) == "unsigned overflow")
        assert(err("i3", 0x800000) == "integer overflow" and string.pack("<i3", -0x800000) == "\0\0\x80")
        assert(string.pack("<l", math.mininteger) == "\0\0\0\0\0\0\0\x80")
        assert(string.pack("<L", -1) == ("\xff"):rep(8) and string.pack("<J", -1) == ("\xff"):rep(8))
        assert(string.pack("<i16", -2) == "\xfe" .. ("\xff"):rep(15))
        assert(string.unpack("<i16", string.pack("<i16", math.mininteger)) == math.mininteger)
        assert(not pcall(string.unpack, "<i9", ("\0"):rep(8) .. "\1"))
        assert(err("i", 3.5) == "number has no integer representation")
        assert(err("s1", ("x"):rep(256)) == "string length does not fit in given size")
        "#,
    );
}

#[test]
fn test_pack_argument_conversions() {
    run_all(
        r#"
        assert(string.pack("<i", "10") == "\10\0\0\0")
        assert(string.pack("<d", "2.5") == string.pack("<d", 2.5))
        assert(string.pack("<s1", 12) == "\2" .. "12")
        assert(string.pack("z", 7) == "7\0")
        local ok, e = pcall(string.pack, "ii", 1)
        assert(not ok and e:find("bad argument #3 to", 1, true) and e:find("(number expected, got nil)", 1, true))
        "#,
    );
}

#[test]
fn test_unpack_dialect_differences() {
    run(
        LuaLanguageLevel::Lua53,
        r#"
        local s, pos = string.unpack("z", "abc")
        assert(s == "abc" and pos == 5)
        assert(not pcall(string.unpack, "b", "abc", 0))
        assert(not pcall(string.unpack, "b", "abc", -10))
        local ok, e = pcall(string.packsize, "c2147483647")
        assert(not ok and e:find("invalid format option '7'", 1, true))
        "#,
    );
    run(
        LuaLanguageLevel::Lua55,
        r#"
        assert(not pcall(string.unpack, "z", "abc"))
        assert(string.unpack("b", "abc", 0) == 97 and string.unpack("b", "abc", -10) == 97)
        assert(string.packsize("c2147483647 c2") == 2147483649)
        "#,
    );
}
