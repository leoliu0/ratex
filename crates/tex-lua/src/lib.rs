//! Lua runtime imported from `CppCXY/lua-rs` commit
//! `79d69c7282842c6dcfc4f68ec92ebb62628e8ebe`.
//!
//! The source compiler, standard libraries, standard binary chunks, and C ABI
//! expose an explicit Lua 5.3 contract for LuaTeX alongside the Lua 5.5 mode.
//!
//! # Example
//! ```ignore
//! use tex_lua::{Lua, LuaApi, SafeOption};
//!
//! let mut lua = Lua::new(SafeOption::default());
//! let value: i64 = lua.load("return 40 + 2").eval()?;
//! assert_eq!(value, 42);
//! # Ok::<(), tex_lua::LuaError>(())
//! ```

#[cfg(not(target_arch = "wasm32"))]
pub mod c_api;

#[cfg(test)]
mod test;

mod compiler;
mod gc;
mod lib_registry;
mod lua_api;
mod lua_value;
mod lua_vm;
mod platform_time;
mod stdlib;

pub use compiler::LuaLanguageLevel;

// Re-export userdata trait types at crate root for convenience
pub use lua_value::LuaUserdata;
pub use lua_value::UserDataBuilder;
pub use lua_value::userdata_trait::{OpaqueUserData, UdValue, UserDataTrait};

pub use lib_registry::{LibraryModule, LibraryRegistry, LuaLibrary, PreloadModule};
pub use lua_api::*;
pub use lua_value::RustCallback;
pub use lua_value::lua_convert::{FromLua, FromLuaMulti, IntoLua};
pub use lua_value::{
    LuaProto, LuaRawFunction, LuaRawTable, LuaValue, LuaValueKind, chunk_serializer::*,
};
pub use lua_vm::SafeOption;
#[cfg(feature = "sandbox")]
pub use lua_vm::SandboxConfig;
pub use lua_vm::async_thread::{
    AsyncCallHandle, AsyncFuture, AsyncReturnValue, AsyncThread, IntoAsyncLua,
};
pub use lua_vm::lua_error::{LuaError, LuaFullError};
pub use lua_vm::{
    CFunction, CallInfo, DebugInfo, GlobalState, Instruction, LuaAnyRef, LuaFunctionRef, LuaResult,
    LuaState, LuaStringRef, LuaTableRef, OpCode, UserDataRef,
};
pub use lua_vm::{LUA_MASKCALL, LUA_MASKCOUNT, LUA_MASKLINE, LUA_MASKRET};
pub use stdlib::Stdlib;
