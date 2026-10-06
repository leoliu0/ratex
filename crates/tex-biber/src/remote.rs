//! Biber 2.22's `locate_data_file` network branch, without a Perl process.
//!
//! Call the embedding resource resolver before `Cache::fetch`. Responses remain
//! bytes: neither MIME/charset, Content-Disposition nor Content-Encoding changes
//! the datasource format or decoding. Those belong to the selected input driver.
//! Successful downloads are cached by the original, unescaped source name.
use std::{collections::BTreeMap, env, sync::Arc};
use url::Url;
#[path = "remote/protocols.rs"]
mod protocols;
#[path = "remote/uri.rs"]
mod uri;
#[path = "remote/http_status.rs"]
mod http_status;
#[path = "remote/net.rs"]
mod net;
#[path = "remote/tls.rs"]
mod tls;
#[path = "remote/http.rs"]
mod http;
#[path = "remote/ftp.rs"]
mod ftp;

/// The schemes with an LWP::Protocol implementor in the oracle's package.
const LWP_SCHEMES: &[&str] = &["cpan", "data", "file", "ftp", "gopher", "http", "https", "loopback", "mailto", "news", "nntp", "nogo"];

#[derive(Default)]
pub(crate) struct Cache {
    sources: BTreeMap<String, Arc<[u8]>>,
    ssl: Option<tls::SslEnv>,
}

impl Cache {
    /// `None` means this name is not recognized as remote by the oracle. A
    /// recognized URL failing to download is fatal, not a missing-file warning.
    pub(crate) fn fetch(&mut self, source: &str, options: &BTreeMap<String, String>) -> Result<Option<Arc<[u8]>>, String> {
        if !is_remote(source) { return Ok(None); }
        if let Some(bytes) = self.sources.get(source) { return Ok(Some(Arc::clone(bytes))); }
        let ssl = self.ssl.get_or_insert_with(tls::SslEnv::from_env);
        if source.starts_with("https://") || source.starts_with("ftps://") { ssl.apply_biber(options); }
        let bytes: Arc<[u8]> = download(source, ssl)?.into();
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

fn download(source: &str, ssl: &tls::SslEnv) -> Result<Vec<u8>, String> {
    let mut target = escape_uri(source.trim_end());
    // LWP's original request gets a proxy slot only when a proxy is selected;
    // that slot is cloned (and retained) on subsequent redirects.
    let mut proxy = None;
    for redirects in 0..=7 {
        let parsed = Url::parse(&target).map_err(|e| failure(source, &format!("400 {e}")))?;
        let scheme = parsed.scheme();
        if proxy.is_none() { proxy = environment_proxy(&parsed); }
        // LWP::UserAgent::send_request: a proxy's scheme selects the protocol.
        let protocol = proxy.as_deref().map_or_else(|| scheme.to_owned(), |proxy| uri::parts(proxy).scheme.unwrap_or_default().to_ascii_lowercase());
        match protocol.as_str() {
            "data" => {
                if proxy.is_some() { return Err(failure(source, "400 You can not proxy with data")); }
                return Ok(data_uri(&target));
            }
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
            // Only reachable as a proxy's scheme: Biber names and redirects are never file:.
            "file" => return Err(failure(source, "400 You can not proxy through the filesystem")),
            "nogo" => return Err(failure(source, &format!("500 Access to '{scheme}' URIs has been disabled"))),
            "ftp" => {
                if proxy.is_some() { return Err(failure(source, "400 You can not proxy through the ftp")); }
                return ftp::get(&target).map_err(|status| failure(source, &status));
            }
            "http" | "https" => {}
            _ => return Err(failure(source, &format!("501 Protocol scheme '{protocol}' is not supported"))),
        }
        let response = http::get(&target, proxy.as_deref(), ssl).map_err(|status| failure(source, &status))?;
        // LWP::Protocol::collect keeps the status and the bytes read before a
        // body-read error (X-Died); Biber tests only is_success.
        if (200..300).contains(&response.code) { return Ok(response.body); }
        if redirects < 7 && matches!(response.code, 301 | 302 | 303 | 307 | 308) {
            let referral = response.location.as_deref().map(escape_header_uri).unwrap_or_default();
            let base = response.base.as_deref().map(|value| uri::absolute(&escape_header_uri(value), &target, false));
            let next = uri::absolute(&referral, base.as_deref().unwrap_or(&target), true);
            // LWP explicitly disallows file redirects and leaves the 3xx status.
            if !uri::parts(&next).scheme.is_some_and(|scheme| scheme.eq_ignore_ascii_case("file")) { target = next; continue; }
        }
        return Err(failure(source, &response.status));
    }
    unreachable!("the final redirect iteration always returns")
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
    // LWP does not interpret ALL_PROXY, wildcard domains, ports or CIDR blocks,
    // and env_proxy skips schemes without an LWP::Protocol implementor.
    proxies.remove(url.scheme()).filter(|_| LWP_SCHEMES.contains(&url.scheme()))
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
