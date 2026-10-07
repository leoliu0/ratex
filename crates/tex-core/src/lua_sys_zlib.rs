//! `zlib` (lzlib 0.4: `version`, `adler32`, `crc32`, `compress`, `decompress`,
//! `compressobj`, `decompressobj`), `gzip` and `zip` (read access to zip
//! archives, LuaZip 1.2.2). The deflate engine is zlib-rs through `flate2`;
//! `windowBits` selects the container as in zlib: negative raw deflate,
//! 8..15 zlib, +16 gzip and (for inflate) +32 automatic detection.
//! `memLevel` and `strategy` are accepted and do not change the output.

use std::cell::RefCell;
use std::fs;

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use tex_lua::{Lua, LuaApi, LuaBytes, LuaString};

use crate::lua_sys::{bytes_of, path_of, strerror, sys_reg};

pub(crate) const PRELUDE: &str = include_str!("lua_sys_zlib.lua");

const Z_OK: i64 = 0;
const Z_STREAM_END: i64 = 1;
const Z_STREAM_ERROR: i64 = -2;
const Z_DATA_ERROR: i64 = -3;
const Z_BUF_ERROR: i64 = -5;

pub(crate) fn adler32(adler: u32, data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (adler & 0xffff, adler >> 16);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn crc32(crc: u32, data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new_with_initial(crc);
    hasher.update(data);
    hasher.finalize()
}

/// Container selected by `windowBits`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrap {
    Raw(u8),
    Zlib(u8),
    Gzip(u8),
    /// inflate only: zlib or gzip by header
    Auto(u8),
}

fn wrap_of(window_bits: i64, deflate: bool) -> Option<Wrap> {
    Some(match window_bits {
        -15..=-8 => Wrap::Raw((-window_bits) as u8),
        8..=15 => Wrap::Zlib(window_bits as u8),
        24..=31 => Wrap::Gzip((window_bits - 16) as u8),
        40..=47 if !deflate => Wrap::Auto((window_bits - 32) as u8),
        0 if !deflate => Wrap::Zlib(15),
        _ => return None,
    })
}

fn level_of(level: i64) -> Option<Compression> {
    match level {
        -1 => Some(Compression::default()),
        0..=9 => Some(Compression::new(level as u32)),
        _ => None,
    }
}

fn new_compress(level: Compression, wrap: Wrap) -> Compress {
    match wrap {
        Wrap::Raw(bits) => Compress::new_with_window_bits(level, false, bits.max(9)),
        Wrap::Zlib(bits) | Wrap::Auto(bits) => Compress::new_with_window_bits(level, true, bits.max(9)),
        Wrap::Gzip(bits) => Compress::new_gzip(level, bits.max(9)),
    }
}

fn new_decompress(wrap: Wrap, first: &[u8]) -> Decompress {
    match wrap {
        Wrap::Raw(bits) => Decompress::new_with_window_bits(false, bits.max(9)),
        Wrap::Zlib(bits) => Decompress::new_with_window_bits(true, bits.max(9)),
        Wrap::Gzip(bits) => Decompress::new_gzip(bits.max(9)),
        Wrap::Auto(bits) => {
            if first.starts_with(&[0x1f, 0x8b]) {
                Decompress::new_gzip(bits.max(9))
            } else {
                Decompress::new_with_window_bits(true, bits.max(9))
            }
        }
    }
}

/// Compress `data` completely (`deflate(..., Z_FINISH)`).
fn deflate_all(data: &[u8], level: Compression, wrap: Wrap) -> Result<Vec<u8>, i64> {
    let mut stream = new_compress(level, wrap);
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    loop {
        let consumed = stream.total_in() as usize;
        match stream.compress_vec(&data[consumed..], &mut out, FlushCompress::Finish) {
            Ok(Status::StreamEnd) => return Ok(out),
            Ok(_) => out.reserve(out.capacity().max(4096)),
            Err(_) => return Err(Z_STREAM_ERROR),
        }
    }
}

/// Inflate as much of `data` as is valid: the output and the zlib code
/// (`Z_STREAM_END`, `Z_BUF_ERROR` for truncated input, `Z_DATA_ERROR`).
fn inflate_all(data: &[u8], wrap: Wrap) -> (Vec<u8>, i64) {
    let mut stream = new_decompress(wrap, data);
    let mut out = Vec::with_capacity(data.len().saturating_mul(3).max(256));
    loop {
        let consumed = stream.total_in() as usize;
        let produced = out.len();
        match stream.decompress_vec(&data[consumed..], &mut out, FlushDecompress::None) {
            Ok(Status::StreamEnd) => return (out, Z_STREAM_END),
            Ok(Status::Ok) | Ok(Status::BufError) => {
                let progressed = stream.total_in() as usize > consumed || out.len() > produced;
                if !progressed {
                    if out.len() == out.capacity() {
                        out.reserve(out.capacity().max(4096));
                        continue;
                    }
                    return (out, Z_BUF_ERROR);
                }
                if out.len() == out.capacity() {
                    out.reserve(out.capacity().max(4096));
                }
            }
            Err(_) => return (out, Z_DATA_ERROR),
        }
    }
}

/// A `compressobj`/`decompressobj` stream.
enum Stream {
    Deflate { stream: Compress, wrap: Wrap, check: u32 },
    Inflate { stream: Option<Decompress>, wrap: Wrap, check: u32, started: bool },
}

thread_local! {
    static STREAMS: RefCell<Vec<Option<Stream>>> = const { RefCell::new(Vec::new()) };
    static ZIPS: RefCell<Vec<Option<ZipArchive>>> = const { RefCell::new(Vec::new()) };
}

fn update_check(wrap: Wrap, check: u32, data: &[u8]) -> u32 {
    match wrap {
        Wrap::Gzip(_) => crc32(check, data),
        Wrap::Raw(_) => check,
        _ => adler32(check, data),
    }
}

fn initial_check(wrap: Wrap) -> u32 {
    match wrap {
        Wrap::Gzip(_) => 0,
        _ => 1,
    }
}

fn with_stream<R>(id: i64, f: impl FnOnce(&mut Stream) -> Result<R, String>) -> Result<R, String> {
    STREAMS.with(|s| {
        let mut s = s.borrow_mut();
        match s.get_mut(id as usize).and_then(Option::as_mut) {
            Some(stream) => f(stream),
            None => Err("attempt to use invalid zlib stream".to_string()),
        }
    })
}

// -------------------------------------------------------------------- zip ---

struct ZipEntry {
    name: Vec<u8>,
    method: u16,
    compressed: u64,
    size: u64,
    header_offset: u64,
}

struct ZipArchive {
    data: Vec<u8>,
    entries: Vec<ZipEntry>,
}

fn le16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn le32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn le64(d: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(at..at + 8)?.try_into().ok()?))
}

/// The central directory of a zip archive (including zip64 records).
fn parse_zip(data: Vec<u8>) -> Option<ZipArchive> {
    let eocd = (0..data.len().saturating_sub(21))
        .rev()
        .take(65_557)
        .find(|&i| data[i..].starts_with(b"PK\x05\x06"))?;
    let mut count = u64::from(le16(&data, eocd + 10)?);
    let mut offset = u64::from(le32(&data, eocd + 16)?);
    if (count == 0xffff || offset == 0xffff_ffff) && eocd >= 20 && data[eocd - 20..].starts_with(b"PK\x06\x07") {
        let rec = le64(&data, eocd - 12)? as usize;
        if data.get(rec..)?.starts_with(b"PK\x06\x06") {
            count = le64(&data, rec + 32)?;
            offset = le64(&data, rec + 48)?;
        }
    }
    let mut entries = Vec::new();
    let mut at = offset as usize;
    for _ in 0..count {
        if !data.get(at..)?.starts_with(b"PK\x01\x02") {
            return None;
        }
        let method = le16(&data, at + 10)?;
        let mut compressed = u64::from(le32(&data, at + 20)?);
        let mut size = u64::from(le32(&data, at + 24)?);
        let name_len = le16(&data, at + 28)? as usize;
        let extra_len = le16(&data, at + 30)? as usize;
        let comment_len = le16(&data, at + 32)? as usize;
        let mut header_offset = u64::from(le32(&data, at + 42)?);
        let name = data.get(at + 46..at + 46 + name_len)?.to_vec();
        let extra = data.get(at + 46 + name_len..at + 46 + name_len + extra_len)?;
        // zip64 extended information
        let mut e = 0;
        while e + 4 <= extra.len() {
            let id = le16(extra, e)?;
            let len = le16(extra, e + 2)? as usize;
            if id == 1 {
                let mut field = e + 4;
                if size == 0xffff_ffff {
                    size = le64(extra, field)?;
                    field += 8;
                }
                if compressed == 0xffff_ffff {
                    compressed = le64(extra, field)?;
                    field += 8;
                }
                if header_offset == 0xffff_ffff {
                    header_offset = le64(extra, field)?;
                }
            }
            e += 4 + len;
        }
        entries.push(ZipEntry { name, method, compressed, size, header_offset });
        at += 46 + name_len + extra_len + comment_len;
    }
    Some(ZipArchive { data, entries })
}

impl ZipArchive {
    fn read(&self, entry: &ZipEntry) -> Option<Vec<u8>> {
        let at = entry.header_offset as usize;
        if !self.data.get(at..)?.starts_with(b"PK\x03\x04") {
            return None;
        }
        let name_len = le16(&self.data, at + 26)? as usize;
        let extra_len = le16(&self.data, at + 28)? as usize;
        let start = at + 30 + name_len + extra_len;
        let raw = self.data.get(start..start + entry.compressed as usize)?;
        match entry.method {
            0 => Some(raw.to_vec()),
            8 => {
                let (out, code) = inflate_all(raw, Wrap::Raw(15));
                (code == Z_STREAM_END && out.len() as u64 == entry.size).then_some(out)
            }
            _ => None,
        }
    }
}

pub(crate) fn register(lua: &mut Lua, s: &tex_lua::LuaTable) -> Result<(), String> {
    sys_reg!(lua, s, "zlib_adler32", |adler: i64, data: LuaString| -> i64 {
        i64::from(adler32(adler as u32, &bytes_of(&data)))
    });
    sys_reg!(lua, s, "zlib_crc32", |crc: i64, data: LuaString| -> i64 {
        i64::from(crc32(crc as u32, &bytes_of(&data)))
    });
    sys_reg!(
        lua,
        s,
        "zlib_compress",
        |data: LuaString, level: i64, window_bits: i64| -> (Option<LuaBytes>, i64) {
            let (Some(level), Some(wrap)) = (level_of(level), wrap_of(window_bits, true)) else {
                return (None, Z_STREAM_ERROR);
            };
            match deflate_all(&bytes_of(&data), level, wrap) {
                Ok(out) => (Some(LuaBytes(out)), Z_STREAM_END),
                Err(code) => (None, code),
            }
        }
    );
    sys_reg!(lua, s, "zlib_decompress", |data: LuaString, window_bits: i64| -> (Option<LuaBytes>, i64) {
        let Some(wrap) = wrap_of(window_bits, false) else {
            return (None, Z_STREAM_ERROR);
        };
        let (out, code) = inflate_all(&bytes_of(&data), wrap);
        (Some(LuaBytes(out)), code)
    });
    // one gzip member at the start of `data`: (contents, code, bytes consumed)
    sys_reg!(lua, s, "gzip_member", |data: LuaString| -> (LuaBytes, i64, i64) {
        let data = bytes_of(&data);
        let mut stream = Decompress::new_gzip(15);
        let mut out = Vec::with_capacity(data.len().saturating_mul(3).max(256));
        loop {
            let consumed = stream.total_in() as usize;
            let produced = out.len();
            match stream.decompress_vec(&data[consumed..], &mut out, FlushDecompress::None) {
                Ok(Status::StreamEnd) => return (LuaBytes(out), Z_STREAM_END, stream.total_in() as i64),
                Ok(_) => {
                    if stream.total_in() as usize == consumed && out.len() == produced {
                        if out.len() == out.capacity() {
                            out.reserve(4096);
                            continue;
                        }
                        return (LuaBytes(out), Z_BUF_ERROR, stream.total_in() as i64);
                    }
                    if out.len() == out.capacity() {
                        out.reserve(out.capacity().max(4096));
                    }
                }
                Err(_) => return (LuaBytes(out), Z_DATA_ERROR, stream.total_in() as i64),
            }
        }
    });
    sys_reg!(lua, s, "zlib_new_deflate", |level: i64, window_bits: i64| -> Result<i64, String> {
        let (Some(level), Some(wrap)) = (level_of(level), wrap_of(window_bits, true)) else {
            return Err("failed to start decompressing".to_string());
        };
        Ok(STREAMS.with(|s| {
            let mut s = s.borrow_mut();
            s.push(Some(Stream::Deflate { stream: new_compress(level, wrap), wrap, check: initial_check(wrap) }));
            s.len() as i64 - 1
        }))
    });
    sys_reg!(lua, s, "zlib_new_inflate", |window_bits: i64| -> Result<i64, String> {
        let Some(wrap) = wrap_of(window_bits, false) else {
            return Err("failed to start compressing".to_string());
        };
        let stream = match wrap {
            Wrap::Auto(_) => None,
            _ => Some(new_decompress(wrap, &[])),
        };
        Ok(STREAMS.with(|s| {
            let mut s = s.borrow_mut();
            s.push(Some(Stream::Inflate {
                stream,
                wrap,
                check: initial_check(wrap),
                started: false,
            }));
            s.len() as i64 - 1
        }))
    });
    // deflate: kind 0 = Z_NO_FLUSH with `data`, 1 = Z_FINISH
    sys_reg!(lua, s, "zlib_stream_compress", |id: i64, data: LuaString, finish: bool| -> Result<LuaBytes, String> {
        let data = bytes_of(&data);
        with_stream(id, |stream| {
            let Stream::Deflate { stream, wrap, check } = stream else {
                return Err("attempt to use invalid zlib stream".to_string());
            };
            *check = update_check(*wrap, *check, &data);
            let flush = if finish { FlushCompress::Finish } else { FlushCompress::None };
            let mut out = Vec::new();
            let base = stream.total_in();
            loop {
                let consumed = (stream.total_in() - base) as usize;
                out.reserve(data.len() / 2 + 256);
                match stream.compress_vec(&data[consumed..], &mut out, flush) {
                    Ok(Status::StreamEnd) => break,
                    Ok(_) => {
                        let done = (stream.total_in() - base) as usize >= data.len();
                        if done && (!finish) && out.len() < out.capacity() {
                            break;
                        }
                    }
                    Err(_) => return Err(format!("failed to compress [{}]", Z_STREAM_ERROR)),
                }
            }
            Ok(LuaBytes(out))
        })
    });
    sys_reg!(lua, s, "zlib_stream_decompress", |id: i64, data: LuaString| -> Result<LuaBytes, String> {
        let data = bytes_of(&data);
        with_stream(id, |stream| {
            let Stream::Inflate { stream, wrap, check, started, .. } = stream else {
                return Err("attempt to use invalid zlib stream".to_string());
            };
            if stream.is_none() {
                *stream = Some(new_decompress(*wrap, &data));
            }
            *started = true;
            let inflater = stream.as_mut().expect("inflate stream");
            let mut out = Vec::new();
            let base = inflater.total_in();
            loop {
                let consumed = (inflater.total_in() - base) as usize;
                let produced = out.len();
                out.reserve(data.len().max(256) * 2);
                match inflater.decompress_vec(&data[consumed..], &mut out, FlushDecompress::Sync) {
                    Ok(Status::StreamEnd) => break,
                    Ok(_) => {
                        let used_all = (inflater.total_in() - base) as usize >= data.len();
                        if used_all && out.len() < out.capacity() {
                            break;
                        }
                        if !used_all && out.len() == produced && (inflater.total_in() - base) as usize == consumed {
                            return Err(format!("failed to decompress [{}]", Z_BUF_ERROR));
                        }
                    }
                    Err(_) => return Err(format!("failed to decompress [{}]", Z_DATA_ERROR)),
                }
            }
            *check = update_check(*wrap, *check, &out);
            Ok(LuaBytes(out))
        })
    });
    sys_reg!(lua, s, "zlib_stream_reset", |id: i64| -> Result<i64, String> {
        with_stream(id, |stream| {
            match stream {
                Stream::Deflate { stream, wrap, check } => {
                    stream.reset();
                    *check = initial_check(*wrap);
                }
                Stream::Inflate { stream, wrap, check, started, .. } => {
                    match stream {
                        Some(inflater) => inflater.reset(!matches!(wrap, Wrap::Raw(_))),
                        None => {}
                    }
                    *check = initial_check(*wrap);
                    *started = false;
                }
            }
            Ok(Z_OK)
        })
    });
    sys_reg!(lua, s, "zlib_stream_adler", |id: i64| -> Result<i64, String> {
        with_stream(id, |stream| match stream {
            Stream::Deflate { check, .. } | Stream::Inflate { check, .. } => Ok(i64::from(*check)),
        })
    });
    sys_reg!(lua, s, "zlib_stream_kind", |id: i64| -> Option<String> {
        STREAMS.with(|s| match s.borrow().get(id as usize).and_then(Option::as_ref) {
            Some(Stream::Deflate { .. }) => Some("deflate".to_string()),
            Some(Stream::Inflate { .. }) => Some("inflate".to_string()),
            None => None,
        })
    });
    sys_reg!(lua, s, "zlib_stream_close", |id: i64| {
        STREAMS.with(|s| {
            if let Some(slot) = s.borrow_mut().get_mut(id as usize) {
                *slot = None;
            }
        });
    });

    // zip archives (read only)
    sys_reg!(lua, s, "zip_open", |path: LuaString| -> (Option<i64>, Option<LuaBytes>) {
        let bytes = bytes_of(&path);
        let read = fs::read(path_of(&bytes));
        let _ = crate::lua_bridge::with_engine(|e| e.lua_dep_read(&path_of(&bytes), read.is_ok()));
        let data = match read {
            Ok(data) => data,
            Err(e) => return (None, Some(LuaBytes(format!("{}: {}", String::from_utf8_lossy(&bytes), strerror(&e)).into_bytes()))),
        };
        match parse_zip(data) {
            Some(archive) => (
                Some(ZIPS.with(|z| {
                    let mut z = z.borrow_mut();
                    z.push(Some(archive));
                    z.len() as i64 - 1
                })),
                None,
            ),
            None => (None, Some(LuaBytes(format!("{}: not a zip archive", String::from_utf8_lossy(&bytes)).into_bytes()))),
        }
    });
    sys_reg!(lua, s, "zip_names", |id: i64| -> Vec<LuaBytes> {
        ZIPS.with(|z| {
            z.borrow()
                .get(id as usize)
                .and_then(Option::as_ref)
                .map(|a| a.entries.iter().map(|e| LuaBytes(e.name.clone())).collect())
                .unwrap_or_default()
        })
    });
    sys_reg!(lua, s, "zip_info", |id: i64| -> Vec<i64> {
        ZIPS.with(|z| {
            z.borrow()
                .get(id as usize)
                .and_then(Option::as_ref)
                .map(|a| {
                    a.entries
                        .iter()
                        .flat_map(|e| [e.compressed as i64, i64::from(e.method), e.size as i64])
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    sys_reg!(lua, s, "zip_read", |id: i64, name: LuaString| -> Option<LuaBytes> {
        let name = bytes_of(&name);
        ZIPS.with(|z| {
            let z = z.borrow();
            let archive = z.get(id as usize)?.as_ref()?;
            let entry = archive.entries.iter().find(|e| e.name == name)?;
            archive.read(entry).map(LuaBytes)
        })
    });
    sys_reg!(lua, s, "zip_close", |id: i64| {
        ZIPS.with(|z| {
            if let Some(slot) = z.borrow_mut().get_mut(id as usize) {
                *slot = None;
            }
        });
    });
    Ok(())
}
