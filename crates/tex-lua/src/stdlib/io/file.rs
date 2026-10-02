// File handle stream: a small port of the parts of C stdio that liolib.c
// relies on (one buffer shared by reads and writes, ungetc, fseek/ftell
// that account for buffered data, setvbuf, and pclose exit statuses).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

use crate::lua_value::userdata_trait::UserDataTrait;

/// Default buffer size for regular files (glibc's BUFSIZ is 8 KiB; a larger
/// buffer cuts the syscall count for line-oriented reads).
const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BufMode {
    No,
    Full,
    Line,
}

enum Stream {
    File(File),
    Stdin,
    Stdout,
    Stderr,
    #[cfg(not(target_arch = "wasm32"))]
    PipeRead(std::process::Child, std::process::ChildStdout),
    #[cfg(not(target_arch = "wasm32"))]
    PipeWrite(std::process::Child, std::process::ChildStdin),
}

/// How a successfully closed stream terminated.
pub(crate) enum CloseStatus {
    File,
    /// A `popen` stream: ("exit" | "signal", code).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Process(&'static str, i32),
}

pub struct LuaFile {
    /// `None` once the file is closed (C Lua's `closef == NULL`).
    stream: Option<Stream>,
    readable: bool,
    writable: bool,
    /// Read buffer; bytes `rbuf[rpos..]` have been read from the stream but
    /// not yet consumed.
    rbuf: Vec<u8>,
    rpos: usize,
    /// Bytes written but not yet handed to the stream.
    wbuf: Vec<u8>,
    mode: BufMode,
    bufsize: usize,
    /// Reusable buffer for building read results.
    scratch: Vec<u8>,
    /// Text-mode translation of a `popen` read pipe (the C runtime of
    /// Windows turns CR LF into LF there).
    #[cfg(windows)]
    text: CrlfDecoder,
}

/// CR LF to LF, as the C runtime does for streams in text mode. A CR at
/// the end of a chunk is held back until the next chunk shows whether an
/// LF follows.
#[cfg(any(windows, test))]
#[derive(Default)]
struct CrlfDecoder {
    held_cr: bool,
    /// Decoded bytes not yet handed out.
    ready: std::collections::VecDeque<u8>,
}

#[cfg(any(windows, test))]
impl CrlfDecoder {
    fn push(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if self.held_cr {
                self.held_cr = false;
                if byte != b'\n' {
                    self.ready.push_back(b'\r');
                }
            }
            if byte == b'\r' {
                self.held_cr = true;
            } else {
                self.ready.push_back(byte);
            }
        }
    }

    /// End of input: a held CR was a lone CR.
    fn finish(&mut self) {
        if std::mem::take(&mut self.held_cr) {
            self.ready.push_back(b'\r');
        }
    }

    fn take(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.ready.len());
        for (slot, byte) in buf.iter_mut().zip(self.ready.drain(..n)) {
            *slot = byte;
        }
        n
    }
}

/// An error carrying a C `errno` and its `strerror` text, for platforms
/// whose OS error numbers are not the C library's (and for conditions
/// that never reach the OS).
#[cfg(not(unix))]
#[derive(Debug)]
struct CError(i32, &'static str);

#[cfg(not(unix))]
impl std::fmt::Display for CError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.1)
    }
}

#[cfg(not(unix))]
impl std::error::Error for CError {}

#[cfg(not(unix))]
fn c_error(kind: io::ErrorKind, code: i32, text: &'static str) -> io::Error {
    io::Error::new(kind, CError(code, text))
}

/// The C `errno` and `strerror` text of an error that is not a plain OS
/// error of this platform's C library (Windows' native error numbers are
/// not `errno` values).
#[cfg(not(unix))]
fn c_errno(error: &io::Error) -> Option<(i32, &'static str)> {
    use io::ErrorKind as K;
    if let Some(custom) = error.get_ref().and_then(|inner| inner.downcast_ref::<CError>()) {
        return Some((custom.0, custom.1));
    }
    Some(match error.kind() {
        K::NotFound => (2, "No such file or directory"),
        K::PermissionDenied => (13, "Permission denied"),
        K::AlreadyExists => (17, "File exists"),
        K::NotADirectory => (20, "Not a directory"),
        K::IsADirectory => (21, "Is a directory"),
        K::InvalidInput => (22, "Invalid argument"),
        K::StorageFull => (28, "No space left on device"),
        K::BrokenPipe => (32, "Broken pipe"),
        K::DirectoryNotEmpty => (41, "Directory not empty"),
        _ => return None,
    })
}

fn ebadf() -> io::Error {
    #[cfg(unix)]
    {
        io::Error::from_raw_os_error(libc::EBADF)
    }
    #[cfg(not(unix))]
    {
        c_error(io::ErrorKind::Other, 9, "Bad file descriptor")
    }
}

fn espipe() -> io::Error {
    #[cfg(unix)]
    {
        io::Error::from_raw_os_error(libc::ESPIPE)
    }
    #[cfg(not(unix))]
    {
        c_error(io::ErrorKind::Other, 29, "Illegal seek")
    }
}

/// `EINVAL`.
pub(crate) fn einval() -> io::Error {
    #[cfg(unix)]
    {
        io::Error::from_raw_os_error(libc::EINVAL)
    }
    #[cfg(not(unix))]
    {
        c_error(io::ErrorKind::InvalidInput, 22, "Invalid argument")
    }
}

/// The text C's `strerror` gives for an I/O error (Rust appends
/// " (os error N)" to OS errors; Lua messages do not have it).
pub(crate) fn error_message(error: &io::Error) -> String {
    #[cfg(not(unix))]
    if let Some((_, text)) = c_errno(error) {
        return text.to_owned();
    }
    let text = error.to_string();
    match error.raw_os_error() {
        Some(code) => {
            let suffix = format!(" (os error {code})");
            text.strip_suffix(suffix.as_str()).map_or(text.clone(), str::to_owned)
        }
        None => text,
    }
}

/// The C `errno` of an I/O error (`luaL_fileresult`'s third result).
pub(crate) fn error_code(error: &io::Error) -> i64 {
    #[cfg(not(unix))]
    if let Some((code, _)) = c_errno(error) {
        return code as i64;
    }
    error.raw_os_error().unwrap_or(0) as i64
}

/// `fopen` mode check of liolib.c: `[rwa]%+?b*`.
pub(crate) fn valid_open_mode(mode: &[u8]) -> bool {
    let Some((&first, mut rest)) = mode.split_first() else {
        return false;
    };
    if !matches!(first, b'r' | b'w' | b'a') {
        return false;
    }
    if rest.first() == Some(&b'+') {
        rest = &rest[1..];
    }
    rest.iter().all(|&c| c == b'b')
}

impl LuaFile {
    fn with_stream(stream: Stream, readable: bool, writable: bool, mode: BufMode) -> Self {
        LuaFile {
            stream: Some(stream),
            readable,
            writable,
            rbuf: Vec::new(),
            rpos: 0,
            wbuf: Vec::new(),
            mode,
            bufsize: BUFFER_SIZE,
            scratch: Vec::new(),
            #[cfg(windows)]
            text: CrlfDecoder::default(),
        }
    }

    // Rust's own stdout/stderr handles already buffer (stdout per line), and
    // other Rust code in the process writes through them too, so the standard
    // streams get no second buffer here unless a script asks for one.
    pub fn stdin() -> Self {
        Self::with_stream(Stream::Stdin, true, false, BufMode::Full)
    }

    pub fn stdout() -> Self {
        Self::with_stream(Stream::Stdout, false, true, BufMode::No)
    }

    pub fn stderr() -> Self {
        Self::with_stream(Stream::Stderr, false, true, BufMode::No)
    }

    /// The operating-system file descriptor behind the stream (`fileno`):
    /// 0, 1 and 2 for the standard streams, the pipe end of a `popen` stream.
    /// `None` once the file is closed, and on platforms without descriptors.
    pub fn raw_fd(&self) -> Option<i32> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            Some(match self.stream.as_ref()? {
                Stream::File(file) => file.as_raw_fd(),
                Stream::Stdin => 0,
                Stream::Stdout => 1,
                Stream::Stderr => 2,
                Stream::PipeRead(_, pipe) => pipe.as_raw_fd(),
                Stream::PipeWrite(_, pipe) => pipe.as_raw_fd(),
            })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// `fopen(path, mode)`; `mode` must satisfy [`valid_open_mode`].
    pub(crate) fn open(path: &[u8], mode: &[u8]) -> io::Result<Self> {
        let plus = mode.get(1) == Some(&b'+');
        let mut options = std::fs::OpenOptions::new();
        let (readable, writable) = match mode[0] {
            b'r' => {
                options.read(true).write(plus);
                (true, plus)
            }
            b'w' => {
                options.write(true).read(plus).create(true).truncate(true);
                (plus, true)
            }
            _ => {
                options.append(true).read(plus).create(true);
                (plus, true)
            }
        };
        #[cfg(unix)]
        let path = std::path::Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path));
        #[cfg(not(unix))]
        let path = String::from_utf8_lossy(path).into_owned();
        let file = options.open(path)?;
        Ok(Self::with_stream(Stream::File(file), readable, writable, BufMode::Full))
    }

    /// `tmpfile()`: an anonymous read/write file removed when closed.
    pub(crate) fn tmpfile() -> io::Result<Self> {
        let dir = std::env::temp_dir();
        let mut attempt = 0u32;
        loop {
            let path = dir.join(format!(
                "lua_{}_{}_{}",
                std::process::id(),
                crate::platform_time::unix_nanos(),
                attempt
            ));
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    let _ = std::fs::remove_file(&path);
                    return Ok(Self::with_stream(Stream::File(file), true, true, BufMode::Full));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// `popen(command, mode)` with mode "r" or "w".
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn popen(command: &str, write: bool) -> io::Result<Self> {
        use std::process::{Command, Stdio};
        // C Lua flushes every stream before forking (`fflush(NULL)`).
        let _ = io::stdout().flush();
        #[cfg(windows)]
        let mut cmd = {
            let mut cmd = Command::new("cmd");
            cmd.args(["/C", command]);
            cmd
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg(command);
            cmd
        };
        if write {
            let mut child = cmd.stdin(Stdio::piped()).spawn()?;
            let stdin = child.stdin.take().expect("piped stdin");
            Ok(Self::with_stream(Stream::PipeWrite(child, stdin), false, true, BufMode::Full))
        } else {
            let mut child = cmd.stdout(Stdio::piped()).spawn()?;
            let stdout = child.stdout.take().expect("piped stdout");
            Ok(Self::with_stream(Stream::PipeRead(child, stdout), true, false, BufMode::Full))
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn popen(_command: &str, _write: bool) -> io::Result<Self> {
        Err(io::Error::other("'popen' not supported"))
    }

    /// Borrow the reusable result buffer (give it back with `put_scratch`).
    pub(crate) fn take_scratch(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.scratch)
    }

    pub(crate) fn put_scratch(&mut self, mut buffer: Vec<u8>) {
        if buffer.capacity() <= BUFFER_SIZE {
            buffer.clear();
            self.scratch = buffer;
        }
    }

    #[inline]
    pub fn is_closed(&self) -> bool {
        self.stream.is_none()
    }

    pub fn is_std_stream(&self) -> bool {
        matches!(
            self.stream,
            Some(Stream::Stdin | Stream::Stdout | Stream::Stderr)
        )
    }

    /// Address identifying the open stream (`%p` of the `FILE*`).
    pub(crate) fn stream_addr(&self) -> *const () {
        match &self.stream {
            Some(stream) => stream as *const Stream as *const (),
            None => std::ptr::null(),
        }
    }

    fn raw_read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let result = match self.stream.as_mut() {
                Some(Stream::File(file)) => file.read(buf),
                Some(Stream::Stdin) => io::stdin().lock().read(buf),
                #[cfg(not(target_arch = "wasm32"))]
                Some(Stream::PipeRead(_, pipe)) => {
                    #[cfg(windows)]
                    {
                        let mut chunk = [0u8; 4096];
                        loop {
                            let n = self.text.take(buf);
                            if n > 0 || buf.is_empty() {
                                return Ok(n);
                            }
                            match pipe.read(&mut chunk) {
                                Ok(0) => {
                                    self.text.finish();
                                    if self.text.ready.is_empty() {
                                        return Ok(0);
                                    }
                                }
                                Ok(read) => self.text.push(&chunk[..read]),
                                Err(error) => return Err(error),
                            }
                        }
                    }
                    #[cfg(not(windows))]
                    pipe.read(buf)
                }
                _ => Err(ebadf()),
            };
            match result {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                other => return other,
            }
        }
    }

    fn raw_write(&mut self, data: &[u8]) -> io::Result<()> {
        match self.stream.as_mut() {
            Some(Stream::File(file)) => file.write_all(data),
            Some(Stream::Stdout) => io::stdout().write_all(data),
            Some(Stream::Stderr) => io::stderr().write_all(data),
            #[cfg(not(target_arch = "wasm32"))]
            Some(Stream::PipeWrite(_, pipe)) => pipe.write_all(data),
            _ => Err(ebadf()),
        }
    }

    fn raw_seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self.stream.as_mut() {
            Some(Stream::File(file)) => file.seek(pos),
            #[cfg(unix)]
            Some(stream @ (Stream::Stdin | Stream::Stdout | Stream::Stderr)) => {
                let fd = match stream {
                    Stream::Stdin => 0,
                    Stream::Stdout => {
                        io::stdout().flush()?;
                        1
                    }
                    _ => 2,
                };
                let (offset, whence) = match pos {
                    SeekFrom::Start(offset) => (offset as i64, libc::SEEK_SET),
                    SeekFrom::Current(offset) => (offset, libc::SEEK_CUR),
                    SeekFrom::End(offset) => (offset, libc::SEEK_END),
                };
                let result = unsafe { libc::lseek(fd, offset as libc::off_t, whence) };
                if result < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(result as u64)
                }
            }
            Some(_) => Err(espipe()),
            None => Err(ebadf()),
        }
    }

    /// Give back unconsumed read-ahead so the OS position matches the
    /// logical one (needed before writing or seeking on a read buffer).
    fn drop_read_buffer(&mut self) -> io::Result<()> {
        let unread = self.rbuf.len() - self.rpos;
        self.rbuf.clear();
        self.rpos = 0;
        if unread > 0 {
            self.raw_seek(SeekFrom::Current(-(unread as i64)))?;
        }
        Ok(())
    }

    fn flush_write_buffer(&mut self) -> io::Result<()> {
        if self.wbuf.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.wbuf);
        let result = self.raw_write(&data);
        self.wbuf = data;
        self.wbuf.clear();
        result
    }

    /// Refill the read buffer; returns false at end of file.
    fn fill(&mut self) -> io::Result<bool> {
        if !self.readable {
            return Err(ebadf());
        }
        self.flush_write_buffer()?;
        let mut buf = std::mem::take(&mut self.rbuf);
        buf.clear();
        buf.resize(self.bufsize.max(1), 0);
        let result = self.raw_read(&mut buf);
        let n = *result.as_ref().unwrap_or(&0);
        buf.truncate(n);
        self.rbuf = buf;
        self.rpos = 0;
        result.map(|n| n > 0)
    }

    /// `getc`: next byte, or `None` at end of file.
    #[inline]
    pub(crate) fn getc(&mut self) -> io::Result<Option<u8>> {
        if self.rpos == self.rbuf.len() && !self.fill()? {
            return Ok(None);
        }
        let c = self.rbuf[self.rpos];
        self.rpos += 1;
        Ok(Some(c))
    }

    /// `ungetc` of the byte just returned by [`getc`](Self::getc).
    #[inline]
    pub(crate) fn ungetc(&mut self) {
        debug_assert!(self.rpos > 0);
        self.rpos -= 1;
    }

    /// Append the next line to `out`; returns whether a '\n' ended it.
    /// The '\n' itself is appended only when `keep_newline` is set.
    pub(crate) fn read_line(&mut self, out: &mut Vec<u8>, keep_newline: bool) -> io::Result<bool> {
        loop {
            if self.rpos == self.rbuf.len() && !self.fill()? {
                return Ok(false);
            }
            let available = &self.rbuf[self.rpos..];
            if let Some(i) = available.iter().position(|&c| c == b'\n') {
                out.extend_from_slice(&available[..i + usize::from(keep_newline)]);
                self.rpos += i + 1;
                return Ok(true);
            }
            out.extend_from_slice(available);
            self.rpos = self.rbuf.len();
        }
    }

    /// `fread` of up to `n` bytes, appended to `out`.
    pub(crate) fn read_chars(&mut self, out: &mut Vec<u8>, n: usize) -> io::Result<()> {
        let mut remaining = n;
        while remaining > 0 {
            if self.rpos == self.rbuf.len() {
                if remaining >= self.bufsize {
                    // Large request: read straight into the result.
                    if !self.readable {
                        return Err(ebadf());
                    }
                    self.flush_write_buffer()?;
                    let start = out.len();
                    out.resize(start + remaining.min(1 << 20), 0);
                    let result = self.raw_read(&mut out[start..]);
                    let got = *result.as_ref().unwrap_or(&0);
                    out.truncate(start + got);
                    if result? == 0 {
                        return Ok(());
                    }
                    remaining -= got;
                    continue;
                }
                if !self.fill()? {
                    return Ok(());
                }
            }
            let take = remaining.min(self.rbuf.len() - self.rpos);
            out.extend_from_slice(&self.rbuf[self.rpos..self.rpos + take]);
            self.rpos += take;
            remaining -= take;
        }
        Ok(())
    }

    /// Read everything up to end of file, appended to `out`.
    pub(crate) fn read_all(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        out.extend_from_slice(&self.rbuf[self.rpos..]);
        self.rpos = self.rbuf.len();
        if !self.readable {
            return Err(ebadf());
        }
        self.flush_write_buffer()?;
        if let Some(Stream::File(file)) = &self.stream
            && let (Ok(meta), Ok(pos)) = (file.metadata(), (&*file).stream_position())
        {
            out.reserve(meta.len().saturating_sub(pos) as usize);
        }
        loop {
            let start = out.len();
            out.resize(start + self.bufsize.max(1), 0);
            let result = self.raw_read(&mut out[start..]);
            let got = *result.as_ref().unwrap_or(&0);
            out.truncate(start + got);
            if result? == 0 {
                return Ok(());
            }
        }
    }

    /// `fwrite` of `data`.
    pub(crate) fn write(&mut self, data: &[u8]) -> io::Result<()> {
        if !self.writable {
            return Err(ebadf());
        }
        if self.rpos < self.rbuf.len() {
            self.drop_read_buffer()?;
        } else {
            self.rbuf.clear();
            self.rpos = 0;
        }
        match self.mode {
            BufMode::No => {
                self.flush_write_buffer()?;
                self.raw_write(data)
            }
            BufMode::Full => {
                if self.wbuf.len() + data.len() > self.bufsize {
                    self.flush_write_buffer()?;
                    if data.len() >= self.bufsize {
                        return self.raw_write(data);
                    }
                }
                self.wbuf.extend_from_slice(data);
                Ok(())
            }
            BufMode::Line => {
                self.wbuf.extend_from_slice(data);
                if data.contains(&b'\n') || self.wbuf.len() >= self.bufsize {
                    self.flush_write_buffer()?;
                }
                Ok(())
            }
        }
    }

    /// `fflush`.
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.flush_write_buffer()?;
        match self.stream.as_mut() {
            Some(Stream::Stdout) => io::stdout().flush(),
            Some(Stream::Stderr) => io::stderr().flush(),
            #[cfg(not(target_arch = "wasm32"))]
            Some(Stream::PipeWrite(_, pipe)) => pipe.flush(),
            _ => Ok(()),
        }
    }

    /// `fseek` followed by `ftell`.
    pub(crate) fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.flush_write_buffer()?;
        let unread = (self.rbuf.len() - self.rpos) as i64;
        let pos = match pos {
            SeekFrom::Current(offset) => SeekFrom::Current(offset - unread),
            other => other,
        };
        let result = self.raw_seek(pos)?;
        self.rbuf.clear();
        self.rpos = 0;
        Ok(result)
    }

    /// `setvbuf`.
    pub(crate) fn setvbuf(&mut self, mode: BufMode, size: usize) -> io::Result<()> {
        self.flush_write_buffer()?;
        self.mode = mode;
        self.bufsize = size.clamp(1, 1 << 24);
        Ok(())
    }

    /// `fclose` / `pclose`.
    pub(crate) fn close(&mut self) -> io::Result<CloseStatus> {
        let flushed = self.flush_write_buffer();
        self.rbuf = Vec::new();
        self.rpos = 0;
        self.wbuf = Vec::new();
        match self.stream.take() {
            #[cfg(not(target_arch = "wasm32"))]
            Some(Stream::PipeRead(mut child, pipe)) => {
                drop(pipe);
                exit_status(child.wait()?)
            }
            #[cfg(not(target_arch = "wasm32"))]
            Some(Stream::PipeWrite(mut child, pipe)) => {
                drop(pipe);
                exit_status(child.wait()?)
            }
            _ => flushed.map(|()| CloseStatus::File),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn exit_status(status: std::process::ExitStatus) -> io::Result<CloseStatus> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return Ok(CloseStatus::Process("signal", signal));
        }
    }
    Ok(CloseStatus::Process("exit", status.code().unwrap_or(-1)))
}

impl Drop for LuaFile {
    fn drop(&mut self) {
        if !self.is_closed() && !self.is_std_stream() {
            let _ = self.close();
        } else {
            let _ = self.flush_write_buffer();
        }
    }
}

// The Lua-visible API lives in the shared FILE* metatable, so only the
// identity part of the trait is implemented.
impl UserDataTrait for LuaFile {
    fn type_name(&self) -> &'static str {
        "FILE*"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::CrlfDecoder;

    fn decode(chunks: &[&[u8]]) -> Vec<u8> {
        let mut decoder = CrlfDecoder::default();
        for chunk in chunks {
            decoder.push(chunk);
        }
        decoder.finish();
        let mut out = vec![0; decoder.ready.len()];
        let n = decoder.take(&mut out);
        out.truncate(n);
        out
    }

    #[test]
    fn crlf_becomes_lf_even_across_chunks_and_lone_cr_stays() {
        assert_eq!(decode(&[b"a\r\nb\r\n"]), b"a\nb\n");
        assert_eq!(decode(&[b"a\r", b"\nb"]), b"a\nb");
        assert_eq!(decode(&[b"a\r", b"b\r"]), b"a\rb\r");
        assert_eq!(decode(&[b"\r\r\n"]), b"\r\n");
    }
}
