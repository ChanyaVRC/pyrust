//! Cross-process OS IPC primitives for `multiprocessing.connection` (`Pipe`,
//! `Connection`, `Listener`, `Client`, `wait`, `deliver_challenge`,
//! `answer_challenge`).
//!
//! Uses loopback TCP streams (`TCP_NODELAY`) with 4-byte big-endian length
//! framing so `Connection` works identically on Windows, Linux, and macOS.
//! When a `Connection` is pickled for a spawned child process, it registers a
//! full-duplex relay on [`super::broker`] and serializes a compact reconnect
//! token, guaranteeing zero race conditions and clean EOF propagation.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::broker::{BrokerEndpoint, register_pipe_relay};
use super::{SHUTDOWN_POLL_SLICE, call_pos, random_bytes};
use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, current_thread_is_daemon_worker};
use crate::value::{Value, ValueKind};

pub(crate) const CHALLENGE: &[u8] = b"#CHALLENGE#";
pub(crate) const WELCOME: &[u8] = b"#WELCOME#";
pub(crate) const FAILURE: &[u8] = b"#FAILURE#";
pub(crate) const MESSAGE_LENGTH: usize = 20;

/// Payload transferred to the broker for a one-shot full-duplex pipe relay.
pub(crate) struct PipeRelayTarget {
    pub stream: TcpStream,
}

/// Serve a full-duplex byte relay between a child's broker connection and the
/// original local pipe stream. Runs on pure-Rust OS threads (never touches GIL).
pub(crate) fn serve_pipe_relay(child_stream: TcpStream, target: PipeRelayTarget) {
    let local_stream = target.stream;
    let Ok(mut child_r) = child_stream.try_clone() else {
        return;
    };
    let mut child_w = child_stream;
    let Ok(mut local_r) = local_stream.try_clone() else {
        return;
    };
    let mut local_w = local_stream;

    let t = thread::Builder::new()
        .name("pyrust-mp-pipe-c2l".to_string())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match child_r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if local_w.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = local_w.shutdown(Shutdown::Write);
        });

    let mut buf = [0u8; 8192];
    loop {
        match local_r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if child_w.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = child_w.shutdown(Shutdown::Write);
    if let Ok(jh) = t {
        let _ = jh.join();
    }
}

/// Shared state for a `multiprocessing.connection.Connection`.
pub(crate) struct ConnectionCore {
    /// Separate read/write mutexes so concurrent send and recv from two threads
    /// never block each other.
    reader: Mutex<Option<TcpStream>>,
    writer: Mutex<Option<TcpStream>>,
    /// Set to true if this endpoint was handed off to a broker relay via
    /// `__reduce__`, so parent `.close()` doesn't kill the relayed stream.
    transferred: Mutex<bool>,
    readable: bool,
    writable: bool,
}

impl ConnectionCore {
    pub(crate) fn from_stream(
        stream: TcpStream,
        readable: bool,
        writable: bool,
    ) -> io::Result<Arc<Self>> {
        let _ = stream.set_nodelay(true);
        let reader_stream = if readable {
            Some(stream.try_clone()?)
        } else {
            None
        };
        let writer_stream = if writable { Some(stream) } else { None };
        Ok(Arc::new(Self {
            reader: Mutex::new(reader_stream),
            writer: Mutex::new(writer_stream),
            transferred: Mutex::new(false),
            readable,
            writable,
        }))
    }

    /// Create a connected loopback TCP pair for `Pipe(duplex)`.
    pub(crate) fn make_pipe(duplex: bool) -> io::Result<(Arc<Self>, Arc<Self>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let s1 = TcpStream::connect(addr)?;
        let (s2, _) = listener.accept()?;
        let _ = s1.set_nodelay(true);
        let _ = s2.set_nodelay(true);
        if duplex {
            Ok((
                Self::from_stream(s1, true, true)?,
                Self::from_stream(s2, true, true)?,
            ))
        } else {
            // conn1 is recv-only, conn2 is send-only (matching CPython 3.12).
            let _ = s1.shutdown(Shutdown::Write);
            let _ = s2.shutdown(Shutdown::Read);
            Ok((
                Self::from_stream(s1, true, false)?,
                Self::from_stream(s2, false, true)?,
            ))
        }
    }

    pub(crate) fn readable(&self) -> bool {
        self.readable
    }

    pub(crate) fn writable(&self) -> bool {
        self.writable
    }

    pub(crate) fn is_closed(&self) -> bool {
        let r_closed = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none();
        let w_closed = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none();
        r_closed && w_closed
    }

    pub(crate) fn close(&self) {
        let was_transferred = *self
            .transferred
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let r = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let w = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if !was_transferred {
            if let Some(s) = w {
                let _ = s.shutdown(Shutdown::Both);
            } else if let Some(s) = r {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
    }

    pub(crate) fn fileno(&self) -> Result<i64> {
        let r = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(ref s) = *r {
            return Ok(stream_raw_handle(s));
        }
        drop(r);
        let w = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(ref s) = *w {
            return Ok(stream_raw_handle(s));
        }
        Err(PyError::named("OSError", "handle is closed"))
    }

    /// Export this connection through the local broker so a spawned child
    /// process can unpickle it and communicate seamlessly with the peer.
    pub(crate) fn export_for_pickle(&self) -> Result<BrokerEndpoint> {
        let mut r_guard = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut w_guard = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stream = if let Some(s) = r_guard.take() {
            // Also drop our local writer clone so the relay thread holds the
            // sole handle to this end of the socket pair.
            let _ = w_guard.take();
            s
        } else if let Some(s) = w_guard.take() {
            s
        } else {
            return Err(PyError::named("OSError", "handle is closed"));
        };
        *self
            .transferred
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        register_pipe_relay(PipeRelayTarget { stream })
            .map_err(|e| PyError::from_io_error(&e, None))
    }

    /// Reconstruct a `ConnectionCore` in a child process from a broker endpoint.
    pub(crate) fn connect_from_endpoint(
        endpoint: &BrokerEndpoint,
        readable: bool,
        writable: bool,
    ) -> Result<Arc<Self>> {
        let stream = endpoint
            .connect()
            .map_err(|e| PyError::from_io_error(&e, None))?;
        Self::from_stream(stream, readable, writable).map_err(|e| PyError::from_io_error(&e, None))
    }

    /// Send a raw framed byte message (releasing GIL during socket write).
    pub(crate) fn send_bytes_raw(&self, interp: &mut Interpreter, payload: &[u8]) -> Result<()> {
        if !self.writable {
            return Err(PyError::named("OSError", "connection is read-only"));
        }
        if self.is_closed() {
            return Err(PyError::named("OSError", "handle is closed"));
        }
        let len = u32::try_from(payload.len())
            .map_err(|_| PyError::named("ValueError", "buffer is too large"))?;
        let hdr = len.to_be_bytes();
        let res: io::Result<()> = interp.release_gil_during(|| {
            let mut guard = self
                .writer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(ref mut stream) = *guard else {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "handle is closed",
                ));
            };
            stream.write_all(&hdr)?;
            if !payload.is_empty() {
                stream.write_all(payload)?;
            }
            stream.flush()
        });
        res.map_err(|e| PyError::from_io_error(&e, None))
    }

    /// Receive a raw framed byte message (releasing GIL while blocked).
    /// Returns `Ok(None)` on clean EOF.
    pub(crate) fn recv_bytes_raw(&self, interp: &mut Interpreter) -> Result<Option<Vec<u8>>> {
        if !self.readable {
            return Err(PyError::named("OSError", "connection is write-only"));
        }
        if self.is_closed() {
            return Err(PyError::named("OSError", "handle is closed"));
        }
        let gil = Arc::clone(&interp.cold.gil_state);
        let res: io::Result<Option<Vec<u8>>> = interp.release_gil_during(|| {
            let mut guard = self
                .reader
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(ref mut stream) = *guard else {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "handle is closed",
                ));
            };
            let _ = stream.set_read_timeout(Some(SHUTDOWN_POLL_SLICE));
            let mut hdr = [0u8; 4];
            let mut hdr_read = 0usize;
            while hdr_read < 4 {
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    return Ok(None);
                }
                match stream.read(&mut hdr[hdr_read..]) {
                    Ok(0) => {
                        if hdr_read == 0 {
                            return Ok(None);
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "unexpected EOF reading header",
                        ));
                    }
                    Ok(n) => hdr_read += n,
                    Err(ref e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut
                            || e.kind() == io::ErrorKind::Interrupted =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            let len = u32::from_be_bytes(hdr) as usize;
            let mut buf = vec![0u8; len];
            let mut body_read = 0usize;
            while body_read < len {
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    return Ok(None);
                }
                match stream.read(&mut buf[body_read..]) {
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "unexpected EOF reading body",
                        ));
                    }
                    Ok(n) => body_read += n,
                    Err(ref e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut
                            || e.kind() == io::ErrorKind::Interrupted =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            let _ = stream.set_read_timeout(None);
            Ok(Some(buf))
        });
        res.map_err(|e| PyError::from_io_error(&e, None))
    }

    /// Check whether data (or EOF) is immediately readable within `timeout`
    /// (`None` = wait indefinitely, `Some(Duration::ZERO)` = non-blocking poll).
    pub(crate) fn poll(&self, interp: &mut Interpreter, timeout: Option<Duration>) -> Result<bool> {
        if !self.readable {
            return Err(PyError::named("OSError", "connection is write-only"));
        }
        if self.is_closed() {
            return Err(PyError::named("OSError", "handle is closed"));
        }
        let gil = Arc::clone(&interp.cold.gil_state);
        let deadline = timeout.map(|d| Instant::now() + d);
        let ready: io::Result<bool> = interp.release_gil_during(|| {
            let mut guard = self
                .reader
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(ref mut stream) = *guard else {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "handle is closed",
                ));
            };
            let mut peek_buf = [0u8; 1];
            loop {
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    return Ok(false);
                }
                let slice = match deadline {
                    Some(dl) => {
                        let now = Instant::now();
                        if now >= dl {
                            // Final non-blocking peek check.
                            let _ = stream.set_nonblocking(true);
                            let r = stream.peek(&mut peek_buf);
                            let _ = stream.set_nonblocking(false);
                            return match r {
                                Ok(_) => Ok(true),
                                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
                                Err(_) => Ok(true),
                            };
                        }
                        (dl - now).min(SHUTDOWN_POLL_SLICE)
                    }
                    None => SHUTDOWN_POLL_SLICE,
                };
                if slice.is_zero() {
                    let _ = stream.set_nonblocking(true);
                    let r = stream.peek(&mut peek_buf);
                    let _ = stream.set_nonblocking(false);
                    return match r {
                        Ok(_) => Ok(true),
                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
                        Err(_) => Ok(true),
                    };
                }
                let _ = stream.set_read_timeout(Some(slice));
                match stream.peek(&mut peek_buf) {
                    Ok(_) => {
                        let _ = stream.set_read_timeout(None);
                        return Ok(true);
                    }
                    Err(ref e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut
                            || e.kind() == io::ErrorKind::Interrupted =>
                    {
                        if let Some(dl) = deadline
                            && Instant::now() >= dl
                        {
                            let _ = stream.set_read_timeout(None);
                            return Ok(false);
                        }
                    }
                    Err(_) => {
                        // Peer reset/closed counts as readable (next recv raises EOFError/OSError).
                        let _ = stream.set_read_timeout(None);
                        return Ok(true);
                    }
                }
            }
        });
        ready.map_err(|e| PyError::from_io_error(&e, None))
    }

    /// Non-blocking poll without releasing the GIL (used by `connection.wait`).
    pub(crate) fn try_poll_now(&self) -> bool {
        let Ok(guard) = self.reader.try_lock() else {
            return false;
        };
        let Some(ref stream) = *guard else {
            return true;
        };
        let mut peek_buf = [0u8; 1];
        let _ = stream.set_nonblocking(true);
        let r = stream.peek(&mut peek_buf);
        let _ = stream.set_nonblocking(false);
        !matches!(r, Err(ref e) if e.kind() == io::ErrorKind::WouldBlock)
    }
}

fn stream_raw_handle(stream: &TcpStream) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        stream.as_raw_fd() as i64
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        stream.as_raw_socket() as i64
    }
}

/// Listener backing `multiprocessing.connection.Listener` (loopback TCP for
/// `AF_INET`, plus temp-file-backed address rendezvous for arbitrary / pipe
/// addresses so `Listener()` with no arguments works cross-platform).
pub(crate) struct ListenerCore {
    listener: Mutex<Option<TcpListener>>,
    pub address: ListenerAddress,
    pub authkey: Option<Vec<u8>>,
    pub last_accepted: Mutex<Option<String>>,
    temp_addr_file: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) enum ListenerAddress {
    Inet(String, u16),
    Named(String),
}

impl ListenerCore {
    pub(crate) fn bind(
        address: Option<ListenerAddress>,
        family: Option<String>,
        authkey: Option<Vec<u8>>,
    ) -> Result<Arc<Self>> {
        let fam = family.unwrap_or_else(|| match &address {
            Some(ListenerAddress::Inet(_, _)) => "AF_INET".to_string(),
            Some(ListenerAddress::Named(s)) => address_type_str(s).to_string(),
            None => default_family_str().to_string(),
        });

        match address {
            Some(ListenerAddress::Inet(host, port)) => {
                let bind_host = if host.is_empty() { "127.0.0.1" } else { &host };
                let listener = TcpListener::bind((bind_host, port))
                    .map_err(|e| PyError::from_io_error(&e, None))?;
                let local = listener
                    .local_addr()
                    .map_err(|e| PyError::from_io_error(&e, None))?;
                let ret_host = if host.is_empty() {
                    "0.0.0.0".to_string()
                } else {
                    local.ip().to_string()
                };
                Ok(Arc::new(Self {
                    listener: Mutex::new(Some(listener)),
                    address: ListenerAddress::Inet(ret_host, local.port()),
                    authkey,
                    last_accepted: Mutex::new(None),
                    temp_addr_file: None,
                }))
            }
            Some(ListenerAddress::Named(path_str)) => {
                let listener = TcpListener::bind("127.0.0.1:0")
                    .map_err(|e| PyError::from_io_error(&e, None))?;
                let local = listener
                    .local_addr()
                    .map_err(|e| PyError::from_io_error(&e, None))?;
                let rendezvous = named_address_rendezvous_path(&path_str);
                if let Some(parent) = rendezvous.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&rendezvous, local.to_string())
                    .map_err(|e| PyError::from_io_error(&e, None))?;
                Ok(Arc::new(Self {
                    listener: Mutex::new(Some(listener)),
                    address: ListenerAddress::Named(path_str),
                    authkey,
                    last_accepted: Mutex::new(None),
                    temp_addr_file: Some(rendezvous),
                }))
            }
            None => {
                if fam == "AF_INET" {
                    let listener = TcpListener::bind("127.0.0.1:0")
                        .map_err(|e| PyError::from_io_error(&e, None))?;
                    let local = listener
                        .local_addr()
                        .map_err(|e| PyError::from_io_error(&e, None))?;
                    Ok(Arc::new(Self {
                        listener: Mutex::new(Some(listener)),
                        address: ListenerAddress::Inet(local.ip().to_string(), local.port()),
                        authkey,
                        last_accepted: Mutex::new(None),
                        temp_addr_file: None,
                    }))
                } else {
                    let name = arbitrary_address_str(&fam);
                    Self::bind(Some(ListenerAddress::Named(name)), Some(fam), authkey)
                }
            }
        }
    }

    pub(crate) fn accept(
        &self,
        interp: &mut Interpreter,
    ) -> Result<(Arc<ConnectionCore>, Option<SocketAddr>)> {
        let gil = Arc::clone(&interp.cold.gil_state);
        let accepted: io::Result<(TcpStream, SocketAddr)> = interp.release_gil_during(|| {
            let guard = self
                .listener
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(ref listener) = *guard else {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "listener is closed",
                ));
            };
            let _ = listener.set_nonblocking(true);
            loop {
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "interpreter shutting down",
                    ));
                }
                match listener.accept() {
                    Ok((stream, peer)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_nodelay(true);
                        return Ok((stream, peer));
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => return Err(e),
                }
            }
        });
        let (stream, peer) = accepted.map_err(|e| PyError::from_io_error(&e, None))?;
        *self
            .last_accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(peer.to_string());
        let conn = ConnectionCore::from_stream(stream, true, true)
            .map_err(|e| PyError::from_io_error(&e, None))?;
        if let Some(ref key) = self.authkey {
            deliver_challenge_core(interp, &conn, key)?;
            answer_challenge_core(interp, &conn, key)?;
        }
        Ok((conn, Some(peer)))
    }

    pub(crate) fn close(&self) {
        let _ = self
            .listener
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(ref p) = self.temp_addr_file {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Drop for ListenerCore {
    fn drop(&mut self) {
        if let Some(ref p) = self.temp_addr_file {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Connect a `Client` to a `Listener`.
pub(crate) fn client_connect(
    interp: &mut Interpreter,
    address: ListenerAddress,
    authkey: Option<Vec<u8>>,
) -> Result<Arc<ConnectionCore>> {
    let target_addr: String = match address {
        ListenerAddress::Inet(host, port) => {
            let h = if host.is_empty() { "127.0.0.1" } else { &host };
            format!("{h}:{port}")
        }
        ListenerAddress::Named(path_str) => {
            let rendezvous = named_address_rendezvous_path(&path_str);
            std::fs::read_to_string(&rendezvous)
                .map_err(|e| PyError::from_io_error(&e, Some(&path_str)))?
                .trim()
                .to_string()
        }
    };
    let stream = interp
        .release_gil_during(|| TcpStream::connect(&target_addr))
        .map_err(|e| PyError::from_io_error(&e, None))?;
    let conn = ConnectionCore::from_stream(stream, true, true)
        .map_err(|e| PyError::from_io_error(&e, None))?;
    if let Some(ref key) = authkey {
        answer_challenge_core(interp, &conn, key)?;
        deliver_challenge_core(interp, &conn, key)?;
    }
    Ok(conn)
}

/// Compute HMAC digest using Python's built-in `hmac` module (matches CPython
/// 3.12's `{sha256}` and legacy `md5` challenge-response).
fn compute_hmac_digest(
    interp: &mut Interpreter,
    authkey: &[u8],
    message: &[u8],
    digest_name: &str,
) -> Result<Vec<u8>> {
    let hmac_mod = interp.load_module("hmac")?;
    let digest_fn = interp.get_attr(&hmac_mod, "digest")?;
    let res = call_pos(
        interp,
        digest_fn,
        &[
            Value::bytes(authkey.to_vec()),
            Value::bytes(message.to_vec()),
            Value::string(digest_name),
        ],
    )?;
    match res.kind() {
        ValueKind::Bytes(b) => Ok(b.as_slice().to_vec()),
        _ => Err(PyError::named("RuntimeError", "hmac.digest failed")),
    }
}

/// Deliver CPython 3.12 HMAC challenge over `conn`.
pub(crate) fn deliver_challenge_core(
    interp: &mut Interpreter,
    conn: &ConnectionCore,
    authkey: &[u8],
) -> Result<()> {
    let msg_raw = random_bytes(MESSAGE_LENGTH);
    let mut challenge_payload = Vec::with_capacity(CHALLENGE.len() + 8 + msg_raw.len());
    challenge_payload.extend_from_slice(CHALLENGE);
    challenge_payload.extend_from_slice(b"{sha256}");
    challenge_payload.extend_from_slice(&msg_raw);
    conn.send_bytes_raw(interp, &challenge_payload)?;

    let response = conn.recv_bytes_raw(interp)?.ok_or_else(|| {
        super::raise_context_error(interp, "AuthenticationError", "digest received was wrong")
    })?;
    let expected = compute_hmac_digest(
        interp,
        authkey,
        &challenge_payload[CHALLENGE.len()..],
        "sha256",
    )?;
    if response == expected {
        conn.send_bytes_raw(interp, WELCOME)?;
        Ok(())
    } else {
        let _ = conn.send_bytes_raw(interp, FAILURE);
        Err(super::raise_context_error(
            interp,
            "AuthenticationError",
            "digest received was wrong",
        ))
    }
}

/// Answer CPython 3.12 HMAC challenge over `conn`.
pub(crate) fn answer_challenge_core(
    interp: &mut Interpreter,
    conn: &ConnectionCore,
    authkey: &[u8],
) -> Result<()> {
    let message = conn.recv_bytes_raw(interp)?.ok_or_else(|| {
        super::raise_context_error(interp, "AuthenticationError", "missing challenge")
    })?;
    if !message.starts_with(CHALLENGE) {
        return Err(super::raise_context_error(
            interp,
            "AuthenticationError",
            "expected challenge message",
        ));
    }
    let body = &message[CHALLENGE.len()..];
    let digest = if body.starts_with(b"{sha256}") {
        compute_hmac_digest(interp, authkey, body, "sha256")?
    } else {
        compute_hmac_digest(interp, authkey, body, "md5")?
    };
    conn.send_bytes_raw(interp, &digest)?;
    let reply = conn.recv_bytes_raw(interp)?.ok_or_else(|| {
        super::raise_context_error(interp, "AuthenticationError", "digest sent was rejected")
    })?;
    if reply != WELCOME {
        return Err(super::raise_context_error(
            interp,
            "AuthenticationError",
            "digest sent was rejected",
        ));
    }
    Ok(())
}

pub(crate) fn default_family_str() -> &'static str {
    #[cfg(windows)]
    {
        "AF_PIPE"
    }
    #[cfg(unix)]
    {
        "AF_UNIX"
    }
}

pub(crate) fn address_type_str(addr: &str) -> &'static str {
    if addr.starts_with(r"\\") {
        "AF_PIPE"
    } else {
        "AF_UNIX"
    }
}

pub(crate) fn arbitrary_address_str(family: &str) -> String {
    let suffix = super::random_hex(6);
    match family {
        "AF_PIPE" => format!(r"\\.\pipe\pyc-{}-{}-{}", std::process::id(), 0, suffix),
        "AF_UNIX" => {
            let mut p = std::env::temp_dir();
            p.push(format!("pymp-{}-{}.sock", std::process::id(), suffix));
            p.to_string_lossy().into_owned()
        }
        _ => "127.0.0.1:0".to_string(),
    }
}

fn named_address_rendezvous_path(addr: &str) -> std::path::PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    addr.hash(&mut h);
    let mut p = std::env::temp_dir();
    p.push(format!("pyrust_mp_addr_{:016x}.addr", h.finish()));
    p
}
