use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::{
    Chunk, FromLua, FromLuaMulti, IntoLua, LuaFunction, LuaResult, LuaState, LuaTable,
};

use crate::lua_api::{Lua, LuaApi};

fn typed_scope_arg<T: FromLua>(state: &mut LuaState, index: usize) -> LuaResult<T> {
    let value = state.get_arg(index).unwrap_or_default();
    T::from_lua(value, state).map_err(|msg| crate::stdlib::lauxlib::argerror(state, index, &msg))
}

#[doc(hidden)]
pub trait ScopedLuaCallback<Args, R> {
    fn invoke_typed(&self, state: &mut LuaState) -> LuaResult<usize>;
}

#[doc(hidden)]
pub trait ScopedLuaCallbackWith<Data, Args, R> {
    fn invoke_typed_with(&self, data: &Data, state: &mut LuaState) -> LuaResult<usize>;
}

#[doc(hidden)]
pub trait ScopedLuaCallbackMutWith<Data, Args, R> {
    fn invoke_typed_with_mut(&self, data: &mut Data, state: &mut LuaState) -> LuaResult<usize>;
}

struct CallbackResource<'scope> {
    ptr: *mut (),
    drop_fn: unsafe fn(*mut ()),
    _marker: PhantomData<&'scope ()>,
}

impl<'scope> CallbackResource<'scope> {
    fn new<T: 'scope>(callback: Box<T>) -> Self {
        unsafe fn drop_box<T>(ptr: *mut ()) {
            drop(unsafe { Box::from_raw(ptr.cast::<T>()) });
        }

        CallbackResource {
            ptr: Box::into_raw(callback).cast::<()>(),
            drop_fn: drop_box::<T>,
            _marker: PhantomData,
        }
    }
}

impl Drop for CallbackResource<'_> {
    fn drop(&mut self) {
        unsafe { (self.drop_fn)(self.ptr) };
    }
}

impl<Func, R> ScopedLuaCallback<(), R> for Func
where
    Func: Fn() -> R,
    R: IntoLua,
{
    fn invoke_typed(&self, state: &mut LuaState) -> LuaResult<usize> {
        (self)().push_callback_result(state)
    }
}

impl<Data, Func, R> ScopedLuaCallbackWith<Data, (), R> for Func
where
    Func: Fn(&Data) -> R,
    R: IntoLua,
{
    fn invoke_typed_with(&self, data: &Data, state: &mut LuaState) -> LuaResult<usize> {
        (self)(data).push_callback_result(state)
    }
}

impl<Data, Func, R> ScopedLuaCallbackMutWith<Data, (), R> for Func
where
    Func: Fn(&mut Data) -> R,
    R: IntoLua,
{
    fn invoke_typed_with_mut(&self, data: &mut Data, state: &mut LuaState) -> LuaResult<usize> {
        (self)(data).push_callback_result(state)
    }
}

macro_rules! impl_scoped_lua_callback {
    ($(($(($ty:ident, $value:ident) => $index:literal),+)),* $(,)?) => {
        $(
            impl<Func, R, $($ty),+> ScopedLuaCallback<($($ty,)+), R> for Func
            where
                Func: Fn($($ty),+) -> R,
                R: IntoLua,
                $($ty: FromLua),+
            {
                fn invoke_typed(&self, state: &mut LuaState) -> LuaResult<usize> {
                    $(
                        let $value = typed_scope_arg::<$ty>(state, $index)?;
                    )+

                    (self)($($value),+).push_callback_result(state)
                }
            }
        )*
    };
}

impl_scoped_lua_callback!(
    ((A, a) => 1),
    ((A, a) => 1, (B, b) => 2),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5, (T6, t6) => 6),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5, (T6, t6) => 6, (T7, t7) => 7),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5, (T6, t6) => 6, (T7, t7) => 7, (T8, t8) => 8)
);

macro_rules! impl_scoped_lua_callback_with {
    ($(($(($ty:ident, $value:ident) => $index:literal),+)),* $(,)?) => {
        $(
            impl<Data, Func, R, $($ty),+> ScopedLuaCallbackWith<Data, ($($ty,)+), R> for Func
            where
                Func: Fn(&Data, $($ty),+) -> R,
                R: IntoLua,
                $($ty: FromLua),+
            {
                fn invoke_typed_with(&self, data: &Data, state: &mut LuaState) -> LuaResult<usize> {
                    $(
                        let $value = typed_scope_arg::<$ty>(state, $index)?;
                    )+

                    (self)(data, $($value),+).push_callback_result(state)
                }
            }
        )*
    };
}

impl_scoped_lua_callback_with!(
    ((A, a) => 1),
    ((A, a) => 1, (B, b) => 2),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5, (T6, t6) => 6),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5, (T6, t6) => 6, (T7, t7) => 7),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5, (T6, t6) => 6, (T7, t7) => 7, (T8, t8) => 8)
);

macro_rules! impl_scoped_lua_callback_mut_with {
    ($(($(($ty:ident, $value:ident) => $index:literal),+)),* $(,)?) => {
        $(
            impl<Data, Func, R, $($ty),+> ScopedLuaCallbackMutWith<Data, ($($ty,)+), R> for Func
            where
                Func: Fn(&mut Data, $($ty),+) -> R,
                R: IntoLua,
                $($ty: FromLua),+
            {
                fn invoke_typed_with_mut(&self, data: &mut Data, state: &mut LuaState) -> LuaResult<usize> {
                    $(
                        let $value = typed_scope_arg::<$ty>(state, $index)?;
                    )+

                    (self)(data, $($value),+).push_callback_result(state)
                }
            }
        )*
    };
}

impl_scoped_lua_callback_mut_with!(
    ((A, a) => 1),
    ((A, a) => 1, (B, b) => 2),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5, (T6, t6) => 6),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5, (T6, t6) => 6, (T7, t7) => 7),
    ((A, a) => 1, (B, b) => 2, (C, c) => 3, (D, d) => 4, (E, e) => 5, (T6, t6) => 6, (T7, t7) => 7, (T8, t8) => 8)
);

fn scoped_expired_error() -> &'static str {
    "scoped value is no longer available"
}

/// Lexical scope for non-`'static` Lua callbacks.
///
/// `'lua` is the borrow of the [`Lua`] runtime passed to [`Lua::scope`]; data
/// lent to scoped callbacks must outlive it, so nothing Lua can still reach is
/// freed while the scope is open. Every callback created through the scope
/// raises an error once the scope has ended.
pub struct Scope<'scope, 'lua> {
    lua: &'scope mut Lua,
    active: Rc<Cell<bool>>,
    callbacks: Vec<CallbackResource<'scope>>,
    _lua: PhantomData<&'lua mut Lua>,
}

impl<'scope, 'lua> Scope<'scope, 'lua> {
    pub(crate) fn new(lua: &'scope mut Lua) -> Self {
        Scope {
            lua,
            active: Rc::new(Cell::new(true)),
            callbacks: Vec::new(),
            _lua: PhantomData,
        }
    }

    /// Access the underlying high-level Lua runtime within this scope.
    pub fn lua(&mut self) -> &mut Lua {
        self.lua
    }

    /// Return a handle to the global environment table.
    pub fn globals(&mut self) -> LuaTable {
        self.lua.globals()
    }

    /// Return a chunk builder bound to this scope's Lua borrow.
    pub fn load<'a>(&'a mut self, source: &str) -> Chunk<'a> {
        self.lua.load(source)
    }

    /// Create a scoped Lua function from a `'static` Rust callback.
    pub fn create_function<F, Args, R>(
        &mut self,
        f: F,
    ) -> LuaResult<ScopedFunction<'scope, 'static>>
    where
        F: ScopedLuaCallback<Args, R> + 'static,
    {
        let callback = CallbackResource::new(Box::new(f));
        let callback_ptr = callback.ptr as usize;
        self.callbacks.push(callback);

        let active = self.active.clone();
        let function = self.lua.create_raw_function(move |state| {
            if !active.get() {
                return Err(state.error(scoped_expired_error().to_owned()));
            }

            let callback = unsafe { &*(callback_ptr as *const F) };
            callback.invoke_typed(state)
        })?;

        Ok(ScopedFunction::new(function))
    }

    /// Create a scoped Lua function that borrows Rust data for the whole scope.
    pub fn create_function_with<Data, F, Args, R>(
        &mut self,
        data: &'lua Data,
        f: F,
    ) -> LuaResult<ScopedFunction<'scope, 'lua>>
    where
        F: ScopedLuaCallbackWith<Data, Args, R> + 'static,
    {
        let callback = CallbackResource::new(Box::new(f));
        let callback_ptr = callback.ptr as usize;
        let data_ptr = (data as *const Data).cast::<()>() as usize;
        self.callbacks.push(callback);

        let active = self.active.clone();
        let function = self.lua.create_raw_function(move |state| {
            if !active.get() {
                return Err(state.error(scoped_expired_error().to_owned()));
            }

            let callback = unsafe { &*(callback_ptr as *const F) };
            let data = unsafe { &*(data_ptr as *const Data) };
            callback.invoke_typed_with(data, state)
        })?;

        Ok(ScopedFunction::new(function))
    }

    /// Create a scoped Lua function that mutably borrows Rust data for the whole scope.
    ///
    /// Calling the function again while it is running (for example through a
    /// Lua callback it invokes) raises a Lua error instead of creating a second
    /// `&mut Data`.
    pub fn create_function_mut_with<Data, F, Args, R>(
        &mut self,
        data: &'lua mut Data,
        f: F,
    ) -> LuaResult<ScopedFunction<'scope, 'lua>>
    where
        F: ScopedLuaCallbackMutWith<Data, Args, R> + 'static,
    {
        let callback = CallbackResource::new(Box::new(f));
        let callback_ptr = callback.ptr as usize;
        let data_ptr = (data as *mut Data).cast::<()>() as usize;
        self.callbacks.push(callback);

        let active = self.active.clone();
        let running = std::cell::Cell::new(false);
        let function = self.lua.create_raw_function(move |state| {
            if !active.get() {
                return Err(state.error(scoped_expired_error().to_owned()));
            }
            if running.replace(true) {
                return Err(state.error("mutable scoped callback called recursively".to_owned()));
            }
            struct Reset<'a>(&'a std::cell::Cell<bool>);
            impl Drop for Reset<'_> {
                fn drop(&mut self) {
                    self.0.set(false);
                }
            }
            let _reset = Reset(&running);

            let callback = unsafe { &*(callback_ptr as *const F) };
            let data = unsafe { &mut *(data_ptr as *mut Data) };
            callback.invoke_typed_with_mut(data, state)
        })?;

        Ok(ScopedFunction::new(function))
    }
}

impl Drop for Scope<'_, '_> {
    fn drop(&mut self) {
        self.active.set(false);
    }
}

/// A Lua function handle tied to a lexical scope.
#[derive(Debug)]
pub struct ScopedFunction<'scope, 'data> {
    inner: LuaFunction,
    _marker: PhantomData<(&'scope mut (), &'data ())>,
}

impl<'scope, 'data> ScopedFunction<'scope, 'data> {
    fn new(inner: LuaFunction) -> Self {
        ScopedFunction {
            inner,
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn call<A: IntoLua, R: FromLuaMulti>(&self, args: A) -> LuaResult<R> {
        self.inner.call(args)
    }

    #[inline]
    pub fn call1<A: IntoLua, R: FromLua>(&self, args: A) -> LuaResult<R> {
        self.inner.call1(args)
    }
}

impl IntoLua for ScopedFunction<'_, '_> {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        self.inner.into_lua(state)
    }
}

impl IntoLua for &ScopedFunction<'_, '_> {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        (&self.inner).into_lua(state)
    }
}
