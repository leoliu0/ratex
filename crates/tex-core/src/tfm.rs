//! TFM (TeX Font Metric) parsing and font objects.

use crate::scaled::ONE;

pub const TAG_NO_TAG: u8 = 0;
pub const TAG_LIG: u8 = 1;
pub const TAG_LIST: u8 = 2;
pub const TAG_EXT: u8 = 3;

pub type FontId = u16;

#[derive(Clone, Debug)]
pub struct CharInfo {
    pub width: i32,   // sp
    pub height: i32,
    pub depth: i32,
    pub italic: i32,
    pub tag: u8,
    pub remainder: u8,
}

#[derive(Clone, Debug)]
pub struct LigStep {
    pub skip: u8,
    pub next_char: u8,
    pub op: u8, // >=128: kern index = (op-128)*256 + rem; else lig with flags in low 3 bits? see below
    pub rem: u8,
    pub stop: bool, // right_boundary / big stop
}

#[derive(Clone, Debug)]
pub struct ExtRecipe {
    pub top: u8,
    pub mid: u8,
    pub bot: u8,
    pub rep: u8,
}

#[derive(Clone)]
pub struct Font {
    pub name: String,     // \font cs name for tracing
    pub tfm_name: String, // e.g. cmr10
    pub at_size: i32,     // sp ("at" size)
    pub dsize: i32,
    pub chars: Vec<CharInfo>, // indexed by char code 0..=ec
    pub bc: u8,
    pub ec: u8,
    pub lig_kern: Vec<LigStep>,
    pub kerns: Vec<i32>,
    pub ext: Vec<ExtRecipe>,
    pub params: Vec<i32>, // fontdimen 1..=n in sp (param[0] unused slot for slant as scaled fraction*65536? store slant*65536 too)
    pub hyphen_char: i32,
    pub skew_char: i32,
    pub type1_path: Option<String>,
    pub enc_name: Option<String>,
    /// pdf font name in map (e.g. NtxRomanUpright)
    pub map_fontname: Option<String>,
    /// parsed encoding file (glyph names by slot)
    pub encoding: Option<Vec<String>>,
}

impl Font {
    pub fn char_width(&self, c: u8) -> i32 {
        if (c as usize) < self.chars.len() {
            self.chars[c as usize].width
        } else {
            0
        }
    }
    pub fn char_height(&self, c: u8) -> i32 {
        if (c as usize) < self.chars.len() {
            self.chars[c as usize].height
        } else {
            0
        }
    }
    pub fn char_depth(&self, c: u8) -> i32 {
        if (c as usize) < self.chars.len() {
            self.chars[c as usize].depth
        } else {
            0
        }
    }
    pub fn char_italic(&self, c: u8) -> i32 {
        if (c as usize) < self.chars.len() {
            self.chars[c as usize].italic
        } else {
            0
        }
    }
    pub fn exists_char(&self, c: u8) -> bool {
        c >= self.bc && c <= self.ec
    }
    /// param(i) with 1-based i (param(1)=slant ...)
    pub fn param(&self, i: usize) -> i32 {
        if i == 0 || i > self.params.len() {
            return 0;
        }
        self.params[i - 1]
    }
    pub fn space(&self) -> i32 {
        if self.params.len() >= 2 {
            self.params[1]
        } else {
            self.at_size / 3
        }
    }
    pub fn space_stretch(&self) -> i32 {
        if self.params.len() >= 3 {
            self.params[2]
        } else {
            self.space() / 2
        }
    }
    pub fn space_shrink(&self) -> i32 {
        if self.params.len() >= 4 {
            self.params[3]
        } else {
            self.space() / 3
        }
    }
    pub fn x_height(&self) -> i32 {
        if self.params.len() >= 5 {
            self.params[4]
        } else {
            0
        }
    }
    pub fn quad(&self) -> i32 {
        if self.params.len() >= 6 {
            self.params[5]
        } else {
            self.at_size
        }
    }
    pub fn extra_space(&self) -> i32 {
        if self.params.len() >= 7 {
            self.params[6]
        } else {
            self.space() / 4
        }
    }
}

fn rd_u16(b: &[u8], off: usize) -> usize {
    ((b[off] as usize) << 8) | b[off + 1] as usize
}

fn rd_i32(b: &[u8], off: usize) -> i32 {
    let v = ((b[off] as u32) << 24) | ((b[off + 1] as u32) << 16) | ((b[off + 2] as u32) << 8) | b[off + 3] as u32;
    v as i32
}

/// fix-word ratio to design size -> sp
fn fix_to_sp(fix: i32, dsize: i32) -> i32 {
    ((fix as i64 * dsize as i64 + if fix >= 0 { 0x80000 } else { -0x80000 }) / 0x100000) as i32
}

pub fn parse_tfm(data: &[u8], tfm_name: &str, at_size: i32) -> Result<Font, String> {
    if data.len() < 24 {
        return Err(format!("tfm {} too short", tfm_name));
    }
    let lf = rd_u16(data, 0);
    if data.len() < lf * 4 {
        return Err(format!("tfm {} truncated", tfm_name));
    }
    let lh = rd_u16(data, 2);
    let bc = rd_u16(data, 4) as u8;
    let ec = rd_u16(data, 6) as u8;
    let nw = rd_u16(data, 8);
    let nh = rd_u16(data, 10);
    let nd = rd_u16(data, 12);
    let ni = rd_u16(data, 14);
    let nl = rd_u16(data, 16);
    let nk = rd_u16(data, 18);
    let ne = rd_u16(data, 20);
    let np = rd_u16(data, 22);

    let dsize = fix_to_sp_design(rd_i32(data, 28)); // header: [24]=checksum, [28]=design size
    let at = if at_size <= 0 { dsize } else { at_size };
    // scale factor from design units to sp at `at`
    // tex.web store_scaled (§11128): byte-wise TRUNCATING multiplication
    // sw = (((d*z)/256 + c*z)/256 + b*z)/16, z = at-size in sp; a=255
    // negates. tex guarantees bit-exact portability with this; a rounded
    // fix*at/2^20 product differs by a few sp and flips badness boundaries.
    let scale = |fix: i32| -> i32 {
        let [a, b, c, d] = fix.to_be_bytes();
        let z = at as i64;
        let sw = (((d as i64 * z) / 256 + c as i64 * z) / 256 + b as i64 * z) / 16;
        if a == 0 {
            sw as i32
        } else {
            (sw - 16 * z) as i32
        }
    };

    let hdr_end = (6 + lh) * 4;
    let char_info_off = hdr_end;
    let nchars = ec as usize - bc as usize + 1;
    let width_off = char_info_off + nchars * 4;
    let height_off = width_off + nw * 4;
    let depth_off = height_off + nh * 4;
    let ital_off = depth_off + nd * 4;
    let lig_off = ital_off + ni * 4;
    let kern_off = lig_off + nl * 4;
    let ext_off = kern_off + nk * 4;
    let param_off = ext_off + ne * 4;

    let rd_fix = |off: usize| rd_i32(data, off);

    let widths: Vec<i32> = (0..nw).map(|k| rd_fix(width_off + k * 4)).collect();
    let heights: Vec<i32> = (0..nh).map(|k| rd_fix(height_off + k * 4)).collect();
    let depths: Vec<i32> = (0..nd).map(|k| rd_fix(depth_off + k * 4)).collect();
    let italics: Vec<i32> = (0..ni).map(|k| rd_fix(ital_off + k * 4)).collect();

    let mut chars = Vec::with_capacity(nchars);
    for i in 0..nchars {
        let o = char_info_off + i * 4;
        let b0 = data[o];
        let b1 = data[o + 1];
        let b2 = data[o + 2];
        let b3 = data[o + 3];
        // TFM char_info byte layout (tex.web §543): b0 = width index (full
        // byte); b1 = height<<4 | depth; b2 = italic<<2 | tag; b3 = remainder
        let wi = b0 as usize;
        let hi = (b1 >> 4) as usize;
        let di = (b1 & 15) as usize;
        let ii = (b2 >> 2) as usize;
        let tag = b2 & 3;
        let rem = b3;
        let w = match wi {
            0 => 0,
            _ => scale(widths.get(wi).copied().unwrap_or(0)),
        };
        let h = match hi {
            0 => 0,
            _ => scale(heights.get(hi).copied().unwrap_or(0)),
        };
        let d = match di {
            0 => 0,
            _ => scale(depths.get(di).copied().unwrap_or(0)),
        };
        let it = match ii {
            0 => 0,
            _ => scale(italics.get(ii).copied().unwrap_or(0)),
        };
        chars.push(CharInfo { width: w, height: h, depth: d, italic: it, tag, remainder: rem });
    }

    let lig_kern: Vec<LigStep> = (0..nl)
        .map(|k| {
            let o = lig_off + k * 4;
            let skip = data[o];
            let next = data[o + 1];
            let op = data[o + 2];
            let rem = data[o + 3];
            LigStep { skip, next_char: next, op, rem, stop: skip & 0x80 != 0 }
        })
        .collect();

    let kerns: Vec<i32> = (0..nk).map(|k| scale(rd_fix(kern_off + k * 4))).collect();

    let ext: Vec<ExtRecipe> = (0..ne)
        .map(|k| {
            let o = ext_off + k * 4;
            ExtRecipe { top: data[o], mid: data[o + 1], bot: data[o + 2], rep: data[o + 3] }
        })
        .collect();

    let mut params = Vec::with_capacity(np);
    for k in 0..np {
        let o = param_off + k * 4;
        let f = rd_fix(o);
        if k == 0 {
            // slant: stored as ratio to dsize too? TeX treats param(1) as pure number:
            // slant = fix/2^20 (radians-ish tan). Store as scaled fraction (tan*65536).
            params.push((f as i64 * ONE as i64 / 0x100000) as i32);
        } else {
            params.push(scale(f));
        }
    }

    Ok(Font {
        name: String::new(),
        tfm_name: tfm_name.to_string(),
        at_size: at,
        dsize,
        chars,
        bc,
        ec,
        lig_kern,
        kerns,
        ext,
        params,
        hyphen_char: b'-' as i32,
        skew_char: -1,
        type1_path: None,
        enc_name: None,
        map_fontname: None,
        encoding: None,
    })
}

/// header design size is a fix_word whose value/2^20 is the size in points (pt),
/// i.e. sp = fix * 2^16 / 2^20 = fix / 16.
fn fix_to_sp_design(fix: i32) -> i32 {
    (fix as i64 / 16) as i32
}
