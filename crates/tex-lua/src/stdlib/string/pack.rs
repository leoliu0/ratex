// string.pack, string.packsize and string.unpack: a port of the
// PACK/UNPACK section of lstrlib.c (Lua 5.3 and 5.5 behaviour).

use crate::LuaLanguageLevel;
use crate::LuaValue;
use crate::lua_vm::{LuaResult, LuaState};
use crate::stdlib::lauxlib;

/// Maximum size for the binary representation of an integer.
const MAXINTSIZE: usize = 16;
/// Size of a lua_Integer.
const SZINT: usize = 8;
/// Native alignment of the platform (offsetof(struct cD, u)).
const NATIVE_ALIGN: usize = 8;
const NATIVE_LITTLE: bool = cfg!(target_endian = "little");

#[derive(Clone, Copy, PartialEq, Eq)]
enum KOption {
    Int,
    Uint,
    /// C float.
    Float,
    /// lua_Number or C double (same representation here).
    Double,
    Char,
    String,
    Zstr,
    Padding,
    PaddAlign,
    Nop,
}

struct Header {
    lua53: bool,
    little: bool,
    maxalign: usize,
}

/// Format cursor; the format ends at its first zero byte, as C sees it.
struct Format<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Format<'_> {
    #[inline]
    fn peek(&self) -> u8 {
        self.bytes.get(self.pos).copied().unwrap_or(0)
    }

    #[inline]
    fn at_end(&self) -> bool {
        self.peek() == 0
    }

    /// `getnum`: read a numeral, or return `default` if there is none.
    fn number(&mut self, lua53: bool, default: usize) -> usize {
        if !self.peek().is_ascii_digit() {
            return default;
        }
        // 5.3 limits sizes to int (MAXSIZE = INT_MAX); 5.4+ to lua_Integer.
        let limit = if lua53 {
            (i32::MAX as usize - 9) / 10
        } else {
            (i64::MAX as usize - 9) / 10
        };
        let mut a: usize = 0;
        loop {
            a = a * 10 + usize::from(self.peek() - b'0');
            self.pos += 1;
            if !self.peek().is_ascii_digit() || a > limit {
                return a;
            }
        }
    }
}

impl Header {
    /// `getnumlimit`: a size in [1, MAXINTSIZE].
    fn number_limit(&self, l: &mut LuaState, fmt: &mut Format, default: usize) -> LuaResult<usize> {
        let size = fmt.number(self.lua53, default);
        if size == 0 || size > MAXINTSIZE {
            // C prints the size with %d (an int).
            let shown = if self.lua53 { size as i32 } else { size as u32 as i32 };
            return Err(lauxlib::lual_error(
                l,
                format!("integral size ({shown}) out of limits [1,{MAXINTSIZE}]"),
            ));
        }
        Ok(size)
    }

    /// `getoption`: read and classify the next option and its size.
    fn option(&mut self, l: &mut LuaState, fmt: &mut Format) -> LuaResult<(KOption, usize)> {
        let opt = fmt.peek();
        fmt.pos += 1;
        Ok(match opt {
            b'b' => (KOption::Int, 1),
            b'B' => (KOption::Uint, 1),
            b'h' => (KOption::Int, 2),
            b'H' => (KOption::Uint, 2),
            b'l' | b'j' => (KOption::Int, 8),
            b'L' | b'J' | b'T' => (KOption::Uint, 8),
            b'f' => (KOption::Float, 4),
            b'n' | b'd' => (KOption::Double, 8),
            b'i' => (KOption::Int, self.number_limit(l, fmt, 4)?),
            b'I' => (KOption::Uint, self.number_limit(l, fmt, 4)?),
            b's' => (KOption::String, self.number_limit(l, fmt, 8)?),
            b'c' => {
                let size = fmt.number(self.lua53, usize::MAX);
                if size == usize::MAX {
                    return Err(lauxlib::lual_error(l, "missing size for format option 'c'"));
                }
                (KOption::Char, size)
            }
            b'z' => (KOption::Zstr, 0),
            b'x' => (KOption::Padding, 1),
            b'X' => (KOption::PaddAlign, 0),
            b' ' => (KOption::Nop, 0),
            b'<' => {
                self.little = true;
                (KOption::Nop, 0)
            }
            b'>' => {
                self.little = false;
                (KOption::Nop, 0)
            }
            b'=' => {
                self.little = NATIVE_LITTLE;
                (KOption::Nop, 0)
            }
            b'!' => {
                self.maxalign = self.number_limit(l, fmt, NATIVE_ALIGN)?;
                (KOption::Nop, 0)
            }
            _ => {
                let shown = String::from_utf8_lossy(&[opt]).into_owned();
                return Err(lauxlib::lual_error(l, format!("invalid format option '{shown}'")));
            }
        })
    }

    /// `getdetails`: the next option, its size and the padding needed to
    /// align it at offset `total`.
    fn details(
        &mut self,
        l: &mut LuaState,
        fmt: &mut Format,
        total: usize,
    ) -> LuaResult<(KOption, usize, usize)> {
        let (opt, size) = self.option(l, fmt)?;
        let mut align = size;
        if opt == KOption::PaddAlign {
            // 'X' takes its alignment from the following option, which is
            // otherwise ignored.
            let next = if fmt.at_end() { None } else { Some(self.option(l, fmt)?) };
            match next {
                Some((next, next_size)) if next != KOption::Char && next_size != 0 => {
                    align = next_size;
                }
                _ => return Err(lauxlib::argerror(l, 1, "invalid next option for option 'X'")),
            }
        }
        if align <= 1 || opt == KOption::Char {
            return Ok((opt, size, 0));
        }
        let align = align.min(self.maxalign);
        if !align.is_power_of_two() {
            return Err(lauxlib::argerror(l, 1, "format asks for alignment not power of 2"));
        }
        Ok((opt, size, (align - (total & (align - 1))) & (align - 1)))
    }
}

fn header(l: &LuaState) -> Header {
    Header {
        lua53: l.global_state().language() == LuaLanguageLevel::Lua53,
        little: NATIVE_LITTLE,
        maxalign: 1,
    }
}

/// The format string, cut at its first zero byte.
fn format_arg(l: &mut LuaState) -> LuaResult<Vec<u8>> {
    let fmt = lauxlib::check_lstring(l, 1)?;
    Ok(fmt.split(|&c| c == 0).next().unwrap_or_default().to_vec())
}

/// `packint`: `size` bytes of `n`, sign-extended past 8 bytes if negative.
fn pack_int(out: &mut Vec<u8>, n: u64, little: bool, size: usize, negative: bool) {
    let start = out.len();
    out.extend((0..size).map(|i| {
        if i < SZINT {
            (n >> (8 * i)) as u8
        } else if negative {
            0xff
        } else {
            0
        }
    }));
    if !little {
        out[start..].reverse();
    }
}

fn push_endian(out: &mut Vec<u8>, bytes: &[u8], little: bool) {
    if little {
        out.extend_from_slice(bytes);
    } else {
        out.extend(bytes.iter().rev());
    }
}

/// C's str_pack pushes a nil after its arguments, so a missing value is
/// reported as "got nil" rather than "got no value".
fn check_present(l: &mut LuaState, arg: usize, expected: &str) -> LuaResult<()> {
    if arg > l.arg_count() {
        return Err(lauxlib::argerror(l, arg, &format!("{expected} expected, got nil")));
    }
    Ok(())
}

pub fn string_pack(l: &mut LuaState) -> LuaResult<usize> {
    let fmt_bytes = format_arg(l)?;
    let mut fmt = Format { bytes: &fmt_bytes, pos: 0 };
    let mut h = header(l);
    let mut out = Vec::new();
    let mut arg = 1;
    while !fmt.at_end() {
        let (opt, size, ntoalign) = h.details(l, &mut fmt, out.len())?;
        if !h.lua53 && size.checked_add(ntoalign).is_none_or(|n| n > i64::MAX as usize - out.len()) {
            return Err(lauxlib::argerror(l, arg, "result too long"));
        }
        out.resize(out.len() + ntoalign, 0);
        arg += 1;
        match opt {
            KOption::Int | KOption::Uint | KOption::Float | KOption::Double => {
                check_present(l, arg, "number")?
            }
            KOption::Char | KOption::String | KOption::Zstr => check_present(l, arg, "string")?,
            _ => {}
        }
        match opt {
            KOption::Int => {
                let n = lauxlib::check_integer(l, arg)?;
                if size < SZINT {
                    let limit = 1i64 << (size * 8 - 1);
                    if !(-limit..limit).contains(&n) {
                        return Err(lauxlib::argerror(l, arg, "integer overflow"));
                    }
                }
                pack_int(&mut out, n as u64, h.little, size, n < 0);
            }
            KOption::Uint => {
                let n = lauxlib::check_integer(l, arg)?;
                if size < SZINT && (n as u64) >= 1u64 << (size * 8) {
                    return Err(lauxlib::argerror(l, arg, "unsigned overflow"));
                }
                pack_int(&mut out, n as u64, h.little, size, false);
            }
            KOption::Float => {
                let n = lauxlib::check_number(l, arg)? as f32;
                push_endian(&mut out, &n.to_le_bytes(), h.little);
            }
            KOption::Double => {
                let n = lauxlib::check_number(l, arg)?;
                push_endian(&mut out, &n.to_le_bytes(), h.little);
            }
            KOption::Char => {
                let s = lauxlib::check_lstring(l, arg)?;
                if s.len() > size {
                    return Err(lauxlib::argerror(l, arg, "string longer than given size"));
                }
                out.extend_from_slice(&s);
                out.resize(out.len() + (size - s.len()), 0);
            }
            KOption::String => {
                let s = lauxlib::check_lstring(l, arg)?;
                if size < 8 && (s.len() as u64) >= 1u64 << (size * 8) {
                    return Err(lauxlib::argerror(
                        l,
                        arg,
                        "string length does not fit in given size",
                    ));
                }
                pack_int(&mut out, s.len() as u64, h.little, size, false);
                out.extend_from_slice(&s);
            }
            KOption::Zstr => {
                let s = lauxlib::check_lstring(l, arg)?;
                if s.contains(&0) {
                    return Err(lauxlib::argerror(l, arg, "string contains zeros"));
                }
                out.extend_from_slice(&s);
                out.push(0);
            }
            KOption::Padding => {
                out.push(0);
                arg -= 1;
            }
            KOption::PaddAlign | KOption::Nop => arg -= 1,
        }
    }
    let result = l.create_bytes(&out)?;
    l.push_value(result)?;
    Ok(1)
}

pub fn string_packsize(l: &mut LuaState) -> LuaResult<usize> {
    let fmt_bytes = format_arg(l)?;
    let mut fmt = Format { bytes: &fmt_bytes, pos: 0 };
    let mut h = header(l);
    let max_size = if h.lua53 { i32::MAX as usize } else { i64::MAX as usize };
    let mut total: usize = 0;
    while !fmt.at_end() {
        let (opt, size, ntoalign) = h.details(l, &mut fmt, total)?;
        let variable = matches!(opt, KOption::String | KOption::Zstr);
        // 5.4+ rejects variable-length options before the size check.
        if variable && !h.lua53 {
            return Err(lauxlib::argerror(l, 1, "variable-length format"));
        }
        let size = size + ntoalign;
        if size > max_size || total > max_size - size {
            return Err(lauxlib::argerror(l, 1, "format result too large"));
        }
        total += size;
        if variable {
            return Err(lauxlib::argerror(l, 1, "variable-length format"));
        }
    }
    l.push_value(LuaValue::integer(total as i64))?;
    Ok(1)
}

/// `unpackint`: read a `size`-byte integer, checking that bytes beyond the
/// eighth are only sign extension.
fn unpack_int(l: &mut LuaState, bytes: &[u8], little: bool, signed: bool) -> LuaResult<i64> {
    let size = bytes.len();
    let byte = |i: usize| if little { bytes[i] } else { bytes[size - 1 - i] };
    let limit = size.min(SZINT);
    let mut res: u64 = 0;
    for i in (0..limit).rev() {
        res = (res << 8) | u64::from(byte(i));
    }
    if size < SZINT {
        if signed {
            let mask = 1u64 << (size * 8 - 1);
            res = (res ^ mask).wrapping_sub(mask);
        }
    } else if size > SZINT {
        let mask = if !signed || (res as i64) >= 0 { 0 } else { 0xff };
        if (limit..size).any(|i| byte(i) != mask) {
            return Err(lauxlib::lual_error(
                l,
                format!("{size}-byte integer does not fit into Lua Integer"),
            ));
        }
    }
    Ok(res as i64)
}

pub fn string_unpack(l: &mut LuaState) -> LuaResult<usize> {
    let fmt_bytes = format_arg(l)?;
    let mut fmt = Format { bytes: &fmt_bytes, pos: 0 };
    let data_arg = lauxlib::check_lstring(l, 2)?;
    let data: &[u8] = &data_arg;
    let ld = data.len();
    let mut h = header(l);
    let init = lauxlib::opt_integer(l, 3, 1)?;
    // 5.3 posrelat (0 and too-negative give 0, i.e. an invalid position);
    // 5.4+ posrelatI (clips those to 1).
    let pos1 = if init > 0 {
        init as u64
    } else if init == 0 || init.unsigned_abs() > ld as u64 {
        u64::from(!h.lua53)
    } else {
        (ld as i64 + init + 1) as u64
    };
    if pos1 == 0 || pos1 - 1 > ld as u64 {
        return Err(lauxlib::argerror(l, 3, "initial position out of string"));
    }
    let mut pos = (pos1 - 1) as usize;
    let mut n = 0;
    while !fmt.at_end() {
        let (opt, size, ntoalign) = h.details(l, &mut fmt, pos)?;
        if ntoalign.checked_add(size).is_none_or(|need| need > ld.saturating_sub(pos)) || pos > ld {
            return Err(lauxlib::argerror(l, 2, "data string too short"));
        }
        pos += ntoalign;
        let value = match opt {
            KOption::Int | KOption::Uint => {
                LuaValue::integer(unpack_int(l, &data[pos..pos + size], h.little, opt == KOption::Int)?)
            }
            KOption::Float => {
                let mut raw = [0u8; 4];
                raw.copy_from_slice(&data[pos..pos + 4]);
                if !h.little {
                    raw.reverse();
                }
                LuaValue::float(f64::from(f32::from_le_bytes(raw)))
            }
            KOption::Double => {
                let mut raw = [0u8; 8];
                raw.copy_from_slice(&data[pos..pos + 8]);
                if !h.little {
                    raw.reverse();
                }
                LuaValue::float(f64::from_le_bytes(raw))
            }
            KOption::Char => l.create_bytes(&data[pos..pos + size])?,
            KOption::String => {
                let len = unpack_int(l, &data[pos..pos + size], h.little, false)? as u64;
                if len > (ld - pos - size) as u64 {
                    return Err(lauxlib::argerror(l, 2, "data string too short"));
                }
                let start = pos + size;
                pos += len as usize;
                l.create_bytes(&data[start..start + len as usize])?
            }
            KOption::Zstr => {
                let rest = &data[pos..];
                let len = match rest.iter().position(|&c| c == 0) {
                    Some(len) => len,
                    // 5.3 reads up to the terminating zero C strings have.
                    None if h.lua53 => rest.len(),
                    None => {
                        return Err(lauxlib::argerror(l, 2, "unfinished string for format 'z'"));
                    }
                };
                let value = l.create_bytes(&rest[..len])?;
                pos += len + 1;
                value
            }
            KOption::PaddAlign | KOption::Padding | KOption::Nop => {
                pos += size;
                continue;
            }
        };
        l.push_value(value)?;
        n += 1;
        pos += size;
    }
    l.push_value(LuaValue::integer(pos as i64 + 1))?;
    Ok(n + 1)
}
