use crate::{FromLua, FromLuaMulti, IntoLua, LuaFunction, LuaResult, LuaTableRef, LuaValue};

/// Safe wrapper around a Lua table handle.
#[derive(Clone, Debug)]
pub struct LuaTable {
    pub(crate) inner: LuaTableRef,
}

impl LuaTable {
    pub(crate) fn new(inner: LuaTableRef) -> Self {
        LuaTable { inner }
    }

    /// Get and convert a field.
    #[inline]
    pub fn get<T: FromLua>(&self, key: impl IntoLua) -> LuaResult<T> {
        self.inner.get_typed(key)
    }

    /// Look up a nested path of string keys.
    pub fn get_path<T: FromLua>(&self, path: &[&str]) -> LuaResult<T> {
        let mut current = self.clone();
        let Some((last, prefix)) = path.split_last() else {
            return Err(crate::LuaError::RuntimeError);
        };

        for key in prefix {
            current = current.get::<LuaTable>(*key)?;
        }

        current.get(*last)
    }

    /// Returns true if the table contains a non-nil value for `key`.
    #[inline]
    pub fn contains_key(&self, key: impl IntoLua) -> LuaResult<bool> {
        self.inner.contains_key(key)
    }

    /// Returns true if this table currently has a metatable.
    #[inline]
    pub fn has_metatable(&self) -> bool {
        self.inner.has_metatable()
    }

    /// Get the table's metatable, if present.
    #[inline]
    pub fn get_metatable(&self) -> Option<LuaTable> {
        self.inner.get_metatable().map(LuaTable::new)
    }

    /// Set or clear the table's metatable.
    #[inline]
    pub fn set_metatable(&self, metatable: Option<&LuaTable>) -> LuaResult<()> {
        self.inner
            .set_metatable(metatable.map(|table| &table.inner))
    }

    /// Set a field.
    #[inline]
    pub fn set(&self, key: impl IntoLua, value: impl IntoLua) -> LuaResult<()> {
        self.inner.set_typed(key, value)
    }

    /// Get a named function field and call it with `args`.
    #[inline]
    pub fn call_function<A: IntoLua, R: FromLuaMulti>(&self, name: &str, args: A) -> LuaResult<R> {
        self.get::<LuaFunction>(name)?.call(args)
    }

    /// Get a named method field and call it with `self` as the first argument.
    #[inline]
    pub fn call_method<A: IntoLua, R: FromLuaMulti>(&self, name: &str, args: A) -> LuaResult<R> {
        self.get::<LuaFunction>(name)?.call((self.clone(), args))
    }

    /// Get a named method field and call it, converting only the first result.
    #[inline]
    pub fn call_method1<A: IntoLua, R: FromLua>(&self, name: &str, args: A) -> LuaResult<R> {
        self.get::<LuaFunction>(name)?.call1((self.clone(), args))
    }

    /// Get a field without metamethods.
    #[inline]
    pub fn raw_get<T: FromLua>(&self, key: impl IntoLua) -> LuaResult<T> {
        self.inner.rawget_typed(key)
    }

    /// Set a field without metamethods.
    #[inline]
    pub fn raw_set(&self, key: impl IntoLua, value: impl IntoLua) -> LuaResult<()> {
        self.inner.rawset_typed(key, value)
    }

    #[inline]
    pub fn raw_seti(&self, idx: i64, value: impl IntoLua) -> LuaResult<()> {
        self.inner.rawseti_typed(idx, value)
    }

    #[inline]
    pub fn raw_geti<T: FromLua>(&self, idx: i64) -> LuaResult<T> {
        self.inner.rawgeti_typed(idx)
    }

    /// Return the array length (`#t`).
    #[inline]
    pub fn len(&self) -> LuaResult<usize> {
        self.inner.len()
    }

    /// Return the raw array length.
    #[inline]
    pub fn raw_len(&self) -> LuaResult<usize> {
        self.inner.len()
    }

    /// Returns true if the table length is zero.
    #[inline]
    pub fn is_empty(&self) -> LuaResult<bool> {
        self.len().map(|len| len == 0)
    }

    /// Return all pairs converted to Rust types.
    #[inline]
    pub fn pairs<K: FromLua, V: FromLua>(&self) -> LuaResult<Vec<(K, V)>> {
        self.inner.pairs_typed()
    }

    /// Return the contiguous sequence values from `1..` until nil.
    #[inline]
    pub fn sequence_values<T: FromLua>(&self) -> LuaResult<Vec<T>> {
        self.inner.sequence_values()
    }

    /// Append a value to the array portion of the table.
    #[inline]
    pub fn push(&self, value: impl IntoLua) -> LuaResult<()> {
        self.inner.push_typed(value)
    }

    pub(crate) fn value(&self) -> crate::LuaValue {
        self.inner.to_value()
    }

    /// Whether `self` belongs to the same Lua state as `state`.
    pub(crate) fn same_state_as(&self, state: &crate::GlobalState) -> bool {
        self.inner.belongs_to(state)
    }
}

impl IntoLua for LuaTable {
    #[inline]
    fn into_lua(self, state: &mut crate::LuaState) -> Result<usize, String> {
        self.inner.push_into(state)
    }
}

impl IntoLua for &LuaTable {
    #[inline]
    fn into_lua(self, state: &mut crate::LuaState) -> Result<usize, String> {
        self.inner.push_into(state)
    }
}

impl FromLua for LuaTable {
    fn from_lua(value: LuaValue, state: &mut crate::LuaState) -> Result<Self, String> {
        let actual = value.type_name();
        let table = state
            .to_table_ref(value)
            .ok_or_else(|| format!("expected table, got {}", actual))?;
        Ok(LuaTable::new(table))
    }
}
