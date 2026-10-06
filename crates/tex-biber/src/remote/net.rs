//! Sockets shared by the LWP and libnet ports. Error wording is glibc's
//! strerror text, as the Linux oracle reports it, on every host platform.
use std::{io::{self, Read, Write}, net::{SocketAddr, TcpStream, ToSocketAddrs}, time::Duration};

/// LWP::UserAgent's default timeout, passed to every socket operation.
pub(super) const TIMEOUT: Duration = Duration::from_secs(180);

pub(super) enum ConnectError { Resolve, Io(io::Error) }

/// IO::Socket::IP tries each resolved address in order; the last failure wins.
pub(super) fn connect(host: &str, port: u16) -> Result<TcpStream, ConnectError> {
    let addresses = (host, port).to_socket_addrs().map_err(|_| ConnectError::Resolve)?;
    let mut error = None;
    for address in addresses {
        match connect_address(&address) {
            Ok(stream) => return Ok(stream),
            Err(e) => error = Some(e),
        }
    }
    Err(error.map_or(ConnectError::Resolve, ConnectError::Io))
}

pub(super) fn connect_address(address: &SocketAddr) -> io::Result<TcpStream> {
    let stream = TcpStream::connect_timeout(address, TIMEOUT)?;
    configure(&stream)?;
    Ok(stream)
}

pub(super) fn configure(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))
}

/// A socket read or write that ran out of time (EAGAIN on Unix, WSAETIMEDOUT on Windows).
pub(super) fn is_timeout(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

/// Perl's `"$!"` for a socket error.
pub(super) fn reason(error: &io::Error) -> String {
    use io::ErrorKind::*;
    let text = match error.kind() {
        NotFound => "No such file or directory",
        ConnectionRefused => "Connection refused",
        ConnectionReset => "Connection reset by peer",
        ConnectionAborted => "Software caused connection abort",
        NotConnected => "Transport endpoint is not connected",
        AddrInUse => "Address already in use",
        AddrNotAvailable => "Cannot assign requested address",
        NetworkUnreachable => "Network is unreachable",
        HostUnreachable => "No route to host",
        NetworkDown => "Network is down",
        BrokenPipe => "Broken pipe",
        TimedOut | WouldBlock => "Connection timed out",
        PermissionDenied => "Permission denied",
        Interrupted => "Interrupted system call",
        _ => {
            let mut text = error.to_string();
            if let Some(index) = text.find(" (os error ") { text.truncate(index); }
            return text;
        }
    };
    text.to_owned()
}

/// A plain or TLS connection; TLS may itself run over a CONNECT tunnel.
pub(super) enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, Stream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(stream) => stream.read(buf),
            // IO::Socket::SSL reports a peer close without close_notify as EOF.
            Stream::Tls(stream) => match stream.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
                result => result,
            },
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self { Stream::Plain(stream) => stream.write(buf), Stream::Tls(stream) => stream.write(buf) }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self { Stream::Plain(stream) => stream.flush(), Stream::Tls(stream) => stream.flush() }
    }
}
