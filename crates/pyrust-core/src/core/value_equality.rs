// ── PartialEq ─────────────────────────────────────────────────────────────────

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        // Fast path for the overwhelmingly common tagged Int/Int pair, then
        // route all five numeric kinds (Int, Bool, Float, BigInt, Complex)
        // through the canonical NumericOps equality slot (issue #458).
        if let (ValueKind::Int(a), ValueKind::Int(b)) = (self.kind(), other.kind()) {
            return a == b;
        }
        if let Some(equal) = crate::numeric_ops::numeric_eq(self, other) {
            return equal;
        }
        // Cycle detection (#364): for collection kinds, check whether we're
        // already comparing this exact pair further up the call stack.  If we
        // are we've hit a cycle; we treat cyclic equality as true (the
        // recursion bottoms out as "we've already proven the prefix equal").
        //
        // We only consult the guard when *both* sides are cycle-capable
        // collection kinds — primitives can't form cycles and shouldn't pay
        // the thread-local lookup.
        let _eq_guard = match (self.kind(), other.kind()) {
            (ValueKind::List(_), ValueKind::List(_))
            | (ValueKind::Dict(_), ValueKind::Dict(_))
            | (ValueKind::Set(_), ValueKind::Set(_))
            | (ValueKind::Tuple(_), ValueKind::Tuple(_)) => {
                match (self.value_id(), other.value_id()) {
                    (Some(a_id), Some(b_id)) => match EqGuard::enter(a_id, b_id) {
                        Some(g) => Some(g),
                        None => return true,
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        match (self.kind(), other.kind()) {
            (ValueKind::Str(a), ValueKind::Str(b)) => a == b,
            (ValueKind::None, ValueKind::None) => true,
            (ValueKind::Ellipsis, ValueKind::Ellipsis) => true,
            // `Ref<T>` doesn't impl `==` directly — deref to compare the
            // underlying containers.  `*a == *b` calls Vec/IndexMap/IndexSet's
            // `PartialEq`, which is what we want.
            (ValueKind::List(a), ValueKind::List(b)) => *a == *b,
            (ValueKind::Tuple(a), ValueKind::Tuple(b)) => a == b,
            (ValueKind::Dict(a), ValueKind::Dict(b)) => *a == *b,
            (ValueKind::Set(a), ValueKind::Set(b)) => *a == *b,
            (ValueKind::Bytes(a), ValueKind::Bytes(b)) => a.as_ref() == b.as_ref(),
            (
                ValueKind::Range {
                    start: as_,
                    stop: ao,
                    step: at,
                },
                ValueKind::Range {
                    start: bs,
                    stop: bo,
                    step: bt,
                },
            ) => {
                // CPython `range_equals` (Objects/rangeobject.c): two ranges are
                // equal iff they yield the same sequence — same length, and (when
                // non-empty) same first element, and (when length ≥ 2) same step.
                // Matches the content-based range hash so equal ranges hash equal.
                let la = range_len(as_, ao, at);
                let lb = range_len(bs, bo, bt);
                la == lb && (la < 1 || as_ == bs) && (la < 2 || at == bt)
            }
            (
                ValueKind::BigRange {
                    start: as_,
                    stop: ao,
                    step: at,
                },
                ValueKind::BigRange {
                    start: bs,
                    stop: bo,
                    step: bt,
                },
            ) => bigrange_eq(as_, ao, at, bs, bo, bt),
            // Cross-width range comparison: a `BigRange` (at least one bound outside
            // i64) can still yield the same sequence as an i64 `Range` — e.g. both
            // empty (`range(10**20, 10**20) == range(0)`) or both length-1
            // (`range(0, 10**20, 10**20) == range(0, 1)`).  Compare via the shared
            // BigInt content rule.
            (
                ValueKind::Range {
                    start: as_,
                    stop: ao,
                    step: at,
                },
                ValueKind::BigRange {
                    start: bs,
                    stop: bo,
                    step: bt,
                },
            ) => bigrange_eq(
                &BigInt::from(as_),
                &BigInt::from(ao),
                &BigInt::from(at),
                bs,
                bo,
                bt,
            ),
            (
                ValueKind::BigRange {
                    start: as_,
                    stop: ao,
                    step: at,
                },
                ValueKind::Range {
                    start: bs,
                    stop: bo,
                    step: bt,
                },
            ) => bigrange_eq(
                as_,
                ao,
                at,
                &BigInt::from(bs),
                &BigInt::from(bo),
                &BigInt::from(bt),
            ),
            (ValueKind::BuiltinFunction(_), ValueKind::BuiltinFunction(_)) => {
                match (self.as_function_rc(), other.as_function_rc()) {
                    (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                    _ => false,
                }
            }
            (ValueKind::UserFunction(a), ValueKind::UserFunction(b)) => Rc::ptr_eq(a, b),
            (ValueKind::PyClass(a), ValueKind::PyClass(b)) => Rc::ptr_eq(a, b),
            (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) => Rc::ptr_eq(a, b),
            (ValueKind::PyModule(a), ValueKind::PyModule(b)) => Rc::ptr_eq(a, b),
            (
                ValueKind::BoundMethod {
                    function: af,
                    receiver: ar,
                },
                ValueKind::BoundMethod {
                    function: bf,
                    receiver: br,
                },
            ) => Rc::ptr_eq(af, bf) && Rc::ptr_eq(ar, br),
            (
                ValueKind::ClassBoundMethod {
                    function: af,
                    class: ac,
                },
                ValueKind::ClassBoundMethod {
                    function: bf,
                    class: bc,
                },
            ) => Rc::ptr_eq(af, bf) && Rc::ptr_eq(ac, bc),
            // Built-in objects dispatch equality through their ops trait so
            // pyrust-core never names a concrete built-in type.  Try both
            // directions so e.g. `frozenset == set` and `set == frozenset`
            // both reach the frozenset impl.
            (ValueKind::BuiltinObject { ops, state }, _) => ops.eq(state, other),
            (_, ValueKind::BuiltinObject { ops, state }) => ops.eq(state, self),
            _ => false,
        }
    }
}

// ── Display / Debug ───────────────────────────────────────────────────────────

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_py_str())
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.repr_raw())
    }
}
