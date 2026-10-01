//! Pattern construction (lptree.c): patterns are trees plus the table of Lua
//! values (the "ktable") their captures and rules refer to by index.

use std::cell::RefCell;
use std::rc::Rc;

use super::tree::*;
use super::value::V;

/// A pattern: its tree, its ktable and (once matched) its compiled code.
pub struct Pat {
    pub tree: Vec<TTree>,
    pub ktable: Option<KTable>,
    pub code: RefCell<Option<Rc<Vec<u32>>>>,
}

/// A failed construction: `Arg(n, msg)` is `luaL_argerror`, `Error(msg)` is
/// `luaL_error`.
#[derive(Debug)]
pub enum Fail {
    Arg(usize, String),
    Error(String),
}

pub type BResult<T> = Result<T, Fail>;

/// The result of a binary or unary operation: one of the operands may be the
/// result as is.
pub enum Built {
    Operand(usize),
    New(Pat),
}

pub const MAXRULES: usize = 1000;
const SHRT_MAX: i64 = 32767;
const USHRT_MAX: usize = 65535;

impl Pat {
    pub fn new(tree: Vec<TTree>, ktable: Option<KTable>) -> Pat {
        Pat { tree, ktable, code: RefCell::new(None) }
    }

    pub fn ktable_len(&self) -> usize {
        self.ktable.as_ref().map_or(0, |k| k.len())
    }

    fn entries(&self) -> &[V] {
        self.ktable.as_ref().map_or(&[], |k| k.as_slice())
    }
}

pub fn leaf(tag: u8) -> Pat {
    Pat::new(vec![TTree::new(tag)], None)
}

pub fn from_string(s: &[u8]) -> Pat {
    if s.is_empty() {
        return leaf(TTRUE);
    }
    let mut tree = vec![TTree::default(); 2 * s.len() - 1];
    let mut nd = 0;
    for (k, &c) in s.iter().enumerate() {
        if k + 1 < s.len() {
            tree[nd] = TTree { tag: TSEQ, u: 2, ..TTree::default() };
            tree[nd + 1] = TTree { tag: TCHAR, u: c as i32, ..TTree::default() };
            nd += 2;
        } else {
            tree[nd] = TTree { tag: TCHAR, u: c as i32, ..TTree::default() };
        }
    }
    Pat::new(tree, None)
}

/// `P(n)`: `n` characters, or not `-n` characters.
pub fn from_int(n: i32) -> Pat {
    if n == 0 {
        return leaf(TTRUE);
    }
    let (count, mut tree, mut nd) = if n > 0 {
        (n as usize, vec![TTree::default(); 2 * n as usize - 1], 0)
    } else {
        let count = (n as i64).unsigned_abs() as usize;
        let mut tree = vec![TTree::default(); 2 * count];
        tree[0] = TTree::new(TNOT);
        (count, tree, 1)
    };
    for _ in 1..count {
        tree[nd] = TTree { tag: TSEQ, u: 2, ..TTree::default() };
        tree[nd + 1] = TTree::new(TANY);
        nd += 2;
    }
    tree[nd] = TTree::new(TANY);
    Pat::new(tree, None)
}

pub fn from_bool(b: bool) -> Pat {
    leaf(if b { TTRUE } else { TFALSE })
}

/// `P(function)`: a match-time capture over the empty string.
pub fn from_function(f: V) -> Pat {
    let tree = vec![TTree { tag: TRUNTIME, key: 1, ..TTree::default() }, TTree::new(TTRUE)];
    Pat::new(tree, Some(Rc::new(vec![f])))
}

pub fn from_charset(cs: &Charset) -> Pat {
    let mut tree = vec![TTree::default(); SET_SLOTS + 1];
    tree[0] = TTree::new(TSET);
    write_set(&mut tree, 0, cs);
    Pat::new(tree, None)
}

pub fn set(s: &[u8]) -> Pat {
    let mut cs = [0u8; CHARSETSIZE];
    for &c in s {
        setchar(&mut cs, c);
    }
    from_charset(&cs)
}

/// `R(...)`; `ranges[k]` is the argument number `k + 1`.
pub fn range(ranges: &[Vec<u8>]) -> BResult<Pat> {
    let mut cs = [0u8; CHARSETSIZE];
    for (k, r) in ranges.iter().enumerate() {
        if r.len() != 2 {
            return Err(Fail::Arg(k + 1, "range must have two characters".to_string()));
        }
        for c in r[0]..=r[1] {
            setchar(&mut cs, c);
        }
    }
    Ok(from_charset(&cs))
}

/// The ktable of a new pattern made of `p`'s values plus `extra`, and the key
/// of `extra`.
fn extend_ktable(p: Option<&Pat>, extra: V) -> (KTable, u16) {
    let mut entries: Vec<V> = p.map_or_else(Vec::new, |p| p.entries().to_vec());
    entries.push(extra);
    let key = entries.len();
    (Rc::new(entries), key as u16)
}

pub fn open_call(name: V) -> BResult<Pat> {
    if name.is_nil() {
        return Err(Fail::Arg(1, "non-nil value expected".to_string()));
    }
    let (kt, key) = extend_ktable(None, name);
    Ok(Pat::new(vec![TTree { tag: TOPENCALL, key, ..TTree::default() }], Some(kt)))
}

fn root1(p: &Pat, node: TTree) -> Pat {
    let mut tree = Vec::with_capacity(1 + p.tree.len());
    tree.push(node);
    tree.extend_from_slice(&p.tree);
    Pat::new(tree, p.ktable.clone())
}

/// The ktable of two joined trees; `at` is where the second tree starts in
/// `tree`, its keys are corrected when its values come after the first's.
fn join_ktables(k1: &Option<KTable>, k2: &Option<KTable>, tree: &mut [TTree], at: usize) -> BResult<Option<KTable>> {
    let n1 = k1.as_ref().map_or(0, |k| k.len());
    let n2 = k2.as_ref().map_or(0, |k| k.len());
    if n1 == 0 && n2 == 0 {
        Ok(None)
    } else if n2 == 0 || matches!((k1, k2), (Some(a), Some(b)) if Rc::ptr_eq(a, b)) {
        Ok(k1.clone())
    } else if n1 == 0 {
        Ok(k2.clone())
    } else {
        if n1 + n2 > USHRT_MAX {
            return Err(Fail::Error("too many Lua values in pattern".to_string()));
        }
        let mut entries = Vec::with_capacity(n1 + n2);
        entries.extend_from_slice(k1.as_ref().expect("first ktable"));
        entries.extend_from_slice(k2.as_ref().expect("second ktable"));
        correctkeys(tree, at, n1);
        Ok(Some(Rc::new(entries)))
    }
}

fn root2(tag: u8, a: &Pat, b: &Pat) -> BResult<Pat> {
    let n1 = a.tree.len();
    let mut tree = Vec::with_capacity(1 + n1 + b.tree.len());
    tree.push(TTree { tag, u: 1 + n1 as i32, ..TTree::default() });
    tree.extend_from_slice(&a.tree);
    tree.extend_from_slice(&b.tree);
    let ktable = join_ktables(&a.ktable, &b.ktable, &mut tree, 1 + n1)?;
    Ok(Pat::new(tree, ktable))
}

/// `a * b`.
pub fn seq(a: &Pat, b: &Pat) -> BResult<Built> {
    if a.tree[0].tag == TFALSE || b.tree[0].tag == TTRUE {
        Ok(Built::Operand(0))
    } else if a.tree[0].tag == TTRUE {
        Ok(Built::Operand(1))
    } else {
        root2(TSEQ, a, b).map(Built::New)
    }
}

/// `a + b`.
pub fn choice(a: &Pat, b: &Pat) -> BResult<Built> {
    let (mut s1, mut s2) = ([0u8; CHARSETSIZE], [0u8; CHARSETSIZE]);
    let mut ta = a.tree.clone();
    let tb = b.tree.clone();
    if tocharset(&ta, 0, &mut s1) && tocharset(&tb, 0, &mut s2) {
        let mut cs = [0u8; CHARSETSIZE];
        for k in 0..CHARSETSIZE {
            cs[k] = s1[k] | s2[k];
        }
        Ok(Built::New(from_charset(&cs)))
    } else if nofail(&mut ta, 0) || tb[0].tag == TFALSE {
        Ok(Built::Operand(0))
    } else if ta[0].tag == TFALSE {
        Ok(Built::Operand(1))
    } else {
        root2(TCHOICE, a, b).map(Built::New)
    }
}

/// `a - b`.
pub fn diff(a: &Pat, b: &Pat) -> BResult<Pat> {
    let (mut s1, mut s2) = ([0u8; CHARSETSIZE], [0u8; CHARSETSIZE]);
    if tocharset(&a.tree, 0, &mut s1) && tocharset(&b.tree, 0, &mut s2) {
        let mut cs = [0u8; CHARSETSIZE];
        for k in 0..CHARSETSIZE {
            cs[k] = s1[k] & !s2[k];
        }
        return Ok(from_charset(&cs));
    }
    let (n1, n2) = (a.tree.len(), b.tree.len());
    let mut tree = Vec::with_capacity(2 + n1 + n2);
    tree.push(TTree { tag: TSEQ, u: 2 + n2 as i32, ..TTree::default() });
    tree.push(TTree::new(TNOT));
    tree.extend_from_slice(&b.tree);
    tree.extend_from_slice(&a.tree);
    let ktable = join_ktables(&a.ktable, &b.ktable, &mut tree, 1)?;
    Ok(Pat::new(tree, ktable))
}

/// `p ^ n`.
pub fn star(p: &Pat, n: i32) -> BResult<Pat> {
    let size1 = p.tree.len();
    let mut tree1 = p.tree.clone();
    let tree;
    if n >= 0 {
        if nullable(&mut tree1, 0) {
            return Err(Fail::Error("loop body may accept empty string".to_string()));
        }
        let mut t = vec![TTree::default(); (n as usize + 1) * (size1 + 1)];
        let mut nd = 0;
        for _ in 0..n {
            t[nd] = TTree { tag: TSEQ, u: size1 as i32 + 1, ..TTree::default() };
            t[nd + 1..nd + 1 + size1].copy_from_slice(&p.tree);
            nd += size1 + 1;
        }
        t[nd] = TTree::new(TREP);
        t[nd + 1..nd + 1 + size1].copy_from_slice(&p.tree);
        tree = t;
    } else {
        let n = (n as i64).unsigned_abs() as usize;
        let mut t = vec![TTree::default(); n * (size1 + 3) - 1];
        let mut nd = 0;
        for k in (2..=n).rev() {
            t[nd] = TTree { tag: TCHOICE, u: (k * (size1 + 3) - 2) as i32, ..TTree::default() };
            let second = nd + t[nd].u as usize;
            t[second] = TTree::new(TTRUE);
            nd += 1;
            t[nd] = TTree { tag: TSEQ, u: size1 as i32 + 1, ..TTree::default() };
            t[nd + 1..nd + 1 + size1].copy_from_slice(&p.tree);
            nd += size1 + 1;
        }
        t[nd] = TTree { tag: TCHOICE, u: size1 as i32 + 1, ..TTree::default() };
        t[nd + 1 + size1] = TTree::new(TTRUE);
        t[nd + 1..nd + 1 + size1].copy_from_slice(&p.tree);
        tree = t;
    }
    Ok(Pat::new(tree, p.ktable.clone()))
}

/// `-p`.
pub fn not(p: &Pat) -> Pat {
    root1(p, TTree::new(TNOT))
}

/// `#p`.
pub fn and(p: &Pat) -> Pat {
    root1(p, TTree::new(TAND))
}

/// `B(p)`.
pub fn behind(p: &Pat) -> BResult<Pat> {
    let mut t = p.tree.clone();
    let n = fixedlen(&mut t, 0);
    if n < 0 {
        return Err(Fail::Arg(1, "pattern may not have fixed length".to_string()));
    }
    if hascaptures(&mut t, 0) {
        return Err(Fail::Arg(1, "pattern have captures".to_string()));
    }
    if n > super::code::MAXBEHIND {
        return Err(Fail::Arg(1, "pattern too long to look behind".to_string()));
    }
    Ok(root1(p, TTree { tag: TBEHIND, u: n, ..TTree::default() }))
}

/// A capture of `p`, with an optional Lua value in the ktable.
pub fn capture(p: &Pat, cap: u8, label: Option<V>) -> Pat {
    let mut node = TTree { tag: TCAPTURE, cap: cap as u16, ..TTree::default() };
    match label {
        None => root1(p, node),
        Some(v) => {
            let (kt, key) = extend_ktable(Some(p), v);
            node.key = key;
            let mut r = root1(p, node);
            r.ktable = Some(kt);
            r
        }
    }
}

/// `p / n`.
pub fn num_capture(p: &Pat, n: i64) -> BResult<Pat> {
    if !(0..=SHRT_MAX).contains(&n) {
        return Err(Fail::Arg(1, "invalid number".to_string()));
    }
    Ok(root1(p, TTree { tag: TCAPTURE, cap: CNUM as u16, key: n as u16, ..TTree::default() }))
}

pub fn match_time(p: &Pat, f: V) -> Pat {
    let (kt, key) = extend_ktable(Some(p), f);
    let mut r = root1(p, TTree { tag: TRUNTIME, key, ..TTree::default() });
    r.ktable = Some(kt);
    r
}

fn empty_cap(cap: u8, key: u16, ktable: Option<KTable>) -> Pat {
    let tree = vec![TTree { tag: TCAPTURE, cap: cap as u16, key, ..TTree::default() }, TTree::new(TTRUE)];
    Pat::new(tree, ktable)
}

pub fn position_capture() -> Pat {
    empty_cap(CPOSITION, 0, None)
}

pub fn arg_capture(n: i64) -> BResult<Pat> {
    if !(0 < n && n <= SHRT_MAX) {
        return Err(Fail::Arg(1, "invalid argument index".to_string()));
    }
    Ok(empty_cap(CARG, n as u16, None))
}

pub fn backref(name: V) -> Pat {
    let (kt, key) = extend_ktable(None, name);
    empty_cap(CBACKREF, key, Some(kt))
}

/// `Cc(...)`.
pub fn const_capture(values: &[V]) -> Pat {
    match values.len() {
        0 => leaf(TTRUE),
        1 => {
            if values[0].is_nil() {
                // a nil constant has key 0 and an (empty) ktable
                empty_cap(CCONST, 0, None)
            } else {
                let (kt, key) = extend_ktable(None, values[0].clone());
                empty_cap(CCONST, key, Some(kt))
            }
        }
        n => {
            let mut tree = vec![TTree::default(); 1 + 3 * (n - 1) + 2];
            let mut entries: Vec<V> = Vec::new();
            tree[0] = TTree { tag: TCAPTURE, cap: CGROUP as u16, ..TTree::default() };
            let mut nd = 1;
            let key_of = |v: &V, entries: &mut Vec<V>| -> u16 {
                if v.is_nil() {
                    0
                } else {
                    entries.push(v.clone());
                    entries.len() as u16
                }
            };
            for v in &values[..n - 1] {
                tree[nd] = TTree { tag: TSEQ, u: 3, ..TTree::default() };
                tree[nd + 1] = TTree { tag: TCAPTURE, cap: CCONST as u16, key: key_of(v, &mut entries), ..TTree::default() };
                tree[nd + 2] = TTree::new(TTRUE);
                nd += 3;
            }
            tree[nd] = TTree { tag: TCAPTURE, cap: CCONST as u16, key: key_of(&values[n - 1], &mut entries), ..TTree::default() };
            tree[nd + 1] = TTree::new(TTRUE);
            Pat::new(tree, Some(Rc::new(entries)))
        }
    }
}

/// The rules of a grammar: `(name, pattern)` with the initial rule first.
pub struct Rule {
    pub name: V,
    pub pat: Rc<Pat>,
}

/// Build a grammar from its rules (`buildgrammar`) and resolve its calls
/// (`finalfix`); `frule` is the number of rules. `names` gives, for a rule
/// name, its position in the tree; the rules are laid out in order.
pub fn grammar(rules: &[Rule], name_error: &dyn Fn(&V) -> String) -> BResult<Pat> {
    let n = rules.len();
    // sizes and positions
    let mut positions: Vec<usize> = Vec::with_capacity(n);
    let mut size = 2; // TGrammar + TTrue
    for r in rules {
        positions.push(size - 1);
        size += 1 + r.pat.tree.len();
    }
    // first rule at position 1: TGrammar is at 0
    let mut tree = vec![TTree::default(); size];
    tree[0] = TTree { tag: TGRAMMAR, u: n as i32, ..TTree::default() };
    let mut entries: Vec<V> = Vec::new();
    let mut nd = 1;
    for (i, r) in rules.iter().enumerate() {
        let rulesize = r.pat.tree.len();
        tree[nd] = TTree { tag: TRULE, key: 0, cap: i as u16, u: rulesize as i32 + 1 };
        tree[nd + 1..nd + 1 + rulesize].copy_from_slice(&r.pat.tree);
        // mergektable
        let n1 = entries.len();
        let k = r.pat.entries();
        if n1 + k.len() > USHRT_MAX {
            return Err(Fail::Error("too many Lua values in pattern".to_string()));
        }
        entries.extend_from_slice(k);
        correctkeys(&mut tree, nd + 1, n1);
        nd += 1 + rulesize;
    }
    tree[nd] = TTree::new(TTRUE);
    finalfix(&mut tree, 1, Some((0, rules, &positions)), &entries, name_error)?;
    // the initial rule's name, when nothing calls it
    if tree[1].key == 0 {
        entries.push(rules[0].name.clone());
        tree[1].key = entries.len() as u16;
    }
    verifygrammar(&mut tree, rules, name_error)?;
    Ok(Pat::new(tree, Some(Rc::new(entries))))
}

/// Resolve open calls: inside a grammar they become calls to the rule of
/// that name; outside one they are an error.
pub fn finalfix(
    t: &mut [TTree],
    mut i: usize,
    g: Option<(usize, &[Rule], &[usize])>,
    ktable: &[V],
    name_error: &dyn Fn(&V) -> String,
) -> BResult<()> {
    loop {
        match t[i].tag {
            TGRAMMAR => return Ok(()),
            TOPENCALL => {
                let name = ktable[t[i].key as usize - 1].clone();
                match g {
                    Some((gi, rules, positions)) => {
                        let Some(rule) = rules.iter().position(|r| r.name.equals(&name)) else {
                            return Err(Fail::Error(format!("rule '{}' undefined in given grammar", name_error(&name))));
                        };
                        let pos = positions[rule];
                        t[i].tag = TCALL;
                        t[i].u = pos as i32 - (i - gi) as i32;
                        let target = pos;
                        t[target].key = t[i].key;
                    }
                    None => {
                        return Err(Fail::Error(format!("rule '{}' used outside a grammar", name_error(&name))));
                    }
                }
                return Ok(());
            }
            TSEQ | TCHOICE => {
                correctassociativity(t, i);
                finalfix(t, i + 1, g, ktable, name_error)?;
                i = sib2(t, i);
            }
            tag => match NUMSIBLINGS_PUB[tag as usize] {
                1 => i += 1,
                2 => {
                    finalfix(t, i + 1, g, ktable, name_error)?;
                    i = sib2(t, i);
                }
                _ => return Ok(()),
            },
        }
    }
}

/// Turn `(p1 op p2) op p3` into `p1 op (p2 op p3)` for sequences and choices.
fn correctassociativity(t: &mut [TTree], tree: usize) {
    let tag = t[tree].tag;
    while t[tree + 1].tag == tag {
        let t1 = tree + 1;
        let n1size = t[tree].u as usize - 1; // t1 == op t11 t12
        let n11size = t[t1].u as usize - 1;
        let n12size = n1size - n11size - 1;
        t.copy_within(t1 + 1..t1 + 1 + n11size, tree + 1); // move t11
        t[tree].u = n11size as i32 + 1;
        let new = tree + n11size + 1;
        t[new].tag = tag;
        t[new].u = n12size as i32 + 1;
    }
}

const NUMSIBLINGS_PUB: [u8; 17] = [0, 0, 0, 0, 0, 1, 2, 2, 1, 1, 0, 0, 2, 1, 1, 1, 1];

/// Left recursion and empty loops in a grammar (`verifygrammar`).
pub fn verifygrammar(t: &mut [TTree], rules: &[Rule], name_error: &dyn Fn(&V) -> String) -> BResult<()> {
    // check left-recursive rules
    let mut rule = 1;
    let mut index = 0;
    while t[rule].tag == TRULE {
        if t[rule].key != 0 {
            let mut passed: Vec<u16> = Vec::new();
            verifyrule(t, rule, &mut passed, false, rules, name_error)?;
        }
        rule = sib2(t, rule);
        index += 1;
    }
    debug_assert_eq!(t[rule].tag, TTRUE);
    // check infinite loops inside rules
    let mut rule = 1;
    let mut index2 = 0;
    while t[rule].tag == TRULE {
        if t[rule].key != 0 {
            checkloops(t, rule + 1, name_error, &rules[index2].name)?;
        }
        rule = sib2(t, rule);
        index2 += 1;
    }
    let _ = index;
    Ok(())
}

fn verifyrule(
    t: &mut [TTree],
    mut i: usize,
    passed: &mut Vec<u16>,
    mut nullable: bool,
    rules: &[Rule],
    name_error: &dyn Fn(&V) -> String,
) -> BResult<bool> {
    loop {
        match t[i].tag {
            TCHAR | TSET | TANY | TFALSE => return Ok(nullable),
            TTRUE | TBEHIND => return Ok(true),
            TNOT | TAND | TREP => {
                i += 1;
                nullable = true;
            }
            TCAPTURE | TRUNTIME => i += 1,
            TCALL => i = sib2(t, i),
            TSEQ => {
                if !verifyrule(t, i + 1, passed, false, rules, name_error)? {
                    return Ok(nullable);
                }
                i = sib2(t, i);
            }
            TCHOICE => {
                nullable = verifyrule(t, i + 1, passed, nullable, rules, name_error)?;
                i = sib2(t, i);
            }
            TRULE => {
                let rule = t[i].key;
                if passed.contains(&rule) {
                    let name = &rules[t[i].cap as usize].name;
                    return Err(Fail::Error(format!("rule '{}' may be left recursive", name_error(name))));
                }
                if passed.len() >= MAXRULES {
                    return Ok(nullable);
                }
                passed.push(rule);
                i += 1;
            }
            TGRAMMAR => return Ok(nullable),
            tag => unreachable!("verifyrule: tag {tag}"),
        }
    }
}

fn checkloops(t: &mut [TTree], mut i: usize, name_error: &dyn Fn(&V) -> String, rule_name: &V) -> BResult<()> {
    loop {
        if t[i].tag == TGRAMMAR {
            return Ok(());
        }
        if t[i].tag == TREP && nullable(t, i + 1) {
            return Err(Fail::Error(format!(
                "empty loop in rule '{}'",
                name_error(rule_name)
            )));
        }
        match NUMSIBLINGS_PUB[t[i].tag as usize] {
            1 => i += 1,
            2 => {
                checkloops(t, i + 1, name_error, rule_name)?;
                i = sib2(t, i);
            }
            _ => return Ok(()),
        }
    }
}

const MAXUNICODE: u32 = 0x10FFFF;

/// Split `[lo, hi]` into code point ranges whose UTF-8 encodings are
/// sequences of byte ranges.
fn utf8_sequences(lo: u32, hi: u32, out: &mut Vec<Vec<(u8, u8)>>) {
    // split at the boundaries of the encoded length
    for &max in &[0x7Fu32, 0x7FF, 0xFFFF] {
        if lo <= max && max < hi {
            utf8_sequences(lo, max, out);
            utf8_sequences(max + 1, hi, out);
            return;
        }
    }
    if hi < 0x80 {
        out.push(vec![(lo as u8, hi as u8)]);
        return;
    }
    // split so that continuation bytes span their full range
    for i in 1..4u32 {
        let m: u32 = (1 << (6 * i)) - 1;
        if lo & !m != hi & !m {
            if lo & m != 0 {
                utf8_sequences(lo, lo | m, out);
                utf8_sequences((lo | m) + 1, hi, out);
                return;
            }
            if hi & m != m {
                utf8_sequences(lo, (hi & !m) - 1, out);
                utf8_sequences(hi & !m, hi, out);
                return;
            }
        }
    }
    let (a, b) = (encode_utf8(lo), encode_utf8(hi));
    out.push(a.iter().zip(b.iter()).map(|(&x, &y)| (x, y)).collect());
}

fn encode_utf8(cp: u32) -> Vec<u8> {
    if cp < 0x80 {
        vec![cp as u8]
    } else if cp < 0x800 {
        vec![0xC0 | (cp >> 6) as u8, 0x80 | (cp & 0x3F) as u8]
    } else if cp < 0x10000 {
        vec![0xE0 | (cp >> 12) as u8, 0x80 | ((cp >> 6) & 0x3F) as u8, 0x80 | (cp & 0x3F) as u8]
    } else {
        vec![
            0xF0 | (cp >> 18) as u8,
            0x80 | ((cp >> 12) & 0x3F) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]
    }
}

/// `utfR(from, to)` (LPeg 1.1): one UTF-8 encoded code point in the range.
pub fn utf_range(from: i64, to: i64) -> BResult<Pat> {
    if !(0..=MAXUNICODE as i64).contains(&from) {
        return Err(Fail::Arg(1, "invalid code point".to_string()));
    }
    if !(0..=MAXUNICODE as i64).contains(&to) {
        return Err(Fail::Arg(2, "invalid code point".to_string()));
    }
    if from > to {
        return Err(Fail::Arg(2, "empty range".to_string()));
    }
    let mut sequences = Vec::new();
    utf8_sequences(from as u32, to as u32, &mut sequences);
    let mut result: Option<Pat> = None;
    for seq_ranges in sequences {
        let mut piece: Option<Pat> = None;
        for (lo, hi) in seq_ranges {
            let mut cs = [0u8; CHARSETSIZE];
            for c in lo..=hi {
                setchar(&mut cs, c);
            }
            let next = from_charset(&cs);
            piece = Some(match piece {
                None => next,
                Some(p) => match seq(&p, &next)? {
                    Built::New(n) => n,
                    Built::Operand(0) => p,
                    Built::Operand(_) => next,
                },
            });
        }
        let piece = piece.expect("sequence");
        result = Some(match result {
            None => piece,
            Some(r) => match choice(&r, &piece)? {
                Built::New(n) => n,
                Built::Operand(0) => r,
                Built::Operand(_) => piece,
            },
        });
    }
    Ok(result.expect("range"))
}
