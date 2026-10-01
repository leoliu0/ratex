// Tests for table library functions
use crate::*;

#[test]
fn test_table_insert() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = {1, 2, 3}
        table.insert(t, 4)
        assert(t[4] == 4)
        
        table.insert(t, 2, 99)
        assert(t[2] == 99)
        assert(t[3] == 2)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_table_remove() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = {10, 20, 30, 40}
        local v = table.remove(t, 2)
        assert(v == 20)
        assert(t[2] == 30)
        assert(#t == 3)
        
        local last = table.remove(t)
        assert(last == 40)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_table_concat() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = {"a", "b", "c"}
        assert(table.concat(t) == "abc")
        assert(table.concat(t, ",") == "a,b,c")
        assert(table.concat(t, "-", 2, 3) == "b-c")
    "#,
    );

    if let Err(e) = &result {
        eprintln!("Error: {}", e);
    }
    assert!(result.is_ok());
}

#[test]
fn test_table_concat_binary_separator() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local sep = string.char(255)
        local t = {"A", "B", "C"}
        local out = table.concat(t, sep)
        assert(#out == 5)
        assert(string.byte(out, 1) == 65)
        assert(string.byte(out, 2) == 255)
        assert(string.byte(out, 3) == 66)
        assert(string.byte(out, 4) == 255)
        assert(string.byte(out, 5) == 67)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_table_sort() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = {3, 1, 4, 1, 5}
        table.sort(t)
        assert(t[1] == 1)
        assert(t[2] == 1)
        assert(t[3] == 3)
        assert(t[4] == 4)
        assert(t[5] == 5)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_table_sort_with_comparator() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = {3, 1, 4, 1, 5}
        table.sort(t, function(a, b) return a > b end)
        assert(t[1] == 5)
        assert(t[2] == 4)
        assert(t[3] == 3)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_table_sort_byte_strings() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = {"\xE1lo", "\0first :-)", "alo", "then this one", "45", "and a new"}
        table.sort(t)

        for i = #t, 2, -1 do
          assert(not (t[i] < t[i - 1]))
        end
    "#,
    );

    if let Err(e) = &result {
        eprintln!("Error: {}", e);
    }
    assert!(result.is_ok());
}

#[test]
fn test_table_pack() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = table.pack(1, 2, 3)
        assert(t[1] == 1)
        assert(t[2] == 2)
        assert(t[3] == 3)
        assert(t.n == 3)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_table_unpack() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = {10, 20, 30}
        local a, b, c = table.unpack(t)
        assert(a == 10)
        assert(b == 20)
        assert(c == 30)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_table_move() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t1 = {1, 2, 3, 4, 5}
        local t2 = {}
        table.move(t1, 2, 4, 1, t2)
        assert(t2[1] == 2)
        assert(t2[2] == 3)
        assert(t2[3] == 4)
    "#,
    );

    assert!(result.is_ok());
}

#[test]
fn test_table_operations() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        -- Test table creation and access
        local t = {}
        t.name = "test"
        t[1] = "first"
        t["key"] = "value"
        
        assert(t.name == "test")
        assert(t[1] == "first")
        assert(t["key"] == "value")
        
        -- Test length operator
        local arr = {1, 2, 3, 4, 5}
        assert(#arr == 5)
    "#,
    );

    assert!(result.is_ok());
}

// ===== Table library metamethod tests =====

#[test]
fn test_table_insert_with_metamethods() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        -- Proxy table: all reads/writes go to backing table via __index/__newindex
        local backing = {}
        local proxy = setmetatable({}, {
            __len = function() return #backing end,
            __index = backing,
            __newindex = backing,
        })

        table.insert(proxy, 10)
        table.insert(proxy, 20)
        table.insert(proxy, 1, 5)

        assert(backing[1] == 5, "insert metamethod: backing[1]")
        assert(backing[2] == 10, "insert metamethod: backing[2]")
        assert(backing[3] == 20, "insert metamethod: backing[3]")
        assert(#backing == 3, "insert metamethod: length")
    "#,
    );

    assert!(
        result.is_ok(),
        "table.insert with metamethods failed: {:?}",
        result
    );
}

#[test]
fn test_table_remove_with_metamethods() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local backing = {10, 20, 30, 40}
        local proxy = setmetatable({}, {
            __len = function() return #backing end,
            __index = backing,
            __newindex = backing,
        })

        local removed = table.remove(proxy, 2)
        assert(removed == 20, "remove metamethod: removed value")
        assert(backing[1] == 10, "remove metamethod: backing[1]")
        assert(backing[2] == 30, "remove metamethod: backing[2]")
        assert(backing[3] == 40, "remove metamethod: backing[3]")
    "#,
    );

    assert!(
        result.is_ok(),
        "table.remove with metamethods failed: {:?}",
        result
    );
}

#[test]
fn test_table_sort_with_metamethods() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local backing = {5, 3, 1, 4, 2}
        local proxy = setmetatable({}, {
            __len = function() return #backing end,
            __index = backing,
            __newindex = backing,
        })

        table.sort(proxy)
        for i = 1, 5 do
            assert(backing[i] == i, "sort metamethod: backing[" .. i .. "]")
        end
    "#,
    );

    assert!(
        result.is_ok(),
        "table.sort with metamethods failed: {:?}",
        result
    );
}

#[test]
fn test_table_concat_with_metamethods() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        -- __index function that returns computed values
        local t = setmetatable({}, {
            __index = function(_, k) return k + 1 end,
            __len = function() return 5 end,
        })

        local s = table.concat(t, ";")
        assert(s == "2;3;4;5;6", "concat metamethod: got " .. s)
    "#,
    );

    assert!(
        result.is_ok(),
        "table.concat with metamethods failed: {:?}",
        result
    );
}

#[test]
fn test_table_unpack_with_metamethods() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local backing = {100, 200, 300}
        local proxy = setmetatable({}, {
            __len = function() return #backing end,
            __index = backing,
        })

        local a, b, c = table.unpack(proxy)
        assert(a == 100 and b == 200 and c == 300,
               "unpack metamethod: " .. tostring(a) .. "," .. tostring(b) .. "," .. tostring(c))
    "#,
    );

    assert!(
        result.is_ok(),
        "table.unpack with metamethods failed: {:?}",
        result
    );
}

#[test]
fn test_table_move_with_metamethods() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local src = setmetatable({}, {
            __index = {10, 20, 30, 40, 50},
        })
        local dst = {}

        table.move(src, 1, 5, 1, dst)
        assert(dst[1] == 10 and dst[3] == 30 and dst[5] == 50,
               "move metamethod failed")
    "#,
    );

    assert!(
        result.is_ok(),
        "table.move with metamethods failed: {:?}",
        result
    );
}

#[test]
fn test_table_insert_overflow_with_len_metamethod() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local t = setmetatable({},
                    {__len = function() return math.maxinteger end})
        table.insert(t, 42)
        local k, v = next(t)
        assert(k == math.mininteger and v == 42,
               "insert overflow: k=" .. tostring(k) .. " v=" .. tostring(v))
    "#,
    );

    assert!(
        result.is_ok(),
        "table.insert overflow with __len failed: {:?}",
        result
    );
}

#[test]
fn test_table_full_proxy_workflow() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    // Full workflow matching the C Lua test: insert, sort, concat, remove, unpack
    let result = vm.main_state().execute(
        r#"
        local t = {}
        local proxy = setmetatable({}, {
            __len = function() return #t end,
            __index = t,
            __newindex = t,
        })

        for i = 1, 10 do
            table.insert(proxy, 1, i)
        end
        assert(#proxy == 10 and #t == 10)

        table.sort(proxy)
        for i = 1, 10 do
            assert(t[i] == i and proxy[i] == i)
        end

        assert(table.concat(proxy, ",") == "1,2,3,4,5,6,7,8,9,10")

        for i = 1, 8 do
            assert(table.remove(proxy, 1) == i)
        end
        assert(#proxy == 2 and #t == 2)

        local a, b, c = table.unpack(proxy)
        assert(a == 9 and b == 10 and c == nil)
    "#,
    );

    assert!(result.is_ok(), "full proxy workflow failed: {:?}", result);
}

#[test]
fn test_table_newindex_counting() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    // Test that __newindex is actually triggered (counting calls)
    let result = vm.main_state().execute(
        r#"
        local count = 0
        local t = setmetatable({}, {
            __newindex = function(tbl, k, v)
                count = count + 1
                rawset(tbl, k, v)
            end
        })

        for i = 1, 10 do
            table.insert(t, 1, i)
        end

        -- Only the first 10 inserts trigger __newindex (new keys)
        assert(count == 10, "expected 10 __newindex calls, got " .. count)

        table.sort(t)
        for i = 1, 10 do
            assert(t[i] == i)
        end
    "#,
    );

    assert!(result.is_ok(), "newindex counting failed: {:?}", result);
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

#[test]
fn test_concat_reads_each_element_once_and_reports_bad_values() {
    for level in [LuaLanguageLevel::Lua53, LuaLanguageLevel::Lua55] {
        run_lua(
            level,
            r#"
            local reads = 0
            local t = setmetatable({}, {
                __index = function(_, i) reads = reads + 1 return "v" .. i end,
                __len = function() return 3 end,
            })
            assert(table.concat(t, "-") == "v1-v2-v3" and reads == 3)
            local ok, err = pcall(table.concat, {1, {}, 3})
            assert(not ok and err == "invalid value (table) at index 2 in table for 'concat'", err)
            assert(table.concat({}, ",", math.maxinteger, math.maxinteger - 1) == "")
            local ok2, err2 = pcall(function() return table.concat({}, {}) end)
            assert(err2:find("^chunk:%d+: bad argument #2 to 'concat' %(string expected, got table%)$"), err2)
            "#,
        );
    }
}

#[test]
fn test_table_argument_errors_follow_dialect() {
    run_lua(
        LuaLanguageLevel::Lua53,
        r#"
        local ok, err = pcall(function() return table.remove({1, 2}, 5) end)
        assert(err:find("bad argument #1 to 'remove' (position out of bounds)", 1, true), err)
        "#,
    );
    run_lua(
        LuaLanguageLevel::Lua55,
        r#"
        local ok, err = pcall(function() return table.remove({1, 2}, 5) end)
        assert(err:find("bad argument #2 to 'remove' (position out of bounds)", 1, true), err)
        ok, err = pcall(function() return table.move({}, 1, math.maxinteger, 2) end)
        assert(err:find("bad argument #4 to 'move' (destination wrap around)", 1, true), err)
        ok, err = pcall(function() return table.insert({}, "x", 1, 2) end)
        assert(err:find("^chunk:%d+: wrong number of arguments to 'insert'$"), err)
        "#,
    );
}

/// Run `code` at both language levels; it must not raise.
fn run_both_levels(code: &str) {
    for level in [LuaLanguageLevel::Lua53, LuaLanguageLevel::Lua55] {
        let mut vm = GlobalState::new_with_language(SafeOption::default(), level);
        vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
        let result = vm.main_state().execute(code);
        assert!(result.is_ok(), "{level}: {result:?}");
    }
}

#[test]
fn test_table_sort_matches_ltablib_for_invalid_orders() {
    // Expected values come from texlua (5.3.6) and lua 5.5.1, which agree.
    run_both_levels(
        r#"
        -- comparators that are not strict weak orders never panic: they either
        -- give some permutation or ltablib's error
        local errs = {}
        for _, n in ipairs{2, 3, 4, 5, 10, 50, 200} do
          local t = {} for i = 1, n do t[i] = (i * 7919) % n end
          local ok, e = pcall(table.sort, t, function() return true end)
          errs[#errs + 1] = ok and "ok" or e
          local t2 = {} for i = 1, n do t2[i] = (i * 7919) % n end
          ok = pcall(table.sort, t2, function(a, b) return a <= b end)
          errs[#errs + 1] = tostring(ok)
        end
        local bad = "invalid order function for sorting"
        assert(table.concat(errs, ",") == "ok,true,ok,true," .. bad .. ",false," .. bad
          .. ",true," .. bad .. ",false," .. bad .. ",false," .. bad .. ",false",
          table.concat(errs, ","))
        -- NaN in the default order
        local t = {} for i = 1, 300 do t[i] = (i % 3 == 0) and 0/0 or (i * 37) % 101 end
        assert(pcall(table.sort, t))
        local t = {3, 0/0, 1, 0/0, 2, 5, 0/0, 4}
        assert(pcall(table.sort, t))
        -- the comparator sequence is ltablib's
        local c = 0
        local t = {} for i = 1, 100 do t[i] = (i * 31) % 100 end
        table.sort(t, function(a, b) c = c + 1 return a < b end)
        assert(c == 626, c)
        -- comparator type
        local ok, e = pcall(table.sort, {3, 2, 1}, 3)
        assert(e:find("bad argument #2 to 'table.sort' (function expected, got number)", 1, true), e)
        assert(pcall(table.sort, {1}, 3))
        -- mixed integers and floats compare exactly above 2^53
        local t = {9007199254740993, 2.0^53, 9007199254740991, 1.5, 1}
        table.sort(t)
        assert(t[3] == 9007199254740991 and t[4] == 2.0^53 and t[5] == 9007199254740993
          and math.type(t[5]) == "integer")
        local t = {math.maxinteger, 2.0^63, math.mininteger, -2.0^63, 0.5, 0}
        table.sort(t)
        assert(t[1] == -2.0^63 or t[1] == math.mininteger)
        assert(t[5] == math.maxinteger and t[6] == 2.0^63)
        "#,
    );
}
