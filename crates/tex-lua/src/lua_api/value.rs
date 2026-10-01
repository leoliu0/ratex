use std::ffi::c_void;

use crate::{FromLua, IntoLua, LuaAnyRef, LuaResult, LuaState, LuaValueKind, UserDataRef};

use crate::lua_api::{LuaFunction, LuaString, LuaTable};

/// Safe handle to any Lua value stored in the registry.
///
/// Unlike raw `LuaValue`, this keeps collectable values alive across GC cycles.
#[derive(Clone, Debug)]
pub struct Value {
    pub(crate) inner: LuaAnyRef,
}

impl Value {
    pub(crate) fn new(inner: LuaAnyRef) -> Self {
        Value { inner }
    }

    /// Returns the Lua value kind.
    #[inline]
    pub fn kind(&self) -> LuaValueKind {
        self.inner.kind()
    }

    /// Returns the Lua type name.
    #[inline]
    pub fn type_name(&self) -> &'static str {
        self.to_value().type_name()
    }

    /// Returns true if the wrapped value is nil.
    #[inline]
    pub fn is_nil(&self) -> bool {
        self.to_value().is_nil()
    }

    /// Convert the wrapped value into a Rust type.
    #[inline]
    pub fn get<T: FromLua>(&self) -> LuaResult<T> {
        self.inner.get_as()
    }

    /// Try to view the wrapped value as a table handle.
    #[inline]
    pub fn as_table(&self) -> Option<LuaTable> {
        self.inner.as_table().map(LuaTable::new)
    }

    /// Try to view the wrapped value as a function handle.
    #[inline]
    pub fn as_function(&self) -> Option<LuaFunction> {
        self.inner.as_function().map(LuaFunction::new)
    }

    /// Try to view the wrapped value as a typed userdata handle.
    #[inline]
    pub fn as_userdata<T: 'static>(&self) -> Option<UserDataRef<T>> {
        self.inner.as_userdata()
    }

    /// Try to view the wrapped value as a safe string handle.
    #[inline]
    pub fn as_string_handle(&self) -> Option<LuaString> {
        self.inner.as_string().map(LuaString::new)
    }

    /// Try to view the wrapped value as an owned string.
    #[inline]
    pub fn as_string(&self) -> Option<String> {
        self.to_value().as_str().map(str::to_owned)
    }

    #[inline]
    pub fn is_lightuserdata(&self) -> bool {
        self.to_value().is_lightuserdata()
    }

    #[inline]
    pub fn as_lightuserdata(&self) -> Option<*mut c_void> {
        self.to_value().as_lightuserdata()
    }

    /// Get the value's metatable, if present.
    #[inline]
    pub fn get_metatable(&self) -> Option<LuaTable> {
        self.inner.get_metatable().map(LuaTable::new)
    }

    /// Set or clear the value's metatable.
    #[inline]
    pub fn set_metatable(&self, metatable: Option<&LuaTable>) -> LuaResult<()> {
        self.inner
            .set_metatable(metatable.map(|table| &table.inner))
    }

    #[inline]
    pub fn to_pointer(&self) -> Option<*const c_void> {
        let value = self.to_value();
        if let Some(pointer) = value.as_lightuserdata() {
            return Some(pointer.cast_const());
        }

        match value.kind() {
            LuaValueKind::String
            | LuaValueKind::Table
            | LuaValueKind::Function
            | LuaValueKind::CFunction
            | LuaValueKind::CClosure
            | LuaValueKind::RClosure
            | LuaValueKind::Userdata
            | LuaValueKind::Thread => Some(value.raw_ptr_repr().cast()),
            LuaValueKind::Nil
            | LuaValueKind::Boolean
            | LuaValueKind::Integer
            | LuaValueKind::Float => None,
        }
    }

    /// Convert the wrapped value to a best-effort owned string.
    #[inline]
    pub fn to_string_lossy(&self) -> String {
        self.to_value()
            .as_str()
            .map(str::to_owned)
            .or_else(|| self.to_value().as_integer().map(|i| i.to_string()))
            .or_else(|| self.to_value().as_float().map(|n| n.to_string()))
            .unwrap_or_else(|| self.type_name().to_owned())
    }

    pub(crate) fn to_value(&self) -> luars::LuaValue {
        self.inner.to_value()
    }
}

impl IntoLua for Value {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        state
            .push_value(self.inner.to_value())
            .map_err(|e| format!("{:?}", e))?;
        Ok(1)
    }
}

impl IntoLua for &Value {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        state
            .push_value(self.inner.to_value())
            .map_err(|e| format!("{:?}", e))?;
        Ok(1)
    }
}

impl FromLua for Value {
    fn from_lua(value: luars::LuaValue, state: &mut LuaState) -> Result<Self, String> {
        Ok(Value::new(state.to_any_ref(value)))
    }
}
