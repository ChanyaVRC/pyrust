// ─────────────────────────────────────────────────────────────────────────────
// Weakref runtime support — non-owning target handles, per-thread drop-hook
// registry, and pending callback queue (issue #3190).
//
// Design invariants:
// 1. Zero memory overhead on `Value`, `OpaqueSlot`, `PyInstance`, `PyClass`,
//    `UserFunction`, `SetInner`, `PyModule`, and `GeneratorCell`.
// 2. Zero hot-path cost when no weakrefs exist: every `Drop` hook tests a
//    single thread-local `Cell<usize>` (`LIVE_WEAKREF_COUNT == 0`) before
//    entering the `#[cold] #[inline(never)]` registry lookup.
// 3. Re-entrancy safe: `LIVE_WEAKREF_REGISTRY` borrows are always released
//    before dropping or enqueuing any Python `Value`.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::VecDeque;

// Thread-local justification:
// (a) Why `thread_local!`: `Value`, `PyInstance`, `PyClass`, etc. are `!Send`
//     (`Rc`-backed) and confined to a single interpreter thread. Keeping the
//     weakref registry thread-local avoids atomic contention on every object
//     drop while matching pyrust's single-thread `Rc` ownership model.
// (b) Multi-interpreter / multi-threaded impact: Independent threads own
//     independent registries; object addresses within a thread are unique for
//     live allocations and removed synchronously during `Drop` before memory
//     can be reused (preventing ABA collisions).
thread_local! {
    static LIVE_WEAKREF_COUNT: Cell<usize> = const { Cell::new(0) };
    static LIVE_WEAKREF_CALLBACK_COUNT: Cell<usize> = const { Cell::new(0) };
    static HAS_PENDING_WEAKREF_CALLBACKS: Cell<bool> = const { Cell::new(false) };
    static NEXT_WEAKREF_HANDLE_ID: Cell<u64> = const { Cell::new(1) };
    static LIVE_WEAKREF_REGISTRY: RefCell<HashMap<usize, Vec<WeakrefRegistryEntry>>> =
        RefCell::new(HashMap::new());
    static PENDING_WEAKREF_CALLBACKS: RefCell<VecDeque<(Value, Value)>> =
        const { RefCell::new(VecDeque::new()) };
}

struct WeakrefRegistryEntry {
    handle_id: u64,
    /// `0` = never deduplicate (callback or subclass), `1` = plain `ref`, `2` = plain `proxy`.
    dedup_kind: u8,
    wrapper: Weak<RefCell<PyInstance>>,
    alive: Rc<Cell<bool>>,
    callback: Rc<RefCell<Option<Value>>>,
}

/// Non-owning weak handle to a weak-referenceable Python object.
///
/// Tracks the underlying `Rc` (or `OpaqueSlot` for bound methods) so that
/// liveness reflects the object's true Python reference count regardless of
/// how many NaN-boxed `OpaqueSlot` wrappers point to the same `Rc`.
#[derive(Clone)]
pub struct WeakTarget(WeakTargetKind);

#[derive(Clone)]
enum WeakTargetKind {
    Instance(Weak<RefCell<PyInstance>>),
    Class(Weak<RefCell<PyClass>>),
    Function(Weak<UserFunction>),
    Set(Weak<SetInner>),
    Module(Weak<RefCell<PyModule>>),
    Generator(Weak<GeneratorCell>),
    BuiltinObject {
        ops: &'static dyn BuiltinTypeOps,
        state: Weak<RefCell<Box<dyn Any>>>,
    },
    BoundMethod(WeakOpaqueSlot),
}

impl WeakTarget {
    /// Downgrade `value` into a `(WeakTarget, target_addr)` pair if its
    /// representation supports weak references.
    ///
    /// Callers are responsible for Python-level weakrefability policy checks
    /// (such as `__slots__` without `'__weakref__'` on `PyInstance`, or
    /// restricting `BuiltinObject` to weak-referenceable types like
    /// `frozenset`).
    pub fn downgrade_value(value: &Value) -> Option<(Self, usize)> {
        if top16(value.0) != TAG_OPAQUE {
            return None;
        }
        let opaque = unsafe { &*value.opaque_ptr() };
        match opaque {
            Opaque::PyInstance(rc) => {
                let addr = &*rc.borrow() as *const PyInstance as usize;
                Some((Self(WeakTargetKind::Instance(Rc::downgrade(rc))), addr))
            }
            Opaque::PyClass(rc) => {
                let addr = &rc.borrow().subclasses as *const _ as usize;
                Some((Self(WeakTargetKind::Class(Rc::downgrade(rc))), addr))
            }
            Opaque::UserFunction(rc) => {
                let addr = Rc::as_ptr(rc) as usize;
                Some((Self(WeakTargetKind::Function(Rc::downgrade(rc))), addr))
            }
            Opaque::Set(rc) => {
                let addr = Rc::as_ptr(rc) as usize;
                Some((Self(WeakTargetKind::Set(Rc::downgrade(rc))), addr))
            }
            Opaque::PyModule(rc) => {
                let addr = &*rc.borrow() as *const PyModule as usize;
                Some((Self(WeakTargetKind::Module(Rc::downgrade(rc))), addr))
            }
            Opaque::Generator(rc) if rc.kind() != GeneratorKind::Iterator => {
                let addr = Rc::as_ptr(rc) as usize;
                Some((Self(WeakTargetKind::Generator(Rc::downgrade(rc))), addr))
            }
            Opaque::BuiltinObject { ops, state } => {
                let addr = Rc::as_ptr(state) as usize;
                Some((
                    Self(WeakTargetKind::BuiltinObject {
                        ops: *ops,
                        state: Rc::downgrade(state),
                    }),
                    addr,
                ))
            }
            Opaque::BoundMethod { .. } | Opaque::ClassBoundMethod { .. } => {
                let slot = WeakOpaqueSlot::downgrade(value)?;
                let addr = slot.slot_ptr() as usize;
                Some((Self(WeakTargetKind::BoundMethod(slot)), addr))
            }
            _ => None,
        }
    }

    /// Upgrade the weak handle back to a strong `Value`, or `None` if dead.
    #[inline]
    pub fn upgrade(&self) -> Option<Value> {
        match &self.0 {
            WeakTargetKind::Instance(w) => w.upgrade().map(Value::py_instance),
            WeakTargetKind::Class(w) => w.upgrade().map(Value::py_class),
            WeakTargetKind::Function(w) => w.upgrade().map(Value::user_function),
            WeakTargetKind::Set(w) => w.upgrade().map(Value::set_shared),
            WeakTargetKind::Module(w) => w.upgrade().map(Value::py_module),
            WeakTargetKind::Generator(w) => w.upgrade().map(Value::generator_shared),
            WeakTargetKind::BuiltinObject { ops, state } => state
                .upgrade()
                .map(|rc| Value::builtin_object_shared(*ops, rc)),
            WeakTargetKind::BoundMethod(slot) => slot.upgrade(),
        }
    }
}

/// Return the canonical weakref registry address key for `value`, if it has
/// a weak-referenceable representation.
#[inline]
pub fn weakref_target_addr(value: &Value) -> Option<usize> {
    if top16(value.0) != TAG_OPAQUE {
        return None;
    }
    let opaque = unsafe { &*value.opaque_ptr() };
    match opaque {
        Opaque::PyInstance(rc) => Some(&*rc.borrow() as *const PyInstance as usize),
        Opaque::PyClass(rc) => Some(&rc.borrow().subclasses as *const _ as usize),
        Opaque::UserFunction(rc) => Some(Rc::as_ptr(rc) as usize),
        Opaque::Set(rc) => Some(Rc::as_ptr(rc) as usize),
        Opaque::PyModule(rc) => Some(&*rc.borrow() as *const PyModule as usize),
        Opaque::Generator(rc) if rc.kind() != GeneratorKind::Iterator => {
            Some(Rc::as_ptr(rc) as usize)
        }
        Opaque::BuiltinObject { state, .. } => Some(Rc::as_ptr(state) as usize),
        Opaque::BoundMethod { .. } | Opaque::ClassBoundMethod { .. } => {
            Some(unsafe { value.opaque_slot_ptr() } as usize)
        }
        _ => None,
    }
}

/// Fast check: whether any live weakrefs exist on the current thread.
#[inline(always)]
pub fn has_live_weakrefs() -> bool {
    LIVE_WEAKREF_COUNT.try_with(Cell::get).unwrap_or(0) != 0
}

/// Fast check: whether any live weakrefs with callbacks exist on the current
/// thread.
#[inline(always)]
pub fn has_live_weakref_callbacks() -> bool {
    LIVE_WEAKREF_CALLBACK_COUNT.try_with(Cell::get).unwrap_or(0) != 0
}

/// Fast check: whether any finalized weakref callbacks are waiting to run.
#[inline(always)]
pub fn has_pending_weakref_callbacks() -> bool {
    HAS_PENDING_WEAKREF_CALLBACKS
        .try_with(Cell::get)
        .unwrap_or(false)
}

/// Whether `value` currently has at least one registered live weakref.
#[inline]
pub fn is_value_weakly_referenced(value: &Value) -> bool {
    if !has_live_weakrefs() {
        return false;
    }
    let Some(addr) = weakref_target_addr(value) else {
        return false;
    };
    LIVE_WEAKREF_REGISTRY.with(|reg| reg.borrow().contains_key(&addr))
}

/// Register a new weakref wrapper (`weakref.ref` or `weakref.proxy` instance)
/// for `target_addr`. Returns the unique `handle_id` for later unregistration.
pub fn register_weakref(
    target_addr: usize,
    dedup_kind: u8,
    wrapper: Weak<RefCell<PyInstance>>,
    alive: Rc<Cell<bool>>,
    callback: Rc<RefCell<Option<Value>>>,
) -> u64 {
    let has_cb = callback.borrow().is_some();
    let handle_id = NEXT_WEAKREF_HANDLE_ID.with(|cell| {
        let id = cell.get();
        cell.set(id.wrapping_add(1).max(1));
        id
    });
    LIVE_WEAKREF_REGISTRY.with(|reg| {
        reg.borrow_mut()
            .entry(target_addr)
            .or_default()
            .push(WeakrefRegistryEntry {
                handle_id,
                dedup_kind,
                wrapper,
                alive,
                callback,
            });
    });
    LIVE_WEAKREF_COUNT.with(|c| c.set(c.get() + 1));
    if has_cb {
        LIVE_WEAKREF_CALLBACK_COUNT.with(|c| c.set(c.get() + 1));
    }
    handle_id
}

/// Unregister a weakref whose wrapper (`ref`/`proxy`) is being dropped before
/// its referent died.
#[cold]
#[inline(never)]
pub fn unregister_weakref(target_addr: usize, handle_id: u64) {
    let removed = LIVE_WEAKREF_REGISTRY
        .try_with(|reg| {
            let mut map = reg.borrow_mut();
            let entry = map.get_mut(&target_addr)?;
            let pos = entry.iter().position(|e| e.handle_id == handle_id)?;
            let removed = entry.remove(pos);
            if entry.is_empty() {
                map.remove(&target_addr);
            }
            Some(removed)
        })
        .ok()
        .flatten();
    if let Some(entry) = removed {
        let _ = LIVE_WEAKREF_COUNT.try_with(|c| c.set(c.get().saturating_sub(1)));
        entry.alive.set(false);
        let cb = entry.callback.borrow_mut().take();
        if cb.is_some() {
            let _ = LIVE_WEAKREF_CALLBACK_COUNT.try_with(|c| c.set(c.get().saturating_sub(1)));
        }
        // Drop `cb` outside any registry borrow.
        drop(cb);
    }
}

/// Look up an existing plain (exact `weakref.ref` with `dedup_kind == 1`, or
/// exact `weakref.proxy` with `dedup_kind == 2`, no callback) weakref for
/// `target_addr` to match CPython's deduplication semantics.
pub fn find_existing_plain_ref(target_addr: usize, dedup_kind: u8) -> Option<Value> {
    if dedup_kind == 0 || !has_live_weakrefs() {
        return None;
    }
    LIVE_WEAKREF_REGISTRY.with(|reg| {
        let map = reg.borrow();
        let entries = map.get(&target_addr)?;
        for entry in entries {
            if entry.dedup_kind == dedup_kind
                && entry.alive.get()
                && let Some(rc) = entry.wrapper.upgrade()
            {
                return Some(Value::py_instance(rc));
            }
        }
        None
    })
}

/// Count live weakrefs currently pointing to `target_addr`.
pub fn get_weakref_count(target_addr: usize) -> usize {
    if !has_live_weakrefs() {
        return 0;
    }
    LIVE_WEAKREF_REGISTRY.with(|reg| {
        let map = reg.borrow();
        let Some(entries) = map.get(&target_addr) else {
            return 0;
        };
        entries
            .iter()
            .filter(|e| e.alive.get() && e.wrapper.strong_count() > 0)
            .count()
    })
}

/// Return all live weakref (`ref` / `proxy`) objects currently pointing to
/// `target_addr`.
pub fn get_weakrefs(target_addr: usize) -> Vec<Value> {
    if !has_live_weakrefs() {
        return Vec::new();
    }
    LIVE_WEAKREF_REGISTRY.with(|reg| {
        let map = reg.borrow();
        let Some(entries) = map.get(&target_addr) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.alive.get()
                && let Some(rc) = entry.wrapper.upgrade()
            {
                out.push(Value::py_instance(rc));
            }
        }
        out
    })
}

/// Drain all pending `(weakref_wrapper, callback)` pairs queued by recent
/// referent drops.
#[cold]
#[inline(never)]
pub fn take_pending_weakref_callbacks() -> Vec<(Value, Value)> {
    HAS_PENDING_WEAKREF_CALLBACKS.with(|flag| flag.set(false));
    PENDING_WEAKREF_CALLBACKS.with(|q| q.borrow_mut().drain(..).collect())
}

/// Drop hook invoked when a weak-referenceable object's final strong reference
/// is being destroyed. Removes all weakref entries for `addr`, marks them dead,
/// and enqueues their callbacks in LIFO order (matching CPython 3.12).
#[cold]
#[inline(never)]
pub(crate) fn notify_object_dropped(addr: usize) {
    let entries = LIVE_WEAKREF_REGISTRY
        .try_with(|reg| reg.borrow_mut().remove(&addr))
        .ok()
        .flatten();
    let Some(entries) = entries else {
        return;
    };
    let count = entries.len();
    let _ = LIVE_WEAKREF_COUNT.try_with(|c| c.set(c.get().saturating_sub(count)));

    let mut dropped_cbs: Vec<Value> = Vec::new();
    let mut queued: Vec<(Value, Value)> = Vec::new();
    // CPython invokes weakref callbacks in LIFO (reverse registration) order.
    for entry in entries.into_iter().rev() {
        entry.alive.set(false);
        let cb = entry.callback.borrow_mut().take();
        if let Some(cb) = cb {
            let _ = LIVE_WEAKREF_CALLBACK_COUNT.try_with(|c| c.set(c.get().saturating_sub(1)));
            if let Some(wrapper_rc) = entry.wrapper.upgrade() {
                queued.push((Value::py_instance(wrapper_rc), cb));
            } else {
                dropped_cbs.push(cb);
            }
        }
    }
    if !queued.is_empty() {
        let _ = PENDING_WEAKREF_CALLBACKS.try_with(|q| q.borrow_mut().extend(queued));
        let _ = HAS_PENDING_WEAKREF_CALLBACKS.try_with(|flag| flag.set(true));
    }
    // Drop unused callbacks outside every registry borrow.
    drop(dropped_cbs);
}

/// Drop hook for `OpaqueSlot` variants whose lifetime is governed by the slot
/// or `BuiltinState` rather than a dedicated struct `Drop` impl (`BoundMethod`,
/// `ClassBoundMethod`, and weak-referenceable `BuiltinObject`s like
/// `frozenset`).
#[cold]
#[inline(never)]
unsafe fn notify_opaque_slot_dropping(ptr: *mut OpaqueSlot) {
    let opaque = unsafe { &*std::ptr::addr_of!((*ptr).value).cast::<Opaque>() };
    match opaque {
        Opaque::BoundMethod { .. } | Opaque::ClassBoundMethod { .. } => {
            notify_object_dropped(ptr as usize);
        }
        Opaque::BuiltinObject { state, .. } if Rc::strong_count(state) == 1 => {
            notify_object_dropped(Rc::as_ptr(state) as usize);
        }
        _ => {}
    }
}

/// Transparent zero-overhead field wrapper that fires the weakref drop hook
/// without requiring its enclosing struct (e.g. `PyClass`) to implement `Drop`
/// directly (preserving `..Default::default()` struct-update construction).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct WeakrefNotifyingField<T>(pub T);

impl<T> std::ops::Deref for WeakrefNotifyingField<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for WeakrefNotifyingField<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> Drop for WeakrefNotifyingField<T> {
    #[inline]
    fn drop(&mut self) {
        if has_live_weakrefs() {
            notify_object_dropped(self as *const Self as usize);
        }
    }
}

impl Drop for PyInstance {
    #[inline]
    fn drop(&mut self) {
        if has_live_weakrefs() {
            notify_object_dropped(self as *const Self as usize);
        }
    }
}

impl Drop for UserFunction {
    #[inline]
    fn drop(&mut self) {
        if has_live_weakrefs() {
            notify_object_dropped(self as *const Self as usize);
        }
    }
}

impl Drop for SetInner {
    #[inline]
    fn drop(&mut self) {
        if has_live_weakrefs() {
            notify_object_dropped(self as *const Self as usize);
        }
    }
}

impl Drop for PyModule {
    #[inline]
    fn drop(&mut self) {
        if has_live_weakrefs() {
            notify_object_dropped(self as *const Self as usize);
        }
    }
}

impl Drop for GeneratorCell {
    #[inline]
    fn drop(&mut self) {
        if has_live_weakrefs() {
            notify_object_dropped(self as *const Self as usize);
        }
    }
}
