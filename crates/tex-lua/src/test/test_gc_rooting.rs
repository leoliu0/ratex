// Values native library code holds across calls into Lua must stay rooted:
// the Lua code it runs can drop every other reference and collect garbage.
// Freed objects are reused by the allocations that follow, so a dangling
// value shows up as the wrong contents.
use crate::*;

fn run(code: &str) {
    let mut vm = GlobalState::new_with_language(SafeOption::default(), LuaLanguageLevel::Lua53);
    vm.open_stdlib(Stdlib::All).unwrap();
    if let Err(err) = vm.main_state().execute(code) {
        panic!("{}", vm.main_state().get_full_error(err));
    }
}

#[test]
fn table_sort_keeps_elements_alive_while_the_comparator_clears_the_table() {
    run(r#"
        local t = {}
        for i = 1, 40 do t[i] = {41 - i} end
        local churn = {}
        table.sort(t, function(a, b)
            for k in pairs(t) do t[k] = nil end
            collectgarbage()
            for i = 1, 20 do churn[#churn % 200 + 1] = {-1} end
            return a[1] < b[1]
        end)
        for i = 1, 40 do assert(t[i][1] == i, "element " .. i .. " was freed") end
    "#);
}

#[test]
fn table_sort_keeps_index_results_alive_until_written_back() {
    run(r#"
        local n = 60
        local store = {}
        local proxy = setmetatable({}, {
            __len = function() return n end,
            __index = function(_, i)
                local junk = {} for j = 1, 30 do junk[j] = {-1} end
                return {n + 1 - i}
            end,
            __newindex = function(_, i, v) store[i] = v end,
        })
        table.sort(proxy, function(a, b) return a[1] < b[1] end)
        for i = 1, n do assert(store[i][1] == i, "element " .. i .. " was freed") end
    "#);
}

#[test]
fn table_remove_keeps_the_removed_element_alive_during_the_shift() {
    run(r#"
        local data = {}
        for i = 1, 30 do data[i] = {i} end
        local proxy = setmetatable({}, {
            __len = function() return #data end,
            __index = function(_, k) return data[k] end,
            __newindex = function(_, k, v)
                data[k] = v
                collectgarbage()
                local junk = {} for j = 1, 30 do junk[j] = {-1} end
            end,
        })
        local removed = table.remove(proxy, 1)
        assert(removed[1] == 1, "removed element was freed")
        assert(#data == 29 and data[1][1] == 2)
    "#);
}

#[test]
fn require_keeps_searcher_data_alive_across_the_loader() {
    // Lua 5.5 returns the searcher's extra value from require.
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(Stdlib::All).unwrap();
    let result = vm.main_state().execute(r#"
        table.insert(package.searchers, 1, function(name)
            if name == "rooted_mod" then
                return function()
                    -- the loader takes no parameters: its locals overwrite the
                    -- (name, data) argument slots before the collection
                    local a, b, c, d = 1, 2, 3, 4
                    collectgarbage()
                    local junk = {} for j = 1, 50 do junk[j] = {tag = "junk"} end
                    return {}
                end, {tag = "data"}
            end
        end)
        local _, data = require("rooted_mod")
        assert(data.tag == "data", "loader data was freed")
    "#);
    if let Err(err) = result {
        panic!("{}", vm.main_state().get_full_error(err));
    }
}

#[test]
fn native_function_overstating_its_results_is_an_error_not_a_panic() {
    let mut vm = GlobalState::new_with_language(SafeOption::default(), LuaLanguageLevel::Lua53);
    vm.open_stdlib(Stdlib::All).unwrap();
    vm.register_function("liar", |l| {
        l.push_value(LuaValue::integer(1))?;
        Ok(1000)
    })
    .unwrap();
    let result = vm
        .main_state()
        .execute("local ok, err = pcall(liar) return ok, err");
    let values = result.unwrap();
    assert_eq!(values[0].as_boolean(), Some(false));
    assert!(values[1].as_str().unwrap().contains("returned 1000 results but pushed only 1"));
}
