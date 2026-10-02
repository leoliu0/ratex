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
//! The native wire format (all integers little-endian) is
//! `MAGIC(8) VER(u16)`, then sections in the order written by
//! [`save_format`]. List = `u32 len` + elements. Strings = byte list. A
//! production `.fmt` may wrap that whole byte stream in a zstd frame; loading
//! detects the frame from its magic bytes rather than its filename.

use std::io;
use std::path::Path;
use std::rc::Rc;

use crate::boxes::Glue;
use crate::engine::Engine;
use crate::eqtb::{Equiv, Macro};
use crate::hyphen::Trie;
use crate::prim::Prim;
use crate::tfm::{CharInfo, ExtRecipe, Font, LigStep};
use crate::token::{CsTable, Token};

const MAGIC: &[u8; 8] = b"RUSTEXFM";
const VERSION: u16 = 22;
/// A production format is currently about 8 MiB decoded. Keep corrupt or
/// unrelated external files from turning format probing into an unbounded
/// allocation while leaving ample room for future format growth.
const MAX_FORMAT_BYTES: usize = 128 * 1024 * 1024;
/// Bumped whenever serialized state changes meaning without changing the
/// wire layout (new engine invariants the loaded state must satisfy, e.g.
/// guards added to `check_dumpable` after the file was written). A `.fmt`
/// from a different engine generation must be rejected, not loaded.
pub const SEMANTICS: u16 = 8;

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
const TAG_LUA_CALL: u8 = 14;
const TAG_ATTRIBUTE_REG: u8 = 15;
const TAG_UMATHCHAR_DEF: u8 = 16;
/// Marks the trailing translation tables (xprn, xord, xchr).
const TCX_TRAILER: u8 = 0xC7;

// ---------------------------------------------------------------------------
// writer / reader primitives
// ---------------------------------------------------------------------------

struct W {
    buf: Vec<u8>,
}

impl W {
    fn new() -> W {
        W {
            buf: Vec::with_capacity(1 << 20),
        }
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
    /// LEB128 unsigned integer
    fn varint(&mut self, mut v: u32) {
        while v >= 0x80 {
            self.buf.push(v as u8 | 0x80);
            v >>= 7;
        }
        self.buf.push(v as u8);
    }
    /// A table whose tail repeats its last element (e.g. a font parameter
    /// array grown by a large `\fontdimen` index): total length, the fill
    /// element, then only the prefix before the run of fill elements.
    fn padded<T: PartialEq + Copy + Default>(&mut self, values: &[T], write: impl Fn(&mut W, &T)) {
        let fill = values.last().copied().unwrap_or_default();
        let explicit = values.iter().rposition(|v| *v != fill).map_or(0, |i| i + 1);
        self.u32(values.len() as u32);
        write(self, &fill);
        self.u32(explicit as u32);
        for v in &values[..explicit] {
            write(self, v);
        }
    }
    fn i32s(&mut self, values: &[i32]) {
        for v in values {
            self.i32(*v);
        }
    }
    fn u16s(&mut self, values: &[u16]) {
        for v in values {
            self.u16(*v);
        }
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
        self.u32(g.spec);
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
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "format truncated",
            ));
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "format corrupt length",
            ));
        }
        Ok(n)
    }
    /// LEB128 unsigned integer (at most five bytes)
    #[inline]
    fn varint(&mut self) -> io::Result<u32> {
        if let Some(&byte) = self.b.get(self.p) {
            if byte < 0x80 {
                self.p += 1;
                return Ok(u32::from(byte));
            }
        }
        let mut v = 0u32;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            let part = u32::from(byte & 0x7f);
            if shift == 28 && part > 0x0f {
                break;
            }
            v |= part << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(bad("varint out of range"))
    }
    /// a length-prefixed byte string, borrowed from the format buffer
    fn byte_slice(&mut self) -> io::Result<&'a [u8]> {
        let n = self.count()?;
        self.take(n)
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.byte_slice()?.to_vec())
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
        let b = self.take(18)?;
        let int = |i: usize| i32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let mut glue = Glue::spec(int(0), int(4), b[12], int(8), b[13]);
        glue.spec = u32::from_le_bytes(b[14..18].try_into().unwrap());
        Ok(glue)
    }
    fn toks(&mut self) -> io::Result<Vec<Token>> {
        let n = self.count()?;
        Ok(self.take_words(n)?.map(Token).collect())
    }
    /// `n` little-endian u32 words as one bounds check
    fn take_words(&mut self, n: usize) -> io::Result<impl Iterator<Item = u32> + 'a> {
        let bytes = self.take(n.checked_mul(4).ok_or_else(|| bad("corrupt length"))?)?;
        Ok(bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap())))
    }
    /// exactly `out.len()` raw i32s (fixed-size tables carry no length prefix)
    fn fill_i32(&mut self, out: &mut [i32]) -> io::Result<()> {
        let words = self.take_words(out.len())?;
        for (slot, word) in out.iter_mut().zip(words) {
            *slot = word as i32;
        }
        Ok(())
    }
    /// exactly `out.len()` raw u16s
    fn fill_u16(&mut self, out: &mut [u16]) -> io::Result<()> {
        let bytes = self.take(out.len() * 2)?;
        for (slot, c) in out.iter_mut().zip(bytes.chunks_exact(2)) {
            *slot = u16::from_le_bytes([c[0], c[1]]);
        }
        Ok(())
    }
    /// Inverse of `W::padded`.
    fn padded<T: Copy>(&mut self, read: impl Fn(&mut R<'a>) -> io::Result<T>) -> io::Result<Vec<T>> {
        const MAX_PADDED: usize = 1 << 20;
        let len = self.u32()? as usize;
        let fill = read(self)?;
        let explicit = self.count()?;
        if len > MAX_PADDED || explicit > len {
            return Err(bad("table length out of range"));
        }
        let mut values = Vec::with_capacity(len);
        for _ in 0..explicit {
            values.push(read(self)?);
        }
        values.resize(len, fill);
        Ok(values)
    }
    fn vec_i32(&mut self) -> io::Result<Vec<i32>> {
        let n = self.count()?;
        Ok(self.take_words(n)?.map(|w| w as i32).collect())
    }
    fn vec_u16(&mut self) -> io::Result<Vec<u16>> {
        let n = self.count()?;
        let mut v = vec![0; n];
        self.fill_u16(&mut v)?;
        Ok(v)
    }
}

// ---------------------------------------------------------------------------
// dumpability checks
// ---------------------------------------------------------------------------

/// Reasons a `\dump` would be refused (tex.web: \dump at top level only).
pub fn check_dumpable(eng: &Engine) -> Result<(), String> {
    if !eng.font_loader.native_fonts.is_empty() {
        return Err("cannot dump: native font programs are not serialized; select native fonts after loading the format".to_string());
    }
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
                crate::eqtb::SaveItem::AfterGroup(_) => {
                    n_ag += 1;
                }
                other => {
                    n_other += 1;
                    if types.len() < 24 {
                        types.push(format!("{:?}", std::mem::discriminant(other)));
                    }
                }
            }
        }
        // If there are only top-level aftergroup tokens (e.g. from \set@color in preamble)
        // and no open groups or modified registers, allow the format dump to proceed.
        if n_level != 0 || n_eq != 0 || n_other != 0 || eng.eqtb.cur_level != crate::eqtb::LEVEL_ONE
        {
            return Err(format!(
                "cannot dump: {} pending (level={} eq={} aftergroup={} other={} cur_level={} ss_open=[{}] types=[{}])",
                eng.eqtb.save_stack.len(),
                n_level,
                n_eq,
                n_ag,
                n_other,
                eng.eqtb.cur_level,
                eng.ss_trace
                    .iter()
                    .map(|(file, line)| format!("{}:{line}", file.split('/').last().unwrap_or("?")))
                    .collect::<Vec<_>>()
                    .join(" | "),
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

/// On-disk encoding for a serialized format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatEncoding {
    /// The native `RUSTEXFM` byte stream. This is retained for tools that
    /// inspect or exchange the wire format directly.
    Raw,
    /// A zstd frame containing the native byte stream.
    Zstd(i32),
}

/// Compression used for embedded and newly generated production formats.
/// Level 3 shrinks the normal LaTeX format by more than an order of magnitude
/// while adding only a small, bounded startup cost.
pub const DEFAULT_FORMAT_ZSTD_LEVEL: i32 = 3;

/// Serialize the engine's boot state to `path` (usually `pdflatex.fmt`).
///
/// For compatibility, a `.zst` suffix selects zstd and every other suffix
/// writes the raw wire format. New callers that need a specific encoding
/// should use [`save_format_with_encoding`].
/// Returns the number of bytes written.
pub fn save_format(eng: &Engine, path: &Path) -> Result<usize, String> {
    let encoding = if path.extension().and_then(|s| s.to_str()) == Some("zst") {
        FormatEncoding::Zstd(DEFAULT_FORMAT_ZSTD_LEVEL)
    } else {
        FormatEncoding::Raw
    };
    save_format_with_encoding(eng, path, encoding)
}

/// Serialize a zstd-compressed format regardless of the file suffix.
pub fn save_format_compressed(eng: &Engine, path: &Path) -> Result<usize, String> {
    save_format_with_encoding(eng, path, FormatEncoding::Zstd(DEFAULT_FORMAT_ZSTD_LEVEL))
}

/// Serialize a format with an explicit on-disk encoding.
pub fn save_format_with_encoding(
    eng: &Engine,
    path: &Path,
    encoding: FormatEncoding,
) -> Result<usize, String> {
    check_dumpable(eng)?;
    let mut w = W::new();
    w.buf.extend_from_slice(MAGIC);
    w.u16(VERSION);
    w.u16(SEMANTICS);
    w.u16(eng.eqtb.cur_font_val);
    w.u8(eng.engine_kind as u8);
    // control-sequence names (id = position); the low two bits of each
    // length mark frozen ids (1 = found by name, 2 = anonymous)
    w.u32(eng.cs.len() as u32);
    for id in eng.cs.all_ids() {
        let name = eng.cs.name(id);
        let kind = match eng.cs.frozen_kind(id) {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        };
        w.varint(((name.len() as u32) << 2) | kind);
        w.buf.extend_from_slice(name);
    }

    // equivalents
    let eqs = eng.eqtb.eqs();
    w.u32(eqs.len() as u32);
    for (id, equiv, level) in eqs {
        w.u32(id);
        w.u16(level);
        match equiv {
            None => {
                w.u8(TAG_NONE);
            }
            Some(Equiv::CountReg(v)) => {
                w.u8(TAG_COUNT_REG);
                w.u16(*v);
            }
            Some(Equiv::AttributeReg(v)) => {
                w.u8(TAG_ATTRIBUTE_REG);
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
            Some(Equiv::UMathCharDef(v)) => {
                w.u8(TAG_UMATHCHAR_DEF);
                w.i32(*v);
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
            Some(Equiv::LuaCall { slot, protected }) => {
                w.u8(TAG_LUA_CALL);
                w.u32(*slot);
                w.u8(u8::from(*protected));
            }
        }
    }
    w.u16(eng.eqtb.cur_level);

    // named parameter tables (fixed sizes: the version pins them)
    let q = &eng.eqtb;
    w.i32s(&q.int_params);
    w.u16s(&q.int_levels);
    w.i32s(&q.dim_params);
    w.u16s(&q.dim_levels);
    for g in &q.glue_params {
        w.glue(g);
    }
    w.u16s(&q.glue_levels);
    for t in &q.tok_params {
        w.toks(t);
    }
    w.u16s(&q.tok_levels);
    w.u32(q.next_spec);

    // registers: only the entries that differ from a fresh engine's
    // (zero/empty value at level one); nearly all 32768 are untouched
    write_sparse(&mut w, &q.count, &q.count_levels, |v| *v == 0, |w, v| w.i32(*v));
    write_sparse(&mut w, &q.dimen, &q.dimen_levels, |v| *v == 0, |w, v| w.i32(*v));
    write_sparse(&mut w, &q.skip, &q.skip_levels, is_zero_glue, |w, g| w.glue(g));
    write_sparse(&mut w, &q.muskip, &q.muskip_levels, is_zero_glue, |w, g| w.glue(g));
    write_sparse(&mut w, &q.toks, &q.toks_levels, |t| t.is_empty(), |w, t| w.toks(t));
    // box registers: dumpability guarantees all void
    write_sparse(&mut w, &q.box_levels, &q.box_levels, |_| true, |_, _| {});

    // code tables
    w.buf.extend_from_slice(&q.cat);
    w.u16s(&q.cat_levels);
    w.u16s(&q.math_code);
    w.u16s(&q.math_levels);
    w.i32s(&q.del_code);
    w.u16s(&q.del_levels);
    w.buf.extend_from_slice(&q.lc_code);
    w.u16s(&q.lc_levels);
    w.u16s(&q.sf_code);
    w.u16s(&q.sf_levels);
    w.buf.extend_from_slice(&q.uc_code);
    w.u16s(&q.uc_levels);

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
    // per-font expansion state; code tables are shared Rc pointers in the
    // engine (base font and its expanded variants alias the same table), so
    // the pool dedups them by address and the loader rebuilds the sharing.
    let mut expand_pool: crate::FxHashMap<usize, u32> = crate::FxHashMap::default();
    for (i, f) in eng.eqtb.fonts.iter().enumerate() {
        write_font(&mut w, f);
        let fp = eng
            .eqtb
            .font_params
            .get(i)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        w.padded(fp, |w, v| w.i32(*v));
        let fpl = eng
            .eqtb
            .font_param_levels
            .get(i)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        w.padded(fpl, |w, v| w.u16(*v));
        w.i32(eng.eqtb.hyphen_char.get(i).copied().unwrap_or(b'-' as i32));
        w.u16(eng.eqtb.hyphen_char_levels.get(i).copied().unwrap_or(0));
        w.i32(eng.eqtb.skew_char.get(i).copied().unwrap_or(-1));
        w.u16(eng.eqtb.skew_char_levels.get(i).copied().unwrap_or(0));
        w.u32(eng.eqtb.font_cs.get(i).copied().unwrap_or(0));
        write_font_expand(&mut w, eng.eqtb.expand.get(i), &mut expand_pool);
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
    for shape in &eng.penalty_shapes {
        w.u32(shape.len() as u32);
        for value in shape.iter() {
            w.i32(*value);
        }
    }
    let mut unicode_codes: Vec<_> = eng.eqtb.unicode_case_codes.iter().collect();
    unicode_codes.sort_unstable_by_key(|(key, _)| **key);
    w.u32(unicode_codes.len() as u32);
    for (&(uppercase, character), &(value, level)) in unicode_codes {
        w.u8(u8::from(uppercase));
        w.u32(character);
        w.u32(value);
        w.u16(level);
    }
    w.u32(eng.hyphen_tries.len() as u32);
    let mut langs: Vec<u8> = eng.hyphen_tries.keys().copied().collect();
    langs.sort_unstable();
    for lang in langs {
        w.u8(lang);
        write_trie(&mut w, &eng.hyphen_tries[&lang]);
    }
    w.u32(eng.hyphen_codes.len() as u32);
    let mut code_languages: Vec<u8> = eng.hyphen_codes.keys().copied().collect();
    code_languages.sort_unstable();
    for language in code_languages {
        w.u8(language);
        w.bytes(eng.hyphen_codes[&language].as_slice());
    }
    // LuaTeX bytecode registers and chunk names (llualib.c
    // dump_luac_registers)
    w.u32(eng.lua_bytecodes.len() as u32);
    for (slot, code) in &eng.lua_bytecodes {
        w.u32(*slot);
        w.bytes(code);
    }
    w.u32(eng.lua_names.len() as u32);
    for (slot, name) in &eng.lua_names {
        w.u16(*slot);
        w.bytes(name.as_bytes());
    }
    // tounicode.c dumptounicode: the \pdfglyphtounicode table
    w.u32(eng.pdf_backend.glyph_unicode.len() as u32);
    for (glyph, value) in &eng.pdf_backend.glyph_unicode {
        w.str(glyph);
        match value {
            crate::pdf_fonts::GlyphUnicode::Undef => w.u8(0),
            crate::pdf_fonts::GlyphUnicode::Code(code) => {
                w.u8(1);
                w.u32(*code);
            }
            crate::pdf_fonts::GlyphUnicode::Seq(seq) => {
                w.u8(2);
                w.str(seq);
            }
        }
    }
    // Sparse Unicode code tables, LuaTeX attributes and catcode tables
    // (luatex textcodes.c dumpcatcodes & co.). A dump happens at level one,
    // so no saved levels are written.
    write_code_map(&mut w, &q.unicode_cat_codes, |w, v| w.u8(v));
    write_code_map(&mut w, &q.unicode_math_codes, |w, v| w.u32(v));
    write_code_map(&mut w, &q.unicode_del_codes, |w, v| w.u64(v as u64));
    write_code_map(&mut w, &q.unicode_sf_codes, |w, v| w.u16(v));
    write_code_map(&mut w, &q.attributes, |w, v| w.i32(v));
    write_code_map(&mut w, &q.math_params, |w, v| w.i32(v));
    write_code_map(&mut w, &q.math_glue_params, |w, v| {
        for x in v {
            w.i32(x);
        }
    });
    write_code_map(&mut w, &q.lua_math_codes, |w, v| w.u64(v));
    write_code_map(&mut w, &q.lua_del_codes, |w, v| w.u64(v));
    w.i32(q.cat_table);
    let mut tables: Vec<_> = q.cat_tables.iter().collect();
    tables.sort_unstable_by_key(|(id, _)| **id);
    w.u32(tables.len() as u32);
    for (id, t) in tables {
        w.i32(*id);
        w.u8(u8::from(t.valid));
        w.buf.extend_from_slice(&t.cat);
        write_code_map(&mut w, &t.unicode, |w, v| w.u8(v));
    }
    // tex.web `format_ident`: the job id of every run that loads this format
    w.str(&eng.format_ident);

    // web2c dumps the TCX tables (xord, xchr, xprn) into the format. Older
    // dumps end before this trailer and were all built with cp227.tcx.
    w.u8(TCX_TRAILER);
    for bits in eng.xprn.chunks(8) {
        w.u8(bits.iter().enumerate().fold(0u8, |acc, (i, &on)| acc | (u8::from(on) << i)));
    }
    match &eng.tcx {
        None => w.u8(0),
        Some(tcx) => {
            w.u8(1);
            w.buf.extend_from_slice(&tcx.xord);
            w.buf.extend_from_slice(&tcx.xchr);
        }
    }

    let payload = match encoding {
        FormatEncoding::Raw => w.buf,
        FormatEncoding::Zstd(_level) => ruzstd::encoding::compress_to_vec(
            &w.buf[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        ),
    };
    tex_kpse::fs::write(path, &payload)
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok(payload.len())
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
    if m.has_param_refs {
        flags |= 8;
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

/// One lazily-allocated pdfTeX per-font code table (`init_font_base`).
type CodeTable = Rc<std::cell::RefCell<[i32; 256]>>;

/// Serialize one font's expansion state. Code tables are shared `Rc`s in
/// the engine (a base font and its auto-expanded variants alias the same
/// table, mirroring pdfTeX's `copy_expand_params`), so each distinct table
/// is written once into `pool` and referenced by index.
fn write_font_expand(
    w: &mut W,
    x: Option<&crate::eqtb::FontExpand>,
    pool: &mut crate::FxHashMap<usize, u32>,
) {
    match x {
        None => {
            w.u8(0);
            return;
        }
        Some(_) => w.u8(1),
    }
    let x = x.unwrap();
    // LuaTeX's limits of a TFM font travel in the slots of pdfTeX's clone
    // links (a LuaTeX font never has both)
    w.i32(if x.step != 0 { x.step } else { x.lua_step });
    w.u8(x.auto_expand as u8);
    w.u16(if x.stretch != 0 { x.stretch } else { x.lua_stretch as u16 });
    w.u16(if x.shrink != 0 { x.shrink } else { x.lua_shrink as u16 });
    w.u16(x.elink);
    w.u16(x.blink);
    w.i32(x.ratio);
    for t in [
        x.ef.as_ref(),
        x.lp.as_ref(),
        x.rp.as_ref(),
        x.kn_bs.as_ref(),
        x.st_bs.as_ref(),
        x.sh_bs.as_ref(),
        x.kn_bc.as_ref(),
        x.kn_ac.as_ref(),
    ] {
        write_code_table(w, t, pool);
    }
}

fn write_code_table(w: &mut W, t: Option<&CodeTable>, pool: &mut crate::FxHashMap<usize, u32>) {
    match t {
        None => w.u32(u32::MAX),
        Some(t) => {
            let addr = Rc::as_ptr(t) as usize;
            if let Some(idx) = pool.get(&addr) {
                w.u32(*idx);
                return;
            }
            let idx = pool.len() as u32;
            pool.insert(addr, idx);
            w.u32(idx);
            for v in t.borrow().iter() {
                w.i32(*v);
            }
        }
    }
}

/// Inverse of `write_font_expand`; `pool` holds the tables already read.
fn read_font_expand(
    r: &mut R,
    pool: &mut Vec<Option<CodeTable>>,
    lua: bool,
) -> io::Result<crate::eqtb::FontExpand> {
    let mut x = crate::eqtb::FontExpand::default();
    if r.u8()? == 0 {
        return Ok(x);
    }
    x.step = r.i32()?;
    x.auto_expand = r.u8()? != 0;
    x.stretch = r.u16()?;
    x.shrink = r.u16()?;
    x.elink = r.u16()?;
    x.blink = r.u16()?;
    x.ratio = r.i32()?;
    if lua {
        (x.lua_step, x.lua_stretch, x.lua_shrink) = (x.step, i32::from(x.stretch), i32::from(x.shrink));
        (x.step, x.stretch, x.shrink) = (0, 0, 0);
    }
    let slots = [
        &mut x.ef,
        &mut x.lp,
        &mut x.rp,
        &mut x.kn_bs,
        &mut x.st_bs,
        &mut x.sh_bs,
        &mut x.kn_bc,
        &mut x.kn_ac,
    ];
    for slot in slots {
        *slot = read_code_table(r, pool)?;
    }
    Ok(x)
}

fn read_code_table(r: &mut R, pool: &mut Vec<Option<CodeTable>>) -> io::Result<Option<CodeTable>> {
    let idx = r.u32()?;
    if idx == u32::MAX {
        return Ok(None);
    }
    if let Some(t) = pool.get(idx as usize) {
        return Ok(t.clone());
    }
    // tables are written in first-use order, so idx is always the next slot
    if idx as usize != pool.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "format expand pool out of order",
        ));
    }
    let mut v = [0i32; 256];
    for e in v.iter_mut() {
        *e = r.i32()?;
    }
    let t = Rc::new(std::cell::RefCell::new(v));
    pool.push(Some(t.clone()));
    Ok(Some(t))
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
            for g in enc.iter() {
                w.str(g);
            }
        }
        None => {
            w.u8(0);
        }
    }
}

/// Trie nodes in index order. Links are written as positive distances:
/// a child is always created after its parent and a sibling (the previous
/// head of the child chain) before the node, so `child - i` and
/// `i - sibling` are small; 0 means "no link".
fn write_trie(w: &mut W, t: &Trie) {
    use crate::hyphen::NO_LINK;
    w.u32(t.nodes.len() as u32);
    for (i, node) in t.nodes.iter().enumerate() {
        let i = i as u32;
        w.u8(node.byte);
        w.varint(if node.child == NO_LINK { 0 } else { node.child - i });
        w.varint(if node.sibling == NO_LINK { 0 } else { i - node.sibling });
        let mut chain = Vec::new();
        let mut link = node.value;
        while link != NO_LINK {
            chain.push(t.values[link as usize]);
            link = t.values[link as usize].next;
        }
        w.varint(chain.len() as u32);
        for value in chain {
            w.varint(value.pos);
            w.u8(value.value);
        }
    }
    let mut excs: Vec<(&Vec<u8>, &Vec<usize>)> = t.exceptions.iter().collect();
    excs.sort_unstable_by(|a, b| a.0.cmp(b.0));
    w.u32(excs.len() as u32);
    for (k, pts) in excs {
        w.varint(k.len() as u32);
        w.buf.extend_from_slice(k);
        w.varint(pts.len() as u32);
        for &p in pts {
            w.varint(p as u32);
        }
    }
}

fn is_zero_glue(g: &Glue) -> bool {
    g.width == 0 && g.stretch == 0 && g.shrink == 0 && g.stretch_order == 0 && g.shrink_order == 0
}

/// A sparse `character -> (value, level)` table, sorted for a stable dump.
fn write_code_map<T: Copy>(
    w: &mut W,
    map: &crate::FxHashMap<u32, (T, u16)>,
    write: impl Fn(&mut W, T),
) {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_unstable_by_key(|(key, _)| **key);
    w.u32(entries.len() as u32);
    for (&key, &(value, _)) in entries {
        w.u32(key);
        write(w, value);
    }
}

fn read_code_map<T>(
    r: &mut R,
    read: impl Fn(&mut R) -> io::Result<T>,
) -> io::Result<crate::FxHashMap<u32, (T, u16)>> {
    let n = r.count()?;
    let mut map = crate::FxHashMap::default();
    for _ in 0..n {
        let key = r.u32()?;
        let value = read(r)?;
        if map.insert(key, (value, crate::eqtb::LEVEL_ONE)).is_some() {
            return Err(bad("duplicate sparse code table entry"));
        }
    }
    Ok(map)
}

/// Write the entries of a register table that differ from a fresh engine
/// (`is_default` value at level one) as `(index, value, level)` triples.
fn write_sparse<T>(
    w: &mut W,
    values: &[T],
    levels: &[u16],
    is_default: impl Fn(&T) -> bool,
    write: impl Fn(&mut W, &T),
) {
    let changed: Vec<usize> = (0..values.len())
        .filter(|&i| !is_default(&values[i]) || levels[i] != crate::eqtb::LEVEL_ONE)
        .collect();
    w.u32(changed.len() as u32);
    for i in changed {
        w.u16(i as u16);
        write(w, &values[i]);
        w.u16(levels[i]);
    }
}

// ---------------------------------------------------------------------------
// load
// ---------------------------------------------------------------------------

/// Deserialize a format file into a ready-to-run engine (ini mode off,
/// `\dump`-completed state). Any I/O, magic, version, or truncation error
/// is reported so the caller can fall back to a full bootstrap.
pub fn load_format(path: &Path) -> Result<Engine, String> {
    let data = read_format_file(path)?;
    load_format_from(&data)
}

fn read_format_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = tex_kpse::fs::metadata(path)
        .map_err(|e| format!("cannot inspect {}: {e}", path.display()))?;
    if metadata.len() > MAX_FORMAT_BYTES as u64 {
        return Err(format!(
            "format file {} is too large ({} bytes; limit is {} bytes)",
            path.display(),
            metadata.len(),
            MAX_FORMAT_BYTES
        ));
    }
    tex_kpse::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Validate the header and return a reader positioned at the first payload
/// byte. Only the current wire version is accepted: the embedded formats
/// are regenerated with the engine, and an external `.fmt` from another
/// release is rejected so the caller falls back to the embedded one.
fn parse_header(data: &[u8]) -> Result<R<'_>, String> {
    if data.len() < MAGIC.len() + 4 || &data[..MAGIC.len()] != MAGIC {
        return Err("not a rustex format file".to_string());
    }
    let mut r = R::new(&data[MAGIC.len()..]);
    if r.u16().map_err(io_err)? != VERSION {
        return Err("format version mismatch".to_string());
    }
    if r.u16().map_err(io_err)? != SEMANTICS {
        return Err("format semantics mismatch (engine updated; delete the .fmt file)".to_string());
    }
    Ok(r)
}

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

pub fn load_format_from(data: &[u8]) -> Result<Engine, String> {
    if data.len() > MAX_FORMAT_BYTES {
        return Err(format!(
            "format input is too large ({} bytes; limit is {} bytes)",
            data.len(),
            MAX_FORMAT_BYTES
        ));
    }
    if data.len() >= 4 && data[..4] == ZSTD_MAGIC {
        let decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
            data,
            MAX_FORMAT_BYTES as u64,
        )
        .map_err(|e| format!("zstd decompression failed: {e}"))?;
        let mut decompressed = Vec::with_capacity(
            data.len()
                .saturating_mul(16)
                .min(MAX_FORMAT_BYTES)
                .min(16 * 1024 * 1024),
        );
        use std::io::Read;
        decoder
            .take(MAX_FORMAT_BYTES as u64 + 1)
            .read_to_end(&mut decompressed)
            .map_err(|e| format!("zstd decompression failed: {e}"))?;
        if decompressed.len() > MAX_FORMAT_BYTES {
            return Err(format!(
                "decompressed format exceeds the {} byte limit",
                MAX_FORMAT_BYTES
            ));
        }
        return load_format_uncompressed(&decompressed);
    }
    load_format_uncompressed(data)
}

fn load_format_uncompressed(data: &[u8]) -> Result<Engine, String> {
    let mut r = parse_header(data)?;
    let mut eng = Engine::new(false);
    eng.init_primitives();
    // Capture immutable primitive identities before replacing the format
    // state. A second full Engine would allocate another 32768-entry set
    // of every register table just to obtain these names.
    let primitives: Vec<_> = (0..eng.cs.len() as u32)
        .filter_map(|id| match eng.eqtb.get(id) {
            Some(Equiv::Prim(p)) => Some((eng.cs.name(id).to_vec(), *p)),
            _ => None,
        })
        .collect();
    let (lua_table, lua_backend) = eng.resolve_lua_primitives();
    load_state(&mut r, &mut eng).map_err(io_err)?;
    let lua = eng.engine_kind == crate::engine::EngineKind::LuaTeX;
    // Repair primitive aliases while preserving LaTeX macro redefinitions.
    // A LuaTeX format defines exactly the primitives it enabled.
    for (name, p) in primitives.into_iter().filter(|_| !lua) {
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
    for (id, font) in eng.eqtb.fonts.iter().enumerate() {
        eng.font_loader
            .restore_native_font(id as crate::tfm::FontId, font)?;
    }
    if eng.engine_kind == crate::engine::EngineKind::XeTeX {
        eng.init_xetex_primitives();
    } else if lua {
        eng.install_lua_primitive_table(lua_table, lua_backend);
        if eng.lua.is_none() {
            let lua_eng = crate::engine_lua::LuaEngine::new().map_err(|e| e)?;
            eng.lua = Some(Box::new(lua_eng));
        }
    }

    // \eTeXversion is engine identity, not format state.
    eng.eqtb.int_params[crate::prim::IntParam::EtxVersion.idx() as usize] = 2;

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
    let data = read_format_file(path)?;
    load_format_bytes_into(&data, eng)
}

pub fn load_format_bytes_into(data: &[u8], eng: &mut Engine) -> Result<(), String> {
    let scratch = load_format_from(data)?;
    // Resolve native declarations using the caller's project resolver before
    // committing any format state. Font files may be supplied by MemoryFs.
    for (id, font) in scratch.eqtb.fonts.iter().enumerate() {
        eng.font_loader
            .restore_native_font(id as crate::tfm::FontId, font)?;
    }
    // Full success only now: transplant the boot state while keeping the
    // caller's process-wide setup (font_loader, ids, out_dir, pdf_doc).
    eng.cs = scratch.cs;
    eng.primitive_names = scratch.primitive_names;
    eng.eqtb = scratch.eqtb;
    eng.hyphen_trie = scratch.hyphen_trie;
    eng.hyphen_tries = scratch.hyphen_tries;
    eng.hyphen_codes = scratch.hyphen_codes;
    eng.pdf_backend.glyph_unicode = scratch.pdf_backend.glyph_unicode;
    eng.xprn = scratch.xprn;
    eng.tcx = scratch.tcx;
    eng.format_ident = scratch.format_ident;
    eng.hyphen_exceptions = scratch.hyphen_exceptions;
    eng.par_shape = scratch.par_shape;
    eng.penalty_shapes = scratch.penalty_shapes;
    eng.primitive_table = scratch.primitive_table;
    eng.lua_primitives = scratch.lua_primitives;
    eng.penalty_shape_levels = scratch.penalty_shape_levels;
    eng.format_done = scratch.format_done;
    eng.ini_mode = scratch.ini_mode;
    // \interactionmode reflects the caller's runtime mode, not the dump's
    eng.eqtb.set_runtime_interaction_mode(eng.interaction_mode.number());
    eng.engine_kind = scratch.engine_kind;
    // luatex keeps the Lua state of the `--lua` script (and the callbacks it
    // registered) across the format load.
    if eng.lua.is_none() {
        eng.lua = scratch.lua;
    }
    Ok(())
}

fn io_err(e: io::Error) -> String {
    format!("format load: {}", e)
}

fn load_state(r: &mut R, eng: &mut Engine) -> io::Result<()> {
    eng.eqtb.cur_font_val = r.u16()?;
    eng.engine_kind = match r.u8()? {
        0 => crate::engine::EngineKind::PdfTeX,
        1 => crate::engine::EngineKind::XeTeX,
        2 => crate::engine::EngineKind::LuaTeX,
        _ => return Err(bad("invalid format engine kind")),
    };

    let n = r.count()?;
    let mut cs = CsTable::new();
    for _ in 0..n {
        let tagged = r.varint()?;
        let name = r.take((tagged >> 2) as usize)?;
        match tagged & 3 {
            0 => {
                cs.intern(name);
            }
            kind => {
                cs.push_frozen(name, kind == 1);
            }
        }
    }
    if cs.name(eng.ids.par) != b"par" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "format cs table incompatible with engine ids",
        ));
    }
    eng.cs = cs;
    eng.eqtb.clear_entries();
    // equivalents
    let n = r.count()?;
    for _ in 0..n {
        let id = r.u32()?;
        let level = r.u16()?;
        let tag = r.u8()?;
        let equiv = match tag {
            TAG_NONE => None,
            TAG_COUNT_REG => Some(Equiv::CountReg(r.u16()?)),
            TAG_ATTRIBUTE_REG => Some(Equiv::AttributeReg(r.u16()?)),
            TAG_DIMEN_REG => Some(Equiv::DimenReg(r.u16()?)),
            TAG_SKIP_REG => Some(Equiv::SkipReg(r.u16()?)),
            TAG_MUSKIP_REG => Some(Equiv::MuSkipReg(r.u16()?)),
            TAG_TOKS_REG => Some(Equiv::ToksReg(r.u16()?)),
            TAG_BOX_REG => Some(Equiv::BoxReg(r.u16()?)),
            TAG_CHAR_DEF => Some(Equiv::CharDef(r.u32()?)),
            TAG_CHAR_TOK => Some(Equiv::CharTok(r.u32()?)),
            TAG_MATHCHAR_DEF => Some(Equiv::MathCharDef(r.u16()?)),
            TAG_UMATHCHAR_DEF => Some(Equiv::UMathCharDef(r.i32()?)),
            TAG_FONT_REF => Some(Equiv::FontRef(r.u16()?)),
            TAG_ALIAS => Some(Equiv::Alias(r.u32()?)),
            TAG_PRIM => {
                let code = r.u16()?;
                match Prim::from_code(code) {
                    Some(p) => Some(Equiv::Prim(p)),
                    None => return Err(bad("has unknown primitive code")),
                }
            }
            TAG_MACRO => Some(Equiv::Macro(Rc::new(read_macro(r)?))),
            TAG_LUA_CALL => {
                let slot = r.u32()?;
                let protected = match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(bad("invalid lua call")),
                };
                Some(Equiv::LuaCall { slot, protected })
            }
            _ => return Err(bad("has unknown equivalent tag")),
        };
        eng.eqtb.restore_eq(id, equiv, level);
    }
    eng.eqtb.cur_level = r.u16()?;

    // named parameter tables, read in place over the fresh engine's tables
    // (their sizes are fixed by the engine and pinned by VERSION)
    let q = &mut eng.eqtb;
    r.fill_i32(&mut q.int_params)?;
    r.fill_u16(&mut q.int_levels)?;
    r.fill_i32(&mut q.dim_params)?;
    r.fill_u16(&mut q.dim_levels)?;
    for g in q.glue_params.iter_mut() {
        *g = r.glue()?;
    }
    r.fill_u16(&mut q.glue_levels)?;
    for t in q.tok_params.iter_mut() {
        *t = Rc::new(r.toks()?);
    }
    r.fill_u16(&mut q.tok_levels)?;
    q.next_spec = r.u32()?.max(Glue::FIRST_SPEC);

    // registers: the fresh engine holds the defaults; apply the changes
    read_sparse(r, &mut q.count, &mut q.count_levels, |r| r.i32())?;
    read_sparse(r, &mut q.dimen, &mut q.dimen_levels, |r| r.i32())?;
    read_sparse(r, &mut q.skip, &mut q.skip_levels, |r| r.glue())?;
    read_sparse(r, &mut q.muskip, &mut q.muskip_levels, |r| r.glue())?;
    read_sparse(r, &mut q.toks, &mut q.toks_levels, |r| Ok(Rc::new(r.toks()?)))?;
    let mut box_levels = std::mem::take(&mut q.box_levels);
    read_sparse(r, &mut vec![(); box_levels.len()], &mut box_levels, |_| Ok(()))?;
    q.box_levels = box_levels;

    // code tables
    q.cat.copy_from_slice(r.take(NUM_CODES)?);
    r.fill_u16(&mut q.cat_levels)?;
    r.fill_u16(&mut q.math_code)?;
    r.fill_u16(&mut q.math_levels)?;
    r.fill_i32(&mut q.del_code)?;
    r.fill_u16(&mut q.del_levels)?;
    q.lc_code.copy_from_slice(r.take(NUM_CODES)?);
    r.fill_u16(&mut q.lc_levels)?;
    r.fill_u16(&mut q.sf_code)?;
    r.fill_u16(&mut q.sf_levels)?;
    q.uc_code.copy_from_slice(r.take(NUM_CODES)?);
    r.fill_u16(&mut q.uc_levels)?;
    // style fonts
    for style in q.style_fonts.iter_mut() {
        r.fill_u16(style)?;
    }
    for style in q.style_font_levels.iter_mut() {
        r.fill_u16(style)?;
    }

    // fonts
    let n = r.count()?;
    let mut expand_pool: Vec<Option<CodeTable>> = Vec::new();
    for _ in 0..n {
        q.fonts.push(Rc::new(read_font(r)?));
        q.font_params.push(r.padded(|r| r.i32())?);
        q.font_param_levels.push(r.padded(|r| r.u16())?);
        q.hyphen_char.push(r.i32()?);
        q.hyphen_char_levels.push(r.u16()?);
        q.skew_char.push(r.i32()?);
        q.skew_char_levels.push(r.u16()?);
        q.font_cs.push(r.u32()?);
        q.expand.push(read_font_expand(r, &mut expand_pool, eng.engine_kind == crate::engine::EngineKind::LuaTeX)?);
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
    for shape in &mut eng.penalty_shapes {
        *shape = Rc::from(r.vec_i32()?);
    }
    let count = r.count()?;
    for _ in 0..count {
        let uppercase = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(bad("invalid Unicode case table selector")),
        };
        let character = r.u32()?;
        let value = r.u32()?;
        let level = r.u16()?;
        if char::from_u32(character).is_none()
            || char::from_u32(value).is_none()
            || level != crate::eqtb::LEVEL_ONE
            || eng
                .eqtb
                .unicode_case_codes
                .insert((uppercase, character), (value, level))
                .is_some()
        {
            return Err(bad("invalid Unicode case table entry"));
        }
    }
    let n_tries = r.count()?;
    if n_tries > 255 {
        return Err(bad("too many hyphenation languages"));
    }
    for _ in 0..n_tries {
        let lang = r.u8()?;
        let trie = read_trie(r)?;
        if lang == 0 || eng.hyphen_tries.insert(lang, trie).is_some() {
            return Err(bad("duplicate hyphenation language"));
        }
    }
    let n_codes = r.count()?;
    if n_codes > 256 {
        return Err(bad("too many hyphenation code tables"));
    }
    for _ in 0..n_codes {
        let language = r.u8()?;
        let codes: [u8; 256] = r
            .byte_slice()?
            .try_into()
            .map_err(|_| bad("invalid hyphenation code table"))?;
        if eng.hyphen_codes.insert(language, Box::new(codes)).is_some() {
            return Err(bad("duplicate hyphenation code language"));
        }
    }
    let n_bytecodes = r.count()?;
    for _ in 0..n_bytecodes {
        let slot = r.u32()?;
        let code = r.bytes()?;
        if eng.lua_bytecodes.insert(slot, code).is_some() {
            return Err(bad("duplicate lua bytecode register"));
        }
    }
    let n_names = r.count()?;
    for _ in 0..n_names {
        let slot = r.u16()?;
        let name = String::from_utf8(r.bytes()?).map_err(|_| bad("invalid lua chunk name"))?;
        if eng.lua_names.insert(slot, name).is_some() {
            return Err(bad("duplicate lua chunk name"));
        }
    }
    let n_glyphs = r.count()?;
    for _ in 0..n_glyphs {
        let glyph = r.str()?;
        let value = match r.u8()? {
            0 => crate::pdf_fonts::GlyphUnicode::Undef,
            1 => crate::pdf_fonts::GlyphUnicode::Code(r.u32()?),
            2 => crate::pdf_fonts::GlyphUnicode::Seq(r.str()?),
            _ => return Err(bad("invalid glyph-to-unicode entry")),
        };
        eng.pdf_backend.glyph_unicode.insert(glyph, value);
    }
    let q = &mut eng.eqtb;
    q.unicode_cat_codes = read_code_map(r, |r| r.u8())?;
    q.unicode_math_codes = read_code_map(r, |r| r.u32())?;
    q.unicode_del_codes = read_code_map(r, |r| Ok(r.u64()? as i64))?;
    q.unicode_sf_codes = read_code_map(r, |r| r.u16())?;
    q.attributes = read_code_map(r, |r| r.i32())?;
    q.refresh_cur_attr();
    q.math_params = read_code_map(r, |r| r.i32())?;
    q.math_glue_params = read_code_map(r, |r| {
        let mut v = [0i32; 6];
        for x in &mut v {
            *x = r.i32()?;
        }
        Ok(v)
    })?;
    q.lua_math_codes = read_code_map(r, |r| r.u64())?;
    q.lua_del_codes = read_code_map(r, |r| r.u64())?;
    q.cat_table = r.i32()?;
    let n_tables = r.count()?;
    for _ in 0..n_tables {
        let id = r.i32()?;
        let valid = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(bad("invalid catcode table")),
        };
        let cat = r.take(NUM_CODES)?.to_vec();
        let unicode = read_code_map(r, |r| r.u8())?;
        let table = crate::eqtb::CatCodeTable {
            cat,
            levels: vec![crate::eqtb::LEVEL_ONE; NUM_CODES],
            unicode,
            valid,
        };
        if !(0..=crate::eqtb::MAX_CAT_TABLE).contains(&id)
            || id == q.cat_table
            || q.cat_tables.insert(id, table).is_some()
        {
            return Err(bad("invalid catcode table"));
        }
    }
    eng.format_ident = r.str()?;
    // translation tables: the trailer, or cp227 for dumps without one
    if r.p == r.b.len() {
        eng.xprn = crate::tex_bytes::cp227_xprn();
        eng.tcx = None;
    } else {
        if r.u8()? != TCX_TRAILER {
            return Err(bad("has trailing data"));
        }
        let mut xprn = [false; 256];
        for (i, &byte) in r.take(32)?.iter().enumerate() {
            for bit in 0..8 {
                xprn[i * 8 + bit] = byte >> bit & 1 == 1;
            }
        }
        eng.xprn = xprn;
        eng.tcx = match r.u8()? {
            0 => None,
            1 => {
                let mut tcx = crate::tex_bytes::Tcx::identity();
                tcx.xord.copy_from_slice(r.take(256)?);
                tcx.xchr.copy_from_slice(r.take(256)?);
                tcx.xprn = xprn;
                Some(Box::new(tcx))
            }
            _ => return Err(bad("invalid translation table flag")),
        };
    }
    if r.p != r.b.len() {
        return Err(bad("has trailing data"));
    }

    // boot-completed production state
    eng.format_done = true;
    eng.ini_mode = false;
    Ok(())
}

/// Inverse of `write_sparse`: overwrite the listed register entries.
fn read_sparse<T>(
    r: &mut R,
    values: &mut [T],
    levels: &mut [u16],
    read: impl Fn(&mut R) -> io::Result<T>,
) -> io::Result<()> {
    let n = r.count()?;
    for _ in 0..n {
        let i = r.u16()? as usize;
        let value = read(r)?;
        let level = r.u16()?;
        if i >= values.len() {
            return Err(bad("register index out of range"));
        }
        values[i] = value;
        levels[i] = level;
    }
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
    let n = r.count()?;
    let body: Vec<Token> = r.take_words(n)?.map(|w| Token(w).unfreeze()).collect();

    let has_param_refs = (flags & 8 != 0)
        || (num_params > 0 && body.iter().any(|t| t.0 >= 0x4000_0000 && t.0 < 0x8000_0000));
    Ok(Macro {
        replacement: Default::default(),
        num_params,
        has_param_refs,
        prefix,
        params,
        body: body.into(),
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
    // parse_tfm: a first lig/kern step with skip 255 names the right
    // boundary character
    let bchar = lig_kern.first().filter(|step| step.skip == 255).map(|step| step.next_char);
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
        Some(v.into())
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
        bchar,
        type1_path,
        enc_name,
        map_fontname,
        encoding,
        lua: None,
    })
}

/// Inverse of `write_trie`.
fn read_trie(r: &mut R) -> io::Result<Trie> {
    use crate::hyphen::{TrieNode, TrieValue, NO_LINK};
    let n = r.count()?;
    if n == 0 {
        return Err(bad("trie has no root"));
    }
    let mut nodes = Vec::with_capacity(n);
    let mut values = Vec::new();
    for i in 0..n as u32 {
        let byte = r.u8()?;
        let child = match r.varint()? {
            0 => NO_LINK,
            d => i.checked_add(d).filter(|&c| (c as usize) < n).ok_or_else(|| bad("trie link"))?,
        };
        let sibling = match r.varint()? {
            0 => NO_LINK,
            d => i.checked_sub(d).ok_or_else(|| bad("trie link"))?,
        };
        let k = r.varint()?;
        let value = if k == 0 { NO_LINK } else { values.len() as u32 };
        for j in 0..k {
            let pos = r.varint()?;
            let v = r.u8()?;
            let next = if j + 1 == k { NO_LINK } else { values.len() as u32 + 1 };
            values.push(TrieValue { pos, value: v, next });
        }
        nodes.push(TrieNode {
            byte,
            child,
            sibling,
            value,
        });
    }
    let mut exceptions = crate::FxHashMap::default();
    let n = r.count()?;
    for _ in 0..n {
        let len = r.varint()? as usize;
        let k = r.take(len)?.to_vec();
        let np = r.varint()?;
        let mut pts = Vec::new();
        for _ in 0..np {
            pts.push(r.varint()? as usize);
        }
        exceptions.insert(k, pts);
    }
    Ok(Trie::from_parts(nodes, values, exceptions))
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim::{DimParam, GlueParam, IntParam, ToksParam};

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
                replacement: Default::default(),
                num_params: 2,
                has_param_refs: true,
                params: vec![vec![], vec![Token::other(b'-')]],
                prefix: vec![Token::letter(b'x'), Token::space()],
                body: vec![
                    Token::from_cs(bar),
                    Token::char(6, 1), // #1 param ref
                    Token::letter(b'q'),
                    Token(0xFFFF_FFFE), // PAR_END-style raw bits must roundtrip
                ]
                .into(),
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
        eng.eqtb.assign_count(32_767, 7654321, true);
        eng.eqtb.assign_dimen(3, 65536 * 12, true);
        eng.eqtb.assign_skip(
            5,
            Glue::spec(10, -3, 2, 7, 1),
            true,
        );
        eng.eqtb.assign_muskip(1, Glue::fil(3, 5), true);
        eng.eqtb
            .assign_toks_reg(9, Rc::new(vec![Token::letter(b'z')]), true);
        eng.eqtb.assign_int_param(IntParam::Tolerance, 2500, true);
        eng.eqtb.assign_dim_param(DimParam::HSize, 123456789, true);
        eng.eqtb
            .assign_glue_param(GlueParam::ParSkip, Glue::new(42), true);
        eng.eqtb
            .assign_toks_param(ToksParam::EveryJob, Rc::new(vec![Token::other(b'X')]), true);

        // codes
        eng.eqtb.assign_cat(b'@', 11, true);
        eng.eqtb.assign_math_code(b'+', 0x0123, true);
        eng.eqtb.assign_del_code(b'.', 0x00ab_cde1, true);
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
                CharInfo {
                    width: 100,
                    height: 10,
                    depth: 2,
                    italic: 3,
                    tag: 1,
                    remainder: 7,
                },
                CharInfo {
                    width: -5,
                    height: 0,
                    depth: 0,
                    italic: 0,
                    tag: 0,
                    remainder: 0,
                },
            ],
            bc: 0,
            ec: 255,
            lig_kern: vec![LigStep {
                skip: 0,
                next_char: 1,
                op: 130,
                rem: 9,
                stop: true,
            }],
            kerns: vec![-5, 17, i32::MIN],
            ext: vec![ExtRecipe {
                top: 1,
                mid: 2,
                bot: 3,
                rep: 4,
            }],
            params: vec![0, 33, 44],
            hyphen_char: b'-' as i32,
            skew_char: -1,
            bchar: None,
            type1_path: Some("pfb/cmr10.pfb".to_string()),
            enc_name: Some("ec".to_string()),
            map_fontname: None,
            encoding: Some(vec!["grave".to_string(), "".to_string()].into()),
            lua: None,
        };
        eng.eqtb.fonts.push(Rc::new(font));
        eng.eqtb.font_params.push(vec![1, 2, 3]);
        eng.eqtb.font_param_levels.push(vec![1, 2, 2]);
        eng.eqtb.hyphen_char.push(b'-' as i32);
        eng.eqtb.hyphen_char_levels.push(1);
        eng.eqtb.skew_char.push(0);
        eng.eqtb.skew_char_levels.push(1);
        eng.eqtb.font_cs.push(myfont);
        eng.eqtb.expand.push(Default::default());
        eng.eqtb.assign(myfont, Equiv::FontRef(0), true);
        eng.eqtb.assign_font_param(0, 3, -77, true);

        // hyphenation
        eng.hyphen_trie.add_pattern(".ach4");
        eng.hyphen_trie.add_pattern("a1bc3cd");
        eng.hyphen_trie.add_pattern("4tion");
        eng.hyphen_trie.add_exception("ta-ble");
        eng.hyphen_exceptions
            .push(("lang-german".to_string(), b"ab-cd".to_vec()));
        let mut language_codes = Box::new([0; 256]);
        language_codes[b'X' as usize] = b'x';
        eng.hyphen_codes.insert(7, language_codes);
        eng.par_shape.push((3, 4));
        eng.par_shape.push((-1, i32::MAX));
        eng.penalty_shapes[0] = Rc::from(vec![10, 20]);
        eng.penalty_shapes[1] = Rc::from(vec![30]);
        eng.penalty_shapes[2] = Rc::from(vec![40, 50, 60]);
        eng.penalty_shapes[3] = Rc::from(vec![70]);

        eng
    }

    fn fingerprint(eng: &Engine) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "cs={} par={:?}\n",
            eng.cs.len(),
            eng.cs.name(eng.ids.par)
        ));
        for id in eng.cs.all_ids() {
            let entry = eng
                .eqtb
                .eqs()
                .iter()
                .find(|(i, _, _)| *i == id)
                .map(|(_, e, l)| (format!("{:?}", e), *l));
            s.push_str(&format!("  cs {:?} = {:?}\n", eng.cs.name(id), entry));
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
            q.cat, q.math_code, q.del_code, q.lc_code, q.sf_code, q.uc_code, q.style_fonts
        ));
        for f in &q.fonts {
            // tfm::Font has no Debug; dump the identity fields
            s.push_str(&format!(
                "font {} ({}) at {}\n",
                f.name, f.tfm_name, f.at_size
            ));
        }
        s.push_str(&format!(
            "fparams={:?}\nflv={:?}\nhyc={:?}\nskwc={:?}\nfcs={:?}\n",
            q.font_params, q.font_param_levels, q.hyphen_char, q.skew_char, q.font_cs
        ));
        let mut tries_sorted: Vec<_> = eng.hyphen_tries.keys().copied().collect();
        tries_sorted.sort_unstable();
        let mut codes_sorted: Vec<_> = eng
            .hyphen_codes
            .iter()
            .map(|(&language, codes)| (language, codes.to_vec()))
            .collect();
        codes_sorted.sort_unstable_by_key(|(language, _)| *language);
        s.push_str(&format!(
            "trie_trans={:?}\ntrie_vals={:?}\nhyphen_tries={:?}\nhyphen_codes={:?}\n",
            trie_shape(&eng.hyphen_trie), "", tries_sorted, codes_sorted
        ));
        s.push_str(&format!(
            "hyexc={:?}\nparshape={:?}\npenaltyshapes={:?}\n",
            eng.hyphen_exceptions, eng.par_shape, eng.penalty_shapes
        ));
        s
    }

    /// Node links plus each node's value chain (value-pool indices are an
    /// implementation detail of insertion order).
    fn trie_shape(t: &Trie) -> Vec<(u8, u32, u32, Vec<(u32, u8)>)> {
        t.nodes
            .iter()
            .map(|n| {
                let mut chain = Vec::new();
                let mut link = n.value;
                while link != crate::hyphen::NO_LINK {
                    let v = t.values[link as usize];
                    chain.push((v.pos, v.value));
                    link = v.next;
                }
                (n.byte, n.child, n.sibling, chain)
            })
            .collect()
    }

    #[test]
    fn preloaded_font_keeps_its_boundary_character() {
        let mut eng = build_sample_engine();
        let index = eng.eqtb.fonts.len() - 1;
        let mut font = (*eng.eqtb.fonts[index]).clone();
        // tfm.rs: a first lig/kern step with skip 255 declares bchar
        font.lig_kern.insert(
            0,
            LigStep {
                skip: 255,
                next_char: 0x2A,
                op: 0,
                rem: 0,
                stop: false,
            },
        );
        font.bchar = Some(0x2A);
        eng.eqtb.fonts[index] = Rc::new(font);
        let dir = std::env::temp_dir().join(format!("rustex-fmt-bchar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pdflatex.fmt");
        save_format(&eng, &path).expect("save");
        let back = load_format(&path).expect("load");
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(back.eqtb.fonts[index].bchar, Some(0x2A));
        assert_eq!(back.eqtb.fonts[0].bchar, eng.eqtb.fonts[0].bchar);
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
                    panic!(
                        "state diverges at line {}:\n  orig: {}\n  load: {}",
                        i, x, y
                    );
                }
            }
            panic!(
                "fingerprint length differs: {} vs {}",
                a.lines().count(),
                b.lines().count()
            );
        }

        // spot checks on live values
        assert_eq!(
            back.eqtb.int_params[IntParam::Tolerance.idx() as usize],
            2500
        );
        assert_eq!(back.eqtb.count[7], -123456);
        assert_eq!(back.eqtb.count[32_767], 7654321);
        assert_eq!(back.eqtb.skip[5].stretch_order, 2);
        assert!(back.eqtb.get(back.cs.lookup(b"foo").unwrap()).is_some());
        // format-ready engine state
        assert!(back.format_done);
        assert!(!back.ini_mode);
        assert_eq!(back.eqtb.save_stack.len(), 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn language_patterns_and_byte_exceptions_survive_format_loading() {
        let mut eng = build_booted_engine();
        eng.hyphen_trie.add_pattern("ab1cd");
        eng.trie_for_language_mut(0).add_exception("a-bcd");
        eng.trie_for_language_mut(7).add_exception("abc-d");
        let word = [0xe0, 0xe1, 0xe2, 0xe3];
        eng.trie_for_language_mut(1)
            .add_pattern_bytes(&[0xe0, b'1', 0xe1, 0xe2, 0xe3]);
        let path = std::env::temp_dir().join(format!(
            "ratex-language-roundtrip-{}.fmt",
            std::process::id()
        ));
        save_format(&eng, &path).unwrap();
        let mut loaded = build_booted_engine();
        load_format_into(&path, &mut loaded).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            loaded
                .trie_for_language(0)
                .unwrap()
                .hyphenate(b"abcd", 1, 1),
            vec![1]
        );
        assert_eq!(
            loaded
                .trie_for_language(7)
                .unwrap()
                .hyphenate(b"abcd", 1, 1),
            vec![3]
        );
        assert_eq!(
            loaded.trie_for_language(1).unwrap().hyphenate(&word, 1, 1),
            vec![1]
        );
        assert!(loaded
            .trie_for_language(0)
            .unwrap()
            .hyphenate(&word, 1, 1)
            .is_empty());
        assert!(loaded.trie_for_language(8).is_none());
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
        // a different value: reassigning the held value saves nothing in e-TeX
        eng.eqtb.assign_cat(b'~', 11, false); // pushes a save item
        let tmp = std::env::temp_dir().join(format!("never-{}.fmt", std::process::id()));
        assert!(save_format(&eng, &tmp).is_err());

        // Boxes (even non-void) are dumpable: LaTeX's boot assigns box
        // registers, and tex.web's \dump only requires top-level state.
        let mut eng = build_booted_engine();
        eng.eqtb.assign_box(1, None, true);
        assert!(save_format(&eng, &tmp).is_ok());
        let _ = std::fs::remove_file(&tmp);
        eng.eqtb.assign_box(
            2,
            Some(crate::boxes::Node::Rule {
                width: 10,
                height: 2,
                depth: 1, subtype: crate::boxes::RULE_NORMAL, index: 0, attr: crate::boxes::Attr::NONE,
            }),
            true,
        );
        assert!(check_dumpable(&eng).is_ok());
    }

    #[test]
    fn native_font_dump_preserves_existing_format() {
        let mut eng = build_booted_engine();
        eng.input.push_file(
            "native-format.tex".into(),
            br#"\font\native="ratex:{Latin Modern Roman}" at 10pt\end"#.to_vec(),
        );
        eng.run();
        assert_eq!(eng.error_count, 0, "{:?}", eng.diagnostics);
        let path = std::env::temp_dir().join(format!("native-font-{}.fmt", std::process::id()));
        std::fs::write(&path, b"existing format").unwrap();
        assert!(save_format(&eng, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing format");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn corrupt_and_foreign_files_are_rejected() {
        assert!(load_format_from(b"").is_err());
        assert!(load_format_from(b"not a format at all").is_err());
        let eng = build_sample_engine();
        let dir = std::env::temp_dir().join(format!("rustex-fmt-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pdflatex.fmt");
        save_format(&eng, &path).expect("save");
        let data = std::fs::read(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert!(load_format_from(&data).expect("complete format loads").format_done);
        // every truncation is an error, never a panic; a dump cut exactly
        // before the translation-table trailer (34 bytes) is a valid older dump
        for cut in (0..data.len()).step_by(97).chain([data.len() - 1]) {
            if cut == data.len() - 34 {
                continue;
            }
            assert!(load_format_from(&data[..cut]).is_err(), "cut={cut}");
        }
        let mut trailing = data.clone();
        trailing.push(0);
        assert!(load_format_from(&trailing).is_err());
        // a format from an older wire version is refused, not misread
        let mut older = data.clone();
        older[MAGIC.len()..MAGIC.len() + 2].copy_from_slice(&(VERSION - 1).to_le_bytes());
        assert_eq!(load_format_from(&older).err().unwrap(), "format version mismatch");
    }

    #[test]
    fn character_tables_survive_a_dump_and_older_dumps_load_as_cp227() {
        let mut eng = build_sample_engine();
        eng.set_tcx(crate::tex_bytes::Tcx::builtin("cp8bit.tcx").unwrap());
        let dir = std::env::temp_dir().join(format!("rustex-fmt-tcx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.fmt");
        save_format(&eng, &path).expect("save");
        let data = std::fs::read(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let loaded = load_format_from(&data).unwrap();
        assert_eq!(loaded.xprn, crate::tex_bytes::Tcx::builtin("cp8bit.tcx").unwrap().xprn);
        // without the trailer, as written before the tables were dumped
        let legacy = load_format_from(&data[..data.len() - 34]).unwrap();
        assert_eq!(legacy.xprn, crate::tex_bytes::cp227_xprn());
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
        assert!(err.contains("cannot inspect"), "sanity: {err}");
        std::fs::write(dir.join("stale.fmt"), &stale).unwrap();
        for err in [
            load_format(&dir.join("stale.fmt")).err().unwrap(),
            load_format_from(&stale).err().unwrap(),
        ] {
            assert_eq!(
                err,
                "format semantics mismatch (engine updated; delete the .fmt file)"
            );
        }

        // a flipped version byte is still reported as a version mismatch,
        // not silently passed to the state decoder
        let mut badver = data.clone();
        badver[MAGIC.len()] = 0xFF;
        assert_eq!(
            load_format_from(&badver).err().unwrap(),
            "format version mismatch"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn check_dumpable_guards() {
        let mut eng = build_booted_engine();
        assert!(check_dumpable(&eng).is_ok(), "fresh booted engine dumps");

        eng.marks[2].push(vec![Token::letter(b'x')]);
        let err = check_dumpable(&eng).unwrap_err();
        assert!(
            err.contains("mark class 2 holds 1 unresolved"),
            "got: {err}"
        );
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
        eng.input
            .push_file("main.tex".to_string(), b"\\dump".to_vec());
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
        assert_eq!(target.eqtb.count[32_767], 7654321);

        // corrupt payload (header intact): caller state must be untouched —
        // in particular no partially pushed font list shifting FontRef ids
        let mut data = std::fs::read(&path).unwrap();
        data.truncate(data.len() - 8);
        std::fs::write(&path, &data).unwrap();
        let mut target = build_booted_engine();
        let cs_before = target.cs.len();
        let fonts_before = target.eqtb.fonts.len();
        assert!(
            load_format_into(&path, &mut target).is_err(),
            "truncated fmt refused"
        );
        assert_eq!(
            target.eqtb.fonts.len(),
            fonts_before,
            "no partial font transplant"
        );
        assert_eq!(
            target.cs.len(),
            cs_before,
            "cs table not replaced on failure"
        );
        assert!(!target.format_done);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn format_zstd_roundtrip() {
        let eng = build_sample_engine();
        let dir = std::env::temp_dir().join(format!("rustex-fmt-zstd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.fmt.zst");
        let n = save_format(&eng, &path).expect("save format with zstd");
        assert!(n > 0);
        let loaded = load_format(&path).expect("load format from zstd");
        assert_eq!(loaded.cs.len(), eng.cs.len());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_compressed_format_does_not_depend_on_suffix() {
        let eng = build_sample_engine();
        let dir =
            std::env::temp_dir().join(format!("rustex-fmt-explicit-zstd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pdflatex.fmt");
        save_format_compressed(&eng, &path).expect("save compressed .fmt");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], &ZSTD_MAGIC);
        let loaded = load_format(&path).expect("load compressed .fmt");
        assert_eq!(loaded.cs.len(), eng.cs.len());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn external_format_size_is_bounded_before_reading() {
        let dir =
            std::env::temp_dir().join(format!("rustex-fmt-size-limit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("oversized.fmt");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FORMAT_BYTES as u64 + 1).unwrap();

        let error = match load_format(&path) {
            Err(error) => error,
            Ok(_) => panic!("oversized format must be refused"),
        };
        assert!(error.contains("too large"), "got: {error}");
        assert!(
            error.contains(&MAX_FORMAT_BYTES.to_string()),
            "got: {error}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
