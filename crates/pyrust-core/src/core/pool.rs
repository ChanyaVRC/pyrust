//! Thread-local fixed-size slab pools for NaN-boxed `Value` payloads.
//!
//! # Ownership & Thread Confinement
//!
//! [`Value`](crate::Value) is deliberately `!Send + !Sync`: its bit payloads
//! point into non-atomic `Rc`/`RefCell`/`Cell` allocations and the thread-local
//! free lists owned by this module. Keeping the free lists in `thread_local!`
//! storage matches `Value`'s thread-confined ownership model and keeps hot
//! slab allocation/deallocation completely free of atomics and locks.
//!
//! # Layout & Alignment Invariants
//!
//! Every pooled block stores an intrusive `*mut u8` next-pointer in its first
//! machine word while parked on a free list. Both pooled layouts (`POOL_B` for
//! 32-byte string-slice headers and `POOL_OPAQUE` for `OpaqueSlot` slabs) must
//! therefore satisfy `size >= size_of::<*mut u8>()` and
//! `align >= align_of::<*mut u8>()` (and `align >= 8` so returned addresses
//! preserve the NaN-box low-bit invariants).
//!
//! # TLS Drop-Order Safety
//!
//! Rust destroys `thread_local!` keys in reverse initialization order at thread
//! exit. When `POOL_B` or `POOL_OPAQUE` drops, [`drain_free_list`] clears the
//! free-list head/length *before* releasing blocks so no later destructor can
//! observe freed pointers. Conversely, if an older TLS key retains a `Value`
//! that drops *after* these pool keys are already destroyed, [`pool_b_alloc`],
//! [`pool_b_dealloc`], [`pool_opaque_alloc`], and [`pool_opaque_dealloc`] use
//! [`std::thread::LocalKey::try_with`] and fall back to direct global
//! allocation/deallocation instead of panicking on a destroyed `LocalKey`.
//!
//! # Multi-Interpreter / Multi-Threaded Impact
//!
//! - **Same thread, multiple interpreters**: Interpreters running on the same
//!   OS thread transparently share these bounded free lists. Because pooled
//!   blocks are uninitialized raw memory recycled purely as an allocator
//!   optimization, sharing carries no Python-observable state across
//!   interpreters.
//! - **Multiple threads**: Each OS thread owns an independent bounded pool
//!   (`POOL_B_CAP` / `POOL_OPAQUE_CAP` blocks max) that drains cleanly on
//!   thread exit with zero cross-thread contention.

use std::alloc::{Layout, alloc, dealloc};
use std::cell::Cell;

use crate::object_model::{STR_SLICE_LAYOUT_SIZE, opaque_layout};

pub(crate) type FreeListState = (*mut u8, usize);

/// Bounded capacity (in 32-byte blocks) of the per-thread string-slice pool.
pub(crate) const POOL_B_CAP: usize = 64;

/// Bounded capacity (in `OpaqueSlot` slabs) of the per-thread opaque-value pool (#281).
pub(crate) const POOL_OPAQUE_CAP: usize = 128;

#[inline(always)]
fn pool_b_layout() -> Layout {
    const {
        assert!(
            STR_SLICE_LAYOUT_SIZE >= std::mem::size_of::<*mut u8>(),
            "POOL_B blocks must be large enough to hold an intrusive next-pointer"
        );
        assert!(
            8 >= std::mem::align_of::<*mut u8>(),
            "POOL_B alignment must satisfy intrusive next-pointer alignment"
        );
    }
    Layout::from_size_align(STR_SLICE_LAYOUT_SIZE, 8).expect("valid string-slice pool layout")
}

/// Allocate a fresh NaN-box payload and apply Rust's standard allocation-error
/// policy before any caller can dereference the returned pointer.
#[inline(always)]
pub(crate) unsafe fn alloc_or_handle(layout: Layout) -> *mut u8 {
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    ptr
}

/// Test-only accounting keyed by the thread whose TLS pool was drained.
///
/// A process-wide before/after counter cannot attribute a drain to one test:
/// libtest may terminate another worker inside that measurement window.  Keep
/// the production pool path free of synchronization while making the teardown
/// assertion deterministic under parallel tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct ThreadExitDrainCounts {
    pub(crate) pool_b: usize,
    pub(crate) pool_opaque: usize,
}

#[cfg(test)]
static THREAD_EXIT_DRAIN_COUNTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<std::thread::ThreadId, ThreadExitDrainCounts>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(test)]
fn record_thread_exit_drains(update: impl FnOnce(&mut ThreadExitDrainCounts)) {
    let mut drains = THREAD_EXIT_DRAIN_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    update(drains.entry(std::thread::current().id()).or_default());
}

#[cfg(test)]
pub(crate) fn take_thread_exit_drains(thread_id: std::thread::ThreadId) -> ThreadExitDrainCounts {
    THREAD_EXIT_DRAIN_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&thread_id)
        .unwrap_or_default()
}

/// Deallocate every block currently retained by one thread-local free list.
///
/// The state is cleared before deallocation so a later destructor cannot
/// observe pointers that have already been released.
pub(crate) unsafe fn drain_free_list(state: &Cell<FreeListState>, layout: Layout) -> usize {
    let (mut head, expected_len) = state.replace((std::ptr::null_mut(), 0));
    let mut drained = 0;
    while !head.is_null() {
        let next = unsafe { *(head as *const *mut u8) };
        unsafe { dealloc(head, layout) };
        head = next;
        drained += 1;
    }
    debug_assert_eq!(
        drained, expected_len,
        "free-list length disagrees with linked blocks"
    );
    drained
}

struct PoolB(Cell<FreeListState>);

impl PoolB {
    const fn new() -> Self {
        Self(Cell::new((std::ptr::null_mut(), 0)))
    }
}

impl Drop for PoolB {
    fn drop(&mut self) {
        let _drained = unsafe { drain_free_list(&self.0, pool_b_layout()) };
        #[cfg(test)]
        record_thread_exit_drains(|counts| counts.pool_b += _drained);
    }
}

struct PoolOpaque(Cell<FreeListState>);

impl PoolOpaque {
    const fn new() -> Self {
        Self(Cell::new((std::ptr::null_mut(), 0)))
    }
}

impl Drop for PoolOpaque {
    fn drop(&mut self) {
        let _drained = unsafe { drain_free_list(&self.0, opaque_layout()) };
        #[cfg(test)]
        record_thread_exit_drains(|counts| counts.pool_opaque += _drained);
    }
}

// Thread-local justification:
// (a) Why `thread_local!`: `Value` is `!Send + !Sync`, and these fixed-size
//     intrusive LIFO free lists sit on the allocation/drop hot path where
//     locks or atomic CAS loops would erase the pooling win.
// (b) Multi-interpreter / multi-threaded impact: Multiple interpreters on one
//     thread safely share recycled raw memory (no semantic state leaks); each
//     OS thread owns independent bounded pools that drain cleanly on exit.
thread_local! {
    /// Free list of 32-byte Layout-B string-slice headers. Each free slot
    /// stores a `*mut u8` to the next free slot in its first 8 bytes.
    static POOL_B: PoolB = const { PoolB::new() };

    /// Free list of `OpaqueSlot` slabs (#281), recycled once the final aliased
    /// `Value` drops.
    static POOL_OPAQUE: PoolOpaque = const { PoolOpaque::new() };
}

#[inline(always)]
pub(crate) unsafe fn pool_b_alloc() -> *mut u8 {
    let layout = pool_b_layout();
    POOL_B
        .try_with(|c| {
            let (head, len) = c.0.get();
            if len > 0 {
                let next = unsafe { *(head as *const *mut u8) };
                c.0.set((next, len - 1));
                head
            } else {
                unsafe { alloc_or_handle(layout) }
            }
        })
        // A destructor from another TLS key may allocate after this pool has
        // already been destroyed. It cannot re-enter the free list, but a
        // matched direct allocation remains valid for the rest of that drop.
        .unwrap_or_else(|_| unsafe { alloc_or_handle(layout) })
}

#[inline(always)]
pub(crate) unsafe fn pool_b_dealloc(ptr: *mut u8) {
    let layout = pool_b_layout();
    if POOL_B
        .try_with(|c| {
            let (head, len) = c.0.get();
            // In debug builds catch double-free: a block already in the pool has its first
            // 8 bytes overwritten with the next-pointer, so it cannot equal any live
            // allocation's first word.  Check ptr != head as a lightweight guard; a full
            // traversal is too expensive for a hot path.
            debug_assert!(
                ptr != head || head.is_null(),
                "pool_b_dealloc: double-free detected (ptr == head)"
            );
            if len < POOL_B_CAP {
                unsafe { *(ptr as *mut *mut u8) = head };
                c.0.set((ptr, len + 1));
            } else {
                unsafe { dealloc(ptr, layout) };
            }
        })
        .is_err()
    {
        // TLS keys are destroyed in reverse initialization order. A value kept
        // by an older key can therefore outlive this pool and must bypass it.
        unsafe { dealloc(ptr, layout) };
    }
}

#[inline(always)]
pub(crate) unsafe fn pool_opaque_alloc() -> *mut u8 {
    let layout = opaque_layout();
    POOL_OPAQUE
        .try_with(|c| {
            let (head, len) = c.0.get();
            if len > 0 {
                let next = unsafe { *(head as *const *mut u8) };
                c.0.set((next, len - 1));
                head
            } else {
                unsafe { alloc_or_handle(layout) }
            }
        })
        .unwrap_or_else(|_| unsafe { alloc_or_handle(layout) })
}

#[inline(always)]
pub(crate) unsafe fn pool_opaque_dealloc(ptr: *mut u8) {
    let layout = opaque_layout();
    if POOL_OPAQUE
        .try_with(|c| {
            let (head, len) = c.0.get();
            if len < POOL_OPAQUE_CAP {
                unsafe { *(ptr as *mut *mut u8) = head };
                c.0.set((ptr, len + 1));
            } else {
                unsafe { dealloc(ptr, layout) };
            }
        })
        .is_err()
    {
        unsafe { dealloc(ptr, layout) };
    }
}

#[cfg(test)]
pub(crate) fn pool_b_len() -> usize {
    POOL_B.with(|pool| pool.0.get().1)
}

#[cfg(test)]
pub(crate) fn pool_opaque_len() -> usize {
    POOL_OPAQUE.with(|pool| pool.0.get().1)
}

#[cfg(test)]
mod tests {
    use super::{
        POOL_B_CAP, POOL_OPAQUE_CAP, drain_free_list, pool_b_alloc, pool_b_dealloc, pool_b_layout,
        pool_b_len, pool_opaque_alloc, pool_opaque_dealloc, pool_opaque_len,
    };
    use crate::object_model::opaque_layout;
    use std::cell::Cell;

    #[test]
    fn pool_layouts_satisfy_intrusive_next_pointer_and_nanbox_alignment() {
        let ptr_size = std::mem::size_of::<*mut u8>();
        let ptr_align = std::mem::align_of::<*mut u8>();
        for layout in [pool_b_layout(), opaque_layout()] {
            assert!(layout.size() >= ptr_size);
            assert!(layout.align() >= ptr_align);
            assert!(layout.align() >= 8);
        }
    }

    #[test]
    fn pool_b_recycles_lifo_and_enforces_capacity_bound() {
        std::thread::spawn(|| {
            assert_eq!(pool_b_len(), 0);
            let p1 = unsafe { pool_b_alloc() };
            let p2 = unsafe { pool_b_alloc() };
            assert_eq!((p1 as usize) % 8, 0);
            assert_eq!((p2 as usize) % 8, 0);

            unsafe { pool_b_dealloc(p1) };
            unsafe { pool_b_dealloc(p2) };
            assert_eq!(pool_b_len(), 2);

            // LIFO pop returns p2 then p1.
            let r1 = unsafe { pool_b_alloc() };
            let r2 = unsafe { pool_b_alloc() };
            assert_eq!(r1, p2);
            assert_eq!(r2, p1);
            assert_eq!(pool_b_len(), 0);
            unsafe {
                pool_b_dealloc(r1);
                pool_b_dealloc(r2);
            }

            // Capacity cap: returning more than POOL_B_CAP blocks frees overflow directly.
            let extra = POOL_B_CAP + 4;
            let ptrs: Vec<*mut u8> = (0..extra).map(|_| unsafe { pool_b_alloc() }).collect();
            assert_eq!(pool_b_len(), 0);
            for ptr in ptrs {
                unsafe { pool_b_dealloc(ptr) };
            }
            assert_eq!(pool_b_len(), POOL_B_CAP);
        })
        .join()
        .expect("pool_b invariant test thread");
    }

    #[test]
    fn pool_opaque_recycles_lifo_and_enforces_capacity_bound() {
        std::thread::spawn(|| {
            assert_eq!(pool_opaque_len(), 0);
            let p1 = unsafe { pool_opaque_alloc() };
            let p2 = unsafe { pool_opaque_alloc() };
            assert_eq!((p1 as usize) % 8, 0);
            assert_eq!((p2 as usize) % 8, 0);

            unsafe { pool_opaque_dealloc(p1) };
            unsafe { pool_opaque_dealloc(p2) };
            assert_eq!(pool_opaque_len(), 2);

            let r1 = unsafe { pool_opaque_alloc() };
            let r2 = unsafe { pool_opaque_alloc() };
            assert_eq!(r1, p2);
            assert_eq!(r2, p1);
            assert_eq!(pool_opaque_len(), 0);
            unsafe {
                pool_opaque_dealloc(r1);
                pool_opaque_dealloc(r2);
            }

            let extra = POOL_OPAQUE_CAP + 4;
            let ptrs: Vec<*mut u8> = (0..extra).map(|_| unsafe { pool_opaque_alloc() }).collect();
            assert_eq!(pool_opaque_len(), 0);
            for ptr in ptrs {
                unsafe { pool_opaque_dealloc(ptr) };
            }
            assert_eq!(pool_opaque_len(), POOL_OPAQUE_CAP);
        })
        .join()
        .expect("pool_opaque invariant test thread");
    }

    #[test]
    fn drain_free_list_handles_empty_state() {
        let empty = Cell::new((std::ptr::null_mut(), 0));
        assert_eq!(unsafe { drain_free_list(&empty, pool_b_layout()) }, 0);
        assert_eq!(empty.get(), (std::ptr::null_mut(), 0));
    }
}
