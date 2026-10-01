//! Engine side of LuaTeX's `lang` library (llanglib.c): patterns,
//! exceptions, hyphen characters and hyphenation limits of the engine's
//! per-language hyphenation tables (`hyphen.rs`).

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString, LuaTable};

use crate::engine::Engine;
use crate::hyphen::Trie;
use crate::lua_bridge::{bytes_of, with_engine};

macro_rules! reg {
    ($lua:expr, $tbl:expr, $name:literal, $f:expr) => {
        $tbl.set($name, $lua.create_function($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}

/// Per-language values `lang.*` reads and writes besides the tries.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LangParams {
    /// `None` until a Lua program sets it: the font \\hyphenchar applies.
    pub pre_hyphen: Option<i32>,
    pub post_hyphen: i32,
    pub pre_exhyphen: i32,
    pub post_exhyphen: i32,
    /// Words shorter than this are not hyphenated (`lang.hyphenationmin`).
    pub hyphenation_min: i32,
}

impl Default for LangParams {
    fn default() -> Self {
        // llanglib / language.c `new_language`: pre_hyphen_char = '-'
        Self { pre_hyphen: None, post_hyphen: 0, pre_exhyphen: 0, post_exhyphen: 0, hyphenation_min: 0 }
    }
}

fn language_id(id: i64) -> Result<u8, String> {
    u8::try_from(id).map_err(|_| format!("lang.new({id}): undefined language"))
}

/// The text of the patterns in `trie` as `lang.patterns` returns them:
/// every pattern followed by one space.
fn trie_patterns(trie: &Trie) -> Vec<u8> {
    let mut patterns: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut stack: Vec<(u32, Vec<u8>)> = vec![(0, Vec::new())];
    while let Some((node, key)) = stack.pop() {
        let n = &trie.nodes[node as usize];
        if n.value != u32::MAX {
            let mut digits = vec![0u8; key.len() + 1];
            let mut v = n.value;
            while v != u32::MAX {
                let tv = &trie.values[v as usize];
                if let Some(slot) = digits.get_mut(tv.pos as usize) {
                    *slot = tv.value;
                }
                v = tv.next;
            }
            let mut text = Vec::new();
            for (i, byte) in key.iter().enumerate() {
                if digits[i] != 0 {
                    text.push(b'0' + digits[i]);
                }
                text.push(*byte);
            }
            if digits[key.len()] != 0 {
                text.push(b'0' + digits[key.len()]);
            }
            patterns.push((key.clone(), text));
        }
        let mut child = n.child;
        while child != u32::MAX {
            let c = &trie.nodes[child as usize];
            let mut k = key.clone();
            k.push(c.byte);
            stack.push((child, k));
            child = c.sibling;
        }
    }
    patterns.sort();
    let mut out = Vec::new();
    for (_, text) in patterns {
        out.extend_from_slice(&text);
        out.push(b' ');
    }
    out
}

impl Engine {
    fn lua_lang_params(&mut self, id: u8) -> &mut LangParams {
        self.lua_tex.lang.entry(id).or_default()
    }

    /// The current `hyphenationmin` of `lang` (0: none).
    pub(crate) fn lang_hyphenation_min(&self, lang: u8) -> usize {
        self.lua_tex.lang.get(&lang).map_or(0, |p| p.hyphenation_min.max(0) as usize)
    }
}

pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let t: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;

    reg!(lua, t, "check", |id: i64| -> Result<i64, String> { language_id(id).map(i64::from) });
    reg!(lua, t, "patterns_add", |id: i64, text: LuaString| -> Result<(), String> {
        let id = language_id(id)?;
        let text = bytes_of(&text);
        with_engine(|e| {
            let trie = e.trie_for_language_mut(id);
            for word in text.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty()) {
                trie.add_pattern_bytes(word);
            }
        })
    });
    reg!(lua, t, "patterns_get", |id: i64| -> Result<LuaBytes, String> {
        let id = language_id(id)?;
        with_engine(|e| LuaBytes(e.trie_for_language(id).map(trie_patterns).unwrap_or_default()))
    });
    reg!(lua, t, "patterns_clear", |id: i64| -> Result<(), String> {
        let id = language_id(id)?;
        with_engine(|e| {
            let trie = e.trie_for_language_mut(id);
            let exceptions = std::mem::take(&mut trie.exceptions);
            *trie = Trie::new();
            trie.exceptions = exceptions;
        })
    });
    reg!(lua, t, "exceptions_add", |id: i64, text: LuaString| -> Result<(), String> {
        let id = language_id(id)?;
        let text = bytes_of(&text);
        with_engine(|e| {
            let trie = e.trie_for_language_mut(id);
            for word in text.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty()) {
                trie.add_exception_bytes(word);
            }
        })
    });
    reg!(lua, t, "exceptions_get", |id: i64| -> Result<Option<LuaBytes>, String> {
        let id = language_id(id)?;
        with_engine(|e| {
            let trie = e.trie_for_language(id)?;
            if trie.exceptions.is_empty() {
                return None;
            }
            let mut words: Vec<(&Vec<u8>, &Vec<usize>)> = trie.exceptions.iter().collect();
            words.sort();
            let mut out = Vec::new();
            for (word, points) in words {
                out.push(b' ');
                for (i, byte) in word.iter().enumerate() {
                    if points.contains(&i) && i > 0 {
                        out.push(b'-');
                    }
                    out.push(*byte);
                }
            }
            Some(LuaBytes(out))
        })
    });
    reg!(lua, t, "exceptions_clear", |id: i64| -> Result<(), String> {
        let id = language_id(id)?;
        with_engine(|e| e.trie_for_language_mut(id).exceptions.clear())
    });
    reg!(lua, t, "param_get", |id: i64, which: String| -> Result<i64, String> {
        let id = language_id(id)?;
        with_engine(|e| {
            let p = *e.lua_lang_params(id);
            i64::from(match which.as_str() {
                "pre" => p.pre_hyphen.unwrap_or(45),
                "post" => p.post_hyphen,
                "preex" => p.pre_exhyphen,
                "postex" => p.post_exhyphen,
                _ => p.hyphenation_min,
            })
        })
    });
    reg!(lua, t, "param_set", |id: i64, which: String, value: i64| -> Result<(), String> {
        let id = language_id(id)?;
        with_engine(|e| {
            let p = e.lua_lang_params(id);
            let v = value as i32;
            match which.as_str() {
                "pre" => p.pre_hyphen = Some(v),
                "post" => p.post_hyphen = v,
                "preex" => p.pre_exhyphen = v,
                "postex" => p.post_exhyphen = v,
                _ => p.hyphenation_min = v,
            }
        })
    });
    reg!(lua, t, "hjcode_get", |id: i64, c: i64| -> Result<i64, String> {
        let id = language_id(id)?;
        let c = usize::try_from(c).ok().filter(|c| *c < 256).ok_or("character code out of range")?;
        with_engine(|e| {
            let lc = i64::from(e.eqtb.lc_code[c]);
            e.hyphen_codes.get(&id).map_or(lc, |codes| match codes[c] {
                0 => lc,
                code => i64::from(code),
            })
        })
    });
    reg!(lua, t, "hjcode_set", |id: i64, c: i64, v: i64| -> Result<(), String> {
        let id = language_id(id)?;
        let c = usize::try_from(c).ok().filter(|c| *c < 256).ok_or("character code out of range")?;
        let v = u8::try_from(v).map_err(|_| "hyphenation code out of range".to_string())?;
        with_engine(|e| {
            let lc = e.eqtb.lc_code.clone();
            let codes = e.hyphen_codes.entry(id).or_insert_with(|| {
                let mut t = Box::new([0u8; 256]);
                t.copy_from_slice(&lc[..256]);
                t
            });
            codes[c] = v;
        })
    });
    reg!(lua, t, "clean", |word: LuaString| -> Result<Option<LuaBytes>, String> {
        // llanglib.c `clean_hyphenation`: lowercase with \lccode, drop
        // everything that is not a letter
        let word = bytes_of(&word);
        with_engine(|e| {
            let mut out = Vec::new();
            for &b in &word {
                if b == b' ' {
                    break;
                }
                let lc = e.eqtb.lc_code[b as usize];
                if lc != 0 {
                    out.push(lc);
                }
            }
            Some(LuaBytes(out))
        })
    });
    crate::lua_ud::install_lang(lua, &t)?;
    lua.set_global("__ratex_langlib", t).map_err(|e| format!("{e:?}"))?;
    lua.load(include_str!("lua_lang.lua"))
        .set_name("=[ratex lang]")
        .exec()
        .map_err(|e| format!("lang library: {}", lua.get_error_message(e).message()))
}
