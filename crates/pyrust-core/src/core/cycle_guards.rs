use std::cell::RefCell;
use std::thread::LocalKey;

// ─────────────────────────────────────────────────────────────────────────────
// Generic cycle-detection guard + `repr` / `==` stacks
//
// CPython uses `Py_ReprEnter` / `Py_ReprLeave` (a thread-local set of currently
// being-formatted ids) to recognise self-referential collections.  Without the
// guard a structure like `a = []; a.append(a); repr(a)` recurses until the
// thread blows its stack.
//
// We mirror the same trick for `Value::repr` (issue #364), interpreter-aware
// `render_value_repr`, and `PartialEq` on collection variants (so `a == b` for
// two distinct cycles terminates and returns `True`, matching CPython's "we've
// already proven the prefix equal" semantics).
//
// The stacks stay empty on the non-cyclic hot path until we actually recurse
// *into* a collection variant, so a flat `repr([1] * 1000)` pays only a
// single thread-local lookup per recursive level and never inserts.
// ─────────────────────────────────────────────────────────────────────────────

// Thread-local justification:
// (a) Why `thread_local!`: Tracks the active recursive call stack of the
//     current OS thread during `repr`/`==` traversal; `Value::repr` and
//     `PartialEq::eq` do not receive an `&Interpreter` handle and are
//     re-entered arbitrarily deep through nested containers and user `__repr__`.
// (b) Multi-interpreter / multi-threaded impact: Separate threads maintain
//     independent traversal stacks with zero synchronization. Two interpreters
//     re-entrantly nested on the same thread share `REPR_IN_PROGRESS`, which is
//     sound because object ids are unique on a thread and RAII pops on unwind.
thread_local! {
    /// Stack of `Value::value_id()`s currently in the middle of being formatted
    /// by `Value::repr` or interpreter `render_value_repr`.  A second `repr`
    /// call for an id already on this stack short-circuits to the CPython
    /// placeholder (`[...]` / `{...}` / `(...)`) instead of recursing.
    ///
    /// Stored as a `Vec` rather than a `HashSet`: in practice the depth is
    /// shallow (a handful of nested collections) so the linear scan is
    /// faster than a HashSet's hashing on the hot path.  Wrapped in
    /// `RefCell` rather than `Cell` so we can borrow the inner `Vec`
    /// without moving it in and out for every push/pop.
    static REPR_IN_PROGRESS: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };

    /// Stack of ordered pairs `(value_id(a), value_id(b))` currently being
    /// compared by `Value::eq`.  Encountering the same pair again means we've
    /// hit a cycle; we treat the cycle as equal (the recursion bottoms out as
    /// "we've already proven the prefix equal") so the comparison terminates
    /// instead of blowing the stack.
    static EQ_IN_PROGRESS: RefCell<Vec<(i64, i64)>> = const { RefCell::new(Vec::new()) };
}

/// Generic RAII guard for a thread-local cycle-detection stack.
///
/// Pushes `key` on [`CycleGuard::enter`] when it is not already present and
/// pops on [`Drop`] so early returns (`?`) and panics cannot poison the stack.
pub struct CycleGuard<K: PartialEq + 'static> {
    stack: &'static LocalKey<RefCell<Vec<K>>>,
}

impl<K: PartialEq + 'static> CycleGuard<K> {
    /// Attempt to enter a cycle-guarded traversal for `key` on `stack`.
    ///
    /// Returns `Some(guard)` when the caller may proceed (recording `key`
    /// until `guard` drops), or `None` if `key` is already active on `stack`.
    pub fn enter(stack: &'static LocalKey<RefCell<Vec<K>>>, key: K) -> Option<Self> {
        stack.with(|cell| {
            let mut active = cell.borrow_mut();
            if active.contains(&key) {
                return None;
            }
            active.push(key);
            Some(Self { stack })
        })
    }
}

impl<K: PartialEq + 'static> Drop for CycleGuard<K> {
    fn drop(&mut self) {
        let _ = self.stack.try_with(|cell| {
            cell.borrow_mut().pop();
        });
    }
}

/// Enter the shared thread-local `repr` cycle-detection stack for `id`.
///
/// Used by both `Value::repr` in `pyrust-core` and interpreter-dispatched
/// container repr in `pyrust` so cross-boundary self-referential containers
/// share one RAII guard stack.
pub fn enter_repr_cycle_guard(id: i64) -> Option<CycleGuard<i64>> {
    CycleGuard::enter(&REPR_IN_PROGRESS, id)
}

/// RAII guard for the `repr` cycle-detection stack.
pub(crate) struct ReprGuard {
    _guard: CycleGuard<i64>,
}

impl ReprGuard {
    /// Attempts to enter the recursion for `id`.  Returns `Some(guard)` when
    /// the caller may proceed to format the children, or `None` if `id` is
    /// already on the stack (the caller should emit the placeholder).
    pub(crate) fn enter(id: i64) -> Option<Self> {
        enter_repr_cycle_guard(id).map(|_guard| Self { _guard })
    }
}

/// RAII guard for the `eq` cycle-detection stack.  Keyed on the ordered pair
/// of value ids currently being compared.
pub(crate) struct EqGuard {
    _guard: CycleGuard<(i64, i64)>,
}

impl EqGuard {
    /// Attempts to enter the recursion for `(a_id, b_id)`.  Returns
    /// `Some(guard)` when the caller may proceed with element-wise comparison,
    /// or `None` if the pair is already on the stack (the caller treats the
    /// cycle as equal).
    pub(crate) fn enter(a_id: i64, b_id: i64) -> Option<Self> {
        CycleGuard::enter(&EQ_IN_PROGRESS, (a_id, b_id)).map(|_guard| Self { _guard })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{CycleGuard, EqGuard, ReprGuard, enter_repr_cycle_guard};

    thread_local! {
        static TEST_CYCLE_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    }

    #[test]
    fn cycle_guard_supports_nested_entry_duplicate_short_circuit_and_raii_drop() {
        let g1 = CycleGuard::enter(&TEST_CYCLE_STACK, 10).expect("outer key enters");
        assert!(CycleGuard::enter(&TEST_CYCLE_STACK, 10).is_none());

        {
            let g2 = CycleGuard::enter(&TEST_CYCLE_STACK, 20).expect("nested distinct key enters");
            assert!(CycleGuard::enter(&TEST_CYCLE_STACK, 10).is_none());
            assert!(CycleGuard::enter(&TEST_CYCLE_STACK, 20).is_none());
            drop(g2);
        }

        // After inner drop, 20 can re-enter while 10 is still active.
        assert!(CycleGuard::enter(&TEST_CYCLE_STACK, 10).is_none());
        let g2_again = CycleGuard::enter(&TEST_CYCLE_STACK, 20).expect("re-enter after drop");
        drop(g2_again);
        drop(g1);

        // After all guards drop, stack is clean and 10 can re-enter.
        assert!(TEST_CYCLE_STACK.with(|cell| cell.borrow().is_empty()));
        assert!(CycleGuard::enter(&TEST_CYCLE_STACK, 10).is_some());
    }

    #[test]
    fn cycle_guard_pops_on_panic_unwind() {
        let outcome = std::panic::catch_unwind(|| {
            let _outer = CycleGuard::enter(&TEST_CYCLE_STACK, 101).expect("outer enters");
            let _inner = CycleGuard::enter(&TEST_CYCLE_STACK, 202).expect("inner enters");
            panic!("simulated failure inside cycle-guarded traversal");
        });
        assert!(outcome.is_err());
        assert!(TEST_CYCLE_STACK.with(|cell| cell.borrow().is_empty()));
        assert!(CycleGuard::enter(&TEST_CYCLE_STACK, 101).is_some());
        assert!(CycleGuard::enter(&TEST_CYCLE_STACK, 202).is_some());
    }

    #[test]
    fn repr_and_eq_guards_delegate_to_generic_cycle_guard() {
        let r1 = ReprGuard::enter(42).expect("repr enters");
        assert!(enter_repr_cycle_guard(42).is_none());
        assert!(ReprGuard::enter(42).is_none());
        drop(r1);
        assert!(enter_repr_cycle_guard(42).is_some());

        let e1 = EqGuard::enter(1, 2).expect("eq pair enters");
        assert!(EqGuard::enter(1, 2).is_none());
        // Ordered pairs: (2, 1) is distinct from (1, 2).
        let e2 = EqGuard::enter(2, 1).expect("reversed pair enters");
        drop(e2);
        drop(e1);
        assert!(EqGuard::enter(1, 2).is_some());
    }
}
