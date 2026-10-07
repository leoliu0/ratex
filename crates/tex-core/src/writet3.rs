//! pdfTeX's PK bitmap fonts: a TFM without a pdftex.map entry is written as
//! a Type 3 font whose glyphs are the PK file's bitmaps (pkin.c `readchar`,
//! writet3.c `writepk` and `writet3`), and its characters advance the text
//! position on the PK raster (`get_pk_char_width`).

use crate::pdfrender::divide_scaled;

/// One character of a PK file (pkin.c `chardesc`): the bitmap rows, each
/// `(width + 7) / 8` bytes, most significant bit first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PkChar {
    pub code: i32,
    pub width: i32,
    pub height: i32,
    pub xoff: i32,
    pub yoff: i32,
    pub rows: Vec<u8>,
}

struct PkReader<'a> {
    data: &'a [u8],
    pos: usize,
    input_byte: u32,
    bit_weight: u32,
    dyn_f: u32,
    repeat_count: u32,
    /// pkin.c `pk_remainder`/`realfunc`: a huge run count being handed out.
    remainder: Option<i64>,
}

impl PkReader<'_> {
    fn byte(&mut self) -> Result<u32, String> {
        let byte = *self.data.get(self.pos).ok_or("unexpected eof in pk file")?;
        self.pos += 1;
        Ok(u32::from(byte))
    }

    fn signed(&mut self, bytes: usize) -> Result<i32, String> {
        let mut value = self.byte()? as i32;
        if value > 127 {
            value -= 256;
        }
        for _ in 1..bytes {
            value = value * 256 + self.byte()? as i32;
        }
        Ok(value)
    }

    fn nybble(&mut self) -> Result<u32, String> {
        if self.bit_weight == 0 {
            self.bit_weight = 16;
            self.input_byte = self.byte()?;
            Ok(self.input_byte >> 4)
        } else {
            self.bit_weight = 0;
            Ok(self.input_byte & 15)
        }
    }

    fn bit(&mut self) -> Result<bool, String> {
        self.bit_weight >>= 1;
        if self.bit_weight == 0 {
            self.input_byte = self.byte()?;
            self.bit_weight = 128;
        }
        Ok(self.input_byte & self.bit_weight != 0)
    }

    /// pkin.c `realfunc`: `rest` while a huge count lasts, else `pkpackednum`.
    fn run_count(&mut self) -> Result<u32, String> {
        if self.remainder.is_some() {
            self.rest()
        } else {
            self.packed_num()
        }
    }

    fn rest(&mut self) -> Result<u32, String> {
        let remainder = self.remainder.unwrap_or(0);
        if remainder < 0 {
            self.remainder = Some(-remainder);
            Ok(0)
        } else if remainder > 4000 {
            self.remainder = Some(4000 - remainder);
            Ok(4000)
        } else if remainder > 0 {
            self.remainder = None;
            Ok(remainder as u32)
        } else {
            Err("shouldn't happen".into())
        }
    }

    /// pkin.c `pkpackednum`.
    fn packed_num(&mut self) -> Result<u32, String> {
        let dyn_f = self.dyn_f;
        let mut i = self.nybble()?;
        if i == 0 {
            let mut j;
            loop {
                j = self.nybble()?;
                i += 1;
                if j != 0 {
                    break;
                }
            }
            if i > 3 {
                // handlehuge
                let mut j = i64::from(j);
                for _ in 0..i {
                    j = (j << 4) + i64::from(self.nybble()?);
                }
                self.remainder = Some(j - 15 + (13 - i64::from(dyn_f)) * 16 + i64::from(dyn_f));
                return self.rest();
            }
            for _ in 0..i {
                j = j * 16 + self.nybble()?;
            }
            Ok((j + (13 - dyn_f) * 16 + dyn_f).wrapping_sub(15))
        } else if i <= dyn_f {
            Ok(i)
        } else if i < 14 {
            Ok((i - dyn_f - 1) * 16 + self.nybble()? + dyn_f + 1)
        } else {
            self.repeat_count = if i == 14 { self.packed_num()? } else { 1 };
            self.run_count()
        }
    }

    /// pkin.c `unpack`: the raster as 16-bit words, then pdfTeX's row bytes.
    fn unpack(&mut self, flag: u32, width: i32, height: i32) -> Result<Vec<u8>, String> {
        let word_width = ((width + 15) / 16).max(0) as usize;
        let rows = height.max(0) as usize;
        let mut raster: Vec<u16> = Vec::with_capacity(rows * word_width);
        self.remainder = None;
        self.dyn_f = flag / 16;
        let mut turn_on = flag & 8 != 0;
        if self.dyn_f == 14 {
            self.bit_weight = 0;
            for _ in 0..height {
                let mut word = 0u32;
                let mut word_weight = 32768u32;
                for _ in 0..width {
                    if self.bit()? {
                        word += word_weight;
                    }
                    word_weight >>= 1;
                    if word_weight == 0 {
                        raster.push(word as u16);
                        word = 0;
                        word_weight = 32768;
                    }
                }
                if word_weight != 32768 {
                    raster.push(word as u16);
                }
            }
        } else {
            const GPOWER: [u32; 17] = [
                0, 1, 3, 7, 15, 31, 63, 127, 255, 511, 1023, 2047, 4095, 8191, 16383, 32767, 65535,
            ];
            let mut rows_left = height;
            let mut h_bit = width as u32;
            self.repeat_count = 0;
            let mut word_weight = 16u32;
            let mut word = 0u32;
            self.bit_weight = 0;
            while rows_left > 0 {
                let mut count = self.run_count()?;
                while count != 0 {
                    if count < word_weight && count < h_bit {
                        if turn_on {
                            word += GPOWER[word_weight as usize] - GPOWER[(word_weight - count) as usize];
                        }
                        h_bit -= count;
                        word_weight -= count;
                        count = 0;
                    } else if count >= h_bit && h_bit <= word_weight {
                        if turn_on {
                            word += GPOWER[word_weight as usize] - GPOWER[(word_weight - h_bit) as usize];
                        }
                        raster.push(word as u16);
                        for _ in 0..self.repeat_count {
                            let start = raster.len().checked_sub(word_width).ok_or("bad pk raster")?;
                            raster.extend_from_within(start..start + word_width);
                        }
                        rows_left -= self.repeat_count as i32 + 1;
                        self.repeat_count = 0;
                        word = 0;
                        word_weight = 16;
                        count -= h_bit;
                        h_bit = width as u32;
                    } else {
                        if turn_on {
                            word += GPOWER[word_weight as usize];
                        }
                        raster.push(word as u16);
                        word = 0;
                        count -= word_weight;
                        h_bit -= word_weight;
                        word_weight = 16;
                    }
                }
                turn_on = !turn_on;
            }
            if rows_left != 0 || h_bit != width as u32 {
                return Err("error while unpacking; more bits than required".into());
            }
        }
        // writepk: each row's first (width + 7) / 8 bytes of its words
        let row_bytes = ((width + 7) / 8).max(0) as usize;
        let mut out = Vec::with_capacity(rows * row_bytes);
        for row in raster.chunks(word_width.max(1)).take(rows) {
            out.extend(row.iter().flat_map(|word| word.to_be_bytes()).take(row_bytes));
        }
        Ok(out)
    }
}

/// pkin.c `readchar` over a whole PK file: its characters in file order.
pub fn read_pk(data: &[u8]) -> Result<Vec<PkChar>, String> {
    let mut r = PkReader { data, pos: 0, input_byte: 0, bit_weight: 0, dyn_f: 0, repeat_count: 0, remainder: None };
    if r.byte()? != 247 {
        return Err("bad pk file, expected pre".into());
    }
    if r.byte()? != 89 {
        return Err("bad version of pk file".into());
    }
    let comment = r.byte()? as usize;
    r.pos += comment;
    r.pos += 16; // design size, checksum, hppp, vppp
    let mut chars = Vec::new();
    loop {
        let flag = r.byte()?;
        if flag == 245 {
            return Ok(chars);
        }
        if flag < 240 {
            let (length, code, width, height, xoff, yoff) = match flag & 7 {
                0..=3 => {
                    let length = (flag & 7) as i32 * 256 + r.byte()? as i32 - 3;
                    let code = r.byte()? as i32;
                    r.pos += 3 + 1; // TFM width, pixel width
                    let width = r.byte()? as i32;
                    let height = r.byte()? as i32;
                    let xoff = r.signed(1)?;
                    let yoff = r.signed(1)?;
                    (length, code, width, height, xoff, yoff)
                }
                4..=6 => {
                    let length = (flag & 3) as i32 * 65536 + r.byte()? as i32 * 256 + r.byte()? as i32 - 4;
                    let code = r.byte()? as i32;
                    r.pos += 3 + 2;
                    let width = r.signed(2)?;
                    let height = r.signed(2)?;
                    let xoff = r.signed(2)?;
                    let yoff = r.signed(2)?;
                    (length, code, width, height, xoff, yoff)
                }
                _ => {
                    let length = r.signed(4)? - 9;
                    let code = r.signed(4)?;
                    r.pos += 4 + 4 + 4; // TFM width, dx, dy
                    let width = r.signed(4)?;
                    let height = r.signed(4)?;
                    let xoff = r.signed(4)?;
                    let yoff = r.signed(4)?;
                    (length, code, width, height, xoff, yoff)
                }
            };
            if length <= 0 {
                return Err(format!("packet length ({length}) too small"));
            }
            let rows = r.unpack(flag, width, height)?;
            chars.push(PkChar { code, width, height, xoff, yoff, rows });
        } else {
            match flag {
                240..=243 => {
                    // pk_xxx1..4: a special of a 1- to 4-byte length
                    let len = if flag == 243 {
                        i64::from(r.signed(4)?)
                    } else {
                        let mut k = 0i64;
                        for _ in 0..=flag - 240 {
                            k = k * 256 + i64::from(r.byte()?);
                        }
                        k
                    };
                    r.pos += len.max(0) as usize;
                }
                244 => r.pos += 4,
                246 => {}
                _ => return Err(format!("unexpected command ({flag})")),
            }
        }
    }
}

/// kpathsea magstep.c `magstep` (`n` in half steps).
fn magstep(n: i32, bdpi: i32) -> i32 {
    let neg = n < 0;
    let mut n = n.abs();
    let mut t = 1.0f64;
    if n & 1 != 0 {
        n &= !1;
        t = 1.095445115;
    }
    while n > 8 {
        n -= 8;
        t *= 2.0736;
    }
    while n > 0 {
        n -= 2;
        t *= 1.2;
    }
    (0.5 + if neg { f64::from(bdpi) / t } else { f64::from(bdpi) * t }) as i32
}

/// kpathsea `kpathsea_magstep_fix`: the magstep within 1 of `dpi`.
pub fn magstep_fix(dpi: i32, bdpi: i32) -> i32 {
    let sign = if dpi < bdpi { -1 } else { 1 };
    for m in 0..40 {
        let mdpi = magstep(m * sign, bdpi);
        if (mdpi - dpi).abs() <= 1 {
            return mdpi;
        }
        if (mdpi - dpi) * sign > 0 {
            return dpi;
        }
    }
    dpi
}

/// PK geometry of one engine font: its pdfTeX `pdf_font_size` (sp),
/// design size (sp) and `fixed_pk_resolution`.
#[derive(Clone, Copy, Debug)]
pub struct PkScale {
    pub font_size: i64,
    pub design_size: i64,
    pub resolution: i32,
    pub decimal_digits: u32,
}

impl PkScale {
    /// writet3.c `writepk`: the resolution of the PK file
    /// (`kpse_magstep_fix(round(res * ((float) size / dsize)), res)`).
    pub fn dpi(&self) -> i32 {
        let ratio = self.font_size as f32 / self.design_size as f32;
        let dpi = self.resolution as f32 * ratio;
        magstep_fix((f64::from(dpi) + 0.5) as i32, self.resolution)
    }

    /// pdftex.web `pk_scale_factor`.
    fn scale_factor(&self) -> i64 {
        divide_scaled(72, i64::from(self.resolution), 5 + self.decimal_digits).0
    }

    /// writet3.c `get_pk_font_scale`: the /FontMatrix entry, 5 decimals.
    pub fn font_scale(&self) -> i64 {
        let size = divide_scaled(self.font_size, 6_578_176, self.decimal_digits + 2).0;
        divide_scaled(self.scale_factor(), size, 0).0
    }

    /// writet3.c `pk_char_width`: a /Widths entry, 2 decimals.
    pub fn char_width(&self, w: i64) -> i64 {
        divide_scaled(divide_scaled(w, self.font_size, 7).0, self.font_scale(), 0).0
    }

    /// writet3.c `getpkcharwidth`: the advance of a character of TFM
    /// width `w` on the PDF text raster (C `double` arithmetic truncated).
    pub fn advance(&self, w: i64) -> i64 {
        ((self.font_scale() as f64 / 100000.0) * (self.char_width(w) as f64 / 100.0) * self.font_size as f64)
            as i64
    }
}

/// The Type 3 font writet3.c writes for a PK font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Type3Font {
    /// `/Name /F<n>`: the internal font number.
    pub name: u32,
    /// `\pdffontattr` text.
    pub font_attr: String,
    /// /FontMatrix scale in units of 10^-5.
    pub font_scale: i64,
    pub bbox: [i32; 4],
    pub first_char: u8,
    pub last_char: u8,
    /// /Widths in hundredths, `first_char..=last_char`.
    pub widths: Vec<i64>,
    /// Glyph procedures in PK file order: (code, content stream).
    pub char_procs: Vec<(u8, Vec<u8>)>,
}

/// writet3.c `writepk` + the font data of `writet3` for the characters
/// `used` of a font of TFM widths `tfm_widths`, character range `bc..=ec`.
pub fn build_type3(
    pk: &[PkChar],
    scale: &PkScale,
    tfm_widths: &dyn Fn(u8) -> i64,
    used: &[u64; 4],
    (bc, ec): (u8, u8),
) -> Type3Font {
    let marked = |c: i32| (0..=255).contains(&c) && used[c as usize / 64] & (1 << (c as usize % 64)) != 0;
    let mut widths = [0i64; 256];
    let mut bbox = [0i32; 4];
    let mut char_procs = Vec::new();
    for ch in pk {
        if !marked(ch.code) {
            continue;
        }
        let code = ch.code as u8;
        let w = scale.char_width(tfm_widths(code));
        widths[code as usize] = w;
        let (mut width, mut height, mut xoff, mut yoff) = (ch.width, ch.height, ch.xoff, ch.yoff);
        let null_glyph = width < 1 || height < 1;
        if null_glyph {
            width = (w as f64 / 100.0).round() as i32;
            height = 1;
            xoff = 0;
            yoff = 0;
        }
        let llx = -xoff;
        let lly = yoff - height + 1;
        let urx = width + llx + 1;
        let ury = height + lly;
        if char_procs.is_empty() {
            bbox = [llx, lly, urx, ury];
        } else {
            bbox = [bbox[0].min(llx), bbox[1].min(lly), bbox[2].max(urx), bbox[3].max(ury)];
        }
        let mut s = String::new();
        crate::pdfrender::push_decimal(&mut s, w, 2);
        s.push_str(&format!(" 0 {llx} {lly} {urx} {ury} d1\n"));
        let mut stream = s.into_bytes();
        if !null_glyph {
            stream.extend_from_slice(
                format!("q\n{width} 0 0 {height} {llx} {lly} cm\nBI\n/W {width}\n/H {height}\n/IM true\n/BPC 1\n/D [1 0]\nID ")
                    .as_bytes(),
            );
            stream.extend_from_slice(&ch.rows);
            stream.extend_from_slice(b"\nEI\nQ\n");
        }
        char_procs.push((code, stream));
    }
    let first_char = (bc..=ec).find(|&c| marked(i32::from(c))).unwrap_or(ec);
    let last_char = (first_char..=ec).rev().find(|&c| marked(i32::from(c))).unwrap_or(first_char);
    Type3Font {
        name: 0,
        font_attr: String::new(),
        font_scale: scale.font_scale(),
        bbox,
        first_char,
        last_char,
        widths: widths[first_char as usize..=last_char as usize].to_vec(),
        char_procs,
    }
}

impl Type3Font {
    /// The /Encoding /Differences array (writet3.c): `/a<code>` for each
    /// glyph, `/.notdef` runs over the codes without one.
    pub fn differences(&self) -> String {
        let has = |c: u8| self.char_procs.iter().any(|(code, _)| *code == c);
        let mut s = format!("[{}", self.first_char);
        let mut is_notdef = !has(self.first_char);
        if is_notdef {
            s.push_str("/.notdef");
        } else {
            s.push_str(&format!("/a{}", self.first_char));
        }
        for c in (u16::from(self.first_char) + 1..=u16::from(self.last_char)).map(|c| c as u8) {
            if !has(c) {
                if !is_notdef {
                    s.push_str(&format!(" {c}/.notdef"));
                    is_notdef = true;
                }
            } else {
                if is_notdef {
                    s.push_str(&format!(" {c}"));
                    is_notdef = false;
                }
                s.push_str(&format!("/a{c}"));
            }
        }
        s.push(']');
        s
    }

    /// The /Widths array: `pdfprintreal(w, 2)` and a space per entry.
    pub fn widths_array(&self) -> String {
        let mut s = String::from("[");
        for &w in &self.widths {
            crate::pdfrender::push_decimal(&mut s, w, 2);
            s.push(' ');
        }
        s.push(']');
        s
    }

    /// The /FontMatrix scale, `pdfprintreal(scale, 5)`.
    pub fn font_matrix(&self) -> String {
        let mut scale = String::new();
        crate::pdfrender::push_decimal(&mut scale, self.font_scale, 5);
        format!("[{scale} 0 0 {scale} 0 0]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magstep_fix_snaps_like_kpathsea() {
        assert_eq!(magstep_fix(657, 600), 657);
        assert_eq!(magstep_fix(658, 600), 657);
        assert_eq!(magstep_fix(1493, 600), 1493);
        assert_eq!(magstep_fix(660, 600), 660);
        assert_eq!(magstep_fix(300, 600), 300);
        assert_eq!(magstep_fix(732, 600), 732);
    }
}
