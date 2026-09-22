#![allow(non_camel_case_types, non_snake_case, unsafe_op_in_unsafe_fn)]

use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Read as _;
use std::pin::Pin;
use std::ptr;
use std::slice;

use crate::lua_value::chunk53;
use crate::lua_value::{LuaUserdata, LuaValue, LuaValueKind, UpvalueStore};
use crate::lua_vm::{
    CApiStackParking, CFunction, GlobalState, LuaError, LuaResult, LuaState, SafeOption,
    get_metatable,
};
use crate::{LuaLanguageLevel, Stdlib};

pub type lua_Number = f64;
pub type lua_Integer = i64;
pub type lua_Unsigned = u64;
pub type lua_KContext = isize;
pub type lua_CFunction = Option<unsafe extern "C" fn(*mut lua_State) -> c_int>;
pub type lua_KFunction = Option<unsafe extern "C" fn(*mut lua_State, c_int, lua_KContext) -> c_int>;
pub type lua_Reader =
    Option<unsafe extern "C" fn(*mut lua_State, *mut c_void, *mut usize) -> *const c_char>;
pub type lua_Writer =
    Option<unsafe extern "C" fn(*mut lua_State, *const c_void, usize, *mut c_void) -> c_int>;
pub type lua_Alloc =
    Option<unsafe extern "C" fn(*mut c_void, *mut c_void, usize, usize) -> *mut c_void>;
#[allow(improper_ctypes)]
unsafe extern "C" {
    fn tex_lua_invoke_c(
        function: unsafe extern "C" fn(*mut lua_State) -> c_int,
        state: *mut lua_State,
        result: *mut c_int,
    ) -> c_int;
    fn tex_lua_invoke_k(
        continuation: unsafe extern "C" fn(*mut lua_State, c_int, lua_KContext) -> c_int,
        state: *mut lua_State,
        status: c_int,
        context: lua_KContext,
        result: *mut c_int,
    ) -> c_int;
    fn tex_lua_invoke_hook(
        hook: unsafe extern "C" fn(*mut lua_State, *mut lua_Debug),
        state: *mut lua_State,
        record: *mut lua_Debug,
    ) -> c_int;
    fn tex_lua_invoke_reader(
        reader: unsafe extern "C" fn(*mut lua_State, *mut c_void, *mut usize) -> *const c_char,
        state: *mut lua_State,
        data: *mut c_void,
        result: *mut *const c_char,
        length: *mut usize,
    ) -> c_int;
    fn tex_lua_invoke_writer(
        writer: unsafe extern "C" fn(*mut lua_State, *const c_void, usize, *mut c_void) -> c_int,
        state: *mut lua_State,
        bytes: *const c_void,
        length: usize,
        data: *mut c_void,
        result: *mut c_int,
    ) -> c_int;
    fn tex_lua_default_alloc(
        userdata: *mut c_void,
        pointer: *mut c_void,
        old_size: usize,
        new_size: usize,
    ) -> *mut c_void;
}
pub type lua_Hook = Option<unsafe extern "C" fn(*mut lua_State, *mut lua_Debug)>;

#[repr(C)]
pub struct lua_Debug {
    pub event: c_int,
    pub name: *const c_char,
    pub namewhat: *const c_char,
    pub what: *const c_char,
    pub source: *const c_char,
    pub currentline: c_int,
    pub linedefined: c_int,
    pub lastlinedefined: c_int,
    pub nups: u8,
    pub nparams: u8,
    pub isvararg: c_char,
    pub istailcall: c_char,
    pub short_src: [c_char; 60],
    pub i_ci: *mut c_void,
}

#[repr(C)]
pub struct luaL_Reg {
    pub name: *const c_char,
    pub func: lua_CFunction,
}

pub const LUA_OPADD: c_int = 0;
pub const LUA_OPSUB: c_int = 1;
pub const LUA_OPMUL: c_int = 2;
pub const LUA_OPMOD: c_int = 3;
pub const LUA_OPPOW: c_int = 4;
pub const LUA_OPDIV: c_int = 5;
pub const LUA_OPIDIV: c_int = 6;
pub const LUA_OPBAND: c_int = 7;
pub const LUA_OPBOR: c_int = 8;
pub const LUA_OPBXOR: c_int = 9;
pub const LUA_OPSHL: c_int = 10;
pub const LUA_OPSHR: c_int = 11;
pub const LUA_OPUNM: c_int = 12;
pub const LUA_OPBNOT: c_int = 13;

pub const LUA_HOOKCALL: c_int = 0;
pub const LUA_HOOKRET: c_int = 1;
pub const LUA_HOOKLINE: c_int = 2;
pub const LUA_HOOKCOUNT: c_int = 3;
pub const LUA_HOOKTAILCALL: c_int = 4;

pub const LUAL_BUFFERSIZE: usize =
    0x80 * std::mem::size_of::<*mut c_void>() * std::mem::size_of::<lua_Integer>();

#[repr(C)]
pub struct luaL_Buffer {
    pub b: *mut c_char,
    pub size: usize,
    pub n: usize,
    pub state: *mut lua_State,
    pub initb: [c_char; LUAL_BUFFERSIZE],
}

pub const LUA_OK: c_int = 0;
pub const LUA_YIELD: c_int = 1;
pub const LUA_ERRRUN: c_int = 2;
pub const LUA_ERRSYNTAX: c_int = 3;
pub const LUA_ERRMEM: c_int = 4;
pub const LUA_ERRGCMM: c_int = 5;
pub const LUA_ERRERR: c_int = 6;
pub const LUA_ERRFILE: c_int = 7;
pub const LUA_MULTRET: c_int = -1;
pub const LUA_REGISTRYINDEX: c_int = -1_001_000;
pub const LUA_RIDX_MAINTHREAD: lua_Integer = 1;
pub const LUA_RIDX_GLOBALS: lua_Integer = 2;
pub const LUA_NOREF: c_int = -2;
pub const LUA_REFNIL: c_int = -1;

pub const LUA_TNONE: c_int = -1;
pub const LUA_TNIL: c_int = 0;
pub const LUA_TBOOLEAN: c_int = 1;
pub const LUA_TLIGHTUSERDATA: c_int = 2;
pub const LUA_TNUMBER: c_int = 3;
pub const LUA_TSTRING: c_int = 4;
pub const LUA_TTABLE: c_int = 5;
pub const LUA_TFUNCTION: c_int = 6;
pub const LUA_TUSERDATA: c_int = 7;
pub const LUA_TTHREAD: c_int = 8;

pub const LUA_OPEQ: c_int = 0;
pub const LUA_OPLT: c_int = 1;
pub const LUA_OPLE: c_int = 2;

pub const LUA_GCSTOP: c_int = 0;
pub const LUA_GCRESTART: c_int = 1;
pub const LUA_GCCOLLECT: c_int = 2;
pub const LUA_GCCOUNT: c_int = 3;
pub const LUA_GCCOUNTB: c_int = 4;
pub const LUA_GCSTEP: c_int = 5;
pub const LUA_GCSETPAUSE: c_int = 6;
pub const LUA_GCSETSTEPMUL: c_int = 7;
pub const LUA_GCISRUNNING: c_int = 9;

thread_local! {
    static ACTIVE_ROOT: Cell<*mut lua_State> = const { Cell::new(ptr::null_mut()) };
}

struct ActiveRootGuard(*mut lua_State);

impl ActiveRootGuard {
    fn enter(root: *mut lua_State) -> Self {
        let previous = ACTIVE_ROOT.with(|active| active.replace(root));
        Self(previous)
    }
}

impl Drop for ActiveRootGuard {
    fn drop(&mut self) {
        ACTIVE_ROOT.with(|active| active.set(self.0));
    }
}

/// Opaque state used by the Lua 5.3 C ABI. The actual VM state remains owned by
/// `GlobalState`; coroutine wrappers only borrow a GC-owned `LuaState`.
#[repr(C)]
pub struct lua_State {
    state: *mut LuaState,
    owner: Option<Pin<Box<GlobalState>>>,
    root: *mut lua_State,
    children: Vec<*mut lua_State>,
    c_strings: Vec<Box<[u8]>>,
    pending_error: Option<LuaValue>,
    suspended_stack: Option<CApiStackParking>,
    allocator: lua_Alloc,
    allocator_ud: *mut c_void,
    panic_function: lua_CFunction,
    c_hook: lua_Hook,
    c_hook_mask: c_int,
    c_hook_count: c_int,
}
#[repr(C)]
struct LuaStateAllocation {
    extra_space: [u8; std::mem::size_of::<*mut c_void>()],
    state: lua_State,
}

unsafe fn allocate_wrapper(mut state: lua_State, source: Option<*mut lua_State>) -> *mut lua_State {
    let mut extra_space = [0; std::mem::size_of::<*mut c_void>()];
    if let Some(source) = source.filter(|source| !source.is_null()) {
        let source_allocation = (source.cast::<u8>())
            .sub(std::mem::offset_of!(LuaStateAllocation, state))
            .cast::<LuaStateAllocation>();
        extra_space = (*source_allocation).extra_space;
        state.allocator = (*source).allocator;
        state.allocator_ud = (*source).allocator_ud;
    }
    let allocator = state.allocator.unwrap_or(tex_lua_default_alloc);
    state.allocator = Some(allocator);
    let size = std::mem::size_of::<LuaStateAllocation>();
    let allocation =
        allocator(state.allocator_ud, ptr::null_mut(), 0, size).cast::<LuaStateAllocation>();
    if allocation.is_null() {
        return ptr::null_mut();
    }
    allocation.write(LuaStateAllocation { extra_space, state });
    &mut (*allocation).state
}

unsafe fn free_wrapper(state: *mut lua_State) {
    if let Some(parking) = (*state).suspended_stack.take()
        && let Some(state_vm) = (*state).state.as_mut()
    {
        let _ = state_vm.restore_stack_from_c_api(parking);
    }
    let allocation = state
        .cast::<u8>()
        .sub(std::mem::offset_of!(LuaStateAllocation, state))
        .cast::<LuaStateAllocation>();
    let allocator = (*state).allocator.unwrap_or(tex_lua_default_alloc);
    let allocator_ud = (*state).allocator_ud;
    ptr::drop_in_place(allocation);
    allocator(
        allocator_ud,
        allocation.cast(),
        std::mem::size_of::<LuaStateAllocation>(),
        0,
    );
}

unsafe fn canonical_wrapper(root: *mut lua_State, state: *mut LuaState) -> *mut lua_State {
    let Some(root_wrapper) = root.as_mut() else {
        return ptr::null_mut();
    };
    if root_wrapper.state == state {
        return root;
    }
    if let Some(wrapper) = root_wrapper.children.iter().copied().find(|wrapper| {
        wrapper
            .as_ref()
            .is_some_and(|wrapper| wrapper.state == state)
    }) {
        return wrapper;
    }
    let wrapper = allocate_wrapper(lua_State::borrowed(state, root), Some(root));
    if !wrapper.is_null() {
        (*state).set_c_api_wrapper(wrapper.cast());
        root_wrapper.children.push(wrapper);
    }
    wrapper
}

#[derive(Clone, Copy)]
#[repr(align(16))]
struct CUserdataUnit([u8; 16]);

struct CUserdata {
    storage: Box<[CUserdataUnit]>,
    size: usize,
    uservalue: LuaValue,
}

impl crate::lua_value::userdata_trait::UserDataTrait for CUserdata {
    fn type_name(&self) -> &'static str {
        "userdata"
    }

    fn trace_lua_values(&self, visit: &mut dyn FnMut(LuaValue)) {
        visit(self.uservalue);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl CUserdata {
    fn as_mut_ptr(&mut self) -> *mut c_void {
        self.storage[0].0.as_mut_ptr().cast()
    }
}

impl lua_State {
    fn borrowed(state: *mut LuaState, root: *mut lua_State) -> Self {
        Self {
            state,
            owner: None,
            root,
            children: Vec::new(),
            c_strings: Vec::new(),
            pending_error: None,
            suspended_stack: None,
            allocator: None,
            allocator_ud: ptr::null_mut(),
            panic_function: None,
            c_hook: None,
            c_hook_mask: 0,
            c_hook_count: 0,
        }
    }
}

#[inline]
unsafe fn api<'a>(state: *mut lua_State) -> Option<&'a mut lua_State> {
    state.as_mut()
}

#[inline]
unsafe fn vm<'a>(state: *mut lua_State) -> Option<&'a mut LuaState> {
    api(state)?.state.as_mut()
}

unsafe fn cache_c_bytes(state: *mut lua_State, bytes: &[u8]) -> *const c_char {
    let Some(wrapper) = api(state) else {
        return ptr::null();
    };
    let cache = if wrapper.root.is_null() {
        wrapper
    } else {
        &mut *wrapper.root
    };
    let mut nul_terminated = Vec::with_capacity(bytes.len() + 1);
    nul_terminated.extend_from_slice(bytes);
    nul_terminated.push(0);
    let boxed = nul_terminated.into_boxed_slice();
    let pointer = boxed.as_ptr().cast::<c_char>();
    cache.c_strings.push(boxed);
    pointer
}

#[inline]
fn frame_base(state: &LuaState) -> usize {
    if state.call_depth() == 0 {
        0
    } else {
        unsafe { (*state.current_ci_ptr()).base }
    }
}

fn absolute_index(state: &LuaState, index: c_int) -> Option<usize> {
    let base = frame_base(state);
    let top = state.get_top();
    if index > 0 {
        let absolute = base.checked_add(index as usize - 1)?;
        (absolute < top).then_some(absolute)
    } else if index > LUA_REGISTRYINDEX {
        let absolute = (top as isize).checked_add(index as isize)?;
        (absolute >= base as isize && absolute < top as isize).then_some(absolute as usize)
    } else {
        None
    }
}

fn current_cclosure(state: &LuaState) -> Option<LuaValue> {
    if state.call_depth() == 0 {
        return None;
    }
    let ci = unsafe { &*state.current_ci_ptr() };
    state.stack_get(ci.base.checked_sub(ci.func_offset as usize)?)
}

fn value_at(state: &mut LuaState, index: c_int) -> Option<LuaValue> {
    if index == LUA_REGISTRYINDEX {
        return Some(state.global_state().registry);
    }
    if index < LUA_REGISTRYINDEX {
        let user_index = (LUA_REGISTRYINDEX - index) as usize;
        if user_index == 0 {
            return None;
        }
        let function = current_cclosure(state)?;
        let closure = function.as_cclosure()?;
        // Upvalue zero is the hidden C callback pointer.
        return closure.upvalues().get(user_index).copied();
    }
    absolute_index(state, index).and_then(|absolute| state.stack_get(absolute))
}

fn set_value_at(state: &mut LuaState, index: c_int, value: LuaValue) -> bool {
    if index == LUA_REGISTRYINDEX {
        return false;
    }
    if index < LUA_REGISTRYINDEX {
        let user_index = (LUA_REGISTRYINDEX - index) as usize;
        let Some(function) = current_cclosure(state) else {
            return false;
        };
        let Some(closure) = function.as_cclosure_mut() else {
            return false;
        };
        let Some(slot) = closure.upvalues_mut().get_mut(user_index) else {
            return false;
        };
        *slot = value;
        if value.is_collectable()
            && let Some(owner) = function.as_gc_ptr()
        {
            state.gc_barrier_back(owner);
        }
        return true;
    }
    let Some(absolute) = absolute_index(state, index) else {
        return false;
    };
    state.stack_set(absolute, value).is_ok()
}

fn value_type(value: Option<LuaValue>) -> c_int {
    match value.map(|value| value.kind()) {
        None => LUA_TNONE,
        Some(LuaValueKind::Nil) => LUA_TNIL,
        Some(LuaValueKind::Boolean) => LUA_TBOOLEAN,
        Some(LuaValueKind::Integer | LuaValueKind::Float) => LUA_TNUMBER,
        Some(LuaValueKind::String) => LUA_TSTRING,
        Some(LuaValueKind::Table) => LUA_TTABLE,
        Some(
            LuaValueKind::Function
            | LuaValueKind::CFunction
            | LuaValueKind::CClosure
            | LuaValueKind::RClosure,
        ) => LUA_TFUNCTION,
        Some(LuaValueKind::Userdata) => {
            if value.and_then(|value| value.as_lightuserdata()).is_some() {
                LUA_TLIGHTUSERDATA
            } else {
                LUA_TUSERDATA
            }
        }
        Some(LuaValueKind::Thread) => LUA_TTHREAD,
    }
}

fn status_for(error: LuaError) -> c_int {
    match error {
        LuaError::Yield => LUA_YIELD,
        LuaError::CompileError => LUA_ERRSYNTAX,
        LuaError::OutOfMemory => LUA_ERRMEM,
        LuaError::ErrorInErrorHandling => LUA_ERRERR,
        _ => LUA_ERRRUN,
    }
}

fn push_error(state: &mut LuaState, error: LuaError) -> c_int {
    let status = status_for(error);
    let message = state.get_error_message(error);
    if let Ok(value) = state.create_string(&message) {
        let _ = state.push_value(value);
    }
    status
}

fn numeric_value(value: LuaValue) -> Option<LuaValue> {
    if value.is_number() {
        Some(value)
    } else {
        value
            .as_str()
            .map(crate::stdlib::basic::parse_number::parse_lua_number)
            .filter(|value| value.is_number())
    }
}

fn integer_value(value: LuaValue) -> Option<i64> {
    numeric_value(value).and_then(|value| value.as_integer())
}

fn number_value(value: LuaValue) -> Option<f64> {
    numeric_value(value).and_then(|value| value.as_number())
}

fn floor_integer_division(left: i64, right: i64) -> Option<i64> {
    if right == 0 {
        return None;
    }
    if left == i64::MIN && right == -1 {
        return Some(i64::MIN);
    }
    let quotient = left / right;
    let remainder = left % right;
    Some(if remainder != 0 && (remainder < 0) != (right < 0) {
        quotient - 1
    } else {
        quotient
    })
}

fn direct_arithmetic(operation: c_int, left: LuaValue, right: LuaValue) -> Option<LuaValue> {
    let left_integer = integer_value(left);
    let right_integer = integer_value(right);
    match operation {
        LUA_OPADD => match left_integer.zip(right_integer) {
            Some((left, right)) => Some(LuaValue::integer(left.wrapping_add(right))),
            None => number_value(left)
                .zip(number_value(right))
                .map(|(left, right)| LuaValue::number(left + right)),
        },
        LUA_OPSUB => match left_integer.zip(right_integer) {
            Some((left, right)) => Some(LuaValue::integer(left.wrapping_sub(right))),
            None => number_value(left)
                .zip(number_value(right))
                .map(|(left, right)| LuaValue::number(left - right)),
        },
        LUA_OPMUL => match left_integer.zip(right_integer) {
            Some((left, right)) => Some(LuaValue::integer(left.wrapping_mul(right))),
            None => number_value(left)
                .zip(number_value(right))
                .map(|(left, right)| LuaValue::number(left * right)),
        },
        LUA_OPMOD => match left_integer.zip(right_integer) {
            Some((left, right)) => floor_integer_division(left, right)
                .map(|quotient| LuaValue::integer(left.wrapping_sub(quotient.wrapping_mul(right)))),
            None => number_value(left)
                .zip(number_value(right))
                .map(|(left, right)| LuaValue::number(left - (left / right).floor() * right)),
        },
        LUA_OPPOW => number_value(left)
            .zip(number_value(right))
            .map(|(left, right)| LuaValue::number(left.powf(right))),
        LUA_OPDIV => number_value(left)
            .zip(number_value(right))
            .map(|(left, right)| LuaValue::number(left / right)),
        LUA_OPIDIV => match left_integer.zip(right_integer) {
            Some((left, right)) => floor_integer_division(left, right).map(LuaValue::integer),
            None => number_value(left)
                .zip(number_value(right))
                .map(|(left, right)| LuaValue::number((left / right).floor())),
        },
        LUA_OPBAND => left_integer
            .zip(right_integer)
            .map(|(left, right)| LuaValue::integer(left & right)),
        LUA_OPBOR => left_integer
            .zip(right_integer)
            .map(|(left, right)| LuaValue::integer(left | right)),
        LUA_OPBXOR => left_integer
            .zip(right_integer)
            .map(|(left, right)| LuaValue::integer(left ^ right)),
        LUA_OPSHL => left_integer
            .zip(right_integer)
            .map(|(left, right)| LuaValue::integer(crate::lua_vm::lua_shiftl(left, right))),
        LUA_OPSHR => left_integer
            .zip(right_integer)
            .map(|(left, right)| LuaValue::integer(crate::lua_vm::lua_shiftl(left, -right))),
        LUA_OPUNM => left
            .as_integer_strict()
            .map(|value| LuaValue::integer(value.wrapping_neg()))
            .or_else(|| number_value(left).map(|value| LuaValue::number(-value))),
        LUA_OPBNOT => integer_value(left).map(|value| LuaValue::integer(!value)),
        _ => None,
    }
}

fn adjust_results(state: &mut LuaState, function_index: usize, actual: usize, wanted: c_int) {
    if wanted == LUA_MULTRET {
        return;
    }
    let wanted = wanted.max(0) as usize;
    if actual > wanted {
        let _ = state.set_top(function_index + wanted);
    } else {
        for _ in actual..wanted {
            let _ = state.push_value(LuaValue::nil());
        }
    }
}

fn save_c_continuation(
    state: &mut LuaState,
    wrapper: *mut lua_State,
    continuation: lua_KFunction,
    context: lua_KContext,
    status: c_int,
    frame_index: usize,
    function_index: usize,
    error_function_index: Option<usize>,
) {
    let Some(continuation) = continuation else {
        return;
    };
    if frame_index >= state.call_depth() {
        return;
    }
    let frame = state.get_call_info_mut(frame_index);
    frame.c_k_function = continuation as *const () as usize;
    frame.c_k_context = context;
    frame.c_k_status = status;
    frame.c_k_state = wrapper as usize;
    frame.c_k_func_index = u32::try_from(function_index).unwrap_or(u32::MAX);
    frame.c_k_error_func_index = error_function_index
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(u32::MAX);
    if error_function_index.is_some() {
        frame.call_status |= crate::lua_vm::call_info::call_status::CIST_XPCALL;
    }
}

fn c_callback_trampoline(state: &mut LuaState) -> LuaResult<usize> {
    let function = current_cclosure(state)
        .ok_or_else(|| state.error("C callback has no active closure".to_string()))?;
    let closure = function
        .as_cclosure()
        .ok_or_else(|| state.error("C callback closure is invalid".to_string()))?;
    let callback_pointer = closure
        .upvalues()
        .first()
        .and_then(LuaValue::as_lightuserdata)
        .ok_or_else(|| state.error("C callback pointer is missing".to_string()))?;
    let callback: unsafe extern "C" fn(*mut lua_State) -> c_int =
        unsafe { std::mem::transmute(callback_pointer) };
    let root = ACTIVE_ROOT.with(Cell::get);
    let wrapper = if root.is_null() {
        state.c_api_wrapper().cast::<lua_State>()
    } else {
        unsafe { canonical_wrapper(root, state as *mut LuaState) }
    };
    if wrapper.is_null() {
        return Err(state.error("C callback has no canonical lua_State".to_string()));
    }
    let mut count = 0;
    let invocation_status = unsafe { tex_lua_invoke_c(callback, wrapper, &mut count) };
    if let Some(error) = unsafe { &mut *wrapper }.pending_error.take() {
        state.set_error_object(error);
        return Err(LuaError::RuntimeError);
    }
    if invocation_status == LUA_YIELD {
        return Err(LuaError::Yield);
    }
    if invocation_status != LUA_OK {
        return Err(state.error("C callback raised an error".to_string()));
    }
    if count < 0 {
        return Err(state.error("C callback returned a negative result count".to_string()));
    }
    Ok(count as usize)
}

pub(crate) fn external_c_function(
    state: &mut LuaState,
    callback: *mut c_void,
) -> LuaResult<LuaValue> {
    state.global_state_mut().create_c_closure(
        c_callback_trampoline as CFunction,
        vec![LuaValue::lightuserdata(callback)],
    )
}

pub(crate) unsafe fn resume_c_continuation(
    state: &mut LuaState,
    wrapper_address: usize,
    function_address: usize,
    status: c_int,
    context: lua_KContext,
) -> LuaResult<usize> {
    let wrapper = wrapper_address as *mut lua_State;
    if wrapper.is_null() || function_address == 0 {
        return Err(state.error("C continuation is missing".to_string()));
    }
    let continuation: unsafe extern "C" fn(*mut lua_State, c_int, lua_KContext) -> c_int =
        std::mem::transmute(function_address);
    let mut count = 0;
    let invocation_status = tex_lua_invoke_k(continuation, wrapper, status, context, &mut count);
    if let Some(error) = (&mut *wrapper).pending_error.take() {
        state.set_error_object(error);
        return Err(LuaError::RuntimeError);
    }
    match invocation_status {
        LUA_OK if count >= 0 => Ok(count as usize),
        LUA_OK => Err(state.error("C continuation returned a negative result count".to_string())),
        LUA_YIELD => Err(LuaError::Yield),
        LUA_ERRMEM => Err(LuaError::OutOfMemory),
        LUA_ERRERR => Err(LuaError::ErrorInErrorHandling),
        _ => Err(state.error("C continuation raised an error".to_string())),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_newstate() -> *mut lua_State {
    lua_newstate(None, ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_newstate(
    allocator: lua_Alloc,
    allocator_ud: *mut c_void,
) -> *mut lua_State {
    let allocator = allocator.or(Some(tex_lua_default_alloc));
    let mut owner = GlobalState::new_with_language(SafeOption::default(), LuaLanguageLevel::Lua53);
    let state = {
        let global = owner.as_mut().get_mut();
        let main_thread = LuaValue::thread(global.get_main_thread_ptr());
        global.registry_seti(LUA_RIDX_MAINTHREAD, main_thread);
        global.registry_seti(LUA_RIDX_GLOBALS, global.global);
        global.main_state() as *mut LuaState
    };
    let pointer = allocate_wrapper(
        lua_State {
            state,
            owner: Some(owner),
            root: ptr::null_mut(),
            children: Vec::new(),
            c_strings: Vec::new(),
            pending_error: None,
            suspended_stack: None,
            allocator,
            allocator_ud,
            panic_function: None,
            c_hook: None,
            c_hook_mask: 0,
            c_hook_count: 0,
        },
        None,
    );
    if pointer.is_null() {
        return ptr::null_mut();
    }
    (*pointer).root = pointer;
    (*state).set_c_api_wrapper(pointer.cast());
    pointer
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_close(state: *mut lua_State) {
    let Some(wrapper) = state.as_mut() else {
        return;
    };
    let root = if wrapper.root.is_null() {
        state
    } else {
        wrapper.root
    };
    let Some(root_wrapper) = root.as_mut() else {
        return;
    };
    if root_wrapper.owner.is_none() {
        return;
    }
    let _active = ActiveRootGuard::enter(root);
    if let Some(owner) = root_wrapper.owner.as_mut() {
        owner.as_mut().get_mut().close();
    }
    let children = std::mem::take(&mut root_wrapper.children);
    for child in children {
        free_wrapper(child);
    }
    free_wrapper(root);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_atpanic(
    state: *mut lua_State,
    panic_function: lua_CFunction,
) -> lua_CFunction {
    let Some(wrapper) = api(state) else {
        return None;
    };
    let root = if wrapper.root.is_null() {
        state
    } else {
        wrapper.root
    };
    let Some(root) = root.as_mut() else {
        return None;
    };
    std::mem::replace(&mut root.panic_function, panic_function)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_getpanic(state: *mut lua_State) -> lua_CFunction {
    let wrapper = api(state)?;
    let root = if wrapper.root.is_null() {
        state
    } else {
        wrapper.root
    };
    root.as_ref()?.panic_function
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_openlibs(state: *mut lua_State) {
    let Some(wrapper) = api(state) else { return };
    let root = if wrapper.root.is_null() {
        state
    } else {
        wrapper.root
    };
    if let Some(root) = root.as_mut()
        && let Some(owner) = root.owner.as_mut()
    {
        let _ = owner.as_mut().get_mut().open_stdlib(Stdlib::All);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_version(_state: *mut lua_State) -> *const lua_Number {
    static VERSION_NUMBER: lua_Number = 503.0;
    &VERSION_NUMBER
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gettop(state: *mut lua_State) -> c_int {
    let Some(state) = vm(state) else { return 0 };
    state.get_top().saturating_sub(frame_base(state)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_settop(state: *mut lua_State, index: c_int) {
    let Some(state) = vm(state) else { return };
    let base = frame_base(state);
    let old_top = state.get_top();
    let new_top = if index >= 0 {
        base.saturating_add(index as usize)
    } else {
        old_top.saturating_sub(index.unsigned_abs() as usize - 1)
    }
    .max(base);
    if new_top > old_top {
        if state.ensure_stack_capacity(new_top - old_top).is_err() {
            return;
        }
        for slot in old_top..new_top {
            let _ = state.stack_set(slot, LuaValue::nil());
        }
    }
    let _ = state.set_top(new_top);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_absindex(state: *mut lua_State, index: c_int) -> c_int {
    if index > 0 || index <= LUA_REGISTRYINDEX {
        return index;
    }
    lua_gettop(state) + index + 1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_checkstack(state: *mut lua_State, size: c_int) -> c_int {
    let Some(state) = vm(state) else { return 0 };
    c_int::from(size >= 0 && state.check_stack(size as usize))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushvalue(state: *mut lua_State, index: c_int) {
    let Some(state) = vm(state) else { return };
    if let Some(value) = value_at(state, index) {
        let _ = state.push_value(value);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_copy(state: *mut lua_State, from: c_int, to: c_int) {
    let Some(state) = vm(state) else { return };
    if let Some(value) = value_at(state, from) {
        let _ = set_value_at(state, to, value);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rotate(state: *mut lua_State, index: c_int, amount: c_int) {
    let Some(state) = vm(state) else { return };
    let Some(start) = absolute_index(state, index) else {
        return;
    };
    let top = state.get_top();
    if start >= top {
        return;
    }
    let len = top - start;
    let shift = amount.rem_euclid(len as c_int) as usize;
    if shift == 0 {
        return;
    }
    state.stack_mut()[start..top].rotate_right(shift);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_xmove(from: *mut lua_State, to: *mut lua_State, count: c_int) {
    if count <= 0 {
        return;
    }
    let Some(from_state) = vm(from) else { return };
    let count = count as usize;
    let start = from_state.get_top().saturating_sub(count);
    let values: Vec<_> = (start..from_state.get_top())
        .filter_map(|slot| from_state.stack_get(slot))
        .collect();
    let _ = from_state.set_top(start);
    let Some(to_state) = vm(to) else { return };
    for value in values {
        let _ = to_state.push_value(value);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_type(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state) = vm(state) else {
        return LUA_TNONE;
    };
    value_type(value_at(state, index))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_typename(_state: *mut lua_State, kind: c_int) -> *const c_char {
    match kind {
        LUA_TNIL => c"nil".as_ptr(),
        LUA_TBOOLEAN => c"boolean".as_ptr(),
        LUA_TLIGHTUSERDATA => c"userdata".as_ptr(),
        LUA_TNUMBER => c"number".as_ptr(),
        LUA_TSTRING => c"string".as_ptr(),
        LUA_TTABLE => c"table".as_ptr(),
        LUA_TFUNCTION => c"function".as_ptr(),
        LUA_TUSERDATA => c"userdata".as_ptr(),
        LUA_TTHREAD => c"thread".as_ptr(),
        _ => c"no value".as_ptr(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isnumber(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state) = vm(state) else { return 0 };
    let Some(value) = value_at(state, index) else {
        return 0;
    };
    c_int::from(
        value.as_number().is_some()
            || value
                .as_str()
                .and_then(|s| crate::stdlib::basic::parse_number::parse_lua_number(s).as_number())
                .is_some(),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isstring(state: *mut lua_State, index: c_int) -> c_int {
    matches!(lua_type(state, index), LUA_TSTRING | LUA_TNUMBER) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_iscfunction(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state) = vm(state) else { return 0 };
    let Some(value) = value_at(state, index) else {
        return 0;
    };
    c_int::from(value.is_cfunction() || value.as_cclosure().is_some())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isinteger(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state) = vm(state) else { return 0 };
    c_int::from(value_at(state, index).is_some_and(|value| value.is_integer()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isuserdata(state: *mut lua_State, index: c_int) -> c_int {
    matches!(lua_type(state, index), LUA_TUSERDATA | LUA_TLIGHTUSERDATA) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tonumberx(
    state: *mut lua_State,
    index: c_int,
    is_number: *mut c_int,
) -> lua_Number {
    let value = vm(state).and_then(|state| value_at(state, index));
    let number = value.and_then(|value| {
        value.as_number().or_else(|| {
            value
                .as_str()
                .map(crate::stdlib::basic::parse_number::parse_lua_number)
                .and_then(|parsed| parsed.as_number())
        })
    });
    if let Some(flag) = is_number.as_mut() {
        *flag = c_int::from(number.is_some());
    }
    number.unwrap_or(0.0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tointegerx(
    state: *mut lua_State,
    index: c_int,
    is_number: *mut c_int,
) -> lua_Integer {
    let value = vm(state).and_then(|state| value_at(state, index));
    let integer = value.and_then(|value| {
        value.as_integer().or_else(|| {
            value
                .as_str()
                .map(crate::stdlib::basic::parse_number::parse_lua_number)
                .and_then(|parsed| parsed.as_integer())
        })
    });
    if let Some(flag) = is_number.as_mut() {
        *flag = c_int::from(integer.is_some());
    }
    integer.unwrap_or(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_arith_impl(state: *mut lua_State, operation: c_int) -> c_int {
    let root = api(state).map_or(ptr::null_mut(), |wrapper| wrapper.root);
    let _active = ActiveRootGuard::enter(root);
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let unary = matches!(operation, LUA_OPUNM | LUA_OPBNOT);
    let operand_count = if unary { 1 } else { 2 };
    let top = state_vm.get_top();
    if top < operand_count {
        let error = state_vm.error("not enough operands for arithmetic".to_string());
        return push_error(state_vm, error);
    }
    let left = state_vm.stack_get(top - operand_count).unwrap_or_default();
    let right = if unary {
        left
    } else {
        state_vm.stack_get(top - 1).unwrap_or_default()
    };
    let _ = state_vm.set_top(top - operand_count);

    let integer_zero_division = matches!(operation, LUA_OPMOD | LUA_OPIDIV)
        && integer_value(left).is_some()
        && integer_value(right) == Some(0);
    state_vm.nny += 1;
    let result = if integer_zero_division {
        Err(state_vm.error("attempt to divide by zero".to_string()))
    } else if let Some(value) = direct_arithmetic(operation, left, right) {
        Ok(value)
    } else {
        let metamethod = match operation {
            LUA_OPADD => Some(crate::lua_vm::TmKind::Add),
            LUA_OPSUB => Some(crate::lua_vm::TmKind::Sub),
            LUA_OPMUL => Some(crate::lua_vm::TmKind::Mul),
            LUA_OPMOD => Some(crate::lua_vm::TmKind::Mod),
            LUA_OPPOW => Some(crate::lua_vm::TmKind::Pow),
            LUA_OPDIV => Some(crate::lua_vm::TmKind::Div),
            LUA_OPIDIV => Some(crate::lua_vm::TmKind::IDiv),
            LUA_OPBAND => Some(crate::lua_vm::TmKind::Band),
            LUA_OPBOR => Some(crate::lua_vm::TmKind::Bor),
            LUA_OPBXOR => Some(crate::lua_vm::TmKind::Bxor),
            LUA_OPSHL => Some(crate::lua_vm::TmKind::Shl),
            LUA_OPSHR => Some(crate::lua_vm::TmKind::Shr),
            LUA_OPUNM => Some(crate::lua_vm::TmKind::Unm),
            LUA_OPBNOT => Some(crate::lua_vm::TmKind::Bnot),
            _ => None,
        };
        let method = metamethod.and_then(|kind| {
            if unary {
                crate::lua_vm::get_metamethod_event(state_vm, &left, kind)
            } else {
                crate::lua_vm::execute::helper::get_binop_metamethod(state_vm, &left, &right, kind)
            }
        });
        if let Some(method) = method {
            state_vm
                .call(method, if unary { vec![left] } else { vec![left, right] })
                .map(|values| values.into_iter().next().unwrap_or_default())
        } else {
            Err(state_vm.error("attempt to perform arithmetic on incompatible values".to_string()))
        }
    };
    state_vm.nny -= 1;
    match result {
        Ok(value) => {
            let _ = state_vm.push_value(value);
            LUA_OK
        }
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_toboolean(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state) = vm(state) else { return 0 };
    c_int::from(value_at(state, index).is_some_and(|value| value.is_truthy()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tolstring(
    state: *mut lua_State,
    index: c_int,
    length: *mut usize,
) -> *const c_char {
    let Some(wrapper) = api(state) else {
        return ptr::null();
    };
    let Some(vm) = wrapper.state.as_mut() else {
        return ptr::null();
    };
    let Some(mut value) = value_at(vm, index) else {
        return ptr::null();
    };
    if !value.is_string() {
        let text = if let Some(integer) = value.as_integer_strict() {
            integer.to_string()
        } else if let Some(number) = value.as_float() {
            crate::stdlib::basic::lua_float_to_string(number)
        } else {
            return ptr::null();
        };
        let Ok(string) = vm.create_string(&text) else {
            return ptr::null();
        };
        let _ = set_value_at(vm, index, string);
        value = string;
    }
    let Some(bytes) = value.as_bytes() else {
        return ptr::null();
    };
    if let Some(length) = length.as_mut() {
        *length = bytes.len();
    }
    let mut nul_terminated = Vec::with_capacity(bytes.len() + 1);
    nul_terminated.extend_from_slice(bytes);
    nul_terminated.push(0);
    let boxed = nul_terminated.into_boxed_slice();
    let pointer = boxed.as_ptr().cast::<c_char>();
    let cache = if !wrapper.root.is_null() {
        &mut *wrapper.root
    } else {
        wrapper
    };
    cache.c_strings.push(boxed);
    pointer
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawlen(state: *mut lua_State, index: c_int) -> usize {
    let Some(state) = vm(state) else { return 0 };
    let Some(value) = value_at(state, index) else {
        return 0;
    };
    if let Some(bytes) = value.as_bytes() {
        bytes.len()
    } else if let Some(table) = value.as_table() {
        table.len()
    } else if let Some(userdata) = value.as_userdata_mut() {
        userdata
            .downcast_ref::<CUserdata>()
            .map_or(0, |data| data.size)
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tocfunction(state: *mut lua_State, index: c_int) -> lua_CFunction {
    let state = vm(state)?;
    let value = value_at(state, index)?;
    let closure = value.as_cclosure()?;
    if closure.func() as usize != c_callback_trampoline as *const () as usize {
        return None;
    }
    let pointer = closure.upvalues().first()?.as_lightuserdata()?;
    Some(std::mem::transmute(pointer))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_touserdata(state: *mut lua_State, index: c_int) -> *mut c_void {
    let Some(state) = vm(state) else {
        return ptr::null_mut();
    };
    let Some(value) = value_at(state, index) else {
        return ptr::null_mut();
    };
    if let Some(pointer) = value.as_lightuserdata() {
        return pointer;
    }
    value
        .as_userdata_mut()
        .and_then(|userdata| userdata.downcast_mut::<CUserdata>())
        .map_or(ptr::null_mut(), CUserdata::as_mut_ptr)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_topointer(state: *mut lua_State, index: c_int) -> *const c_void {
    let Some(state) = vm(state) else {
        return ptr::null();
    };
    let Some(value) = value_at(state, index) else {
        return ptr::null();
    };
    if let Some(pointer) = value.as_lightuserdata() {
        return pointer.cast_const();
    }
    match value.kind() {
        LuaValueKind::String
        | LuaValueKind::Table
        | LuaValueKind::Function
        | LuaValueKind::CFunction
        | LuaValueKind::CClosure
        | LuaValueKind::RClosure
        | LuaValueKind::Userdata
        | LuaValueKind::Thread => value.raw_ptr_repr().cast(),
        LuaValueKind::Nil | LuaValueKind::Boolean | LuaValueKind::Integer | LuaValueKind::Float => {
            ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushnil(state: *mut lua_State) {
    if let Some(state) = vm(state) {
        let _ = state.push_value(LuaValue::nil());
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushnumber(state: *mut lua_State, value: lua_Number) {
    if let Some(state) = vm(state) {
        let _ = state.push_value(LuaValue::number(value));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushinteger(state: *mut lua_State, value: lua_Integer) {
    if let Some(state) = vm(state) {
        let _ = state.push_value(LuaValue::integer(value));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushlstring(
    state: *mut lua_State,
    string: *const c_char,
    length: usize,
) -> *const c_char {
    let Some(state_vm) = vm(state) else {
        return ptr::null();
    };
    if string.is_null() && length != 0 {
        return ptr::null();
    }
    let bytes = if length == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(string.cast::<u8>(), length)
    };
    let Ok(value) = state_vm.create_bytes(bytes) else {
        return ptr::null();
    };
    let _ = state_vm.push_value(value);
    lua_tolstring(state, -1, ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushstring(
    state: *mut lua_State,
    string: *const c_char,
) -> *const c_char {
    if string.is_null() {
        lua_pushnil(state);
        return ptr::null();
    }
    let bytes = CStr::from_ptr(string).to_bytes();
    lua_pushlstring(state, string, bytes.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushcclosure(
    state: *mut lua_State,
    callback: lua_CFunction,
    upvalue_count: c_int,
) {
    let Some(state) = vm(state) else { return };
    let Some(callback) = callback else {
        let _ = state.push_value(LuaValue::nil());
        return;
    };
    let count = upvalue_count.max(0) as usize;
    let start = state.get_top().saturating_sub(count);
    let mut upvalues = Vec::with_capacity(count + 1);
    upvalues.push(LuaValue::lightuserdata(
        callback as *const () as *mut c_void,
    ));
    for slot in start..state.get_top() {
        upvalues.push(state.stack_get(slot).unwrap_or_default());
    }
    let _ = state.set_top(start);
    if let Ok(value) = state
        .global_state_mut()
        .create_c_closure(c_callback_trampoline as CFunction, upvalues)
    {
        let _ = state.push_value(value);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushboolean(state: *mut lua_State, value: c_int) {
    if let Some(state) = vm(state) {
        let _ = state.push_value(LuaValue::boolean(value != 0));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushlightuserdata(state: *mut lua_State, value: *mut c_void) {
    if let Some(state) = vm(state) {
        let _ = state.push_value(LuaValue::lightuserdata(value));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushthread(state: *mut lua_State) -> c_int {
    let Some(state_vm) = vm(state) else { return 0 };
    let is_main = state_vm.is_main_thread();
    let value = LuaValue::thread(state_vm.thread_ptr());
    let _ = state_vm.push_value(value);
    c_int::from(is_main)
}

unsafe fn table_and_key(state: &mut LuaState, index: c_int) -> Option<(LuaValue, LuaValue)> {
    let table = value_at(state, index)?;
    let key = state.stack_get(state.get_top().checked_sub(1)?)?;
    let _ = state.set_top(state.get_top() - 1);
    Some((table, key))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_getglobal_impl(
    state: *mut lua_State,
    name: *const c_char,
) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let Some(name) = (!name.is_null()).then(|| CStr::from_ptr(name).to_string_lossy()) else {
        let _ = state_vm.push_value(LuaValue::nil());
        return LUA_OK;
    };
    match state_vm.get_global_value(&name) {
        Ok(value) => {
            let _ = state_vm.push_value(value.unwrap_or_default());
            LUA_OK
        }
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_gettable_impl(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let Some((table, key)) = table_and_key(state_vm, index) else {
        let error = state_vm.error("table index or key is missing".to_string());
        return push_error(state_vm, error);
    };
    match state_vm.table_get(&table, &key) {
        Ok(value) => {
            let _ = state_vm.push_value(value.unwrap_or_default());
            LUA_OK
        }
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_geti_impl(
    state: *mut lua_State,
    index: c_int,
    key: lua_Integer,
) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let Some(table) = value_at(state_vm, index) else {
        let error = state_vm.error("table index is missing".to_string());
        return push_error(state_vm, error);
    };
    match state_vm.table_geti(&table, key) {
        Ok(value) => {
            let _ = state_vm.push_value(value);
            LUA_OK
        }
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawget(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_TNIL;
    };
    let Some((table, key)) = table_and_key(state_vm, index) else {
        return LUA_TNIL;
    };
    let value = state_vm.raw_get(&table, &key).unwrap_or_default();
    let kind = value_type(Some(value));
    let _ = state_vm.push_value(value);
    kind
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawgeti(
    state: *mut lua_State,
    index: c_int,
    key: lua_Integer,
) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_TNIL;
    };
    let Some(table) = value_at(state_vm, index) else {
        return LUA_TNIL;
    };
    let value = state_vm.raw_geti(&table, key).unwrap_or_default();
    let kind = value_type(Some(value));
    let _ = state_vm.push_value(value);
    kind
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawgetp(
    state: *mut lua_State,
    index: c_int,
    key: *const c_void,
) -> c_int {
    let absolute = lua_absindex(state, index);
    lua_pushlightuserdata(state, key.cast_mut());
    lua_rawget(state, absolute)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_createtable(state: *mut lua_State, array: c_int, hash: c_int) {
    let Some(state) = vm(state) else { return };
    if let Ok(value) = state.create_table(array.max(0) as usize, hash.max(0) as usize) {
        let _ = state.push_value(value);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_newuserdata(state: *mut lua_State, size: usize) -> *mut c_void {
    let Some(state_vm) = vm(state) else {
        return ptr::null_mut();
    };
    let Some(storage_bytes) = size.checked_add(15) else {
        return ptr::null_mut();
    };
    let unit_count = (storage_bytes / 16).max(1);
    let userdata = CUserdata {
        storage: vec![CUserdataUnit([0; 16]); unit_count].into_boxed_slice(),
        size,
        uservalue: LuaValue::nil(),
    };
    let Ok(value) = state_vm.create_userdata(LuaUserdata::new(userdata)) else {
        return ptr::null_mut();
    };
    let pointer = value
        .as_userdata_mut()
        .and_then(|userdata| userdata.downcast_mut::<CUserdata>())
        .map_or(ptr::null_mut(), CUserdata::as_mut_ptr);
    let _ = state_vm.push_value(value);
    pointer
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_setglobal_impl(
    state: *mut lua_State,
    name: *const c_char,
) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    if name.is_null() || state_vm.get_top() == 0 {
        let error = state_vm.error("global name or value is missing".to_string());
        return push_error(state_vm, error);
    }
    let value = state_vm
        .stack_get(state_vm.get_top() - 1)
        .unwrap_or_default();
    let _ = state_vm.set_top(state_vm.get_top() - 1);
    let name = CStr::from_ptr(name).to_string_lossy();
    match state_vm.set_global_value(&name, value) {
        Ok(()) => LUA_OK,
        Err(error) => push_error(state_vm, error),
    }
}

unsafe fn table_key_value(
    state: &mut LuaState,
    index: c_int,
) -> Option<(LuaValue, LuaValue, LuaValue)> {
    let table = value_at(state, index)?;
    let top = state.get_top();
    let key = state.stack_get(top.checked_sub(2)?)?;
    let value = state.stack_get(top.checked_sub(1)?)?;
    let _ = state.set_top(top - 2);
    Some((table, key, value))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_settable_impl(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let Some((table, key, value)) = table_key_value(state_vm, index) else {
        let error = state_vm.error("table index, key, or value is missing".to_string());
        return push_error(state_vm, error);
    };
    match state_vm.table_set(&table, key, value) {
        Ok(()) => LUA_OK,
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_seti_impl(
    state: *mut lua_State,
    index: c_int,
    key: lua_Integer,
) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let Some(table) = value_at(state_vm, index) else {
        let error = state_vm.error("table index is missing".to_string());
        return push_error(state_vm, error);
    };
    let top = state_vm.get_top();
    let Some(value) = state_vm.stack_get(top.saturating_sub(1)) else {
        let error = state_vm.error("table value is missing".to_string());
        return push_error(state_vm, error);
    };
    let _ = state_vm.set_top(top.saturating_sub(1));
    match state_vm.table_seti(&table, key, value) {
        Ok(()) => LUA_OK,
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawset(state: *mut lua_State, index: c_int) {
    let Some(state_vm) = vm(state) else { return };
    if let Some((table, key, value)) = table_key_value(state_vm, index) {
        state_vm.raw_set(&table, key, value);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawseti(state: *mut lua_State, index: c_int, key: lua_Integer) {
    let Some(state_vm) = vm(state) else { return };
    let Some(table) = value_at(state_vm, index) else {
        return;
    };
    let top = state_vm.get_top();
    let value = state_vm
        .stack_get(top.saturating_sub(1))
        .unwrap_or_default();
    let _ = state_vm.set_top(top.saturating_sub(1));
    state_vm.raw_seti(&table, key, value);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawsetp(state: *mut lua_State, index: c_int, key: *const c_void) {
    let Some(state_vm) = vm(state) else { return };
    let Some(table) = value_at(state_vm, index) else {
        return;
    };
    let top = state_vm.get_top();
    let value = state_vm
        .stack_get(top.saturating_sub(1))
        .unwrap_or_default();
    let _ = state_vm.set_top(top.saturating_sub(1));
    state_vm.raw_set(&table, LuaValue::lightuserdata(key.cast_mut()), value);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getmetatable(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state_vm) = vm(state) else { return 0 };
    let Some(value) = value_at(state_vm, index) else {
        return 0;
    };
    let Some(metatable) = get_metatable(state_vm, &value) else {
        return 0;
    };
    let _ = state_vm.push_value(metatable);
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setmetatable(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state_vm) = vm(state) else { return 0 };
    let Some(value) = value_at(state_vm, index) else {
        return 0;
    };
    let top = state_vm.get_top();
    let metatable = state_vm
        .stack_get(top.saturating_sub(1))
        .unwrap_or_default();
    let _ = state_vm.set_top(top.saturating_sub(1));
    if !metatable.is_nil() && !metatable.is_table() {
        return 0;
    }
    if let Some(table) = value.as_table_mut() {
        table.set_metatable((!metatable.is_nil()).then_some(metatable));
    } else if let Some(userdata) = value.as_userdata_mut() {
        userdata.set_metatable(metatable);
    } else {
        state_vm
            .global_state_mut()
            .set_basic_metatable(value.kind(), (!metatable.is_nil()).then_some(metatable));
    }
    if metatable.is_collectable()
        && let Some(owner) = value.as_gc_ptr()
    {
        state_vm.gc_barrier_back(owner);
    }
    if value.is_table() || value.is_userdata() {
        state_vm.global_state_mut().gc.check_finalizer(&value);
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getuservalue(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_TNIL;
    };
    let Some(target) = value_at(state_vm, index) else {
        let _ = state_vm.push_value(LuaValue::nil());
        return LUA_TNIL;
    };
    let value = target
        .as_userdata_mut()
        .and_then(|userdata| userdata.downcast_mut::<CUserdata>())
        .map_or(LuaValue::nil(), |userdata| userdata.uservalue);
    let kind = value_type(Some(value));
    let _ = state_vm.push_value(value);
    kind
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setuservalue(state: *mut lua_State, index: c_int) {
    let Some(state_vm) = vm(state) else { return };
    let Some(target) = value_at(state_vm, index) else {
        return;
    };
    let top = state_vm.get_top();
    let uservalue = state_vm
        .stack_get(top.saturating_sub(1))
        .unwrap_or_default();
    let _ = state_vm.set_top(top.saturating_sub(1));
    let changed = if let Some(userdata) = target
        .as_userdata_mut()
        .and_then(|userdata| userdata.downcast_mut::<CUserdata>())
    {
        userdata.uservalue = uservalue;
        true
    } else {
        false
    };
    if changed
        && uservalue.is_collectable()
        && let Some(owner) = target.as_gc_ptr()
    {
        state_vm.gc_barrier_back(owner);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_callk_impl(
    state: *mut lua_State,
    nargs: c_int,
    nresults: c_int,
    context: lua_KContext,
    continuation: lua_KFunction,
) -> c_int {
    let root = api(state).map_or(ptr::null_mut(), |wrapper| wrapper.root);
    let _active = ActiveRootGuard::enter(root);
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let nargs = nargs.max(0) as usize;
    let function_index = state_vm.get_top().saturating_sub(nargs + 1);
    let continuation_frame = state_vm.call_depth().saturating_sub(1);
    let nonyieldable = continuation.is_none();
    if nonyieldable {
        state_vm.nny += 1;
    }
    let result = state_vm.call_stack_based(function_index, nargs);
    if nonyieldable {
        state_vm.nny -= 1;
    }
    match result {
        Ok(actual) => {
            adjust_results(state_vm, function_index, actual, nresults);
            LUA_OK
        }
        Err(LuaError::Yield) => {
            save_c_continuation(
                state_vm,
                state,
                continuation,
                context,
                LUA_YIELD,
                continuation_frame,
                function_index,
                None,
            );
            LUA_YIELD
        }
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_pcallk_impl(
    state: *mut lua_State,
    nargs: c_int,
    nresults: c_int,
    error_function: c_int,
    context: lua_KContext,
    continuation: lua_KFunction,
) -> c_int {
    let root = api(state).map_or(ptr::null_mut(), |wrapper| wrapper.root);
    let _active = ActiveRootGuard::enter(root);
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let nargs = nargs.max(0) as usize;
    let function_index = state_vm.get_top().saturating_sub(nargs + 1);
    let continuation_frame = state_vm.call_depth().saturating_sub(1);
    let handler_index = (error_function != 0)
        .then(|| absolute_index(state_vm, error_function))
        .flatten();
    if error_function != 0 && handler_index.is_none() {
        return LUA_ERRRUN;
    }
    let nonyieldable = continuation.is_none();
    if nonyieldable {
        state_vm.nny += 1;
    }
    let result = if let Some(handler_index) = handler_index {
        state_vm.xpcall_stack_based(function_index, nargs, handler_index)
    } else {
        state_vm.pcall_stack_based(function_index, nargs)
    };
    if nonyieldable {
        state_vm.nny -= 1;
    }
    match result {
        Ok((true, actual)) => {
            adjust_results(state_vm, function_index, actual, nresults);
            LUA_OK
        }
        Ok((false, _)) => LUA_ERRRUN,
        Err(LuaError::Yield) => {
            save_c_continuation(
                state_vm,
                state,
                continuation,
                context,
                LUA_YIELD,
                continuation_frame,
                function_index,
                handler_index,
            );
            LUA_YIELD
        }
        Err(error) => push_error(state_vm, error),
    }
}

unsafe fn load_bytes(state: *mut lua_State, bytes: &[u8], name: &str, mode: Option<&str>) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let binary = bytes.first() == Some(&0x1b);
    if mode.is_some_and(|mode| binary && !mode.contains('b') || !binary && !mode.contains('t')) {
        let message = state_vm
            .create_string("attempt to load a chunk with incompatible mode")
            .unwrap_or_default();
        let _ = state_vm.push_value(message);
        return LUA_ERRSYNTAX;
    }
    let loaded = if binary {
        if bytes.get(4) != Some(&0x53) {
            Err("binary chunk is not standard Lua 5.3".to_string())
        } else {
            chunk53::load(bytes, state_vm.global_state_mut()).and_then(|chunk| {
                let global = state_vm.global_state().global;
                let env = state_vm
                    .global_state_mut()
                    .create_upvalue_closed(global)
                    .map_err(|error| state_vm.get_error_message(error))?;
                state_vm
                    .global_state_mut()
                    .create_loaded_function(chunk, UpvalueStore::from_single(env))
                    .map_err(|error| state_vm.get_error_message(error))
            })
        }
    } else if state_vm.global_state().language() == LuaLanguageLevel::Lua53 {
        state_vm
            .load_bytes_with_name(bytes, name)
            .map_err(|error| state_vm.get_error_message(error))
    } else {
        match std::str::from_utf8(bytes) {
            Ok(source) => state_vm
                .load_with_name(source, name)
                .map_err(|error| state_vm.get_error_message(error)),
            Err(_) => Err("source is not valid UTF-8".to_string()),
        }
    };
    match loaded {
        Ok(function) => {
            let _ = state_vm.push_value(function);
            LUA_OK
        }
        Err(message) => {
            if let Ok(error) = state_vm.create_string(&message) {
                let _ = state_vm.push_value(error);
            }
            LUA_ERRSYNTAX
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_load(
    state: *mut lua_State,
    reader: lua_Reader,
    data: *mut c_void,
    chunk_name: *const c_char,
    mode: *const c_char,
) -> c_int {
    let Some(reader) = reader else {
        return LUA_ERRSYNTAX;
    };
    let mut bytes = Vec::new();
    loop {
        let mut length = 0usize;
        let mut pointer = ptr::null();
        let status = tex_lua_invoke_reader(reader, state, data, &mut pointer, &mut length);
        if status != LUA_OK {
            if let Some(wrapper) = api(state) {
                wrapper.pending_error.take();
            }
            return status;
        }
        if pointer.is_null() || length == 0 {
            break;
        }
        bytes.extend_from_slice(slice::from_raw_parts(pointer.cast::<u8>(), length));
        if bytes.len() > (128 << 20) {
            return LUA_ERRMEM;
        }
    }
    let name = if chunk_name.is_null() {
        "=(load)".into()
    } else {
        CStr::from_ptr(chunk_name).to_string_lossy()
    };
    let mode = (!mode.is_null()).then(|| CStr::from_ptr(mode).to_string_lossy());
    load_bytes(state, &bytes, &name, mode.as_deref())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_dump(
    state: *mut lua_State,
    writer: lua_Writer,
    data: *mut c_void,
    strip: c_int,
) -> c_int {
    let Some(writer) = writer else { return 1 };
    let Some(state_vm) = vm(state) else { return 1 };
    let Some(function_value) = value_at(state_vm, -1) else {
        return 1;
    };
    let Some(function) = function_value.as_lua_function() else {
        return 1;
    };
    let Ok(bytes) = chunk53::dump(function.chunk(), strip != 0) else {
        return 1;
    };
    let mut result = 0;
    let status = tex_lua_invoke_writer(
        writer,
        state,
        bytes.as_ptr().cast(),
        bytes.len(),
        data,
        &mut result,
    );
    if status != LUA_OK {
        if let Some(wrapper) = api(state) {
            wrapper.pending_error.take();
        }
        return status;
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_capture_error(state: *mut lua_State) -> c_int {
    let Some(wrapper) = api(state) else {
        return LUA_ERRRUN;
    };
    let error = if let Some(state) = wrapper.state.as_mut() {
        value_at(state, -1)
            .unwrap_or_else(|| state.create_string("Lua C API error").unwrap_or_default())
    } else {
        LuaValue::nil()
    };
    wrapper.pending_error = Some(error);
    LUA_ERRRUN
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_next_impl(
    state: *mut lua_State,
    index: c_int,
    result: *mut c_int,
) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let Some(table_value) = value_at(state_vm, index) else {
        let error = state_vm.error("table index is missing".to_string());
        return push_error(state_vm, error);
    };
    let top = state_vm.get_top();
    let key = state_vm
        .stack_get(top.saturating_sub(1))
        .unwrap_or_default();
    let _ = state_vm.set_top(top.saturating_sub(1));
    let Some(table) = table_value.as_table() else {
        let error = state_vm.error("table expected".to_string());
        return push_error(state_vm, error);
    };
    match table.next(&key) {
        Ok(Some((next_key, value))) => {
            let _ = state_vm.push_value(next_key);
            let _ = state_vm.push_value(value);
            if let Some(result) = result.as_mut() {
                *result = 1;
            }
            LUA_OK
        }
        Ok(None) => {
            if let Some(result) = result.as_mut() {
                *result = 0;
            }
            LUA_OK
        }
        Err(()) => {
            let error = state_vm.error("invalid key to 'next'".to_string());
            push_error(state_vm, error)
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_concat_impl(state: *mut lua_State, count: c_int) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let count = count.max(0) as usize;
    if count == 0 {
        return match state_vm
            .create_string("")
            .and_then(|value| state_vm.push_value(value))
        {
            Ok(()) => LUA_OK,
            Err(error) => push_error(state_vm, error),
        };
    }
    if count == 1 {
        return LUA_OK;
    }
    state_vm.nny += 1;
    let result = crate::lua_vm::execute::concat::concat(state_vm, None, count);
    state_vm.nny -= 1;
    match result {
        Ok(()) => LUA_OK,
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_len_impl(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let Some(value) = value_at(state_vm, index) else {
        return LUA_ERRRUN;
    };
    state_vm.nny += 1;
    let result = state_vm.obj_len(&value);
    state_vm.nny -= 1;
    match result {
        Ok(length) => match state_vm.push_value(LuaValue::integer(length)) {
            Ok(()) => LUA_OK,
            Err(error) => push_error(state_vm, error),
        },
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_stringtonumber(state: *mut lua_State, string: *const c_char) -> usize {
    if string.is_null() {
        return 0;
    }
    let text = CStr::from_ptr(string).to_string_lossy();
    let value = crate::stdlib::basic::parse_number::parse_lua_number(&text);
    if value.is_nil() {
        return 0;
    }
    if let Some(state) = vm(state) {
        let _ = state.push_value(value);
    }
    text.len() + 1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gc(state: *mut lua_State, what: c_int, data: c_int) -> c_int {
    let Some(state_vm) = vm(state) else { return -1 };
    let state_ptr = state_vm as *mut LuaState;
    let global = state_vm.global_state_handle();
    match what {
        LUA_GCSTOP => {
            global.gc_set_running(false);
            0
        }
        LUA_GCRESTART => {
            global.gc_set_running(true);
            0
        }
        LUA_GCCOLLECT => {
            global.full_gc(state_ptr, false);
            0
        }
        LUA_GCCOUNT => (global.gc_total_bytes().max(0) / 1024).min(c_int::MAX as isize) as c_int,
        LUA_GCCOUNTB => (global.gc_total_bytes().max(0) % 1024) as c_int,
        LUA_GCSTEP => c_int::from(global.gc_step(state_ptr, data)),
        LUA_GCSETPAUSE => global.gc_set_parameter(crate::gc::PAUSE, data),
        LUA_GCSETSTEPMUL => global.gc_set_parameter(crate::gc::STEPMUL, data),
        LUA_GCISRUNNING => c_int::from(global.gc_is_running()),
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getallocf(
    state: *mut lua_State,
    userdata: *mut *mut c_void,
) -> lua_Alloc {
    let Some(wrapper) = api(state) else {
        return None;
    };
    let root = if wrapper.root.is_null() {
        state
    } else {
        wrapper.root
    };
    let Some(root) = root.as_mut() else {
        return None;
    };
    if let Some(slot) = userdata.as_mut() {
        *slot = root.allocator_ud;
    }
    root.allocator
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setallocf(
    state: *mut lua_State,
    allocator: lua_Alloc,
    userdata: *mut c_void,
) {
    let Some(wrapper) = api(state) else { return };
    let root = if wrapper.root.is_null() {
        state
    } else {
        wrapper.root
    };
    let Some(root) = root.as_mut() else { return };
    root.allocator = allocator.or(Some(tex_lua_default_alloc));
    root.allocator_ud = userdata;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_newthread(state: *mut lua_State) -> *mut lua_State {
    let Some(parent) = api(state) else {
        return ptr::null_mut();
    };
    let Some(parent_vm) = parent.state.as_mut() else {
        return ptr::null_mut();
    };
    let Ok(value) = parent_vm.create_thread() else {
        return ptr::null_mut();
    };
    let Some(thread) = value.as_thread_mut() else {
        return ptr::null_mut();
    };
    let child_state = thread as *mut LuaState;
    let _ = parent_vm.push_value(value);
    let root = if parent.root.is_null() {
        state
    } else {
        parent.root
    };
    canonical_wrapper(root, child_state)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tothread(state: *mut lua_State, index: c_int) -> *mut lua_State {
    let Some(parent) = api(state) else {
        return ptr::null_mut();
    };
    let Some(parent_vm) = parent.state.as_mut() else {
        return ptr::null_mut();
    };
    let Some(thread_value) = value_at(parent_vm, index) else {
        return ptr::null_mut();
    };
    let Some(thread) = thread_value.as_thread_mut() else {
        return ptr::null_mut();
    };
    let thread_pointer = thread as *mut LuaState;
    let root = if parent.root.is_null() {
        state
    } else {
        parent.root
    };
    canonical_wrapper(root, thread_pointer)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_status(state: *mut lua_State) -> c_int {
    let Some(state) = vm(state) else {
        return LUA_ERRRUN;
    };
    if state.is_yielded() {
        LUA_YIELD
    } else if state.is_dead() {
        LUA_ERRRUN
    } else {
        LUA_OK
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isyieldable(state: *mut lua_State) -> c_int {
    c_int::from(vm(state).is_some_and(|state| !state.is_main_thread() && state.nny == 0))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_resume(
    state: *mut lua_State,
    _from: *mut lua_State,
    nargs: c_int,
) -> c_int {
    let Some(wrapper) = api(state) else {
        return LUA_ERRRUN;
    };
    let root = wrapper.root;
    let _active = ActiveRootGuard::enter(root);
    let Some(state_vm) = wrapper.state.as_mut() else {
        return LUA_ERRRUN;
    };
    let count = nargs.max(0) as usize;
    let args = if let Some(parking) = wrapper.suspended_stack.take() {
        let external = state_vm.restore_stack_from_c_api(parking);
        if count > external.len() {
            let error = state_vm.error("not enough arguments to resume".to_string());
            return push_error(state_vm, error);
        }
        external[external.len() - count..].to_vec()
    } else {
        let top = state_vm.get_top();
        let start = top.saturating_sub(count);
        let args: Vec<_> = (start..top)
            .filter_map(|slot| state_vm.stack_get(slot))
            .collect();
        let _ = state_vm.set_top(start);
        args
    };

    match state_vm.resume(args) {
        Ok((true, results)) => {
            let _ = state_vm.set_top(0);
            for value in results {
                let _ = state_vm.push_value(value);
            }
            LUA_OK
        }
        Ok((false, results)) => {
            wrapper.suspended_stack = Some(state_vm.park_stack_for_c_api(results));
            LUA_YIELD
        }
        Err(error) => {
            let status = push_error(state_vm, error);
            let error_value = state_vm
                .stack_get(state_vm.get_top().saturating_sub(1))
                .unwrap_or_default();
            wrapper.suspended_stack = Some(state_vm.park_stack_for_c_api(vec![error_value]));
            status
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_yieldk_impl(
    state: *mut lua_State,
    nresults: c_int,
    context: lua_KContext,
    continuation: lua_KFunction,
) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    if state_vm.nny > 0 || state_vm.call_depth() == 0 {
        let message = if state_vm.is_main_thread() {
            "attempt to yield from outside a coroutine"
        } else {
            "attempt to yield across a C-call boundary"
        };
        let error = state_vm.error(message.to_string());
        return push_error(state_vm, error);
    }
    let count = nresults.max(0) as usize;
    let start = state_vm.get_top().saturating_sub(count);
    let values = (start..state_vm.get_top())
        .filter_map(|slot| state_vm.stack_get(slot))
        .collect();
    let function_index = state_vm
        .current_frame()
        .map_or(0, |frame| frame.func_index());
    let continuation_frame = state_vm.call_depth().saturating_sub(1);
    save_c_continuation(
        state_vm,
        state,
        continuation,
        context,
        LUA_YIELD,
        continuation_frame,
        function_index,
        None,
    );
    state_vm.set_yield(values);
    LUA_YIELD
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_loadbufferx(
    state: *mut lua_State,
    buffer: *const c_char,
    size: usize,
    name: *const c_char,
    mode: *const c_char,
) -> c_int {
    if buffer.is_null() && size != 0 {
        return LUA_ERRSYNTAX;
    }
    let bytes = if size == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(buffer.cast::<u8>(), size)
    };
    let name = if name.is_null() {
        "=(load)".into()
    } else {
        CStr::from_ptr(name).to_string_lossy()
    };
    let mode = (!mode.is_null()).then(|| CStr::from_ptr(mode).to_string_lossy());
    load_bytes(state, bytes, &name, mode.as_deref())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_loadstring(state: *mut lua_State, source: *const c_char) -> c_int {
    if source.is_null() {
        return LUA_ERRSYNTAX;
    }
    let source = CStr::from_ptr(source);
    luaL_loadbufferx(
        state,
        source.as_ptr(),
        source.to_bytes().len(),
        c"=(loadstring)".as_ptr(),
        c"t".as_ptr(),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_loadfilex(
    state: *mut lua_State,
    filename: *const c_char,
    mode: *const c_char,
) -> c_int {
    let (mut bytes, chunk_name) = if filename.is_null() {
        let mut bytes = Vec::new();
        if let Err(error) = std::io::stdin().read_to_end(&mut bytes) {
            if let Some(state_vm) = vm(state)
                && let Ok(message) = state_vm.create_string(&error.to_string())
            {
                let _ = state_vm.push_value(message);
            }
            return LUA_ERRFILE;
        }
        (bytes, "=stdin".to_string())
    } else {
        let path = CStr::from_ptr(filename).to_string_lossy();
        match std::fs::read(path.as_ref()) {
            Ok(bytes) => (bytes, format!("@{path}")),
            Err(error) => {
                if let Some(state_vm) = vm(state)
                    && let Ok(message) =
                        state_vm.create_string(&format!("cannot open {path}: {error}"))
                {
                    let _ = state_vm.push_value(message);
                }
                return LUA_ERRFILE;
            }
        }
    };

    let mut start = usize::from(bytes.starts_with(&[0xEF, 0xBB, 0xBF])) * 3;
    if bytes.get(start) == Some(&b'#') {
        start = bytes[start..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(bytes.len(), |offset| start + offset + 1);
        if bytes.get(start) != Some(&0x1b) {
            bytes.insert(start, b'\n');
        }
    }
    let mode = (!mode.is_null()).then(|| CStr::from_ptr(mode).to_string_lossy());
    load_bytes(state, &bytes[start..], &chunk_name, mode.as_deref())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawequal(state: *mut lua_State, left: c_int, right: c_int) -> c_int {
    let Some(state) = vm(state) else { return 0 };
    c_int::from(
        value_at(state, left)
            .zip(value_at(state, right))
            .is_some_and(|(a, b)| a == b),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tex_lua_compare_impl(
    state: *mut lua_State,
    left_index: c_int,
    right_index: c_int,
    operation: c_int,
    result: *mut c_int,
) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_ERRRUN;
    };
    let Some((left, right)) = value_at(state_vm, left_index).zip(value_at(state_vm, right_index))
    else {
        if let Some(result) = result.as_mut() {
            *result = 0;
        }
        return LUA_OK;
    };
    state_vm.nny += 1;
    let comparison = match operation {
        LUA_OPEQ => crate::lua_vm::execute::helper::equalobj(state_vm, left, right),
        LUA_OPLT => state_vm.obj_lt(&left, &right),
        LUA_OPLE => {
            if let Some((a, b)) = left.as_number().zip(right.as_number()) {
                Ok(a <= b)
            } else if let Some((a, b)) = left.as_bytes().zip(right.as_bytes()) {
                Ok(a <= b)
            } else {
                match crate::lua_vm::execute::metamethod::try_comp_tm(
                    state_vm,
                    left,
                    right,
                    crate::lua_vm::TmKind::Le,
                ) {
                    Ok(Some(value)) => Ok(value),
                    Ok(None) if state_vm.global_state().language() == LuaLanguageLevel::Lua53 => {
                        state_vm.obj_lt(&right, &left).map(|value| !value)
                    }
                    Ok(None) => {
                        Err(state_vm.error("attempt to compare incompatible values".to_string()))
                    }
                    Err(error) => Err(error),
                }
            }
        }
        _ => Ok(false),
    };
    state_vm.nny -= 1;
    match comparison {
        Ok(value) => {
            if let Some(result) = result.as_mut() {
                *result = c_int::from(value);
            }
            LUA_OK
        }
        Err(error) => push_error(state_vm, error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_ref(state: *mut lua_State, index: c_int) -> c_int {
    let Some(state_vm) = vm(state) else {
        return LUA_NOREF;
    };
    let Some(table) = value_at(state_vm, index) else {
        return LUA_NOREF;
    };
    let top = state_vm.get_top();
    let value = state_vm
        .stack_get(top.saturating_sub(1))
        .unwrap_or_default();
    let _ = state_vm.set_top(top.saturating_sub(1));
    if value.is_nil() {
        return LUA_REFNIL;
    }
    let free_head = state_vm
        .raw_geti(&table, 0)
        .and_then(|value| value.as_integer())
        .unwrap_or(0);
    let reference = if free_head > 0 && free_head <= c_int::MAX as i64 {
        let next = state_vm
            .raw_geti(&table, free_head)
            .and_then(|value| value.as_integer())
            .unwrap_or(0);
        state_vm.raw_seti(&table, 0, LuaValue::integer(next));
        free_head as c_int
    } else {
        let Some(length) = table.as_table().map(|table| table.len()) else {
            return LUA_NOREF;
        };
        let Some(reference) = length
            .checked_add(1)
            .and_then(|value| c_int::try_from(value).ok())
        else {
            return LUA_NOREF;
        };
        reference
    };
    state_vm.raw_seti(&table, reference as i64, value);
    reference
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_unref(state: *mut lua_State, index: c_int, reference: c_int) {
    if reference < 0 {
        return;
    }
    let Some(state_vm) = vm(state) else { return };
    if let Some(table) = value_at(state_vm, index) {
        let free_head = state_vm
            .raw_geti(&table, 0)
            .and_then(|value| value.as_integer())
            .unwrap_or(0);
        let next = if free_head == 0 {
            LuaValue::nil()
        } else {
            LuaValue::integer(free_head)
        };
        state_vm.raw_seti(&table, reference as i64, next);
        state_vm.raw_seti(&table, 0, LuaValue::integer(reference as i64));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_where(state: *mut lua_State, level: c_int) {
    if level < 0 {
        lua_pushstring(state, c"".as_ptr());
        return;
    }
    let mut record = lua_Debug {
        event: 0,
        name: ptr::null(),
        namewhat: ptr::null(),
        what: ptr::null(),
        source: ptr::null(),
        currentline: -1,
        linedefined: 0,
        lastlinedefined: 0,
        nups: 0,
        nparams: 0,
        isvararg: 0,
        istailcall: 0,
        short_src: [0; 60],
        i_ci: (level as usize + 1) as *mut c_void,
    };
    if lua_getinfo(state, c"Sl".as_ptr(), &mut record) == 0 || record.currentline <= 0 {
        lua_pushstring(state, c"".as_ptr());
        return;
    }
    let source_len = record
        .short_src
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(record.short_src.len());
    let source = String::from_utf8_lossy(slice::from_raw_parts(
        record.short_src.as_ptr().cast::<u8>(),
        source_len,
    ));
    let location = format!("{source}:{}: ", record.currentline);
    lua_pushlstring(state, location.as_ptr().cast(), location.len());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_traceback(
    state: *mut lua_State,
    source: *mut lua_State,
    message: *const c_char,
    level: c_int,
) {
    let source = if source.is_null() { state } else { source };
    let trace = vm(source)
        .map(|source| source.generate_traceback_from(level.max(0) as usize))
        .unwrap_or_default();
    let mut output = String::new();
    if !message.is_null() {
        output.push_str(&CStr::from_ptr(message).to_string_lossy());
        output.push('\n');
    }
    output.push_str("stack traceback:\n");
    output.push_str(&trace);
    lua_pushlstring(state, output.as_ptr().cast(), output.len());
}

fn c_closure_user_offset(closure: &crate::lua_value::CClosureFunction) -> usize {
    usize::from(closure.func() as *const () == c_callback_trampoline as *const ())
}

fn upvalue_name(function: LuaValue, index: usize) -> Option<String> {
    if let Some(function) = function.as_lua_function() {
        function.upvalues().get(index)?;
        return Some(
            function
                .chunk()
                .upvalue_descs
                .get(index)
                .map_or_else(String::new, |descriptor| descriptor.name.to_string()),
        );
    }
    if let Some(closure) = function.as_cclosure() {
        closure
            .upvalues()
            .get(index.checked_add(c_closure_user_offset(closure))?)?;
        return Some(String::new());
    }
    if let Some(closure) = function.as_rclosure() {
        closure.upvalues().get(index)?;
        return Some(String::new());
    }
    None
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getupvalue(
    state: *mut lua_State,
    function_index: c_int,
    upvalue_index: c_int,
) -> *const c_char {
    let Some(state_vm) = vm(state) else {
        return ptr::null();
    };
    let Some(function) = value_at(state_vm, function_index) else {
        return ptr::null();
    };
    let Some(index) = usize::try_from(upvalue_index)
        .ok()
        .and_then(|index| index.checked_sub(1))
    else {
        return ptr::null();
    };
    let (name, value) = if let Some(lua_function) = function.as_lua_function() {
        let Some(upvalue) = lua_function.upvalues().get(index).copied() else {
            return ptr::null();
        };
        let name = lua_function
            .chunk()
            .upvalue_descs
            .get(index)
            .map_or_else(String::new, |descriptor| descriptor.name.to_string());
        (name, upvalue.as_ref().data.get_value())
    } else if let Some(closure) = function.as_cclosure() {
        let Some(value) = closure
            .upvalues()
            .get(index + c_closure_user_offset(closure))
            .copied()
        else {
            return ptr::null();
        };
        (String::new(), value)
    } else if let Some(closure) = function.as_rclosure() {
        let Some(value) = closure.upvalues().get(index).copied() else {
            return ptr::null();
        };
        (String::new(), value)
    } else {
        return ptr::null();
    };
    let _ = state_vm.push_value(value);
    if name.is_empty() {
        c"".as_ptr()
    } else {
        cache_c_bytes(state, name.as_bytes())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setupvalue(
    state: *mut lua_State,
    function_index: c_int,
    upvalue_index: c_int,
) -> *const c_char {
    let Some(state_vm) = vm(state) else {
        return ptr::null();
    };
    let Some(function) = value_at(state_vm, function_index) else {
        return ptr::null();
    };
    let Some(index) = usize::try_from(upvalue_index)
        .ok()
        .and_then(|index| index.checked_sub(1))
    else {
        return ptr::null();
    };
    let Some(name) = upvalue_name(function, index) else {
        return ptr::null();
    };
    let top = state_vm.get_top();
    let value = state_vm
        .stack_get(top.saturating_sub(1))
        .unwrap_or_default();
    let _ = state_vm.set_top(top.saturating_sub(1));
    let mut upvalue_owner = None::<crate::gc::GcObjectPtr>;
    if let Some(lua_function) = function.as_lua_function_mut() {
        if let Some(upvalue) = lua_function.upvalues_mut().get_mut(index) {
            upvalue.as_mut_ref().data.set_value(value);
            upvalue_owner = Some((*upvalue).into());
        }
    } else if let Some(closure) = function.as_cclosure_mut() {
        let offset = c_closure_user_offset(closure);
        if let Some(slot) = closure.upvalues_mut().get_mut(index + offset) {
            *slot = value;
        }
    } else if let Some(closure) = function.as_rclosure_mut()
        && let Some(slot) = closure.upvalues_mut().get_mut(index)
    {
        *slot = value;
    }
    if value.is_collectable() {
        if let Some(owner) = upvalue_owner.or_else(|| function.as_gc_ptr()) {
            state_vm.gc_barrier_back(owner);
        }
    }
    if name.is_empty() {
        c"".as_ptr()
    } else {
        cache_c_bytes(state, name.as_bytes())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_upvalueid(
    state: *mut lua_State,
    function_index: c_int,
    upvalue_index: c_int,
) -> *mut c_void {
    let Some(state_vm) = vm(state) else {
        return ptr::null_mut();
    };
    let Some(function) = value_at(state_vm, function_index) else {
        return ptr::null_mut();
    };
    let Some(index) = usize::try_from(upvalue_index)
        .ok()
        .and_then(|index| index.checked_sub(1))
    else {
        return ptr::null_mut();
    };
    if let Some(function) = function.as_lua_function() {
        return function
            .upvalues()
            .get(index)
            .map_or(ptr::null_mut(), |upvalue| {
                upvalue.as_ptr().cast_mut().cast()
            });
    }
    if let Some(closure) = function.as_cclosure() {
        let offset = c_closure_user_offset(closure);
        return closure
            .upvalues()
            .get(index + offset)
            .map_or(ptr::null_mut(), |upvalue| {
                (upvalue as *const LuaValue).cast_mut().cast()
            });
    }
    if let Some(closure) = function.as_rclosure() {
        return closure
            .upvalues()
            .get(index)
            .map_or(ptr::null_mut(), |upvalue| {
                (upvalue as *const LuaValue).cast_mut().cast()
            });
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_upvaluejoin(
    state: *mut lua_State,
    first_function_index: c_int,
    first_upvalue_index: c_int,
    second_function_index: c_int,
    second_upvalue_index: c_int,
) {
    let Some(state_vm) = vm(state) else { return };
    let Some(first) = value_at(state_vm, first_function_index) else {
        return;
    };
    let Some(second) = value_at(state_vm, second_function_index) else {
        return;
    };
    let (Some(first_index), Some(second_index)) = (
        usize::try_from(first_upvalue_index)
            .ok()
            .and_then(|index| index.checked_sub(1)),
        usize::try_from(second_upvalue_index)
            .ok()
            .and_then(|index| index.checked_sub(1)),
    ) else {
        return;
    };
    let Some(shared) = second
        .as_lua_function()
        .and_then(|function| function.upvalues().get(second_index))
        .copied()
    else {
        return;
    };
    let Some(slot) = first
        .as_lua_function_mut()
        .and_then(|function| function.upvalues_mut().get_mut(first_index))
    else {
        return;
    };
    *slot = shared;
    if let Some(owner) = first.as_gc_ptr() {
        state_vm.gc_barrier_back(owner);
    }
}

fn debug_level(record: *const lua_Debug) -> Option<usize> {
    let encoded = unsafe { record.as_ref() }?.i_ci as usize;
    encoded.checked_sub(1)
}

fn active_local_slot(
    state: &LuaState,
    level: usize,
    local_index: usize,
) -> Option<(String, usize)> {
    if local_index == 0 || level >= state.call_depth() {
        return None;
    }
    let frame_index = state.call_depth() - 1 - level;
    let function = state.get_frame_func(frame_index)?;
    let function = function.as_lua_function()?;
    let frame = state.get_frame(frame_index)?;
    let pc = frame.pc.saturating_sub(1) as usize;
    let mut active = 0usize;
    for local in &function.chunk().locals {
        if local.startpc as usize > pc {
            break;
        }
        if pc < local.endpc as usize {
            active += 1;
            if active == local_index {
                return Some((local.name.to_string(), frame.base + active - 1));
            }
        }
    }
    None
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getstack(
    state: *mut lua_State,
    level: c_int,
    record: *mut lua_Debug,
) -> c_int {
    if level < 0 || record.is_null() {
        return 0;
    }
    let Some(state_vm) = vm(state) else { return 0 };
    if state_vm.get_info_by_level(level as usize, "").is_none() {
        return 0;
    }
    (*record).i_ci = (level as usize + 1) as *mut c_void;
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getinfo(
    state: *mut lua_State,
    what: *const c_char,
    record: *mut lua_Debug,
) -> c_int {
    if what.is_null() || record.is_null() {
        return 0;
    }
    let options = CStr::from_ptr(what).to_string_lossy();
    let function_mode = options.starts_with('>');
    let options = options.strip_prefix('>').unwrap_or(&options);
    let Some(state_vm) = vm(state) else { return 0 };
    let (info, hidden_callback_upvalue) = if function_mode {
        let top = state_vm.get_top();
        let Some(function) = state_vm.stack_get(top.saturating_sub(1)) else {
            return 0;
        };
        let _ = state_vm.set_top(top.saturating_sub(1));
        let hidden = function
            .as_cclosure()
            .is_some_and(|closure| c_closure_user_offset(closure) != 0);
        (state_vm.get_info_for_func(&function, options), hidden)
    } else {
        let Some(level) = debug_level(record) else {
            return 0;
        };
        let Some(frame_index) = state_vm.call_depth().checked_sub(level + 1) else {
            return 0;
        };
        let hidden = state_vm
            .get_frame_func(frame_index)
            .is_some_and(|function| {
                function
                    .as_cclosure()
                    .is_some_and(|closure| c_closure_user_offset(closure) != 0)
            });
        let Some(info) = state_vm.get_info_by_level(level, options) else {
            return 0;
        };
        (info, hidden)
    };

    if options.contains('n') {
        (*record).name = info
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .map_or(ptr::null(), |name| cache_c_bytes(state, name.as_bytes()));
        (*record).namewhat = info.namewhat.as_deref().map_or(c"".as_ptr(), |name| {
            if name.is_empty() {
                c"".as_ptr()
            } else {
                cache_c_bytes(state, name.as_bytes())
            }
        });
    }
    if options.contains('S') {
        (*record).source = info.source.as_deref().map_or(c"=?".as_ptr(), |source| {
            cache_c_bytes(state, source.as_bytes())
        });
        (*record).what = match info.what {
            Some("Lua") => c"Lua".as_ptr(),
            Some("C") => c"C".as_ptr(),
            Some("main") => c"main".as_ptr(),
            Some("tail") => c"tail".as_ptr(),
            _ => c"".as_ptr(),
        };
        (*record).linedefined = info.linedefined.unwrap_or(-1);
        (*record).lastlinedefined = info.lastlinedefined.unwrap_or(-1);
        (*record).short_src.fill(0);
        if let Some(short) = info.short_src.as_deref() {
            let bytes = short.as_bytes();
            let count = bytes.len().min((*record).short_src.len() - 1);
            ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (*record).short_src.as_mut_ptr().cast::<u8>(),
                count,
            );
        }
    }
    if options.contains('l') {
        (*record).currentline = info.currentline.unwrap_or(-1);
    }
    if options.contains('u') {
        let nups = info.nups.unwrap_or(0);
        (*record).nups = nups.saturating_sub(u8::from(hidden_callback_upvalue));
        (*record).nparams = info.nparams.unwrap_or(0);
        (*record).isvararg = c_char::from(info.isvararg.unwrap_or(false));
    }
    if options.contains('t') {
        (*record).istailcall = c_char::from(info.istailcall.unwrap_or(false));
    }
    if options.contains('f') {
        let _ = state_vm.push_value(info.func.unwrap_or_default());
    }
    if options.contains('L') {
        let lines = info.activelines.unwrap_or_default();
        let Ok(table) = state_vm.create_table(0, lines.len()) else {
            return 0;
        };
        for line in lines {
            state_vm.raw_seti(&table, line as i64, LuaValue::boolean(true));
        }
        let _ = state_vm.push_value(table);
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getlocal(
    state: *mut lua_State,
    record: *const lua_Debug,
    local_index: c_int,
) -> *const c_char {
    if local_index <= 0 {
        return ptr::null();
    }
    let Some(state_vm) = vm(state) else {
        return ptr::null();
    };
    if record.is_null() {
        let Some(function_value) = value_at(state_vm, -1) else {
            return ptr::null();
        };
        let Some(function) = function_value.as_lua_function() else {
            return ptr::null();
        };
        let Some(local) = function.chunk().locals.get(local_index as usize - 1) else {
            return ptr::null();
        };
        return cache_c_bytes(state, local.name.as_bytes());
    }
    let Some(level) = debug_level(record) else {
        return ptr::null();
    };
    let Some((name, value)) = state_vm.get_local(level, local_index as usize) else {
        return ptr::null();
    };
    let _ = state_vm.push_value(value);
    cache_c_bytes(state, name.as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setlocal(
    state: *mut lua_State,
    record: *const lua_Debug,
    local_index: c_int,
) -> *const c_char {
    let Some(state_vm) = vm(state) else {
        return ptr::null();
    };
    let top = state_vm.get_top();
    let value = state_vm
        .stack_get(top.saturating_sub(1))
        .unwrap_or_default();
    let _ = state_vm.set_top(top.saturating_sub(1));
    if local_index <= 0 {
        return ptr::null();
    }
    let Some(level) = debug_level(record) else {
        return ptr::null();
    };
    let Some((name, slot)) = active_local_slot(state_vm, level, local_index as usize) else {
        return ptr::null();
    };
    let _ = state_vm.stack_set(slot, value);
    cache_c_bytes(state, name.as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_sethook(
    state: *mut lua_State,
    hook: lua_Hook,
    mask: c_int,
    count: c_int,
) {
    let Some(wrapper) = api(state) else { return };
    wrapper.c_hook = hook;
    let root = if wrapper.root.is_null() {
        state
    } else {
        wrapper.root
    };
    let Some(state_vm) = wrapper.state.as_mut() else {
        return;
    };
    let Some(hook) = hook else {
        wrapper.c_hook_mask = 0;
        wrapper.c_hook_count = 0;
        state_vm.set_hook(LuaValue::nil(), 0, 0);
        return;
    };
    wrapper.c_hook_mask = mask;
    wrapper.c_hook_count = count;
    let closure = state_vm.global_state_mut().create_closure(move |state_vm| {
        let base = frame_base(state_vm);
        let event_name = state_vm
            .stack_get(base)
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        let event = match event_name.as_str() {
            "call" => LUA_HOOKCALL,
            "return" => LUA_HOOKRET,
            "line" => LUA_HOOKLINE,
            "count" => LUA_HOOKCOUNT,
            "tail call" => LUA_HOOKTAILCALL,
            _ => LUA_HOOKCOUNT,
        };
        let currentline = state_vm
            .stack_get(base + 1)
            .and_then(|value| value.as_integer())
            .map_or(-1, |line| line as c_int);
        let wrapper = unsafe { canonical_wrapper(root, state_vm as *mut LuaState) };
        if wrapper.is_null() {
            return Err(state_vm.error("debug hook has no canonical lua_State".to_string()));
        }
        let mut record = lua_Debug {
            event,
            name: ptr::null(),
            namewhat: ptr::null(),
            what: ptr::null(),
            source: ptr::null(),
            currentline,
            linedefined: 0,
            lastlinedefined: 0,
            nups: 0,
            nparams: 0,
            isvararg: 0,
            istailcall: 0,
            short_src: [0; 60],
            i_ci: 2usize as *mut c_void,
        };
        let status = unsafe { tex_lua_invoke_hook(hook, wrapper, &mut record) };
        if let Some(error) = unsafe { &mut *wrapper }.pending_error.take() {
            state_vm.set_error_object(error);
            return Err(LuaError::RuntimeError);
        }
        if status != LUA_OK {
            return Err(state_vm.error("C debug hook raised an error".to_string()));
        }
        Ok(0)
    });
    match closure {
        Ok(closure) => state_vm.set_hook(closure, mask as u8, count),
        Err(error) => {
            let had_object = state_vm.has_error_object();
            let object = state_vm.take_error_object();
            let value = if had_object {
                object
            } else {
                let message = state_vm.get_error_message(error);
                state_vm.create_string(&message).unwrap_or_default()
            };
            wrapper.pending_error = Some(value);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gethook(state: *mut lua_State) -> lua_Hook {
    api(state).and_then(|wrapper| wrapper.c_hook)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gethookmask(state: *mut lua_State) -> c_int {
    api(state).map_or(0, |wrapper| wrapper.c_hook_mask)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gethookcount(state: *mut lua_State) -> c_int {
    api(state).map_or(0, |wrapper| wrapper.c_hook_count)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_gsub(
    state: *mut lua_State,
    source: *const c_char,
    pattern: *const c_char,
    replacement: *const c_char,
) -> *const c_char {
    if source.is_null() || pattern.is_null() || replacement.is_null() {
        lua_pushnil(state);
        return ptr::null();
    }
    let source = CStr::from_ptr(source).to_bytes();
    let pattern = CStr::from_ptr(pattern).to_bytes();
    let replacement = CStr::from_ptr(replacement).to_bytes();
    let mut result = Vec::with_capacity(source.len());
    if pattern.is_empty() {
        result.extend_from_slice(replacement);
        for byte in source {
            result.push(*byte);
            result.extend_from_slice(replacement);
        }
    } else {
        let mut remaining = source;
        while let Some(offset) = remaining
            .windows(pattern.len())
            .position(|window| window == pattern)
        {
            result.extend_from_slice(&remaining[..offset]);
            result.extend_from_slice(replacement);
            remaining = &remaining[offset + pattern.len()..];
        }
        result.extend_from_slice(remaining);
    }
    lua_pushlstring(state, result.as_ptr().cast(), result.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_fileresult(
    state: *mut lua_State,
    success: c_int,
    filename: *const c_char,
) -> c_int {
    if success != 0 {
        lua_pushboolean(state, 1);
        return 1;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    let error = std::io::Error::from_raw_os_error(errno);
    lua_pushnil(state);
    let message = if filename.is_null() {
        error.to_string()
    } else {
        format!("{}: {error}", CStr::from_ptr(filename).to_string_lossy())
    };
    lua_pushlstring(state, message.as_ptr().cast(), message.len());
    lua_pushinteger(state, errno as lua_Integer);
    3
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_execresult(state: *mut lua_State, status: c_int) -> c_int {
    #[cfg(unix)]
    let (success, kind, code) = if libc::WIFEXITED(status) {
        let code = libc::WEXITSTATUS(status);
        (code == 0, c"exit".as_ptr(), code)
    } else if libc::WIFSIGNALED(status) {
        (false, c"signal".as_ptr(), libc::WTERMSIG(status))
    } else {
        (false, c"exit".as_ptr(), status)
    };
    #[cfg(not(unix))]
    let (success, kind, code) = (status == 0, c"exit".as_ptr(), status);

    if success {
        lua_pushboolean(state, 1);
    } else {
        lua_pushnil(state);
    }
    lua_pushstring(state, kind);
    lua_pushinteger(state, code as lua_Integer);
    3
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_buffinit(state: *mut lua_State, buffer: *mut luaL_Buffer) {
    let Some(buffer) = buffer.as_mut() else {
        return;
    };
    buffer.b = buffer.initb.as_mut_ptr();
    buffer.size = LUAL_BUFFERSIZE;
    buffer.n = 0;
    buffer.state = state;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_prepbuffsize(
    buffer: *mut luaL_Buffer,
    additional: usize,
) -> *mut c_char {
    let Some(buffer) = buffer.as_mut() else {
        return ptr::null_mut();
    };
    if additional <= buffer.size.saturating_sub(buffer.n) {
        return buffer.b.add(buffer.n);
    }
    let Some(required) = buffer.n.checked_add(additional) else {
        return ptr::null_mut();
    };
    let new_size = buffer.size.saturating_mul(2).max(required);
    let init_pointer = buffer.initb.as_mut_ptr();
    let had_backing_userdata = buffer.b != init_pointer;
    let new_pointer = lua_newuserdata(buffer.state, new_size).cast::<c_char>();
    if new_pointer.is_null() {
        return ptr::null_mut();
    }
    ptr::copy_nonoverlapping(buffer.b, new_pointer, buffer.n);
    if had_backing_userdata {
        lua_rotate(buffer.state, -2, -1);
        lua_settop(buffer.state, -2);
    }
    buffer.b = new_pointer;
    buffer.size = new_size;
    buffer.b.add(buffer.n)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_addlstring(
    buffer: *mut luaL_Buffer,
    string: *const c_char,
    length: usize,
) {
    if string.is_null() && length != 0 {
        return;
    }
    let destination = luaL_prepbuffsize(buffer, length);
    if destination.is_null() {
        return;
    }
    if length != 0 {
        ptr::copy_nonoverlapping(string, destination, length);
    }
    (*buffer).n += length;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_addstring(buffer: *mut luaL_Buffer, string: *const c_char) {
    if string.is_null() {
        return;
    }
    luaL_addlstring(buffer, string, CStr::from_ptr(string).to_bytes().len());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_addvalue(buffer: *mut luaL_Buffer) {
    let Some(buffer_ref) = buffer.as_ref() else {
        return;
    };
    let state = buffer_ref.state;
    let mut length = 0usize;
    let string = lua_tolstring(state, -1, &mut length);
    if string.is_null() {
        return;
    }
    let bytes = slice::from_raw_parts(string.cast::<u8>(), length).to_vec();
    lua_settop(state, -2);
    luaL_addlstring(buffer, bytes.as_ptr().cast(), bytes.len());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_pushresult(buffer: *mut luaL_Buffer) {
    let Some(buffer) = buffer.as_mut() else {
        return;
    };
    let init_pointer = buffer.initb.as_mut_ptr();
    let had_backing_userdata = buffer.b != init_pointer;
    lua_pushlstring(buffer.state, buffer.b, buffer.n);
    if had_backing_userdata {
        lua_rotate(buffer.state, -2, -1);
        lua_settop(buffer.state, -2);
    }
    buffer.b = init_pointer;
    buffer.size = LUAL_BUFFERSIZE;
    buffer.n = 0;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_pushresultsize(buffer: *mut luaL_Buffer, size: usize) {
    let Some(buffer_ref) = buffer.as_mut() else {
        return;
    };
    buffer_ref.n = buffer_ref.n.saturating_add(size).min(buffer_ref.size);
    luaL_pushresult(buffer);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_buffinitsize(
    state: *mut lua_State,
    buffer: *mut luaL_Buffer,
    size: usize,
) -> *mut c_char {
    luaL_buffinit(state, buffer);
    luaL_prepbuffsize(buffer, size)
}

unsafe fn open_standard_library(
    state: *mut lua_State,
    library: Stdlib,
    global_name: &str,
) -> c_int {
    let Some(state_vm) = vm(state) else { return 0 };
    if state_vm.global_state_mut().open_stdlib(library).is_err() {
        return 0;
    }
    let value = state_vm
        .get_global_value(global_name)
        .ok()
        .flatten()
        .unwrap_or_default();
    let _ = state_vm.push_value(value);
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_base(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Basic, "_G")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_coroutine(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Coroutine, "coroutine")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_table(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Table, "table")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_io(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Io, "io")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_os(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Os, "os")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_string(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::String, "string")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_utf8(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Utf8, "utf8")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_bit32(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Bit32, "bit32")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_math(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Math, "math")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_debug(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Debug, "debug")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_package(state: *mut lua_State) -> c_int {
    open_standard_library(state, Stdlib::Package, "package")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[allow(improper_ctypes)]
    unsafe extern "C" {
        fn lua_getfield(state: *mut lua_State, index: c_int, key: *const c_char) -> c_int;
        fn lua_getmetatable(state: *mut lua_State, index: c_int) -> c_int;
        fn lua_setfield(state: *mut lua_State, index: c_int, key: *const c_char);
        fn lua_callk(
            state: *mut lua_State,
            nargs: c_int,
            nresults: c_int,
            context: lua_KContext,
            continuation: lua_KFunction,
        );
    }

    unsafe extern "C" fn fallback(state: *mut lua_State) -> c_int {
        lua_pushstring(state, c"fallback".as_ptr());
        1
    }

    #[test]
    fn c_closure_metamethod_works_without_an_active_lua_frame() {
        unsafe {
            let state = luaL_newstate();
            assert!(!state.is_null());
            lua_createtable(state, 0, 0);
            lua_createtable(state, 0, 1);
            lua_pushcclosure(state, Some(fallback), 0);
            lua_setfield(state, 2, c"__index".as_ptr());
            assert_eq!(lua_setmetatable(state, 1), 1);
            assert_eq!(lua_getfield(state, 1, c"missing".as_ptr()), LUA_TSTRING);
            assert_eq!(
                CStr::from_ptr(lua_tolstring(state, -1, ptr::null_mut())).to_bytes(),
                b"fallback"
            );
            lua_close(state);
        }
    }

    #[test]
    fn c_api_stack_growth_initializes_exposed_slots_to_nil() {
        unsafe {
            let state = luaL_newstate();
            assert!(!state.is_null());
            lua_pushinteger(state, 73);
            lua_settop(state, 0);
            lua_settop(state, 1);
            assert_eq!(lua_type(state, 1), LUA_TNIL);
            lua_close(state);
        }
    }

    #[test]
    fn first_released_auxiliary_reference_reads_as_nil() {
        unsafe {
            let state = luaL_newstate();
            assert!(!state.is_null());
            lua_createtable(state, 0, 0);
            lua_pushinteger(state, 73);
            let reference = luaL_ref(state, 1);
            assert!(reference > 0);
            luaL_unref(state, 1, reference);
            assert_eq!(lua_rawgeti(state, 1, reference as lua_Integer), LUA_TNIL);
            lua_close(state);
        }
    }

    static DEBUG_UPVALUES_ARE_HIDDEN: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn inspect_debug_record(state: *mut lua_State) -> c_int {
        let mut record: lua_Debug = std::mem::zeroed();
        if lua_getstack(state, 0, &mut record) != 0
            && lua_getinfo(state, c"u".as_ptr(), &mut record) != 0
            && record.nups == 0
        {
            DEBUG_UPVALUES_ARE_HIDDEN.store(true, Ordering::Relaxed);
        }
        0
    }

    #[test]
    fn debug_api_hides_bridge_pointer_from_c_closure_upvalues() {
        unsafe {
            DEBUG_UPVALUES_ARE_HIDDEN.store(false, Ordering::Relaxed);
            let state = luaL_newstate();
            assert!(!state.is_null());
            lua_pushcclosure(state, Some(inspect_debug_record), 0);
            lua_callk(state, 0, 0, 0, None);
            assert!(DEBUG_UPVALUES_ARE_HIDDEN.load(Ordering::Relaxed));
            lua_close(state);
        }
    }
    static CLOSE_FINALIZER_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn reregister_finalizer(state: *mut lua_State) -> c_int {
        CLOSE_FINALIZER_COUNT.fetch_add(1, Ordering::Relaxed);
        if lua_getmetatable(state, 1) != 0 {
            let _ = lua_setmetatable(state, 1);
        }
        0
    }

    #[test]
    fn close_does_not_repeat_a_reregistered_finalizer() {
        unsafe {
            CLOSE_FINALIZER_COUNT.store(0, Ordering::Relaxed);
            let state = luaL_newstate();
            assert!(!state.is_null());
            lua_createtable(state, 0, 1);
            lua_pushcclosure(state, Some(reregister_finalizer), 0);
            lua_setfield(state, -2, c"__gc".as_ptr());
            lua_pushvalue(state, 1);
            assert_eq!(lua_setmetatable(state, -2), 1);
            lua_settop(state, 0);
            lua_close(state);
            assert_eq!(CLOSE_FINALIZER_COUNT.load(Ordering::Relaxed), 1);
        }
    }
}
