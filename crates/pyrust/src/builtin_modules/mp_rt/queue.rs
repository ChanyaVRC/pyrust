//! Cross-process `Queue`, `SimpleQueue`, and `JoinableQueue` cores.
//!
//! - In-process (same PID as owner): direct `Mutex` + `Condvar` fast path with
//!   `Interpreter::release_gil_during(...)` while waiting.
//! - Across processes (`spawn` on Windows/Unix or `fork` on Unix): transparently
//!   exports to [`super::broker`] and speaks a compact binary RPC protocol over
//!   loopback TCP. Items are always pickled bytes (`Vec<u8>`), matching CPython
//!   3.12's exact serialization semantics.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::SHUTDOWN_POLL_SLICE;
use super::broker::{BrokerEndpoint, register_queue, track_queue_for_fork};
use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, current_thread_is_daemon_worker};

const OP_PUT: u8 = 1;
const OP_GET: u8 = 2;
const OP_QSIZE: u8 = 3;
const OP_EMPTY: u8 = 4;
const OP_FULL: u8 = 5;
const OP_TASK_DONE: u8 = 6;
const OP_JOIN: u8 = 7;

const RESP_OK: u8 = 0;
const RESP_EMPTY: u8 = 1;
const RESP_FULL: u8 = 2;
const RESP_CLOSED: u8 = 3;
const RESP_TOO_MANY_TASK_DONE: u8 = 4;

struct QueueInner {
    items: VecDeque<Vec<u8>>,
    maxsize: usize, // 0 = unbounded
    unfinished_tasks: usize,
    closed: bool,
}

/// Authoritative queue core owned by the creator process.
pub(crate) struct QueueCore {
    owner_pid: u32,
    inner: Mutex<QueueInner>,
    not_empty: Condvar,
    not_full: Condvar,
    all_done: Condvar,
    endpoint: Mutex<Option<BrokerEndpoint>>,
}

#[derive(Debug)]
pub(crate) enum QueueOpError {
    Empty,
    Full,
    Closed,
    TooManyTaskDone,
    Io(io::Error),
}

impl QueueCore {
    pub(crate) fn new(maxsize: usize) -> Arc<Self> {
        let core = Arc::new(Self {
            owner_pid: std::process::id(),
            inner: Mutex::new(QueueInner {
                items: VecDeque::new(),
                maxsize,
                unfinished_tasks: 0,
                closed: false,
            }),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
            all_done: Condvar::new(),
            endpoint: Mutex::new(None),
        });
        track_queue_for_fork(&core);
        core
    }

    pub(crate) fn ensure_exported(self: &Arc<Self>) -> io::Result<BrokerEndpoint> {
        let mut ep = self
            .endpoint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(ref existing) = *ep {
            return Ok(existing.clone());
        }
        let created = register_queue(Arc::clone(self))?;
        *ep = Some(created.clone());
        Ok(created)
    }

    pub(crate) fn put_blocking(
        &self,
        data: Vec<u8>,
        block: bool,
        timeout: Option<Duration>,
        should_abort: impl Fn() -> bool,
    ) -> std::result::Result<(), QueueOpError> {
        let deadline = timeout.map(|d| Instant::now() + d);
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if g.closed {
                return Err(QueueOpError::Closed);
            }
            if g.maxsize == 0 || g.items.len() < g.maxsize {
                g.items.push_back(data);
                g.unfinished_tasks += 1;
                self.not_empty.notify_one();
                return Ok(());
            }
            if !block {
                return Err(QueueOpError::Full);
            }
            if should_abort() {
                return Err(QueueOpError::Full);
            }
            let wait_dur = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        return Err(QueueOpError::Full);
                    }
                    (dl - now).min(SHUTDOWN_POLL_SLICE)
                }
                None => SHUTDOWN_POLL_SLICE,
            };
            let (next_g, _) = self
                .not_full
                .wait_timeout(g, wait_dur)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g = next_g;
        }
    }

    pub(crate) fn get_blocking(
        &self,
        block: bool,
        timeout: Option<Duration>,
        should_abort: impl Fn() -> bool,
    ) -> std::result::Result<Vec<u8>, QueueOpError> {
        let deadline = timeout.map(|d| Instant::now() + d);
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(item) = g.items.pop_front() {
                self.not_full.notify_one();
                return Ok(item);
            }
            if g.closed {
                return Err(QueueOpError::Closed);
            }
            if !block {
                return Err(QueueOpError::Empty);
            }
            if should_abort() {
                return Err(QueueOpError::Empty);
            }
            let wait_dur = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        return Err(QueueOpError::Empty);
                    }
                    (dl - now).min(SHUTDOWN_POLL_SLICE)
                }
                None => SHUTDOWN_POLL_SLICE,
            };
            let (next_g, _) = self
                .not_empty
                .wait_timeout(g, wait_dur)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g = next_g;
        }
    }

    pub(crate) fn qsize(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .items
            .len()
    }

    pub(crate) fn empty(&self) -> bool {
        self.qsize() == 0
    }

    pub(crate) fn full(&self) -> bool {
        let g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        g.maxsize > 0 && g.items.len() >= g.maxsize
    }

    pub(crate) fn task_done(&self) -> std::result::Result<(), QueueOpError> {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.unfinished_tasks == 0 {
            return Err(QueueOpError::TooManyTaskDone);
        }
        g.unfinished_tasks -= 1;
        if g.unfinished_tasks == 0 {
            self.all_done.notify_all();
        }
        Ok(())
    }

    pub(crate) fn join_blocking(&self, should_abort: impl Fn() -> bool) {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while g.unfinished_tasks > 0 {
            if should_abort() {
                break;
            }
            let (next_g, _) = self
                .all_done
                .wait_timeout(g, SHUTDOWN_POLL_SLICE)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g = next_g;
        }
    }
}

/// Broker server loop for a single connected child queue client (pure Rust, no GIL).
pub(crate) fn serve_queue_client(mut stream: TcpStream, core: Arc<QueueCore>) {
    loop {
        let mut op = [0u8; 1];
        if stream.read_exact(&mut op).is_err() {
            break;
        }
        match op[0] {
            OP_PUT => {
                let Some((block, timeout)) = read_block_timeout(&mut stream) else {
                    break;
                };
                let mut len_buf = [0u8; 4];
                if stream.read_exact(&mut len_buf).is_err() {
                    break;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut payload = vec![0u8; len];
                if len > 0 && stream.read_exact(&mut payload).is_err() {
                    break;
                }
                let code = match core.put_blocking(payload, block, timeout, || false) {
                    Ok(()) => RESP_OK,
                    Err(QueueOpError::Full) => RESP_FULL,
                    Err(QueueOpError::Closed) => RESP_CLOSED,
                    Err(_) => RESP_FULL,
                };
                if stream.write_all(&[code]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_GET => {
                let Some((block, timeout)) = read_block_timeout(&mut stream) else {
                    break;
                };
                match core.get_blocking(block, timeout, || false) {
                    Ok(bytes) => {
                        let mut hdr = [0u8; 5];
                        hdr[0] = RESP_OK;
                        hdr[1..5].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
                        if stream.write_all(&hdr).is_err()
                            || (!bytes.is_empty() && stream.write_all(&bytes).is_err())
                            || stream.flush().is_err()
                        {
                            break;
                        }
                    }
                    Err(QueueOpError::Closed) => {
                        if stream.write_all(&[RESP_CLOSED]).is_err() || stream.flush().is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        if stream.write_all(&[RESP_EMPTY]).is_err() || stream.flush().is_err() {
                            break;
                        }
                    }
                }
            }
            OP_QSIZE => {
                let n = core.qsize() as u64;
                if stream.write_all(&n.to_be_bytes()).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_EMPTY => {
                let b = u8::from(core.empty());
                if stream.write_all(&[b]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_FULL => {
                let b = u8::from(core.full());
                if stream.write_all(&[b]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_TASK_DONE => {
                let code = match core.task_done() {
                    Ok(()) => RESP_OK,
                    Err(QueueOpError::TooManyTaskDone) => RESP_TOO_MANY_TASK_DONE,
                    Err(_) => RESP_CLOSED,
                };
                if stream.write_all(&[code]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_JOIN => {
                core.join_blocking(|| false);
                if stream.write_all(&[RESP_OK]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
}

fn write_block_timeout(
    stream: &mut TcpStream,
    block: bool,
    timeout: Option<Duration>,
) -> io::Result<()> {
    let mut buf = [0u8; 9];
    buf[0] = u8::from(block);
    let nanos: i64 = match timeout {
        None => -1,
        Some(d) => d.as_nanos().min(i64::MAX as u128) as i64,
    };
    buf[1..9].copy_from_slice(&nanos.to_be_bytes());
    stream.write_all(&buf)
}

fn read_block_timeout(stream: &mut TcpStream) -> Option<(bool, Option<Duration>)> {
    let mut buf = [0u8; 9];
    stream.read_exact(&mut buf).ok()?;
    let block = buf[0] != 0;
    let nanos = i64::from_be_bytes([
        buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8],
    ]);
    let timeout = if nanos < 0 {
        None
    } else {
        Some(Duration::from_nanos(nanos as u64))
    };
    Some((block, timeout))
}

/// Unified queue handle stored in Python `Queue` / `SimpleQueue` / `JoinableQueue`.
///
/// Automatically routes to the in-process `QueueCore` when running in the owner
/// process, or to the parent's `GlobalIpcBroker` over TCP when running in a
/// spawned or forked child process.
pub(crate) enum QueueHandle {
    Local(Arc<QueueCore>),
    Remote {
        endpoint: BrokerEndpoint,
        conn: Mutex<Option<TcpStream>>,
    },
}

impl QueueHandle {
    pub(crate) fn new_local(maxsize: usize) -> Self {
        Self::Local(QueueCore::new(maxsize))
    }

    pub(crate) fn new_remote(endpoint: BrokerEndpoint) -> Self {
        Self::Remote {
            endpoint,
            conn: Mutex::new(None),
        }
    }

    /// Export this queue as a `BrokerEndpoint` for pickling (`__reduce__`).
    pub(crate) fn export_endpoint(&self) -> Result<BrokerEndpoint> {
        match self {
            Self::Local(core) => core
                .ensure_exported()
                .map_err(|e| PyError::from_io_error(&e, None)),
            Self::Remote { endpoint, .. } => Ok(endpoint.clone()),
        }
    }

    /// Return `Some(endpoint)` when we are a forked child holding a `Local`
    /// `QueueCore` whose `owner_pid != current_pid`.
    fn remote_endpoint_if_forked_or_remote(&self) -> Option<BrokerEndpoint> {
        match self {
            Self::Local(core) => {
                if core.owner_pid == std::process::id() {
                    None
                } else {
                    core.endpoint
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                }
            }
            Self::Remote { endpoint, .. } => Some(endpoint.clone()),
        }
    }

    fn with_remote_stream<R>(
        &self,
        ep: &BrokerEndpoint,
        f: impl FnOnce(&mut TcpStream) -> io::Result<R>,
    ) -> io::Result<R> {
        if let Self::Remote { conn, .. } = self {
            let mut guard = conn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.is_none() {
                *guard = Some(ep.connect()?);
            }
            let stream = guard.as_mut().expect("just connected");
            match f(stream) {
                Ok(v) => Ok(v),
                Err(e) => {
                    *guard = None;
                    Err(e)
                }
            }
        } else {
            let mut stream = ep.connect()?;
            f(&mut stream)
        }
    }

    pub(crate) fn put(
        &self,
        interp: &mut Interpreter,
        payload: Vec<u8>,
        block: bool,
        timeout: Option<Duration>,
    ) -> std::result::Result<(), QueueOpError> {
        let gil = Arc::clone(&interp.cold.gil_state);
        if let Some(ep) = self.remote_endpoint_if_forked_or_remote() {
            return interp.release_gil_during(|| {
                let code = self
                    .with_remote_stream(&ep, |s| {
                        s.write_all(&[OP_PUT])?;
                        write_block_timeout(s, block, timeout)?;
                        s.write_all(&(payload.len() as u32).to_be_bytes())?;
                        if !payload.is_empty() {
                            s.write_all(&payload)?;
                        }
                        s.flush()?;
                        let mut resp = [0u8; 1];
                        s.read_exact(&mut resp)?;
                        Ok(resp[0])
                    })
                    .map_err(QueueOpError::Io)?;
                match code {
                    RESP_OK => Ok(()),
                    RESP_FULL => Err(QueueOpError::Full),
                    RESP_CLOSED => Err(QueueOpError::Closed),
                    _ => Err(QueueOpError::Full),
                }
            });
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        interp.release_gil_during(|| {
            core.put_blocking(payload, block, timeout, || {
                gil.is_shutting_down() && current_thread_is_daemon_worker()
            })
        })
    }

    pub(crate) fn get(
        &self,
        interp: &mut Interpreter,
        block: bool,
        timeout: Option<Duration>,
    ) -> std::result::Result<Vec<u8>, QueueOpError> {
        let gil = Arc::clone(&interp.cold.gil_state);
        if let Some(ep) = self.remote_endpoint_if_forked_or_remote() {
            return interp.release_gil_during(|| {
                self.with_remote_stream(&ep, |s| {
                    s.write_all(&[OP_GET])?;
                    write_block_timeout(s, block, timeout)?;
                    s.flush()?;
                    let mut code = [0u8; 1];
                    s.read_exact(&mut code)?;
                    if code[0] != RESP_OK {
                        return Ok(Err(code[0]));
                    }
                    let mut len_buf = [0u8; 4];
                    s.read_exact(&mut len_buf)?;
                    let len = u32::from_be_bytes(len_buf) as usize;
                    let mut buf = vec![0u8; len];
                    if len > 0 {
                        s.read_exact(&mut buf)?;
                    }
                    Ok(Ok(buf))
                })
                .map_err(QueueOpError::Io)?
                .map_err(|code| match code {
                    RESP_CLOSED => QueueOpError::Closed,
                    _ => QueueOpError::Empty,
                })
            });
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        interp.release_gil_during(|| {
            core.get_blocking(block, timeout, || {
                gil.is_shutting_down() && current_thread_is_daemon_worker()
            })
        })
    }

    pub(crate) fn qsize(&self, interp: &mut Interpreter) -> Result<usize> {
        if let Some(ep) = self.remote_endpoint_if_forked_or_remote() {
            let n = interp
                .release_gil_during(|| {
                    self.with_remote_stream(&ep, |s| {
                        s.write_all(&[OP_QSIZE])?;
                        s.flush()?;
                        let mut buf = [0u8; 8];
                        s.read_exact(&mut buf)?;
                        Ok(u64::from_be_bytes(buf) as usize)
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None))?;
            return Ok(n);
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(core.qsize())
    }

    pub(crate) fn empty(&self, interp: &mut Interpreter) -> Result<bool> {
        if let Some(ep) = self.remote_endpoint_if_forked_or_remote() {
            let b = interp
                .release_gil_during(|| {
                    self.with_remote_stream(&ep, |s| {
                        s.write_all(&[OP_EMPTY])?;
                        s.flush()?;
                        let mut buf = [0u8; 1];
                        s.read_exact(&mut buf)?;
                        Ok(buf[0] != 0)
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None))?;
            return Ok(b);
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(core.empty())
    }

    pub(crate) fn full(&self, interp: &mut Interpreter) -> Result<bool> {
        if let Some(ep) = self.remote_endpoint_if_forked_or_remote() {
            let b = interp
                .release_gil_during(|| {
                    self.with_remote_stream(&ep, |s| {
                        s.write_all(&[OP_FULL])?;
                        s.flush()?;
                        let mut buf = [0u8; 1];
                        s.read_exact(&mut buf)?;
                        Ok(buf[0] != 0)
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None))?;
            return Ok(b);
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(core.full())
    }

    pub(crate) fn task_done(
        &self,
        interp: &mut Interpreter,
    ) -> std::result::Result<(), QueueOpError> {
        if let Some(ep) = self.remote_endpoint_if_forked_or_remote() {
            let code = interp
                .release_gil_during(|| {
                    self.with_remote_stream(&ep, |s| {
                        s.write_all(&[OP_TASK_DONE])?;
                        s.flush()?;
                        let mut buf = [0u8; 1];
                        s.read_exact(&mut buf)?;
                        Ok(buf[0])
                    })
                })
                .map_err(QueueOpError::Io)?;
            return match code {
                RESP_OK => Ok(()),
                RESP_TOO_MANY_TASK_DONE => Err(QueueOpError::TooManyTaskDone),
                _ => Err(QueueOpError::Closed),
            };
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        core.task_done()
    }

    pub(crate) fn join(&self, interp: &mut Interpreter) -> Result<()> {
        let gil = Arc::clone(&interp.cold.gil_state);
        if let Some(ep) = self.remote_endpoint_if_forked_or_remote() {
            interp
                .release_gil_during(|| {
                    self.with_remote_stream(&ep, |s| {
                        s.write_all(&[OP_JOIN])?;
                        s.flush()?;
                        let mut buf = [0u8; 1];
                        s.read_exact(&mut buf)?;
                        Ok(())
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None))?;
            return Ok(());
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        interp.release_gil_during(|| {
            core.join_blocking(|| gil.is_shutting_down() && current_thread_is_daemon_worker());
        });
        Ok(())
    }
}
