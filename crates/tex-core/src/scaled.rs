//! TeX fixed-point (`scaled`) arithmetic: dimensions in sp where 65536 sp = 1 pt.

pub const ONE: i32 = 65536;
pub const INF_BAD: i32 = 10000;
pub const INF_PENALTY: i32 = 10000;
pub const EJECT_PENALTY: i32 = -10000;
pub const AWFUL_BAD: i32 = 0x3FFFFFFF;
pub const TOO_WIDE: i32 = 0x7FFFFFFF;

#[inline]
fn saturate(v: i128) -> i32 {
    v.clamp(i32::MIN as i128, i32::MAX as i128) as i32
}

/// TeX's mult(n, x): round(n*x/2^16), saturating on overflow.
#[inline]
pub fn mult(n: i32, x: i32) -> i32 {
    let prod = n as i128 * x as i128;
    let acc = prod + if prod >= 0 { 0x8000 } else { -0x8000 };
    saturate(acc / 0x10000)
}

/// nx+y with product rounded, saturating.
#[inline]
pub fn mult_and_add(n: i32, x: i32, y: i32) -> i32 {
    saturate(mult(n, x) as i128 + y as i128)
}

/// TeX's x_over_y(x, y): round(x/y) as scaled (x scaled, y unitless scalar),
/// rounding half away from zero, saturating.
pub fn x_over_y(x: i32, y: i32) -> i32 {
    if y == 0 {
        return if x >= 0 { i32::MAX } else { i32::MIN };
    }
    let den = (y as i64).abs();
    let mut negative = y < 0;
    let mut xv = x as i64;
    if xv < 0 {
        xv = -xv;
        negative = !negative;
    }
    let q = xv / den;
    let r = xv - q * den;
    let mut res = q;
    if r * 2 >= den {
        res += 1;
    }
    if res > i32::MAX as i64 {
        return if negative { i32::MIN } else { i32::MAX };
    }
    if negative {
        (-res) as i32
    } else {
        res as i32
    }
}

/// TeX's xn_over_d(x, n, d): computes round(x*n/d) where x scaled, n,d integers.
pub fn xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    if d == 0 {
        return i32::MAX;
    }
    let tl = (x as i128) * (n as i128);
    let dd = (d as i128).abs();
    let mut q = tl / dd;
    let rem = tl - q * dd;
    if rem * 2 >= dd {
        q += 1;
    }
    let neg = ((d < 0) != (n < 0)) != (x < 0);
    let v: i128 = if neg { -q } else { q };
    saturate(v)
}

/// round a scaled to nearest integer
#[inline]
pub fn scaled_round(x: i32) -> i32 {
    let v = (x as i64 + if x >= 0 { 0x8000 } else { -0x8000 }) / 0x10000;
    v as i32
}

/// badness(t, s) — tex.web §2337 verbatim: r approximates alpha*t/s with
/// alpha^3 ~ 100*2^18, then rounds r^3/2^18 to nearest; capped at inf_bad.
pub fn badness(t: i32, s: i32) -> i32 {
    if t == 0 {
        return 0;
    }
    if s <= 0 {
        return INF_BAD;
    }
    let (t, s) = (t as i64, s as i64);
    let r: i64 = if t <= 7230584 {
        (t * 297) / s // 297^3 = 99.94 * 2^18
    } else if s >= 1663497 {
        t / (s / 297)
    } else {
        t
    };
    if r > 1290 {
        // 1290^3 < 2^31 < 1291^3
        INF_BAD
    } else {
        ((r * r * r + 131072) / 262144) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mult() {
        assert_eq!(mult(ONE, 10), 10);
        assert_eq!(mult(ONE / 2, 10), 5);
        assert_eq!(mult(-ONE, 10), -10);
    }

    #[test]
    fn test_div() {
        assert_eq!(x_over_y(10 * ONE, 2), 5 * ONE);
        assert_eq!(x_over_y(ONE, 2), ONE / 2);
        assert_eq!(x_over_y(-ONE, 2), -(ONE / 2));
    }

    #[test]
    fn test_badness() {
        assert_eq!(badness(0, ONE), 0);
        assert!((badness(ONE, ONE) - 100).abs() <= 2);
        assert_eq!(badness(ONE, 0), INF_BAD);
        assert!(badness(5 * ONE, ONE) >= INF_BAD);
    }
}
