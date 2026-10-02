//! `md5` (`sum`, `exor`, `crypt`, `decrypt`: Roberto Ierusalimschy's md5
//! library as in LuaTeX) and `sha2` (`digest256`, `digest384`, `digest512`).
//! All results are raw byte strings.

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString};

use crate::lua_sys::{bytes_of, sys_reg};

pub(crate) const PRELUDE: &str = include_str!("lua_sys_hash.lua");

fn md5_sum(data: &[u8]) -> [u8; 16] {
    md5::compute(data).0
}

const MAX_KEY: usize = 256;
const BLOCK: usize = 16;

/// `initblock`: the 16 seed bytes (zero padded) followed by the key.
fn init_block(seed: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() > MAX_KEY {
        return Err(format!("key too long (> {MAX_KEY})"));
    }
    let mut block = vec![0u8; BLOCK];
    block[..seed.len()].copy_from_slice(seed);
    block.extend_from_slice(key);
    Ok(block)
}

fn crypt(message: &[u8], key: &[u8], seed: &[u8]) -> Result<Vec<u8>, String> {
    if seed.len() > BLOCK {
        return Err(format!("seed too long (> {BLOCK})"));
    }
    let mut out = vec![seed.len() as u8];
    out.extend_from_slice(seed);
    let mut block = init_block(seed, key)?;
    for chunk in message.chunks(BLOCK) {
        let mut code = md5_sum(&block);
        for (c, m) in code.iter_mut().zip(chunk) {
            *c ^= m;
        }
        out.extend_from_slice(&code[..chunk.len()]);
        block[..chunk.len()].copy_from_slice(&code[..chunk.len()]);
    }
    Ok(out)
}

fn decrypt(cipher: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
    // `size_t lseed = cyphertext[0]` with a signed `char`: bytes >= 0x80
    // become huge and fail the length check.
    let lseed = cipher.first().map_or(0usize, |&b| if b >= 0x80 { usize::MAX } else { b as usize });
    if cipher.is_empty() || lseed > BLOCK || cipher.len() < lseed.saturating_add(1) {
        return Err("bad argument #1 to 'decrypt' (invalid cyphered string)".to_string());
    }
    let seed = &cipher[1..=lseed];
    let body = &cipher[lseed + 1..];
    let mut block = init_block(seed, key)?;
    let mut out = Vec::with_capacity(body.len());
    for chunk in body.chunks(BLOCK) {
        let mut code = md5_sum(&block);
        for (c, m) in code.iter_mut().zip(chunk) {
            *c ^= m;
        }
        out.extend_from_slice(&code[..chunk.len()]);
        block[..chunk.len()].copy_from_slice(chunk);
    }
    Ok(out)
}

// ---------------------------------------------------------------- SHA-2 ---

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const K512: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

/// Merkle-Damgard padding for a block size of `block` bytes with a length
/// field of `len_bytes` bytes.
fn pad(data: &[u8], block: usize, len_bytes: usize) -> Vec<u8> {
    let mut m = data.to_vec();
    m.push(0x80);
    while m.len() % block != block - len_bytes {
        m.push(0);
    }
    let bits = (data.len() as u128) * 8;
    m.extend_from_slice(&bits.to_be_bytes()[16 - len_bytes..]);
    m
}

fn sha256(data: &[u8]) -> Vec<u8> {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    for chunk in pad(data, 64, 8).chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K256[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    h.iter().flat_map(|x| x.to_be_bytes()).collect()
}

fn sha512_with(data: &[u8], init: [u64; 8], out_len: usize) -> Vec<u8> {
    let mut h = init;
    for chunk in pad(data, 128, 16).chunks(128) {
        let mut w = [0u64; 80];
        for (i, word) in chunk.chunks(8).enumerate() {
            w[i] = u64::from_be_bytes(word.try_into().expect("8-byte word"));
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..80 {
            let s1 = v[4].rotate_right(14) ^ v[4].rotate_right(18) ^ v[4].rotate_right(41);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K512[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(28) ^ v[0].rotate_right(34) ^ v[0].rotate_right(39);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    let mut out: Vec<u8> = h.iter().flat_map(|x| x.to_be_bytes()).collect();
    out.truncate(out_len);
    out
}

fn sha384(data: &[u8]) -> Vec<u8> {
    sha512_with(
        data,
        [
            0xcbbb9d5dc1059ed8, 0x629a292a367cd507, 0x9159015a3070dd17, 0x152fecd8f70e5939,
            0x67332667ffc00b31, 0x8eb44a8768581511, 0xdb0c2e0d64f98fa7, 0x47b5481dbefa4fa4,
        ],
        48,
    )
}

fn sha512(data: &[u8]) -> Vec<u8> {
    sha512_with(
        data,
        [
            0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
            0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
        ],
        64,
    )
}

pub(crate) fn register(lua: &mut Lua, s: &tex_lua::LuaTable) -> Result<(), String> {
    sys_reg!(lua, s, "md5_sum", |data: LuaString| -> LuaBytes { LuaBytes(md5_sum(&bytes_of(&data)).to_vec()) });
    sys_reg!(lua, s, "md5_crypt", |msg: LuaString, key: LuaString, seed: LuaString| -> Result<LuaBytes, String> {
        crypt(&bytes_of(&msg), &bytes_of(&key), &bytes_of(&seed)).map(LuaBytes)
    });
    sys_reg!(lua, s, "md5_decrypt", |cipher: LuaString, key: LuaString| -> Result<LuaBytes, String> {
        decrypt(&bytes_of(&cipher), &bytes_of(&key)).map(LuaBytes)
    });
    sys_reg!(lua, s, "sha2_256", |data: LuaString| -> LuaBytes { LuaBytes(sha256(&bytes_of(&data))) });
    sys_reg!(lua, s, "sha2_384", |data: LuaString| -> LuaBytes { LuaBytes(sha384(&bytes_of(&data))) });
    sys_reg!(lua, s, "sha2_512", |data: LuaString| -> LuaBytes { LuaBytes(sha512(&bytes_of(&data))) });
    Ok(())
}
