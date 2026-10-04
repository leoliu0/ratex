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
pub mod lpeg;
mod lua_api;
mod lua_value;
mod lua_vm;
mod platform_time;
mod stdlib;

pub use compiler::LuaLanguageLevel;

// The public surface is the safe, handle-based API: every Lua value the host
// can keep is rooted in the registry (`Value`, `LuaTable`, `LuaFunction`,
// `LuaString`, `UserDataRef`). The raw VM layer (`LuaValue`, `LuaState`,
// `GlobalState`, C functions) holds unrooted GC pointers and stays crate-private.
pub use lua_value::userdata_trait::{LuaCFunction, OpaqueUserData, UdValue, UserDataTrait};

pub use lib_registry::LuaLibrary;
pub use lua_api::*;
pub use lua_value::LuaValueKind;
pub use lua_value::lua_convert::{FromLua, FromLuaMulti, IntoLua};
pub use lua_vm::SafeOption;
#[cfg(feature = "sandbox")]
pub use lua_vm::SandboxConfig;
pub use lua_vm::async_thread::{AsyncFuture, AsyncReturnValue, IntoAsyncLua};
pub use lua_vm::lua_error::{LuaError, LuaFullError};
pub use lua_vm::{Borrowed, LuaResult, UserDataBorrow, UserDataBorrowMut, UserDataRef};
pub use stdlib::Stdlib;
pub use stdlib::io::file::LuaFile;

pub(crate) use lua_value::userdata_trait::LuaValueVisitor;
pub(crate) use lua_value::{LuaProto, LuaRawFunction, LuaRawTable, LuaUserdata, LuaValue};
pub(crate) use lua_value::RustCallback;
pub(crate) use lua_vm::{
    CallInfo, DebugInfo, GlobalState, Instruction, LuaAnyRef, LuaFunctionRef, LuaState,
    LuaStringRef, LuaTableRef, OpCode,
};
pub(crate) use lua_vm::{LUA_MASKCALL, LUA_MASKCOUNT, LUA_MASKLINE, LUA_MASKRET};
