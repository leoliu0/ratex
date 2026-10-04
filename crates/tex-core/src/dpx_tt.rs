//! CIDFontType2 TrueType subsets written like xdvipdfmx (cidtype2.c).
//!
//! This is a line-by-line port of the embedding path of
//! `CIDFont_type2_dofont` for `Adobe-Identity` (glyph-ordering) fonts, i.e. what
//! xdvipdfmx does for XeTeX native TrueType fonts: CID == original GID,
//! `tt_build_init`/`tt_add_glyph`/`tt_build_tables` (tt_glyf.c), the table
//! selection and file assembly of `sfnt_create_FontFile_stream` (sfnt.c),
//! `add_TTCIDHMetrics`, the `CIDSet` and the `CIDToGIDMap` decision.
//!
//! Behaviours worth knowing (all inherited from the C code):
//! * The tables written are, in the *original directory order*, those of
//!   `OS/2 head hhea loca maxp name glyf hmtx fpgm cvt  prep` that exist in the
//!   source font (`name`, `head`, `hhea`, `loca`, `maxp`, `glyf`, `hmtx` must
//!   exist). `cmap`, `post` and everything else are dropped.
//! * `head.checkSumAdjustment` is written as 0 and is *not* recomputed;
//!   unmodified tables keep the checksum recorded in the source directory.
//! * Glyph IDs are preserved. Composite components that are not used by
//!   themselves are put into the lowest free slot and the composite is patched.
//! * `CIDToGIDMap` is always `/Identity` (`NO_GHOSTSCRIPT_BUG` is not defined).
//!
//! The C implementation leaves `ury` of empty glyphs and typo ascender/
//! descender of an existing OS/2 table shorter than 78 bytes uninitialized.
//! Their vertical metrics depend on heap residue, so no deterministic port
//! can reproduce them; this module initializes those fields to zero.

use std::collections::BTreeSet;

/// Result of embedding a TrueType font as a CIDFontType2 subset.
#[derive(Debug, Clone, PartialEq)]
pub struct TtSubset {
    /// The TrueType font program (FontFile2 payload before Flate, `/Length1` is
    /// its length). Glyph IDs are preserved (CID == GID, glyf entries of unused
    /// glyphs empty) exactly as dvipdfmx does.
    pub data: Vec<u8>,
    /// Highest glyph slot used (`glyphs->last_gid`).
    pub last_gid: u16,
    /// `/CIDSet` payload: `last_gid/8 + 1` bytes, bit `7 - gid%8` of byte
    /// `gid/8` set for every gid in `1..=last_gid` (`.notdef` omitted).
    pub cidset: Vec<u8>,
    /// `/CIDToGIDMap`: `None` means `/CIDToGIDMap /Identity`.
    pub cid_to_gid_map: Option<Vec<u8>>,
    /// `/DW` as printed by `add_TTCIDHMetrics`.
    pub dw: f64,
    /// `/W` entries `start [w w ...]` (widths in 1/1000 em, rounded to integers
    /// like `PDFUNIT`); empty when the C code writes no `/W`.
    pub w: Vec<(u32, Vec<f64>)>,
}

const NUM_GLYPH_LIMIT: usize = 65534;

const SFNT_TRUETYPE: u32 = 0x0001_0000;
const SFNT_MAC_TRUE: u32 = 0x7472_7565;
const SFNT_TTC: u32 = 0x7474_6366;

const TT_HEAD_TABLE_SIZE: usize = 54;
const TT_MAXP_TABLE_SIZE: usize = 32;
const TT_HHEA_TABLE_SIZE: usize = 36;

const ARG_1_AND_2_ARE_WORDS: u16 = 1 << 0;
const WE_HAVE_A_SCALE: u16 = 1 << 3;
const MORE_COMPONENT: u16 = 1 << 5;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 1 << 6;
const WE_HAVE_A_TWO_BY_TWO: u16 = 1 << 7;

/// `required_table[]` of cidtype2.c: (tag, must_exist).
const REQUIRED_TABLES: [(&[u8; 4], bool); 11] = [
    (b"OS/2", false),
    (b"head", true),
    (b"hhea", true),
    (b"loca", true),
    (b"maxp", true),
    (b"name", true),
    (b"glyf", true),
    (b"hmtx", true),
    (b"fpgm", false),
    (b"cvt ", false),
    (b"prep", false),
];

// ---------------------------------------------------------------------------
// Raw big-endian access (the C code seeks in the file stream without bounds
// other than EOF, so all reads are file-relative, not table-relative).
// ---------------------------------------------------------------------------

fn get<const N: usize>(f: &[u8], pos: usize) -> Result<[u8; N], String> {
    pos.checked_add(N)
        .and_then(|end| f.get(pos..end))
        .map(|s| {
            let mut a = [0u8; N];
            a.copy_from_slice(s);
            a
        })
        .ok_or_else(|| format!("unexpected end of font file at offset {pos}"))
}

fn u16_at(f: &[u8], pos: usize) -> Result<u16, String> {
    Ok(u16::from_be_bytes(get::<2>(f, pos)?))
}

fn i16_at(f: &[u8], pos: usize) -> Result<i16, String> {
    Ok(i16::from_be_bytes(get::<2>(f, pos)?))
}

fn u32_at(f: &[u8], pos: usize) -> Result<u32, String> {
    Ok(u32::from_be_bytes(get::<4>(f, pos)?))
}

// ---------------------------------------------------------------------------
// sfnt.c
// ---------------------------------------------------------------------------

struct SfntTable {
    tag: [u8; 4],
    check_sum: u32,
    offset: u32,
    length: u32,
    data: Option<Vec<u8>>,
    required: bool,
}

struct Sfnt<'a> {
    file: &'a [u8],
    version: u32,
    tables: Vec<SfntTable>,
    num_kept_tables: usize,
}

impl<'a> Sfnt<'a> {
    /// `sfnt_open` + the type switch of `CIDFont_type2_dofont` +
    /// `sfnt_read_table_directory`.
    fn open(file: &'a [u8], face_index: u32) -> Result<Self, String> {
        let kind = u32_at(file, 0).map_err(|_| "not a TrueType/TTC font".to_string())?;
        let offset = match kind {
            SFNT_TRUETYPE | SFNT_MAC_TRUE => {
                if face_index > 0 {
                    return Err(
                        "Found TrueType font file while expecting TTC file (face index > 0)"
                            .to_string(),
                    );
                }
                0usize
            }
            SFNT_TTC => {
                // ttc_read_offset
                let num_dirs = u32_at(file, 8)?;
                if face_index >= num_dirs {
                    return Err("Invalid TTC index number".to_string());
                }
                let off = u32_at(file, 12 + face_index as usize * 4)?;
                if off == 0 {
                    return Err("Invalid TTC index".to_string());
                }
                off as usize
            }
            _ => return Err("Not a TrueType/TTC font".to_string()),
        };
        let version = u32_at(file, offset)?;
        let num_tables = u16_at(file, offset + 4)? as usize;
        let mut tables = Vec::with_capacity(num_tables);
        for i in 0..num_tables {
            let p = offset + 12 + i * 16;
            tables.push(SfntTable {
                tag: get::<4>(file, p)?,
                check_sum: u32_at(file, p + 4)?,
                offset: u32_at(file, p + 8)?,
                length: u32_at(file, p + 12)?,
                data: None,
                required: false,
            });
        }
        Ok(Sfnt { file, version, tables, num_kept_tables: 0 })
    }

    fn index(&self, tag: &[u8; 4]) -> Option<usize> {
        self.tables.iter().position(|t| &t.tag == tag)
    }

    /// `sfnt_find_table_pos` (0 when absent).
    fn pos(&self, tag: &[u8; 4]) -> u32 {
        self.index(tag).map_or(0, |i| self.tables[i].offset)
    }

    /// `sfnt_find_table_len` (0 when absent).
    fn len(&self, tag: &[u8; 4]) -> u32 {
        self.index(tag).map_or(0, |i| self.tables[i].length)
    }

    /// `sfnt_locate_table`: an offset of 0 counts as missing.
    fn locate(&self, tag: &[u8; 4]) -> Result<usize, String> {
        match self.pos(tag) {
            0 => Err(format!(
                "sfnt: table not found ({})",
                String::from_utf8_lossy(tag)
            )),
            p => Ok(p as usize),
        }
    }

    /// `sfnt_set_table`.
    fn set_table(&mut self, tag: &[u8; 4], data: Vec<u8>) {
        let check_sum = calc_checksum(&data);
        let length = data.len() as u32;
        let idx = match self.index(tag) {
            Some(i) => i,
            None => {
                self.tables.push(SfntTable {
                    tag: *tag,
                    check_sum: 0,
                    offset: 0,
                    length: 0,
                    data: None,
                    required: false,
                });
                self.tables.len() - 1
            }
        };
        let t = &mut self.tables[idx];
        t.check_sum = check_sum;
        t.offset = 0;
        t.length = length;
        t.data = Some(data);
    }

    /// `sfnt_require_table`.
    fn require(&mut self, tag: &[u8; 4], must_exist: bool) -> Result<(), String> {
        match self.index(tag) {
            None if must_exist => Err(format!(
                "Some required TrueType table ({}) does not exist.",
                String::from_utf8_lossy(tag)
            )),
            None => Ok(()),
            Some(i) => {
                self.tables[i].required = true;
                self.num_kept_tables += 1;
                Ok(())
            }
        }
    }

    /// `sfnt_create_FontFile_stream` (the uncompressed payload).
    fn create_font_file(&self) -> Result<Vec<u8>, String> {
        let n = self.num_kept_tables;
        let length = self.tables.iter().filter(|t| t.required)
            .fold(12 + 16 * n, |off, t| off.next_multiple_of(4) + t.length as usize);
        let mut out = Vec::with_capacity(length);
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&(n as u16).to_be_bytes());
        let sr = max2floor(n) * 16;
        out.extend_from_slice(&(sr as u16).to_be_bytes());
        out.extend_from_slice(&(log2floor(n) as u16).to_be_bytes());
        out.extend_from_slice(&((n * 16 - sr) as u16).to_be_bytes());

        let mut offset = 12 + 16 * n;
        for t in self.tables.iter().filter(|t| t.required) {
            offset = offset.next_multiple_of(4);
            out.extend_from_slice(&t.tag);
            out.extend_from_slice(&t.check_sum.to_be_bytes());
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.extend_from_slice(&t.length.to_be_bytes());
            offset += t.length as usize;
        }

        let mut offset = 12 + 16 * n;
        for t in self.tables.iter().filter(|t| t.required) {
            let aligned = offset.next_multiple_of(4);
            out.resize(out.len() + (aligned - offset), 0);
            offset = aligned;
            match &t.data {
                Some(d) => out.extend_from_slice(d),
                None => {
                    let start = t.offset as usize;
                    let end = start + t.length as usize;
                    let s = self
                        .file
                        .get(start..end)
                        .ok_or_else(|| "Reading file failed...".to_string())?;
                    out.extend_from_slice(s);
                }
            }
            offset += t.length as usize;
        }
        debug_assert_eq!(out.len(), offset);
        Ok(out)
    }
}

/// Max power of 2 <= n (1 for n <= 1).
fn max2floor(mut n: usize) -> usize {
    let mut val = 1;
    while n > 1 {
        n /= 2;
        val *= 2;
    }
    val
}

fn log2floor(mut n: usize) -> usize {
    let mut val = 0;
    while n > 1 {
        n /= 2;
        val += 1;
    }
    val
}

/// `sfnt_calc_checksum`: big-endian u32 sum, short tail zero padded.
fn calc_checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for (i, &b) in data.iter().enumerate() {
        sum = sum.wrapping_add((b as u32) << (8 * (3 - (i & 3))));
    }
    sum
}

// ---------------------------------------------------------------------------
// tt_table.c readers
// ---------------------------------------------------------------------------

/// `tt_read_longMetrics`: (advance, sideBearing) for every glyph.
fn read_long_metrics(
    f: &[u8],
    mut pos: usize,
    num_glyphs: u16,
    num_long: u16,
    num_ex: u16,
) -> Result<Vec<(u16, i16)>, String> {
    let mut m = Vec::with_capacity(num_glyphs as usize);
    let mut last_adv = 0u16;
    let mut last_esb = 0i16;
    for gid in 0..num_glyphs as usize {
        if gid < num_long as usize {
            last_adv = u16_at(f, pos)?;
            pos += 2;
        }
        if gid < num_long as usize + num_ex as usize {
            last_esb = i16_at(f, pos)?;
            pos += 2;
        }
        m.push((last_adv, last_esb));
    }
    Ok(m)
}

/// `numOfExSideBearings` of `tt_read_hhea_table`/`tt_read_vhea_table`.
fn num_ex_side_bearings(len: u32, num_long: u16) -> u16 {
    (len.wrapping_sub((num_long as u32).wrapping_mul(4)) / 2) as u16
}

// ---------------------------------------------------------------------------
// tt_glyf.c
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct GlyphDesc {
    gid: u16,
    ogid: u16,
    advw: u16,
    advh: u16,
    lsb: i16,
    tsb: i16,
    #[allow(dead_code)]
    llx: i16,
    #[allow(dead_code)]
    lly: i16,
    #[allow(dead_code)]
    urx: i16,
    ury: i16,
    length: u32,
    data: Vec<u8>,
}

struct Glyphs {
    gd: Vec<GlyphDesc>,
    last_gid: u16,
    emsize: u16,
    dw: u16,
    default_advh: u16,
    default_tsb: i16,
    used_slot: Vec<u8>,
}

impl Glyphs {
    /// `tt_build_init`.
    fn new() -> Result<Self, String> {
        let mut g = Glyphs {
            gd: Vec::new(),
            last_gid: 0,
            emsize: 1,
            dw: 0,
            default_advh: 0,
            default_tsb: 0,
            used_slot: vec![0; 8192],
        };
        g.add_glyph(0, 0)?;
        Ok(g)
    }

    fn slot_used(&self, gid: u16) -> bool {
        self.used_slot[gid as usize / 8] & (1 << (7 - gid % 8)) != 0
    }

    /// `find_empty_slot`.
    fn find_empty_slot(&self) -> Result<u16, String> {
        (0..NUM_GLYPH_LIMIT as u16)
            .find(|&gid| !self.slot_used(gid))
            .ok_or_else(|| "No empty glyph slot available.".to_string())
    }

    /// `tt_find_glyph`: new gid of the glyph whose original gid is `gid`, 0 if
    /// none (note that this is also 0 for `.notdef`).
    fn find_glyph(&self, gid: u16) -> u16 {
        self.gd.iter().find(|d| d.ogid == gid).map_or(0, |d| d.gid)
    }

    /// `tt_get_index`: index into the sorted `gd` (metrics are queried only
    /// after `tt_build_tables` sorts it), 0 if none.
    fn get_index(&self, gid: u16) -> usize {
        self.gd.binary_search_by_key(&gid, |d| d.gid).unwrap_or(0)
    }

    /// `tt_add_glyph`.
    fn add_glyph(&mut self, gid: u16, new_gid: u16) -> Result<u16, String> {
        // (a slot that is already taken only emits a warning in C)
        if !self.slot_used(new_gid) {
            if self.gd.len() + 1 >= NUM_GLYPH_LIMIT {
                return Err("Too many glyphs.".to_string());
            }
            self.gd.push(GlyphDesc { gid: new_gid, ogid: gid, ..GlyphDesc::default() });
            self.used_slot[new_gid as usize / 8] |= 1 << (7 - new_gid % 8);
        }
        if new_gid > self.last_gid {
            self.last_gid = new_gid;
        }
        Ok(new_gid)
    }
}

/// `tt_build_tables`; fills `g` (metrics, dw) and replaces head, hhea, maxp,
/// hmtx, loca and glyf in `sfont`.
fn tt_build_tables(sfont: &mut Sfnt, g: &mut Glyphs) -> Result<(), String> {
    let f = sfont.file;

    if g.gd.len() > NUM_GLYPH_LIMIT {
        return Err("Too many glyphs.".to_string());
    }

    // head, hhea, maxp
    let head_pos = sfont.locate(b"head")?;
    let mut head: [u8; TT_HEAD_TABLE_SIZE] = get(f, head_pos)?;
    let hhea_pos = sfont.locate(b"hhea")?;
    let mut hhea: [u8; TT_HHEA_TABLE_SIZE] = get(f, hhea_pos)?;
    let maxp_pos = sfont.locate(b"maxp")?;
    let mut maxp: [u8; TT_MAXP_TABLE_SIZE] = get(f, maxp_pos)?;

    let metric_data_format = i16_at(&hhea, 32)?;
    if metric_data_format != 0 {
        return Err("Unknown metricDataFormat.".to_string());
    }
    let num_of_long_hor_metrics = u16_at(&hhea, 34)?;
    let units_per_em = u16_at(&head, 18)?;
    let index_to_loc_format = i16_at(&head, 50)?;
    let num_glyphs = u16_at(&maxp, 4)?;
    if units_per_em == 0 {
        return Err("Invalid unitsPerEm (0).".to_string());
    }
    g.emsize = units_per_em;

    // hmtx
    let hmtx_pos = sfont.locate(b"hmtx")?;
    let num_ex = num_ex_side_bearings(sfont.len(b"hmtx"), num_of_long_hor_metrics);
    let hmtx = read_long_metrics(f, hmtx_pos, num_glyphs, num_of_long_hor_metrics, num_ex)?;

    // OS/2 (tt_read_os2__table). Missing table uses its defined 880/-120
    // defaults; a short existing table has uninitialized typo fields in C,
    // represented by zero here rather than reading heap residue.
    let (typo_asc, typo_desc): (i32, i32) = if sfont.pos(b"OS/2") > 0 {
        let p = sfont.locate(b"OS/2")?;
        if sfont.len(b"OS/2") >= 78 {
            (i16_at(f, p + 68)? as i32, i16_at(f, p + 70)? as i32)
        } else {
            (0, 0)
        }
    } else {
        (880, -120)
    };
    g.default_advh = (typo_asc - typo_desc) as u16;
    g.default_tsb = (g.default_advh as i32 - typo_asc) as i16;

    // vmtx
    let vmtx = if sfont.pos(b"vmtx") > 0 {
        let vhea_pos = sfont.locate(b"vhea")?;
        let num_long_ver = u16_at(f, vhea_pos + 34)?;
        let vmtx_pos = sfont.locate(b"vmtx")?;
        let num_ex_v = num_ex_side_bearings(sfont.len(b"vmtx"), num_long_ver);
        Some(read_long_metrics(f, vmtx_pos, num_glyphs, num_long_ver, num_ex_v)?)
    } else {
        None
    };

    // loca
    let loca_pos = sfont.locate(b"loca")?;
    let mut location = Vec::with_capacity(num_glyphs as usize + 1);
    match index_to_loc_format {
        0 => {
            for i in 0..=num_glyphs as usize {
                location.push(2 * u16_at(f, loca_pos + 2 * i)? as u32);
            }
        }
        1 => {
            for i in 0..=num_glyphs as usize {
                location.push(u32_at(f, loca_pos + 4 * i)?);
            }
        }
        _ => return Err("Unknown IndexToLocFormat.".to_string()),
    }

    let mut w_stat = vec![0u16; g.emsize as usize + 2];
    let glyf_off = sfont.locate(b"glyf")?;

    // The glyph array may grow while composite glyphs are patched.
    let mut i = 0;
    while i < NUM_GLYPH_LIMIT && i < g.gd.len() {
        let gid = g.gd[i].ogid;
        if gid >= num_glyphs {
            return Err(format!("Invalid glyph index (gid {gid})"));
        }
        let loc = location[gid as usize];
        let len = location[gid as usize + 1].wrapping_sub(loc);
        {
            let d = &mut g.gd[i];
            d.advw = hmtx[gid as usize].0;
            d.lsb = hmtx[gid as usize].1;
            if let Some(vmtx) = &vmtx {
                d.advh = vmtx[gid as usize].0;
                d.tsb = vmtx[gid as usize].1;
            } else {
                d.advh = g.default_advh;
                d.tsb = g.default_tsb;
            }
            d.length = len;
            d.data = Vec::new();
            if d.advw <= g.emsize {
                let c = &mut w_stat[d.advw as usize];
                *c = c.wrapping_add(1);
            } else {
                let c = &mut w_stat[g.emsize as usize + 1];
                *c = c.wrapping_add(1);
            }
        }

        if len == 0 {
            i += 1;
            continue;
        } else if len < 10 {
            return Err(format!("Invalid TrueType glyph data (gid {gid})."));
        }

        let start = glyf_off + loc as usize;
        let mut data = f
            .get(start..start + len as usize)
            .ok_or_else(|| format!("Invalid TrueType glyph data (gid {gid})."))?
            .to_vec();
        let number_of_contours = i16::from_be_bytes([data[0], data[1]]);
        let llx = i16::from_be_bytes([data[2], data[3]]);
        let lly = i16::from_be_bytes([data[4], data[5]]);
        let urx = i16::from_be_bytes([data[6], data[7]]);
        let ury = i16::from_be_bytes([data[8], data[9]]);
        {
            let d = &mut g.gd[i];
            d.llx = llx;
            d.lly = lly;
            d.urx = urx;
            d.ury = ury;
            if vmtx.is_none() {
                // vertOriginY == sTypeAscender
                d.tsb = (g.default_advh as i32 - g.default_tsb as i32 - ury as i32) as i16;
            }
        }

        // Fix GIDs of composite glyphs.
        if number_of_contours < 0 {
            let mut p = 10usize;
            loop {
                if p + 4 > data.len() {
                    return Err(format!(
                        "Invalid TrueType glyph data (gid {gid}): {len} bytes"
                    ));
                }
                let flags = u16::from_be_bytes([data[p], data[p + 1]]);
                p += 2;
                let cgid = u16::from_be_bytes([data[p], data[p + 1]]);
                if cgid >= num_glyphs {
                    return Err(format!(
                        "Invalid gid ({cgid} > {num_glyphs}) in composite glyph {gid}."
                    ));
                }
                let mut new_gid = g.find_glyph(cgid);
                if new_gid == 0 {
                    let slot = g.find_empty_slot()?;
                    new_gid = g.add_glyph(cgid, slot)?;
                }
                data[p..p + 2].copy_from_slice(&new_gid.to_be_bytes());
                p += 2;
                p += if flags & ARG_1_AND_2_ARE_WORDS != 0 { 4 } else { 2 };
                if flags & WE_HAVE_A_SCALE != 0 {
                    p += 2;
                } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                    p += 4;
                } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
                    p += 8;
                }
                if flags & MORE_COMPONENT == 0 {
                    break;
                }
            }
        }
        g.gd[i].data = data;
        i += 1;
    }

    // Most frequent advance width.
    {
        let mut max_count: i32 = -1;
        g.dw = g.gd[0].advw;
        for (i, &c) in w_stat.iter().enumerate().take(g.emsize as usize + 1) {
            if c as i32 > max_count {
                max_count = c as i32;
                g.dw = i as u16;
            }
        }
    }

    g.gd.sort_by_key(|d| d.gid);

    // hmtx / loca / glyf
    let last_advw = g.gd[g.gd.len() - 1].advw;
    let mut glyf_table_size: usize = 0;
    let mut num_hm_known = false;
    let mut num_long = num_of_long_hor_metrics;
    for d in g.gd.iter().rev() {
        let padlen = if d.length % 4 != 0 { 4 - d.length % 4 } else { 0 };
        glyf_table_size += (d.length + padlen) as usize;
        if !num_hm_known && last_advw != d.advw {
            num_long = d.gid.wrapping_add(2);
            num_hm_known = true;
        }
    }
    if !num_hm_known {
        num_long = 1;
    }
    let hmtx_table_size = num_long as usize * 2 + (g.last_gid as usize + 1) * 2;

    let short_loca = glyf_table_size < 0x20000;
    let loca_format: i16 = if short_loca { 0 } else { 1 };
    let loca_table_size = (g.last_gid as usize + 2) * if short_loca { 2 } else { 4 };

    let mut hmtx_out: Vec<u8> = Vec::with_capacity(hmtx_table_size);
    let mut loca_out: Vec<u8> = Vec::with_capacity(loca_table_size);
    let mut glyf_out: Vec<u8> = Vec::with_capacity(glyf_table_size);

    let put_loca = |loca: &mut Vec<u8>, offset: usize| {
        if short_loca {
            loca.extend_from_slice(&((offset / 2) as u16).to_be_bytes());
        } else {
            loca.extend_from_slice(&(offset as u32).to_be_bytes());
        }
    };

    let mut prev: i32 = 0;
    for d in g.gd.iter_mut() {
        let gap = d.gid as i32 - prev - 1;
        for j in 1..=gap {
            let k = prev + j;
            if k == num_long as i32 - 1 {
                hmtx_out.extend_from_slice(&last_advw.to_be_bytes());
            } else if k < num_long as i32 {
                hmtx_out.extend_from_slice(&0u16.to_be_bytes());
            }
            hmtx_out.extend_from_slice(&0i16.to_be_bytes());
            put_loca(&mut loca_out, glyf_out.len());
        }
        let padlen = if d.length % 4 != 0 { 4 - d.length % 4 } else { 0 };
        if d.gid < num_long {
            hmtx_out.extend_from_slice(&d.advw.to_be_bytes());
        }
        hmtx_out.extend_from_slice(&d.lsb.to_be_bytes());
        put_loca(&mut loca_out, glyf_out.len());
        let total = (d.length + padlen) as usize;
        let start = glyf_out.len();
        glyf_out.extend_from_slice(&d.data);
        glyf_out.resize(start + total, 0);
        prev = d.gid as i32;
        d.data = Vec::new();
        d.length = 0;
    }
    put_loca(&mut loca_out, glyf_out.len());

    debug_assert_eq!(hmtx_out.len(), hmtx_table_size);
    debug_assert_eq!(loca_out.len(), loca_table_size);
    debug_assert_eq!(glyf_out.len(), glyf_table_size);

    sfont.set_table(b"hmtx", hmtx_out);
    sfont.set_table(b"loca", loca_out);
    sfont.set_table(b"glyf", glyf_out);

    // head: checkSumAdjustment = 0, indexToLocFormat; maxp: numGlyphs;
    // hhea: numOfLongHorMetrics.
    head[8..12].copy_from_slice(&0u32.to_be_bytes());
    head[50..52].copy_from_slice(&loca_format.to_be_bytes());
    maxp[4..6].copy_from_slice(&(g.last_gid.wrapping_add(1)).to_be_bytes());
    hhea[34..36].copy_from_slice(&num_long.to_be_bytes());
    sfont.set_table(b"maxp", maxp.to_vec());
    sfont.set_table(b"hhea", hhea.to_vec());
    sfont.set_table(b"head", head.to_vec());
    Ok(())
}

// ---------------------------------------------------------------------------
// cidtype2.c
// ---------------------------------------------------------------------------

/// `PDFUNIT(v)` of cidtype2.c.
fn pdfunit(v: f64, emsize: u16) -> f64 {
    (1000.0 * v / emsize as f64 + 0.5).floor()
}

/// `last_cid`: highest used CID (0 if none).
fn last_cid_of(used: &BTreeSet<u16>) -> u16 {
    used.iter().next_back().copied().unwrap_or(0)
}

/// `add_TTCIDHMetrics`.
fn add_tt_cid_h_metrics(g: &Glyphs, used: &BTreeSet<u16>, last_cid: u16) -> (f64, Vec<(u32, Vec<f64>)>) {
    let em = g.emsize;
    let dw = if g.dw != 0 && g.dw <= g.emsize {
        pdfunit(g.dw as f64, em)
    } else {
        pdfunit(g.gd[0].advw as f64, em)
    };

    let mut w: Vec<(u32, Vec<f64>)> = Vec::new();
    let mut an_array: Option<(u32, Vec<f64>)> = None;
    let mut prev: i32 = 0;
    for cid in 0..=last_cid as i32 {
        if !used.contains(&(cid as u16)) {
            continue;
        }
        let idx = g.get_index(cid as u16);
        if cid != 0 && idx == 0 {
            continue;
        }
        let width = pdfunit(g.gd[idx].advw as f64, em);
        if width == dw {
            if let Some(a) = an_array.take() {
                w.push(a);
            }
        } else {
            if cid != prev + 1 {
                if let Some(a) = an_array.take() {
                    w.push(a);
                }
            }
            an_array.get_or_insert_with(|| (cid as u32, Vec::new())).1.push(width);
            prev = cid;
        }
    }
    if let Some(a) = an_array.take() {
        w.push(a);
    }
    (dw, w)
}

fn check_used(used: &BTreeSet<u16>) -> Result<u16, String> {
    let last_cid = last_cid_of(used);
    if last_cid >= 0xFFFF {
        return Err("CID out of range (>= 65535)".to_string());
    }
    Ok(last_cid)
}

/// CIDFont_type2_dofont up to and including `tt_build_tables`, for
/// glyph-ordering (`Adobe-Identity`) fonts.
fn build<'a>(ttf: &'a [u8], face_index: u32, used: &BTreeSet<u16>) -> Result<(Sfnt<'a>, Glyphs, u16), String> {
    let mut sfont = Sfnt::open(ttf, face_index)?;
    let last_cid = check_used(used)?;
    let mut g = Glyphs::new()?;
    for &cid in used.range(1..) {
        // glyph_ordering: gid = cid, in ascending CID order.
        g.add_glyph(cid, cid)?;
    }
    tt_build_tables(&mut sfont, &mut g)?;
    Ok((sfont, g, last_cid))
}

/// Embeds `ttf` (TrueType, or TTC selected by `face_index`) as xdvipdfmx's
/// CIDFontType2 with Identity-H (CID = original GID).
///
/// `used` holds the used glyph IDs; glyph 0 is always part of the subset, and
/// contributes to `/W` only if it is in `used` (as `is_used_char2(.., 0)`).
/// Fonts that xdvipdfmx rejects (CFF-flavoured OpenType, face index without a
/// TTC, missing required tables, invalid glyph data) yield `Err`.
pub fn subset_cid_truetype(
    ttf: &[u8],
    face_index: u32,
    used: &BTreeSet<u16>,
) -> Result<TtSubset, String> {
    let (mut sfont, g, last_cid) = build(ttf, face_index, used)?;

    let (dw, w) = add_tt_cid_h_metrics(&g, used, last_cid);

    // CIDSet (PDF < 2.0): all gids 1..=last_gid.
    let mut cidset = vec![0u8; g.last_gid as usize / 8 + 1];
    for i in 1..=g.last_gid as usize {
        cidset[i / 8] |= 1 << (7 - i % 8);
    }

    for (tag, must_exist) in REQUIRED_TABLES {
        sfont.require(tag, must_exist)?;
    }
    let data = sfont.create_font_file()?;

    Ok(TtSubset { data, last_gid: g.last_gid, cidset, cid_to_gid_map: None, dw, w })
}

/// `add_TTCIDVMetrics` (used for Identity-V fonts): `(DW2, W2)`.
///
/// `DW2` is `Some([vertOriginY, -advanceHeight])` when it differs from the PDF
/// default `[880 -1000]`; each `W2` entry is the five numbers
/// `cid cid -w1y vx vy` emitted for a used CID whose vertical metrics differ
/// from the defaults. `Ok(None)` when neither DW2 nor W2 would be written.
/// (For glyphs without outline the C code reads uninitialised bounding boxes;
/// they are taken as 0 here.)
#[allow(clippy::type_complexity)]
pub fn truetype_vertical_metrics(
    ttf: &[u8],
    face_index: u32,
    used: &BTreeSet<u16>,
) -> Result<Option<(Option<[f64; 2]>, Vec<Vec<f64>>)>, String> {
    let (_sfont, g, last_cid) = build(ttf, face_index, used)?;
    let em = g.emsize;
    let default_vert_origin_y = pdfunit(g.default_advh as f64 - g.default_tsb as f64, em);
    let default_advance_height = pdfunit(g.default_advh as f64, em);

    let mut w2: Vec<Vec<f64>> = Vec::new();
    for cid in 0..=last_cid {
        if !used.contains(&cid) {
            continue;
        }
        let idx = g.get_index(cid);
        if cid != 0 && idx == 0 {
            continue;
        }
        let d = &g.gd[idx];
        let advance_height = pdfunit(d.advh as f64, em);
        let vert_origin_x = pdfunit(0.5 * d.advw as f64, em);
        let vert_origin_y = pdfunit(d.tsb as f64 + d.ury as f64, em);
        if vert_origin_y != default_vert_origin_y || advance_height != default_advance_height {
            let neg = if advance_height == 0.0 { 0.0 } else { -advance_height };
            w2.push(vec![cid as f64, cid as f64, neg, vert_origin_x, vert_origin_y]);
        }
    }

    let dw2 = if default_vert_origin_y != 880.0 || default_advance_height != 1000.0 {
        Some([default_vert_origin_y, -default_advance_height])
    } else {
        None
    };
    if dw2.is_none() && w2.is_empty() {
        Ok(None)
    } else {
        Ok(Some((dw2, w2)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fnv64(d: &[u8]) -> u64 {
        d.iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3))
    }

    fn set(v: &[u16]) -> BTreeSet<u16> {
        v.iter().copied().collect()
    }

    // ---- TeX Live reference: embedded FontFile2 / W / DW / CIDSet taken from
    // the xelatex (xdvipdfmx 20260113) PDFs of the TeXres corpus. The used GID
    // lists were recovered from the Identity-H strings of each PDF's content.
    struct Ref {
        font: &'static str,
        face: u32,
        used: &'static [u16],
        len: usize,
        fnv: u64,
        last_gid: u16,
        dw: f64,
        w: &'static [(u32, &'static [f64])],
    }

    const REFS: &[Ref] = &[
        Ref {
            font: "font_unioned_subsets/shared.ttf",
            face: 0,
            used: &[17, 20, 21, 29, 41, 43, 47, 50, 51, 53, 54, 55, 59, 62, 64, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 79, 80, 81, 82, 83, 85, 86, 87, 88, 90, 91, 92, 93],
            len: 9490,
            fnv: FNV_UNIONED,
            last_gid: 93,
            dw: 340.0,
            w: &[(17, &[238.0]), (20, &[618.0, 618.0]), (29, &[288.0]), (41, &[640.0]), (43, &[819.0]), (47, &[607.0]), (50, &[801.0, 642.0]), (53, &[695.0, 614.0, 624.0]), (59, &[720.0]), (68, &[543.0, 601.0, 535.0, 614.0, 558.0, 313.0, 551.0, 616.0, 301.0, 304.0]), (79, &[293.0, 897.0, 628.0, 589.0, 615.0]), (85, &[405.0, 476.0, 332.0, 609.0]), (90, &[780.0, 520.0, 534.0, 461.0])],
        },
        Ref {
            font: "font_truetype_collection/collection.ttc",
            face: 1,
            used: &[17, 20, 38, 41, 49, 55, 68, 70, 72, 76, 79, 81, 82, 83, 85, 86, 87, 88, 92, 93],
            len: 5594,
            fnv: FNV_TTC,
            last_gid: 93,
            dw: 238.0,
            w: &[(20, &[618.0]), (38, &[730.0]), (41, &[640.0]), (49, &[816.0]), (55, &[624.0]), (68, &[543.0]), (70, &[535.0]), (72, &[558.0]), (76, &[301.0]), (79, &[293.0]), (81, &[628.0, 589.0, 615.0]), (85, &[405.0, 476.0, 332.0, 609.0]), (92, &[534.0, 461.0])],
        },
        Ref {
            font: "font_unicode_supplementary/supplementary.ttf",
            face: 0,
            used: &[11, 12, 14, 17, 19, 20, 22, 23, 29, 39, 54, 56, 68, 70, 71, 72, 76, 79, 80, 81, 82, 83, 85, 86, 87, 88, 92, 109, 12238, 12239, 12240, 12241],
            len: 80526,
            fnv: FNV_SUPP,
            last_gid: 12241,
            dw: 618.0,
            w: &[(11, &[365.0, 365.0]), (14, &[678.0]), (17, &[238.0]), (29, &[288.0]), (39, &[787.0]), (54, &[614.0]), (56, &[792.0]), (68, &[543.0]), (70, &[535.0, 614.0, 558.0]), (76, &[301.0]), (79, &[293.0, 897.0, 628.0, 589.0, 615.0]), (85, &[405.0, 476.0, 332.0, 609.0]), (92, &[534.0]), (109, &[550.0]), (12238, &[724.0, 709.0, 730.0, 787.0])],
        },
        Ref {
            font: "font_cache_invalidation/dynamic.ttf",
            face: 0,
            used: &[17, 20, 29, 53, 68, 72, 74, 76, 81, 83, 85, 86, 87, 91],
            len: 4750,
            fnv: FNV_CACHE,
            last_gid: 91,
            dw: 238.0,
            w: &[(20, &[618.0]), (29, &[288.0]), (53, &[695.0]), (68, &[543.0]), (72, &[558.0]), (74, &[551.0]), (76, &[301.0]), (81, &[628.0]), (83, &[615.0]), (85, &[405.0, 476.0, 332.0]), (91, &[520.0])],
        },
        Ref {
            font: "font_error_restricted_font/restricted.ttf",
            face: 0,
            used: &[17, 20, 29, 53, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 80, 81, 82, 85, 86, 87, 88, 90],
            len: 6378,
            fnv: FNV_RESTR,
            last_gid: 90,
            dw: 238.0,
            w: &[(20, &[618.0]), (29, &[288.0]), (53, &[695.0]), (68, &[543.0, 601.0, 535.0, 614.0, 558.0, 313.0, 551.0, 616.0, 301.0, 304.0]), (80, &[897.0, 628.0, 589.0]), (85, &[405.0, 476.0, 332.0, 609.0]), (90, &[780.0])],
        },
        Ref {
            font: "/home/leo/rv-build/XeShip/fontgate/ref_env/texmf/fonts/truetype/public/ipaex/ipaexm.ttf",
            face: 0,
            used: &[400, 401, 426, 440, 441, 452, 453, 454, 455, 611, 619, 626, 630, 632, 636, 645, 646, 649, 650, 653, 654, 669, 670, 673, 675, 680, 683, 689, 699, 715, 728, 730, 743, 754, 1243, 1510, 1525, 1643, 1686, 1718, 1788, 1868, 2184, 2264, 2267, 2528, 2589, 2630, 2718, 2795, 2896, 2970, 2999, 3050, 3488, 3514, 3665, 3688, 3694, 3704, 3709, 3844],
            len: 40802,
            fnv: 0x9ff19230c61e9068,
            last_gid: 3844,
            dw: 1000.0,
            w: &[],
        },
        Ref {
            font: "/usr/share/texmf-dist/fonts/truetype/public/arphic-ttf/gbsn00lp.ttf",
            face: 0,
            used: &[714, 958, 980, 1609, 3075, 3230, 3899, 3974, 4253, 6284],
            len: 74000,
            fnv: 0x8cbae6164a6d9f7b,
            last_gid: 6284,
            dw: 1000.0,
            w: &[],
        },
        // Generated with real xdvipdfmx 20260113: accented Latin, Vietnamese,
        // Greek and Cyrillic, including nested composites (13 used composites).
        Ref {
            font: "/usr/share/fonts/liberation/LiberationSerif-Regular.ttf",
            face: 0,
            used: &[20, 43, 74, 79, 80, 81, 82, 85, 86, 87, 135, 171, 179, 184, 190, 392, 408, 444, 835, 999, 1002, 1526, 1705, 1719],
            len: 25464,
            fnv: 0xe85695ec3bfeaa55,
            last_gid: 1719,
            dw: 500.0,
            w: &[(43, &[722.0]), (79, &[278.0, 778.0]), (85, &[333.0, 389.0, 278.0]), (135, &[722.0]), (171, &[444.0]), (392, &[944.0]), (444, &[722.0]), (835, &[269.0]), (999, &[691.0]), (1002, &[535.0]), (1526, &[444.0]), (1705, &[444.0])],
        },
    ];

    const FNV_UNIONED: u64 = 0xcd363061d25f1c85;
    const FNV_TTC: u64 = 0xee3a453db865db83;
    const FNV_SUPP: u64 = 0xa8affe692d57dcf3;
    const FNV_CACHE: u64 = 0x571d2408935fb9fb;
    const FNV_RESTR: u64 = 0xb319e5694eb54b16;

    /// Our subset of the same used-GID set equals the stream xdvipdfmx embedded
    /// (length + FNV-1a hash of the decoded FontFile2), and /DW /W /CIDSet /
    /// /CIDToGIDMap match the PDF objects.
    #[test]
    fn matches_texlive_embedded_truetype() {
        let root = Path::new("/home/leo/rv-build/xe-corpus");
        for r in REFS {
            let Ok(bytes) = std::fs::read(root.join(r.font)) else { continue };
            eprintln!("checking {} against the TeX Live stream", r.font);
            let s = subset_cid_truetype(&bytes, r.face, &set(r.used)).unwrap();
            assert_eq!(s.data.len(), r.len, "{} length", r.font);
            assert_eq!(fnv64(&s.data), r.fnv, "{} bytes", r.font);
            assert_eq!(s.last_gid, r.last_gid, "{}", r.font);
            assert_eq!(s.dw, r.dw, "{} DW", r.font);
            let w: Vec<(u32, Vec<f64>)> = r.w.iter().map(|(a, b)| (*a, b.to_vec())).collect();
            assert_eq!(s.w, w, "{} W", r.font);
            // CIDSet: bits 1..=last_gid set, .notdef clear.
            let n = r.last_gid as usize;
            assert_eq!(s.cidset.len(), n / 8 + 1);
            for i in 0..=n {
                let bit = s.cidset[i / 8] & (1 << (7 - i % 8)) != 0;
                assert_eq!(bit, i != 0, "{} CIDSet bit {i}", r.font);
            }
            assert!(s.cid_to_gid_map.is_none());
        }
    }

    // ---- synthetic fonts ---------------------------------------------------

    fn be16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn be32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    /// Simple glyph record of `len` bytes (header + payload).
    fn simple_glyph(len: usize, ymax: i16) -> Vec<u8> {
        let mut g = Vec::new();
        be16(&mut g, 1); // numberOfContours
        be16(&mut g, 0);
        be16(&mut g, 0);
        be16(&mut g, 100);
        be16(&mut g, ymax as u16);
        g.resize(len, 0xAB);
        g
    }

    /// Composite of `comps` (each with 2-byte args, no transform).
    fn composite_glyph(comps: &[u16]) -> Vec<u8> {
        let mut g = Vec::new();
        be16(&mut g, 0xFFFF);
        be16(&mut g, 0);
        be16(&mut g, 0);
        be16(&mut g, 100);
        be16(&mut g, 100);
        for (i, &c) in comps.iter().enumerate() {
            let more = if i + 1 < comps.len() { MORE_COMPONENT } else { 0 };
            be16(&mut g, 0x0002 | more); // ARGS_ARE_XY_VALUES
            be16(&mut g, c);
            g.push(5);
            g.push(7);
        }
        while g.len() % 4 != 0 {
            g.push(0);
        }
        g
    }

    struct Spec {
        glyphs: Vec<Vec<u8>>,
        adv: Vec<u16>,
        upem: u16,
        os2: Option<Vec<u8>>,
        long_loca: bool,
        drop: &'static [&'static [u8; 4]],
        extra: Vec<([u8; 4], Vec<u8>)>,
    }

    fn build_font(spec: &Spec) -> Vec<u8> {
        let n = spec.glyphs.len();
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for g in &spec.glyphs {
            if spec.long_loca {
                be32(&mut loca, glyf.len() as u32);
            } else {
                be16(&mut loca, (glyf.len() / 2) as u16);
            }
            glyf.extend_from_slice(g);
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
        }
        if spec.long_loca {
            be32(&mut loca, glyf.len() as u32);
        } else {
            be16(&mut loca, (glyf.len() / 2) as u16);
        }
        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        head[8..12].copy_from_slice(&0xdead_beefu32.to_be_bytes());
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
        head[18..20].copy_from_slice(&spec.upem.to_be_bytes());
        head[50..52].copy_from_slice(&(spec.long_loca as i16).to_be_bytes());
        let mut hhea = vec![0u8; 36];
        hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea[34..36].copy_from_slice(&(n as u16).to_be_bytes());
        let mut maxp = vec![0u8; 32];
        maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&(n as u16).to_be_bytes());
        let mut hmtx = Vec::new();
        for (i, a) in spec.adv.iter().enumerate() {
            be16(&mut hmtx, *a);
            be16(&mut hmtx, i as u16); // lsb
        }
        let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"OS/2", spec.os2.clone().unwrap_or_default()),
            (*b"cmap", vec![1, 2, 3, 4]),
            (*b"glyf", glyf),
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"hmtx", hmtx),
            (*b"loca", loca),
            (*b"maxp", maxp),
            (*b"name", vec![9, 9, 9, 9, 9]),
            (*b"post", vec![7; 8]),
        ];
        if spec.os2.is_none() {
            tables.remove(0);
        }
        tables.extend(spec.extra.iter().cloned());
        tables.retain(|(t, _)| !spec.drop.contains(&t));
        let mut out = Vec::new();
        be32(&mut out, 0x0001_0000);
        be16(&mut out, tables.len() as u16);
        be16(&mut out, 0);
        be16(&mut out, 0);
        be16(&mut out, 0);
        let mut off = 12 + 16 * tables.len();
        let mut body = Vec::new();
        for (tag, d) in &tables {
            out.extend_from_slice(tag);
            be32(&mut out, 0x1111_1111); // recorded checksum (kept verbatim)
            be32(&mut out, off as u32);
            be32(&mut out, d.len() as u32);
            body.extend_from_slice(d);
            while body.len() % 4 != 0 {
                body.push(0);
            }
            off = 12 + 16 * tables.len() + body.len();
        }
        out.extend_from_slice(&body);
        out
    }

    fn table<'a>(font: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
        let n = u16::from_be_bytes([font[4], font[5]]) as usize;
        for i in 0..n {
            let p = 12 + 16 * i;
            if &font[p..p + 4] == tag {
                let off = u32::from_be_bytes(font[p + 8..p + 12].try_into().unwrap()) as usize;
                let len = u32::from_be_bytes(font[p + 12..p + 16].try_into().unwrap()) as usize;
                return Some(&font[off..off + len]);
            }
        }
        None
    }

    fn tags(font: &[u8]) -> Vec<String> {
        let n = u16::from_be_bytes([font[4], font[5]]) as usize;
        (0..n).map(|i| String::from_utf8_lossy(&font[12 + 16 * i..16 + 16 * i]).into()).collect()
    }

    fn base_spec() -> Spec {
        Spec {
            glyphs: vec![
                simple_glyph(12, 500),            // 0 .notdef
                vec![],                           // 1 empty (space)
                simple_glyph(20, 700),            // 2
                simple_glyph(14, 600),            // 3
                composite_glyph(&[3, 2]),         // 4: components 3 and 2
                composite_glyph(&[0]),            // 5: component .notdef
                simple_glyph(10, 100),            // 6
            ],
            adv: vec![500, 250, 600, 600, 700, 800, 900],
            upem: 1000,
            os2: None,
            long_loca: false,
            drop: &[],
            extra: vec![],
        }
    }

    /// Expectations below follow tt_glyf.c/cidtype2.c/sfnt.c: keep OS/2? head
    /// hhea loca maxp name glyf hmtx in directory order, drop cmap/post, glyph
    /// IDs preserved, composite components pulled into the lowest free slot.
    #[test]
    fn synthetic_tables_composites_and_metrics() {
        let font = build_font(&base_spec());
        let s = subset_cid_truetype(&font, 0, &set(&[4, 6])).unwrap();
        // 4 -> composite refs 3 and 2. Used slots {0,4,6}; 3 goes to slot 1,
        // 2 to slot 2 (lowest free slots), in order of appearance.
        assert_eq!(s.last_gid, 6);
        assert_eq!(tags(&s.data), ["glyf", "head", "hhea", "hmtx", "loca", "maxp", "name"]);
        let hdr = &s.data[..12];
        assert_eq!(&hdr[0..4], &[0, 1, 0, 0]);
        assert_eq!(u16::from_be_bytes([hdr[4], hdr[5]]), 7);
        assert_eq!(u16::from_be_bytes([hdr[6], hdr[7]]), 64); // searchRange
        assert_eq!(u16::from_be_bytes([hdr[8], hdr[9]]), 2); // entrySelector
        assert_eq!(u16::from_be_bytes([hdr[10], hdr[11]]), 48); // rangeShift
        // unmodified table keeps its recorded checksum; rebuilt ones are summed
        let dir = |tag: &[u8; 4]| {
            (0..7)
                .map(|i| 12 + 16 * i)
                .find(|&p| &s.data[p..p + 4] == tag)
                .map(|p| u32::from_be_bytes(s.data[p + 4..p + 8].try_into().unwrap()))
                .unwrap()
        };
        assert_eq!(dir(b"name"), 0x1111_1111);
        assert_eq!(dir(b"glyf"), calc_checksum(table(&s.data, b"glyf").unwrap()));
        let head = table(&s.data, b"head").unwrap();
        assert_eq!(&head[8..12], &[0, 0, 0, 0]); // checkSumAdjustment zeroed
        assert_eq!(i16::from_be_bytes([head[50], head[51]]), 0);
        let maxp = table(&s.data, b"maxp").unwrap();
        assert_eq!(u16::from_be_bytes([maxp[4], maxp[5]]), 7);

        let loca = table(&s.data, b"loca").unwrap();
        let glyf = table(&s.data, b"glyf").unwrap();
        assert_eq!(loca.len(), 2 * 8);
        let off: Vec<usize> =
            (0..8).map(|i| 2 * u16::from_be_bytes([loca[2 * i], loca[2 * i + 1]]) as usize).collect();
        // slot0 .notdef 12, slot1 comp 3 (14 -> 16), slot2 comp 2 (20),
        // slots 3,4: 3 is empty, 4 is the composite (header 10 + 2*(2+2+2)=22 -> 24)
        assert_eq!(off[0], 0);
        assert_eq!(off[1], 12);
        assert_eq!(off[2], 12 + 16);
        assert_eq!(off[3], 12 + 16 + 20);
        assert_eq!(off[4], off[3]);
        assert_eq!(off[5], off[4] + 24);
        assert_eq!(off[6], off[5]);
        assert_eq!(off[7], off[6] + 12);
        assert_eq!(glyf.len(), off[7]);
        // composite patched to the new slots 1 and 2
        let c = &glyf[off[4]..];
        assert_eq!(u16::from_be_bytes([c[12], c[13]]), 1);
        assert_eq!(u16::from_be_bytes([c[12 + 6], c[13 + 6]]), 2);

        // hmtx (slots 0,1,2,4,6 present, 3 and 5 gaps): the last glyph (slot 6)
        // has advance 900; scanning down, slot 4 (advance 700) is the first
        // that differs, so numOfLongHorMetrics = 4 + 2 = 6.
        let hhea = table(&s.data, b"hhea").unwrap();
        let nlong = u16::from_be_bytes([hhea[34], hhea[35]]) as usize;
        assert_eq!(nlong, 6);
        let hmtx = table(&s.data, b"hmtx").unwrap();
        let mut want = Vec::new();
        for (adv, lsb) in [(500u16, 0i16), (600, 3), (600, 2), (0, 0), (700, 4), (900, 0)] {
            want.extend_from_slice(&adv.to_be_bytes());
            want.extend_from_slice(&lsb.to_be_bytes());
        }
        want.extend_from_slice(&6i16.to_be_bytes()); // slot 6: only lsb (gid >= nlong)
        assert_eq!(hmtx, &want[..]);

        // dw = most frequent advance <= em among processed glyphs
        // (500 notdef, 600+600 comps, 700, 900): 600.
        assert_eq!(s.dw, 600.0);
        assert_eq!(s.w, vec![(4, vec![700.0]), (6, vec![900.0])]);
        assert_eq!(s.cidset, vec![0b0111_1110]); // gids 1..=6
        assert!(s.cid_to_gid_map.is_none());
    }

    #[test]
    fn long_loca_for_big_glyf_and_gid_checks() {
        let mut spec = base_spec();
        spec.glyphs[2] = simple_glyph(0x20000, 10);
        spec.long_loca = true;
        let font = build_font(&spec);
        let s = subset_cid_truetype(&font, 0, &set(&[2])).unwrap();
        let head = table(&s.data, b"head").unwrap();
        assert_eq!(i16::from_be_bytes([head[50], head[51]]), 1); // glyf >= 0x20000
        let loca = table(&s.data, b"loca").unwrap();
        assert_eq!(loca.len(), (2 + 2) * 4);
        // gid beyond numGlyphs is an error
        assert!(subset_cid_truetype(&font, 0, &set(&[7])).is_err());
        // face index without TTC
        assert!(subset_cid_truetype(&font, 1, &set(&[2])).is_err());
    }

    #[test]
    fn composite_notdef_component_and_used_zero_width() {
        let font = build_font(&base_spec());
        // tt_find_glyph(0) returns 0 even though .notdef exists; C therefore
        // duplicates it in the first free slot, which is 1 here.
        let s = subset_cid_truetype(&font, 0, &set(&[5])).unwrap();
        let glyf = table(&s.data, b"glyf").unwrap();
        let loca = table(&s.data, b"loca").unwrap();
        let p = 2 * u16_at(loca, 2 * 5).unwrap() as usize;
        assert_eq!(u16_at(glyf, p + 12).unwrap(), 1);
        assert_eq!(s.dw, 500.0);
        assert_eq!(s.w, vec![(5, vec![800.0])]);
        // Program always includes .notdef; /W only includes it if used.
        let without = subset_cid_truetype(&font, 0, &set(&[2, 3])).unwrap();
        let with = subset_cid_truetype(&font, 0, &set(&[0, 2, 3])).unwrap();
        assert_eq!(without.data, with.data);
        assert!(without.w.is_empty());
        assert_eq!(with.w, vec![(0, vec![500.0])]);
    }

    #[test]
    fn abbreviated_hmtx_repeats_last_bearing_and_advance() {
        let mut font = build_font(&base_spec());
        let n = u16_at(&font, 4).unwrap() as usize;
        for i in 0..n {
            let p = 12 + 16 * i;
            let off = u32_at(&font, p + 8).unwrap() as usize;
            if &font[p..p + 4] == b"hhea" {
                font[off + 34..off + 36].copy_from_slice(&1u16.to_be_bytes());
            } else if &font[p..p + 4] == b"hmtx" {
                // One long metric (500,0), one additional side bearing (-10).
                // C stops reading bearings when these declared entries end.
                font[p + 12..p + 16].copy_from_slice(&6u32.to_be_bytes());
                font[off + 4..off + 6].copy_from_slice(&(-10i16).to_be_bytes());
            }
        }
        let s = subset_cid_truetype(&font, 0, &set(&[2, 6])).unwrap();
        assert_eq!(s.dw, 500.0);
        assert!(s.w.is_empty());
        assert_eq!(u16_at(table(&s.data, b"hhea").unwrap(), 34).unwrap(), 1);
        let hm = table(&s.data, b"hmtx").unwrap();
        assert_eq!(u16_at(hm, 0).unwrap(), 500);
        assert_eq!(i16_at(hm, 6).unwrap(), -10); // slot 2
        assert_eq!(i16_at(hm, 14).unwrap(), -10); // slot 6
    }

    #[test]
    fn missing_required_tables_and_cff_rejected() {
        let mut spec = base_spec();
        spec.drop = &[b"name"];
        assert!(subset_cid_truetype(&build_font(&spec), 0, &set(&[2])).is_err());
        let mut cff = build_font(&base_spec());
        cff[0..4].copy_from_slice(b"OTTO");
        assert!(subset_cid_truetype(&cff, 0, &set(&[2])).is_err());
    }

    #[test]
    fn optional_tables_kept_in_directory_order() {
        let mut spec = base_spec();
        spec.os2 = Some(vec![0u8; 78]);
        spec.extra = vec![(*b"prep", vec![1, 2, 3]), (*b"fpgm", vec![4, 5]), (*b"cvt ", vec![6, 7, 8, 9])];
        let s = subset_cid_truetype(&build_font(&spec), 0, &set(&[2])).unwrap();
        assert_eq!(
            tags(&s.data),
            ["OS/2", "glyf", "head", "hhea", "hmtx", "loca", "maxp", "name", "prep", "fpgm", "cvt "]
        );
        // unpadded last table: total = last offset + length
        assert_eq!(table(&s.data, b"cvt ").unwrap(), &[6, 7, 8, 9]);
    }

    #[test]
    fn vertical_metrics_defaults_and_overrides() {
        let mut spec = base_spec();
        // OS/2 with sTypoAscender 900, sTypoDescender -100 -> advh 1000, tsb 100
        let mut os2 = vec![0u8; 78];
        os2[68..70].copy_from_slice(&900i16.to_be_bytes());
        os2[70..72].copy_from_slice(&(-100i16).to_be_bytes());
        spec.os2 = Some(os2);
        let font = build_font(&spec);
        let v = truetype_vertical_metrics(&font, 0, &set(&[2])).unwrap().unwrap();
        // default vertOriginY = 1000-100 = 900 != 880 -> DW2 = [900 -1000]
        assert_eq!(v.0, Some([900.0, -1000.0]));
        // glyph 2: tsb = advh - default_tsb - ury = 1000-100-700 = 200,
        // vertOriginY = tsb + ury = 900 == default; advh default -> no W2
        assert!(v.1.is_empty());
    }
    /// Real XeTeX `[gbsn00lp.ttf]:vertical`, xdvipdfmx 20260113: nonempty
    /// glyphs, including native vmtx top bearings (not synthesized OS/2).
    #[test]
    fn matches_texlive_vertical_native_vmtx() {
        let Ok(font) = std::fs::read(
            "/usr/share/texmf-dist/fonts/truetype/public/arphic-ttf/gbsn00lp.ttf",
        ) else { return };
        let v = truetype_vertical_metrics(&font, 0, &set(&[100, 300, 1000, 2000]))
            .unwrap().unwrap();
        assert_eq!(v.0, Some([879.0, -1000.0]));
        assert_eq!(v.1, vec![
            vec![300.0, 300.0, -1000.0, 500.0, 934.0],
            vec![1000.0, 1000.0, -1000.0, 500.0, 897.0],
            vec![2000.0, 2000.0, -1000.0, 500.0, 929.0],
        ]);
    }
}
