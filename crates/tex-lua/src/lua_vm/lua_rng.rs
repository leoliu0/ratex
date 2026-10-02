/// xoshiro256** RNG matching C Lua's implementation exactly
#[derive(Debug, Clone)]
pub struct LuaRng {
    pub state: [u64; 4],
}

impl LuaRng {
    /// Seed from two integers, matching C Lua's setseed
    pub fn from_seed(n1: i64, n2: i64) -> Self {
        let mut rng = LuaRng {
            state: [n1 as u64, 0xff, n2 as u64, 0],
        };
        // Warm up: discard 16 values to spread the seed
        for _ in 0..16 {
            rng.next_rand();
        }
        rng
    }

    /// Seed from a time value (for default initialization)
    pub fn from_seed_time(time: u64) -> Self {
        Self::from_seed(time as i64, 0)
    }

    /// Generate next random u64 using xoshiro256**
    pub fn next_rand(&mut self) -> u64 {
        let s = &mut self.state;
        let s0 = s[0];
        let s1 = s[1];
        let s2 = s[2] ^ s0;
        let s3 = s[3] ^ s1;
        // result = s1 * 5, rotate left 7, then * 9
        let res = s1.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        s[0] = s0 ^ s3;
        s[1] = s1 ^ s2;
        s[2] = s2 ^ (s1 << 17);
        s[3] = s3.rotate_left(45);
        res
    }

    /// `I2d`: the float in [0, 1) of a drawn value, from its top 53 bits (DBL_MANT_DIG).
    pub fn to_float(rv: u64) -> f64 {
        let mantissa = rv >> (64 - 53); // = rv >> 11
        (mantissa as f64) * f64::from_bits(0x3CA0000000000000) // 2^-53
    }
}

/// glibc `random()`/`srandom()` (TYPE_3, degree 31, separation 3): what Lua
/// 5.3's `math.random` uses on POSIX (`l_rand`/`l_srand`), so that a seeded
/// sequence equals LuaTeX's.
#[derive(Debug, Clone)]
pub struct LibcRandom {
    r: [i32; 34],
    f: usize,
    b: usize,
}

impl LibcRandom {
    /// `srandom(seed)`.
    pub fn from_seed(seed: u32) -> Self {
        let seed = if seed == 0 { 1 } else { seed };
        let mut r = [0i32; 34];
        r[0] = seed as i32;
        let mut word = i64::from(seed as i32);
        for slot in r.iter_mut().take(31).skip(1) {
            let hi = word / 127_773;
            let lo = word % 127_773;
            word = 16807 * lo - 2836 * hi;
            if word < 0 {
                word += 2_147_483_647;
            }
            *slot = word as i32;
        }
        let mut rng = LibcRandom { r, f: 3, b: 0 };
        for _ in 0..310 {
            rng.next_rand();
        }
        rng
    }

    /// `random()`: an integer in `[0, 2^31 - 1]`.
    pub fn next_rand(&mut self) -> i64 {
        let sum = self.r[self.f].wrapping_add(self.r[self.b]);
        self.r[self.f] = sum;
        let result = (sum as u32 >> 1) as i64;
        self.f = if self.f + 1 == 31 { 0 } else { self.f + 1 };
        self.b = if self.b + 1 == 31 { 0 } else { self.b + 1 };
        result
    }
}
