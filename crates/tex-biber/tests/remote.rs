//! Remote datasource paths against deterministic local HTTP, FTP, NNTP and Gopher
//! servers: no oracle and no outside network. The expected outputs in
//! tests/fixtures/remote were recorded from Biber 2.22 against the same servers.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{channel, Sender};

fn fixture(name: &str) -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/remote").join(name) }

/// Accepts connections forever on a loopback port, one thread per connection.
fn serve(handler: impl Fn(TcpStream) + Send + Sync + 'static) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handler = Arc::clone(&handler);
            std::thread::spawn(move || handler(stream));
        }
    });
    port
}

fn read_line(reader: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    (reader.read_line(&mut line).ok()? > 0).then(|| line.trim_end_matches(['\r', '\n']).to_owned())
}

/// HTTP/1.0 like Python's BaseHTTPRequestHandler: one response, then close.
fn http(body: Arc<Vec<u8>>, seen: Sender<String>, redirects: Vec<(String, String)>) -> u16 {
    serve(move |stream| {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let Some(request) = read_line(&mut reader) else { return };
        let path = request.split(' ').nth(1).unwrap_or_default().to_owned();
        let mut headers = Vec::new();
        while let Some(line) = read_line(&mut reader).filter(|line| !line.is_empty()) {
            if let Some((key, value)) = line.split_once(':') {
                headers.push((key.trim().to_ascii_lowercase(), value.trim().to_owned()));
            }
        }
        let _ = seen.send(path.clone());
        let header = |key: &str| headers.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        let mut out = stream;
        let response = if header("user-agent") != Some("Mozilla/5.0") || header("zotero-allowed-request") != Some("1") {
            "HTTP/1.0 403 Datasource authorization headers required\r\nContent-Length: 0\r\n\r\n".to_owned()
        } else if let Some((_, location)) = redirects.iter().find(|(from, _)| *from == path) {
            format!("HTTP/1.0 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
        } else if path == "/missing" {
            "HTTP/1.0 404 Fixture absent\r\nContent-Length: 0\r\n\r\n".to_owned()
        } else {
            // MIME type and filename deliberately disagree with the requested input driver.
            let length = body.len() + if path == "/truncated" { 100 } else { 0 };
            let head = format!("HTTP/1.0 200 OK\r\nContent-Type: application/json; charset=ISO-8859-1\r\nContent-Disposition: attachment; filename=\"wrong.json\"\r\nContent-Length: {length}\r\n\r\n");
            let _ = out.write_all(head.as_bytes());
            let _ = out.write_all(&body);
            return;
        };
        let _ = out.write_all(response.as_bytes());
    })
}

fn ftp(body: Arc<Vec<u8>>) -> u16 {
    serve(move |stream| {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut out = stream;
        let mut send = |line: &str| { let _ = out.write_all(format!("{line}\r\n").as_bytes()); };
        let mut passive: Option<TcpListener> = None;
        send("220 Fixture ready");
        while let Some(line) = read_line(&mut reader).filter(|line| !line.is_empty()) {
            let (command, value) = line.split_once(' ').unwrap_or((&line, ""));
            match command {
                "USER" => send("331 Password required"),
                "PASS" => send("230 Logged in"),
                "PWD" => send("257 \"/\" is current directory"),
                "CWD" => send(if value == "missing.bib" { "550 No directory" } else { "250 Directory changed" }),
                "TYPE" => send("200 Transfer type accepted"),
                "SYST" => send("215 UNIX Type: L8"),
                "MDTM" => send("213 20250101000000"),
                "SIZE" => send(&format!("213 {}", body.len())),
                "PASV" | "EPSV" => {
                    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                    let port = listener.local_addr().unwrap().port();
                    passive = Some(listener);
                    send(&if command == "EPSV" { format!("229 Entering Extended Passive Mode (|||{port}|)") } else { format!("227 Entering Passive Mode (127,0,0,1,{},{})", port / 256, port % 256) });
                }
                "RETR" if value == "missing.bib" => send("550 File not found"),
                "RETR" => {
                    send("150 Opening data connection");
                    if let Some(listener) = passive.take() {
                        let (mut connection, _) = listener.accept().unwrap();
                        let _ = connection.write_all(&body);
                    }
                    send("226 Transfer complete");
                }
                "QUIT" => { send("221 Goodbye"); break; }
                _ => send("200 Command accepted"),
            }
        }
    })
}

fn nntp(body: Arc<Vec<u8>>) -> u16 {
    serve(move |stream| {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut out = stream;
        let _ = out.write_all(b"200 fixture ready\r\n");
        while let Some(line) = read_line(&mut reader) {
            let reply: Vec<u8> = if line == "MODE READER" {
                b"200 fixture ready\r\n".to_vec()
            } else if line.starts_with("ARTICLE ") {
                let mut article = b"220 Article follows\r\nContent-Type: application/x-bibtex\r\n\r\n".to_vec();
                for &byte in body.iter() {
                    if byte == b'\n' { article.push(b'\r'); }
                    article.push(byte);
                }
                article.extend_from_slice(b".\r\n");
                article
            } else if line.starts_with("GROUP ") {
                b"211 1 1 1 fixture\r\n".to_vec()
            } else if line == "QUIT" {
                let _ = out.write_all(b"205 Goodbye\r\n");
                break;
            } else {
                b"500 Unknown command\r\n".to_vec()
            };
            let _ = out.write_all(&reply);
        }
    })
}

fn gopher(body: Arc<Vec<u8>>) -> u16 {
    serve(move |mut stream| {
        let _ = stream.read(&mut [0; 4096]);
        let _ = stream.write_all(&body);
    })
}

struct Work(PathBuf);
impl Drop for Work {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn run(root: &Path, arguments: &[&str], env: &[(&str, String)], status: i32) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_biber"));
    command.args(arguments).current_dir(root);
    for (key, _) in std::env::vars().filter(|(key, _)| key.to_ascii_lowercase().ends_with("_proxy")) {
        command.env_remove(key);
    }
    command.envs(env.iter().map(|(k, v)| (k, v)));
    let output = command.output().unwrap();
    let text = String::from_utf8_lossy(&[output.stdout, output.stderr].concat()).into_owned();
    assert_eq!(output.status.code(), Some(status), "{arguments:?}\n{text}");
    text
}

#[test]
fn remote_datasources() {
    let body = Arc::new(std::fs::read(fixture("refs.bib")).unwrap());
    let expected = std::fs::read(fixture("expected.bib")).unwrap();
    let (ftp_port, nntp_port, gopher_port) = (ftp(Arc::clone(&body)), nntp(Arc::clone(&body)), gopher(Arc::clone(&body)));
    let work = Work(std::env::temp_dir().join(format!("tex-biber-remote-{}", std::process::id())));
    std::fs::create_dir_all(&work.0).unwrap();
    let root = work.0.as_path();
    let data = {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in body.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
            for i in 0..4 {
                out.push(if i <= chunk.len() { ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        format!("data:text/plain;base64,{out}")
    };
    let redirects = [
        ("/redirect", "/odd%20name.bib".to_owned()), ("/loop", "/loop".to_owned()), ("/data", data),
        ("/nntp", "nntp:article@fixture".to_owned()), ("/gopher", format!("gopher://127.0.0.1:{gopher_port}/0fixture")),
        ("/cpan", "cpan:authors/id/fixture.bib".to_owned()), ("/loopback", "loopback:fixture".to_owned()), ("/file", "file:///not-readable".to_owned()),
    ].map(|(from, to)| (from.to_owned(), to)).to_vec();
    let (sender, requests) = channel();
    let http_port = http(Arc::clone(&body), sender, redirects);
    let base = format!("http://127.0.0.1:{http_port}");
    // Each server thread records a request before answering it, so a finished run's requests are all queued.
    let fetched = || requests.try_iter().collect::<Vec<String>>();

    // One download is shared by both refsections of the cached BBL.
    let url = format!("{base}/odd name é [1].bib?x=a b#ignored");
    let escaped = url.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let bcf = std::fs::read_to_string(fixture("main.bcf")).unwrap().replace(">refs.bib</bcf:datasource>", &format!(">{escaped}</bcf:datasource>"));
    std::fs::write(root.join("main.bcf"), bcf).unwrap();
    let _ = fetched();
    run(root, &["main.bcf"], &[], 0);
    assert_eq!(std::fs::read(root.join("main.bbl")).unwrap(), std::fs::read(fixture("expected.bbl")).unwrap(), "cached two-section BBL");
    assert_eq!(fetched(), ["/odd%20name%20%C3%A9%20%5B1%5D.bib?x=a%20b"], "one download shared across refsections");

    let output = root.join("converted.bib");
    let tool = |source: &str, wanted: &[u8], env: &[(&str, String)]| {
        run(root, &["--tool", &format!("--output-file={}", output.display()), &format!("--logfile={}", root.join("tool.blg").display()), source], env, 0);
        assert_eq!(std::fs::read(&output).unwrap(), wanted, "{source}");
    };
    tool(&url, &expected, &[]);
    tool(&format!("{base}/redirect"), &expected, &[]);
    tool(&format!("{base}/data"), &expected, &[]);
    tool(&format!("{base}/truncated"), &expected, &[]);
    tool(&format!("ftp://127.0.0.1:{ftp_port}/fixture.bib"), &expected, &[]);
    tool(&format!("{base}/nntp"), &expected, &[("NNTPSERVER", format!("127.0.0.1:{nntp_port}"))]);
    tool(&format!("{base}/gopher"), &expected, &[]);
    tool(&format!("{base}/cpan"), &expected, &[("http_proxy", base.clone()), ("no_proxy", "127.0.0.1".to_owned())]);
    tool(&format!("{base}/loopback"), &std::fs::read(fixture("expected-loopback.bib")).unwrap(), &[]);

    for (source, message, fetches) in [
        (format!("{base}/missing"), "404 Fixture absent", 1),
        (format!("{base}/loop"), "302 Found", 8),
        (format!("{base}/file"), "302 Found", 1),
        (format!("ftps://127.0.0.1:{ftp_port}/fixture.bib"), "501 Protocol scheme 'ftps' is not supported", 0),
        (format!("ftp://127.0.0.1:{ftp_port}/missing.bib"), "404 File 'missing.bib' not found", 0),
    ] {
        let _ = fetched();
        let text = run(root, &["--tool", &format!("--output-file={}", root.join("error.bib").display()), &format!("--logfile={}", root.join("error.blg").display()), &source], &[], 2);
        assert!(text.contains(&format!("Could not fetch '{source}' (HTTP error: {message})")), "{text}");
        assert_eq!(fetched().len(), fetches, "{source}");
    }
}
