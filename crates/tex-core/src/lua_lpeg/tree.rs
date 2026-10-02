//! Pattern trees and their static analyses (lptree.h, the analysis half of
//! lpcode.c). A tree is a flat array of nodes: the first child of node `i`
//! is `i + 1`, the second child is `i + u`.

use super::value::V;

pub const TCHAR: u8 = 0;
pub const TSET: u8 = 1;
pub const TANY: u8 = 2;
pub const TTRUE: u8 = 3;
pub const TFALSE: u8 = 4;
pub const TREP: u8 = 5;
pub const TSEQ: u8 = 6;
pub const TCHOICE: u8 = 7;
pub const TNOT: u8 = 8;
pub const TAND: u8 = 9;
pub const TCALL: u8 = 10;
pub const TOPENCALL: u8 = 11;
pub const TRULE: u8 = 12;
pub const TGRAMMAR: u8 = 13;
pub const TBEHIND: u8 = 14;
pub const TCAPTURE: u8 = 15;
pub const TRUNTIME: u8 = 16;

/// Number of children of each tag (`numsiblings`); `TCall` has none because
/// its rule is not part of its subtree.
const NUMSIBLINGS: [u8; 17] = [0, 0, 0, 0, 0, 1, 2, 2, 1, 1, 0, 0, 2, 1, 1, 1, 1];

pub const CCLOSE: u8 = 0;
pub const CPOSITION: u8 = 1;
pub const CCONST: u8 = 2;
pub const CBACKREF: u8 = 3;
pub const CARG: u8 = 4;
pub const CSIMPLE: u8 = 5;
pub const CTABLE: u8 = 6;
pub const CFUNCTION: u8 = 7;
pub const CQUERY: u8 = 8;
pub const CSTRING: u8 = 9;
pub const CNUM: u8 = 10;
pub const CSUBST: u8 = 11;
pub const CFOLD: u8 = 12;
pub const CRUNTIME: u8 = 13;
pub const CGROUP: u8 = 14;

pub const CHARSETSIZE: usize = 32;
pub type Charset = [u8; CHARSETSIZE];

pub const FULLSET: Charset = [0xFF; CHARSETSIZE];

/// Tree nodes occupied by a charset (`bytes2slots(CHARSETSIZE)`).
pub const SET_SLOTS: usize = 4;

#[derive(Clone, Copy, Default, Debug)]
pub struct TTree {
    pub tag: u8,
    /// Capture kind (`TCapture`), or rule number (`TRule`).
    pub cap: u16,
    /// Index into the ktable (0: none), or the argument number of `Carg`.
    pub key: u16,
    /// Offset of the second child (`ps`), or a count / character (`n`).
    pub u: i32,
}

impl TTree {
    pub fn new(tag: u8) -> TTree {
        TTree { tag, ..TTree::default() }
    }

    fn to_bytes(self) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[0] = self.tag;
        b[1] = self.cap as u8;
        b[2..4].copy_from_slice(&self.key.to_le_bytes());
        b[4..8].copy_from_slice(&self.u.to_le_bytes());
        b
    }

    fn from_bytes(b: &[u8]) -> TTree {
        TTree {
            tag: b[0],
            cap: b[1] as u16,
            key: u16::from_le_bytes([b[2], b[3]]),
            u: i32::from_le_bytes([b[4], b[5], b[6], b[7]]),
        }
    }
}

/// The charset stored behind the `TSet` node `i`.
pub fn read_set(t: &[TTree], i: usize) -> Charset {
    let mut cs = [0u8; CHARSETSIZE];
    for k in 0..SET_SLOTS {
        cs[k * 8..k * 8 + 8].copy_from_slice(&t[i + 1 + k].to_bytes());
    }
    cs
}

pub fn write_set(t: &mut [TTree], i: usize, cs: &Charset) {
    for k in 0..SET_SLOTS {
        t[i + 1 + k] = TTree::from_bytes(&cs[k * 8..k * 8 + 8]);
    }
}

#[inline]
pub fn setchar(cs: &mut Charset, c: u8) {
    cs[(c >> 3) as usize] |= 1 << (c & 7);
}

#[inline]
pub fn testchar(cs: &[u8], c: u8) -> bool {
    cs[(c >> 3) as usize] & (1 << (c & 7)) != 0
}

pub fn cs_complement(cs: &mut Charset) {
    for b in cs.iter_mut() {
        *b = !*b;
    }
}

#[inline]
pub fn sib2(t: &[TTree], i: usize) -> usize {
    (i as i64 + t[i].u as i64) as usize
}

/// Convert a `TChar`/`TSet`/`TAny` node into a charset.
pub fn tocharset(t: &[TTree], i: usize, cs: &mut Charset) -> bool {
    match t[i].tag {
        TSET => {
            *cs = read_set(t, i);
            true
        }
        TCHAR => {
            *cs = [0; CHARSETSIZE];
            setchar(cs, t[i].u as u8);
            true
        }
        TANY => {
            *cs = [0xFF; CHARSETSIZE];
            true
        }
        _ => false,
    }
}

/// Run `f` on the rule called by the `TCall` node `i`, treating the call as
/// already visited meanwhile (a recursive call yields `def`).
fn callrecursive<R>(t: &mut [TTree], i: usize, f: fn(&mut [TTree], usize) -> R, def: R) -> R {
    let key = t[i].key;
    if key == 0 {
        def
    } else {
        t[i].key = 0;
        let rule = sib2(t, i);
        let result = f(t, rule);
        t[i].key = key;
        result
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Pred {
    Nullable,
    NoFail,
}

fn checkaux(t: &mut [TTree], mut i: usize, pred: Pred) -> bool {
    loop {
        match t[i].tag {
            TCHAR | TSET | TANY | TFALSE | TOPENCALL => return false,
            TREP | TTRUE => return true,
            TNOT | TBEHIND => return pred != Pred::NoFail,
            TAND => {
                if pred == Pred::Nullable {
                    return true;
                }
                i += 1;
            }
            TRUNTIME => {
                if pred == Pred::NoFail {
                    return false;
                }
                i += 1;
            }
            TSEQ => {
                if !checkaux(t, i + 1, pred) {
                    return false;
                }
                i = sib2(t, i);
            }
            TCHOICE => {
                let second = sib2(t, i);
                if checkaux(t, second, pred) {
                    return true;
                }
                i += 1;
            }
            TCAPTURE | TGRAMMAR | TRULE => i += 1,
            TCALL => i = sib2(t, i),
            tag => unreachable!("checkaux: tag {tag}"),
        }
    }
}

/// True if the pattern can match the empty string.
pub fn nullable(t: &mut [TTree], i: usize) -> bool {
    checkaux(t, i, Pred::Nullable)
}

/// True if the pattern can never fail.
pub fn nofail(t: &mut [TTree], i: usize) -> bool {
    checkaux(t, i, Pred::NoFail)
}

/// Length of every match of the pattern, or -1 if it varies.
pub fn fixedlen(t: &mut [TTree], mut i: usize) -> i32 {
    let mut len = 0;
    loop {
        match t[i].tag {
            TCHAR | TSET | TANY => return len + 1,
            TFALSE | TTRUE | TNOT | TAND | TBEHIND => return len,
            TREP | TRUNTIME | TOPENCALL => return -1,
            TCAPTURE | TRULE | TGRAMMAR => i += 1,
            TCALL => {
                let n1 = callrecursive(t, i, fixedlen, -1);
                return if n1 < 0 { -1 } else { len + n1 };
            }
            TSEQ => {
                let n1 = fixedlen(t, i + 1);
                if n1 < 0 {
                    return -1;
                }
                len += n1;
                i = sib2(t, i);
            }
            TCHOICE => {
                let n1 = fixedlen(t, i + 1);
                let second = sib2(t, i);
                let n2 = fixedlen(t, second);
                return if n1 != n2 || n1 < 0 { -1 } else { len + n1 };
            }
            tag => unreachable!("fixedlen: tag {tag}"),
        }
    }
}

pub fn hascaptures(t: &mut [TTree], mut i: usize) -> bool {
    loop {
        match t[i].tag {
            TCAPTURE | TRUNTIME => return true,
            TCALL => return callrecursive(t, i, hascaptures, false),
            TRULE => i += 1,
            tag => match NUMSIBLINGS[tag as usize] {
                1 => i += 1,
                2 => {
                    if hascaptures(t, i + 1) {
                        return true;
                    }
                    i = sib2(t, i);
                }
                _ => return false,
            },
        }
    }
}

/// The first set of a pattern: a conservative approximation of the characters
/// that can start a match. Returns 0 when the set can be used for a test
/// instruction that avoids the pattern altogether; non-zero when the pattern
/// can accept the empty string (the set then also holds `follow`), with bit 1
/// set when a match-time capture is involved.
pub fn getfirst(t: &mut [TTree], mut i: usize, follow: &Charset, firstset: &mut Charset) -> i32 {
    loop {
        match t[i].tag {
            TCHAR | TSET | TANY => {
                tocharset(t, i, firstset);
                return 0;
            }
            TTRUE => {
                *firstset = *follow;
                return 1;
            }
            TFALSE => {
                *firstset = [0; CHARSETSIZE];
                return 0;
            }
            TCHOICE => {
                let mut csaux = [0u8; CHARSETSIZE];
                let e1 = getfirst(t, i + 1, follow, firstset);
                let second = sib2(t, i);
                let e2 = getfirst(t, second, follow, &mut csaux);
                for k in 0..CHARSETSIZE {
                    firstset[k] |= csaux[k];
                }
                return e1 | e2;
            }
            TSEQ => {
                if !nullable(t, i + 1) {
                    i += 1;
                } else {
                    let mut csaux = [0u8; CHARSETSIZE];
                    let second = sib2(t, i);
                    let e2 = getfirst(t, second, follow, &mut csaux);
                    let e1 = getfirst(t, i + 1, &csaux, firstset);
                    return if e1 == 0 {
                        0
                    } else if (e1 | e2) & 2 != 0 {
                        2
                    } else {
                        e2
                    };
                }
            }
            TREP => {
                getfirst(t, i + 1, follow, firstset);
                for k in 0..CHARSETSIZE {
                    firstset[k] |= follow[k];
                }
                return 1;
            }
            TCAPTURE | TGRAMMAR | TRULE => i += 1,
            TRUNTIME => {
                let e = getfirst(t, i + 1, &FULLSET, firstset);
                return if e != 0 { 2 } else { 0 };
            }
            TCALL => i = sib2(t, i),
            TAND => {
                let e = getfirst(t, i + 1, follow, firstset);
                for k in 0..CHARSETSIZE {
                    firstset[k] &= follow[k];
                }
                return e;
            }
            TNOT => {
                if tocharset(t, i + 1, firstset) {
                    cs_complement(firstset);
                    return 1;
                }
                let mut tmp = [0u8; CHARSETSIZE];
                let e = getfirst(t, i + 1, follow, &mut tmp);
                *firstset = *follow;
                return 1 | (e & 2);
            }
            TBEHIND => {
                *firstset = *follow;
                return 1;
            }
            tag => unreachable!("getfirst: tag {tag}"),
        }
    }
}

/// True if the pattern can fail only depending on the next character.
pub fn headfail(t: &mut [TTree], mut i: usize) -> bool {
    loop {
        match t[i].tag {
            TCHAR | TSET | TANY | TFALSE => return true,
            TTRUE | TREP | TRUNTIME | TNOT | TBEHIND => return false,
            TCAPTURE | TGRAMMAR | TRULE | TAND => i += 1,
            TCALL => i = sib2(t, i),
            TSEQ => {
                let second = sib2(t, i);
                if !nofail(t, second) {
                    return false;
                }
                i += 1;
            }
            TCHOICE => {
                if !headfail(t, i + 1) {
                    return false;
                }
                i = sib2(t, i);
            }
            tag => unreachable!("headfail: tag {tag}"),
        }
    }
}

/// Whether code generation for the tree can benefit from a follow set.
pub fn needfollow(t: &[TTree], mut i: usize) -> bool {
    loop {
        match t[i].tag {
            TCHAR | TSET | TANY | TFALSE | TTRUE | TAND | TNOT | TRUNTIME | TGRAMMAR | TCALL
            | TBEHIND => return false,
            TCHOICE | TREP => return true,
            TCAPTURE => i += 1,
            TSEQ => i = sib2(t, i),
            tag => unreachable!("needfollow: tag {tag}"),
        }
    }
}

/// Add `n` to the ktable keys of the tree rooted at `i`.
pub fn correctkeys(t: &mut [TTree], mut i: usize, n: usize) {
    if n == 0 {
        return;
    }
    loop {
        let node = &mut t[i];
        match node.tag {
            TOPENCALL | TCALL | TRUNTIME | TRULE => {
                if node.key > 0 {
                    node.key += n as u16;
                }
            }
            TCAPTURE => {
                if node.key > 0 && node.cap != CARG as u16 && node.cap != CNUM as u16 {
                    node.key += n as u16;
                }
            }
            _ => {}
        }
        match NUMSIBLINGS[t[i].tag as usize] {
            1 => i += 1,
            2 => {
                correctkeys(t, i + 1, n);
                i = sib2(t, i);
            }
            _ => return,
        }
    }
}

pub type KTable = std::rc::Rc<Vec<V>>;
