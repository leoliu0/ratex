//! LWP::Protocol::http(s) 6.76 over Net::HTTP::Methods 6.23: one GET per
//! connection (Biber's agent has no connection cache) and the response that
//! LWP::UserAgent sees. `Err` is the status line of LWP's internal response.
use super::{http_status, net::{self, ConnectError, Stream}, tls::{self, ContextError, SslEnv}, uri};
use std::io::{self, Read, Write};

const MAX_LINE_LENGTH: usize = 8 * 1024;
const MAX_HEADER_LINES: usize = 128;

pub(super) struct Response {
    pub(super) code: u16,
    pub(super) status: String,
    /// `$response->header('Location')`: every value, joined by ", ".
    pub(super) location: Option<Vec<u8>>,
    /// The first Content-Base value, else the first Base value.
    pub(super) base: Option<Vec<u8>>,
    /// Bytes collected before the body ended or a read died (X-Died).
    pub(super) body: Vec<u8>,
}

/// `HTTP::Response::status_line`.
pub(super) fn status_line(code: u16, message: &str) -> String {
    if message.is_empty() || message == "0" { format!("{code} {}", http_status::reason(code.into())) } else { format!("{code} {message}") }
}

pub(super) fn latin1(bytes: &[u8]) -> String { bytes.iter().map(|&b| char::from(b)).collect() }

fn internal(message: &str) -> String { status_line(500, message) }

/// URI::_server::host and ::port: userinfo, a trailing port and IPv6
/// brackets are removed; an empty port is the scheme default.
pub(super) fn host_port(authority: &str, default_port: u16) -> (String, u16) {
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, rest)| rest);
    let digits = host_port.bytes().rev().take_while(u8::is_ascii_digit).count();
    let (host, port) = match host_port.len().checked_sub(digits + 1).filter(|&colon| host_port.as_bytes()[colon] == b':') {
        Some(colon) => (&host_port[..colon], &host_port[colon + 1..]),
        None => (host_port, ""),
    };
    let host = host.strip_prefix('[').and_then(|host| host.strip_suffix(']')).unwrap_or(host);
    let host = String::from_utf8_lossy(&super::percent_decode(host.as_bytes())).into_owned();
    // An out-of-range port cannot be connected to; 0 fails the same way.
    (host, if port.is_empty() { default_port } else { port.parse().unwrap_or(0) })
}

fn default_port(scheme: &str) -> u16 {
    match scheme { "https" => 443, "ftp" => 21, "gopher" => 70, "nntp" | "news" => 119, _ => 80 }
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 { out.push(if i <= chunk.len() { ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' }); }
    }
    out
}

/// HTTP::Headers::authorization_basic from `split(":", $userinfo, 2)`,
/// each part unescaped. An empty userinfo splits into nothing: no header.
fn basic(userinfo: &str) -> Option<String> {
    if userinfo.is_empty() { return None; }
    let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    let mut credentials = super::percent_decode(user.as_bytes());
    credentials.push(b':');
    credentials.extend(super::percent_decode(password.as_bytes()));
    Some(format!("Basic {}", base64(&credentials)))
}

struct Connection {
    stream: Stream,
    buffer: Vec<u8>,
}

impl Connection {
    /// Net::HTTP::Methods::my_readline. `None` is EOF with nothing buffered.
    fn readline(&mut self, what: &str) -> Result<Option<(Vec<u8>, usize)>, String> {
        loop {
            if let Some(position) = self.buffer.iter().position(|&b| b == b'\n') {
                if position > MAX_LINE_LENGTH { return Err(format!("{what} line too long ({position}; limit is {MAX_LINE_LENGTH})")); }
                let mut line = self.buffer.drain(..=position).collect::<Vec<_>>();
                line.pop();
                let mut eol = 1;
                if line.last() == Some(&b'\r') { line.pop(); eol = 2; }
                return Ok(Some((line, eol)));
            }
            if self.buffer.len() > MAX_LINE_LENGTH { return Err(format!("{what} line too long (limit is {MAX_LINE_LENGTH})")); }
            let mut chunk = [0; 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok((!self.buffer.is_empty()).then(|| (std::mem::take(&mut self.buffer), 0))),
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if net::is_timeout(&e) => return Err("read timeout".into()),
                Err(e) if self.buffer.is_empty() => return Err(format!("{what} read failed: {}", net::reason(&e))),
                Err(_) => return Ok(Some((std::mem::take(&mut self.buffer), 0))),
            }
        }
    }

    /// Net::HTTP::Methods::my_read: buffered bytes first, else one sysread.
    fn read(&mut self, limit: usize) -> Result<Vec<u8>, String> {
        if !self.buffer.is_empty() {
            let n = limit.min(self.buffer.len());
            return Ok(self.buffer.drain(..n).collect());
        }
        let mut chunk = vec![0; limit.min(8 * 1024)];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(n) => { chunk.truncate(n); return Ok(chunk); }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if net::is_timeout(&e) => return Err("read timeout".into()),
                Err(e) => return Err(format!("read failed: {}", net::reason(&e))),
            }
        }
    }

    /// Net::HTTP::Methods::_read_header_lines.
    fn header_lines(&mut self, junk: bool) -> Result<Headers, String> {
        let pattern = regex::bytes::Regex::new(r"(?s-u)\A(\S+?)\s*:\s*(.*)").expect("valid header pattern");
        let mut headers: Headers = Vec::new();
        let mut count = 0;
        while let Some((line, _)) = self.readline("Header")? {
            // Perl's loop condition: an empty line, and "0", end the headers.
            if line.is_empty() || line == b"0" { break; }
            if let Some(captures) = pattern.captures(&line) {
                headers.push((captures[1].to_vec(), captures[2].to_vec()));
            } else if let (Some(last), true) = (headers.last_mut(), line.first().is_some_and(u8::is_ascii_whitespace)) {
                last.1.push(b' ');
                last.1.extend(line.iter().skip_while(|b| b.is_ascii_whitespace()));
            } else if !junk {
                return Err(format!("Bad header: '{}'", latin1(&line)));
            }
            count += 1;
            if count >= MAX_HEADER_LINES { return Err(format!("Too many header lines (limit is {MAX_HEADER_LINES})")); }
        }
        Ok(headers)
    }
}

/// Header names and values in arrival order, as raw bytes.
type Headers = Vec<(Vec<u8>, Vec<u8>)>;

struct Head {
    code: u16,
    message: String,
    headers: Headers,
    /// `None` for an assumed HTTP/0.9 response: the body runs to EOF.
    framing: Option<(String, Option<u64>)>,
}

/// Net::HTTP::Methods::read_response_headers(laxed => 1).
fn read_head(connection: &mut Connection) -> Result<Head, String> {
    let Some((status, eol)) = connection.readline("Status")? else {
        return Err("Server closed connection without sending any data back".into());
    };
    // split(/\s+/, $status, 3): whitespace runs separate the version and the
    // code; the message keeps its inner and trailing whitespace.
    let space = |b: &u8| b" \t\n\x0b\x0c\r".contains(b);
    let field = |bytes: &[u8]| bytes.iter().position(space).unwrap_or(bytes.len());
    let skip = |bytes: &[u8]| bytes.iter().position(|b| !space(b)).unwrap_or(bytes.len());
    let version = &status[..field(&status)];
    let rest = &status[version.len()..];
    let rest = &rest[skip(rest)..];
    let code = &rest[..field(rest)];
    let message = &rest[code.len()..];
    let message = &message[skip(message)..];
    let valid = version.starts_with(b"HTTP/")
        && code.len() == 3 && (b'1'..=b'5').contains(&code[0]) && code[1..].iter().all(u8::is_ascii_digit);
    if !valid {
        // Assume HTTP/0.9: the status line, and its line end, is body.
        let mut line = status;
        line.extend_from_slice(&b"\r\n"[2 - eol..]);
        connection.buffer.splice(..0, line);
        return Ok(Head { code: 200, message: "Assumed OK".into(), headers: Vec::new(), framing: None });
    }
    let code = code.iter().fold(0u16, |n, &d| n * 10 + u16::from(d - b'0'));
    let headers = connection.header_lines(true)?;
    let mut te = Vec::new();
    let mut content_length = None;
    let length = regex::bytes::Regex::new(r"(?-u)\A\s*(\d{1,15})(?:\s|\z)").expect("valid length pattern");
    for (name, value) in &headers {
        if name.eq_ignore_ascii_case(b"transfer-encoding") {
            let value = latin1(value.trim_ascii());
            if !value.is_empty() { te.push(value); }
        } else if name.eq_ignore_ascii_case(b"content-length") {
            if let Some(captures) = length.captures(value) { content_length = Some(latin1(&captures[1]).parse().expect("15 digits")); }
        }
    }
    Ok(Head { code, message: latin1(message), headers, framing: Some((te.join(","), content_length)) })
}

enum Transform { Identity, Deflate(Option<flate2::Decompress>), Gzip(Vec<u8>) }

impl Transform {
    /// Compress::Raw::Zlib inflation ignores errors; gunzip buffers to the end.
    fn apply(&mut self, input: Vec<u8>, last: bool) -> Result<Vec<u8>, String> {
        match self {
            Transform::Identity => Ok(input),
            Transform::Deflate(state) => {
                let Some(inflater) = state else { return Ok(Vec::new()) };
                let mut output = Vec::new();
                let mut consumed = 0;
                // Inflate until the input is consumed and no output is pending.
                loop {
                    output.reserve(input.len().max(1024) * 4);
                    let (read, written) = (inflater.total_in(), inflater.total_out());
                    let status = inflater.decompress_vec(&input[consumed..], &mut output, flate2::FlushDecompress::None);
                    consumed += (inflater.total_in() - read) as usize;
                    match status {
                        Ok(flate2::Status::StreamEnd) | Err(_) => { *state = None; break; }
                        Ok(_) if inflater.total_in() == read && inflater.total_out() == written => break,
                        Ok(_) => {}
                    }
                }
                Ok(output)
            }
            Transform::Gzip(buffer) => {
                buffer.extend_from_slice(&input);
                if !last { return Ok(Vec::new()); }
                flate2::read::GzDecoder::new(&buffer[..]).read_to_end(&mut Vec::new())
                    .map_err(|e| format!("Can't gunzip content: {e}"))?;
                // Net::HTTP::Methods returns `\$output` here, so LWP writes the
                // reference's stringification; its address is not reproducible.
                Ok(b"SCALAR(0x0)".to_vec())
            }
        }
    }
}

/// Net::HTTP::Methods::read_entity_body until it returns 0. A die leaves the
/// bytes collected so far, as LWP::Protocol::collect does.
fn read_body(connection: &mut Connection, head: &Head, body: &mut Vec<u8>) -> Result<(), String> {
    const SIZE: usize = 4096;
    let Some((te, content_length)) = &head.framing else {
        loop { let bytes = connection.read(8 * 1024)?; if bytes.is_empty() { return Ok(()); } body.extend(bytes); }
    };
    if !te.is_empty() {
        let lower = te.to_ascii_lowercase();
        let mut codings = split_commas(&lower);
        if codings.pop() != Some("chunked") { return Err(format!("Chunked must be last Transfer-Encoding '{te}'")); }
        while codings.last() == Some(&"chunked") { codings.pop(); }
        let mut transforms = codings.iter().map(|coding| match *coding {
            "deflate" => Ok(Transform::Deflate(Some(flate2::Decompress::new(true)))),
            "gzip" => Ok(Transform::Gzip(Vec::new())),
            "identity" => Ok(Transform::Identity),
            _ => Err(format!("Can't handle transfer encoding '{te}'")),
        }).collect::<Result<Vec<_>, _>>()?;
        transforms.reverse();
        let mut first = true;
        loop {
            let mut line = connection.readline("Entity body")?.map(|(line, _)| line);
            if !first {
                if line.as_deref() != Some(b"") {
                    return Err(format!("Missing newline after chunk data: '{}'", latin1(line.as_deref().unwrap_or_default())));
                }
                line = connection.readline("Entity body")?.map(|(line, _)| line);
            }
            first = false;
            let Some(line) = line else { return Err("EOF when chunk header expected".into()) };
            let size = line.iter().position(|&b| b == b';').map_or(&line[..], |end| &line[..end]);
            let digits = size.iter().take_while(|b| b.is_ascii_hexdigit()).count();
            if digits == 0 || !size[digits..].iter().all(u8::is_ascii_whitespace) {
                return Err(format!("Bad chunk-size in HTTP response: {}", latin1(&line)));
            }
            let mut remaining = size[..digits].iter().fold(0u64, |n, &d| n.saturating_mul(16).saturating_add(u64::from((d as char).to_digit(16).expect("hex digit"))));
            if remaining == 0 {
                let mut tail = Vec::new();
                for transform in &mut transforms { tail = transform.apply(std::mem::take(&mut tail), true)?; }
                body.extend(tail);
                connection.header_lines(false)?;
                return Ok(());
            }
            while remaining > 0 {
                let mut bytes = connection.read(SIZE.min(remaining.try_into().unwrap_or(SIZE)))?;
                // EOF inside a chunk ends the body without an error.
                if bytes.is_empty() { return Ok(()); }
                remaining -= bytes.len() as u64;
                for transform in &mut transforms { bytes = transform.apply(bytes, false)?; }
                body.extend(bytes);
            }
        }
    }
    let mut remaining = match content_length {
        Some(length) => *length,
        None if head.code < 200 || head.code == 204 || head.code == 304 => 0,
        None => u64::MAX,
    };
    while remaining > 0 {
        let bytes = connection.read(SIZE.min(remaining.try_into().unwrap_or(SIZE)))?;
        if bytes.is_empty() { break; }
        remaining = remaining.saturating_sub(bytes.len() as u64);
        body.extend(bytes);
    }
    Ok(())
}

/// `split(/\s*,\s*/, $te)`: leading empty fields stay, trailing ones go.
fn split_commas(value: &str) -> Vec<&str> {
    let mut fields = value.split(',').map(|field| field.trim_matches(|c: char| c.is_ascii_whitespace())).collect::<Vec<_>>();
    while fields.last() == Some(&"") { fields.pop(); }
    fields
}

/// LWP::Protocol::http::_new_socket (and the https subclass).
fn new_socket(host: &str, port: u16, https: bool, ssl: &SslEnv) -> Result<Stream, String> {
    let shown = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_owned() };
    let failed = |reason: &str| internal(&format!("Can't connect to {shown}:{port} ({reason})"));
    // IO::Socket::SSL builds (and checks) its context before connecting.
    let config = if https {
        Some(ssl.config().map_err(|error| match error {
            ContextError::Die(message) => internal(&message),
            ContextError::Error(message) => failed(&message),
        })?)
    } else { None };
    let tcp = net::connect(host, port).map_err(|error| match error {
        ConnectError::Resolve => failed("Name or service not known"),
        ConnectError::Io(error) => failed(&net::reason(&error)),
    })?;
    let Some(config) = config else { return Ok(Stream::Plain(tcp)) };
    tls::handshake(Stream::Plain(tcp), host, config).map_err(|failure| failed(failure.connect_reason()))
}

/// Net::HTTP::Methods::format_request without content, keep-alive or an
/// explicit TE header; HTTP::Headers orders the request headers.
fn format_request(method: &str, path: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    if path.is_empty() || path.bytes().any(|b| b.is_ascii_whitespace()) { return Err(internal("Bad method or uri")); }
    let mut request = format!("{method} {path} HTTP/1.1\r\nTE: deflate,gzip;q=0.3\r\nConnection: TE, close\r\n");
    for (name, value) in headers { request.push_str(&format!("{name}: {}\r\n", value.replace('\n', " "))); }
    request.push_str("\r\n");
    Ok(request.into_bytes())
}

/// Sends one request and reads the response up to (not including) the body.
fn exchange(mut stream: Stream, request: &[u8]) -> Result<(Connection, Head), String> {
    stream.write_all(request).and_then(|()| stream.flush()).map_err(|e| internal(&format!("write failed: {}", net::reason(&e))))?;
    let mut connection = Connection { stream, buffer: Vec::new() };
    let mut head = read_head(&mut connection).map_err(|e| internal(&e))?;
    if head.code == 100 { head = read_head(&mut connection).map_err(|e| internal(&e))?; }
    Ok((connection, head))
}

fn header<'a>(head: &'a Head, name: &'a str) -> impl Iterator<Item = &'a [u8]> {
    head.headers.iter().filter(move |(key, _)| key.eq_ignore_ascii_case(name.as_bytes())).map(|(_, value)| &value[..])
}

/// A GET of `target` (an escaped absolute URI), optionally through `proxy`.
pub(super) fn get(target: &str, proxy: Option<&str>, ssl: &SslEnv) -> Result<Response, String> {
    let url = uri::parts(target);
    let scheme = url.scheme.unwrap_or_default().to_ascii_lowercase();
    let authority = url.authority.unwrap_or_default();
    let proxy = proxy.map(|proxy| {
        let parts = uri::parts(proxy);
        (parts.scheme.unwrap_or_default().to_ascii_lowercase(), parts.authority.unwrap_or_default())
    });
    let ssl_tunnel = proxy.is_some() && scheme == "https";
    // _fixup_header: the Host header drops "user:pass@", which becomes
    // Authorization; a proxy's userinfo becomes Proxy-Authorization.
    let (userinfo, host_header) = authority.split_once('@').map_or((None, authority), |(info, host)| (Some(info), host));
    let authorization = userinfo.and_then(basic);
    let proxy_authorization = proxy.as_ref().and_then(|(_, authority)| authority.rsplit_once('@')).and_then(|(info, _)| basic(info));
    let stream = if let Some((proxy_scheme, proxy_authority)) = &proxy {
        let (host, port) = host_port(proxy_authority, default_port(proxy_scheme));
        let stream = new_socket(&host, port, proxy_scheme == "https", ssl)?;
        if ssl_tunnel {
            let (host, port) = host_port(authority, 443);
            // URI::_server::host_port: greedy userinfo removal, default port added.
            let server = authority.rsplit_once('@').map_or(authority, |(_, server)| server);
            let mut tunnel = server.strip_suffix(':').unwrap_or(server).to_owned();
            if !tunnel.rsplit_once(':').is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())) {
                tunnel.push_str(&format!(":{port}"));
            }
            let mut headers = vec![("Host", tunnel.as_str())];
            if let Some(value) = &proxy_authorization { headers.push(("Proxy-Authorization", value)); }
            let (connection, head) = exchange(stream, &format_request("CONNECT", &tunnel, &headers)?)?;
            if !(200..300).contains(&head.code) {
                return Err(internal(&format!("establishing SSL tunnel failed: {}", status_line(head.code, &head.message))));
            }
            let config = ssl.config().map_err(|error| match error {
                ContextError::Die(message) => internal(&message),
                ContextError::Error(message) => internal(&format!("SSL upgrade failed: {message}")),
            })?;
            tls::handshake(connection.stream, &host, config).map_err(|failure| internal(&format!("SSL upgrade failed: {}", failure.upgrade_reason())))?
        } else { stream }
    } else {
        let (host, port) = host_port(authority, default_port(&scheme));
        new_socket(&host, port, scheme == "https", ssl)?
    };
    let path = if proxy.is_some() && !ssl_tunnel { target.to_owned() } else {
        let mut path = url.path.to_owned();
        if let Some(query) = url.query { path.push('?'); path.push_str(query); }
        if !path.starts_with('/') { path.insert(0, '/'); }
        path
    };
    let mut headers = Vec::new();
    if let Some(value) = &authorization { headers.push(("Authorization", value.as_str())); }
    headers.push(("Host", host_header));
    if let (Some(value), false) = (&proxy_authorization, scheme == "https") { headers.push(("Proxy-Authorization", value.as_str())); }
    headers.push(("User-Agent", "Mozilla/5.0"));
    headers.push(("Zotero-Allowed-Request", "1"));
    let (mut connection, head) = exchange(stream, &format_request("GET", &path, &headers)?)?;
    let mut body = Vec::new();
    // A die while reading the body only records X-Died; the status stands.
    let _ = read_body(&mut connection, &head, &mut body);
    let locations = header(&head, "location").collect::<Vec<_>>();
    let location = (!locations.is_empty()).then(|| locations.join(&b", "[..]));
    let base = header(&head, "content-base").next().or_else(|| header(&head, "base").next()).map(<[u8]>::to_vec);
    Ok(Response { code: head.code, status: status_line(head.code, &head.message), location, base, body })
}
