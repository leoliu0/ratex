// Debug library implementation
// Implements: traceback, getinfo, getlocal, getmetatable, getupvalue, etc.

use crate::lib_registry::LibraryModule;
use crate::lua_value::block_userdata::{set_userdata_uservalue, userdata_uservalue};
use crate::lua_value::{LuaProto, LuaValue};
use crate::lua_vm::call_info::call_status;
use crate::lua_vm::opcode::OpCode;
use crate::lua_vm::{LuaError, LuaResult, LuaState, TmKind, get_metatable};
use crate::stdlib::lauxlib;
use crate::{Instruction, LUA_MASKCALL, LUA_MASKCOUNT, LUA_MASKLINE, LUA_MASKRET, lib_module};

/// Get the type name of an object, checking __name in metatable first.
/// Mirrors C Lua's luaT_objtypename.
pub fn objtypename(l: &mut LuaState, v: &LuaValue) -> String {
    metatable_name(l, v).unwrap_or_else(|| v.type_name().to_string())
}

/// The string `__name` field of the metatable of `v`, if any.
pub(crate) fn metatable_name(l: &mut LuaState, v: &LuaValue) -> Option<String> {
    let mt = get_metatable(l, v)?;
    let mt_table = mt.as_table()?;
    let key = l.create_string("__name").ok()?;
    mt_table.raw_get(&key)?.as_str().map(str::to_string)
}

// ============================================================================
// Function name resolution (mirrors Lua 5.5 ldebug.c)
// ============================================================================

/// Get the name of the Nth active local variable at the given PC.
/// Mirrors Lua 5.5's luaF_getlocalname.
/// local_number is 1-based.
fn getlocalname(chunk: &LuaProto, local_number: usize, pc: usize) -> Option<&str> {
    let mut n = local_number;
    for locvar in &chunk.locals {
        if (locvar.startpc as usize) > pc {
            break;
        }
        if pc < locvar.endpc as usize {
            n -= 1;
            if n == 0 {
                return Some(&locvar.name);
            }
        }
    }
    None
}

/// Whether the opcode writes to register A (testAMode)
fn test_a_mode(op: OpCode) -> bool {
    matches!(
        op,
        OpCode::Move
            | OpCode::LoadI
            | OpCode::LoadF
            | OpCode::LoadK
            | OpCode::LoadKX
            | OpCode::LoadFalse
            | OpCode::LFalseSkip
            | OpCode::LoadTrue
            | OpCode::LoadNil
            | OpCode::GetUpval
            | OpCode::GetTabUp
            | OpCode::GetTable
            | OpCode::GetI
            | OpCode::GetField
            | OpCode::NewTable
            | OpCode::Self_
            | OpCode::AddI
            | OpCode::AddK
            | OpCode::SubK
            | OpCode::MulK
            | OpCode::ModK
            | OpCode::PowK
            | OpCode::DivK
            | OpCode::IDivK
            | OpCode::BAndK
            | OpCode::BOrK
            | OpCode::BXorK
            | OpCode::ShlI
            | OpCode::ShrI
            | OpCode::Add
            | OpCode::Sub
            | OpCode::Mul
            | OpCode::Mod
            | OpCode::Pow
            | OpCode::Div
            | OpCode::IDiv
            | OpCode::BAnd
            | OpCode::BOr
            | OpCode::BXor
            | OpCode::Shl
            | OpCode::Shr
            | OpCode::Unm
            | OpCode::BNot
            | OpCode::Not
            | OpCode::Len
            | OpCode::Concat
            | OpCode::TestSet
            | OpCode::Call
            | OpCode::TailCall
            | OpCode::ForLoop
            | OpCode::ForPrep
            | OpCode::TForLoop
            | OpCode::Closure
            | OpCode::Vararg
            | OpCode::GetVarg
            | OpCode::VarargPrep
    )
}

/// Whether the opcode is a metamethod instruction (OP_MMBIN*)
fn test_mm_mode(op: OpCode) -> bool {
    matches!(op, OpCode::MmBin | OpCode::MmBinI | OpCode::MmBinK)
}

/// Get the upvalue name from chunk
fn upvalname(chunk: &LuaProto, uv: usize) -> String {
    if uv < chunk.upvalue_descs.len() {
        chunk.upvalue_descs[uv].name.clone()
    } else {
        "?".to_string()
    }
}

/// Get a constant name (if it's a string)
fn kname(chunk: &LuaProto, index: usize) -> Option<String> {
    if index < chunk.constants.len()
        && let Some(s) = chunk.constants[index].as_str()
    {
        return Some(s.to_string());
    }
    None
}

/// Find the last instruction before lastpc that sets register reg.
/// Returns -1 if not found.
/// Mirrors Lua 5.5's findsetreg.
fn findsetreg(chunk: &LuaProto, lastpc: usize, reg: u32) -> i32 {
    let mut setreg: i32 = -1;
    let mut jmptarget: usize = 0;

    // If the instruction at lastpc is an MM-mode instruction, back up one
    let lastpc = if lastpc < chunk.code.len() && test_mm_mode(chunk.code[lastpc].get_opcode()) {
        lastpc.saturating_sub(1)
    } else {
        lastpc
    };

    for pc in 0..lastpc {
        let i = chunk.code[pc];
        let op = i.get_opcode();
        let a = i.get_a();

        let change = match op {
            OpCode::LoadNil => reg >= a && reg <= a + i.get_b(),
            OpCode::TForCall => reg >= a + 2,
            OpCode::Call | OpCode::TailCall => reg >= a,
            OpCode::Jmp => {
                let b = i.get_sj();
                let dest = (pc as i32 + 1 + b) as usize;
                if dest <= lastpc && dest > jmptarget {
                    jmptarget = dest;
                }
                false
            }
            _ => test_a_mode(op) && reg == a,
        };

        if change {
            // filterpc: if inside a jump target region, discard
            setreg = if pc < jmptarget { -1 } else { pc as i32 };
        }
    }
    setreg
}

/// Basic object name resolution.
/// Returns (kind, name) or None.
/// Mirrors Lua 5.5's basicgetobjname.
fn basicgetobjname(chunk: &LuaProto, pc: &mut i32, reg: u32) -> Option<(&'static str, String)> {
    let pc_val = *pc as usize;

    // First try: is reg a local variable at this PC?
    if let Some(name) = getlocalname(chunk, (reg + 1) as usize, pc_val) {
        return Some(("local", name.to_string()));
    }

    // Symbolic execution: find the instruction that set this register
    let setreg_pc = findsetreg(chunk, pc_val, reg);
    *pc = setreg_pc;

    if setreg_pc >= 0 {
        let i = chunk.code[setreg_pc as usize];
        let op = i.get_opcode();

        match op {
            OpCode::Move => {
                let b = i.get_b();
                if b < i.get_a() {
                    return basicgetobjname(chunk, pc, b);
                }
            }
            OpCode::GetUpval => {
                let b = i.get_b() as usize;
                let name = upvalname(chunk, b);
                return Some(("upvalue", name));
            }
            OpCode::LoadK => {
                let bx = i.get_bx() as usize;
                if let Some(name) = kname(chunk, bx) {
                    return Some(("constant", name));
                }
            }
            OpCode::LoadKX if (setreg_pc as usize + 1) < chunk.code.len() => {
                let ax = chunk.code[setreg_pc as usize + 1].get_ax() as usize;
                if let Some(name) = kname(chunk, ax) {
                    return Some(("constant", name));
                }
            }
            _ => {}
        }
    }
    None
}

/// Get a register name for rname helper
fn rname(chunk: &LuaProto, pc: usize, c: u32) -> String {
    let mut ppc = pc as i32;
    if let Some((kind, name)) = basicgetobjname(chunk, &mut ppc, c)
        && kind == "constant"
    {
        return name;
    }
    "?".to_string()
}

/// Check if the table operand names _ENV (making it a "global")
fn is_env(chunk: &LuaProto, pc: usize, i: Instruction, isup: bool) -> &'static str {
    let t = i.get_b();
    let name = if isup {
        Some(upvalname(chunk, t as usize))
    } else {
        let mut ppc = pc as i32;
        match basicgetobjname(chunk, &mut ppc, t) {
            Some(("local", name)) | Some(("upvalue", name)) => Some(name),
            _ => None,
        }
    };
    match name {
        Some(ref n) if n == "_ENV" => "global",
        _ => "field",
    }
}

/// Extended object name resolution (handles table accesses).
/// Mirrors Lua 5.5's getobjname.
fn getobjname(
    chunk: &LuaProto,
    lastpc: usize,
    reg: u32,
    lua53: bool,
) -> Option<(&'static str, String)> {
    let mut pc = lastpc as i32;
    if let Some(result) = basicgetobjname(chunk, &mut pc, reg) {
        return Some(result);
    }
    if pc >= 0 {
        let i = chunk.code[pc as usize];
        match i.get_opcode() {
            OpCode::GetTabUp => {
                let k = i.get_c() as usize;
                let name = kname(chunk, k).unwrap_or_else(|| "?".to_string());
                let kind = is_env(chunk, pc as usize, i, true);
                return Some((kind, name));
            }
            OpCode::GetTable => {
                let k = i.get_c();
                let name = rname(chunk, pc as usize, k);
                // A method call whose name constant does not fit an RK operand compiles
                // to MOVE + GETTABLE; Lua 5.3 still emitted OP_SELF for it.
                let is_method = lua53 && pc > 0 && {
                    let setup = chunk.code[pc as usize - 1];
                    setup.get_opcode() == OpCode::Move
                        && setup.get_a() == i.get_a() + 1
                        && setup.get_b() == i.get_b()
                };
                let kind = if is_method {
                    "method"
                } else {
                    is_env(chunk, pc as usize, i, false)
                };
                return Some((kind, name));
            }
            OpCode::GetI => {
                return Some(("field", "integer index".to_string()));
            }
            OpCode::GetField => {
                let k = i.get_c() as usize;
                let field_name = kname(chunk, k).unwrap_or_else(|| "?".to_string());
                let kind = is_env(chunk, pc as usize, i, false);
                return Some((kind, field_name));
            }
            OpCode::Self_ => {
                let k = i.get_c() as usize;
                let name = kname(chunk, k).unwrap_or_else(|| "?".to_string());
                return Some(("method", name));
            }
            _ => {}
        }
    }
    None
}

/// Determine function name from bytecode at the calling instruction.
/// Mirrors Lua 5.5's funcnamefromcode.
fn funcnamefromcode(chunk: &LuaProto, pc: usize, lua53: bool) -> Option<(&'static str, String)> {
    if pc >= chunk.code.len() {
        return None;
    }
    let i = chunk.code[pc];
    match i.get_opcode() {
        OpCode::Call | OpCode::TailCall => getobjname(chunk, pc, i.get_a(), lua53),
        OpCode::TForCall | OpCode::TForCall53 => Some(("for iterator", "for iterator".to_string())),
        // Metamethod-triggering instructions
        OpCode::Self_ | OpCode::GetTabUp | OpCode::GetTable | OpCode::GetI | OpCode::GetField => {
            Some(("metamethod", "__index".to_string()))
        }
        OpCode::SetTabUp | OpCode::SetTable | OpCode::SetI | OpCode::SetField => {
            Some(("metamethod", "__newindex".to_string()))
        }
        OpCode::MmBin | OpCode::MmBinI | OpCode::MmBinK => {
            let tm = TmKind::from_u8(i.get_c() as u8);
            Some(("metamethod", tm.name().to_string()))
        }
        OpCode::Unm => Some(("metamethod", "__unm".to_string())),
        OpCode::BNot => Some(("metamethod", "__bnot".to_string())),
        OpCode::Len => Some(("metamethod", "__len".to_string())),
        OpCode::Concat => Some(("metamethod", "__concat".to_string())),
        OpCode::Eq => Some(("metamethod", "__eq".to_string())),
        OpCode::Lt | OpCode::LtI | OpCode::GtI => Some(("metamethod", "__lt".to_string())),
        OpCode::Le | OpCode::LeI | OpCode::GeI => Some(("metamethod", "__le".to_string())),
        OpCode::Close | OpCode::Return => Some(("metamethod", "__close".to_string())),
        _ => None,
    }
}

/// Get function name by looking at the calling frame.
/// Mirrors Lua 5.5's getfuncname/funcnamefromcall (5.3: getfuncname/funcnamefromcode).
/// ci_frame_idx is the frame index of the TARGET function.
fn getfuncname(l: &LuaState, ci_frame_idx: usize) -> Option<(&'static str, String)> {
    let lua53 = l.global_state().language() == crate::LuaLanguageLevel::Lua53;
    let ci = l.get_frame(ci_frame_idx)?;
    // GCTM flags the frame that was running when the finalizer started. Lua 5.3 reports
    // that frame itself as the "__gc" metamethod; 5.4+ report the finalizer it called.
    if lua53 && ci.call_status & call_status::CIST_FIN != 0 {
        return Some(("metamethod", "__gc".to_string()));
    }
    if ci_frame_idx == 0 {
        return None; // No caller frame
    }
    // If tail call, cannot find name
    if ci.is_tail() {
        return None;
    }
    // Look at the immediately previous frame (the caller)
    let prev_idx = ci_frame_idx - 1;
    let prev = l.get_frame(prev_idx)?;
    if prev.call_status & call_status::CIST_HOOKED != 0 {
        return Some(("hook", "?".to_string()));
    }
    if !lua53 && prev.call_status & call_status::CIST_FIN != 0 {
        return Some(("metamethod", "__gc".to_string()));
    }
    if prev.is_lua() {
        // Get caller's chunk
        let prev_func = l.get_frame_func(prev_idx)?;
        let lua_func = prev_func.as_lua_function()?;
        let chunk = lua_func.chunk();
        // prev.pc points to the instruction AFTER the call (due to pc += 1 in fetch).
        // So the call instruction is at pc - 1.
        let pc = prev.pc.saturating_sub(1) as usize;
        let (kind, name) = funcnamefromcode(chunk, pc, lua53)?;
        // Lua 5.4+ name metamethods without the "__" prefix (ldebug.c `tmname + 2`)
        if kind == "metamethod" && !lua53 {
            return Some((kind, name.trim_start_matches("__").to_owned()));
        }
        return Some((kind, name));
    }
    // Previous frame is C — cannot determine name from bytecode
    None
}

// ============================================================================
// Public API for error message generation (mirrors ldebug.c luaG_typeerror)
// ============================================================================

/// Generate variable info string like " (global 'X')" for error messages.
/// Mirrors Lua 5.5's varinfo() from ldebug.c.
/// Must be called AFTER save_pc so the current frame's PC is up to date.
pub fn varinfo(l: &LuaState) -> String {
    let ci_idx = l.call_depth().wrapping_sub(1);
    let ci = match l.get_frame(ci_idx) {
        Some(ci) => ci,
        None => return String::new(),
    };
    if !ci.is_lua() {
        return String::new();
    }
    let func = match l.get_frame_func(ci_idx) {
        Some(f) => f,
        None => return String::new(),
    };
    let lua_func = match func.as_lua_function() {
        Some(f) => f,
        None => return String::new(),
    };
    let chunk = lua_func.chunk();
    // currentpc: saved pc points AFTER the current instruction (pc += 1 in fetch)
    let currentpc = ci.pc.saturating_sub(1) as usize;

    // Get the instruction at currentpc to determine which register holds the object
    if currentpc >= chunk.code.len() {
        return String::new();
    }
    let instr = chunk.code[currentpc];
    let op = instr.get_opcode();

    // Determine which register to look up based on the opcode
    let reg = match op {
        // GET* instructions: table is in register B
        OpCode::GetTable | OpCode::GetI | OpCode::GetField | OpCode::Self_ => Some(instr.get_b()),
        // SET* instructions: table is in register A
        OpCode::SetTable | OpCode::SetI | OpCode::SetField => Some(instr.get_a()),
        // GETTABUP: table is upvalue B — the upvalue itself is being indexed
        // When this instruction fails, it's because the upvalue is not indexable
        OpCode::GetTabUp => {
            let upval_idx = instr.get_b() as usize;
            if upval_idx < chunk.upvalue_descs.len() {
                let upname = &chunk.upvalue_descs[upval_idx].name;
                if upname == "_ENV" {
                    // For _ENV, report the key as global
                    let c = instr.get_c() as usize;
                    let name = kname(chunk, c).unwrap_or_else(|| "?".to_string());
                    return format!(" (global '{}')", name);
                } else {
                    return format!(" (upvalue '{}')", upname);
                }
            }
            return String::new();
        }
        // SETTABUP: table is upvalue A — the upvalue itself is being indexed
        OpCode::SetTabUp => {
            let upval_idx = instr.get_a() as usize;
            if upval_idx < chunk.upvalue_descs.len() {
                let upname = &chunk.upvalue_descs[upval_idx].name;
                if upname == "_ENV" {
                    // For _ENV, report the key as global
                    let b = instr.get_b() as usize;
                    let name = kname(chunk, b).unwrap_or_else(|| "?".to_string());
                    return format!(" (global '{}')", name);
                } else {
                    return format!(" (upvalue '{}')", upname);
                }
            }
            return String::new();
        }
        // CALL/TAILCALL: function being called is in register A
        OpCode::Call | OpCode::TailCall => Some(instr.get_a()),
        // Unary ops: operand is in register B
        OpCode::Unm | OpCode::BNot | OpCode::Len | OpCode::Not => Some(instr.get_b()),
        // CONCAT: operand is in register A (first concat value)
        OpCode::Concat => Some(instr.get_a()),
        // MmBin: look at previous instruction for the actual arithmetic/comparison op
        OpCode::MmBin => {
            // MmBin is emitted AFTER the arithmetic op (ADD, SUB, etc.)
            // The previous instruction has the operands
            if currentpc > 0 {
                let prev_instr = chunk.code[currentpc - 1];
                let prev_op = prev_instr.get_opcode();
                match prev_op {
                    OpCode::Add
                    | OpCode::Sub
                    | OpCode::Mul
                    | OpCode::Mod
                    | OpCode::Pow
                    | OpCode::Div
                    | OpCode::IDiv
                    | OpCode::BAnd
                    | OpCode::BOr
                    | OpCode::BXor
                    | OpCode::Shl
                    | OpCode::Shr
                    | OpCode::Eq
                    | OpCode::Lt
                    | OpCode::Le => {
                        // Binary ops: first operand in register A (aka sRA)
                        Some(prev_instr.get_a())
                    }
                    _ => None,
                }
            } else {
                None
            }
        }
        OpCode::MmBinI => {
            if currentpc > 0 {
                let prev_instr = chunk.code[currentpc - 1];
                Some(prev_instr.get_a())
            } else {
                None
            }
        }
        OpCode::MmBinK => {
            if currentpc > 0 {
                let prev_instr = chunk.code[currentpc - 1];
                Some(prev_instr.get_a())
            } else {
                None
            }
        }
        _ => None,
    };

    if let Some(reg) = reg
        && let Some((kind, name)) = getobjname(chunk, currentpc, reg, is_lua53(l))
    {
        return format!(" ({} '{}')", kind, name);
    }
    String::new()
}

/// Generate a type error with variable info.
/// Mirrors Lua 5.5's luaG_typeerror.
/// `op` is typically "index" for table access errors.
pub fn typeerror(l: &mut LuaState, val: &LuaValue, op: &str) -> LuaError {
    let tname = objtypename(l, val);
    let info = varinfo(l);
    l.error(format!("attempt to {} a {} value{}", op, tname, info))
}

/// Get the name and kind of the current function from the calling frame's bytecode.
/// Used by C stdlib functions to get their name for error messages.
/// Mirrors C Lua's approach in luaL_argerror: lua_getinfo(L, 0, "n").
pub fn current_func_name_with_kind(l: &LuaState) -> Option<(&'static str, String)> {
    let ci_idx = l.call_depth().wrapping_sub(1);
    getfuncname(l, ci_idx)
}

/// Search through loaded modules (package.loaded) to find the name of a function.
/// Mirrors C Lua's pushglobalfuncname / findfield.
/// Returns e.g. "table.sort", "string.sub", "math.sin", etc.
pub(crate) fn find_global_func_name(l: &LuaState, target: &LuaValue) -> Option<String> {
    // Get _LOADED from registry by iterating registry entries
    let vm = l.global_state();
    let registry_table = vm.registry.as_table()?;
    let mut loaded: Option<LuaValue> = None;
    for (key, val) in registry_table.iter_all() {
        if let Some(s) = key.as_str()
            && s == "_LOADED"
        {
            loaded = Some(val);
            break;
        }
    }
    let loaded = loaded?;
    let loaded_table = loaded.as_table()?;

    // Search through loaded modules (level 1: check each module's values)
    for (mod_key, mod_val) in loaded_table.iter_all() {
        if let Some(mod_name) = mod_key.as_str() {
            // Check if the module itself IS the target
            if mod_val == *target {
                let name = mod_name.to_string();
                // Strip _G. prefix
                return Some(if let Some(rest) = name.strip_prefix("_G.") {
                    rest.to_string()
                } else {
                    name
                });
            }
            // Search within the module table (level 2)
            if let Some(mod_table) = mod_val.as_table() {
                for (field_key, field_val) in mod_table.iter_all() {
                    if let Some(field_name) = field_key.as_str()
                        && field_val == *target
                    {
                        let full_name = format!("{}.{}", mod_name, field_name);
                        // Strip _G. prefix
                        return Some(if let Some(rest) = full_name.strip_prefix("_G.") {
                            rest.to_string()
                        } else {
                            full_name
                        });
                    }
                }
            }
        }
    }
    None
}

/// Get variable info for a specific register.
/// Like varinfo() but for a known register number.
pub fn varinfo_for_reg(l: &LuaState, reg: u32) -> String {
    let ci_idx = l.call_depth().wrapping_sub(1);
    let ci = match l.get_frame(ci_idx) {
        Some(ci) => ci,
        None => return String::new(),
    };
    if !ci.is_lua() {
        return String::new();
    }
    let func = match l.get_frame_func(ci_idx) {
        Some(f) => f,
        None => return String::new(),
    };
    let lua_func = match func.as_lua_function() {
        Some(f) => f,
        None => return String::new(),
    };
    let chunk = lua_func.chunk();
    let currentpc = ci.pc.saturating_sub(1) as usize;
    if let Some((kind, name)) = getobjname(chunk, currentpc, reg, is_lua53(l)) {
        format!(" ({} '{}')", kind, name)
    } else {
        String::new()
    }
}

/// Generate an arithmetic/bitwise type error (mirrors luaG_opinterror).
/// Determines which operand is the "bad" one and generates a type error.
pub fn opinterror(
    l: &mut LuaState,
    p1_reg: u32,
    p2_reg: u32,
    p1: &LuaValue,
    p2: &LuaValue,
    op: &str,
) -> LuaError {
    // If p1 is not a number, blame p1; otherwise blame p2
    let (blame_val, blame_reg) = if !p1.is_number() && !p1.is_integer() {
        (p1, p1_reg)
    } else {
        (p2, p2_reg)
    };
    let blame_type = objtypename(l, blame_val);
    let mut info = varinfo_for_reg(l, blame_reg);
    // Lua 5.3 arithmetic reads constants as RK operands, not registers, so
    // varinfo finds no name for them.
    if is_lua53(l) && info.starts_with(" (constant ") {
        info.clear();
    }
    l.error(format!("attempt to {} a {} value{}", op, blame_type, info))
}

/// Generate a comparison error (mirrors luaG_ordererror).
pub fn ordererror(l: &mut LuaState, p1: &LuaValue, p2: &LuaValue) -> LuaError {
    let t1 = objtypename(l, p1);
    let t2 = objtypename(l, p2);
    if t1 == t2 {
        l.error(format!("attempt to compare two {} values", t1))
    } else {
        l.error(format!("attempt to compare {} with {}", t1, t2))
    }
}

/// Generate a call error with function name info (mirrors luaG_callerror).
/// Used when attempting to call a non-callable value.
pub fn callerror(l: &mut LuaState, val: &LuaValue) -> LuaError {
    let t = objtypename(l, val);
    // Look at the current frame's instruction to determine what was being called
    let ci_idx = l.call_depth().wrapping_sub(1);
    if let Some(ci) = l.get_frame(ci_idx)
        && ci.is_lua()
        && let Some(func) = l.get_frame_func(ci_idx)
        && let Some(lua_func) = func.as_lua_function()
    {
        let chunk = lua_func.chunk();
        let pc = ci.pc.saturating_sub(1) as usize;
        if let Some((kind, name)) = funcnamefromcode(chunk, pc, is_lua53(l)) {
            let extra = match kind {
                // 5.3's luaG_typeerror names only stack slots; a metamethod is none.
                "metamethod" if is_lua53(l) => String::new(),
                // 5.4+ funcnamefromcall: `tmname + 2`
                "metamethod" => format!(" (metamethod '{}')", name.trim_start_matches("__")),
                _ => format!(" ({kind} '{name}')"),
            };
            return l.error(format!("attempt to call a {t} value{extra}"));
        }
    }
    // Fallback: no name info available
    l.error(format!("attempt to call a {} value", t))
}

/// Get the function name for a given frame index (public wrapper).
/// Returns (kind, name) or None.
pub fn pub_getfuncname(l: &LuaState, ci_frame_idx: usize) -> Option<(&'static str, String)> {
    getfuncname(l, ci_frame_idx)
}

fn is_lua53(l: &LuaState) -> bool {
    l.global_state().language() == crate::LuaLanguageLevel::Lua53
}

/// C: getthread. Returns the argument offset (1 when a thread is given) and
/// the target state.
fn getthread(l: &mut LuaState) -> (usize, *mut LuaState) {
    match l.get_arg(1) {
        Some(value) if value.is_thread() => {
            (1, value.as_thread_mut().map_or(l as *mut LuaState, |t| t as *mut LuaState))
        }
        _ => (0, l as *mut LuaState),
    }
}

/// `luaL_pushfail` (nil) and return one result.
fn push_fail(l: &mut LuaState) -> LuaResult<usize> {
    l.push_value(LuaValue::nil())?;
    Ok(1)
}

/// C: luaL_traceback. Frames are read with the same level numbering as
/// `lua_getstack` (level 0 is the running function of `target`).
pub(crate) fn traceback_text(lua53: bool, target: &LuaState, msg: Option<&[u8]>, mut level: usize) -> Vec<u8> {
    const LEVELS1: usize = 10;
    const LEVELS2: usize = 11;
    let depth = target.call_depth();
    let last = depth.saturating_sub(1); // lastlevel()
    let mut out = Vec::new();
    if let Some(msg) = msg {
        out.extend_from_slice(msg);
        out.push(b'\n');
    }
    out.extend_from_slice(b"stack traceback:");
    let mut limit2show: isize = if last.saturating_sub(level) > LEVELS1 + LEVELS2 { LEVELS1 as isize } else { -1 };
    while let Some(info) = target.get_info_by_level(level, "Slntf") {
        if limit2show == 0 {
            let n = last - level - LEVELS2 + 1;
            if lua53 {
                out.extend_from_slice(b"\n\t...");
            } else {
                out.extend_from_slice(format!("\n\t...\t(skipping {n} levels)").as_bytes());
            }
            level += n;
            limit2show -= 1;
            continue;
        }
        limit2show -= 1;
        let short_src = info.short_src.as_deref().unwrap_or("?");
        out.extend_from_slice(format!("\n\t{short_src}:").as_bytes());
        if let Some(line) = info.currentline.filter(|&line| line > 0) {
            out.extend_from_slice(format!("{line}:").as_bytes());
        }
        out.extend_from_slice(b" in ");
        let what = info.what.unwrap_or("?");
        let namewhat = info.namewhat.as_deref().unwrap_or("");
        let global = || info.func.as_ref().and_then(|f| find_global_func_name(target, f));
        let name = if lua53 {
            if let Some(global) = global() {
                format!("function '{global}'")
            } else if !namewhat.is_empty() {
                format!("{namewhat} '{}'", info.name.as_deref().unwrap_or("?"))
            } else if what == "main" {
                "main chunk".to_owned()
            } else if what != "C" {
                format!("function <{short_src}:{}>", info.linedefined.unwrap_or(0))
            } else {
                "?".to_owned()
            }
        } else if !namewhat.is_empty() {
            format!("{namewhat} '{}'", info.name.as_deref().unwrap_or("?"))
        } else if what == "main" {
            "main chunk".to_owned()
        } else if what != "C" {
            format!("function <{short_src}:{}>", info.linedefined.unwrap_or(0))
        } else if let Some(global) = global() {
            format!("function '{global}'")
        } else {
            "?".to_owned()
        };
        out.extend_from_slice(name.as_bytes());
        if info.istailcall == Some(true) {
            out.extend_from_slice(b"\n\t(...tail calls...)");
        }
        level += 1;
    }
    out
}

/// debug.debug(): read and run lines from stdin until "cont" (ldblib.c).
fn debug_debug(l: &mut LuaState) -> LuaResult<usize> {
    use std::io::{BufRead, Write};
    loop {
        eprint!("lua_debug> ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => return Ok(0),
            Ok(_) => {}
        }
        if line == "cont\n" || line == "cont" {
            return Ok(0);
        }
        let result = l
            .load_with_name(&line, "=(debug command)")
            .and_then(|function| l.call(function, Vec::new()).map(|_| ()));
        if let Err(error) = result {
            let message = l.get_error_message(error);
            eprintln!("{message}");
            let _ = std::io::stderr().flush();
        }
    }
}

pub fn create_debug_lib() -> LibraryModule {
    let mut module = lib_module!("debug", {
        "traceback" => debug_traceback,
        "getinfo" => debug_getinfo,
        "getmetatable" => debug_getmetatable,
        "setmetatable" => debug_setmetatable,
        "getregistry" => debug_getregistry,
        "getlocal" => debug_getlocal,
        "setlocal" => debug_setlocal,
        "getupvalue" => debug_getupvalue,
        "setupvalue" => debug_setupvalue,
        "upvalueid" => debug_upvalueid,
        "upvaluejoin" => debug_upvaluejoin,
        "gethook" => debug_gethook,
        "sethook" => debug_sethook,
        "setuservalue" => debug_setuservalue,
        "getuservalue" => debug_getuservalue,
        "debug" => debug_debug,
    });
    module.initializer = Some(debug_lib_init);
    module
}

/// Initialize the debug library: create _HOOKKEY table in registry with weak keys.
/// This matches C Lua's luaopen_debug which creates a hook table for per-thread hooks.
fn debug_lib_init(l: &mut LuaState) -> LuaResult<()> {
    // Create the hook table
    let hook_table = l.create_table(0, 0)?;
    // Create its metatable with __mode = "k" (weak keys)
    let meta = l.create_table(0, 1)?;
    let mode_key = l.create_string("__mode")?;
    let mode_val = l.create_string("k")?;
    l.raw_set(&meta, mode_key, mode_val);
    if let Some(hook_tbl) = hook_table.as_table_mut() {
        hook_tbl.set_metatable(Some(meta));
    }
    // Store in registry as _HOOKKEY
    let reg = l.global_state_mut().registry;
    let hook_key = l.create_string("_HOOKKEY")?;
    l.raw_set(&reg, hook_key, hook_table);
    Ok(())
}

/// debug.traceback([message [, level]]) - Get stack traceback
fn debug_traceback(l: &mut LuaState) -> LuaResult<usize> {
    let (arg, target) = getthread(l);
    let msg_value = l.get_arg(arg + 1).unwrap_or_default();
    let msg = lauxlib::to_lstr(l, &msg_value);
    if msg.is_none() && !msg_value.is_nil() {
        l.push_value(msg_value)?; // non-string message: return it untouched
        return Ok(1);
    }
    let same = std::ptr::eq(target, l);
    let level = lauxlib::opt_integer(l, arg + 2, if same { 1 } else { 0 })?;
    let msg = msg.map(|m| m.to_vec());
    let lua53 = is_lua53(l);
    // SAFETY: `target` is `l` or a live coroutine passed as argument 1.
    let level = if level < 0 { usize::MAX } else { level as usize };
    let text = traceback_text(lua53, unsafe { &*target }, msg.as_deref(), level);
    let value = l.create_bytes(&text)?;
    l.push_value(value)?;
    Ok(1)
}

/// debug.getinfo([thread,] f [, what]) - Get function info
/// Thin wrapper: delegates to LuaState::get_info_by_level / get_info_for_func,
/// then converts the DebugInfo struct to a Lua table.
fn debug_getinfo(l: &mut LuaState) -> LuaResult<usize> {
    let (arg, target_ptr) = getthread(l);
    // SAFETY: `target_ptr` is `l` or a live coroutine passed as argument 1.
    let target: &LuaState = unsafe { &*target_ptr };
    let lua53 = is_lua53(l);
    let options = match lauxlib::opt_lstring(l, arg + 2)? {
        Some(text) => String::from_utf8_lossy(&text).into_owned(),
        None => if lua53 { "flnStu" } else { "flnSrtu" }.to_owned(),
    };
    if !lua53 && options.starts_with('>') {
        return Err(lauxlib::argerror(l, arg + 2, "invalid option '>'"));
    }
    let valid = if lua53 { "SlnutLf" } else { "SlnutrLf" };
    let func_or_level = l.get_arg(arg + 1).unwrap_or_default();
    let info = if func_or_level.is_function() {
        if !options.chars().all(|c| valid.contains(c)) {
            return Err(lauxlib::argerror(l, arg + 2, "invalid option"));
        }
        target.get_info_for_func(&func_or_level, &options)
    } else {
        let level = lauxlib::check_integer(l, arg + 1)?;
        let info = usize::try_from(level).ok().and_then(|level| target.get_info_by_level(level, &options));
        let Some(info) = info else {
            return push_fail(l); // level out of range
        };
        if !options.chars().all(|c| valid.contains(c)) {
            return Err(lauxlib::argerror(l, arg + 2, "invalid option"));
        }
        info
    };
    let what_str = options;

    // Convert DebugInfo to Lua table
    let info_table = l.create_table(0, 12)?;

    // 'S' fields
    if let Some(ref source) = info.source {
        let k = l.create_string("source")?;
        let v = l.create_string(source)?;
        l.raw_set(&info_table, k, v);
    }
    if let Some(ref short_src) = info.short_src {
        let k = l.create_string("short_src")?;
        let v = l.create_string(short_src)?;
        l.raw_set(&info_table, k, v);
    }
    if let Some(linedefined) = info.linedefined {
        let k = l.create_string("linedefined")?;
        l.raw_set(&info_table, k, LuaValue::integer(linedefined as i64));
    }
    if let Some(lastlinedefined) = info.lastlinedefined {
        let k = l.create_string("lastlinedefined")?;
        l.raw_set(&info_table, k, LuaValue::integer(lastlinedefined as i64));
    }
    if let Some(what) = info.what {
        let k = l.create_string("what")?;
        let v = l.create_string(what)?;
        l.raw_set(&info_table, k, v);
    }

    // 'l' field
    if let Some(currentline) = info.currentline {
        let k = l.create_string("currentline")?;
        l.raw_set(&info_table, k, LuaValue::integer(currentline as i64));
    }

    // 'u' fields
    if let Some(nups) = info.nups {
        let k = l.create_string("nups")?;
        l.raw_set(&info_table, k, LuaValue::integer(nups as i64));
    }
    if let Some(nparams) = info.nparams {
        let k = l.create_string("nparams")?;
        l.raw_set(&info_table, k, LuaValue::integer(nparams as i64));
    }
    if let Some(isvararg) = info.isvararg {
        let k = l.create_string("isvararg")?;
        l.raw_set(&info_table, k, LuaValue::boolean(isvararg));
    }

    // 'n' fields
    if info.namewhat.is_some() {
        let k = l.create_string("name")?;
        let v = if let Some(ref name) = info.name {
            l.create_string(name)?
        } else {
            LuaValue::nil()
        };
        l.raw_set(&info_table, k, v);

        let k2 = l.create_string("namewhat")?;
        let v2 = l.create_string(info.namewhat.as_deref().unwrap_or(""))?;
        l.raw_set(&info_table, k2, v2);
    }

    // 't' fields
    if let Some(istailcall) = info.istailcall {
        let k = l.create_string("istailcall")?;
        l.raw_set(&info_table, k, LuaValue::boolean(istailcall));
    }
    if let Some(extraargs) = info.extraargs.filter(|_| !lua53) {
        let k = l.create_string("extraargs")?;
        l.raw_set(&info_table, k, LuaValue::integer(extraargs as i64));
    }

    // 'r' fields
    if let Some(ftransfer) = info.ftransfer.filter(|_| !lua53) {
        let k = l.create_string("ftransfer")?;
        l.raw_set(&info_table, k, LuaValue::integer(ftransfer as i64));
    }
    if let Some(ntransfer) = info.ntransfer.filter(|_| !lua53) {
        let k = l.create_string("ntransfer")?;
        l.raw_set(&info_table, k, LuaValue::integer(ntransfer as i64));
    }

    // 'L' field
    if what_str.contains('L') {
        let k = l.create_string("activelines")?;
        if let Some(ref lines) = info.activelines {
            let lines_table = l.create_table(0, lines.len())?;
            for &line in lines {
                l.raw_set(
                    &lines_table,
                    LuaValue::integer(line as i64),
                    LuaValue::boolean(true),
                );
            }
            l.raw_set(&info_table, k, lines_table);
        } else {
            l.raw_set(&info_table, k, LuaValue::nil());
        }
    }

    // 'f' field
    if let Some(func) = info.func {
        let k = l.create_string("func")?;
        l.raw_set(&info_table, k, func);
    }

    l.push_value(info_table)?;
    Ok(1)
}

/// debug.getmetatable(value) - Get metatable of a value (no protection)
fn debug_getmetatable(l: &mut LuaState) -> LuaResult<usize> {
    let value = lauxlib::check_any(l, 1)?;
    let metatable = get_metatable(l, &value).unwrap_or_default();
    l.push_value(metatable)?;
    Ok(1)
}

/// debug.setmetatable(value, table) - Set metatable of a value
fn debug_setmetatable(l: &mut LuaState) -> LuaResult<usize> {
    let value = l.get_arg(1).unwrap_or_default();
    let mt_val = match l.get_arg(2) {
        Some(mt) if mt.is_nil() => None,
        Some(mt) if mt.is_table() => Some(mt),
        _ if is_lua53(l) => return Err(lauxlib::argerror(l, 2, "nil or table expected")),
        _ => return Err(lauxlib::typeerror(l, 2, "nil or table")),
    };

    if let Some(table) = value.as_table_mut() {
        // For tables, set metatable directly on the table
        table.set_metatable(mt_val);
        // GC write barrier: table may be BLACK, new metatable may be WHITE
        if let Some(gc_ptr) = value.as_gc_ptr() {
            l.gc_barrier_back(gc_ptr);
        }
    } else {
        // For basic types (number, string, boolean), set the global type metatable
        let kind = value.kind();
        l.global_state_mut().set_basic_metatable(kind, mt_val);
    }

    // Register for finalization if __gc is present
    l.global_state_mut().gc.check_finalizer(&value);

    l.push_value(value)?;
    Ok(1)
}

/// debug.gethook([thread]) - Get current hook settings
/// Returns the hook function, mask string, and count.
/// Hooks are per-thread: if a thread arg is given, returns that thread's hook.
fn debug_gethook(l: &mut LuaState) -> LuaResult<usize> {
    let arg1 = l.get_arg(1).unwrap_or_default();
    let target_ptr: *const LuaState = if arg1.is_thread() {
        arg1.as_thread_mut().unwrap() as *const LuaState
    } else {
        l as *const LuaState
    };
    let target = unsafe { &*target_ptr };

    let hook = target.hook;
    let mask = target.hook_mask;
    let count = target.base_hook_count;

    if hook.is_nil() && !is_lua53(l) {
        return push_fail(l);
    }
    l.push_value(hook)?;

    // Build mask string
    let mut mask_str = String::new();
    if mask & LUA_MASKCALL != 0 {
        mask_str.push('c');
    }
    if mask & LUA_MASKRET != 0 {
        mask_str.push('r');
    }
    if mask & LUA_MASKLINE != 0 {
        mask_str.push('l');
    }
    let mask_val = l.create_string(&mask_str)?;
    l.push_value(mask_val)?;

    // Push count
    l.push_value(LuaValue::integer(count as i64))?;

    Ok(3)
}

/// debug.sethook([thread,] hook, mask [, count]) - Set a debug hook
/// Hooks are per-thread. If a thread arg is given, sets that thread's hook.
///
/// Arguments:
///   hook: function to call, or nil/nothing to clear
///   mask: string containing 'c' (call), 'r' (return), 'l' (line)
///   count: (optional) fire hook every N instructions
///
/// Calling with no arguments clears the hook.
fn debug_sethook(l: &mut LuaState) -> LuaResult<usize> {
    let (arg, target_ptr) = getthread(l);
    let hook_value = l.get_arg(arg + 1).unwrap_or_default();
    let (hook, mask, count) = if hook_value.is_nil() {
        (LuaValue::nil(), 0u8, 0i32)
    } else {
        let smask = lauxlib::check_lstring(l, arg + 2)?.to_vec();
        if !hook_value.is_function() {
            return Err(lauxlib::typeerror(l, arg + 1, "function"));
        }
        let count = lauxlib::opt_integer(l, arg + 3, 0)? as i32;
        let mut mask = 0u8;
        if smask.contains(&b'c') {
            mask |= LUA_MASKCALL;
        }
        if smask.contains(&b'r') {
            mask |= LUA_MASKRET;
        }
        if smask.contains(&b'l') {
            mask |= LUA_MASKLINE;
        }
        if count > 0 {
            mask |= LUA_MASKCOUNT;
        }
        (hook_value, mask, count)
    };

    // lua_sethook: an empty mask turns the hook off
    let hook = if mask == 0 { LuaValue::nil() } else { hook };

    // Set hook state on the target thread
    // SAFETY: target_ptr points to a valid LuaState
    let target = unsafe { &mut *target_ptr };
    target.hook = hook;
    target.hook_mask = mask;
    target.base_hook_count = count;
    target.hook_count = count;

    Ok(0)
}

/// debug.getregistry() - Return the registry table
fn debug_getregistry(l: &mut LuaState) -> LuaResult<usize> {
    let registry = l.global_state_mut().registry;
    l.push_value(registry)?;
    Ok(1)
}

/// debug.getlocal([thread,] f, local) - Get the name and value of a local variable
fn debug_getlocal(l: &mut LuaState) -> LuaResult<usize> {
    let (arg, target_ptr) = getthread(l);
    // SAFETY: `target_ptr` is `l` or a live coroutine passed as argument 1.
    let target: &LuaState = unsafe { &*target_ptr };
    let local_index = lauxlib::check_integer(l, arg + 2)?;
    let func_or_level = l.get_arg(arg + 1).unwrap_or_default();
    let lua53 = is_lua53(l);

    // Case 1: a function → parameter names only (lua_getlocal(L, NULL, n))
    if func_or_level.is_function() {
        let mut name = None;
        if let Some(lua_func) = func_or_level.as_lua_function()
            && local_index > 0
        {
            // 5.5 starts the vararg parameter after VARARGPREP (startpc 1), so only named
            // parameters qualify; 5.3 has no vararg parameter at all.
            name = lua_func
                .chunk()
                .locals
                .iter()
                .take_while(|locvar| locvar.startpc == 0)
                .nth(local_index as usize - 1)
                .map(|locvar| locvar.name.clone());
        }
        let value = match name {
            Some(name) => l.create_string(&name)?,
            None => LuaValue::nil(),
        };
        l.push_value(value)?;
        return Ok(1);
    }

    let level = lauxlib::check_integer(l, arg + 1)?;
    if level < 0 || level as usize >= target.call_depth() {
        return Err(lauxlib::argerror(l, arg + 1, "level out of range"));
    }
    let level = level as usize;

    let frame_idx = target.call_depth() - 1 - level;
    match findlocal(target, frame_idx, local_index, lua53) {
        Some((name, slot)) => {
            let value = target.stack_get(slot).unwrap_or_default();
            let name = l.create_string(name)?;
            l.push_value(name)?;
            l.push_value(value)?;
            Ok(2)
        }
        None => push_fail(l),
    }
}

/// debug.setlocal([thread,] level, local, value) - Set the value of a local variable
fn debug_setlocal(l: &mut LuaState) -> LuaResult<usize> {
    let (arg, target_ptr) = getthread(l);
    // SAFETY: `target_ptr` is `l` or a live coroutine passed as argument 1.
    let target: &mut LuaState = unsafe { &mut *target_ptr };
    let level = lauxlib::check_integer(l, arg + 1)?;
    let local_index = lauxlib::check_integer(l, arg + 2)?;
    let call_depth = target.call_depth();
    if level < 0 || level as usize >= call_depth {
        return Err(lauxlib::argerror(l, arg + 1, "level out of range"));
    }
    let level = level as usize;
    let value = lauxlib::check_any(l, arg + 3)?;
    let frame_idx = call_depth - 1 - level;
    match findlocal(target, frame_idx, local_index, is_lua53(l)) {
        Some((name, slot)) => {
            let name = l.create_string(name)?;
            target.stack_set(slot, value)?;
            l.push_value(name)?;
            Ok(1)
        }
        None => push_fail(l),
    }
}

/// luaG_findlocal: name and stack slot of local `n` of the frame `frame_idx` of `target`.
/// Negative `n` selects a vararg of a Lua frame; slots without a variable name between the
/// frame base and the next frame (or the top) are temporaries.
fn findlocal<'a>(target: &'a LuaState, frame_idx: usize, n: i64, lua53: bool) -> Option<(&'a str, usize)> {
    let ci = target.get_call_info(frame_idx);
    let base = ci.base;
    let func = target.get_frame_func(frame_idx)?;
    let temporary = if let Some(lua_func) = func.as_lua_function() {
        // SAFETY: the frame keeps its closure (and so the prototype) alive while `target`
        // is borrowed.
        let chunk: &'a LuaProto = unsafe { &*(lua_func.chunk() as *const LuaProto) };
        if n < 0 {
            // findvararg
            if !chunk.is_vararg {
                return None;
            }
            let var_idx = (n.unsigned_abs() - 1) as usize;
            if var_idx >= ci.nextraargs as usize {
                return None;
            }
            let func_offset = ci.func_offset as usize;
            let original_func_pos = if func_offset > 0 { base - func_offset } else { base.saturating_sub(1) };
            let slot = original_func_pos + 1 + chunk.param_count + var_idx;
            if slot >= target.stack_len() {
                return None;
            }
            return Some((if lua53 { "(*vararg)" } else { "(vararg)" }, slot));
        }
        if n > 0 {
            let pc = (target.get_frame_pc(frame_idx) as usize).saturating_sub(1);
            if let Some(name) = getlocalname(chunk, n as usize, pc) {
                return Some((name, base + n as usize - 1));
            }
        }
        if lua53 { "(*temporary)" } else { "(temporary)" }
    } else if lua53 {
        "(*temporary)"
    } else {
        "(C temporary)"
    };
    let limit = if frame_idx + 1 == target.call_depth() {
        target.get_top()
    } else {
        let next_ci = target.get_call_info(frame_idx + 1);
        next_ci.base - next_ci.func_offset as usize
    };
    if n > 0 && limit.saturating_sub(base) >= n as usize {
        Some((temporary, base + n as usize - 1))
    } else {
        None
    }
}

/// debug.getupvalue(f, up) - Get the name and value of an upvalue
fn debug_getupvalue(l: &mut LuaState) -> LuaResult<usize> {
    let up_index = lauxlib::check_integer(l, 2)?;
    let func = l.get_arg(1).unwrap_or_default();
    if !func.is_function() {
        return Err(lauxlib::typeerror(l, 1, "function"));
    }
    let up_index = usize::try_from(up_index).unwrap_or(0);

    if let Some(lua_func) = func.as_lua_function() {
        // Get upvalue from Lua function
        let upvalues = lua_func.upvalues();
        if up_index > 0 && up_index <= upvalues.len() {
            let upvalue = &upvalues[up_index - 1];

            // Get the name from chunk
            let chunk = lua_func.chunk();
            if up_index <= chunk.upvalue_descs.len() {
                // Use actual upvalue name from chunk (or "(no name)" if stripped)
                let name = &chunk.upvalue_descs[up_index - 1].name;
                let display_name = if name.is_empty() {
                    if is_lua53(l) { "(*no name)" } else { "(no name)" }
                } else {
                    name.as_str()
                };
                let name_str = l.create_string(display_name)?;

                // Get the value
                let value = upvalue.as_ref().data.get_value();
                l.push_value(name_str)?;
                l.push_value(value)?;
                return Ok(2);
            }
        }
    } else if let Some(cclosure) = func.as_cclosure() {
        // C closures: upvalue names are always "" (empty string)
        let upvalues = cclosure.upvalues();
        if up_index > 0 && up_index <= upvalues.len() {
            let value = upvalues[up_index - 1];
            let name_str = l.create_string("")?;
            l.push_value(name_str)?;
            l.push_value(value)?;
            return Ok(2);
        }
    } else if let Some(rclosure) = func.as_rclosure() {
        // RClosures: upvalue names are always "" (empty string)
        let upvalues = rclosure.upvalues();
        if up_index > 0 && up_index <= upvalues.len() {
            let value = upvalues[up_index - 1];
            let name_str = l.create_string("")?;
            l.push_value(name_str)?;
            l.push_value(value)?;
            return Ok(2);
        }
    }

    // No upvalue found, return nil
    Ok(0)
}

/// debug.setupvalue(f, up, value) - Set the value of an upvalue
fn debug_setupvalue(l: &mut LuaState) -> LuaResult<usize> {
    let value = lauxlib::check_any(l, 3)?;
    let up_index = lauxlib::check_integer(l, 2)?;
    let func = l.get_arg(1).unwrap_or_default();
    if !func.is_function() {
        return Err(lauxlib::typeerror(l, 1, "function"));
    }
    let up_index = usize::try_from(up_index).unwrap_or(0);

    if let Some(lua_func) = func.as_lua_function() {
        // Set upvalue in Lua function
        let upvalues = lua_func.upvalues();
        if up_index > 0 && up_index <= upvalues.len() {
            let upvalue_ptr = upvalues[up_index - 1];

            let chunk = lua_func.chunk();
            // Get the upvalue name from the chunk
            let upvalue_name = if up_index - 1 < chunk.upvalue_descs.len() {
                chunk.upvalue_descs[up_index - 1].name.clone()
            } else {
                String::new()
            };

            // Set the upvalue value (similar to SETUPVAL instruction)
            let upval_ref = upvalue_ptr.as_mut_ref();
            upval_ref.data.set_value(value);

            // GC barrier if needed
            if value.is_collectable()
                && let Some(value_gc_ptr) = value.as_gc_ptr()
            {
                l.gc_barrier(upvalue_ptr, value_gc_ptr);
            }

            // Return the upvalue name ("(no name)" if stripped)
            let display_name = if upvalue_name.is_empty() {
                if is_lua53(l) { "(*no name)" } else { "(no name)" }.to_string()
            } else {
                upvalue_name
            };
            let name_val = l.create_string(&display_name)?;
            l.push_value(name_val)?;
            return Ok(1);
        }
    }

    // No upvalue found, return nil
    Ok(0)
}

/// debug.upvalueid(f, n) - Get a unique identifier for an upvalue
fn debug_upvalueid(l: &mut LuaState) -> LuaResult<usize> {
    match checkupval(l, 1, 2, is_lua53(l))? {
        Some(id) => {
            l.push_value(LuaValue::lightuserdata(id))?;
            Ok(1)
        }
        None => push_fail(l),
    }
}

/// C: checkupval. Returns the upvalue id of upvalue `argnup` of function
/// `argf`; with `required`, an invalid index is an argument error.
fn checkupval(l: &mut LuaState, argf: usize, argnup: usize, required: bool) -> LuaResult<Option<*mut std::ffi::c_void>> {
    let nup = lauxlib::check_integer(l, argnup)?;
    let func = l.get_arg(argf).unwrap_or_default();
    if !func.is_function() {
        return Err(lauxlib::typeerror(l, argf, "function"));
    }
    let index = usize::try_from(nup).ok().and_then(|n| n.checked_sub(1));
    let id = index.and_then(|index| {
        if let Some(function) = func.as_lua_function() {
            function.upvalues().get(index).map(|up| up.as_ptr() as *mut std::ffi::c_void)
        } else if let Some(closure) = func.as_cclosure() {
            closure.upvalues().get(index).map(|up| up as *const _ as *mut std::ffi::c_void)
        } else {
            None
        }
    });
    if required && id.is_none() {
        return Err(lauxlib::argerror(l, argnup, "invalid upvalue index"));
    }
    Ok(id)
}

/// debug.upvaluejoin(f1, n1, f2, n2) - Make upvalue n1 of f1 refer to upvalue n2 of f2
fn debug_upvaluejoin(l: &mut LuaState) -> LuaResult<usize> {
    checkupval(l, 1, 2, true)?;
    checkupval(l, 3, 4, true)?;
    let func1 = l.get_arg(1).unwrap_or_default();
    let func2 = l.get_arg(3).unwrap_or_default();
    if func1.as_lua_function().is_none() {
        return Err(lauxlib::argerror(l, 1, "Lua function expected"));
    }
    let Some(lua_func2) = func2.as_lua_function() else {
        return Err(lauxlib::argerror(l, 3, "Lua function expected"));
    };
    let n1 = lauxlib::check_integer(l, 2)? as usize;
    let n2 = lauxlib::check_integer(l, 4)? as usize;
    let shared = lua_func2.upvalues()[n2 - 1];
    if let Some(lua_func1) = func1.as_lua_function_mut() {
        lua_func1.upvalues_mut()[n1 - 1] = shared;
    }
    Ok(0)
}

/// debug.setuservalue(udata, value [, n]) - Set user value of a userdata
fn debug_setuservalue(l: &mut LuaState) -> LuaResult<usize> {
    let udata = l.get_arg(1).unwrap_or_default();
    if !udata.is_userdata() || udata.ttislightuserdata() {
        return Err(lauxlib::typeerror(l, 1, "userdata"));
    }
    let value = lauxlib::check_any(l, 2)?;
    let n = if is_lua53(l) { 1 } else { lauxlib::opt_integer(l, 3, 1)? };
    if n == 1 && set_userdata_uservalue(l, &udata, value) {
        l.push_value(udata)?;
        return Ok(1);
    }
    // Only userdata created through the C API carry a user value; others
    // have none (Lua 5.4+: 0 user values, so setiuservalue fails).
    if is_lua53(l) {
        return Err(lauxlib::lual_error(l, "userdata has no user value"));
    }
    push_fail(l)
}

/// debug.getuservalue(udata [, n]) - Get user value of a userdata
fn debug_getuservalue(l: &mut LuaState) -> LuaResult<usize> {
    let udata = l.get_arg(1).unwrap_or_default();
    let lua53 = is_lua53(l);
    let n = if lua53 { 1 } else { lauxlib::opt_integer(l, 2, 1)? };
    if !udata.is_userdata() || udata.ttislightuserdata() {
        return push_fail(l);
    }
    let value = if n == 1 { userdata_uservalue(&udata) } else { None };
    match value {
        Some(value) => {
            l.push_value(value)?;
            if lua53 {
                return Ok(1);
            }
            l.push_value(LuaValue::boolean(true))?;
            Ok(2)
        }
        // no such user value: 5.3 pushes nil; 5.5 pushes nil (LUA_TNONE)
        None => push_fail(l),
    }
}
