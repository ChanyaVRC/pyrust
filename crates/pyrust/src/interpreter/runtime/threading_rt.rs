//! Per-interpreter cooperative GIL and real OS-thread execution runtime (issue #3263).
//!
//! # Architecture & Soundness
//!
//! PyRust represents Python objects with non-atomic `Rc` / `RefCell` / `Cell`
//! types so single-threaded execution has zero atomic or locking overhead.
//! When Python code spawns threads via `threading.Thread.start()` or
//! `_thread.start_new_thread()`, all OS threads belonging to one root
//! interpreter coordinate through a single [`GilState`] (`Arc<GilState>`):
//!
//! 1. **Zero single-threaded overhead**: Before any worker thread is started,
//!    `GilState::multi_threaded` is `false` and `VM_HOOK_THREADING` is clear on
//!    `Interpreter::vm_hooks_flags`. The hot VM loop, lock-free `Value` ops,
//!    and single-threaded sleeps pay zero mutex operations.
//! 2. **Strict GIL serialization**: Once a worker thread is started, ONLY the
//!    single OS thread holding `GilState::inner` (`holder == Some(current_id)`)
//!    may access `Interpreter`, `Value`, `Rc`/`RefCell`, or `HotTlsState`.
//!    Every GIL handoff passes through `Mutex` unlock -> lock, providing a full
//!    acquire/release happens-before edge across all non-atomic refcounts and
//!    mutations.
//! 3. **Per-thread `Interpreter` handle**: Each spawned worker OS thread owns
//!    its own `Interpreter` struct (separate `tramp_arena`, `tramp_stack`,
//!    `gen_drive_stack`, `vm_frame_views`, `active_exception`, `env`) sharing
//!    interpreter-wide state (`module_cache`, `sys_state`, `warnings_state`,
//!    `atexit_state`, `gc_state`, `gil_state`, etc.) via `Rc`/`Arc` — the exact
//!    same pattern used by filesystem-import child interpreters in `modules.rs`.
//!    There is no cross-thread raw `*mut Interpreter` pointer.
//! 4. **Shared GIL TLS**: While holding the GIL, worker OS threads install a
//!    `pyrust_core::GilTlsGuard` pointing at the root thread's `HotTlsOwner` so
//!    all canonical classes (`int`, `str`, `Exception`), weakrefs, `structseq`
//!    specs, and class-mutation epochs are identical across all Python threads
//!    while staying isolated across parallel Rust test threads.
//! 5. **Cooperative preemption & blocking release**: Blocking operations
//!    (`time.sleep`, `Lock.acquire`, `Thread.join`, `Condition.wait`,
//!    `Future.result`, subprocess waits, socket I/O) release the GIL via
//!    [`Interpreter::release_gil_during`], and CPU-bound bytecode loops check a
//!    cold `VM_HOOK_THREADING` safepoint to yield fairly according to
//!    `sys.getswitchinterval()`.
//! 6. **Clean shutdown**: Before interpreter exit, non-daemon threads are
//!    joined while releasing the GIL; daemon threads are then signaled via
//!    `shutting_down` so sleeping/waiting/looping daemons unwind cleanly while
//!    holding the GIL. Any daemon stuck in an uninterruptible OS syscall parks
//!    safely on GIL re-acquisition instead of unwinding without the GIL.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use pyrust_core::{GilTlsGuard, HotTlsOwner, active_tls, home_tls_owner};

use crate::error::{PyError, Result};
use crate::interpreter::{
    AtexitCallback, FaulthandlerRuntimeState, GcRuntimeState, Interpreter, InterpreterColdState,
    MonitoringRuntimeState, RuntimeConfig, SharedWarningsState, SysRuntimeState,
    TracemallocRuntimeState, get_int_max_str_digits,
};
use crate::value::{EnvRef, Value};

/// Default virtual stack reservation for spawned worker OS threads.
///
/// Windows defaults to 1 MiB, which debug-build Python frames can quickly
/// exhaust; virtual stack pages are committed lazily by the OS.
#[cfg(debug_assertions)]
const DEFAULT_WORKER_STACK_SIZE: usize = 32 * 1024 * 1024;
#[cfg(not(debug_assertions))]
const DEFAULT_WORKER_STACK_SIZE: usize = 8 * 1024 * 1024;

/// Polling slice for interruptible unbounded waits so daemon threads notice
/// interpreter shutdown promptly without holding any global lock.
const SHUTDOWN_POLL_SLICE: Duration = Duration::from_millis(20);

/// Number of VM bytecode instructions between cold threading safepoint checks.
const THREAD_SAFEPOINT_TICK_INTERVAL: u32 = 256;

thread_local! {
    /// Per-OS-thread countdown until the next GIL yield / shutdown check.
    static SAFEPOINT_TICKS: Cell<u32> = const { Cell::new(THREAD_SAFEPOINT_TICK_INTERVAL) };
    /// Timestamp of the last GIL acquire / yield on this OS thread.
    static LAST_SWITCH_INSTANT: Cell<Option<Instant>> = const { Cell::new(None) };
    /// True only on OS threads spawned by `spawn_python_thread`.
    static IS_WORKER_THREAD: Cell<bool> = const { Cell::new(false) };
    /// True if the current OS thread is a daemon Python worker thread.
    static IS_DAEMON_WORKER: Cell<bool> = const { Cell::new(false) };
    /// Cached positive 63-bit thread identifier for the current OS thread.
    static CACHED_THREAD_IDENT: Cell<i64> = const { Cell::new(0) };
}

/// Return a stable, strictly positive 63-bit identifier for the current OS thread.
#[inline]
pub(crate) fn current_os_thread_ident() -> i64 {
    CACHED_THREAD_IDENT.with(|cell| {
        let cached = cell.get();
        if cached != 0 {
            return cached;
        }
        let mut hasher = DefaultHasher::new();
        thread::current().id().hash(&mut hasher);
        let raw = (hasher.finish() & 0x7fff_ffff_ffff_ffff) as i64;
        let ident = if raw == 0 { 1 } else { raw };
        cell.set(ident);
        ident
    })
}

/// Return a positive OS native thread ID when available, falling back to
/// [`current_os_thread_ident`].
#[inline]
pub(crate) fn current_os_native_id() -> i64 {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `syscall(SYS_gettid)` has no memory-safety preconditions and
        // returns the calling thread's kernel TID.
        let tid = unsafe { libc::syscall(libc::SYS_gettid) } as i64;
        if tid > 0 {
            return tid;
        }
    }
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        // SAFETY: Win32 `GetCurrentThreadId` has no preconditions and returns
        // the calling thread's unique 32-bit OS thread ID.
        let tid = unsafe { GetCurrentThreadId() } as i64;
        if tid > 0 {
            return tid;
        }
    }
    current_os_thread_ident()
}

/// Return true if the current OS thread is a spawned daemon Python thread.
#[inline]
pub(crate) fn current_thread_is_daemon_worker() -> bool {
    IS_DAEMON_WORKER.with(Cell::get)
}

struct GilInner {
    holder: Option<ThreadId>,
    waiters: usize,
    /// When a thread cooperatively yields while other threads are waiting,
    /// record its `ThreadId` and the current `epoch` so at least one waiter
    /// acquires the GIL before the yielding thread re-acquires.
    yielding_thread: Option<(ThreadId, u64)>,
    epoch: u64,
    active_workers: usize,
    shutting_down: bool,
    interpreter_dropped: bool,
}

/// Shared cooperative GIL and thread lifecycle coordinator for one root
/// interpreter and all of its spawned Python threads.
pub(crate) struct GilState {
    inner: Mutex<GilInner>,
    cvar: Condvar,
    all_workers_cvar: Condvar,
    shutdown_lock: Mutex<bool>,
    shutdown_cvar: Condvar,
    multi_threaded: AtomicBool,
    has_waiters: AtomicBool,
    shutting_down: AtomicBool,
    active_worker_count: AtomicUsize,
    tls_owner: Arc<HotTlsOwner>,
    non_daemon_joins: Mutex<Vec<JoinHandle<()>>>,
}

impl GilState {
    pub(crate) fn new_for_current_thread() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(GilInner {
                holder: None,
                waiters: 0,
                yielding_thread: None,
                epoch: 0,
                active_workers: 0,
                shutting_down: false,
                interpreter_dropped: false,
            }),
            cvar: Condvar::new(),
            all_workers_cvar: Condvar::new(),
            shutdown_lock: Mutex::new(false),
            shutdown_cvar: Condvar::new(),
            multi_threaded: AtomicBool::new(false),
            has_waiters: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            active_worker_count: AtomicUsize::new(0),
            tls_owner: home_tls_owner(),
            non_daemon_joins: Mutex::new(Vec::new()),
        })
    }

    #[inline(always)]
    pub(crate) fn is_multi_threaded(&self) -> bool {
        self.multi_threaded.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub(crate) fn has_active_workers(&self) -> bool {
        self.active_worker_count.load(Ordering::Relaxed) > 0
    }

    #[inline(always)]
    pub(crate) fn active_worker_count_val(&self) -> usize {
        self.active_worker_count.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub(crate) fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub(crate) fn has_waiters(&self) -> bool {
        self.has_waiters.load(Ordering::Relaxed)
    }

    /// Sleep for `dur` without holding the GIL, waking early if interpreter
    /// shutdown is triggered on a daemon thread.
    pub(crate) fn sleep_interruptible(&self, dur: Duration) -> bool {
        if dur.is_zero() {
            return !self.is_shutting_down();
        }
        let deadline = Instant::now() + dur;
        let mut guard = self
            .shutdown_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if *guard && current_thread_is_daemon_worker() {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            let rem = deadline - now;
            let (next_guard, _) = self
                .shutdown_cvar
                .wait_timeout(guard, rem)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard = next_guard;
        }
    }

    /// Acquire the GIL for the calling OS thread.
    ///
    /// If `self.inner.interpreter_dropped` is true on a worker thread (meaning
    /// the root interpreter already exited its shutdown window), the worker
    /// parks safely rather than touching any shared `Rc`/`HotTlsState`.
    fn acquire_gil(&self, after_yield_epoch: Option<u64>) -> GilTlsGuard {
        let me = thread::current().id();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.waiters += 1;
        self.has_waiters.store(true, Ordering::Relaxed);
        loop {
            if inner.interpreter_dropped && IS_WORKER_THREAD.with(Cell::get) {
                // Never return without GIL ownership after root interpreter drop.
                inner = self
                    .cvar
                    .wait(inner)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            let must_defer = matches!(
                (after_yield_epoch, inner.yielding_thread),
                (Some(ep), Some((ytid, yep))) if ytid == me && yep == ep && inner.waiters > 1
            );
            if inner.holder.is_none() && !must_defer {
                inner.holder = Some(me);
                inner.waiters -= 1;
                inner.epoch = inner.epoch.wrapping_add(1);
                if inner.yielding_thread.is_some_and(|(ytid, _)| ytid != me) || inner.waiters == 0 {
                    inner.yielding_thread = None;
                }
                self.has_waiters.store(inner.waiters > 0, Ordering::Relaxed);
                LAST_SWITCH_INSTANT.with(|c| c.set(Some(Instant::now())));
                // SAFETY: We now exclusively hold the GIL (`inner.holder == Some(me)`).
                return unsafe { GilTlsGuard::enter(&self.tls_owner) };
            }
            inner = self
                .cvar
                .wait(inner)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Release the GIL held by the calling OS thread.
    fn release_gil(&self, mark_yield: bool) -> Option<u64> {
        let me = thread::current().id();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        debug_assert_eq!(
            inner.holder,
            Some(me),
            "release_gil called without holding GIL"
        );
        inner.holder = None;
        let yield_epoch = if mark_yield && inner.waiters > 0 {
            let ep = inner.epoch;
            inner.yielding_thread = Some((me, ep));
            Some(ep)
        } else {
            None
        };
        self.cvar.notify_all();
        yield_epoch
    }

    /// Transition the GIL to multi-threaded mode (called by the spawning thread
    /// while it already holds exclusive interpreter access) and register a new
    /// pending worker.
    fn register_worker_spawn(&self) {
        let me = thread::current().id();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.multi_threaded.load(Ordering::Relaxed) {
            inner.holder = Some(me);
            self.multi_threaded.store(true, Ordering::Release);
            LAST_SWITCH_INSTANT.with(|c| c.set(Some(Instant::now())));
        }
        inner.active_workers += 1;
        self.active_worker_count
            .store(inner.active_workers, Ordering::Relaxed);
    }

    /// Mark one worker thread as completely finished (after dropping its
    /// `Interpreter` and all Python `Value`s) and release the GIL if `release`
    /// is true.
    fn finish_worker(&self, release: bool) {
        let me = thread::current().id();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.active_workers = inner.active_workers.saturating_sub(1);
        self.active_worker_count
            .store(inner.active_workers, Ordering::Relaxed);
        if release && inner.holder == Some(me) {
            inner.holder = None;
            self.cvar.notify_all();
        }
        if inner.active_workers == 0 {
            self.all_workers_cvar.notify_all();
        }
    }
}

/// RAII guard that releases the GIL on construction and unconditionally
/// re-acquires it on drop (even if the wrapped blocking closure panics).
struct GilReleaseGuard<'a> {
    gil: &'a GilState,
    prev_tls: *const pyrust_core::HotTlsState,
    yield_epoch: Option<u64>,
}

impl<'a> GilReleaseGuard<'a> {
    fn new(gil: &'a GilState, mark_yield: bool) -> Self {
        let prev_tls = active_tls() as *const pyrust_core::HotTlsState;
        let yield_epoch = gil.release_gil(mark_yield);
        // Clear ACTIVE_TLS while GIL is released so no accidental TLS touch can occur.
        // Note: on drop, `acquire_gil` re-installs `gil.tls_owner`, which is identical
        // to `prev_tls`.
        Self {
            gil,
            prev_tls,
            yield_epoch,
        }
    }
}

impl Drop for GilReleaseGuard<'_> {
    fn drop(&mut self) {
        let guard = self.gil.acquire_gil(self.yield_epoch);
        // Keep `ACTIVE_TLS` pointing to `self.prev_tls` (which equals `gil.tls_owner`).
        // Leak the temporary guard after restoring because the outer scope already
        // owns the enclosing TLS lifetime.
        let _ = self.prev_tls;
        std::mem::forget(guard);
    }
}

/// Wrapper for transferring `!Send` interpreter initialization payloads across
/// `std::thread::spawn` strictly under GIL serialization.
struct GilSend<T>(T);

impl<T> GilSend<T> {
    #[inline]
    fn into_inner(self) -> T {
        self.0
    }
}

// SAFETY: Every `GilSend<T>` is constructed by a thread holding the `GilState`
// mutex and is ONLY unpacked, accessed, or dropped by the receiving thread
// AFTER that thread has acquired the exact same `GilState` mutex (and installed
// `GilTlsGuard`).
unsafe impl<T> Send for GilSend<T> {}

/// Shared interpreter state transferred from the spawning thread to a newly
/// created worker OS thread.
struct WorkerInitBundle {
    bootstrap_fn: Value,
    root_env: EnvRef,
    warnings_state: SharedWarningsState,
    sys_state: Rc<RefCell<SysRuntimeState>>,
    script_dir: Option<PathBuf>,
    script_filename: Option<Arc<str>>,
    script_argv: Vec<String>,
    module_cache: Rc<RefCell<std::collections::HashMap<String, Value>>>,
    bootstrap_module_registry: Value,
    atexit_state: Rc<RefCell<Vec<Option<AtexitCallback>>>>,
    gc_state: Rc<RefCell<GcRuntimeState>>,
    tracemalloc_state: Rc<RefCell<TracemallocRuntimeState>>,
    faulthandler_state: Rc<RefCell<FaulthandlerRuntimeState>>,
    monitoring_state: Rc<RefCell<MonitoringRuntimeState>>,
    runtime_config: Rc<RuntimeConfig>,
    recursion_limit: usize,
    int_max_str_digits: usize,
}

/// Completion synchronization handle for a spawned OS thread (`Thread.join`).
pub(crate) struct ThreadJoinState {
    done: Mutex<bool>,
    cvar: Condvar,
}

impl ThreadJoinState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            done: Mutex::new(false),
            cvar: Condvar::new(),
        })
    }

    fn mark_done(&self) {
        let mut g = self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *g = true;
        self.cvar.notify_all();
    }

    pub(crate) fn is_done(&self) -> bool {
        *self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Block the calling Python thread (releasing the GIL) until this thread
    /// finishes or `timeout` seconds elapse. Returns `true` if finished.
    pub(crate) fn join(&self, interp: &mut Interpreter, timeout: Option<f64>) -> Result<bool> {
        if self.is_done() {
            return Ok(true);
        }
        let deadline = timeout.and_then(|secs| {
            if secs.is_infinite() && secs.is_sign_positive() {
                None
            } else if secs <= 0.0 || secs.is_nan() {
                Some(Instant::now())
            } else {
                Some(Instant::now() + Duration::from_secs_f64(secs))
            }
        });
        let gil = Arc::clone(&interp.cold.gil_state);
        let interrupted = interp.release_gil_during(|| {
            let mut g = self
                .done
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if *g {
                    return Ok(true);
                }
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    return Err(());
                }
                let wait_dur = match deadline {
                    Some(dl) => {
                        let now = Instant::now();
                        if now >= dl {
                            return Ok(false);
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
        });
        match interrupted {
            Ok(done) => Ok(done),
            Err(()) => Err(PyError::named("SystemExit", "")),
        }
    }
}

/// Real OS-thread-safe, GIL-releasing non-reentrant lock backing
/// `threading.Lock`, `Condition`, `Event`, `Semaphore`, `Barrier`, and `queue`.
pub(crate) struct NativeLock {
    locked: Mutex<bool>,
    cvar: Condvar,
}

impl NativeLock {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            locked: Mutex::new(false),
            cvar: Condvar::new(),
        })
    }

    #[inline]
    pub(crate) fn is_locked(&self) -> bool {
        *self
            .locked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn reset_unlocked(&self) {
        let mut g = self
            .locked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *g = false;
        self.cvar.notify_all();
    }

    pub(crate) fn release(&self) -> Result<()> {
        let mut g = self
            .locked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*g {
            return Err(PyError::named("RuntimeError", "release unlocked lock"));
        }
        *g = false;
        self.cvar.notify_one();
        Ok(())
    }

    /// Acquire the lock with CPython `Lock.acquire(blocking, timeout)` semantics,
    /// releasing the GIL whenever blocking is required.
    pub(crate) fn acquire(
        &self,
        interp: &mut Interpreter,
        blocking: bool,
        timeout: Option<f64>,
    ) -> Result<bool> {
        // Fast uncontended path: try acquiring without touching the GIL.
        {
            let mut g = self
                .locked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !*g {
                *g = true;
                return Ok(true);
            }
        }
        if !blocking {
            return Ok(false);
        }
        let deadline = match timeout {
            None => None,
            Some(secs) if secs <= 0.0 => return Ok(false),
            Some(secs) if secs.is_infinite() => None,
            Some(secs) => Some(Instant::now() + Duration::from_secs_f64(secs)),
        };

        let gil = Arc::clone(&interp.cold.gil_state);
        // Single-threaded deadlock safeguard: if no other worker threads exist
        // and no timeout was given, waiting forever would hang the test runner.
        if !gil.has_active_workers() && deadline.is_none() {
            return Err(PyError::named(
                "RuntimeError",
                "cannot acquire already-locked Lock without timeout when no other threads are running",
            ));
        }

        let res = interp.release_gil_during(|| {
            let mut g = self
                .locked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if !*g {
                    *g = true;
                    return Ok(true);
                }
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    return Err(());
                }
                let wait_dur = match deadline {
                    Some(dl) => {
                        let now = Instant::now();
                        if now >= dl {
                            return Ok(false);
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
        });
        match res {
            Ok(acquired) => Ok(acquired),
            Err(()) => Err(PyError::named("SystemExit", "")),
        }
    }
}

impl Interpreter {
    /// Execute a blocking Rust closure `f` with this interpreter's GIL released.
    ///
    /// - In single-threaded mode (zero spawned threads), executes `f()` directly
    ///   with zero mutex overhead.
    /// - In multi-threaded mode, releases the GIL before calling `f()` and
    ///   re-acquires it before returning (even if `f()` panics).
    #[inline]
    pub(crate) fn release_gil_during<T>(&mut self, f: impl FnOnce() -> T) -> T {
        if !self.cold.gil_state.is_multi_threaded() {
            return f();
        }
        let gil = Arc::clone(&self.cold.gil_state);
        let result = {
            let _release = GilReleaseGuard::new(&gil, false);
            f()
        };
        self.sync_vm_hooks_flags();
        result
    }

    /// Cooperatively yield the GIL if another thread is waiting to run.
    #[cold]
    #[inline(never)]
    pub(crate) fn yield_gil_if_needed(&mut self) -> Result<()> {
        let gil = Arc::clone(&self.cold.gil_state);
        if gil.is_shutting_down() && current_thread_is_daemon_worker() {
            return Err(PyError::named("SystemExit", ""));
        }
        if !gil.has_active_workers() {
            self.sync_vm_hooks_flags();
            return Ok(());
        }
        if !gil.has_waiters() {
            return Ok(());
        }
        let switch_secs = self.sys_state.borrow().switch_interval.max(0.000_1);
        let should_yield = LAST_SWITCH_INSTANT.with(|c| match c.get() {
            None => true,
            Some(ts) => ts.elapsed().as_secs_f64() >= switch_secs,
        });
        if !should_yield {
            return Ok(());
        }
        {
            let _release = GilReleaseGuard::new(&gil, true);
            thread::yield_now();
        }
        self.sync_vm_hooks_flags();
        if gil.is_shutting_down() && current_thread_is_daemon_worker() {
            return Err(PyError::named("SystemExit", ""));
        }
        Ok(())
    }

    /// Cold VM step hook invoked when `VM_HOOK_THREADING` is active.
    #[cold]
    #[inline(never)]
    pub(crate) fn vm_threading_safepoint(&mut self) -> Result<()> {
        let ticks = SAFEPOINT_TICKS.with(|c| {
            let next = c.get().saturating_sub(1);
            if next == 0 {
                c.set(THREAD_SAFEPOINT_TICK_INTERVAL);
                0
            } else {
                c.set(next);
                next
            }
        });
        if ticks == 0 {
            self.yield_gil_if_needed()?;
        }
        Ok(())
    }

    /// Sleep for `dur` seconds, releasing the GIL so other Python threads run
    /// concurrently and waking promptly if a daemon thread is shutting down.
    pub(crate) fn sleep_releasing_gil(&mut self, dur: Duration) -> Result<()> {
        if dur.is_zero() {
            if self.cold.gil_state.is_multi_threaded() && self.cold.gil_state.has_waiters() {
                let gil = Arc::clone(&self.cold.gil_state);
                let _release = GilReleaseGuard::new(&gil, true);
                thread::yield_now();
            }
            return Ok(());
        }
        if !self.cold.gil_state.is_multi_threaded() {
            thread::sleep(dur);
            return Ok(());
        }
        let gil = Arc::clone(&self.cold.gil_state);
        let completed = self.release_gil_during(|| gil.sleep_interruptible(dur));
        if !completed && current_thread_is_daemon_worker() {
            return Err(PyError::named("SystemExit", ""));
        }
        Ok(())
    }

    /// Spawn a real OS thread sharing this interpreter's runtime state under
    /// the cooperative GIL and executing `bootstrap_fn()`.
    pub(crate) fn spawn_python_thread(
        &mut self,
        bootstrap_fn: Value,
        is_daemon: bool,
        requested_stack_size: usize,
    ) -> Result<Arc<ThreadJoinState>> {
        let gil = Arc::clone(&self.cold.gil_state);
        gil.register_worker_spawn();
        self.sync_vm_hooks_flags();

        // Walk to root module env so worker threads resolve module globals cleanly.
        let mut root_env = Rc::clone(&self.env);
        loop {
            let parent = root_env.borrow().parent.clone();
            match parent {
                Some(p) => root_env = p,
                None => break,
            }
        }

        let bundle = GilSend(WorkerInitBundle {
            bootstrap_fn,
            root_env,
            warnings_state: Rc::clone(&self.warnings_state),
            sys_state: Rc::clone(&self.sys_state),
            script_dir: self.cold.script_dir.clone(),
            script_filename: self.cold.script_filename.clone(),
            script_argv: self.cold.script_argv.clone(),
            module_cache: Rc::clone(&self.cold.module_cache),
            bootstrap_module_registry: self.cold.bootstrap_module_registry.clone(),
            atexit_state: Rc::clone(&self.cold.atexit_state),
            gc_state: Rc::clone(&self.cold.gc_state),
            tracemalloc_state: Rc::clone(&self.cold.tracemalloc_state),
            faulthandler_state: Rc::clone(&self.cold.faulthandler_state),
            monitoring_state: Rc::clone(&self.cold.monitoring_state),
            runtime_config: Rc::clone(&self.cold.runtime_config),
            recursion_limit: self.recursion_limit,
            int_max_str_digits: get_int_max_str_digits(self),
        });

        let join_state = ThreadJoinState::new();
        let worker_join_state = Arc::clone(&join_state);
        let worker_gil = Arc::clone(&gil);

        let stack_size = if requested_stack_size > 0 {
            requested_stack_size.max(DEFAULT_WORKER_STACK_SIZE)
        } else {
            DEFAULT_WORKER_STACK_SIZE
        };

        let spawn_res = thread::Builder::new()
            .stack_size(stack_size)
            .spawn(move || {
                IS_WORKER_THREAD.with(|c| c.set(true));
                IS_DAEMON_WORKER.with(|c| c.set(is_daemon));

                // Acquire the GIL and install shared TLS BEFORE touching `bundle`.
                let _tls_guard = worker_gil.acquire_gil(None);

                // Unwrap the bundle strictly while holding the GIL.
                let init = bundle.into_inner();
                let mut cold = InterpreterColdState::new_with_gil(Arc::clone(&worker_gil), false);
                cold.script_dir = init.script_dir;
                cold.script_filename = init.script_filename;
                cold.script_argv = init.script_argv;
                cold.module_cache = init.module_cache;
                cold.bootstrap_module_registry = init.bootstrap_module_registry;
                cold.atexit_state = init.atexit_state;
                cold.gc_state = init.gc_state;
                cold.tracemalloc_state = init.tracemalloc_state;
                cold.faulthandler_state = init.faulthandler_state;
                cold.monitoring_state = init.monitoring_state;
                cold.runtime_config = init.runtime_config;
                let mut worker_interp = Interpreter::new_worker(
                    init.root_env,
                    init.warnings_state,
                    init.sys_state,
                    init.int_max_str_digits,
                    init.recursion_limit,
                    Box::new(cold),
                );
                worker_interp.sync_vm_hooks_flags();

                let _digit_guard = super::enter_int_max_str_digits_guard(&worker_interp);

                if !worker_gil.is_shutting_down() || !is_daemon {
                    let _ = worker_interp.call_function_expanded(init.bootstrap_fn, &[]);
                } else {
                    drop(init.bootstrap_fn);
                }

                // Mark thread completion before dropping the interpreter so joiners wake promptly.
                worker_join_state.mark_done();

                // Drop all Python objects and `worker_interp` WHILE STILL HOLDING THE GIL!
                drop(_digit_guard);
                drop(worker_interp);

                // Release GIL and decrement active worker count before dropping `_tls_guard`.
                worker_gil.finish_worker(true);
            });

        match spawn_res {
            Ok(handle) => {
                if !is_daemon {
                    gil.non_daemon_joins
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(handle);
                }
                Ok(join_state)
            }
            Err(e) => {
                gil.finish_worker(false);
                self.sync_vm_hooks_flags();
                Err(PyError::named(
                    "RuntimeError",
                    format!("can't start new thread: {e}"),
                ))
            }
        }
    }

    /// Join all non-daemon Python threads belonging to this interpreter
    /// (releasing the GIL while waiting).
    pub fn join_non_daemon_threads(&mut self) {
        if !self.cold.gil_state.is_multi_threaded() {
            return;
        }
        let gil = Arc::clone(&self.cold.gil_state);
        loop {
            let handles: Vec<JoinHandle<()>> = {
                let mut list = gil
                    .non_daemon_joins
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                std::mem::take(&mut *list)
            };
            if handles.is_empty() {
                break;
            }
            self.release_gil_during(|| {
                for h in handles {
                    let _ = h.join();
                }
            });
        }
    }

    /// Shut down all Python threads belonging to this interpreter before exit:
    /// 1. Join every non-daemon thread (releasing the GIL while waiting).
    /// 2. Signal daemon threads to exit cleanly and give them a brief grace
    ///    window to unwind while holding the GIL.
    /// 3. Mark the interpreter dropped so any uninterruptible daemon parks
    ///    safely without touching shared `Rc`/`HotTlsState`.
    pub fn shutdown_threads(&mut self) {
        if !self.cold.gil_state.is_multi_threaded() {
            return;
        }
        let gil = Arc::clone(&self.cold.gil_state);

        // 1. Join all non-daemon threads.
        self.join_non_daemon_threads();

        // 2. Signal shutdown to any remaining daemon threads.
        {
            let mut flag = gil
                .shutdown_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *flag = true;
            gil.shutting_down.store(true, Ordering::Release);
            gil.shutdown_cvar.notify_all();
        }
        {
            let mut inner = gil
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.shutting_down = true;
            gil.cvar.notify_all();
        }

        // 3. Release the GIL briefly so waking daemon threads can unwind cleanly.
        if gil.has_active_workers() {
            let deadline = Instant::now() + Duration::from_millis(250);
            self.release_gil_during(|| {
                let mut inner = gil
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while inner.active_workers > 0 {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    let (next, _) = gil
                        .all_workers_cvar
                        .wait_timeout(inner, deadline - now)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    inner = next;
                }
            });
        }

        // 4. Seal the GIL so any remaining stuck daemon thread will park rather
        //    than dereferencing shared runtime state after shutdown.
        {
            let mut inner = gil
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.interpreter_dropped = true;
        }
        self.sync_vm_hooks_flags();
    }
}
