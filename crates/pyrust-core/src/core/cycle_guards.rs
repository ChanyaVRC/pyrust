use std::cell::RefCell;

// ─────────────────────────────────────────────────────────────────────────────
// Cycle-detection guards for `repr` and `==`
//
// CPython uses `Py_ReprEnter` / `Py_ReprLeave` (a thread-local set of currently
// being-formatted ids) to recognise self-referential collections.  Without the
// guard a structure like `a = []; a.append(a); repr(a)` recurses until the
// thread blows its stack.
//
// We mirror the same trick for `Value::repr` (issue #364) and for `PartialEq`
// on collection variants (so `a == b` for two distinct cycles terminates and
// returns `True`, matching CPython's "we've already proven the prefix equal"
// semantics).
//
// The sets stay empty on the non-cyclic hot path until we actually recurse
// *into* a collection variant, so a flat `repr([1] * 1000)` pays only a
// single thread-local lookup per recursive level and never inserts.
// ─────────────────────────────────────────────────────────────────────────────

thread_local! {
    /// Stack of `Value::value_id()`s currently in the middle of being formatted
    /// by `Value::repr`.  A second `repr` call for an id already on this stack
    /// short-circuits to the CPython placeholder (`[...]` / `{...}` / `(...)`)
    /// instead of recursing.
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

/// RAII guard for the `repr` cycle-detection stack.  Pushes `id` on
/// construction (caller must have checked it wasn't already present) and pops
/// on drop so an early-return or panic in the recursive body can't poison the
/// stack.
pub(crate) struct ReprGuard;

impl ReprGuard {
    /// Attempts to enter the recursion for `id`.  Returns `Some(guard)` when
    /// the caller may proceed to format the children, or `None` if `id` is
    /// already on the stack (the caller should emit the placeholder).
    pub(crate) fn enter(id: i64) -> Option<Self> {
        REPR_IN_PROGRESS.with(|cell| {
            let mut stack = cell.borrow_mut();
            if stack.contains(&id) {
                return None;
            }
            stack.push(id);
            Some(ReprGuard)
        })
    }
}

impl Drop for ReprGuard {
    fn drop(&mut self) {
        REPR_IN_PROGRESS.with(|cell| {
            cell.borrow_mut().pop();
        });
    }
}

/// RAII guard for the `eq` cycle-detection stack.  Identical shape to
/// [`ReprGuard`] but keyed on the ordered pair of value ids being compared.
pub(crate) struct EqGuard;

impl EqGuard {
    /// Attempts to enter the recursion for `(a_id, b_id)`.  Returns
    /// `Some(guard)` when the caller may proceed with element-wise comparison,
    /// or `None` if the pair is already on the stack (the caller treats the
    /// cycle as equal).
    pub(crate) fn enter(a_id: i64, b_id: i64) -> Option<Self> {
        EQ_IN_PROGRESS.with(|cell| {
            let mut stack = cell.borrow_mut();
            let pair = (a_id, b_id);
            if stack.contains(&pair) {
                return None;
            }
            stack.push(pair);
            Some(EqGuard)
        })
    }
}

impl Drop for EqGuard {
    fn drop(&mut self) {
        EQ_IN_PROGRESS.with(|cell| {
            cell.borrow_mut().pop();
        });
    }
}
