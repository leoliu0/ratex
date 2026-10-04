//! `mime` (LuaSocket 3 `mime.core` plus `mime.lua`) and `ltn12`.
//!
//! `mime.core` is the set of incremental transfer-encoding filters
//! (`b64`, `unb64`, `qp`, `unqp`, `qpwrp`, `wrp`, `eol`, `dot`); `mime.lua`
//! and `ltn12.lua` are LuaSocket's own Lua files, run unchanged.

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString};

use crate::lua_sys::{bytes_of, sys_reg};

pub(crate) const PRELUDE: &str = include_str!("lua_sys_mime.lua");

const CRLF: &[u8] = b"\r\n";
const EQCRLF: &[u8] = b"=\r\n";
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const QPBASE: &[u8; 16] = b"0123456789ABCDEF";

fn b64_unbase(c: u8) -> u8 {
    match c {
        b'=' => 0,
        _ => B64.iter().position(|&b| b == c).map_or(255, |i| i as u8),
    }
}

fn qp_unbase(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'A'..=b'F' => c - b'A' + 10,
        b'a'..=b'f' => c - b'a' + 10,
        _ => 255,
    }
}

#[derive(PartialEq)]
enum QpClass {
    Plain,
    Quoted,
    Cr,
    IfLast,
}

fn qp_class(c: u8) -> QpClass {
    match c {
        33..=60 | 62..=126 => QpClass::Plain,
        b'\t' | b' ' => QpClass::IfLast,
        b'\r' => QpClass::Cr,
        _ => QpClass::Quoted,
    }
}

fn qp_quote(out: &mut Vec<u8>, c: u8) {
    out.extend_from_slice(&[b'=', QPBASE[(c >> 4) as usize], QPBASE[(c & 15) as usize]]);
}

fn b64_encode_all(atom: &mut Vec<u8>, input: &[u8], out: &mut Vec<u8>) {
    for &c in input {
        atom.push(c);
        if atom.len() == 3 {
            let value = u32::from(atom[0]) << 16 | u32::from(atom[1]) << 8 | u32::from(atom[2]);
            out.extend_from_slice(&[
                B64[(value >> 18) as usize & 0x3f],
                B64[(value >> 12) as usize & 0x3f],
                B64[(value >> 6) as usize & 0x3f],
                B64[value as usize & 0x3f],
            ]);
            atom.clear();
        }
    }
}

fn b64_pad(atom: &[u8], out: &mut Vec<u8>) {
    match atom.len() {
        1 => {
            let value = u32::from(atom[0]) << 4;
            out.extend_from_slice(&[B64[(value >> 6) as usize & 0x3f], B64[value as usize & 0x3f], b'=', b'=']);
        }
        2 => {
            let value = (u32::from(atom[0]) << 8 | u32::from(atom[1])) << 2;
            out.extend_from_slice(&[
                B64[(value >> 12) as usize & 0x3f],
                B64[(value >> 6) as usize & 0x3f],
                B64[value as usize & 0x3f],
                b'=',
            ]);
        }
        _ => {}
    }
}

fn b64_decode_all(atom: &mut Vec<u8>, input: &[u8], out: &mut Vec<u8>) {
    for &c in input {
        if b64_unbase(c) > 64 {
            continue;
        }
        atom.push(c);
        if atom.len() == 4 {
            let mut value = 0u32;
            for &a in atom.iter() {
                value = value << 6 | u32::from(b64_unbase(a));
            }
            let decoded = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
            let valid = if atom[2] == b'=' {
                1
            } else if atom[3] == b'=' {
                2
            } else {
                3
            };
            out.extend_from_slice(&decoded[..valid]);
            atom.clear();
        }
    }
}

fn qp_encode_all(atom: &mut Vec<u8>, input: &[u8], marker: &[u8], out: &mut Vec<u8>) {
    for &c in input {
        atom.push(c);
        while !atom.is_empty() {
            match qp_class(atom[0]) {
                QpClass::Cr => {
                    if atom.len() < 2 {
                        break;
                    }
                    if atom[1] == b'\n' {
                        out.extend_from_slice(marker);
                        atom.clear();
                        break;
                    }
                    qp_quote(out, atom[0]);
                }
                QpClass::IfLast => {
                    if atom.len() < 3 {
                        break;
                    }
                    if atom[1] == b'\r' && atom[2] == b'\n' {
                        qp_quote(out, atom[0]);
                        out.extend_from_slice(marker);
                        atom.clear();
                        break;
                    }
                    out.push(atom[0]);
                }
                QpClass::Quoted => qp_quote(out, atom[0]),
                QpClass::Plain => out.push(atom[0]),
            }
            atom.remove(0);
        }
    }
}

fn qp_pad(atom: &[u8], out: &mut Vec<u8>) {
    for &c in atom {
        if qp_class(c) == QpClass::Plain {
            out.push(c);
        } else {
            qp_quote(out, c);
        }
    }
    if !atom.is_empty() {
        out.extend_from_slice(EQCRLF);
    }
}

fn qp_decode_all(atom: &mut Vec<u8>, input: &[u8], out: &mut Vec<u8>) {
    for &c in input {
        atom.push(c);
        match atom[0] {
            b'=' => {
                if atom.len() < 3 {
                    continue;
                }
                if !(atom[1] == b'\r' && atom[2] == b'\n') {
                    let (hi, lo) = (qp_unbase(atom[1]), qp_unbase(atom[2]));
                    if hi > 15 || lo > 15 {
                        out.extend_from_slice(&atom[..3]);
                    } else {
                        out.push((hi << 4) + lo);
                    }
                }
                atom.clear();
            }
            b'\r' => {
                if atom.len() < 2 {
                    continue;
                }
                if atom[1] == b'\n' {
                    out.extend_from_slice(&atom[..2]);
                }
                atom.clear();
            }
            first => {
                if first == b'\t' || (first > 31 && first < 127) {
                    out.push(first);
                }
                atom.clear();
            }
        }
    }
}

/// `(A, B) = filter(C, D)` for the encoders that keep an `atom`.
fn two_part(
    first: Option<LuaString>,
    second: Option<LuaString>,
    encode: impl Fn(&mut Vec<u8>, &[u8], &mut Vec<u8>),
    pad: impl Fn(&[u8], &mut Vec<u8>),
) -> (Option<LuaBytes>, Option<LuaBytes>) {
    let Some(first) = first else { return (None, None) };
    let mut atom = Vec::new();
    let mut out = Vec::new();
    encode(&mut atom, &bytes_of(&first), &mut out);
    match second {
        None => {
            pad(&atom, &mut out);
            let out = (!out.is_empty()).then_some(LuaBytes(out));
            (out, None)
        }
        Some(second) => {
            encode(&mut atom, &bytes_of(&second), &mut out);
            (Some(LuaBytes(out)), Some(LuaBytes(atom)))
        }
    }
}

pub(crate) fn register(lua: &mut Lua, s: &tex_lua::LuaTable) -> Result<(), String> {
    lua.set_global("__texres_ltn12_source", include_str!("../assets/ltn12.lua")).map_err(|e| format!("{e:?}"))?;
    lua.set_global("__texres_mime_source", include_str!("../assets/mime.lua")).map_err(|e| format!("{e:?}"))?;
    sys_reg!(lua, s, "mime_b64", |a: Option<LuaString>, b: Option<LuaString>| -> (Option<LuaBytes>, Option<LuaBytes>) {
        two_part(a, b, b64_encode_all, b64_pad)
    });
    sys_reg!(lua, s, "mime_unb64", |a: Option<LuaString>, b: Option<LuaString>| -> (Option<LuaBytes>, Option<LuaBytes>) {
        two_part(a, b, b64_decode_all, |_, _| {})
    });
    sys_reg!(
        lua,
        s,
        "mime_qp",
        |a: Option<LuaString>, b: Option<LuaString>, marker: Option<LuaString>| -> (Option<LuaBytes>, Option<LuaBytes>) {
            let marker = marker.map_or_else(|| CRLF.to_vec(), |m| bytes_of(&m));
            two_part(a, b, |atom, input, out| qp_encode_all(atom, input, &marker, out), qp_pad)
        }
    );
    sys_reg!(lua, s, "mime_unqp", |a: Option<LuaString>, b: Option<LuaString>| -> (Option<LuaBytes>, Option<LuaBytes>) {
        two_part(a, b, qp_decode_all, |_, _| {})
    });
    // line wrapping: (A, n) = wrp(left, B, length)
    sys_reg!(
        lua,
        s,
        "mime_wrp",
        |left: f64, input: Option<LuaString>, length: Option<f64>| -> (Option<LuaBytes>, f64) {
            let length = length.unwrap_or(76.0) as i32;
            let mut left = left as i32;
            let Some(input) = input else {
                return if left < length { (Some(LuaBytes(CRLF.to_vec())), f64::from(length)) } else { (None, f64::from(length)) };
            };
            let mut out = Vec::new();
            for &c in bytes_of(&input).iter() {
                match c {
                    b'\r' => {}
                    b'\n' => {
                        out.extend_from_slice(CRLF);
                        left = length;
                    }
                    _ => {
                        if left <= 0 {
                            left = length;
                            out.extend_from_slice(CRLF);
                        }
                        out.push(c);
                        left -= 1;
                    }
                }
            }
            (Some(LuaBytes(out)), f64::from(left))
        }
    );
    sys_reg!(
        lua,
        s,
        "mime_qpwrp",
        |left: f64, input: Option<LuaString>, length: Option<f64>| -> (Option<LuaBytes>, f64) {
            let length = length.unwrap_or(76.0) as i32;
            let mut left = left as i32;
            let Some(input) = input else {
                return if left < length { (Some(LuaBytes(EQCRLF.to_vec())), f64::from(length)) } else { (None, f64::from(length)) };
            };
            let mut out = Vec::new();
            for &c in bytes_of(&input).iter() {
                match c {
                    b'\r' => {}
                    b'\n' => {
                        left = length;
                        out.extend_from_slice(CRLF);
                    }
                    b'=' => {
                        if left <= 3 {
                            left = length;
                            out.extend_from_slice(EQCRLF);
                        }
                        out.push(c);
                        left -= 1;
                    }
                    _ => {
                        if left <= 1 {
                            left = length;
                            out.extend_from_slice(EQCRLF);
                        }
                        out.push(c);
                        left -= 1;
                    }
                }
            }
            (Some(LuaBytes(out)), f64::from(left))
        }
    );
    sys_reg!(
        lua,
        s,
        "mime_eol",
        |ctx: i64, input: Option<LuaString>, marker: Option<LuaString>| -> (Option<LuaBytes>, f64) {
            let marker = marker.map_or_else(|| CRLF.to_vec(), |m| bytes_of(&m));
            let Some(input) = input else { return (None, 0.0) };
            let mut ctx = ctx as i32;
            let mut out = Vec::new();
            let candidate = |c: i32| c == i32::from(b'\r') || c == i32::from(b'\n');
            for &c in bytes_of(&input).iter() {
                let c = i32::from(c);
                if candidate(c) {
                    if candidate(ctx) {
                        if c == ctx {
                            out.extend_from_slice(&marker);
                        }
                        ctx = 0;
                    } else {
                        out.extend_from_slice(&marker);
                        ctx = c;
                    }
                } else {
                    out.push(c as u8);
                    ctx = 0;
                }
            }
            (Some(LuaBytes(out)), f64::from(ctx))
        }
    );
    sys_reg!(lua, s, "mime_dot", |state: f64, input: Option<LuaString>| -> (Option<LuaBytes>, f64) {
        let Some(input) = input else { return (None, 2.0) };
        let mut state = state as usize;
        let mut out = Vec::new();
        for &c in bytes_of(&input).iter() {
            out.push(c);
            state = match c {
                b'\r' => 1,
                b'\n' => {
                    if state == 1 {
                        2
                    } else {
                        0
                    }
                }
                b'.' => {
                    if state == 2 {
                        out.push(b'.');
                    }
                    0
                }
                _ => 0,
            };
        }
        (Some(LuaBytes(out)), state as f64)
    });
    Ok(())
}
