use std::ffi::c_void;

mod callback;
mod chunk;
mod function;
mod lua;
mod lua_state;
mod lua_string;
mod scope;
mod table;
mod test;
mod value;
mod variadic;

pub use callback::CallbackLua;
pub use chunk::Chunk;
pub use function::LuaFunction;
pub use lua::Lua;
pub use lua_string::{LuaBytes, LuaString};
pub use scope::{Scope, ScopedFunction};
pub use table::LuaTable;
pub use value::Value;
pub use variadic::Variadic;

#[cfg(feature = "sandbox")]
use crate::SandboxConfig;
use crate::{
    FromLua, FromLuaMulti, IntoLua, LuaError, LuaFullError, LuaResult, LuaValue, LuaValueKind,
    Stdlib, UserDataRef, UserDataTrait,
    lua_vm::{LuaTypedAsyncCallback, LuaTypedCallback},
};

/// High-level, embedding-oriented API shared by safe host-side Lua handles.
///
/// This trait intentionally covers the typed, ergonomic surface. Low-level raw
/// runtime escape hatches such as `global_state` stay on the concrete type.
pub trait LuaApi {
    fn open_stdlib(&mut self, lib: Stdlib) -> LuaResult<()>;
    fn open_stdlibs(&mut self, libs: &[Stdlib]) -> LuaResult<()>;
    fn collect_garbage(&mut self) -> LuaResult<()>;
    fn execute(&mut self, source: &str) -> LuaResult<()>;
    fn dofile<R: FromLuaMulti>(&mut self, path: &str) -> LuaResult<R>;
    fn eval<R: FromLua>(&mut self, source: &str) -> LuaResult<R>;
    fn eval_multi<R: FromLuaMulti>(&mut self, source: &str) -> LuaResult<R>;
    fn set_global<T: IntoLua>(&mut self, name: &str, value: T) -> LuaResult<()>;
    fn globals(&mut self) -> LuaTable;
    fn get_global<T: FromLua>(&mut self, name: &str) -> LuaResult<Option<T>>;
    fn call_global<A: IntoLua, R: FromLuaMulti>(&mut self, name: &str, args: A) -> LuaResult<R>;
    fn call_global1<A: IntoLua, R: FromLua>(&mut self, name: &str, args: A) -> LuaResult<R>;
    fn register_function<F, Args, R>(&mut self, name: &str, f: F) -> LuaResult<()>
    where
        F: LuaTypedCallback<Args, R>;
    fn create_function<F, Args, R>(&mut self, f: F) -> LuaResult<LuaFunction>
    where
        F: LuaTypedCallback<Args, R>;
    fn register_async_function<F, Args, R>(&mut self, name: &str, f: F) -> LuaResult<()>
    where
        F: LuaTypedAsyncCallback<Args, R>;
    fn load<'lua>(&'lua mut self, source: &str) -> Chunk<'lua, Self>
    where
        Self: Sized + chunk::ChunkHost;
    fn load_function(&mut self, source: &str) -> LuaResult<LuaFunction>;
    fn create_string(&mut self, value: &str) -> LuaResult<LuaString>;
    /// Create a Lua string from arbitrary bytes (Lua strings are byte strings).
    fn create_bytes(&mut self, bytes: &[u8]) -> LuaResult<LuaString>;
    fn create_table(&mut self) -> LuaResult<LuaTable>;
    fn create_table_with_capacity(&mut self, narr: usize, nrec: usize) -> LuaResult<LuaTable>;
    fn create_userdata<T: UserDataTrait + 'static>(&mut self, data: T)
    -> LuaResult<UserDataRef<T>>;
    fn create_table_from<K, V, I>(&mut self, iter: I) -> LuaResult<LuaTable>
    where
        K: IntoLua,
        V: IntoLua,
        I: IntoIterator<Item = (K, V)>;
    fn create_sequence_from<T, I>(&mut self, iter: I) -> LuaResult<LuaTable>
    where
        T: IntoLua,
        I: IntoIterator<Item = T>;

    fn pack<T: IntoLua>(&mut self, value: T) -> LuaResult<Value>;
    fn unpack<T: FromLua>(&mut self, value: Value) -> LuaResult<T>;
    fn convert<T: IntoLua, U: FromLua>(&mut self, value: T) -> LuaResult<U>;
    fn set_extra_space(&mut self, pointer: *mut c_void);
    fn extra_space(&self) -> *mut c_void;
    fn create_lightuserdata(&mut self, pointer: *mut c_void) -> Value;
    fn to_pointer<T: IntoLua>(&mut self, value: T) -> LuaResult<Option<*const c_void>>;
    fn registry(&mut self) -> LuaTable;
    fn registry_get<T: FromLua>(&mut self, key: &str) -> LuaResult<Option<T>>;
    fn registry_set<T: IntoLua>(&mut self, key: &str, value: T) -> LuaResult<()>;
    fn registry_geti<T: FromLua>(&mut self, key: i64) -> LuaResult<Option<T>>;

    fn get_type_metatable(&mut self, kind: LuaValueKind) -> Option<LuaTable>;
    fn set_type_metatable(
        &mut self,
        kind: LuaValueKind,
        metatable: Option<&LuaTable>,
    ) -> LuaResult<()>;
    fn get_error_message(&mut self, error: LuaError) -> LuaFullError;
    fn gc_stop(&mut self);
    fn gc_restart(&mut self);
}

pub(crate) trait StackValueApi {
    fn collect_values<T: IntoLua>(&mut self, value: T, api_name: &str) -> LuaResult<Vec<LuaValue>>;
    fn collect_single_value<T: IntoLua>(&mut self, value: T, api_name: &str)
    -> LuaResult<LuaValue>;
    #[allow(clippy::wrong_self_convention)]
    fn from_value<T: FromLua>(&mut self, value: LuaValue, api_name: &str) -> LuaResult<T>;
}

/// Async high-level API shared by safe host-side Lua handles.
#[allow(async_fn_in_trait)]
pub trait LuaAsyncApi {
    async fn exec_async(&mut self, source: &str) -> LuaResult<()>;
    async fn eval_async<R: FromLua>(&mut self, source: &str) -> LuaResult<R>;
    async fn eval_multi_async<R: FromLuaMulti>(&mut self, source: &str) -> LuaResult<R>;
    async fn call_async<A: IntoLua, R: FromLuaMulti>(
        &mut self,
        function: &LuaFunction,
        args: A,
    ) -> LuaResult<R>;
    async fn call_async1<A: IntoLua, R: FromLua>(
        &mut self,
        function: &LuaFunction,
        args: A,
    ) -> LuaResult<R>;
    async fn call_async_global<A: IntoLua, R: FromLuaMulti>(
        &mut self,
        name: &str,
        args: A,
    ) -> LuaResult<R>;
    async fn call_async_global1<A: IntoLua, R: FromLua>(
        &mut self,
        name: &str,
        args: A,
    ) -> LuaResult<R>;
}

/// Sandbox-oriented high-level API shared by safe host-side Lua handles.
#[cfg(feature = "sandbox")]
pub trait LuaSandboxApi {
    fn load_sandboxed<'lua>(
        &'lua mut self,
        source: &str,
        config: &SandboxConfig,
    ) -> Chunk<'lua, Self>
    where
        Self: Sized + chunk::ChunkHost;
    fn execute_sandboxed(&mut self, source: &str, config: &SandboxConfig) -> LuaResult<()>;
    fn eval_sandboxed<R: FromLua>(&mut self, source: &str, config: &SandboxConfig) -> LuaResult<R>;
    fn eval_multi_sandboxed<R: FromLuaMulti>(
        &mut self,
        source: &str,
        config: &SandboxConfig,
    ) -> LuaResult<R>;
    fn sandbox_capture_global(&mut self, config: &mut SandboxConfig, name: &str) -> LuaResult<()>;
    fn sandbox_insert_global<T: IntoLua>(
        &mut self,
        config: &mut SandboxConfig,
        name: &str,
        value: T,
    ) -> LuaResult<()>;
}
