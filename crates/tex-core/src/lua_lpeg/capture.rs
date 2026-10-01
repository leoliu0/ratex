//! Capture evaluation (lpcap.c): the list of captures a match recorded is
//! turned into Lua values.

use tex_lua::LuaError;

use super::tree::*;
use super::value::V;

pub type LResult<T> = Result<T, LuaError>;

/// What capture evaluation needs from the running Lua state.
pub trait Env {
    /// Call `f` with `args`, returning all its results.
    fn call(&mut self, f: &V, args: &[V]) -> LResult<Vec<V>>;
    /// `lua_gettable` (honours `__index`).
    fn index(&mut self, table: &V, key: &V) -> LResult<V>;
    fn new_table(&mut self) -> LResult<V>;
    /// `t[key] = value` on a table created by `new_table`.
    fn table_set(&mut self, table: &V, key: &V, value: &V) -> LResult<()>;
    /// `luaL_error`.
    fn error(&mut self, message: String) -> LuaError;
}

/// One entry of the capture list: `siz` is 0 for an open capture, else the
/// length of a complete capture plus one (a close entry has `siz == 1`).
#[derive(Clone, Copy, Debug)]
pub struct Cap {
    pub s: usize,
    pub idx: u16,
    pub kind: u8,
    pub siz: u8,
}

impl Cap {
    #[inline]
    pub fn isclose(&self) -> bool {
        self.kind == CCLOSE
    }

    #[inline]
    pub fn isfull(&self) -> bool {
        self.siz != 0
    }

    /// Where a complete or close entry ends.
    #[inline]
    fn closeaddr(&self) -> usize {
        self.s + self.siz as usize - 1
    }
}

const MAXSTRCAPTURES: usize = 32;

/// First dynamic capture among `caps[from..to]`: its 1-based position in the
/// dynamic values, or 0 if there is none.
pub fn finddyncap(caps: &[Cap], from: usize, to: usize) -> usize {
    caps[from..to]
        .iter()
        .find(|c| c.kind == CRUNTIME)
        .map_or(0, |c| c.idx as usize)
}

/// The open capture matching the close entry at `c`.
pub fn findopen(caps: &[Cap], mut c: usize) -> usize {
    let mut n = 0;
    loop {
        c -= 1;
        if caps[c].isclose() {
            n += 1;
        } else if !caps[c].isfull() {
            if n == 0 {
                return c;
            }
            n -= 1;
        }
    }
}

enum StrAux {
    Str(usize, usize),
    Cap(usize),
}

pub struct CapState<'a> {
    pub caps: &'a [Cap],
    /// Index of the capture being evaluated.
    pub cap: usize,
    pub subject: &'a [u8],
    pub ktable: &'a [V],
    pub dynvals: &'a [V],
    /// Extra arguments of `match` (`Carg n` is `args[n - 1]`).
    pub args: &'a [V],
    pub env: &'a mut dyn Env,
    /// The value stack results are pushed to.
    pub stack: Vec<V>,
}

impl CapState<'_> {
    /// The ktable value of `idx` (nil for key 0).
    fn luaval(&self, idx: u16) -> V {
        if idx == 0 {
            V::Nil
        } else {
            self.ktable.get(idx as usize - 1).cloned().unwrap_or(V::Nil)
        }
    }

    /// Go to the next capture at the same level.
    fn nextcap(&mut self) {
        let mut c = self.cap;
        if !self.caps[c].isfull() {
            let mut n = 0;
            loop {
                c += 1;
                if self.caps[c].isclose() {
                    if n == 0 {
                        break;
                    }
                    n -= 1;
                } else if !self.caps[c].isfull() {
                    n += 1;
                }
            }
        }
        self.cap = c + 1;
    }

    /// Push the values of the captures nested in the current one, or the
    /// whole match when there are none (or `addextra`).
    pub fn pushnestedvalues(&mut self, addextra: bool) -> LResult<usize> {
        let co = self.caps[self.cap];
        self.cap += 1;
        if co.isfull() {
            self.stack.push(V::str(&self.subject[co.s..co.s + co.siz as usize - 1]));
            Ok(1)
        } else {
            let mut n = 0;
            while !self.caps[self.cap].isclose() {
                n += self.pushcapture()?;
            }
            if addextra || n == 0 {
                self.stack.push(V::str(&self.subject[co.s..self.caps[self.cap].s]));
                n += 1;
            }
            self.cap += 1;
            Ok(n)
        }
    }

    fn pushonenestedvalue(&mut self) -> LResult<()> {
        let n = self.pushnestedvalues(false)?;
        if n > 1 {
            let keep = self.stack.len() - n + 1;
            self.stack.truncate(keep);
        }
        Ok(())
    }

    pub fn pushcapture(&mut self) -> LResult<usize> {
        let c = self.caps[self.cap];
        match c.kind {
            CPOSITION => {
                self.stack.push(V::Int(c.s as i64 + 1));
                self.cap += 1;
                Ok(1)
            }
            CCONST => {
                let v = self.luaval(c.idx);
                self.stack.push(v);
                self.cap += 1;
                Ok(1)
            }
            CARG => {
                let arg = c.idx as usize;
                self.cap += 1;
                if arg > self.args.len() {
                    return Err(self.env.error(format!("reference to absent extra argument #{arg}")));
                }
                self.stack.push(self.args[arg - 1].clone());
                Ok(1)
            }
            CSIMPLE => {
                let k = self.pushnestedvalues(true)?;
                let len = self.stack.len();
                self.stack[len - k..].rotate_right(1);
                Ok(k)
            }
            CRUNTIME => {
                self.cap += 1;
                self.stack.push(self.dynvals[c.idx as usize - 1].clone());
                Ok(1)
            }
            CSTRING => {
                let mut b = Vec::new();
                self.stringcap(&mut b)?;
                self.stack.push(V::str(&b));
                Ok(1)
            }
            CSUBST => {
                let mut b = Vec::new();
                self.substcap(&mut b)?;
                self.stack.push(V::str(&b));
                Ok(1)
            }
            CGROUP => {
                if c.idx == 0 {
                    self.pushnestedvalues(false)
                } else {
                    self.nextcap();
                    Ok(0)
                }
            }
            CBACKREF => self.backrefcap(),
            CTABLE => self.tablecap(),
            CFUNCTION => self.functioncap(),
            CNUM => self.numcap(),
            CQUERY => self.querycap(),
            CFOLD => self.foldcap(),
            kind => unreachable!("pushcapture: kind {kind}"),
        }
    }

    fn tablecap(&mut self) -> LResult<usize> {
        let table = self.env.new_table()?;
        let mut n: i64 = 0;
        let co = self.caps[self.cap];
        self.cap += 1;
        if co.isfull() {
            self.stack.push(table);
            return Ok(1);
        }
        while !self.caps[self.cap].isclose() {
            let c = self.caps[self.cap];
            if c.kind == CGROUP && c.idx != 0 {
                let name = self.luaval(c.idx);
                self.pushonenestedvalue()?;
                let value = self.stack.pop().expect("nested value");
                self.env.table_set(&table, &name, &value)?;
            } else {
                let k = self.pushcapture()?;
                let first = self.stack.len() - k;
                for i in 0..k {
                    let value = self.stack[first + i].clone();
                    self.env.table_set(&table, &V::Int(n + 1 + i as i64), &value)?;
                }
                self.stack.truncate(first);
                n += k as i64;
            }
        }
        self.cap += 1;
        self.stack.push(table);
        Ok(1)
    }

    fn functioncap(&mut self) -> LResult<usize> {
        let f = self.luaval(self.caps[self.cap].idx);
        let base = self.stack.len();
        let n = self.pushnestedvalues(false)?;
        let args: Vec<V> = self.stack.drain(base..base + n).collect();
        let results = self.env.call(&f, &args)?;
        let count = results.len();
        self.stack.extend(results);
        Ok(count)
    }

    fn numcap(&mut self) -> LResult<usize> {
        let idx = self.caps[self.cap].idx as usize;
        if idx == 0 {
            self.nextcap();
            Ok(0)
        } else {
            let n = self.pushnestedvalues(false)?;
            if n < idx {
                Err(self.env.error(format!("no capture '{idx}'")))
            } else {
                let first = self.stack.len() - n;
                let selected = self.stack[first + idx - 1].clone();
                self.stack.truncate(first);
                self.stack.push(selected);
                Ok(1)
            }
        }
    }

    fn querycap(&mut self) -> LResult<usize> {
        let table = self.luaval(self.caps[self.cap].idx);
        self.pushonenestedvalue()?;
        let key = self.stack.pop().expect("nested value");
        let value = self.env.index(&table, &key)?;
        if value.is_nil() {
            Ok(0)
        } else {
            self.stack.push(value);
            Ok(1)
        }
    }

    fn foldcap(&mut self) -> LResult<usize> {
        let f = self.luaval(self.caps[self.cap].idx);
        let first = self.caps[self.cap].isfull();
        self.cap += 1;
        let mut n = 0;
        if first || self.caps[self.cap].isclose() || {
            n = self.pushcapture()?;
            n == 0
        } {
            return Err(self.env.error("no initial value for fold capture".to_string()));
        }
        // leave only one result for the accumulator
        let base = self.stack.len() - n;
        self.stack.truncate(base + 1);
        let mut acc = self.stack.pop().expect("accumulator");
        while !self.caps[self.cap].isclose() {
            let at = self.stack.len();
            let k = self.pushcapture()?;
            let mut args = Vec::with_capacity(k + 1);
            args.push(acc);
            args.extend(self.stack.drain(at..at + k));
            acc = self.env.call(&f, &args)?.into_iter().next().unwrap_or(V::Nil);
        }
        self.cap += 1;
        self.stack.push(acc);
        Ok(1)
    }

    fn findback(&mut self, mut cap: usize) -> LResult<usize> {
        let name = self.luaval(self.caps[cap].idx);
        while cap > 0 {
            cap -= 1;
            if self.caps[cap].isclose() {
                cap = findopen(self.caps, cap);
            } else if !self.caps[cap].isfull() {
                continue;
            }
            if self.caps[cap].kind == CGROUP {
                let group = self.luaval(self.caps[cap].idx);
                if name.equals(&group) {
                    return Ok(cap);
                }
            }
        }
        let shown = name.to_bytes().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        Err(self.env.error(format!("back reference '{shown}' not found")))
    }

    fn backrefcap(&mut self) -> LResult<usize> {
        let curr = self.cap;
        self.cap = self.findback(curr)?;
        let n = self.pushnestedvalues(false)?;
        self.cap = curr + 1;
        Ok(n)
    }

    /// Add the first value of the current capture to `b`, converted to a
    /// string. Returns the number of values the capture produced.
    fn addonestring(&mut self, b: &mut Vec<u8>, what: &str) -> LResult<usize> {
        match self.caps[self.cap].kind {
            CSTRING => {
                self.stringcap(b)?;
                Ok(1)
            }
            CSUBST => {
                self.substcap(b)?;
                Ok(1)
            }
            _ => {
                let n = self.pushcapture()?;
                if n > 0 {
                    let first = self.stack.len() - n;
                    self.stack.truncate(first + 1);
                    let value = self.stack.pop().expect("value");
                    match value.to_bytes() {
                        Some(bytes) => b.extend_from_slice(&bytes),
                        None => {
                            return Err(self.env.error(format!(
                                "invalid {what} value (a {})",
                                value.type_name()
                            )));
                        }
                    }
                }
                Ok(n)
            }
        }
    }

    fn getstrcaps(&mut self, cps: &mut Vec<StrAux>) {
        let k = cps.len();
        let co = self.caps[self.cap];
        cps.push(StrAux::Str(co.s, 0));
        self.cap += 1;
        if !co.isfull() {
            while !self.caps[self.cap].isclose() {
                if cps.len() >= MAXSTRCAPTURES {
                    self.nextcap();
                } else if self.caps[self.cap].kind == CSIMPLE {
                    self.getstrcaps(cps);
                } else {
                    cps.push(StrAux::Cap(self.cap));
                    self.nextcap();
                }
            }
            cps[k] = StrAux::Str(co.s, self.caps[self.cap].s);
            self.cap += 1;
        } else {
            cps[k] = StrAux::Str(co.s, self.caps[self.cap - 1].closeaddr());
        }
    }

    fn stringcap(&mut self, b: &mut Vec<u8>) -> LResult<()> {
        let fmt = self.luaval(self.caps[self.cap].idx);
        let fmt = fmt.to_bytes().map(|f| f.into_owned()).unwrap_or_default();
        let mut cps = Vec::new();
        self.getstrcaps(&mut cps);
        let n = cps.len() - 1;
        let mut i = 0;
        while i < fmt.len() {
            if fmt[i] != b'%' {
                b.push(fmt[i]);
            } else {
                i += 1;
                if i >= fmt.len() || !fmt[i].is_ascii_digit() {
                    // C reads the terminating NUL here and adds it
                    b.push(fmt.get(i).copied().unwrap_or(0));
                } else {
                    let l = (fmt[i] - b'0') as usize;
                    if l > n {
                        return Err(self.env.error(format!("invalid capture index ({l})")));
                    }
                    match cps[l] {
                        StrAux::Str(s, e) => b.extend_from_slice(&self.subject[s..e]),
                        StrAux::Cap(cp) => {
                            let curr = self.cap;
                            self.cap = cp;
                            if self.addonestring(b, "capture")? == 0 {
                                return Err(self.env.error(format!("no values in capture index {l}")));
                            }
                            self.cap = curr;
                        }
                    }
                }
            }
            i += 1;
        }
        Ok(())
    }

    fn substcap(&mut self, b: &mut Vec<u8>) -> LResult<()> {
        let co = self.caps[self.cap];
        let mut curr = co.s;
        if co.isfull() {
            b.extend_from_slice(&self.subject[curr..curr + co.siz as usize - 1]);
        } else {
            self.cap += 1;
            while !self.caps[self.cap].isclose() {
                let next = self.caps[self.cap].s;
                b.extend_from_slice(&self.subject[curr..next]);
                if self.addonestring(b, "replacement")? != 0 {
                    curr = self.caps[self.cap - 1].closeaddr();
                } else {
                    curr = next;
                }
            }
            b.extend_from_slice(&self.subject[curr..self.caps[self.cap].s]);
        }
        self.cap += 1;
        Ok(())
    }
}
