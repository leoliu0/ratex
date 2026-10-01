//! Callbacks that can allocate: [`Lua::create_callback`] runs a Rust closure
//! with a [`CallbackLua`], the running call's view of the VM. Unlike typed
//! callbacks (`create_function`), such a closure can create tables, byte
//! strings and userdata, and it reads its arguments and pushes its results
//! one by one, with Lua's own argument errors.
//!
//! `CallbackLua` borrows the call's state for the duration of the call, so it
//! cannot escape the closure or be used on another VM. Everything it creates
//! is handed out as a rooted handle ([`LuaTable`], [`LuaString`],
//! [`UserDataRef`], [`Value`]) that the existing liveness checks police, and
//! results are anchored on the call's stack as they are pushed.

use crate::lua_api::{LuaApi, LuaFunction, LuaString, LuaTable, Lua, Value};
use crate::stdlib::lauxlib;
use crate::{
    FromLua, IntoLua, LuaError, LuaResult, LuaState, LuaValueKind, UserDataRef, UserDataTrait,
};

/// The running native call: its arguments, its result stack and allocation.
pub struct CallbackLua<'a> {
    state: &'a mut LuaState,
}

impl CallbackLua<'_> {
    /// Number of arguments passed to the call.
    #[inline]
    pub fn arg_count(&self) -> usize {
        self.state.arg_count()
    }

    /// Kind of argument `index` (1-based); `None` when it was not passed.
    #[inline]
    pub fn arg_kind(&self, index: usize) -> Option<LuaValueKind> {
        self.state.get_arg(index).map(|value| value.kind())
    }

    /// Argument `index` (1-based) converted to `T`; an absent argument is nil.
    /// A failed conversion raises Lua's "bad argument #n to 'f' (...)" error.
    pub fn arg<T: FromLua>(&mut self, index: usize) -> LuaResult<T> {
        let value = self.state.get_arg(index).unwrap_or_default();
        T::from_lua(value, self.state)
            .map_err(|message| lauxlib::argerror(self.state, index, &message))
    }

    /// `luaL_argerror`.
    pub fn arg_error(&mut self, index: usize, message: &str) -> LuaError {
        lauxlib::argerror(self.state, index, message)
    }

    /// `luaL_typeerror`: "`expected` expected, got <type of argument>".
    pub fn type_error(&mut self, index: usize, expected: &str) -> LuaError {
        lauxlib::typeerror(self.state, index, expected)
    }

    /// `luaL_error`: an error message prefixed with the caller's position.
    pub fn error(&mut self, message: impl AsRef<str>) -> LuaError {
        lauxlib::lual_error(self.state, message)
    }

    /// Push `value` as a result of the call; returns the number of values
    /// pushed, to be added to the closure's result count.
    pub fn push<T: IntoLua>(&mut self, value: T) -> LuaResult<usize> {
        self.state.push_multi(value)
    }

    pub fn create_table(&mut self) -> LuaResult<LuaTable> {
        LuaApi::create_table(self.state)
    }

    pub fn create_table_with_capacity(&mut self, narr: usize, nrec: usize) -> LuaResult<LuaTable> {
        LuaApi::create_table_with_capacity(self.state, narr, nrec)
    }

    pub fn create_bytes(&mut self, bytes: &[u8]) -> LuaResult<LuaString> {
        LuaApi::create_bytes(self.state, bytes)
    }

    pub fn create_userdata<T: UserDataTrait + 'static>(
        &mut self,
        data: T,
    ) -> LuaResult<UserDataRef<T>> {
        LuaApi::create_userdata(self.state, data)
    }

    /// Root any convertible Rust value as a handle.
    pub fn pack<T: IntoLua>(&mut self, value: T) -> LuaResult<Value> {
        LuaApi::pack(self.state, value)
    }
}

impl Lua {
    /// Create a Lua function from a closure that reads its arguments from
    /// and pushes its results through a [`CallbackLua`]. The closure returns
    /// the number of results it pushed.
    pub fn create_callback<F>(&mut self, f: F) -> LuaResult<LuaFunction>
    where
        F: Fn(&mut CallbackLua<'_>) -> LuaResult<usize> + 'static,
    {
        self.create_raw_function(move |state| f(&mut CallbackLua { state }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LuaBytes, SafeOption, Stdlib, UserDataTrait};

    struct Counter(i64);

    impl UserDataTrait for Counter {
        fn type_name(&self) -> &'static str {
            "counter"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    fn lua() -> Lua {
        let mut lua = Lua::new_lua53(SafeOption::default());
        lua.open_stdlib(Stdlib::All).unwrap();
        lua
    }

    // A callback allocates a table, a binary string and a userdata, and
    // returns them; they survive a full collection and are usable after it.
    #[test]
    fn callback_created_values_survive_collection() {
        let mut lua = lua();
        let make = lua
            .create_callback(|cx| {
                let n: i64 = cx.arg(1)?;
                let table = cx.create_table()?;
                table.raw_seti(1, LuaBytes(vec![0xFF, 0x00, b'a']))?;
                let counter = cx.create_userdata(Counter(n))?;
                let bytes = cx.create_bytes(&[0x80, 0x81])?;
                Ok(cx.push(table)? + cx.push(counter)? + cx.push(bytes)?)
            })
            .unwrap();
        lua.set_global("make", make).unwrap();
        let ok: bool = lua
            .load(
                "local t, u, s = make(7)
                 collectgarbage() collectgarbage()
                 return type(t) == 'table' and #t[1] == 3 and t[1]:byte(1) == 255
                   and type(u) == 'userdata' and s == '\\x80\\x81'",
            )
            .eval()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn callback_argument_errors_are_lua_argument_errors() {
        let mut lua = lua();
        let f = lua
            .create_callback(|cx| {
                let _: i64 = cx.arg(1)?;
                Ok(0)
            })
            .unwrap();
        lua.set_global("f", f).unwrap();
        let message: String = lua
            .load("local ok, err = pcall(f, {}) return err")
            .eval()
            .unwrap();
        assert!(message.starts_with("bad argument #1 to 'f'"), "{message}");
    }

    // A handle made on one VM is rejected when pushed from a callback on another.
    #[test]
    fn callback_rejects_values_of_another_vm() {
        let mut other = lua();
        let foreign = other.create_table().unwrap();
        let mut lua = lua();
        let f = lua
            .create_callback(move |cx| {
                cx.push(foreign.clone())?;
                Ok(1)
            })
            .unwrap();
        lua.set_global("f", f).unwrap();
        let rejected: bool = lua.load("return not pcall(f)").eval().unwrap();
        assert!(rejected);
    }
}
