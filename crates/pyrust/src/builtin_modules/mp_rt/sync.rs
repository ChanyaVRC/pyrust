//! Cross-process synchronization primitives (`SemLock`, `Lock`, `RLock`,
//! `Semaphore`, `BoundedSemaphore`, `Event`, `Condition`, `Barrier`).
//!
//! Uses a local `Mutex` + `Condvar` fast path in the creator process (releasing
//! the GIL via `Interpreter::release_gil_during(...)`) and transparently bridges
//! over [`super::broker`] for spawned (Windows & Unix) and forked (Unix) child
//! processes.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::SHUTDOWN_POLL_SLICE;
use super::broker::{BrokerEndpoint, register_sync, track_sync_for_fork};
use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, current_os_thread_ident, current_thread_is_daemon_worker};

pub(crate) const RECURSIVE_MUTEX: i64 = 0;
pub(crate) const SEMAPHORE: i64 = 1;
pub(crate) const SEM_VALUE_MAX: i64 = i32::MAX as i64;

/// Unique caller identity `(pid, tid)` for `RLock` ownership across processes
/// and threads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct CallerId {
    pub pid: u32,
    pub tid: i64,
}

impl CallerId {
    pub(crate) fn current() -> Self {
        Self {
            pid: std::process::id(),
            tid: current_os_thread_ident(),
        }
    }
}

// RPC opcodes
const OP_ACQUIRE: u8 = 1;
const OP_RELEASE: u8 = 2;
const OP_GET_VALUE: u8 = 3;
const OP_COUNT: u8 = 4;
const OP_IS_MINE: u8 = 5;
const OP_IS_ZERO: u8 = 6;
const OP_EVENT_IS_SET: u8 = 10;
const OP_EVENT_SET: u8 = 11;
const OP_EVENT_CLEAR: u8 = 12;
const OP_EVENT_WAIT: u8 = 13;
const OP_COND_WAIT: u8 = 20;
const OP_COND_NOTIFY: u8 = 21;
const OP_COND_NOTIFY_ALL: u8 = 22;
const OP_BARRIER_WAIT: u8 = 30;
const OP_BARRIER_RESET: u8 = 31;
const OP_BARRIER_ABORT: u8 = 32;
const OP_BARRIER_INFO: u8 = 33;

const RESP_OK: u8 = 0;
const RESP_FALSE: u8 = 1;
const RESP_VALUE_ERR: u8 = 2;
const RESP_ASSERT_ERR: u8 = 3;
const RESP_BROKEN_BARRIER: u8 = 4;

pub(crate) enum SyncKind {
    SemLock {
        kind: i64, // RECURSIVE_MUTEX or SEMAPHORE
        value: i64,
        maxvalue: i64,
        owner: Option<CallerId>,
        recursion: i64,
    },
    Event {
        flag: bool,
    },
    CondWaitQueue {
        next_ticket: u64,
        notified_up_to: u64,
        waiters: usize,
    },
    Barrier {
        parties: usize,
        count: usize,
        generation: u64,
        broken: bool,
        resetting: bool,
    },
}

pub(crate) struct SyncCore {
    owner_pid: u32,
    inner: Mutex<SyncKind>,
    cvar: Condvar,
    endpoint: Mutex<Option<BrokerEndpoint>>,
}

#[derive(Debug)]
pub(crate) enum SyncError {
    ValueError(String),
    AssertionError(String),
    BrokenBarrier,
    Io(io::Error),
}

impl SyncCore {
    pub(crate) fn new_semlock(kind: i64, value: i64, maxvalue: i64) -> Arc<Self> {
        let core = Arc::new(Self {
            owner_pid: std::process::id(),
            inner: Mutex::new(SyncKind::SemLock {
                kind,
                value,
                maxvalue,
                owner: None,
                recursion: 0,
            }),
            cvar: Condvar::new(),
            endpoint: Mutex::new(None),
        });
        track_sync_for_fork(&core);
        core
    }

    pub(crate) fn new_event() -> Arc<Self> {
        let core = Arc::new(Self {
            owner_pid: std::process::id(),
            inner: Mutex::new(SyncKind::Event { flag: false }),
            cvar: Condvar::new(),
            endpoint: Mutex::new(None),
        });
        track_sync_for_fork(&core);
        core
    }

    pub(crate) fn new_cond_queue() -> Arc<Self> {
        let core = Arc::new(Self {
            owner_pid: std::process::id(),
            inner: Mutex::new(SyncKind::CondWaitQueue {
                next_ticket: 0,
                notified_up_to: 0,
                waiters: 0,
            }),
            cvar: Condvar::new(),
            endpoint: Mutex::new(None),
        });
        track_sync_for_fork(&core);
        core
    }

    pub(crate) fn new_barrier(parties: usize) -> Arc<Self> {
        let core = Arc::new(Self {
            owner_pid: std::process::id(),
            inner: Mutex::new(SyncKind::Barrier {
                parties,
                count: 0,
                generation: 0,
                broken: false,
                resetting: false,
            }),
            cvar: Condvar::new(),
            endpoint: Mutex::new(None),
        });
        track_sync_for_fork(&core);
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
        let created = register_sync(Arc::clone(self))?;
        *ep = Some(created.clone());
        Ok(created)
    }

    // ── SemLock operations ───────────────────────────────────────────────────

    pub(crate) fn sem_acquire(
        &self,
        caller: CallerId,
        block: bool,
        timeout: Option<Duration>,
        should_abort: impl Fn() -> bool,
    ) -> bool {
        let deadline = timeout.map(|d| Instant::now() + d);
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let SyncKind::SemLock {
                kind,
                value,
                owner,
                recursion,
                ..
            } = &mut *g
            {
                if *kind == RECURSIVE_MUTEX && *owner == Some(caller) {
                    *recursion += 1;
                    return true;
                }
                if *value > 0 {
                    *value -= 1;
                    if *kind == RECURSIVE_MUTEX {
                        *owner = Some(caller);
                        *recursion = 1;
                    }
                    return true;
                }
            } else {
                return false;
            }
            if !block || should_abort() {
                return false;
            }
            let wait_dur = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        return false;
                    }
                    (dl - now).min(SHUTDOWN_POLL_SLICE)
                }
                None => SHUTDOWN_POLL_SLICE,
            };
            let (next_g, _) = self
                .cvar
                .wait_timeout(g, wait_dur)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g = next_g;
        }
    }

    pub(crate) fn sem_release(&self, caller: CallerId) -> std::result::Result<(), SyncError> {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let SyncKind::SemLock {
            kind,
            value,
            maxvalue,
            owner,
            recursion,
        } = &mut *g
        else {
            return Err(SyncError::ValueError("invalid sync kind".into()));
        };
        if *kind == RECURSIVE_MUTEX {
            if *owner != Some(caller) {
                return Err(SyncError::AssertionError(
                    "attempt to release recursive lock not owned by thread".into(),
                ));
            }
            *recursion -= 1;
            if *recursion == 0 {
                *owner = None;
                *value += 1;
                self.cvar.notify_one();
            }
            return Ok(());
        }
        if *value >= *maxvalue {
            return Err(SyncError::ValueError(
                "semaphore or lock released too many times".into(),
            ));
        }
        *value += 1;
        self.cvar.notify_one();
        Ok(())
    }

    pub(crate) fn sem_get_value(&self) -> i64 {
        let g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *g {
            SyncKind::SemLock { value, .. } => value,
            _ => 0,
        }
    }

    pub(crate) fn sem_count(&self) -> i64 {
        let g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *g {
            SyncKind::SemLock {
                kind,
                recursion,
                maxvalue,
                value,
                ..
            } => {
                if kind == RECURSIVE_MUTEX {
                    recursion
                } else {
                    maxvalue - value
                }
            }
            _ => 0,
        }
    }

    pub(crate) fn sem_is_mine(&self, caller: CallerId) -> bool {
        let g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *g {
            SyncKind::SemLock {
                kind,
                owner,
                recursion,
                ..
            } => kind == RECURSIVE_MUTEX && owner == Some(caller) && recursion > 0,
            _ => false,
        }
    }

    pub(crate) fn sem_is_zero(&self) -> bool {
        self.sem_get_value() == 0
    }

    // ── Event operations ─────────────────────────────────────────────────────

    pub(crate) fn event_is_set(&self) -> bool {
        let g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        matches!(*g, SyncKind::Event { flag: true })
    }

    pub(crate) fn event_set(&self) {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let SyncKind::Event { ref mut flag } = *g {
            *flag = true;
            self.cvar.notify_all();
        }
    }

    pub(crate) fn event_clear(&self) {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let SyncKind::Event { ref mut flag } = *g {
            *flag = false;
        }
    }

    pub(crate) fn event_wait(
        &self,
        timeout: Option<Duration>,
        should_abort: impl Fn() -> bool,
    ) -> bool {
        let deadline = timeout.map(|d| Instant::now() + d);
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if matches!(*g, SyncKind::Event { flag: true }) {
                return true;
            }
            if should_abort() {
                return false;
            }
            let wait_dur = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        return false;
                    }
                    (dl - now).min(SHUTDOWN_POLL_SLICE)
                }
                None => SHUTDOWN_POLL_SLICE,
            };
            let (next_g, _) = self
                .cvar
                .wait_timeout(g, wait_dur)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g = next_g;
        }
    }

    // ── Condition wait-queue operations ──────────────────────────────────────

    pub(crate) fn cond_wait(
        &self,
        timeout: Option<Duration>,
        should_abort: impl Fn() -> bool,
    ) -> bool {
        let deadline = timeout.map(|d| Instant::now() + d);
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let my_ticket = match &mut *g {
            SyncKind::CondWaitQueue {
                next_ticket,
                waiters,
                ..
            } => {
                *next_ticket += 1;
                *waiters += 1;
                *next_ticket
            }
            _ => return false,
        };
        let result = loop {
            if let SyncKind::CondWaitQueue { notified_up_to, .. } = *g
                && notified_up_to >= my_ticket
            {
                break true;
            }
            if should_abort() {
                break false;
            }
            let wait_dur = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        break false;
                    }
                    (dl - now).min(SHUTDOWN_POLL_SLICE)
                }
                None => SHUTDOWN_POLL_SLICE,
            };
            let (next_g, _) = self
                .cvar
                .wait_timeout(g, wait_dur)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g = next_g;
        };
        if let SyncKind::CondWaitQueue { waiters, .. } = &mut *g {
            *waiters = waiters.saturating_sub(1);
        }
        result
    }

    pub(crate) fn cond_notify(&self, n: usize) {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let SyncKind::CondWaitQueue {
            next_ticket,
            notified_up_to,
            ..
        } = &mut *g
        {
            let target = (*notified_up_to).saturating_add(n as u64).min(*next_ticket);
            *notified_up_to = target;
            self.cvar.notify_all();
        }
    }

    pub(crate) fn cond_notify_all(&self) {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let SyncKind::CondWaitQueue {
            next_ticket,
            notified_up_to,
            ..
        } = &mut *g
        {
            *notified_up_to = *next_ticket;
            self.cvar.notify_all();
        }
    }

    // ── Barrier operations ───────────────────────────────────────────────────

    pub(crate) fn barrier_wait(
        &self,
        timeout: Option<Duration>,
        should_abort: impl Fn() -> bool,
    ) -> std::result::Result<usize, SyncError> {
        let deadline = timeout.map(|d| Instant::now() + d);
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let SyncKind::Barrier {
            parties,
            count,
            generation,
            broken,
            ..
        } = &mut *g
        else {
            return Err(SyncError::BrokenBarrier);
        };
        if *broken {
            return Err(SyncError::BrokenBarrier);
        }
        let my_gen = *generation;
        let idx = *count;
        *count += 1;
        if *count >= *parties {
            *count = 0;
            *generation = generation.wrapping_add(1);
            self.cvar.notify_all();
            return Ok(idx);
        }
        loop {
            let SyncKind::Barrier {
                generation,
                broken,
                resetting,
                ..
            } = &mut *g
            else {
                return Err(SyncError::BrokenBarrier);
            };
            if *broken || *resetting {
                return Err(SyncError::BrokenBarrier);
            }
            if *generation != my_gen {
                return Ok(idx);
            }
            if should_abort() {
                *broken = true;
                self.cvar.notify_all();
                return Err(SyncError::BrokenBarrier);
            }
            let wait_dur = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        *broken = true;
                        self.cvar.notify_all();
                        return Err(SyncError::BrokenBarrier);
                    }
                    (dl - now).min(SHUTDOWN_POLL_SLICE)
                }
                None => SHUTDOWN_POLL_SLICE,
            };
            let (next_g, _) = self
                .cvar
                .wait_timeout(g, wait_dur)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g = next_g;
        }
    }

    pub(crate) fn barrier_reset(&self) {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let SyncKind::Barrier {
            count,
            generation,
            broken,
            resetting,
            ..
        } = &mut *g
        {
            if *count > 0 {
                *resetting = true;
                self.cvar.notify_all();
            }
            *count = 0;
            *generation = generation.wrapping_add(1);
            *broken = false;
            *resetting = false;
            self.cvar.notify_all();
        }
    }

    pub(crate) fn barrier_abort(&self) {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let SyncKind::Barrier { broken, .. } = &mut *g {
            *broken = true;
            self.cvar.notify_all();
        }
    }

    pub(crate) fn barrier_info(&self) -> (usize, usize, bool) {
        let g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *g {
            SyncKind::Barrier {
                parties,
                count,
                broken,
                ..
            } => (parties, count, broken),
            _ => (0, 0, true),
        }
    }
}

/// Broker server loop for one sync client connection (pure Rust, no GIL).
pub(crate) fn serve_sync_client(mut stream: TcpStream, core: Arc<SyncCore>) {
    loop {
        let mut op = [0u8; 1];
        if stream.read_exact(&mut op).is_err() {
            break;
        }
        match op[0] {
            OP_ACQUIRE => {
                let Some((caller, block, timeout)) = read_caller_block_timeout(&mut stream) else {
                    break;
                };
                let ok = core.sem_acquire(caller, block, timeout, || false);
                let resp = if ok { RESP_OK } else { RESP_FALSE };
                if stream.write_all(&[resp]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_RELEASE => {
                let Some(caller) = read_caller(&mut stream) else {
                    break;
                };
                let resp = match core.sem_release(caller) {
                    Ok(()) => RESP_OK,
                    Err(SyncError::AssertionError(_)) => RESP_ASSERT_ERR,
                    Err(_) => RESP_VALUE_ERR,
                };
                if stream.write_all(&[resp]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_GET_VALUE => {
                let v = core.sem_get_value();
                if stream.write_all(&v.to_be_bytes()).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_COUNT => {
                let v = core.sem_count();
                if stream.write_all(&v.to_be_bytes()).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_IS_MINE => {
                let Some(caller) = read_caller(&mut stream) else {
                    break;
                };
                let b = u8::from(core.sem_is_mine(caller));
                if stream.write_all(&[b]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_IS_ZERO => {
                let b = u8::from(core.sem_is_zero());
                if stream.write_all(&[b]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_EVENT_IS_SET => {
                let b = u8::from(core.event_is_set());
                if stream.write_all(&[b]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_EVENT_SET => {
                core.event_set();
                if stream.write_all(&[RESP_OK]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_EVENT_CLEAR => {
                core.event_clear();
                if stream.write_all(&[RESP_OK]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_EVENT_WAIT => {
                let Some(timeout) = read_opt_timeout(&mut stream) else {
                    break;
                };
                let ok = core.event_wait(timeout, || false);
                if stream.write_all(&[u8::from(ok)]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_COND_WAIT => {
                let Some(timeout) = read_opt_timeout(&mut stream) else {
                    break;
                };
                let ok = core.cond_wait(timeout, || false);
                if stream.write_all(&[u8::from(ok)]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_COND_NOTIFY => {
                let mut n_buf = [0u8; 8];
                if stream.read_exact(&mut n_buf).is_err() {
                    break;
                }
                let n = u64::from_be_bytes(n_buf) as usize;
                core.cond_notify(n);
                if stream.write_all(&[RESP_OK]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_COND_NOTIFY_ALL => {
                core.cond_notify_all();
                if stream.write_all(&[RESP_OK]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_BARRIER_WAIT => {
                let Some(timeout) = read_opt_timeout(&mut stream) else {
                    break;
                };
                match core.barrier_wait(timeout, || false) {
                    Ok(idx) => {
                        let mut out = [0u8; 9];
                        out[0] = RESP_OK;
                        out[1..9].copy_from_slice(&(idx as u64).to_be_bytes());
                        if stream.write_all(&out).is_err() || stream.flush().is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        if stream.write_all(&[RESP_BROKEN_BARRIER]).is_err()
                            || stream.flush().is_err()
                        {
                            break;
                        }
                    }
                }
            }
            OP_BARRIER_RESET => {
                core.barrier_reset();
                if stream.write_all(&[RESP_OK]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_BARRIER_ABORT => {
                core.barrier_abort();
                if stream.write_all(&[RESP_OK]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            OP_BARRIER_INFO => {
                let (parties, waiting, broken) = core.barrier_info();
                let mut out = [0u8; 17];
                out[0..8].copy_from_slice(&(parties as u64).to_be_bytes());
                out[8..16].copy_from_slice(&(waiting as u64).to_be_bytes());
                out[16] = u8::from(broken);
                if stream.write_all(&out).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
}

fn write_caller(s: &mut TcpStream, c: CallerId) -> io::Result<()> {
    let mut buf = [0u8; 12];
    buf[0..4].copy_from_slice(&c.pid.to_be_bytes());
    buf[4..12].copy_from_slice(&c.tid.to_be_bytes());
    s.write_all(&buf)
}

fn read_caller(s: &mut TcpStream) -> Option<CallerId> {
    let mut buf = [0u8; 12];
    s.read_exact(&mut buf).ok()?;
    let pid = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let tid = i64::from_be_bytes([
        buf[4], buf[5], buf[6], buf[7], buf[8], buf[9], buf[10], buf[11],
    ]);
    Some(CallerId { pid, tid })
}

fn write_opt_timeout(s: &mut TcpStream, timeout: Option<Duration>) -> io::Result<()> {
    let nanos: i64 = match timeout {
        None => -1,
        Some(d) => d.as_nanos().min(i64::MAX as u128) as i64,
    };
    s.write_all(&nanos.to_be_bytes())
}

fn read_opt_timeout(s: &mut TcpStream) -> Option<Option<Duration>> {
    let mut buf = [0u8; 8];
    s.read_exact(&mut buf).ok()?;
    let nanos = i64::from_be_bytes(buf);
    Some(if nanos < 0 {
        None
    } else {
        Some(Duration::from_nanos(nanos as u64))
    })
}

fn read_caller_block_timeout(s: &mut TcpStream) -> Option<(CallerId, bool, Option<Duration>)> {
    let caller = read_caller(s)?;
    let mut b = [0u8; 1];
    s.read_exact(&mut b).ok()?;
    let timeout = read_opt_timeout(s)?;
    Some((caller, b[0] != 0, timeout))
}

/// Unified sync handle stored in Python sync objects.
pub(crate) enum SyncHandle {
    Local(Arc<SyncCore>),
    Remote { endpoint: BrokerEndpoint },
}

impl SyncHandle {
    pub(crate) fn new_local(core: Arc<SyncCore>) -> Self {
        Self::Local(core)
    }

    pub(crate) fn new_remote(endpoint: BrokerEndpoint) -> Self {
        Self::Remote { endpoint }
    }

    pub(crate) fn export_endpoint(&self) -> Result<BrokerEndpoint> {
        match self {
            Self::Local(core) => core
                .ensure_exported()
                .map_err(|e| PyError::from_io_error(&e, None)),
            Self::Remote { endpoint } => Ok(endpoint.clone()),
        }
    }

    fn remote_ep(&self) -> Option<BrokerEndpoint> {
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
            Self::Remote { endpoint } => Some(endpoint.clone()),
        }
    }

    fn rpc<R>(
        ep: &BrokerEndpoint,
        f: impl FnOnce(&mut TcpStream) -> io::Result<R>,
    ) -> io::Result<R> {
        let mut s = ep.connect()?;
        f(&mut s)
    }

    pub(crate) fn sem_acquire(
        &self,
        interp: &mut Interpreter,
        block: bool,
        timeout: Option<Duration>,
    ) -> Result<bool> {
        let caller = CallerId::current();
        let gil = Arc::clone(&interp.cold.gil_state);
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_ACQUIRE])?;
                        write_caller(s, caller)?;
                        s.write_all(&[u8::from(block)])?;
                        write_opt_timeout(s, timeout)?;
                        s.flush()?;
                        let mut resp = [0u8; 1];
                        s.read_exact(&mut resp)?;
                        Ok(resp[0] == RESP_OK)
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(interp.release_gil_during(|| {
            core.sem_acquire(caller, block, timeout, || {
                gil.is_shutting_down() && current_thread_is_daemon_worker()
            })
        }))
    }

    pub(crate) fn sem_release(&self, interp: &mut Interpreter) -> Result<()> {
        let caller = CallerId::current();
        let res = if let Some(ep) = self.remote_ep() {
            let code = interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_RELEASE])?;
                        write_caller(s, caller)?;
                        s.flush()?;
                        let mut resp = [0u8; 1];
                        s.read_exact(&mut resp)?;
                        Ok(resp[0])
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None))?;
            match code {
                RESP_OK => Ok(()),
                RESP_ASSERT_ERR => Err(SyncError::AssertionError(
                    "attempt to release recursive lock not owned by thread".into(),
                )),
                _ => Err(SyncError::ValueError(
                    "semaphore or lock released too many times".into(),
                )),
            }
        } else {
            let Self::Local(core) = self else {
                unreachable!();
            };
            core.sem_release(caller)
        };
        match res {
            Ok(()) => Ok(()),
            Err(SyncError::ValueError(m)) => Err(PyError::named("ValueError", m)),
            Err(SyncError::AssertionError(m)) => Err(PyError::named("AssertionError", m)),
            Err(SyncError::BrokenBarrier) => Err(PyError::named("RuntimeError", "broken barrier")),
            Err(SyncError::Io(e)) => Err(PyError::from_io_error(&e, None)),
        }
    }

    pub(crate) fn sem_get_value(&self, interp: &mut Interpreter) -> Result<i64> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_GET_VALUE])?;
                        s.flush()?;
                        let mut buf = [0u8; 8];
                        s.read_exact(&mut buf)?;
                        Ok(i64::from_be_bytes(buf))
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(core.sem_get_value())
    }

    pub(crate) fn sem_count(&self, interp: &mut Interpreter) -> Result<i64> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_COUNT])?;
                        s.flush()?;
                        let mut buf = [0u8; 8];
                        s.read_exact(&mut buf)?;
                        Ok(i64::from_be_bytes(buf))
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(core.sem_count())
    }

    pub(crate) fn sem_is_mine(&self, interp: &mut Interpreter) -> Result<bool> {
        let caller = CallerId::current();
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_IS_MINE])?;
                        write_caller(s, caller)?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(b[0] != 0)
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(core.sem_is_mine(caller))
    }

    pub(crate) fn sem_is_zero(&self, interp: &mut Interpreter) -> Result<bool> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_IS_ZERO])?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(b[0] != 0)
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(core.sem_is_zero())
    }

    // ── Event ────────────────────────────────────────────────────────────────

    pub(crate) fn event_is_set(&self, interp: &mut Interpreter) -> Result<bool> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_EVENT_IS_SET])?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(b[0] != 0)
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(core.event_is_set())
    }

    pub(crate) fn event_set(&self, interp: &mut Interpreter) -> Result<()> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_EVENT_SET])?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(())
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        core.event_set();
        Ok(())
    }

    pub(crate) fn event_clear(&self, interp: &mut Interpreter) -> Result<()> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_EVENT_CLEAR])?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(())
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        core.event_clear();
        Ok(())
    }

    pub(crate) fn event_wait(
        &self,
        interp: &mut Interpreter,
        timeout: Option<Duration>,
    ) -> Result<bool> {
        let gil = Arc::clone(&interp.cold.gil_state);
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_EVENT_WAIT])?;
                        write_opt_timeout(s, timeout)?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(b[0] != 0)
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(interp.release_gil_during(|| {
            core.event_wait(timeout, || {
                gil.is_shutting_down() && current_thread_is_daemon_worker()
            })
        }))
    }

    // ── Condition wait queue ─────────────────────────────────────────────────

    pub(crate) fn cond_wait(
        &self,
        interp: &mut Interpreter,
        timeout: Option<Duration>,
    ) -> Result<bool> {
        let gil = Arc::clone(&interp.cold.gil_state);
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_COND_WAIT])?;
                        write_opt_timeout(s, timeout)?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(b[0] != 0)
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(interp.release_gil_during(|| {
            core.cond_wait(timeout, || {
                gil.is_shutting_down() && current_thread_is_daemon_worker()
            })
        }))
    }

    pub(crate) fn cond_notify(&self, interp: &mut Interpreter, n: usize) -> Result<()> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_COND_NOTIFY])?;
                        s.write_all(&(n as u64).to_be_bytes())?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(())
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        core.cond_notify(n);
        Ok(())
    }

    pub(crate) fn cond_notify_all(&self, interp: &mut Interpreter) -> Result<()> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_COND_NOTIFY_ALL])?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(())
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        core.cond_notify_all();
        Ok(())
    }

    // ── Barrier ──────────────────────────────────────────────────────────────

    pub(crate) fn barrier_wait(
        &self,
        interp: &mut Interpreter,
        timeout: Option<Duration>,
    ) -> std::result::Result<usize, SyncError> {
        let gil = Arc::clone(&interp.cold.gil_state);
        if let Some(ep) = self.remote_ep() {
            return interp.release_gil_during(|| {
                Self::rpc(&ep, |s| {
                    s.write_all(&[OP_BARRIER_WAIT])?;
                    write_opt_timeout(s, timeout)?;
                    s.flush()?;
                    let mut code = [0u8; 1];
                    s.read_exact(&mut code)?;
                    if code[0] != RESP_OK {
                        return Ok(Err(SyncError::BrokenBarrier));
                    }
                    let mut idx_buf = [0u8; 8];
                    s.read_exact(&mut idx_buf)?;
                    Ok(Ok(u64::from_be_bytes(idx_buf) as usize))
                })
                .map_err(SyncError::Io)?
            });
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        interp.release_gil_during(|| {
            core.barrier_wait(timeout, || {
                gil.is_shutting_down() && current_thread_is_daemon_worker()
            })
        })
    }

    pub(crate) fn barrier_reset(&self, interp: &mut Interpreter) -> Result<()> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_BARRIER_RESET])?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(())
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        core.barrier_reset();
        Ok(())
    }

    pub(crate) fn barrier_abort(&self, interp: &mut Interpreter) -> Result<()> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_BARRIER_ABORT])?;
                        s.flush()?;
                        let mut b = [0u8; 1];
                        s.read_exact(&mut b)?;
                        Ok(())
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        core.barrier_abort();
        Ok(())
    }

    pub(crate) fn barrier_info(&self, interp: &mut Interpreter) -> Result<(usize, usize, bool)> {
        if let Some(ep) = self.remote_ep() {
            return interp
                .release_gil_during(|| {
                    Self::rpc(&ep, |s| {
                        s.write_all(&[OP_BARRIER_INFO])?;
                        s.flush()?;
                        let mut out = [0u8; 17];
                        s.read_exact(&mut out)?;
                        let parties = u64::from_be_bytes([
                            out[0], out[1], out[2], out[3], out[4], out[5], out[6], out[7],
                        ]) as usize;
                        let waiting = u64::from_be_bytes([
                            out[8], out[9], out[10], out[11], out[12], out[13], out[14], out[15],
                        ]) as usize;
                        let broken = out[16] != 0;
                        Ok((parties, waiting, broken))
                    })
                })
                .map_err(|e| PyError::from_io_error(&e, None));
        }
        let Self::Local(core) = self else {
            unreachable!();
        };
        Ok(core.barrier_info())
    }
}
