//! Lua 5.3 compatibility `bit32` library.

use crate::lib_registry::LibraryModule;
use crate::lua_value::LuaValue;
use crate::lua_vm::{LuaResult, LuaState};

pub fn create_bit32_lib() -> LibraryModule {
    crate::lib_module!("bit32", {
        "arshift" => arshift,
        "band" => band,
        "bnot" => bnot,
        "bor" => bor,
        "btest" => btest,
        "bxor" => bxor,
        "extract" => extract,
        "lrotate" => lrotate,
        "lshift" => lshift,
        "replace" => replace,
        "rrotate" => rrotate,
        "rshift" => rshift,
    })
}

fn integer(l: &mut LuaState, index: usize, function: &str) -> Result<i64, crate::LuaError> {
    let value = l.get_arg(index).ok_or_else(|| {
        l.error(format!(
            "bad argument #{} to '{}' (number expected, got no value)",
            index, function
        ))
    })?;
    let value = if let Some(text) = value.as_str() {
        crate::stdlib::basic::parse_number::parse_lua_number(text)
    } else {
        value
    };
    if let Some(value) = value.as_integer() {
        return Ok(value);
    }
    if let Some(value) = value.as_number() {
        if value.is_finite()
            && value.fract() == 0.0
            && value >= i64::MIN as f64
            && value < -(i64::MIN as f64)
        {
            return Ok(value as i64);
        }
        return Err(l.error(format!(
            "bad argument #{} to '{}' (number has no integer representation)",
            index, function
        )));
    }
    Err(l.error(format!(
        "bad argument #{} to '{}' (number expected)",
        index, function
    )))
}

#[inline]
fn unsigned(l: &mut LuaState, index: usize, function: &str) -> Result<u32, crate::LuaError> {
    Ok(integer(l, index, function)? as u32)
}

#[inline]
fn push(l: &mut LuaState, value: u32) -> LuaResult<usize> {
    l.push_value(LuaValue::integer(i64::from(value)))?;
    Ok(1)
}

fn fold(
    l: &mut LuaState,
    function: &str,
    initial: u32,
    operation: impl Fn(u32, u32) -> u32,
) -> LuaResult<usize> {
    let mut result = initial;
    for index in 1..=l.get_args().len() {
        result = operation(result, unsigned(l, index, function)?);
    }
    push(l, result)
}

fn band(l: &mut LuaState) -> LuaResult<usize> {
    fold(l, "band", u32::MAX, |left, right| left & right)
}

fn bor(l: &mut LuaState) -> LuaResult<usize> {
    fold(l, "bor", 0, |left, right| left | right)
}

fn bxor(l: &mut LuaState) -> LuaResult<usize> {
    fold(l, "bxor", 0, |left, right| left ^ right)
}

fn btest(l: &mut LuaState) -> LuaResult<usize> {
    let mut result = u32::MAX;
    for index in 1..=l.get_args().len() {
        result &= unsigned(l, index, "btest")?;
    }
    l.push_value(LuaValue::boolean(result != 0))?;
    Ok(1)
}

fn bnot(l: &mut LuaState) -> LuaResult<usize> {
    let value = unsigned(l, 1, "bnot")?;
    push(l, !value)
}

fn displacement(l: &mut LuaState, function: &str) -> Result<i64, crate::LuaError> {
    integer(l, 2, function)
}

fn lshift(l: &mut LuaState) -> LuaResult<usize> {
    let value = unsigned(l, 1, "lshift")?;
    let shift = displacement(l, "lshift")?;
    let result = if shift < 0 {
        logical_right(value, shift.unsigned_abs())
    } else if shift >= 32 {
        0
    } else {
        value.wrapping_shl(shift as u32)
    };
    push(l, result)
}

fn rshift(l: &mut LuaState) -> LuaResult<usize> {
    let value = unsigned(l, 1, "rshift")?;
    let shift = displacement(l, "rshift")?;
    let result = if shift < 0 {
        logical_left(value, shift.unsigned_abs())
    } else {
        logical_right(value, shift as u64)
    };
    push(l, result)
}

#[inline]
fn logical_left(value: u32, shift: u64) -> u32 {
    if shift >= 32 {
        0
    } else {
        value.wrapping_shl(shift as u32)
    }
}

#[inline]
fn logical_right(value: u32, shift: u64) -> u32 {
    if shift >= 32 { 0 } else { value >> shift }
}

fn arshift(l: &mut LuaState) -> LuaResult<usize> {
    let value = unsigned(l, 1, "arshift")?;
    let shift = displacement(l, "arshift")?;
    let result = if shift < 0 {
        logical_left(value, shift.unsigned_abs())
    } else if shift >= 32 {
        if value & 0x8000_0000 == 0 {
            0
        } else {
            u32::MAX
        }
    } else {
        ((value as i32) >> shift) as u32
    };
    push(l, result)
}

fn lrotate(l: &mut LuaState) -> LuaResult<usize> {
    let value = unsigned(l, 1, "lrotate")?;
    let shift = displacement(l, "lrotate")?.rem_euclid(32) as u32;
    push(l, value.rotate_left(shift))
}

fn rrotate(l: &mut LuaState) -> LuaResult<usize> {
    let value = unsigned(l, 1, "rrotate")?;
    let shift = displacement(l, "rrotate")?.rem_euclid(32) as u32;
    push(l, value.rotate_right(shift))
}

fn field(
    l: &mut LuaState,
    field_index: usize,
    width_index: usize,
    function: &str,
) -> Result<(u32, u32), crate::LuaError> {
    let field = integer(l, field_index, function)?;
    let width = if l.get_arg(width_index).is_some() {
        integer(l, width_index, function)?
    } else {
        1
    };
    if field < 0 || field >= 32 {
        return Err(l.error("field cannot be negative or greater than 31".to_string()));
    }
    if width <= 0 || width > 32 || field + width > 32 {
        return Err(l.error("width must be positive and fit inside 32 bits".to_string()));
    }
    Ok((field as u32, width as u32))
}

fn low_mask(width: u32) -> u32 {
    if width == 32 {
        u32::MAX
    } else {
        (1u32 << width) - 1
    }
}

fn extract(l: &mut LuaState) -> LuaResult<usize> {
    let value = unsigned(l, 1, "extract")?;
    let (field, width) = field(l, 2, 3, "extract")?;
    push(l, (value >> field) & low_mask(width))
}

fn replace(l: &mut LuaState) -> LuaResult<usize> {
    let value = unsigned(l, 1, "replace")?;
    let replacement = unsigned(l, 2, "replace")?;
    let (field, width) = field(l, 3, 4, "replace")?;
    let mask = low_mask(width) << field;
    push(l, (value & !mask) | ((replacement << field) & mask))
}
