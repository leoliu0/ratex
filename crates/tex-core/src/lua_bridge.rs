//! LuaTeX's Lua <-> TeX bridge: engine access while Lua runs, the `token`
//! library (lnewtokenlib.c), live `tex` registers (ltexlib.c), Lua function
//! and bytecode registers (llualib.c), the callback table (lcallbacklib.c)
//! and the kpathsea package searcher (luainit.c).
//!
//! Lua code reaches the engine through a thread-local pointer that
//! [`Engine::lua_run`] sets for the dynamic extent of every Lua entry made
//! from the engine (`\directlua`, `\luafunction`, lua calls, callbacks). Lua
//! token objects are tables `{packed}` sharing one metatable; `packed` is the
//! ratex [`Token`] value.

use std::cell::Cell;

use tex_lua::{Lua, LuaApi, LuaBytes, LuaFunction, LuaString, LuaTable};

use crate::engine::Engine;
use crate::engine_lua::LuaEngine;
use crate::eqtb::Equiv;
use crate::prim::{IntParam, Prim};
use crate::token::{CsId, Token};

thread_local! {
    static ACTIVE: Cell<*mut Engine> = const { Cell::new(std::ptr::null_mut()) };
    /// Nesting depth of Lua entries from TeX (luatex `lua_active`).
    static DEPTH: Cell<i64> = const { Cell::new(0) };
}

/// `lua.getcalllevel()`.
pub(crate) fn lua_call_level() -> i64 {
    DEPTH.with(Cell::get)
}

/// Restores the previously active engine when a Lua entry ends, also when
/// the Lua call unwinds.
struct ActiveGuard(*mut Engine);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get() - 1));
        ACTIVE.with(|active| active.set(self.0));
    }
}

/// Run `f` on the engine whose Lua call is in progress.
pub(crate) fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> Result<R, String> {
    let engine = ACTIVE.with(Cell::get);
    if engine.is_null() {
        return Err("the TeX engine is not available here".to_string());
    }
    // SAFETY: `Engine::lua_run` stores a pointer derived from its `&mut self`
    // for exactly the duration of the Lua call it makes and does not touch
    // the engine itself until that call returns. Callbacks run on this
    // thread, strictly nested inside that call.
    Ok(f(unsafe { &mut *engine }))
}

/// LuaTeX's `cs_token_flag` (the `tok` field of control-sequence tokens).
const CS_TOKEN_FLAG: i64 = 0x1FFF_FFFF;
/// Name prefix of engine-internal control sequences: the shared undefined
/// control sequence of `token.create` and anonymous `char_given` /
/// `math_given` tokens built by `token.new`.
const ANON_PREFIX: &[u8] = b"\x00ratex-anon:";
const CMD_RELAX: u8 = 0;
const CMD_CHAR_GIVEN: u8 = 82;
const CMD_MATH_GIVEN: u8 = 83;
const CMD_XMATH_GIVEN: u8 = 84;
const CMD_ASSIGN_TOKS: u8 = 87;
const CMD_ASSIGN_INT: u8 = 88;
const CMD_ASSIGN_ATTR: u8 = 89;
const CMD_ASSIGN_DIMEN: u8 = 90;
const CMD_ASSIGN_GLUE: u8 = 91;
const CMD_ASSIGN_MU_GLUE: u8 = 92;
const CMD_LUA_CALL: u8 = 68;
const CMD_NORMAL: u8 = 63;
const CMD_SET_FONT: u8 = 109;
const CMD_UNDEFINED_CS: u8 = 133;
const CMD_LUA_EXPANDABLE_CALL: u8 = 137;
const CMD_CONVERT: u8 = 142;
const CMD_CALL: u8 = 147;
/// LuaTeX 1.24 eqtb bases of the register commands (`count_base` etc.):
/// `mode` of a `\countdef` token is `count_base + n`.
const COUNT_BASE: i64 = 333_450;
const ATTRIBUTE_BASE: i64 = 398_986;
const DIMEN_BASE: i64 = 464_561;
const SKIP_BASE: i64 = 71_107;
const MU_SKIP_BASE: i64 = 136_643;
const TOKS_BASE: i64 = 202_200;
const BIGGEST_CHAR: i64 = 0x10_FFFF;

/// Saved `cur_cmd`/`cur_chr`/`cur_cs`/`cur_tok` (lnewtokenlib.c
/// `save_tex_scanner`): a Lua scanner must not disturb the command TeX is
/// executing.
pub(crate) struct ScannerState {
    tok: Token,
    cs: Option<CsId>,
    prim: Option<Prim>,
    chr: i32,
}

impl Engine {
    pub(crate) fn save_scanner(&self) -> ScannerState {
        ScannerState {
            tok: self.cur_tok,
            cs: self.cur_cs,
            prim: self.cur_prim,
            chr: self.cur_chr,
        }
    }

    pub(crate) fn restore_scanner(&mut self, s: ScannerState) {
        self.cur_tok = s.tok;
        self.cur_cs = s.cs;
        self.cur_prim = s.prim;
        self.cur_chr = s.chr;
    }

    /// Enter Lua: make this engine reachable from Lua callbacks while `f`
    /// runs on the (lazily created) Lua state.
    pub(crate) fn lua_run<R>(
        &mut self,
        f: impl FnOnce(&mut LuaEngine) -> Result<R, String>,
    ) -> Result<R, String> {
        if self.lua.is_none() {
            self.lua = Some(Box::new(LuaEngine::new()?));
        }
        let lua: *mut LuaEngine = &mut **self.lua.as_mut().expect("Lua state exists");
        DEPTH.with(|d| d.set(d.get() + 1));
        let _guard = ActiveGuard(ACTIVE.with(|active| active.replace(self as *mut Engine)));
        // SAFETY: the boxed Lua state lives in `self.lua` for the whole
        // call; Lua callbacks reach the engine only through the pointer
        // installed above and never drop or replace `self.lua`.
        let result = f(unsafe { &mut *lua });
        // lists Lua took from the engine (`tex.getbox`, `tex.nest`, ...) go
        // back to it; they stay tied while an enclosing Lua call runs
        self.lua_sync_links(DEPTH.with(Cell::get) <= 1);
        result
    }

    /// Run Lua and feed what it printed back to TeX (luatex
    /// `luacstrings`/`lua_string_start`). Output of an enclosing Lua call
    /// that is still pending stays queued for that call.
    fn lua_run_with_output(
        &mut self,
        f: impl FnOnce(&mut LuaEngine) -> Result<(), String>,
    ) -> Result<(), String> {
        let outer = std::mem::take(&mut self.lua_print_queue);
        let result = self.lua_run(f);
        self.flush_lua_output();
        self.lua_print_queue = outer;
        result
    }

    /// `\directlua{code}`.
    pub fn execute_directlua(&mut self, code: &[u8]) -> Result<(), String> {
        self.lua_run_with_output(|lua| lua.execute(code, "=[\\directlua]"))
    }

    /// `\luafunction n`, `\luafunctioncall n` and lua-call commands
    /// (luastuff.c `luafunctioncall`): call `lua.get_functions_table()[n]`
    /// with `n` when it is a function.
    pub(crate) fn call_lua_function(&mut self, slot: i32) {
        if slot <= 0 {
            self.error("luafunction: invalid number");
            return;
        }
        if let Err(err) = self.lua_run_with_output(|lua| lua.call_function_slot(slot)) {
            self.error(&format!("LuaTeX error {err}"));
        }
    }

    /// `\luabytecode n` / `\luabytecodecall n` (llualib.c `luabytecodecall`).
    pub(crate) fn call_lua_bytecode(&mut self, slot: i32) {
        let Some(bytes) = u32::try_from(slot).ok().and_then(|k| self.lua_bytecodes.get(&k).cloned())
        else {
            self.error("LuaTeX error undefined bytecode register");
            return;
        };
        if let Err(err) = self.lua_run_with_output(|lua| lua.call_bytecode(slot, &bytes)) {
            self.error(&format!("LuaTeX error {err}"));
        }
    }

    /// Run the callback `name` with no arguments (`"->"` callbacks such as
    /// `pre_dump`); nothing happens when none is registered.
    pub(crate) fn run_lua_callback(&mut self, name: &str) {
        if !self.lua.as_mut().is_some_and(|lua| lua.has_callback(name)) {
            return;
        }
        if let Err(err) = self.lua_run_with_output(|lua| lua.call_callback(name, None).map(drop)) {
            self.error(&format!("LuaTeX error {err}"));
        }
    }

    /// Run a `"S->S"` callback (`process_jobname`); `None` when no callback
    /// is registered or it returned no string.
    pub(crate) fn run_lua_string_callback(&mut self, name: &str, arg: &str) -> Option<String> {
        if !self.lua.as_mut().is_some_and(|lua| lua.has_callback(name)) {
            return None;
        }
        match self.lua_run(|lua| lua.call_callback(name, Some(arg))) {
            Ok(result) => result,
            Err(err) => {
                self.error(&format!("LuaTeX error {err}"));
                None
            }
        }
    }

    /// The end marker of `tex.runtoks` local control (luatex
    /// `end_local_code`).
    pub(crate) fn lua_end_local_control_token(&mut self) -> Token {
        self.lua_end_local_control()
    }

    fn lua_end_local_control(&mut self) -> Token {
        let end = self.cs.intern(&[ANON_PREFIX, b"end-local-control"].concat());
        // luatex inserts `\endlocalcontrol` itself (extension_cmd end_local_code)
        if self.eqtb.get(end).is_none() {
            self.eqtb.assign(end, Equiv::Prim(Prim::U(crate::uprim::UPrim::EndLocalControl)), true);
        }
        Token::from_cs(end)
    }

    /// ltexlib.c `runtoks` / maincontrol.c `local_control`: with the end
    /// marker below what Lua put into the input, execute commands in
    /// restricted horizontal mode until the marker is read.
    pub(crate) fn lua_local_control(&mut self) {
        // TeX runs: lists tied to the engine must be current and are dropped
        self.lua_sync_links(true);
        let saved = self.save_scanner();
        let mode = std::mem::replace(&mut self.mode, crate::engine::Mode::RestrictedHorizontal);
        // `tex.runtoks(f)` already entered the level before f ran (luatex
        // reports 1 inside f)
        let entered = std::mem::take(&mut self.lua_local_entered);
        let ll = self.local_level - i32::from(entered);
        if !entered {
            self.local_level += 1;
        }
        while !self.end_occurred {
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                break;
            }
            self.dispatch(t);
            // `\endlocalcontrol` (or `tex.quittoks`) lowered the level
            if self.local_level <= ll {
                break;
            }
        }
        self.local_level = ll;
        self.mode = mode;
        self.restore_scanner(saved);
    }

    /// maincontrol.c `end_local_control`.
    pub(crate) fn end_local_control(&mut self) {
        if self.local_level > 0 {
            self.local_level -= 1;
        } else {
            let msg = format!("local control level {}: redundant end local control", self.local_level);
            self.tex_print_str(true, true, &msg);
            self.tex_print_nl(true, true);
        }
    }

    /// LuaTeX command code and `mode` of what `t` means now
    /// (lnewtokenlib.c `get_command` / `get_mode`).
    pub(crate) fn lua_cmd_mode(&self, t: Token) -> (u8, i64) {
        let t = t.unfreeze();
        // the parameters of a macro definition (`token.scan_toks(true)`): out_param
        // (command 5) and match (command 13) tokens
        if t.is_char() && t.0 >= crate::expand::PAR_REF_FLAG {
            return (5, i64::from(t.0 & 0xF));
        }
        if t.is_char() && t.cc() == 0 {
            return (13, i64::from(t.chr()));
        }
        if t.is_char() && t.cc() != 13 {
            return (t.cc(), i64::from(t.chr()));
        }
        let id = if t.is_char() {
            match self.active_cs_lookup(t.chr()) {
                Some(id) => id,
                None => return (CMD_UNDEFINED_CS, 0),
            }
        } else {
            t.cs_id()
        };
        let mut id = id;
        while let Some(Equiv::Alias(next)) = self.eqtb.get(id) {
            id = *next;
        }
        match self.eqtb.get(id) {
            None => (CMD_UNDEFINED_CS, 0),
            Some(Equiv::CountReg(i)) => (CMD_ASSIGN_INT, COUNT_BASE + i64::from(*i)),
            Some(Equiv::AttributeReg(i)) => (CMD_ASSIGN_ATTR, ATTRIBUTE_BASE + i64::from(*i)),
            Some(Equiv::DimenReg(i)) => (CMD_ASSIGN_DIMEN, DIMEN_BASE + i64::from(*i)),
            Some(Equiv::SkipReg(i)) => (CMD_ASSIGN_GLUE, SKIP_BASE + i64::from(*i)),
            Some(Equiv::MuSkipReg(i)) => (CMD_ASSIGN_MU_GLUE, MU_SKIP_BASE + i64::from(*i)),
            Some(Equiv::ToksReg(i)) => (CMD_ASSIGN_TOKS, TOKS_BASE + i64::from(*i)),
            Some(Equiv::BoxReg(i)) => (CMD_CHAR_GIVEN, i64::from(*i)),
            Some(Equiv::CharDef(v)) => (CMD_CHAR_GIVEN, i64::from(*v)),
            Some(Equiv::MathCharDef(v)) => (CMD_MATH_GIVEN, i64::from(*v)),
            Some(Equiv::UMathCharDef(v)) => (CMD_XMATH_GIVEN, i64::from(*v)),
            Some(Equiv::CharTok(v)) => {
                let tok = Token(*v);
                (tok.cc(), i64::from(tok.chr()))
            }
            Some(Equiv::FontRef(f)) => (CMD_SET_FONT, i64::from(*f)),
            Some(Equiv::LuaCall { slot, protected }) => (
                if *protected { CMD_LUA_CALL } else { CMD_LUA_EXPANDABLE_CALL },
                i64::from(*slot),
            ),
            Some(Equiv::Macro(m)) => {
                let cmd = CMD_CALL + u8::from(m.long) + 2 * u8::from(m.outer);
                (cmd, i64::from(id))
            }
            Some(Equiv::Alias(_)) => unreachable!("aliases resolved above"),
            Some(Equiv::Prim(p)) => self.lua_prim_cmd_mode(t, *p),
        }
    }

    /// A primitive's LuaTeX meaning, found by the name it is called by and
    /// otherwise by its primitive name.
    fn lua_prim_cmd_mode(&self, t: Token, p: Prim) -> (u8, i64) {
        let lookup = |name: &[u8]| {
            crate::lua_cmds::PRIMITIVES
                .binary_search_by(|(n, _, _)| (*n).cmp(name))
                .ok()
                .map(|i| {
                    let (_, cmd, mode) = crate::lua_cmds::PRIMITIVES[i];
                    (cmd, i64::from(mode))
                })
        };
        if t.is_cs() {
            if let Some(found) = lookup(self.cs.name(t.cs_id())) {
                return found;
            }
        }
        if let Some(found) = self.primitive_names.get(&p.code()).and_then(|name| lookup(name)) {
            return found;
        }
        match p {
            Prim::Relax => (CMD_RELAX, i64::from(BIGGEST_CHAR as i32 + 1)),
            _ if self.is_expandable(p) => (CMD_CONVERT, i64::from(p.code())),
            _ => (CMD_NORMAL, i64::from(p.code())),
        }
    }

    pub(crate) fn lua_tok_csname(&self, t: Token) -> Option<String> {
        let t = t.unfreeze();
        if t.is_char() {
            if t.cc() == 13 {
                return char::from_u32(t.chr()).map(String::from);
            }
            return None;
        }
        let name = self.cs.name(t.cs_id());
        if let Some(rest) = name.strip_prefix(ANON_PREFIX) {
            return (rest == b"undefined").then(String::new);
        }
        Some(String::from_utf8_lossy(name).into_owned())
    }

    pub(crate) fn lua_tok_is_protected(&self, t: Token) -> bool {
        let t = t.unfreeze();
        if !t.is_cs() {
            return false;
        }
        matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Macro(m)) if m.protected)
    }

    /// The shared undefined control sequence of `token.create`.
    fn lua_undefined_cs(&mut self) -> CsId {
        self.cs.intern(&[ANON_PREFIX, b"undefined"].concat())
    }

    /// `token.create(name)` (lnewtokenlib.c `run_lookup`): an existing
    /// control sequence, else the undefined one; no new name is entered.
    fn lua_create_cs(&mut self, name: &[u8]) -> Token {
        if name.is_empty() {
            return Token::from_cs(self.lua_undefined_cs());
        }
        match self.cs.lookup(name) {
            Some(id) => Token::from_cs(id),
            None => Token::from_cs(self.lua_undefined_cs()),
        }
    }

    /// A token of command `cmd` and value `chr` (`token.new`, and
    /// `token.create(chr, cmd)`). Character commands become character
    /// tokens; `char_given`/`math_given` become an anonymous control
    /// sequence with that meaning.
    fn lua_make_token(&mut self, chr: i64, cmd: i64) -> Result<Token, String> {
        let char_at = |chr: i64| {
            u32::try_from(chr).ok().filter(|c| char::from_u32(*c).is_some())
        };
        match cmd {
            1..=8 | 10..=12 => {
                let c = char_at(chr).ok_or_else(|| format!("invalid character code {chr}"))?;
                Ok(Token::unicode_char(cmd as u8, c))
            }
            13 => {
                let c = char_at(chr).ok_or_else(|| format!("invalid character code {chr}"))?;
                Ok(Token::char(13, c))
            }
            82 | 83 => {
                let value = u32::try_from(chr).map_err(|_| format!("invalid value {chr}"))?;
                let kind: &[u8] = if cmd == 82 { b"char_given:" } else { b"math_given:" };
                let name = [ANON_PREFIX, kind, value.to_string().as_bytes()].concat();
                let id = self.cs.intern(&name);
                if self.eqtb.get(id).is_none() {
                    let equiv = if cmd == 82 {
                        Equiv::CharDef(value)
                    } else {
                        Equiv::MathCharDef(
                            u16::try_from(value).map_err(|_| format!("invalid math char {chr}"))?,
                        )
                    };
                    self.eqtb.assign(id, equiv, true);
                }
                Ok(Token::from_cs(id))
            }
            84 => {
                let value = i32::try_from(chr).map_err(|_| format!("invalid value {chr}"))?;
                let name = [ANON_PREFIX, b"xmath_given:", value.to_string().as_bytes()].concat();
                let id = self.cs.intern(&name);
                if self.eqtb.get(id).is_none() {
                    self.eqtb.assign(id, Equiv::UMathCharDef(value), true);
                }
                Ok(Token::from_cs(id))
            }
            c if c == i64::from(CMD_LUA_CALL) || c == i64::from(CMD_LUA_EXPANDABLE_CALL) => {
                let slot = u32::try_from(chr).map_err(|_| format!("invalid value {chr}"))?;
                let protected = c == i64::from(CMD_LUA_CALL);
                let kind: &[u8] = if protected { b"lua_call:" } else { b"lua_expandable_call:" };
                let name = [ANON_PREFIX, kind, slot.to_string().as_bytes()].concat();
                let id = self.cs.intern(&name);
                if self.eqtb.get(id).is_none() {
                    self.eqtb.assign(id, Equiv::LuaCall { slot, protected }, true);
                }
                Ok(Token::from_cs(id))
            }
            _ => Err(format!(
                "token.new: command {} is not supported by this engine",
                crate::lua_cmds::COMMAND_NAMES.get(cmd as usize).copied().unwrap_or("?")
            )),
        }
    }

    /// Catcode of `c` in catcode table `table` (the current regime when the
    /// table is not valid; lnewtokenlib.c `set_macro`).
    fn lua_catcode_in(&self, table: Option<i32>, c: u32) -> u8 {
        match table.filter(|t| self.eqtb.cat_table_valid(*t)) {
            Some(t) => self.eqtb.cat_code_in(t, c),
            None => self.eqtb.cat_code(c),
        }
    }

    /// lnewtokenlib.c `set_macro`: tokenize `body` under catcode table
    /// `table`; a would-be control sequence must already exist, otherwise
    /// its escape character stays a character.
    pub(crate) fn lua_string_to_macro_body(&mut self, table: Option<i32>, body: &[u8]) -> Vec<Token> {
        let text = String::from_utf8_lossy(body);
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i] as u32;
            let cc = self.lua_catcode_in(table, c);
            i += 1;
            if cc == 0 {
                let start = i;
                let mut end = i;
                while end < chars.len() {
                    let cc2 = self.lua_catcode_in(table, chars[end] as u32);
                    if cc2 == 11 {
                        end += 1;
                    } else {
                        if end == start {
                            end += 1;
                        }
                        break;
                    }
                }
                let mut skip_space = false;
                if end == start + 0 && end < chars.len() {
                    end += 1;
                }
                if end > start && end - start > 1
                    || (end > start && self.lua_catcode_in(table, chars[start] as u32) == 11)
                {
                    skip_space = end < chars.len() && self.lua_catcode_in(table, chars[end] as u32) == 10;
                }
                let name: String = chars[start..end].iter().collect();
                match self.cs.lookup(name.as_bytes()) {
                    Some(id) if end > start => {
                        out.push(Token::from_cs(id));
                        i = end;
                        if skip_space {
                            i += 1;
                        }
                    }
                    _ => out.push(Token::unicode_char(cc, c)),
                }
                continue;
            }
            out.push(match cc {
                13 => Token::char(13, c),
                _ => Token::unicode_char(cc, c),
            });
        }
        out
    }

    fn lua_get_next(&mut self) -> Token {
        let saved = self.save_scanner();
        let t = self.raw_token().unfreeze();
        self.restore_scanner(saved);
        t
    }

    fn lua_scan_token(&mut self) -> Token {
        let saved = self.save_scanner();
        let t = self.get_x_raw();
        self.restore_scanner(saved);
        t
    }

    fn lua_scan_keyword(&mut self, kw: &[u8], case_sensitive: bool) -> bool {
        let saved = self.save_scanner();
        let found = if case_sensitive {
            let mut collected = Vec::new();
            let mut ok = true;
            for &expected in kw {
                let t = loop {
                    let t = self.get_x_raw();
                    if collected.is_empty() && t.is_space() {
                        continue;
                    }
                    break t;
                };
                collected.push(t);
                if !(t.is_char() && t.chr() == u32::from(expected)) {
                    ok = false;
                    break;
                }
            }
            if !ok {
                for t in collected.into_iter().rev() {
                    self.push_token(t);
                }
            }
            ok
        } else {
            self.scan_keyword(kw)
        };
        self.restore_scanner(saved);
        found
    }

    /// Letters and others from `first` on, as a string (scan_string's
    /// word branch); the first non-matching token is put back.
    fn lua_scan_word_from(&mut self, first: Token) -> String {
        let mut word = String::new();
        let mut t = first;
        while t.is_char() && matches!(t.cc(), 11 | 12) {
            if let Some(c) = char::from_u32(t.chr()) {
                word.push(c);
            }
            t = self.get_x_raw();
        }
        self.push_token(t);
        word
    }

    fn lua_scan_string(&mut self) -> Option<String> {
        let saved = self.save_scanner();
        let t = loop {
            let t = self.get_x_raw();
            if t.is_space() || (t.is_cs() && self.cur_prim == Some(Prim::Relax)) {
                continue;
            }
            break t;
        };
        let result = if t.is_left_brace() {
            self.push_token(t);
            let toks = self.scan_general_text_expanded();
            Some(self.tokens_to_string(&toks))
        } else if t.is_char() && matches!(t.cc(), 11 | 12) {
            Some(self.lua_scan_word_from(t))
        } else {
            self.push_token(t);
            None
        };
        self.restore_scanner(saved);
        result
    }

    fn lua_scan_argument(&mut self, expand: bool) -> Option<String> {
        let saved = self.save_scanner();
        let t = loop {
            let t = self.raw_token();
            if t.is_space() {
                continue;
            }
            if t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Relax))) {
                continue;
            }
            break t;
        };
        let is_macro = t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Macro(_)));
        let result = if t.is_left_brace() {
            self.push_token(t);
            let toks = if expand {
                self.scan_general_text_expanded()
            } else {
                self.scan_general_text()
            };
            Some(self.tokens_to_string(&toks))
        } else if is_macro {
            self.push_token(Token::char(2, u32::from(b'}')));
            if expand {
                self.push_token(t);
            } else if let Some(Equiv::Macro(m)) = self.eqtb.resolve(t.cs_id()).cloned() {
                self.expand_macro(t.cs_id(), &m, t.cs_id());
            }
            self.push_token(Token::char(1, u32::from(b'{')));
            let toks = if expand {
                self.scan_general_text_expanded()
            } else {
                self.scan_general_text()
            };
            Some(self.tokens_to_string(&toks))
        } else {
            let t = self.get_x_raw_from(t);
            if t.is_char() && matches!(t.cc(), 11 | 12) {
                Some(self.lua_scan_word_from(t))
            } else {
                self.push_token(t);
                None
            }
        };
        self.restore_scanner(saved);
        result
    }

    fn lua_scan_word(&mut self) -> Option<String> {
        let saved = self.save_scanner();
        let t = loop {
            let t = self.get_x_raw();
            if t.is_space() || (t.is_cs() && self.cur_prim == Some(Prim::Relax)) {
                continue;
            }
            break t;
        };
        let result = if t.is_char() && matches!(t.cc(), 11 | 12) {
            Some(self.lua_scan_word_from(t))
        } else {
            self.push_token(t);
            None
        };
        self.restore_scanner(saved);
        result
    }

    fn lua_scan_csname(&mut self) -> Option<String> {
        let saved = self.save_scanner();
        let t = self.raw_token().unfreeze();
        let name = if t.is_cs() || (t.is_char() && t.cc() == 13) {
            self.lua_tok_csname(t)
        } else {
            None
        };
        self.restore_scanner(saved);
        name
    }

    /// lnewtokenlib.c `run_scan_float_indeed`.
    fn lua_scan_float(&mut self, exponent: bool) -> Option<f64> {
        let saved = self.save_scanner();
        let digit = |t: Token| t.is_char() && matches!(t.cc(), 11 | 12) && (0x30..=0x39).contains(&t.chr());
        let is = |t: Token, c: u8| t.is_char() && matches!(t.cc(), 11 | 12) && t.chr() == u32::from(c);
        let mut text = String::new();
        let mut negative = false;
        let mut t;
        loop {
            t = self.get_x_raw();
            if t.is_space() {
                continue;
            }
            if is(t, b'-') {
                negative = !negative;
            } else if !is(t, b'+') {
                break;
            }
        }
        if negative {
            text.push('-');
        }
        let mut exp_candidate = None;
        if is(t, b'.') || is(t, b',') {
            text.push('.');
            loop {
                t = self.get_x_raw();
                if digit(t) {
                    text.push(char::from_u32(t.chr()).unwrap_or('0'));
                } else {
                    break;
                }
            }
            if exponent {
                exp_candidate = Some(t);
            } else {
                self.push_token(t);
            }
        } else {
            loop {
                if digit(t) {
                    text.push(char::from_u32(t.chr()).unwrap_or('0'));
                    t = self.get_x_raw();
                } else if is(t, b'.') || is(t, b',') {
                    text.push('.');
                    loop {
                        t = self.get_x_raw();
                        if digit(t) {
                            text.push(char::from_u32(t.chr()).unwrap_or('0'));
                        } else {
                            break;
                        }
                    }
                    self.push_token(t);
                    break;
                } else if exponent {
                    exp_candidate = Some(t);
                    break;
                } else {
                    self.push_token(t);
                    break;
                }
            }
        }
        if let Some(mut t) = exp_candidate {
            if is(t, b'E') || is(t, b'e') {
                text.push(char::from_u32(t.chr()).unwrap_or('e'));
                t = self.get_x_raw();
                if is(t, b'-') || is(t, b'+') || digit(t) {
                    text.push(char::from_u32(t.chr()).unwrap_or('0'));
                }
                loop {
                    t = self.get_x_raw();
                    if digit(t) {
                        text.push(char::from_u32(t.chr()).unwrap_or('0'));
                    } else {
                        break;
                    }
                }
            }
            self.push_token(t);
        }
        self.restore_scanner(saved);
        text.parse().ok()
    }

    fn lua_scan_code(&mut self, mask: i64) -> Option<i64> {
        let saved = self.save_scanner();
        let t = self.get_x_raw();
        let result = if t.is_char() && t.cc() < 16 && mask & (1 << t.cc()) != 0 {
            Some(i64::from(t.chr()))
        } else {
            self.push_token(t);
            None
        };
        self.restore_scanner(saved);
        result
    }

    /// lnewtokenlib.c `run_expand`: expand the next token once.
    fn lua_expand(&mut self) {
        let t = self.raw_token();
        let id = if t.is_cs() {
            Some(t.cs_id())
        } else if t.is_char() && t.cc() == 13 {
            Some(self.active_cs_id(t.chr()))
        } else {
            None
        };
        match id.and_then(|id| self.eqtb.resolve(id).cloned().map(|e| (id, e))) {
            Some((id, Equiv::Macro(m))) => self.expand_macro(id, &m, id),
            Some((id, Equiv::Prim(p))) if self.is_expandable(p) => {
                if let Some(tok) = self.expand_prim(p, id) {
                    self.push_token(tok);
                }
            }
            Some((_, Equiv::LuaCall { slot, protected: false })) => self.call_lua_function(slot as i32),
            _ => self.push_token(t),
        }
    }

    /// The meaning text of a macro, `params->body` (lnewtokenlib.c
    /// `get_meaning`), or just the body (`get_macro`).
    fn lua_macro_text(&self, name: &[u8], with_params: bool) -> Option<String> {
        let id = self.cs.lookup(name)?;
        let Some(Equiv::Macro(m)) = self.eqtb.resolve(id) else {
            return None;
        };
        let mut s = String::new();
        if with_params {
            if !m.prefix.is_empty() {
                s.push_str(&self.tokens_to_string(&m.prefix));
            }
            for (i, d) in m.params.iter().enumerate() {
                s.push_str(&format!("#{}", i + 1));
                if !d.is_empty() {
                    s.push_str(&self.tokens_to_string(d));
                }
            }
            s.push_str("->");
        }
        s.push_str(&self.tokens_to_string(&m.body));
        Some(s)
    }

    /// Register index for a Lua register key: a number, or the name of a
    /// control sequence defined by `\countdef` & co. (ltexlib.c
    /// `get_item_index_plus`).
    fn lua_register_index(&self, name: &[u8], cmd: u8, what: &str) -> Result<u16, String> {
        let id = self.cs.lookup(name).ok_or_else(|| format!("incorrect {what} name"))?;
        let (c, mode) = self.lua_cmd_mode(Token::from_cs(id));
        let base = match cmd {
            CMD_ASSIGN_INT => COUNT_BASE,
            CMD_ASSIGN_DIMEN => DIMEN_BASE,
            CMD_ASSIGN_TOKS => TOKS_BASE,
            CMD_ASSIGN_ATTR => ATTRIBUTE_BASE,
            _ => return Err(format!("incorrect {what} name")),
        };
        if c != cmd {
            return Err(format!("incorrect {what} name"));
        }
        u16::try_from(mode - base).map_err(|_| format!("incorrect {what} name"))
    }

    fn lua_global(&self, global: bool) -> bool {
        global || self.eqtb.int_params[IntParam::GlobalDefs.idx() as usize] > 0
    }

    /// `tex.print`-style string to character tokens (`str_toks`: spaces
    /// are spacers, everything else other characters).
    pub(crate) fn lua_str_toks(text: &[u8]) -> Vec<Token> {
        String::from_utf8_lossy(text)
            .chars()
            .map(|c| {
                if c == ' ' {
                    Token::space()
                } else {
                    Token::unicode_char(12, c as u32)
                }
            })
            .collect()
    }
}


impl Engine {
    /// `token.scan_toks(true, expand)` (tex.web §473 `scan_toks(true, xpand)`):
    /// the parameter text up to the opening brace and the body, as the token
    /// list `\def` would store. A parameter `#n` of the text becomes a match
    /// token (`Token::char(0, '#')`, LuaTeX command 13), the brace ends it with
    /// the end-match token (`Token::char(14, 0)`, command 14), and `#n` in the
    /// body is a `PAR_REF` out_param token.
    pub(crate) fn lua_scan_toks_def(&mut self, expand: bool) -> Vec<Token> {
        let mut out = Vec::new();
        let mut params = 0u32;
        let mut hash_brace = None;
        loop {
            let t = self.raw_token();
            if t == crate::input::EOF_MARKER || t.is_left_brace() {
                break;
            }
            if t.is_right_brace() {
                self.error("Missing { inserted");
                break;
            }
            if self.is_macro_param(t) {
                let t2 = self.raw_token();
                if t2.is_left_brace() {
                    out.push(t2);
                    hash_brace = Some(t2);
                    out.push(Token::char(14, 0));
                    return self.lua_scan_def_body(expand, out, params, hash_brace);
                }
                params += 1;
                if params > 9 {
                    self.error("You already have nine parameters");
                    params = 9;
                } else if t2.is_char() && t2.chr() != u32::from(b'0') + params {
                    self.error("Parameters must be numbered consecutively");
                    self.push_token(t2);
                }
                out.push(Token::char(0, t.chr()));
                continue;
            }
            out.push(t);
        }
        out.push(Token::char(14, 0));
        self.lua_scan_def_body(expand, out, params, hash_brace)
    }

    fn lua_scan_def_body(&mut self, expand: bool, mut out: Vec<Token>, params: u32, hash_brace: Option<Token>) -> Vec<Token> {
        let mut depth = 1u32;
        loop {
            let t = if expand { self.get_x_raw() } else { self.raw_token() };
            if t == crate::input::EOF_MARKER {
                break;
            }
            if t.is_left_brace() {
                depth += 1;
            } else if t.is_right_brace() {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            } else if self.is_macro_param(t) {
                let t2 = if expand { self.get_x_raw() } else { self.raw_token() };
                if self.is_macro_param(t2) {
                    out.push(t);
                    continue;
                }
                let n = if t2.is_char() { t2.chr().wrapping_sub(u32::from(b'0')) } else { u32::MAX };
                if (1..=params).contains(&n) {
                    out.push(Token(crate::expand::PAR_REF_FLAG | n));
                    continue;
                }
                self.error("Illegal parameter number in definition of \\");
                self.push_token(t2);
                out.push(t);
                continue;
            }
            out.push(t);
        }
        out.extend(hash_brace);
        out
    }
}

fn token_arg(packed: i64) -> Result<Token, String> {
    u32::try_from(packed)
        .map(Token)
        .map_err(|_| "lua <token> expected".to_string())
}

pub(crate) fn bytes_of(s: &LuaString) -> Vec<u8> {
    s.as_bytes().map(|b| b.to_vec()).unwrap_or_default()
}

impl Engine {
    /// `kpse.find_file(name, format)`: a disk path from the TeX search
    /// path, else the path of an embedded file in the archive's virtual
    /// tree (readable by `io.open`, see `lua_sys_embedded.lua`).
    fn lua_kpse_find(&mut self, name: &str, format: tex_kpse::Format) -> Option<String> {
        if let Some(path) = self.font_loader.kpse.find(name, format) {
            return Some(path.to_string_lossy().into_owned());
        }
        tex_kpse::Kpse::candidates(name, format)
            .into_iter()
            .find(|candidate| tex_kpse::has_embedded_package(candidate))
            .and_then(|member| tex_kpse::embedded_tree::member_path(&member))
    }
}

/// Read a file that `kpse.find_file` reported.
fn read_found_file(path: &str) -> Option<Vec<u8>> {
    if tex_kpse::embedded_tree::is_embedded_path(path) {
        tex_kpse::embedded_tree::read(path)
    } else {
        tex_kpse::fs::read(path).ok()
    }
}

/// Register a Rust function in `tbl`.
macro_rules! reg {
    ($lua:expr, $tbl:expr, $name:literal, $f:expr) => {
        $tbl.set($name, $lua.create_function($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}

/// Install the bridge libraries into a fresh Lua state: native helpers in
/// a private table consumed by [`LUA_PRELUDE`].
pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let b: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;

    // ---- token fields ----
    reg!(lua, b, "tok_cmd", |t: i64| -> Result<i64, String> {
        let t = token_arg(t)?;
        with_engine(|e| i64::from(e.lua_cmd_mode(t).0))
    });
    reg!(lua, b, "tok_mode", |t: i64| -> Result<i64, String> {
        let t = token_arg(t)?;
        with_engine(|e| e.lua_cmd_mode(t).1)
    });
    reg!(lua, b, "tok_index", |t: i64| -> Result<Option<i64>, String> {
        let t = token_arg(t)?;
        with_engine(|e| e.lua_tok_index(t))
    });
    reg!(lua, b, "tok_cmdname", |t: i64| -> Result<String, String> {
        let t = token_arg(t)?;
        with_engine(|e| {
            let cmd = e.lua_cmd_mode(t).0;
            crate::lua_cmds::COMMAND_NAMES[cmd as usize].to_string()
        })
    });
    reg!(lua, b, "tok_csname", |t: i64| -> Result<Option<String>, String> {
        let t = token_arg(t)?;
        with_engine(|e| e.lua_tok_csname(t))
    });
    reg!(lua, b, "tok_active", |t: i64| -> Result<bool, String> {
        let t = token_arg(t)?.unfreeze();
        Ok(t.is_char() && t.cc() == 13)
    });
    reg!(lua, b, "tok_expandable", |t: i64| -> Result<bool, String> {
        let t = token_arg(t)?;
        with_engine(|e| e.lua_cmd_mode(t).0 >= CMD_UNDEFINED_CS)
    });
    reg!(lua, b, "tok_protected", |t: i64| -> Result<bool, String> {
        let t = token_arg(t)?;
        with_engine(|e| e.lua_tok_is_protected(t))
    });
    reg!(lua, b, "tok_tok", |t: i64| -> Result<i64, String> {
        let t = token_arg(t)?.unfreeze();
        if t.is_cs() {
            Ok(CS_TOKEN_FLAG + i64::from(t.cs_id()))
        } else {
            Ok(i64::from(t.cc()) * (1 << 21) + i64::from(t.chr()))
        }
    });

    // ---- tex.print family and texio (ltexlib.c luac_store, ltexiolib.c) ----
    reg!(lua, b, "print_text", |partial: bool, cattable: i64, text: LuaString| -> Result<(), String> {
        let text = bytes_of(&text);
        let cattable = i32::try_from(cattable).unwrap_or(crate::engine_lua::DEFAULT_CAT_TABLE);
        with_engine(|e| e.lua_print_text(&text, partial, cattable))
    });
    reg!(lua, b, "print_token", |partial: bool, cattable: i64, t: i64| -> Result<(), String> {
        let t = token_arg(t)?;
        let cattable = i32::try_from(cattable).unwrap_or(crate::engine_lua::DEFAULT_CAT_TABLE);
        with_engine(|e| e.lua_print_token(t, partial, cattable))
    });
    reg!(lua, b, "texio_print", |target: i64, nl: bool, text: LuaString| -> Result<(), String> {
        let text = bytes_of(&text);
        with_engine(|e| e.lua_texio_print(target, nl, &text))
    });

    // ---- token construction ----
    reg!(lua, b, "create_cs", |name: LuaString| -> Result<i64, String> {
        let name = bytes_of(&name);
        with_engine(|e| i64::from(e.lua_create_cs(&name).0))
    });
    reg!(lua, b, "create_char", |chr: i64, cmd: Option<i64>| -> Result<i64, String> {
        with_engine(|e| {
            let c = u32::try_from(chr).map_err(|_| format!("invalid character code {chr}"))?;
            let cmd = match cmd {
                Some(cmd) => cmd,
                None => i64::from(e.eqtb.cat_code(c)),
            };
            let cmd = if matches!(cmd, 0 | 9 | 14 | 15) { 12 } else { cmd };
            e.lua_make_token(chr, cmd).map(|t| i64::from(t.0))
        })?
    });
    reg!(lua, b, "new", |chr: i64, cmd: i64| -> Result<i64, String> {
        with_engine(|e| e.lua_make_token(chr, cmd).map(|t| i64::from(t.0)))?
    });
    reg!(lua, b, "is_defined", |name: LuaString, exists: Option<bool>| -> Result<bool, String> {
        let name = bytes_of(&name);
        with_engine(|e| {
            if name.is_empty() {
                return false;
            }
            match e.cs.lookup(&name) {
                None => false,
                Some(_) if exists.unwrap_or(false) => true,
                Some(id) => e.eqtb.resolve(id).is_some(),
            }
        })
    });

    // ---- scanners ----
    reg!(lua, b, "get_next", || -> Result<i64, String> { with_engine(|e| i64::from(e.lua_get_next().0)) });
    reg!(lua, b, "scan_token", || -> Result<i64, String> { with_engine(|e| i64::from(e.lua_scan_token().0)) });
    reg!(lua, b, "scan_keyword", |kw: LuaString, cs: bool| -> Result<bool, String> {
        let kw = bytes_of(&kw);
        with_engine(|e| e.lua_scan_keyword(&kw, cs))
    });
    reg!(lua, b, "scan_int", || -> Result<i64, String> {
        with_engine(|e| {
            let saved = e.save_scanner();
            let v = e.scan_int();
            e.restore_scanner(saved);
            i64::from(v)
        })
    });
    reg!(lua, b, "scan_dimen", |mu: bool| -> Result<i64, String> {
        with_engine(|e| {
            let saved = e.save_scanner();
            let v = e.scan_dimen(mu, false);
            e.restore_scanner(saved);
            i64::from(v)
        })
    });
    reg!(lua, b, "scan_float", |exponent: bool| -> Result<Option<f64>, String> {
        with_engine(|e| e.lua_scan_float(exponent))
    });
    reg!(lua, b, "scan_toks", |expand: bool| -> Result<Vec<i64>, String> {
        with_engine(|e| {
            let saved = e.save_scanner();
            let toks = if expand {
                e.scan_general_text_expanded()
            } else {
                e.scan_general_text()
            };
            e.restore_scanner(saved);
            toks.into_iter().map(|t| i64::from(t.unfreeze().0)).collect()
        })
    });
    reg!(lua, b, "scan_toks_def", |expand: bool| -> Result<Vec<i64>, String> {
        with_engine(|e| {
            let saved = e.save_scanner();
            let toks = e.lua_scan_toks_def(expand);
            e.restore_scanner(saved);
            toks.into_iter().map(|t| i64::from(t.unfreeze().0)).collect()
        })
    });
    reg!(lua, b, "scan_string", || -> Result<Option<String>, String> { with_engine(Engine::lua_scan_string) });
    reg!(lua, b, "scan_argument", |expand: Option<bool>| -> Result<Option<String>, String> {
        with_engine(|e| e.lua_scan_argument(expand.unwrap_or(true)))
    });
    reg!(lua, b, "scan_word", || -> Result<Option<String>, String> { with_engine(Engine::lua_scan_word) });
    reg!(lua, b, "scan_csname", || -> Result<Option<String>, String> { with_engine(Engine::lua_scan_csname) });
    reg!(lua, b, "scan_code", |mask: i64| -> Result<Option<i64>, String> {
        with_engine(|e| e.lua_scan_code(mask))
    });
    reg!(lua, b, "put_next", |list: LuaTable| -> Result<(), String> {
        let packed: Vec<i64> = list.sequence_values().map_err(|e| format!("{e:?}"))?;
        let toks = packed.into_iter().map(token_arg).collect::<Result<Vec<_>, _>>()?;
        with_engine(|e| {
            if !toks.is_empty() {
                e.push_tokens_named(toks, "<lua put_next>");
            }
        })
    });
    reg!(lua, b, "expand", || -> Result<(), String> { with_engine(Engine::lua_expand) });

    // ---- primitives (ltexlib.c) ----
    reg!(lua, b, "enable_primitives", |prefix: LuaString, names: LuaTable| -> Result<(), String> {
        let prefix = bytes_of(&prefix);
        let names: Vec<LuaString> = names.sequence_values().map_err(|e| format!("{e:?}"))?;
        let names: Vec<Vec<u8>> = names.iter().map(bytes_of).collect();
        with_engine(|e| e.lua_enable_primitives(&prefix, &names))
    });
    reg!(lua, b, "primitive_names", |mask: i64| -> Result<Vec<LuaBytes>, String> {
        with_engine(|e| {
            e.lua_primitive_names(mask as u8)
                .into_iter()
                .map(|name| LuaBytes(name.to_vec()))
                .collect()
        })
    });

    // ---- macros and definitions ----
    reg!(lua, b, "get_macro", |name: LuaString, params: bool| -> Result<Option<String>, String> {
        let name = bytes_of(&name);
        with_engine(|e| e.lua_macro_text(&name, params))
    });
    reg!(lua, b, "set_macro", |table: Option<i64>, name: LuaString, body: Option<LuaString>, global: bool| -> Result<(), String> {
        let name = bytes_of(&name);
        let body = body.as_ref().map(bytes_of).unwrap_or_default();
        with_engine(|e| {
            let table = table.and_then(|t| i32::try_from(t).ok());
            let body = e.lua_string_to_macro_body(table, &body);
            let id = e.cs.intern(&name);
            let m = crate::eqtb::Macro {
                num_params: 0,
                has_param_refs: false,
                params: Vec::new(),
                prefix: Vec::new(),
                body: body.into(),
                long: false,
                outer: false,
                protected: false,
                replacement: Default::default(),
            };
            e.eqtb.assign(id, Equiv::Macro(std::rc::Rc::new(m)), global);
        })
    });
    reg!(lua, b, "set_char", |name: LuaString, value: i64, global: bool| -> Result<(), String> {
        let name = bytes_of(&name);
        with_engine(|e| {
            if let Ok(v) = u32::try_from(value) {
                let id = e.cs.intern(&name);
                e.eqtb.assign(id, Equiv::CharDef(v), global);
            }
        })
    });
    reg!(lua, b, "set_lua", |name: LuaString, slot: i64, protected: bool, global: bool| -> Result<(), String> {
        let name = bytes_of(&name);
        with_engine(|e| {
            let slot = u32::try_from(slot).unwrap_or(0);
            let id = e.cs.intern(&name);
            e.eqtb.assign(id, Equiv::LuaCall { slot, protected }, global);
        })
    });

    // ---- registers and codes ----
    reg!(lua, b, "count_get", |idx: Option<i64>, name: Option<LuaString>| -> Result<i64, String> {
        let name = name.as_ref().map(bytes_of);
        with_engine(|e| {
            let i = match name {
                Some(n) => match e.lua_named_int_param(&n) {
                    Some(p) => return Ok(i64::from(e.eqtb.int_params[p.idx() as usize])),
                    None => e.lua_register_index(&n, CMD_ASSIGN_INT, "count")?,
                },
                None => register_number(idx, "count")?,
            };
            Ok(i64::from(e.eqtb.count.get(i as usize).copied().unwrap_or(0)))
        })?
    });
    reg!(lua, b, "count_set", |idx: Option<i64>, name: Option<LuaString>, value: i64, global: bool| -> Result<(), String> {
        let name = name.as_ref().map(bytes_of);
        let v = i32::try_from(value).map_err(|_| "incorrect count value".to_string())?;
        with_engine(|e| {
            let global = e.lua_global(global);
            match name {
                Some(n) => match e.lua_named_int_param(&n) {
                    Some(p) => e.eqtb.assign_int_param(p, v, global),
                    None => {
                        let i = e.lua_register_index(&n, CMD_ASSIGN_INT, "count")?;
                        e.eqtb.assign_count(i, v, global);
                    }
                },
                None => e.eqtb.assign_count(register_number(idx, "count")?, v, global),
            }
            Ok(())
        })?
    });
    reg!(lua, b, "attribute_get", |idx: Option<i64>, name: Option<LuaString>| -> Result<i64, String> {
        let name = name.as_ref().map(bytes_of);
        with_engine(|e| {
            let n = match name {
                Some(n) => e.lua_register_index(&n, CMD_ASSIGN_ATTR, "attribute")?,
                None => register_number(idx, "attribute")?,
            };
            Ok(i64::from(e.eqtb.attribute(u32::from(n))))
        })?
    });
    reg!(lua, b, "attribute_set", |idx: Option<i64>, name: Option<LuaString>, value: i64, global: bool| -> Result<(), String> {
        let name = name.as_ref().map(bytes_of);
        let v = i32::try_from(value).map_err(|_| "incorrect attribute value".to_string())?;
        with_engine(|e| {
            let global = e.lua_global(global);
            let n = match name {
                Some(n) => e.lua_register_index(&n, CMD_ASSIGN_ATTR, "attribute")?,
                None => register_number(idx, "attribute")?,
            };
            e.eqtb.assign_attribute(u32::from(n), v, global);
            Ok(())
        })?
    });
    reg!(lua, b, "dimen_get", |idx: Option<i64>, name: Option<LuaString>| -> Result<i64, String> {
        let name = name.as_ref().map(bytes_of);
        with_engine(|e| {
            let i = match name {
                Some(n) => match e.lua_named_dim_param(&n) {
                    Some(p) => return Ok(i64::from(e.eqtb.dim_params[p.idx() as usize])),
                    None => e.lua_register_index(&n, CMD_ASSIGN_DIMEN, "dimen")?,
                },
                None => register_number(idx, "dimen")?,
            };
            Ok(i64::from(e.eqtb.dimen.get(i as usize).copied().unwrap_or(0)))
        })?
    });
    reg!(lua, b, "dimen_set", |idx: Option<i64>, name: Option<LuaString>, value: i64, global: bool| -> Result<(), String> {
        let name = name.as_ref().map(bytes_of);
        let v = i32::try_from(value).map_err(|_| "incorrect dimen value".to_string())?;
        with_engine(|e| {
            let global = e.lua_global(global);
            match name {
                Some(n) => match e.lua_named_dim_param(&n) {
                    Some(p) => e.eqtb.assign_dim_param(p, v, global),
                    None => {
                        let i = e.lua_register_index(&n, CMD_ASSIGN_DIMEN, "dimen")?;
                        e.eqtb.assign_dimen(i, v, global);
                    }
                },
                None => e.eqtb.assign_dimen(register_number(idx, "dimen")?, v, global),
            }
            Ok(())
        })?
    });
    reg!(lua, b, "toks_get", |idx: Option<i64>, name: Option<LuaString>| -> Result<String, String> {
        let name = name.as_ref().map(bytes_of);
        with_engine(|e| {
            let i = match name {
                Some(n) => e.lua_register_index(&n, CMD_ASSIGN_TOKS, "toks")?,
                None => register_number(idx, "toks")?,
            };
            let toks = e.eqtb.toks.get(i as usize).cloned().unwrap_or_default();
            Ok(e.tokens_to_string(&toks))
        })?
    });
    reg!(lua, b, "toks_set", |idx: Option<i64>, name: Option<LuaString>, value: LuaString, global: bool| -> Result<(), String> {
        let name = name.as_ref().map(bytes_of);
        let toks = Engine::lua_str_toks(&bytes_of(&value));
        with_engine(|e| {
            let global = e.lua_global(global);
            let i = match name {
                Some(n) => e.lua_register_index(&n, CMD_ASSIGN_TOKS, "toks")?,
                None => register_number(idx, "toks")?,
            };
            e.eqtb.assign_toks_reg(i, std::rc::Rc::new(toks), global);
            Ok(())
        })?
    });
    reg!(lua, b, "catcode_set", |table: Option<i64>, c: i64, value: i64, global: bool| -> Result<(), String> {
        let c = u32::try_from(c).ok().filter(|c| *c <= 0x10_FFFF).ok_or("incorrect character value")?;
        let v = u8::try_from(value).ok().filter(|v| *v < 16).ok_or("incorrect catcode value")?;
        with_engine(|e| {
            let global = e.lua_global(global);
            match table {
                Some(t) => {
                    let t = i32::try_from(t)
                        .ok()
                        .filter(|t| (0..=crate::eqtb::MAX_CAT_TABLE).contains(t))
                        .ok_or("invalid catcode table")?;
                    e.eqtb.assign_cat_code_in(t, c, v, global);
                }
                None => e.eqtb.assign_cat_code(c, v, global),
            }
            Ok::<(), String>(())
        })?
    });
    // ltexlib.c `getcatcode`: luatex `get_cat_code(table, c)`.
    reg!(lua, b, "catcode_get", |table: Option<i64>, c: i64| -> Result<i64, String> {
        let c = u32::try_from(c).map_err(|_| "incorrect character value".to_string())?;
        with_engine(|e| match table.and_then(|t| i32::try_from(t).ok()) {
            Some(t) => i64::from(e.eqtb.cat_code_in(t, c)),
            None => i64::from(e.eqtb.cat_code(c)),
        })
    });
    reg!(lua, b, "inputlineno", || -> Result<i64, String> {
        with_engine(|e| i64::from(e.input.current_file_line()))
    });
    reg!(lua, b, "tex_error", |msg: LuaString| -> Result<(), String> {
        let msg = String::from_utf8_lossy(&bytes_of(&msg)).into_owned();
        with_engine(|e| e.error(&msg))
    });
    reg!(lua, b, "runtoks_toks", |idx: Option<i64>, name: Option<LuaString>| -> Result<(), String> {
        let name = name.as_ref().map(bytes_of);
        with_engine(|e| {
            let i = match name {
                Some(n) => e.lua_register_index(&n, CMD_ASSIGN_TOKS, "toks")?,
                None => register_number(idx, "toks")?,
            };
            let toks = (*e.eqtb.toks.get(i as usize).cloned().unwrap_or_default()).clone();
            if !toks.is_empty() {
                // ltexlib.c runtoks: the end marker sits below the register's tokens
                let end = e.lua_end_local_control();
                e.push_token(end);
                e.push_tokens_named(toks, "<lua runtoks>");
                e.lua_local_control();
            }
            Ok(())
        })?
    });
    reg!(lua, b, "runtoks_begin", || -> Result<(), String> {
        with_engine(|e| {
            let end = e.lua_end_local_control();
            e.push_token(end);
            e.local_level += 1;
            e.lua_local_entered = true;
        })
    });
    reg!(lua, b, "runtoks_pending", || -> Result<(), String> { with_engine(Engine::lua_local_control) });

    // ---- bytecode registers and chunk names (dumped into the format) ----
    reg!(lua, b, "bytecode_set", |slot: i64, code: Option<LuaString>| -> Result<(), String> {
        let slot = u32::try_from(slot).map_err(|_| "negative values not allowed".to_string())?;
        let code = code.as_ref().map(bytes_of);
        with_engine(|e| match code {
            Some(code) => {
                e.lua_bytecodes.insert(slot, code);
            }
            None => {
                e.lua_bytecodes.remove(&slot);
            }
        })
    });
    reg!(lua, b, "bytecode_get", |slot: i64| -> Result<Option<LuaBytes>, String> {
        with_engine(|e| {
            u32::try_from(slot)
                .ok()
                .and_then(|k| e.lua_bytecodes.get(&k).cloned())
                .map(LuaBytes)
        })
    });
    reg!(lua, b, "name_set", |slot: i64, name: Option<LuaString>| -> Result<(), String> {
        let name = name.as_ref().map(|n| String::from_utf8_lossy(&bytes_of(n)).into_owned());
        with_engine(|e| {
            if let Ok(k) = u16::try_from(slot) {
                match name {
                    Some(n) => {
                        e.lua_names.insert(k, n);
                    }
                    None => {
                        e.lua_names.remove(&k);
                    }
                }
            }
        })
    });
    reg!(lua, b, "name_get", |slot: i64| -> Result<Option<String>, String> {
        with_engine(|e| u16::try_from(slot).ok().and_then(|k| e.lua_names.get(&k).cloned()))
    });
    reg!(lua, b, "callback_set", |index: i64, state: i64| -> Result<(), String> {
        with_engine(|e| {
            if let Some(slot) = usize::try_from(index).ok().and_then(|i| e.lua_cb.get_mut(i)) {
                *slot = state.clamp(-1, 1) as i8;
            }
        })
    });

    // ---- kpathsea ----
    reg!(lua, b, "lua_module", |name: String| -> Result<Option<(LuaBytes, String)>, String> {
        with_engine(|e| {
            let alt = name.replace('.', "/");
            let found = [alt.as_str(), name.as_str()].into_iter().find_map(|n| {
                let path = e.lua_kpse_find(n, tex_kpse::Format::Lua)?;
                let bytes = read_found_file(&path)?;
                e.record_loaded_bytes(std::path::Path::new(&path), &bytes);
                Some((LuaBytes(bytes), path))
            });
            found
        })
    });

    crate::lua_ud::install_tokens(lua, &b)?;
    lua.set_global("__ratex_bridge", b).map_err(|e| format!("{e:?}"))?;
    let names: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;
    for (code, name) in crate::lua_cmds::COMMAND_NAMES.iter().enumerate() {
        names.raw_seti(code as i64, *name).map_err(|e| format!("{e:?}"))?;
    }
    lua.set_global("__ratex_command_names", names).map_err(|e| format!("{e:?}"))?;
    let (function_slot, callback_lookup): (LuaFunction, LuaFunction) = lua
        .load(LUA_PRELUDE)
        .set_name("=[ratex bridge]")
        .call(())
        .map_err(|e| format!("bridge prelude: {}", lua.get_error_message(e).message()))?;
    lua.registry_set(REG_FUNCTION, function_slot).map_err(|e| format!("{e:?}"))?;
    lua.registry_set(REG_CALLBACK, callback_lookup).map_err(|e| format!("{e:?}"))?;
    crate::lua_tex::install(lua)
}

fn register_number(idx: Option<i64>, what: &str) -> Result<u16, String> {
    idx.and_then(|i| u16::try_from(i).ok())
        .ok_or_else(|| format!("incorrect {what} index"))
}

impl Engine {
    /// lnewtokenlib.c `get_index`: the register or character a token is
    /// about, `None` when it has no index.
    pub(crate) fn lua_tok_index(&self, t: Token) -> Option<i64> {
        let (cmd, mode) = self.lua_cmd_mode(t);
        let index = match cmd {
            CMD_ASSIGN_INT => mode - COUNT_BASE,
            CMD_ASSIGN_ATTR => mode - ATTRIBUTE_BASE,
            CMD_ASSIGN_DIMEN => mode - DIMEN_BASE,
            CMD_ASSIGN_GLUE => mode - SKIP_BASE,
            CMD_ASSIGN_MU_GLUE => mode - MU_SKIP_BASE,
            CMD_ASSIGN_TOKS => mode - TOKS_BASE,
            _ => mode,
        };
        (0..=65535).contains(&index).then_some(index)
    }

    /// `tok` of a token object: the engine-independent token value.
    pub(crate) fn lua_tok_value(t: Token) -> i64 {
        let t = t.unfreeze();
        if t.is_cs() {
            CS_TOKEN_FLAG + i64::from(t.cs_id())
        } else if t.0 >= crate::expand::PAR_REF_FLAG {
            5 * (1 << 21) + i64::from(t.0 & 0xF)
        } else if t.cc() == 0 {
            13 * (1 << 21) + i64::from(t.chr())
        } else {
            i64::from(t.cc()) * (1 << 21) + i64::from(t.chr())
        }
    }

    fn lua_named_param(&self, name: &[u8]) -> Option<Prim> {
        let id = self.cs.lookup(name)?;
        match self.eqtb.resolve(id) {
            Some(Equiv::Prim(p)) => Some(*p),
            _ => None,
        }
    }

    fn lua_named_int_param(&self, name: &[u8]) -> Option<IntParam> {
        match self.lua_named_param(name)? {
            Prim::IntP(p) => Some(p),
            _ => None,
        }
    }

    fn lua_named_dim_param(&self, name: &[u8]) -> Option<crate::prim::DimParam> {
        match self.lua_named_param(name)? {
            Prim::DimP(p) => Some(p),
            _ => None,
        }
    }
}

/// Registry keys of the two Lua-side lookups the prelude hands to Rust, so
/// they never appear in `_G`.
const REG_FUNCTION: &str = "ratex.function";
const REG_CALLBACK: &str = "ratex.callback";
/// Registry key of the table `font.getfont` consults.
pub(crate) const REG_FONT_CACHE: &str = "ratex.font_cache";

impl LuaEngine {
    /// The function registered in `lua.get_functions_table()` at `slot`.
    fn lua_function_slot(&mut self, slot: i64) -> Result<Option<LuaFunction>, String> {
        let lookup: LuaFunction = self
            .lua
            .registry_get(REG_FUNCTION)
            .ok()
            .flatten()
            .ok_or("function lookup missing")?;
        lookup.call(slot).map_err(|e| self.lua.get_error_message(e).message().to_string())
    }

    /// The Lua function registered for callback `name`, if any.
    pub(crate) fn lua_callback_fn(&mut self, name: &str) -> Result<Option<LuaFunction>, String> {
        let lookup: LuaFunction = self
            .lua
            .registry_get(REG_CALLBACK)
            .ok()
            .flatten()
            .ok_or("callback lookup missing")?;
        lookup.call(name).map_err(|e| self.lua.get_error_message(e).message().to_string())
    }

    /// The function behind `\luafunction n`.
    pub(crate) fn call_function_slot(&mut self, slot: i32) -> Result<(), String> {
        let f: Option<LuaFunction> = self
            .lua_function_slot(i64::from(slot))?;
        if let Some(f) = f {
            f.call::<_, ()>(i64::from(slot))
                .map_err(|e| self.lua.get_error_message(e).message().to_string())?;
        }
        Ok(())
    }

    pub(crate) fn call_bytecode(&mut self, slot: i32, code: &[u8]) -> Result<(), String> {
        let chunk = self
            .lua
            .create_bytes(code)
            .map_err(|e| format!("{e:?}"))?;
        self.lua
            .load("local code, slot = ... local f = assert(load(code, 'bytecode', 'b')) f(slot)")
            .set_name("=[luabytecode]")
            .call::<_, ()>((chunk, i64::from(slot)))
            .map_err(|e| self.lua.get_error_message(e).message().to_string())
    }

    pub(crate) fn has_callback(&mut self, name: &str) -> bool {
        matches!(self.lua_callback_fn(name), Ok(Some(_)))
    }

    /// Call callback `name` with an optional string argument and return its
    /// string result.
    pub(crate) fn call_callback(&mut self, name: &str, arg: Option<&str>) -> Result<Option<String>, String> {
        let f = self.lua_callback_fn(name)?;
        let Some(f) = f else {
            return Ok(None);
        };
        let result: Option<String> = match arg {
            Some(arg) => f.call(arg.to_string()),
            None => f.call(()),
        }
        .map_err(|e| self.lua.get_error_message(e).message().to_string())?;
        Ok(result)
    }
}

/// Lua side of the bridge: the `token`, `tex`, `lua`, `callback` and `kpse`
/// tables, the package searchers and the embedded-file `io` layer.
const LUA_PRELUDE: &str = include_str!("lua_bridge.lua");
