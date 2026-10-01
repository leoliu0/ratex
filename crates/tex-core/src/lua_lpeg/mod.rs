//! LPeg 1.0.1, the pattern-matching library LuaTeX ships as the global
//! `lpeg` (and `package.loaded.lpeg`), implemented over the `tex_lua` host
//! API: patterns are userdata whose metatable holds the operators, compiled
//! lazily to LPeg's instruction array and run by a backtrack-stack machine.
//!
//! The module layout follows LPeg's sources: `tree` (lptree.h and the
//! analyses of lpcode.c), `build` (lptree.c), `code` (lpcode.c), `vm`
//! (lpvm.c), `capture` (lpcap.c) and `print` (lpprint.c).

mod build;
mod capture;
mod code;
mod print;
mod tree;
mod value;
mod vm;

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use tex_lua::{
    CallbackLua, Lua, LuaApi, LuaBytes, LuaError, LuaFunction, LuaResult, LuaString, LuaTable, LuaValueKind,
    UserDataRef, UserDataTrait, Value,
};

use build::*;
use capture::{CapState, Env, LResult};
use tree::*;
use value::V;

const VERSION: &str = "1.0.1";
const PATTERN_NAME: &str = "lpeg-pattern";

/// The Lua userdata of a pattern.
struct PatUd(Rc<Pat>);

impl UserDataTrait for PatUd {
    fn type_name(&self) -> &'static str {
        PATTERN_NAME
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// State shared by the library's functions of one Lua state.
struct Shared {
    metatable: LuaTable,
    max_stack: Cell<usize>,
}

/// A pattern operand: the pattern, and the Lua userdata it came from (none
/// when the operand was coerced from a string, number, table...).
struct Operand {
    pat: Rc<Pat>,
    ud: Option<Value>,
}

// ---- values -------------------------------------------------------------

fn value_to_v(value: Value) -> V {
    match value.kind() {
        LuaValueKind::Nil => V::Nil,
        LuaValueKind::Boolean => V::Bool(value.as_boolean().unwrap_or(false)),
        LuaValueKind::Integer => V::Int(value.as_integer().unwrap_or(0)),
        LuaValueKind::Float => V::Num(value.as_number().unwrap_or(0.0)),
        LuaValueKind::String => {
            V::Str(Rc::from(value.as_string_handle().map(|s| s.to_bytes()).unwrap_or_default()))
        }
        _ => V::Obj(value),
    }
}

fn arg_v(cx: &mut CallbackLua<'_>, idx: usize) -> LuaResult<V> {
    match cx.arg_kind(idx) {
        None | Some(LuaValueKind::Nil) => Ok(V::Nil),
        Some(LuaValueKind::Boolean) => Ok(V::Bool(cx.arg::<bool>(idx)?)),
        Some(LuaValueKind::Integer) => Ok(V::Int(cx.arg::<i64>(idx)?)),
        Some(LuaValueKind::Float) => Ok(V::Num(cx.arg::<f64>(idx)?)),
        Some(LuaValueKind::String) => {
            let s: LuaString = cx.arg(idx)?;
            Ok(V::Str(Rc::from(s.to_bytes())))
        }
        Some(_) => Ok(V::Obj(cx.arg::<Value>(idx)?)),
    }
}

/// `luaL_checklstring`: a string or a number converted to one.
fn check_bytes(cx: &mut CallbackLua<'_>, idx: usize) -> LuaResult<Vec<u8>> {
    match arg_v(cx, idx)? {
        V::Str(s) => Ok(s.to_vec()),
        v @ (V::Int(_) | V::Num(_)) => Ok(v.to_bytes().expect("number").into_owned()),
        _ => Err(cx.type_error(idx, "string")),
    }
}

/// `luaL_checkinteger`.
fn check_integer(cx: &mut CallbackLua<'_>, idx: usize) -> LuaResult<i64> {
    let v = arg_v(cx, idx)?;
    integer_of(cx, idx, v)
}

fn integer_of(cx: &mut CallbackLua<'_>, idx: usize, v: V) -> LuaResult<i64> {
    match &v {
        V::Int(i) => Ok(*i),
        V::Num(_) | V::Str(_) => {
            let numeric = match &v {
                V::Str(s) => std::str::from_utf8(s).ok().is_some_and(|t| t.trim().parse::<f64>().is_ok()),
                _ => true,
            };
            if !numeric {
                return Err(cx.type_error(idx, "number"));
            }
            v.to_integer().ok_or_else(|| cx.arg_error(idx, "number has no integer representation"))
        }
        _ => Err(cx.type_error(idx, "number")),
    }
}

fn v_to_value(cx: &mut CallbackLua<'_>, v: &V) -> LuaResult<Value> {
    match v {
        V::Nil => cx.pack(Option::<i64>::None),
        V::Bool(b) => cx.pack(*b),
        V::Int(i) => cx.pack(*i),
        V::Num(n) => cx.pack(*n),
        V::Str(s) => cx.pack(LuaBytes(s.to_vec())),
        V::Obj(o) => Ok(o.clone()),
    }
}

fn push_v(cx: &mut CallbackLua<'_>, v: &V) -> LuaResult<usize> {
    match v {
        V::Nil => cx.push(Option::<i64>::None),
        V::Bool(b) => cx.push(*b),
        V::Int(i) => cx.push(*i),
        V::Num(n) => cx.push(*n),
        V::Str(s) => cx.push(LuaBytes(s.to_vec())),
        V::Obj(o) => cx.push(o),
    }
}

/// `val2str`: a rule name for error messages.
fn val2str(v: &V) -> String {
    match v.to_bytes() {
        Some(b) => String::from_utf8_lossy(&b).into_owned(),
        None => format!("(a {})", v.type_name()),
    }
}

fn fail(cx: &mut CallbackLua<'_>, f: Fail) -> LuaError {
    match f {
        Fail::Arg(n, message) => cx.arg_error(n, &message),
        Fail::Error(message) => cx.error(message),
    }
}

// ---- Lua access during matching ------------------------------------------

struct CxEnv<'a, 'b> {
    cx: &'a mut CallbackLua<'b>,
}

impl Env for CxEnv<'_, '_> {
    fn call(&mut self, f: &V, args: &[V]) -> LResult<Vec<V>> {
        let function = match f {
            V::Obj(o) => o.as_function(),
            _ => None,
        };
        let Some(function) = function else {
            return Err(self.cx.error(format!("attempt to call a {} value", f.type_name())));
        };
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            values.push(v_to_value(self.cx, a)?);
        }
        let results: tex_lua::Variadic<Value> = function.call(tex_lua::Variadic(values))?;
        Ok(results.0.into_iter().map(value_to_v).collect())
    }

    fn index(&mut self, table: &V, key: &V) -> LResult<V> {
        let V::Obj(t) = table else {
            return Err(self.cx.error(format!("attempt to index a {} value", table.type_name())));
        };
        let Some(t) = t.as_table() else {
            return Err(self.cx.error(format!("attempt to index a {} value", table.type_name())));
        };
        let key = v_to_value(self.cx, key)?;
        Ok(value_to_v(t.get::<Value>(key)?))
    }

    fn new_table(&mut self) -> LResult<V> {
        let t = self.cx.create_table()?;
        Ok(V::Obj(self.cx.pack(t)?))
    }

    fn table_set(&mut self, table: &V, key: &V, value: &V) -> LResult<()> {
        let V::Obj(t) = table else { unreachable!("table capture") };
        let t = t.as_table().expect("table capture");
        let key = v_to_value(self.cx, key)?;
        let value = v_to_value(self.cx, value)?;
        t.raw_set(key, value)
    }

    fn error(&mut self, message: String) -> LuaError {
        self.cx.error(message)
    }
}

// ---- pattern values -------------------------------------------------------

fn new_value(cx: &mut CallbackLua<'_>, shared: &Shared, pat: Pat) -> LuaResult<Value> {
    new_value_rc(cx, shared, Rc::new(pat))
}

fn new_value_rc(cx: &mut CallbackLua<'_>, shared: &Shared, pat: Rc<Pat>) -> LuaResult<Value> {
    let ud = cx.create_userdata(PatUd(pat))?;
    let value = cx.pack(&ud)?;
    value.set_metatable(Some(&shared.metatable))?;
    Ok(value)
}

fn push_new(cx: &mut CallbackLua<'_>, shared: &Shared, pat: Pat) -> LuaResult<usize> {
    let value = new_value(cx, shared, pat)?;
    cx.push(value)
}

/// Push an operand as the result of an operation (the very same userdata
/// when it was one already).
fn push_operand(cx: &mut CallbackLua<'_>, shared: &Shared, op: &Operand) -> LuaResult<usize> {
    match &op.ud {
        Some(v) => cx.push(v),
        None => {
            let value = new_value_rc(cx, shared, op.pat.clone())?;
            cx.push(value)
        }
    }
}

fn push_built(cx: &mut CallbackLua<'_>, shared: &Shared, ops: &[&Operand], built: Built) -> LuaResult<usize> {
    match built {
        Built::Operand(i) => push_operand(cx, shared, ops[i]),
        Built::New(pat) => push_new(cx, shared, pat),
    }
}

/// `getpatt`: the pattern an argument stands for.
fn getpatt(cx: &mut CallbackLua<'_>, shared: &Shared, idx: usize) -> LuaResult<Operand> {
    let operand = |pat: Pat| Operand { pat: Rc::new(pat), ud: None };
    match cx.arg_kind(idx) {
        Some(LuaValueKind::String) => {
            let s: LuaString = cx.arg(idx)?;
            Ok(operand(from_string(&s.to_bytes())))
        }
        Some(LuaValueKind::Integer) => Ok(operand(from_int(cx.arg::<i64>(idx)? as i32))),
        Some(LuaValueKind::Float) => {
            let n = V::Num(cx.arg::<f64>(idx)?).to_integer().unwrap_or(0);
            Ok(operand(from_int(n as i32)))
        }
        Some(LuaValueKind::Boolean) => Ok(operand(from_bool(cx.arg::<bool>(idx)?))),
        Some(LuaValueKind::Table) => Ok(operand(newgrammar(cx, shared, idx)?)),
        Some(LuaValueKind::Function | LuaValueKind::CFunction | LuaValueKind::CClosure | LuaValueKind::RClosure) => {
            let f: Value = cx.arg(idx)?;
            Ok(operand(from_function(V::Obj(f))))
        }
        Some(LuaValueKind::Userdata) => {
            let value: Value = cx.arg(idx)?;
            match value.as_userdata::<PatUd>() {
                Some(ud) => {
                    let pat = ud.borrow()?.0.clone();
                    Ok(Operand { pat, ud: Some(value) })
                }
                None => Err(cx.type_error(idx, PATTERN_NAME)),
            }
        }
        _ => Err(cx.type_error(idx, PATTERN_NAME)),
    }
}

fn pattern_of(value: &Value) -> Option<Rc<Pat>> {
    let ud = value.as_userdata::<PatUd>()?;
    let pat = ud.borrow().ok()?.0.clone();
    Some(pat)
}

/// `newgrammar`: a pattern from a table of rules.
fn newgrammar(cx: &mut CallbackLua<'_>, _shared: &Shared, idx: usize) -> LuaResult<Pat> {
    let table: LuaTable = cx.arg(idx)?;
    let first = value_to_v(table.raw_geti::<Value>(1)?);
    // the initial rule: given by name, or the pattern at [1]
    let (init_key, init_rule) = if first.is_stringy() {
        let rule = value_to_v(table.get::<Value>(v_to_value(cx, &first)?)?);
        (first, rule)
    } else {
        (V::Int(1), first)
    };
    let init_pat = match &init_rule {
        V::Obj(o) => pattern_of(o),
        _ => None,
    };
    let Some(init_pat) = init_pat else {
        return Err(if init_rule.is_nil() {
            cx.error("grammar has no initial rule")
        } else {
            cx.error(format!("initial rule '{}' is not a pattern", val2str(&init_key)))
        });
    };
    let mut rules = vec![Rule { name: init_key.clone(), pat: init_pat }];
    for (key, value) in table.pairs::<Value, Value>()? {
        let key = value_to_v(key);
        let is_first = match &key {
            V::Int(1) => true,
            V::Num(n) => *n == 1.0,
            V::Str(_) => key.to_bytes().is_some_and(|b| {
                std::str::from_utf8(&b).ok().and_then(|t| t.trim().parse::<f64>().ok()) == Some(1.0)
            }),
            _ => false,
        };
        if is_first || key.equals(&init_key) {
            continue;
        }
        let Some(pat) = pattern_of(&value) else {
            return Err(cx.error(format!("rule '{}' is not a pattern", val2str(&key))));
        };
        rules.push(Rule { name: key, pat });
    }
    grammar(&rules, &val2str).map_err(|f| fail(cx, f))
}

// ---- matching -----------------------------------------------------------

/// The compiled code of a pattern, compiling on first use.
fn compiled(cx: &mut CallbackLua<'_>, pat: &Pat) -> LuaResult<Rc<Vec<u32>>> {
    if let Some(code) = pat.code.borrow().as_ref() {
        return Ok(code.clone());
    }
    let mut tree = pat.tree.clone();
    let empty = Rc::new(Vec::new());
    let ktable = pat.ktable.as_ref().unwrap_or(&empty);
    finalfix(&mut tree, 0, None, ktable, &val2str).map_err(|f| fail(cx, f))?;
    let code = Rc::new(code::compile(&mut tree));
    *pat.code.borrow_mut() = Some(code.clone());
    Ok(code)
}

fn has_runtime(code: &[u32]) -> bool {
    let mut i = 0;
    while i < code.len() {
        if code::op(code, i) == code::ICLOSERUNTIME {
            return true;
        }
        i += code::sizei(code, i);
    }
    false
}

/// `lpeg.match(pattern, subject [, init [, ...]])`.
fn lp_match(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let operand = getpatt(cx, shared, 1)?;
    let code = compiled(cx, &operand.pat)?;
    // the subject
    let (subject_handle, subject_value): (Option<LuaString>, Option<Vec<u8>>) = match cx.arg_kind(2) {
        Some(LuaValueKind::String) => (Some(cx.arg(2)?), None),
        Some(LuaValueKind::Integer | LuaValueKind::Float) => (None, Some(check_bytes(cx, 2)?)),
        _ => return Err(cx.type_error(2, "string")),
    };
    let borrowed = subject_handle.as_ref().map(|h| h.as_bytes().expect("live subject"));
    let subject: &[u8] = match (&borrowed, &subject_value) {
        (Some(b), _) => b,
        (_, Some(v)) => v,
        _ => unreachable!("subject"),
    };
    // the initial position
    let len = subject.len();
    let init = match cx.arg_kind(3) {
        None | Some(LuaValueKind::Nil) => 1,
        _ => check_integer(cx, 3)?,
    };
    let start = if init > 0 {
        if (init as u64) <= len as u64 { init as usize - 1 } else { len }
    } else if init.unsigned_abs() <= len as u64 {
        len - init.unsigned_abs() as usize
    } else {
        0
    };
    // extra arguments
    let nargs = cx.arg_count();
    let mut args = Vec::new();
    for idx in 4..=nargs {
        args.push(arg_v(cx, idx)?);
    }
    let subject_v = if has_runtime(&code) { value_to_v(cx.arg::<Value>(2)?) } else { V::Nil };
    let empty = Rc::new(Vec::new());
    let ktable = operand.pat.ktable.as_ref().unwrap_or(&empty);
    let margs = vm::MatchArgs {
        subject,
        subject_v,
        ktable,
        args: &args,
        max_stack: shared.max_stack.get(),
    };
    let matched = vm::run(&code, start, &margs, &mut CxEnv { cx })?;
    let Some(matched) = matched else {
        return cx.push(Option::<i64>::None);
    };
    // getcaptures
    let mut env = CxEnv { cx };
    let mut cs = CapState {
        caps: &matched.caps,
        cap: 0,
        subject,
        ktable,
        dynvals: &matched.dynvals,
        args: &args,
        env: &mut env,
        stack: Vec::new(),
    };
    let mut n = 0;
    if !matched.caps[0].isclose() {
        loop {
            n += cs.pushcapture()?;
            if cs.caps[cs.cap].isclose() {
                break;
            }
        }
    }
    let results = cs.stack;
    if n == 0 {
        return cx.push(matched.end as i64 + 1 - 0);
    }
    let mut pushed = 0;
    for v in &results {
        pushed += push_v(cx, v)?;
    }
    Ok(pushed)
}

// ---- operators and constructors --------------------------------------------

fn lp_seq(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let a = getpatt(cx, shared, 1)?;
    let b = getpatt(cx, shared, 2)?;
    let built = seq(&a.pat, &b.pat).map_err(|f| fail(cx, f))?;
    push_built(cx, shared, &[&a, &b], built)
}

fn lp_choice(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let a = getpatt(cx, shared, 1)?;
    let b = getpatt(cx, shared, 2)?;
    let built = choice(&a.pat, &b.pat).map_err(|f| fail(cx, f))?;
    push_built(cx, shared, &[&a, &b], built)
}

fn lp_diff(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let a = getpatt(cx, shared, 1)?;
    let b = getpatt(cx, shared, 2)?;
    let pat = diff(&a.pat, &b.pat).map_err(|f| fail(cx, f))?;
    push_new(cx, shared, pat)
}

fn lp_star(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let n = check_integer(cx, 2)? as i32;
    let a = getpatt(cx, shared, 1)?;
    let pat = star(&a.pat, n).map_err(|f| fail(cx, f))?;
    push_new(cx, shared, pat)
}

fn lp_not(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let a = getpatt(cx, shared, 1)?;
    push_new(cx, shared, not(&a.pat))
}

fn lp_and(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let a = getpatt(cx, shared, 1)?;
    push_new(cx, shared, and(&a.pat))
}

fn lp_behind(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let a = getpatt(cx, shared, 1)?;
    let pat = behind(&a.pat).map_err(|f| fail(cx, f))?;
    push_new(cx, shared, pat)
}

fn lp_divcapture(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let pat = match cx.arg_kind(2) {
        Some(LuaValueKind::Function | LuaValueKind::CFunction | LuaValueKind::CClosure | LuaValueKind::RClosure) => {
            let f = arg_v(cx, 2)?;
            let a = getpatt(cx, shared, 1)?;
            capture(&a.pat, CFUNCTION, Some(f))
        }
        Some(LuaValueKind::Table) => {
            let t = arg_v(cx, 2)?;
            let a = getpatt(cx, shared, 1)?;
            capture(&a.pat, CQUERY, Some(t))
        }
        Some(LuaValueKind::String) => {
            let s = arg_v(cx, 2)?;
            let a = getpatt(cx, shared, 1)?;
            capture(&a.pat, CSTRING, Some(s))
        }
        Some(LuaValueKind::Integer | LuaValueKind::Float) => {
            let n = arg_v(cx, 2)?.to_integer().unwrap_or(0);
            let a = getpatt(cx, shared, 1)?;
            num_capture(&a.pat, n).map_err(|f| fail(cx, f))?
        }
        _ => return Err(cx.arg_error(2, "invalid replacement value")),
    };
    push_new(cx, shared, pat)
}

fn simple_capture(cx: &mut CallbackLua<'_>, shared: &Shared, kind: u8) -> LuaResult<usize> {
    let a = getpatt(cx, shared, 1)?;
    push_new(cx, shared, capture(&a.pat, kind, None))
}

fn lp_groupcapture(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let name = match cx.arg_kind(2) {
        None | Some(LuaValueKind::Nil) => None,
        _ => Some(V::str(&check_bytes(cx, 2)?)),
    };
    let a = getpatt(cx, shared, 1)?;
    push_new(cx, shared, capture(&a.pat, CGROUP, name))
}

fn is_function_kind(kind: Option<LuaValueKind>) -> bool {
    matches!(
        kind,
        Some(LuaValueKind::Function | LuaValueKind::CFunction | LuaValueKind::CClosure | LuaValueKind::RClosure)
    )
}

fn lp_foldcapture(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    if !is_function_kind(cx.arg_kind(2)) {
        return Err(cx.type_error(2, "function"));
    }
    let f = arg_v(cx, 2)?;
    let a = getpatt(cx, shared, 1)?;
    push_new(cx, shared, capture(&a.pat, CFOLD, Some(f)))
}

fn lp_matchtime(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    if !is_function_kind(cx.arg_kind(2)) {
        return Err(cx.type_error(2, "function"));
    }
    let f = arg_v(cx, 2)?;
    let a = getpatt(cx, shared, 1)?;
    push_new(cx, shared, match_time(&a.pat, f))
}

fn lp_p(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    if cx.arg_count() == 0 {
        return Err(cx.arg_error(1, "value expected"));
    }
    let a = getpatt(cx, shared, 1)?;
    push_operand(cx, shared, &a)
}

fn lp_set(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let s = check_bytes(cx, 1)?;
    push_new(cx, shared, set(&s))
}

fn lp_range(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let mut ranges = Vec::new();
    for idx in 1..=cx.arg_count() {
        let r = check_bytes(cx, idx)?;
        if r.len() != 2 {
            return Err(cx.arg_error(idx, "range must have two characters"));
        }
        ranges.push(r);
    }
    let pat = range(&ranges).map_err(|f| fail(cx, f))?;
    push_new(cx, shared, pat)
}

fn lp_v(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let pat = if cx.arg_count() == 0 {
        Pat::new(vec![TTree { tag: TOPENCALL, key: 1, ..TTree::default() }], None)
    } else {
        open_call(arg_v(cx, 1)?).map_err(|f| fail(cx, f))?
    };
    push_new(cx, shared, pat)
}

fn lp_constcapture(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let mut values = Vec::new();
    for idx in 1..=cx.arg_count() {
        values.push(arg_v(cx, idx)?);
    }
    push_new(cx, shared, const_capture(&values))
}

fn lp_argcapture(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let n = check_integer(cx, 1)?;
    let pat = arg_capture(n).map_err(|f| fail(cx, f))?;
    push_new(cx, shared, pat)
}

fn lp_backref(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let name = arg_v(cx, 1)?;
    push_new(cx, shared, backref(name))
}

fn lp_type(cx: &mut CallbackLua<'_>) -> LuaResult<usize> {
    let is_pattern = matches!(cx.arg_kind(1), Some(LuaValueKind::Userdata))
        && cx.arg::<Value>(1)?.as_userdata::<PatUd>().is_some();
    if is_pattern {
        cx.push("pattern")
    } else {
        cx.push(Option::<i64>::None)
    }
}

fn lp_setmax(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let max = check_integer(cx, 1)?;
    if !(0 < max && max <= i32::MAX as i64) {
        return Err(cx.arg_error(1, "out of range"));
    }
    shared.max_stack.set(max as usize);
    Ok(0)
}

fn lp_locale(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let table: LuaTable = match cx.arg_kind(1) {
        None | Some(LuaValueKind::Nil) => cx.create_table_with_capacity(0, 12)?,
        Some(LuaValueKind::Table) => cx.arg(1)?,
        _ => return Err(cx.type_error(1, "table")),
    };
    type Class = (&'static str, fn(u8) -> bool);
    let classes: [Class; 11] = [
        ("alnum", |c| c.is_ascii_alphanumeric()),
        ("alpha", |c| c.is_ascii_alphabetic()),
        ("cntrl", |c| c.is_ascii_control()),
        ("digit", |c| c.is_ascii_digit()),
        ("graph", |c| c.is_ascii_graphic()),
        ("lower", |c| c.is_ascii_lowercase()),
        ("print", |c| c.is_ascii_graphic() || c == b' '),
        ("punct", |c| c.is_ascii_punctuation()),
        ("space", |c| matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')),
        ("upper", |c| c.is_ascii_uppercase()),
        ("xdigit", |c| c.is_ascii_hexdigit()),
    ];
    for (name, class) in classes {
        let mut cs = [0u8; CHARSETSIZE];
        for c in 0..=255u8 {
            if class(c) {
                setchar(&mut cs, c);
            }
        }
        let value = new_value(cx, shared, from_charset(&cs))?;
        table.set(name, value)?;
    }
    cx.push(table)
}

fn lp_ptree(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    let a = getpatt(cx, shared, 1)?;
    let finalize = cx.arg_kind(2).is_some_and(|_| arg_v(cx, 2).map(|v| v.truthy()).unwrap_or(false));
    let mut tree = a.pat.tree.clone();
    if finalize {
        let empty = Rc::new(Vec::new());
        let ktable = a.pat.ktable.as_ref().unwrap_or(&empty);
        finalfix(&mut tree, 0, None, ktable, &val2str).map_err(|f| fail(cx, f))?;
    }
    print::printktable(a.pat.ktable.as_deref().map_or(&[], |k| k.as_slice()));
    print::printtree(&tree, 0, 0);
    Ok(0)
}

fn lp_pcode(cx: &mut CallbackLua<'_>, shared: &Shared) -> LuaResult<usize> {
    if !matches!(cx.arg_kind(1), Some(LuaValueKind::Userdata)) || pattern_of(&cx.arg::<Value>(1)?).is_none() {
        return Err(cx.type_error(1, PATTERN_NAME));
    }
    let a = getpatt(cx, shared, 1)?;
    print::printktable(a.pat.ktable.as_deref().map_or(&[], |k| k.as_slice()));
    let code = compiled(cx, &a.pat)?;
    print::printpatt(&code);
    Ok(0)
}

// ---- installation -------------------------------------------------------------

/// Install `lpeg` as a global and in `package.loaded`.
pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let err = |e: LuaError| format!("lpeg: {e:?}");
    let lib = lua.create_table().map_err(err)?;
    let metatable = lua.create_table().map_err(err)?;
    let shared = Rc::new(Shared { metatable: metatable.clone(), max_stack: Cell::new(vm::MAXBACK) });

    macro_rules! reg {
        ($table:expr, $name:literal, $body:expr) => {{
            let shared = shared.clone();
            let f = lua
                .create_callback(move |cx| {
                    let body: fn(&mut CallbackLua<'_>, &Shared) -> LuaResult<usize> = $body;
                    body(cx, &shared)
                })
                .map_err(err)?;
            $table.set($name, f).map_err(err)?;
        }};
    }

    reg!(lib, "match", lp_match);
    reg!(lib, "P", lp_p);
    reg!(lib, "S", lp_set);
    reg!(lib, "R", lp_range);
    reg!(lib, "B", lp_behind);
    reg!(lib, "V", lp_v);
    reg!(lib, "C", |cx, sh| simple_capture(cx, sh, CSIMPLE));
    reg!(lib, "Cs", |cx, sh| simple_capture(cx, sh, CSUBST));
    reg!(lib, "Ct", |cx, sh| simple_capture(cx, sh, CTABLE));
    reg!(lib, "Cc", lp_constcapture);
    reg!(lib, "Cg", lp_groupcapture);
    reg!(lib, "Cf", lp_foldcapture);
    reg!(lib, "Cb", lp_backref);
    reg!(lib, "Cp", |cx, sh| push_new(cx, sh, position_capture()));
    reg!(lib, "Carg", lp_argcapture);
    reg!(lib, "Cmt", lp_matchtime);
    reg!(lib, "setmaxstack", lp_setmax);
    reg!(lib, "locale", lp_locale);
    reg!(lib, "ptree", lp_ptree);
    reg!(lib, "pcode", lp_pcode);
    let type_fn = lua.create_callback(lp_type).map_err(err)?;
    lib.set("type", type_fn).map_err(err)?;
    let version: LuaFunction = lua
        .create_callback(|cx| cx.push(VERSION))
        .map_err(err)?;
    lib.set("version", version).map_err(err)?;

    reg!(metatable, "__mul", lp_seq);
    reg!(metatable, "__add", lp_choice);
    reg!(metatable, "__pow", lp_star);
    reg!(metatable, "__len", lp_and);
    reg!(metatable, "__div", lp_divcapture);
    reg!(metatable, "__unm", lp_not);
    reg!(metatable, "__sub", lp_diff);
    metatable.set("__index", &lib).map_err(err)?;
    metatable.set("__name", PATTERN_NAME).map_err(err)?;

    lua.set_global("lpeg", &lib).map_err(err)?;
    let package: LuaTable = lua.get_global("package").map_err(err)?.ok_or("lpeg: no package table")?;
    let loaded: LuaTable = package.get("loaded").map_err(err)?;
    loaded.set("lpeg", &lib).map_err(err)?;
    Ok(())
}
