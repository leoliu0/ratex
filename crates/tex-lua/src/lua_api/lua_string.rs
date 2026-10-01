use crate::{Borrowed, FromLua, IntoLua, LuaState, LuaStringRef, LuaValue};

/// Safe handle to a Lua string kept alive in the registry.
#[derive(Clone, Debug)]
pub struct LuaString {
    pub(crate) inner: LuaStringRef,
}

impl LuaString {
    pub(crate) fn new(inner: LuaStringRef) -> Self {
        LuaString { inner }
    }

    /// Borrow the string as UTF-8. `None` if it is not valid UTF-8 or the
    /// owning `Lua` was dropped.
    #[inline]
    pub fn as_str(&self) -> Option<Borrowed<'_, str>> {
        self.inner.as_str()
    }

    /// Borrow the raw bytes. `None` if the owning `Lua` was dropped.
    #[inline]
    pub fn as_bytes(&self) -> Option<Borrowed<'_, [u8]>> {
        self.inner.as_bytes()
    }

    /// Copy the raw bytes (empty if the owning `Lua` was dropped).
    #[inline]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.as_bytes().map(|bytes| bytes.to_vec()).unwrap_or_default()
    }

    #[inline]
    pub fn to_string_lossy(&self) -> String {
        self.inner.to_string_lossy()
    }

    #[inline]
    pub fn byte_len(&self) -> usize {
        self.inner.byte_len()
    }
}

impl IntoLua for LuaString {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        self.inner.push_into(state)
    }
}

impl IntoLua for &LuaString {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        self.inner.push_into(state)
    }
}

impl FromLua for LuaString {
    fn from_lua(value: LuaValue, state: &mut LuaState) -> Result<Self, String> {
        let actual = value.type_name();
        let string = state
            .global_state_mut()
            .to_string_ref(value)
            .ok_or_else(|| format!("expected string, got {}", actual))?;
        Ok(LuaString::new(string))
    }
}

/// Arbitrary bytes pushed to Lua as a string (Lua strings are byte strings).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LuaBytes(pub Vec<u8>);

impl IntoLua for LuaBytes {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        let value = state
            .create_bytes(&self.0)
            .map_err(|e| format!("{:?}", e))?;
        state.push_value(value).map_err(|e| format!("{:?}", e))?;
        Ok(1)
    }
}
