#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::io::Write;
    use std::path::PathBuf;

    use crate::{
        GlobalState, LuaApi, LuaAsyncApi, LuaValueKind, SafeOption, Stdlib, UdValue,
        UserDataTrait,
        lua_api::{Lua, LuaFunction, LuaTable},
    };
    #[cfg(feature = "sandbox")]
    use crate::{LuaSandboxApi, SandboxConfig};

    struct ApiCounter {
        count: i64,
    }

    impl UserDataTrait for ApiCounter {
        fn type_name(&self) -> &'static str {
            "ApiCounter"
        }

        fn get_field(&self, key: &str) -> Option<UdValue> {
            match key {
                "count" => Some(UdValue::Integer(self.count)),
                "inc" => Some(UdValue::Function(crate::LuaCFunction(api_counter_inc))),
                "get" => Some(UdValue::Function(crate::LuaCFunction(api_counter_get))),
                _ => None,
            }
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    fn api_counter_inc(l: &mut crate::LuaState) -> crate::LuaResult<usize> {
        let delta = l.get_arg(2).and_then(|v| v.as_integer()).unwrap_or(0);
        let ud = l.get_arg(1).unwrap_or_default();
        if let Some(counter) = ud.as_userdata_mut().and_then(|ud| ud.downcast_mut::<ApiCounter>()) {
            counter.count += delta;
        }
        Ok(0)
    }

    fn api_counter_get(l: &mut crate::LuaState) -> crate::LuaResult<usize> {
        let ud = l.get_arg(1).unwrap_or_default();
        let count = ud
            .as_userdata_mut()
            .and_then(|ud| ud.downcast_ref::<ApiCounter>())
            .map_or(0, |counter| counter.count);
        l.push_value(crate::LuaValue::integer(count))?;
        Ok(1)
    }

    fn test_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join("luars_api_tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn eval_and_typed_globals_work() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        lua.set_global("name", "Lua").unwrap();

        let result: String = lua.eval("return 'hello ' .. name").unwrap();
        assert_eq!(result, "hello Lua");
    }

    #[test]
    fn register_and_call_typed_function() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        lua.register_function("sum", |a: i64, b: i64| a + b)
            .unwrap();

        let result: i64 = lua.eval("return sum(20, 22)").unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn call_global_for_lua_defined_function() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();
        lua.load("function mul(a, b) return a * b end")
            .exec()
            .unwrap();

        let result: i64 = lua.call_global1("mul", (6, 7)).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn high_level_collect_garbage_works() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        lua.load(
            r#"
            local t = {}
            for i = 1, 200 do
                t[i] = { index = i, payload = string.rep("x", 32) }
            end
            t = nil
            "#,
        )
        .exec()
        .unwrap();

        lua.collect_garbage().unwrap();

        let answer: i64 = lua.eval("return 40 + 2").unwrap();
        assert_eq!(answer, 42);
    }

    #[test]
    fn safe_table_round_trip() {
        let mut lua = Lua::new(SafeOption::default());
        let table = lua.create_table_with_capacity(0, 2).unwrap();
        table.set("host", "localhost").unwrap();
        table.set("port", 8080_i64).unwrap();
        lua.globals().set("config", &table).unwrap();

        let config = lua.globals().get::<LuaTable>("config").unwrap();
        let host: String = config.get("host").unwrap();
        let port: i64 = config.get("port").unwrap();

        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
    }

    #[test]
    fn globals_and_generic_table_api_feel_like_mlua() {
        let mut lua = Lua::new(SafeOption::default());
        let globals = lua.globals();

        globals.set("host", "localhost").unwrap();
        globals.set("port", 8080_i64).unwrap();

        assert!(globals.contains_key("host").unwrap());
        assert_eq!(globals.get::<String>("host").unwrap(), "localhost");
        assert_eq!(globals.raw_get::<i64>("port").unwrap(), 8080);
    }

    #[test]
    fn create_table_from_and_sequence_from_work() {
        let mut lua = Lua::new(SafeOption::default());

        let config = lua
            .create_table_from([("host", "localhost"), ("mode", "dev")])
            .unwrap();
        let seq = lua.create_sequence_from([10_i64, 20_i64, 30_i64]).unwrap();

        assert_eq!(config.get::<String>("host").unwrap(), "localhost");
        assert_eq!(config.pairs::<String, String>().unwrap().len(), 2);
        assert_eq!(seq.sequence_values::<i64>().unwrap(), vec![10, 20, 30]);
    }

    #[test]
    fn create_function_and_convert_helpers_work() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let double = lua.create_function(|x: i64| x * 2).unwrap();
        lua.globals().set("double", double.clone()).unwrap();

        let packed = lua.pack("42").unwrap();
        let unpacked: String = lua.unpack(packed).unwrap();
        let converted: i64 = lua.convert(123_i64).unwrap();
        let result: i64 = lua.eval("return double(21)").unwrap();

        assert_eq!(unpacked, "42");
        assert_eq!(converted, 123);
        assert_eq!(result, 42);
    }

    #[test]
    fn safe_function_upvalue_helpers_work() {
        let mut lua = Lua::new(SafeOption::default());

        let first: LuaFunction = lua
            .load(
                r#"
                local value = 40
                return function(x)
                    return value + x
                end
                "#,
            )
            .eval()
            .unwrap();

        let second: LuaFunction = lua
            .load(
                r#"
                local value = 10
                return function(x)
                    return value + x
                end
                "#,
            )
            .eval()
            .unwrap();

        assert_eq!(first.upvalue_count(), 1);
        let (name, current) = first.get_upvalue::<i64>(1).unwrap().unwrap();
        assert_eq!(name, "value");
        assert_eq!(current, 40);
        assert!(first.get_upvalue::<i64>(2).unwrap().is_none());

        let first_id = first.upvalue_id(1).unwrap();
        let second_id = second.upvalue_id(1).unwrap();
        assert_ne!(first_id, second_id);

        assert_eq!(
            first.set_upvalue(1, 41_i64).unwrap().as_deref(),
            Some("value")
        );
        assert_eq!(first.call1::<_, i64>(1_i64).unwrap(), 42);

        assert!(first.join_upvalue(1, &second, 1).unwrap());
        assert_eq!(first.upvalue_id(1).unwrap(), second_id);

        second.set_upvalue(1, 39_i64).unwrap();
        assert_eq!(first.call1::<_, i64>(3_i64).unwrap(), 42);
        assert_eq!(second.call1::<_, i64>(3_i64).unwrap(), 42);
    }

    #[test]
    fn table_objectlike_helpers_work() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let obj: LuaTable = lua
            .load(
                r#"
                return {
                    nested = { answer = 42 },
                    add = function(a, b) return a + b end,
                    scale = function(self, x) return self.factor * x end,
                    factor = 3,
                }
                "#,
            )
            .eval()
            .unwrap();

        assert_eq!(obj.get_path::<i64>(&["nested", "answer"]).unwrap(), 42);
        assert_eq!(obj.call_function::<_, i64>("add", (20, 22)).unwrap(), 42);
        assert_eq!(obj.call_method1::<_, i64>("scale", 14_i64).unwrap(), 42);
    }

    #[test]
    fn high_level_metatable_helpers_work() {
        let mut lua = Lua::new(SafeOption::default());

        let defaults = lua.create_table_from([("answer", 42_i64)]).unwrap();
        let metatable = lua.create_table().unwrap();
        metatable.set("__index", defaults).unwrap();

        let table = lua.create_table().unwrap();
        assert!(!table.has_metatable());
        table.set_metatable(Some(&metatable)).unwrap();

        assert!(table.has_metatable());
        assert!(table.get_metatable().is_some());
        lua.globals().set("t", &table).unwrap();
        let answer: i64 = lua.eval("return t.answer").unwrap();
        assert_eq!(answer, 42);

        let value = lua.pack(table.clone()).unwrap();
        assert!(value.get_metatable().is_some());
    }

    #[test]
    fn high_level_type_metatable_helpers_work() {
        let mut lua = Lua::new(SafeOption::default());

        let index = lua.create_table_from([("tag", "custom-string")]).unwrap();
        let metatable = lua.create_table().unwrap();
        metatable.set("__index", index).unwrap();

        lua.set_type_metatable(LuaValueKind::String, Some(&metatable))
            .unwrap();

        let string_mt = lua.get_type_metatable(LuaValueKind::String).unwrap();
        let index: LuaTable = string_mt.get("__index").unwrap();
        assert_eq!(index.get::<String>("tag").unwrap(), "custom-string");

        let tag: String = lua.eval("return ('hello').tag").unwrap();
        assert_eq!(tag, "custom-string");
    }

    #[test]
    fn lua_api_extra_space_and_dofile_work() {
        let dir = test_temp_dir();
        let path = dir.join("lua_api_dofile.lua");
        {
            let mut file = std::fs::File::create(&path).unwrap();
            writeln!(file, "return 40 + 2").unwrap();
        }

        let mut lua = Lua::new(SafeOption::default());
        let raw = 0x1234usize as *mut c_void;
        lua.set_extra_space(raw);
        assert_eq!(lua.extra_space(), raw);

        let answer: i64 = lua.dofile(path.to_str().unwrap()).unwrap();
        assert_eq!(answer, 42);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn lua_api_metatable_helpers_work() {
        let mut lua = Lua::new(SafeOption::default());

        let defaults = lua.create_table_from([("answer", 42_i64)]).unwrap();
        let metatable = lua.create_table().unwrap();
        metatable.set("__index", defaults).unwrap();

        let table = lua.create_table().unwrap();
        assert!(table.get_metatable().is_none());
        table.set_metatable(Some(&metatable)).unwrap();
        assert!(table.get_metatable().is_some());

        lua.globals().set("t", &table).unwrap();
        let answer: i64 = lua.eval("return t.answer").unwrap();
        assert_eq!(answer, 42);
    }

    #[test]
    fn safe_value_handle_supports_string_and_downcasts() {
        let mut lua = Lua::new(SafeOption::default());

        let string_value = lua.pack("hello").unwrap();
        let table = lua.create_table_from([("answer", 42_i64)]).unwrap();
        let table_value = lua.pack(table).unwrap();
        let userdata = lua.create_userdata(ApiCounter { count: 1 }).unwrap();
        let userdata_value = lua.pack(userdata.clone()).unwrap();

        assert_eq!(string_value.type_name(), "string");
        assert_eq!(string_value.as_string().unwrap(), "hello");
        assert_eq!(string_value.to_string_lossy(), "hello");
        assert_eq!(
            string_value.as_string_handle().unwrap().as_str().as_deref(),
            Some("hello")
        );

        let table = table_value.as_table().unwrap();
        assert_eq!(table.get::<i64>("answer").unwrap(), 42);

        let counter = userdata_value.as_userdata::<ApiCounter>().unwrap();
        assert_eq!(counter.borrow().unwrap().count, 1);

        let converted: String = string_value.get().unwrap();
        assert_eq!(converted, "hello");
    }

    #[test]
    fn high_level_userdata_api_works() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let counter = lua.create_userdata(ApiCounter { count: 1 }).unwrap();
        lua.globals().set("counter", counter.clone()).unwrap();
        lua.load("counter:inc(41)").exec().unwrap();

        assert_eq!(counter.borrow().unwrap().count, 42);
        assert_eq!(lua.load("return counter:get()").eval::<i64>().unwrap(), 42);
    }

    #[test]
    fn lua_state_now_implements_lua_api() {
        let mut vm = GlobalState::new(SafeOption::default());
        let state = vm.main_state();

        state.open_stdlib(Stdlib::All).unwrap();
        LuaApi::set_global(state, "base", 40_i64).unwrap();

        let answer: i64 = state.eval("return base + 2").unwrap();
        assert_eq!(answer, 42);

        let doubled: i64 = LuaApi::load(state, "return 21 * 2").eval().unwrap();
        assert_eq!(doubled, 42);
    }

    #[test]
    fn lua_state_lua_api_supports_extra_space_and_dofile() {
        let dir = test_temp_dir();
        let path = dir.join("lua_state_api_dofile.lua");
        {
            let mut file = std::fs::File::create(&path).unwrap();
            writeln!(file, "return 6 * 7").unwrap();
        }

        let mut vm = GlobalState::new(SafeOption::default());
        let state = vm.main_state();
        let raw = 0x5678usize as *mut c_void;
        state.set_extra_space(raw);
        assert_eq!(state.extra_space(), raw);

        let answer: i64 = LuaApi::dofile(state, path.to_str().unwrap()).unwrap();
        assert_eq!(answer, 42);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn scope_supports_non_static_functions() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let base = 40_i64;
        lua.scope(|scope| {
            let add_base = scope.create_function_with(&base, |base: &i64, x: i64| x + *base)?;
            scope.globals().set("add_base", &add_base)?;

            let result: i64 = scope.load("return add_base(2)").eval()?;
            assert_eq!(result, 42);
            Ok(())
        })
        .unwrap();

        assert!(lua.load("return add_base(1)").eval::<i64>().is_err());
    }

    #[test]
    fn scope_function_with_borrowed_state_works() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let total = Cell::new(0_i64);
        lua.scope(|scope| {
            let push = scope.create_function_with(&total, |total: &Cell<i64>, delta: i64| {
                total.set(total.get() + delta);
                total.get()
            })?;
            scope.globals().set("push_total", &push)?;

            let value: i64 = scope
                .load("return push_total(19) + push_total(23)")
                .eval()?;
            assert_eq!(value, 61);
            Ok(())
        })
        .unwrap();

        assert_eq!(total.get(), 42);
    }

    #[test]
    fn scope_function_mut_with_borrowed_state_works() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let mut total = 0_i64;
        lua.scope(|scope| {
            let push =
                scope.create_function_mut_with(&mut total, |total: &mut i64, delta: i64| {
                    *total += delta;
                    *total
                })?;
            scope.globals().set("push_total_mut", &push)?;

            let value: i64 = scope
                .load("return push_total_mut(19) + push_total_mut(23)")
                .eval()?;
            assert_eq!(value, 61);
            Ok(())
        })
        .unwrap();

        assert_eq!(total, 42);
    }

    #[test]
    fn scope_mut_callback_rejects_reentrant_call() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let mut calls = 0_i64;
        let result = lua.scope(|scope| {
            let bump = scope.create_function_mut_with(
                &mut calls,
                |calls: &mut i64, again: LuaFunction| -> crate::LuaResult<i64> {
                    *calls += 1;
                    again.call::<_, ()>(())?;
                    Ok(*calls)
                },
            )?;
            scope.globals().set("bump", &bump)?;
            scope
                .load("return bump(function() bump(function() end) end)")
                .eval::<i64>()
        });

        let err = result.unwrap_err();
        let message = lua.get_error_message(err).message;
        assert!(message.contains("called recursively"), "{message}");
        assert_eq!(calls, 1);
    }

    #[test]
    fn callback_error_rethrows_original_lua_error_value() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let call = lua
            .create_function(|f: LuaFunction| -> crate::LuaResult<()> { f.call::<_, ()>(()) })
            .unwrap();
        lua.set_global("call", call).unwrap();
        let (code, message): (i64, String) = lua
            .load(
                r#"
                local _, e1 = pcall(call, function() error({code = 7}) end)
                local _, e2 = pcall(call, function() error("boom", 0) end)
                return e1.code, e2
                "#,
            )
            .eval_multi()
            .unwrap();
        assert_eq!(code, 7);
        assert_eq!(message, "boom");
    }

    #[test]
    fn typed_integer_args_follow_lua_checkinteger() {
        let mut lua = Lua::new_lua53(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();
        let id = lua.create_function(|n: i64| n).unwrap();
        lua.set_global("id", id).unwrap();
        let byte = lua.create_function(|n: u8| n).unwrap();
        lua.set_global("byte", byte).unwrap();

        let (a, b, c): (i64, i64, i64) = lua
            .load("return id(3.0), id('10'), id(' 0x10 ')")
            .eval_multi()
            .unwrap();
        assert_eq!((a, b, c), (3, 10, 16));
        let (e1, e2, e3): (String, String, String) = lua
            .load(
                r#"
                local _, e1 = pcall(id, 2.5)
                local _, e2 = pcall(byte, 300)
                local _, e3 = pcall(id, {})
                return e1, e2, e3
                "#,
            )
            .eval_multi()
            .unwrap();
        assert!(e1.contains("bad argument #1 to 'id' (number has no integer representation)"), "{e1}");
        assert!(e2.contains("out of range"), "{e2}");
        assert!(e3.contains("number expected, got table"), "{e3}");

        assert!(lua.set_global("big", u64::MAX).is_err());
    }

    #[test]
    fn typed_string_args_use_lua_number_format_and_reject_invalid_utf8() {
        let mut lua = Lua::new_lua53(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();
        let echo = lua.create_function(|s: String| s).unwrap();
        lua.set_global("echo", echo).unwrap();

        let (a, b, c): (String, String, String) = lua
            .load("return echo(1.0), echo(1e100), echo(-0.0)")
            .eval_multi()
            .unwrap();
        assert_eq!((a.as_str(), b.as_str(), c.as_str()), ("1.0", "1e+100", "-0.0"));
        let err: String = lua
            .load(r#"local _, e = pcall(echo, "\255") return e"#)
            .eval()
            .unwrap();
        assert!(err.contains("string is not valid UTF-8"), "{err}");
    }

    #[test]
    fn vec_return_counts_every_pushed_value() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();
        let pairs = lua.create_function(|| vec![(1_i64, 2_i64), (3, 4)]).unwrap();
        lua.set_global("pairs2", pairs).unwrap();
        let joined: String = lua
            .load("return table.concat({pairs2()}, ',')")
            .eval()
            .unwrap();
        assert_eq!(joined, "1,2,3,4");
    }

    #[test]
    fn chunk_builder_exec_eval_and_into_function_work() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        lua.load("answer = 41").set_name("init.lua").exec().unwrap();
        let answer: i64 = lua.load("return answer + 1").eval().unwrap();
        let add = lua
            .load("local a, b = ...; return a + b")
            .set_name("adder.lua")
            .into_function()
            .unwrap();

        assert_eq!(answer, 42);
        assert_eq!(add.call1::<_, i64>((20, 22)).unwrap(), 42);
    }

    #[tokio::test]
    async fn high_level_async_api_exec_and_call_work() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        lua.register_async_function("double_async", |x: i64| async move { Ok(x * 2) })
            .unwrap();
        lua.load(
            r#"
            function add_async(a, b)
                return double_async(a + b)
            end
            "#,
        )
        .exec()
        .unwrap();

        let chunk_value: i64 = lua
            .load("return double_async(21)")
            .eval_async()
            .await
            .unwrap();
        let global_value: i64 = lua
            .call_async_global1("add_async", (20_i64, 1_i64))
            .await
            .unwrap();
        let compiled: LuaFunction = lua
            .load("return function(x) return double_async(x) end")
            .eval()
            .unwrap();
        let function_value: i64 = lua.call_async1(&compiled, 21_i64).await.unwrap();

        assert_eq!(chunk_value, 42);
        assert_eq!(global_value, 42);
        assert_eq!(function_value, 42);
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn high_level_sandbox_api_supports_injected_globals_and_isolation() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();
        lua.register_function("greet", |name: String| format!("hello, {name}"))
            .unwrap();

        let mut config = SandboxConfig::default();
        lua.sandbox_capture_global(&mut config, "greet").unwrap();
        let value: String = lua
            .load_sandboxed(
                r#"
                sandbox_value = 41
                return greet("sandbox")
                "#,
                &config,
            )
            .eval()
            .unwrap();

        assert_eq!(value, "hello, sandbox");
        assert!(lua.get_global::<i64>("sandbox_value").unwrap().is_none());
    }

    #[test]
    fn table_and_function_convert_from_lua() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let table: LuaTable = lua
            .load("return { host = 'localhost', port = 8080 }")
            .eval()
            .unwrap();
        let function: LuaFunction = lua
            .load("return function(x) return x * 2 end")
            .eval()
            .unwrap();

        assert_eq!(table.get::<String>("host").unwrap(), "localhost");
        assert_eq!(table.get::<i64>("port").unwrap(), 8080);
        assert_eq!(function.call1::<_, i64>(21).unwrap(), 42);
    }

    #[test]
    fn high_level_lua_install_library_works() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let module = crate::lua_module!("hostlib", {
            "answer" => |l| {
                l.push_value(crate::LuaValue::integer(42))?;
                Ok(1)
            },
            value "name" => |vm| vm.create_string("hostlib"),
        });

        lua.install_library(module).unwrap();

        let answer: i64 = lua.load("return hostlib.answer()").eval().unwrap();
        let name: String = lua.load("return hostlib.name").eval().unwrap();
        assert_eq!(answer, 42);
        assert_eq!(name, "hostlib");
    }

    #[test]
    fn high_level_lua_install_preload_library_works() {
        let mut lua = Lua::new(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        lua.install_library(crate::lua_preload_module!("test_install_module" => |l| {
            let table = l.create_table(0, 1)?;
            let key = l.create_string("value")?;
            l.global_state_mut().raw_set(&table, key, crate::LuaValue::integer(42));
            l.push_value(table)?;
            Ok(1)
        }))
        .unwrap();

        let value: i64 = lua
            .load("local mod = require('test_install_module'); return mod.value")
            .eval()
            .unwrap();

        assert_eq!(value, 42);
    }

    #[test]
    fn test_userdata() {
        #[derive(Clone, Debug)]
        struct RustStruct {
            a: i32,
            b: i32,
        }
        crate::impl_simple_userdata!(RustStruct, "RustStruct");

        let mut l = Lua::new(SafeOption::default());
        let t = l.create_table().unwrap();
        let value = l.create_userdata(RustStruct { a: 1, b: 2 }).unwrap();
        t.set(1, value).unwrap();
        let seq = t.sequence_values::<RustStruct>().unwrap();
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0].a, 1);
        assert_eq!(seq[0].b, 2);
    }

    #[test]
    fn lua53_contract_rejects_newer_syntax_and_exposes_53_libraries() {
        let mut lua = Lua::new_lua53(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();

        let surface: LuaTable = lua
            .load(
                r#"
                return {
                    version = _VERSION,
                    bit = bit32.band(0xffffffff, bit32.bnot(0xf0)),
                    no_warn = warn == nil,
                    no_table_create = table.create == nil,
                    no_coroutine_close = coroutine.close == nil,
                    math_compat = type(math.atan2) == "function"
                        and type(math.log10) == "function",
                    seed_returns_none = select('#', math.randomseed(1)) == 0,
                    random_zero_rejected = not pcall(math.random, 0),
                }
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(surface.get::<String>("version").unwrap(), "Lua 5.3");
        assert_eq!(surface.get::<i64>("bit").unwrap(), 0xffff_ff0f);
        assert!(surface.get::<bool>("no_warn").unwrap());
        assert!(surface.get::<bool>("no_table_create").unwrap());
        assert!(surface.get::<bool>("no_coroutine_close").unwrap());
        assert!(surface.get::<bool>("math_compat").unwrap());
        assert!(surface.get::<bool>("seed_returns_none").unwrap());
        assert!(surface.get::<bool>("random_zero_rejected").unwrap());

        for source in [
            "local value <const> = 1",
            "local function f(...named) return named end",
            "global value",
        ] {
            assert!(
                lua.load(source).exec().is_err(),
                "accepted Lua 5.5 syntax: {source}"
            );
        }
    }

    #[test]
    fn lua53_integer_bytes_metatables_and_loop_assignment_match_contract() {
        let mut lua = Lua::new_lua53(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();
        let values: LuaTable = lua
            .load(
                r#"
                local total = 0
                for i = 1, 2 do
                    i = 7
                    total = total + i
                end
                local mt = { __idiv = function() return 91 end }
                local left = setmetatable({}, mt)
                local bytes = "a\0b"
                local generic = 0
                for key, value in ipairs({2, 3}) do
                    key = 10
                    generic = generic + key + value
                end
                return {
                    total = total,
                    generic = generic,
                    floor = -7 // 3,
                    wrapped = 0x7fffffffffffffff + 1,
                    metamethod = left // {},
                    byte_length = #bytes,
                    nul = string.byte(bytes, 2),
                    rotate = bit32.rrotate(1, 1),
                }
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(values.get::<i64>("total").unwrap(), 14);
        assert_eq!(values.get::<i64>("generic").unwrap(), 25);
        assert_eq!(values.get::<i64>("floor").unwrap(), -3);
        assert_eq!(values.get::<i64>("wrapped").unwrap(), i64::MIN);
        assert_eq!(values.get::<i64>("metamethod").unwrap(), 91);
        assert_eq!(values.get::<i64>("byte_length").unwrap(), 3);
        assert_eq!(values.get::<i64>("nul").unwrap(), 0);
        assert_eq!(values.get::<i64>("rotate").unwrap(), 0x8000_0000);
    }

    fn stdlib_lua() -> Lua {
        let mut lua = Lua::new_lua53(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();
        lua
    }

    #[test]
    fn handles_become_inert_when_their_lua_is_dropped() {
        let mut lua = stdlib_lua();
        let table = lua.create_table().unwrap();
        table.set("k", 1).unwrap();
        let function = lua.load_function("return 1").unwrap();
        let string = lua.create_string("a string long enough to be a long string").unwrap();
        let value: crate::Value = lua.eval("return {}").unwrap();
        let counter = lua.create_userdata(ApiCounter { count: 3 }).unwrap();
        drop(lua);

        assert_eq!(table.set("k", 2), Err(crate::LuaError::StateClosed));
        assert_eq!(table.get::<i64>("k"), Err(crate::LuaError::StateClosed));
        assert_eq!(function.call::<_, i64>(()), Err(crate::LuaError::StateClosed));
        assert!(string.as_bytes().is_none());
        assert!(value.is_nil());
        assert!(counter.borrow().is_err());
        let copy = table.clone();
        drop((table, function, string, value, counter, copy));
    }

    #[test]
    fn string_borrow_stays_valid_after_the_lua_is_dropped() {
        let mut lua = stdlib_lua();
        let text = "a string long enough to live in its own allocation";
        let string = lua.create_string(text).unwrap();
        let bytes = string.as_bytes().unwrap();
        drop(lua);
        assert_eq!(&*bytes, text.as_bytes());
    }

    #[test]
    fn borrowed_string_is_rooted_even_if_its_registry_slot_is_cleared() {
        let mut lua = stdlib_lua();
        let string: crate::LuaString = lua
            .eval("return string.rep('rooted ', 10)")
            .unwrap();
        let bytes = string.as_str().unwrap();
        // Lua code can rewrite the registry through the debug library.
        lua.execute(
            "local r = debug.getregistry()
             for k, v in pairs(r) do
               if type(k) == 'number' and type(v) == 'string' then r[k] = nil end
             end
             collectgarbage()
             local junk = {} for i = 1, 100 do junk[i] = string.rep('x', 70) .. i end",
        )
        .unwrap();
        assert_eq!(&*bytes, "rooted ".repeat(10));
    }

    #[test]
    fn userdata_borrows_are_shared_by_clones_and_checked_like_refcell() {
        let mut lua = stdlib_lua();
        let counter = lua.create_userdata(ApiCounter { count: 1 }).unwrap();
        let alias = counter.clone();

        let mut exclusive = counter.borrow_mut().unwrap();
        assert!(alias.borrow().is_err(), "a clone must see the exclusive borrow");
        assert!(alias.borrow_mut().is_err());
        exclusive.count = 5;
        drop(exclusive);

        let shared = counter.borrow().unwrap();
        let shared_too = alias.borrow().unwrap();
        assert!(alias.borrow_mut().is_err());
        assert_eq!(shared.count + shared_too.count, 10);
        drop((shared, shared_too));
        assert_eq!(alias.borrow_mut().unwrap().count, 5);
    }

    #[test]
    fn lua_access_to_a_host_borrowed_userdata_panics_instead_of_aliasing() {
        let mut lua = stdlib_lua();
        let counter = lua.create_userdata(ApiCounter { count: 1 }).unwrap();
        lua.set_global("counter", &counter).unwrap();

        let shared = counter.borrow().unwrap();
        // Reading through Lua is compatible with a shared host borrow ...
        assert_eq!(lua.eval::<i64>("return counter.count").unwrap(), 1);
        // ... mutation is not.
        let mutate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = lua.execute("counter:inc(1)");
        }));
        assert!(mutate.is_err());
        assert_eq!(shared.count, 1);
        drop(shared);

        let exclusive = counter.borrow_mut().unwrap();
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = lua.eval::<i64>("return counter.count");
        }));
        assert!(read.is_err());
        drop(exclusive);
    }

    #[test]
    fn values_from_another_state_are_rejected() {
        let mut first = stdlib_lua();
        let mut second = stdlib_lua();
        let table = first.create_table().unwrap();
        assert!(second.set_global("t", &table).is_err());
        assert!(second.create_table().unwrap().set_metatable(Some(&table)).is_err());
        assert!(second.set_type_metatable(LuaValueKind::String, Some(&table)).is_err());
        assert!(second.eval::<bool>("return t == nil").unwrap());
    }

    #[test]
    fn variadic_callback_arguments_and_results() {
        let mut lua = stdlib_lua();
        let f = lua
            .create_function(|sep: String, rest: crate::Variadic<crate::Value>| {
                let parts: Vec<String> = rest.iter().map(|v| v.to_string_lossy()).collect();
                crate::Variadic(vec![parts.join(&sep), rest.len().to_string()])
            })
            .unwrap();
        lua.set_global("join", f).unwrap();
        let (joined, count): (String, String) =
            lua.eval_multi("return join('-', 'a', 2, 'c')").unwrap();
        assert_eq!((joined.as_str(), count.as_str()), ("a-2-c", "3"));
        let none: (String, String) = lua.eval_multi("return join('-')").unwrap();
        assert_eq!(none, (String::new(), "0".to_string()));
    }

    #[test]
    fn lua_bytes_round_trip_arbitrary_bytes() {
        let mut lua = stdlib_lua();
        let raw = vec![0xff, 0x00, b'a', 0xe9];
        lua.set_global("raw", crate::LuaBytes(raw.clone())).unwrap();
        assert_eq!(lua.eval::<i64>("return #raw").unwrap(), 4);
        let back: crate::LuaString = lua.eval("return raw").unwrap();
        assert_eq!(back.to_bytes(), raw);
        assert!(back.as_str().is_none());
        let made = lua.create_bytes(&raw).unwrap();
        assert_eq!(made.to_bytes(), raw);
    }
}
