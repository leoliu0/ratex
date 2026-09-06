//! Format dump: binary serialization of the engine boot state (`.fmt`).
//!
//! Mirrors TeX's `\dump`: after `-ini` mode finishes loading the macro
//! package (e.g. `latex.ltx` ends with `\dump`), the whole boot-derived
//! state — control-sequence table, equivalents (macros, registers,
//! equivalents), parameter/register/code tables with save levels, font
//! registry (full TFM metrics), catcode table, and hyphenation trie — is
//! written as one compact little-endian blob. A later run loads it in
//! a few milliseconds instead of re-executing the bootstrap.
//!
//! Runtime state (input stack, page lists, PDF document, write streams)
//! is never serialized: at `\dump` time it is empty. Non-void box
//! registers and a non-empty save stack make the dump refuse, matching
//! tex.web's requirement that `\dump` happen at top level.
//!
//! Wire format (all integers little-endian):
//! `MAGIC(8) VER(u16)`, then sections in the order written by
//! [`save_format`]. List = `u32 len` + elements. Strings = byte list.

use std::io;
use std::path::Path;
use std::rc::Rc;

use crate::boxes::Glue;
use crate::engine::Engine;
use crate::eqtb::{Equiv, Macro, NUM_REGISTERS};
use crate::hyphen::Trie;
use crate::prim::{DimParam, GlueParam, IntParam, Prim, ToksParam, NUM_DIM_PARAMS, NUM_GLUE_PARAMS, NUM_INT_PARAMS, NUM_TOKS_PARAMS};
use crate::tfm::{CharInfo, ExtRecipe, Font, LigStep};
use crate::token::{CsTable, Token};

const MAGIC: &[u8; 8] = b"RUSTEXFM";
const VERSION: u16 = 3;
/// Bumped whenever serialized state changes meaning without changing the
/// wire layout (new engine invariants the loaded state must satisfy, e.g.
/// guards added to `check_dumpable` after the file was written). A `.fmt`
/// from a different engine generation must be rejected, not loaded.
pub const SEMANTICS: u16 = 2;


const NUM_CODES: usize = 256;

// Equiv wire tags.
const TAG_NONE: u8 = 0;
const TAG_COUNT_REG: u8 = 1;
const TAG_DIMEN_REG: u8 = 2;
const TAG_SKIP_REG: u8 = 3;
const TAG_MUSKIP_REG: u8 = 4;
const TAG_TOKS_REG: u8 = 5;
const TAG_BOX_REG: u8 = 6;
const TAG_CHAR_DEF: u8 = 7;
const TAG_CHAR_TOK: u8 = 8;
const TAG_MATHCHAR_DEF: u8 = 9;
const TAG_FONT_REF: u8 = 10;
const TAG_ALIAS: u8 = 11;
const TAG_PRIM: u8 = 12;
const TAG_MACRO: u8 = 13;

// ---------------------------------------------------------------------------
// writer / reader primitives
// ---------------------------------------------------------------------------

struct W {
    buf: Vec<u8>,
}

impl W {
    fn new() -> W {
        W { buf: Vec::with_capacity(1 << 20) }
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.buf.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn opt_str(&mut self, s: &Option<String>) {
        match s {
            Some(x) => {
                self.u8(1);
                self.str(x);
            }
            None => self.u8(0),
        }
    }
    fn glue(&mut self, g: &Glue) {
        self.i32(g.width);
        self.i32(g.stretch);
        self.i32(g.shrink);
        self.u8(g.stretch_order);
        self.u8(g.shrink_order);
    }
    fn toks(&mut self, t: &[Token]) {
        self.u32(t.len() as u32);
        for &tok in t {
            self.u32(tok.0);
        }
    }
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> R<'a> {
        R { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.b.len() - self.p < n {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "format truncated"));
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    /// list length, guarded against corrupt headers demanding huge allocations
    fn count(&mut self) -> io::Result<usize> {
        let n = self.u32()? as usize;
        if n > self.b.len() - self.p {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "format corrupt length"));
        }
        Ok(n)
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let cnt = self.count()?;
        Ok(self.take(cnt)?.to_vec())
    }
    fn str(&mut self) -> io::Result<String> {
        let b = self.bytes()?;
        String::from_utf8(b)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "format bad utf8"))
    }
    fn opt_str(&mut self) -> io::Result<Option<String>> {
        if self.u8()? == 1 {
            Ok(Some(self.str()?))
        } else {
            Ok(None)
        }
    }
    fn glue(&mut self) -> io::Result<Glue> {
        Ok(Glue {
            width: self.i32()?,
            stretch: self.i32()?,
            shrink: self.i32()?,
            stretch_order: self.u8()?,
            shrink_order: self.u8()?,
        })
    }
    fn toks(&mut self) -> io::Result<Vec<Token>> {
        let n = self.count()?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(Token(self.u32()?));
        }
        Ok(v)
    }
    /// exactly `n` raw i32s (fixed-size tables carry no length prefix)
    fn raw_i32(&mut self, n: usize) -> io::Result<Vec<i32>> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(self.i32()?);
        }
        Ok(v)
    }
    /// exactly `n` raw u16s
    fn raw_u16(&mut self, n: usize) -> io::Result<Vec<u16>> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(self.u16()?);
        }
        Ok(v)
    }
    fn vec_i32(&mut self) -> io::Result<Vec<i32>> {
        let n = self.count()?;
        self.raw_i32(n)
    }
    fn vec_u16(&mut self) -> io::Result<Vec<u16>> {
        let n = self.count()?;
        self.raw_u16(n)
    }
}

fn fixed<const N: usize>(r: &mut R) -> io::Result<[u8; N]> {
    let mut a = [0u8; N];
    a.copy_from_slice(r.take(N)?);
    Ok(a)
}

// ---------------------------------------------------------------------------
// dumpability checks
// ---------------------------------------------------------------------------

/// Reasons a `\dump` would be refused (tex.web: \dump at top level only).
pub fn check_dumpable(eng: &Engine) -> Result<(), String> {
    if !eng.eqtb.save_stack.is_empty() {
        let mut n_level = 0u32;
        let mut n_eq = 0u32;
        let mut n_ag = 0u32;
        let mut n_other = 0u32;
        let mut types: Vec<String> = Vec::new();
        for it in &eng.eqtb.save_stack {
            match it {
                crate::eqtb::SaveItem::Level(lvl, ty) => {
                    n_level += 1;
                    if types.len() < 24 {
                        types.push(format!("{ty:?}@{lvl}"));
                    }
                }
                crate::eqtb::SaveItem::Eq(..) => n_eq += 1,
                crate::eqtb::SaveItem::AfterGroup(tok) => {
                    n_ag += 1;
                    let name = if tok.is_cs() { String::from_utf8_lossy(eng.cs.name(tok.cs_id())).into_owned() } else { format!("c{}:{}", tok.cc(), tok.chr()) };
                    eprintln!("AFTERGROUP-TOKEN: {}", name);
                }
                other => {
                    n_other += 1;
                    if types.len() < 24 {
                        types.push(format!("{:?}", std::mem::discriminant(other)));
                    }
                }
            }
        }
        eprintln!(
            "DUMP-SAVE cur_level={} n={} level={} eq={} aftergroup={} other={} types=[{}]",
            eng.eqtb.cur_level,
            eng.eqtb.save_stack.len(),
            n_level,
            n_eq,
            n_ag,
            n_other,
            types.join(",")
        );
        // If there are only top-level aftergroup tokens (e.g. from \set@color in preamble)
        // and no open groups or modified registers, allow the format dump to proceed.
        if n_level != 0 || n_eq != 0 || n_other != 0 || eng.eqtb.cur_level != crate::eqtb::LEVEL_ONE {
            return Err(format!(
                "cannot dump: {} pending (level={} eq={} aftergroup={} other={} cur_level={} ss_open=[{}] types=[{}])",
                eng.eqtb.save_stack.len(),
                n_level,
                n_eq,
                n_ag,
                n_other,
                eng.eqtb.cur_level,
                eng.ss_trace.join(" | "),
                types.join(",")
            ));
        }
    }

    if eng.input.stack.len() > 3 {
        return Err(format!(
            "cannot dump: {} input sources above the base file (\\dump must be at the top level)",
            eng.input.stack.len() - 1
        ));
    }
    for (class, marks) in eng.marks.iter().enumerate() {
        if !marks.is_empty() {
            return Err(format!(
                "cannot dump: mark class {} holds {} unresolved \\mark{}",
                class,
                marks.len(),
                if marks.len() == 1 { "" } else { "s" }
            ));
        }
    }
    if !eng.pdf_page_attr.is_empty() {
        return Err("cannot dump: \\pdfpageattr is set (not serialized)".to_string());
    }
    if !eng.pdf_pages_attr.is_empty() {
        return Err("cannot dump: \\pdfpagesattr is set (not serialized)".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// save
// ---------------------------------------------------------------------------

/// Serialize the engine's boot state to `path` (usually `pdflatex.fmt`).
/// Returns the number of bytes written.
pub fn save_format(eng: &Engine, path: &Path) -> Result<usize, String> {
    check_dumpable(eng)?;
    let mut w = W::new();
    w.buf.extend_from_slice(MAGIC);
    w.u16(VERSION);
    w.u16(SEMANTICS);
    w.u16(eng.eqtb.cur_font_val);
    // control-sequence names (id = position)
    w.u32(eng.cs.len() as u32);
    for id in eng.cs.all_ids() {
        w.bytes(eng.cs.name(id));
    }

    // equivalents
    let eqs = eng.eqtb.eqs();
    w.u32(eqs.len() as u32);
    for (id, equiv, level) in eqs {
        w.u32(id);
        w.u16(level);
        match equiv {
            None => w.u8(TAG_NONE),
            Some(Equiv::CountReg(v)) => {
                w.u8(TAG_COUNT_REG);
                w.u16(*v);
            }
            Some(Equiv::DimenReg(v)) => {
                w.u8(TAG_DIMEN_REG);
                w.u16(*v);
            }
            Some(Equiv::SkipReg(v)) => {
                w.u8(TAG_SKIP_REG);
                w.u16(*v);
            }
            Some(Equiv::MuSkipReg(v)) => {
                w.u8(TAG_MUSKIP_REG);
                w.u16(*v);
            }
            Some(Equiv::ToksReg(v)) => {
                w.u8(TAG_TOKS_REG);
                w.u16(*v);
            }
            Some(Equiv::BoxReg(v)) => {
                w.u8(TAG_BOX_REG);
                w.u16(*v);
            }
            Some(Equiv::CharDef(v)) => {
                w.u8(TAG_CHAR_DEF);
                w.u32(*v);
            }
            Some(Equiv::CharTok(v)) => {
                w.u8(TAG_CHAR_TOK);
                w.u32(*v);
            }
            Some(Equiv::MathCharDef(v)) => {
                w.u8(TAG_MATHCHAR_DEF);
                w.u16(*v);
            }
            Some(Equiv::FontRef(v)) => {
                w.u8(TAG_FONT_REF);
                w.u16(*v);
            }
            Some(Equiv::Alias(v)) => {
                w.u8(TAG_ALIAS);
                w.u32(*v);
            }
            Some(Equiv::Prim(p)) => {
                w.u8(TAG_PRIM);
                w.u16(p.code());
            }
            Some(Equiv::Macro(m)) => {
                w.u8(TAG_MACRO);
                write_macro(&mut w, m);
            }
        }
    }
    w.u16(eng.eqtb.cur_level);

    // named parameter tables
    for v in &eng.eqtb.int_params {
        w.i32(*v);
    }
    for v in &eng.eqtb.int_levels {
        w.u16(*v);
    }
    for v in &eng.eqtb.dim_params {
        w.i32(*v);
    }
    for v in &eng.eqtb.dim_levels {
        w.u16(*v);
    }
    for g in &eng.eqtb.glue_params {
        w.glue(g);
    }
    for v in &eng.eqtb.glue_levels {
        w.u16(*v);
    }
    for t in &eng.eqtb.tok_params {
        w.toks(t);
    }
    for v in &eng.eqtb.tok_levels {
        w.u16(*v);
    }

    // registers
    for v in &eng.eqtb.count {
        w.i32(*v);
    }
    for v in &eng.eqtb.count_levels {
        w.u16(*v);
    }
    for v in &eng.eqtb.dimen {
        w.i32(*v);
    }
    for v in &eng.eqtb.dimen_levels {
        w.u16(*v);
    }
    for g in &eng.eqtb.skip {
        w.glue(g);
    }
    for v in &eng.eqtb.skip_levels {
        w.u16(*v);
    }
    for g in &eng.eqtb.muskip {
        w.glue(g);
    }
    for v in &eng.eqtb.muskip_levels {
        w.u16(*v);
    }
    for t in &eng.eqtb.toks {
        w.toks(t);
    }
    for v in &eng.eqtb.toks_levels {
        w.u16(*v);
    }
    // box registers: dumpability guarantees all void
    for v in &eng.eqtb.box_levels {
        w.u16(*v);
    }

    // code tables
    w.buf.extend_from_slice(&eng.eqtb.cat);
    for v in &eng.eqtb.cat_levels {
        w.u16(*v);
    }
    for v in &eng.eqtb.math_code {
        w.u16(*v);
    }
    for v in &eng.eqtb.math_levels {
        w.u16(*v);
    }
    for v in &eng.eqtb.del_code {
        w.i32(*v);
    }
    for v in &eng.eqtb.del_levels {
        w.u16(*v);
    }
    w.buf.extend_from_slice(&eng.eqtb.lc_code);
    for v in &eng.eqtb.lc_levels {
        w.u16(*v);
    }
    for v in &eng.eqtb.sf_code {
        w.u16(*v);
    }
    for v in &eng.eqtb.sf_levels {
        w.u16(*v);
    }
    w.buf.extend_from_slice(&eng.eqtb.uc_code);
    for v in &eng.eqtb.uc_levels {
        w.u16(*v);
    }

    // style fonts
    for style in &eng.eqtb.style_fonts {
        for f in style {
            w.u16(*f);
        }
    }
    for style in &eng.eqtb.style_font_levels {
        for f in style {
            w.u16(*f);
        }
    }

    // fonts
    w.u32(eng.eqtb.fonts.len() as u32);
    for (i, f) in eng.eqtb.fonts.iter().enumerate() {
        write_font(&mut w, f);
        let fp = eng.eqtb.font_params.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
        w.u32(fp.len() as u32);
        for p in fp {
            w.i32(*p);
        }
        let fpl = eng.eqtb.font_param_levels.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
        w.u32(fpl.len() as u32);
        for l in fpl {
            w.u16(*l);
        }
        w.i32(eng.eqtb.hyphen_char.get(i).copied().unwrap_or(b'-' as i32));
        w.u16(eng.eqtb.hyphen_char_levels.get(i).copied().unwrap_or(0));
        w.i32(eng.eqtb.skew_char.get(i).copied().unwrap_or(-1));
        w.u16(eng.eqtb.skew_char_levels.get(i).copied().unwrap_or(0));
        w.u32(eng.eqtb.font_cs.get(i).copied().unwrap_or(0));
    }
    // hyphenation
    write_trie(&mut w, &eng.hyphen_trie);
    w.u32(eng.hyphen_exceptions.len() as u32);
    for (name, word) in &eng.hyphen_exceptions {
        w.str(name);
        w.bytes(word);
    }

    // paragraph shape
    w.u32(eng.par_shape.len() as u32);
    for (a, b) in &eng.par_shape {
        w.i32(*a);
        w.i32(*b);
    }

    std::fs::write(path, &w.buf)
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok(w.buf.len())
}

fn write_macro(w: &mut W, m: &Macro) {
    let mut flags = 0u8;
    if m.long {
        flags |= 1;
    }
    if m.outer {
        flags |= 2;
    }
    if m.protected {
        flags |= 4;
    }
    w.u8(flags);
    w.u8(m.num_params);
    w.toks(&m.prefix);
    w.u32(m.params.len() as u32);
    for p in &m.params {
        w.toks(p);
    }
    w.toks(&m.body);
}

fn write_font(w: &mut W, f: &Font) {
    w.str(&f.name);
    w.str(&f.tfm_name);
    w.i32(f.at_size);
    w.i32(f.dsize);
    w.u8(f.bc);
    w.u8(f.ec);
    w.u32(f.chars.len() as u32);
    for c in &f.chars {
        w.i32(c.width);
        w.i32(c.height);
        w.i32(c.depth);
        w.i32(c.italic);
        w.u8(c.tag);
        w.u8(c.remainder);
    }
    w.u32(f.lig_kern.len() as u32);
    for l in &f.lig_kern {
        w.u8(l.skip);
        w.u8(l.next_char);
        w.u8(l.op);
        w.u8(l.rem);
        w.u8(l.stop as u8);
    }
    w.u32(f.kerns.len() as u32);
    for k in &f.kerns {
        w.i32(*k);
    }
    w.u32(f.ext.len() as u32);
    for e in &f.ext {
        w.u8(e.top);
        w.u8(e.mid);
        w.u8(e.bot);
        w.u8(e.rep);
    }
    w.u32(f.params.len() as u32);
    for p in &f.params {
        w.i32(*p);
    }
    w.i32(f.hyphen_char);
    w.i32(f.skew_char);
    w.opt_str(&f.type1_path);
    w.opt_str(&f.enc_name);
    w.opt_str(&f.map_fontname);
    match &f.encoding {
        Some(enc) => {
            w.u8(1);
            w.u32(enc.len() as u32);
            for g in enc {
                w.str(g);
            }
        }
        None => {
            w.u8(0);
        }
    }
}

fn write_trie(w: &mut W, t: &Trie) {
    w.u32(t.trans.len() as u32);
    for (i, node) in t.trans.iter().enumerate() {
        let mut edges: Vec<(u8, usize)> = node.iter().map(|(&b, &n)| (b, n)).collect();
        edges.sort_unstable();
        w.u32(edges.len() as u32);
        for (b, n) in edges {
            w.u8(b);
            w.u32(n as u32);
        }
        let vals = t.values.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
        w.u32(vals.len() as u32);
        for (pos, v) in vals {
            w.u64(*pos as u64);
            w.u8(*v);
        }
    }
    let mut excs: Vec<(&Vec<u8>, &Vec<usize>)> = t.exceptions.iter().collect();
    excs.sort_unstable_by(|a, b| a.0.cmp(b.0));
    w.u32(excs.len() as u32);
    for (k, pts) in excs {
        w.bytes(k);
        w.u32(pts.len() as u32);
        for &p in pts {
            w.u64(p as u64);
        }
    }
}

// ---------------------------------------------------------------------------
// load
// ---------------------------------------------------------------------------

/// Deserialize a format file into a ready-to-run engine (ini mode off,
/// `\dump`-completed state). Any I/O, magic, version, or truncation error
/// is reported so the caller can fall back to a full bootstrap.
pub fn load_format(path: &Path) -> Result<Engine, String> {
    let data = std::fs::read(path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    load_format_from(&data)
}

/// Validate the header (magic, VERSION, SEMANTICS) and return a reader
/// positioned at the first payload byte. Shared by every load path so a
/// stale-format check can never be bypassed.
fn parse_header(data: &[u8]) -> Result<R<'_>, String> {
    if data.len() < MAGIC.len() + 4 || &data[..MAGIC.len()] != MAGIC {
        return Err("not a rustex format file".to_string());
    }
    let mut r = R::new(&data[MAGIC.len()..]);
    if r.u16().map_err(io_err)? != VERSION {
        return Err("format version mismatch".to_string());
    }
    if r.u16().map_err(io_err)? != SEMANTICS {
        return Err(
            "format semantics mismatch (engine updated; delete the .fmt file)".to_string(),
        );
    }
    Ok(r)
}

pub fn load_format_from(data: &[u8]) -> Result<Engine, String> {
    let mut r = parse_header(data)?;
    let mut eng = Engine::new(false);
    load_state(&mut r, &mut eng).map_err(io_err)?;
    // Repair primitives a bad boot stored as \\relax. Do not clobber
    // LaTeX redefinitions: \\end (Macro), \\bgroup (CharTok), \\everypar
    // (ToksReg from \\newtoks). \\protected is restored even if dumped
    // as a macro (that was the original boot bug).
    let mut snap = Engine::new(true);
    snap.init_primitives();

    for sid in 0..snap.cs.len() as u32 {
        let p = match snap.eqtb.get(sid) {
            Some(Equiv::Prim(p)) => *p,
            _ => continue,
        };
        let name = snap.cs.name(sid).to_vec();
        let did = eng.cs.lookup(&name).unwrap_or_else(|| eng.cs.intern(&name));
        let force = name.as_slice() == b"protected";
        match eng.eqtb.get(did) {
            Some(Equiv::Prim(p2)) if *p2 == p => {}
            None | Some(Equiv::Prim(Prim::Relax)) => {
                eng.eqtb.assign(did, Equiv::Prim(p), true);
            }
            _ if force => {
                eng.eqtb.assign(did, Equiv::Prim(p), true);
            }
            _ => {}
        }
    }


    Ok(eng)
}



/// Deserialize `path` into `eng` in place. The CLI uses this with its
/// already-constructed engine so process-wide one-time setup (kpse ls-R
/// parsing, primitive interning) is not paid twice.
///
/// The file is decoded into a scratch engine first; the caller's state is
/// only transplanted after the whole format parsed cleanly. A corrupt or
/// truncated file therefore cannot poison the engine (a partially loaded
/// font list would shift every FontRef id); the caller falls back to a
/// full bootstrap on any error.
pub fn load_format_into(path: &Path, eng: &mut Engine) -> Result<(), String> {
    let data = std::fs::read(path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    load_format_bytes_into(&data, eng)
}

pub fn load_format_bytes_into(data: &[u8], eng: &mut Engine) -> Result<(), String> {
    let scratch = load_format_from(data)?;
    // Full success only now: transplant the boot state while keeping the
    // caller's process-wide setup (font_loader, ids, out_dir, pdf_doc).
    eng.cs = scratch.cs;
    eng.eqtb = scratch.eqtb;
    eng.hyphen_trie = scratch.hyphen_trie;
    eng.hyphen_exceptions = scratch.hyphen_exceptions;
    eng.par_shape = scratch.par_shape;
    eng.format_done = scratch.format_done;
    eng.ini_mode = scratch.ini_mode;
    Ok(())
}

fn io_err(e: io::Error) -> String {
    format!("format load: {}", e)
}

fn load_state(r: &mut R, eng: &mut Engine) -> io::Result<()> {
    eng.eqtb.cur_font_val = r.u16()?;

    let n = r.count()?;
    let mut cs = CsTable::new();
    for _ in 0..n {
        let name = r.bytes()?;
        cs.intern(&name);
    }
    if cs.name(eng.ids.par) != b"par" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "format cs table incompatible with engine ids",
        ));
    }
    eng.cs = cs;
    // equivalents
    let n = r.count()?;
    for _ in 0..n {
        let id = r.u32()?;
        let level = r.u16()?;
        let tag = r.u8()?;
        let equiv = match tag {
            TAG_NONE => None,
            TAG_COUNT_REG => Some(Equiv::CountReg(r.u16()?)),
            TAG_DIMEN_REG => Some(Equiv::DimenReg(r.u16()?)),
            TAG_SKIP_REG => Some(Equiv::SkipReg(r.u16()?)),
            TAG_MUSKIP_REG => Some(Equiv::MuSkipReg(r.u16()?)),
            TAG_TOKS_REG => Some(Equiv::ToksReg(r.u16()?)),
            TAG_BOX_REG => Some(Equiv::BoxReg(r.u16()?)),
            TAG_CHAR_DEF => Some(Equiv::CharDef(r.u32()?)),
            TAG_CHAR_TOK => Some(Equiv::CharTok(r.u32()?)),
            TAG_MATHCHAR_DEF => Some(Equiv::MathCharDef(r.u16()?)),
            TAG_FONT_REF => Some(Equiv::FontRef(r.u16()?)),
            TAG_ALIAS => Some(Equiv::Alias(r.u32()?)),
            TAG_PRIM => {
                let code = r.u16()?;
                match Prim::from_code(code) {
                    Some(p) => Some(Equiv::Prim(p)),
                    None => {
                        eprintln!("TAG_PRIM FAILED code={:#x} ({}) id={} cs={:?}", code, code, id, String::from_utf8_lossy(eng.cs.name(id)));
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "format has unknown primitive code",
                        ));
                    }
                }
            }
            TAG_MACRO => Some(Equiv::Macro(Rc::new(read_macro(r)?))),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "format has unknown equivalent tag",
                ))
            }
        };
        eng.eqtb.restore_eq(id, equiv, level);
    }
    eng.eqtb.cur_level = r.u16()?;

    // named parameter tables (fresh-engine tables are pre-filled to the
    // engine's fixed sizes: validate lengths and replace, never append)
    fn expect_len<T>(v: Vec<T>, want: usize, what: &str) -> io::Result<Vec<T>> {
        if v.len() != want {
            return Err(bad(&format!("{} length {} != {}", what, v.len(), want)));
        }
        Ok(v)
    }
    eng.eqtb.int_params = r.raw_i32(NUM_INT_PARAMS)?;
    eng.eqtb.int_levels = r.raw_u16(NUM_INT_PARAMS)?;
    eng.eqtb.dim_params = r.raw_i32(NUM_DIM_PARAMS)?;
    eng.eqtb.dim_levels = r.raw_u16(NUM_DIM_PARAMS)?;
    eng.eqtb.glue_params.clear();
    for _ in 0..NUM_GLUE_PARAMS {
        eng.eqtb.glue_params.push(r.glue()?);
    }
    eng.eqtb.glue_levels = r.raw_u16(NUM_GLUE_PARAMS)?;
    eng.eqtb.tok_params.clear();
    for _ in 0..NUM_TOKS_PARAMS {
        eng.eqtb.tok_params.push(Rc::new(r.toks()?));
    }
    eng.eqtb.tok_levels = r.raw_u16(NUM_TOKS_PARAMS)?;

    // registers
    eng.eqtb.count = r.raw_i32(NUM_REGISTERS)?;
    eng.eqtb.count_levels = r.raw_u16(NUM_REGISTERS)?;
    eng.eqtb.dimen = r.raw_i32(NUM_REGISTERS)?;
    eng.eqtb.dimen_levels = r.raw_u16(NUM_REGISTERS)?;
    eng.eqtb.skip.clear();
    for _ in 0..NUM_REGISTERS {
        eng.eqtb.skip.push(r.glue()?);
    }
    eng.eqtb.skip_levels = r.raw_u16(NUM_REGISTERS)?;
    eng.eqtb.muskip.clear();
    for _ in 0..NUM_REGISTERS {
        eng.eqtb.muskip.push(r.glue()?);
    }
    eng.eqtb.muskip_levels = r.raw_u16(NUM_REGISTERS)?;
    eng.eqtb.toks.clear();
    for _ in 0..NUM_REGISTERS {
        eng.eqtb.toks.push(Rc::new(r.toks()?));
    }
    eng.eqtb.toks_levels = r.raw_u16(NUM_REGISTERS)?;
    eng.eqtb.box_levels = r.raw_u16(NUM_REGISTERS)?;

    // code tables
    eng.eqtb.cat = fixed::<NUM_CODES>(r)?.to_vec();
    eng.eqtb.cat_levels = r.raw_u16(NUM_CODES)?;
    eng.eqtb.math_code = r.raw_u16(NUM_CODES)?;
    eng.eqtb.math_levels = r.raw_u16(NUM_CODES)?;
    eng.eqtb.del_code = r.raw_i32(NUM_CODES)?;
    eng.eqtb.del_levels = r.raw_u16(NUM_CODES)?;
    eng.eqtb.lc_code = fixed::<NUM_CODES>(r)?.to_vec();
    eng.eqtb.lc_levels = r.raw_u16(NUM_CODES)?;
    eng.eqtb.sf_code = r.raw_u16(NUM_CODES)?;
    eng.eqtb.sf_levels = r.raw_u16(NUM_CODES)?;
    eng.eqtb.uc_code = fixed::<NUM_CODES>(r)?.to_vec();
    eng.eqtb.uc_levels = r.raw_u16(NUM_CODES)?;
    // style fonts
    for style in eng.eqtb.style_fonts.iter_mut() {
        for f in style.iter_mut() {
            *f = r.u16()?;
        }
    }
    for style in eng.eqtb.style_font_levels.iter_mut() {
        for f in style.iter_mut() {
            *f = r.u16()?;
        }
    }

    // fonts
    let n = r.count()?;
    for _ in 0..n {
        eng.eqtb.fonts.push(Rc::new(read_font(r)?));
        eng.eqtb.font_params.push(r.vec_i32()?);
        eng.eqtb.font_param_levels.push(r.vec_u16()?);
        eng.eqtb.hyphen_char.push(r.i32()?);
        eng.eqtb.hyphen_char_levels.push(r.u16()?);
        eng.eqtb.skew_char.push(r.i32()?);
        eng.eqtb.skew_char_levels.push(r.u16()?);
        eng.eqtb.font_cs.push(r.u32()?);
    }

    // hyphenation
    eng.hyphen_trie = read_trie(r)?;
    let n = r.count()?;
    for _ in 0..n {
        let name = r.str()?;
        let word = r.bytes()?;
        eng.hyphen_exceptions.push((name, word));
    }

    // paragraph shape
    let n = r.count()?;
    for _ in 0..n {
        let a = r.i32()?;
        let b = r.i32()?;
        eng.par_shape.push((a, b));
    }

    // boot-completed production state
    eng.format_done = true;
    eng.ini_mode = false;
    Ok(())
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("format {}", what))
}

fn read_macro(r: &mut R) -> io::Result<Macro> {
    let flags = r.u8()?;
    let num_params = r.u8()?;
    let prefix = r.toks()?;
    let n = r.count()?;
    let mut params = Vec::with_capacity(n);
    for _ in 0..n {
        params.push(r.toks()?);
    }
    let body = r.toks()?.into_iter().map(crate::token::Token::unfreeze).collect();

    Ok(Macro {
        num_params,
        prefix,
        params,
        body,
        long: flags & 1 != 0,
        outer: flags & 2 != 0,
        protected: flags & 4 != 0,
    })
}

fn read_font(r: &mut R) -> io::Result<Font> {
    let name = r.str()?;
    let tfm_name = r.str()?;
    let at_size = r.i32()?;
    let dsize = r.i32()?;
    let bc = r.u8()?;
    let ec = r.u8()?;
    let n = r.count()?;
    let mut chars = Vec::with_capacity(n);
    for _ in 0..n {
        chars.push(CharInfo {
            width: r.i32()?,
            height: r.i32()?,
            depth: r.i32()?,
            italic: r.i32()?,
            tag: r.u8()?,
            remainder: r.u8()?,
        });
    }
    let n = r.count()?;
    let mut lig_kern = Vec::with_capacity(n);
    for _ in 0..n {
        lig_kern.push(LigStep {
            skip: r.u8()?,
            next_char: r.u8()?,
            op: r.u8()?,
            rem: r.u8()?,
            stop: r.u8()? != 0,
        });
    }
    let kerns = r.vec_i32()?;
    let n = r.count()?;
    let mut ext = Vec::with_capacity(n);
    for _ in 0..n {
        ext.push(ExtRecipe {
            top: r.u8()?,
            mid: r.u8()?,
            bot: r.u8()?,
            rep: r.u8()?,
        });
    }
    let params = r.vec_i32()?;
    let hyphen_char = r.i32()?;
    let skew_char = r.i32()?;
    let type1_path = r.opt_str()?;
    let enc_name = r.opt_str()?;
    let map_fontname = r.opt_str()?;
    let encoding = if r.u8()? == 1 {
        let n = r.count()?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(r.str()?);
        }
        Some(v)
    } else {
        None
    };
    Ok(Font {
        name,
        tfm_name,
        at_size,
        dsize,
        chars,
        bc,
        ec,
        lig_kern,
        kerns,
        ext,
        params,
        hyphen_char,
        skew_char,
        type1_path,
        enc_name,
        map_fontname,
        encoding,
    })
}

fn read_trie(r: &mut R) -> io::Result<Trie> {
    let n = r.count()?;
    let mut trans = Vec::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    for _ in 0..n {
        let ne = r.count()?;
        let mut m = std::collections::HashMap::with_capacity(ne);
        for _ in 0..ne {
            let b = r.u8()?;
            m.insert(b, r.u32()? as usize);
        }
        trans.push(m);
        let nv = r.count()?;
        let mut vals = Vec::with_capacity(nv);
        for _ in 0..nv {
            vals.push((r.u64()? as usize, r.u8()?));
        }
        values.push(vals);
    }
    let mut exceptions = std::collections::HashMap::new();
    let n = r.count()?;
    for _ in 0..n {
        let k = r.bytes()?;
        let np = r.count()?;
        let mut pts = Vec::with_capacity(np);
        for _ in 0..np {
            pts.push(r.u64()? as usize);
        }
        exceptions.insert(k, pts);
    }
    Ok(Trie { trans, values, exceptions })
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim::{
        DimParam, GlueParam, IntParam, ToksParam, NUM_DIM_PARAMS, NUM_GLUE_PARAMS,
        NUM_INT_PARAMS, NUM_TOKS_PARAMS,
    };


    fn build_booted_engine() -> Engine {
        let mut eng = Engine::new(true);
        eng.init_primitives();
        eng.add_nullfont();
        eng
    }

    /// A boot state exercising every serialized structure.
    fn build_sample_engine() -> Engine {
        let mut eng = build_booted_engine();
        let id = |eng: &mut Engine, n: &[u8]| eng.cs.intern(n);

        let foo = id(&mut eng, b"foo");
        let bar = id(&mut eng, b"bar");
        let baz = id(&mut eng, b"baz");
        let ali = id(&mut eng, b"alias");
        let myfont = id(&mut eng, b"myfont");

        // macro: delimited + undelimited params, prefixes, all token kinds
        eng.eqtb.assign(
            foo,
            Equiv::Macro(Rc::new(Macro {
                num_params: 2,
                params: vec![vec![], vec![Token::other(b'-')]],
                prefix: vec![Token::letter(b'x'), Token::space()],
                body: vec![
                    Token::from_cs(bar),
                    Token::char(6, 1), // #1 param ref
                    Token::letter(b'q'),
                    Token(0xFFFF_FFFE), // PAR_END-style raw bits must roundtrip
                ],
                long: true,
                outer: false,
                protected: true,
            })),
            true,
        );
        eng.eqtb.assign(bar, Equiv::CharDef(65), true);
        eng.eqtb.assign(baz, Equiv::Prim(Prim::IfNum), true);
        eng.eqtb.assign(ali, Equiv::Alias(foo), true);
        // non-trivial save level (dumpable: inserted directly, no save-stack entry)
        let localcs = id(&mut eng, b"localcs");
        let undefcs = id(&mut eng, b"undefcs");
        eng.eqtb.restore_eq(localcs, Some(Equiv::CharDef(7)), 2);
        eng.eqtb.restore_eq(undefcs, None, 1);

        // registers & params
        eng.eqtb.assign_count(7, -123456, true);
        eng.eqtb.assign_dimen(3, 65536 * 12, true);
        eng.eqtb.assign_skip(
            5,
            Glue { width: 10, stretch: -3, shrink: 7, stretch_order: 2, shrink_order: 1 },
            true,
        );
        eng.eqtb.assign_muskip(1, Glue::fil(3, 5), true);
        eng.eqtb.assign_toks_reg(9, Rc::new(vec![Token::letter(b'z')]), true);
        eng.eqtb.assign_int_param(IntParam::Tolerance, 2500, true);
        eng.eqtb.assign_dim_param(DimParam::HSize, 123456789, true);
        eng.eqtb.assign_glue_param(GlueParam::ParSkip, Glue::new(42), true);
        eng.eqtb.assign_toks_param(ToksParam::EveryJob, Rc::new(vec![Token::other(b'X')]), true);

        // codes
        eng.eqtb.assign_cat(b'@', 11, true);
        eng.eqtb.assign_math_code(b'+', 0x0123, true);
        eng.eqtb.assign_del_code(b'.', 0x0abc_de1, true);
        eng.eqtb.assign_lc_code(b'E', 101, true);
        eng.eqtb.assign_sf_code(b'e', 999, true);
        eng.eqtb.assign_uc_code(b'e', 69, true);
        eng.eqtb.assign_style_font(1, 3, 2, true);

        // font
        let font = Font {
            name: "TenRoman".to_string(),
            tfm_name: "cmr10".to_string(),
            at_size: 655360,
            dsize: 655360,
            chars: vec![
                CharInfo { width: 100, height: 10, depth: 2, italic: 3, tag: 1, remainder: 7 },
                CharInfo { width: -5, height: 0, depth: 0, italic: 0, tag: 0, remainder: 0 },
            ],
            bc: 0,
            ec: 255,
            lig_kern: vec![LigStep { skip: 0, next_char: 1, op: 130, rem: 9, stop: true }],
            kerns: vec![-5, 17, i32::MIN],
            ext: vec![ExtRecipe { top: 1, mid: 2, bot: 3, rep: 4 }],
            params: vec![0, 33, 44],
            hyphen_char: b'-' as i32,
            skew_char: -1,
            type1_path: Some("pfb/cmr10.pfb".to_string()),
            enc_name: Some("ec".to_string()),
            map_fontname: None,
            encoding: Some(vec!["grave".to_string(), "".to_string()]),
        };
        eng.eqtb.fonts.push(Rc::new(font));
        eng.eqtb.font_params.push(vec![1, 2, 3]);
        eng.eqtb.font_param_levels.push(vec![1, 2, 2]);
        eng.eqtb.hyphen_char.push(b'-' as i32);
        eng.eqtb.hyphen_char_levels.push(1);
        eng.eqtb.skew_char.push(0);
        eng.eqtb.skew_char_levels.push(1);
        eng.eqtb.font_cs.push(myfont);
        eng.eqtb.assign(myfont, Equiv::FontRef(0), true);
        eng.eqtb.assign_font_param(0, 3, -77, true);

        // hyphenation
        eng.hyphen_trie.add_pattern(".ach4");
        eng.hyphen_trie.add_pattern("a1bc3cd");
        eng.hyphen_trie.add_pattern("4tion");
        eng.hyphen_trie.add_exception("ta-ble");
        eng.hyphen_exceptions.push(("lang-german".to_string(), b"ab-cd".to_vec()));
        eng.par_shape.push((3, 4));
        eng.par_shape.push((-1, i32::MAX));

        eng
    }

    fn fingerprint(eng: &Engine) -> String {
        let mut s = String::new();
        s.push_str(&format!("cs={} par={:?}\n", eng.cs.len(), eng.cs.name(eng.ids.par)));
        for id in eng.cs.all_ids() {
            let entry = eng
                .eqtb
                .eqs()
                .iter()
                .find(|(i, _, _)| *i == id)
                .map(|(_, e, l)| (format!("{:?}", e), *l));
            s.push_str(&format!(
                "  cs {:?} = {:?}\n",
                eng.cs.name(id),
                entry
            ));
        }
        let q = &eng.eqtb;
        s.push_str(&format!(
            "cur_level={} save={}\nint={:?}\nintlv={:?}\ndim={:?}\nglue={:?}\ntoks={:?}\n",
            q.cur_level,
            q.save_stack.len(),
            q.int_params,
            q.int_levels,
            q.dim_params,
            q.glue_params,
            q.tok_params,
        ));
        s.push_str(&format!(
            "count={:?}\ndimen={:?}\nskip={:?}\nmuskip={:?}\ntoksreg={:?}\nboxlv={:?}\n",
            q.count, q.dimen, q.skip, q.muskip, q.toks, q.box_levels
        ));
        s.push_str(&format!(
            "cat={:?}\nmath={:?}\ndel={:?}\nlc={:?}\nsf={:?}\nuc={:?}\nstyle={:?}\n",
            q.cat,
            q.math_code,
            q.del_code,
            q.lc_code,
            q.sf_code,
            q.uc_code,
            q.style_fonts
        ));
        for f in &q.fonts {
            // tfm::Font has no Debug; dump the identity fields
            s.push_str(&format!("font {} ({}) at {}\n", f.name, f.tfm_name, f.at_size));
        }
        s.push_str(&format!(
            "fparams={:?}\nflv={:?}\nhyc={:?}\nskwc={:?}\nfcs={:?}\n",
            q.font_params, q.font_param_levels, q.hyphen_char, q.skew_char, q.font_cs
        ));
        let mut trans_sorted = Vec::new();
        for node in &eng.hyphen_trie.trans {
            let mut edges: Vec<_> = node.iter().map(|(&b, &n)| (b, n)).collect();
            edges.sort_unstable();
            trans_sorted.push(edges);
        }
        s.push_str(&format!("trie_trans={:?}\ntrie_vals={:?}\n", trans_sorted, eng.hyphen_trie.values));
        s.push_str(&format!("hyexc={:?}\nparshape={:?}\n", eng.hyphen_exceptions, eng.par_shape));
        s
    }

    #[test]
    fn format_save_and_load_roundtrip() {
        let eng = build_sample_engine();
        let dir = std::env::temp_dir().join(format!("rustex-fmt-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pdflatex.fmt");

        let n = save_format(&eng, &path).expect("save");
        assert!(n > MAGIC.len(), "format suspiciously small: {}", n);

        let back = load_format(&path).expect("load");

        // byte-identical state (Debug-based deep comparison)
        let a = fingerprint(&eng);
        let b = fingerprint(&back);
        if a != b {
            for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
                if x != y {
                    panic!("state diverges at line {}:\n  orig: {}\n  load: {}", i, x, y);
                }
            }
            panic!("fingerprint length differs: {} vs {}", a.lines().count(), b.lines().count());
        }

        // spot checks on live values
        assert_eq!(back.eqtb.int_params[IntParam::Tolerance.idx() as usize], 2500);
        assert_eq!(back.eqtb.count[7], -123456);
        assert_eq!(back.eqtb.skip[5].stretch_order, 2);
        assert!(back.eqtb.get(back.cs.lookup(b"foo").unwrap()).is_some());
        // format-ready engine state
        assert!(back.format_done);
        assert!(!back.ini_mode);
        assert_eq!(back.eqtb.save_stack.len(), 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn primitive_codes_roundtrip_exhaustively() {
        for c in 0..=u16::MAX {
            if let Some(p) = Prim::from_code(c) {
                assert_eq!(p.code(), c, "code collision at {c:#x}");
            }
        }
        // every unit variant survives a re-code
        for c in 0..0x1000u16 {
            if let Some(p) = Prim::from_code(c) {
                assert_eq!(p.code(), c);
            }
        }
    }

    #[test]
    fn dump_refuses_group_state() {
        let mut eng = build_booted_engine();
        eng.eqtb.assign_cat(b'~', 10, false); // non-global at level 1: no save
        eng.eqtb.cur_level = 2;
        eng.eqtb.assign_cat(b'~', 10, false); // pushes a save item
        assert!(save_format(&eng, Path::new("/tmp/never.fmt")).is_err());

        // Boxes (even non-void) are dumpable: LaTeX's boot assigns box
        // registers, and tex.web's \dump only requires top-level state.
        let mut eng = build_booted_engine();
        eng.eqtb.assign_box(1, None, true);
        assert!(save_format(&eng, Path::new("/tmp/never.fmt")).is_ok());
        eng.eqtb.assign_box(
            2,
            Some(crate::boxes::Node::Rule { width: 10, height: 2, depth: 1 }),
            true,
        );
        assert!(check_dumpable(&eng).is_ok());
    }

    #[test]
    fn corrupt_and_foreign_files_are_rejected() {
        assert!(load_format_from(b"").is_err());
        assert!(load_format_from(b"not a format at all").is_err());
        let eng = build_sample_engine();
        let data = {
            let mut w = W::new();
            w.buf.extend_from_slice(MAGIC);
            w.u16(VERSION);
            w.u16(SEMANTICS);
            w.u16(0); // cur_font: no font selected
            w.u32(eng.cs.len() as u32);
            for id in eng.cs.all_ids() {
                w.bytes(eng.cs.name(id));
            }
            w.u32(0); // zero equivalents is still valid
            w.u16(1);
            for _ in 0..NUM_INT_PARAMS {
                w.i32(0);
            }
            for _ in 0..NUM_INT_PARAMS {
                w.u16(1);
            }
            for _ in 0..NUM_DIM_PARAMS {
                w.i32(0);
            }
            for _ in 0..NUM_DIM_PARAMS {
                w.u16(1);
            }
            for _ in 0..NUM_GLUE_PARAMS {
                w.glue(&Glue::zero());
            }
            for _ in 0..NUM_GLUE_PARAMS {
                w.u16(1);
            }
            for _ in 0..NUM_TOKS_PARAMS {
                w.toks(&[]);
            }
            for _ in 0..NUM_TOKS_PARAMS {
                w.u16(1);
            }
            // registers, codes, style fonts, fonts, trie, exceptions, parshape
            for _ in 0..NUM_REGISTERS {
                w.i32(0);
            }
            for _ in 0..NUM_REGISTERS {
                w.u16(1);
            }
            for _ in 0..NUM_REGISTERS {
                w.i32(0);
            }
            for _ in 0..NUM_REGISTERS {
                w.u16(1);
            }
            for _ in 0..NUM_REGISTERS {
                w.glue(&Glue::zero());
            }
            for _ in 0..NUM_REGISTERS {
                w.u16(1);
            }
            for _ in 0..NUM_REGISTERS {
                w.glue(&Glue::zero());
            }
            for _ in 0..NUM_REGISTERS {
                w.u16(1);
            }
            for _ in 0..NUM_REGISTERS {
                w.toks(&[]);
            }
            for _ in 0..NUM_REGISTERS {
                w.u16(1);
            }
            for _ in 0..NUM_REGISTERS {
                w.u16(1);
            }
            w.buf.extend_from_slice(&[0u8; NUM_CODES]);
            for _ in 0..NUM_CODES {
                w.u16(1);
            }
            for _ in 0..NUM_CODES {
                w.u16((7u16 << 8));
            }
            for _ in 0..NUM_CODES {
                w.u16(1);
            }
            for _ in 0..NUM_CODES {
                w.i32(-1);
            }
            for _ in 0..NUM_CODES {
                w.u16(1);
            }
            w.buf.extend_from_slice(&[0u8; NUM_CODES]);
            for _ in 0..NUM_CODES {
                w.u16(1);
            }
            for _ in 0..NUM_CODES {
                w.u16(1000);
            }
            for _ in 0..NUM_CODES {
                w.u16(1);
            }
            w.buf.extend_from_slice(&[0u8; NUM_CODES]);
            for _ in 0..NUM_CODES {
                w.u16(1);
            }
            for _ in 0..(3 * 256) {
                w.u16(0);
            }
            for _ in 0..(3 * 256) {
                w.u16(1);
            }
            w.u32(0); // no fonts
            w.u32(1);
            w.u8(0);
            w.u32(0);
            w.u32(0);
            w.u32(0);
            w.u32(0); // trie: 0 nodes; then 0 exceptions
            w.u32(0);
            w.u32(0);
            w.buf
        };
        // truncated blob must not panic
        for cut in [0, 5, 40, 120, data.len() / 2] {
            assert!(load_format_from(&data[..cut.min(data.len())]).is_err(), "cut={}", cut);
        }
        // complete minimal blob loads
        let eng2 = load_format_from(&data).expect("minimal format loads");
        assert!(eng2.format_done);
        // flipped version byte is invalid
        let mut bad = data.clone();
        bad[MAGIC.len()] = 0xFF;
        assert!(load_format_from(&bad).is_err());
    }

    #[test]
    fn semantics_version_mismatch_rejected() {
        let eng = build_sample_engine();
        let dir = std::env::temp_dir().join(format!("rustex-fmt-sem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pdflatex.fmt");
        save_format(&eng, &path).expect("save");

        // control: a current-semantics file loads through both paths
        assert!(load_format(&path).is_ok());
        let data = std::fs::read(&path).unwrap();
        assert!(load_format_from(&data).is_ok());

        // a fmt written with the previous semantics value must be refused
        // by both load paths with the actionable message
        let mut stale = data.clone();
        stale[MAGIC.len() + 2] = ((SEMANTICS - 1) & 0xFF) as u8;
        stale[MAGIC.len() + 3] = (((SEMANTICS - 1) >> 8) & 0xFF) as u8;
        let err = load_format(&path.with_extension("stale")).err().unwrap();
        assert!(err.contains("cannot read"), "sanity: {err}");
        std::fs::write(dir.join("stale.fmt"), &stale).unwrap();
        for err in [load_format(&dir.join("stale.fmt")).err().unwrap(), load_format_from(&stale).err().unwrap()] {
            assert_eq!(
                err,
                "format semantics mismatch (engine updated; delete the .fmt file)"
            );
        }

        // a flipped version byte is still reported as a version mismatch,
        // not silently passed to the state decoder
        let mut badver = data.clone();
        badver[MAGIC.len()] = 0xFF;
        assert_eq!(load_format_from(&badver).err().unwrap(), "format version mismatch");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn check_dumpable_guards() {
        let mut eng = build_booted_engine();
        assert!(check_dumpable(&eng).is_ok(), "fresh booted engine dumps");



        eng.marks[2].push(vec![Token::letter(b'x')]);
        let err = check_dumpable(&eng).unwrap_err();
        assert!(err.contains("mark class 2 holds 1 unresolved"), "got: {err}");
        eng.marks[2].clear();

        eng.pdf_page_attr = "/Creator (rustex)".into();
        let err = check_dumpable(&eng).unwrap_err();
        assert!(err.contains("\\pdfpageattr is set"), "got: {err}");
        eng.pdf_page_attr.clear();

        eng.pdf_pages_attr = "/CropBox [0 0 612 792]".into();
        let err = check_dumpable(&eng).unwrap_err();
        assert!(err.contains("\\pdfpagesattr is set"), "got: {err}");
        eng.pdf_pages_attr.clear();

        // the base file alone is dumpable; more than 3 sources is not
        eng.input.push_file("main.tex".to_string(), b"\\dump".to_vec());
        assert!(check_dumpable(&eng).is_ok(), "base file alone dumps");
        eng.input.push_file("child.tex".to_string(), b"x".to_vec());
        eng.input.push_file("child2.tex".to_string(), b"x".to_vec());
        eng.input.push_file("child3.tex".to_string(), b"x".to_vec());
        let err = check_dumpable(&eng).unwrap_err();
        assert!(err.contains("sources above the base file"), "got: {err}");
    }

    #[test]
    fn load_into_is_atomic_on_corrupt_file() {
        let eng = build_sample_engine();
        let dir = std::env::temp_dir().join(format!("rustex-fmt-into-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pdflatex.fmt");
        save_format(&eng, &path).expect("save");

        // happy path: full transplant into a caller engine
        let mut target = build_booted_engine();
        assert!(load_format_into(&path, &mut target).is_ok());
        assert!(target.format_done);
        assert_eq!(target.eqtb.fonts.len(), eng.eqtb.fonts.len());
        assert_eq!(target.eqtb.count[7], -123456);

        // corrupt payload (header intact): caller state must be untouched —
        // in particular no partially pushed font list shifting FontRef ids
        let mut data = std::fs::read(&path).unwrap();
        data.truncate(data.len() - 8);
        std::fs::write(&path, &data).unwrap();
        let mut target = build_booted_engine();
        let cs_before = target.cs.len();
        let fonts_before = target.eqtb.fonts.len();
        assert!(load_format_into(&path, &mut target).is_err(), "truncated fmt refused");
        assert_eq!(target.eqtb.fonts.len(), fonts_before, "no partial font transplant");
        assert_eq!(target.cs.len(), cs_before, "cs table not replaced on failure");
        assert!(!target.format_done);

        std::fs::remove_dir_all(&dir).ok();
    }
}
