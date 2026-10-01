//! Userdata with a user value slot: the userdata `lua_newuserdata` creates
//! (a raw memory block) and that `debug.getuservalue`/`debug.setuservalue` and
//! `lua_getuservalue`/`lua_setuservalue` access. The type is portable; only
//! the C API (not built for wasm32) creates and addresses the memory block.

// Without the C API nothing constructs a block, but `debug.getuservalue` still
// has to answer for every userdata.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use crate::{LuaState, LuaValue, UserDataTrait};

/// 16 bytes, so a block's address is aligned for any C type.
#[derive(Clone, Copy)]
#[repr(align(16))]
pub(crate) struct BlockUnit(pub(crate) [u8; 16]);

pub(crate) struct BlockUserdata {
    pub(crate) storage: Box<[BlockUnit]>,
    /// The size requested by `lua_newuserdata`.
    pub(crate) size: usize,
    pub(crate) uservalue: LuaValue,
}

impl UserDataTrait for BlockUserdata {
    fn type_name(&self) -> &'static str {
        "userdata"
    }

    fn trace_lua_values(&self, visit: &mut crate::LuaValueVisitor<'_>) {
        (visit.0)(self.uservalue);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// The user value of a block userdata; `None` for other userdata, which have
/// no user value slot.
pub(crate) fn userdata_uservalue(target: &LuaValue) -> Option<LuaValue> {
    target
        .as_userdata_mut()
        .and_then(|userdata| userdata.downcast_mut::<BlockUserdata>())
        .map(|userdata| userdata.uservalue)
}

/// Set the user value of a block userdata; returns false for other userdata.
pub(crate) fn set_userdata_uservalue(state: &mut LuaState, target: &LuaValue, uservalue: LuaValue) -> bool {
    let Some(userdata) = target
        .as_userdata_mut()
        .and_then(|userdata| userdata.downcast_mut::<BlockUserdata>())
    else {
        return false;
    };
    userdata.uservalue = uservalue;
    if uservalue.is_collectable()
        && let Some(owner) = target.as_gc_ptr()
    {
        state.gc_barrier_back(owner);
    }
    true
}
