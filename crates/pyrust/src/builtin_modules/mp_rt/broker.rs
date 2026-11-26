//! Single-port per-process multiplexed loopback IPC broker (`GlobalIpcBroker`).
//!
//! Lazily binds ONE `127.0.0.1:0` `TcpListener` with a 16-byte CSPRNG auth
//! token per process. Registered cross-process primitives (`QueueCore`,
//! `SyncCore`, `PipeRelay`) receive unique `u64` entry IDs so spawned (Windows
//! & Unix) and forked (Unix) child processes can connect directly to the parent
//! process's live Rust cores without leaking OS named semaphores or exhausting
//! ephemeral ports.
//!
//! Every broker thread is pure Rust and NEVER touches the Python GIL.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::sync::Weak;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use super::ipc::{PipeRelayTarget, serve_pipe_relay};
use super::queue::{QueueCore, serve_queue_client};
use super::random_token_16;
use super::sync::{SyncCore, serve_sync_client};

/// Entry registered in the local process's IPC broker.
pub(crate) enum BrokerEntry {
    Queue(Arc<QueueCore>),
    Sync(Arc<SyncCore>),
    PipeRelay(Mutex<Option<PipeRelayTarget>>),
}

struct BrokerInner {
    owner_pid: u32,
    addr: SocketAddr,
    token: [u8; 16],
    entries: Arc<Mutex<HashMap<u64, Arc<BrokerEntry>>>>,
}

static NEXT_ENTRY_ID: AtomicU64 = AtomicU64::new(1);

/// Per-process broker state (protected by a `Mutex` so a Unix `fork()` child
/// detects `owner_pid != std::process::id()` and lazily creates its own fresh
/// broker if it spawns grandchildren).
static LOCAL_BROKER: OnceLock<Mutex<Option<Arc<BrokerInner>>>> = OnceLock::new();

/// Weak list of locally created `QueueCore`s and `SyncCore`s that may need to
/// be exported to the broker right before a Unix `fork()`.
#[cfg(unix)]
enum ForkExportable {
    Queue(Weak<QueueCore>),
    Sync(Weak<SyncCore>),
}

#[cfg(unix)]
static FORK_REGISTRY: OnceLock<Mutex<Vec<ForkExportable>>> = OnceLock::new();

/// Register a local `QueueCore` for automatic broker export before `fork()`.
#[cfg(unix)]
pub(crate) fn track_queue_for_fork(core: &Arc<QueueCore>) {
    let reg = FORK_REGISTRY.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut list) = reg.lock() {
        if list.len() > 256 {
            list.retain(|item| match item {
                ForkExportable::Queue(w) => w.strong_count() > 0,
                ForkExportable::Sync(w) => w.strong_count() > 0,
            });
        }
        list.push(ForkExportable::Queue(Arc::downgrade(core)));
    }
}

#[cfg(not(unix))]
pub(crate) fn track_queue_for_fork(_core: &Arc<QueueCore>) {}

/// Register a local `SyncCore` for automatic broker export before `fork()`.
#[cfg(unix)]
pub(crate) fn track_sync_for_fork(core: &Arc<SyncCore>) {
    let reg = FORK_REGISTRY.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut list) = reg.lock() {
        if list.len() > 256 {
            list.retain(|item| match item {
                ForkExportable::Queue(w) => w.strong_count() > 0,
                ForkExportable::Sync(w) => w.strong_count() > 0,
            });
        }
        list.push(ForkExportable::Sync(Arc::downgrade(core)));
    }
}

#[cfg(not(unix))]
pub(crate) fn track_sync_for_fork(_core: &Arc<SyncCore>) {}

/// Called right before `libc::fork()` on Unix: ensures every live local
/// `QueueCore` and `SyncCore` has a broker endpoint running in the parent so
/// the forked child can seamlessly reach the parent's cores over loopback TCP.
#[cfg(unix)]
pub(crate) fn pre_fork_prepare() {
    let Some(reg) = FORK_REGISTRY.get() else {
        return;
    };
    let items: Vec<ForkExportable> = {
        let mut list = reg
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        list.retain(|item| match item {
            ForkExportable::Queue(w) => w.strong_count() > 0,
            ForkExportable::Sync(w) => w.strong_count() > 0,
        });
        list.iter()
            .map(|item| match item {
                ForkExportable::Queue(w) => ForkExportable::Queue(Weak::clone(w)),
                ForkExportable::Sync(w) => ForkExportable::Sync(Weak::clone(w)),
            })
            .collect()
    };
    for item in items {
        match item {
            ForkExportable::Queue(w) => {
                if let Some(q) = w.upgrade() {
                    let _ = q.ensure_exported();
                }
            }
            ForkExportable::Sync(w) => {
                if let Some(s) = w.upgrade() {
                    let _ = s.ensure_exported();
                }
            }
        }
    }
}

/// Ensure this process's `GlobalIpcBroker` is running and return it.
fn ensure_broker() -> std::io::Result<Arc<BrokerInner>> {
    let cell = LOCAL_BROKER.get_or_init(|| Mutex::new(None));
    let mut guard = cell
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let pid = std::process::id();
    if let Some(ref b) = *guard
        && b.owner_pid == pid
    {
        return Ok(Arc::clone(b));
    }

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let token = random_token_16();
    let entries: Arc<Mutex<HashMap<u64, Arc<BrokerEntry>>>> = Arc::new(Mutex::new(HashMap::new()));

    let broker = Arc::new(BrokerInner {
        owner_pid: pid,
        addr,
        token,
        entries: Arc::clone(&entries),
    });

    let accept_entries = Arc::clone(&entries);
    thread::Builder::new()
        .name("pyrust-mp-broker".to_string())
        .spawn(move || {
            for incoming in listener.incoming() {
                let Ok(stream) = incoming else {
                    break;
                };
                let _ = stream.set_nodelay(true);
                let entries_ref = Arc::clone(&accept_entries);
                let _ = thread::Builder::new()
                    .name("pyrust-mp-client".to_string())
                    .spawn(move || {
                        handle_broker_connection(stream, token, entries_ref);
                    });
            }
        })?;

    *guard = Some(Arc::clone(&broker));
    Ok(broker)
}

fn handle_broker_connection(
    mut stream: TcpStream,
    expected_token: [u8; 16],
    entries: Arc<Mutex<HashMap<u64, Arc<BrokerEntry>>>>,
) {
    // Handshake: [16-byte token][8-byte BE entry_id]
    let mut hdr = [0u8; 24];
    if stream.read_exact(&mut hdr).is_err() {
        return;
    }
    if hdr[..16] != expected_token {
        return;
    }
    let entry_id = u64::from_be_bytes([
        hdr[16], hdr[17], hdr[18], hdr[19], hdr[20], hdr[21], hdr[22], hdr[23],
    ]);
    let entry = {
        let map = entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.get(&entry_id).cloned()
    };
    let Some(entry) = entry else {
        return;
    };

    match &*entry {
        BrokerEntry::Queue(q) => {
            serve_queue_client(stream, Arc::clone(q));
        }
        BrokerEntry::Sync(s) => {
            serve_sync_client(stream, Arc::clone(s));
        }
        BrokerEntry::PipeRelay(slot) => {
            let target = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            // Remove one-shot pipe relay entry from table so its socket drops
            // immediately once the relay completes.
            entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&entry_id);
            if let Some(target) = target {
                serve_pipe_relay(stream, target);
            }
        }
    }
}

/// Descriptor serialized across processes to connect back to a broker entry.
#[derive(Clone, Debug)]
pub(crate) struct BrokerEndpoint {
    pub addr: String,
    pub token: [u8; 16],
    pub entry_id: u64,
}

impl BrokerEndpoint {
    /// Open a fresh authenticated TCP connection to this broker entry.
    pub(crate) fn connect(&self) -> std::io::Result<TcpStream> {
        let mut stream = TcpStream::connect(&self.addr)?;
        let _ = stream.set_nodelay(true);
        let mut hdr = [0u8; 24];
        hdr[..16].copy_from_slice(&self.token);
        hdr[16..24].copy_from_slice(&self.entry_id.to_be_bytes());
        stream.write_all(&hdr)?;
        stream.flush()?;
        Ok(stream)
    }
}

/// Register a `QueueCore` with the local broker and return its endpoint.
pub(crate) fn register_queue(core: Arc<QueueCore>) -> std::io::Result<BrokerEndpoint> {
    let broker = ensure_broker()?;
    let id = NEXT_ENTRY_ID.fetch_add(1, Ordering::Relaxed);
    broker
        .entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, Arc::new(BrokerEntry::Queue(core)));
    Ok(BrokerEndpoint {
        addr: broker.addr.to_string(),
        token: broker.token,
        entry_id: id,
    })
}

/// Register a `SyncCore` with the local broker and return its endpoint.
pub(crate) fn register_sync(core: Arc<SyncCore>) -> std::io::Result<BrokerEndpoint> {
    let broker = ensure_broker()?;
    let id = NEXT_ENTRY_ID.fetch_add(1, Ordering::Relaxed);
    broker
        .entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, Arc::new(BrokerEntry::Sync(core)));
    Ok(BrokerEndpoint {
        addr: broker.addr.to_string(),
        token: broker.token,
        entry_id: id,
    })
}

/// Register a one-shot `PipeRelayTarget` with the local broker and return its
/// endpoint.
pub(crate) fn register_pipe_relay(target: PipeRelayTarget) -> std::io::Result<BrokerEndpoint> {
    let broker = ensure_broker()?;
    let id = NEXT_ENTRY_ID.fetch_add(1, Ordering::Relaxed);
    broker
        .entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            id,
            Arc::new(BrokerEntry::PipeRelay(Mutex::new(Some(target)))),
        );
    Ok(BrokerEndpoint {
        addr: broker.addr.to_string(),
        token: broker.token,
        entry_id: id,
    })
}
