// Lua execution state (equivalent to lua_State in Lua C API)
// Represents a single thread/coroutine execution context
// Multiple LuaStates can share the same LuaVM (global_State)

use crate::compiler::format_source;
use crate::gc::{
    CreateResult, GcKind, GcObjectPtr, Pooled, ProtoPtr, StringPtr, TablePtr, ThreadPtr, UpvaluePtr,
};
use crate::lua_value::userdata_trait::UserDataTrait;
use crate::lua_value::{LuaUserdata, LuaValue, LuaValueKind, UpvalueStore};
use crate::lua_vm::async_thread::AsyncFuture;
use crate::lua_vm::call_info::call_status::{
    self, CIST_C, CIST_HOOKED, CIST_RECST, CIST_XPCALL, CIST_YPCALL,
};
use crate::lua_vm::error_msg::ErrorMsg;
use crate::lua_vm::execute::call::{call_c_function, resolve_call_chain};
use crate::lua_vm::execute::{self, lua_execute};
use crate::lua_vm::lua_limits::{
    BASIC_STACK_SIZE, CSTACKERR, EXTRA_STACK, LUAI_MAXCSTACK, STACK_ERROR_ZONE,
};
use crate::lua_vm::safe_option::{LuaSafeState, SafeOption};
#[cfg(feature = "sandbox")]
use crate::lua_vm::sandbox::{SANDBOX_TIMEOUT_CHECK_INTERVAL, SandboxConfig, SandboxRuntimeLimits};
use crate::lua_vm::{
    CallInfo, CallInfoPtr, GlobalState, GlobalStateHandle, LuaError, LuaResult,
    LuaTypedAsyncCallback, LuaTypedCallback, StkId, TmKind, get_metamethod_event,
};
use crate::lua_vm::{
    LUA_HOOKCALL, LUA_HOOKCOUNT, LUA_HOOKLINE, LUA_HOOKRET, LUA_HOOKTAILCALL, async_thread,
};
#[cfg(feature = "sandbox")]
use crate::platform_time::unix_nanos;
use crate::stdlib::debug::{objtypename, ordererror, pub_getfuncname};
use crate::stdlib::lauxlib;
use crate::{
    AsyncReturnValue, DebugInfo, FromLua, IntoLua, LuaAnyRef, LuaFullError, LuaFunctionRef,
    LuaProto, LuaStringRef, LuaTableRef, UserDataRef,
};

/// Internal description of a call frame to be pushed.
///
/// Phase 1 refactor: all push variants build a `FrameInit` and then pass it to
/// the single `init_call_info` core, instead of each variant writing a full
/// `CallInfo` literal itself.
#[derive(Clone, Copy)]
pub(crate) struct FrameInit {
    pub(crate) base: usize,
    pub(crate) frame_top: usize,
    pub(crate) call_status: u32,
    pub(crate) nextraargs: i32,
    pub(crate) chunk_ptr: *const LuaProto,
    pub(crate) upvalue_ptrs: *const UpvaluePtr,
    /// Only vararg Lua frames ever read `CallInfo::nextraargs`.
    /// C Lua does not reset it on every call; neither do we.
    pub(crate) write_nextraargs: bool,
}

impl FrameInit {
    #[inline(always)]
    pub(crate) fn lua(
        base: usize,
        nresults: i32,
        max_stack_size: usize,
        chunk_ptr: *const LuaProto,
        upvalue_ptrs: *const UpvaluePtr,
        nextraargs: i32,
    ) -> Self {
        Self {
            base,
            frame_top: base + max_stack_size,
            call_status: call_status::with_nresults(call_status::CIST_LUA, nresults),
            nextraargs,
            chunk_ptr,
            upvalue_ptrs,
            write_nextraargs: unsafe { !chunk_ptr.is_null() && (*chunk_ptr).is_vararg },
        }
    }

    #[inline(always)]
    pub(crate) fn c(base: usize, nargs: usize, nresults: i32) -> Self {
        Self {
            base,
            frame_top: base + nargs,
            call_status: call_status::with_nresults(CIST_C, nresults),
            nextraargs: 0,
            chunk_ptr: std::ptr::null(),
            upvalue_ptrs: std::ptr::null(),
            write_nextraargs: false,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) struct CApiStackParking {
    stack: Vec<LuaValue>,
    stack_top: usize,
    frames: Vec<(usize, u32, u32)>,
}

/// Outcome of a protected call (`LUA_OK`, `LUA_ERRRUN`, `LUA_ERRERR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProtectedCallStatus {
    Ok,
    Error,
    /// Error handling itself failed ("error in error handling"): the message handler
    /// failed, or the stack overflowed again while an overflow was being handled.
    ErrorInHandler,
}

/// Execution state for a Lua thread/coroutine
/// This is separate from LuaVM (global_State) to support multiple execution contexts
pub struct LuaState {
    global_state: GlobalStateHandle,

    thread: ThreadPtr,
    /// Data stack - stores all values (registers, temporaries, function arguments)
    /// Layout: [frame0_values...][frame1_values...][frame2_values...]
    /// Similar to Lua's TValue stack[] in lua_State
    /// IMPORTANT: This is the PHYSICAL stack, only grows, never shrinks
    pub(crate) stack: Vec<LuaValue>,

    /// Logical stack top - index of first free slot (Lua's L->top.p)
    /// This is the actual "top" that controls which stack slots are active
    /// Values above stack_top are considered "garbage" and can be reused
    pub(crate) stack_top: usize,

    /// Call stack - one CallInfo per active function call
    /// Grows dynamically on demand (like Lua 5.5's linked list approach)
    /// Similar to Lua's CallInfo *ci in lua_State
    call_stack: Vec<CallInfoPtr>,

    /// Owns the stable CallInfo slots referenced by `call_stack`.
    call_stack_storage: Vec<Pooled<CallInfo>>,

    /// Current call depth (index into call_stack)
    /// This is the actual depth, NOT call_stack.len()
    /// Implements Lua's optimization: never shrink call_stack, only move this index
    call_depth: usize,

    /// Direct pointer to the current (top) `CallInfo`.
    /// Mirrors C Lua's `L->ci` and is kept in sync with `call_depth`.
    current_ci: *mut CallInfo,

    /// Open upvalues - upvalues pointing to stack locations
    /// Sorted Vec (higher stack indices first) for efficient lookup and close traversal.
    /// Linear scan is faster than HashMap for typical 0-5 open upvalues due to
    /// no hashing overhead and better cache locality. Matches C Lua's sorted list design.
    open_upvalues_list: Vec<UpvaluePtr>,

    /// Whether this thread is linked in the collector's `twups` list
    /// (C Lua's `L->twups != L`).
    pub(crate) in_twups: bool,

    /// Yield values storage (for coroutine yield)
    yield_values: Vec<LuaValue>,

    /// Whether hook re-entry is allowed. Set to false while a hook is running
    /// to prevent recursive hook invocations within the same thread.
    pub(crate) allow_hook: bool,

    /// Hook function (nil = no hook). Per-thread, set via debug.sethook.
    pub(crate) hook: LuaValue,

    /// Active hook mask (bitmask of LUA_MASKCALL/RET/LINE/COUNT). Per-thread.
    pub(crate) hook_mask: u8,

    /// Base value for count hook (reset interval). Per-thread.
    pub(crate) base_hook_count: i32,

    /// Current countdown for count hook (per-thread, counts instructions independently).
    pub(crate) hook_count: i32,

    /// Last instruction PC (0-based index) seen by the line hook (like C Lua's L->oldpc).
    /// Stored per-thread so the main execution loop doesn't waste a register on it.
    /// Used by hook_check_instruction to detect line changes via changedline logic.
    /// Also set by rethook/return paths to the caller's current PC.
    pub(crate) oldpc: u32,

    /// Transfer info for call/return hooks (like C Lua's L->transferinfo).
    /// ftransfer: 1-based index of first transferred value (relative to func position).
    /// ntransfer: number of values being transferred.
    /// Set before calling hooks, read by debug.getinfo with 'r' option.
    pub(crate) ftransfer: i32,
    pub(crate) ntransfer: i32,

    safe_state: LuaSafeState,

    is_main: bool,

    /// To-be-closed variable list - stack indices of variables marked with `<close>`
    /// Maintained in order: most recently added TBC variable is last
    /// When leaving a block (OpCode::Close), we iterate from the end and call __close
    /// on each TBC variable whose stack index >= the close level
    pub(crate) tbc_list: Vec<usize>,

    /// Whether this coroutine has yielded and is waiting to be resumed.
    /// Used to distinguish "running" from "yielded" when call_stack is non-empty.
    /// Set to true when yield is captured by resume, false when execution resumes.
    yielded: bool,

    /// Whether this coroutine is dead (finished with error).
    /// Unlike normal completion (stack cleared), dead-by-error coroutines
    /// retain their stack and call frames for debug.traceback inspection,
    /// matching C Lua's behavior.
    pub(crate) dead: bool,

    /// Archived error for a dead coroutine.
    /// Active runtime errors live in GlobalState, but dead coroutines must
    /// preserve their own terminal error for coroutine.resume/close without
    /// leaking that error into later unrelated calls.
    dead_error: ErrorMsg,

    /// Whether close_tbc_with_error is currently running on this thread.
    /// Used to detect re-entrant coroutine.close() calls from __close handlers.
    pub(crate) is_closing: bool,

    /// Non-yieldable nesting depth (like C Lua's nny packed in nCcalls).
    /// Main thread starts at 1 (always non-yieldable).
    /// Coroutine threads start at 0 (yieldable).
    /// Incremented when entering a non-yieldable C call boundary (e.g., pcall method
    /// used by C stdlib functions like gsub that don't support continuations).
    /// `yieldable(L)` == `nny == 0`.
    pub(crate) nny: u32,

    /// Canonical opaque wrapper for the optional Lua 5.3 C ABI.
    ///
    /// This is job-local and remains null until the state is exposed through
    /// that ABI. Keeping it on the thread avoids process-global lookup state
    /// when a metamethod enters a C closure from any API operation.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) c_api_wrapper: *mut (),

    /// Pending async future — set by async CFunction wrappers before yielding.
    /// When an async function is called from Lua, it creates the Future and stores
    /// it here, then yields with ASYNC_SENTINEL. The AsyncThread polls this future
    /// and resumes the coroutine when it completes.
    /// `Option<Pin<Box<...>>>` is null-pointer-optimized: zero overhead when None.
    pub(crate) pending_future: Option<AsyncFuture>,

    #[cfg(feature = "sandbox")]
    pub(crate) sandbox_limits: Option<SandboxRuntimeLimits>,
}

impl LuaState {
    /// Create a new execution state
    pub(crate) fn new(
        call_stack_size: usize,
        global_state: GlobalStateHandle,
        is_main: bool,
        safe_option: SafeOption,
    ) -> Self {
        Self {
            global_state,
            stack: Vec::with_capacity(BASIC_STACK_SIZE),
            thread: ThreadPtr::null(),
            stack_top: 0, // Start with empty stack (Lua's L->top.p = L->stack)
            call_stack: Vec::with_capacity(call_stack_size),
            call_stack_storage: Vec::with_capacity(call_stack_size),
            call_depth: 0,
            current_ci: std::ptr::null_mut(),
            open_upvalues_list: Vec::new(),
            in_twups: false,
            yield_values: Vec::new(),
            allow_hook: true,
            hook: LuaValue::nil(),
            hook_mask: 0,
            base_hook_count: 0,
            hook_count: 0,
            oldpc: 0,
            ftransfer: 0,
            ntransfer: 0,
            safe_state: safe_option.into(),
            is_main,
            tbc_list: Vec::new(),
            yielded: false,
            dead: false,
            dead_error: ErrorMsg::None,
            is_closing: false,
            nny: if is_main { 1 } else { 0 },
            c_api_wrapper: std::ptr::null_mut(),
            pending_future: None,
            #[cfg(feature = "sandbox")]
            sandbox_limits: None,
        }
    }

    // please donot use this function directly unless you are very sure of what you are doing
    pub(crate) fn thread_ptr(&self) -> ThreadPtr {
        self.thread
    }

    pub(crate) fn set_thread_ptr(&mut self, thread: ThreadPtr) {
        self.thread = thread;
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn set_c_api_wrapper(&mut self, wrapper: *mut ()) {
        self.c_api_wrapper = wrapper;
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn c_api_wrapper(&self) -> *mut () {
        self.c_api_wrapper
    }

    /// Remove a dead string from the intern map (called by GC during sweep)
    pub(crate) fn remove_dead_string(&mut self, str_ptr: StringPtr) {
        self.global_state_mut().object_allocator.remove_str(str_ptr);
    }

    /// Get current call frame (equivalent to Lua's L->ci)
    #[inline(always)]
    pub fn current_frame(&self) -> Option<&CallInfo> {
        self.call_depth
            .checked_sub(1)
            .and_then(|index| self.call_stack.get(index))
            .map(|ci| &**ci)
    }

    /// Get mutable current call frame
    #[inline(always)]
    pub fn current_frame_mut(&mut self) -> Option<&mut CallInfo> {
        self.call_depth
            .checked_sub(1)
            .and_then(|index| self.call_stack.get(index).copied())
            .map(|ci| unsafe { ci.as_mut() })
    }

    #[inline(always)]
    fn try_acquire_call_info_slot(&mut self) -> Option<*mut CallInfo> {
        if self.call_depth < self.call_stack.len() {
            Some(unsafe { self.call_stack.get_unchecked(self.call_depth).as_ptr() })
        } else {
            None
        }
    }

    /// Returns a reusable `CallInfo` slot, allocating a new stable slot only
    /// when this call depth has never been reached before.
    #[inline(always)]
    fn acquire_call_info_slot(&mut self) -> *mut CallInfo {
        if let Some(ptr) = self.try_acquire_call_info_slot() {
            return ptr;
        }

        let mut value = CallInfo::default();
        // CallInfo slots are indexed by call depth and never move. The
        // previous link for depth `d` is always the slot for depth `d-1`,
        // so it only needs to be initialized when the slot is created.
        value.previous = self.current_ci;
        let pooled = self
            .global_state_mut()
            .object_allocator
            .alloc_call_info(value);
        let ptr = pooled.as_mut_ptr();
        self.call_stack_storage.push(pooled);
        self.call_stack
            .push(CallInfoPtr::from_mut(unsafe { &mut *ptr }));
        ptr
    }

    #[inline]
    pub(crate) fn release_ci(&mut self) {
        self.call_depth = 0;
        self.current_ci = std::ptr::null_mut();
        self.call_stack.clear();
        self.call_stack_storage.clear();
    }

    /// Get call stack depth
    #[inline(always)]
    pub fn call_depth(&self) -> usize {
        self.call_depth
    }

    /// Push a new call frame (equivalent to Lua's luaD_precall)
    /// OPTIMIZED: Reuses CallInfo slots, only allocates when needed
    #[inline]
    pub(crate) fn push_frame(
        &mut self,
        func: &LuaValue,
        base: usize,
        nparams: usize,
        nresults: i32,
    ) -> LuaResult<()> {
        // Fast path: check Lua call-stack depth
        if self.call_depth >= self.safe_state.max_call_depth {
            return Err(self.stack_overflow_error());
        }

        // Cache lua_function extraction (avoid repeated enum matching)
        // This single call replaces multiple is_c_function/as_lua_function checks

        // Determine function type and extract metadata in one pass
        let (call_status, maxstacksize, numparams, nextraargs, chunk_raw, upvalue_raw) =
            if let Some(func_obj) = func.as_lua_function() {
                let chunk = func_obj.chunk();
                // Lua function with chunk
                let numparams = chunk.param_count;
                let nextraargs = if nparams > numparams {
                    (nparams - numparams) as i32
                } else {
                    0
                };
                let chunk_ptr: *const LuaProto = chunk;
                let upvalue_ptr = func_obj.upvalues().as_ptr();
                (
                    call_status::with_nresults(call_status::CIST_LUA, nresults),
                    chunk.max_stack_size,
                    numparams,
                    nextraargs,
                    chunk_ptr,
                    upvalue_ptr,
                )
            } else if func.is_c_callable() {
                // Light C function
                (
                    call_status::with_nresults(CIST_C, nresults),
                    nparams,
                    nparams,
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            } else {
                // Not callable - this should be prevented by caller
                debug_assert!(false, "push_frame called with non-callable value");
                return Err(self.error(format!("attempt to call a {} value", func.type_name())));
            };

        // Fill missing parameters with nil (optimized batch operation)
        if nparams < numparams {
            let start = base + nparams;
            let end = base + numparams;

            // Ensure stack capacity.
            // CRITICAL: Use self.resize() instead of self.stack.resize() so that
            // open upvalue raw pointers are fixed if the Vec reallocates.
            // Direct self.stack.resize() bypasses fix_open_upvalue_pointers(),
            // causing use-after-free when upvalues read/write via stale pointers.
            if self.stack.len() < end {
                self.resize(end)?;
            }
            // Batch fill missing parameter slots with nil
            self.stack[start..end].fill(LuaValue::nil());

            // Update stack_top if necessary
            if self.stack_top < end {
                self.stack_top = end;
            }
        }

        let frame_top = base + maxstacksize;

        // Ensure physical stack has EXTRA_STACK slots above frame_top
        // for metamethod arguments (matching Lua 5.5's EXTRA_STACK guarantee)
        let needed_physical = frame_top + EXTRA_STACK;
        if needed_physical > self.stack.len() {
            self.resize(needed_physical)?;
        }

        let init = if call_status & CIST_C == 0 {
            FrameInit::lua(
                base,
                nresults,
                maxstacksize,
                chunk_raw,
                upvalue_raw,
                nextraargs,
            )
        } else {
            FrameInit::c(base, nparams, nresults)
        };

        // Acquire a stable CallInfo slot (reuse when possible, allocate on first depth).
        let ci = self.acquire_call_info_slot();
        self.init_call_info(ci, init);

        self.current_ci = ci;
        self.call_depth += 1;

        // Match Lua 5.5's luaD_precall: L->top.p = ci->top.p
        // For Lua functions, set stack_top to frame_top so the GC's
        // traverse_thread scans the full frame extent. Without this,
        // stack_top stays at the CALL instruction's ra+b, which can be
        // BELOW caller-frame locals that are still live — causing the GC
        // to miss marking those objects.
        if call_status & CIST_C == 0 && frame_top > self.stack_top {
            self.stack_top = frame_top;
        }

        Ok(())
    }

    /// Push a Lua function call frame (specialized fast path).
    /// Caller MUST already know `func` is a Lua function and provide the chunk metadata.
    /// Skips the function-type dispatch entirely.
    #[inline(always)]
    pub(crate) fn try_push_lua_frame_exact(
        &mut self,
        base: usize,
        nresults: i32,
        max_stack_size: usize,
        chunk_ptr: *const LuaProto,
        upvalue_ptrs: *const UpvaluePtr,
    ) -> LuaResult<bool> {
        if self.call_depth >= self.safe_state.max_call_depth {
            self.push_lua_frame_overflow()?;
            return Ok(false);
        }

        let frame_top = base + max_stack_size;
        if frame_top + EXTRA_STACK > self.stack.len() {
            return Ok(false);
        }

        // Exact fast path intentionally does not allocate a new CallInfo slot;
        // if no reusable slot exists the caller falls back to the general path.
        let Some(ci) = self.try_acquire_call_info_slot() else {
            return Ok(false);
        };
        let init = FrameInit::lua(base, nresults, max_stack_size, chunk_ptr, upvalue_ptrs, 0);
        self.init_call_info(ci, init);

        self.current_ci = ci;
        self.call_depth += 1;
        if self.stack_top < frame_top {
            self.stack_top = frame_top;
        }
        Ok(true)
    }

    /// 极薄 Lua 进帧原语。
    ///
    /// 与 `try_push_lua_frame_exact` 语义一致，但直接返回新帧的 `CallInfo`
    /// 指针和 `base_stk`，避免调用方再走一次 `current_ci_ptr()` / `ci.base_stk`
    /// 往返。仅覆盖物理栈/调用栈已有余量的快路径；任一条件不满足返回
    /// `None`，由调用方走既有慢路径。
    ///
    /// Caller guarantees: 目标 chunk 的 `param_count` 与实参数量一致。
    #[inline(always)]
    pub(crate) fn enter_lua_frame_fast(
        &mut self,
        base: usize,
        nresults: i32,
        frame_top: usize,
        chunk_ptr: *const LuaProto,
        upvalue_ptrs: *const UpvaluePtr,
    ) -> Option<(*mut CallInfo, StkId)> {
        if self.call_depth >= self.safe_state.max_call_depth {
            return None;
        }
        if frame_top + EXTRA_STACK > self.stack.len() {
            return None;
        }
        let ci = self.try_acquire_call_info_slot()?;
        let base_stk = StkId::from_stack(self.stack.as_mut_ptr(), base);
        unsafe {
            let ci_ref = &mut *ci;
            ci_ref.base = base;
            ci_ref.base_stk = base_stk;
            ci_ref.func_offset = 1;
            ci_ref.top = frame_top as u32;
            ci_ref.pc = 0;
            ci_ref.call_status = call_status::with_nresults(call_status::CIST_LUA, nresults);
            // 精确实参路径 nextraargs 必为 0；仅 vararg chunk 会读该字段。
            if !chunk_ptr.is_null() && (*chunk_ptr).is_vararg {
                ci_ref.nextraargs = 0;
            }
            ci_ref.chunk_ptr = chunk_ptr;
            ci_ref.upvalue_ptrs = upvalue_ptrs;
        }
        self.current_ci = ci;
        self.call_depth += 1;
        if self.stack_top < frame_top {
            self.stack_top = frame_top;
        }
        Some((ci, base_stk))
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_lua_frame(
        &mut self,
        base: usize,
        nparams: usize,
        nresults: i32,
        param_count: usize,
        max_stack_size: usize,
        chunk_ptr: *const LuaProto,
        upvalue_ptrs: *const UpvaluePtr,
    ) -> LuaResult<()> {
        // Check stack depth (cold — almost never triggers)
        if self.call_depth >= self.safe_state.max_call_depth {
            return self.push_lua_frame_overflow();
        }

        // Pre-compute common values
        let frame_top = base + max_stack_size;

        // Fast path for the common case: enough params (no nil filling needed),
        // stack already large enough, call_stack slot available for reuse.
        // Covers exact match AND extra args (common in metamethods like __len
        // which receives 2 args but declares 1 param).
        if nparams >= param_count
            && frame_top + EXTRA_STACK <= self.stack.len()
            && let Some(ci) = self.try_acquire_call_info_slot()
        {
            let init = FrameInit::lua(
                base,
                nresults,
                max_stack_size,
                chunk_ptr,
                upvalue_ptrs,
                (nparams - param_count) as i32,
            );
            self.init_call_info(ci, init);

            self.current_ci = ci;
            self.call_depth += 1;

            if self.stack_top < frame_top {
                self.stack_top = frame_top;
            }

            return Ok(());
        }

        // Slow path: handle extra args, nil filling, stack resize, new slot allocation
        self.push_lua_frame_slow(
            base,
            nparams,
            nresults,
            param_count,
            max_stack_size,
            frame_top,
            chunk_ptr,
            upvalue_ptrs,
        )
    }

    /// Stack overflow error for push_lua_frame (cold path)
    #[cold]
    #[inline(never)]
    fn push_lua_frame_overflow(&mut self) -> LuaResult<()> {
        Err(self.stack_overflow_error())
    }

    /// Slow path for push_lua_frame — handles nil filling, resize, new slot allocation
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn push_lua_frame_slow(
        &mut self,
        base: usize,
        nparams: usize,
        nresults: i32,
        param_count: usize,
        max_stack_size: usize,
        frame_top: usize,
        chunk_ptr: *const LuaProto,
        upvalue_ptrs: *const UpvaluePtr,
    ) -> LuaResult<()> {
        let nextraargs = if nparams > param_count {
            (nparams - param_count) as i32
        } else {
            0
        };

        // Fill missing parameters with nil
        if nparams < param_count {
            let start = base + nparams;
            let end = base + param_count;
            // CRITICAL: Use self.resize() instead of self.stack.resize() so that
            // open upvalue raw pointers are fixed if the Vec reallocates.
            if self.stack.len() < end {
                self.resize(end)?;
            }
            // Batch fill missing parameter slots with nil
            self.stack[start..end].fill(LuaValue::nil());
            if self.stack_top < end {
                self.stack_top = end;
            }
        }

        let needed_physical = frame_top + EXTRA_STACK;
        if needed_physical > self.stack.len() {
            self.resize(needed_physical)?;
        }

        // Reuse an existing CallInfo slot or allocate a new stable one.
        let init = FrameInit::lua(
            base,
            nresults,
            max_stack_size,
            chunk_ptr,
            upvalue_ptrs,
            nextraargs,
        );
        let ci = self.acquire_call_info_slot();
        self.init_call_info(ci, init);

        self.current_ci = ci;
        self.call_depth += 1;

        if self.stack_top < frame_top {
            self.stack_top = frame_top;
        }

        Ok(())
    }

    /// Push a C function call frame (specialized fast path).
    /// Caller MUST already know `func` is a C function / cclosure.
    /// Skips the function-type dispatch entirely; mirrors `push_lua_frame`.
    #[inline(always)]
    pub(crate) fn push_c_frame(
        &mut self,
        base: usize,
        nargs: usize,
        nresults: i32,
    ) -> LuaResult<()> {
        // Check Lua call-stack depth
        if self.call_depth >= self.safe_state.max_call_depth {
            return Err(self.stack_overflow_error());
        }

        // For C functions: maxstacksize = nargs, numparams = nargs (no nil filling needed)
        let frame_top = base + nargs;

        // Ensure physical stack has EXTRA_STACK slots above frame_top
        let needed_physical = frame_top + EXTRA_STACK;
        if needed_physical > self.stack.len() {
            self.resize(needed_physical)?;
        }

        // Reuse an existing CallInfo slot or allocate a new stable one.
        let init = FrameInit::c(base, nargs, nresults);
        let ci = self.acquire_call_info_slot();
        self.init_call_info(ci, init);

        self.current_ci = ci;
        self.call_depth += 1;

        if self.stack_top < frame_top {
            self.stack_top = frame_top;
        }

        Ok(())
    }

    /// Pop call frame (equivalent to Lua's luaD_poscall)
    #[inline(always)]
    pub(crate) fn pop_frame(&mut self) {
        if self.call_depth > 0 {
            let previous = unsafe { (*self.current_ci).previous };
            self.call_depth -= 1;
            self.current_ci = previous;
        }
    }

    /// Pop a C call frame (specialized fast path, skips call_status bit check).
    /// Caller MUST know the current frame is a C frame.
    #[inline(always)]
    pub(crate) fn pop_c_frame(&mut self) {
        debug_assert!(self.call_depth > 0);
        let previous = unsafe { (*self.current_ci).previous };
        self.call_depth -= 1;
        self.current_ci = previous;
    }

    /// Get logical stack top (L->top.p in Lua source)
    /// This is the first free slot in the stack, NOT the length of physical stack
    #[inline(always)]
    pub fn get_top(&self) -> usize {
        self.stack_top
    }

    /// Port of lua_checkstack (lapi.c): check if the stack can grow by `n` slots.
    /// Returns true if the stack can accommodate `n` more elements.
    pub fn check_stack(&self, n: usize) -> bool {
        self.stack_top + n <= self.safe_state.max_stack_size
    }

    /// Ensure the physical stack has room for `additional` more values beyond stack_top.
    /// Checks both the logical limit (max_stack_size) and physical capacity (Vec length).
    /// After this call, `push_value_unchecked` can be safely used `additional` times.
    #[inline]
    pub fn ensure_stack_capacity(&mut self, additional: usize) -> LuaResult<()> {
        let needed = self.stack_top + additional;
        if needed > self.safe_state.max_stack_size {
            return Err(LuaError::StackOverflow);
        }
        if needed > self.stack.len() {
            self.resize(needed)?;
        }
        Ok(())
    }

    /// Get a raw slice view of current frame arguments on the stack.
    /// Returns args[0..n] where arg(1) = slice\[0\], arg(2) = slice\[1\], etc.
    #[inline]
    pub fn arg_slice(&self) -> &[LuaValue] {
        if self.call_depth == 0 {
            return &[];
        }
        let frame = &self.call_stack[self.call_depth - 1];
        let end = (frame.top as usize).min(self.stack.len());
        &self.stack[frame.base..end]
    }

    /// Set logical stack top (L->top.p = L->stack + new_top in Lua)
    /// This is an internal VM operation — just moves the pointer.
    /// GC safety for stale slots above top is handled by the GC atomic phase
    /// (traverse_thread clears dead stack slices), matching Lua 5.5's design.
    #[inline(always)]
    pub fn set_top(&mut self, new_top: usize) -> LuaResult<()> {
        // Ensure physical stack is large enough
        if new_top > self.stack.len() {
            self.resize(new_top)?;
        }
        self.stack_top = new_top;

        Ok(())
    }

    /// Set logical stack top without any checks (fastest path).
    /// Caller must ensure physical stack is already large enough.
    /// Equivalent to Lua 5.5's `L->top.p = L->stack + new_top`.
    #[inline(always)]
    pub fn set_top_raw(&mut self, new_top: usize) {
        self.stack_top = new_top;
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn park_stack_for_c_api(&mut self, values: Vec<LuaValue>) -> CApiStackParking {
        let stack = std::mem::replace(&mut self.stack, values);
        let stack_top = std::mem::replace(&mut self.stack_top, self.stack.len());
        let exposed_top = self.stack_top as u32;
        let mut frames = Vec::with_capacity(self.call_depth);
        for index in 0..self.call_depth {
            let frame = self.get_call_info_mut(index);
            frames.push((frame.base, frame.top, frame.func_offset));
            frame.base = 0;
            frame.top = exposed_top;
            frame.func_offset = 0;
        }
        self.fix_call_info_base_stk();
        CApiStackParking {
            stack,
            stack_top,
            frames,
        }
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn restore_stack_from_c_api(&mut self, parking: CApiStackParking) -> Vec<LuaValue> {
        let external_top = self.stack_top.min(self.stack.len());
        let external = self.stack[..external_top].to_vec();
        self.stack = parking.stack;
        self.stack_top = parking.stack_top;
        for (index, (base, top, func_offset)) in parking.frames.into_iter().enumerate() {
            if index >= self.call_depth {
                break;
            }
            let frame = self.get_call_info_mut(index);
            frame.base = base;
            frame.top = top;
            frame.func_offset = func_offset;
        }
        self.fix_call_info_base_stk();
        external
    }

    /// Get stack value at absolute index
    #[inline(always)]
    pub fn stack_get(&self, index: usize) -> Option<LuaValue> {
        self.stack.get(index).copied()
    }

    /// Set stack value at absolute index
    #[inline(always)]
    pub fn stack_set(&mut self, index: usize, value: LuaValue) -> LuaResult<()> {
        if index >= self.safe_state.max_stack_size {
            return Err(self.stack_overflow_error());
        }
        if index >= self.stack.len() {
            self.resize(index + 1)?;
        }
        self.stack[index] = value;
        Ok(())
    }

    /// Lua stack exhausted (C Lua's `luaD_growstack`; the call-depth guard counts as stack):
    /// the first overflow raises "stack overflow" and opens an error zone of
    /// `STACK_ERROR_ZONE` slots and frames so the error can be handled; overflowing the
    /// zone is "error in error handling" (`LUA_ERRERR`).
    #[cold]
    #[inline(never)]
    fn stack_overflow_error(&mut self) -> LuaError {
        if self.safe_state.max_stack_size > self.safe_state.base_stack_size {
            return LuaError::ErrorInErrorHandling;
        }
        self.safe_state.max_stack_size = self.safe_state.base_stack_size + STACK_ERROR_ZONE;
        self.safe_state.max_call_depth = self.safe_state.base_call_depth + STACK_ERROR_ZONE;
        self.error("stack overflow".to_string());
        LuaError::StackOverflow
    }

    /// The error-zone part of `luaD_shrinkstack`: leave the zone once the stack in use
    /// (`top`, the current frame's top and the frames) fits the limits again.
    pub(crate) fn shrink_stack_error_zone(&mut self, top: usize) {
        if self.safe_state.max_stack_size > self.safe_state.base_stack_size {
            let frame_top = self.current_frame().map_or(0, |ci| ci.top as usize);
            if top.max(frame_top) + EXTRA_STACK <= self.safe_state.base_stack_size
                && self.call_depth < self.safe_state.base_call_depth
            {
                self.safe_state.max_stack_size = self.safe_state.base_stack_size;
                self.safe_state.max_call_depth = self.safe_state.base_call_depth;
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn resize(&mut self, new_size: usize) -> LuaResult<()> {
        if new_size > self.safe_state.max_stack_size {
            return Err(self.stack_overflow_error());
        }
        let capacity = self.stack.capacity();
        self.stack.resize(new_size, LuaValue::nil());
        if self.stack.capacity() > capacity {
            // If the vector had to reallocate, we need to update all cached stack pointers
            self.fix_open_upvalue_pointers();
            self.fix_call_info_base_stk();
        }

        Ok(())
    }

    /// Fix all open upvalue cached pointers after a Vec reallocation.
    /// Must be called whenever the stack Vec's internal buffer moves
    /// (e.g., after Vec::push triggers a reallocation).
    pub fn fix_open_upvalue_pointers(&mut self) {
        let sp = self.stack.as_mut_ptr();
        for upval_ptr in &self.open_upvalues_list {
            let data = &mut upval_ptr.as_mut_ref().data;
            // All entries in open_upvalues_list must be open
            debug_assert!(data.is_open());
            let stack_index = data.get_stack_index();
            if stack_index < self.stack.len() {
                data.update_stack_ptr(StkId::from_mut_ptr(unsafe { sp.add(stack_index) }));
            }
        }
    }

    #[inline(always)]
    fn ci_base_stk(&self, base: usize) -> StkId {
        StkId::from_stack(self.stack.as_ptr() as *mut LuaValue, base)
    }

    /// Single place that initializes a `CallInfo` from a `FrameInit`.
    ///
    /// This is the Phase 1 convergence point: every push variant should build
    /// a `FrameInit` and call this instead of writing `CallInfo` literals.
    #[inline(always)]
    fn init_call_info(&self, ci: *mut CallInfo, init: FrameInit) {
        let base_stk = self.ci_base_stk(init.base);
        unsafe {
            let ci_ref = &mut *ci;
            ci_ref.base = init.base;
            ci_ref.base_stk = base_stk;
            ci_ref.func_offset = 1;
            ci_ref.top = init.frame_top as u32;
            ci_ref.pc = 0;
            ci_ref.call_status = init.call_status;
            if init.write_nextraargs {
                ci_ref.nextraargs = init.nextraargs;
            }
            ci_ref.chunk_ptr = init.chunk_ptr;
            ci_ref.upvalue_ptrs = init.upvalue_ptrs;
        }
    }

    pub(crate) fn fix_call_info_base_stk(&mut self) {
        let sp = self.stack.as_mut_ptr();
        for ci_ptr in self.call_stack.iter() {
            let ci = unsafe { &mut *ci_ptr.as_ptr() };
            ci.base_stk = StkId::from_stack(sp, ci.base);
        }
    }

    pub(crate) fn offset_of_stk_id(&self, stk_id: StkId) -> i32 {
        let sp = self.stack.as_ptr();
        let offset = (stk_id.as_ptr() as usize).wrapping_sub(sp as usize);
        (offset / std::mem::size_of::<LuaValue>()) as i32
    }

    /// Get register relative to current frame base
    #[inline(always)]
    pub fn reg_get(&self, reg: u8) -> Option<LuaValue> {
        if let Some(frame) = self.current_frame() {
            self.stack_get(frame.base + reg as usize)
        } else {
            None
        }
    }

    /// Set register relative to current frame base
    #[inline(always)]
    pub fn reg_set(&mut self, reg: u8, value: LuaValue) -> LuaResult<()> {
        if let Some(frame) = self.current_frame() {
            let index = frame.base + reg as usize;
            self.stack_set(index, value)?;
        }

        Ok(())
    }

    /// Get open upvalues list
    #[inline(always)]
    pub fn open_upvalues(&self) -> &[UpvaluePtr] {
        &self.open_upvalues_list
    }

    /// Get mutable open upvalues list
    #[inline(always)]
    pub fn open_upvalues_mut(&mut self) -> &mut Vec<UpvaluePtr> {
        &mut self.open_upvalues_list
    }

    /// Set a runtime error message.
    ///
    /// Mirrors Lua 5.5's luaG_runerror boundary: VM/runtime errors raised from
    /// an active Lua frame carry source information immediately, while errors
    /// raised from C/host frames keep their raw message.
    #[cold]
    #[inline(never)]
    pub fn error(&mut self, msg: String) -> LuaError {
        let msg = self.add_runtime_error_info(msg);
        self.global_state_mut().error(msg)
    }

    /// `error` for a message of bytes: a name quoted in it (a local variable
    /// of Lua 5.3 may hold any byte >= 0x80) need not be valid UTF-8. Such a
    /// message is raised as a string error object, with the position that
    /// `error` would add.
    #[cold]
    #[inline(never)]
    pub fn error_bytes(&mut self, msg: Vec<u8>) -> LuaError {
        let msg = match String::from_utf8(msg) {
            Ok(msg) => return self.error(msg),
            Err(error) => error.into_bytes(),
        };
        let location = self.add_runtime_error_info(String::new());
        let mut full = Vec::with_capacity(location.len() + msg.len());
        if !msg.starts_with(location.as_bytes()) {
            full.extend_from_slice(location.as_bytes());
        }
        full.extend_from_slice(&msg);
        match self.create_bytes(&full) {
            Ok(value) => self.error_with_object(value),
            Err(error) => error,
        }
    }

    #[inline(always)]
    fn add_runtime_error_info(&self, msg: String) -> String {
        let Some(ci) = self.current_frame() else {
            return msg;
        };

        let Some(location) = self.frame_error_location(ci) else {
            return msg;
        };

        if msg.starts_with(&location) {
            msg
        } else {
            format!("{}{}", location, msg)
        }
    }

    #[inline(always)]
    fn frame_error_location(&self, ci: &CallInfo) -> Option<String> {
        if !ci.is_lua() || ci.chunk_ptr.is_null() {
            return None;
        }

        let chunk = unsafe { &*ci.chunk_ptr };
        let source = match chunk.source_name.as_deref() {
            Some(raw) => format_source(raw),
            None => "?".to_string(),
        };
        let line = if ci.pc > 0 && (ci.pc as usize - 1) < chunk.line_info.len() {
            chunk.line_info[ci.pc as usize - 1] as usize
        } else {
            0
        };

        Some(if line > 0 {
            format!("{}:{}: ", source, line)
        } else if self.global_state().language() == crate::LuaLanguageLevel::Lua53 {
            format!("{}:-1: ", source)
        } else if chunk.line_info.is_empty() {
            format!("{}:?: ", source)
        } else {
            format!("{}: ", source)
        })
    }

    /// Set the current error object for protected calls to retrieve.
    #[cold]
    #[inline(never)]
    pub fn error_with_object(&mut self, obj: LuaValue) -> LuaError {
        self.global_state_mut().error_with_object(obj)
    }

    #[inline(always)]
    pub fn error_object(&self) -> LuaValue {
        self.global_state()
            .get_error_object_ref()
            .copied()
            .unwrap_or(LuaValue::nil())
    }

    #[inline(always)]
    pub fn has_error_object(&self) -> bool {
        self.global_state().get_error_object_ref().is_some()
    }

    #[inline(always)]
    pub fn set_error_object(&mut self, obj: LuaValue) {
        let _ = self.global_state_mut().error_with_object(obj);
    }

    #[inline(always)]
    pub fn take_error_object(&mut self) -> LuaValue {
        match self.global_state_mut().take_error() {
            ErrorMsg::Object(obj) => obj,
            ErrorMsg::None => LuaValue::nil(),
            other => {
                self.global_state_mut().error_msg = other;
                LuaValue::nil()
            }
        }
    }

    /// The value a protected call catches for the error `e` (C Lua's
    /// `L->top - 1` after the throw): the error object, else the message
    /// (nil when the error carries neither).
    pub(crate) fn caught_error_value(&mut self, e: LuaError) -> LuaResult<LuaValue> {
        if self.has_error_object() {
            return Ok(self.take_error_object());
        }
        let msg = self.get_error_message(e);
        self.clear_error();
        if msg.is_empty() {
            return Ok(LuaValue::nil());
        }
        self.create_string(&msg)
    }

    /// Tail of Lua 5.5's `luaG_errormsg`: once any message handler has run,
    /// a nil error object becomes "<no error object>". Lua 5.3 keeps nil.
    pub(crate) fn errormsg_object(&mut self, err: LuaValue) -> LuaResult<LuaValue> {
        if err.is_nil() && self.global_state().language() == crate::LuaLanguageLevel::Lua55 {
            return self.create_string("<no error object>");
        }
        Ok(err)
    }

    /// Run an xpcall message handler on `err` while the frames that raised
    /// it are still on the call stack, as `luaG_errormsg` does before it
    /// unwinds. An error inside the handler calls the handler again on the
    /// new error. Returns the handler's result after `errormsg_object`, or
    /// `None` for "error in error handling" (`LUA_ERRERR`).
    pub(crate) fn call_message_handler(
        &mut self,
        handler: LuaValue,
        err: LuaValue,
    ) -> LuaResult<Option<LuaValue>> {
        // C Lua lets the handler run in an extra zone above the C stack limit
        // (CSTACKERR), so it can run after a C stack overflow; a Lua stack overflow
        // opens its own zone (`stack_overflow_error`).
        let saved_max_c_depth = self.safe_state.max_c_stack_depth;
        self.safe_state.max_c_stack_depth = saved_max_c_depth + CSTACKERR;
        self.nny += 1;
        let result = self.message_handler_loop(handler, err);
        self.nny -= 1;
        self.safe_state.max_c_stack_depth = saved_max_c_depth;
        match result? {
            Some(value) => self.errormsg_object(value).map(Some),
            None => Ok(None),
        }
    }

    fn message_handler_loop(
        &mut self,
        handler: LuaValue,
        mut err: LuaValue,
    ) -> LuaResult<Option<LuaValue>> {
        // Each handler error recurses through luaG_errormsg in C Lua, adding a
        // C call: at LUAI_MAXCSTACK the handler gets "C stack overflow", and
        // CSTACKERR calls later error handling fails. The loop does not
        // recurse, so count the retries against the same budget.
        let depth_budget = LUAI_MAXCSTACK.saturating_sub(self.global_state().n_ccalls);
        let hard_limit = depth_budget + CSTACKERR;
        for retry_count in 1..=hard_limit {
            if retry_count > depth_budget {
                err = self.create_string("C stack overflow")?;
            }
            let handler_func_idx = self.stack_top;
            self.push_value(handler)?;
            self.push_value(err)?;
            let handler_depth = self.call_depth();
            let result = if handler.is_c_callable() {
                call_c_function(self, handler_func_idx, 1, -1).map(|_| ())
            } else {
                match self.push_frame(&handler, handler_func_idx + 1, 1, -1) {
                    Ok(()) => match self.inc_n_ccalls() {
                        Ok(()) => {
                            let r = lua_execute(self, handler_depth);
                            self.dec_n_ccalls();
                            r
                        }
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error),
                }
            };
            let caught = match result {
                Ok(()) => {
                    let value = self.stack_get(handler_func_idx).unwrap_or_default();
                    self.set_top_raw(handler_func_idx);
                    return Ok(Some(value));
                }
                Err(LuaError::ErrorInErrorHandling) => {
                    self.clear_error();
                    None
                }
                Err(error) => Some(self.caught_error_value(error)?),
            };
            while self.call_depth() > handler_depth {
                self.pop_frame();
            }
            self.set_top_raw(handler_func_idx);
            match caught {
                Some(new_err) => err = new_err,
                None => return Ok(None),
            }
        }
        Ok(None)
    }

    #[inline(always)]
    pub(crate) fn take_error_msg_raw(&mut self) -> String {
        match self.global_state_mut().take_error() {
            ErrorMsg::Msg(msg) | ErrorMsg::Traced { message: msg, .. } => msg,
            ErrorMsg::Object(obj) => {
                let _ = self.global_state_mut().error_with_object(obj);
                String::new()
            }
            ErrorMsg::None => String::new(),
        }
    }

    #[inline(always)]
    pub(crate) fn clear_error(&mut self) {
        let _ = self.global_state_mut().take_error();
    }

    #[inline(always)]
    pub fn archive_dead_error(&mut self, err: ErrorMsg) {
        self.dead_error = err;
    }

    #[inline(always)]
    pub fn dead_error(&self) -> &ErrorMsg {
        &self.dead_error
    }

    #[inline(always)]
    pub fn take_dead_error(&mut self) -> ErrorMsg {
        std::mem::take(&mut self.dead_error)
    }

    #[inline(always)]
    pub fn clear_dead_error(&mut self) {
        self.dead_error = ErrorMsg::None;
    }

    /// Set yield values
    #[inline(always)]
    pub fn set_yield(&mut self, values: Vec<LuaValue>) {
        self.yield_values = values;
    }

    /// Take yield values
    #[inline(always)]
    pub fn take_yield(&mut self) -> Vec<LuaValue> {
        std::mem::take(&mut self.yield_values)
    }

    /// Store a pending async future (called by async function wrappers before yielding)
    #[inline(always)]
    pub fn set_pending_future(&mut self, future: AsyncFuture) {
        self.pending_future = Some(future);
    }

    /// Take the pending async future (called by AsyncThread after detecting async yield)
    #[inline(always)]
    pub fn take_pending_future(&mut self) -> Option<AsyncFuture> {
        self.pending_future.take()
    }

    /// Close upvalues from a given stack index upwards
    /// This is called when exiting a function or block scope
    /// Close upvalues from a given stack index upwards
    /// This is called when exiting a function or block scope
    pub fn close_upvalues(&mut self, level: usize) {
        // Optimization: The list is sorted by stack index descending (higher indices first).
        // Upvalues to close (index >= level) are at the beginning of the list.
        // We scan to find the cutoff point.
        let mut count = 0;
        let len = self.open_upvalues_list.len();

        while count < len {
            let upval_ptr = self.open_upvalues_list[count];
            let data = &upval_ptr.as_ref().data;
            // All entries in open_upvalues_list should be open
            if !data.is_open() || data.get_stack_index() < level {
                break;
            }
            count += 1;
        }

        if count > 0 {
            // Process upvalues to close in-place, then drain.
            for i in 0..count {
                let upval_ptr = self.open_upvalues_list[i];
                let data = &upval_ptr.as_ref().data;
                if data.is_open() {
                    let stack_idx = data.get_stack_index();

                    // Capture value from stack
                    let value = self
                        .stack
                        .get(stack_idx)
                        .copied()
                        .unwrap_or(LuaValue::nil());

                    // Close the upvalue (move value to heap)
                    upval_ptr.as_mut_ref().data.close(value);
                    let gc_ptr = GcObjectPtr::from(upval_ptr);

                    if let Some(header) = gc_ptr.header_mut()
                        && !header.is_white()
                    {
                        header.make_black();
                        if let Some(value_gc_ptr) = value.as_gc_ptr() {
                            self.gc_barrier(upval_ptr, value_gc_ptr);
                        }
                    }
                }
            }
            // Batch remove from front of list
            self.open_upvalues_list.drain(0..count);
        }
    }

    /// Get the name of a local variable at the given stack index
    /// by looking at the current frame's locvars debug info
    fn get_local_var_name(&self, stack_index: usize) -> Option<Box<[u8]>> {
        let ci = self.current_frame()?;
        if !ci.is_lua() {
            return None;
        }
        if ci.chunk_ptr.is_null() {
            return None;
        }
        let chunk = unsafe { &*ci.chunk_ptr };
        let reg = stack_index.checked_sub(ci.base)?;
        // Use ci.pc (next instruction) as the PC for lookup,
        // because at TBC instruction, the variable's startpc equals the TBC PC
        // and ci.pc has already been incremented past it
        let pc = ci.pc as usize;
        // Walk locvars to find which variable occupies register 'reg' at 'pc'
        let mut n = 0usize;
        for locvar in &chunk.locals {
            if (locvar.startpc as usize) > pc {
                break;
            }
            if pc < locvar.endpc as usize {
                if n == reg {
                    return Some(locvar.name.clone());
                }
                n += 1;
            }
        }
        None
    }

    /// Mark a stack slot as to-be-closed (TBC)
    /// Called by OpCode::Tbc
    /// If the value is nil or false, it doesn't need to be closed
    /// Otherwise, it must have a __close metamethod
    pub fn mark_tbc(&mut self, stack_index: usize) -> LuaResult<()> {
        let value = self
            .stack
            .get(stack_index)
            .copied()
            .unwrap_or(LuaValue::nil());

        // nil and false don't need to be closed
        if value.is_falsy() {
            return Ok(());
        }

        // Check that the value has a __close metamethod (or trait-based close)
        use crate::lua_vm::execute::TmKind;
        use crate::lua_vm::execute::get_metamethod_event;
        let has_metatable_close = get_metamethod_event(self, &value, TmKind::Close).is_some();
        let has_trait_close = value.ttisfulluserdata();
        let has_close = has_metatable_close || has_trait_close;

        if !has_close {
            // Try to get the variable name from locvars
            return Err(match self.get_local_var_name(stack_index) {
                Some(name) => {
                    let mut msg = b"variable '".to_vec();
                    msg.extend_from_slice(&name);
                    msg.extend_from_slice(b"' got a non-closable value");
                    self.error_bytes(msg)
                }
                None => self.error("variable got a non-closable value".to_string()),
            });
        }

        self.tbc_list.push(stack_index);
        Ok(())
    }

    /// Close the to-be-closed variables at or above `level` on a normal block
    /// exit (`luaF_close` with `CLOSEKTOP`): `__close(obj)` in reverse order.
    /// The calls are unprotected, as in C Lua: an error leaves the close
    /// method's frames in place for the message handler and propagates at
    /// once; the variables still pending are closed by the protected call
    /// that catches it. A yield propagates too: the entry is already popped,
    /// so a later call continues with the remaining ones.
    pub fn close_tbc(&mut self, level: usize) -> LuaResult<()> {
        while let Some(&tbc_idx) = self.tbc_list.last() {
            if tbc_idx < level {
                break;
            }
            self.tbc_list.pop();

            let value = self.stack.get(tbc_idx).copied().unwrap_or(LuaValue::nil());

            // Skip nil/false (shouldn't be in the list, but be safe)
            if value.is_falsy() {
                continue;
            }


            match get_metamethod_event(self, &value, TmKind::Close) {
                Some(close_fn) => self.call_close_method(&close_fn, &value, None)?,
                // A userdata without a `__close` metamethod uses its trait-based close.
                None if value.ttisfulluserdata() => self.close_userdata_trait(tbc_idx),
                None => return Err(self.non_closable_error(tbc_idx)),
            }
        }
        Ok(())
    }

    /// `UserDataTrait::lua_close` of the full userdata in stack slot `tbc_idx`.
    fn close_userdata_trait(&mut self, tbc_idx: usize) {
        if let Some(ud_mut) = self.stack_mut().get_mut(tbc_idx).and_then(|v| v.as_userdata_mut()) {
            ud_mut.get_trait_mut().lua_close();
        }
    }

    /// A to-be-closed variable whose `__close` metamethod was removed.
    fn non_closable_error(&mut self, tbc_idx: usize) -> LuaError {
        let msg = match self.get_local_var_name(tbc_idx) {
            Some(name) => {
                let mut msg = b"attempt to close non-closable variable '".to_vec();
                msg.extend_from_slice(&name);
                msg.extend_from_slice(b"' (no metamethod 'close')");
                msg
            }
            None => b"attempt to close variable (no metamethod 'close')".to_vec(),
        };
        match self.create_bytes(&msg) {
            Ok(object) => self.error_with_object(object),
            Err(error) => error,
        }
    }

    /// Close all upvalues AND to-be-closed variables down to the given level
    /// This is the main "close" operation used by OpCode::Close and return handlers
    pub fn close_all(&mut self, level: usize) -> LuaResult<()> {
        // First close upvalues (captures values from stack)
        self.close_upvalues(level);
        // Then call __close on TBC variables (in reverse order)
        self.close_tbc(level)
    }

    /// Close the to-be-closed variables at or above `level` while unwinding
    /// an error (`luaD_closeprotected` without a message handler): each
    /// `__close(obj, err)` runs protected, and an error it raises becomes
    /// the error passed on. Returns `Err(RuntimeError)` with the final error
    /// object set if any close failed; a yield propagates.
    pub fn close_tbc_with_error(&mut self, level: usize, err: LuaValue) -> LuaResult<()> {
        self.close_tbc_with_handler(level, err, None)
    }

    /// `close_tbc_with_error` inside an xpcall: an error raised by a close
    /// method goes through the message `handler` while the method's frames
    /// are still on the stack (C Lua keeps `L->errfunc` set while closing).
    pub(crate) fn close_tbc_with_handler(
        &mut self,
        level: usize,
        err: LuaValue,
        handler: Option<LuaValue>,
    ) -> LuaResult<()> {
        let was_closing = self.is_closing;
        self.is_closing = true;
        let result = self.close_tbc_with_handler_inner(level, err, handler);
        self.is_closing = was_closing;
        result
    }

    fn close_tbc_with_handler_inner(
        &mut self,
        level: usize,
        err: LuaValue,
        handler: Option<LuaValue>,
    ) -> LuaResult<()> {
        let mut current_error = err;
        let mut had_close_error = false;

        while let Some(&tbc_idx) = self.tbc_list.last() {
            if tbc_idx < level {
                break;
            }
            self.tbc_list.pop();

            let value = self.stack.get(tbc_idx).copied().unwrap_or(LuaValue::nil());

            if value.is_falsy() {
                continue;
            }


            let caller_depth = self.call_depth();
            let result = match get_metamethod_event(self, &value, TmKind::Close) {
                Some(close_fn) => {
                    // C Lua's prepcallclosemth: the error object goes right
                    // after the variable (rooting it for the GC) and the call
                    // is placed above it.
                    let err_slot = tbc_idx + 1;
                    if err_slot + 1 > self.stack.len() {
                        self.grow_stack(err_slot + 4)?;
                    }
                    self.stack[err_slot] = current_error;
                    self.set_top_raw(err_slot + 1);
                    self.call_close_method(&close_fn, &value, Some(current_error))
                }
                None if value.ttisfulluserdata() => {
                    self.close_userdata_trait(tbc_idx);
                    Ok(())
                }
                None => Err(self.non_closable_error(tbc_idx)),
            };

            match result {
                Ok(()) => {}
                Err(LuaError::Yield) => {
                    if had_close_error {
                        self.set_error_object(current_error);
                    }
                    return Err(LuaError::Yield);
                }
                Err(error) => {
                    had_close_error = true;
                    let raised = self.caught_error_value(error)?;
                    current_error = match handler {
                        Some(handler) => match self.call_message_handler(handler, raised)? {
                            Some(transformed) => transformed,
                            None => self.create_string("error in error handling")?,
                        },
                        None => self.errormsg_object(raised)?,
                    };
                    while self.call_depth() > caller_depth {
                        self.pop_frame();
                    }
                }
            }
        }

        if had_close_error {
            self.set_error_object(current_error);
            return Err(LuaError::RuntimeError);
        }
        Ok(())
    }

    /// `callclosemethod`: call `close_fn(obj)`, or `close_fn(obj, err)` while
    /// unwinding an error, at the current top. Frames are left in place on
    /// error and yield; the caller unwinds them.
    fn call_close_method(
        &mut self,
        close_fn: &LuaValue,
        obj: &LuaValue,
        err: Option<LuaValue>,
    ) -> LuaResult<()> {
        use crate::lua_vm::execute::{call, lua_execute};

        let caller_depth = self.call_depth();
        // Use the current top directly: during error unwinding frames have
        // already been popped and ci->top may lie below the variables.
        let func_pos = self.get_top();
        let nargs = if err.is_some() { 2 } else { 1 };
        if func_pos + 1 + nargs > self.stack.len() {
            self.grow_stack(func_pos + 1 + nargs)?;
        }
        {
            let stack = self.stack_mut();
            stack[func_pos] = *close_fn;
            stack[func_pos + 1] = *obj;
            if let Some(err) = err {
                stack[func_pos + 2] = err;
            }
        }
        self.set_top_raw(func_pos + 1 + nargs);

        if close_fn.is_c_callable() {
            call::call_c_function(self, func_pos, nargs, 0)
        } else if close_fn.is_lua_function() {
            self.push_frame(close_fn, func_pos + 1, nargs, 0)?;
            self.inc_n_ccalls()?;
            let r = lua_execute(self, caller_depth);
            self.dec_n_ccalls();
            r
        } else {
            let type_name = close_fn.type_name();
            Err(self.error(format!(
                "attempt to call a {} value (metamethod 'close')",
                type_name
            )))
        }
    }

    /// Find or create an open upvalue for the given stack index.
    /// Uses linear scan on sorted Vec — faster than HashMap for typical 0-5 open upvalues.
    pub fn find_or_create_upvalue(&mut self, stack_index: usize) -> LuaResult<UpvaluePtr> {
        // Single scan on sorted list (descending by stack index).
        // Finds existing upvalue or determines insert position in one pass.
        let mut insert_pos = self.open_upvalues_list.len();
        for (i, &upval_ptr) in self.open_upvalues_list.iter().enumerate() {
            let idx = upval_ptr.as_ref().data.get_stack_index();
            if idx == stack_index {
                return Ok(upval_ptr);
            }
            if idx < stack_index {
                insert_pos = i;
                break;
            }
        }

        // Not found, create a new one
        let upval_ptr = {
            let stk_id = StkId::from_mut_ptr(unsafe { self.stack.as_mut_ptr().add(stack_index) });
            let vm = self.global_state_mut();
            vm.create_upvalue_open(stack_index, stk_id)?
        };

        self.open_upvalues_list.insert(insert_pos, upval_ptr);
        if !self.in_twups {
            self.in_twups = true;
            self.global_state.push_twups(self.thread);
        }

        Ok(upval_ptr)
    }

    /// Get stack reference (for GC tracing)
    #[inline(always)]
    pub fn stack(&self) -> &[LuaValue] {
        &self.stack
    }

    /// Get mutable pointer to stack for VM execution
    ///
    /// # Safety
    /// Caller must ensure stack is not reallocated during pointer usage
    #[inline(always)]
    pub(crate) fn stack_mut(&mut self) -> &mut [LuaValue] {
        &mut self.stack
    }

    /// Get stack length
    #[inline(always)]
    pub fn stack_len(&self) -> usize {
        self.stack.len()
    }

    /// Truncate stack to specified length
    /// Used after function calls to remove temporary values
    pub fn stack_truncate(&mut self) {
        let new_len = 0;
        if new_len < self.stack.len() {
            for upval_ptr in &self.open_upvalues_list {
                let upval = &mut upval_ptr.as_mut_ref().data;
                if upval.is_open() {
                    let stack_idx = upval.get_stack_index();
                    if stack_idx >= new_len {
                        // Invalidate upvalue pointing to truncated stack
                        upval.close(self.stack[stack_idx]);
                    }
                }
            }

            self.stack.truncate(new_len);
        }
    }

    /// Grow stack to accommodate more values
    /// Grow stack to accommodate needed size (similar to luaD_growstack in Lua)
    /// Stack can grow dynamically up to MAX_STACK_SIZE
    /// C functions can call this, which means Vec may reallocate
    #[inline(never)]
    pub fn grow_stack(&mut self, needed: usize) -> LuaResult<()> {
        if needed > self.safe_state.max_stack_size {
            return Err(self.stack_overflow_error());
        }
        if self.stack.len() < needed {
            self.resize(needed)?;
        }

        Ok(())
    }

    /// Get frame base by index
    #[inline(always)]
    pub fn get_frame_base(&self, frame_idx: usize) -> usize {
        self.call_stack.get(frame_idx).map(|f| f.base).unwrap_or(0)
    }

    /// Get frame PC by index
    #[inline(always)]
    pub fn get_frame_pc(&self, frame_idx: usize) -> u32 {
        // Only return PC if frame is within current call_depth (valid frames)
        if frame_idx < self.call_depth {
            self.call_stack.get(frame_idx).map(|f| f.pc).unwrap_or(0)
        } else {
            0
        }
    }

    /// Get frame function by index
    #[inline(always)]
    pub fn get_frame_func(&self, frame_idx: usize) -> Option<LuaValue> {
        // Only return frame if it's within current call_depth (valid frames)
        if frame_idx < self.call_depth {
            let ci = self.call_stack.get(frame_idx)?;
            self.stack.get(ci.func_index()).copied()
        } else {
            None
        }
    }

    /// Get frame by index (for GC root collection)
    #[inline(always)]
    pub fn get_frame(&self, frame_idx: usize) -> Option<&CallInfo> {
        // Only return frame if it's within current call_depth (valid frames)
        if frame_idx < self.call_depth {
            self.call_stack.get(frame_idx).map(|ci| &**ci)
        } else {
            None
        }
    }

    // ========================================================================
    // Debug info (port of lua_getinfo / auxgetinfo from ldebug.c)
    // ========================================================================

    /// Get debug info for a stack frame at the given level.
    /// Level 0 = most-recent frame (top of stack), level 1 = its caller, etc.
    /// `what` controls which fields are filled (same options as C Lua's lua_getinfo).
    /// Returns `None` if the level is out of range.
    pub fn get_info_by_level(&self, level: usize, what: &str) -> Option<DebugInfo> {
        let call_depth = self.call_depth();
        if level >= call_depth {
            return None;
        }
        let frame_idx = call_depth - 1 - level;
        let func = self.get_frame_func(frame_idx)?;
        let ci = self.get_frame(frame_idx);
        Some(self.fill_debug_info(&func, ci, Some(frame_idx), what))
    }

    /// Get debug info for a function value (not on the call stack).
    /// Only fields that don't require a stack frame are meaningful
    /// ('n' and 'l' will return empty/defaults).
    pub fn get_info_for_func(&self, func: &LuaValue, what: &str) -> DebugInfo {
        self.fill_debug_info(func, None, None, what)
    }

    /// Core implementation: fill a DebugInfo struct based on the 'what' options.
    /// Port of auxgetinfo from ldebug.c.
    fn fill_debug_info(
        &self,
        func: &LuaValue,
        ci: Option<&CallInfo>,
        frame_idx: Option<usize>,
        what: &str,
    ) -> DebugInfo {
        let mut info = DebugInfo::new();

        if let Some(lua_func) = func.as_lua_function() {
            let chunk = lua_func.chunk();

            for ch in what.chars() {
                match ch {
                    'S' => {
                        info.fill_source(
                            chunk.source_name.as_deref(),
                            chunk.linedefined as i32,
                            chunk.lastlinedefined as i32,
                        );
                    }
                    'l' => {
                        let currentline = if let Some(ci) = ci {
                            if ci.is_lua() {
                                let pc_idx = ci.pc.saturating_sub(1) as usize;
                                let line = chunk.line_info.get(pc_idx).copied().unwrap_or(0);
                                if line == 0 { -1 } else { line as i32 }
                            } else {
                                -1
                            }
                        } else {
                            -1
                        };
                        info.fill_currentline(currentline);
                    }
                    'u' => {
                        info.fill_upvalues(
                            chunk.upvalue_count as u8,
                            chunk.param_count as u8,
                            chunk.is_vararg,
                        );
                    }
                    'n' => {
                        if let Some(fidx) = frame_idx {
                            if let Some((namewhat, name)) = pub_getfuncname(self, fidx) {
                                info.fill_name(namewhat, &name);
                            } else {
                                info.fill_name_empty();
                            }
                        } else {
                            info.fill_name_empty();
                        }
                    }
                    't' => {
                        let (istailcall, extraargs) = if let Some(ci) = ci {
                            (ci.is_tail(), call_status::get_ccmt_count(ci.call_status))
                        } else {
                            (false, 0)
                        };
                        info.fill_tail(istailcall, extraargs);
                    }
                    'r' => {
                        // Transfer info — only meaningful inside hooks (CIST_HOOKED).
                        // Like C Lua: only return actual values when ci has CIST_HOOKED flag.
                        if let Some(ci) = ci {
                            if ci.call_status & CIST_HOOKED != 0 {
                                info.fill_transfer(self.ftransfer, self.ntransfer);
                            } else {
                                info.fill_transfer(0, 0);
                            }
                        } else {
                            info.fill_transfer(0, 0);
                        }
                    }
                    'L' => {
                        info.fill_activelines(&chunk.line_info, chunk.is_vararg);
                    }
                    'f' => {
                        info.fill_func(*func);
                    }
                    _ => {} // ignore unknown options
                }
            }
        } else if func.is_c_callable() {
            // C function
            for ch in what.chars() {
                match ch {
                    'S' => {
                        info.fill_source_c();
                    }
                    'l' => {
                        info.fill_currentline(-1);
                    }
                    'u' => {
                        // C functions: nups from the closure, params=0, isvararg=true
                        let nups = func
                            .as_cclosure()
                            .map(|c| c.upvalues().len() as u8)
                            .unwrap_or(0);
                        info.fill_upvalues_c(nups);
                    }
                    'n' => {
                        if let Some(fidx) = frame_idx {
                            if let Some((namewhat, name)) = pub_getfuncname(self, fidx) {
                                info.fill_name(namewhat, &name);
                            } else {
                                info.fill_name_empty();
                            }
                        } else {
                            info.fill_name_empty();
                        }
                    }
                    't' => {
                        let (istailcall, extraargs) = if let Some(ci) = ci {
                            (ci.is_tail(), call_status::get_ccmt_count(ci.call_status))
                        } else {
                            (false, 0)
                        };
                        info.fill_tail(istailcall, extraargs);
                    }
                    'r' => {
                        if let Some(ci) = ci {
                            if ci.call_status & CIST_HOOKED != 0 {
                                info.fill_transfer(self.ftransfer, self.ntransfer);
                            } else {
                                info.fill_transfer(0, 0);
                            }
                        } else {
                            info.fill_transfer(0, 0);
                        }
                    }
                    'L' => {
                        info.fill_activelines_nil();
                    }
                    'f' => {
                        info.fill_func(*func);
                    }
                    _ => {}
                }
            }
        }

        info
    }

    /// Get all open upvalues (for GC root collection)
    #[inline(always)]
    pub fn get_open_upvalues(&self) -> &[UpvaluePtr] {
        &self.open_upvalues_list
    }

    #[inline(always)]
    pub(crate) fn global_state(&self) -> &GlobalState {
        self.global_state.as_ref()
    }

    #[inline(always)]
    pub(crate) fn global_state_mut(&mut self) -> &mut GlobalState {
        self.global_state.as_mut()
    }

    #[inline(always)]
    pub(crate) fn global_state_handle(&self) -> GlobalStateHandle {
        self.global_state
    }

    #[inline(always)]
    pub(crate) fn allow_load_bytecode(&self) -> bool {
        self.safe_state.allow_load_bytecode
    }

    /// Lua 5.5-style ccall depth tracking: increment shared n_ccalls before
    /// a recursive `lua_execute` call.  Returns `Err("C stack overflow")` if
    /// the limit is reached.  The limit is checked against this thread's
    /// `safe_option.max_c_stack_depth` so that xpcall's EXTRA zone increase
    /// takes effect.
    #[inline(always)]
    pub(crate) fn inc_n_ccalls(&mut self) -> LuaResult<()> {
        let max_c_stack_depth = self.safe_state.max_c_stack_depth;
        let base_c_stack_depth = self.safe_state.base_c_stack_depth;
        let vm = self.global_state_mut();
        vm.n_ccalls += 1;
        if vm.n_ccalls >= max_c_stack_depth {
            vm.n_ccalls -= 1;
            self.c_stack_overflow(max_c_stack_depth > base_c_stack_depth)
        } else {
            Ok(())
        }
    }

    /// C Lua checks the C stack (`luaE_checkcstack`) before it pushes the
    /// callee's frame, so the error belongs to the caller: drop a Lua frame
    /// that was pushed but has not started running.
    #[cold]
    #[inline(never)]
    fn c_stack_overflow(&mut self, in_error_handler: bool) -> LuaResult<()> {
        if self.current_frame().is_some_and(|ci| ci.is_lua() && ci.pc == 0) {
            self.pop_frame();
        }
        if in_error_handler {
            // In error handler extra zone — C Lua's stackerror behavior
            return Err(LuaError::ErrorInErrorHandling);
        }
        Err(self.error("C stack overflow".to_string()))
    }

    /// Decrement shared n_ccalls after returning from a recursive `lua_execute`.
    #[inline(always)]
    pub(crate) fn dec_n_ccalls(&self) {
        self.global_state.as_mut().n_ccalls -= 1;
    }

    // ===== Call Frame Management =====

    /// Get current CallInfo by index (unchecked — caller must ensure idx < call_depth)
    #[inline(always)]
    pub(crate) fn get_call_info(&self, idx: usize) -> &CallInfo {
        debug_assert!(idx < self.call_stack.len());
        unsafe { self.call_stack.get_unchecked(idx).as_ref() }
    }

    #[inline(always)]
    pub(crate) fn current_ci_ptr(&self) -> *mut CallInfo {
        self.current_ci
    }

    /// Get mutable CallInfo by index (unchecked — caller must ensure idx < call_depth)
    #[inline(always)]
    pub(crate) fn get_call_info_mut(&mut self, idx: usize) -> &mut CallInfo {
        debug_assert!(idx < self.call_stack.len());
        unsafe { self.call_stack.get_unchecked(idx).as_mut() }
    }

    /// Pop the current call frame (Lua callers only — does NOT adjust VM n_ccalls)
    #[inline(always)]
    pub(crate) fn pop_call_frame(&mut self) {
        debug_assert!(self.call_depth > 0);
        let previous = unsafe { (*self.current_ci).previous };
        self.call_depth -= 1;
        self.current_ci = previous;
    }

    /// Get return values from stack
    /// Returns values from stack_base to stack_base + count
    pub fn get_return_values(&self, stack_base: usize, count: usize) -> Vec<LuaValue> {
        let mut results = Vec::with_capacity(count);
        for i in 0..count {
            if let Some(val) = self.stack_get(stack_base + i) {
                results.push(val);
            } else {
                results.push(LuaValue::nil());
            }
        }
        results
    }

    /// Get all return values from stack starting at stack_base
    pub fn get_all_return_values(&self, stack_base: usize) -> Vec<LuaValue> {
        let top = self.get_top();
        let count = top.saturating_sub(stack_base);
        self.get_return_values(stack_base, count)
    }

    // ===== Function Argument Access =====

    /// Get all arguments for the current C function call
    /// Returns arguments starting from index 1 (index 0 is the function itself)
    pub fn get_args(&self) -> Vec<LuaValue> {
        if self.call_depth == 0 {
            return Vec::new();
        }

        let frame = &self.call_stack[self.call_depth - 1];
        let base = frame.base;
        let top = frame.top as usize;

        // Arguments are from base to top-1 (NOT base+1!)
        // In Lua, the function itself is NOT part of the frame's stack
        // The frame starts at the first argument
        let arg_count = top.saturating_sub(base);

        let mut args = Vec::with_capacity(arg_count);
        for i in 0..arg_count {
            if let Some(val) = self.stack_get(base + i) {
                args.push(val);
            } else {
                args.push(LuaValue::nil());
            }
        }
        args
    }

    /// Get a specific argument (1-based index, Lua convention)
    /// Returns None if index is out of bounds
    #[inline(always)]
    pub fn get_arg(&self, index: usize) -> Option<LuaValue> {
        if index == 0 || self.call_depth == 0 {
            return None;
        }

        let frame = unsafe { &*self.current_ci };
        let base = frame.base;
        let top = frame.top as usize;

        // Arguments are 1-based: arg 1 is at base, arg 2 is at base+1, etc.
        // (NOT base+1, because C function frame.base already points to first arg)
        let stack_index = base + index - 1;

        // Check if argument position is within the frame's range and the stack
        if stack_index < top && stack_index < self.stack.len() {
            // Return the value (including nil values)
            Some(self.stack[stack_index])
        } else {
            // Argument doesn't exist
            None
        }
    }

    /// Get and convert a specific argument using the regular `FromLua` bridge.
    #[inline]
    pub fn get_arg_as<T: FromLua>(&mut self, index: usize) -> LuaResult<Option<T>> {
        let Some(value) = self.get_arg(index) else {
            return Ok(None);
        };

        T::from_lua(value, self)
            .map(Some)
            .map_err(|msg| self.error(msg))
    }

    /// Get the number of arguments for the current function call
    #[inline(always)]
    pub fn arg_count(&self) -> usize {
        if self.call_depth == 0 {
            return 0;
        }

        let frame = &self.call_stack[self.call_depth - 1];
        let base = frame.base;
        let top = frame.top as usize;

        // Arguments are from base to top-1
        top.saturating_sub(base)
    }

    /// Push one Rust value through the `IntoLua` bridge.
    ///
    /// This keeps the low-level stack API interoperable with high-level handle
    /// types like `Table`, `LuaString`, `Function`, and `Value`.
    #[inline]
    pub fn push<T: IntoLua>(&mut self, value: T) -> LuaResult<()> {
        let pushed = value.into_lua(self).map_err(|msg| self.error(msg))?;
        if pushed != 1 {
            return Err(self.error(format!(
                "push_value expects exactly one Lua value, got {}",
                pushed
            )));
        }
        Ok(())
    }

    /// Push one or more Rust values through the `IntoLua` bridge.
    #[inline]
    pub fn push_multi<T: IntoLua>(&mut self, value: T) -> LuaResult<usize> {
        value.into_lua(self).map_err(|msg| self.error(msg))
    }

    #[inline(always)]
    pub fn push_value(&mut self, value: LuaValue) -> LuaResult<()> {
        // Check stack limit (Lua's luaD_checkstack equivalent)
        if self.stack_top >= self.safe_state.max_stack_size {
            return Err(self.stack_overflow_error());
        }

        // Save current top before any borrows
        let current_top = self.stack_top;

        // Ensure physical stack is large enough (Lua's luaD_reallocstack equivalent)
        if current_top >= self.stack.len() {
            // 1.5 x growth strategy
            let mut new_size = current_top + current_top / 2;
            if new_size < current_top + 1 {
                new_size = current_top + 1;
            }
            if new_size > self.safe_state.max_stack_size {
                new_size = self.safe_state.max_stack_size;
            }
            self.resize(new_size)?;
        }

        // Write at logical top position (L->top.p->value = value)
        self.stack[current_top] = value;

        // Increment logical top (L->top.p++)
        // Do NOT modify frame.top (ci->top) — it's immutable after push_frame.
        self.stack_top = current_top + 1;

        Ok(())
    }

    // ===== Object Creation =====

    /// Create a raw table value.
    ///
    /// The returned LuaValue is not rooted by the API itself. Callers must push
    /// it onto the stack, store it in the registry/table/global, or otherwise
    /// anchor it before triggering any further allocation or GC step.
    ///
    /// For host-facing code, prefer LuaState::create_table_ref.
    #[inline(always)]
    pub fn create_table(&mut self, narr: usize, nrec: usize) -> CreateResult {
        self.global_state_mut().create_table(narr, nrec)
    }

    /// Create a raw function value.
    ///
    /// The returned LuaValue is not rooted by the API itself. Callers must root
    /// it before any subsequent allocation or GC step.
    ///
    /// For host-facing code, prefer LuaState::create_closure_ref or
    /// LuaState::create_function_typed.
    #[inline]
    pub fn create_function(&mut self, chunk: ProtoPtr, upvalues: UpvalueStore) -> CreateResult {
        self.global_state_mut().create_function(chunk, upvalues)
    }

    #[inline]
    pub fn create_upvalue_closed(&mut self, value: LuaValue) -> LuaResult<UpvaluePtr> {
        self.global_state_mut().create_upvalue_closed(value)
    }

    #[inline]
    pub fn create_upvalue_open(
        &mut self,
        stack_index: usize,
        stk_id: StkId,
    ) -> LuaResult<UpvaluePtr> {
        self.global_state_mut()
            .create_upvalue_open(stack_index, stk_id)
    }

    /// Create a raw string value.
    ///
    /// The returned LuaValue is not rooted by the API itself. Callers must root
    /// it before any subsequent allocation or GC step.
    ///
    /// For host-facing code, prefer LuaState::create_string_ref.
    #[inline]
    pub fn create_string(&mut self, s: &str) -> CreateResult {
        self.global_state_mut().create_string(s)
    }

    /// Create a registry-rooted string handle.
    #[inline]
    pub fn create_string_ref(&mut self, s: &str) -> LuaResult<LuaStringRef> {
        let value = self.create_string(s)?;
        Ok(self.to_string_ref(value).unwrap())
    }

    /// Create a raw binary string value.
    ///
    /// The returned LuaValue is not rooted by the API itself. Callers must root
    /// it before any subsequent allocation or GC step.
    ///
    /// For host-facing code, prefer LuaState::create_binary_ref.
    #[inline]
    pub fn create_binary(&mut self, data: Vec<u8>) -> CreateResult {
        self.global_state_mut().create_binary(data)
    }

    /// Create a registry-rooted binary/string handle.
    #[inline]
    pub fn create_binary_ref(&mut self, data: Vec<u8>) -> LuaResult<LuaStringRef> {
        let value = self.create_binary(data)?;
        Ok(self.to_string_ref(value).unwrap())
    }

    /// Create a raw string-like value from bytes.
    ///
    /// The returned LuaValue is not rooted by the API itself. Callers must root
    /// it before any subsequent allocation or GC step.
    ///
    /// For host-facing code, prefer LuaState::create_bytes_ref.
    #[inline]
    pub fn create_bytes(&mut self, bytes: &[u8]) -> CreateResult {
        self.global_state_mut().create_bytes(bytes)
    }

    /// Create a registry-rooted string handle from bytes.
    #[inline]
    pub fn create_bytes_ref(&mut self, bytes: &[u8]) -> LuaResult<LuaStringRef> {
        let value = self.create_bytes(bytes)?;
        Ok(self.to_string_ref(value).unwrap())
    }

    /// Create a raw userdata value.
    ///
    /// The returned LuaValue is not rooted by the API itself. Callers must root
    /// it before any subsequent allocation or GC step.
    ///
    /// For host-facing code, prefer LuaState::create_userdata_handle.
    #[inline]
    pub fn create_userdata(&mut self, data: LuaUserdata) -> CreateResult {
        self.global_state_mut().create_userdata(data)
    }

    /// Create a registry-rooted userdata handle.
    #[inline]
    pub fn create_userdata_handle<T: UserDataTrait + 'static>(
        &mut self,
        data: T,
    ) -> LuaResult<UserDataRef<T>> {
        let value = self.create_userdata(LuaUserdata::new(data))?;
        Ok(self.to_userdata_ref(value).unwrap())
    }

    /// Create a raw RClosure value.
    ///
    /// The returned LuaValue is not rooted by the API itself. Callers must root
    /// it before any subsequent allocation or GC step.
    ///
    /// For host-facing code, prefer LuaState::create_closure_ref.
    #[inline]
    pub fn create_closure<F>(&mut self, func: F) -> CreateResult
    where
        F: Fn(&mut LuaState) -> LuaResult<usize> + 'static,
    {
        self.global_state_mut().create_closure(func)
    }

    /// Create a registry-rooted closure handle.
    #[inline]
    pub fn create_closure_ref<F>(&mut self, func: F) -> LuaResult<LuaFunctionRef>
    where
        F: Fn(&mut LuaState) -> LuaResult<usize> + 'static,
    {
        let value = self.create_closure(func)?;
        Ok(self.to_function_ref(value).unwrap())
    }

    /// Create an RClosure with upvalues.
    #[inline]
    pub fn create_closure_with_upvalues<F>(
        &mut self,
        func: F,
        upvalues: Vec<LuaValue>,
    ) -> CreateResult
    where
        F: Fn(&mut LuaState) -> LuaResult<usize> + 'static,
    {
        self.global_state_mut()
            .create_closure_with_upvalues(func, upvalues)
    }

    /// Create a raw thread value with an empty stack.
    #[inline]
    pub fn create_thread(&mut self) -> CreateResult {
        self.global_state_mut().create_empty_thread()
    }

    /// Create a registry-rooted table handle.
    #[inline]
    pub fn create_table_ref(&mut self, narr: usize, nrec: usize) -> LuaResult<LuaTableRef> {
        let value = self.create_table(narr, nrec)?;
        Ok(self.to_table_ref(value).unwrap())
    }

    // ===== Global Access =====

    /// Get global variable
    #[inline]
    pub fn get_global_value(&mut self, name: &str) -> LuaResult<Option<LuaValue>> {
        self.global_state_mut().get_global(name)
    }

    /// Set global variable
    #[inline]
    pub fn set_global_value(&mut self, name: &str, value: LuaValue) -> LuaResult<()> {
        self.global_state_mut().set_global(name, value)
    }

    // ===== Convenience API =====

    /// Compile source code and return a callable function value with _ENV wired.
    ///
    /// See [`LuaState::load`] for details.
    pub fn load(&mut self, source: &str) -> LuaResult<LuaValue> {
        let global = self.global_state().global;
        let chunk = self.compile_chunk(source)?;
        let env_upval = self.global_state_mut().create_upvalue_closed(global)?;
        self.global_state_mut()
            .create_loaded_function(chunk, UpvalueStore::from_single(env_upval))
    }

    #[cfg(feature = "sandbox")]
    pub fn load_sandboxed(&mut self, source: &str, config: &SandboxConfig) -> LuaResult<LuaValue> {
        let chunk = self.compile_chunk(source)?;
        let env = self.global_state_mut().create_sandbox_env(config)?;
        let env_upval = self.global_state_mut().create_upvalue_closed(env)?;
        self.global_state_mut()
            .create_loaded_function(chunk, UpvalueStore::from_single(env_upval))
    }

    /// Compile source code with a chunk name and return a callable function value.
    ///
    /// See [`LuaState::load_with_name`] for details.
    pub fn load_with_name(&mut self, source: &str, chunk_name: &str) -> LuaResult<LuaValue> {
        let global = self.global_state().global;
        let chunk = self.compile_chunk_with_name(source, chunk_name)?;
        let env_upval = self.global_state_mut().create_upvalue_closed(global)?;
        self.global_state_mut()
            .create_loaded_function(chunk, UpvalueStore::from_single(env_upval))
    }

    pub fn load_bytes_with_name(&mut self, source: &[u8], chunk_name: &str) -> LuaResult<LuaValue> {
        let global = self.global_state().global;
        let chunk = self
            .global_state_mut()
            .compile_bytes_with_name(source, chunk_name)
            .map_err(|message| self.compile_error_bytes(message))?;
        let env_upval = self.global_state_mut().create_upvalue_closed(global)?;
        self.global_state_mut()
            .create_loaded_function(chunk, UpvalueStore::from_single(env_upval))
    }

    #[cfg(feature = "sandbox")]
    pub fn load_with_name_sandboxed(
        &mut self,
        source: &str,
        chunk_name: &str,
        config: &SandboxConfig,
    ) -> LuaResult<LuaValue> {
        let chunk = self.compile_chunk_with_name(source, chunk_name)?;
        let env = self.global_state_mut().create_sandbox_env(config)?;
        let env_upval = self.global_state_mut().create_upvalue_closed(env)?;
        self.global_state_mut()
            .create_loaded_function(chunk, UpvalueStore::from_single(env_upval))
    }

    /// Read a file, compile it, and execute it.
    ///
    /// See [`LuaState::dofile`] for details.
    pub fn dofile(&mut self, path: &str) -> LuaResult<Vec<LuaValue>> {
        let proto = self.load_proto_from_file(path)?;
        self.execute_chunk(proto)
    }

    /// Call a function value with arguments.
    ///
    /// Unlike the LuaVM version, this operates on the *current* coroutine state
    /// and is safe to call from within a CFunction.
    #[inline]
    pub fn call_function(
        &mut self,
        func: LuaValue,
        args: Vec<LuaValue>,
    ) -> LuaResult<Vec<LuaValue>> {
        self.call(func, args)
    }

    /// Look up a global function by name and call it.
    ///
    /// Safe to call from within a CFunction — operates on the current state.
    pub fn call_global(&mut self, name: &str, args: Vec<LuaValue>) -> LuaResult<Vec<LuaValue>> {
        let func = self
            .get_global_value(name)?
            .ok_or_else(|| self.error(format!("global '{}' not found", name)))?;
        self.call(func, args)
    }

    /// Register a synchronous Rust closure as a Lua global function.
    ///
    /// See [`LuaState::register_function`] for details.
    pub fn register_function<F>(&mut self, name: &str, f: F) -> LuaResult<()>
    where
        F: Fn(&mut LuaState) -> LuaResult<usize> + 'static,
    {
        let closure_val = self.create_closure(f)?;
        self.set_global_value(name, closure_val)
    }

    /// Create a typed Rust closure as a standalone rooted function handle.
    pub fn create_function_typed<F, Args, R>(&mut self, f: F) -> LuaResult<LuaFunctionRef>
    where
        F: LuaTypedCallback<Args, R>,
    {
        self.create_closure_ref(move |state| f.invoke_typed(state))
    }

    pub fn register_function_typed<F, Args, R>(&mut self, name: &str, f: F) -> LuaResult<()>
    where
        F: LuaTypedCallback<Args, R>,
    {
        self.register_function(name, move |state| f.invoke_typed(state))
    }

    /// Register a typed async Rust closure as a Lua global function.
    ///
    /// See [`LuaState::register_async_typed`] for details.
    pub fn register_async_typed<F, Args, R>(&mut self, name: &str, f: F) -> LuaResult<()>
    where
        F: LuaTypedAsyncCallback<Args, R>,
    {
        let wrapper = move |state: &mut LuaState| {
            let future = f.invoke_typed_async(state)?;
            state.set_pending_future(future);
            state.do_yield(vec![async_thread::async_sentinel_value()])?;
            Ok(0)
        };

        let closure_val = self.create_closure(wrapper)?;
        self.set_global_value(name, closure_val)
    }

    // ===== Async Support =====

    /// Register an async function as a Lua global (convenience proxy for `LuaVM::register_async`).
    ///
    /// See [`LuaState::register_async`] for details.
    pub fn register_async<F, Fut>(&mut self, name: &str, f: F) -> LuaResult<()>
    where
        F: Fn(Vec<LuaValue>) -> Fut + 'static,
        Fut: std::future::Future<Output = LuaResult<Vec<AsyncReturnValue>>> + 'static,
    {
        let wrapper = async_thread::wrap_async_function(f);
        let closure_val = self.create_closure(wrapper)?;
        self.set_global_value(name, closure_val)
    }

    pub fn create_async_thread(
        &mut self,
        chunk: LuaProto,
        args: Vec<LuaValue>,
    ) -> LuaResult<async_thread::AsyncThread> {
        let global = self.global_state().global;
        let env_upval = self.global_state_mut().create_upvalue_closed(global)?;
        let func_val = self
            .global_state_mut()
            .create_loaded_function(chunk, UpvalueStore::from_single(env_upval))?;
        let thread_val = self.global_state_mut().create_thread(func_val)?;
        Ok(async_thread::AsyncThread::new(
            thread_val,
            GlobalStateHandle::from_global(self.global_state_mut()),
            args,
        ))
    }

    pub async fn execute_async(&mut self, source: &str) -> LuaResult<Vec<LuaValue>> {
        let chunk = self.compile_chunk(source)?;
        let async_thread = self.create_async_thread(chunk, vec![])?;
        async_thread.await
    }

    // ===== Execute =====

    /// Compile and execute a Lua source string, returning results.
    ///
    /// This is a convenience proxy for `LuaVM::execute_string`.
    ///
    /// # Example
    /// ```ignore
    /// let results = state.execute("return 1 + 2")?;
    /// assert_eq!(results[0].as_integer(), Some(3));
    /// ```
    pub fn execute(&mut self, source: &str) -> LuaResult<Vec<LuaValue>> {
        let func = self.load(source)?;
        self.call(func, vec![])
    }

    #[cfg(feature = "sandbox")]
    pub fn execute_sandboxed(
        &mut self,
        source: &str,
        config: &SandboxConfig,
    ) -> LuaResult<Vec<LuaValue>> {
        let chunk = self
            .global_state_mut()
            .compile(source)
            .map_err(|msg| self.compile_error(msg))?;
        let env = self.global_state_mut().create_sandbox_env(config)?;
        let limits = config.runtime_limits();
        self.with_sandbox_runtime_limits(limits, |state| {
            let chunk = state.global_state_mut().prepare_loaded_chunk(chunk)?;
            let env_upval = state.global_state_mut().create_upvalue_closed(env)?;
            let func = state
                .global_state_mut()
                .create_function(chunk, UpvalueStore::from_single(env_upval))?;
            state.call(func, vec![])
        })
    }

    /// Execute a pre-compiled chunk, returning results.
    ///
    /// This is a convenience proxy for `LuaVM::execute`.
    pub fn execute_chunk(&mut self, chunk: ProtoPtr) -> LuaResult<Vec<LuaValue>> {
        let global = self.global_state().global;
        let env_upval = self.global_state_mut().create_upvalue_closed(global)?;
        let func = self
            .global_state_mut()
            .create_function(chunk, UpvalueStore::from_single(env_upval))?;
        self.call(func, vec![])
    }

    pub fn get_global_as<T: FromLua>(&mut self, name: &str) -> LuaResult<Option<T>> {
        match self.get_global_value(name)? {
            None => Ok(None),
            Some(val) => {
                let converted = T::from_lua(val, self).map_err(|msg| self.error(msg))?;
                Ok(Some(converted))
            }
        }
    }

    #[inline]
    pub fn compile_error(&mut self, message: impl Into<String>) -> LuaError {
        self.error(message.into());
        LuaError::CompileError
    }

    /// `compile_error` for a message of bytes (a syntax error quotes names
    /// and tokens of the chunk, which need not be valid UTF-8).
    #[cold]
    pub fn compile_error_bytes(&mut self, message: Vec<u8>) -> LuaError {
        self.error_bytes(message);
        LuaError::CompileError
    }

    pub fn compile_chunk(&mut self, source: &str) -> LuaResult<LuaProto> {
        self.global_state_mut()
            .compile(source)
            .map_err(|msg| self.compile_error(msg))
    }

    pub fn compile_chunk_with_name(
        &mut self,
        source: &str,
        chunk_name: &str,
    ) -> LuaResult<LuaProto> {
        self.global_state_mut()
            .compile_with_name(source, chunk_name)
            .map_err(|msg| self.compile_error(msg))
    }

    pub fn load_proto_from_file(&mut self, path: &str) -> LuaResult<ProtoPtr> {
        self.global_state_mut()
            .load_proto_from_file(path)
            .map_err(|msg| self.error_bytes(msg))
    }

    /// `load_proto_from_file` for contents already read with `read_chunk_file`.
    pub(crate) fn load_proto_from_file_bytes(
        &mut self,
        path: &str,
        file_bytes: Vec<u8>,
    ) -> LuaResult<ProtoPtr> {
        self.global_state_mut()
            .load_proto_from_file_bytes(path, file_bytes)
            .map_err(|msg| self.error_bytes(msg))
    }

    pub fn get_error_message(&mut self, e: LuaError) -> String {
        self.get_error_msg(e)
    }

    /// The message of error `e` as the bytes a Lua caller sees: unlike
    /// `get_error_msg` it keeps the bytes of a message that is not UTF-8.
    pub(crate) fn take_error_bytes(&mut self, e: LuaError) -> Vec<u8> {
        if matches!(e, LuaError::OutOfMemory) {
            return self.get_error_msg(e).into_bytes();
        }
        match self.global_state_mut().take_error() {
            ErrorMsg::Msg(msg) | ErrorMsg::Traced { message: msg, .. } => msg.into_bytes(),
            ErrorMsg::Object(obj) => match lauxlib::to_lstr(self, &obj) {
                Some(text) => text.to_vec(),
                None => format!("(error object is a {} value)", obj.type_name()).into_bytes(),
            },
            ErrorMsg::None => Vec::new(),
        }
    }

    /// The error message; for an error that escaped a top-level call, with
    /// the stack traceback rendered when it was raised (`unwind_host_error`).
    pub fn get_full_error(&mut self, e: LuaError) -> LuaFullError {
        let message = match self.global_state_mut().take_error() {
            ErrorMsg::Traced { full, .. } => full,
            other => {
                self.global_state_mut().error_msg = other;
                self.get_error_msg(e)
            }
        };
        LuaFullError { kind: e, message }
    }

    /// The host side of a top-level call that failed (lua.c's `docall`):
    /// run `host_message_handler` while the failing frames are still on the
    /// stack, unwind them as `lua_pcall` does (closing upvalues and
    /// to-be-closed variables from `func_idx`), and keep the rendered
    /// message for `get_full_error`.
    #[cold]
    #[inline(never)]
    fn unwind_host_error(&mut self, e: LuaError, func_idx: usize) -> LuaError {
        if self.call_depth() == 0 {
            // Failed before any frame existed (e.g. calling a non-function).
            self.set_top_raw(func_idx);
            return e;
        }
        let message = match &self.global_state().error_msg {
            _ if matches!(e, LuaError::OutOfMemory) => None,
            ErrorMsg::Msg(msg) => Some(msg.clone()),
            ErrorMsg::Object(obj) => lauxlib::to_lstr(self, obj)
                .map(|text| String::from_utf8_lossy(&text).into_owned()),
            _ => None,
        };
        let handler = LuaValue::cfunction(Self::host_message_handler);
        match self.recover_protected_call(e, 0, func_idx, Some(handler)) {
            Ok((err, error_in_handler)) => {
                let full = self.error_object_text(&err);
                // A `__tostring` result or an error raised by a `__close`
                // replaces the message: then there is no shorter form.
                let message = message
                    .filter(|message| full.starts_with(message.as_str()))
                    .unwrap_or_else(|| full.clone());
                self.set_top_raw(func_idx);
                self.global_state_mut().error_msg = ErrorMsg::Traced { message, full };
                if error_in_handler { LuaError::ErrorInErrorHandling } else { e }
            }
            Err(error) => {
                while self.call_depth() > 0 {
                    self.pop_frame();
                }
                self.close_upvalues(func_idx);
                self.tbc_list.retain(|&index| index < func_idx);
                self.set_top_raw(func_idx);
                error
            }
        }
    }

    /// A failed call on a thread that is not the one executing (see `call`):
    /// no pcall of this thread will unwind the frames of the call, so do it
    /// as `luaD_pcall` does — pop the frames above `initial_depth`, close the
    /// upvalues and to-be-closed variables from `func_idx` with the error —
    /// and raise the error value that is left, which is the original one
    /// unless a `__close` method replaced it. The value is the pending error
    /// object (a GC root) from here on.
    #[cold]
    #[inline(never)]
    fn unwind_foreign_error(&mut self, e: LuaError, initial_depth: usize, func_idx: usize) -> LuaError {
        if self.call_depth() <= initial_depth {
            // Failed before a frame existed (e.g. calling a non-function).
            self.set_top_raw(func_idx);
            return e;
        }
        match self.recover_protected_call(e, initial_depth, func_idx, None) {
            Ok((err, error_in_handler)) => {
                self.set_top_raw(func_idx);
                self.set_error_object(err);
                if error_in_handler { LuaError::ErrorInErrorHandling } else { e }
            }
            Err(error) => {
                while self.call_depth() > initial_depth {
                    self.pop_frame();
                }
                self.close_upvalues(func_idx);
                self.tbc_list.retain(|&index| index < func_idx);
                self.set_top_raw(func_idx);
                error
            }
        }
    }

    /// lua.c's `msghandler`: a string or number message, else the result of
    /// a `__tostring` metamethod (returned without a traceback), else
    /// "(error object is a X value)"; then the stack traceback.
    fn host_message_handler(l: &mut LuaState) -> LuaResult<usize> {
        let err = l.get_arg(1).unwrap_or_default();
        let msg = match lauxlib::to_lstr(l, &err) {
            Some(text) => text.to_vec(),
            None => {
                if let Some(mm) = get_metamethod_event(l, &err, TmKind::ToString) {
                    let text = execute::call_tm_res1(l, mm, err)?;
                    if text.is_string() {
                        l.push_value(text)?;
                        return Ok(1);
                    }
                }
                format!("(error object is a {} value)", err.type_name()).into_bytes()
            }
        };
        let lua53 = l.global_state().language() == crate::LuaLanguageLevel::Lua53;
        let text = crate::stdlib::debug::traceback_text(lua53, l, Some(&msg), 1);
        let text = l.create_bytes(&text)?;
        l.push_value(text)?;
        Ok(1)
    }

    /// `lua_tostring` of an error object, or lua.c's description of it.
    fn error_object_text(&self, err: &LuaValue) -> String {
        match lauxlib::to_lstr(self, err) {
            Some(text) => String::from_utf8_lossy(&text).into_owned(),
            None => format!("(error object is a {} value)", err.type_name()),
        }
    }

    pub fn protected_call(
        &mut self,
        func: LuaValue,
        args: Vec<LuaValue>,
    ) -> LuaResult<(bool, Vec<LuaValue>)> {
        self.pcall(func, args)
    }

    pub async fn call_async(
        &mut self,
        func: LuaValue,
        args: Vec<LuaValue>,
    ) -> LuaResult<Vec<LuaValue>> {
        let thread_val = self.global_state_mut().create_thread(func)?;
        let async_thread = async_thread::AsyncThread::new(
            thread_val,
            GlobalStateHandle::from_global(self.global_state_mut()),
            args,
        );
        async_thread.await
    }

    pub async fn call_async_global(
        &mut self,
        name: &str,
        args: Vec<LuaValue>,
    ) -> LuaResult<Vec<LuaValue>> {
        let func = self
            .get_global_value(name)?
            .ok_or_else(|| self.error(format!("global '{}' not found", name)))?;
        self.call_async(func, args).await
    }

    pub fn create_async_call_handle(
        &mut self,
        func: LuaValue,
    ) -> LuaResult<async_thread::AsyncCallHandle> {
        let global = self.global_state().global;
        let chunk = self.compile_chunk(async_thread::ASYNC_CALL_RUNNER)?;
        let env_upval = self.global_state_mut().create_upvalue_closed(global)?;
        let runner_func = self
            .global_state_mut()
            .create_loaded_function(chunk, UpvalueStore::from_single(env_upval))?;
        let thread_val = self.global_state_mut().create_thread(runner_func)?;
        async_thread::AsyncCallHandle::new(
            thread_val,
            GlobalStateHandle::from_global(self.global_state_mut()),
            func,
        )
    }

    pub fn create_async_call_handle_global(
        &mut self,
        name: &str,
    ) -> LuaResult<async_thread::AsyncCallHandle> {
        let func = self
            .get_global_value(name)?
            .ok_or_else(|| self.error(format!("global '{}' not found", name)))?;
        self.create_async_call_handle(func)
    }

    // ===== Table Operations =====

    /// Get value from table (raw, no metamethods)
    pub fn raw_get(&mut self, table: &LuaValue, key: &LuaValue) -> Option<LuaValue> {
        self.global_state_mut().raw_get(table, key)
    }

    /// Get value from table with __index metamethod support
    pub fn table_get(&mut self, table: &LuaValue, key: &LuaValue) -> LuaResult<Option<LuaValue>> {
        // First try raw access
        if let Some(val) = self.global_state_mut().raw_get(table, key) {
            return Ok(Some(val));
        }
        // If not found, try __index metamethod
        execute::helper::finishget(self, table, key)
    }

    /// Set value in table with metamethod support (__newindex)
    pub fn table_set(&mut self, table: &LuaValue, key: LuaValue, value: LuaValue) -> LuaResult<()> {
        execute::helper::finishset(self, table, &key, value, false)?;
        Ok(())
    }

    /// Compare two values using < operator with metamethod support (__lt)
    pub fn obj_lt(&mut self, a: &LuaValue, b: &LuaValue) -> LuaResult<bool> {
        // Integer-integer
        if let (Some(i1), Some(i2)) = (a.as_integer(), b.as_integer()) {
            return Ok(i1 < i2);
        }
        // Float-float
        if let (Some(f1), Some(f2)) = (a.as_float(), b.as_float()) {
            return Ok(f1 < f2);
        }
        // Mixed number
        if let (Some(n1), Some(n2)) = (a.as_number(), b.as_number()) {
            return Ok(n1 < n2);
        }
        // String (including binary): compare raw bytes
        if a.is_string() && b.is_string() {
            let ba = a.as_bytes();
            let bb = b.as_bytes();
            if let (Some(ba), Some(bb)) = (ba, bb) {
                return Ok(ba < bb);
            }
        }
        // Try __lt metamethod
        match execute::metamethod::try_comp_tm(self, *a, *b, execute::TmKind::Lt) {
            Ok(Some(result)) => Ok(result),
            Ok(None) => Err(ordererror(self, a, b)),
            Err(e) => Err(e),
        }
    }

    /// Get object length with metamethod support (__len)
    /// Returns the length as i64, going through __len if available.
    pub fn obj_len(&mut self, obj: &LuaValue) -> LuaResult<i64> {
        if let Some(bytes) = obj.as_bytes() {
            return Ok(bytes.len() as i64);
        }
        if obj.ttistable() {
            if let Some(mm) = execute::get_metamethod_event(self, obj, execute::TmKind::Len) {
                let result = execute::call_tm_res(self, mm, *obj, *obj)?;
                return result
                    .as_integer()
                    .ok_or_else(|| self.error("object length is not an integer".to_string()));
            }

            return Ok(obj.as_table().unwrap().len() as i64);
        }
        if let Some(mm) = execute::get_metamethod_event(self, obj, execute::TmKind::Len) {
            let result = execute::call_tm_res(self, mm, *obj, *obj)?;
            return result
                .as_integer()
                .ok_or_else(|| self.error("object length is not an integer".to_string()));
        }
        Err(self.error(format!(
            "attempt to get length of a {} value",
            obj.type_name()
        )))
    }

    /// Set value in table
    pub fn raw_set(&mut self, table: &LuaValue, key: LuaValue, value: LuaValue) -> bool {
        self.global_state_mut().raw_set(table, key, value)
    }

    pub fn raw_geti(&mut self, table: &LuaValue, index: i64) -> Option<LuaValue> {
        self.global_state_mut().raw_geti(table, index)
    }

    pub fn raw_seti(&mut self, table: &LuaValue, index: i64, value: LuaValue) -> bool {
        self.global_state_mut().raw_seti(table, index, value)
    }

    /// Get element from table by integer key with __index metamethod support.
    /// Like C Lua's lua_geti. Returns nil if not found.
    pub fn table_geti(&mut self, table: &LuaValue, key: i64) -> LuaResult<LuaValue> {
        let k = LuaValue::integer(key);
        Ok(self.table_get(table, &k)?.unwrap_or(LuaValue::nil()))
    }

    /// Set element in table by integer key with __newindex metamethod support.
    /// Like C Lua's lua_seti.
    pub fn table_seti(&mut self, table: &LuaValue, key: i64, value: LuaValue) -> LuaResult<()> {
        let k = LuaValue::integer(key);
        self.table_set(table, k, value)
    }

    pub fn get_error_msg(&mut self, e: LuaError) -> String {
        match e {
            LuaError::OutOfMemory => {
                format!(
                    "out of memory: {}",
                    self.global_state_mut().gc.get_error_message()
                )
            }
            _ => match self.global_state_mut().take_error() {
                ErrorMsg::Msg(msg) | ErrorMsg::Traced { message: msg, .. } => msg,
                ErrorMsg::Object(obj) => self.error_object_text(&obj),
                ErrorMsg::None => String::new(),
            },
        }
    }

    // ===== Unprotected Call =====

    /// Unprotected call - like C Lua's lua_call / lua_callk.
    /// Errors propagate as Err(LuaError) to the enclosing pcall boundary.
    /// Does NOT create an error recovery boundary, so __close handlers
    /// see the correct error chain without an extra pcall frame. A call made
    /// by the host alone (no thread is executing) is the host's boundary: its
    /// error is rendered with a traceback and its frames are unwound
    /// (`unwind_host_error`). A call on a thread other than the one that is
    /// executing (a Rust callback of a coroutine running a function handle on
    /// the main thread) raises its error in that thread: the frames of this
    /// call are unwound here, as no pcall of this thread will do it, and the
    /// error value stays the one that was raised (`unwind_foreign_error`).
    pub fn call(&mut self, func: LuaValue, args: Vec<LuaValue>) -> LuaResult<Vec<LuaValue>> {
        let initial_depth = self.call_depth();
        // Use stack_top (logical top) instead of stack.len() (physical end).
        // The physical stack can be much larger than needed (e.g., after deep
        // recursion tests), so placing the function at stack.len() would waste
        // address space and risk hitting max_stack_size limits unnecessarily.
        let func_idx = self.stack_top;
        let arg_count = args.len();
        let needed = func_idx + 1 + arg_count;

        // Ensure physical stack has room for function + args
        if needed > self.stack.len() {
            self.resize(needed)?;
        }

        // Write function and args at stack_top position
        self.stack[func_idx] = func;
        for (i, arg) in args.into_iter().enumerate() {
            self.stack[func_idx + 1 + i] = arg;
        }
        self.stack_top = needed;

        // A Rust caller has no continuation, so the callee may not yield
        // (C Lua's luaD_callnoyield).
        self.nny += 1;
        let me: *const LuaState = self;
        let outer = std::mem::replace(&mut self.global_state_mut().executing, me);
        let foreign = !outer.is_null() && !std::ptr::eq(outer, me);
        let result = 'call: {
            // Resolve __call metamethod chain if needed
            let (actual_arg_count, ccmt_depth) = match resolve_call_chain(self, func_idx, arg_count) {
                Ok(resolved) => resolved,
                Err(error) => break 'call Err(error),
            };
            let func_val = self.stack[func_idx];
            if func_val.is_c_callable() {
                // C function - call directly via call_c_function (unprotected)
                break 'call call_c_function(self, func_idx, actual_arg_count, -1);
            }
            // Lua function - push frame and execute
            let base = func_idx + 1;
            if let Err(error) = self.push_frame(&func_val, base, actual_arg_count, -1) {
                break 'call Err(error);
            }
            if ccmt_depth > 0 {
                let frame_idx = self.call_depth - 1;
                if let Some(frame) = self.call_stack.get_mut(frame_idx) {
                    frame.call_status = call_status::set_ccmt_count(frame.call_status, ccmt_depth);
                }
            }
            self.inc_n_ccalls().and_then(|()| {
                let r = lua_execute(self, initial_depth);
                self.dec_n_ccalls();
                r
            })
        };
        let result = match result {
            Err(error) if !matches!(error, LuaError::Yield | LuaError::CloseThread) => {
                if foreign {
                    Err(self.unwind_foreign_error(error, initial_depth, func_idx))
                } else if initial_depth == 0 {
                    Err(self.unwind_host_error(error, func_idx))
                } else {
                    Err(error)
                }
            }
            other => other,
        };
        self.global_state_mut().executing = outer;
        self.nny -= 1;
        result?; // Propagate errors without catching

        // Collect results from func_idx to stack_top
        let mut results = Vec::new();
        for i in func_idx..self.stack_top {
            if let Some(val) = self.stack_get(i) {
                results.push(val);
            }
        }

        // Clean up: nil out used slots for GC safety, restore stack_top.
        // Don't truncate the physical stack — it may be needed by the caller's
        // frame (ci.top). The physical stack will be shrunk naturally by GC or
        // when the enclosing frame exits.
        {
            let clear_end = self.stack_top.min(self.stack.len());
            for i in func_idx..clear_end {
                self.stack[i] = LuaValue::nil();
            }
        }
        self.stack_top = func_idx;

        // Restore caller frame top if needed
        if self.call_depth() > 0 {
            let ci_idx = self.call_depth() - 1;
            let frame_top = self.get_call_info(ci_idx).top as usize;
            if self.stack_top < frame_top {
                self.stack_top = frame_top;
            }
        }

        Ok(results)
    }

    /// Fast comparison call for sort — avoids Vec allocations entirely.
    /// Calls `func(a, b)` and returns whether the result is truthy.
    ///
    /// This is a specialized hot-path for `table.sort` with a custom comparator.
    /// Instead of going through `call()` which allocates two `Vec<LuaValue>` per
    /// invocation (~10,000 comparisons per 1000-element sort = ~20,000 heap allocs),
    /// this writes func+args directly on the stack and reads the boolean result
    /// in-place.
    #[inline]
    pub(crate) fn call_compare(
        &mut self,
        func: LuaValue,
        a: LuaValue,
        b: LuaValue,
    ) -> LuaResult<bool> {
        let initial_depth = self.call_depth();
        let func_idx = self.stack_top;
        let needed = func_idx + 3;

        // Ensure stack has room for func + 2 args
        if needed > self.stack.len() {
            self.resize(needed)?;
        }

        // Place func, a, b directly on stack (zero allocation)
        self.stack[func_idx] = func;
        self.stack[func_idx + 1] = a;
        self.stack[func_idx + 2] = b;
        self.stack_top = needed;

        if func.is_lua_function() {
            // Lua function fast path — skip resolve_call_chain, skip Vec alloc
            let lua_func = func
                .as_lua_function()
                .expect("protected call checked lua function type before access");
            let chunk = lua_func.chunk();
            let base = func_idx + 1;

            self.push_lua_frame(
                base,
                2, // nparams
                1, // nresults — we only need the boolean
                chunk.param_count,
                chunk.max_stack_size,
                chunk as *const _,
                lua_func.upvalues().as_ptr(),
            )?;

            self.inc_n_ccalls()?;
            let r = lua_execute(self, initial_depth);
            self.dec_n_ccalls();
            r?;
        } else if func.is_c_callable() {
            // C function path
            call_c_function(self, func_idx, 2, 1)?;
        } else {
            // Fallback for __call metamethods — rare in sort comparators
            let results = self.call(func, vec![a, b])?;
            return Ok(results.first().map(|v| v.is_truthy()).unwrap_or(false));
        }

        // Result is at stack[func_idx] (placed there by RETURN handler / call_c_function)
        let result = self.stack[func_idx].is_truthy();

        // Clean up: nil out the slot, restore stack_top
        self.stack[func_idx] = LuaValue::nil();
        self.stack_top = func_idx;

        // Restore caller frame top if needed
        if self.call_depth() > 0 {
            let ci_idx = self.call_depth() - 1;
            let frame_top = self.get_call_info(ci_idx).top as usize;
            if self.stack_top < frame_top {
                self.stack_top = frame_top;
            }
        }

        Ok(result)
    }

    // ===== Protected Call (pcall/xpcall) =====

    /// Protected call - execute function with error handling (pcall semantics)
    /// Returns (success, results) where:
    /// - success=true, results=return values
    /// - success=false, results=\[error_message\]
    ///   Note: Yields are NOT caught by pcall - they propagate through
    pub fn pcall(
        &mut self,
        func: LuaValue,
        args: Vec<LuaValue>,
    ) -> LuaResult<(bool, Vec<LuaValue>)> {
        // This is equivalent to C Lua's lua_call → luaD_callnoyield:
        // the callback runs in a non-yieldable context.
        self.nny += 1;
        let result = self.pcall_inner(func, args);
        self.nny -= 1;
        result
    }

    /// Inner implementation of pcall (separated for nny scoping)
    fn pcall_inner(
        &mut self,
        func: LuaValue,
        args: Vec<LuaValue>,
    ) -> LuaResult<(bool, Vec<LuaValue>)> {
        // Save state for cleanup
        let initial_depth = self.call_depth();
        let saved_stack_top = self.stack_top;
        // Use stack_top (logical top) instead of stack.len() (physical end).
        // See call() for rationale.
        let func_idx = self.stack_top;
        let arg_count = args.len();
        let needed = func_idx + 1 + arg_count;

        // Ensure physical stack has room for function + args
        if needed > self.stack.len() {
            self.resize(needed)?;
        }

        // Write function and args at stack_top position
        self.stack[func_idx] = func;
        for (i, arg) in args.into_iter().enumerate() {
            self.stack[func_idx + 1 + i] = arg;
        }

        // Sync logical stack top
        self.stack_top = needed;

        // Resolve __call chain if needed

        let (actual_arg_count, ccmt_depth) = match resolve_call_chain(self, func_idx, arg_count) {
            Ok((count, depth)) => (count, depth),
            Err(e) => {
                let error_msg = self.get_error_message(e);
                let err_str = self.create_string(&error_msg)?;
                self.stack_top = saved_stack_top;
                return Ok((false, vec![err_str]));
            }
        };

        // Get resolved function
        let func = self
            .stack_get(func_idx)
            .ok_or_else(|| self.error("pcall: function not found".to_string()))?;

        // Check if it's a C function
        let is_c_callable = func.is_c_callable();
        if is_c_callable {
            // Create frame for C function
            let base = func_idx + 1;
            if let Err(e) = self.push_frame(&func, base, actual_arg_count, -1) {
                self.stack_top = saved_stack_top;
                let error_msg = self.get_error_message(e);
                let err_str = self.create_string(&error_msg)?;
                return Ok((false, vec![err_str]));
            }

            // Set ccmt count in call_status
            if ccmt_depth > 0 {
                let frame_idx = self.call_depth - 1;
                if let Some(frame) = self.call_stack.get_mut(frame_idx) {
                    frame.call_status = call_status::set_ccmt_count(frame.call_status, ccmt_depth);
                }
            }

            // Call the C function; a Rust closure has no raw function pointer
            let result = if let Some(c_func) = func.as_cfunction() {
                c_func(self)
            } else if let Some(closure) = func.as_cclosure() {
                (closure.func())(self)
            } else {
                match func.as_rclosure() {
                    Some(closure) => closure.call(self),
                    None => unreachable!("is_c_callable covers cfunction, cclosure and rclosure"),
                }
            };

            // CloseThread bypasses all pcalls — don't pop this frame,
            // handle_resume_result will pop everything.
            if matches!(result, Err(LuaError::CloseThread)) {
                return Err(LuaError::CloseThread);
            }

            // Pop frame
            self.pop_frame();

            match result {
                Ok(nresults) => {
                    // Success - collect results from stack_top (where push_value writes)
                    let mut results = Vec::new();
                    let result_start = self.stack_top.saturating_sub(nresults);

                    for i in result_start..self.stack_top {
                        if let Some(val) = self.stack_get(i) {
                            results.push(val);
                        }
                    }

                    // Clean up stack
                    self.stack_top = saved_stack_top;

                    Ok((true, results))
                }
                Err(LuaError::Yield) => Err(LuaError::Yield),
                Err(LuaError::CloseThread) => Err(LuaError::CloseThread),
                Err(e) => {
                    let raised = self.caught_error_value(e)?;
                    let result_err = self.errormsg_object(raised)?;
                    self.stack_top = saved_stack_top;
                    Ok((false, vec![result_err]))
                }
            }
        } else {
            // Lua function - use lua_execute
            let base = func_idx + 1;
            // pcall expects all return values
            if let Err(e) = self.push_frame(&func, base, actual_arg_count, -1) {
                self.stack_top = saved_stack_top;
                let error_msg = self.get_error_message(e);
                let err_str = self.create_string(&error_msg)?;
                return Ok((false, vec![err_str]));
            }

            // Execute via lua_execute — only execute the new frame
            self.inc_n_ccalls()?;
            let result = execute::lua_execute(self, initial_depth);
            self.dec_n_ccalls();

            match result {
                Ok(()) => {
                    // Success - collect return values from stack
                    // Use stack_top (not stack.len()) because RETURN opcodes
                    // set stack_top to reflect actual return values
                    let mut results = Vec::new();
                    for i in func_idx..self.stack_top {
                        if let Some(val) = self.stack_get(i) {
                            results.push(val);
                        }
                    }

                    // Ensure call_depth is back to initial_depth
                    // (normally RETURN should have handled this, but double-check)
                    while self.call_depth() > initial_depth {
                        self.pop_frame();
                    }

                    // Clean up stack
                    self.stack_top = saved_stack_top;

                    Ok((true, results))
                }
                Err(LuaError::Yield) => Err(LuaError::Yield),
                Err(LuaError::CloseThread) => Err(LuaError::CloseThread),
                Err(e) => {
                    let (result_err, _) =
                        self.recover_protected_call(e, initial_depth, func_idx, None)?;
                    self.stack_top = saved_stack_top;
                    Ok((false, vec![result_err]))
                }
            }
        }
    }

    /// The error path of `luaD_pcall` for a callee whose frames start at
    /// `initial_depth`: run the message `handler` (if any) with those frames
    /// still in place, unwind them, then close upvalues and to-be-closed
    /// variables from `level` with the error. Returns the final error object
    /// and whether error handling itself failed (`LUA_ERRERR`). If a
    /// `__close` yields, the protected call's frame (`initial_depth - 1`) is
    /// marked for `finish_c_frame` and the yield propagates.
    fn recover_protected_call(
        &mut self,
        e: LuaError,
        initial_depth: usize,
        level: usize,
        handler: Option<LuaValue>,
    ) -> LuaResult<(LuaValue, bool)> {
        let mut error_in_handler = matches!(e, LuaError::ErrorInErrorHandling);
        let mut err = if error_in_handler {
            self.clear_error();
            self.create_string("error in error handling")?
        } else {
            let raised = self.caught_error_value(e)?;
            match handler {
                Some(handler) => match self.call_message_handler(handler, raised)? {
                    Some(transformed) => transformed,
                    None => {
                        error_in_handler = true;
                        self.create_string("error in error handling")?
                    }
                },
                None => self.errormsg_object(raised)?,
            }
        };
        while self.call_depth() > initial_depth {
            self.pop_frame();
        }
        self.close_upvalues(level);
        match self.close_tbc_with_handler(level, err, handler) {
            Ok(()) => {}
            Err(LuaError::Yield) => {
                if initial_depth > 0 && initial_depth - 1 < self.call_depth() {
                    let ci = self.get_call_info_mut(initial_depth - 1);
                    ci.call_status |= CIST_YPCALL | CIST_RECST;
                }
                if !self.has_error_object() {
                    self.set_error_object(err);
                }
                return Err(LuaError::Yield);
            }
            Err(_) => err = self.take_error_object(),
        }
        // luaD_pcall: the error object goes at `level`, then luaD_shrinkstack
        self.shrink_stack_error_zone(level + 1);
        Ok((err, error_in_handler))
    }

    /// Unprotected call with stack-based arguments that supports yields.
    /// Like pcall_stack_based but errors propagate instead of being caught.
    /// Uses CIST_YCALL flag so finish_c_frame can properly move results after yield.
    /// Returns result_count on success; results are left on stack starting at func_idx.
    pub fn call_stack_based(&mut self, func_idx: usize, arg_count: usize) -> LuaResult<usize> {
        let initial_depth = self.call_depth();

        let (actual_arg_count, _ccmt_depth) = resolve_call_chain(self, func_idx, arg_count)?;

        let func = self
            .stack_get(func_idx)
            .ok_or_else(|| self.error("call: function not found".to_string()))?;

        let result = if func.is_c_callable() {
            call_c_function(self, func_idx, actual_arg_count, -1).map(|_| ())
        } else {
            let base = func_idx + 1;
            self.push_frame(&func, base, actual_arg_count, -1)?;
            self.inc_n_ccalls()?;
            let r = lua_execute(self, initial_depth);
            self.dec_n_ccalls();
            r
        };

        match result {
            Ok(()) => {
                let stack_top = self.get_top();
                let result_count = stack_top.saturating_sub(func_idx);
                Ok(result_count)
            }
            Err(LuaError::Yield) => {
                // Mark this C frame with CIST_YCALL so finish_c_frame
                // knows to move results (without prepending true/false).
                if initial_depth > 0 {
                    use crate::lua_vm::call_info::call_status::CIST_YCALL;
                    let frame_idx = initial_depth - 1;
                    if frame_idx < self.call_depth {
                        let ci = self.get_call_info_mut(frame_idx);
                        ci.call_status |= CIST_YCALL;
                    }
                }
                Err(LuaError::Yield)
            }
            Err(e) => Err(e), // Propagate all other errors
        }
    }

    /// Protected call with stack-based arguments (zero-allocation fast path)
    /// Args are already on stack at [arg_base, arg_base+arg_count)
    /// Returns (success, result_count) where results are left on stack
    pub fn pcall_stack_based(
        &mut self,
        func_idx: usize,
        arg_count: usize,
    ) -> LuaResult<(bool, usize)> {
        let (status, count) = self.pcall_stack_based_status(func_idx, arg_count)?;
        Ok((status == ProtectedCallStatus::Ok, count))
    }

    /// `pcall_stack_based`, telling a plain error apart from a failure of error handling
    /// itself (`LUA_ERRERR`: the stack overflowed inside its error zone).
    pub(crate) fn pcall_stack_based_status(
        &mut self,
        func_idx: usize,
        arg_count: usize,
    ) -> LuaResult<(ProtectedCallStatus, usize)> {
        // Save current call stack depth
        let initial_depth = self.call_depth();

        // Resolve __call metamethod chain if needed

        let (actual_arg_count, ccmt_depth) = match resolve_call_chain(self, func_idx, arg_count) {
            Ok((count, depth)) => (count, depth),
            Err(e) => {
                // __call resolution failed - return error
                let error_msg = self.get_error_message(e);
                let err_str = self.create_string(&error_msg)?;
                self.stack_set(func_idx, err_str)?;
                self.set_top(func_idx + 1)?;
                return Ok((ProtectedCallStatus::Error, 1));
            }
        };

        // Now func_idx contains a real callable (after __call resolution)
        let func = self
            .stack_get(func_idx)
            .ok_or_else(|| self.error("pcall: function not found after resolution".to_string()))?;

        // Call the function using the internal call machinery
        let result = if func.is_c_callable() {
            // C function - call directly
            call_c_function(
                self,
                func_idx,
                actual_arg_count,
                -1, // MULTRET - want all results
            )
            .map(|_| ())
        } else {
            // Lua function - push frame and execute, expecting all return values
            let base = func_idx + 1;
            self.push_frame(&func, base, actual_arg_count, -1)?;

            // Set ccmt count in call_status for Lua functions too
            if ccmt_depth > 0 {
                use crate::lua_vm::call_info::call_status;
                let frame_idx = self.call_depth - 1;
                if let Some(frame) = self.call_stack.get_mut(frame_idx) {
                    frame.call_status = call_status::set_ccmt_count(frame.call_status, ccmt_depth);
                }
            }

            self.inc_n_ccalls()?;
            let r = lua_execute(self, initial_depth);
            self.dec_n_ccalls();
            r
        };

        match result {
            Ok(()) => {
                // Success - count results from func_idx to logical stack top
                let stack_top = self.get_top();
                let result_count = stack_top.saturating_sub(func_idx);
                Ok((ProtectedCallStatus::Ok, result_count))
            }
            Err(LuaError::Yield) => {
                // Mark pcall's own C frame with CIST_YPCALL so that
                // finish_c_frame knows to wrap results with true on resume.
                // pcall's C frame is at initial_depth - 1.
                if initial_depth > 0 {
                    use crate::lua_vm::call_info::call_status::CIST_YPCALL;
                    let pcall_frame_idx = initial_depth - 1;
                    if pcall_frame_idx < self.call_depth {
                        let ci = self.get_call_info_mut(pcall_frame_idx);
                        ci.call_status |= CIST_YPCALL;
                    }
                }
                Err(LuaError::Yield)
            }
            Err(LuaError::CloseThread) => {
                // CloseThread bypasses all pcalls — propagate to resume
                Err(LuaError::CloseThread)
            }
            Err(e) => {
                let (result_err, error_in_handler) =
                    self.recover_protected_call(e, initial_depth, func_idx, None)?;
                self.stack_set(func_idx, result_err)?;
                self.set_top(func_idx + 1)?;
                let status = if error_in_handler {
                    ProtectedCallStatus::ErrorInHandler
                } else {
                    ProtectedCallStatus::Error
                };
                Ok((status, 1))
            }
        }
    }

    /// Protected call with error handler, stack-based (xpcall semantics).
    /// Like pcall_stack_based but calls the error handler at `handler_idx`
    /// BEFORE unwinding call frames, so debug.traceback can see the full stack.
    /// Returns (success, result_count) where results are left on stack at func_idx.
    pub fn xpcall_stack_based(
        &mut self,
        func_idx: usize,
        arg_count: usize,
        handler_idx: usize,
    ) -> LuaResult<(bool, usize)> {
        let (status, count) = self.xpcall_stack_based_status(func_idx, arg_count, handler_idx)?;
        Ok((status == ProtectedCallStatus::Ok, count))
    }

    /// `xpcall_stack_based`, telling a plain error apart from a failure of
    /// error handling itself (`LUA_ERRERR`).
    pub(crate) fn xpcall_stack_based_status(
        &mut self,
        func_idx: usize,
        arg_count: usize,
        handler_idx: usize,
    ) -> LuaResult<(ProtectedCallStatus, usize)> {
        let initial_depth = self.call_depth();

        let (actual_arg_count, ccmt_depth) = match resolve_call_chain(self, func_idx, arg_count) {
            Ok((count, depth)) => (count, depth),
            Err(e) => {
                let error_msg = self.get_error_message(e);
                let err_str = self.create_string(&error_msg)?;
                self.stack_set(func_idx, err_str)?;
                self.set_top(func_idx + 1)?;
                return Ok((ProtectedCallStatus::Error, 1));
            }
        };

        let func = self
            .stack_get(func_idx)
            .ok_or_else(|| self.error("xpcall: function not found after resolution".to_string()))?;

        let result = if func.is_c_callable() {
            call_c_function(self, func_idx, actual_arg_count, -1).map(|_| ())
        } else {
            let base = func_idx + 1;
            self.push_frame(&func, base, actual_arg_count, -1)?;
            if ccmt_depth > 0 {
                let frame_idx = self.call_depth - 1;
                if let Some(frame) = self.call_stack.get_mut(frame_idx) {
                    frame.call_status = call_status::set_ccmt_count(frame.call_status, ccmt_depth);
                }
            }
            self.inc_n_ccalls()?;
            let r = lua_execute(self, initial_depth);
            self.dec_n_ccalls();
            r
        };

        match result {
            Ok(()) => {
                let stack_top = self.get_top();
                let result_count = stack_top.saturating_sub(func_idx);
                Ok((ProtectedCallStatus::Ok, result_count))
            }
            Err(LuaError::Yield) => {
                if initial_depth > 0 {
                    let pcall_frame_idx = initial_depth - 1;
                    if pcall_frame_idx < self.call_depth {
                        let ci = self.get_call_info_mut(pcall_frame_idx);
                        ci.call_status |= CIST_YPCALL;
                    }
                }
                Err(LuaError::Yield)
            }
            Err(LuaError::CloseThread) => Err(LuaError::CloseThread),
            Err(e) => {
                let handler = self.stack_get(handler_idx).unwrap_or_default();
                let (final_error, error_in_handler) =
                    self.recover_protected_call(e, initial_depth, func_idx, Some(handler))?;
                self.stack_set(func_idx, final_error)?;
                self.set_top(func_idx + 1)?;
                let status = if error_in_handler {
                    ProtectedCallStatus::ErrorInHandler
                } else {
                    ProtectedCallStatus::Error
                };
                Ok((status, 1))
            }
        }
    }

    // ===== Coroutine Support (resume/yield) =====

    /// Resume a coroutine (should be called on the thread's LuaState)
    /// Returns (finished, results) where:
    /// - finished=true: coroutine completed normally
    /// - finished=false: coroutine yielded
    pub fn resume(&mut self, args: Vec<LuaValue>) -> LuaResult<(bool, Vec<LuaValue>)> {
        let me: *const LuaState = self;
        let outer = std::mem::replace(&mut self.global_state_mut().executing, me);
        let result = self.resume_thread(args);
        self.global_state_mut().executing = outer;
        result
    }

    fn resume_thread(&mut self, args: Vec<LuaValue>) -> LuaResult<(bool, Vec<LuaValue>)> {
        // Check coroutine state:
        // - dead flag set → dead by error (cannot resume)
        // - call_depth > 0 && !yielded → running (cannot resume)
        // - call_depth > 0 && yielded → suspended after yield (can resume)
        // - call_depth == 0 && stack not empty → initial state (can resume)
        // - call_depth == 0 && stack empty → dead (cannot resume)
        if self.dead {
            // Clear stale error_object so that this error uses the fresh
            // error_msg set by self.error(), rather than a leftover
            // error_object from a previous failed resume.
            self.clear_error();
            return Err(self.error("cannot resume dead coroutine".to_string()));
        }
        if self.call_depth > 0 && !self.yielded {
            self.clear_error();
            return Err(self.error("cannot resume non-suspended coroutine".to_string()));
        }

        // Mark as running (not yielded)
        self.yielded = false;

        // Check if this is the first resume (no active frames)
        if self.call_depth == 0 {
            // Initial resume - need to set up the function
            // The function should be at stack[0] (set by create_thread)
            if self.stack.is_empty() {
                self.clear_error(); // clear stale error object
                return Err(self.error("cannot resume dead coroutine".to_string()));
            }

            let func = self.stack[0];

            // Push arguments
            let old_capacity = self.stack.capacity();
            for arg in args {
                self.stack.push(arg);
            }
            // Fix open upvalue pointers if Vec was reallocated
            if self.stack.capacity() != old_capacity {
                self.fix_open_upvalue_pointers();
            }

            // Create initial frame, expecting all return values
            let nargs = self.stack.len() - 1; // -1 for function itself
            let base = 1; // Arguments start at index 1 (function is at 0)
            self.push_frame(&func, base, nargs, -1)?;

            // Execute until yield or completion
            let result = if func.is_c_callable() {
                // Call C function directly
                execute::call::call_c_function(self, 0, nargs, -1)
            } else {
                // Execute Lua bytecode
                self.inc_n_ccalls()?;
                let r = execute::lua_execute(self, 0);
                self.dec_n_ccalls();
                r
            };

            self.handle_resume_result(result)
        } else {
            // A directly-yielded C continuation keeps its frame and stack;
            // the resume values replace the yielded ones. Other yield
            // frames return normally and are popped before execution resumes.
            let (result_pos, nresults, pop_yield_frame) = if let Some(frame) = self.current_frame()
            {
                if frame.is_c() && frame.c_k_function != 0 {
                    (frame.c_k_func_index as usize, -1, false)
                } else {
                    (frame.func_index(), frame.nresults(), true)
                }
            } else {
                return Err(self.error("cannot resume: no frame".to_string()));
            };
            if pop_yield_frame {
                // The C function that yielded returns now (luaD_poscall in
                // resume), which fires its return hook.
                if self.hook_mask & crate::lua_vm::LUA_MASKRET != 0
                    && self.allow_hook
                    && self.current_frame().is_some_and(|frame| frame.is_c())
                {
                    self.run_hook(LUA_HOOKRET, -1, 0, 0)?;
                }
                self.pop_frame();
            }

            // Resume arguments replace the values returned by the yield point.
            let actual_nresults = args.len();
            for (i, arg) in args.into_iter().enumerate() {
                if nresults >= 0 && i >= nresults as usize {
                    break; // Don't place more than requested
                }
                self.stack_set(result_pos + i, arg)?;
            }

            // Adjust stack top based on expected nresults
            if nresults >= 0 {
                let wanted = nresults as usize;
                // Nil-pad if fewer results than expected
                for i in actual_nresults..wanted {
                    self.stack_set(result_pos + i, LuaValue::nil())?;
                }
                // Restore caller frame's top (matching call_c_function behavior)
                if self.call_depth() > 0 {
                    let ci_top = self.get_call_info(self.call_depth() - 1).top as usize;
                    self.set_top_raw(ci_top);
                } else {
                    self.set_top_raw(result_pos + wanted);
                }
            } else {
                let new_top = result_pos + actual_nresults;
                self.set_top_raw(new_top);
            }

            // Execute until yield or completion
            self.inc_n_ccalls()?;
            let result = execute::lua_execute(self, 0);
            self.dec_n_ccalls();

            // Handle result with pcall error recovery (precover)
            self.handle_resume_result(result)
        }
    }

    /// Handle the result of lua_execute during resume.
    /// Implements Lua 5.5's precover: when an error occurs, search the
    /// call stack for a pcall frame (CIST_YPCALL), recover there, and
    /// continue execution.
    fn handle_resume_result(
        &mut self,
        initial_result: LuaResult<()>,
    ) -> LuaResult<(bool, Vec<LuaValue>)> {
        let mut result = initial_result;

        loop {
            match result {
                Ok(()) => {
                    // Coroutine completed — pop any remaining frames
                    // (e.g., the initial frame pushed by resume for C functions)
                    let results = self.get_all_return_values(0);
                    while self.call_depth() > 0 {
                        self.pop_frame();
                    }
                    self.stack.clear();
                    self.stack_top = 0;
                    return Ok((true, results));
                }
                Err(LuaError::Yield) => {
                    // Coroutine yielded — mark as yielded for resume detection
                    self.yielded = true;
                    let yield_vals = self.take_yield();
                    return Ok((false, yield_vals));
                }
                Err(LuaError::CloseThread) => {
                    // Self-close: coroutine.close() closed TBC vars/upvalues
                    // and threw CloseThread to bypass all pcalls.
                    // Pop all remaining frames and mark thread as dead.
                    while self.call_depth() > 0 {
                        self.pop_frame();
                    }
                    // Check if __close set an error
                    let err_obj = self.take_error_object();
                    self.stack.clear();
                    self.stack_top = 0;
                    if err_obj.is_nil() {
                        // Normal close — success with no return values
                        return Ok((true, vec![]));
                    } else {
                        // __close errored — coroutine dies with error.
                        // Store error back so coroutine_resume can retrieve it.
                        self.set_error_object(err_obj);
                        return Err(LuaError::RuntimeError);
                    }
                }
                Err(_e) => {
                    // Error — try to find a pcall frame to recover
                    let pcall_idx = self.find_pcall_recovery_frame();
                    if pcall_idx.is_none() {
                        // No recovery point — coroutine dies.
                        // Keep stack and call frames intact for debug.traceback
                        // (matching C Lua behavior: dead coroutines retain their
                        // call stack for inspection).
                        let raised = self.caught_error_value(_e)?;
                        let error_val = self.errormsg_object(raised)?;

                        // Close all upvalues and TBC variables from level 0
                        self.close_upvalues(0);
                        let _ = self.close_tbc_with_error(0, error_val);

                        let archived_error = if self.has_error_object() {
                            ErrorMsg::Object(self.take_error_object())
                        } else {
                            let msg = self.take_error_msg_raw();
                            if msg.is_empty() {
                                ErrorMsg::Object(error_val)
                            } else {
                                ErrorMsg::Msg(msg)
                            }
                        };
                        self.archive_dead_error(archived_error);

                        // Mark coroutine as dead but keep stack for debug.traceback
                        self.dead = true;

                        return Err(_e);
                    }
                    let pcall_frame_idx = pcall_idx.unwrap();

                    // Get pcall's info before cleanup
                    let pcall_ci = self.get_call_info(pcall_frame_idx);
                    let pcall_func_pos = pcall_ci.func_index();
                    let pcall_nresults = pcall_ci.nresults();
                    let has_c_continuation = pcall_ci.c_k_function != 0;
                    let continuation_func_pos = pcall_ci.c_k_func_index as usize;
                    let close_level = if has_c_continuation {
                        continuation_func_pos.saturating_add(1)
                    } else {
                        pcall_ci.base
                    };
                    let is_xpcall = pcall_ci.call_status & CIST_XPCALL != 0;

                    // Save the xpcall handler before anything overwrites it.
                    let xpcall_handler = if is_xpcall {
                        let handler_pos =
                            if has_c_continuation && pcall_ci.c_k_error_func_index != u32::MAX {
                                pcall_ci.c_k_error_func_index as usize
                            } else {
                                // stdlib xpcall keeps the handler at its second argument
                                pcall_func_pos + 2
                            };
                        self.stack_get(handler_pos).unwrap_or_default()
                    } else {
                        LuaValue::nil()
                    };

                    // The message handler runs before anything is unwound
                    // (luaG_errormsg), then the callee frames are popped and
                    // the pending to-be-closed variables closed with the error.
                    let raised = self.caught_error_value(_e)?;
                    let mut error_in_handler = false;
                    let error_val = if is_xpcall {
                        match self.call_message_handler(xpcall_handler, raised)? {
                            Some(transformed) => transformed,
                            None => {
                                error_in_handler = true;
                                self.create_string("error in error handling")?
                            }
                        }
                    } else {
                        self.errormsg_object(raised)?
                    };

                    // Pop frames down to pcall (exclusive — keep pcall's frame temporarily)
                    while self.call_depth() > pcall_frame_idx + 1 {
                        self.pop_frame();
                    }
                    self.close_upvalues(close_level);
                    let handler = is_xpcall.then_some(xpcall_handler);
                    let result_err = match self.close_tbc_with_handler(close_level, error_val, handler)
                    {
                        Ok(()) => error_val,
                        Err(LuaError::Yield) => {
                            // A __close yielded during error recovery: mark the
                            // pcall frame so finish_c_frame sets up the error
                            // result once the remaining variables are closed.
                            if pcall_frame_idx < self.call_depth() {
                                let ci = self.get_call_info_mut(pcall_frame_idx);
                                ci.call_status |= CIST_RECST;
                            }
                            if !self.has_error_object() {
                                self.set_error_object(error_val);
                            }
                            self.yielded = true;
                            let yield_vals = self.take_yield();
                            return Ok((false, yield_vals));
                        }
                        Err(_) => self.take_error_object(),
                    };

                    if has_c_continuation {
                        // lua_pcallk resumes its continuation with the
                        // error object in the protected call's result slot.
                        self.stack_set(continuation_func_pos, result_err)?;
                        self.set_top_raw(continuation_func_pos + 1);
                        let frame = self.get_call_info_mut(pcall_frame_idx);
                        // LUA_ERRERR or LUA_ERRRUN
                        frame.c_k_status = if error_in_handler { 6 } else { 2 };
                        frame.call_status &= !CIST_RECST;
                    } else {
                        // Set up stdlib pcall error result: (false, error).
                        self.stack_set(pcall_func_pos, LuaValue::boolean(false))?;
                        self.stack_set(pcall_func_pos + 1, result_err)?;
                        let n = 2;
                        self.pop_frame();

                        let final_n = if pcall_nresults == -1 {
                            n
                        } else {
                            pcall_nresults as usize
                        };
                        let new_top = pcall_func_pos + final_n;
                        if pcall_nresults >= 0 {
                            let wanted = pcall_nresults as usize;
                            for i in n..wanted {
                                self.stack_set(pcall_func_pos + i, LuaValue::nil())?;
                            }
                        }
                        self.set_top_raw(new_top);

                        if self.call_depth() > 0 {
                            let ci_idx = self.call_depth() - 1;
                            if pcall_nresults == -1 {
                                let ci_top = self.get_call_info(ci_idx).top as usize;
                                if ci_top < new_top {
                                    self.get_call_info_mut(ci_idx).top = new_top as u32;
                                }
                            } else {
                                let frame_top = self.get_call_info(ci_idx).top as usize;
                                self.set_top_raw(frame_top);
                            }
                        }
                    }

                    // Continue execution
                    if let Err(e) = self.inc_n_ccalls() {
                        result = Err(e);
                    } else {
                        result = execute::lua_execute(self, 0);
                        self.dec_n_ccalls();
                    }
                    // Loop again to check for more errors/yields
                }
            }
        }
    }

    /// Find a pcall C frame (CIST_YPCALL) on the call stack for error recovery.
    /// Returns the frame index if found.
    fn find_pcall_recovery_frame(&self) -> Option<usize> {
        for i in (0..self.call_depth()).rev() {
            let ci = self.get_call_info(i);
            if ci.call_status & CIST_YPCALL != 0 {
                return Some(i);
            }
        }
        None
    }

    /// Yield from current coroutine
    /// This should be called by Lua code via coroutine.yield
    pub fn do_yield(&mut self, values: Vec<LuaValue>) -> LuaResult<()> {
        self.set_yield(values);
        Err(LuaError::Yield)
    }

    // ============ GC Barriers ============

    /// Forward GC barrier (luaC_barrier in Lua 5.5)
    /// Called when modifying an object to point to another object
    #[inline(always)]
    pub(crate) fn gc_barrier(&mut self, upvalue_ptr: UpvaluePtr, value_gc_ptr: GcObjectPtr) {
        let owner_ptr = GcObjectPtr::from(upvalue_ptr);
        self.global_state
            .gc_barrier(self as *mut LuaState, owner_ptr, value_gc_ptr);
    }

    /// Backward GC barrier (luaC_barrierback in Lua 5.5)
    /// Called when modifying a BLACK object (typically table) with new values
    /// Instead of marking the value, re-gray the object for re-traversal
    #[inline(always)]
    pub(crate) fn gc_barrier_back(&mut self, gc_ptr: GcObjectPtr) {
        let vm = self.global_state_mut();
        vm.gc.barrier_back(gc_ptr);
    }

    /// Track table memory change from a resize delta.
    #[inline]
    pub(crate) fn gc_track_table_resize(&mut self, table_ptr: TablePtr, delta: isize) {
        let vm = self.global_state_mut();
        vm.gc.track_resize(table_ptr, delta);
    }

    #[inline(always)]
    pub(crate) fn check_gc_in_loop(
        &mut self,
        ci: &mut CallInfo,
        pc: usize,
        c: usize,
        trap: &mut bool,
    ) {
        if self.global_state.gc_debt() > 0 {
            return;
        }

        ci.save_pc(pc);
        self.set_top_raw(c);
        self.global_state.check_gc(self as *mut LuaState);
        *trap = self.hook_mask != 0;
    }

    #[inline(always)]
    pub(crate) fn check_gc_safe_point(&mut self) {
        if self.global_state.gc_debt() > 0 {
            return;
        }
        self.global_state.check_gc(self as *mut LuaState);
    }

    pub fn collect_garbage(&mut self) -> LuaResult<()> {
        let global = self.global_state;
        global.full_gc(self as *mut LuaState, false);
        if let Some(message) = global.take_finalizer_error() {
            return Err(self.error(message));
        }
        Ok(())
    }

    pub(crate) fn change_gc_mode(&mut self, kind: GcKind) {
        self.global_state
            .change_gc_mode(self as *mut LuaState, kind);
    }

    #[cfg(feature = "sandbox")]
    #[inline(always)]
    pub(crate) fn has_active_instruction_watch(&self) -> bool {
        self.hook_mask != 0 || self.sandbox_limits.is_some()
    }

    #[cfg(feature = "sandbox")]
    pub(crate) fn check_sandbox_runtime_limits(&mut self) -> LuaResult<()> {
        let Some(mut limits) = self.sandbox_limits else {
            return Ok(());
        };

        if let Some(remaining) = limits.remaining_instructions {
            if remaining == 0 {
                return Err(self.error("sandbox instruction limit exceeded".to_string()));
            }
            limits.remaining_instructions = Some(remaining - 1);
        }

        if let Some(deadline_nanos) = limits.deadline_nanos {
            if limits.instructions_until_time_check == 0 {
                limits.instructions_until_time_check = SANDBOX_TIMEOUT_CHECK_INTERVAL;
                if unix_nanos() >= deadline_nanos {
                    return Err(self.error("sandbox timeout exceeded".to_string()));
                }
            } else {
                limits.instructions_until_time_check -= 1;
            }
        }

        self.sandbox_limits = Some(limits);
        Ok(())
    }

    #[cfg(feature = "sandbox")]
    pub(crate) fn with_sandbox_runtime_limits<T, F>(
        &mut self,
        limits: Option<SandboxRuntimeLimits>,
        f: F,
    ) -> LuaResult<T>
    where
        F: FnOnce(&mut LuaState) -> LuaResult<T>,
    {
        let saved_limits = self.sandbox_limits;
        let saved_memory_limit = self.global_state_mut().gc.temporary_memory_limit();

        self.sandbox_limits = limits;

        if let Some(memory_limit_bytes) = limits.and_then(|limit| limit.memory_limit_bytes) {
            self.global_state_mut()
                .gc
                .set_temporary_memory_limit(memory_limit_bytes);
        }

        let result = f(self);

        self.sandbox_limits = saved_limits;
        self.global_state_mut()
            .gc
            .restore_temporary_memory_limit(saved_memory_limit);

        result
    }

    // ===== Debug Hook Support =====

    /// Invoke the debug hook function for the given event.
    /// This is called from the execute loop at hook check points.
    ///
    /// Mirrors C Lua's luaD_hook: saves/restores stack_top, sets allow_hook
    /// to false during the hook call, increments nny (hooks are non-yieldable).
    ///
    /// # Arguments
    /// * `event` - One of LUA_HOOKCALL, LUA_HOOKRET, LUA_HOOKLINE, etc.
    /// * `line` - Current line number (meaningful for LUA_HOOKLINE, -1 otherwise)
    /// * `ftransfer` - 1-based index of first transferred value (0 for line/count hooks)
    /// * `ntransfer` - number of values being transferred (0 for line/count hooks)
    #[cold]
    pub fn run_hook(
        &mut self,
        event: i32,
        line: i32,
        ftransfer: i32,
        ntransfer: i32,
    ) -> LuaResult<()> {
        let hook = self.hook;
        if hook.is_nil() || !self.allow_hook {
            return Ok(());
        }

        let global_state = self.global_state_mut();

        // Get the cached event name string from ConstString (no allocation)
        let event_name = match event {
            LUA_HOOKCALL => global_state.const_strings.str_hook_call,
            LUA_HOOKRET => global_state.const_strings.str_hook_return,
            LUA_HOOKLINE => global_state.const_strings.str_hook_line,
            LUA_HOOKCOUNT => global_state.const_strings.str_hook_count,
            LUA_HOOKTAILCALL => global_state.const_strings.str_hook_tail_call,
            _ => return Ok(()),
        };

        // Save and restore allow_hook (like C Lua's luaD_hook)
        let old_allow_hook = self.allow_hook;
        self.allow_hook = false;
        // Hooks are non-yieldable
        self.nny += 1;
        // Store transfer info for debug.getinfo 'r' option
        self.ftransfer = ftransfer;
        self.ntransfer = ntransfer;
        // Save stack_top (restored after hook returns, like C Lua's luaD_hook)
        let old_top = self.stack_top;

        // Protect the current frame's registers: ensure stack_top >= ci.top
        // so that hook function/args are placed ABOVE the current frame.
        // This matches C Lua's luaD_hook: "if (isLua(ci) && L->top < ci->top)
        //   L->top = ci->top; /* protect entire activation register */"
        let ci_idx = self.call_depth().wrapping_sub(1);
        if ci_idx < self.call_depth() {
            let ci_top = self.get_call_info(ci_idx).top as usize;
            if self.stack_top < ci_top {
                self.stack_top = ci_top;
            }
        }

        // Mark current frame as hooked (for debug.getinfo namewhat="hook")
        let ci_idx = self.call_depth().wrapping_sub(1);
        if ci_idx < self.call_depth() {
            let ci = self.get_call_info_mut(ci_idx);
            ci.call_status |= call_status::CIST_HOOKED;
        }

        // Build args: (event_name, line_or_nil)
        let line_val = if line >= 0 {
            LuaValue::integer(line as i64)
        } else {
            LuaValue::nil()
        };

        let result = self.call(hook, vec![event_name, line_val]);

        // Clear hooked flag
        if ci_idx < self.call_depth() {
            let ci = self.get_call_info_mut(ci_idx);
            ci.call_status &= !CIST_HOOKED;
        }

        // Restore state
        self.stack_top = old_top;
        self.nny -= 1;
        self.allow_hook = old_allow_hook;

        result.map(|_| ())
    }

    pub fn to_string(&mut self, value: &LuaValue) -> LuaResult<String> {
        // Fast path: simple types without metamethods
        match value.kind() {
            LuaValueKind::Nil => return Ok("nil".to_string()),
            LuaValueKind::Boolean => {
                if let Some(b) = value.as_boolean() {
                    return Ok(if b {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    });
                }
            }
            LuaValueKind::Integer => {
                if let Some(n) = value.as_integer() {
                    return Ok(n.to_string());
                }
            }
            LuaValueKind::Float => {
                if let Some(n) = value.as_number() {
                    return Ok(crate::stdlib::numfmt::lua_float_to_string(n, self.global_state().language()));
                }
            }
            LuaValueKind::String => {
                if let Some(s) = value.as_str() {
                    return Ok(s.to_string());
                }
                if let Some(bytes) = value.as_bytes() {
                    return Ok(String::from_utf8_lossy(bytes).into_owned());
                }
            }
            LuaValueKind::Function | LuaValueKind::CFunction => {
                // Functions: use default representation (no metatable support for functions)
                return Ok(format!("{}", value));
            }
            _ => {
                // Check for trait-based __tostring on userdata
                if value.ttisfulluserdata()
                    && let Some(ud) = value.as_userdata_mut()
                    && let Some(s) = ud.get_trait().lua_tostring()
                {
                    return Ok(s);
                }
                // Check for __tostring metamethod
                if let Some(mm) = get_metamethod_event(self, value, TmKind::ToString) {
                    let result = execute::call_tm_res1(self, mm, *value)?;
                    if let Some(s) = result.as_str() {
                        return Ok(s.to_string());
                    }
                    return Err(self.error("'__tostring' must return a string".to_string()));
                }
            }
        }

        // Fallback: generic representation
        // Check for __name in metatable (luaT_objtypename)
        let type_prefix = objtypename(self, value);
        if type_prefix != value.type_name() {
            // Use __name as prefix instead of built-in type name
            Ok(format!(
                "{}: 0x{:x}",
                type_prefix,
                value.raw_ptr_repr() as usize
            ))
        } else {
            Ok(format!("{}", value))
        }
    }

    pub fn is_main_thread(&self) -> bool {
        self.is_main
    }

    /// Check if this coroutine has yielded and is waiting to be resumed.
    pub fn is_yielded(&self) -> bool {
        self.yielded
    }

    #[inline(always)]
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    // ========================================================================
    // Debugger API — public methods for external debugger integration
    // ========================================================================

    /// Set a Lua hook function with the given mask and count.
    /// This is the programmatic equivalent of `debug.sethook`.
    pub fn set_hook(&mut self, hook: LuaValue, mask: u8, count: i32) {
        self.hook = hook;
        self.hook_mask = mask;
        self.base_hook_count = count;
        self.hook_count = count;
    }

    /// Get the current hook mask (bitmask of LUA_MASKCALL/RET/LINE/COUNT).
    pub fn hook_mask(&self) -> u8 {
        self.hook_mask
    }

    /// Get the current hook function value.
    pub fn hook_func(&self) -> LuaValue {
        self.hook
    }

    /// Get the source name of the function at the given stack level (0 = current).
    /// This is a fast path that only reads the chunk's source_name without
    /// building a full DebugInfo. Returns `None` for C functions or invalid levels.
    pub fn get_source(&self, level: usize) -> Option<String> {
        let call_depth = self.call_depth();
        if level >= call_depth {
            return None;
        }
        let frame_idx = call_depth - 1 - level;
        let func = self.get_frame_func(frame_idx)?;
        let lua_func = func.as_lua_function()?;
        lua_func
            .chunk()
            .source_name
            .as_ref()
            .map(|name| name.as_ref().to_string())
    }

    /// Get a local variable name and value at the given stack level and index.
    /// `level` is 0-based (0 = current frame). `local_idx` is 1-based.
    /// Returns `None` if the level/index is out of range.
    pub fn get_local(&self, level: usize, local_idx: usize) -> Option<(Box<[u8]>, LuaValue)> {
        let call_depth = self.call_depth();
        if level >= call_depth {
            return None;
        }
        let frame_idx = call_depth - 1 - level;
        let func = self.get_frame_func(frame_idx)?;
        let lua_func = func.as_lua_function()?;
        let chunk = lua_func.chunk();
        let ci = self.get_frame(frame_idx)?;
        let pc = if ci.pc > 0 { ci.pc as usize - 1 } else { 0 };
        let base = ci.base;

        // Find the n-th active local variable at this PC
        let mut active_count = 0usize;
        for locvar in &chunk.locals {
            if (locvar.startpc as usize) > pc {
                break;
            }
            if pc < locvar.endpc as usize {
                active_count += 1;
                if active_count == local_idx {
                    let reg = active_count - 1;
                    let value = self.stack_get(base + reg).unwrap_or_default();
                    return Some((locvar.name.clone(), value));
                }
            }
        }
        None
    }

    /// Count the number of active local variables at the given stack level.
    pub fn local_count(&self, level: usize) -> usize {
        let call_depth = self.call_depth();
        if level >= call_depth {
            return 0;
        }
        let frame_idx = call_depth - 1 - level;
        let func = match self.get_frame_func(frame_idx) {
            Some(f) => f,
            None => return 0,
        };
        let lua_func = match func.as_lua_function() {
            Some(f) => f,
            None => return 0,
        };
        let chunk = lua_func.chunk();
        let ci = match self.get_frame(frame_idx) {
            Some(c) => c,
            None => return 0,
        };
        let pc = if ci.pc > 0 { ci.pc as usize - 1 } else { 0 };

        let mut count = 0usize;
        for locvar in &chunk.locals {
            if (locvar.startpc as usize) > pc {
                break;
            }
            if pc < locvar.endpc as usize {
                count += 1;
            }
        }
        count
    }

    /// Get an upvalue name and value for the function at the given stack level.
    /// `level` is 0-based. `up_idx` is 1-based.
    pub fn get_upvalue(&self, level: usize, up_idx: usize) -> Option<(Box<[u8]>, LuaValue)> {
        let call_depth = self.call_depth();
        if level >= call_depth || up_idx == 0 {
            return None;
        }
        let frame_idx = call_depth - 1 - level;
        let func = self.get_frame_func(frame_idx)?;
        let lua_func = func.as_lua_function()?;
        let upvalues = lua_func.upvalues();
        let chunk = lua_func.chunk();
        let idx = up_idx - 1;
        if idx >= upvalues.len() || idx >= chunk.upvalue_descs.len() {
            return None;
        }
        let name = chunk.upvalue_descs[idx].name.clone();
        let value = upvalues[idx].as_ref().data.get_value();
        Some((name, value))
    }

    /// Count the number of upvalues for the function at the given stack level.
    pub fn upvalue_count(&self, level: usize) -> usize {
        let call_depth = self.call_depth();
        if level >= call_depth {
            return 0;
        }
        let frame_idx = call_depth - 1 - level;
        let func = match self.get_frame_func(frame_idx) {
            Some(f) => f,
            None => return 0,
        };
        let lua_func = match func.as_lua_function() {
            Some(f) => f,
            None => return 0,
        };
        lua_func.upvalues().len()
    }

    pub fn to_any_ref(&mut self, value: LuaValue) -> LuaAnyRef {
        self.global_state_mut().to_ref(value)
    }

    pub fn to_table_ref(&mut self, value: LuaValue) -> Option<LuaTableRef> {
        self.global_state_mut().to_table_ref(value)
    }

    pub fn to_function_ref(&mut self, value: LuaValue) -> Option<LuaFunctionRef> {
        self.global_state_mut().to_function_ref(value)
    }

    pub fn to_string_ref(&mut self, value: LuaValue) -> Option<LuaStringRef> {
        self.global_state_mut().to_string_ref(value)
    }

    pub fn to_userdata_ref<T: 'static>(&mut self, value: LuaValue) -> Option<UserDataRef<T>> {
        self.global_state_mut().to_userdata_ref(value)
    }
}
