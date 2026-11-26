//! GIL-synchronized per-interpreter-group TLS store (`HotTlsState` & `gil_local!`).
//!
//! PyRust represents Python values with non-atomic `Rc`/`RefCell`/`Cell` types
//! so single-threaded execution pays zero atomic or mutex overhead.  When a
//! script spawns real OS threads via `threading.Thread`, all threads belonging
//! to the same root interpreter serialize access to Python objects through that
//! interpreter's cooperative GIL mutex.
//!
//! Because Rust's `std::thread_local!` is scoped to a single OS thread, shared
//! runtime singletons (`int`/`str`/`Exception` classes, weakref tables,
//! `structseq` registries, class mutation epoch, etc.) must be visible across
//! all OS threads sharing one GIL, while remaining completely isolated between
//! independent interpreters running on separate OS threads (e.g. parallel
//! `cargo test` workers).
//!
//! This module solves both requirements with zero single-threaded hot-path
//! overhead:
//! - Each root OS thread lazily allocates one heap-pinned [`HotTlsState`]
//!   wrapped in `Arc<HotTlsOwner>` and caches its raw pointer in an OS-TLS
//!   `Cell<*const HotTlsState>` (`ACTIVE_TLS`).
//! - Hot single-word counters (`class_mutation_epoch`, `live_weakref_count`,
//!   `has_namespace_aliases`, etc.) live at fixed inline offsets inside
//!   [`HotTlsState`]: one TLS pointer load + one field offset, zero atomics.
//! - Cold singleton/registry statics use [`gil_local!`], which indexes into
//!   `HotTlsState::slots` with a lazily assigned `usize` slot ID.
//! - When a spawned worker OS thread acquires the GIL, it installs a
//!   [`GilTlsGuard`] pointing its OS-thread `ACTIVE_TLS` at the root thread's
//!   `HotTlsOwner` (and restores it on GIL release).

use std::any::Any;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Next globally unique slot index for [`GilLocalKey`] entries.
static NEXT_GIL_LOCAL_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Sentinel indicating that a [`GilLocalKey`] has not yet been assigned a slot.
const UNASSIGNED_SLOT: usize = usize::MAX;

/// Shared per-interpreter-group TLS state.
///
/// Accessed exclusively by its creator OS thread in single-threaded mode, or
/// exclusively by whichever OS thread currently holds the owning interpreter's
/// GIL mutex in multi-threaded mode.
pub struct HotTlsState {
    /// Global class-mutation epoch counter (`class_epoch.rs`).
    pub class_mutation_epoch: Cell<u64>,
    /// Count of live weakrefs (`weakref_support.rs`).
    pub live_weakref_count: Cell<usize>,
    /// Count of live weakrefs with callbacks (`weakref_support.rs`).
    pub live_weakref_callback_count: Cell<usize>,
    /// Fast flag for pending weakref callbacks (`weakref_support.rs`).
    pub has_pending_weakref_callbacks: Cell<bool>,
    /// Monotonic weakref handle id generator (`weakref_support.rs`).
    pub next_weakref_handle_id: Cell<u64>,
    /// Fast gate for escaped namespace dicts (`root_namespace.rs`).
    pub has_namespace_aliases: Cell<bool>,
    /// Monotonic root namespace id generator (`root_namespace.rs`).
    pub next_root_namespace_id: Cell<u64>,
    /// Active observed dict/set mutation state count (`mutation_state.rs`).
    pub active_collection_mutation_states: Cell<usize>,
    /// Monotonic synthetic object id counter (`object_identity.rs`).
    pub next_obj_id: Cell<Option<u64>>,
    /// Monotonic NaN identity payload counter (`nanbox_strings.rs`).
    pub nan_identity_counter: Cell<u64>,
    /// Lazily-initialized [`GilLocalKey`] storage slots.
    ///
    /// Each slot holds a heap-pinned `Box<dyn Any>` whose interior address
    /// remains stable even if the outer `Vec` grows.
    slots: UnsafeCell<Vec<Option<Box<dyn Any>>>>,
}

impl HotTlsState {
    fn new() -> Self {
        Self {
            class_mutation_epoch: Cell::new(0),
            live_weakref_count: Cell::new(0),
            live_weakref_callback_count: Cell::new(0),
            has_pending_weakref_callbacks: Cell::new(false),
            next_weakref_handle_id: Cell::new(1),
            has_namespace_aliases: Cell::new(false),
            next_root_namespace_id: Cell::new(1),
            active_collection_mutation_states: Cell::new(0),
            next_obj_id: Cell::new(Some(1)),
            nan_identity_counter: Cell::new(0),
            slots: UnsafeCell::new(Vec::new()),
        }
    }
}

/// Owned, heap-pinned [`HotTlsState`] that can be shared across OS threads
/// belonging to one interpreter's GIL group.
pub struct HotTlsOwner {
    state: Box<HotTlsState>,
}

// SAFETY: `HotTlsOwner` is shared with worker OS threads ONLY alongside the
// owning interpreter's GIL mutex. Every read/write of `HotTlsState` (and any
// `!Send` Python object stored inside its slots) occurs exclusively while the
// accessing thread holds that GIL mutex (or before any worker thread exists),
// so Rust's `Mutex` acquire/release barrier establishes a strict happens-before
// total order across all accesses and drops.
unsafe impl Send for HotTlsOwner {}
unsafe impl Sync for HotTlsOwner {}

impl HotTlsOwner {
    fn new() -> Self {
        Self {
            state: Box::new(HotTlsState::new()),
        }
    }

    #[inline(always)]
    pub fn as_ptr(&self) -> *const HotTlsState {
        &*self.state as *const HotTlsState
    }
}

impl Drop for HotTlsOwner {
    fn drop(&mut self) {
        // Ensure `ACTIVE_TLS` is cleared (not dangling) before dropping slots,
        // and mark teardown active so `try_active_tls` returns `None` instead
        // of re-initializing during destructor execution.
        let self_ptr = self.as_ptr();
        let _ = TLS_TEARING_DOWN.try_with(|flag| flag.set(true));
        let _ = ACTIVE_TLS.try_with(|cell| {
            if cell.get() == self_ptr {
                cell.set(std::ptr::null());
            }
        });
        // Take all slots out before dropping so no reentrant lookup can touch
        // a partially-dropped Vec.
        let taken = std::mem::take(self.state.slots.get_mut());
        drop(taken);
        let _ = TLS_TEARING_DOWN.try_with(|flag| flag.set(false));
    }
}

thread_local! {
    /// Raw pointer to the currently active [`HotTlsState`] on this OS thread.
    ///
    /// - In single-threaded mode, initialized lazily to `HOME_TLS`.
    /// - On a worker thread holding a GIL, temporarily overridden by
    ///   [`GilTlsGuard`] to point at the root interpreter's [`HotTlsOwner`].
    static ACTIVE_TLS: Cell<*const HotTlsState> = const { Cell::new(std::ptr::null()) };

    /// True while a [`HotTlsOwner`] is running its destructor on this thread.
    static TLS_TEARING_DOWN: Cell<bool> = const { Cell::new(false) };

    /// This OS thread's own default [`HotTlsOwner`], created lazily on first use.
    static HOME_TLS: RefCell<Option<Arc<HotTlsOwner>>> = const { RefCell::new(None) };
}

#[cold]
#[inline(never)]
fn init_home_tls() -> *const HotTlsState {
    HOME_TLS.with(|cell| {
        let mut opt = cell.borrow_mut();
        let owner = opt.get_or_insert_with(|| Arc::new(HotTlsOwner::new()));
        let ptr = owner.as_ptr();
        ACTIVE_TLS.with(|active| active.set(ptr));
        ptr
    })
}

/// Return a reference to the currently active [`HotTlsState`].
///
/// Hot path: one OS-TLS pointer load + predicted-taken non-null check.
#[inline(always)]
pub fn active_tls() -> &'static HotTlsState {
    let ptr = ACTIVE_TLS.with(Cell::get);
    if !ptr.is_null() {
        // SAFETY: While `ACTIVE_TLS` is non-null, the pointed-to `HotTlsState`
        // is kept alive by either `HOME_TLS` or an enclosing `GilTlsGuard` /
        // `Arc<HotTlsOwner>`, and exclusive access is guaranteed by either
        // thread confinement or the GIL mutex.
        unsafe { &*ptr }
    } else {
        let ptr = init_home_tls();
        unsafe { &*ptr }
    }
}

/// Teardown-safe variant of [`active_tls`] for `Drop` implementations and
/// early-bootstrap call sites (e.g. NaN identity minting).
///
/// Lazily initializes `HOME_TLS` on first normal use; returns `None` only when
/// OS thread-local storage is already being destroyed.
#[inline(always)]
pub fn try_active_tls() -> Option<&'static HotTlsState> {
    let ptr = ACTIVE_TLS.try_with(Cell::get).ok()?;
    if !ptr.is_null() {
        // SAFETY: Same lifetime & GIL exclusivity guarantee as `active_tls`.
        return Some(unsafe { &*ptr });
    }
    if TLS_TEARING_DOWN.try_with(Cell::get).unwrap_or(true) {
        return None;
    }
    let ptr = HOME_TLS
        .try_with(|cell| {
            let mut opt = cell.borrow_mut();
            let owner = opt.get_or_insert_with(|| Arc::new(HotTlsOwner::new()));
            let ptr = owner.as_ptr();
            let _ = ACTIVE_TLS.try_with(|active| active.set(ptr));
            ptr
        })
        .ok()?;
    // SAFETY: Newly initialized `HOME_TLS` stays alive until thread exit.
    Some(unsafe { &*ptr })
}

/// Return the [`Arc<HotTlsOwner>`] backing the current OS thread's home TLS,
/// initializing it if needed.
///
/// Called by the root interpreter when setting up its GIL so all spawned
/// worker threads share this exact [`HotTlsOwner`].
pub fn home_tls_owner() -> Arc<HotTlsOwner> {
    HOME_TLS.with(|cell| {
        let mut opt = cell.borrow_mut();
        let owner = opt.get_or_insert_with(|| Arc::new(HotTlsOwner::new()));
        let ptr = owner.as_ptr();
        ACTIVE_TLS.with(|active| {
            if active.get().is_null() {
                active.set(ptr);
            }
        });
        Arc::clone(owner)
    })
}

/// RAII guard installing a shared [`HotTlsOwner`] as the current OS thread's
/// `ACTIVE_TLS` while the thread holds the owning interpreter's GIL.
#[must_use = "GilTlsGuard must remain alive while holding the GIL"]
pub struct GilTlsGuard {
    prev: *const HotTlsState,
}

impl GilTlsGuard {
    /// Install `owner` as this OS thread's active TLS context.
    ///
    /// # Safety
    ///
    /// The caller MUST hold the GIL mutex protecting `owner` for the entire
    /// lifetime of the returned guard, and `owner` must outlive the guard.
    #[inline]
    pub unsafe fn enter(owner: &HotTlsOwner) -> Self {
        let prev = ACTIVE_TLS.with(|cell| cell.replace(owner.as_ptr()));
        Self { prev }
    }
}

impl Drop for GilTlsGuard {
    #[inline]
    fn drop(&mut self) {
        let _ = ACTIVE_TLS.try_with(|cell| cell.set(self.prev));
    }
}

/// Error returned by [`GilLocalKey::try_with`] during OS thread teardown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GilTlsAccessError;

/// Static key for a lazily-initialized value stored in [`HotTlsState`].
///
/// Mirrors the `.with(...)` and `.try_with(...)` API of `std::thread::LocalKey`
/// so existing runtime singletons can migrate cleanly.
pub struct GilLocalKey<T: 'static> {
    slot: AtomicUsize,
    init: fn() -> T,
}

impl<T: 'static> GilLocalKey<T> {
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            slot: AtomicUsize::new(UNASSIGNED_SLOT),
            init,
        }
    }

    #[inline(always)]
    fn slot_id(&self) -> usize {
        let id = self.slot.load(Ordering::Relaxed);
        if id != UNASSIGNED_SLOT {
            return id;
        }
        self.assign_slot_cold()
    }

    #[cold]
    #[inline(never)]
    fn assign_slot_cold(&self) -> usize {
        let fresh = NEXT_GIL_LOCAL_SLOT.fetch_add(1, Ordering::Relaxed);
        match self.slot.compare_exchange(
            UNASSIGNED_SLOT,
            fresh,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => fresh,
            Err(existing) => existing,
        }
    }

    #[inline]
    fn get_or_init_ptr(&self, tls: &HotTlsState) -> *const T {
        let slot = self.slot_id();
        // Fast path: already initialized.
        {
            // SAFETY: Single-thread / GIL-serialized access; reference is
            // dropped before calling `self.init` or the user closure.
            let slots = unsafe { &*tls.slots.get() };
            if let Some(Some(boxed)) = slots.get(slot) {
                return boxed
                    .downcast_ref::<T>()
                    .expect("GilLocalKey slot type mismatch") as *const T;
            }
        }
        self.init_slot_cold(tls, slot)
    }

    #[cold]
    #[inline(never)]
    fn init_slot_cold(&self, tls: &HotTlsState, slot: usize) -> *const T {
        // Run initializer with NO borrow of `tls.slots` active so reentrant
        // accesses to other `GilLocalKey`s during initialization are sound.
        let value = (self.init)();
        // SAFETY: Single-thread / GIL-serialized access; mutable borrow is
        // confined strictly to inserting into the Vec and dropped immediately.
        let slots = unsafe { &mut *tls.slots.get() };
        if slots.len() <= slot {
            slots.resize_with(slot + 1, || None);
        }
        let entry = slots[slot].get_or_insert_with(|| Box::new(value));
        entry
            .downcast_ref::<T>()
            .expect("GilLocalKey slot type mismatch") as *const T
    }

    /// Borrow the value stored in the active GIL TLS context and pass it to `f`.
    #[inline]
    pub fn with<R>(&'static self, f: impl FnOnce(&T) -> R) -> R {
        let tls = active_tls();
        let ptr = self.get_or_init_ptr(tls);
        // SAFETY: `Box<dyn Any>` inside `tls.slots[slot]` has a stable heap
        // address that is never removed or moved until `HotTlsOwner::drop`,
        // even if `f` grows `tls.slots` by initializing other `GilLocalKey`s.
        f(unsafe { &*ptr })
    }

    /// Teardown-safe variant of [`Self::with`].
    ///
    /// Returns `Err(())` if `ACTIVE_TLS` is unavailable during OS thread
    /// teardown or the slot has not been initialized yet during teardown.
    #[inline]
    pub fn try_with<R>(&'static self, f: impl FnOnce(&T) -> R) -> Result<R, GilTlsAccessError> {
        let Some(tls) = try_active_tls() else {
            return Err(GilTlsAccessError);
        };
        let ptr = self.get_or_init_ptr(tls);
        // SAFETY: Same stable heap address guarantee as `with`.
        Ok(f(unsafe { &*ptr }))
    }
}

/// Declare one or more GIL-synchronized TLS statics backed by [`GilLocalKey`].
///
/// Drop-in replacement for `std::thread_local!` for runtime singletons and
/// registries that must be shared across Python threads under one GIL while
/// staying isolated across independent interpreters on separate OS threads.
#[macro_export]
macro_rules! gil_local {
    () => {};
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty = const { $init:expr }; $($rest:tt)*) => {
        $(#[$attr])*
        $vis static $name: $crate::GilLocalKey<$ty> = $crate::GilLocalKey::new(|| $init);
        $crate::gil_local!($($rest)*);
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr; $($rest:tt)*) => {
        $(#[$attr])*
        $vis static $name: $crate::GilLocalKey<$ty> = $crate::GilLocalKey::new(|| $init);
        $crate::gil_local!($($rest)*);
    };
}
