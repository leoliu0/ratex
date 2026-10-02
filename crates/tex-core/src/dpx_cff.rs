//! CID-keyed CFF subsets written like xdvipdfmx (cidtype0.c, cff.c, cff_dict.c, cs_type2.c).
//!
//! This is a line-by-line port of the parts of dvipdfmx that embed an OpenType/CFF font as a
//! `/CIDFontType0C` stream:
//!
//! * `CIDFont_type0_t1cdofont` for name-keyed CFF (CID == original glyph id, one FD, ROS
//!   Adobe/Identity/0),
//! * `CIDFont_type0_dofont` for CID-keyed CFF (CID kept, per-glyph FD kept, FDSelect format 3),
//! * `write_fontfile` (layout), `cs_copy_charstring` (subroutine inlining, stem counting, operand
//!   re-encoding), `cff_dict_pack` (5-byte offsets, `%.13g` reals) and the string INDEX handling
//!   (`cff_add_string`/`cff_update_string`).
//!
//! Behaviour that looks odd is on purpose: it is what xdvipdfmx 20260113 writes (for instance
//! the FDArray `FontName` of name-keyed fonts is `fontname + 7`, the Name INDEX holds the
//! original, untagged CFF name, and charstring widths are kept in the charstrings).

use std::collections::{BTreeMap, BTreeSet};

type R<T> = Result<T, String>;

const CFF_STDSTR_MAX: usize = 391;
/// `CS_STR_LEN_MAX`: longest charstring dvipdfmx accepts.
const CS_STR_LEN_MAX: usize = 65536;
const CS_SUBR_NEST_MAX: usize = 10;
const CS_ARG_STACK_MAX: usize = 48;
const CS_TRANS_ARRAY_MAX: usize = 32;
const CFF_DICT_STACK_LIMIT: usize = 64;

// ---------------------------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------------------------

/// A CID-keyed CFF font program as embedded by xdvipdfmx.
pub struct CffSubset {
    /// the CFF font program (FontFile3 /Subtype /CIDFontType0C payload before Flate), always
    /// CID-keyed, CID == original GID (name-keyed input) or original CID (CID-keyed input)
    pub data: Vec<u8>,
    /// largest used CID
    pub last_cid: u16,
    /// /CIDSet payload: bit (7 - cid%8) of byte cid/8, length last_cid/8+1, .notdef (cid 0) set
    pub cidset: Vec<u8>,
    /// /StemV from the Private DICT StdVW if present (name-keyed path only:
    /// CIDFont_type0_t1cdofont adds it)
    pub std_vw: Option<f64>,
}

/// CFF facts the descriptor/metrics code needs.
pub struct CffInfo {
    pub num_glyphs: u16,
    pub is_cid: bool,
    /// CID-keyed only: CID -> GID through the font charset (`cff_charsets_lookup`); CID 0 maps
    /// to GID 0 and CIDs not in the charset are absent.
    pub cid_to_gid: Option<BTreeMap<u16, u16>>,
    /// name-keyed only: charset glyph name per gid (`cff_get_glyphname`); empty for CID-keyed
    /// fonts. `None` where the SID is not resolvable (or the charset is a predefined Expert
    /// one).
    pub glyph_names: Vec<Option<String>>,
    /// `CIDCount` of the top DICT, if present.
    pub cid_count: Option<u32>,
    /// The CFF Name INDEX entry (`cff_get_name`), i.e. the PostScript name dvipdfmx uses for
    /// BaseFont/FontName (after the `XXXXXX+` tag).
    pub font_name: String,
}

/// Embed `used` (CIDs; gid 0 is always added) of the OpenType/CFF font `otf` (sfnt `OTTO` or a
/// TTC; `face_index` selects the TTC face) the way xdvipdfmx's `CIDFont_type0_t1cdofont` /
/// `CIDFont_type0_dofont` do.
///
/// `tagged_name` is the `XXXXXX+Name` BaseFont name. dvipdfmx writes the *untagged* CFF name
/// into the Name INDEX (and `name + 7` as FDArray FontName for name-keyed fonts), taking it
/// from the font itself; this function therefore checks that `tagged_name` is `tag+<CFF name>`
/// and fails otherwise, since the PDF font descriptor would not match dvipdfmx's.
pub fn subset_cid_cff(
    otf: &[u8],
    face_index: u32,
    used: &BTreeSet<u16>,
    tagged_name: &str,
) -> Result<CffSubset, String> {
    let d = cff_data(otf, face_index)?;
    let cff = CffFont::open(d)?;
    let name = String::from_utf8_lossy(&cff.name).into_owned();
    match tagged_name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && rest == name => {}
        _ => {
            return Err(format!(
                "tagged font name {tagged_name:?} is not a six-letter tag plus the CFF name {name:?}"
            ))
        }
    }
    if cff.is_cid {
        cid_dofont(&cff, used)
    } else {
        t1c_dofont(&cff, used)
    }
}

/// Parse the CFF table of `otf` and report the facts the descriptor/metrics code needs.
pub fn cff_info(otf: &[u8], face_index: u32) -> Result<CffInfo, String> {
    let d = cff_data(otf, face_index)?;
    let cff = CffFont::open(d)?;
    let cid_count = if cff.topdict.known("CIDCount") {
        Some(cff.topdict.get("CIDCount", 0)? as u32)
    } else {
        None
    };
    let mut info = CffInfo {
        num_glyphs: cff.num_glyphs,
        is_cid: cff.is_cid,
        cid_to_gid: None,
        glyph_names: Vec::new(),
        cid_count,
        font_name: String::from_utf8_lossy(&cff.name).into_owned(),
    };
    if cff.is_cid {
        let cs = cff.read_charsets()?;
        let cs = cs.ok_or("Predefined CFF charsets not supported yet")?;
        info.cid_to_gid = Some(cs.cid_to_gid());
    } else {
        let strings = Strings::new(&cff);
        let list: Option<Vec<u16>> = match cff.read_charsets()? {
            Some(cs) => Some(cs.glyphs),
            // ISOAdobe: SID == GID for 1..=228.
            None if cff.charset_predefined() == Some(0) => Some((1..=228u16).collect()),
            None => None,
        };
        info.glyph_names = (0..cff.num_glyphs as usize)
            .map(|gid| {
                if gid == 0 {
                    return Some(".notdef".to_string());
                }
                let sid = *list.as_ref()?.get(gid - 1)?;
                strings
                    .get(sid)
                    .map(|s| String::from_utf8_lossy(&s).into_owned())
            })
            .collect();
    }
    Ok(info)
}

// ---------------------------------------------------------------------------------------------
// sfnt wrapper (sfnt.c / ttc_read_offset)
// ---------------------------------------------------------------------------------------------

fn be16(d: &[u8], pos: usize) -> R<u16> {
    match d.get(pos..pos + 2) {
        Some(b) => Ok(u16::from_be_bytes([b[0], b[1]])),
        None => Err("File ended prematurely".into()),
    }
}

fn be32(d: &[u8], pos: usize) -> R<u32> {
    match d.get(pos..pos + 4) {
        Some(b) => Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
        None => Err("File ended prematurely".into()),
    }
}

fn byte(d: &[u8], pos: usize) -> R<u8> {
    d.get(pos).copied().ok_or_else(|| "File ended prematurely".to_string())
}

/// `sfnt_open` + `ttc_read_offset` + `sfnt_read_table_directory` + `sfnt_find_table_pos("CFF ")`;
/// returns the file from the start of the `CFF ` table to the end of the file (the C code reads
/// the CFF through a FILE stream and is not bounded by the table length either).
fn cff_data(otf: &[u8], face_index: u32) -> R<&[u8]> {
    const SFNT_TRUETYPE: u32 = 0x0001_0000;
    const SFNT_MAC_TRUE: u32 = 0x7472_7565;
    const SFNT_POSTSCRIPT: u32 = 0x4f54_544f;
    const SFNT_TTC: u32 = 0x7474_6366;
    let tag = be32(otf, 0).map_err(|_| "Failed to read font file".to_string())?;
    let mut dir = 0usize;
    match tag {
        SFNT_POSTSCRIPT => {}
        SFNT_TTC => {
            let num_dirs = be32(otf, 8)?;
            if face_index >= num_dirs {
                return Err("Invalid TTC index number".into());
            }
            dir = be32(otf, 12 + face_index as usize * 4)? as usize;
        }
        SFNT_TRUETYPE | SFNT_MAC_TRUE => return Err("Not a CFF/OpenType font".into()),
        _ => return Err("Not a CFF/OpenType font".into()),
    }
    let num_tables = be16(otf, dir + 4)? as usize;
    let mut cff_off = 0usize;
    for i in 0..num_tables {
        let rec = dir + 12 + i * 16;
        let t = otf.get(rec..rec + 16).ok_or("File ended prematurely")?;
        if &t[0..4] == b"CFF " {
            cff_off = u32::from_be_bytes([t[8], t[9], t[10], t[11]]) as usize;
            break;
        }
    }
    if cff_off == 0 {
        return Err("Not a CFF/OpenType font".into());
    }
    otf.get(cff_off..).ok_or_else(|| "CFF table beyond end of file".to_string())
}

// ---------------------------------------------------------------------------------------------
// INDEX (cff_get_index / cff_pack_index)
// ---------------------------------------------------------------------------------------------

/// A parsed INDEX; `offs` are the raw 1-based CFF offsets (`count + 1` of them), `data` the
/// object data.
struct Index<'a> {
    offs: Vec<u32>,
    data: &'a [u8],
}

impl<'a> Index<'a> {
    fn count(&self) -> usize {
        self.offs.len().saturating_sub(1)
    }

    fn item(&self, i: usize) -> &'a [u8] {
        &self.data[(self.offs[i] - 1) as usize..(self.offs[i + 1] - 1) as usize]
    }
}

/// `cff_get_index` at absolute (CFF-relative) position `pos`; returns the index and the position
/// following it.
fn read_index(d: &[u8], pos: usize) -> R<(Index<'_>, usize)> {
    let (offs, data_start) = read_index_header(d, pos)?;
    if offs.is_empty() {
        return Ok((Index { offs, data: &[] }, pos + 2));
    }
    let len = (offs[offs.len() - 1] - offs[0]) as usize;
    let data = d.get(data_start..data_start + len).ok_or("File ended prematurely")?;
    Ok((Index { offs, data }, data_start + len))
}

/// `cff_get_index_header`: the offsets and the position of the first data byte.
fn read_index_header(d: &[u8], pos: usize) -> R<(Vec<u32>, usize)> {
    let count = be16(d, pos)? as usize;
    if count == 0 {
        return Ok((Vec::new(), pos + 2));
    }
    let offsize = byte(d, pos + 2)? as usize;
    if !(1..=4).contains(&offsize) {
        return Err("invalid offsize data".into());
    }
    let mut offs = Vec::with_capacity(count + 1);
    let table = d
        .get(pos + 3..pos + 3 + (count + 1) * offsize)
        .ok_or("File ended prematurely")?;
    for c in table.chunks_exact(offsize) {
        offs.push(c.iter().fold(0u32, |v, &b| v * 0x100 + b as u32));
    }
    if offs[0] != 1 {
        return Err("Invalid CFF Index offset data".into());
    }
    if offs.windows(2).any(|w| w[1] < w[0]) {
        return Err("Invalid CFF Index offset data".into());
    }
    Ok((offs, pos + 3 + (count + 1) * offsize))
}

/// `cff_index_size` for `count` objects of `datalen` bytes in total.
fn index_size(count: usize, datalen: usize) -> usize {
    if count == 0 {
        2
    } else {
        3 + offsize_for(datalen) * (count + 1) + datalen
    }
}

fn offsize_for(datalen: usize) -> usize {
    if datalen < 0xff {
        1
    } else if datalen < 0xffff {
        2
    } else if datalen < 0xff_ffff {
        3
    } else {
        4
    }
}

/// `cff_pack_index`.
fn pack_index(items: &[Vec<u8>], out: &mut Vec<u8>) -> R<()> {
    if items.len() > 0xffff {
        return Err("too many INDEX entries".into());
    }
    if items.is_empty() {
        out.extend_from_slice(&[0, 0]);
        return Ok(());
    }
    let datalen: usize = items.iter().map(Vec::len).sum();
    let offsize = offsize_for(datalen);
    out.extend_from_slice(&(items.len() as u16).to_be_bytes());
    out.push(offsize as u8);
    let mut off = 1usize;
    let put = |off: usize, out: &mut Vec<u8>| {
        out.extend_from_slice(&(off as u32).to_be_bytes()[4 - offsize..]);
    };
    put(off, out);
    for it in items {
        off += it.len();
        put(off, out);
    }
    for it in items {
        out.extend_from_slice(it);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// DICT (cff_dict.c)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ty {
    Sid,
    Array,
    Delta,
    Number,
    Boolean,
    Offset,
    SzOff,
    Ros,
    Unused,
}

const LAST_DICT_OP1: usize = 22;
const LAST_DICT_OP: usize = 61;

/// `dict_operator[]`: operator name and operand type, indexed by `op` (one byte operators) or
/// `22 + second byte` (`12 x` operators).
static DICT_OPERATOR: [(&str, Ty); LAST_DICT_OP] = [
    ("version", Ty::Sid),
    ("Notice", Ty::Sid),
    ("FullName", Ty::Sid),
    ("FamilyName", Ty::Sid),
    ("Weight", Ty::Sid),
    ("FontBBox", Ty::Array),
    ("BlueValues", Ty::Delta),
    ("OtherBlues", Ty::Delta),
    ("FamilyBlues", Ty::Delta),
    ("FamilyOtherBlues", Ty::Delta),
    ("StdHW", Ty::Number),
    ("StdVW", Ty::Number),
    ("", Ty::Unused),
    ("UniqueID", Ty::Number),
    ("XUID", Ty::Array),
    ("charset", Ty::Offset),
    ("Encoding", Ty::Offset),
    ("CharStrings", Ty::Offset),
    ("Private", Ty::SzOff),
    ("Subrs", Ty::Offset),
    ("defaultWidthX", Ty::Number),
    ("nominalWidthX", Ty::Number),
    ("Copyright", Ty::Sid),
    ("IsFixedPitch", Ty::Boolean),
    ("ItalicAngle", Ty::Number),
    ("UnderlinePosition", Ty::Number),
    ("UnderlineThickness", Ty::Number),
    ("PaintType", Ty::Number),
    ("CharstringType", Ty::Number),
    ("FontMatrix", Ty::Array),
    ("StrokeWidth", Ty::Number),
    ("BlueScale", Ty::Number),
    ("BlueShift", Ty::Number),
    ("BlueFuzz", Ty::Number),
    ("StemSnapH", Ty::Delta),
    ("StemSnapV", Ty::Delta),
    ("ForceBold", Ty::Boolean),
    ("", Ty::Unused),
    ("", Ty::Unused),
    ("LanguageGroup", Ty::Number),
    ("ExpansionFactor", Ty::Number),
    ("InitialRandomSeed", Ty::Number),
    ("SyntheticBase", Ty::Number),
    ("PostScript", Ty::Sid),
    ("BaseFontName", Ty::Sid),
    ("BaseFontBlend", Ty::Delta),
    ("", Ty::Unused),
    ("", Ty::Unused),
    ("", Ty::Unused),
    ("", Ty::Unused),
    ("", Ty::Unused),
    ("", Ty::Unused),
    ("ROS", Ty::Ros),
    ("CIDFontVersion", Ty::Number),
    ("CIDFontRevision", Ty::Number),
    ("CIDFontType", Ty::Number),
    ("CIDCount", Ty::Number),
    ("UIDBase", Ty::Number),
    ("FDArray", Ty::Offset),
    ("FDSelect", Ty::Offset),
    ("FontName", Ty::Sid),
];

fn op_id(key: &str) -> Option<usize> {
    DICT_OPERATOR.iter().position(|(n, _)| !n.is_empty() && *n == key)
}

#[derive(Clone)]
struct Entry {
    id: usize,
    /// empty == removed (`count == 0`)
    values: Vec<f64>,
}

#[derive(Clone, Default)]
struct Dict {
    entries: Vec<Entry>,
}

impl Dict {
    /// `cff_dict_unpack`
    fn unpack(data: &[u8]) -> R<Dict> {
        let mut dict = Dict::default();
        let mut stack: Vec<f64> = Vec::new();
        let mut p = 0usize;
        let end = data.len();
        while p < end {
            let b = data[p];
            if b < 22 {
                dict.add_op(data, &mut p, &mut stack)?;
            } else if b == 30 {
                if stack.len() < CFF_DICT_STACK_LIMIT {
                    stack.push(get_real(data, &mut p)?);
                } else {
                    return Err("Parsing CFF DICT failed. (stack overflow)".into());
                }
            } else if b == 255 || (22..=27).contains(&b) {
                p += 1;
            } else if stack.len() < CFF_DICT_STACK_LIMIT {
                stack.push(get_dict_integer(data, &mut p)?);
            } else {
                return Err("Parsing CFF DICT failed. (stack overflow)".into());
            }
        }
        Ok(dict)
    }

    /// `add_dict` (an operator)
    fn add_op(&mut self, data: &[u8], p: &mut usize, stack: &mut Vec<f64>) -> R<()> {
        let mut id = data[*p] as usize;
        if id == 0x0c {
            *p += 1;
            if *p >= data.len() {
                return Err("Parsing CFF DICT failed. (parse error)".into());
            }
            id = data[*p] as usize + LAST_DICT_OP1;
            if id >= LAST_DICT_OP {
                return Err("Parsing CFF DICT failed. (parse error)".into());
            }
        } else if id >= LAST_DICT_OP1 {
            return Err("Parsing CFF DICT failed. (parse error)".into());
        }
        let (name, ty) = DICT_OPERATOR[id];
        if name.is_empty() || ty == Ty::Unused {
            // Unknown operator (e.g. YuppySC-Regular.otf's 12 37): ignored; note dvipdfmx does
            // not advance past (or clear the operands of) it.
            return Ok(());
        }
        match ty {
            Ty::Number | Ty::Boolean | Ty::Sid | Ty::Offset => {
                let v = stack
                    .pop()
                    .ok_or("Parsing CFF DICT failed. (stack underflow)")?;
                self.entries.push(Entry { id, values: vec![v] });
            }
            _ => {
                if !stack.is_empty() {
                    self.entries.push(Entry { id, values: std::mem::take(stack) });
                }
            }
        }
        *p += 1;
        Ok(())
    }

    fn find(&self, key: &str) -> Option<&Entry> {
        let id = op_id(key)?;
        self.entries.iter().find(|e| e.id == id)
    }

    /// `cff_dict_known`: the first entry with that key, if it has operands. (dvipdfmx scans all
    /// entries for any with `count > 0`.)
    fn known(&self, key: &str) -> bool {
        match op_id(key) {
            Some(id) => self.entries.iter().any(|e| e.id == id && !e.values.is_empty()),
            None => false,
        }
    }

    /// `cff_dict_get`
    fn get(&self, key: &str, idx: usize) -> R<f64> {
        match self.find(key) {
            Some(e) => e
                .values
                .get(idx)
                .copied()
                .ok_or_else(|| format!("CFF: Invalid index number in DICT entry \"{key}\".")),
            None => Err(format!("CFF: DICT entry \"{key}\" not found.")),
        }
    }

    /// `cff_dict_set`
    fn set(&mut self, key: &str, idx: usize, value: f64) -> R<()> {
        let id = op_id(key).ok_or("CFF: Unknown CFF DICT operator.")?;
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(e) => match e.values.get_mut(idx) {
                Some(v) => {
                    *v = value;
                    Ok(())
                }
                None => Err(format!("CFF: Invalid index number in DICT entry \"{key}\".")),
            },
            None => Err(format!("CFF: DICT entry \"{key}\" not found.")),
        }
    }

    /// `cff_dict_add`
    fn add(&mut self, key: &str, count: usize) -> R<()> {
        let id = op_id(key).ok_or("CFF: Unknown CFF DICT operator.")?;
        if let Some(e) = self.entries.iter().find(|e| e.id == id) {
            if e.values.len() != count {
                return Err("CFF: Inconsistent DICT argument number.".into());
            }
            return Ok(());
        }
        self.entries.push(Entry { id, values: vec![0.0; count] });
        Ok(())
    }

    /// `cff_dict_remove` (clears the operands of every entry with that key)
    fn remove(&mut self, key: &str) {
        if let Some(id) = op_id(key) {
            for e in self.entries.iter_mut().filter(|e| e.id == id) {
                e.values.clear();
            }
        }
    }

    /// `cff_dict_pack`: the ROS entry first, then every other entry in order.
    fn pack(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let ros = op_id("ROS").unwrap();
        if let Some(e) = self.entries.iter().find(|e| e.id == ros) {
            put_dict_entry(e, &mut out);
        }
        for e in self.entries.iter().filter(|e| e.id != ros) {
            put_dict_entry(e, &mut out);
        }
        out
    }

    /// `cff_dict_update`: move SID operands into the new string table.
    fn update(&mut self, strings: &mut Strings) -> R<()> {
        for e in &mut self.entries {
            if e.values.is_empty() {
                continue;
            }
            match DICT_OPERATOR[e.id].1 {
                Ty::Sid => e.values[0] = strings.remap(e.values[0])?,
                Ty::Ros => {
                    e.values[0] = strings.remap(e.values[0])?;
                    e.values[1] = strings.remap(*e.values.get(1).ok_or("Invalid ROS")?)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// DICT integer operand (`get_integer` in cff_dict.c); `data[*p]` is the first byte.
fn get_dict_integer(data: &[u8], p: &mut usize) -> R<f64> {
    let end = data.len();
    let b0 = data[*p];
    *p += 1;
    let bad = || "Parsing CFF DICT failed. (parse error)".to_string();
    let result: i32 = if b0 == 28 && *p + 2 < end {
        let v = data[*p] as i32 * 256 + data[*p + 1] as i32;
        *p += 2;
        if v > 0x7fff {
            v - 0x10000
        } else {
            v
        }
    } else if b0 == 29 && *p + 4 < end {
        let mut v = data[*p] as i32;
        *p += 1;
        if v > 0x7f {
            v -= 0x100;
        }
        for _ in 0..3 {
            v = v.wrapping_mul(256).wrapping_add(data[*p] as i32);
            *p += 1;
        }
        v
    } else if (32..=246).contains(&b0) {
        b0 as i32 - 139
    } else if (247..=250).contains(&b0) {
        let b1 = *data.get(*p).ok_or_else(bad)? as i32;
        *p += 1;
        (b0 as i32 - 247) * 256 + b1 + 108
    } else if (251..=254).contains(&b0) {
        let b1 = *data.get(*p).ok_or_else(bad)? as i32;
        *p += 1;
        -(b0 as i32 - 251) * 256 - b1 - 108
    } else {
        return Err(bad());
    };
    Ok(result as f64)
}

/// DICT real operand (`get_real`); `data[*p] == 30`.
fn get_real(data: &[u8], p: &mut usize) -> R<f64> {
    let end = data.len();
    let bad = || "Parsing CFF DICT failed. (parse error)".to_string();
    if data[*p] != 30 || *p + 1 >= end {
        return Err(bad());
    }
    *p += 1;
    let mut buf = String::new();
    let mut pos = 0usize;
    let mut nibble = 0u8;
    let mut fail = false;
    while !fail && buf.len() < 1024 - 2 && *p < end {
        if pos % 2 == 1 {
            nibble = data[*p] & 0x0f;
            *p += 1;
        } else {
            nibble = (data[*p] >> 4) & 0x0f;
        }
        match nibble {
            0..=9 => buf.push((b'0' + nibble) as char),
            0x0a => buf.push('.'),
            0x0b => buf.push('e'),
            0x0c => buf.push_str("e-"),
            0x0e => buf.push('-'),
            0x0d => {}
            0x0f => {
                if pos % 2 == 0 && data[*p] != 0xff {
                    fail = true;
                }
                break;
            }
            _ => fail = true,
        }
        pos += 1;
    }
    if fail || nibble != 0x0f {
        return Err(bad());
    }
    if buf.is_empty() {
        return Ok(0.0);
    }
    match buf.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(bad()),
    }
}

/// `pack_integer`
fn pack_integer(out: &mut Vec<u8>, value: i32) {
    if (-107..=107).contains(&value) {
        out.push((value + 139) as u8);
    } else if (108..=1131).contains(&value) {
        let v = 0xf700 + value - 108;
        out.extend_from_slice(&[(v >> 8) as u8, v as u8]);
    } else if (-1131..=-108).contains(&value) {
        let v = 0xfb00 - value - 108;
        out.extend_from_slice(&[(v >> 8) as u8, v as u8]);
    } else if (-32768..=32767).contains(&value) {
        out.extend_from_slice(&[28, (value >> 8) as u8, value as u8]);
    } else {
        out.push(29);
        out.extend_from_slice(&value.to_be_bytes());
    }
}

/// C's `sprintf("%.13g", v)` for finite `v > 0`.
fn fmt_g13(v: f64) -> String {
    fn trim(mut s: String) -> String {
        if s.contains('.') {
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.pop();
            }
        }
        s
    }
    let e = format!("{:.12e}", v);
    let (mant, exp) = e.split_once('e').unwrap();
    let x: i32 = exp.parse().unwrap();
    if !(-4..13).contains(&x) {
        format!("{}e{}{:02}", trim(mant.to_string()), if x < 0 { '-' } else { '+' }, x.abs())
    } else {
        trim(format!("{:.*}", (12 - x) as usize, v))
    }
}

/// `pack_real`
fn pack_real(out: &mut Vec<u8>, mut value: f64) {
    out.push(30);
    if value == 0.0 {
        out.push(0x0f);
        return;
    }
    let mut nibbles: Vec<u8> = Vec::new();
    if value < 0.0 {
        nibbles.push(0x0e);
        value = -value;
    }
    let s = fmt_g13(value);
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let ch = match b[i] {
            b'.' => 0x0a,
            b'0'..=b'9' => b[i] - b'0',
            b'e' => {
                i += 1;
                if b[i] == b'-' {
                    0x0c
                } else {
                    0x0b
                }
            }
            _ => unreachable!(),
        };
        nibbles.push(ch);
        i += 1;
    }
    // Terminator: one `f` nibble if the last byte has a free low nibble, else a whole 0xff byte.
    nibbles.push(0x0f);
    if nibbles.len() % 2 == 1 {
        nibbles.push(0x0f);
    }
    for c in nibbles.chunks(2) {
        out.push(c[0] << 4 | c[1]);
    }
}

/// `cff_dict_put_number`
fn put_number(out: &mut Vec<u8>, value: f64, offset: bool) {
    if offset {
        out.push(29);
        out.extend_from_slice(&(value as i32).to_be_bytes());
        return;
    }
    let nearint = (value + 0.5).floor();
    if value > i32::MAX as f64 || value < i32::MIN as f64 || (value - nearint).abs() > 1.0e-5 {
        pack_real(out, value);
    } else {
        pack_integer(out, nearint as i32);
    }
}

/// `put_dict_entry`
fn put_dict_entry(e: &Entry, out: &mut Vec<u8>) {
    if e.values.is_empty() {
        return;
    }
    let offset = matches!(DICT_OPERATOR[e.id].1, Ty::Offset | Ty::SzOff);
    for &v in &e.values {
        put_number(out, v, offset);
    }
    if e.id < LAST_DICT_OP1 {
        out.push(e.id as u8);
    } else {
        out.push(12);
        out.push((e.id - LAST_DICT_OP1) as u8);
    }
}

// ---------------------------------------------------------------------------------------------
// Strings (cff_get_string / cff_add_string / cff_update_string)
// ---------------------------------------------------------------------------------------------

struct Strings {
    /// `cff->string`
    old: Vec<Vec<u8>>,
    /// `cff->_string`
    new: Vec<Vec<u8>>,
}

impl Strings {
    fn new(cff: &CffFont) -> Strings {
        Strings {
            old: (0..cff.string.count()).map(|i| cff.string.item(i).to_vec()).collect(),
            new: Vec::new(),
        }
    }

    /// `cff_get_string`
    fn get(&self, sid: u16) -> Option<Vec<u8>> {
        let sid = sid as usize;
        if sid < CFF_STDSTR_MAX {
            Some(STD_STRINGS[sid].as_bytes().to_vec())
        } else {
            self.old.get(sid - CFF_STDSTR_MAX).cloned()
        }
    }

    /// `cff_add_string(.., unique = 1)`
    fn add_unique(&mut self, s: &[u8]) -> R<u16> {
        if let Some(i) = STD_STRINGS.iter().position(|x| x.as_bytes() == s) {
            return Ok(i as u16);
        }
        if let Some(i) = self.new.iter().position(|x| x == s) {
            return Ok((i + CFF_STDSTR_MAX) as u16);
        }
        self.new.push(s.to_vec());
        let sid = self.new.len() - 1 + CFF_STDSTR_MAX;
        u16::try_from(sid).map_err(|_| "too many strings".to_string())
    }

    /// SID operand of a DICT entry -> SID in the new table.
    fn remap(&mut self, v: f64) -> R<f64> {
        let s = self.get(v as u16).ok_or("Invalid SID")?;
        Ok(self.add_unique(&s)? as f64)
    }

    /// `cff_get_sid` on the updated string table.
    fn sid_of(&self, s: &str) -> R<u16> {
        if let Some(i) = self.new.iter().position(|x| x == s.as_bytes()) {
            return Ok((i + CFF_STDSTR_MAX) as u16);
        }
        STD_STRINGS
            .iter()
            .position(|x| *x == s)
            .map(|i| i as u16)
            .ok_or_else(|| "SID not found".to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// CFF reader (cff_open, cff_read_*)
// ---------------------------------------------------------------------------------------------

struct CffFont<'a> {
    /// the stream from the start of the CFF table
    d: &'a [u8],
    major: u8,
    minor: u8,
    /// `cff_get_name`
    name: Vec<u8>,
    topdict: Dict,
    string: Index<'a>,
    gsubr_offset: usize,
    num_glyphs: u16,
    is_cid: bool,
}

/// Expanded charset: `glyphs[gid - 1]` is the SID/CID of glyph `gid`.
struct Charsets {
    glyphs: Vec<u16>,
}

impl Charsets {
    /// `cff_charsets_lookup_gid` for every CID: first match wins, CID 0 is GID 0.
    fn cid_to_gid(&self) -> BTreeMap<u16, u16> {
        let mut map = BTreeMap::new();
        for (i, &c) in self.glyphs.iter().enumerate() {
            if let Ok(gid) = u16::try_from(i + 1) {
                map.entry(c).or_insert(gid);
            }
        }
        map.insert(0, 0);
        map
    }
}

enum FdSelect {
    F0(Vec<u8>),
    F3(Vec<(u16, u8)>),
}

impl<'a> CffFont<'a> {
    /// `cff_open(stream, offset, 0)`
    fn open(d: &'a [u8]) -> R<CffFont<'a>> {
        let major = byte(d, 0)?;
        let minor = byte(d, 1)?;
        let hdr_size = byte(d, 2)? as usize;
        let offsize = byte(d, 3)?;
        if !(1..=4).contains(&offsize) {
            return Err("invalid offsize data".into());
        }
        if major > 1 || minor > 0 {
            return Err(format!("CFF version {major}.{minor} not supported."));
        }
        let (name_idx, pos) = read_index(d, hdr_size)?;
        if name_idx.count() < 1 {
            return Err("Invalid CFF fontset index number.".into());
        }
        let name = name_idx.item(0).to_vec();
        let (top_idx, pos) = read_index(d, pos)?;
        if top_idx.count() < 1 {
            return Err("CFF Top DICT not exist...".into());
        }
        let topdict = Dict::unpack(top_idx.item(0))?;
        if topdict.known("CharstringType") && topdict.get("CharstringType", 0)? != 2.0 {
            return Err("Only Type 2 Charstrings supported...".into());
        }
        if topdict.known("SyntheticBase") {
            return Err("CFF Synthetic font not supported.".into());
        }
        let (string, pos) = read_index(d, pos)?;
        let gsubr_offset = pos;
        let cs_off = topdict.get("CharStrings", 0)? as i32;
        let num_glyphs = be16(d, usize::try_from(cs_off).map_err(|_| "bad CharStrings offset")?)?;
        let is_cid = topdict.known("ROS");
        Ok(CffFont { d, major, minor, name, topdict, string, gsubr_offset, num_glyphs, is_cid })
    }

    fn off(&self, key: &str, idx: usize) -> R<usize> {
        let v = self.topdict.get(key, idx)? as i32;
        usize::try_from(v).map_err(|_| format!("bad {key} offset"))
    }

    /// `Some(n)` when the top DICT selects predefined charset `n` (0 ISOAdobe, 1 Expert,
    /// 2 ExpertSubset; also when absent: ISOAdobe).
    fn charset_predefined(&self) -> Option<u8> {
        if !self.topdict.known("charset") {
            return Some(0);
        }
        match self.topdict.get("charset", 0) {
            Ok(v) if v == 0.0 || v == 1.0 || v == 2.0 => Some(v as u8),
            _ => None,
        }
    }

    /// `cff_read_charsets`: `None` for predefined charsets.
    fn read_charsets(&self) -> R<Option<Charsets>> {
        if self.charset_predefined().is_some() {
            return Ok(None);
        }
        let d = self.d;
        let mut p = self.off("charset", 0)?;
        let format = byte(d, p)?;
        p += 1;
        let ng = self.num_glyphs;
        let mut count: u16 = ng.wrapping_sub(1);
        let mut glyphs: Vec<u16> = Vec::new();
        match format {
            0 => {
                for _ in 0..ng.wrapping_sub(1) {
                    glyphs.push(be16(d, p)?);
                    p += 2;
                }
                count = 0;
            }
            1 | 2 => {
                let mut entries: u16 = 0;
                while count > 0 && entries < ng {
                    let first = be16(d, p)?;
                    p += 2;
                    let n_left = if format == 1 {
                        p += 1;
                        byte(d, p - 1)? as u16
                    } else {
                        p += 2;
                        be16(d, p - 2)?
                    };
                    count = count.wrapping_sub(n_left.wrapping_add(1));
                    entries += 1;
                    for n in 0..=n_left as u32 {
                        glyphs.push(first.wrapping_add(n as u16));
                    }
                }
            }
            _ => return Err("Unknown Charset format".into()),
        }
        if count > 0 {
            return Err("Charset data possibly broken".into());
        }
        Ok(Some(Charsets { glyphs }))
    }

    /// `cff_read_fdselect` (CID-keyed fonts)
    fn read_fdselect(&self) -> R<FdSelect> {
        let d = self.d;
        let mut p = self.off("FDSelect", 0)?;
        let format = byte(d, p)?;
        p += 1;
        match format {
            0 => {
                let b = d.get(p..p + self.num_glyphs as usize).ok_or("File ended prematurely")?;
                Ok(FdSelect::F0(b.to_vec()))
            }
            3 => {
                let n = be16(d, p)? as usize;
                p += 2;
                let mut ranges = Vec::with_capacity(n);
                for _ in 0..n {
                    ranges.push((be16(d, p)?, byte(d, p + 2)?));
                    p += 3;
                }
                if ranges.is_empty() || ranges[0].0 != 0 {
                    return Err("Range not starting with 0.".into());
                }
                if be16(d, p)? != self.num_glyphs {
                    return Err("Sentinel value mismatched with number of glyphs.".into());
                }
                Ok(FdSelect::F3(ranges))
            }
            _ => Err("Unknown FDSelect format.".into()),
        }
    }

    /// `cff_fdselect_lookup`
    fn fd_lookup(&self, fdsel: &FdSelect, gid: u16, num_fds: usize) -> R<u8> {
        if gid >= self.num_glyphs {
            return Err("in cff_fdselect_lookup(): Invalid glyph index".into());
        }
        let fd = match fdsel {
            FdSelect::F0(fds) => fds[gid as usize],
            FdSelect::F3(ranges) => {
                if gid == 0 {
                    ranges[0].1
                } else {
                    let mut i = 1;
                    while i < ranges.len() {
                        if gid < ranges[i].0 {
                            break;
                        }
                        i += 1;
                    }
                    ranges[i - 1].1
                }
            }
        };
        if fd as usize >= num_fds {
            return Err("in cff_fdselect_lookup(): Invalid Font DICT index".into());
        }
        Ok(fd)
    }

    /// `cff_read_fdarray`
    fn read_fdarray(&self) -> R<Vec<Option<Dict>>> {
        let (idx, _) = read_index(self.d, self.off("FDArray", 0)?)?;
        if idx.count() > 255 {
            return Err("too many Font DICTs".into());
        }
        (0..idx.count())
            .map(|i| {
                let it = idx.item(i);
                if it.is_empty() {
                    Ok(None)
                } else {
                    Dict::unpack(it).map(Some)
                }
            })
            .collect()
    }

    /// `Private` DICT described by `holder` (size at operand 0, offset at operand 1).
    fn read_private_of(&self, holder: &Dict) -> R<Option<Dict>> {
        if holder.known("Private") {
            let size = holder.get("Private", 0)? as i32;
            if size > 0 {
                let off = usize::try_from(holder.get("Private", 1)? as i32)
                    .map_err(|_| "bad Private offset")?;
                let b = self
                    .d
                    .get(off..off + size as usize)
                    .ok_or("reading file failed")?;
                return Dict::unpack(b).map(Some);
            }
        }
        Ok(None)
    }

    /// `Subrs` INDEX of `private`, whose `Private` operands live in `holder`.
    fn read_subrs_of(&self, holder: &Dict, private: &Option<Dict>) -> R<Option<Index<'a>>> {
        match private {
            Some(p) if p.known("Subrs") => {
                let off = holder.get("Private", 1)? as i32 + p.get("Subrs", 0)? as i32;
                let off = usize::try_from(off).map_err(|_| "bad Subrs offset")?;
                Ok(Some(read_index(self.d, off)?.0))
            }
            _ => Ok(None),
        }
    }

    fn read_gsubr(&self) -> R<Index<'a>> {
        Ok(read_index(self.d, self.gsubr_offset)?.0)
    }
}

// ---------------------------------------------------------------------------------------------
// Type 2 charstrings (cs_type2.c)
// ---------------------------------------------------------------------------------------------

#[derive(PartialEq, Clone, Copy)]
enum Status {
    Ok,
    SubrReturn,
    CharEnd,
}

/// `cs_copy_charstring` state. `trn_array` is a static in dvipdfmx and therefore survives from
/// one glyph to the next; the same happens here (one `Cs` per embedded font).
struct Cs<'a> {
    trn_array: [f64; CS_TRANS_ARRAY_MAX],
    stack: Vec<f64>,
    num_stems: usize,
    phase: u8,
    nest: usize,
    status: Status,
    gsubr: Option<&'a Index<'a>>,
    subr: Option<&'a Index<'a>>,
}

fn cs_err<T>(msg: &str) -> R<T> {
    Err(format!("Type2 Charstring Parser: {msg}"))
}

impl<'a> Cs<'a> {
    fn new() -> Cs<'a> {
        Cs {
            trn_array: [0.0; CS_TRANS_ARRAY_MAX],
            stack: Vec::new(),
            num_stems: 0,
            phase: 0,
            nest: 0,
            status: Status::Ok,
            gsubr: None,
            subr: None,
        }
    }

    /// `cs_copy_charstring` (with `ginfo == NULL`): the charstring `src` with every call(g)subr
    /// replaced by the subroutine body, operands re-encoded; appended to `dst`.
    fn copy(
        &mut self,
        dst: &mut Vec<u8>,
        src: &[u8],
        gsubr: Option<&'a Index<'a>>,
        subr: Option<&'a Index<'a>>,
    ) -> R<()> {
        self.status = Status::Ok;
        self.nest = 0;
        self.phase = 0;
        self.num_stems = 0;
        self.stack.clear();
        self.gsubr = gsubr;
        self.subr = subr;
        let start = dst.len();
        self.do_charstring(dst, src)?;
        if dst.len() - start > 2 * CS_STR_LEN_MAX {
            return cs_err("Possible buffer overflow.");
        }
        Ok(())
    }

    /// `clear_stack`: write all operands.
    fn clear_stack(&mut self, dst: &mut Vec<u8>) -> R<()> {
        for &value in &self.stack {
            let mut ivalue = (value + 0.5).floor() as i32;
            if value >= 32768.0 || value <= -32769.0 {
                return cs_err("Argument value too large. (This is bug)");
            } else if (value - ivalue as f64).abs() > 3.0e-5 {
                // 16.16-bit signed fixed value
                dst.push(255);
                ivalue = value.floor() as i32;
                dst.push((ivalue >> 8) as u8);
                dst.push(ivalue as u8);
                let frac = ((value - ivalue as f64) * 65536.0) as i32;
                dst.push((frac >> 8) as u8);
                dst.push(frac as u8);
            } else if (-107..=107).contains(&ivalue) {
                dst.push((ivalue + 139) as u8);
            } else if (108..=1131).contains(&ivalue) {
                let v = 0xf700 + ivalue - 108;
                dst.push((v >> 8) as u8);
                dst.push(v as u8);
            } else if (-1131..=-108).contains(&ivalue) {
                let v = 0xfb00 - ivalue - 108;
                dst.push((v >> 8) as u8);
                dst.push(v as u8);
            } else if (-32768..=32767).contains(&ivalue) {
                dst.push(28);
                dst.push((ivalue >> 8) as u8);
                dst.push(ivalue as u8);
            } else {
                return cs_err("Unexpected error.");
            }
        }
        self.stack.clear();
        Ok(())
    }

    fn push(&mut self, v: f64) -> R<()> {
        if self.stack.len() + 1 > CS_ARG_STACK_MAX {
            return cs_err("stack overflow");
        }
        self.stack.push(v);
        Ok(())
    }

    /// `do_operator1`: single byte operators; `data[*p]` is the operator.
    fn do_operator1(&mut self, dst: &mut Vec<u8>, data: &[u8], p: &mut usize) -> R<()> {
        let op = data[*p];
        *p += 1;
        match op {
            // hstem vstem hstemhm vstemhm
            1 | 3 | 18 | 23 => {
                self.num_stems += self.stack.len() / 2;
                self.clear_stack(dst)?;
                dst.push(op);
                self.phase = 1;
            }
            // hintmask cntrmask
            19 | 20 => {
                if self.phase < 2 {
                    self.num_stems += self.stack.len() / 2;
                }
                self.clear_stack(dst)?;
                dst.push(op);
                if self.num_stems > 0 {
                    let masklen = (self.num_stems + 7) / 8;
                    let mask = data.get(*p..*p + masklen).ok_or("Type2 Charstring Parser: parse error")?;
                    dst.extend_from_slice(mask);
                    *p += masklen;
                }
                self.phase = 2;
            }
            // rmoveto hmoveto vmoveto
            21 | 22 | 4 => {
                self.clear_stack(dst)?;
                dst.push(op);
                self.phase = 2;
            }
            // endchar
            14 => {
                let n = self.stack.len();
                if n == 1 {
                    self.clear_stack(dst)?;
                } else if n == 4 || n == 5 {
                    return cs_err("\"seac\" character deprecated in Type 2 charstring.");
                }
                // Other non-empty stacks only produce a warning in dvipdfmx; the operands
                // are dropped.
                dst.push(op);
                self.status = Status::CharEnd;
            }
            // rlineto hlineto vlineto rrcurveto rcurveline rlinecurve vvcurveto hhcurveto
            // vhcurveto hvcurveto
            5 | 6 | 7 | 8 | 24 | 25 | 26 | 27 | 30 | 31 => {
                if self.phase < 2 {
                    return cs_err("Broken Type 2 charstring.");
                }
                self.clear_stack(dst)?;
                dst.push(op);
            }
            // return callgsubr callsubr
            11 | 29 | 10 => return cs_err("Unexpected call(g)subr/return"),
            _ => return cs_err(&format!("Unknown charstring operator: 0x{op:02x}")),
        }
        Ok(())
    }

    /// `do_operator2`: `12 x` operators; `data[*p] == 12`.
    fn do_operator2(&mut self, dst: &mut Vec<u8>, data: &[u8], p: &mut usize) -> R<()> {
        *p += 1;
        let op = *data.get(*p).ok_or("Type2 Charstring Parser: parse error")?;
        *p += 1;
        let need = |s: &Self, n: usize| -> R<()> {
            if s.stack.len() < n {
                cs_err("stack error")
            } else {
                Ok(())
            }
        };
        match op {
            0 => return cs_err("Operator \"dotsection\" deprecated in Type 2 charstring."),
            // hflex flex hflex1 flex1
            34 | 35 | 36 | 37 => {
                if self.phase < 2 {
                    return cs_err("Broken Type 2 charstring.");
                }
                self.clear_stack(dst)?;
                dst.push(12);
                dst.push(op);
            }
            3 => {
                // and
                need(self, 2)?;
                let b = self.stack.pop().unwrap();
                let a = self.stack.last_mut().unwrap();
                *a = if b != 0.0 && *a != 0.0 { 1.0 } else { 0.0 };
            }
            4 => {
                // or
                need(self, 2)?;
                let b = self.stack.pop().unwrap();
                let a = self.stack.last_mut().unwrap();
                *a = if b != 0.0 || *a != 0.0 { 1.0 } else { 0.0 };
            }
            5 => {
                // not
                need(self, 1)?;
                let a = self.stack.last_mut().unwrap();
                *a = if *a != 0.0 { 0.0 } else { 1.0 };
            }
            9 => {
                need(self, 1)?;
                let a = self.stack.last_mut().unwrap();
                *a = a.abs();
            }
            10 | 11 | 12 | 24 => {
                // add sub div mul
                need(self, 2)?;
                let b = self.stack.pop().unwrap();
                let a = self.stack.last_mut().unwrap();
                match op {
                    10 => *a += b,
                    11 => *a -= b,
                    12 => *a /= b,
                    _ => *a *= b,
                }
            }
            14 => {
                need(self, 1)?;
                let a = self.stack.last_mut().unwrap();
                *a *= -1.0;
            }
            15 => {
                // eq
                need(self, 2)?;
                let b = self.stack.pop().unwrap();
                let a = self.stack.last_mut().unwrap();
                *a = if b == *a { 1.0 } else { 0.0 };
            }
            18 => {
                need(self, 1)?;
                self.stack.pop();
            }
            20 => {
                // put
                need(self, 2)?;
                let idx = self.stack.pop().unwrap() as i32;
                if !(0..CS_TRANS_ARRAY_MAX as i32).contains(&idx) {
                    return cs_err("stack error");
                }
                self.trn_array[idx as usize] = self.stack.pop().unwrap();
            }
            21 => {
                // get
                need(self, 1)?;
                let idx = *self.stack.last().unwrap() as i32;
                if !(0..CS_TRANS_ARRAY_MAX as i32).contains(&idx) {
                    return cs_err("stack error");
                }
                *self.stack.last_mut().unwrap() = self.trn_array[idx as usize];
            }
            22 => {
                // ifelse
                need(self, 4)?;
                let n = self.stack.len() - 3;
                self.stack.truncate(n + 3);
                if self.stack[n + 1] > self.stack[n + 2] {
                    self.stack[n - 1] = self.stack[n];
                }
                self.stack.truncate(n);
            }
            23 => {
                // random
                self.push(1.0)?;
            }
            26 => {
                need(self, 1)?;
                let a = self.stack.last_mut().unwrap();
                *a = a.sqrt();
            }
            27 => {
                // dup
                need(self, 1)?;
                let v = *self.stack.last().unwrap();
                self.push(v)?;
            }
            28 => {
                // exch
                need(self, 2)?;
                let n = self.stack.len();
                self.stack.swap(n - 1, n - 2);
            }
            29 => {
                // index
                need(self, 2)?;
                let n = self.stack.len();
                let idx = self.stack[n - 1] as i32;
                if idx < 0 {
                    self.stack[n - 1] = self.stack[n - 2];
                } else {
                    need(self, idx as usize + 2)?;
                    self.stack[n - 1] = self.stack[n - idx as usize - 2];
                }
            }
            30 => {
                // roll
                need(self, 2)?;
                let j = self.stack.pop().unwrap() as i32;
                let n = self.stack.pop().unwrap() as i32;
                if n == 0 {
                    return cs_err("stack error");
                }
                if n > 0 {
                    need(self, n as usize)?;
                    let top = self.stack.len();
                    let win = &mut self.stack[top - n as usize..];
                    if j > 0 {
                        win.rotate_right((j % n) as usize);
                    } else {
                        win.rotate_left((j.unsigned_abs() % n as u32) as usize);
                    }
                }
                // n < 0: dvipdfmx's loops do nothing.
            }
            _ => return cs_err(&format!("Unknown charstring operator: 0x0c{op:02x}")),
        }
        Ok(())
    }

    /// `get_integer` (charstring flavour): one operand pushed.
    fn get_integer(&mut self, data: &[u8], p: &mut usize) -> R<()> {
        let b0 = data[*p];
        *p += 1;
        let bad = || "Type2 Charstring Parser: parse error".to_string();
        let result: i32 = if b0 == 28 {
            let b = data.get(*p..*p + 2).ok_or_else(bad)?;
            let v = b[0] as i32 * 256 + b[1] as i32;
            *p += 2;
            if v > 0x7fff {
                v - 0x10000
            } else {
                v
            }
        } else if (32..=246).contains(&b0) {
            b0 as i32 - 139
        } else if (247..=250).contains(&b0) {
            let b1 = *data.get(*p).ok_or_else(bad)? as i32;
            *p += 1;
            (b0 as i32 - 247) * 256 + b1 + 108
        } else if (251..=254).contains(&b0) {
            let b1 = *data.get(*p).ok_or_else(bad)? as i32;
            *p += 1;
            -(b0 as i32 - 251) * 256 - b1 - 108
        } else {
            return Err(bad());
        };
        self.push(result as f64)
    }

    /// `get_fixed`: signed 16.16 operand (`255` prefix)
    fn get_fixed(&mut self, data: &[u8], p: &mut usize) -> R<()> {
        *p += 1;
        let b = data.get(*p..*p + 4).ok_or("Type2 Charstring Parser: parse error")?;
        let mut ivalue = b[0] as i32 * 0x100 + b[1] as i32;
        let mut rvalue = if ivalue > 0x7fff { ivalue - 0x10000 } else { ivalue } as f64;
        ivalue = b[2] as i32 * 0x100 + b[3] as i32;
        rvalue += ivalue as f64 / 65536.0;
        self.push(rvalue)?;
        *p += 4;
        Ok(())
    }

    /// `get_subr`: body of the subroutine with biased number `id`.
    fn get_subr(idx: Option<&'a Index<'a>>, id: i32) -> R<&'a [u8]> {
        let idx = match idx {
            Some(i) => i,
            None => return cs_err("Subroutine called but no subroutine found."),
        };
        let count = idx.count();
        let id = id as i64
            + if count < 1240 {
                107
            } else if count < 33900 {
                1131
            } else {
                32768
            };
        if id < 0 || id as usize >= count {
            return cs_err(&format!("Invalid Subr index: {id} (max={count})"));
        }
        Ok(idx.item(id as usize))
    }

    /// `do_charstring`
    fn do_charstring(&mut self, dst: &mut Vec<u8>, data: &[u8]) -> R<()> {
        if self.nest > CS_SUBR_NEST_MAX {
            return cs_err("Subroutine nested too deeply.");
        }
        self.nest += 1;
        let mut p = 0usize;
        while p < data.len() && self.status == Status::Ok {
            let b0 = data[p];
            if b0 == 255 {
                self.get_fixed(data, &mut p)?;
            } else if b0 == 11 {
                self.status = Status::SubrReturn;
            } else if b0 == 29 || b0 == 10 {
                let n = self.stack.pop().ok_or("Type2 Charstring Parser: stack error")?;
                let which = if b0 == 29 { self.gsubr } else { self.subr };
                let subr = Self::get_subr(which, n as i32)?;
                self.do_charstring(dst, subr)?;
                p += 1;
            } else if b0 == 12 {
                self.do_operator2(dst, data, &mut p)?;
            } else if b0 < 32 && b0 != 28 {
                self.do_operator1(dst, data, &mut p)?;
            } else {
                self.get_integer(data, &mut p)?;
            }
        }
        if self.status == Status::SubrReturn {
            self.status = Status::Ok;
        }
        self.nest -= 1;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// write_fontfile
// ---------------------------------------------------------------------------------------------

/// The `cff_font` fields `write_fontfile` consumes.
struct OutFont {
    major: u8,
    minor: u8,
    name: Vec<u8>,
    topdict: Dict,
    string: Vec<Vec<u8>>,
    /// charset format 0 entries (glyphs 1..)
    charset: Vec<u16>,
    /// FDSelect format 3 ranges (first gid, fd)
    fdselect: Vec<(u16, u8)>,
    cstrings: Vec<Vec<u8>>,
    num_glyphs: u16,
    fdarray: Vec<Option<Dict>>,
    private: Vec<Option<Dict>>,
}

fn write_fontfile(mut f: OutFont) -> R<Vec<u8>> {
    if f.name.len() > 127 {
        return Err("FontName string length too large...".into());
    }
    for k in ["UniqueID", "XUID", "Private", "Encoding"] {
        f.topdict.remove(k);
    }
    let num_fds = f.fdarray.len();
    let topdict_len = f.topdict.pack().len();
    let mut priv_sizes = vec![0usize; num_fds];
    let mut fd_lens = vec![0usize; num_fds];
    for i in 0..num_fds {
        let fd = f.fdarray[i].as_mut().ok_or("empty Font DICT")?;
        let mut size = 0;
        if let Some(p) = &f.private[i] {
            size = p.pack().len();
            if size < 1 {
                // Private had contained only Subr
                fd.remove("Private");
            }
        }
        priv_sizes[i] = size;
        fd_lens[i] = fd.pack().len();
    }

    let mut dest: Vec<u8> = vec![f.major, f.minor, 4, 4];
    pack_index(&[f.name.clone()], &mut dest)?;
    let topdict_pos = dest.len();
    dest.resize(dest.len() + index_size(1, topdict_len), 0);
    pack_index(&f.string, &mut dest)?;
    dest.extend_from_slice(&[0, 0]); // empty global subrs

    f.topdict.set("charset", 0, dest.len() as f64)?;
    dest.push(0);
    for &g in &f.charset {
        dest.extend_from_slice(&g.to_be_bytes());
    }

    f.topdict.set("FDSelect", 0, dest.len() as f64)?;
    dest.push(3);
    dest.extend_from_slice(&(f.fdselect.len() as u16).to_be_bytes());
    for &(first, fd) in &f.fdselect {
        dest.extend_from_slice(&first.to_be_bytes());
        dest.push(fd);
    }
    dest.extend_from_slice(&f.num_glyphs.to_be_bytes());

    f.topdict.set("CharStrings", 0, dest.len() as f64)?;
    pack_index(&f.cstrings, &mut dest)?;
    f.cstrings = Vec::new();

    f.topdict.set("FDArray", 0, dest.len() as f64)?;
    let fdarray_pos = dest.len();
    let fdarray_size = index_size(num_fds, fd_lens.iter().sum());
    dest.resize(dest.len() + fdarray_size, 0);

    let mut fd_bytes: Vec<Vec<u8>> = Vec::with_capacity(num_fds);
    for i in 0..num_fds {
        let fd = f.fdarray[i].as_mut().unwrap();
        if let Some(p) = &f.private[i] {
            if priv_sizes[i] > 0 {
                let packed = p.pack();
                fd.set("Private", 0, priv_sizes[i] as f64)?;
                fd.set("Private", 1, dest.len() as f64)?;
                dest.extend_from_slice(&packed);
            }
        }
        let b = fd.pack();
        if b.len() != fd_lens[i] {
            return Err("Font DICT size changed".into());
        }
        fd_bytes.push(b);
    }
    let mut fdarray_idx = Vec::with_capacity(fdarray_size);
    pack_index(&fd_bytes, &mut fdarray_idx)?;
    dest[fdarray_pos..fdarray_pos + fdarray_size].copy_from_slice(&fdarray_idx);

    let td = f.topdict.pack();
    if td.len() != topdict_len {
        return Err("Top DICT size changed".into());
    }
    let mut td_idx = Vec::new();
    pack_index(&[td], &mut td_idx)?;
    dest[topdict_pos..topdict_pos + td_idx.len()].copy_from_slice(&td_idx);
    Ok(dest)
}

fn cidset_of(cids: &BTreeSet<u16>, last_cid: u16) -> Vec<u8> {
    let mut v = vec![0u8; last_cid as usize / 8 + 1];
    for &c in cids {
        v[c as usize / 8] |= 1 << (7 - (c % 8));
    }
    v
}

// ---------------------------------------------------------------------------------------------
// CIDFont_type0_t1cdofont: name-keyed CFF
// ---------------------------------------------------------------------------------------------

fn t1c_dofont(cff: &CffFont, used: &BTreeSet<u16>) -> R<CffSubset> {
    let mut topdict = cff.topdict.clone();
    let private0 = cff.read_private_of(&cff.topdict)?;
    let gsubr = cff.read_gsubr()?;
    let subr0 = cff.read_subrs_of(&cff.topdict, &private0)?;

    let std_vw = match &private0 {
        Some(p) if p.known("StdVW") => Some(p.get("StdVW", 0)?),
        _ => None,
    };

    // The output charset replaces the font's; with a predefined charset dvipdfmx's
    // cff_pack_charsets writes nothing and the result is broken.
    if cff.charset_predefined().is_some() {
        return Err("Predefined CFF charsets not supported (charset entry missing)".into());
    }

    let mut cids: BTreeSet<u16> = used.clone();
    cids.insert(0); // .notdef
    let last_cid = *cids.iter().next_back().unwrap();
    if last_cid >= cff.num_glyphs {
        return Err(format!(
            "glyph {last_cid} is out of range (font has {} glyphs)",
            cff.num_glyphs
        ));
    }
    let num_glyphs = u16::try_from(cids.len()).map_err(|_| "too many glyphs".to_string())?;

    let charset: Vec<u16> = cids.iter().copied().skip(1).collect();
    let fdselect = vec![(0u16, 0u8)];

    let mut strings = Strings::new(cff);

    topdict.add("CIDCount", 1)?;
    topdict.set("CIDCount", 0, last_cid as f64 + 1.0)?;

    let mut fd0 = Dict::default();
    fd0.add("FontName", 1)?;
    // dvipdfmx: `font->fontname + 7` ("FIXME: Skip XXXXXX+") on the *untagged* name.
    let skip = cff.name.get(7..).unwrap_or(&[]);
    let sid = strings.add_unique(skip)?;
    fd0.set("FontName", 0, sid as f64)?;
    fd0.add("Private", 2)?;
    fd0.set("Private", 0, 0.0)?;
    fd0.set("Private", 1, 0.0)?;
    topdict.add("FDArray", 1)?;
    topdict.set("FDArray", 0, 0.0)?;
    topdict.add("FDSelect", 1)?;
    topdict.set("FDSelect", 0, 0.0)?;

    for k in ["UniqueID", "XUID", "Private", "Encoding"] {
        topdict.remove(k);
    }

    let (offs, data_start) = read_index_header(cff.d, cff.off("CharStrings", 0)?)?;
    if offs.len() < 3 {
        return Err("No valid charstring data found".into());
    }
    let mut cs = Cs::new();
    let mut cstrings: Vec<Vec<u8>> = Vec::with_capacity(cids.len());
    for &cid in &cids {
        let a = offs[cid as usize] as usize;
        let b = offs[cid as usize + 1] as usize;
        let size = b - a;
        if size > CS_STR_LEN_MAX {
            return Err(format!("Charstring too long (gid={cid})"));
        }
        let src = cff
            .d
            .get(data_start + a - 1..data_start + b - 1)
            .ok_or("File ended prematurely")?;
        let mut out = Vec::new();
        cs.copy(&mut out, src, Some(&gsubr), subr0.as_ref())?;
        cstrings.push(out);
    }

    // no Subrs
    let mut private0 = private0;
    if let Some(p) = &mut private0 {
        p.remove("Subrs");
    }

    strings.add_unique(b"Adobe")?;
    strings.add_unique(b"Identity")?;
    topdict.update(&mut strings)?;
    if let Some(p) = &mut private0 {
        p.update(&mut strings)?;
    }
    let adobe = strings.sid_of("Adobe")?;
    let identity = strings.sid_of("Identity")?;
    topdict.add("ROS", 3)?;
    topdict.set("ROS", 0, adobe as f64)?;
    topdict.set("ROS", 1, identity as f64)?;
    topdict.set("ROS", 2, 0.0)?;

    let data = write_fontfile(OutFont {
        major: cff.major,
        minor: cff.minor,
        name: cff.name.clone(),
        topdict,
        string: strings.new,
        charset,
        fdselect,
        cstrings,
        num_glyphs,
        fdarray: vec![Some(fd0)],
        private: vec![private0],
    })?;
    Ok(CffSubset { data, last_cid, cidset: cidset_of(&cids, last_cid), std_vw })
}

// ---------------------------------------------------------------------------------------------
// CIDFont_type0_dofont: CID-keyed CFF
// ---------------------------------------------------------------------------------------------

fn cid_dofont(cff: &CffFont, used: &BTreeSet<u16>) -> R<CffSubset> {
    let charsets = cff
        .read_charsets()?
        .ok_or("Predefined CFF charsets not supported yet")?;
    let cid_to_gid = charsets.cid_to_gid();

    // CIDToGIDMap through the font charset; CIDs without a glyph are dropped.
    let mut cids: BTreeMap<u16, u16> = BTreeMap::new();
    cids.insert(0, 0);
    for &cid in used {
        if cid == 0 {
            continue;
        }
        match cid_to_gid.get(&cid) {
            Some(&gid) if gid != 0 => {
                cids.insert(cid, gid);
            }
            _ => {} // "Glyph for CID %u missing in font": removed from the used set
        }
    }
    let last_cid = *cids.keys().next_back().unwrap();
    let num_glyphs = u16::try_from(cids.len()).map_err(|_| "too many glyphs".to_string())?;

    let fdsel = cff.read_fdselect()?;
    let mut fdarray = cff.read_fdarray()?;
    let num_fds = fdarray.len();
    let mut private: Vec<Option<Dict>> = Vec::with_capacity(num_fds);
    for fd in &fdarray {
        private.push(match fd {
            Some(fd) => cff.read_private_of(fd)?,
            None => None,
        });
    }
    let gsubr = cff.read_gsubr()?;
    let mut subrs: Vec<Option<Index>> = Vec::with_capacity(num_fds);
    for i in 0..num_fds {
        subrs.push(match &fdarray[i] {
            Some(fd) => cff.read_subrs_of(fd, &private[i])?,
            None => None,
        });
    }

    let (offs, data_start) = read_index_header(cff.d, cff.off("CharStrings", 0)?)?;
    if offs.len() < 3 {
        return Err("No valid charstring data found".into());
    }

    let mut cs = Cs::new();
    let mut cstrings: Vec<Vec<u8>> = Vec::with_capacity(cids.len());
    let mut charset: Vec<u16> = Vec::with_capacity(cids.len());
    let mut fdselect: Vec<(u16, u8)> = Vec::new();
    let mut prev_fd: i32 = -1;
    for (gid, (&cid, &gid_org)) in cids.iter().enumerate() {
        let a = *offs.get(gid_org as usize).ok_or("Invalid glyph index")? as usize;
        let b = *offs.get(gid_org as usize + 1).ok_or("Invalid glyph index")? as usize;
        let size = b - a;
        if size > CS_STR_LEN_MAX {
            return Err(format!("Charstring too long (gid={gid_org})"));
        }
        let src = cff
            .d
            .get(data_start + a - 1..data_start + b - 1)
            .ok_or("File ended prematurely")?;
        let fd = cff.fd_lookup(&fdsel, gid_org, num_fds)?;
        let mut out = Vec::new();
        cs.copy(&mut out, src, Some(&gsubr), subrs[fd as usize].as_ref())?;
        cstrings.push(out);
        if cid > 0 && gid_org > 0 {
            charset.push(cid);
        }
        if fd as i32 != prev_fd {
            fdselect.push((gid as u16, fd));
            prev_fd = fd as i32;
        }
    }

    // no Subrs
    for p in private.iter_mut().flatten() {
        p.remove("Subrs");
    }
    for fd in fdarray.iter_mut() {
        if fd.is_none() {
            return Err("empty Font DICT".into());
        }
    }

    let data = write_fontfile(OutFont {
        major: cff.major,
        minor: cff.minor,
        name: cff.name.clone(),
        topdict: cff.topdict.clone(),
        string: (0..cff.string.count()).map(|i| cff.string.item(i).to_vec()).collect(),
        charset,
        fdselect,
        cstrings,
        num_glyphs,
        fdarray,
        private,
    })?;
    let set: BTreeSet<u16> = cids.keys().copied().collect();
    Ok(CffSubset { data, last_cid, cidset: cidset_of(&set, last_cid), std_vw: None })
}

const STD_STRINGS: [&str; 391] = [
    ".notdef", "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand",
    "quoteright", "parenleft", "parenright", "asterisk", "plus", "comma", "hyphen", "period",
    "slash", "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    "colon", "semicolon", "less", "equal", "greater", "question", "at", "A", "B", "C", "D", "E",
    "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W",
    "X", "Y", "Z", "bracketleft", "backslash", "bracketright", "asciicircum", "underscore",
    "quoteleft", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p",
    "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "braceleft", "bar", "braceright",
    "asciitilde", "exclamdown", "cent", "sterling", "fraction", "yen", "florin", "section",
    "currency", "quotesingle", "quotedblleft", "guillemotleft", "guilsinglleft",
    "guilsinglright", "fi", "fl", "endash", "dagger", "daggerdbl", "periodcentered",
    "paragraph", "bullet", "quotesinglbase", "quotedblbase", "quotedblright", "guillemotright",
    "ellipsis", "perthousand", "questiondown", "grave", "acute", "circumflex", "tilde",
    "macron", "breve", "dotaccent", "dieresis", "ring", "cedilla", "hungarumlaut", "ogonek",
    "caron", "emdash", "AE", "ordfeminine", "Lslash", "Oslash", "OE", "ordmasculine", "ae",
    "dotlessi", "lslash", "oslash", "oe", "germandbls", "onesuperior", "logicalnot", "mu",
    "trademark", "Eth", "onehalf", "plusminus", "Thorn", "onequarter", "divide", "brokenbar",
    "degree", "thorn", "threequarters", "twosuperior", "registered", "minus", "eth", "multiply",
    "threesuperior", "copyright", "Aacute", "Acircumflex", "Adieresis", "Agrave", "Aring",
    "Atilde", "Ccedilla", "Eacute", "Ecircumflex", "Edieresis", "Egrave", "Iacute",
    "Icircumflex", "Idieresis", "Igrave", "Ntilde", "Oacute", "Ocircumflex", "Odieresis",
    "Ograve", "Otilde", "Scaron", "Uacute", "Ucircumflex", "Udieresis", "Ugrave", "Yacute",
    "Ydieresis", "Zcaron", "aacute", "acircumflex", "adieresis", "agrave", "aring", "atilde",
    "ccedilla", "eacute", "ecircumflex", "edieresis", "egrave", "iacute", "icircumflex",
    "idieresis", "igrave", "ntilde", "oacute", "ocircumflex", "odieresis", "ograve", "otilde",
    "scaron", "uacute", "ucircumflex", "udieresis", "ugrave", "yacute", "ydieresis", "zcaron",
    "exclamsmall", "Hungarumlautsmall", "dollaroldstyle", "dollarsuperior", "ampersandsmall",
    "Acutesmall", "parenleftsuperior", "parenrightsuperior", "twodotenleader", "onedotenleader",
    "zerooldstyle", "oneoldstyle", "twooldstyle", "threeoldstyle", "fouroldstyle",
    "fiveoldstyle", "sixoldstyle", "sevenoldstyle", "eightoldstyle", "nineoldstyle",
    "commasuperior", "threequartersemdash", "periodsuperior", "questionsmall", "asuperior",
    "bsuperior", "centsuperior", "dsuperior", "esuperior", "isuperior", "lsuperior",
    "msuperior", "nsuperior", "osuperior", "rsuperior", "ssuperior", "tsuperior", "ff", "ffi",
    "ffl", "parenleftinferior", "parenrightinferior", "Circumflexsmall", "hyphensuperior",
    "Gravesmall", "Asmall", "Bsmall", "Csmall", "Dsmall", "Esmall", "Fsmall", "Gsmall",
    "Hsmall", "Ismall", "Jsmall", "Ksmall", "Lsmall", "Msmall", "Nsmall", "Osmall", "Psmall",
    "Qsmall", "Rsmall", "Ssmall", "Tsmall", "Usmall", "Vsmall", "Wsmall", "Xsmall", "Ysmall",
    "Zsmall", "colonmonetary", "onefitted", "rupiah", "Tildesmall", "exclamdownsmall",
    "centoldstyle", "Lslashsmall", "Scaronsmall", "Zcaronsmall", "Dieresissmall", "Brevesmall",
    "Caronsmall", "Dotaccentsmall", "Macronsmall", "figuredash", "hypheninferior",
    "Ogoneksmall", "Ringsmall", "Cedillasmall", "questiondownsmall", "oneeighth",
    "threeeighths", "fiveeighths", "seveneighths", "onethird", "twothirds", "zerosuperior",
    "foursuperior", "fivesuperior", "sixsuperior", "sevensuperior", "eightsuperior",
    "ninesuperior", "zeroinferior", "oneinferior", "twoinferior", "threeinferior",
    "fourinferior", "fiveinferior", "sixinferior", "seveninferior", "eightinferior",
    "nineinferior", "centinferior", "dollarinferior", "periodinferior", "commainferior",
    "Agravesmall", "Aacutesmall", "Acircumflexsmall", "Atildesmall", "Adieresissmall",
    "Aringsmall", "AEsmall", "Ccedillasmall", "Egravesmall", "Eacutesmall", "Ecircumflexsmall",
    "Edieresissmall", "Igravesmall", "Iacutesmall", "Icircumflexsmall", "Idieresissmall",
    "Ethsmall", "Ntildesmall", "Ogravesmall", "Oacutesmall", "Ocircumflexsmall", "Otildesmall",
    "Odieresissmall", "OEsmall", "Oslashsmall", "Ugravesmall", "Uacutesmall",
    "Ucircumflexsmall", "Udieresissmall", "Yacutesmall", "Thornsmall", "Ydieresissmall",
    "001.000", "001.001", "001.002", "001.003", "Black", "Bold", "Book", "Light", "Medium",
    "Regular", "Roman", "Semibold",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::path::{Path, PathBuf};

    // ------------------------------------------------------------------------------------
    // Expectations below come from real xdvipdfmx 20260113 output: the `/CIDFontType0C` streams
    // of the TeX Live reference PDFs in /tmp/ratex-issues/tl-cache/*-fc2/work/main.pdf. Tests
    // that need those assets (or the original font files) skip silently when they are absent.
    // ------------------------------------------------------------------------------------

    const TL_CACHE: &str = "/tmp/ratex-issues/tl-cache";
    const FONT_ROOT: &str = "/usr/share/texmf-dist/fonts/opentype/public";

    /// All Flate-decoded `/Subtype/CIDFontType0C` streams of a pdf written by xdvipdfmx.
    fn cid_type0c_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
        let key = b"/Subtype/CIDFontType0C";
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(i) = find(&pdf[from..], key).map(|i| i + from) {
            from = i + key.len();
            let rest = &pdf[from..];
            let lpos = match find(&rest[..rest.len().min(200)], b"/Length ") {
                Some(p) => p + 8,
                None => continue,
            };
            let digits: String = rest[lpos..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .map(|&b| b as char)
                .collect();
            let len: usize = match digits.parse() {
                Ok(l) => l,
                Err(_) => continue,
            };
            let spos = match find(rest, b"stream") {
                Some(p) => p + 6,
                None => continue,
            };
            let mut s = spos;
            if rest[s] == b'\r' {
                s += 1;
            }
            if rest[s] == b'\n' {
                s += 1;
            }
            let mut dec = flate2::read::ZlibDecoder::new(&rest[s..s + len]);
            let mut buf = Vec::new();
            if dec.read_to_end(&mut buf).is_ok() {
                out.push(buf);
            }
        }
        out
    }

    fn find(h: &[u8], n: &[u8]) -> Option<usize> {
        h.windows(n.len()).position(|w| w == n)
    }

    fn tl_pdfs() -> Vec<(String, PathBuf)> {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(TL_CACHE) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                let p = e.path().join("work/main.pdf");
                if n.ends_with("-fc2") && p.exists() {
                    v.push((n, p));
                }
            }
        }
        v.sort();
        v
    }

    /// Locate an OpenType font by its PostScript name below the TeX Live font tree.
    fn find_font(name: &str) -> Option<PathBuf> {
        fn walk(dir: &Path, want: &str, depth: usize) -> Option<PathBuf> {
            for e in std::fs::read_dir(dir).ok()?.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if depth < 3 {
                        if let Some(r) = walk(&p, want, depth + 1) {
                            return Some(r);
                        }
                    }
                } else if p.extension().map_or(false, |x| x == "otf")
                    && p.file_stem().map_or(false, |s| s.to_string_lossy().to_lowercase() == want)
                {
                    return Some(p);
                }
            }
            None
        }
        let want = if name.starts_with("LatinModernMath") {
            "latinmodern-math".to_string()
        } else {
            name.to_lowercase()
        };
        // extra roots: DPX_CFF_FONT_DIRS=dir1:dir2 (not needed for the default checks)
        let extra = std::env::var("DPX_CFF_FONT_DIRS").unwrap_or_default();
        std::iter::once(FONT_ROOT)
            .chain(extra.split(':').filter(|d| !d.is_empty()))
            .find_map(|root| walk(Path::new(root), &want, 0))
    }

    /// The font a document used: a test font shipped with the document (`xe-corpus/<doc>/*.otf`,
    /// matched by CFF name) or the TeX Live font of that name.
    fn find_font_for(doc: &str, name: &str) -> Option<PathBuf> {
        let stem = doc.split("-main-").next().unwrap();
        let dir = Path::new("/home/leo/rv-build/xe-corpus").join(stem);
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map_or(false, |x| x == "otf") {
                    let Ok(b) = std::fs::read(&p) else { continue };
                    if let Ok(d) = cff_data(&b, 0) {
                        if CffFont::open(d).map_or(false, |c| c.name == name.as_bytes()) {
                            return Some(p);
                        }
                    }
                }
            }
        }
        find_font(name)
    }

    /// The glyph ids dvipdfmx used = the CIDs of the reference stream's charset.
    fn used_of_reference(reference: &[u8]) -> BTreeSet<u16> {
        let cff = CffFont::open(reference).unwrap();
        assert!(cff.is_cid);
        let cs = cff.read_charsets().unwrap().unwrap();
        let mut set: BTreeSet<u16> = cs.glyphs.iter().copied().collect();
        set.insert(0);
        set
    }

    /// Subset every font of every TL reference PDF and compare with the embedded stream.
    /// Returns (number compared, names).
    fn compare_with_tl(only: Option<&str>) -> Vec<String> {
        let mut compared = Vec::new();
        for (doc, pdf) in tl_pdfs() {
            if let Some(o) = only {
                if !doc.starts_with(o) {
                    continue;
                }
            }
            let bytes = std::fs::read(&pdf).unwrap();
            for reference in cid_type0c_streams(&bytes) {
                let name = {
                    let c = CffFont::open(&reference).unwrap();
                    String::from_utf8_lossy(&c.name).into_owned()
                };
                let Some(path) = find_font_for(&doc, &name) else { continue };
                let otf = std::fs::read(path).unwrap();
                let used = used_of_reference(&reference);
                let got = subset_cid_cff(&otf, 0, &used, &format!("ABCDEF+{name}"))
                    .unwrap_or_else(|e| panic!("{doc} {name}: {e}"));
                assert!(
                    got.data == reference,
                    "{doc} {name}: subset differs from xdvipdfmx output ({} vs {} bytes, first \
                     difference at {:?})",
                    got.data.len(),
                    reference.len(),
                    got.data.iter().zip(&reference).position(|(a, b)| a != b)
                );
                assert_eq!(got.last_cid, *used.iter().next_back().unwrap());
                assert_eq!(got.cidset, cidset_of(&used, got.last_cid));
                compared.push(format!("{doc}:{name}"));
            }
        }
        compared
    }

    #[test]
    fn hello_lmroman10_matches_xdvipdfmx() {
        let n = compare_with_tl(Some("hello-"));
        if find_font("lmroman10-regular").is_some() && !tl_pdfs().is_empty() {
            assert!(!n.is_empty(), "hello reference PDF not compared");
        }
    }

    #[test]
    fn shaping_and_native_fonts_match_xdvipdfmx() {
        for doc in ["font_shaping_features-", "font_opentype_cff-", "font_fontspec_native-"] {
            let n = compare_with_tl(Some(doc));
            if find_font("lmroman10-regular").is_some() && !tl_pdfs().is_empty() {
                assert!(!n.is_empty(), "{doc}: nothing compared");
            }
        }
    }

    #[test]
    fn reference_cidset_matches_pdf_semantics() {
        // CIDSet: bit (7 - cid % 8) of byte cid / 8; cid 0 always set.
        let used: BTreeSet<u16> = [0u16, 1, 9, 20].into_iter().collect();
        assert_eq!(cidset_of(&used, 20), vec![0b1100_0000, 0b0100_0000, 0b0000_1000]);
    }

    #[test]
    fn cff_info_agrees_with_ttf_parser() {
        let Some(path) = find_font("lmroman10-regular") else { return };
        let otf = std::fs::read(path).unwrap();
        let info = cff_info(&otf, 0).unwrap();
        let face = ttf_parser::Face::parse(&otf, 0).unwrap();
        assert!(!info.is_cid);
        assert_eq!(info.num_glyphs, face.number_of_glyphs());
        assert_eq!(info.font_name, "LMRoman10-Regular");
        assert_eq!(info.glyph_names.len(), info.num_glyphs as usize);
        for gid in 0..info.num_glyphs {
            let want = face.glyph_name(ttf_parser::GlyphId(gid)).map(str::to_string);
            assert_eq!(info.glyph_names[gid as usize], want, "gid {gid}");
        }
        assert!(info.cid_to_gid.is_none());
    }

    #[test]
    fn cff_info_cid_keyed_charset() {
        let Some(path) = find_font("FandolHei-Regular") else { return };
        let otf = std::fs::read(path).unwrap();
        let info = cff_info(&otf, 0).unwrap();
        assert!(info.is_cid);
        assert!(info.glyph_names.is_empty());
        let m = info.cid_to_gid.unwrap();
        assert_eq!(m[&0], 0);
        assert!(m.len() > 1000);
        assert!(info.cid_count.is_some());
        // the charset is a bijection CID <-> GID 1.. in file order
        let face = ttf_parser::Face::parse(&otf, 0).unwrap();
        assert!(m.values().all(|&g| g < face.number_of_glyphs()));
    }

    #[test]
    fn cid_keyed_subset_keeps_cids_and_fds() {
        let Some(path) = find_font("FandolHei-Regular") else { return };
        let otf = std::fs::read(path).unwrap();
        let info = cff_info(&otf, 0).unwrap();
        let map = info.cid_to_gid.unwrap();
        let cids: Vec<u16> = map.keys().copied().filter(|&c| c != 0).step_by(997).take(8).collect();
        let mut used: BTreeSet<u16> = cids.iter().copied().collect();
        used.insert(60000); // not in the font: dropped like dvipdfmx does
        let name = "FandolHei-Regular";
        let got = subset_cid_cff(&otf, 0, &used, &format!("ABCDEF+{name}")).unwrap();
        assert_eq!(got.last_cid, *cids.iter().max().unwrap());
        assert!(got.std_vw.is_none());
        let out = CffFont::open(&got.data).unwrap();
        assert!(out.is_cid);
        assert_eq!(out.num_glyphs as usize, cids.len() + 1);
        let cs = out.read_charsets().unwrap().unwrap();
        assert_eq!(cs.glyphs, cids);
        // glyph programs are the desubroutinized originals
        let orig = CffFont::open(cff_data(&otf, 0).unwrap()).unwrap();
        assert_eq!(out.topdict.get("CIDCount", 0).unwrap(), orig.topdict.get("CIDCount", 0).unwrap());
        let (offs, ds) = read_index_header(out.d, out.off("CharStrings", 0).unwrap()).unwrap();
        assert_eq!(offs.len(), out.num_glyphs as usize + 1);
        let fdsel = out.read_fdselect().unwrap();
        assert!(matches!(fdsel, FdSelect::F3(_)));
        // fonttools-independent sanity: every output charstring ends with endchar
        for i in 0..out.num_glyphs as usize {
            let cs = &out.d[ds + offs[i] as usize - 1..ds + offs[i + 1] as usize - 1];
            assert_eq!(*cs.last().unwrap(), 14);
        }
    }

    #[test]
    fn rejects_what_xdvipdfmx_rejects() {
        let used: BTreeSet<u16> = BTreeSet::new();
        // TrueType outlines are not CFF
        let ttf = [0u8, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(subset_cid_cff(&ttf, 0, &used, "ABCDEF+X").is_err());
        assert!(subset_cid_cff(b"junk", 0, &used, "ABCDEF+X").is_err());
        if let Some(path) = find_font("lmroman10-regular") {
            let otf = std::fs::read(path).unwrap();
            // wrong BaseFont
            assert!(subset_cid_cff(&otf, 0, &used, "ABCDEF+Other").is_err());
            assert!(subset_cid_cff(&otf, 0, &used, "LMRoman10-Regular").is_err());
            // glyph beyond the font
            let big: BTreeSet<u16> = [60000u16].into_iter().collect();
            assert!(subset_cid_cff(&otf, 0, &big, "ABCDEF+LMRoman10-Regular").is_err());
            // face index of a non-TTC is ignored (sfnt.c does so)
            assert!(subset_cid_cff(&otf, 7, &used, "ABCDEF+LMRoman10-Regular").is_ok());
            // only .notdef
            let g = subset_cid_cff(&otf, 0, &used, "ABCDEF+LMRoman10-Regular").unwrap();
            assert_eq!(g.last_cid, 0);
            assert_eq!(g.cidset, vec![0x80]);
            assert_eq!(g.std_vw, Some(69.0));
        }
    }

    #[test]
    fn ttc_face_selects_cff_table_directory() {
        let Some(path) = find_font("lmroman10-regular") else { return };
        let otf = std::fs::read(path).unwrap();
        // wrap the OTF into a one-face TTC: header (12 + 4 bytes), table offsets shifted by 16
        let mut ttc = Vec::new();
        ttc.extend_from_slice(b"ttcf");
        ttc.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 16]);
        ttc.extend_from_slice(&otf);
        let n = u16::from_be_bytes([otf[4], otf[5]]) as usize;
        for i in 0..n {
            let at = 16 + 12 + i * 16 + 8;
            let off = u32::from_be_bytes(ttc[at..at + 4].try_into().unwrap()) + 16;
            ttc[at..at + 4].copy_from_slice(&off.to_be_bytes());
        }
        ttc[16..20].copy_from_slice(b"OTTO");
        let used: BTreeSet<u16> = [5u16, 40, 77].into_iter().collect();
        let name = "ABCDEF+LMRoman10-Regular";
        let a = subset_cid_cff(&otf, 0, &used, name).unwrap();
        let b = subset_cid_cff(&ttc, 0, &used, name).unwrap();
        assert!(a.data == b.data);
        assert!(subset_cid_cff(&ttc, 1, &used, name).is_err());
        assert_eq!(cff_info(&ttc, 0).unwrap().num_glyphs, cff_info(&otf, 0).unwrap().num_glyphs);
    }

    // --- DICT numbers ---------------------------------------------------------------------

    #[test]
    fn dict_number_packing() {
        let mut b = Vec::new();
        pack_real(&mut b, 0.04546);
        assert_eq!(b, [30, 0x0a, 0x04, 0x54, 0x6f]);
        b.clear();
        pack_real(&mut b, -0.5);
        assert_eq!(b, [30, 0xe0, 0xa5, 0xff]);
        b.clear();
        pack_real(&mut b, 1e-5);
        assert_eq!(b, [30, 0x1c, 0x05, 0xff]);
        b.clear();
        pack_real(&mut b, 0.0);
        assert_eq!(b, [30, 0x0f]);
        assert_eq!(fmt_g13(100.0), "100");
        assert_eq!(fmt_g13(1.0 / 3.0), "0.3333333333333");
        assert_eq!(fmt_g13(123456789012345.0), "1.234567890123e+14");
        assert_eq!(fmt_g13(0.0001), "0.0001");
        assert_eq!(fmt_g13(0.00001234), "1.234e-05");
        // integers use the shortest form, offsets always five bytes
        let mut d = Vec::new();
        put_number(&mut d, 107.0, false);
        put_number(&mut d, 108.0, false);
        put_number(&mut d, -1131.0, false);
        put_number(&mut d, 1132.0, false);
        put_number(&mut d, 5.0, true);
        assert_eq!(&d[..], &[246, 247, 0, 254, 255, 28, 4, 108, 29, 0, 0, 0, 5][..]);
    }

    #[test]
    fn dict_roundtrip_and_order() {
        // version=391, ROS=(390 451 0) in the middle, FontBBox=0 0 600 600
        let raw: Vec<u8> = vec![
            0x1d, 0, 0, 1, 0x87, 0, // version 391 (5-byte int)
            248, 26, 248, 87, 139, 12, 30, // ROS
            139, 139, 248, 236, 248, 236, 5, // FontBBox
        ];
        let d = Dict::unpack(&raw).unwrap();
        assert_eq!(d.get("version", 0).unwrap(), 391.0);
        assert_eq!(d.get("FontBBox", 3).unwrap(), 600.0);
        assert_eq!(d.get("ROS", 1).unwrap(), 451.0);
        assert!(d.known("ROS") && !d.known("Private"));
        // ROS is packed first, integers in their shortest form
        assert_eq!(
            d.pack(),
            vec![248, 26, 248, 87, 139, 12, 30, 248, 27, 0, 139, 139, 248, 236, 248, 236, 5]
        );
        let mut d = d;
        d.remove("ROS");
        assert!(!d.known("ROS"));
        assert_eq!(d.pack(), vec![248, 27, 0, 139, 139, 248, 236, 248, 236, 5]);
    }

    // --- charstrings ----------------------------------------------------------------------

    fn index_of<'a>(items: &[&[u8]], store: &'a mut Vec<u8>) -> Index<'a> {
        let mut offs = vec![1u32];
        store.clear();
        for it in items {
            store.extend_from_slice(it);
            offs.push(store.len() as u32 + 1);
        }
        Index { offs, data: &store[..] }
    }

    #[test]
    fn desubroutinizer_inlines_and_reencodes() {
        // local subr 0: 10 20 rmoveto return; global subr 0: 30 40 rlineto return
        let (mut ls, mut gs) = (Vec::new(), Vec::new());
        let local = index_of(&[&[149, 159, 21, 11]], &mut ls);
        let global = index_of(&[&[169, 179, 5, 11]], &mut gs);
        // 50 (width)  -107 callsubr  -107 callgsubr  endchar   (bias 107 -> subr number 0)
        let src = [189, 32, 10, 32, 29, 14];
        let mut cs = Cs::new();
        let mut out = Vec::new();
        cs.copy(&mut out, &src, Some(&global), Some(&local)).unwrap();
        assert_eq!(out, [189, 149, 159, 21, 169, 179, 5, 14]);

        // operands are re-encoded: shortint 10 -> one byte, 1024 -> two bytes, arithmetic is
        // evaluated, 16.16 fixed values stay fixed, integral fixed values become integers
        let src = [
            28, 0, 10, // 10
            28, 4, 0, // 1024
            139, 143, 32, // 0 4 -107
            12, 10, // add: -103
            255, 0, 3, 0x80, 0, // 3.5
            255, 0, 7, 0, 0, // 7.0
            21, // rmoveto
            14,
        ];
        let mut out = Vec::new();
        Cs::new().copy(&mut out, &src, None, None).unwrap();
        assert_eq!(out, vec![149, 250, 148, 139, 36, 255, 0, 3, 128, 0, 146, 21, 14]);
    }

    #[test]
    fn desubroutinizer_counts_stems_for_hintmask() {
        // w 0 10 hstemhm | 20 30 (implicit vstem) hintmask <2 stems + 1 = 3 -> one mask byte>
        // then 5 6 rmoveto, 7 hlineto, endchar
        let src = [
            144, 139, 149, 18, // 5 0 10 hstemhm   (width + 1 stem)
            159, 169, 19, 0xe0, // 20 30 hintmask + mask byte
            144, 145, 21, 146, 6, 14,
        ];
        let mut out = Vec::new();
        Cs::new().copy(&mut out, &src, None, None).unwrap();
        assert_eq!(out, src);
        // 9 stems need two mask bytes
        let mut src = vec![139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 18];
        src.extend_from_slice(&[19, 0xff, 0x80, 139, 139, 21, 14]);
        let mut out = Vec::new();
        Cs::new().copy(&mut out, &src, None, None).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn desubroutinizer_rejects_like_dvipdfmx() {
        let mut out = Vec::new();
        // path operator before any moveto
        assert!(Cs::new().copy(&mut out, &[139, 139, 5, 14], None, None).is_err());
        // call without subroutines
        assert!(Cs::new().copy(&mut out, &[32, 10, 14], None, None).is_err());
        // deprecated seac-style endchar
        assert!(Cs::new().copy(&mut out, &[139, 139, 139, 139, 14], None, None).is_err());
        // reserved operator
        assert!(Cs::new().copy(&mut out, &[9], None, None).is_err());
        // hintmask mask bytes missing
        assert!(Cs::new().copy(&mut out, &[139, 140, 1, 19], None, None).is_err());
    }
}
