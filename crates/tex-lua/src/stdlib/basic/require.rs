use crate::stdlib::lauxlib;
use crate::{LuaResult, LuaValue, lua_vm::LuaState};

/// require(modname): port of loadlib.c `ll_require`/`findloader`.
/// Searchers and the loader run unprotected, so their errors propagate
/// unchanged. Lua 5.3 returns the module; Lua 5.5 also the loader data.
pub fn lua_require(l: &mut LuaState) -> LuaResult<usize> {
    let name_bytes = lauxlib::check_lstring(l, 1)?.to_vec();
    let name = String::from_utf8_lossy(&name_bytes).into_owned();
    let name_value = l.create_bytes(&name_bytes)?;
    let lua53 = l.global_state().language() == crate::LuaLanguageLevel::Lua53;

    let loaded = l.global_state_mut().registry_get("_LOADED")?.unwrap_or_default();
    if let Some(module) = l.table_get(&loaded, &name_value)?
        && module.is_truthy()
    {
        l.push_value(module)?;
        return Ok(1);
    }

    // findloader
    let package = l.global_state_mut().registry_get("_PACKAGE")?.unwrap_or_default();
    let searchers_key = l.create_string("searchers")?;
    let searchers = l.table_get(&package, &searchers_key)?.unwrap_or_default();
    if !searchers.is_table() {
        return Err(lauxlib::lual_error(l, "'package.searchers' must be a table"));
    }
    let mut messages = Vec::new();
    let (loader, data) = 'search: {
        for i in 1.. {
            let searcher = searchers.as_table().and_then(|t| t.raw_geti(i)).unwrap_or_default();
            if searcher.is_nil() {
                let list = String::from_utf8_lossy(&messages).into_owned();
                return Err(lauxlib::lual_error(l, format!("module '{name}' not found:{list}")));
            }
            let mut results = l.call(searcher, vec![name_value])?.into_iter();
            let first = results.next().unwrap_or_default();
            let second = results.next().unwrap_or_default();
            if first.is_function() {
                break 'search (first, second);
            }
            if first.is_string() || first.is_number() {
                if !lua53 {
                    messages.extend_from_slice(b"\n\t");
                }
                messages.extend_from_slice(&lauxlib::tolstring(l, &first)?);
            }
        }
        unreachable!("the searcher loop only exits by returning or breaking")
    };

    let module = l.call(loader, vec![name_value, data])?.into_iter().next().unwrap_or_default();
    if !module.is_nil() {
        l.table_set(&loaded, name_value, module)?;
    }
    let module = match l.table_get(&loaded, &name_value)? {
        Some(value) if !value.is_nil() => value,
        _ => {
            let value = LuaValue::boolean(true);
            l.table_set(&loaded, name_value, value)?;
            value
        }
    };
    l.push_value(module)?;
    if lua53 {
        return Ok(1);
    }
    l.push_value(data)?;
    Ok(2)
}
