//! Redirect-only protocols shipped in the oracle's LWP package. Direct Biber
//! datasource names still pass the narrower http(s)/ftp(s) recognition check.
use super::{failure, percent_decode};
use std::{env, io::{BufRead, BufReader, Read, Write}, net::{TcpStream, ToSocketAddrs}, time::Duration};
use url::Url;

fn connect(address: &str) -> std::io::Result<TcpStream> {
    let mut error = None;
    for address in address.to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, Duration::from_secs(180)) {
            Ok(stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(180)))?;
                stream.set_write_timeout(Some(Duration::from_secs(180)))?;
                return Ok(stream);
            }
            Err(e) => error = Some(e),
        }
    }
    Err(error.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "No host addresses")))
}

pub(super) fn nntp(source: &str, url: &Url, target: &str, proxy: Option<&str>) -> Result<Vec<u8>, String> {
    if proxy.is_some() { return Err(failure(source, "400 You can not proxy through NNTP")); }
    // The oracle intentionally does not pass URI::news's port to Net::NNTP.
    // An environment host can contain a port, as accepted by IO::Socket::IP.
    let host = url.host_str().map(str::to_owned).or_else(|| env::var("NNTPSERVER").ok().filter(|s| !s.is_empty() && s != "0"))
        .or_else(|| env::var("NEWSHOST").ok().filter(|s| !s.is_empty() && s != "0")).unwrap_or_else(|| "news".into());
    let address = if host.starts_with('[') && host.contains("]:") || host.matches(':').count() == 1 {
        host.clone()
    } else if host.contains(':') { format!("[{host}]:119") } else { format!("{host}:119") };
    let mut socket = BufReader::new(connect(&address).map_err(|_| failure(source, "500 Can't connect to nntp server"))?);
    let (greeting, greeting_message) = nntp_status(&mut socket).map_err(|_| failure(source, "500 Can't connect to nntp server"))?;
    if greeting / 100 != 2 { return Err(failure(source, "500 Can't connect to nntp server")); }
    socket.get_mut().write_all(b"MODE READER\r\n").map_err(|_| failure(source, "500 Can't connect to nntp server"))?;
    let (reader_code, reader_message) = nntp_status(&mut socket).map_err(|_| failure(source, "500 Can't connect to nntp server"))?;
    let (code, message) = if reader_code / 100 == 2 { (reader_code, reader_message) } else { (greeting, greeting_message) };
    if code / 100 != 2 { return Err(failure(source, &format!("503 {message}"))); }
    let raw_path = super::uri::parts(target).path;
    let path = raw_path.strip_prefix('/').unwrap_or(raw_path);
    let group = percent_decode(path.as_bytes());
    let article = group.contains(&b'@');
    let mut command = Vec::new();
    command.extend_from_slice(if article { b"ARTICLE <" } else { b"GROUP " });
    command.extend_from_slice(&group);
    if article { command.push(b'>'); }
    command.extend_from_slice(b"\r\n");
    socket.get_mut().write_all(&command).map_err(|_| failure(source, "404 Connection closed"))?;
    let (code, message) = nntp_status(&mut socket).map_err(|_| failure(source, "404 Connection closed"))?;
    if code / 100 != 2 { let _ = socket.get_mut().write_all(b"QUIT\r\n"); return Err(failure(source, &format!("404 {message}"))); }
    if !article { let _ = socket.get_mut().write_all(b"QUIT\r\n"); return Err(failure(source, "501 GET newsgroup not implemented yet")); }
    let mut body = Vec::new();
    let mut header = true;
    let mut header_key = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        if socket.read_until(b'\n', &mut line).map_err(|_| failure(source, "404 Connection closed"))? == 0 { return Err(failure(source, "404 Connection closed")); }
        if line.ends_with(b"\r\n") { line.remove(line.len() - 2); }
        if line == b".\n" { break; }
        if line.starts_with(b"..") { line.remove(0); }
        if header {
            if line.iter().all(u8::is_ascii_whitespace) { header = false; continue; }
            if line.first().is_some_and(u8::is_ascii_whitespace) && header_key { continue; }
            if line.iter().position(|&c| c == b':').is_some_and(|colon| colon > 0 && !line[..colon].iter().any(u8::is_ascii_whitespace)) {
                header_key = true; continue;
            }
            header = false;
        }
        body.extend_from_slice(&line);
    }
    let _ = socket.get_mut().write_all(b"QUIT\r\n");
    Ok(body)
}

fn nntp_status(socket: &mut BufReader<TcpStream>) -> std::io::Result<(u16, String)> {
    let mut message = String::new();
    let mut code = 0;
    loop {
        let mut line = String::new();
        if socket.read_line(&mut line)? == 0 { return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)); }
        let bytes = line.as_bytes();
        if bytes.len() < 3 { continue; }
        let Ok(parsed) = line[..3].parse::<u16>() else { continue; };
        if code == 0 { code = parsed; }
        if bytes.len() > 4 { if !message.is_empty() { message.push('\n'); } message.push_str(line[4..].trim_end_matches(['\r','\n'])); }
        if bytes.get(3) != Some(&b'-') { return Ok((code, message)); }
    }
}

pub(super) fn gopher(source: &str, url: &Url, target: &str, proxy: Option<&str>) -> Result<Vec<u8>, String> {
    if proxy.is_some() { return Err(failure(source, "400 You can not proxy through the gopher")); }
    let raw = super::uri::parts(target);
    let mut path = raw.path.strip_prefix('/').unwrap_or(raw.path).to_owned();
    if let Some(query) = raw.query { path.push('?'); path.push_str(query); }
    let gtype = path.as_bytes().first().copied().unwrap_or(b'1');
    if !b"0145679hgI".contains(&gtype) { return Err(failure(source, &format!("501 Library does not support gophertype {}", gtype as char))); }
    let decoded = percent_decode(path.replacen('?', "\t", 1).as_bytes());
    let selector = decoded.get(1..).unwrap_or_default();
    let fields = selector.splitn(3, |&c| c == b'\t').collect::<Vec<_>>();
    if gtype == b'7' && fields.get(1).is_none_or(|search| search.is_empty() || *search == b"0") {
        return Ok(format!("<HEAD>\n<TITLE>Gopher Index</TITLE>\n<ISINDEX>\n</HEAD>\n<BODY>\n<H1>{target}<BR>Gopher Search</H1>\nThis is a searchable Gopher index.\nUse the search function of your browser to enter search terms.\n</BODY>\n").into_bytes());
    }
    let host = url.host_str().unwrap_or("");
    let port = url.port().unwrap_or(70);
    let address = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut stream = connect(&address).map_err(|_| failure(source, &format!("500 Can't connect to {host}:{port}")))?;
    stream.write_all(selector).and_then(|_| stream.write_all(b"\r\n")).map_err(|e| failure(source, &format!("500 {e}")))?;
    let mut bytes = Vec::new();
    // LWP retains bytes already collected when a socket read fails.
    let _ = stream.read_to_end(&mut bytes);
    if matches!(gtype, b'1' | b'7') { Ok(gopher_menu(&bytes)) } else { Ok(bytes) }
}

fn gopher_menu(menu: &[u8]) -> Vec<u8> {
    let mut result = b"<HTML>\n<HEAD>\n   <TITLE>Gopher menu</TITLE>\n</HEAD>\n<BODY>\n<H1>Gopher menu</H1>\n".to_vec();
    let menu = menu.iter().copied().filter(|&b| b != b'\r').collect::<Vec<_>>();
    for line in menu.split(|&b| b == b'\n') {
        if line.starts_with(b".") { break; }
        if line.is_empty() { continue; }
        let fields = line.split(|&b| b == b'\t').collect::<Vec<_>>();
        let pretty = fields[0];
        let gtype = pretty[0];
        let selector = fields.get(1).copied().unwrap_or_default();
        let host = fields.get(2).copied().unwrap_or_default();
        let port = fields.get(3).copied().unwrap_or_default();
        let mut link = Vec::new();
        if matches!(gtype, b'8' | b'T') {
            link.extend_from_slice(if gtype == b'8' { b"telnet://" } else { b"tn3270://" });
            if !selector.is_empty() { link.extend_from_slice(&escape_component(selector)); link.push(b'@'); }
        } else { link.extend_from_slice(b"gopher://"); }
        if host.contains(&b':') && !host.starts_with(b"[") { link.push(b'['); link.extend_from_slice(host); link.push(b']'); }
        else { link.extend_from_slice(host); }
        if !port.is_empty() { link.push(b':'); link.extend_from_slice(port); }
        if !matches!(gtype, b'8' | b'T') { link.push(b'/'); link.push(gtype); link.extend_from_slice(&escape_component(selector)); }
        result.extend_from_slice(b"<A HREF=\""); result.extend_from_slice(&link); result.extend_from_slice(b"\">");
        result.extend_from_slice(&pretty[1..]); result.extend_from_slice(b"</A><BR>\n");
    }
    result.extend_from_slice(b"</BODY>\n</HTML>\n");
    result
}

fn escape_component(input: &[u8]) -> Vec<u8> {
    const HEX: &[u8] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(input.len());
    for &b in input {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { out.push(b); }
        else { out.extend_from_slice(&[b'%', HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]); }
    }
    out
}

pub(super) fn loopback(target: &str) -> Vec<u8> {
    format!("GET {target}\nUser-Agent: Mozilla/5.0\nZotero-Allowed-Request: 1\n\n").into_bytes()
}

pub(super) fn mailto_error(source: &str, proxy: Option<&str>) -> String {
    let specified = env::var("SENDMAIL").is_ok_and(|s| !s.is_empty() && s != "0");
    let installed = ["/usr/sbin/sendmail", "/usr/lib/sendmail", "/usr/ucblib/sendmail"].iter().any(|path| {
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))] { std::path::Path::new(path).is_file() }
    });
    if !specified && !installed { failure(source, "501 Can't find the 'sendmail' program") }
    else if proxy.is_some() { failure(source, "400 You can not proxy with mail") }
    else { failure(source, "400 Library does not allow method GET for 'mailto:' URLs") }
}
