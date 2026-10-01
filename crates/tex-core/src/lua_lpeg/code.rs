//! The pattern compiler (lpcode.c): trees to the LPeg virtual machine's
//! instruction array, with its first-set tests, charset spans, disjoint
//! choices and jump peephole. Instructions are 32-bit slots exactly as in
//! LPeg: opcode, aux and key packed in one slot, offsets and charsets in the
//! slots that follow.

use super::tree::*;

pub const IANY: u8 = 0;
pub const ICHAR: u8 = 1;
pub const ISET: u8 = 2;
pub const ITESTANY: u8 = 3;
pub const ITESTCHAR: u8 = 4;
pub const ITESTSET: u8 = 5;
pub const ISPAN: u8 = 6;
pub const IBEHIND: u8 = 7;
pub const IRET: u8 = 8;
pub const IEND: u8 = 9;
pub const ICHOICE: u8 = 10;
pub const IJMP: u8 = 11;
pub const ICALL: u8 = 12;
pub const IOPENCALL: u8 = 13;
pub const ICOMMIT: u8 = 14;
pub const IPARTIALCOMMIT: u8 = 15;
pub const IBACKCOMMIT: u8 = 16;
pub const IFAILTWICE: u8 = 17;
pub const IFAIL: u8 = 18;
pub const IGIVEUP: u8 = 19;
pub const IFULLCAPTURE: u8 = 20;
pub const IOPENCAPTURE: u8 = 21;
pub const ICLOSECAPTURE: u8 = 22;
pub const ICLOSERUNTIME: u8 = 23;

/// Slots of a charset instruction: the opcode slot plus the set.
pub const CHARSETINSTSIZE: usize = 1 + CHARSETSIZE / 4;
/// Largest `aux` of an `IBehind`, and of a full capture's size.
pub const MAXBEHIND: i32 = 255;
pub const MAXOFF: i32 = 0xF;
const NOINST: i32 = -1;

#[inline]
pub fn mk(op: u8, aux: u8, key: u16) -> u32 {
    op as u32 | (aux as u32) << 8 | (key as u32) << 16
}

#[inline]
pub fn op(code: &[u32], i: usize) -> u8 {
    code[i] as u8
}

#[inline]
pub fn aux(code: &[u32], i: usize) -> u8 {
    (code[i] >> 8) as u8
}

#[inline]
pub fn key(code: &[u32], i: usize) -> u16 {
    (code[i] >> 16) as u16
}

#[inline]
pub fn offset(code: &[u32], i: usize) -> i32 {
    code[i + 1] as i32
}

/// The charset stored in the slots from `i` on.
pub fn code_set(code: &[u32], i: usize) -> Charset {
    let mut cs = [0u8; CHARSETSIZE];
    for k in 0..CHARSETSIZE / 4 {
        cs[k * 4..k * 4 + 4].copy_from_slice(&code[i + k].to_le_bytes());
    }
    cs
}

/// Size in slots of the instruction at `i`.
pub fn sizei(code: &[u32], i: usize) -> usize {
    match op(code, i) {
        ISET | ISPAN => CHARSETINSTSIZE,
        ITESTSET => CHARSETINSTSIZE + 1,
        ITESTCHAR | ITESTANY | ICHOICE | IJMP | ICALL | IOPENCALL | ICOMMIT | IPARTIALCOMMIT
        | IBACKCOMMIT => 2,
        _ => 1,
    }
}

/// Pack a capture kind and an offset into an `aux` byte.
pub fn joinkindoff(kind: u8, off: i32) -> u8 {
    kind | ((off as u8) << 4)
}

pub fn getkind(aux: u8) -> u8 {
    aux & 0xF
}

pub fn getoff(aux: u8) -> i32 {
    ((aux >> 4) & 0xF) as i32
}

struct Code {
    code: Vec<u32>,
}

impl Code {
    fn here(&self) -> i32 {
        self.code.len() as i32
    }

    fn add(&mut self, opcode: u8, aux: u8) -> i32 {
        self.code.push(mk(opcode, aux, 0));
        self.code.len() as i32 - 1
    }

    /// An instruction followed by room for its offset.
    fn add_offset_inst(&mut self, opcode: u8) -> i32 {
        let i = self.add(opcode, 0);
        self.code.push(0);
        i
    }

    fn add_charset(&mut self, cs: &Charset) {
        for k in 0..CHARSETSIZE / 4 {
            self.code.push(u32::from_le_bytes([cs[k * 4], cs[k * 4 + 1], cs[k * 4 + 2], cs[k * 4 + 3]]));
        }
    }

    fn jump_to_there(&mut self, inst: i32, target: i32) {
        if inst >= 0 {
            self.code[inst as usize + 1] = (target - inst) as u32;
        }
    }

    fn jump_to_here(&mut self, inst: i32) {
        let here = self.here();
        self.jump_to_there(inst, here);
    }

    fn add_inst_cap(&mut self, opcode: u8, cap: u8, k: u16, auxoff: i32) {
        let i = self.add(opcode, joinkindoff(cap, auxoff));
        self.code[i as usize] = mk(opcode, joinkindoff(cap, auxoff), k);
    }

    fn set_aux(&mut self, inst: i32, aux: u8) {
        let word = &mut self.code[inst as usize];
        *word = (*word & !0xFF00) | (aux as u32) << 8;
    }
}

/// Classify a charset: empty (`IFAIL`), one character (`ICHAR`, returned in
/// `c`), all characters (`IANY`) or anything else (`ISET`).
fn charsettype(cs: &Charset, c: &mut u8) -> u8 {
    let count: u32 = cs.iter().map(|b| b.count_ones()).sum();
    match count {
        0 => IFAIL,
        1 => {
            let byte = cs.iter().position(|&b| b != 0).expect("one bit");
            *c = (byte * 8) as u8 + cs[byte].trailing_zeros() as u8;
            ICHAR
        }
        256 => IANY,
        _ => ISET,
    }
}

fn cs_disjoint(a: &Charset, b: &Charset) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| x & y == 0)
}

fn codechar(cs: &mut Code, c: u8, tt: i32) {
    if tt >= 0 && op(&cs.code, tt as usize) == ITESTCHAR && aux(&cs.code, tt as usize) == c {
        cs.add(IANY, 0);
    } else {
        cs.add(ICHAR, c);
    }
}

fn codecharset(cs: &mut Code, set: &Charset, tt: i32) {
    let mut c = 0u8;
    match charsettype(set, &mut c) {
        ICHAR => codechar(cs, c, tt),
        ISET => {
            if tt >= 0
                && op(&cs.code, tt as usize) == ITESTSET
                && code_set(&cs.code, tt as usize + 2) == *set
            {
                cs.add(IANY, 0);
            } else {
                cs.add(ISET, 0);
                cs.add_charset(set);
            }
        }
        opcode => {
            cs.add(opcode, c);
        }
    }
}

/// A test instruction for a set; `e` says the pattern may match the empty
/// string, in which case no test is possible.
fn codetestset(cs: &mut Code, set: &Charset, e: bool) -> i32 {
    if e {
        return NOINST;
    }
    let mut c = 0u8;
    match charsettype(set, &mut c) {
        IFAIL => cs.add_offset_inst(IJMP),
        IANY => cs.add_offset_inst(ITESTANY),
        ICHAR => {
            let i = cs.add_offset_inst(ITESTCHAR);
            cs.set_aux(i, c);
            i
        }
        _ => {
            let i = cs.add_offset_inst(ITESTSET);
            cs.add_charset(set);
            i
        }
    }
}

fn finaltarget(code: &[u32], mut i: usize) -> usize {
    while op(code, i) == IJMP {
        i = (i as i64 + offset(code, i) as i64) as usize;
    }
    i
}

fn finallabel(code: &[u32], i: usize) -> usize {
    finaltarget(code, (i as i64 + offset(code, i) as i64) as usize)
}

fn codebehind(cs: &mut Code, t: &mut [TTree], i: usize) {
    if t[i].u > 0 {
        cs.add(IBEHIND, t[i].u as u8);
    }
    codegen(cs, t, i + 1, false, NOINST, &FULLSET);
}

fn codechoice(cs: &mut Code, t: &mut [TTree], p1: usize, p2: usize, opt: bool, fl: &Charset) {
    let emptyp2 = t[p2].tag == TTRUE;
    let mut cs1 = [0u8; CHARSETSIZE];
    let mut cs2 = [0u8; CHARSETSIZE];
    let e1 = getfirst(t, p1, &FULLSET, &mut cs1);
    let disjoint = e1 == 0 && {
        getfirst(t, p2, fl, &mut cs2);
        cs_disjoint(&cs1, &cs2)
    };
    if headfail(t, p1) || disjoint {
        // test (fail(p1)) -> L1 ; p1 ; jmp L2 ; L1: p2 ; L2:
        let test = codetestset(cs, &cs1, false);
        let mut jmp = NOINST;
        codegen(cs, t, p1, false, test, fl);
        if !emptyp2 {
            jmp = cs.add_offset_inst(IJMP);
        }
        cs.jump_to_here(test);
        codegen(cs, t, p2, opt, NOINST, fl);
        cs.jump_to_here(jmp);
    } else if opt && emptyp2 {
        // p1? == partial_commit ; p1
        let pc = cs.add_offset_inst(IPARTIALCOMMIT);
        cs.jump_to_here(pc);
        codegen(cs, t, p1, true, NOINST, &FULLSET);
    } else {
        // test(first(p1)) -> L1 ; choice L1 ; p1 ; commit L2 ; L1: p2 ; L2:
        let test = codetestset(cs, &cs1, e1 != 0);
        let pchoice = cs.add_offset_inst(ICHOICE);
        codegen(cs, t, p1, emptyp2, test, &FULLSET);
        let pcommit = cs.add_offset_inst(ICOMMIT);
        cs.jump_to_here(pchoice);
        cs.jump_to_here(test);
        codegen(cs, t, p2, opt, NOINST, fl);
        cs.jump_to_here(pcommit);
    }
}

fn codeand(cs: &mut Code, t: &mut [TTree], i: usize, tt: i32) {
    let n = fixedlen(t, i);
    if n >= 0 && n <= MAXBEHIND && !hascaptures(t, i) {
        codegen(cs, t, i, false, tt, &FULLSET);
        if n > 0 {
            cs.add(IBEHIND, n as u8);
        }
    } else {
        // choice L1 ; p1 ; back_commit L2 ; L1: fail ; L2:
        let pchoice = cs.add_offset_inst(ICHOICE);
        codegen(cs, t, i, false, tt, &FULLSET);
        let pcommit = cs.add_offset_inst(IBACKCOMMIT);
        cs.jump_to_here(pchoice);
        cs.add(IFAIL, 0);
        cs.jump_to_here(pcommit);
    }
}

fn codecapture(cs: &mut Code, t: &mut [TTree], i: usize, tt: i32, fl: &Charset) {
    let len = fixedlen(t, i + 1);
    if len >= 0 && len <= MAXOFF && !hascaptures(t, i + 1) {
        codegen(cs, t, i + 1, false, tt, fl);
        cs.add_inst_cap(IFULLCAPTURE, t[i].cap as u8, t[i].key, len);
    } else {
        cs.add_inst_cap(IOPENCAPTURE, t[i].cap as u8, t[i].key, 0);
        codegen(cs, t, i + 1, false, tt, fl);
        cs.add_inst_cap(ICLOSECAPTURE, CCLOSE, 0, 0);
    }
}

fn coderuntime(cs: &mut Code, t: &mut [TTree], i: usize, tt: i32) {
    cs.add_inst_cap(IOPENCAPTURE, CGROUP, t[i].key, 0);
    codegen(cs, t, i + 1, false, tt, &FULLSET);
    cs.add_inst_cap(ICLOSERUNTIME, CCLOSE, 0, 0);
}

fn coderep(cs: &mut Code, t: &mut [TTree], body: usize, fl: &Charset) {
    let mut st = [0u8; CHARSETSIZE];
    if tocharset(t, body, &mut st) {
        cs.add(ISPAN, 0);
        cs.add_charset(&st);
        return;
    }
    let e1 = getfirst(t, body, &FULLSET, &mut st);
    if headfail(t, body) || (e1 == 0 && cs_disjoint(&st, fl)) {
        // L1: test (fail(p1)) -> L2 ; p ; jmp L1 ; L2:
        let test = codetestset(cs, &st, false);
        codegen(cs, t, body, false, test, &FULLSET);
        let jmp = cs.add_offset_inst(IJMP);
        cs.jump_to_here(test);
        cs.jump_to_there(jmp, test);
    } else {
        // test(fail(p1)) -> L2 ; choice L2 ; L1: p ; partial_commit L1 ; L2:
        // (without the test when p can match the empty string)
        let test = codetestset(cs, &st, e1 != 0);
        let pchoice = cs.add_offset_inst(ICHOICE);
        codegen(cs, t, body, false, NOINST, &FULLSET);
        let pcommit = cs.add_offset_inst(IPARTIALCOMMIT);
        cs.jump_to_there(pcommit, pchoice + 2);
        cs.jump_to_here(pchoice);
        cs.jump_to_here(test);
    }
}

fn codenot(cs: &mut Code, t: &mut [TTree], i: usize) {
    let mut st = [0u8; CHARSETSIZE];
    let e = getfirst(t, i, &FULLSET, &mut st);
    let test = codetestset(cs, &st, e != 0);
    if headfail(t, i) {
        // test (fail(p1)) -> L1 ; fail ; L1:
        cs.add(IFAIL, 0);
    } else {
        // test(fail(p)) -> L1 ; choice L1 ; p ; failtwice ; L1:
        let pchoice = cs.add_offset_inst(ICHOICE);
        codegen(cs, t, i, false, NOINST, &FULLSET);
        cs.add(IFAILTWICE, 0);
        cs.jump_to_here(pchoice);
    }
    cs.jump_to_here(test);
}

fn codecall(cs: &mut Code, t: &[TTree], i: usize) {
    let c = cs.add_offset_inst(IOPENCALL);
    let rule = sib2(t, i);
    debug_assert_eq!(t[rule].tag, TRULE);
    cs.code[c as usize] = mk(IOPENCALL, 0, t[rule].cap);
}

fn correctcalls(cs: &mut Code, positions: &[i32], from: i32, to: i32) {
    let mut i = from as usize;
    while i < to as usize {
        if op(&cs.code, i) == IOPENCALL {
            let n = key(&cs.code, i) as usize;
            let rule = positions[n];
            let tail = op(&cs.code, finaltarget(&cs.code, i + 2)) == IRET;
            cs.code[i] = mk(if tail { IJMP } else { ICALL }, 0, 0);
            cs.jump_to_there(i as i32, rule);
        }
        i += sizei(&cs.code, i);
    }
    debug_assert_eq!(i, to as usize);
}

fn codegrammar(cs: &mut Code, t: &mut [TTree], g: usize) {
    let mut positions = Vec::new();
    let firstcall = cs.add_offset_inst(ICALL);
    let jumptoend = cs.add_offset_inst(IJMP);
    let start = cs.here();
    cs.jump_to_here(firstcall);
    let mut rule = g + 1;
    while t[rule].tag == TRULE {
        positions.push(cs.here());
        codegen(cs, t, rule + 1, false, NOINST, &FULLSET);
        cs.add(IRET, 0);
        rule = sib2(t, rule);
    }
    debug_assert_eq!(t[rule].tag, TTRUE);
    cs.jump_to_here(jumptoend);
    let end = cs.here();
    correctcalls(cs, &positions, start, end);
}

/// Code a sequence's first pattern; returns the test target that still
/// protects the second one.
fn codeseq1(cs: &mut Code, t: &mut [TTree], p1: usize, p2: usize, tt: i32, fl: &Charset) -> i32 {
    if needfollow(t, p1) {
        let mut fl1 = [0u8; CHARSETSIZE];
        getfirst(t, p2, fl, &mut fl1);
        codegen(cs, t, p1, false, tt, &fl1);
    } else {
        codegen(cs, t, p1, false, tt, &FULLSET);
    }
    if fixedlen(t, p1) != 0 {
        NOINST
    } else {
        tt
    }
}

fn codegen(cs: &mut Code, t: &mut [TTree], mut i: usize, opt: bool, mut tt: i32, fl: &Charset) {
    loop {
        match t[i].tag {
            TCHAR => codechar(cs, t[i].u as u8, tt),
            TANY => {
                cs.add(IANY, 0);
            }
            TSET => {
                let set = read_set(t, i);
                codecharset(cs, &set, tt);
            }
            TTRUE => {}
            TFALSE => {
                cs.add(IFAIL, 0);
            }
            TCHOICE => {
                let second = sib2(t, i);
                codechoice(cs, t, i + 1, second, opt, fl);
            }
            TREP => coderep(cs, t, i + 1, fl),
            TBEHIND => codebehind(cs, t, i),
            TNOT => codenot(cs, t, i + 1),
            TAND => codeand(cs, t, i + 1, tt),
            TCAPTURE => codecapture(cs, t, i, tt, fl),
            TRUNTIME => coderuntime(cs, t, i, tt),
            TGRAMMAR => codegrammar(cs, t, i),
            TCALL => codecall(cs, t, i),
            TSEQ => {
                let second = sib2(t, i);
                tt = codeseq1(cs, t, i + 1, second, tt, fl);
                i = second;
                continue;
            }
            tag => unreachable!("codegen: tag {tag}"),
        }
        return;
    }
}

/// Optimise jumps and jump-like instructions.
fn peephole(cs: &mut Code) {
    let n = cs.code.len();
    let mut i = 0;
    while i < n {
        loop {
            match op(&cs.code, i) {
                ICHOICE | ICALL | ICOMMIT | IPARTIALCOMMIT | IBACKCOMMIT | ITESTCHAR | ITESTSET
                | ITESTANY => {
                    let target = finallabel(&cs.code, i) as i32;
                    cs.jump_to_there(i as i32, target);
                }
                IJMP => {
                    let ft = finaltarget(&cs.code, i);
                    match op(&cs.code, ft) {
                        IRET | IFAIL | IFAILTWICE | IEND => {
                            cs.code[i] = cs.code[ft];
                            // 'no-op' for the target position
                            cs.code[i + 1] &= !0xFF;
                            cs.code[i + 1] |= IANY as u32;
                        }
                        ICOMMIT | IPARTIALCOMMIT | IBACKCOMMIT => {
                            let fft = finallabel(&cs.code, ft) as i32;
                            cs.code[i] = cs.code[ft];
                            cs.jump_to_there(i as i32, fft);
                            continue;
                        }
                        _ => cs.jump_to_there(i as i32, ft as i32),
                    }
                }
                _ => {}
            }
            break;
        }
        i += sizei(&cs.code, i);
    }
    debug_assert_eq!(op(&cs.code, i - 1), IEND);
}

/// Compile the tree to instructions.
pub fn compile(t: &mut [TTree]) -> Vec<u32> {
    let mut cs = Code { code: Vec::new() };
    codegen(&mut cs, t, 0, false, NOINST, &FULLSET);
    cs.add(IEND, 0);
    peephole(&mut cs);
    cs.code
}
