//! LWP::Protocol::ftp 6.76 over Net::FTP 3.15: a GET on a fresh control
//! connection (Biber's agent has no connection cache, and never sends QUIT).
//! `Err` is the status line of the response LWP returns.
use super::{http::{host_port, latin1, status_line}, net::{self, ConnectError}, percent_decode, uri};
use std::{collections::VecDeque, env, io::{self, Read, Write}, net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs}, time::{Duration, Instant}};

const PACKAGE: &str = "LWP::Protocol::MyFTP";
/// Net::Cmd::DEF_REPLY_CODE.
const DEFAULT_CODE: u16 = 421;
const BLOCK_SIZE: usize = 10240;

struct Control {
    stream: TcpStream,
    partial: Vec<u8>,
    lines: VecDeque<Vec<u8>>,
    closed: bool,
    code: u16,
    message: Vec<Vec<u8>>,
}

impl Control {
    fn set_status(&mut self, code: u16, message: Option<String>) {
        self.code = code;
        self.message = message.map(String::into_bytes).into_iter().collect();
    }

    fn set_closed(&mut self) {
        self.closed = true;
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        self.set_status(DEFAULT_CODE, Some(format!("[{PACKAGE}] Connection closed")));
    }

    /// Net::Cmd::command: words joined by spaces, newlines become spaces.
    fn command(&mut self, words: &[&[u8]]) {
        if self.closed { self.set_closed(); return; }
        let mut line = words.join(&b' ').iter().map(|&b| if b == b'\n' { b' ' } else { b }).collect::<Vec<_>>();
        line.extend_from_slice(b"\r\n");
        match self.stream.write_all(&line) {
            Ok(()) => {}
            Err(e) if net::is_timeout(&e) => self.set_status(DEFAULT_CODE, Some(format!("[{PACKAGE}] Timeout"))),
            Err(_) => self.set_closed(),
        }
    }

    /// Net::Cmd::getline: lines split at CRLF or LF, each ending in "\n". A
    /// final line without a line end is lost when the server closes.
    fn getline(&mut self) -> Option<Vec<u8>> {
        if let Some(line) = self.lines.pop_front() { return Some(line); }
        if self.closed { self.set_closed(); return None; }
        while self.lines.is_empty() {
            let mut chunk = [0; 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => { self.set_closed(); return None; }
                Ok(n) => {
                    self.partial.extend_from_slice(&chunk[..n]);
                    while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
                        let mut line = self.partial.drain(..=end).collect::<Vec<_>>();
                        line.pop();
                        if line.last() == Some(&b'\r') { line.pop(); }
                        line.push(b'\n');
                        self.lines.push_back(line);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if net::is_timeout(&e) => { self.set_status(DEFAULT_CODE, Some(format!("[{PACKAGE}] Timeout"))); return None; }
                Err(_) => { self.set_closed(); return None; }
            }
        }
        self.lines.pop_front()
    }

    /// Net::FTP::response: the first digit of the reply code, 5 on failure.
    fn response(&mut self) -> u8 {
        self.set_status(DEFAULT_CODE, None);
        loop {
            let Some(mut line) = self.getline() else { return 5 };
            // Net::FTP::parse_response: a line without a code continues the reply.
            let coded = line.len() >= 3 && line[..3].iter().all(u8::is_ascii_digit);
            let more = if coded {
                self.code = line[..3].iter().fold(0, |n, &d| n * 10 + u16::from(d - b'0'));
                let separator = line.get(3).copied().filter(|b| matches!(b, b'-' | b' '));
                line.drain(..3 + usize::from(separator.is_some()));
                separator == Some(b'-')
            } else if self.code == 0 {
                self.lines.push_front(line);
                return 5;
            } else { true };
            self.message.push(line);
            if !more { break; }
        }
        match self.code / 100 { 0 => 5, digit => digit as u8 }
    }

    fn cmd(&mut self, words: &[&[u8]]) -> u8 { self.command(words); self.response() }

    fn message(&self) -> String { latin1(&self.message.concat()) }

    /// Net::FTP::cwd: blank names change to "/", ".." uses CDUP.
    fn cwd(&mut self, directory: &[u8]) -> bool {
        let directory: &[u8] = if directory.iter().all(u8::is_ascii_whitespace) { b"/" } else { directory };
        (if directory == b".." { self.cmd(&[b"CDUP"]) } else { self.cmd(&[b"CWD", directory]) }) == 2
    }

    /// Net::FTP::pasv (EPSV on an IPv6 control connection).
    fn pasv(&mut self) -> Option<(String, u32)> {
        if self.stream.local_addr().is_ok_and(|address| address.is_ipv6()) { return self.epsv(); }
        if self.cmd(&[b"PASV"]) != 2 { return None; }
        let pattern = regex::bytes::Regex::new(r"(?-u)(\d+,\d+,\d+,\d+),(\d+),(\d+)").expect("valid PASV pattern");
        let message = self.message.concat();
        let captures = pattern.captures(&message)?;
        let number = |bytes: &[u8]| latin1(bytes).parse::<u32>().unwrap_or(u32::MAX);
        let port = number(&captures[2]).saturating_mul(256).saturating_add(number(&captures[3]));
        Some((latin1(&captures[1]).replace(',', "."), port))
    }

    /// Net::FTP::epsv: `(|||port|)` with any repeated delimiter in \x33-\x7e.
    fn epsv(&mut self) -> Option<(String, u32)> {
        if self.cmd(&[b"EPSV"]) != 2 { return None; }
        let message = self.message.concat();
        let port = (0..message.len()).find_map(|start| {
            let rest = &message[start..];
            let delimiter = *rest.get(1)?;
            if rest[0] != b'(' || !(0x33..=0x7e).contains(&delimiter) || rest.get(2) != Some(&delimiter) || rest.get(3) != Some(&delimiter) { return None; }
            let digits = rest[4..].iter().take_while(|b| b.is_ascii_digit()).count();
            // \d+ backtracks when the delimiter is itself a digit.
            (5..=4 + digits).rev().find(|&end| rest.get(end) == Some(&delimiter) && rest.get(end + 1) == Some(&b')'))
                .map(|end| latin1(&rest[4..end]).parse::<u32>().unwrap_or(u32::MAX))
        })?;
        Some((self.stream.peer_addr().ok()?.ip().to_string(), port))
    }

    /// Net::FTP::port: a listener on the control connection's local address.
    fn port(&mut self) -> Option<TcpListener> {
        let local = self.stream.local_addr().ok()?;
        let listener = TcpListener::bind(SocketAddr::new(local.ip(), 0)).ok()?;
        let address = listener.local_addr().ok()?;
        let argument = match address {
            SocketAddr::V4(v4) => {
                let [a, b, c, d] = v4.ip().octets();
                format!("{a},{b},{c},{d},{},{}", address.port() >> 8, address.port() & 0xff)
            }
            SocketAddr::V6(v6) => format!("|2|{}|{}|", v6.ip(), address.port()),
        };
        let command: &[u8] = if address.is_ipv6() { b"EPRT" } else { b"PORT" };
        (self.cmd(&[command, argument.as_bytes()]) == 2).then_some(listener)
    }

    /// Net::FTP::_data_cmd. `Err` is a croak, which LWP reports as a 500.
    fn data_command(&mut self, command: &[u8], arguments: &[&[u8]], passive: bool) -> Result<Option<Data>, String> {
        if let Some(bad) = arguments.iter().find(|argument| argument.iter().any(|&b| b == b'\r' || b == b'\n')) {
            return Err(format!("Bad argument '{}'\n", latin1(bad)));
        }
        let mut words = vec![command];
        words.extend_from_slice(arguments);
        if passive {
            let Some((host, port)) = self.pasv() else { return Ok(None) };
            // Send the command first, then open the data connection.
            self.command(&words);
            let data = u16::try_from(port).ok().and_then(|port| connect_data(&host, port));
            if self.response() == 1 { return Ok(data); }
            return Ok(None);
        }
        let Some(listener) = self.port() else { return Ok(None) };
        self.command(&words);
        if self.response() != 1 { return Ok(None); }
        Ok(accept(&listener))
    }

    /// Net::FTP::abort after a transfer that did not reach EOF.
    fn abort(&mut self, data: Data) -> bool {
        const IAC: u8 = 255;
        const IP: u8 = 244;
        const DM: u8 = 242;
        let _ = self.stream.write_all(&[IAC, IP, IAC]);
        self.command(&[&[DM, b'A', b'B', b'O', b'R']]);
        drop(data);
        self.response();
        self.code / 100 == 2
    }
}

struct Data {
    stream: TcpStream,
    ascii: bool,
    carriage_return: bool,
    eof: bool,
}

fn connect_data(host: &str, port: u16) -> Option<Data> {
    let address = (host, port).to_socket_addrs().ok()?.next()?;
    net::connect_address(&address).ok().map(|stream| Data { stream, ascii: false, carriage_return: false, eof: false })
}

/// IO::Socket::accept with the control timeout.
fn accept(listener: &TcpListener) -> Option<Data> {
    listener.set_nonblocking(true).ok()?;
    let deadline = Instant::now() + net::TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).ok()?;
                net::configure(&stream).ok()?;
                return Some(Data { stream, ascii: false, carriage_return: false, eof: false });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return None,
        }
    }
}

enum Stop { Error, Timeout }

impl Data {
    /// Net::FTP::I::read / Net::FTP::A::read until EOF. ASCII mode turns
    /// CRLF into LF, holding a final CR until the next block.
    fn read_all(&mut self, output: &mut Vec<u8>) -> Result<(), Stop> {
        let mut block = vec![0; BLOCK_SIZE];
        loop {
            let n = match self.stream.read(&mut block) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if net::is_timeout(&e) => return Err(Stop::Timeout),
                Err(_) => return Err(Stop::Error),
            };
            if !self.ascii {
                if n == 0 { self.eof = true; return Ok(()); }
                output.extend_from_slice(&block[..n]);
                continue;
            }
            let mut bytes = Vec::with_capacity(n + 1);
            if std::mem::take(&mut self.carriage_return) { bytes.push(b'\r'); }
            bytes.extend_from_slice(&block[..n]);
            if n > 0 && bytes.last() == Some(&b'\r') { bytes.pop(); self.carriage_return = true; }
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') { index += 1; }
                output.push(bytes[index]);
                index += 1;
            }
            if n == 0 { self.eof = true; return Ok(()); }
        }
    }
}

/// `int($ENV{FTP_PASSIVE}) == 0`: Perl numifies the leading decimal number.
fn perl_int_is_zero(value: &str) -> bool {
    let value = value.trim_start_matches([' ', '\t', '\n', '\r', '\x0c', '\x0b']);
    let bytes = value.as_bytes();
    let start = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let word = value[start..].to_ascii_lowercase();
    if word.starts_with("inf") || word.starts_with("nan") { return false; }
    let digits = |from: usize| from + bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    let integer_end = digits(start);
    let mut end = integer_end;
    if bytes.get(end) == Some(&b'.') { end = digits(end + 1); }
    if integer_end == start && end <= start + 1 { return true; }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let sign = end + 1 + usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        if digits(sign) > sign { end = digits(sign); }
    }
    value[..end].parse::<f64>().map_or(true, |number| number.abs() < 1.0)
}

/// URI::_generic::path_segments of `path_query` (URI::ftp's path), with
/// URI::_segment for segments carrying `;parameters`.
fn segments(path_query: &str) -> Vec<(Vec<u8>, Vec<&str>)> {
    path_query.split('/').map(|segment| {
        let mut parts = segment.split(';');
        let name = percent_decode(parts.next().unwrap_or_default().as_bytes());
        (name, if segment.contains(';') { parts.collect() } else { Vec::new() })
    }).collect()
}

pub(super) fn get(target: &str) -> Result<Vec<u8>, String> {
    let url = uri::parts(target);
    let authority = url.authority.unwrap_or_default();
    let (host, port) = host_port(authority, 21);
    // URI::_userpass and URI::ftp: anonymous unless the URL names a user.
    let userinfo = authority.rsplit_once('@').map(|(info, _)| info);
    let user = userinfo.map_or_else(|| b"anonymous".to_vec(), |info| percent_decode(info.split(':').next().unwrap_or_default().as_bytes()));
    let mut password = userinfo.and_then(|info| info.split_once(':')).map(|(_, password)| percent_decode(password.as_bytes()));
    if password.is_none() && matches!(&user[..], b"anonymous" | b"ftp") { password = Some(b"anonymous@".to_vec()); }

    let stream = net::connect(&host, port).map_err(|error| match error {
        ConnectError::Resolve => status_line(500, "Name or service not known"),
        ConnectError::Io(error) => status_line(500, &net::reason(&error)),
    })?;
    let mut ftp = Control { stream, partial: Vec::new(), lines: VecDeque::new(), closed: false, code: DEFAULT_CODE, message: Vec::new() };
    if ftp.response() != 2 { return Err(status_line(500, &ftp.message())); }

    // Net::FTP::login without .netrc: user and password are always defined here.
    let user: &[u8] = if user.is_empty() || user == b"0" { b"anonymous" } else { &user };
    let mut reply = ftp.cmd(&[b"USER", user]);
    if reply != 2 && reply != 3 {
        reply = ftp.cmd(&[b"user", user]);
    }
    if reply == 2 && ftp.code == 220 && user.contains(&b'@') { reply = ftp.response(); }
    if reply == 3 {
        if password.is_none() && user.starts_with(b"anonymous") { password = Some(b"-anonymous@".to_vec()); }
        let password = password.filter(|password| !password.is_empty() && password != b"0").unwrap_or_default();
        reply = ftp.cmd(&[b"PASS", &password]);
    }
    if reply != 2 {
        let message = ftp.message();
        return Err(status_line(401, message.strip_suffix('\n').unwrap_or(&message)));
    }
    ftp.cmd(&[b"PWD"]);

    let mut path_query = url.path.to_owned();
    if let Some(query) = url.query { path_query.push('?'); path_query.push_str(query); }
    let mut path = segments(&path_query).into_iter().filter(|(name, _)| !name.is_empty()).collect::<Vec<_>>();
    let (remote_file, parameters) = path.pop().unwrap_or_default();
    let ascii = parameters.iter().filter_map(|parameter| parameter.strip_prefix("type=")).next_back() == Some("a");
    ftp.cmd(&[b"TYPE", if ascii { b"A" } else { b"I" }]);
    for (directory, _) in &path {
        if !ftp.cwd(directory) { return Err(status_line(404, &format!("Can't chdir to {}", latin1(directory)))); }
    }
    ftp.cmd(&[b"MDTM", &remote_file]);

    let passive = env::var("FTP_PASSIVE").map_or(true, |value| !perl_int_is_zero(&value));
    let croak = |message: String| status_line(500, message.split('\n').next().unwrap_or_default());
    if !remote_file.is_empty() {
        if let Some(mut data) = ftp.data_command(b"RETR", &[&remote_file], passive).map_err(croak)? {
            data.ascii = ascii;
            let mut content = Vec::new();
            let read = data.read_all(&mut content);
            // Net::FTP::dataconn::abort: close after EOF, else ABOR.
            let closed = if read.is_ok() && data.eof { drop(data); ftp.response() == 2 } else { ftp.abort(data) };
            if !closed { return Err(status_line(500, &format!("FTP close response: {} {}", ftp.code, ftp.message()))); }
            return Ok(content);
        }
        if !(400..600).contains(&ftp.code) {
            return Err(status_line(400, &format!("FTP return code {}", ftp.code)));
        }
    }
    // Not a plain file: list it as a directory instead.
    if !remote_file.is_empty() && !ftp.cwd(&remote_file) {
        return Err(status_line(404, &format!("File '{}' not found", latin1(&remote_file))));
    }
    let mut listing = Vec::new();
    if let Some(mut data) = ftp.data_command(b"LIST", &[], passive).map_err(croak)? {
        data.ascii = true;
        if let Err(Stop::Timeout) = data.read_all(&mut listing) { return Err(status_line(500, "Timeout")); }
        drop(data);
        ftp.response();
    }
    // split(/\n/) drops trailing empty lines; join("\n", @lsl, '') ends each.
    let mut lines = listing.split(|&b| b == b'\n').collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.is_empty()) { lines.pop(); }
    let mut content = Vec::with_capacity(listing.len() + 1);
    for line in lines { content.extend_from_slice(line); content.push(b'\n'); }
    Ok(content)
}
