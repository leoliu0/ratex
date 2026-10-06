//! LWP::Protocol::https over IO::Socket::SSL, verified with rustls. The CA
//! selection follows Biber's %ENV edits, LWP::UserAgent's ssl_opts defaults
//! and IO::Socket::SSL's default_ca, in that order.
use super::net::Stream;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{ring, verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme, StreamOwned};
use std::{collections::BTreeMap, env, ffi::{OsStr, OsString}, path::Path, sync::Arc};

/// Mozilla::CA's bundle from the oracle's package.
const CA_ROOTS: &[u8] = include_bytes!("../remote-ca.pem");

#[derive(Clone)]
enum CaFile { Internal, Path(OsString) }

/// The %ENV entries Biber edits before requiring LWP::Protocol::https. Biber
/// changes its own process environment, so later datasources see them too.
pub(super) struct SslEnv {
    ca_file: Option<CaFile>,
    ca_path: Option<OsString>,
    verify_hostname: Option<OsString>,
}

fn perl_true(value: &OsStr) -> bool { !value.is_empty() && value != "0" }

fn env_true(name: &str) -> Option<OsString> { env::var_os(name).filter(|value| perl_true(value)) }

impl SslEnv {
    pub(super) fn from_env() -> Self {
        SslEnv {
            ca_file: env::var_os("PERL_LWP_SSL_CA_FILE").map(CaFile::Path),
            ca_path: env::var_os("PERL_LWP_SSL_CA_PATH"),
            verify_hostname: env::var_os("PERL_LWP_SSL_VERIFY_HOSTNAME"),
        }
    }

    /// Biber::Utils::locate_data_file for an https:// or ftps:// datasource.
    pub(super) fn apply_biber(&mut self, options: &BTreeMap<String, String>) {
        let option = |name: &str| options.contains_key(name) || options.contains_key(&name.replace('-', "_"));
        if self.ca_file.is_none() && self.ca_path.is_none() && !option("ssl-nointernalca") {
            self.ca_file = Some(CaFile::Internal);
        }
        if self.ca_file.is_none() {
            let bundle = ["/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/ca-bundle.pem"]
                .into_iter().find(|path| Path::new(path).exists());
            if let Some(bundle) = bundle { self.ca_file = Some(CaFile::Path(bundle.into())); }
            let directory = ["/etc/ssl/certs/", "/etc/pki/tls/"].into_iter().find(|path| Path::new(path).is_dir());
            if let Some(directory) = directory { self.ca_path = Some(directory.into()); }
        }
        if option("ssl-noverify-host") { self.verify_hostname = Some("0".into()); }
    }

    /// IO::Socket::SSL's client context for LWP's ssl_opts.
    pub(super) fn config(&self) -> Result<Arc<ClientConfig>, ContextError> {
        let https_ca_file = env_true("HTTPS_CA_FILE");
        let https_ca_dir = env_true("HTTPS_CA_DIR");
        let verify_hostname = match &self.verify_hostname {
            Some(value) => perl_true(value),
            None => https_ca_file.is_none() && https_ca_dir.is_none(),
        };
        let ca_file = match &self.ca_file {
            Some(CaFile::Path(path)) if !perl_true(path) => None,
            Some(file) => Some(file.clone()),
            None => None,
        }.or_else(|| https_ca_file.map(CaFile::Path));
        let ca_path = self.ca_path.clone().filter(|path| perl_true(path)).or(https_ca_dir);
        check_locations(ca_file.as_ref(), ca_path.as_deref())?;
        let mut roots = RootCertStore::empty();
        if ca_file.is_some() || ca_path.is_some() {
            let loaded = ca_file.as_ref().is_none_or(|file| add_file(&mut roots, file));
            if let Some(path) = &ca_path {
                for directory in std::env::split_paths(path) { add_directory(&mut roots, &directory); }
            }
            // OpenSSL rejects an unusable CA file even when the directory works.
            if !loaded { return Err(ContextError::Error("Invalid certificate authority locations".into())); }
        } else {
            // default_ca: SSL_CERT_DIR/SSL_CERT_FILE or OpenSSL's own locations,
            // then Mozilla::CA, which is always packaged with the oracle.
            let directory = env_true("SSL_CERT_DIR").unwrap_or_else(|| "/etc/ssl/certs".into());
            add_directory(&mut roots, Path::new(&directory));
            let file = env_true("SSL_CERT_FILE").unwrap_or_else(|| "/etc/ssl/cert.pem".into());
            add_file(&mut roots, &CaFile::Path(file));
            if roots.is_empty() { add_file(&mut roots, &CaFile::Internal); }
        }
        let provider = Arc::new(ring::default_provider());
        let inner = if roots.is_empty() { None } else {
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider)).build().ok()
        };
        let verifier = Verifier { inner, verify_hostname, provider: Arc::clone(&provider) };
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions().map_err(|e| ContextError::Error(e.to_string()))?
            .dangerous().with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        Ok(Arc::new(config))
    }
}

/// Why IO::Socket::SSL could not build its context.
pub(super) enum ContextError {
    /// A die from the CA location checks; LWP reports the message itself.
    Die(String),
    /// An error return, reported like any other failed connect.
    Error(String),
}

/// IO::Socket::SSL's `$CHECK_SSL_PATH`, run before connecting whenever the
/// peer is verified (always here). Perl can open a directory for reading.
fn check_locations(ca_file: Option<&CaFile>, ca_path: Option<&OsStr>) -> Result<(), ContextError> {
    if let Some(CaFile::Path(file)) = ca_file {
        if let Err(error) = std::fs::metadata(file).and_then(|metadata| if metadata.is_dir() { Ok(()) } else { std::fs::File::open(file).map(drop) }) {
            return Err(ContextError::Die(format!("SSL_ca_file {} can't be used: {}", file.to_string_lossy(), super::net::reason(&error))));
        }
    }
    let Some(path) = ca_path else { return Ok(()) };
    let mut errors = Vec::new();
    for directory in std::env::split_paths(path) {
        if !directory.is_dir() {
            errors.push(format!("SSL_ca_path {} does not exist", directory.display()));
        } else if let Err(error) = std::fs::read_dir(&directory) {
            errors.push(format!("SSL_ca_path {} is not accessible: {}", directory.display(), super::net::reason(&error)));
        } else {
            errors.clear();
            break;
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(ContextError::Die(errors.join(" "))) }
}

fn add_pem(roots: &mut RootCertStore, bytes: &[u8]) -> bool {
    let (added, _) = roots.add_parsable_certificates(CertificateDer::pem_slice_iter(bytes).filter_map(Result::ok));
    added > 0
}

fn add_file(roots: &mut RootCertStore, file: &CaFile) -> bool {
    match file {
        CaFile::Internal => add_pem(roots, CA_ROOTS),
        CaFile::Path(path) => std::fs::read(path).is_ok_and(|bytes| add_pem(roots, &bytes)),
    }
}

/// OpenSSL's CApath lookup only reaches subject-hash names such as `5ad8a5d6.0`.
fn add_directory(roots: &mut RootCertStore, directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    let mut names = entries.flatten().map(|entry| entry.file_name()).filter(|name| {
        name.to_str().and_then(|name| name.split_once('.')).is_some_and(|(hash, index)| {
            hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit())
        })
    }).collect::<Vec<_>>();
    names.sort();
    for name in names {
        if let Ok(bytes) = std::fs::read(directory.join(name)) { add_pem(roots, &bytes); }
    }
}

/// IO::Socket::SSL always verifies the chain; LWP's verify_hostname only
/// decides whether the certificate must also name the host.
#[derive(Debug)]
struct Verifier {
    inner: Option<Arc<WebPkiServerVerifier>>,
    verify_hostname: bool,
    provider: Arc<CryptoProvider>,
}

fn name_mismatch(error: &rustls::Error) -> bool {
    matches!(error, rustls::Error::InvalidCertificate(CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }))
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>], server_name: &ServerName<'_>,
        ocsp_response: &[u8], now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let Some(inner) = &self.inner else { return Err(rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer)) };
        match inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now) {
            Err(error) if !self.verify_hostname && name_mismatch(&error) => Ok(ServerCertVerified::assertion()),
            result => result,
        }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

pub(super) enum Failure { Certificate, Hostname, Handshake }

impl Failure {
    /// `$@` of a direct connect, after LWP's "certificate verify failed" extraction.
    pub(super) fn connect_reason(&self) -> &'static str {
        match self {
            Failure::Certificate => "certificate verify failed",
            Failure::Hostname => "hostname verification failed",
            Failure::Handshake => "SSL connect attempt failed because of handshake problems",
        }
    }

    /// IO::Socket::SSL's errstr after start_SSL on a CONNECT tunnel; the oracle
    /// links OpenSSL 1.1, whose error string this reproduces.
    pub(super) fn upgrade_reason(&self) -> &'static str {
        match self {
            Failure::Certificate => "SSL connect attempt failed error:14090086:SSL routines:ssl3_get_server_certificate:certificate verify failed",
            failure => failure.connect_reason(),
        }
    }
}

pub(super) fn handshake(stream: Stream, host: &str, config: Arc<ClientConfig>) -> Result<Stream, Failure> {
    // IO::Socket::SSL sends SNI only for names, as does rustls; a name rustls
    // cannot represent can still pass when hostname verification is off.
    let name = ServerName::try_from(host.to_owned()).unwrap_or_else(|_| ServerName::try_from("invalid.invalid").expect("valid DNS name"));
    let mut connection = ClientConnection::new(config, name).map_err(|_| Failure::Handshake)?;
    let mut stream = stream;
    while connection.is_handshaking() {
        if let Err(error) = connection.complete_io(&mut stream) {
            let tls = error.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>());
            return Err(match tls {
                Some(error) if name_mismatch(error) => Failure::Hostname,
                Some(rustls::Error::InvalidCertificate(_) | rustls::Error::InvalidCertRevocationList(_)) => Failure::Certificate,
                _ => Failure::Handshake,
            });
        }
    }
    Ok(Stream::Tls(Box::new(StreamOwned::new(connection, stream))))
}
