//! `lpeg.ptree` and `lpeg.pcode` (lpprint.c): dumps of a pattern's tree and
//! of its compiled instructions, printed on standard output like LPeg's C
//! `printf`s do.

use std::fmt::Write;
use std::io::Write as IoWrite;

use super::code::*;
use super::tree::*;
use super::value::V;

const TAGNAMES: [&str; 17] = [
    "char", "set", "any", "true", "false", "rep", "seq", "choice", "not", "and", "call", "opencall", "rule",
    "grammar", "behind", "capture", "run-time",
];

const CAPKIND: [&str; 16] = [
    "close", "position", "constant", "backref", "argument", "simple", "table", "function", "query", "string",
    "num", "substitution", "fold", "runtime", "group", "accumulator",
];

const OPNAMES: [&str; 24] = [
    "any", "char", "set", "testany", "testchar", "testset", "span", "behind", "ret", "end", "choice", "jmp",
    "call", "open_call", "commit", "partial_commit", "back_commit", "failtwice", "fail", "giveup",
    "fullcapture", "opencapture", "closecapture", "closeruntime",
];

fn emit(text: &str) {
    emit_bytes(text.as_bytes());
}

fn emit_bytes(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

fn charset(cs: &[u8]) -> String {
    let mut s = String::from("[");
    let mut i = 0usize;
    while i <= 255 {
        let first = i;
        while i <= 255 && testchar(cs, i as u8) {
            i += 1;
        }
        if i == first + 1 {
            let _ = write!(s, "({first:02x})");
        } else if i > first + 1 {
            let _ = write!(s, "({first:02x}-{:02x})", i - 1);
        }
        i += 1;
    }
    s.push(']');
    s
}

pub fn printktable(ktable: &[V]) {
    let mut s = String::from("[");
    for (i, v) in ktable.iter().enumerate() {
        let _ = write!(s, "{} = ", i + 1);
        match v.to_bytes() {
            Some(b) => s.push_str(&String::from_utf8_lossy(&b)),
            None => s.push_str(v.type_name()),
        }
        s.push_str("  ");
    }
    s.push_str("]\n");
    emit(&s);
}

pub fn printtree(t: &[TTree], i: usize, ident: usize) {
    let mut s = String::new();
    treetext(t, i, ident, &mut s);
    emit(&s);
}

fn treetext(t: &[TTree], i: usize, ident: usize, out: &mut String) {
    for _ in 0..ident {
        out.push(' ');
    }
    let tag = t[i].tag;
    out.push_str(TAGNAMES[tag as usize]);
    match tag {
        TCHAR => {
            let c = t[i].u;
            if (0x20..0x7F).contains(&c) {
                let _ = writeln!(out, " '{}'", c as u8 as char);
            } else {
                let _ = writeln!(out, " ({c:02X})");
            }
        }
        TSET => {
            out.push_str(&charset(&read_set(t, i)));
            out.push('\n');
        }
        TOPENCALL | TCALL => {
            let rule = sib2(t, i);
            let _ = writeln!(out, " key: {}  (rule: {})", t[i].key, t[rule].cap);
        }
        TBEHIND => {
            let _ = writeln!(out, " {}", t[i].u);
            treetext(t, i + 1, ident + 2, out);
        }
        TCAPTURE => {
            let _ = writeln!(out, " kind: '{}'  key: {}", CAPKIND[t[i].cap as usize], t[i].key);
            treetext(t, i + 1, ident + 2, out);
        }
        TRULE => {
            let _ = writeln!(out, " n: {}  key: {}", t[i].cap, t[i].key);
            treetext(t, i + 1, ident + 2, out);
        }
        TGRAMMAR => {
            let _ = writeln!(out, " {}", t[i].u);
            let mut rule = i + 1;
            for _ in 0..t[i].u {
                treetext(t, rule, ident + 2, out);
                rule = sib2(t, rule);
            }
        }
        _ => {
            out.push('\n');
            let sibs = match tag {
                TREP | TNOT | TAND | TRUNTIME => 1,
                TSEQ | TCHOICE => 2,
                _ => 0,
            };
            if sibs >= 1 {
                treetext(t, i + 1, ident + 2, out);
                if sibs >= 2 {
                    let second = sib2(t, i);
                    treetext(t, second, ident + 2, out);
                }
            }
        }
    }
}

pub fn printpatt(code: &[u32]) {
    let mut out: Vec<u8> = Vec::new();
    let mut s = String::new();
    let mut p = 0;
    while p < code.len() {
        let _ = write!(s, "{p:02}: {} ", OPNAMES[op(code, p) as usize]);
        match op(code, p) {
            ICHAR => {
                s.push('\'');
                out.extend_from_slice(s.as_bytes());
                out.push(aux(code, p));
                s.clear();
                s.push('\'');
            }
            IFULLCAPTURE => {
                let a = aux(code, p);
                let _ = write!(s, "{} (size = {})  (idx = {})", CAPKIND[getkind(a) as usize], getoff(a), key(code, p));
            }
            IOPENCAPTURE | ICLOSECAPTURE | ICLOSERUNTIME => {
                let a = aux(code, p);
                if op(code, p) == IOPENCAPTURE {
                    let _ = write!(s, "{} (idx = {})", CAPKIND[getkind(a) as usize], key(code, p));
                }
            }
            ISET | ISPAN => {
                s.push_str(&charset(&code_set(code, p + 1)));
            }
            ITESTSET => {
                s.push_str(&charset(&code_set(code, p + 2)));
                let _ = write!(s, "-> {}", p as i64 + offset(code, p) as i64);
            }
            ITESTCHAR => {
                s.push('\'');
                out.extend_from_slice(s.as_bytes());
                out.push(aux(code, p));
                s.clear();
                let _ = write!(s, "'-> {}", p as i64 + offset(code, p) as i64);
            }
            ITESTANY | ICHOICE | IJMP | ICALL | ICOMMIT | IPARTIALCOMMIT | IBACKCOMMIT => {
                let _ = write!(s, "-> {}", p as i64 + offset(code, p) as i64);
            }
            IOPENCALL => {
                let _ = write!(s, "-> {}", key(code, p));
            }
            IBEHIND => {
                let _ = write!(s, "{}", aux(code, p));
            }
            _ => {}
        }
        s.push('\n');
        out.extend_from_slice(s.as_bytes());
        s.clear();
        p += sizei(code, p);
    }
    emit_bytes(&out);
}
