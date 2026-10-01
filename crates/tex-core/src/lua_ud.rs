//! Userdata types of the Lua libraries: `luatex.token` (lnewtokenlib.c) and
//! `luatex.lang` (llanglib.c).

use std::any::Any;

use tex_lua::{Lua, LuaTable, UdValue, UserDataTrait};

use crate::engine::Engine;
use crate::lua_bridge::with_engine;
use crate::token::Token;

/// A token object: the packed engine token.
pub(crate) struct TokenUd(pub u32);

impl UserDataTrait for TokenUd {
    fn type_name(&self) -> &'static str {
        "luatex.token"
    }

    fn get_field(&self, key: &str) -> Option<UdValue> {
        let t = Token(self.0);
        with_engine(|e| token_field(e, t, key)).ok().flatten()
    }

    fn set_field(&mut self, _key: &str, _value: UdValue) -> Option<Result<(), String>> {
        Some(Err("attempt to assign to a luatex.token value".to_string()))
    }

    fn lua_tostring(&self) -> Option<String> {
        Some(format!("<lua token {}: {}>", self.0, Engine::lua_tok_value(Token(self.0))))
    }

    fn lua_eq(&self, other: &dyn UserDataTrait) -> Option<bool> {
        other.as_any().downcast_ref::<TokenUd>().map(|o| o.0 == self.0)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn token_field(e: &mut Engine, t: Token, key: &str) -> Option<UdValue> {
    let (cmd, mode) = e.lua_cmd_mode(t);
    Some(match key {
        "command" => UdValue::Integer(i64::from(cmd)),
        "mode" => UdValue::Integer(mode),
        "index" => e.lua_tok_index(t).map_or(UdValue::Nil, UdValue::Integer),
        "cmdname" => UdValue::Str(crate::lua_cmds::COMMAND_NAMES[cmd as usize].to_string()),
        "csname" => e.lua_tok_csname(t).map_or(UdValue::Nil, UdValue::Str),
        "active" => {
            let u = t.unfreeze();
            UdValue::Boolean(u.is_char() && u.cc() == 13)
        }
        "expandable" => UdValue::Boolean(cmd >= 133),
        "protected" => UdValue::Boolean(e.lua_tok_is_protected(t)),
        "tok" => UdValue::Integer(Engine::lua_tok_value(t)),
        "id" => UdValue::Integer(i64::from(t.0)),
        _ => UdValue::Nil,
    })
}

/// A language object of the `lang` library.
pub(crate) struct LangUd(pub i64);

impl UserDataTrait for LangUd {
    fn type_name(&self) -> &'static str {
        "luatex.lang"
    }

    fn lua_tostring(&self) -> Option<String> {
        Some(format!("luatex.lang: {:#x}", 0x5600_0000_0000_i64 + self.0 * 16))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Install the token constructors and accessors into the bridge table `b`.
pub(crate) fn install_tokens(lua: &mut Lua, b: &LuaTable) -> Result<(), String> {
    let wrap = lua
        .create_callback(|cb| {
            let packed: i64 = cb.arg(1)?;
            let packed = u32::try_from(packed).map_err(|_| cb.arg_error(1, "lua <token> expected"))?;
            let ud = cb.create_userdata(TokenUd(packed))?;
            cb.push(ud)
        })
        .map_err(|e| format!("{e:?}"))?;
    b.set("tok_wrap", wrap).map_err(|e| format!("{e:?}"))?;

    // packed value of a token object, nil for anything else
    let unwrap = lua
        .create_callback(|cb| {
            let value = cb.arg::<tex_lua::Value>(1)?;
            match value.as_userdata::<TokenUd>() {
                Some(ud) => {
                    let packed = i64::from(ud.borrow()?.0);
                    cb.push(packed)
                }
                None => cb.push(()),
            }
        })
        .map_err(|e| format!("{e:?}"))?;
    b.set("tok_unwrap", unwrap).map_err(|e| format!("{e:?}"))?;
    Ok(())
}

/// Install the language object constructors into the `lang` natives `b`.
pub(crate) fn install_lang(lua: &mut Lua, b: &LuaTable) -> Result<(), String> {
    let lang_new = lua
        .create_callback(|cb| {
            let id: i64 = cb.arg(1)?;
            let ud = cb.create_userdata(LangUd(id))?;
            cb.push(ud)
        })
        .map_err(|e| format!("{e:?}"))?;
    b.set("lang_new", lang_new).map_err(|e| format!("{e:?}"))?;
    let lang_id = lua
        .create_callback(|cb| {
            let value = cb.arg::<tex_lua::Value>(1)?;
            match value.as_userdata::<LangUd>() {
                Some(ud) => {
                    let id = ud.borrow()?.0;
                    cb.push(id)
                }
                None => cb.push(()),
            }
        })
        .map_err(|e| format!("{e:?}"))?;
    b.set("lang_id", lang_id).map_err(|e| format!("{e:?}"))?;
    Ok(())
}
