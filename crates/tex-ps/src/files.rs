//! PostScript files: in-memory sources, decoding filters (ASCIIHex, ASCII85,
//! RunLength, LZW, Flate, SubFile, ReusableStream) and `eexec` decryption.

use std::rc::Rc;

use crate::interp::{err, Frame, Interp, Res};
use crate::lexer::{hex_digit, is_space};
use crate::types::{FileId, PsDict, PsString, Value};

pub(crate) struct PsFile {
    pub(crate) kind: FileKind,
    pub(crate) closed: bool,
    /// Bytes returned to the stream (scanner lookahead, filter over-reads), last first.
    pushback: Vec<u8>,
    /// Opened for executing a string or scanning a token; recyclable once closed.
    pub(crate) temporary: bool,
    /// `ReusableStreamDecode` output: `resetfile` rewinds it.
    rewindable: bool,
}

pub(crate) enum FileKind {
    Bytes { data: Rc<[u8]>, pos: usize },
    Filter(Box<FilterState>),
    Sink,
}

pub(crate) struct FilterState {
    src: Source,
    kind: FilterKind,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
}

enum Source {
    File(FileId),
    Bytes { data: Rc<[u8]>, pos: usize },
    Proc { proc: Value, buf: Vec<u8>, pos: usize, eof: bool },
}

enum FilterKind {
    AsciiHex { high: Option<u8> },
    Ascii85(Ascii85),
    RunLength,
    Lzw(Box<Lzw>),
    Flate(Box<flate2::Decompress>),
    SubFile { eod: Vec<u8>, count: i64, window: Vec<u8>, remaining: Option<i64> },
    Eexec { r: u16, hex: Option<bool>, skip: u8, lookahead: Vec<u8> },
    Dct,
}

impl PsFile {
    pub(crate) fn new(kind: FileKind) -> Self {
        Self { kind, closed: false, pushback: Vec::new(), temporary: false, rewindable: false }
    }

    pub(crate) fn reusable(&self) -> bool {
        self.closed && self.temporary
    }

    pub(crate) fn is_input(&self) -> bool {
        !matches!(self.kind, FileKind::Sink)
    }

    pub(crate) fn is_dct(&self) -> bool {
        matches!(&self.kind, FileKind::Filter(st) if matches!(st.kind, FilterKind::Dct))
    }

    pub(crate) fn bytes_available(&self) -> i64 {
        match &self.kind {
            FileKind::Bytes { data, pos } if !self.closed => (data.len() - pos + self.pushback.len()) as i64,
            _ => -1,
        }
    }

    pub(crate) fn position(&self) -> Option<usize> {
        match &self.kind {
            FileKind::Bytes { pos, .. } => Some(pos - self.pushback.len()),
            _ => None,
        }
    }

    pub(crate) fn seek(&mut self, to: usize) -> bool {
        match &mut self.kind {
            FileKind::Bytes { data, pos } if to <= data.len() => {
                *pos = to;
                self.pushback.clear();
                self.closed = false;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn reset(&mut self) {
        if self.rewindable {
            self.seek(0);
        }
    }
}

pub(crate) enum ReadMode {
    Binary,
    Hex,
    Line,
}

impl Interp {
    #[inline]
    pub(crate) fn getc(&mut self, f: FileId) -> Res<Option<u8>> {
        let file = &mut self.files[f as usize];
        if let Some(b) = file.pushback.pop() {
            return Ok(Some(b));
        }
        if file.closed {
            return Ok(None);
        }
        match &mut file.kind {
            FileKind::Bytes { data, pos } => {
                let b = data.get(*pos).copied();
                *pos += usize::from(b.is_some());
                Ok(b)
            }
            FileKind::Filter(st) if st.pos < st.buf.len() => {
                st.pos += 1;
                Ok(Some(st.buf[st.pos - 1]))
            }
            FileKind::Filter(_) => self.getc_filter(f),
            FileKind::Sink => err("invalidaccess", "reading from an output file"),
        }
    }

    #[inline]
    pub(crate) fn ungetc(&mut self, f: FileId, b: u8) {
        self.files[f as usize].pushback.push(b);
    }

    fn getc_filter(&mut self, f: FileId) -> Res<Option<u8>> {
        loop {
            let FileKind::Filter(st) = &mut self.files[f as usize].kind else { return Ok(None) };
            if st.pos < st.buf.len() {
                st.pos += 1;
                return Ok(Some(st.buf[st.pos - 1]));
            }
            if st.eof {
                return Ok(None);
            }
            let FileKind::Filter(mut st) = std::mem::replace(&mut self.files[f as usize].kind, FileKind::Sink) else {
                unreachable!()
            };
            st.buf.clear();
            st.pos = 0;
            let result = self.refill(&mut st);
            self.files[f as usize].kind = FileKind::Filter(st);
            result?;
        }
    }

    pub(crate) fn close_file(&mut self, f: FileId) {
        let file = &mut self.files[f as usize];
        file.closed = true;
        file.pushback.clear();
        if let FileKind::Filter(st) = &mut file.kind {
            st.buf.clear();
            st.pos = 0;
            st.eof = true;
        }
    }

    /// Reads into `dst`; returns the number of bytes stored and whether the
    /// request completed (string filled, or end of line for `readline`).
    pub(crate) fn read_string(&mut self, f: FileId, dst: &PsString, mode: ReadMode) -> Res<(usize, bool)> {
        let cap = dst.len as usize;
        let mut out = Vec::with_capacity(cap.min(1 << 16));
        let complete = match mode {
            ReadMode::Binary => {
                self.read_bytes(f, cap, &mut out)?;
                out.len() == cap
            }
            ReadMode::Hex => {
                let mut high = None;
                while out.len() < cap {
                    let Some(b) = self.getc(f)? else { break };
                    let Some(d) = hex_digit(b) else { continue };
                    match high.take() {
                        Some(h) => out.push(h << 4 | d),
                        None => high = Some(d),
                    }
                }
                out.len() == cap
            }
            ReadMode::Line => loop {
                match self.getc(f)? {
                    None => break false,
                    Some(b'\n') => break true,
                    Some(b'\r') => {
                        match self.getc(f)? {
                            Some(b'\n') | None => {}
                            Some(c) => self.ungetc(f, c),
                        }
                        break true;
                    }
                    Some(b) => {
                        if out.len() == cap {
                            return err("rangecheck", "readline: line longer than the string");
                        }
                        out.push(b);
                    }
                }
            },
        };
        let at = dst.start as usize;
        dst.data.borrow_mut()[at..at + out.len()].copy_from_slice(&out);
        Ok((out.len(), complete))
    }

    /// Reads up to `max` bytes from a file (bulk path for image data).
    pub(crate) fn read_bytes(&mut self, f: FileId, max: usize, out: &mut Vec<u8>) -> Res {
        let file = &mut self.files[f as usize];
        if let (FileKind::Bytes { data, pos }, true, false) = (&mut file.kind, file.pushback.is_empty(), file.closed) {
            let n = max.min(data.len() - *pos);
            out.extend_from_slice(&data[*pos..*pos + n]);
            *pos += n;
            return Ok(());
        }
        for _ in 0..max {
            match self.getc(f)? {
                Some(b) => out.push(b),
                None => break,
            }
        }
        Ok(())
    }

    /// The raw bytes of a JPEG stream behind a `DCTDecode` filter, up to and
    /// including its EOI marker.
    pub(crate) fn read_dct(&mut self, f: FileId) -> Res<Vec<u8>> {
        let FileKind::Filter(mut st) = std::mem::replace(&mut self.files[f as usize].kind, FileKind::Sink) else {
            return err("typecheck", "not a DCTDecode filter");
        };
        let result = self.read_jpeg(&mut st.src);
        st.eof = true;
        self.files[f as usize].kind = FileKind::Filter(st);
        result
    }

    fn read_jpeg(&mut self, src: &mut Source) -> Res<Vec<u8>> {
        let mut out = Vec::new();
        let mut next = |i: &mut Interp, out: &mut Vec<u8>| -> Res<u8> {
            match i.src_getc(src)? {
                Some(b) => {
                    out.push(b);
                    if out.len() > crate::interp::MAX_OBJECT_LEN * 4 {
                        return err("limitcheck", "JPEG data too large");
                    }
                    Ok(b)
                }
                None => err("ioerror", "truncated JPEG data"),
            }
        };
        if next(self, &mut out)? != 0xFF || next(self, &mut out)? != 0xD8 {
            return err("ioerror", "DCTDecode data is not a JPEG stream");
        }
        loop {
            // Marker segments up to SOS, then entropy-coded data up to EOI.
            let mut marker = next(self, &mut out)?;
            while marker == 0xFF {
                marker = next(self, &mut out)?;
            }
            if out[out.len() - 2] != 0xFF {
                return err("ioerror", "malformed JPEG marker");
            }
            match marker {
                0xD9 => return Ok(out),
                0x01 | 0xD0..=0xD7 => continue,
                _ => {}
            }
            let len = usize::from(next(self, &mut out)?) << 8 | usize::from(next(self, &mut out)?);
            for _ in 2..len {
                next(self, &mut out)?;
            }
            if marker == 0xDA {
                loop {
                    if next(self, &mut out)? != 0xFF {
                        continue;
                    }
                    let mut b = next(self, &mut out)?;
                    while b == 0xFF {
                        b = next(self, &mut out)?;
                    }
                    match b {
                        0x00 | 0xD0..=0xD7 => {}
                        0xD9 => return Ok(out),
                        _ => {
                            // Another segment (e.g. progressive scans): re-enter marker parsing.
                            out.pop();
                            out.pop();
                            out.push(0xFF);
                            out.push(b);
                            let len = usize::from(next(self, &mut out)?) << 8 | usize::from(next(self, &mut out)?);
                            for _ in 2..len {
                                next(self, &mut out)?;
                            }
                        }
                    }
                }
            }
        }
    }

    fn src_getc(&mut self, src: &mut Source) -> Res<Option<u8>> {
        match src {
            Source::File(f) => self.getc(*f),
            Source::Bytes { data, pos } => {
                let b = data.get(*pos).copied();
                *pos += usize::from(b.is_some());
                Ok(b)
            }
            Source::Proc { proc, buf, pos, eof } => loop {
                if *pos < buf.len() {
                    *pos += 1;
                    return Ok(Some(buf[*pos - 1]));
                }
                if *eof {
                    return Ok(None);
                }
                let proc = proc.clone();
                self.call(proc)?;
                match self.pop()? {
                    Value::String(s) | Value::ExecString(s) => {
                        *buf = s.to_vec();
                        *pos = 0;
                        *eof = buf.is_empty();
                    }
                    _ => return err("typecheck", "filter data source must return a string"),
                }
            },
        }
    }

    fn src_read(&mut self, src: &mut Source, max: usize) -> Res<Vec<u8>> {
        let mut out = Vec::with_capacity(max.min(4096));
        if let Source::File(f) = src {
            self.read_bytes(*f, max, &mut out)?;
            return Ok(out);
        }
        while out.len() < max {
            match self.src_getc(src)? {
                Some(b) => out.push(b),
                None => break,
            }
        }
        Ok(out)
    }

    fn src_unread(&mut self, src: &mut Source, bytes: &[u8]) {
        match src {
            Source::File(f) => {
                let file = &mut self.files[*f as usize];
                match (&mut file.kind, file.pushback.is_empty()) {
                    (FileKind::Bytes { pos, .. }, true) if *pos >= bytes.len() => *pos -= bytes.len(),
                    _ => file.pushback.extend(bytes.iter().rev()),
                }
            }
            Source::Bytes { pos, .. } => *pos -= bytes.len(),
            Source::Proc { buf, pos, .. } => {
                let mut rest = bytes.to_vec();
                rest.extend_from_slice(&buf[*pos..]);
                *buf = rest;
                *pos = 0;
            }
        }
    }

    fn refill(&mut self, st: &mut FilterState) -> Res {
        const CHUNK: usize = 4096;
        match &mut st.kind {
            FilterKind::AsciiHex { high } => {
                while st.buf.len() < CHUNK {
                    match self.src_getc(&mut st.src)? {
                        None | Some(b'>') => {
                            st.buf.extend(high.take().map(|h| h << 4));
                            st.eof = true;
                            break;
                        }
                        Some(b) if is_space(b) => {}
                        Some(b) => {
                            let Some(d) = hex_digit(b) else { return err("ioerror", "bad ASCIIHex data") };
                            match high.take() {
                                Some(h) => st.buf.push(h << 4 | d),
                                None => *high = Some(d),
                            }
                        }
                    }
                }
            }
            FilterKind::Ascii85(dec) => {
                while st.buf.len() < CHUNK {
                    match self.src_getc(&mut st.src)? {
                        None => {
                            dec.finish(&mut st.buf)?;
                            st.eof = true;
                            break;
                        }
                        Some(b) => {
                            if dec.push(b, &mut st.buf)? {
                                st.eof = true;
                                break;
                            }
                        }
                    }
                }
            }
            FilterKind::RunLength => match self.src_getc(&mut st.src)? {
                None | Some(128) => st.eof = true,
                Some(n) if n < 128 => {
                    for _ in 0..=n {
                        match self.src_getc(&mut st.src)? {
                            Some(b) => st.buf.push(b),
                            None => {
                                st.eof = true;
                                break;
                            }
                        }
                    }
                }
                Some(n) => match self.src_getc(&mut st.src)? {
                    Some(b) => st.buf.extend(std::iter::repeat_n(b, 257 - usize::from(n))),
                    None => st.eof = true,
                },
            },
            FilterKind::Lzw(lzw) => {
                while st.buf.len() < CHUNK {
                    let Some(code) = lzw.read_code(|| self.src_getc(&mut st.src))? else {
                        st.eof = true;
                        break;
                    };
                    if !lzw.decode(code, &mut st.buf)? {
                        st.eof = true;
                        break;
                    }
                }
            }
            FilterKind::Flate(z) => {
                let input = self.src_read(&mut st.src, CHUNK)?;
                st.buf.reserve(CHUNK * 4);
                let before = z.total_in();
                let status = z
                    .decompress_vec(&input, &mut st.buf, flate2::FlushDecompress::None)
                    .map_err(|e| crate::interp::Flow::Error("ioerror", format!("FlateDecode: {e}")))?;
                let used = (z.total_in() - before) as usize;
                if used < input.len() {
                    self.src_unread(&mut st.src, &input[used..]);
                }
                if status == flate2::Status::StreamEnd || (input.is_empty() && st.buf.is_empty()) {
                    st.eof = true;
                }
            }
            FilterKind::SubFile { eod, count, window, remaining } => {
                if let Some(left) = remaining {
                    let n = (*left).min(CHUNK as i64) as usize;
                    let data = self.src_read(&mut st.src, n)?;
                    *left -= data.len() as i64;
                    if data.len() < n || *left == 0 {
                        st.eof = true;
                    }
                    st.buf.extend_from_slice(&data);
                    return Ok(());
                }
                while st.buf.len() < CHUNK {
                    let Some(b) = self.src_getc(&mut st.src)? else {
                        st.buf.append(window);
                        st.eof = true;
                        break;
                    };
                    if eod.is_empty() {
                        st.buf.push(b);
                        continue;
                    }
                    window.push(b);
                    while !window.is_empty() && !eod.starts_with(window) {
                        st.buf.push(window.remove(0));
                    }
                    if window.len() == eod.len() {
                        if *count == 0 {
                            window.clear();
                            st.eof = true;
                            break;
                        }
                        *count -= 1;
                        st.buf.append(window);
                    }
                }
            }
            FilterKind::Eexec { r, hex, skip, lookahead } => {
                // One byte at a time so `closefile` leaves the source exactly after
                // the consumed ciphertext.
                if hex.is_none() {
                    while lookahead.len() < 4 {
                        match self.src_getc(&mut st.src)? {
                            Some(b) if lookahead.is_empty() && is_space(b) => {}
                            Some(b) => lookahead.push(b),
                            None => break,
                        }
                    }
                    *hex = Some(lookahead.len() == 4 && lookahead.iter().all(|&b| hex_digit(b).is_some()));
                    lookahead.reverse();
                }
                loop {
                    let mut next_raw = |i: &mut Interp| -> Res<Option<u8>> {
                        match lookahead.pop() {
                            Some(b) => Ok(Some(b)),
                            None => i.src_getc(&mut st.src),
                        }
                    };
                    let cipher = if *hex == Some(true) {
                        let mut digits = [0u8; 2];
                        let mut k = 0;
                        while k < 2 {
                            match next_raw(self)? {
                                Some(b) if is_space(b) => {}
                                Some(b) => match hex_digit(b) {
                                    Some(d) => {
                                        digits[k] = d;
                                        k += 1;
                                    }
                                    None => {
                                        // End of the hex ciphertext.
                                        self.src_unread(&mut st.src, &[b]);
                                        st.eof = true;
                                        return Ok(());
                                    }
                                },
                                None => {
                                    st.eof = true;
                                    return Ok(());
                                }
                            }
                        }
                        digits[0] << 4 | digits[1]
                    } else {
                        match next_raw(self)? {
                            Some(b) => b,
                            None => {
                                st.eof = true;
                                return Ok(());
                            }
                        }
                    };
                    let plain = cipher ^ (*r >> 8) as u8;
                    *r = (u16::from(cipher).wrapping_add(*r)).wrapping_mul(52845).wrapping_add(22719);
                    if *skip > 0 {
                        *skip -= 1;
                        continue;
                    }
                    st.buf.push(plain);
                    return Ok(());
                }
            }
            FilterKind::Dct => return err("ioerror", "DCTDecode data can only be used as image data"),
        }
        Ok(())
    }

    fn source_at(&mut self, i: usize) -> Res<Source> {
        self.need(i + 1)?;
        Ok(match self.arg(i).clone() {
            Value::File(f) | Value::ExecFile(f) => Source::File(f),
            Value::String(s) | Value::ExecString(s) => Source::Bytes { data: Rc::from(s.to_vec()), pos: 0 },
            Value::Proc(p) => Source::Proc { proc: Value::Proc(p), buf: Vec::new(), pos: 0, eof: false },
            _ => return err("typecheck", "invalid filter data source"),
        })
    }

    fn new_filter(&mut self, src: Source, kind: FilterKind) -> FileId {
        self.add_file(PsFile::new(FileKind::Filter(Box::new(FilterState {
            src,
            kind,
            buf: Vec::new(),
            pos: 0,
            eof: false,
        }))))
    }

    fn decode_filter(&mut self, name: &[u8], src: Source, params: Option<&PsDict>) -> Res<FileId> {
        let kind = match name {
            b"ASCIIHexDecode" => FilterKind::AsciiHex { high: None },
            b"ASCII85Decode" => FilterKind::Ascii85(Ascii85::default()),
            b"RunLengthDecode" => FilterKind::RunLength,
            b"LZWDecode" => {
                let early = params
                    .and_then(|p| p.get(&self.key_bytes(b"EarlyChange")))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0);
                FilterKind::Lzw(Box::new(Lzw::new(early != 0.0)))
            }
            b"FlateDecode" => FilterKind::Flate(Box::new(flate2::Decompress::new(true))),
            b"DCTDecode" => FilterKind::Dct,
            b"SubFileDecode" => {
                let (count, eod) = match params {
                    Some(p) => (
                        p.get(&Key::Name(self.n.eod_count)).and_then(|v| v.as_f64()).unwrap_or(0.0) as i64,
                        match p.get(&Key::Name(self.n.eod_string)) {
                            Some(Value::String(s)) => s.to_vec(),
                            _ => Vec::new(),
                        },
                    ),
                    None => (0, Vec::new()),
                };
                if count < 0 {
                    return err("rangecheck", "SubFileDecode EODCount");
                }
                let remaining = (eod.is_empty() && count > 0).then_some(count);
                FilterKind::SubFile { eod, count, window: Vec::new(), remaining }
            }
            b"ReusableStreamDecode" => return self.reusable_stream(src, params),
            b"NullEncode" | b"ASCIIHexEncode" | b"ASCII85Encode" | b"RunLengthEncode" | b"LZWEncode"
            | b"FlateEncode" | b"DCTEncode" => return Ok(self.add_file(PsFile::new(FileKind::Sink))),
            _ => return err("undefined", format!("filter /{}", String::from_utf8_lossy(name))),
        };
        Ok(self.new_filter(src, kind))
    }

    fn reusable_stream(&mut self, src: Source, params: Option<&PsDict>) -> Res<FileId> {
        let mut f = match src {
            Source::File(f) => f,
            other => self.new_filter(other, FilterKind::SubFile { eod: Vec::new(), count: 0, window: Vec::new(), remaining: None }),
        };
        if let Some(p) = params {
            let filters = match p.get(&Key::Name(self.n.filter)) {
                Some(Value::Name(n)) => vec![n],
                Some(Value::Array(a)) => a.to_vec().into_iter().filter_map(|v| match v {
                    Value::Name(n) => Some(n),
                    _ => None,
                }).collect(),
                _ => Vec::new(),
            };
            for n in filters {
                let name = self.name_text(n);
                f = self.decode_filter(&name, Source::File(f), None)?;
            }
        }
        let mut data = Vec::new();
        loop {
            let before = data.len();
            self.read_bytes(f, 1 << 16, &mut data)?;
            self.alloc(data.len() - before)?;
            if data.len() == before {
                break;
            }
        }
        let mut file = PsFile::new(FileKind::Bytes { data: Rc::from(data), pos: 0 });
        file.rewindable = true;
        Ok(self.add_file(file))
    }

    pub(crate) fn open_temp(&mut self, data: Rc<[u8]>) -> FileId {
        let mut file = PsFile::new(FileKind::Bytes { data, pos: 0 });
        file.temporary = true;
        self.add_file(file)
    }
}

use crate::types::Key;

/// `src [params] /Name filter` (and `src count string /SubFileDecode filter`).
pub(crate) fn op_filter(i: &mut Interp) -> Res {
    i.need(2)?;
    let name = match i.arg(0) {
        Value::Name(n) | Value::ExecName(n) => i.name_text(*n),
        _ => return err("typecheck", "filter name"),
    };
    let mut used = 1;
    let mut params = None;
    if &*name == b"SubFileDecode" && !matches!(i.arg(1), Value::Dict(_)) {
        let eod = i.string_at(1)?.to_vec();
        let count = i.int_at(2)?;
        let p = PsDict::new();
        p.put(Key::Name(i.n.eod_count), Value::Int(count));
        p.put(Key::Name(i.n.eod_string), Value::String(PsString::new(eod)));
        params = Some(p);
        used = 3;
    } else if let Value::Dict(d) = i.arg(1) {
        params = Some(d.clone());
        used = 2;
    }
    let src = i.source_at(used)?;
    let f = i.decode_filter(&name, src, params.as_ref())?;
    i.pop_n(used + 1);
    i.push(Value::File(f))
}

/// `file eexec`: executes the decrypted remainder of `file` with `systemdict`
/// on the dictionary stack.
pub(crate) fn op_eexec(i: &mut Interp) -> Res {
    let src = i.source_at(0)?;
    i.pop_n(1);
    let f = i.new_filter(src, FilterKind::Eexec { r: 55665, hex: None, skip: 4, lookahead: Vec::new() });
    let systemdict = i.systemdict.clone();
    i.dstack.push(systemdict);
    i.push_frame(Frame::EexecEnd)?;
    i.push_frame(Frame::File(f))
}

/// Streaming ASCII85 decoder.
#[derive(Default)]
pub(crate) struct Ascii85 {
    group: [u8; 5],
    n: usize,
    tilde: bool,
}

impl Ascii85 {
    /// Feeds one character; returns true at the `~>` end marker.
    pub(crate) fn push(&mut self, b: u8, out: &mut Vec<u8>) -> Res<bool> {
        if self.tilde {
            if b == b'>' {
                self.finish(out)?;
                return Ok(true);
            }
            return err("ioerror", "bad ASCII85 end marker");
        }
        match b {
            b'~' => self.tilde = true,
            b'z' if self.n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                self.group[self.n] = b - b'!';
                self.n += 1;
                if self.n == 5 {
                    out.extend_from_slice(&self.value()?.to_be_bytes());
                    self.n = 0;
                }
            }
            b if is_space(b) => {}
            _ => return err("ioerror", "bad ASCII85 data"),
        }
        Ok(false)
    }

    fn value(&self) -> Res<u32> {
        let v = self.group.iter().fold(0u64, |acc, &d| acc * 85 + u64::from(d));
        u32::try_from(v).map_or_else(|_| err("ioerror", "ASCII85 group overflow"), Ok)
    }

    pub(crate) fn finish(&mut self, out: &mut Vec<u8>) -> Res {
        match self.n {
            0 => Ok(()),
            1 => err("ioerror", "truncated ASCII85 group"),
            n => {
                for k in n..5 {
                    self.group[k] = 84;
                }
                let bytes = self.value()?.to_be_bytes();
                out.extend_from_slice(&bytes[..n - 1]);
                self.n = 0;
                Ok(())
            }
        }
    }
}

/// LZW decoder (PostScript/PDF variant, 9–12 bit codes, optional early change).
pub(crate) struct Lzw {
    prefix: Vec<u16>,
    last: Vec<u8>,
    first: Vec<u8>,
    width: u32,
    early: u32,
    prev: Option<u16>,
    bits: u32,
    nbits: u32,
    scratch: Vec<u8>,
}

impl Lzw {
    fn new(early: bool) -> Self {
        let mut lzw = Self {
            prefix: Vec::with_capacity(4096),
            last: Vec::with_capacity(4096),
            first: Vec::with_capacity(4096),
            width: 9,
            early: u32::from(early),
            prev: None,
            bits: 0,
            nbits: 0,
            scratch: Vec::new(),
        };
        lzw.reset();
        lzw
    }

    fn reset(&mut self) {
        self.prefix.clear();
        self.last.clear();
        self.first.clear();
        for b in 0..=255u8 {
            self.prefix.push(u16::MAX);
            self.last.push(b);
            self.first.push(b);
        }
        // 256 = clear table, 257 = end of data.
        for _ in 0..2 {
            self.prefix.push(u16::MAX);
            self.last.push(0);
            self.first.push(0);
        }
        self.width = 9;
        self.prev = None;
    }

    fn read_code(&mut self, mut next: impl FnMut() -> Res<Option<u8>>) -> Res<Option<u16>> {
        while self.nbits < self.width {
            match next()? {
                Some(b) => {
                    self.bits = self.bits << 8 | u32::from(b);
                    self.nbits += 8;
                }
                None => return Ok(None),
            }
        }
        self.nbits -= self.width;
        let code = (self.bits >> self.nbits) & ((1 << self.width) - 1);
        self.bits &= (1 << self.nbits) - 1;
        Ok(Some(code as u16))
    }

    /// Decodes one code; returns false at end of data.
    fn decode(&mut self, code: u16, out: &mut Vec<u8>) -> Res<bool> {
        match code {
            256 => {
                self.reset();
                return Ok(true);
            }
            257 => return Ok(false),
            _ => {}
        }
        let next = self.prefix.len() as u16;
        let Some(prev) = self.prev else {
            if code > 255 {
                return err("ioerror", "bad LZW data");
            }
            out.push(code as u8);
            self.prev = Some(code);
            return Ok(true);
        };
        let first = if code < next {
            self.first[usize::from(code)]
        } else if code == next {
            self.first[usize::from(prev)]
        } else {
            return err("ioerror", "bad LZW code");
        };
        if next < 4096 {
            self.prefix.push(prev);
            self.last.push(first);
            self.first.push(self.first[usize::from(prev)]);
        }
        self.scratch.clear();
        let mut c = code;
        while c != u16::MAX {
            self.scratch.push(self.last[usize::from(c)]);
            c = self.prefix[usize::from(c)];
        }
        out.extend(self.scratch.iter().rev());
        self.prev = Some(code);
        if self.prefix.len() as u32 + self.early >= 1 << self.width && self.width < 12 {
            self.width += 1;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii85_decodes_groups_and_partial_tail() {
        let mut out = Vec::new();
        let mut dec = Ascii85::default();
        for &b in b"87cURD]j7BEbo7~>" {
            if dec.push(b, &mut out).unwrap() {
                break;
            }
        }
        assert_eq!(out, b"Hello world");
    }

    #[test]
    fn lzw_round_trips_the_pdf_spec_example() {
        // PDF Reference example: 45 45 45 45 45 65 45 45 45 66 -> 800B6050220C0C8501
        let data = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        let mut lzw = Lzw::new(true);
        let mut out = Vec::new();
        let mut it = data.iter().copied();
        while let Some(code) = lzw.read_code(|| Ok(it.next())).unwrap() {
            if !lzw.decode(code, &mut out).unwrap() {
                break;
            }
        }
        assert_eq!(out, [45, 45, 45, 45, 45, 65, 45, 45, 45, 66]);
    }
}
