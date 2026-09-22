//! Standard Lua 5.3 binary chunk reader and writer.
//!
//! The wire format and instruction set are Lua 5.3's. Instructions are
//! translated at the boundary; the VM continues to execute its native opcode
//! set and never needs to retain a second copy of the bytecode.

use crate::Instruction;
use crate::lua_value::{LocVar, LuaProto, LuaValue, UpvalueDesc};
use crate::lua_vm::{GlobalState, OpCode, TmKind};

const SIGNATURE: &[u8; 4] = b"\x1bLua";
const VERSION: u8 = 0x53;
const FORMAT: u8 = 0;
const DATA: &[u8; 6] = b"\x19\x93\r\n\x1a\n";
const LUAC_INT: i64 = 0x5678;
const LUAC_NUM: f64 = 370.5;
const MAX_CHUNK_BYTES: usize = 128 << 20;
const MAX_VECTOR_ITEMS: usize = 16_000_000;
const MAX_STRING_BYTES: usize = 64 << 20;
const MAX_PROTO_DEPTH: usize = 200;
const BITRK: u32 = 1 << 8;
const MAXARG_BX53: i32 = (1 << 18) - 1;
const MAXARG_SBX53: i32 = MAXARG_BX53 >> 1;

// Lua 5.3 opcode numbers (lopcodes.h).
const OP_MOVE: u8 = 0;
const OP_LOADK: u8 = 1;
const OP_LOADKX: u8 = 2;
const OP_LOADBOOL: u8 = 3;
const OP_LOADNIL: u8 = 4;
const OP_GETUPVAL: u8 = 5;
const OP_GETTABUP: u8 = 6;
const OP_GETTABLE: u8 = 7;
const OP_SETTABUP: u8 = 8;
const OP_SETUPVAL: u8 = 9;
const OP_SETTABLE: u8 = 10;
const OP_NEWTABLE: u8 = 11;
const OP_SELF: u8 = 12;
const OP_ADD: u8 = 13;
const OP_SUB: u8 = 14;
const OP_MUL: u8 = 15;
const OP_MOD: u8 = 16;
const OP_POW: u8 = 17;
const OP_DIV: u8 = 18;
const OP_IDIV: u8 = 19;
const OP_BAND: u8 = 20;
const OP_BOR: u8 = 21;
const OP_BXOR: u8 = 22;
const OP_SHL: u8 = 23;
const OP_SHR: u8 = 24;
const OP_UNM: u8 = 25;
const OP_BNOT: u8 = 26;
const OP_NOT: u8 = 27;
const OP_LEN: u8 = 28;
const OP_CONCAT: u8 = 29;
const OP_JMP: u8 = 30;
const OP_EQ: u8 = 31;
const OP_LT: u8 = 32;
const OP_LE: u8 = 33;
const OP_TEST: u8 = 34;
const OP_TESTSET: u8 = 35;
const OP_CALL: u8 = 36;
const OP_TAILCALL: u8 = 37;
const OP_RETURN: u8 = 38;
const OP_FORLOOP: u8 = 39;
const OP_FORPREP: u8 = 40;
const OP_TFORCALL: u8 = 41;
const OP_TFORLOOP: u8 = 42;
const OP_SETLIST: u8 = 43;
const OP_CLOSURE: u8 = 44;
const OP_VARARG: u8 = 45;
const OP_EXTRAARG: u8 = 46;

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    endian: Endian,
    int_size: usize,
    size_t_size: usize,
    instruction_size: usize,
    integer_size: usize,
    number_size: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Result<Self, String> {
        if data.len() > MAX_CHUNK_BYTES {
            return Err("Lua 5.3 chunk exceeds the 128 MiB safety limit".to_string());
        }
        Ok(Self {
            data,
            at: 0,
            endian: Endian::Little,
            int_size: 4,
            size_t_size: 8,
            instruction_size: 4,
            integer_size: 8,
            number_size: 8,
        })
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(len)
            .ok_or_else(|| "Lua 5.3 chunk offset overflow".to_string())?;
        if end > self.data.len() {
            return Err("truncated Lua 5.3 binary chunk".to_string());
        }
        let result = &self.data[self.at..end];
        self.at = end;
        Ok(result)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }

    fn unsigned_width(&mut self, width: usize) -> Result<u64, String> {
        if !(1..=8).contains(&width) {
            return Err(format!(
                "unsupported integer width {width} in Lua 5.3 chunk"
            ));
        }
        let bytes = self.bytes(width)?;
        let mut padded = [0u8; 8];
        match self.endian {
            Endian::Little => padded[..width].copy_from_slice(bytes),
            Endian::Big => padded[8 - width..].copy_from_slice(bytes),
        }
        Ok(match self.endian {
            Endian::Little => u64::from_le_bytes(padded),
            Endian::Big => u64::from_be_bytes(padded),
        })
    }

    fn signed_width(&mut self, width: usize) -> Result<i64, String> {
        let raw = self.unsigned_width(width)?;
        if width == 8 {
            Ok(raw as i64)
        } else {
            let shift = 64 - width * 8;
            Ok(((raw << shift) as i64) >> shift)
        }
    }

    fn int(&mut self) -> Result<i32, String> {
        let value = self.signed_width(self.int_size)?;
        i32::try_from(value).map_err(|_| "Lua 5.3 int is out of range".to_string())
    }

    fn count(&mut self, what: &str) -> Result<usize, String> {
        let value = self.int()?;
        if value < 0 {
            return Err(format!("negative {what} count in Lua 5.3 chunk"));
        }
        let value = value as usize;
        if value > MAX_VECTOR_ITEMS {
            return Err(format!("{what} count exceeds the safety limit"));
        }
        Ok(value)
    }

    fn uint32(&mut self) -> Result<u32, String> {
        let value = self.unsigned_width(self.instruction_size)?;
        u32::try_from(value).map_err(|_| "Lua 5.3 instruction is wider than 32 bits".to_string())
    }

    fn lua_integer(&mut self) -> Result<i64, String> {
        self.signed_width(self.integer_size)
    }

    fn lua_number(&mut self) -> Result<f64, String> {
        if self.number_size != 8 {
            return Err(format!(
                "unsupported Lua number width {} (expected 8)",
                self.number_size
            ));
        }
        let bits = self.unsigned_width(8)?;
        Ok(f64::from_bits(bits))
    }

    fn size_t(&mut self) -> Result<usize, String> {
        let width = self.size_t_size;
        usize::try_from(self.unsigned_width(width)?)
            .map_err(|_| "Lua 5.3 size_t is out of range".to_string())
    }

    fn string(&mut self) -> Result<Option<Vec<u8>>, String> {
        let marker = self.byte()?;
        if marker == 0 {
            return Ok(None);
        }
        let stored = if marker == 0xff {
            self.size_t()?
        } else {
            marker as usize
        };
        let len = stored
            .checked_sub(1)
            .ok_or_else(|| "invalid zero-length Lua 5.3 string".to_string())?;
        if len > MAX_STRING_BYTES {
            return Err("Lua 5.3 string exceeds the 64 MiB safety limit".to_string());
        }
        Ok(Some(self.bytes(len)?.to_vec()))
    }
}

#[inline]
fn op53(i: u32) -> u8 {
    (i & 0x3f) as u8
}
#[inline]
fn a53(i: u32) -> u32 {
    (i >> 6) & 0xff
}
#[inline]
fn c53(i: u32) -> u32 {
    (i >> 14) & 0x1ff
}
#[inline]
fn b53(i: u32) -> u32 {
    (i >> 23) & 0x1ff
}
#[inline]
fn bx53(i: u32) -> u32 {
    (i >> 14) & 0x3ffff
}
#[inline]
fn sbx53(i: u32) -> i32 {
    bx53(i) as i32 - MAXARG_SBX53
}
#[inline]
fn ax53(i: u32) -> u32 {
    i >> 6
}

fn int2fb(mut value: u32) -> u32 {
    let mut exponent = 0;
    if value < 8 {
        return value;
    }
    while value >= 16 {
        value = (value + 1) >> 1;
        exponent += 1;
    }
    ((exponent + 1) << 3) | (value - 8)
}

fn fb2int(value: u32) -> u32 {
    if value < 8 {
        value
    } else {
        ((value & 7) + 8) << ((value >> 3) - 1)
    }
}

fn emit_loadk(out: &mut Vec<Instruction>, register: u32, index: u32) -> Result<(), String> {
    if index <= Instruction::MAX_BX {
        out.push(Instruction::create_abx(OpCode::LoadK, register, index));
    } else if index <= Instruction::MAX_AX {
        out.push(Instruction::create_abx(OpCode::LoadKX, register, 0));
        out.push(Instruction::create_ax(OpCode::ExtraArg, index));
    } else {
        return Err("Lua 5.3 constant index exceeds the native VM limit".to_string());
    }
    Ok(())
}

fn load_rk(
    out: &mut Vec<Instruction>,
    constants_len: usize,
    operand: u32,
    temp: u32,
) -> Result<u32, String> {
    if operand & BITRK == 0 {
        return Ok(operand);
    }
    let index = operand & !BITRK;
    if index as usize >= constants_len {
        return Err(format!("Lua 5.3 constant index {index} is out of bounds"));
    }
    emit_loadk(out, temp, index)?;
    Ok(temp)
}

fn validate_register(register: u32, max_stack: u8, context: &str) -> Result<(), String> {
    if register >= u32::from(max_stack) {
        return Err(format!(
            "Lua 5.3 {context} register {register} is outside max stack {max_stack}"
        ));
    }
    Ok(())
}

fn validate_register_range(
    start: u32,
    count: u32,
    max_stack: u8,
    context: &str,
) -> Result<(), String> {
    if count == 0 {
        return Ok(());
    }
    let end = start
        .checked_add(count - 1)
        .ok_or_else(|| format!("Lua 5.3 {context} register range overflows"))?;
    validate_register(start, max_stack, context)?;
    validate_register(end, max_stack, context)
}

fn validate_rk(
    operand: u32,
    constants_len: usize,
    max_stack: u8,
    context: &str,
) -> Result<(), String> {
    if operand & BITRK != 0 {
        let index = operand & !BITRK;
        if index as usize >= constants_len {
            return Err(format!(
                "Lua 5.3 {context} constant index {index} is out of bounds"
            ));
        }
        Ok(())
    } else {
        validate_register(operand, max_stack, context)
    }
}

fn validate_code53(
    raw: &[u32],
    constants_len: usize,
    upvalue_count: usize,
    child_count: usize,
    max_stack: u8,
) -> Result<(), String> {
    for (pc, &instruction) in raw.iter().enumerate() {
        let op = op53(instruction);
        let a = a53(instruction);
        let b = b53(instruction);
        let c = c53(instruction);
        match op {
            OP_MOVE => {
                validate_register(a, max_stack, "MOVE destination")?;
                validate_register(b, max_stack, "MOVE source")?;
            }
            OP_LOADK => {
                validate_register(a, max_stack, "LOADK destination")?;
                let index = bx53(instruction);
                if index as usize >= constants_len {
                    return Err(format!(
                        "Lua 5.3 LOADK constant index {index} is out of bounds"
                    ));
                }
            }
            OP_LOADKX => {
                validate_register(a, max_stack, "LOADKX destination")?;
                let extra = raw
                    .get(pc + 1)
                    .copied()
                    .ok_or_else(|| "LOADKX is missing EXTRAARG".to_string())?;
                if op53(extra) != OP_EXTRAARG {
                    return Err("LOADKX is not followed by EXTRAARG".to_string());
                }
                let index = ax53(extra);
                if index as usize >= constants_len {
                    return Err(format!(
                        "Lua 5.3 LOADKX constant index {index} is out of bounds"
                    ));
                }
            }
            OP_LOADBOOL => {
                validate_register(a, max_stack, "LOADBOOL destination")?;
                if c != 0 && pc + 1 >= raw.len() {
                    return Err("Lua 5.3 LOADBOOL skip has no following instruction".to_string());
                }
            }
            OP_LOADNIL => validate_register_range(a, b + 1, max_stack, "LOADNIL")?,
            OP_GETUPVAL => {
                validate_register(a, max_stack, "GETUPVAL destination")?;
                if b as usize >= upvalue_count {
                    return Err(format!("Lua 5.3 GETUPVAL index {b} is out of bounds"));
                }
            }
            OP_GETTABUP => {
                validate_register(a, max_stack, "GETTABUP destination")?;
                if b as usize >= upvalue_count {
                    return Err(format!(
                        "Lua 5.3 GETTABUP upvalue index {b} is out of bounds"
                    ));
                }
                validate_rk(c, constants_len, max_stack, "GETTABUP key")?;
            }
            OP_GETTABLE => {
                validate_register(a, max_stack, "GETTABLE destination")?;
                validate_register(b, max_stack, "GETTABLE table")?;
                validate_rk(c, constants_len, max_stack, "GETTABLE key")?;
            }
            OP_SETTABUP => {
                if a as usize >= upvalue_count {
                    return Err(format!(
                        "Lua 5.3 SETTABUP upvalue index {a} is out of bounds"
                    ));
                }
                validate_rk(b, constants_len, max_stack, "SETTABUP key")?;
                validate_rk(c, constants_len, max_stack, "SETTABUP value")?;
            }
            OP_SETUPVAL => {
                validate_register(a, max_stack, "SETUPVAL source")?;
                if b as usize >= upvalue_count {
                    return Err(format!("Lua 5.3 SETUPVAL index {b} is out of bounds"));
                }
            }
            OP_SETTABLE => {
                validate_register(a, max_stack, "SETTABLE table")?;
                validate_rk(b, constants_len, max_stack, "SETTABLE key")?;
                validate_rk(c, constants_len, max_stack, "SETTABLE value")?;
            }
            OP_NEWTABLE => validate_register(a, max_stack, "NEWTABLE destination")?,
            OP_SELF => {
                validate_register_range(a, 2, max_stack, "SELF destination")?;
                validate_register(b, max_stack, "SELF object")?;
                validate_rk(c, constants_len, max_stack, "SELF key")?;
            }
            OP_UNM | OP_BNOT | OP_NOT | OP_LEN => {
                validate_register(a, max_stack, "unary destination")?;
                validate_register(b, max_stack, "unary operand")?;
            }
            OP_CONCAT => {
                validate_register(a, max_stack, "CONCAT destination")?;
                if c < b {
                    return Err("invalid Lua 5.3 CONCAT register range".to_string());
                }
                validate_register_range(b, c - b + 1, max_stack, "CONCAT operands")?;
            }
            OP_JMP => {
                if a != 0 {
                    validate_register(a - 1, max_stack, "JMP close level")?;
                }
            }
            OP_EQ | OP_LT | OP_LE => {
                validate_rk(b, constants_len, max_stack, "comparison left operand")?;
                validate_rk(c, constants_len, max_stack, "comparison right operand")?;
                if pc + 1 >= raw.len() {
                    return Err("Lua 5.3 comparison has no following instruction".to_string());
                }
            }
            OP_TEST => {
                validate_register(a, max_stack, "TEST operand")?;
                if pc + 1 >= raw.len() {
                    return Err("Lua 5.3 TEST has no following instruction".to_string());
                }
            }
            OP_TESTSET => {
                validate_register(a, max_stack, "TESTSET destination")?;
                validate_register(b, max_stack, "TESTSET source")?;
                if pc + 1 >= raw.len() {
                    return Err("Lua 5.3 TESTSET has no following instruction".to_string());
                }
            }
            OP_CALL | OP_TAILCALL => {
                validate_register(a, max_stack, "CALL function")?;
                if b != 0 {
                    validate_register_range(a, b, max_stack, "CALL function and arguments")?;
                }
                if op == OP_CALL && c > 1 {
                    validate_register_range(a, c - 1, max_stack, "CALL results")?;
                }
            }
            OP_RETURN => {
                if b == 0 {
                    if a > u32::from(max_stack) {
                        return Err(format!(
                            "Lua 5.3 RETURN base register {a} exceeds max stack {max_stack}"
                        ));
                    }
                } else if b > 1 {
                    validate_register_range(a, b - 1, max_stack, "RETURN values")?;
                }
            }
            OP_FORLOOP | OP_FORPREP => {
                validate_register_range(a, 4, max_stack, "numeric for state")?;
            }
            OP_TFORCALL => {
                validate_register_range(a, 3, max_stack, "generic for state")?;
                if c != 0 {
                    validate_register_range(a + 3, c, max_stack, "generic for results")?;
                }
                if raw.get(pc + 1).copied().map(op53) != Some(OP_TFORLOOP) {
                    return Err("TFORCALL is not followed by TFORLOOP".to_string());
                }
            }
            OP_TFORLOOP => {
                if a < 2 {
                    return Err("invalid Lua 5.3 TFORLOOP register".to_string());
                }
                validate_register_range(a, 2, max_stack, "generic for loop")?;
            }
            OP_SETLIST => {
                validate_register(a, max_stack, "SETLIST table")?;
                if b != 0 {
                    validate_register_range(a + 1, b, max_stack, "SETLIST values")?;
                }
                if c == 0 && raw.get(pc + 1).copied().map(op53) != Some(OP_EXTRAARG) {
                    return Err("SETLIST is not followed by EXTRAARG".to_string());
                }
            }
            OP_CLOSURE => {
                validate_register(a, max_stack, "CLOSURE destination")?;
                let index = bx53(instruction);
                if index as usize >= child_count {
                    return Err(format!(
                        "Lua 5.3 CLOSURE prototype index {index} is out of bounds"
                    ));
                }
            }
            OP_VARARG => {
                validate_register(a, max_stack, "VARARG destination")?;
                if b > 1 {
                    validate_register_range(a, b - 1, max_stack, "VARARG results")?;
                }
            }
            OP_EXTRAARG => {
                if pc == 0 || !matches!(op53(raw[pc - 1]), OP_LOADKX | OP_SETLIST) {
                    return Err("orphan Lua 5.3 EXTRAARG instruction".to_string());
                }
                if op53(raw[pc - 1]) == OP_SETLIST && c53(raw[pc - 1]) != 0 {
                    return Err("orphan Lua 5.3 EXTRAARG instruction".to_string());
                }
            }
            _ if arithmetic53(op).is_some() => {
                validate_register(a, max_stack, "arithmetic destination")?;
                validate_rk(b, constants_len, max_stack, "arithmetic left operand")?;
                validate_rk(c, constants_len, max_stack, "arithmetic right operand")?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum PatchKind {
    Jump,
    ForwardBx,
    BackwardBx,
}

struct Patch {
    instruction: usize,
    target_old: isize,
    kind: PatchKind,
}

fn arithmetic53(op: u8) -> Option<(OpCode, TmKind)> {
    Some(match op {
        OP_ADD => (OpCode::Add, TmKind::Add),
        OP_SUB => (OpCode::Sub, TmKind::Sub),
        OP_MUL => (OpCode::Mul, TmKind::Mul),
        OP_MOD => (OpCode::Mod, TmKind::Mod),
        OP_POW => (OpCode::Pow, TmKind::Pow),
        OP_DIV => (OpCode::Div, TmKind::Div),
        OP_IDIV => (OpCode::IDiv, TmKind::IDiv),
        OP_BAND => (OpCode::BAnd, TmKind::Band),
        OP_BOR => (OpCode::BOr, TmKind::Bor),
        OP_BXOR => (OpCode::BXor, TmKind::Bxor),
        OP_SHL => (OpCode::Shl, TmKind::Shl),
        OP_SHR => (OpCode::Shr, TmKind::Shr),
        _ => return None,
    })
}

fn translate_code53(
    raw: &[u32],
    constants_len: usize,
    upvalue_count: usize,
    child_count: usize,
    old_lines: &[u32],
    old_max_stack: u8,
    is_vararg: bool,
) -> Result<(Vec<Instruction>, Vec<u32>, Vec<usize>, u8), String> {
    validate_code53(
        raw,
        constants_len,
        upvalue_count,
        child_count,
        old_max_stack,
    )?;
    let scratch = old_max_stack as u32;
    let mut max_stack = old_max_stack as u32;
    let mut code = Vec::with_capacity(raw.len() + usize::from(is_vararg));
    let mut lines = Vec::with_capacity(raw.len() + usize::from(is_vararg));
    let mut map = vec![0usize; raw.len() + 1];
    let mut patches = Vec::new();

    if is_vararg {
        code.push(Instruction::create_abck(OpCode::VarargPrep, 0, 0, 0, false));
        lines.push(old_lines.first().copied().unwrap_or(0));
    }

    for (pc, &instruction) in raw.iter().enumerate() {
        map[pc] = code.len();
        let line = old_lines.get(pc).copied().unwrap_or(0);
        let op = op53(instruction);
        let a = a53(instruction);
        let b = b53(instruction);
        let c = c53(instruction);
        let mut emitted = Vec::new();

        match op {
            OP_MOVE => emitted.push(Instruction::create_abck(OpCode::Move, a, b, 0, false)),
            OP_LOADK => emit_loadk(&mut emitted, a, bx53(instruction))?,
            OP_LOADKX => emitted.push(Instruction::create_abx(OpCode::LoadKX, a, 0)),
            OP_LOADBOOL => {
                emitted.push(Instruction::create_abck(
                    if b == 0 {
                        OpCode::LoadFalse
                    } else {
                        OpCode::LoadTrue
                    },
                    a,
                    0,
                    0,
                    false,
                ));
                if c != 0 {
                    let at = code.len() + emitted.len();
                    emitted.push(Instruction::create_sj(OpCode::Jmp, 0));
                    patches.push(Patch {
                        instruction: at,
                        target_old: pc as isize + 2,
                        kind: PatchKind::Jump,
                    });
                }
            }
            OP_LOADNIL => emitted.push(Instruction::create_abck(OpCode::LoadNil, a, b, 0, false)),
            OP_GETUPVAL => emitted.push(Instruction::create_abck(OpCode::GetUpval, a, b, 0, false)),
            OP_SETUPVAL => emitted.push(Instruction::create_abck(OpCode::SetUpval, a, b, 0, false)),
            OP_GETTABUP => {
                if c & BITRK != 0 && (c & !BITRK) <= u8::MAX as u32 {
                    emitted.push(Instruction::create_abck(
                        OpCode::GetTabUp,
                        a,
                        b,
                        c & !BITRK,
                        false,
                    ));
                } else {
                    let key = load_rk(&mut emitted, constants_len, c, scratch)?;
                    emitted.push(Instruction::create_abck(
                        OpCode::GetUpval,
                        scratch + 1,
                        b,
                        0,
                        false,
                    ));
                    emitted.push(Instruction::create_abck(
                        OpCode::GetTable,
                        a,
                        scratch + 1,
                        key,
                        false,
                    ));
                    max_stack = max_stack.max(scratch + 2);
                }
            }
            OP_GETTABLE => {
                let key = load_rk(&mut emitted, constants_len, c, scratch)?;
                max_stack = max_stack.max(scratch + 1);
                emitted.push(Instruction::create_abck(OpCode::GetTable, a, b, key, false));
            }
            OP_SETTABUP => {
                if b & BITRK != 0 && (b & !BITRK) <= u8::MAX as u32 {
                    let constant_value = c & BITRK != 0 && (c & !BITRK) <= u8::MAX as u32;
                    let value = if constant_value {
                        c & !BITRK
                    } else {
                        load_rk(&mut emitted, constants_len, c, scratch)?
                    };
                    emitted.push(Instruction::create_abck(
                        OpCode::SetTabUp,
                        a,
                        b & !BITRK,
                        value,
                        constant_value,
                    ));
                    max_stack = max_stack.max(scratch + 1);
                } else {
                    let key = load_rk(&mut emitted, constants_len, b, scratch)?;
                    let value = load_rk(&mut emitted, constants_len, c, scratch + 1)?;
                    emitted.push(Instruction::create_abck(
                        OpCode::GetUpval,
                        scratch + 2,
                        a,
                        0,
                        false,
                    ));
                    emitted.push(Instruction::create_abck(
                        OpCode::SetTable,
                        scratch + 2,
                        key,
                        value,
                        false,
                    ));
                    max_stack = max_stack.max(scratch + 3);
                }
            }
            OP_SETTABLE => {
                let key = load_rk(&mut emitted, constants_len, b, scratch)?;
                let value = load_rk(&mut emitted, constants_len, c, scratch + 1)?;
                max_stack = max_stack.max(scratch + 2);
                emitted.push(Instruction::create_abck(
                    OpCode::SetTable,
                    a,
                    key,
                    value,
                    false,
                ));
            }
            OP_NEWTABLE => {
                let array_size = fb2int(b);
                let hash_size = fb2int(c);
                let hash_bits = if hash_size == 0 {
                    0
                } else {
                    32 - hash_size.saturating_sub(1).leading_zeros() + 1
                };
                let low = array_size & 0x3ff;
                let extra = array_size >> 10;
                emitted.push(Instruction::create_vabck(
                    OpCode::NewTable,
                    a,
                    hash_bits,
                    low,
                    extra != 0,
                ));
                // The native VM reserves the following instruction for the
                // extended array-size payload even when the low field suffices.
                emitted.push(Instruction::create_ax(OpCode::ExtraArg, extra));
            }
            OP_SELF => {
                if c & BITRK != 0 {
                    emitted.push(Instruction::create_abck(
                        OpCode::Self_,
                        a,
                        b,
                        c & !BITRK,
                        false,
                    ));
                } else {
                    emitted.push(Instruction::create_abck(OpCode::Move, a + 1, b, 0, false));
                    emitted.push(Instruction::create_abck(OpCode::GetTable, a, b, c, false));
                }
            }
            OP_UNM | OP_BNOT | OP_NOT | OP_LEN => {
                let native = match op {
                    OP_UNM => OpCode::Unm,
                    OP_BNOT => OpCode::BNot,
                    OP_NOT => OpCode::Not,
                    _ => OpCode::Len,
                };
                emitted.push(Instruction::create_abck(native, a, b, 0, false));
            }
            OP_CONCAT => {
                if c < b {
                    return Err("invalid Lua 5.3 CONCAT register range".to_string());
                }
                let count = c - b + 1;
                if count > 255 {
                    return Err("Lua 5.3 CONCAT has too many operands".to_string());
                }
                for offset in 0..count {
                    emitted.push(Instruction::create_abck(
                        OpCode::Move,
                        scratch + offset,
                        b + offset,
                        0,
                        false,
                    ));
                }
                max_stack = max_stack.max(scratch + count);
                emitted.push(Instruction::create_abck(OpCode::Concat, a, count, 0, false));
            }
            OP_JMP => {
                if a != 0 {
                    emitted.push(Instruction::create_abck(OpCode::Close, a - 1, 0, 0, false));
                }
                let at = code.len() + emitted.len();
                emitted.push(Instruction::create_sj(OpCode::Jmp, 0));
                patches.push(Patch {
                    instruction: at,
                    target_old: pc as isize + 1 + sbx53(instruction) as isize,
                    kind: PatchKind::Jump,
                });
            }
            OP_EQ | OP_LT | OP_LE => {
                let left = load_rk(&mut emitted, constants_len, b, scratch)?;
                let right = load_rk(&mut emitted, constants_len, c, scratch + 1)?;
                max_stack = max_stack.max(scratch + 2);
                let native = match op {
                    OP_EQ => OpCode::Eq,
                    OP_LT => OpCode::Lt,
                    _ => OpCode::Le,
                };
                emitted.push(Instruction::create_abck(native, left, right, 0, a != 0));
            }
            OP_TEST => emitted.push(Instruction::create_abck(OpCode::Test, a, 0, 0, c != 0)),
            OP_TESTSET => emitted.push(Instruction::create_abck(OpCode::TestSet, a, b, 0, c != 0)),
            OP_CALL => emitted.push(Instruction::create_abck(OpCode::Call, a, b, c, false)),
            OP_TAILCALL => emitted.push(Instruction::create_abck(OpCode::TailCall, a, b, c, true)),
            OP_RETURN => emitted.push(Instruction::create_abck(OpCode::Return, a, b, 0, true)),
            OP_FORLOOP | OP_FORPREP | OP_TFORLOOP => {
                let native = match op {
                    OP_FORLOOP => OpCode::ForLoop53,
                    OP_FORPREP => OpCode::ForPrep53,
                    _ => OpCode::TForLoop53,
                };
                let loop_base = if op == OP_TFORLOOP {
                    a.checked_sub(2)
                        .ok_or_else(|| "invalid Lua 5.3 TFORLOOP register".to_string())?
                } else {
                    a
                };
                let at = code.len() + emitted.len();
                emitted.push(Instruction::create_abx(native, loop_base, 0));
                patches.push(Patch {
                    instruction: at,
                    target_old: pc as isize + 1 + sbx53(instruction) as isize,
                    kind: if op == OP_FORPREP {
                        PatchKind::ForwardBx
                    } else {
                        PatchKind::BackwardBx
                    },
                });
            }
            OP_TFORCALL => {
                emitted.push(Instruction::create_abck(OpCode::TForCall53, a, 0, c, false))
            }
            OP_SETLIST => {
                let block = if c == 0 {
                    let extra = raw
                        .get(pc + 1)
                        .copied()
                        .ok_or_else(|| "SETLIST is missing EXTRAARG".to_string())?;
                    if op53(extra) != OP_EXTRAARG {
                        return Err("SETLIST is not followed by EXTRAARG".to_string());
                    }
                    ax53(extra)
                } else {
                    c
                };
                if block == 0 {
                    return Err("invalid zero Lua 5.3 SETLIST block".to_string());
                }
                let start = (block - 1)
                    .checked_mul(50)
                    .ok_or_else(|| "Lua 5.3 SETLIST index overflow".to_string())?;
                let low = start & 0x3ff;
                let extra = start >> 10;
                emitted.push(Instruction::create_vabck(
                    OpCode::SetList,
                    a,
                    b,
                    low,
                    extra != 0,
                ));
                if extra != 0 {
                    emitted.push(Instruction::create_ax(OpCode::ExtraArg, extra));
                }
            }
            OP_CLOSURE => emitted.push(Instruction::create_abx(
                OpCode::Closure,
                a,
                bx53(instruction),
            )),
            OP_VARARG => emitted.push(Instruction::create_abck(OpCode::Vararg, a, 0, b, false)),
            OP_EXTRAARG => {
                if pc == 0 || !matches!(op53(raw[pc - 1]), OP_LOADKX | OP_SETLIST) {
                    return Err("orphan Lua 5.3 EXTRAARG instruction".to_string());
                }
                if op53(raw[pc - 1]) == OP_LOADKX {
                    emitted.push(Instruction::create_ax(OpCode::ExtraArg, ax53(instruction)));
                }
            }
            _ => {
                if let Some((native, event)) = arithmetic53(op) {
                    let left = load_rk(&mut emitted, constants_len, b, scratch)?;
                    let right = load_rk(&mut emitted, constants_len, c, scratch + 1)?;
                    max_stack = max_stack.max(scratch + 2);
                    emitted.push(Instruction::create_abck(native, a, left, right, false));
                    emitted.push(Instruction::create_abck(
                        OpCode::MmBin,
                        left,
                        right,
                        event as u32,
                        false,
                    ));
                } else {
                    return Err(format!("unsupported Lua 5.3 opcode {op}"));
                }
            }
        }

        lines.extend(std::iter::repeat_n(line, emitted.len()));
        code.extend(emitted);
    }
    map[raw.len()] = code.len();

    for patch in patches {
        if patch.target_old < 0 || patch.target_old as usize > raw.len() {
            return Err("Lua 5.3 jump target is outside the function".to_string());
        }
        let target = map[patch.target_old as usize];
        let instruction = code
            .get_mut(patch.instruction)
            .ok_or_else(|| "internal Lua 5.3 jump patch error".to_string())?;
        match patch.kind {
            PatchKind::Jump => {
                let offset = target as isize - patch.instruction as isize - 1;
                let max = (Instruction::MAX_SJ >> 1) as isize;
                if !(-max..=max).contains(&offset) {
                    return Err("translated Lua 5.3 jump is too long".to_string());
                }
                instruction.set_sj(offset as i32);
            }
            PatchKind::ForwardBx => {
                let distance = target
                    .checked_sub(patch.instruction + 1)
                    .ok_or_else(|| "Lua 5.3 forward loop has a backward target".to_string())?;
                if distance > Instruction::MAX_BX as usize {
                    return Err("translated Lua 5.3 loop is too long".to_string());
                }
                Instruction::set_bx(instruction, distance as u32);
            }
            PatchKind::BackwardBx => {
                let distance = (patch.instruction + 1)
                    .checked_sub(target)
                    .ok_or_else(|| "Lua 5.3 backward loop has a forward target".to_string())?;
                if distance > Instruction::MAX_BX as usize {
                    return Err("translated Lua 5.3 loop is too long".to_string());
                }
                Instruction::set_bx(instruction, distance as u32);
            }
        }
    }

    let max_stack = u8::try_from(max_stack).map_err(|_| {
        "Lua 5.3 function needs more than 255 registers after translation".to_string()
    })?;
    Ok((code, lines, map, max_stack))
}

fn parse_header(reader: &mut Reader<'_>) -> Result<u8, String> {
    if reader.bytes(4)? != SIGNATURE {
        return Err("not a standard Lua binary chunk".to_string());
    }
    if reader.byte()? != VERSION {
        return Err("binary chunk is not Lua 5.3".to_string());
    }
    if reader.byte()? != FORMAT {
        return Err("unsupported Lua 5.3 binary chunk format".to_string());
    }
    if reader.bytes(6)? != DATA {
        return Err("invalid Lua 5.3 binary chunk header".to_string());
    }

    reader.int_size = reader.byte()? as usize;
    reader.size_t_size = reader.byte()? as usize;
    reader.instruction_size = reader.byte()? as usize;
    reader.integer_size = reader.byte()? as usize;
    reader.number_size = reader.byte()? as usize;
    if reader.instruction_size != 4 || reader.integer_size != 8 || reader.number_size != 8 {
        return Err(format!(
            "unsupported Lua 5.3 binary layout (instruction={}, integer={}, number={})",
            reader.instruction_size, reader.integer_size, reader.number_size
        ));
    }
    if !matches!(reader.int_size, 4 | 8) || !matches!(reader.size_t_size, 4 | 8) {
        return Err("unsupported Lua 5.3 int or size_t width".to_string());
    }

    let probe = reader.bytes(reader.integer_size)?;
    let mut little = [0u8; 8];
    little.copy_from_slice(probe);
    let mut big = [0u8; 8];
    big.copy_from_slice(probe);
    if i64::from_le_bytes(little) == LUAC_INT {
        reader.endian = Endian::Little;
    } else if i64::from_be_bytes(big) == LUAC_INT {
        reader.endian = Endian::Big;
    } else {
        return Err("Lua 5.3 chunk uses an incompatible integer representation".to_string());
    }
    let number = reader.lua_number()?;
    if number.to_bits() != LUAC_NUM.to_bits() {
        return Err("Lua 5.3 chunk uses an incompatible floating-point representation".to_string());
    }
    reader.byte()
}

fn read_proto(
    reader: &mut Reader<'_>,
    vm: &mut GlobalState,
    parent_source: Option<&str>,
    depth: usize,
) -> Result<LuaProto, String> {
    if depth > MAX_PROTO_DEPTH {
        return Err("Lua 5.3 prototype nesting exceeds the safety limit".to_string());
    }
    let source_bytes = reader.string()?;
    let source = source_bytes
        .as_deref()
        .map(String::from_utf8_lossy)
        .map(|s| s.into_owned())
        .or_else(|| parent_source.map(str::to_owned))
        .unwrap_or_default();
    let line_defined = reader.int()?.max(0) as u32;
    let last_line_defined = reader.int()?.max(0) as u32;
    let param_count = reader.byte()?;
    let is_vararg = reader.byte()? != 0;
    let old_max_stack = reader.byte()?;
    if usize::from(param_count) > usize::from(old_max_stack) {
        return Err("Lua 5.3 parameter count exceeds max stack size".to_string());
    }

    let code_count = reader.count("instruction")?;
    let mut raw_code = Vec::with_capacity(code_count);
    for _ in 0..code_count {
        raw_code.push(reader.uint32()?);
    }

    let constant_count = reader.count("constant")?;
    let mut constants = Vec::with_capacity(constant_count);
    for _ in 0..constant_count {
        let value = match reader.byte()? {
            0 => LuaValue::nil(),
            1 => LuaValue::boolean(reader.byte()? != 0),
            3 => LuaValue::number(reader.lua_number()?),
            19 => LuaValue::integer(reader.lua_integer()?),
            4 | 20 => {
                let bytes = reader
                    .string()?
                    .ok_or_else(|| "nil Lua 5.3 string constant".to_string())?;
                vm.create_bytes(&bytes).map_err(|error| error.to_string())?
            }
            tag => return Err(format!("unknown Lua 5.3 constant tag {tag}")),
        };
        constants.push(value);
    }

    let upvalue_count = reader.count("upvalue")?;
    let mut upvalue_descs = Vec::with_capacity(upvalue_count);
    for _ in 0..upvalue_count {
        upvalue_descs.push(UpvalueDesc {
            name: String::new(),
            is_local: reader.byte()? != 0,
            index: reader.byte()? as u32,
        });
    }

    let child_count = reader.count("child prototype")?;
    let mut child_protos = Vec::with_capacity(child_count);
    for _ in 0..child_count {
        let child = read_proto(reader, vm, Some(&source), depth + 1)?;
        child_protos.push(vm.create_proto(child).map_err(|error| error.to_string())?);
    }

    let line_count = reader.count("line")?;
    let mut old_lines = Vec::with_capacity(line_count);
    for _ in 0..line_count {
        old_lines.push(reader.int()?.max(0) as u32);
    }
    if !old_lines.is_empty() && old_lines.len() != raw_code.len() {
        return Err("Lua 5.3 line table length does not match the code".to_string());
    }

    let locvar_count = reader.count("local variable")?;
    let mut raw_locals = Vec::with_capacity(locvar_count);
    for _ in 0..locvar_count {
        let name = reader
            .string()?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let start = reader.int()?.max(0) as usize;
        let end = reader.int()?.max(0) as usize;
        raw_locals.push((name, start, end));
    }

    let upvalue_name_count = reader.count("upvalue name")?;
    if upvalue_name_count > upvalue_descs.len() {
        return Err("Lua 5.3 chunk has more upvalue names than upvalues".to_string());
    }
    for descriptor in upvalue_descs.iter_mut().take(upvalue_name_count) {
        descriptor.name = reader
            .string()?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
    }

    let (code, line_info, pc_map, max_stack_size) = translate_code53(
        &raw_code,
        constants.len(),
        upvalue_count,
        child_count,
        &old_lines,
        old_max_stack,
        is_vararg,
    )?;
    let locvars = raw_locals
        .into_iter()
        .map(|(name, start, end)| LocVar {
            name,
            // Parameters start at wire PC 0 and remain active before the
            // synthetic native VARARGPREP inserted by this loader.
            startpc: if start == 0 {
                0
            } else {
                pc_map.get(start).copied().unwrap_or(code.len()) as u32
            },
            endpc: pc_map.get(end).copied().unwrap_or(code.len()) as u32,
        })
        .collect();

    let mut proto = LuaProto {
        code,
        constants,
        locals: locvars,
        upvalue_count,
        param_count: param_count as usize,
        is_vararg,
        needs_vararg_table: false,
        use_hidden_vararg: false,
        max_stack_size: max_stack_size as usize,
        child_protos,
        upvalue_descs,
        source_name: if source.is_empty() {
            None
        } else {
            Some(std::sync::Arc::<str>::from(source))
        },
        line_info,
        linedefined: line_defined as usize,
        lastlinedefined: last_line_defined as usize,
        proto_data_size: 0,
    };
    proto.compute_proto_data_size();
    Ok(proto)
}

pub fn load(data: &[u8], vm: &mut GlobalState) -> Result<LuaProto, String> {
    let mut reader = Reader::new(data)?;
    let main_upvalues = parse_header(&mut reader)? as usize;
    let proto = read_proto(&mut reader, vm, None, 0)?;
    if proto.upvalue_descs.len() != main_upvalues {
        return Err("Lua 5.3 main upvalue count does not match the header".to_string());
    }
    if reader.at != data.len() {
        return Err("trailing data after Lua 5.3 binary chunk".to_string());
    }
    Ok(proto)
}

#[inline]
fn abc53(op: u8, a: u32, b: u32, c: u32) -> Result<u32, String> {
    if op > 0x3f || a > 0xff || b > 0x1ff || c > 0x1ff {
        return Err("instruction operand does not fit the Lua 5.3 wire format".to_string());
    }
    Ok(op as u32 | (a << 6) | (c << 14) | (b << 23))
}

#[inline]
fn abx53(op: u8, a: u32, bx: u32) -> Result<u32, String> {
    if op > 0x3f || a > 0xff || bx > MAXARG_BX53 as u32 {
        return Err("instruction operand does not fit the Lua 5.3 wire format".to_string());
    }
    Ok(op as u32 | (a << 6) | (bx << 14))
}

#[inline]
fn asbx53(op: u8, a: u32, sbx: i32) -> Result<u32, String> {
    if !(-MAXARG_SBX53..=MAXARG_SBX53).contains(&sbx) {
        return Err("jump is too long for a Lua 5.3 binary chunk".to_string());
    }
    abx53(op, a, (sbx + MAXARG_SBX53) as u32)
}

#[inline]
fn ax53_wire(op: u8, ax: u32) -> Result<u32, String> {
    if op > 0x3f || ax >= (1 << 26) {
        return Err("EXTRAARG does not fit the Lua 5.3 wire format".to_string());
    }
    Ok(op as u32 | (ax << 6))
}

fn push_loadk53(out: &mut Vec<u32>, register: u32, index: u32) -> Result<(), String> {
    if index <= MAXARG_BX53 as u32 {
        out.push(abx53(OP_LOADK, register, index)?);
    } else if index < (1 << 26) {
        out.push(abx53(OP_LOADKX, register, 0)?);
        out.push(ax53_wire(OP_EXTRAARG, index)?);
    } else {
        return Err("constant index exceeds the Lua 5.3 wire limit".to_string());
    }
    Ok(())
}

fn add_constant(constants: &mut Vec<LuaValue>, value: LuaValue) -> Result<u32, String> {
    let index = u32::try_from(constants.len())
        .map_err(|_| "too many constants for a Lua 5.3 chunk".to_string())?;
    constants.push(value);
    Ok(index)
}

fn rk53(out: &mut Vec<u32>, index: u32, scratch: u32, max_stack: &mut u32) -> Result<u32, String> {
    if index < BITRK {
        Ok(BITRK | index)
    } else {
        push_loadk53(out, scratch, index)?;
        *max_stack = (*max_stack).max(scratch + 1);
        Ok(scratch)
    }
}

fn added_rk53(
    out: &mut Vec<u32>,
    constants: &mut Vec<LuaValue>,
    value: LuaValue,
    scratch: u32,
    max_stack: &mut u32,
) -> Result<u32, String> {
    let index = add_constant(constants, value)?;
    rk53(out, index, scratch, max_stack)
}

struct WirePatch {
    instruction: usize,
    target_native: isize,
}

struct WireFunction {
    code: Vec<u32>,
    constants: Vec<LuaValue>,
    lines: Vec<u32>,
    pc_map: Vec<usize>,
    max_stack_size: u8,
}

fn native_arithmetic(op: OpCode) -> Option<u8> {
    Some(match op {
        OpCode::Add | OpCode::AddK | OpCode::AddI => OP_ADD,
        OpCode::Sub | OpCode::SubK => OP_SUB,
        OpCode::Mul | OpCode::MulK => OP_MUL,
        OpCode::Mod | OpCode::ModK => OP_MOD,
        OpCode::Pow | OpCode::PowK => OP_POW,
        OpCode::Div | OpCode::DivK => OP_DIV,
        OpCode::IDiv | OpCode::IDivK => OP_IDIV,
        OpCode::BAnd | OpCode::BAndK => OP_BAND,
        OpCode::BOr | OpCode::BOrK => OP_BOR,
        OpCode::BXor | OpCode::BXorK => OP_BXOR,
        OpCode::Shl | OpCode::ShlI => OP_SHL,
        OpCode::Shr | OpCode::ShrI => OP_SHR,
        _ => return None,
    })
}

fn translate_native53(proto: &LuaProto) -> Result<WireFunction, String> {
    let mut constants = proto.constants.clone();
    let mut code = Vec::with_capacity(proto.code.len());
    let mut lines = Vec::with_capacity(proto.code.len());
    let mut pc_map = vec![0usize; proto.code.len() + 1];
    let mut patches = Vec::new();
    let scratch = u32::try_from(proto.max_stack_size)
        .map_err(|_| "native function has an invalid stack size".to_string())?;
    let mut max_stack = scratch;

    for (pc, &instruction) in proto.code.iter().enumerate() {
        pc_map[pc] = code.len();
        let line = proto.line_info.get(pc).copied().unwrap_or(0);
        let start = code.len();
        let op = instruction.get_opcode();
        let a = instruction.get_a();
        let b = instruction.get_b();
        let c = instruction.get_c();

        match op {
            OpCode::Move => code.push(abc53(OP_MOVE, a, b, 0)?),
            OpCode::LoadI => {
                let index = add_constant(
                    &mut constants,
                    LuaValue::integer(instruction.get_sbx() as i64),
                )?;
                push_loadk53(&mut code, a, index)?;
            }
            OpCode::LoadF => {
                let index = add_constant(
                    &mut constants,
                    LuaValue::number(instruction.get_sbx() as f64),
                )?;
                push_loadk53(&mut code, a, index)?;
            }
            OpCode::LoadK => push_loadk53(&mut code, a, instruction.get_bx())?,
            OpCode::LoadKX => code.push(abx53(OP_LOADKX, a, 0)?),
            OpCode::LoadFalse => code.push(abc53(OP_LOADBOOL, a, 0, 0)?),
            OpCode::LFalseSkip => {
                code.push(abc53(OP_LOADBOOL, a, 0, 0)?);
                let at = code.len();
                code.push(asbx53(OP_JMP, 0, 0)?);
                patches.push(WirePatch {
                    instruction: at,
                    target_native: pc as isize + 2,
                });
            }
            OpCode::LoadTrue => code.push(abc53(OP_LOADBOOL, a, 1, 0)?),
            OpCode::LoadNil => code.push(abc53(OP_LOADNIL, a, b, 0)?),
            OpCode::GetUpval => code.push(abc53(OP_GETUPVAL, a, b, 0)?),
            OpCode::SetUpval => code.push(abc53(OP_SETUPVAL, a, b, 0)?),
            OpCode::GetTabUp => {
                let key = instruction.get_c();
                if key < BITRK {
                    code.push(abc53(OP_GETTABUP, a, b, BITRK | key)?);
                } else {
                    let key_reg = rk53(&mut code, key, scratch, &mut max_stack)?;
                    code.push(abc53(OP_GETUPVAL, scratch + 1, b, 0)?);
                    code.push(abc53(OP_GETTABLE, a, scratch + 1, key_reg)?);
                    max_stack = max_stack.max(scratch + 2);
                }
            }
            OpCode::GetTable => code.push(abc53(OP_GETTABLE, a, b, c)?),
            OpCode::GetI => {
                let key = added_rk53(
                    &mut code,
                    &mut constants,
                    LuaValue::integer(c as i64),
                    scratch,
                    &mut max_stack,
                )?;
                code.push(abc53(OP_GETTABLE, a, b, key)?);
            }
            OpCode::GetField => code.push(abc53(OP_GETTABLE, a, b, BITRK | c)?),
            OpCode::SetTabUp => {
                let value = if instruction.get_k() { BITRK | c } else { c };
                code.push(abc53(OP_SETTABUP, a, BITRK | b, value)?);
            }
            OpCode::SetTable => {
                let value = if instruction.get_k() { BITRK | c } else { c };
                code.push(abc53(OP_SETTABLE, a, b, value)?);
            }
            OpCode::SetI => {
                let key = added_rk53(
                    &mut code,
                    &mut constants,
                    LuaValue::integer(b as i64),
                    scratch,
                    &mut max_stack,
                )?;
                let value = if instruction.get_k() { BITRK | c } else { c };
                code.push(abc53(OP_SETTABLE, a, key, value)?);
            }
            OpCode::SetField => {
                let value = if instruction.get_k() { BITRK | c } else { c };
                code.push(abc53(OP_SETTABLE, a, BITRK | b, value)?);
            }
            OpCode::NewTable => {
                let mut array_size = instruction.get_vc();
                if instruction.get_k() {
                    let extra = proto
                        .code
                        .get(pc + 1)
                        .ok_or_else(|| "NEWTABLE is missing EXTRAARG".to_string())?;
                    if extra.get_opcode() != OpCode::ExtraArg {
                        return Err("NEWTABLE is not followed by EXTRAARG".to_string());
                    }
                    array_size = array_size
                        .checked_add(extra.get_ax() << Instruction::SIZE_V_C)
                        .ok_or_else(|| "NEWTABLE array size overflow".to_string())?;
                }
                let hash_bits = instruction.get_vb();
                let hash_size = if hash_bits == 0 {
                    0
                } else {
                    1u32.checked_shl(hash_bits - 1).unwrap_or(u32::MAX)
                };
                code.push(abc53(
                    OP_NEWTABLE,
                    a,
                    int2fb(array_size),
                    int2fb(hash_size),
                )?);
            }
            OpCode::Self_ => code.push(abc53(OP_SELF, a, b, BITRK | c)?),
            OpCode::Unm | OpCode::BNot | OpCode::Not | OpCode::Len => {
                let wire = match op {
                    OpCode::Unm => OP_UNM,
                    OpCode::BNot => OP_BNOT,
                    OpCode::Not => OP_NOT,
                    _ => OP_LEN,
                };
                code.push(abc53(wire, a, b, 0)?);
            }
            OpCode::Concat => {
                let count = b;
                if count == 0 {
                    return Err("cannot encode an empty CONCAT in Lua 5.3".to_string());
                }
                code.push(abc53(OP_CONCAT, a, a, a + count - 1)?);
            }
            OpCode::Close => code.push(asbx53(OP_JMP, a + 1, 0)?),
            OpCode::Jmp => {
                let at = code.len();
                code.push(asbx53(OP_JMP, 0, 0)?);
                patches.push(WirePatch {
                    instruction: at,
                    target_native: pc as isize + 1 + instruction.get_sj() as isize,
                });
            }
            OpCode::Eq | OpCode::Lt | OpCode::Le => {
                let wire = match op {
                    OpCode::Eq => OP_EQ,
                    OpCode::Lt => OP_LT,
                    _ => OP_LE,
                };
                code.push(abc53(wire, u32::from(instruction.get_k()), a, b)?);
            }
            OpCode::EqK => code.push(abc53(OP_EQ, u32::from(instruction.get_k()), a, BITRK | b)?),
            OpCode::EqI | OpCode::LtI | OpCode::LeI | OpCode::GtI | OpCode::GeI => {
                let immediate = added_rk53(
                    &mut code,
                    &mut constants,
                    LuaValue::integer(instruction.get_sb() as i64),
                    scratch,
                    &mut max_stack,
                )?;
                let (wire, left, right) = match op {
                    OpCode::EqI => (OP_EQ, a, immediate),
                    OpCode::LtI => (OP_LT, a, immediate),
                    OpCode::LeI => (OP_LE, a, immediate),
                    OpCode::GtI => (OP_LT, immediate, a),
                    _ => (OP_LE, immediate, a),
                };
                code.push(abc53(wire, u32::from(instruction.get_k()), left, right)?);
            }
            OpCode::Test => code.push(abc53(OP_TEST, a, 0, u32::from(instruction.get_k()))?),
            OpCode::TestSet => code.push(abc53(OP_TESTSET, a, b, u32::from(instruction.get_k()))?),
            OpCode::Call => code.push(abc53(OP_CALL, a, b, c)?),
            OpCode::TailCall => code.push(abc53(OP_TAILCALL, a, b, c)?),
            OpCode::Return => code.push(abc53(OP_RETURN, a, b, 0)?),
            OpCode::Return0 => code.push(abc53(OP_RETURN, 0, 1, 0)?),
            OpCode::Return1 => code.push(abc53(OP_RETURN, a, 2, 0)?),
            OpCode::ForPrep53 | OpCode::ForLoop53 | OpCode::TForLoop53 => {
                let (wire, target, wire_a) = match op {
                    OpCode::ForPrep53 => (
                        OP_FORPREP,
                        pc as isize + 1 + instruction.get_bx() as isize,
                        a,
                    ),
                    OpCode::ForLoop53 => (
                        OP_FORLOOP,
                        pc as isize + 1 - instruction.get_bx() as isize,
                        a,
                    ),
                    _ => (
                        OP_TFORLOOP,
                        pc as isize + 1 - instruction.get_bx() as isize,
                        a + 2,
                    ),
                };
                let at = code.len();
                code.push(asbx53(wire, wire_a, 0)?);
                patches.push(WirePatch {
                    instruction: at,
                    target_native: target,
                });
            }
            OpCode::TForCall53 => code.push(abc53(OP_TFORCALL, a, 0, c)?),
            OpCode::SetList => {
                let mut start_index = instruction.get_vc();
                if instruction.get_k() {
                    let extra = proto
                        .code
                        .get(pc + 1)
                        .ok_or_else(|| "SETLIST is missing EXTRAARG".to_string())?;
                    if extra.get_opcode() != OpCode::ExtraArg {
                        return Err("SETLIST is not followed by EXTRAARG".to_string());
                    }
                    start_index = start_index
                        .checked_add(extra.get_ax() << Instruction::SIZE_V_C)
                        .ok_or_else(|| "SETLIST index overflow".to_string())?;
                }
                if start_index % 50 != 0 {
                    return Err(
                        "native SETLIST cannot be represented exactly in Lua 5.3".to_string()
                    );
                }
                let block = start_index / 50 + 1;
                if block <= 0x1ff {
                    code.push(abc53(OP_SETLIST, a, instruction.get_vb(), block)?);
                } else {
                    code.push(abc53(OP_SETLIST, a, instruction.get_vb(), 0)?);
                    code.push(ax53_wire(OP_EXTRAARG, block)?);
                }
            }
            OpCode::Closure => code.push(abx53(OP_CLOSURE, a, instruction.get_bx())?),
            OpCode::Vararg => code.push(abc53(OP_VARARG, a, c, 0)?),
            OpCode::ExtraArg => {
                let previous = pc
                    .checked_sub(1)
                    .and_then(|index| proto.code.get(index))
                    .ok_or_else(|| "orphan native EXTRAARG".to_string())?;
                if previous.get_opcode() == OpCode::LoadKX {
                    code.push(ax53_wire(OP_EXTRAARG, instruction.get_ax())?);
                } else if previous.get_opcode() != OpCode::NewTable
                    && !(previous.get_opcode() == OpCode::SetList && previous.get_k())
                {
                    return Err("orphan native EXTRAARG".to_string());
                }
            }
            OpCode::VarargPrep | OpCode::MmBin | OpCode::MmBinI | OpCode::MmBinK => {}
            OpCode::AddI
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
            | OpCode::Shr => {
                let mut wire = native_arithmetic(op).unwrap();
                let (left, right) = match op {
                    OpCode::AddI | OpCode::ShlI | OpCode::ShrI => {
                        let fallback = proto.code.get(pc + 1).filter(|next| {
                            next.get_opcode() == OpCode::MmBinI
                                && matches!(
                                    TmKind::from_u8(next.get_c() as u8),
                                    TmKind::Add | TmKind::Shl | TmKind::Shr
                                )
                        });
                        if let Some(fallback) = fallback {
                            wire = match TmKind::from_u8(fallback.get_c() as u8) {
                                TmKind::Add => OP_ADD,
                                TmKind::Shl => OP_SHL,
                                TmKind::Shr => OP_SHR,
                                _ => unreachable!(),
                            };
                            let immediate = added_rk53(
                                &mut code,
                                &mut constants,
                                LuaValue::integer(fallback.get_sb() as i64),
                                scratch,
                                &mut max_stack,
                            )?;
                            if fallback.get_k() {
                                (immediate, fallback.get_a())
                            } else {
                                (fallback.get_a(), immediate)
                            }
                        } else {
                            match op {
                                OpCode::AddI => (
                                    b,
                                    added_rk53(
                                        &mut code,
                                        &mut constants,
                                        LuaValue::integer(instruction.get_sc() as i64),
                                        scratch,
                                        &mut max_stack,
                                    )?,
                                ),
                                OpCode::ShlI => (
                                    added_rk53(
                                        &mut code,
                                        &mut constants,
                                        LuaValue::integer(instruction.get_sc() as i64),
                                        scratch,
                                        &mut max_stack,
                                    )?,
                                    b,
                                ),
                                OpCode::ShrI => (
                                    b,
                                    added_rk53(
                                        &mut code,
                                        &mut constants,
                                        LuaValue::integer(instruction.get_sc() as i64),
                                        scratch,
                                        &mut max_stack,
                                    )?,
                                ),
                                _ => unreachable!(),
                            }
                        }
                    }
                    OpCode::AddK
                    | OpCode::SubK
                    | OpCode::MulK
                    | OpCode::ModK
                    | OpCode::PowK
                    | OpCode::DivK
                    | OpCode::IDivK
                    | OpCode::BAndK
                    | OpCode::BOrK
                    | OpCode::BXorK => (b, BITRK | c),
                    _ => (b, c),
                };
                code.push(abc53(wire, a, left, right)?);
            }
            OpCode::ForLoop
            | OpCode::ForPrep
            | OpCode::TForPrep
            | OpCode::TForCall
            | OpCode::TForLoop
            | OpCode::Tbc
            | OpCode::GetVarg
            | OpCode::ErrNNil => {
                return Err(format!(
                    "opcode {op:?} belongs to Lua 5.5 and cannot be dumped as Lua 5.3"
                ));
            }
            _ => return Err(format!("unsupported native opcode {op:?} in Lua 5.3 dump")),
        }

        lines.extend(std::iter::repeat_n(line, code.len() - start));
    }
    pc_map[proto.code.len()] = code.len();

    for patch in patches {
        if patch.target_native < 0 || patch.target_native as usize > proto.code.len() {
            return Err("native jump target is outside the function".to_string());
        }
        let target = pc_map[patch.target_native as usize];
        let offset = target as isize - patch.instruction as isize - 1;
        let offset = i32::try_from(offset)
            .map_err(|_| "jump is too long for a Lua 5.3 binary chunk".to_string())?;
        let old = code[patch.instruction];
        code[patch.instruction] = asbx53(op53(old), a53(old), offset)?;
    }

    let max_stack_size = u8::try_from(max_stack)
        .map_err(|_| "function needs more than 255 registers for a Lua 5.3 dump".to_string())?;
    Ok(WireFunction {
        code,
        constants,
        lines,
        pc_map,
        max_stack_size,
    })
}

fn put_int(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_count(out: &mut Vec<u8>, count: usize, what: &str) -> Result<(), String> {
    let count =
        i32::try_from(count).map_err(|_| format!("too many {what} entries for a Lua 5.3 chunk"))?;
    put_int(out, count);
    Ok(())
}

fn put_string(out: &mut Vec<u8>, bytes: Option<&[u8]>) -> Result<(), String> {
    let Some(bytes) = bytes else {
        out.push(0);
        return Ok(());
    };
    let stored = bytes
        .len()
        .checked_add(1)
        .ok_or_else(|| "Lua 5.3 string length overflow".to_string())?;
    if stored < 0xff {
        out.push(stored as u8);
    } else {
        out.push(0xff);
        out.extend_from_slice(
            &u64::try_from(stored)
                .map_err(|_| "Lua 5.3 string is too long".to_string())?
                .to_le_bytes(),
        );
    }
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_proto53(
    out: &mut Vec<u8>,
    proto: &LuaProto,
    parent_source: Option<&str>,
    strip: bool,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_PROTO_DEPTH {
        return Err("prototype nesting exceeds the Lua 5.3 safety limit".to_string());
    }
    let wire = translate_native53(proto)?;
    let source = proto.source_name.as_deref();
    let source_bytes = if strip || source == parent_source {
        None
    } else {
        source.map(str::as_bytes)
    };
    put_string(out, source_bytes)?;
    put_int(
        out,
        i32::try_from(proto.linedefined)
            .map_err(|_| "line number exceeds the Lua 5.3 limit".to_string())?,
    );
    put_int(
        out,
        i32::try_from(proto.lastlinedefined)
            .map_err(|_| "line number exceeds the Lua 5.3 limit".to_string())?,
    );
    out.push(
        u8::try_from(proto.param_count)
            .map_err(|_| "parameter count exceeds the Lua 5.3 limit".to_string())?,
    );
    out.push(u8::from(proto.is_vararg));
    out.push(wire.max_stack_size.max(2));

    put_count(out, wire.code.len(), "instruction")?;
    for instruction in &wire.code {
        out.extend_from_slice(&instruction.to_le_bytes());
    }

    put_count(out, wire.constants.len(), "constant")?;
    for constant in &wire.constants {
        if constant.is_nil() {
            out.push(0);
        } else if let Some(value) = constant.as_boolean() {
            out.extend_from_slice(&[1, u8::from(value)]);
        } else if let Some(value) = constant.as_integer_strict() {
            out.push(19);
            out.extend_from_slice(&value.to_le_bytes());
        } else if constant.is_float() {
            out.push(3);
            out.extend_from_slice(&constant.as_float().unwrap().to_bits().to_le_bytes());
        } else if let Some(bytes) = constant.as_bytes() {
            out.push(if bytes.len() <= 40 { 4 } else { 20 });
            put_string(out, Some(bytes))?;
        } else {
            return Err(format!(
                "constant of type {} cannot appear in a Lua 5.3 chunk",
                constant.type_name()
            ));
        }
    }

    put_count(out, proto.upvalue_descs.len(), "upvalue")?;
    for descriptor in &proto.upvalue_descs {
        out.push(u8::from(descriptor.is_local));
        out.push(
            u8::try_from(descriptor.index)
                .map_err(|_| "upvalue index exceeds the Lua 5.3 limit".to_string())?,
        );
    }

    put_count(out, proto.child_protos.len(), "child prototype")?;
    for child in &proto.child_protos {
        write_proto53(out, &child.as_ref().data, source, strip, depth + 1)?;
    }

    if strip {
        put_int(out, 0);
        put_int(out, 0);
        put_int(out, 0);
    } else {
        put_count(out, wire.lines.len(), "line")?;
        for line in &wire.lines {
            put_int(
                out,
                i32::try_from(*line)
                    .map_err(|_| "line number exceeds the Lua 5.3 limit".to_string())?,
            );
        }
        put_count(out, proto.locals.len(), "local variable")?;
        for local in &proto.locals {
            put_string(out, Some(local.name.as_bytes()))?;
            let start = wire
                .pc_map
                .get(local.startpc as usize)
                .copied()
                .unwrap_or(wire.code.len());
            let end = wire
                .pc_map
                .get(local.endpc as usize)
                .copied()
                .unwrap_or(wire.code.len());
            put_int(
                out,
                i32::try_from(start)
                    .map_err(|_| "local range exceeds the Lua 5.3 limit".to_string())?,
            );
            put_int(
                out,
                i32::try_from(end)
                    .map_err(|_| "local range exceeds the Lua 5.3 limit".to_string())?,
            );
        }
        put_count(out, proto.upvalue_descs.len(), "upvalue name")?;
        for descriptor in &proto.upvalue_descs {
            put_string(out, Some(descriptor.name.as_bytes()))?;
        }
    }
    Ok(())
}

pub fn dump(proto: &LuaProto, strip: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(SIGNATURE);
    out.extend_from_slice(&[VERSION, FORMAT]);
    out.extend_from_slice(DATA);
    out.extend_from_slice(&[4, 8, 4, 8, 8]);
    out.extend_from_slice(&LUAC_INT.to_le_bytes());
    out.extend_from_slice(&LUAC_NUM.to_bits().to_le_bytes());
    out.push(
        u8::try_from(proto.upvalue_descs.len())
            .map_err(|_| "main upvalue count exceeds the Lua 5.3 limit".to_string())?,
    );
    write_proto53(&mut out, proto, None, strip, 0)?;
    if out.len() > MAX_CHUNK_BYTES {
        return Err("Lua 5.3 chunk exceeds the 128 MiB safety limit".to_string());
    }
    Ok(out)
}
