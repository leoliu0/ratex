/// Lua reference mechanism (similar to luaL_ref/luaL_unref in C API)
///
/// This module provides a way to store Lua values in the registry and get a stable reference to them.
/// This is useful for keeping values alive across GC cycles and for passing values between Rust and Lua.
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::LuaResult;
use crate::LuaState;
use crate::lua_value::LuaUserdata;
use crate::lua_value::LuaValue;
use crate::lua_value::LuaValueKind;
use crate::lua_value::lua_convert::collect_into_lua_values;
use crate::lua_value::lua_convert::{FromLua, FromLuaMulti, IntoLua};
use crate::lua_vm::{GlobalState, GlobalStateHandle, LuaError, get_metatable};

/// A reference ID in the registry.
/// Similar to Lua's luaL_ref return value.
pub type RefId = i32;

/// Special reference constants (matching Lua's C API)
pub const LUA_REFNIL: RefId = -1; // Reference to nil (no storage needed)
pub const LUA_NOREF: RefId = -2; // Invalid reference

/// A reference to a Lua value stored in the VM's registry.
///
/// This is similar to Lua's C API luaL_ref mechanism: the value lives in the
/// registry under the reference ID, and the ID is released with
/// `GlobalState::release_ref_id` (or the value stays forever).
pub struct LuaRefValue {
    /// The actual storage
    ref_id: RefId,
}

impl LuaRefValue {
    /// Create a new reference with a registry ID
    pub(crate) fn new_registry(ref_id: RefId) -> Self {
        Self { ref_id }
    }

    /// Get the reference ID (if stored in registry)
    pub fn ref_id(&self) -> RefId {
        self.ref_id
    }
}

impl std::fmt::Debug for LuaRefValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LuaRefValue::Registry(ref_id={})", self.ref_id)
    }
}

// ============================================================================
// State liveness shared with host handles
// ============================================================================

/// Shared between a [`GlobalState`] and every host handle into it.
///
/// Handles reach the state only through this record, so a handle that outlives
/// its `Lua` sees a closed state instead of freed memory. Borrow guards that
/// point into GC memory (string bytes, userdata) also register the borrowed
/// value here: the collector marks it as a root, and an owner dropped while a
/// guard is alive leaks the heap instead of freeing it under the guard.
pub(crate) struct StateLiveness {
    state: Cell<Option<NonNull<GlobalState>>>,
    pinned: RefCell<Vec<LuaValue>>,
}

impl StateLiveness {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(StateLiveness {
            state: Cell::new(None),
            pinned: RefCell::new(Vec::new()),
        })
    }

    pub(crate) fn attach(&self, global_state: &mut GlobalState) {
        self.state.set(Some(NonNull::from(global_state)));
    }

    /// Called when the state starts to drop: every handle becomes inert.
    pub(crate) fn close(&self) {
        self.state.set(None);
    }

    #[inline]
    fn handle(&self) -> Option<GlobalStateHandle> {
        self.state.get().map(GlobalStateHandle)
    }

    /// True while a host borrow guard points into GC memory.
    pub(crate) fn has_pins(&self) -> bool {
        !self.pinned.borrow().is_empty()
    }

    pub(crate) fn pinned_len(&self) -> usize {
        self.pinned.borrow().len()
    }

    pub(crate) fn pinned_at(&self, index: usize) -> LuaValue {
        self.pinned.borrow()[index]
    }
}

/// Keeps one collectable value rooted (and its state's heap allocated) while a
/// borrow guard handed to the host is alive.
pub(crate) struct Pin {
    liveness: Rc<StateLiveness>,
    value: LuaValue,
}

impl Pin {
    fn new(liveness: &Rc<StateLiveness>, value: LuaValue) -> Self {
        liveness.pinned.borrow_mut().push(value);
        Pin {
            liveness: Rc::clone(liveness),
            value,
        }
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        let mut pinned = self.liveness.pinned.borrow_mut();
        if let Some(index) = pinned
            .iter()
            .rposition(|value| value.raw_ptr_repr() == self.value.raw_ptr_repr())
        {
            pinned.swap_remove(index);
        }
    }
}

// ============================================================================
// User-facing Ref types (mlua-inspired)
// ============================================================================

/// Internal core shared by all user-facing Ref types.
///
/// Holds a registry reference ID and the liveness record of the owning global
/// state. Releases the registry entry on `Drop` while the state is alive; once
/// the state is closed every operation reports [`LuaError::StateClosed`].
///
/// `!Send + !Sync` by design (the `Rc`) — Lua VM is single-threaded.
struct RefInner {
    ref_id: RefId,
    liveness: Rc<StateLiveness>,
}

impl RefInner {
    /// Create a new RefInner. The value must already be stored in the registry.
    fn new(ref_id: RefId, global_state: GlobalStateHandle) -> Self {
        RefInner {
            ref_id,
            liveness: Rc::clone(&global_state.as_ref().liveness),
        }
    }

    /// Retrieve the LuaValue from the registry (nil once the state is closed).
    #[inline]
    fn to_value(&self) -> LuaValue {
        match self.liveness.handle() {
            Some(global_state) => global_state
                .as_ref()
                .registry_geti(self.ref_id as i64)
                .unwrap_or_default(),
            None => LuaValue::nil(),
        }
    }

    #[inline]
    fn handle(&self) -> LuaResult<GlobalStateHandle> {
        self.liveness.handle().ok_or(LuaError::StateClosed)
    }

    /// Get a mutable reference to the VM.
    #[allow(clippy::mut_from_ref)]
    #[inline]
    fn global_state_mut(&self) -> LuaResult<&mut GlobalState> {
        Ok(self.handle()?.as_mut())
    }

    /// Root the referenced value for the lifetime of a host borrow guard.
    fn pin(&self) -> Option<(LuaValue, Pin)> {
        let value = self.to_value();
        if !value.is_collectable() {
            return None;
        }
        Some((value, Pin::new(&self.liveness, value)))
    }

    fn same_state(&self, other: &RefInner) -> bool {
        Rc::ptr_eq(&self.liveness, &other.liveness)
    }

    fn dispose(&mut self) {
        if self.ref_id > 0
            && let Some(global_state) = self.liveness.handle()
        {
            global_state.as_mut().release_ref_id(self.ref_id);
        }
        self.ref_id = LUA_NOREF; // Mark as released
    }
}

impl Drop for RefInner {
    fn drop(&mut self) {
        self.dispose();
    }
}

impl std::fmt::Debug for RefInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RefInner(ref_id={})", self.ref_id)
    }
}

impl Clone for RefInner {
    fn clone(&self) -> Self {
        let ref_id = match self.liveness.handle() {
            Some(global_state) => {
                let value = self.to_value();
                store_in_registry(global_state.as_mut(), value)
            }
            None => LUA_NOREF,
        };
        RefInner {
            ref_id,
            liveness: Rc::clone(&self.liveness),
        }
    }
}

// ---- helper: create a registry ref for a LuaValue ---------------------------

fn collect_single_value<T: IntoLua>(
    global_state: &mut GlobalState,
    value: T,
    context: &str,
) -> LuaResult<LuaValue> {
    let base_top = global_state.main_state().get_top();

    let pushed = {
        let state = global_state.main_state();
        match value.into_lua(state) {
            Ok(pushed) => pushed,
            Err(err) => {
                state.set_top_raw(base_top);
                return Err(global_state.error(err));
            }
        }
    };

    if pushed != 1 {
        global_state.main_state().set_top_raw(base_top);
        return Err(global_state.error(format!(
            "{} expects exactly one Lua value, got {}",
            context, pushed
        )));
    }

    let result = {
        let state = global_state.main_state();
        let Some(value) = state.stack_get(base_top) else {
            state.set_top_raw(base_top);
            return Err(global_state
                .error("internal error: failed to collect Lua value from stack".to_owned()));
        };
        state.set_top_raw(base_top);
        value
    };

    Ok(result)
}

/// Store a LuaValue in the VM registry and return its RefId.
pub(crate) fn store_in_registry(global_state: &mut GlobalState, value: LuaValue) -> RefId {
    global_state.registry_ref(value)
}

/// Push a handle's value onto `state`, refusing values owned by another state.
fn push_handle_value(inner: &RefInner, state: &mut LuaState) -> Result<usize, String> {
    if !Rc::ptr_eq(&inner.liveness, &state.global_state().liveness) {
        return Err("value belongs to a different or closed Lua state".to_owned());
    }
    state
        .push_value(inner.to_value())
        .map_err(|e| format!("{:?}", e))?;
    Ok(1)
}

// ============================================================================
// Borrow guards
// ============================================================================

/// A borrow of data owned by a Lua value (string bytes, userdata contents).
///
/// While it is alive the value is a GC root, and dropping the `Lua` leaks the
/// heap instead of freeing the borrowed memory.
pub struct Borrowed<'a, T: ?Sized> {
    value: &'a T,
    _pin: Pin,
}

impl<T: ?Sized> std::ops::Deref for Borrowed<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

impl<T: ?Sized> AsRef<T> for Borrowed<'_, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self.value
    }
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for Borrowed<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(f)
    }
}

impl<T: ?Sized + std::fmt::Display> std::fmt::Display for Borrowed<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(f)
    }
}

impl<T: ?Sized + PartialEq> PartialEq<T> for Borrowed<'_, T> {
    fn eq(&self, other: &T) -> bool {
        self.value == other
    }
}

impl PartialEq<&str> for Borrowed<'_, str> {
    fn eq(&self, other: &&str) -> bool {
        self.value == *other
    }
}

impl PartialEq<&[u8]> for Borrowed<'_, [u8]> {
    fn eq(&self, other: &&[u8]) -> bool {
        self.value == *other
    }
}

/// Shared borrow of a userdata's Rust value. While it is alive, Lua code that
/// needs the value mutably panics (like `RefCell`).
pub struct UserDataBorrow<'a, T: 'static> {
    value: &'a T,
    userdata: NonNull<LuaUserdata>,
    _pin: Pin,
}

impl<T> std::ops::Deref for UserDataBorrow<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

impl<T> Drop for UserDataBorrow<'_, T> {
    fn drop(&mut self) {
        // SAFETY: the pin keeps the userdata allocated (rooted, or leaked with
        // its state) for the guard's lifetime.
        unsafe { self.userdata.as_ref() }.release_host_borrow();
    }
}

/// Exclusive borrow of a userdata's Rust value. While it is alive, any Lua
/// access to the value panics (like `RefCell`).
pub struct UserDataBorrowMut<'a, T: 'static> {
    value: &'a mut T,
    userdata: NonNull<LuaUserdata>,
    _pin: Pin,
}

impl<T> std::ops::Deref for UserDataBorrowMut<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

impl<T> std::ops::DerefMut for UserDataBorrowMut<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.value
    }
}

impl<T> Drop for UserDataBorrowMut<'_, T> {
    fn drop(&mut self) {
        // SAFETY: as for `UserDataBorrow`.
        unsafe { self.userdata.as_ref() }.release_host_borrow_mut();
    }
}

// ============================================================================
// LuaTableRef
// ============================================================================

/// A reference to a Lua table held in the VM registry.
///
/// Holds the table in the VM registry so it won't be garbage-collected.
/// The registry entry is automatically released when this value is dropped.
pub struct LuaTableRef {
    inner: RefInner,
}

impl LuaTableRef {
    /// Create from an already-registered ref id. The caller guarantees the
    /// value at `ref_id` is a table.
    pub(crate) fn from_raw(ref_id: RefId, global_state: GlobalStateHandle) -> Self {
        LuaTableRef {
            inner: RefInner::new(ref_id, global_state),
        }
    }

    // ==================== Read ====================

    /// Get a value by string key (raw access, no metamethods).
    pub fn get(&self, key: &str) -> LuaResult<LuaValue> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        let key_val = vm.create_string(key)?;
        Ok(vm
            .main_state()
            .table_get(&table, &key_val)?
            .unwrap_or(LuaValue::nil()))
    }

    /// Get a value by integer key.
    pub fn geti(&self, key: i64) -> LuaResult<LuaValue> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        vm.main_state().table_geti(&table, key)
    }

    /// Get a value by arbitrary LuaValue key.
    pub fn get_value(&self, key: &LuaValue) -> LuaResult<LuaValue> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        Ok(vm
            .main_state()
            .table_get(&table, key)?
            .unwrap_or(LuaValue::nil()))
    }

    /// Get a value by string key and convert to a Rust type via `FromLua`.
    pub fn get_as<T: FromLua>(&self, key: &str) -> LuaResult<T> {
        let val = self.get(key)?;
        let vm = self.inner.global_state_mut()?;
        T::from_lua(val, vm.main_state()).map_err(|msg| vm.error(msg))
    }

    pub fn set_typed<K: IntoLua, V: IntoLua>(&self, key: K, value: V) -> LuaResult<()> {
        let vm = self.inner.global_state_mut()?;
        let key = collect_single_value(vm, key, "LuaTableRef::set_typed(key)")?;
        let value = collect_single_value(vm, value, "LuaTableRef::set_typed(value)")?;
        let table = self.inner.to_value();
        vm.main_state().table_set(&table, key, value)?;
        Ok(())
    }

    /// Get a value by arbitrary Rust-convertible key and convert it to `T`.
    pub fn get_typed<K: IntoLua, T: FromLua>(&self, key: K) -> LuaResult<T> {
        let vm = self.inner.global_state_mut()?;
        let key = collect_single_value(vm, key, "LuaTableRef::get_typed(key)")?;
        let table = self.inner.to_value();
        let value = vm
            .main_state()
            .table_get(&table, &key)?
            .unwrap_or(LuaValue::nil());
        T::from_lua(value, vm.main_state()).map_err(|msg| vm.error(msg))
    }

    /// Returns true if the table contains a non-nil value for the given key.
    pub fn contains_key<K: IntoLua>(&self, key: K) -> LuaResult<bool> {
        let vm = self.inner.global_state_mut()?;
        let key = collect_single_value(vm, key, "LuaTableRef::contains_key(key)")?;
        let table = self.inner.to_value();
        Ok(!vm
            .main_state()
            .table_get(&table, &key)?
            .unwrap_or(LuaValue::nil())
            .is_nil())
    }

    /// Returns true if this table currently has a metatable.
    pub fn has_metatable(&self) -> bool {
        self.inner
            .to_value()
            .as_table()
            .is_some_and(|table| table.has_metatable())
    }

    /// Get the table's metatable, if present.
    pub fn get_metatable(&self) -> Option<LuaTableRef> {
        let handle = self.inner.handle().ok()?;
        let vm = handle.as_mut();
        let value = self.inner.to_value();
        let metatable = get_metatable(vm.main_state(), &value)?;
        if !metatable.is_table() {
            return None;
        }
        let ref_id = store_in_registry(vm, metatable);
        Some(LuaTableRef::from_raw(ref_id, handle))
    }

    /// Set or clear the table's metatable.
    pub fn set_metatable(&self, metatable: Option<&LuaTableRef>) -> LuaResult<()> {
        let vm = self.inner.global_state_mut()?;
        if let Some(metatable) = metatable
            && !self.inner.same_state(&metatable.inner)
        {
            return Err(vm.error("metatable belongs to a different Lua state".to_string()));
        }
        let value = self.inner.to_value();
        let Some(table) = value.as_table_mut() else {
            return Err(vm
                .main_state()
                .error("LuaTableRef does not reference a table".to_string()));
        };

        table.set_metatable(metatable.map(LuaTableRef::to_value));
        if let Some(gc_ptr) = value.as_gc_ptr() {
            vm.main_state().gc_barrier_back(gc_ptr);
        }
        vm.gc.check_finalizer(&value);
        Ok(())
    }

    /// Create an empty table that inherits fields from this table.
    pub fn inherit(&self) -> LuaResult<LuaTableRef> {
        let vm = self.inner.global_state_mut()?;
        let table = vm.main_state().create_table_ref(0, 0)?;
        let metatable = vm.main_state().create_table_ref(0, 1)?;
        metatable.set("__index", self.to_value())?;
        table.set_metatable(Some(&metatable))?;
        Ok(table)
    }

    // ==================== Write ====================

    /// Set a string-keyed value.
    pub fn set(&self, key: &str, value: LuaValue) -> LuaResult<()> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        let key_val = vm.create_string(key)?;
        vm.main_state().table_set(&table, key_val, value)?;
        Ok(())
    }

    /// Set an integer-keyed value.
    pub fn seti(&self, key: i64, value: LuaValue) -> LuaResult<()> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        vm.main_state().table_seti(&table, key, value)?;
        Ok(())
    }

    /// Set an arbitrary key-value pair.
    pub fn set_value(&self, key: LuaValue, value: LuaValue) -> LuaResult<()> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        vm.raw_set(&table, key, value);
        Ok(())
    }

    /// Set an arbitrary key-value pair from Rust-convertible values.
    pub fn rawset_typed<K: IntoLua, V: IntoLua>(&self, key: K, value: V) -> LuaResult<()> {
        let vm = self.inner.global_state_mut()?;
        let key = collect_single_value(vm, key, "LuaTableRef::rawset_typed(key)")?;
        let value = collect_single_value(vm, value, "LuaTableRef::rawset_typed(value)")?;
        let table = self.inner.to_value();
        vm.raw_set(&table, key, value);
        Ok(())
    }

    pub fn rawget_typed<K: IntoLua, V: FromLua>(&self, key: K) -> LuaResult<V> {
        let vm = self.inner.global_state_mut()?;
        let key = collect_single_value(vm, key, "LuaTableRef::rawget_typed(key)")?;
        let table = self.inner.to_value();
        let value = vm.raw_get(&table, &key).unwrap_or_default();
        V::from_lua(value, vm.main_state()).map_err(|msg| vm.error(msg))
    }

    pub fn rawseti_typed<V: IntoLua>(&self, key: i64, value: V) -> LuaResult<()> {
        let vm = self.inner.global_state_mut()?;
        let value = collect_single_value(vm, value, "LuaTableRef::rawseti_typed(value)")?;
        let table = self.inner.to_value();
        vm.raw_seti(&table, key, value);
        Ok(())
    }

    pub fn rawgeti_typed<V: FromLua>(&self, key: i64) -> LuaResult<V> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        let value = vm.raw_geti(&table, key).unwrap_or_default();
        V::from_lua(value, vm.main_state()).map_err(|msg| vm.error(msg))
    }

    // ==================== Iteration ====================

    /// Get all key-value pairs (snapshot, no metamethods).
    pub fn pairs(&self) -> LuaResult<Vec<(LuaValue, LuaValue)>> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        vm.table_pairs(&table)
    }

    /// Get the array length (equivalent to Lua's `#t`).
    pub fn len(&self) -> LuaResult<usize> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        vm.table_length(&table)
    }

    pub fn is_empty(&self) -> LuaResult<bool> {
        self.len().map(|len| len == 0)
    }

    /// Append a value to the array part (equivalent to `table.insert`).
    pub fn push(&self, value: LuaValue) -> LuaResult<()> {
        let current_len = self.len()?;
        self.seti((current_len + 1) as i64, value)
    }

    /// Append a Rust value to the array part of the table.
    pub fn push_typed<V: IntoLua>(&self, value: V) -> LuaResult<()> {
        let vm = self.inner.global_state_mut()?;
        let value = collect_single_value(vm, value, "LuaTableRef::push_typed(value)")?;
        let current_len = self.len()?;
        self.seti((current_len + 1) as i64, value)
    }

    /// Convert all table pairs to typed Rust key-value pairs.
    pub fn pairs_typed<K: FromLua, V: FromLua>(&self) -> LuaResult<Vec<(K, V)>> {
        let pairs = self.pairs()?;
        let vm = self.inner.global_state_mut()?;
        let mut converted = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            let key = K::from_lua(key, vm.main_state()).map_err(|msg| vm.error(msg))?;
            let value = V::from_lua(value, vm.main_state()).map_err(|msg| vm.error(msg))?;
            converted.push((key, value));
        }
        Ok(converted)
    }

    /// Read contiguous sequence values from `1..` until a nil is encountered.
    pub fn sequence_values<V: FromLua>(&self) -> LuaResult<Vec<V>> {
        let vm = self.inner.global_state_mut()?;
        let table = self.inner.to_value();
        let mut values = Vec::new();
        let mut index = 1_i64;

        while let Some(value) = vm.raw_geti(&table, index) {
            if value.is_nil() {
                break;
            }
            let value = V::from_lua(value, vm.main_state()).map_err(|msg| vm.error(msg))?;
            values.push(value);
            index += 1;
        }
        Ok(values)
    }

    // ==================== Conversion ====================

    /// Get the underlying LuaValue (retrieved from registry).
    pub fn to_value(&self) -> LuaValue {
        self.inner.to_value()
    }

    pub(crate) fn push_into(&self, state: &mut LuaState) -> Result<usize, String> {
        push_handle_value(&self.inner, state)
    }

    /// Whether this handle points into `global_state`.
    pub(crate) fn belongs_to(&self, global_state: &GlobalState) -> bool {
        Rc::ptr_eq(&self.inner.liveness, &global_state.liveness)
    }

    /// Get the registry reference ID.
    pub fn ref_id(&self) -> RefId {
        self.inner.ref_id
    }
}

impl std::fmt::Debug for LuaTableRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LuaTableRef(ref_id={})", self.inner.ref_id)
    }
}

impl Clone for LuaTableRef {
    fn clone(&self) -> Self {
        LuaTableRef {
            inner: self.inner.clone(),
        }
    }
}

// ============================================================================
// LuaFunctionRef
// ============================================================================

/// A reference to a Lua function (Lua closure, C function, or Rust closure).
///
/// The function value is held in the VM registry and released on drop.
pub struct LuaFunctionRef {
    inner: RefInner,
}

impl LuaFunctionRef {
    pub(crate) fn from_raw(ref_id: RefId, vm: GlobalStateHandle) -> Self {
        LuaFunctionRef {
            inner: RefInner::new(ref_id, vm),
        }
    }

    /// Return the number of upvalues captured by this function.
    pub fn upvalue_count(&self) -> usize {
        let func = self.inner.to_value();

        if let Some(lua_func) = func.as_lua_function() {
            return lua_func.upvalues().len();
        }

        if let Some(cclosure) = func.as_cclosure() {
            return cclosure.upvalues().len();
        }

        if let Some(rclosure) = func.as_rclosure() {
            return rclosure.upvalues().len();
        }

        0
    }

    /// Read an upvalue as a raw Lua value, with its name (the bytes of the
    /// source text; empty for a C or Rust closure or a stripped chunk).
    pub fn get_upvalue_value(&self, n: usize) -> Option<(Vec<u8>, LuaValue)> {
        if n == 0 {
            return None;
        }

        let func = self.inner.to_value();
        let up_idx = n - 1;

        if let Some(lua_func) = func.as_lua_function() {
            let upvalue_ptr = *lua_func.upvalues().get(up_idx)?;
            let name = lua_func
                .chunk()
                .upvalue_descs
                .get(up_idx)
                .map(|desc| desc.name.to_vec())
                .unwrap_or_default();
            let value = upvalue_ptr.as_ref().data.get_value();
            return Some((name, value));
        }

        if let Some(cclosure) = func.as_cclosure() {
            let value = *cclosure.upvalues().get(up_idx)?;
            return Some((Vec::new(), value));
        }

        if let Some(rclosure) = func.as_rclosure() {
            let value = *rclosure.upvalues().get(up_idx)?;
            return Some((Vec::new(), value));
        }

        None
    }

    /// Read and convert an upvalue.
    pub fn get_upvalue<T: FromLua>(&self, n: usize) -> LuaResult<Option<(Vec<u8>, T)>> {
        let Some((name, value)) = self.get_upvalue_value(n) else {
            return Ok(None);
        };

        let vm = self.inner.global_state_mut()?;
        let value = T::from_lua(value, vm.main_state()).map_err(|msg| vm.error(msg))?;
        Ok(Some((name, value)))
    }

    /// Replace an upvalue with a raw Lua value.
    pub fn set_upvalue_value(&self, n: usize, value: LuaValue) -> LuaResult<Option<Vec<u8>>> {
        if n == 0 {
            return Ok(None);
        }

        let vm = self.inner.global_state_mut()?;
        let state = vm.main_state();
        let func = self.inner.to_value();
        let up_idx = n - 1;

        if let Some(lua_func) = func.as_lua_function() {
            let Some(upvalue_ptr) = lua_func.upvalues().get(up_idx).copied() else {
                return Ok(None);
            };
            let name = lua_func
                .chunk()
                .upvalue_descs
                .get(up_idx)
                .map(|desc| desc.name.to_vec())
                .unwrap_or_default();

            upvalue_ptr.as_mut_ref().data.set_value(value);
            if value.is_collectable()
                && let Some(value_gc_ptr) = value.as_gc_ptr()
            {
                state.gc_barrier(upvalue_ptr, value_gc_ptr);
            }
            return Ok(Some(name));
        }

        let cclosure_owner = func.as_cclosure_ptr();
        if let Some(cclosure) = func.as_cclosure_mut() {
            let Some(slot) = cclosure.upvalues_mut().get_mut(up_idx) else {
                return Ok(None);
            };
            *slot = value;
            if value.is_collectable()
                && let Some(owner) = cclosure_owner
            {
                state.gc_barrier_back(owner.into());
            }
            return Ok(Some(Vec::new()));
        }

        let rclosure_owner = func.as_rclosure_ptr();
        if let Some(rclosure) = func.as_rclosure_mut() {
            let Some(slot) = rclosure.upvalues_mut().get_mut(up_idx) else {
                return Ok(None);
            };
            *slot = value;
            if value.is_collectable()
                && let Some(owner) = rclosure_owner
            {
                state.gc_barrier_back(owner.into());
            }
            return Ok(Some(Vec::new()));
        }

        Ok(None)
    }

    /// Replace an upvalue with a Rust value.
    pub fn set_upvalue<T: IntoLua>(&self, n: usize, value: T) -> LuaResult<Option<Vec<u8>>> {
        let vm = self.inner.global_state_mut()?;
        let value = collect_single_value(vm, value, "LuaFunctionRef::set_upvalue(value)")?;
        self.set_upvalue_value(n, value)
    }

    /// Return an opaque identity for the requested upvalue.
    pub fn upvalue_id(&self, n: usize) -> Option<*mut c_void> {
        if n == 0 {
            return None;
        }

        let func = self.inner.to_value();
        let up_idx = n - 1;

        if let Some(lua_func) = func.as_lua_function() {
            let upvalue = lua_func.upvalues().get(up_idx)?;
            return Some(upvalue.as_ptr() as *mut c_void);
        }

        if let Some(cclosure) = func.as_cclosure() {
            let upvalue = cclosure.upvalues().get(up_idx)?;
            return Some(upvalue as *const _ as *mut c_void);
        }

        if let Some(rclosure) = func.as_rclosure() {
            let upvalue = rclosure.upvalues().get(up_idx)?;
            return Some(upvalue as *const _ as *mut c_void);
        }

        None
    }

    /// Make two Lua function upvalues share the same storage.
    pub fn join_upvalue(&self, n1: usize, other: &LuaFunctionRef, n2: usize) -> LuaResult<bool> {
        if n1 == 0 || n2 == 0 {
            return Ok(false);
        }

        let vm = self.inner.global_state_mut()?;
        if !self.inner.same_state(&other.inner) {
            return Err(vm.error(
                "LuaFunctionRef::join_upvalue requires functions from the same Lua VM".to_string(),
            ));
        }

        let func1 = self.inner.to_value();
        let func2 = other.inner.to_value();

        let Some(lua_func2) = func2.as_lua_function() else {
            return Err(vm.error("LuaFunctionRef::join_upvalue expects Lua functions".to_string()));
        };
        let Some(shared_upvalue) = lua_func2.upvalues().get(n2 - 1).copied() else {
            return Ok(false);
        };

        let func1_owner = func1.as_function_ptr();
        let Some(lua_func1) = func1.as_lua_function_mut() else {
            return Err(vm.error("LuaFunctionRef::join_upvalue expects Lua functions".to_string()));
        };
        let Some(slot) = lua_func1.upvalues_mut().get_mut(n1 - 1) else {
            return Ok(false);
        };
        *slot = shared_upvalue;

        if let Some(owner) = func1_owner {
            vm.main_state().gc_barrier_back(owner.into());
        }

        Ok(true)
    }

    /// Call the function synchronously.
    pub fn call_raw(&self, args: Vec<LuaValue>) -> LuaResult<Vec<LuaValue>> {
        let vm = self.inner.global_state_mut()?;
        let func = self.inner.to_value();
        vm.main_state().call(func, args)
    }

    /// Call the function and return the first result (or nil if no results).
    pub fn call1_raw(&self, args: Vec<LuaValue>) -> LuaResult<LuaValue> {
        let results = self.call_raw(args)?;
        Ok(results.into_iter().next().unwrap_or(LuaValue::nil()))
    }

    /// Call the function with Rust arguments and convert all results into a Rust type.
    pub fn call<A: IntoLua, R: FromLuaMulti>(&self, args: A) -> LuaResult<R> {
        let vm = self.inner.global_state_mut()?;
        let args = collect_into_lua_values(vm.main_state(), args).map_err(|msg| vm.error(msg))?;
        let func = self.inner.to_value();
        let results = vm.main_state().call(func, args)?;
        R::from_lua_multi(results, vm.main_state()).map_err(|msg| vm.error(msg))
    }

    /// Call the function with Rust arguments and convert the first result into a Rust type.
    pub fn call1<A: IntoLua, R: FromLua>(&self, args: A) -> LuaResult<R> {
        let vm = self.inner.global_state_mut()?;
        let args = collect_into_lua_values(vm.main_state(), args).map_err(|msg| vm.error(msg))?;
        let func = self.inner.to_value();
        let result = vm
            .main_state()
            .call(func, args)?
            .into_iter()
            .next()
            .unwrap_or(LuaValue::nil());
        R::from_lua(result, vm.main_state()).map_err(|msg| vm.error(msg))
    }

    /// Call the function asynchronously.
    pub async fn call_async(&self, args: Vec<LuaValue>) -> LuaResult<Vec<LuaValue>> {
        let vm = self.inner.global_state_mut()?;
        let func = self.inner.to_value();
        vm.main_state().call_async(func, args).await
    }

    /// Get the underlying LuaValue.
    pub fn to_value(&self) -> LuaValue {
        self.inner.to_value()
    }

    pub(crate) fn push_into(&self, state: &mut LuaState) -> Result<usize, String> {
        push_handle_value(&self.inner, state)
    }

    /// Get the registry reference ID.
    pub fn ref_id(&self) -> RefId {
        self.inner.ref_id
    }
}

impl Clone for LuaFunctionRef {
    fn clone(&self) -> Self {
        LuaFunctionRef {
            inner: self.inner.clone(),
        }
    }
}

impl std::fmt::Debug for LuaFunctionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LuaFunctionRef(ref_id={})", self.inner.ref_id)
    }
}

// ============================================================================
// LuaStringRef
// ============================================================================

/// A reference to a Lua string held in the VM registry.
pub struct LuaStringRef {
    inner: RefInner,
}

impl LuaStringRef {
    pub(crate) fn from_raw(ref_id: RefId, global_state: GlobalStateHandle) -> Self {
        LuaStringRef {
            inner: RefInner::new(ref_id, global_state),
        }
    }

    /// Borrow the string bytes (None once the state is closed).
    pub fn as_bytes(&self) -> Option<Borrowed<'_, [u8]>> {
        let (value, pin) = self.inner.pin()?;
        let bytes = value.as_bytes()?;
        // SAFETY: `pin` roots the string and keeps the heap allocated (even past
        // the `Lua`'s drop) for as long as the guard lives; Lua strings are
        // immutable, so no `&mut` to these bytes can exist.
        let bytes = unsafe { &*(bytes as *const [u8]) };
        Some(Borrowed { value: bytes, _pin: pin })
    }

    /// Borrow the string as UTF-8 (None if invalid or the state is closed).
    pub fn as_str(&self) -> Option<Borrowed<'_, str>> {
        let bytes = self.as_bytes()?;
        let text = std::str::from_utf8(bytes.value).ok()?;
        Some(Borrowed { value: text, _pin: bytes._pin })
    }

    /// Copy the string content into an owned String.
    pub fn to_string_lossy(&self) -> String {
        self.as_bytes()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    }

    /// Get the byte length.
    pub fn byte_len(&self) -> usize {
        self.inner.to_value().as_bytes().map_or(0, <[u8]>::len)
    }

    /// Get the underlying LuaValue.
    pub fn to_value(&self) -> LuaValue {
        self.inner.to_value()
    }

    pub(crate) fn push_into(&self, state: &mut LuaState) -> Result<usize, String> {
        push_handle_value(&self.inner, state)
    }

    /// Get the registry reference ID.
    pub fn ref_id(&self) -> RefId {
        self.inner.ref_id
    }
}

impl std::fmt::Debug for LuaStringRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "LuaStringRef(ref_id={}, {:?})",
            self.inner.ref_id,
            self.to_string_lossy()
        )
    }
}

impl std::fmt::Display for LuaStringRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_lossy())
    }
}

impl Clone for LuaStringRef {
    fn clone(&self) -> Self {
        LuaStringRef {
            inner: self.inner.clone(),
        }
    }
}

// ============================================================================
// UserDataRef<T>
// ============================================================================

/// A typed user-facing reference to Lua userdata.
///
/// Holds the userdata in the VM registry so it stays alive across Rust calls,
/// and provides `RefCell`-style checked access to the wrapped Rust value: the
/// borrow state lives in the userdata itself, so it is shared by every clone
/// of the handle and by Lua code using the value.
pub struct UserDataRef<T: 'static> {
    inner: RefInner,
    _marker: PhantomData<fn() -> T>,
}

impl<T: 'static> UserDataRef<T> {
    pub(crate) fn from_raw(ref_id: RefId, vm: GlobalStateHandle) -> Self {
        UserDataRef {
            inner: RefInner::new(ref_id, vm),
            _marker: PhantomData,
        }
    }

    /// Resolve the userdata, rooted for a guard's lifetime.
    fn pinned_userdata(&self) -> LuaResult<(NonNull<LuaUserdata>, Pin)> {
        let vm = self.inner.global_state_mut()?;
        let expected = std::any::type_name::<T>();
        let Some((value, pin)) = self.inner.pin() else {
            return Err(vm.error(format!("expected userdata {}, got nil", expected)));
        };
        let Some(userdata) = value.as_userdata_ptr() else {
            return Err(vm.error(format!(
                "expected userdata {}, got {}",
                expected,
                value.type_name()
            )));
        };
        let userdata = NonNull::from(&userdata.as_ref().data);
        // SAFETY: rooted by `pin`.
        let actual = unsafe { userdata.as_ref() };
        if !actual.is_type::<T>() {
            return Err(vm.error(format!(
                "expected userdata {}, got {}",
                expected,
                actual.type_name()
            )));
        }
        Ok((userdata, pin))
    }

    /// Borrow the wrapped value. Fails while it is mutably borrowed.
    pub fn borrow(&self) -> LuaResult<UserDataBorrow<'_, T>> {
        let (userdata, pin) = self.pinned_userdata()?;
        // SAFETY: `pin` keeps the userdata allocated for the guard's lifetime.
        let userdata_ref = unsafe { userdata.as_ref() };
        if !userdata_ref.try_host_borrow() {
            let vm = self.inner.global_state_mut()?;
            return Err(vm.error("userdata already mutably borrowed".to_string()));
        }
        // SAFETY: the borrow flag excludes `&mut` access until the guard drops.
        let value = unsafe { &*userdata_ref.data_ptr::<T>() };
        Ok(UserDataBorrow {
            value,
            userdata,
            _pin: pin,
        })
    }

    /// Mutably borrow the wrapped value. Fails while any borrow is active.
    pub fn borrow_mut(&self) -> LuaResult<UserDataBorrowMut<'_, T>> {
        let (userdata, pin) = self.pinned_userdata()?;
        // SAFETY: `pin` keeps the userdata allocated for the guard's lifetime.
        let userdata_ref = unsafe { userdata.as_ref() };
        if !userdata_ref.try_host_borrow_mut() {
            let vm = self.inner.global_state_mut()?;
            return Err(vm.error("userdata already borrowed".to_string()));
        }
        // SAFETY: the borrow flag excludes any other access until the guard drops.
        let value = unsafe { &mut *userdata_ref.data_ptr::<T>() };
        Ok(UserDataBorrowMut {
            value,
            userdata,
            _pin: pin,
        })
    }

    /// Get the wrapped type name reported by the userdata.
    pub fn type_name(&self) -> LuaResult<&'static str> {
        let (userdata, _pin) = self.pinned_userdata()?;
        // SAFETY: rooted by `_pin`.
        Ok(unsafe { userdata.as_ref() }.type_name_unchecked())
    }

    /// Get the underlying LuaValue.
    pub fn to_value(&self) -> LuaValue {
        self.inner.to_value()
    }

    /// Get the registry reference ID.
    pub fn ref_id(&self) -> RefId {
        self.inner.ref_id
    }
}

impl<T: 'static> FromLua for UserDataRef<T> {
    fn from_lua(value: LuaValue, state: &mut LuaState) -> Result<Self, String> {
        let expected = std::any::type_name::<T>();
        let Some(userdata) = value.as_userdata_ptr() else {
            return Err(format!(
                "expected userdata {}, got {}",
                expected,
                value.type_name()
            ));
        };

        let userdata = &userdata.as_ref().data;
        if !userdata.is_type::<T>() {
            return Err(format!(
                "expected userdata {}, got {}",
                expected,
                userdata.type_name_unchecked()
            ));
        }

        let vm = state.global_state_mut();
        let ref_id = store_in_registry(vm, value);
        Ok(UserDataRef::from_raw(ref_id, state.global_state_handle()))
    }
}

impl<T: 'static> IntoLua for UserDataRef<T> {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        push_handle_value(&self.inner, state)
    }
}

impl<T: 'static> IntoLua for &UserDataRef<T> {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        push_handle_value(&self.inner, state)
    }
}

impl<T: 'static> std::fmt::Debug for UserDataRef<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "UserDataRef<{}>(ref_id={})",
            std::any::type_name::<T>(),
            self.inner.ref_id
        )
    }
}

impl<T: 'static> Clone for UserDataRef<T> {
    fn clone(&self) -> Self {
        UserDataRef {
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

// ============================================================================
// LuaAnyRef
// ============================================================================

/// A reference to any Lua value held in the VM registry.
///
/// Can be down-cast to a typed ref (`LuaTableRef`, `LuaFunctionRef`, `LuaStringRef`)
/// when the concrete type is known.
pub struct LuaAnyRef {
    inner: RefInner,
}

impl LuaAnyRef {
    pub(crate) fn from_raw(ref_id: RefId, vm: GlobalStateHandle) -> Self {
        LuaAnyRef {
            inner: RefInner::new(ref_id, vm),
        }
    }

    /// Get the underlying LuaValue.
    pub fn to_value(&self) -> LuaValue {
        self.inner.to_value()
    }

    /// Register the value again, for a typed handle.
    fn rereference(&self, accept: impl FnOnce(&LuaValue) -> bool) -> Option<(RefId, GlobalStateHandle)> {
        let handle = self.inner.handle().ok()?;
        let value = self.inner.to_value();
        if !accept(&value) {
            return None;
        }
        Some((store_in_registry(handle.as_mut(), value), handle))
    }

    /// Try to convert to a `LuaTableRef`. Returns `None` if the value is not a table.
    /// **Creates a new registry entry** so that both refs are independent.
    pub fn as_table(&self) -> Option<LuaTableRef> {
        let (ref_id, handle) = self.rereference(LuaValue::is_table)?;
        Some(LuaTableRef::from_raw(ref_id, handle))
    }

    /// Try to convert to a `LuaFunctionRef`.
    pub fn as_function(&self) -> Option<LuaFunctionRef> {
        let (ref_id, handle) = self.rereference(LuaValue::is_function)?;
        Some(LuaFunctionRef::from_raw(ref_id, handle))
    }

    /// Try to convert to a `LuaStringRef`.
    pub fn as_string(&self) -> Option<LuaStringRef> {
        let (ref_id, handle) = self.rereference(LuaValue::is_string)?;
        Some(LuaStringRef::from_raw(ref_id, handle))
    }

    /// Try to convert to a typed userdata ref.
    pub fn as_userdata<T: 'static>(&self) -> Option<UserDataRef<T>> {
        let (ref_id, handle) = self.rereference(|value| {
            value
                .as_userdata_ptr()
                .is_some_and(|userdata| userdata.as_ref().data.is_type::<T>())
        })?;
        Some(UserDataRef::from_raw(ref_id, handle))
    }

    /// Get the value's type kind.
    pub fn kind(&self) -> LuaValueKind {
        self.inner.to_value().kind()
    }

    /// Get the referenced value's metatable, if present.
    pub fn get_metatable(&self) -> Option<LuaTableRef> {
        let handle = self.inner.handle().ok()?;
        let vm = handle.as_mut();
        let value = self.inner.to_value();
        let metatable = get_metatable(vm.main_state(), &value)?;
        if !metatable.is_table() {
            return None;
        }
        let ref_id = store_in_registry(vm, metatable);
        Some(LuaTableRef::from_raw(ref_id, handle))
    }

    /// Set or clear the referenced value's metatable.
    pub fn set_metatable(&self, metatable: Option<&LuaTableRef>) -> LuaResult<()> {
        let vm = self.inner.global_state_mut()?;
        if let Some(metatable) = metatable
            && !self.inner.same_state(&metatable.inner)
        {
            return Err(vm.error("metatable belongs to a different Lua state".to_string()));
        }
        let value = self.inner.to_value();
        let mt_value = metatable.map(LuaTableRef::to_value);

        if let Some(table) = value.as_table_mut() {
            table.set_metatable(mt_value);
            if let Some(gc_ptr) = value.as_gc_ptr() {
                vm.main_state().gc_barrier_back(gc_ptr);
            }
            vm.gc.check_finalizer(&value);
            return Ok(());
        }

        if let Some(userdata) = value.as_userdata_ptr() {
            userdata
                .as_mut_ref()
                .data
                .set_metatable(mt_value.unwrap_or_else(LuaValue::nil));
            if let Some(gc_ptr) = value.as_gc_ptr() {
                vm.main_state().gc_barrier_back(gc_ptr);
            }
            vm.gc.check_finalizer(&value);
            return Ok(());
        }

        match value.kind() {
            LuaValueKind::String
            | LuaValueKind::Integer
            | LuaValueKind::Float
            | LuaValueKind::Boolean
            | LuaValueKind::Nil => {
                vm.set_basic_metatable(value.kind(), mt_value);
                Ok(())
            }
            _ => Err(vm.error(format!(
                "metatables are not supported for {} values",
                value.type_name()
            ))),
        }
    }

    /// Extract the value as a Rust type via `FromLua`.
    pub fn get_as<T: crate::FromLua>(&self) -> LuaResult<T> {
        let val = self.inner.to_value();
        let vm = self.inner.global_state_mut()?;
        T::from_lua(val, vm.main_state()).map_err(|msg| vm.error(msg))
    }

    pub(crate) fn push_into(&self, state: &mut LuaState) -> Result<usize, String> {
        push_handle_value(&self.inner, state)
    }

    /// Get the registry reference ID.
    pub fn ref_id(&self) -> RefId {
        self.inner.ref_id
    }
}

impl std::fmt::Debug for LuaAnyRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "LuaAnyRef(ref_id={}, kind={:?})",
            self.inner.ref_id,
            self.kind()
        )
    }
}

impl Clone for LuaAnyRef {
    fn clone(&self) -> Self {
        LuaAnyRef {
            inner: self.inner.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{GlobalState, LuaValue, lua_vm::SafeOption};

    #[test]
    fn references_hold_values_until_released() {
        let mut global_state = GlobalState::new(SafeOption::default());
        let table = global_state.create_table(0, 1).unwrap();
        let key = global_state.create_string("num").unwrap();
        global_state.raw_set(&table, key, LuaValue::number(42.0));

        let table_ref = global_state.create_ref(table);
        let nil_ref = global_state.create_ref(LuaValue::nil());
        assert!(table_ref.ref_id() > 0);
        assert_eq!(nil_ref.ref_id(), super::LUA_REFNIL, "nil needs no registry slot");

        let held = global_state.registry_geti(table_ref.ref_id() as i64).unwrap();
        assert_eq!(
            global_state.raw_get(&held, &key).and_then(|v| v.as_number()),
            Some(42.0)
        );

        global_state.release_ref_id(table_ref.ref_id());
        let after = global_state.registry_geti(table_ref.ref_id() as i64);
        assert!(!after.is_some_and(|value| value.is_table()), "the slot no longer holds the table");
    }

    #[test]
    fn released_reference_ids_are_reused() {
        let mut global_state = GlobalState::new(SafeOption::default());
        let first = global_state.create_table(0, 0).unwrap();
        let first_id = global_state.create_ref(first).ref_id();
        global_state.release_ref_id(first_id);

        let second = global_state.create_table(0, 0).unwrap();
        let second_id = global_state.create_ref(second).ref_id();
        assert_eq!(first_id, second_id);
    }

    #[test]
    fn many_references_keep_their_own_values() {
        let mut global_state = GlobalState::new(SafeOption::default());
        let key = global_state.create_string("value").unwrap();
        let mut ids = Vec::new();
        for i in 0..10 {
            let table = global_state.create_table(0, 1).unwrap();
            global_state.raw_set(&table, key, LuaValue::number(i as f64));
            ids.push(global_state.create_ref(table).ref_id());
        }
        for (i, id) in ids.iter().enumerate() {
            let table = global_state.registry_geti(*id as i64).unwrap();
            assert_eq!(
                global_state.raw_get(&table, &key).and_then(|v| v.as_number()),
                Some(i as f64)
            );
        }
    }
}
