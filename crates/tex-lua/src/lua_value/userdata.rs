//! Userdata — GC-managed Rust objects exposed to Lua.
//!
//! [`LuaUserdata`] owns its value; the value is dropped when the userdata is
//! collected. Field access, methods and metamethods dispatch through
//! [`UserDataTrait`]; a metatable can still be attached as a Lua-level fallback.

use std::{any::Any, fmt};

use crate::{LuaValue, UserDataTrait, gc::TablePtr};

/// GC-managed userdata — the bridge between Rust types and Lua values.
pub struct LuaUserdata {
    data: Box<dyn UserDataTrait>,
    metatable: TablePtr,
}

impl LuaUserdata {
    /// Create a new userdata owning `data`.
    pub fn new<T: UserDataTrait>(data: T) -> Self {
        Self::from_boxed(Box::new(data))
    }

    /// Create a userdata from an already-boxed trait object.
    pub fn from_boxed(data: Box<dyn UserDataTrait>) -> Self {
        LuaUserdata {
            data,
            metatable: TablePtr::null(),
        }
    }

    /// The wrapped value as a trait object.
    #[inline]
    pub fn get_trait(&self) -> &dyn UserDataTrait {
        self.data.as_ref()
    }

    /// The wrapped value as a mutable trait object.
    #[inline]
    pub fn get_trait_mut(&mut self) -> &mut dyn UserDataTrait {
        self.data.as_mut()
    }

    /// The type name reported by the wrapped value.
    #[inline]
    pub fn type_name(&self) -> &'static str {
        self.data.type_name()
    }

    /// Downcast to a concrete type. Returns `None` on type mismatch.
    #[inline]
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.data.as_any().downcast_ref::<T>()
    }

    /// Downcast to a concrete type (mutable).
    #[inline]
    pub fn downcast_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.data.as_any_mut().downcast_mut::<T>()
    }

    pub fn get_metatable(&self) -> Option<LuaValue> {
        if self.metatable.is_null() {
            None
        } else {
            Some(LuaValue::table(self.metatable))
        }
    }

    pub(crate) fn set_metatable(&mut self, metatable: LuaValue) {
        if let Some(table_ptr) = metatable.as_table_ptr() {
            self.metatable = table_ptr;
        } else if metatable.is_nil() {
            self.metatable = TablePtr::null();
        }
    }
}

impl fmt::Debug for LuaUserdata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Userdata({}@{:p})",
            self.data.type_name(),
            self.data.as_any() as *const dyn Any
        )
    }
}
