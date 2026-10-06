use super::{escape_header_uri, http_status};
use curl::easy::{Easy, InfoType};

#[derive(Default)]
pub(super) struct Response {
    pub(super) bytes: Vec<u8>,
    pub(super) status: String,
    pub(super) location: Option<String>,
    pub(super) base: Option<String>,
    pub(super) ftp_error: Option<(u32, String)>,
    pub(super) ftp_cwd: Option<Vec<u8>>,
}

pub(super) fn collect(easy: &mut Easy) -> (Response, Result<(), curl::Error>) {
    let mut response = Response::default();
    let outcome = (|| {
        let Response { bytes, status, location, base, ftp_error, ftp_cwd } = &mut response;
        let mut transfer = easy.transfer();
        transfer.write_function(|data| { bytes.extend_from_slice(data); Ok(data.len()) })?;
        transfer.debug_function(|kind, data| {
            if matches!(kind, InfoType::HeaderOut) {
                if let Some(directory) = data.strip_prefix(b"CWD ") { *ftp_cwd = Some(directory.trim_ascii().to_vec()); }
            }
        })?;
        transfer.header_function(|header| {
            let line = header.trim_ascii();
            if line.starts_with(b"HTTP/") {
                let value = line.iter().position(|&b| b == b' ').map_or(&[][..], |space| &line[space + 1..]);
                *status = value.iter().map(|&b| char::from(b)).collect();
                let (code, reason) = status.split_once(' ').unwrap_or((status, ""));
                if reason.is_empty() || reason == "0" {
                    let code = code.parse().unwrap_or(0);
                    *status = format!("{code} {}", http_status::reason(code));
                }
                *location = None;
                *base = None;
            } else if let Some(colon) = line.iter().position(|&b| b == b':') {
                let (name, value) = (&line[..colon], line[colon + 1..].trim_ascii());
                if name.eq_ignore_ascii_case(b"location") { *location = Some(escape_header_uri(value)); }
                if name.eq_ignore_ascii_case(b"content-base") { *base = Some(escape_header_uri(value)); }
            } else if line.len() >= 4 && line[..3].iter().all(u8::is_ascii_digit) {
                let code = u32::from(line[0] - b'0') * 100 + u32::from(line[1] - b'0') * 10 + u32::from(line[2] - b'0');
                if code >= 400 { *ftp_error = Some((code, line[4..].iter().map(|&b| char::from(b)).collect())); }
            }
            true
        })?;
        transfer.perform()
    })();
    (response, outcome)
}
