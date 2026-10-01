// Table library (port of ltablib.c)
// Implements: concat, create (5.5), insert, move, pack, remove, sort, unpack

use crate::lib_registry::LibraryModule;
use crate::lua_value::LuaValue;
use crate::lua_vm::{LuaResult, LuaState, get_metatable};
use crate::stdlib::lauxlib;
use crate::stdlib::numfmt::tostring_float;
use crate::stdlib::sort_table::table_sort;

pub fn create_table_lib() -> LibraryModule {
    crate::lib_module!("table", {
        "concat" => table_concat,
        "create" => table_create,
        "insert" => table_insert,
        "move" => table_move,
        "pack" => table_pack,
        "remove" => table_remove,
        "sort" => table_sort,
        "unpack" => table_unpack,
    })
}

const TAB_R: u8 = 1;
const TAB_W: u8 = 2;
const TAB_L: u8 = 4;

/// `checktab`: a table, or a value whose metatable provides the needed
/// `__index` / `__newindex` / `__len` metamethods.
fn check_tab(l: &mut LuaState, narg: usize, what: u8) -> LuaResult<LuaValue> {
    let value = l.get_arg(narg).unwrap_or_default();
    if value.is_table() {
        return Ok(value);
    }
    if let Some(metatable) = get_metatable(l, &value)
        && let Some(metatable) = metatable.as_table()
    {
        let mut has = |name: &str, flag: u8| -> LuaResult<bool> {
            if what & flag == 0 {
                return Ok(true);
            }
            let key = l.create_string(name)?;
            Ok(metatable.raw_get(&key).is_some_and(|v| !v.is_nil()))
        };
        if has("__index", TAB_R)? && has("__newindex", TAB_W)? && has("__len", TAB_L)? {
            return Ok(value);
        }
    }
    Err(lauxlib::typeerror(l, narg, "table"))
}

/// A table argument: raw access when it is a table without metatable,
/// otherwise `lua_geti`/`lua_seti`/`luaL_len` with metamethods.
struct TabArg {
    value: LuaValue,
    raw: bool,
}

impl TabArg {
    fn new(l: &mut LuaState, narg: usize, what: u8) -> LuaResult<TabArg> {
        let value = check_tab(l, narg, what)?;
        let raw = value.as_table().is_some_and(|t| !t.has_metatable());
        Ok(TabArg { value, raw })
    }

    /// `aux_getn`: `luaL_len`.
    fn len(&self, l: &mut LuaState) -> LuaResult<i64> {
        if self.raw {
            Ok(self.value.as_table().map_or(0, |t| t.len() as i64))
        } else {
            l.obj_len(&self.value)
        }
    }

    #[inline]
    fn geti(&self, l: &mut LuaState, i: i64) -> LuaResult<LuaValue> {
        if self.raw {
            Ok(l.raw_geti(&self.value, i).unwrap_or_default())
        } else {
            l.table_geti(&self.value, i)
        }
    }

    #[inline]
    fn seti(&self, l: &mut LuaState, i: i64, value: LuaValue) -> LuaResult<()> {
        if self.raw {
            l.raw_seti(&self.value, i, value);
            Ok(())
        } else {
            l.table_seti(&self.value, i, value)
        }
    }
}

/// table.create(narray [, nhash]) - Create a pre-allocated table (Lua 5.5)
fn table_create(l: &mut LuaState) -> LuaResult<usize> {
    let narray = lauxlib::check_integer(l, 1)?;
    let nhash = lauxlib::opt_integer(l, 2, 0)?;
    if !(0..=i64::from(i32::MAX)).contains(&narray) {
        return Err(lauxlib::argerror(l, 1, "out of range"));
    }
    if !(0..=i64::from(i32::MAX)).contains(&nhash) {
        return Err(lauxlib::argerror(l, 2, "out of range"));
    }
    // Limit eager allocation; the table grows on demand beyond this.
    let max_eager = 1 << 24;
    if nhash > max_eager {
        return Err(lauxlib::lual_error(l, "table overflow"));
    }
    let table = l.create_table((narray as usize).min(max_eager as usize), nhash as usize)?;
    l.push_value(table)?;
    Ok(1)
}

/// table.concat(list [, sep [, i [, j]]])
fn table_concat(l: &mut LuaState) -> LuaResult<usize> {
    let list = TabArg::new(l, 1, TAB_R | TAB_L)?;
    let len = list.len(l)?;
    let sep = lauxlib::opt_lstring(l, 2)?;
    let sep: &[u8] = sep.as_deref().unwrap_or_default();
    let mut i = lauxlib::opt_integer(l, 3, 1)?;
    let last = lauxlib::opt_integer(l, 4, len)?;
    let level = l.global_state().language();
    let mut buf: Vec<u8> = Vec::new();
    while i <= last {
        let value = list.geti(l, i)?;
        if let Some(bytes) = value.as_bytes() {
            buf.extend_from_slice(bytes);
        } else if value.ttisinteger() {
            buf.extend_from_slice(itoa::Buffer::new().format(value.ivalue()).as_bytes());
        } else if value.ttisfloat() {
            buf.extend_from_slice(tostring_float(value.fltvalue(), level).as_bytes());
        } else {
            let message = format!(
                "invalid value ({}) at index {} in table for 'concat'",
                value.type_name(),
                i
            );
            return Err(lauxlib::lual_error(l, message));
        }
        if i == last {
            break;
        }
        buf.extend_from_slice(sep);
        i += 1;
    }
    let result = l.create_binary(buf)?;
    l.push_value(result)?;
    Ok(1)
}

/// table.insert(list, [pos,] value)
fn table_insert(l: &mut LuaState) -> LuaResult<usize> {
    let list = TabArg::new(l, 1, TAB_R | TAB_W | TAB_L)?;
    let first_empty = list.len(l)?.wrapping_add(1);
    let (pos, value) = match l.arg_count() {
        2 => (first_empty, l.get_arg(2).unwrap_or_default()),
        3 => {
            let pos = lauxlib::check_integer(l, 2)?;
            // Unsigned comparison checks 1 <= pos <= first_empty at once.
            if (pos as u64).wrapping_sub(1) >= first_empty as u64 {
                return Err(lauxlib::argerror(l, 2, "position out of bounds"));
            }
            let mut i = first_empty;
            while i > pos {
                let moved = list.geti(l, i - 1)?;
                list.seti(l, i, moved)?;
                i -= 1;
            }
            (pos, l.get_arg(3).unwrap_or_default())
        }
        _ => return Err(lauxlib::lual_error(l, "wrong number of arguments to 'insert'")),
    };
    list.seti(l, pos, value)?;
    Ok(0)
}

/// table.remove(list [, pos])
fn table_remove(l: &mut LuaState) -> LuaResult<usize> {
    let list = TabArg::new(l, 1, TAB_R | TAB_W | TAB_L)?;
    let size = list.len(l)?;
    let mut pos = lauxlib::opt_integer(l, 2, size)?;
    if pos != size && (pos as u64).wrapping_sub(1) > size as u64 {
        // Lua 5.3 blames argument #1 here; 5.4+ fixed it to #2.
        let narg = if l.global_state().language() == crate::LuaLanguageLevel::Lua53 { 1 } else { 2 };
        return Err(lauxlib::argerror(l, narg, "position out of bounds"));
    }
    let removed = list.geti(l, pos)?;
    while pos < size {
        let next = list.geti(l, pos + 1)?;
        list.seti(l, pos, next)?;
        pos += 1;
    }
    list.seti(l, pos, LuaValue::nil())?;
    l.push_value(removed)?;
    Ok(1)
}

/// table.move(a1, f, e, t [, a2])
fn table_move(l: &mut LuaState) -> LuaResult<usize> {
    let f = lauxlib::check_integer(l, 2)?;
    let e = lauxlib::check_integer(l, 3)?;
    let t = lauxlib::check_integer(l, 4)?;
    let target_arg = if l.get_arg(5).is_some_and(|v| !v.is_nil()) { 5 } else { 1 };
    let source = TabArg::new(l, 1, TAB_R)?;
    let target = TabArg::new(l, target_arg, TAB_W)?;
    if e >= f {
        if !(f > 0 || e < i64::MAX + f) {
            return Err(lauxlib::argerror(l, 3, "too many elements to move"));
        }
        let n = e - f + 1;
        if t > i64::MAX - n + 1 {
            return Err(lauxlib::argerror(l, 4, "destination wrap around"));
        }
        let other_table = target_arg != 1 && source.value != target.value;
        if t > e || t <= f || other_table {
            for i in 0..n {
                let value = source.geti(l, f + i)?;
                target.seti(l, t + i, value)?;
            }
        } else {
            for i in (0..n).rev() {
                let value = source.geti(l, f + i)?;
                target.seti(l, t + i, value)?;
            }
        }
    }
    l.push_value(target.value)?;
    Ok(1)
}

/// table.pack(...)
fn table_pack(l: &mut LuaState) -> LuaResult<usize> {
    let n = l.arg_count();
    let table = l.create_table(n, 1)?;
    for i in 0..n {
        let value = l.get_arg(i + 1).unwrap_or_default();
        l.raw_seti(&table, (i + 1) as i64, value);
    }
    let n_key = l.create_string("n")?;
    l.raw_set(&table, n_key, LuaValue::integer(n as i64));
    l.push_value(table)?;
    Ok(1)
}

/// table.unpack(list [, i [, j]])
fn table_unpack(l: &mut LuaState) -> LuaResult<usize> {
    let list = l.get_arg(1).unwrap_or_default();
    let raw = list.as_table().is_some_and(|t| !t.has_metatable());
    let first = lauxlib::opt_integer(l, 2, 1)?;
    let last = match l.get_arg(3) {
        Some(value) if !value.is_nil() => lauxlib::check_integer(l, 3)?,
        _ if raw => list.as_table().map_or(0, |t| t.len() as i64),
        _ => l.obj_len(&list)?,
    };
    if first > last {
        return Ok(0);
    }
    let n = (last as u64).wrapping_sub(first as u64);
    if n >= i32::MAX as u64 || !l.check_stack(n as usize + 1) {
        return Err(lauxlib::lual_error(l, "too many results to unpack"));
    }
    let count = n as usize + 1;
    l.ensure_stack_capacity(count)?;
    for i in 0..count as i64 {
        let value = if raw {
            l.raw_geti(&list, first + i).unwrap_or_default()
        } else {
            l.table_geti(&list, first + i)?
        };
        l.push_value(value)?;
    }
    Ok(count)
}
