//! Userdata — GC-managed Rust objects exposed to Lua.
//!
//! [`LuaUserdata`] owns its value; the value is dropped when the userdata is
//! collected. Field access, methods and metamethods dispatch through
//! [`UserDataTrait`]; a metatable can still be attached as a Lua-level fallback.
//!
//! Host code borrows the value through `UserDataRef::borrow`/`borrow_mut`
//! guards. The borrow state lives here, like a `RefCell`'s: while the host holds
//! a mutable borrow every VM access panics, and while it holds a shared borrow
//! VM access that needs `&mut` panics. VM accesses themselves are transient
//! (trait methods cannot run Lua code), so they never conflict with each other.

use std::any::TypeId;
use std::cell::Cell;
use std::fmt;

use crate::{LuaValue, UserDataTrait, gc::TablePtr};

/// GC-managed userdata — the bridge between Rust types and Lua values.
pub struct LuaUserdata {
    data: Box<dyn UserDataTrait>,
    type_id: TypeId,
    metatable: TablePtr,
    /// Host borrows: `> 0` shared guards, `-1` one exclusive guard.
    host_borrow: Cell<isize>,
}

impl LuaUserdata {
    /// Create a new userdata owning `data`.
    pub fn new<T: UserDataTrait>(data: T) -> Self {
        Self::from_boxed(Box::new(data))
    }

    /// Create a userdata from an already-boxed trait object.
    pub fn from_boxed(data: Box<dyn UserDataTrait>) -> Self {
        let type_id = data.as_any().type_id();
        LuaUserdata {
            data,
            type_id,
            metatable: TablePtr::null(),
            host_borrow: Cell::new(0),
        }
    }

    #[cold]
    #[inline(never)]
    fn borrowed_by_host(&self) -> ! {
        panic!(
            "userdata {} is {} by the host",
            if self.host_borrow.get() < 0 { "<mutably borrowed>" } else { self.data.type_name() },
            if self.host_borrow.get() < 0 { "mutably borrowed" } else { "borrowed" }
        );
    }

    /// The wrapped value as a trait object.
    #[inline]
    pub fn get_trait(&self) -> &dyn UserDataTrait {
        if self.host_borrow.get() < 0 {
            self.borrowed_by_host();
        }
        self.data.as_ref()
    }

    /// The wrapped value as a mutable trait object.
    #[inline]
    pub fn get_trait_mut(&mut self) -> &mut dyn UserDataTrait {
        if self.host_borrow.get() != 0 {
            self.borrowed_by_host();
        }
        self.data.as_mut()
    }

    /// The wrapped value for the collector's traversal, which only reads
    /// `LuaValue`s owned by internal userdata types.
    #[inline]
    pub(crate) fn trait_for_gc(&self) -> &dyn UserDataTrait {
        self.data.as_ref()
    }

    /// The type name reported by the wrapped value.
    #[inline]
    pub fn type_name(&self) -> &'static str {
        self.get_trait().type_name()
    }

    /// Type name for diagnostics that must not touch a mutably borrowed value.
    pub(crate) fn type_name_unchecked(&self) -> &'static str {
        if self.host_borrow.get() < 0 {
            "userdata"
        } else {
            self.data.type_name()
        }
    }

    /// Whether the wrapped value is a `T` (does not touch the value).
    #[inline]
    pub(crate) fn is_type<T: 'static>(&self) -> bool {
        self.type_id == TypeId::of::<T>()
    }

    /// Downcast to a concrete type. Returns `None` on type mismatch.
    #[inline]
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.get_trait().as_any().downcast_ref::<T>()
    }

    /// Downcast to a concrete type (mutable).
    #[inline]
    pub fn downcast_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.get_trait_mut().as_any_mut().downcast_mut::<T>()
    }

    /// Raw pointer to the wrapped `T`, without creating a reference.
    /// The caller has checked `is_type::<T>()` and holds a host borrow.
    #[inline]
    pub(crate) fn data_ptr<T: 'static>(&self) -> *mut T {
        debug_assert!(self.is_type::<T>());
        (&raw const *self.data).cast::<T>().cast_mut()
    }

    pub(crate) fn try_host_borrow(&self) -> bool {
        let state = self.host_borrow.get();
        if state < 0 {
            return false;
        }
        self.host_borrow.set(state + 1);
        true
    }

    pub(crate) fn try_host_borrow_mut(&self) -> bool {
        if self.host_borrow.get() != 0 {
            return false;
        }
        self.host_borrow.set(-1);
        true
    }

    pub(crate) fn release_host_borrow(&self) {
        self.host_borrow.set(self.host_borrow.get() - 1);
    }

    pub(crate) fn release_host_borrow_mut(&self) {
        self.host_borrow.set(0);
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
            self.type_name_unchecked(),
            (&raw const *self.data).cast::<()>()
        )
    }
}
