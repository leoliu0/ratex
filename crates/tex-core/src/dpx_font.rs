//! Native-font PDF objects like xdvipdfmx writes them (XeTeX's native fonts
//! go through `pdf_insert_native_fontmap_record` with Identity-H/V and
//! `use_glyph_encoding`): `/ToUnicode` built as `otf_create_ToUnicode_stream`
//! of tt_cmap.c does (inverse cmap, GSUB single/alternate/ligature
//! decomposition, glyph names, presentation forms last), the font descriptor
//! of `tt_get_fontdesc` (tt_aux.c), `/DW`/`/W` of `add_CIDHMetrics` and
//! `/DW2`/`/W2` of `add_CIDVMetrics` (cidtype0.c), and the CMap stream layout
//! of `CMap_create_stream` (cmap_write.c).

use std::collections::{BTreeMap, BTreeSet};

// ------------------------------------------------------------------ sfnt

/// A table directory of one face (`sfnt_read_table_directory`).
pub struct Sfnt<'a> {
    pub data: &'a [u8],
    tables: Vec<([u8; 4], usize, usize)>,
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}
fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}
fn bei16(d: &[u8], o: usize) -> Option<i16> {
    be16(d, o).map(|v| v as i16)
}

impl<'a> Sfnt<'a> {
    /// Read the directory of face `index` (`ttc_read_offset` for a TTC).
    pub fn parse(data: &'a [u8], index: u32) -> Result<Sfnt<'a>, String> {
        let mut base = 0usize;
        if data.get(0..4) == Some(b"ttcf") {
            let n = be32(data, 8).ok_or("truncated TTC header")?;
            if index >= n {
                return Err("Invalid TTC index number".into());
            }
            base = be32(data, 12 + 4 * index as usize).ok_or("truncated TTC header")? as usize;
        } else if index > 0 {
            return Err("Invalid TTC index (not TTC font)".into());
        }
        let n = be16(data, base + 4).ok_or("truncated table directory")? as usize;
        let mut tables = Vec::with_capacity(n);
        for i in 0..n {
            let r = base + 12 + 16 * i;
            let tag = data.get(r..r + 4).ok_or("truncated table directory")?;
            let off = be32(data, r + 8).ok_or("truncated table directory")? as usize;
            let len = be32(data, r + 12).ok_or("truncated table directory")? as usize;
            tables.push(([tag[0], tag[1], tag[2], tag[3]], off, len));
        }
        Ok(Sfnt { data, tables })
    }

    pub fn table(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        let &(_, off, len) = self.tables.iter().find(|t| &t.0 == tag)?;
        self.data.get(off..off.checked_add(len)?.min(self.data.len()))
    }

    pub fn has(&self, tag: &[u8; 4]) -> bool {
        self.tables.iter().any(|t| &t.0 == tag)
    }
}

/// `tt_read_longMetrics`: `(advance, side bearing)` of every glyph.
fn long_metrics(table: &[u8], num_glyphs: u16, num_long: u16) -> Vec<u16> {
    let mut out = Vec::with_capacity(num_glyphs as usize);
    let mut last = 0;
    for g in 0..num_glyphs as usize {
        if g < num_long as usize {
            last = be16(table, 4 * g).unwrap_or(0);
        }
        out.push(last);
    }
    out
}

/// The tables `add_CIDMetrics`/`tt_get_fontdesc` read.
pub struct FaceMetrics {
    pub units_per_em: u16,
    pub num_glyphs: u16,
    pub hmtx: Vec<u16>,
    pub vmtx: Option<Vec<u16>>,
    pub hhea_ascent: i16,
    pub hhea_descent: i16,
}

impl FaceMetrics {
    pub fn read(sf: &Sfnt) -> Result<FaceMetrics, String> {
        let head = sf.table(b"head").ok_or("no head table")?;
        let maxp = sf.table(b"maxp").ok_or("no maxp table")?;
        let hhea = sf.table(b"hhea").ok_or("no hhea table")?;
        let hmtx = sf.table(b"hmtx").ok_or("no hmtx table")?;
        let num_glyphs = be16(maxp, 4).ok_or("bad maxp")?;
        let num_long = be16(hhea, 34).ok_or("bad hhea")?;
        let vmtx = match (sf.table(b"vhea"), sf.table(b"vmtx")) {
            (Some(vhea), Some(vmtx)) => {
                be16(vhea, 34).map(|n| long_metrics(vmtx, num_glyphs, n))
            }
            _ => None,
        };
        Ok(FaceMetrics {
            units_per_em: be16(head, 18).ok_or("bad head")?,
            num_glyphs,
            hmtx: long_metrics(hmtx, num_glyphs, num_long),
            vmtx,
            hhea_ascent: bei16(hhea, 4).ok_or("bad hhea")?,
            hhea_descent: bei16(hhea, 6).ok_or("bad hhea")?,
        })
    }
}

// --------------------------------------------------------- numbers & CMap

/// pdfdev.c `p_dtoa(value, prec)`, `prec` decimals with trailing zeros
/// dropped and no leading zero for |v| < 1.
pub fn p_dtoa(value: f64, prec: usize) -> String {
    let (neg, v) = if value < 0.0 { (true, -value) } else { (false, value) };
    let p = 10f64.powi(prec as i32);
    let mut i = v.trunc();
    let f = v - i;
    let mut g = (f * p + 0.5) as i64;
    if g == p as i64 {
        g = 0;
        i += 1.0;
    }
    let mut s = String::new();
    if i != 0.0 {
        if neg {
            s.push('-');
        }
        s.push_str(&format!("{i:.0}"));
    } else if g == 0 {
        return "0".into();
    } else if neg {
        s.push('-');
    }
    if g != 0 {
        s.push('.');
        let digits = format!("{g:0prec$}");
        s.push_str(digits.trim_end_matches('0'));
    }
    s
}

/// `pdf_sprint_number`: a PDF number object (8 decimals).
pub fn pdf_number(v: f64) -> String {
    p_dtoa(v, 8)
}

/// `PDFUNIT(v)`: `ROUND(1000 * v / unitsPerEm, 1)`.
fn pdf_unit(v: f64, upem: u16) -> f64 {
    (1000.0 * v / upem as f64 + 0.5).floor()
}

fn utf16be(ch: u32, out: &mut Vec<u8>) {
    if ch <= 0xFFFF {
        out.extend_from_slice(&(ch as u16).to_be_bytes());
    } else {
        let c = ch - 0x10000;
        out.extend_from_slice(&(((c >> 10) + 0xD800) as u16).to_be_bytes());
        out.extend_from_slice(&(((c & 0x3FF) + 0xDC00) as u16).to_be_bytes());
    }
}

fn uc_is_valid(ch: i32) -> bool {
    (0..=0x10FFFF).contains(&ch) && !(0xD800..=0xDFFF).contains(&ch)
}

/// `CMap_create_stream` for a two-byte-source ToUnicode map: bfchar blocks of
/// at most 100 entries, runs of consecutive codes with consecutive
/// destinations in one `bfrange`.
fn cmap_stream(name: &str, map: &BTreeMap<u16, Vec<u8>>) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    out.push_str("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n");
    out.push_str("/CMapName /");
    for c in name.bytes() {
        if !(b'!'..=b'~').contains(&c) || c == b'#' || b"()/<>[]{}%".contains(&c) {
            let _ = write!(out, "#{c:02X}");
        } else {
            out.push(c as char);
        }
    }
    out.push_str(" def\n/CMapType 2 def\n/CIDSystemInfo <<\n  /Registry (Adobe)\n  /Ordering (UCS)\n  /Supplement 0\n>> def\n");
    out.push_str("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
    let hex = |b: &[u8]| b.iter().fold(String::new(), |mut s, c| {
        let _ = write!(s, "{c:02X}");
        s
    });
    let mut chars = String::new();
    let mut count = 0usize;
    let flush = |out: &mut String, chars: &mut String, count: &mut usize| {
        if *count > 0 {
            let _ = write!(out, "{count} beginbfchar\n{chars}endbfchar\n");
            chars.clear();
            *count = 0;
        }
    };
    for hi in 0..=255u16 {
        let row: Vec<(u8, &Vec<u8>)> = map
            .range(hi << 8..=(hi << 8 | 0xFF))
            .map(|(k, v)| ((*k & 0xFF) as u8, v))
            .collect();
        if row.is_empty() {
            continue;
        }
        // block_count of cmap_write.c: the following entries are adjacent
        // codes whose destinations differ only in a last byte one larger
        let mut blocks: Vec<(u8, usize, &Vec<u8>)> = Vec::new();
        let mut k = 0;
        while k < row.len() {
            let (c, dst) = row[k];
            let n = dst.len() - 1;
            let mut run = 0;
            while k + run + 1 < row.len() {
                let (pc, pd) = row[k + run];
                let (cc, cd) = row[k + run + 1];
                if cc as u16 == pc as u16 + 1
                    && pd.len() == cd.len()
                    && pd[..n] == cd[..n]
                    && pd[n] < 255
                    && pd[n] + 1 == cd[n]
                {
                    run += 1;
                } else {
                    break;
                }
            }
            if run >= 2 {
                blocks.push((c, run, dst));
                k += run + 1;
            } else {
                let _ = writeln!(chars, "<{:04X}> <{}>", hi << 8 | c as u16, hex(dst));
                count += 1;
                k += 1;
                if count >= 100 {
                    flush(&mut out, &mut chars, &mut count);
                }
            }
        }
        if !blocks.is_empty() {
            flush(&mut out, &mut chars, &mut count);
            let _ = writeln!(out, "{} beginbfrange", blocks.len());
            for (c, run, dst) in blocks {
                let _ = writeln!(
                    out,
                    "<{:04X}> <{:04X}> <{}>",
                    hi << 8 | c as u16,
                    hi << 8 | (c as usize + run) as u16,
                    hex(dst)
                );
            }
            out.push_str("endbfrange\n");
        }
    }
    flush(&mut out, &mut chars, &mut count);
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out
}

// ------------------------------------------------------------- cmap table

/// tt_cmap.c `is_PUA_or_presentation`
fn is_pua_or_presentation(u: u32) -> bool {
    (0x2E80..=0x2EF3).contains(&u)
        || (0x2F00..=0x2FD5).contains(&u)
        || (0xE000..=0xF8FF).contains(&u)
        || (0xFB00..=0xFB4F).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0x2F800..=0x2FA1F).contains(&u)
        || (0xF0000..=0xFFFFD).contains(&u)
        || (0x100000..=0x10FFFD).contains(&u)
        || u == 0xAD
}

/// `create_inverse_cmap4/12` over the first usable subtable of
/// `otf_create_ToUnicode_stream`: `(map_base, map_sub)` indexed by glyph.
fn inverse_cmap(cmap: &[u8], num_glyphs: u16) -> Option<(Vec<i32>, Vec<i32>)> {
    const ORDER: [(u16, u16); 6] = [(3, 10), (0, 3), (0, 4), (0, 0), (3, 1), (0, 1)];
    let n = be16(cmap, 2)? as usize;
    let mut chosen = None;
    for (plat, enc) in ORDER {
        // tt_cmap_read: the first record of that platform/encoding
        let rec = (0..n).find_map(|i| {
            let r = 4 + 8 * i;
            (be16(cmap, r)? == plat && be16(cmap, r + 2)? == enc).then(|| be32(cmap, r + 4))?
        });
        let Some(off) = rec else { continue };
        let off = off as usize;
        let fmt = be16(cmap, off)?;
        if fmt == 4 || fmt == 12 {
            chosen = Some((off, fmt));
            break;
        }
    }
    let (off, fmt) = chosen?;
    let ng = num_glyphs as usize;
    let mut base = vec![-1i32; ng];
    let mut sub = vec![-1i32; ng];
    let mut put = |gid: usize, ch: u32| {
        if gid < ng {
            if is_pua_or_presentation(ch) {
                sub[gid] = ch as i32;
            } else {
                base[gid] = ch as i32;
            }
        }
    };
    if fmt == 4 {
        let t = cmap.get(off..)?;
        let seg = (be16(t, 6)? / 2) as usize;
        let end = 14;
        let start = end + 2 * seg + 2;
        let delta = start + 2 * seg;
        let range = delta + 2 * seg;
        let gia = range + 2 * seg;
        for i in 0..seg {
            let c1 = be16(t, end + 2 * i)?;
            let c0 = be16(t, start + 2 * i)?;
            let d = be16(t, delta + 2 * i)?;
            let ro = be16(t, range + 2 * i)?;
            if c1 < c0 {
                continue;
            }
            for j in 0..=(c1 - c0) as usize {
                let ch = c0.wrapping_add(j as u16);
                let gid = if ro == 0 {
                    ch.wrapping_add(d)
                } else if c0 == 0xFFFF && c1 == 0xFFFF && ro == 0xFFFF {
                    0
                } else {
                    // glyphIndexArray[j + idRangeOffset/2 - (segCount - i)]
                    let idx = j as isize + (ro / 2) as isize - (seg - i) as isize;
                    if idx < 0 {
                        continue;
                    }
                    let Some(v) = be16(t, gia + 2 * idx as usize) else { continue };
                    v.wrapping_add(d)
                };
                put(gid as usize, ch as u32);
            }
        }
    } else {
        let t = cmap.get(off..)?;
        let groups = be32(t, 12)? as usize;
        for i in 0..groups {
            let r = 16 + 12 * i;
            let (s, e, g) = (be32(t, r)?, be32(t, r + 4)?, be32(t, r + 8)?);
            if e < s || e - s > 0x10FFFF {
                continue;
            }
            for ch in s..=e {
                put(((g + (ch - s)) & 0xFFFF) as usize, ch);
            }
        }
    }
    Some((base, sub))
}

// ------------------------------------------------------------------- GSUB

struct Coverage(Vec<u16>);

fn coverage(t: &[u8], off: usize) -> Option<Coverage> {
    let mut v = Vec::new();
    match be16(t, off)? {
        1 => {
            let n = be16(t, off + 2)? as usize;
            for i in 0..n {
                v.push(be16(t, off + 4 + 2 * i)?);
            }
        }
        2 => {
            let n = be16(t, off + 2)? as usize;
            // glyphs in range order; the coverage index of a glyph is its
            // StartCoverageIndex + offset in the range, so the vector index
            // must equal that index
            for i in 0..n {
                let r = off + 4 + 6 * i;
                let (s, e, ci) = (be16(t, r)?, be16(t, r + 2)?, be16(t, r + 4)? as usize);
                if e < s {
                    continue;
                }
                if v.len() < ci + (e - s) as usize + 1 {
                    v.resize(ci + (e - s) as usize + 1, 0);
                }
                for g in s..=e {
                    v[ci + (g - s) as usize] = g;
                }
            }
        }
        _ => return None,
    }
    Some(Coverage(v))
}

enum Subst {
    /// (gid, substituted gid) pairs of a single substitution
    Single(Vec<(u16, u16)>),
    /// (gid, alternates) in coverage order
    Alternate(Vec<(u16, Vec<u16>)>),
    /// (first glyph, [(ligature glyph, component count, further components)])
    Ligature(Vec<(u16, Vec<(u16, u16, Vec<u16>)>)>),
}

fn read_subtable(t: &[u8], off: usize, typ: u16) -> Option<Subst> {
    match typ {
        1 => {
            let fmt = be16(t, off)?;
            let cov = coverage(t, off + be16(t, off + 2)? as usize)?;
            if fmt == 1 {
                let delta = be16(t, off + 4)?;
                Some(Subst::Single(cov.0.iter().map(|&g| (g, g.wrapping_add(delta))).collect()))
            } else if fmt == 2 {
                let n = be16(t, off + 4)? as usize;
                let mut out = Vec::new();
                for (i, &g) in cov.0.iter().enumerate() {
                    if i < n {
                        out.push((g, be16(t, off + 6 + 2 * i)?));
                    }
                }
                Some(Subst::Single(out))
            } else {
                None
            }
        }
        3 => {
            if be16(t, off)? != 1 {
                return None;
            }
            let cov = coverage(t, off + be16(t, off + 2)? as usize)?;
            let n = be16(t, off + 4)? as usize;
            let mut out = Vec::new();
            for (i, &g) in cov.0.iter().enumerate() {
                if i >= n {
                    break;
                }
                let set = off + be16(t, off + 6 + 2 * i)? as usize;
                let cnt = be16(t, set)? as usize;
                let mut alts = Vec::new();
                for k in 0..cnt {
                    alts.push(be16(t, set + 2 + 2 * k)?);
                }
                out.push((g, alts));
            }
            Some(Subst::Alternate(out))
        }
        4 => {
            if be16(t, off)? != 1 {
                return None;
            }
            let cov = coverage(t, off + be16(t, off + 2)? as usize)?;
            let n = be16(t, off + 4)? as usize;
            let mut out = Vec::new();
            for (i, &g) in cov.0.iter().enumerate() {
                if i >= n {
                    break;
                }
                let set = off + be16(t, off + 6 + 2 * i)? as usize;
                let cnt = be16(t, set)? as usize;
                let mut ligs = Vec::new();
                for k in 0..cnt {
                    let lig = set + be16(t, set + 2 + 2 * k)? as usize;
                    let glyph = be16(t, lig)?;
                    let comp = be16(t, lig + 2)?;
                    let mut rest = Vec::new();
                    for c in 0..comp.saturating_sub(1) as usize {
                        rest.push(be16(t, lig + 4 + 2 * c)?);
                    }
                    ligs.push((glyph, comp, rest));
                }
                out.push((g, ligs));
            }
            Some(Subst::Ligature(out))
        }
        _ => None,
    }
}

/// The subtables `otl_gsub_add_feat("*","*","*")` collects, in its order:
/// features by ascending index among those any language system lists, each
/// feature's lookups in order, extension lookups unwrapped.
fn gsub_subtables(gsub: &[u8]) -> Vec<Subst> {
    let mut out = Vec::new();
    let (Some(script_list), Some(feature_list), Some(lookup_list)) =
        (be16(gsub, 4), be16(gsub, 6), be16(gsub, 8))
    else {
        return out;
    };
    let (script_list, feature_list, lookup_list) =
        (script_list as usize, feature_list as usize, lookup_list as usize);
    let mut enabled = BTreeSet::new();
    let nscripts = be16(gsub, script_list).unwrap_or(0) as usize;
    for s in 0..nscripts {
        let Some(so) = be16(gsub, script_list + 2 + 6 * s + 4) else { continue };
        let st = script_list + so as usize;
        let mut langsys = Vec::new();
        if let Some(d) = be16(gsub, st).filter(|&d| d != 0) {
            langsys.push(st + d as usize);
        }
        for l in 0..be16(gsub, st + 2).unwrap_or(0) as usize {
            if let Some(lo) = be16(gsub, st + 4 + 6 * l + 4) {
                langsys.push(st + lo as usize);
            }
        }
        for ls in langsys {
            if let Some(req) = be16(gsub, ls + 2).filter(|&r| r != 0xFFFF) {
                enabled.insert(req);
            }
            for f in 0..be16(gsub, ls + 4).unwrap_or(0) as usize {
                if let Some(fi) = be16(gsub, ls + 6 + 2 * f) {
                    enabled.insert(fi);
                }
            }
        }
    }
    let nfeat = be16(gsub, feature_list).unwrap_or(0);
    let nlookups = be16(gsub, lookup_list).unwrap_or(0) as usize;
    for fi in enabled {
        if fi >= nfeat {
            continue;
        }
        let Some(fo) = be16(gsub, feature_list + 2 + 6 * fi as usize + 4) else { continue };
        let ft = feature_list + fo as usize;
        for i in 0..be16(gsub, ft + 2).unwrap_or(0) as usize {
            let Some(li) = be16(gsub, ft + 4 + 2 * i).map(|v| v as usize) else { continue };
            if li >= nlookups {
                continue;
            }
            let Some(lo) = be16(gsub, lookup_list + 2 + 2 * li) else { continue };
            let lt = lookup_list + lo as usize;
            let Some(typ) = be16(gsub, lt) else { continue };
            if !matches!(typ, 1 | 3 | 4 | 7) {
                continue;
            }
            for st in 0..be16(gsub, lt + 4).unwrap_or(0) as usize {
                let Some(so) = be16(gsub, lt + 6 + 2 * st) else { continue };
                let off = lt + so as usize;
                let sub = if typ == 7 {
                    if be16(gsub, off) != Some(1) {
                        continue;
                    }
                    let (Some(et), Some(eo)) = (be16(gsub, off + 2), be32(gsub, off + 4)) else {
                        continue;
                    };
                    read_subtable(gsub, off + eo as usize, et)
                } else {
                    read_subtable(gsub, off, typ)
                };
                out.extend(sub);
            }
        }
    }
    out
}

// -------------------------------------------------------------- ToUnicode

/// `otf_create_ToUnicode_stream`: the ToUnicode CMap of the used glyphs of an
/// SFNT font (`used`: glyph ids = CIDs). `cmap_name` is `<basefont>-UTF16`.
/// `glyph_names` gives the PostScript glyph name of a glyph (post table or
/// CFF charset) for the glyph-name step.
pub fn to_unicode_cmap(
    sf: &Sfnt,
    used: &BTreeSet<u16>,
    cmap_name: &str,
    glyph_name: &dyn Fn(u16) -> Option<String>,
) -> Option<String> {
    let maxp = sf.table(b"maxp")?;
    let num_glyphs = be16(maxp, 4)?;
    let (base, sub) = inverse_cmap(sf.table(b"cmap")?, num_glyphs)?;
    let mut remaining: BTreeSet<u16> = used.clone();
    let mut map: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
    let mut count = 0;
    let mut add = |map: &mut BTreeMap<u16, Vec<u8>>, cid: u16, dst: Vec<u8>| {
        map.insert(cid, dst);
    };
    // base mapping
    for &gid in used {
        if gid < num_glyphs {
            let ch = base[gid as usize];
            if uc_is_valid(ch) {
                let mut d = Vec::new();
                utf16be(ch as u32, &mut d);
                add(&mut map, gid, d);
                remaining.remove(&gid);
                count += 1;
            }
        }
    }
    // GSUB decomposition
    let valid = |gid: u16| -> Option<i32> {
        let g = gid as usize;
        (g < num_glyphs as usize).then(|| if uc_is_valid(base[g]) { base[g] } else { sub[g] })
    };
    if let Some(gsub) = sf.table(b"GSUB") {
        for st in gsub_subtables(gsub) {
            let mut pairs: Vec<(u16, u16)> = Vec::new();
            match st {
                Subst::Single(v) => pairs = v,
                Subst::Alternate(v) => {
                    for (g, alts) in v {
                        for a in alts {
                            pairs.push((g, a));
                        }
                    }
                }
                Subst::Ligature(v) => {
                    for (first, ligs) in v {
                        for (lig, comp, rest) in ligs {
                            if lig >= num_glyphs || !remaining.contains(&lig) {
                                continue;
                            }
                            let mut fail = 0;
                            let mut ucv = Vec::new();
                            match valid(first) {
                                Some(ch) if uc_is_valid(ch) => ucv.push(ch),
                                _ => fail += 1,
                            }
                            for &g in rest.iter().take(comp.saturating_sub(1) as usize) {
                                match valid(g) {
                                    Some(ch) if uc_is_valid(ch) => ucv.push(ch),
                                    _ => fail += 1,
                                }
                            }
                            if fail == 0 {
                                let mut d = Vec::new();
                                for ch in ucv {
                                    utf16be(ch as u32, &mut d);
                                }
                                add(&mut map, lig, d);
                                remaining.remove(&lig);
                            }
                        }
                    }
                    continue;
                }
            }
            for (gid, gid_sub) in pairs {
                if gid_sub >= num_glyphs || gid >= num_glyphs || !remaining.contains(&gid_sub) {
                    continue;
                }
                if let Some(ch) = valid(gid).filter(|&c| uc_is_valid(c)) {
                    let mut d = Vec::new();
                    utf16be(ch as u32, &mut d);
                    add(&mut map, gid_sub, d);
                    remaining.remove(&gid_sub);
                    count += 1;
                }
            }
        }
    }
    // glyph names
    for gid in remaining.clone() {
        if let Some(name) = glyph_name(gid) {
            let us = agl_unicodes(&name);
            if !us.is_empty() {
                let mut d = Vec::new();
                for u in us {
                    utf16be(u, &mut d);
                }
                add(&mut map, gid, d);
                remaining.remove(&gid);
                count += 1;
            }
        }
    }
    // presentation forms and PUA last
    for gid in remaining.clone() {
        if gid < num_glyphs && uc_is_valid(sub[gid as usize]) {
            let mut d = Vec::new();
            utf16be(sub[gid as usize] as u32, &mut d);
            add(&mut map, gid, d);
            remaining.remove(&gid);
            count += 1;
        }
    }
    if count < 1 {
        return None;
    }
    Some(cmap_stream(cmap_name, &map))
}

/// `agl_get_unicodes` for the name forms that do not need the Adobe glyph
/// list: `uniXXXX[XXXX..]` and `uXXXX[XX]`, components joined by `_`,
/// anything after the first `.` ignored. Other names yield nothing.
fn agl_unicodes(name: &str) -> Vec<u32> {
    let stem = name.split('.').next().unwrap_or("");
    let mut out = Vec::new();
    for comp in stem.split('_') {
        if let Some(h) = comp.strip_prefix("uni") {
            if h.len() >= 4 && h.len() % 4 == 0 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase()) {
                for k in (0..h.len()).step_by(4) {
                    out.push(u32::from_str_radix(&h[k..k + 4], 16).unwrap_or(0xFFFD));
                }
                continue;
            }
        }
        if let Some(h) = comp.strip_prefix('u') {
            if (4..=6).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase()) {
                out.push(u32::from_str_radix(h, 16).unwrap_or(0xFFFD));
                continue;
            }
        }
        return Vec::new();
    }
    out
}

// ------------------------------------------------------------- descriptor

/// Everything of a native font's PDF objects except the font program.
pub struct Descriptor {
    /// `key value` pairs of the FontDescriptor, without FontName/FontFile
    pub entries: Vec<(&'static str, String)>,
    pub units_per_em: u16,
}

/// tt_aux.c `tt_get_fontdesc` for a CID-keyed (`type` 0) font; `stem_v`
/// overrides the OS/2 estimate.
pub fn font_descriptor(sf: &Sfnt) -> Result<Descriptor, String> {
    let head = sf.table(b"head").ok_or("no head table")?;
    let post = sf.table(b"post").ok_or("no post table")?;
    let upem = be16(head, 18).ok_or("bad head table")?;
    let os2 = sf.table(b"OS/2");
    let mut e: Vec<(&'static str, String)> = Vec::new();
    let mut flags = 4i32; // SYMBOLIC
    if let Some(o) = os2 {
        let asc = bei16(o, 68).unwrap_or(0) as f64;
        let desc = bei16(o, 70).unwrap_or(0) as f64;
        e.push(("Ascent", pdf_number(pdf_unit(asc, upem))));
        e.push(("Descent", pdf_number(pdf_unit(desc, upem))));
        let w = be16(o, 4).unwrap_or(0) as f64;
        let stem_v = (w / 65.0) * (w / 65.0) + 50.0;
        e.push(("StemV", pdf_number(stem_v)));
        if be16(o, 0) == Some(2) {
            e.push(("CapHeight", pdf_number(pdf_unit(bei16(o, 88).unwrap_or(0) as f64, upem))));
            e.push(("XHeight", pdf_number(pdf_unit(bei16(o, 86).unwrap_or(0) as f64, upem))));
        } else {
            e.push(("CapHeight", pdf_number(pdf_unit(asc, upem))));
        }
        let avg = bei16(o, 2).unwrap_or(0);
        if avg != 0 {
            e.push(("AvgWidth", pdf_number(pdf_unit(avg as f64, upem))));
        }
    }
    let rd = |o: usize| bei16(head, o).unwrap_or(0) as f64;
    e.push((
        "FontBBox",
        format!(
            "[{} {} {} {}]",
            pdf_number(pdf_unit(rd(36), upem)),
            pdf_number(pdf_unit(rd(38), upem)),
            pdf_number(pdf_unit(rd(40), upem)),
            pdf_number(pdf_unit(rd(42), upem))
        ),
    ));
    let italic = be32(post, 4).unwrap_or(0) as i32 as f64 / 65536.0;
    e.push(("ItalicAngle", pdf_number(italic)));
    if let Some(o) = os2 {
        let fs = be16(o, 62).unwrap_or(0);
        let class = bei16(o, 30).unwrap_or(0);
        if fs & 1 != 0 {
            flags |= 1 << 6;
        }
        if fs & (1 << 5) != 0 {
            flags |= 1 << 18;
        }
        if (class >> 8) & 0xff != 8 {
            flags |= 1 << 1;
        }
        if (class >> 8) & 0xff == 10 {
            flags |= 1 << 3;
        }
        if be32(post, 12).unwrap_or(0) != 0 {
            flags |= 1;
        }
    }
    e.push(("Flags", flags.to_string()));
    if let Some(o) = os2 {
        let class = bei16(o, 30).unwrap_or(0) as u16;
        let mut panose = vec![(class >> 8) as u8, (class & 0xff) as u8];
        panose.extend_from_slice(o.get(32..42).unwrap_or(&[0; 10]));
        let hex: String = panose.iter().map(|b| format!("{b:02x}")).collect();
        e.push(("Style", format!("<< /Panose <{hex}> >>")));
    }
    Ok(Descriptor { entries: e, units_per_em: upem })
}

/// The PostScript name of an SFNT (`tt_get_ps_fontname`): name id 6 of the
/// Windows English record, else the Macintosh Roman one.
pub fn ps_name(sf: &Sfnt) -> Option<String> {
    let name = sf.table(b"name")?;
    let count = be16(name, 2)? as usize;
    let storage = be16(name, 4)? as usize;
    let mut mac = None;
    for i in 0..count {
        let r = 6 + 12 * i;
        let (plat, enc, lang, id, len, off) = (
            be16(name, r)?,
            be16(name, r + 2)?,
            be16(name, r + 4)?,
            be16(name, r + 6)?,
            be16(name, r + 8)? as usize,
            be16(name, r + 10)? as usize,
        );
        if id != 6 {
            continue;
        }
        let s = name.get(storage + off..storage + off + len)?;
        if plat == 3 && enc == 1 && lang == 0x409 {
            let units: Vec<u16> = s.chunks(2).filter_map(|c| Some(u16::from_be_bytes([c[0], *c.get(1)?]))).collect();
            return String::from_utf16(&units).ok();
        }
        if plat == 1 && enc == 0 && lang == 0 && mac.is_none() {
            mac = Some(s.iter().map(|&b| b as char).collect::<String>());
        }
    }
    mac
}

/// The first name of a CFF table's Name INDEX (`cff_get_name`).
pub fn cff_name(cff: &[u8]) -> Option<String> {
    let hdr = *cff.get(2)? as usize;
    let count = be16(cff, hdr)? as usize;
    if count == 0 {
        return None;
    }
    let off_size = *cff.get(hdr + 2)? as usize;
    let rd = |i: usize| -> Option<usize> {
        let p = hdr + 3 + i * off_size;
        let mut v = 0usize;
        for k in 0..off_size {
            v = v << 8 | *cff.get(p + k)? as usize;
        }
        Some(v)
    };
    let data = hdr + 3 + (count + 1) * off_size - 1;
    let (a, b) = (rd(0)?, rd(1)?);
    let s = cff.get(data + a..data + b)?;
    Some(String::from_utf8_lossy(s).into_owned())
}

/// `add_CIDHMetrics`: `(DW, W runs)` of the used CIDs (`cid == gid`).
pub fn horizontal_metrics(m: &FaceMetrics, used: &BTreeSet<u16>) -> (f64, Vec<(u16, Vec<f64>)>) {
    let upem = m.units_per_em;
    let unit = |adv: u16| pdf_unit(adv as f64, upem);
    let default = m.hmtx.first().map_or(0.0, |&a| unit(a));
    let mut w: Vec<(u16, Vec<f64>)> = Vec::new();
    let mut cur: Option<(u16, Vec<f64>)> = None;
    let mut prev = 0u32;
    let mut cids: BTreeSet<u16> = used.clone();
    cids.insert(0);
    for cid in cids {
        let gid = cid;
        if gid >= m.num_glyphs || (cid != 0 && gid == 0) {
            continue;
        }
        let adv = unit(m.hmtx[gid as usize]);
        if adv == default {
            if let Some(run) = cur.take() {
                w.push(run);
            }
        } else {
            if cid as u32 != prev + 1 {
                if let Some(run) = cur.take() {
                    w.push(run);
                }
            }
            cur.get_or_insert_with(|| (cid, Vec::new())).1.push(adv);
            prev = cid as u32;
        }
    }
    if let Some(run) = cur {
        w.push(run);
    }
    (default, w)
}

/// `add_CIDVMetrics`: `(DW2 [y, -h] when not the default, W2 entries
/// [first last w1y vx vy])`, empty without a VORG table.
pub fn vertical_metrics(sf: &Sfnt, m: &FaceMetrics, used: &BTreeSet<u16>) -> Option<(Option<[f64; 2]>, Vec<[f64; 5]>)> {
    let vorg = sf.table(b"VORG")?;
    let upem = m.units_per_em;
    let default_y = pdf_unit(bei16(vorg, 4)? as f64, upem);
    let n = be16(vorg, 6)? as usize;
    let metrics: Vec<(u16, i16)> = (0..n)
        .filter_map(|i| Some((be16(vorg, 8 + 4 * i)?, bei16(vorg, 10 + 4 * i)?)))
        .collect();
    let default_h = 1000.0;
    let mut w2 = Vec::new();
    let mut cids: BTreeSet<u16> = used.clone();
    cids.insert(0);
    for cid in cids {
        let gid = cid;
        if gid >= m.num_glyphs || (cid != 0 && gid == 0) {
            continue;
        }
        let h = match &m.vmtx {
            Some(v) => pdf_unit(v[gid as usize] as f64, upem),
            None => default_h,
        };
        let vx = pdf_unit(m.hmtx[gid as usize] as f64 * 0.5, upem);
        let mut vy = default_y;
        for &(g, y) in &metrics {
            if gid < g {
                break;
            }
            if gid == g {
                vy = pdf_unit(y as f64, upem);
            }
        }
        if vy != default_y || h != default_h {
            w2.push([cid as f64, cid as f64, -h, vx, vy]);
        }
    }
    let dw2 = (default_y != 880.0 || default_h != 1000.0).then_some([default_y, -default_h]);
    Some((dw2, w2))
}

/// `/W` text of horizontal metric runs.
pub fn w_array_text(runs: &[(u16, Vec<f64>)]) -> String {
    let mut s = String::from("[");
    for (start, ws) in runs {
        s.push_str(&format!(" {start} ["));
        for (i, w) in ws.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(&pdf_number(*w));
        }
        s.push(']');
    }
    s.push_str(" ]");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p_dtoa_matches_dvipdfmx() {
        assert_eq!(p_dtoa(0.5, 3), ".5");
        assert_eq!(p_dtoa(-0.0004, 3), "0");
        assert_eq!(p_dtoa(76.712, 3), "76.712");
        assert_eq!(p_dtoa(9.9626, 4), "9.9626");
        assert_eq!(p_dtoa(-62.7654, 3), "-62.765");
        assert_eq!(p_dtoa(2.0, 3), "2");
    }

    /// Expectation from TL xdvipdfmx: runs of consecutive codes with
    /// consecutive destinations become bfrange, others bfchar.
    #[test]
    fn cmap_stream_ranges() {
        let mut m = BTreeMap::new();
        for (g, u) in [(0x10u16, 0x41u16), (0x11, 0x42), (0x12, 0x43), (0x20, 0x61), (0x30, 0x62)] {
            m.insert(g, u.to_be_bytes().to_vec());
        }
        let s = cmap_stream("X-UTF16", &m);
        assert!(s.contains("2 beginbfchar\n<0020> <0061>\n<0030> <0062>\nendbfchar\n1 beginbfrange\n<0010> <0012> <0041>\nendbfrange\n"), "{s}");
    }
}
