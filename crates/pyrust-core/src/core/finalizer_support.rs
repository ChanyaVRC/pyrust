// ─────────────────────────────────────────────────────────────────────────────
// `__del__` finalizer support — deferred finalizer queue (issue #3341).
//
// Design invariants:
// 1. Zero cost until a class defines `__del__`: the `Drop` hooks test the same
//    word as the weakref gate (`FINALIZERS_ARMED_BIT` in `LIVE_WEAKREF_COUNT`).
// 2. `Drop` never runs Python code. A dying instance whose class defines
//    `__del__` is kept alive and pushed on `PENDING_FINALIZERS`; the
//    interpreter runs the queue at its next safepoint, where no Rust borrow
//    is held.
// 3. PEP 442: `InstanceAttrs::is_finalized` is set when an instance is queued,
//    so `__del__` runs at most once even if it resurrects the instance.
// 4. `LIVE_FINALIZABLE` counts constructed instances that still await their
//    `__del__`, so the interpreter polls per instruction only while there is
//    something that could become due. It is a pacing hint, not an invariant:
//    an instance made outside the counted constructor path is still finalized,
//    just at a coarser safepoint.
// ─────────────────────────────────────────────────────────────────────────────

crate::gil_local! {
    static HAS_PENDING_FINALIZERS: Cell<bool> = const { Cell::new(false) };
    static LIVE_FINALIZABLE: Cell<usize> = const { Cell::new(0) };
    static PENDING_FINALIZERS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
}

/// Start offering dying instances to the finalizer queue. Called when a class
/// gains a `__del__` attribute; stays on until [`disarm_finalizers`].
pub fn arm_finalizers() {
    let _ = LIVE_WEAKREF_COUNT.try_with(|c| c.set(c.get() | FINALIZERS_ARMED_BIT));
}

/// Stop queueing finalizers (interpreter shutdown): instances that die from
/// here on are simply freed.
pub fn disarm_finalizers() {
    let _ = LIVE_WEAKREF_COUNT.try_with(|c| c.set(c.get() & !FINALIZERS_ARMED_BIT));
}

/// Whether any class has defined `__del__` on the current interpreter.
#[inline(always)]
pub fn finalizers_armed() -> bool {
    LIVE_WEAKREF_COUNT.try_with(Cell::get).unwrap_or(0) & FINALIZERS_ARMED_BIT != 0
}

/// Record that an instance of a class defining `__del__` was constructed.
pub fn note_finalizable_instance() {
    let _ = LIVE_FINALIZABLE.try_with(|c| c.set(c.get().saturating_add(1)));
}

/// Constructed instances of `__del__` classes not yet queued for it.
#[inline]
pub fn live_finalizable_instances() -> usize {
    LIVE_FINALIZABLE.try_with(Cell::get).unwrap_or(0)
}

/// Fast check: whether instances are waiting for their `__del__` call.
#[inline(always)]
pub fn has_pending_finalizers() -> bool {
    HAS_PENDING_FINALIZERS.try_with(Cell::get).unwrap_or(false)
}

/// Take every instance waiting for its `__del__` call, oldest first.
#[cold]
#[inline(never)]
pub fn take_pending_finalizers() -> Vec<Value> {
    HAS_PENDING_FINALIZERS.with(|flag| flag.set(false));
    PENDING_FINALIZERS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// Whether `class` or one of its bases defines `__del__`, memoized in
/// `PyClass::del_slot_cache`. A class that is mutably borrowed right now is
/// reported as having none: the instance is then freed without finalization
/// rather than risking a borrow panic inside `Drop`.
fn class_has_finalizer(class: &Rc<RefCell<PyClass>>) -> bool {
    fn defines_del(class: &Rc<RefCell<PyClass>>) -> bool {
        let Ok(borrowed) = class.try_borrow() else {
            return false;
        };
        borrowed.attrs.contains_key("__del__")
            || borrowed.base.as_ref().is_some_and(defines_del)
            || borrowed.extra_bases.iter().any(defines_del)
    }

    let Ok(borrowed) = class.try_borrow() else {
        return false;
    };
    let version = borrowed.mutation_version.get();
    if let Some((cached_version, cached_epoch, has_del)) = borrowed.del_slot_cache.get()
        && crate::class_cache_stamp_matches(version, cached_version, cached_epoch)
    {
        return has_del;
    }
    drop(borrowed);
    let has_del = defines_del(class);
    if let Ok(borrowed) = class.try_borrow()
        && let Some((version, epoch)) = crate::class_cache_stamp(version)
    {
        borrowed.del_slot_cache.set(Some((version, epoch, has_del)));
    }
    has_del
}

/// Queue `instance` for its `__del__` call. If the queue is unreachable
/// (thread teardown) the value is dropped, which frees the instance now that
/// it is marked finalized.
fn push_pending_finalizer(instance: Value) {
    let _ = LIVE_FINALIZABLE.try_with(|c| c.set(c.get().saturating_sub(1)));
    let _ = PENDING_FINALIZERS.try_with(|q| {
        if let Ok(mut q) = q.try_borrow_mut() {
            q.push(instance);
            let _ = HAS_PENDING_FINALIZERS.try_with(|flag| flag.set(true));
        }
    });
}

/// Drop hook for the last `Value` (or bound method) referencing an instance:
/// when its class defines `__del__` and it was not finalized yet, keep the
/// instance alive in the finalizer queue. Identity and weak references are
/// preserved, as in CPython, where `__del__` runs before the object is freed.
#[cold]
#[inline(never)]
fn queue_finalizer_for_last_ref(rc: &Rc<RefCell<PyInstance>>) {
    if !finalizers_armed() || Rc::strong_count(rc) != 1 {
        return;
    }
    {
        let Ok(mut instance) = rc.try_borrow_mut() else {
            return;
        };
        if instance.attrs.is_finalized() || !class_has_finalizer(&instance.class) {
            return;
        }
        instance.attrs.mark_finalized();
    }
    push_pending_finalizer(Value::py_instance(Rc::clone(rc)));
}

/// Drop hook for an instance whose storage is being destroyed. The last
/// reference was a bare `Rc` rather than a `Value`, so the allocation cannot
/// be kept: the instance's state moves to a fresh allocation that is queued
/// for `__del__` instead, and weak references to the old allocation die.
#[cold]
#[inline(never)]
fn instance_dropping(instance: &mut PyInstance) {
    if finalizers_armed() && !instance.attrs.is_finalized() && class_has_finalizer(&instance.class)
    {
        let mut attrs = std::mem::take(&mut instance.attrs);
        attrs.mark_finalized();
        let revived = PyInstance {
            class: Rc::clone(&instance.class),
            attrs,
        };
        push_pending_finalizer(Value::py_instance(Rc::new(RefCell::new(revived))));
    }
    notify_object_dropped(instance as *const PyInstance as usize);
}
