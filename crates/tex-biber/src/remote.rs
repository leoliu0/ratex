//! Biber 2.22's `locate_data_file` network branch, without a Perl process.
//!
//! Call the embedding resource resolver before `Cache::fetch`. Responses remain
//! bytes: neither MIME/charset, Content-Disposition nor Content-Encoding changes
//! the datasource format or decoding. Those belong to the selected input driver.
//! Successful downloads are cached by the original, unescaped source name.
use curl::easy::{Easy, HttpVersion, List};
use std::{collections::BTreeMap, env, sync::Arc, time::Duration};
use url::Url;
#[path = "remote/protocols.rs"]
mod protocols;
#[path = "remote/uri.rs"]
mod uri;
#[path = "remote/http_status.rs"]
mod http_status;
#[path = "remote/response.rs"]
mod response;

const CA_ROOTS: &[u8] = include_bytes!("remote-ca.pem");

#[derive(Default)]
pub(crate) struct Cache {
    sources: BTreeMap<String, Arc<[u8]>>,
}

impl Cache {
    /// `None` means this name is not recognized as remote by the oracle. A
    /// recognized URL failing to download is fatal, not a missing-file warning.
    pub(crate) fn fetch(&mut self, source: &str, options: &BTreeMap<String, String>) -> Result<Option<Arc<[u8]>>, String> {
        if !is_remote(source) { return Ok(None); }
        if let Some(bytes) = self.sources.get(source) { return Ok(Some(Arc::clone(bytes))); }
        let bytes: Arc<[u8]> = download(source, options)?.into();
        self.sources.insert(source.to_owned(), Arc::clone(&bytes));
        Ok(Some(bytes))
    }
}

pub(crate) fn is_remote(source: &str) -> bool {
    ["http://", "https://", "ftp://", "ftps://"].iter().any(|prefix| source.starts_with(prefix))
}

fn failure(source: &str, status: &str) -> String {
    format!("Could not fetch '{source}' (HTTP error: {status})")
}

fn download(source: &str, options: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    let mut target = escape_uri(source.trim_end());
    // LWP's original request gets a proxy slot only when a proxy is selected;
    // that slot is cloned (and retained) on subsequent redirects.
    let mut proxy = None;
    for redirects in 0..=7 {
        let parsed = Url::parse(&target).map_err(|e| failure(source, &format!("400 {e}")))?;
        let scheme = parsed.scheme();
        if scheme == "ftps" { return Err(failure(source, "501 Protocol scheme 'ftps' is not supported")); }
        if proxy.is_none() { proxy = environment_proxy(&parsed); }
        if scheme == "data" {
            if proxy.is_some() { return Err(failure(source, "400 You can not proxy with data")); }
            return Ok(data_uri(&target));
        }
        match scheme {
            "nntp" | "news" => return protocols::nntp(source, &parsed, &target, proxy.as_deref()),
            "gopher" => return protocols::gopher(source, &parsed, &target, proxy.as_deref()),
            "loopback" => return Ok(protocols::loopback(&target)),
            "mailto" => return Err(protocols::mailto_error(source, proxy.as_deref())),
            "cpan" => {
                if proxy.is_some() { return Err(failure(source, "400 You can not proxy with cpan")); }
                if redirects == 7 { return Err(failure(source, "302 Found")); }
                // CPAN::Config is absent from the oracle package, so its LWP
                // driver uses the documented http://cpan.org/ fallback mirror.
                let raw_path = uri::parts(&target).path;
                let path = raw_path.strip_prefix('/').unwrap_or(raw_path);
                target = uri::absolute(path, "http://cpan.org/", false);
                continue;
            }
            _ => {}
        }
        if !matches!(scheme, "http" | "https" | "ftp") {
            return Err(failure(source, &format!("501 Protocol scheme '{scheme}' is not supported")));
        }
        let mut easy = Easy::new();
        let setup = (|| -> Result<(), curl::Error> {
            let wire_url = if scheme == "ftp" && proxy.is_none() { ftp_url(&target) } else { std::borrow::Cow::Borrowed(target.as_str()) };
            easy.url(&wire_url)?;
            easy.path_as_is(true)?;
            easy.get(true)?;
            easy.useragent("Mozilla/5.0")?;
            easy.http_version(HttpVersion::V11)?;
            easy.http_content_decoding(false)?;
            easy.connect_timeout(Duration::from_secs(180))?;
            // LWP times out idle socket operations, not the entire download.
            easy.low_speed_limit(1)?;
            easy.low_speed_time(Duration::from_secs(180))?;
            easy.noproxy("")?;
            easy.proxy(proxy.as_deref().unwrap_or(""))?;
            let mut headers = List::new();
            headers.append("Zotero-Allowed-Request: 1")?;
            headers.append("TE: deflate,gzip;q=0.3")?;
            headers.append("Connection: TE, close")?;
            headers.append("Accept:")?;
            easy.http_headers(headers)?;
            if scheme == "https" { configure_tls(&mut easy, options)?; }
            if scheme == "ftp" { configure_ftp(&mut easy)?; }
            if scheme == "ftp" && proxy.is_none() && parsed.username().is_empty() {
                easy.username("anonymous")?;
                easy.password("anonymous@")?;
            } else if scheme == "ftp" && proxy.is_none() && parsed.password().is_none() && matches!(parsed.username(), "anonymous" | "ftp") {
                easy.password("anonymous@")?;
            }
            Ok(())
        })();
        setup.map_err(|e| failure(source, &format!("500 {e}")))?;
        let (response, transfer_result) = response::collect(&mut easy);
        if let Err(error) = transfer_result {
            if scheme == "ftp" && response.status.is_empty() {
                if error.is_login_denied() {
                    if let Some((_, message)) = &response.ftp_error { return Err(failure(source, &format!("401 {message}"))); }
                }
                if error.is_remote_access_denied() {
                    if let Some(directory) = &response.ftp_cwd {
                        return Err(failure(source, &format!("404 Can't chdir to {}", String::from_utf8_lossy(directory))));
                    }
                }
                if error.code() == curl_sys::CURLE_REMOTE_FILE_NOT_FOUND {
                    let file = ftp_filename(&target);
                    easy.url(&ftp_directory(&target)).map_err(|e| failure(source, &format!("500 {e}")))?;
                    let (directory, result) = response::collect(&mut easy);
                    match result {
                        Ok(()) => return Ok(normalize_listing(directory.bytes)),
                        Err(e) if e.code() == curl_sys::CURLE_FTP_COULDNT_RETR_FILE => return Ok(Vec::new()),
                        Err(e) if e.is_remote_access_denied() || e.code() == curl_sys::CURLE_REMOTE_FILE_NOT_FOUND => return Err(failure(source, &format!("404 File '{file}' not found"))),
                        Err(e) => return Err(failure(source, &transport_error(&e, &easy, &parsed, proxy.as_deref()))),
                    }
                }
            }
            // LWP::Protocol::collect retains the original HTTP status and the
            // bytes written before a body-read error (Client-Aborted/X-Died).
            // Biber tests only is_success, including unclean TLS EOF and
            // truncated Content-Length/chunks. Header/connection errors fail.
            if response.status.is_empty() {
                return Err(failure(source, &transport_error(&error, &easy, &parsed, proxy.as_deref())));
            }
        }
        let code = easy.response_code().map_err(|e| failure(source, &format!("500 {e}")))?;
        if (scheme == "ftp" && response.status.is_empty()) || (200..300).contains(&code) {
            return Ok(if scheme == "ftp" && target.split('#').next().unwrap_or(&target).ends_with('/') { normalize_listing(response.bytes) } else { response.bytes });
        }
        if redirects < 7 && matches!(code, 301 | 302 | 303 | 307 | 308) {
            let referral = response.location.as_deref().unwrap_or("");
            let base = response.base.as_deref().map(|value| uri::absolute(value, &target, false));
            let next = uri::absolute(referral, base.as_deref().unwrap_or(&target), true);
            // LWP explicitly disallows file redirects and leaves the 3xx status.
            if !uri::parts(&next).scheme.is_some_and(|scheme| scheme.eq_ignore_ascii_case("file")) { target = next; continue; }
        }
        let status = if response.status.is_empty() { format!("{code} {}", http_status::reason(code)) } else { response.status };
        return Err(failure(source, &status));
    }
    unreachable!("the final redirect iteration always returns")
}

fn configure_ftp(easy: &mut Easy) -> Result<(), curl::Error> {
    easy.ignore_content_length(true)?;
    easy.verbose(true)?;
    // Net::FTP defaults to PASV for IPv4. Libcurl still uses EPSV/EPRT where
    // necessary for IPv6, even with these IPv4 extension preferences disabled.
    for option in [curl_sys::CURLOPT_FTP_USE_EPSV, curl_sys::CURLOPT_FTP_USE_EPRT] {
        // SAFETY: the handle is live, each option requires a C long, and no
        // callback runs while changing the handle's configuration.
        let code = unsafe { curl_sys::curl_easy_setopt(easy.raw(), option, 0 as std::os::raw::c_long) };
        if code != curl_sys::CURLE_OK { return Err(curl::Error::new(code)); }
    }
    if env::var("FTP_PASSIVE").is_ok_and(|value| value.is_empty() || value == "0") {
        // CURLOPT_FTPPORT copies this static NUL-terminated string. "-" chooses
        // the control connection's local address, matching Net::FTP::port.
        let code = unsafe { curl_sys::curl_easy_setopt(easy.raw(), curl_sys::CURLOPT_FTPPORT, b"-\0".as_ptr().cast::<std::os::raw::c_char>()) };
        if code != curl_sys::CURLE_OK { return Err(curl::Error::new(code)); }
    }
    Ok(())
}

fn ftp_url(target: &str) -> std::borrow::Cow<'_, str> {
    let url = target.split('#').next().unwrap_or(target);
    let last = url.rfind('/').map_or(0, |index| index + 1);
    let parameter_start = url[last..].find(';').map(|index| index + last);
    let query = url.find('?');
    if parameter_start.is_none() && query.is_none() { return std::borrow::Cow::Borrowed(target); }
    let end = parameter_start.unwrap_or(url.len());
    let mut result = String::with_capacity(end + 8);
    if let Some(query) = query.filter(|&query| query < end) {
        result.push_str(&url[..query]);
        if uri::parts(url).path.is_empty() { result.push('/'); }
        result.push_str("%3F"); result.push_str(&url[query + 1..end]);
    } else { result.push_str(&url[..end]); }
    if let Some(start) = parameter_start {
        let transfer_type = url[start + 1..].split(';').filter_map(|parameter| {
            let parameter = percent_decode(parameter.as_bytes());
            parameter.strip_prefix(b"type=").map(|kind| kind.to_vec())
        }).last();
        // Only lowercase type=a selects ASCII in LWP; every other parameter
        // selects binary. In particular type=d is not a directory request.
        if transfer_type.as_deref() == Some(b"a") { result.push_str(";type=a"); }
    }
    std::borrow::Cow::Owned(result)
}

fn ftp_filename(target: &str) -> String {
    let last = target.split('#').next().unwrap_or(target).rsplit('/').next().unwrap_or("");
    String::from_utf8_lossy(&percent_decode(last.split(';').next().unwrap_or(last).as_bytes())).into_owned()
}

fn ftp_directory(target: &str) -> String {
    let wire = ftp_url(target);
    let wire = wire.split('#').next().unwrap_or(&wire);
    let directory = wire.strip_suffix(";type=a").unwrap_or(wire);
    format!("{directory}/")
}

fn normalize_listing(mut bytes: Vec<u8>) -> Vec<u8> {
    let mut write = 0;
    for read in 0..bytes.len() {
        if bytes[read] == b'\r' && bytes.get(read + 1) == Some(&b'\n') { continue; }
        bytes[write] = bytes[read]; write += 1;
    }
    bytes.truncate(write);
    if bytes.last().is_some_and(|&b| b != b'\n') { bytes.push(b'\n'); }
    bytes
}

fn transport_error(error: &curl::Error, easy: &Easy, url: &Url, proxy: Option<&str>) -> String {
    let proxy_url = proxy.and_then(|value| Url::parse(value).ok());
    let destination = if error.is_peer_failed_verification() { url } else { proxy_url.as_ref().unwrap_or(url) };
    let host = destination.host_str().unwrap_or("");
    let port = destination.port_or_known_default().unwrap_or(if destination.scheme() == "ftp" { 21 } else { 80 });
    if error.is_couldnt_resolve_host() || error.is_couldnt_resolve_proxy() {
        return if url.scheme() == "ftp" && proxy.is_none() { "500 Name or service not known".into() } else { format!("500 Can't connect to {host}:{port} (Name or service not known)") };
    }
    if error.is_couldnt_connect() {
        let errno = easy.os_errno().unwrap_or(0);
        let mut reason = std::io::Error::from_raw_os_error(errno).to_string();
        if let Some(index) = reason.find(" (os error ") { reason.truncate(index); }
        return if url.scheme() == "ftp" && proxy.is_none() { format!("500 {reason}") } else { format!("500 Can't connect to {host}:{port} ({reason})") };
    }
    if error.is_peer_failed_verification() {
        let detail = error.to_string();
        let reason = if detail.contains("certificate subject name") || detail.contains("no alternative certificate") {
            "hostname verification failed"
        } else { "certificate verify failed" };
        return format!("500 Can't connect to {host}:{port} ({reason})");
    }
    if error.is_operation_timedout() { return "500 read timeout".into(); }
    format!("500 {error}")
}

fn configure_tls(easy: &mut Easy, options: &BTreeMap<String, String>) -> Result<(), curl::Error> {
    let noverify = options.contains_key("ssl_noverify_host") || options.contains_key("ssl-noverify-host");
    let compatibility_ca = env::var_os("HTTPS_CA_FILE").is_some() || env::var_os("HTTPS_CA_DIR").is_some();
    let verify_host = !noverify && match env::var("PERL_LWP_SSL_VERIFY_HOSTNAME") {
        Ok(value) => !value.is_empty() && value != "0",
        Err(_) => !compatibility_ca,
    };
    easy.ssl_verify_host(verify_host)?;
    // IO::Socket::SSL verifies the chain even when hostname verification is off.
    easy.ssl_verify_peer(true)?;
    let perl_file = env::var_os("PERL_LWP_SSL_CA_FILE");
    let perl_path = env::var_os("PERL_LWP_SSL_CA_PATH");
    let nointernal = options.contains_key("ssl_nointernalca") || options.contains_key("ssl-nointernalca");
    if perl_file.is_none() && perl_path.is_none() && !nointernal {
        // Biber sets PERL_LWP_SSL_CA_FILE to this bundle before LWP reads
        // HTTPS_CA_FILE/HTTPS_CA_DIR compatibility variables.
        easy.ssl_cainfo_blob(CA_ROOTS)?;
        if let Some(path) = env::var_os("HTTPS_CA_DIR") { easy.capath(std::path::Path::new(&path))?; }
        else { easy.capath("")?; }
    } else {
        let fallback = ["/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/ca-bundle.pem"]
            .into_iter().find(|path| std::path::Path::new(path).is_file());
        let ca_file = perl_file.or_else(|| fallback.map(Into::into)).or_else(|| env::var_os("HTTPS_CA_FILE"));
        let ca_path = perl_path.or_else(|| {
            ["/etc/ssl/certs/", "/etc/pki/tls/"].into_iter().find(|path| std::path::Path::new(path).is_dir()).map(Into::into)
        }).or_else(|| env::var_os("HTTPS_CA_DIR"));
        if let Some(path) = ca_file { easy.cainfo(std::path::Path::new(&path))?; }
        if let Some(path) = ca_path { easy.capath(std::path::Path::new(&path))?; }
    }
    Ok(())
}

fn environment_proxy(url: &Url) -> Option<String> {
    let mut variables = env::vars().collect::<Vec<_>>();
    variables.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let cgi = env::var("REQUEST_METHOD").is_ok_and(|value| !value.is_empty() && value != "0");
    let mut proxies = BTreeMap::new();
    for (name, value) in variables {
        if value.is_empty() || value == "0" || (cgi && name.starts_with("HTTP_")) { continue; }
        let name = if cgi && name == "CGI_HTTP_PROXY" { "HTTP_PROXY".to_owned() } else { name };
        let name = name.to_ascii_lowercase();
        if let Some(scheme) = name.strip_suffix("_proxy") { proxies.entry(scheme.to_owned()).or_insert(value); }
    }
    if let (Some(host), Some(domains)) = (url.host_str(), proxies.get("no")) {
        for domain in domains.split(',').map(str::trim).map(|s| s.strip_prefix('.').unwrap_or(s)) {
            if host == domain || host.strip_suffix(domain).is_some_and(|prefix| prefix.ends_with('.')) { return None; }
        }
    }
    // LWP does not interpret ALL_PROXY, wildcard domains, ports or CIDR blocks.
    proxies.remove(url.scheme())
}

/// URI 5.27 permits RFC2396 characters, preserves existing percent escapes and
/// emits UTF-8 octets for escaped Unicode. Brackets are reserved only in hosts.
fn escape_uri(source: &str) -> String {
    escape_uri_encoding(source, true)
}

fn escape_header_uri(source: &[u8]) -> String {
    // HTTP header values are byte strings in Perl, not decoded Unicode.
    let latin1 = source.iter().map(|&b| char::from(b)).collect::<String>();
    escape_uri_encoding(&latin1, false)
}

fn escape_uri_encoding(source: &str, utf8: bool) -> String {
    let authority = source.find("://").map(|start| {
        let start = start + 3;
        let end = source[start..].find(['/', '?', '#']).map_or(source.len(), |n| start + n);
        let host_start = source[start..end].rfind('@').map_or(start, |n| start + n + 1);
        (host_start, end)
    });
    let square_brackets = env::var("URI_HAS_RESERVED_SQUARE_BRACKETS").is_ok_and(|s| !s.is_empty() && s != "0");
    let mut out = String::with_capacity(source.len());
    const HEX: &[u8] = b"0123456789ABCDEF";
    for (index, c) in source.char_indices() {
        let host = authority.is_some_and(|(start, end)| index >= start && index < end);
        if c.is_ascii() && (c.is_ascii_alphanumeric() || b";/?:@&=+$,-_.!~*'()%#".contains(&(c as u8)) || ((host || square_brackets) && b"[]".contains(&(c as u8)))) {
            out.push(c);
        } else {
            let mut buffer = [0; 4];
            let bytes = if utf8 { c.encode_utf8(&mut buffer).as_bytes() } else { buffer[0] = c as u8; &buffer[..1] };
            for &byte in bytes { out.push('%'); out.push(HEX[(byte >> 4) as usize] as char); out.push(HEX[(byte & 15) as usize] as char); }
        }
    }
    // URI::_server applies IDNA before escaping non-ASCII authority characters.
    if let Some((start, end)) = authority {
        if !source[start..end].is_ascii() {
            let authority = &source[start..end];
            let (host, port) = authority.rsplit_once(':').filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
                .map_or((authority, ""), |(host, port)| (host, &authority[host.len()..host.len() + port.len() + 1]));
            if let Some(host) = idna_host(host) {
                let escaped_start = escape_uri_encoding(&source[..start], utf8).len();
                let escaped_end = escaped_start + escape_uri_encoding(authority, utf8).len();
                out.replace_range(escaped_start..escaped_end, &format!("{host}{port}"));
            }
        }
    }
    out
}

/// URI::_idna's nameprep is deliberately only Perl lowercase, not UTS46
/// normalization/mapping. Using Url's IDNA here changes accepted hostnames.
fn idna_host(host: &str) -> Option<String> {
    let mut result = String::new();
    for (index, label) in host.split('.').enumerate() {
        if index > 0 { result.push('.'); }
        if label.is_ascii() { result.push_str(label); continue; }
        let label = label.chars().flat_map(crate::perl_unicode::lower_chars).map(u32::from).collect::<Vec<_>>();
        let mut encoded = String::from("xn--");
        for &c in &label { if c < 128 { encoded.push(char::from_u32(c)?); } }
        let mut handled = encoded.len() - 4;
        let basic = handled;
        if basic > 0 { encoded.push('-'); }
        let (mut n, mut delta, mut bias) = (128u64, 0u64, 72u64);
        while handled < label.len() {
            let next = label.iter().copied().map(u64::from).filter(|&c| c >= n).min()?;
            delta = delta.checked_add((next - n).checked_mul(handled as u64 + 1)?)?;
            n = next;
            for &c in &label {
                let c = u64::from(c);
                if c < n { delta = delta.checked_add(1)?; }
                if c != n { continue; }
                let mut q = delta;
                let mut k = 36u64;
                loop {
                    let t = k.saturating_sub(bias).clamp(1, 26);
                    if q < t { break; }
                    encoded.push(punycode_digit(t + (q - t) % (36 - t)));
                    q = (q - t) / (36 - t);
                    k += 36;
                }
                encoded.push(punycode_digit(q));
                let mut d = if handled == basic { delta / 700 } else { delta / 2 };
                d += d / (handled as u64 + 1);
                let mut k = 0;
                while d > 455 { d /= 35; k += 36; }
                bias = k + 36 * d / (d + 38);
                delta = 0;
                handled += 1;
            }
            delta = delta.checked_add(1)?;
            n += 1;
        }
        if encoded.len() > 63 { return None; }
        result.push_str(&encoded);
    }
    Some(result)
}

fn punycode_digit(value: u64) -> char {
    if value < 26 { (b'a' + value as u8) as char } else { (b'0' + (value - 26) as u8) as char }
}

fn data_uri(uri: &str) -> Vec<u8> {
    let opaque = uri.strip_prefix("data:").unwrap_or(uri).split('#').next().unwrap_or("");
    let (encoding, data) = opaque.split_once(',').unwrap_or((opaque, ""));
    let bytes = percent_decode(data.as_bytes());
    if !encoding.to_ascii_lowercase().ends_with(";base64") { return bytes; }
    // MIME::Base64 accepts whitespace and other non-alphabet characters, and
    // stops at padding; it does not require a final padded quartet.
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut bits = 0u32;
    let mut count = 0u32;
    for byte in bytes {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A', b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52, b'+' => 62, b'/' => 63,
            b'=' => break, _ => continue,
        };
        bits = (bits << 6) | u32::from(value); count += 6;
        if count >= 8 { count -= 8; out.push((bits >> count) as u8); }
    }
    out
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(a), Some(b)) = (hex(input[index + 1]), hex(input[index + 2])) {
                out.push((a * 16 + b) as u8); index += 3; continue;
            }
        }
        out.push(input[index]); index += 1;
    }
    out
}
