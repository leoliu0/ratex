//! pdfTeX's random number generator (pdftex.web "random numbers", taken
//! from MetaPost): the additive lagged-Fibonacci scheme of TAOCP 3.6 with
//! the `unif_rand`/`norm_rand` deviates behind `\pdfuniformdeviate` and
//! `\pdfnormaldeviate` (LuaTeX `\uniformdeviate`/`\normaldeviate`).

const FRACTION_HALF: i32 = 1 << 27;
const FRACTION_ONE: i32 = 1 << 28;
const FRACTION_FOUR: i32 = 1 << 30;
const EL_GORDO: i32 = i32::MAX;
const UNITY: i32 = 65536;

#[derive(Clone)]
pub struct Randoms {
    /// pdfTeX `random_seed` (`\pdfrandomseed`).
    pub seed: i32,
    randoms: [i32; 55],
    j_random: usize,
}

impl Randoms {
    /// pdftex.web `init_randoms(seed)`.
    pub fn new(seed: i32) -> Self {
        let mut r = Randoms {
            seed,
            randoms: [0; 55],
            j_random: 0,
        };
        let mut j = seed.unsigned_abs().min(i32::MAX as u32) as i32;
        while j >= FRACTION_ONE {
            j /= 2;
        }
        let mut k = 1;
        for i in 0..55 {
            let jj = k;
            k = j - k;
            j = jj;
            if k < 0 {
                k += FRACTION_ONE;
            }
            r.randoms[(i * 21) % 55] = j;
        }
        r.new_randoms();
        r.new_randoms();
        r.new_randoms();
        r
    }

    /// pdfTeX's job-start seed: `(microseconds*1000)+(epochseconds mod
    /// 1000000)` of the wall clock (SOURCE_DATE_EPOCH does not apply).
    pub fn from_clock() -> Self {
        let (seconds, micros) = crate::clock::now_micros();
        Self::new(micros * 1000 + seconds.rem_euclid(1_000_000) as i32)
    }

    fn new_randoms(&mut self) {
        for k in 0..24 {
            let mut x = self.randoms[k] - self.randoms[k + 31];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.randoms[k] = x;
        }
        for k in 24..55 {
            let mut x = self.randoms[k] - self.randoms[k - 24];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.randoms[k] = x;
        }
        self.j_random = 54;
    }

    fn next_random(&mut self) -> i32 {
        if self.j_random == 0 {
            self.new_randoms();
        } else {
            self.j_random -= 1;
        }
        self.randoms[self.j_random]
    }

    /// pdftex.web `unif_rand(x)`: uniform in `0<=u<x` (or `x<u<=0`).
    pub fn unif_rand(&mut self, x: i32) -> i32 {
        let r = self.next_random();
        let ax = x.saturating_abs();
        let y = take_frac(ax, r);
        if y == ax {
            0
        } else if x > 0 {
            y
        } else {
            -y
        }
    }

    /// pdftex.web `norm_rand`: normal deviate (mean 0, deviation 65536).
    pub fn norm_rand(&mut self) -> i32 {
        loop {
            let (mut x, u) = loop {
                let r = self.next_random();
                let x = take_frac(112429, r - FRACTION_HALF);
                let u = self.next_random();
                if x.abs() < u {
                    break (x, u);
                }
            };
            x = make_frac(x, u);
            let l = 139548960 - m_log(u);
            if ab_vs_cd(1024, l, x, x) >= 0 {
                return x;
            }
        }
    }
}

/// pdftex.web `make_frac(p, q)`: `floor(2^28 p/q + 1/2)`.
fn make_frac(mut p: i32, mut q: i32) -> i32 {
    let mut negative = false;
    if p < 0 {
        p = -p;
        negative = true;
    }
    if q <= 0 {
        q = -q;
        negative = !negative;
    }
    let n = p / q;
    p %= q;
    if n >= 8 {
        return if negative { -EL_GORDO } else { EL_GORDO };
    }
    let n = (n - 1) * FRACTION_ONE;
    let mut f = 1;
    loop {
        let be_careful = p - q;
        p = be_careful + p;
        if p >= 0 {
            f = f + f + 1;
        } else {
            f += f;
            p += q;
        }
        if f >= FRACTION_ONE {
            break;
        }
    }
    if (p - q) + p >= 0 {
        f += 1;
    }
    if negative { -(f + n) } else { f + n }
}

/// pdftex.web `take_frac(q, f)`: `floor(q f/2^28 + 1/2)`.
fn take_frac(mut q: i32, mut f: i32) -> i32 {
    let mut negative = false;
    if f < 0 {
        f = -f;
        negative = true;
    }
    if q < 0 {
        q = -q;
        negative = !negative;
    }
    let mut n;
    if f < FRACTION_ONE {
        n = 0;
    } else {
        n = f / FRACTION_ONE;
        f %= FRACTION_ONE;
        if q <= EL_GORDO / n {
            n *= q;
        } else {
            n = EL_GORDO;
        }
    }
    f += FRACTION_ONE;
    let mut p = FRACTION_HALF;
    if q < FRACTION_FOUR {
        loop {
            p = if f & 1 == 1 { (p + q) / 2 } else { p / 2 };
            f /= 2;
            if f == 1 {
                break;
            }
        }
    } else {
        loop {
            p = if f & 1 == 1 { p + (q - p) / 2 } else { p / 2 };
            f /= 2;
            if f == 1 {
                break;
            }
        }
    }
    if (n - EL_GORDO) + p > 0 {
        n = EL_GORDO - p;
    }
    if negative { -(n + p) } else { n + p }
}

/// pdftex.web `spec_log[1..=28]`.
const SPEC_LOG: [i32; 29] = [
    0, 93032640, 38612034, 17922280, 8662214, 4261238, 2113709, 1052693, 525315, 262400,
    131136, 65552, 32772, 16385, 8192, 4096, 2048, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2,
    1, 1,
];

/// pdftex.web `m_log(x)`: `2^24 ln(x/2^28)` for the positive fractions
/// `norm_rand` passes.
fn m_log(mut x: i32) -> i32 {
    if x <= 0 {
        return 0;
    }
    let mut y: i32 = 1302456956 + 4 - 100;
    let mut z: i32 = 27595 + 6553600;
    while x < FRACTION_FOUR {
        x += x;
        y -= 93032639;
        z -= 48782;
    }
    y += z / UNITY;
    let mut k = 2usize;
    while x > FRACTION_FOUR + 4 {
        let mut z = ((x - 1) / (1 << k)) + 1;
        while x < FRACTION_FOUR + z {
            z = (z + 1) / 2;
            k += 1;
        }
        y += SPEC_LOG[k];
        x -= z;
    }
    y / 8
}

/// pdftex.web `ab_vs_cd`: the sign of `ab - cd`.
fn ab_vs_cd(mut a: i32, mut b: i32, mut c: i32, mut d: i32) -> i32 {
    if a < 0 {
        a = -a;
        b = -b;
    }
    if c < 0 {
        c = -c;
        d = -d;
    }
    if d <= 0 {
        if b >= 0 {
            return if (a == 0 || b == 0) && (c == 0 || d == 0) { 0 } else { 1 };
        }
        if d == 0 {
            return if a == 0 { 0 } else { -1 };
        }
        let q = a;
        a = c;
        c = q;
        let q = -b;
        b = -d;
        d = q;
    } else if b <= 0 {
        if b < 0 && a > 0 {
            return -1;
        }
        return if c == 0 { 0 } else { -1 };
    }
    loop {
        let q = a / d;
        let r = c / b;
        if q != r {
            return if q > r { 1 } else { -1 };
        }
        let q = a % d;
        let r = c % b;
        if r == 0 {
            return if q == 0 { 0 } else { 1 };
        }
        if q == 0 {
            return -1;
        }
        a = b;
        b = q;
        c = d;
        d = r;
    }
}

/// pdftex.web `get_microinterval`: the time since `start` (web2c
/// `epochseconds`, `microseconds`) in scaled seconds, saturating at
/// `max_integer` after 32767 seconds.
pub fn microinterval(start: (i64, i32)) -> i32 {
    elapsed_between(start, crate::clock::now_micros())
}

fn elapsed_between((epoch, micros): (i64, i32), (s, m): (i64, i32)) -> i32 {
    if s - epoch > 32767 {
        i32::MAX
    } else if micros > m {
        (((s - 1 - epoch) * 65536) + ((((m + 1_000_000 - micros) / 100) as i64 * 65536) / 10000))
            as i32
    } else {
        (((s - epoch) * 65536) + ((((m - micros) / 100) as i64 * 65536) / 10000)) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::elapsed_between;
    use super::Randoms;

    /// Sequences printed by TeX Live 2026 pdftex (and identically by
    /// luatex's \uniformdeviate/\normaldeviate) after \pdfsetrandomseed.
    #[test]
    fn deviates_match_pdftex() {
        let mut r = Randoms::new(12345);
        let uniform: Vec<i32> = [1000, 1000, -1000, 0, i32::MAX]
            .iter()
            .map(|&x| r.unif_rand(x))
            .collect();
        assert_eq!(uniform, [709, 377, -201, 0, 196746024]);
        let normal: Vec<i32> = (0..3).map(|_| r.norm_rand()).collect();
        assert_eq!(normal, [-32937, -28583, -18657]);
        let mut r = Randoms::new(7);
        assert_eq!((r.unif_rand(100), r.norm_rand()), (50, -27960));
        let mut r = Randoms::new(i32::MAX);
        assert_eq!((r.unif_rand(65536), r.norm_rand()), (50812, -103556));
        let mut r = Randoms::new(0);
        assert_eq!((r.unif_rand(65536), r.norm_rand()), (15777, 71649));
    }

    /// pdftex.web get_microinterval: scaled seconds with web2c's integer
    /// arithmetic, a microsecond borrow, and saturation past 32767 s.
    #[test]
    fn elapsed_time_is_scaled_seconds() {
        assert_eq!(elapsed_between((100, 0), (101, 500_000)), 65536 + 32768);
        assert_eq!(elapsed_between((100, 900_000), (101, 100_000)), 13107);
        assert_eq!(elapsed_between((0, 0), (32767, 0)), 32767 * 65536);
        assert_eq!(elapsed_between((0, 0), (32768, 0)), i32::MAX);
    }
}
