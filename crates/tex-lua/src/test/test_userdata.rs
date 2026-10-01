// Tests for the trait-based userdata system
use crate::lua_value::LuaUserdata;
use crate::lua_value::userdata_trait::{UdValue, UserDataTrait};
use crate::*;

#[test]
fn test_udvalue_conversions() {
    // From impls
    assert!(matches!(UdValue::from(42i64), UdValue::Integer(42)));
    assert!(matches!(UdValue::from(3.15f64), UdValue::Number(n) if n == 3.15));
    assert!(matches!(UdValue::from(true), UdValue::Boolean(true)));
    assert!(matches!(UdValue::from("hello"), UdValue::Str(s) if s == "hello"));

    // Option → UdValue
    let some: Option<i64> = Some(10);
    assert!(matches!(UdValue::from(some), UdValue::Integer(10)));
    let none: Option<i64> = None;
    assert!(matches!(UdValue::from(none), UdValue::Nil));

    // UdValue → Rust
    assert_eq!(UdValue::Integer(5).to_integer(), Some(5));
    assert_eq!(UdValue::Number(3.0).to_integer(), Some(3)); // exact float→int
    assert_eq!(UdValue::Number(3.5).to_integer(), None); // non-exact
    assert_eq!(UdValue::Number(2f64.powi(63)).to_integer(), None); // out of i64 range
    assert_eq!(UdValue::Integer(5).to_number(), Some(5.0));
    assert_eq!(UdValue::Str("hi".into()).to_str(), Some("hi"));
    assert!(!UdValue::Nil.to_bool());
    assert!(UdValue::Integer(0).to_bool()); // Lua truthiness
}

// ==================== Simple userdata trait (macro) ====================

struct SimpleHandle {
    id: u32,
}

crate::impl_simple_userdata!(SimpleHandle, "SimpleHandle");

#[test]
fn test_simple_userdata_macro() {
    let h = SimpleHandle { id: 42 };
    assert_eq!(h.type_name(), "SimpleHandle");

    // Simple userdata has no fields exposed
    assert!(h.get_field("id").is_none());

    // But downcast still works
    let ud = LuaUserdata::new(h);
    assert!(ud.downcast_ref::<SimpleHandle>().is_some());
    assert_eq!(ud.downcast_ref::<SimpleHandle>().unwrap().id, 42);
}

// ==================== VM Integration Tests ====================
// These tests verify that userdata is properly wired to the VM,
// so Lua scripts can access fields, set fields, and trigger metamethods.

use crate::lua_vm::{GlobalState, SafeOption};
use crate::stdlib;

// ==================== __call tests ====================

/// A callable userdata — acts like a function when called from Lua
struct Adder {
    pub base: i64,
}

impl Adder {
    fn lua_call_impl(l: &mut crate::lua_vm::LuaState) -> crate::lua_vm::LuaResult<usize> {
        // arg1 = self (userdata), arg2 = value to add
        let ud = l.get_arg(1).unwrap();
        let ud_ref = ud.as_userdata_mut().unwrap();
        let adder = ud_ref.downcast_ref::<Adder>().unwrap();
        let base = adder.base;

        let val = l.get_arg(2).and_then(|v| v.as_integer()).unwrap_or(0);

        l.push_value(crate::lua_value::LuaValue::integer(base + val))?;
        Ok(1)
    }
}

impl UserDataTrait for Adder {
    fn type_name(&self) -> &'static str {
        "Adder"
    }

    fn get_field(&self, key: &str) -> Option<UdValue> {
        match key {
            "base" => Some(UdValue::Integer(self.base)),
            _ => None,
        }
    }

    fn lua_call(&self) -> Option<crate::LuaCFunction> {
        Some(crate::LuaCFunction(Adder::lua_call_impl))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[test]
fn test_userdata_call_basic() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(stdlib::Stdlib::Basic).unwrap();

    let adder = Adder { base: 100 };
    let ud = LuaUserdata::new(adder);
    let state = vm.main_state();
    let ud_val = state.create_userdata(ud).unwrap();
    state.set_global_value("add100", ud_val).unwrap();

    let results = vm.main_state().execute("return add100(42)").unwrap();
    assert_eq!(results[0].as_integer(), Some(142));
}

#[test]
fn test_userdata_call_multiple_args() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(stdlib::Stdlib::Basic).unwrap();

    let adder = Adder { base: 10 };
    let ud = LuaUserdata::new(adder);
    let state = vm.main_state();
    let ud_val = state.create_userdata(ud).unwrap();
    state.set_global_value("add10", ud_val).unwrap();

    // Can use field access and call on the same userdata
    let results = vm
        .main_state()
        .execute(
            r#"
        local base = add10.base
        local result = add10(5)
        return base, result
    "#,
        )
        .unwrap();
    assert_eq!(results[0].as_integer(), Some(10));
    assert_eq!(results[1].as_integer(), Some(15));
}

#[test]
fn test_userdata_call_in_expression() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(stdlib::Stdlib::Basic).unwrap();

    let adder = Adder { base: 1 };
    let ud = LuaUserdata::new(adder);
    let state = vm.main_state();
    let ud_val = state.create_userdata(ud).unwrap();
    state.set_global_value("inc", ud_val).unwrap();

    // Use callable userdata in expressions
    let results = vm.main_state().execute("return inc(10) + inc(20)").unwrap();
    assert_eq!(results[0].as_integer(), Some(32)); // (1+10) + (1+20)
}

/// A multi-return callable userdata
struct Splitter;

impl Splitter {
    fn lua_call_impl(l: &mut crate::lua_vm::LuaState) -> crate::lua_vm::LuaResult<usize> {
        // arg1 = self, arg2 = number to split into quotient and remainder by 10
        let val = l.get_arg(2).and_then(|v| v.as_integer()).unwrap_or(0);
        l.push_value(crate::lua_value::LuaValue::integer(val / 10))?;
        l.push_value(crate::lua_value::LuaValue::integer(val % 10))?;
        Ok(2)
    }
}

impl UserDataTrait for Splitter {
    fn type_name(&self) -> &'static str {
        "Splitter"
    }

    fn lua_call(&self) -> Option<crate::LuaCFunction> {
        Some(crate::LuaCFunction(Splitter::lua_call_impl))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[test]
fn test_userdata_call_multi_return() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(stdlib::Stdlib::Basic).unwrap();

    let splitter = Splitter;
    let ud = LuaUserdata::new(splitter);
    let state = vm.main_state();
    let ud_val = state.create_userdata(ud).unwrap();
    state.set_global_value("split10", ud_val).unwrap();

    let results = vm.main_state().execute("return split10(47)").unwrap();
    assert_eq!(results[0].as_integer(), Some(4)); // 47 / 10
    assert_eq!(results[1].as_integer(), Some(7)); // 47 % 10
}

// ==================== Manual lua_close test ====================

struct ManualClose {
    closed: bool,
}

impl UserDataTrait for ManualClose {
    fn type_name(&self) -> &'static str {
        "ManualClose"
    }
    fn lua_close(&mut self) {
        self.closed = true;
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[test]
fn test_manual_lua_close_direct() {
    let mut mc = ManualClose { closed: false };
    assert!(!mc.closed);
    mc.lua_close();
    assert!(mc.closed);
}

#[test]
fn test_manual_lua_close_via_lua_tbc() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(Stdlib::All).unwrap();
    let mc = ManualClose { closed: false };
    {
        let state = vm.main_state();
        let ud_val = state.create_userdata(LuaUserdata::new(mc)).unwrap();
        state.set_global_value("r", ud_val).unwrap();
    }
    // <close> fires lua_close() when r goes out of scope
    vm.main_state()
        .execute(r#"do local x <close> = r end"#)
        .unwrap();
    // Verify lua_close was called via downcasting
    let state = vm.main_state();
    let r_val = state.get_global_value("r").unwrap().unwrap();
    let manual_close = r_val
        .as_userdata_mut()
        .unwrap()
        .downcast_ref::<ManualClose>()
        .unwrap();
    assert!(manual_close.closed);
}

