//! C `printf`-compatible number formatting.
//!
//! Lua formats numbers through the C library: `tostring` uses
//! `lua_Number2str` (`"%.14g"` in Lua 5.3; `"%.15g"`, or `"%.17g"` when that
//! does not round-trip, in Lua 5.5) and `string.format` hands each conversion
//! to `snprintf`. Rust's exact float formatting (`{:.*e}` / `{:.*}`, which
//! round half to even on the exact binary value, as glibc does) supplies the
//! digits; this module reproduces the C layout: `e+05` exponents, `%g` style
//! selection and trailing-zero removal, `%a` hexadecimal floats, flags,
//! width and precision.

use std::fmt::Write as _;

use crate::LuaLanguageLevel;

/// Fixed-capacity byte buffer on the stack.
pub(crate) struct StackBuf<const N: usize> {
    len: usize,
    buf: [u8; N],
}

impl<const N: usize> StackBuf<N> {
    pub(crate) const fn new() -> Self {
        Self { len: 0, buf: [0; N] }
    }

    #[inline]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// The contents; only ASCII is ever written into number buffers.
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        std::str::from_utf8(self.as_bytes()).unwrap_or_default()
    }

    #[inline]
    fn push(&mut self, byte: u8) {
        self.buf[self.len] = byte;
        self.len += 1;
    }

    #[inline]
    fn extend(&mut self, bytes: &[u8]) {
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }
}

impl<const N: usize> std::fmt::Write for StackBuf<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        if self.len + s.len() > N {
            return Err(std::fmt::Error);
        }
        self.extend(s.as_bytes());
        Ok(())
    }
}

/// Buffer large enough for any `tostring` of a number.
pub(crate) type NumBuf = StackBuf<48>;

/// Flags, width and precision of one `printf` conversion.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Spec {
    pub left: bool,
    pub plus: bool,
    pub space: bool,
    pub alt: bool,
    pub zero: bool,
    pub width: usize,
    pub precision: Option<usize>,
}

/// A finite non-negative float rounded to `ndigits` significant digits:
/// Rust's `{:.*e}` output `d.ddde<exp>`.
struct Sci {
    buf: StackBuf<128>,
    mant_end: usize,
    exp: i32,
}

impl Sci {
    fn new(abs: f64, ndigits: usize) -> Sci {
        debug_assert!((1..=110).contains(&ndigits));
        let mut buf = StackBuf::new();
        let _ = write!(buf, "{:.*e}", ndigits - 1, abs);
        let bytes = buf.as_bytes();
        let mant_end = bytes.iter().position(|&b| b == b'e').unwrap_or(bytes.len());
        let mut exp: i32 = 0;
        let mut negative = false;
        for &b in &bytes[(mant_end + 1).min(bytes.len())..] {
            if b == b'-' {
                negative = true;
            } else {
                exp = exp * 10 + i32::from(b - b'0');
            }
        }
        Sci {
            buf,
            mant_end,
            exp: if negative { -exp } else { exp },
        }
    }

    fn first(&self) -> u8 {
        self.buf.buf[0]
    }

    /// The significant digits after the first one.
    fn rest(&self) -> &[u8] {
        if self.mant_end > 2 {
            &self.buf.buf[2..self.mant_end]
        } else {
            &[]
        }
    }
}

fn push_exponent<const N: usize>(out: &mut StackBuf<N>, exp: i32, upper: bool) {
    out.push(if upper { b'E' } else { b'e' });
    out.push(if exp < 0 { b'-' } else { b'+' });
    let exp = exp.unsigned_abs();
    if exp < 10 {
        out.push(b'0');
    }
    let mut digits = itoa::Buffer::new();
    out.extend(digits.format(exp).as_bytes());
}

/// `%e` body of a finite non-negative value.
fn body_e<const N: usize>(out: &mut StackBuf<N>, abs: f64, precision: usize, alt: bool, upper: bool) {
    let sci = Sci::new(abs, precision + 1);
    out.push(sci.first());
    if precision > 0 || alt {
        out.push(b'.');
    }
    out.extend(sci.rest());
    push_exponent(out, sci.exp, upper);
}

/// `%f` body of a finite non-negative value.
fn body_f<const N: usize>(out: &mut StackBuf<N>, abs: f64, precision: usize, alt: bool) {
    let _ = write!(out, "{:.*}", precision, abs);
    if precision == 0 && alt {
        out.push(b'.');
    }
}

/// `%g` body of a finite non-negative value.
fn body_g<const N: usize>(out: &mut StackBuf<N>, abs: f64, precision: usize, alt: bool, upper: bool) {
    let p = precision.max(1);
    let sci = Sci::new(abs, p);
    let x = sci.exp;
    let start = out.len;
    if x < p as i32 && x >= -4 {
        // Fixed notation with p - 1 - x decimals, built from the same digits.
        let rest = sci.rest();
        if x >= 0 {
            let int_rest = x as usize;
            out.push(sci.first());
            out.extend(&rest[..int_rest]);
            out.push(b'.');
            out.extend(&rest[int_rest..]);
        } else {
            out.extend(b"0.");
            for _ in 0..(-x - 1) {
                out.push(b'0');
            }
            out.push(sci.first());
            out.extend(rest);
        }
        if !alt {
            strip_fraction_zeros(out, start);
        }
    } else {
        out.push(sci.first());
        out.push(b'.');
        out.extend(sci.rest());
        if !alt {
            strip_fraction_zeros(out, start);
        }
        push_exponent(out, x, upper);
    }
}

/// Remove trailing fraction zeros (and a bare decimal point) from the
/// number written at `out[start..]`, which contains a '.'.
fn strip_fraction_zeros<const N: usize>(out: &mut StackBuf<N>, start: usize) {
    while out.len > start && out.buf[out.len - 1] == b'0' {
        out.len -= 1;
    }
    if out.len > start && out.buf[out.len - 1] == b'.' {
        out.len -= 1;
    }
}

/// `%a` body of a finite non-negative value (glibc layout: subnormals as
/// `0x0.xxxp-1022`, no renormalization after rounding).
fn body_a<const N: usize>(out: &mut StackBuf<N>, abs: f64, precision: Option<usize>, alt: bool, upper: bool) {
    let bits = abs.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = bits & ((1u64 << 52) - 1);
    let (mut lead, exp) = if abs == 0.0 {
        (0u8, 0)
    } else if biased == 0 {
        (0u8, -1022)
    } else {
        (1u8, biased - 1023)
    };
    let mut digits = [0u8; 13];
    for (i, digit) in digits.iter_mut().enumerate() {
        *digit = ((mantissa >> (48 - 4 * i)) & 0xf) as u8;
    }
    let mut ndigits = 13;
    while ndigits > 0 && digits[ndigits - 1] == 0 {
        ndigits -= 1;
    }
    let shown = match precision {
        None => ndigits,
        Some(p) if p >= ndigits => p,
        Some(p) => {
            let last = if p > 0 { digits[p - 1] } else { lead };
            let next = digits[p];
            let more = (next & 7) != 0 || p + 1 < ndigits;
            if next >= 8 && (last & 1 == 1 || more) {
                let mut i = p;
                loop {
                    if i == 0 {
                        lead += 1;
                        break;
                    }
                    i -= 1;
                    if digits[i] == 15 {
                        digits[i] = 0;
                    } else {
                        digits[i] += 1;
                        break;
                    }
                }
            }
            p
        }
    };
    let hex: &[u8; 16] = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    out.push(hex[lead as usize]);
    if shown > 0 || alt {
        out.push(b'.');
    }
    for i in 0..shown {
        out.push(if i < 13 { hex[digits[i] as usize] } else { b'0' });
    }
    out.push(if upper { b'P' } else { b'p' });
    out.push(if exp < 0 { b'-' } else { b'+' });
    let mut buffer = itoa::Buffer::new();
    out.extend(buffer.format(exp.unsigned_abs()).as_bytes());
}

/// Append `sign`, `prefix` and `body` padded to the spec's width.
fn push_padded(out: &mut Vec<u8>, sign: Option<u8>, prefix: &[u8], body: &[u8], spec: &Spec, zero_pad: bool) {
    let len = usize::from(sign.is_some()) + prefix.len() + body.len();
    let fill = spec.width.saturating_sub(len);
    if fill > 0 && !spec.left && !zero_pad {
        out.resize(out.len() + fill, b' ');
    }
    if let Some(sign) = sign {
        out.push(sign);
    }
    out.extend_from_slice(prefix);
    if fill > 0 && !spec.left && zero_pad {
        out.resize(out.len() + fill, b'0');
    }
    out.extend_from_slice(body);
    if fill > 0 && spec.left {
        out.resize(out.len() + fill, b' ');
    }
}

/// Append `bytes` padded with spaces (`%s`, `%c`, `%p`).
pub(crate) fn push_padded_bytes(out: &mut Vec<u8>, bytes: &[u8], spec: &Spec) {
    push_padded(out, None, b"", bytes, spec, false);
}

/// Append one floating-point conversion (`conv` is one of `aAeEfFgG`).
pub(crate) fn push_float(out: &mut Vec<u8>, n: f64, conv: u8, spec: &Spec) {
    let upper = conv.is_ascii_uppercase();
    let sign = if n.is_sign_negative() {
        Some(b'-')
    } else if spec.plus {
        Some(b'+')
    } else if spec.space {
        Some(b' ')
    } else {
        None
    };
    let zero_pad = spec.zero && !spec.left;
    if !n.is_finite() {
        let body: &[u8] = match (n.is_nan(), upper) {
            (true, false) => b"nan",
            (true, true) => b"NAN",
            (false, false) => b"inf",
            (false, true) => b"INF",
        };
        push_padded(out, sign, b"", body, spec, false);
        return;
    }
    let abs = n.abs();
    let mut body = StackBuf::<512>::new();
    let prefix: &[u8] = match conv {
        b'e' | b'E' => {
            body_e(&mut body, abs, spec.precision.unwrap_or(6), spec.alt, upper);
            b""
        }
        b'f' | b'F' => {
            body_f(&mut body, abs, spec.precision.unwrap_or(6), spec.alt);
            b""
        }
        b'g' | b'G' => {
            body_g(&mut body, abs, spec.precision.unwrap_or(6), spec.alt, upper);
            b""
        }
        _ => {
            body_a(&mut body, abs, spec.precision, spec.alt, upper);
            if upper { b"0X" } else { b"0x" }
        }
    };
    push_padded(out, sign, prefix, body.as_bytes(), spec, zero_pad);
}

/// Append one integer conversion (`conv` is one of `diuoxX`).
pub(crate) fn push_int(out: &mut Vec<u8>, n: i64, conv: u8, spec: &Spec) {
    let mut digits = StackBuf::<24>::new();
    let signed = matches!(conv, b'd' | b'i');
    let magnitude = if signed { n.unsigned_abs() } else { n as u64 };
    if !(spec.precision == Some(0) && magnitude == 0) {
        let _ = match conv {
            b'o' => write!(digits, "{magnitude:o}"),
            b'x' => write!(digits, "{magnitude:x}"),
            b'X' => write!(digits, "{magnitude:X}"),
            _ => write!(digits, "{magnitude}"),
        };
    }
    let mut zeros = spec.precision.unwrap_or(0).saturating_sub(digits.len);
    if conv == b'o' && spec.alt && zeros == 0 && digits.as_bytes().first() != Some(&b'0') {
        zeros = 1;
    }
    let mut body = StackBuf::<128>::new();
    for _ in 0..zeros {
        body.push(b'0');
    }
    body.extend(digits.as_bytes());
    let sign = if !signed {
        None
    } else if n < 0 {
        Some(b'-')
    } else if spec.plus {
        Some(b'+')
    } else if spec.space {
        Some(b' ')
    } else {
        None
    };
    let prefix: &[u8] = match conv {
        b'x' if spec.alt && magnitude != 0 => b"0x",
        b'X' if spec.alt && magnitude != 0 => b"0X",
        _ => b"",
    };
    let zero_pad = spec.zero && !spec.left && spec.precision.is_none();
    push_padded(out, sign, prefix, body.as_bytes(), spec, zero_pad);
}

/// `%.<precision>g` of a float, as `tostring` and `lua_Number2str` use it.
fn push_g<const N: usize>(out: &mut StackBuf<N>, n: f64, precision: usize) {
    if n.is_nan() {
        out.extend(if n.is_sign_negative() { b"-nan" } else { b"nan" });
    } else if n.is_infinite() {
        out.extend(if n < 0.0 { b"-inf" } else { b"inf" });
    } else {
        if n.is_sign_negative() {
            out.push(b'-');
        }
        body_g(out, n.abs(), precision, false, false);
    }
}

/// `lua_Number2str` of the dialect without the `.0` suffix that `tostring`
/// adds (used where C Lua prints with `LUA_NUMBER_FMT`, e.g. `io.write` in 5.3).
pub(crate) fn number2str(n: f64, level: LuaLanguageLevel) -> NumBuf {
    let mut buf = NumBuf::new();
    if level == LuaLanguageLevel::Lua53 {
        push_g(&mut buf, n, 14);
    } else {
        push_g(&mut buf, n, 15);
        if n.is_finite() && buf.as_str().parse::<f64>().ok() != Some(n) {
            buf.len = 0;
            push_g(&mut buf, n, 17);
        }
    }
    buf
}

/// `tostring` of a float in the given dialect (`1.0`, `1e+15`, `-nan`).
pub(crate) fn tostring_float(n: f64, level: LuaLanguageLevel) -> NumBuf {
    let mut buf = number2str(n, level);
    if buf.as_bytes().iter().all(|&b| b == b'-' || b.is_ascii_digit()) {
        buf.extend(b".0");
    }
    buf
}

/// `tostring` of a float in the given dialect, as an owned string.
pub(crate) fn lua_float_to_string(n: f64, level: LuaLanguageLevel) -> String {
    tostring_float(n, level).as_str().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printf(conv: u8, spec: Spec, n: f64) -> String {
        let mut out = Vec::new();
        push_float(&mut out, n, conv, &spec);
        String::from_utf8(out).unwrap()
    }

    fn printf_int(conv: u8, spec: Spec, n: i64) -> String {
        let mut out = Vec::new();
        push_int(&mut out, n, conv, &spec);
        String::from_utf8(out).unwrap()
    }

    fn spec(width: usize, precision: Option<usize>) -> Spec {
        Spec {
            width,
            precision,
            ..Spec::default()
        }
    }

    #[test]
    fn float_conversions_match_glibc() {
        assert_eq!(printf(b'e', Spec::default(), 0.0), "0.000000e+00");
        assert_eq!(printf(b'e', spec(22, Some(14)), 3.14159265358979), "  3.14159265358979e+00");
        assert_eq!(printf(b'E', Spec { alt: true, ..spec(0, Some(0)) }, 1.0), "1.E+00");
        assert_eq!(printf(b'f', Spec { zero: true, ..spec(10, Some(2)) }, -2.5), "-000002.50");
        assert_eq!(printf(b'f', spec(0, Some(0)), 2.5), "2");
        assert_eq!(printf(b'g', Spec::default(), 1e20), "1e+20");
        assert_eq!(printf(b'g', Spec::default(), 100000.0), "100000");
        assert_eq!(printf(b'g', Spec::default(), 1e-5), "1e-05");
        assert_eq!(printf(b'g', spec(0, Some(3)), 99950.0), "1e+05");
        assert_eq!(printf(b'g', spec(0, Some(3)), 0.0001234), "0.000123");
        assert_eq!(printf(b'g', Spec { alt: true, ..Spec::default() }, 1.0), "1.00000");
        assert_eq!(printf(b'G', Spec { plus: true, ..Spec::default() }, f64::NAN), "+NAN");
        assert_eq!(printf(b'f', Spec { zero: true, ..spec(6, None) }, f64::NEG_INFINITY), "  -inf");
        assert_eq!(printf(b'a', Spec::default(), 5e-324), "0x0.0000000000001p-1022");
        assert_eq!(printf(b'a', Spec::default(), 1.0), "0x1p+0");
        assert_eq!(printf(b'a', spec(0, Some(0)), 1.5), "0x2p+0");
        assert_eq!(printf(b'a', spec(0, Some(0)), 2.5), "0x1p+1");
        assert_eq!(printf(b'a', spec(0, Some(1)), 1.96875), "0x2.0p+0");
        assert_eq!(printf(b'A', Spec { zero: true, ..spec(10, None) }, 1.0), "0X00001P+0");
        assert_eq!(printf(b'a', spec(0, Some(20)), 1.0 / 3.0), "0x1.55555555555550000000p-2");
    }

    #[test]
    fn integer_conversions_match_glibc() {
        assert_eq!(printf_int(b'd', Spec { space: true, zero: true, ..spec(4, None) }, 0), " 000");
        assert_eq!(printf_int(b'd', Spec { plus: true, ..spec(17, Some(0)) }, 0), "                +");
        assert_eq!(printf_int(b'x', Spec { alt: true, ..spec(0, Some(11)) }, 255), "0x000000000ff");
        assert_eq!(printf_int(b'x', Spec { zero: true, ..spec(23, Some(1)) }, i64::MIN), "       8000000000000000");
        assert_eq!(printf_int(b'o', Spec { alt: true, ..spec(14, Some(3)) }, 0), "           000");
        assert_eq!(printf_int(b'o', Spec { alt: true, ..Spec::default() }, 8), "010");
        assert_eq!(printf_int(b'u', Spec::default(), -1), "18446744073709551615");
        assert_eq!(printf_int(b'd', Spec { left: true, ..spec(6, Some(3)) }, -7), "-007  ");
    }

    #[test]
    fn tostring_follows_the_dialect() {
        let l53 = LuaLanguageLevel::Lua53;
        let l55 = LuaLanguageLevel::Lua55;
        assert_eq!(lua_float_to_string(0.1 + 0.2, l53), "0.3");
        assert_eq!(lua_float_to_string(0.1 + 0.2, l55), "0.30000000000000004");
        assert_eq!(lua_float_to_string(1e15, l53), "1e+15");
        assert_eq!(lua_float_to_string(2f64.powi(63), l53), "9.2233720368548e+18");
        assert_eq!(lua_float_to_string(2f64.powi(63), l55), "9.2233720368547758e+18");
        assert_eq!(lua_float_to_string(-0.0, l53), "-0.0");
        assert_eq!(lua_float_to_string(100.0, l55), "100.0");
        assert_eq!(lua_float_to_string(1e100, l53), "1e+100");
        assert_eq!(lua_float_to_string(f64::INFINITY, l53), "inf");
        assert_eq!(lua_float_to_string(-f64::NAN, l55), "-nan");
        assert_eq!(number2str(1.0, l53).as_str(), "1");
    }
}
