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
        Self { pre_hyphen: None, post_hyphen: 0, pre_exhyphen: 0, post_exhyphen: 0, hyphenation_min: -1 }
    }
}

fn language_id(id: i64) -> Result<u8, String> {
    u8::try_from(id).map_err(|_| format!("lang.new({id}): undefined language"))
}

/// hyphen.c `hnj_string_hash(word) % HASH_SIZE`.
fn pattern_bucket(word: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &b in word {
        h = (h << 4).wrapping_add(u32::from(b));
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
            h ^= g;
        }
    }
    h % 31627
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
    // hyphen.c keeps the patterns in a hash table of `HASH_SIZE` chains and
    // `hnj_serialize` walks its buckets in order; the newest pattern leads a
    // chain, which the (unrecorded) insertion order would decide, so a chain
    // is walked from the greatest word down.
    patterns.sort_by(|a, b| pattern_bucket(&a.0).cmp(&pattern_bucket(&b.0)).then_with(|| b.0.cmp(&a.0)));
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

    fn lua_language_made(&self, lang: u8) -> bool {
        self.lua_tex.lang_made[usize::from(lang >> 6)] >> (lang & 63) & 1 != 0
    }

    /// texlang.c `tex_languages[lang] != NULL`: the language has patterns,
    /// exceptions, parameters or hjcodes, or was made by `lang.new` or by
    /// typesetting text in it. Language 0 is made by the patterns of a format.
    pub(crate) fn lua_language_exists(&self, lang: u8) -> bool {
        self.lua_language_made(lang)
            || self.lua_tex.lang.contains_key(&lang)
            || self.lua_tex.rich_exceptions.contains_key(&lang)
            || self.hyphen_codes.contains_key(&lang)
            || if lang == 0 { !self.hyphen_trie.is_empty() } else { self.hyphen_tries.contains_key(&lang) }
    }

    /// texlang.c `get_language(lang)` (`new_language` when it does not exist
    /// yet): the language is there from now on, with the `\lccode`s as its
    /// hjcodes under `\savinghyphcodes`.
    pub(crate) fn lua_get_language(&mut self, lang: u8) {
        if self.lua_language_exists(lang) {
            return;
        }
        self.lua_tex.lang_made[usize::from(lang >> 6)] |= 1 << (lang & 63);
        if self.eqtb.int_params[crate::prim::IntParam::SavingHyphCodes.idx() as usize] > 0 {
            let mut codes = Box::new([0; 256]);
            codes.copy_from_slice(&self.eqtb.lc_code[..256]);
            self.hyphen_codes.insert(lang, codes);
        }
    }

    /// luatex makes the language of every character it typesets (`new_char`
    /// reads it with `get_language`), so `lang.new()` never hands it out.
    pub(crate) fn lua_note_text_language(&mut self) {
        let v = self.eqtb.int_params[crate::prim::IntParam::Language.idx() as usize];
        let lang = if (1..=255).contains(&v) { v as u8 } else { 0 };
        self.lua_tex.lang_made[usize::from(lang >> 6)] |= 1 << (lang & 63);
    }

    /// `lang.new([id])` (llanglib.c `lang_new`): `id` is made if it is not
    /// there yet; without one `new_language(-1)` takes the number after the
    /// highest language there is.
    fn lua_new_language(&mut self, id: Option<u8>) -> Result<u8, String> {
        let id = match id {
            Some(id) => id,
            None => {
                let next = (0..=255u8).rev().find(|&l| self.lua_language_exists(l)).map_or(0, |l| u16::from(l) + 1);
                u8::try_from(next).map_err(|_| "lang.new(): undefined language".to_string())?
            }
        };
        self.lua_get_language(id);
        Ok(id)
    }

    /// The current `hyphenationmin` of `lang` (0: none).
    pub(crate) fn lang_hyphenation_min(&self, lang: u8) -> usize {
        self.lua_tex.lang.get(&lang).map_or(0, |p| p.hyphenation_min.max(0) as usize)
    }

    /// texlang/textcodes `get_hj_code(lang, c)`: the language's table once
    /// `\hjcode` created it or `\savinghyphcodes` copied the `\lccode`s into
    /// it, else the `\lccode`. Characters above 255 have no `\lccode` here:
    /// their lower case stands in unless `\hjcode` assigned them.
    pub(crate) fn hj_code_of(&self, lang: u8, c: i32) -> i32 {
        if c < 0 {
            return 0;
        }
        if let Some(&v) = self.lua_tex.hj_wide.get(&(lang, c)) {
            return v;
        }
        let table = self.hyphen_codes.get(&lang);
        if c < 256 {
            return match table {
                Some(codes) => i32::from(codes[c as usize]),
                None => i32::from(self.eqtb.lc_code[c as usize]),
            };
        }
        if self.lua_tex.hj_pure.contains(&lang) {
            return 0;
        }
        match char::from_u32(c as u32) {
            Some(ch) if ch.is_alphabetic() => {
                let mut lower = ch.to_lowercase();
                match (lower.next(), lower.next()) {
                    (Some(l), None) => l as i32,
                    _ => c,
                }
            }
            _ => 0,
        }
    }

    /// textcodes `set_hj_code`: the first assignment of a language creates
    /// its table with every code 0.
    pub(crate) fn set_hj_code_of(&mut self, lang: u8, c: i32, v: i32) -> bool {
        if c < 0 || c > 0x10FFFF || v < 0 {
            return false;
        }
        if !self.hyphen_codes.contains_key(&lang) {
            self.hyphen_codes.insert(lang, Box::new([0u8; 256]));
            self.lua_tex.hj_pure.insert(lang);
        }
        if c < 256 && v < 256 {
            self.lua_tex.hj_wide.remove(&(lang, c));
            if let Some(codes) = self.hyphen_codes.get_mut(&lang) {
                codes[c as usize] = v as u8;
            }
        } else {
            self.lua_tex.hj_wide.insert((lang, c), v);
        }
        true
    }

    /// llanglib `clean_hyphenation`: the key an exception word is stored
    /// under (hyphens dropped, `=` as a hyphen letter, only the replacement
    /// of `{pre}{post}{replace}`), or `None` for a malformed word.
    pub(crate) fn clean_exception_word(&self, lang: u8, word: &[char]) -> Option<Vec<u8>> {
        let at = |i: usize| word.get(i).copied().unwrap_or('\0');
        let mut out = String::new();
        let store = |out: &mut String, c: char| {
            let mut x = self.hj_code_of(lang, c as i32);
            if x <= 32 {
                x = c as i32;
            }
            out.push(char::from_u32(x as u32).unwrap_or(c));
        };
        let mut i = 0;
        while i < word.len() {
            let u = word[i];
            i += 1;
            match u {
                '-' => {}
                '=' => store(&mut out, '-'),
                '{' => {
                    let mut items = 0;
                    let mut u = at(i);
                    i += 1;
                    while u != '\0' && u != '}' {
                        u = at(i);
                        i += 1;
                    }
                    if u == '}' {
                        items += 1;
                        u = at(i);
                        i += 1;
                    }
                    while u != '\0' && u != '}' {
                        u = at(i);
                        i += 1;
                    }
                    if u == '}' {
                        items += 1;
                        u = at(i);
                        i += 1;
                    }
                    if u == '{' {
                        u = at(i);
                        i += 1;
                    }
                    while u != '\0' && u != '}' {
                        store(&mut out, u);
                        u = at(i);
                        i += 1;
                    }
                    if u == '}' {
                        items += 1;
                    }
                    if items != 3 {
                        return None;
                    }
                    if at(i) == '[' && at(i + 1).is_ascii_digit() && at(i + 2) == ']' {
                        i += 3;
                    }
                }
                c => store(&mut out, c),
            }
        }
        Some(out.into_bytes())
    }

    /// texlang.c `new_patterns` / `new_hyph_exceptions`: `\patterns` and
    /// `\hyphenation` expand their braced text (`scan_toks(false, true)`)
    /// and load it as one string (`\par` left out), in INITEX and after a
    /// format alike.
    pub(crate) fn lua_hyphenation_words(&mut self, is_patterns: bool) {
        self.skip_spaces_relax();
        let toks = self.scan_general_text_expanded();
        if self.stopped_on_error {
            return;
        }
        let text = self.tokens_to_lua_text(&toks);
        let language = self.eqtb.int_params[crate::prim::IntParam::Language.idx() as usize];
        let language = u8::try_from(language).unwrap_or(0);
        if !is_patterns {
            self.lua_load_hyphenation(language, text.as_bytes());
            return;
        }
        // luatex copies the \lccode's once, when the language comes into being
        if self.eqtb.int_params[crate::prim::IntParam::SavingHyphCodes.idx() as usize] > 0
            && !self.hyphen_codes.contains_key(&language)
        {
            let mut codes = Box::new([0; 256]);
            codes.copy_from_slice(&self.eqtb.lc_code[..256]);
            self.hyphen_codes.insert(language, codes);
        }
        self.lua_load_patterns(language, text.as_bytes());
    }

    /// texlang.c `load_patterns`: add the whitespace separated patterns of
    /// `text` to `lang`.
    pub(crate) fn lua_load_patterns(&mut self, lang: u8, text: &[u8]) {
        let trie = self.trie_for_language_mut(lang);
        for word in text.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty()) {
            trie.add_pattern_bytes(word);
        }
    }

    /// llanglib `load_hyphenation`: add the whitespace separated exceptions
    /// of `text` to `lang`. A word with nothing but `-` hyphens joins the
    /// language's trie (and so the format); the others are kept as written.
    pub(crate) fn lua_load_hyphenation(&mut self, lang: u8, text: &[u8]) {
        let text = String::from_utf8_lossy(text).into_owned();
        for raw in text.split(|c: char| c.is_ascii_whitespace()).filter(|w| !w.is_empty()) {
            let chars: Vec<char> = raw.chars().collect();
            if raw.len() > 64 {
                self.error("exception too long");
                continue;
            }
            let Some(key) = self.clean_exception_word(lang, &chars) else {
                self.error("exception syntax error");
                continue;
            };
            let simple = !chars.iter().any(|c| matches!(c, '=' | '{' | '}'));
            if simple {
                let mut points = Vec::new();
                let mut n = 0usize;
                let mut key_bytes = Vec::new();
                for &c in &chars {
                    if c == '-' {
                        points.push(key_bytes.len());
                        continue;
                    }
                    let mut buf = [0u8; 4];
                    key_bytes.extend_from_slice(self.exception_letter(lang, c).encode_utf8(&mut buf).as_bytes());
                    n += 1;
                }
                debug_assert_eq!(key_bytes, key);
                if let Some(map) = self.lua_tex.rich_exceptions.get_mut(&lang) {
                    map.remove(&key);
                }
                if n >= 1 {
                    // the paragraph pass of 8-bit (TFM) text looks words up as
                    // bytes, the pass of Lua fonts as UTF-8
                    let text = String::from_utf8_lossy(&key).into_owned();
                    if !key.is_ascii() && text.chars().all(|c| (c as u32) < 256) {
                        let mut latin1 = Vec::with_capacity(key.len());
                        let mut latin1_points = Vec::with_capacity(points.len());
                        let mut at = 0usize;
                        for c in text.chars() {
                            if points.contains(&at) {
                                latin1_points.push(latin1.len());
                            }
                            latin1.push(c as u8);
                            at += c.len_utf8();
                        }
                        self.trie_for_language_mut(lang).exceptions.insert(latin1, latin1_points);
                    }
                    self.trie_for_language_mut(lang).exceptions.insert(key, points);
                }
            } else {
                self.trie_for_language_mut(lang).exceptions.remove(&key);
                self.lua_tex.rich_exceptions.entry(lang).or_default().insert(key, raw.as_bytes().to_vec());
            }
        }
    }

    fn exception_letter(&self, lang: u8, c: char) -> char {
        let mut x = self.hj_code_of(lang, c as i32);
        if x <= 32 {
            x = c as i32;
        }
        char::from_u32(x as u32).unwrap_or(c)
    }

    /// The exception of the cleaned word `word` as it was written (`-` marks
    /// a syllable break, `=` a hyphen letter), if there is one.
    pub(crate) fn lua_exception_raw(&self, lang: u8, word: &[u32]) -> Option<Vec<u8>> {
        let mut key: Vec<u8> = Vec::with_capacity(word.len());
        for &c in word {
            let ch = char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER);
            let mut buf = [0u8; 4];
            key.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
        if let Some(raw) = self.lua_tex.rich_exceptions.get(&lang).and_then(|m| m.get(&key)) {
            return Some(raw.clone());
        }
        let points = self.trie_for_language(lang)?.exceptions.get(&key)?;
        let mut raw = Vec::with_capacity(key.len() + points.len());
        for (i, &b) in key.iter().enumerate() {
            if points.contains(&i) && i > 0 {
                raw.push(b'-');
            }
            raw.push(if b == b'-' { b'=' } else { b });
        }
        Some(raw)
    }
}

pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let t: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;

    reg!(lua, t, "new", |id: Option<i64>| -> Result<i64, String> {
        let id = id.map(language_id).transpose()?;
        with_engine(|e| e.lua_new_language(id))?.map(i64::from)
    });
    reg!(lua, t, "patterns_add", |id: i64, text: LuaString| -> Result<(), String> {
        let id = language_id(id)?;
        let text = bytes_of(&text);
        with_engine(|e| e.lua_load_patterns(id, &text))
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
        with_engine(|e| e.lua_load_hyphenation(id, &text))
    });
    reg!(lua, t, "exceptions_get", |id: i64| -> Result<Option<LuaBytes>, String> {
        let id = language_id(id)?;
        with_engine(|e| {
            let mut words: Vec<Vec<u8>> = Vec::new();
            if let Some(trie) = e.trie_for_language(id) {
                for (word, points) in &trie.exceptions {
                    // the 8-bit twin of a UTF-8 word is not a word of its own
                    if std::str::from_utf8(word).is_err() {
                        continue;
                    }
                    let mut raw = Vec::with_capacity(word.len() + points.len());
                    for (i, byte) in word.iter().enumerate() {
                        if points.contains(&i) && i > 0 {
                            raw.push(b'-');
                        }
                        raw.push(if *byte == b'-' { b'=' } else { *byte });
                    }
                    words.push(raw);
                }
            }
            if let Some(rich) = e.lua_tex.rich_exceptions.get(&id) {
                words.extend(rich.values().cloned());
            }
            if words.is_empty() {
                return None;
            }
            words.sort();
            let mut out = Vec::new();
            for word in words {
                out.push(b' ');
                out.extend_from_slice(&word);
            }
            Some(LuaBytes(out))
        })
    });
    reg!(lua, t, "exceptions_clear", |id: i64| -> Result<(), String> {
        let id = language_id(id)?;
        with_engine(|e| {
            e.trie_for_language_mut(id).exceptions.clear();
            e.lua_tex.rich_exceptions.remove(&id);
        })
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
        let c = i32::try_from(c).ok().filter(|c| (0..=0x10FFFF).contains(c)).ok_or("character code out of range")?;
        with_engine(|e| i64::from(e.hj_code_of(id, c)))
    });
    reg!(lua, t, "hjcode_set", |id: i64, c: i64, v: i64| -> Result<(), String> {
        let id = language_id(id)?;
        let c = i32::try_from(c).ok().filter(|c| (0..=0x10FFFF).contains(c)).ok_or("character code out of range")?;
        let v = i32::try_from(v).ok().filter(|v| *v >= 0).ok_or_else(|| "hyphenation code out of range".to_string())?;
        with_engine(|e| {
            e.set_hj_code_of(id, c, v);
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
    lua.set_global("__texres_langlib", t).map_err(|e| format!("{e:?}"))?;
    lua.load(include_str!("lua_lang.lua"))
        .set_name("=[texres lang]")
        .exec()
        .map_err(|e| format!("lang library: {}", lua.get_error_message(e).message()))
}
