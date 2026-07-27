// ── PartialEq helpers ─────────────────────────────────────────────────────────

/// Exact equality between an `i64` integer and an `f64` float.
///
/// CPython's int-float comparison converts the float to its exact integer
/// representation and compares, rather than converting the int to float (which
/// loses precision beyond 2^53).  Concretely: `2**53 + 1 == float(2**53 + 1)`
/// is `False` in CPython because `float(2**53 + 1)` rounds to `2**53`.
///
/// The algorithm:
/// 1. If `f` is not finite, return `false`.
/// 2. If `f` has a fractional part, return `false` (no integer can equal it).
/// 3. If `f` is outside the `i64` value range, return `false`.
/// 4. Otherwise convert `f` to `i64` exactly (safe: `f` is finite,
///    integer-valued, and in range) and compare.
///
/// Step 4 is the key insight: `f as i64` gives the *float's* exact integer
/// value, not a lossy round-trip through `i as f64`.
fn int_float_eq(i: i64, f: f64) -> bool {
    if !f.is_finite() || f != f.trunc() {
        return false;
    }
    // 9223372036854775808.0 is 2^63, the smallest f64 strictly greater than
    // i64::MAX.  Any finite integer-valued float in [i64::MIN, 2^63) is
    // safely representable as i64.
    const I64_MAX_PLUS_ONE: f64 = 9_223_372_036_854_775_808.0_f64;
    if f < (i64::MIN as f64) || f >= I64_MAX_PLUS_ONE {
        return false;
    }
    (f as i64) == i
}

/// Exact equality between a `BigInt` and an `f64` float.
///
/// Mirrors `int_float_eq` for arbitrarily large integers.  Only finite,
/// integer-valued floats can equal a `BigInt`; anything else (NaN, infinity,
/// or a fractional value like 1.2) returns `false` immediately.
///
/// We must guard with `f.is_finite() && f == f.trunc()` before calling
/// `BigInt::from_f64`, because `from_f64` **truncates** fractional floats
/// (e.g. 1.2 → BigInt(1)) rather than returning `None`, which would make
/// `bigint_float_eq(&BigInt::from(1), 1.2)` incorrectly return `true`.
fn bigint_float_eq(big: &BigInt, f: f64) -> bool {
    if !f.is_finite() || f != f.trunc() {
        return false;
    }
    match BigInt::from_f64(f) {
        Some(f_as_bigint) => f_as_bigint == *big,
        None => false,
    }
}

// ── PartialEq ─────────────────────────────────────────────────────────────────

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
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
            (ValueKind::Int(a), ValueKind::Int(b)) => a == b,
            // Python: 1 == 1.0 is True, but 2**53+1 == float(2**53+1) is False.
            // Convert the float to its exact integer value (not the int to float)
            // to avoid precision loss for values beyond the 53-bit mantissa.
            (ValueKind::Int(a), ValueKind::Float(b)) => int_float_eq(a, b),
            (ValueKind::Float(a), ValueKind::Int(b)) => int_float_eq(b, a),
            (ValueKind::Float(a), ValueKind::Float(b)) => a == b,
            (ValueKind::BigInt(a), ValueKind::BigInt(b)) => a == b,
            (ValueKind::BigInt(a), ValueKind::Int(b)) => *a == BigInt::from(b),
            (ValueKind::Int(a), ValueKind::BigInt(b)) => BigInt::from(a) == *b,
            (ValueKind::BigInt(a), ValueKind::Float(b)) => bigint_float_eq(a, b),
            (ValueKind::Float(a), ValueKind::BigInt(b)) => bigint_float_eq(b, a),
            (ValueKind::Str(a), ValueKind::Str(b)) => a == b,
            (ValueKind::Bool(a), ValueKind::Bool(b)) => a == b,
            // Python: True == 1 is True
            (ValueKind::Bool(a), ValueKind::Int(b)) => (a as i64) == b,
            (ValueKind::Int(a), ValueKind::Bool(b)) => a == (b as i64),
            // Python: True == 1.0 is True
            (ValueKind::Bool(a), ValueKind::Float(b)) => (a as u8 as f64) == b,
            (ValueKind::Float(a), ValueKind::Bool(b)) => a == (b as u8 as f64),
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
            // Plain component equality — NO NaN bit-equality fallback here, to
            // match the `ValueKind::Float` arm above: bare `==` on two distinct
            // NaN-bearing complex values is `False` in CPython
            // (`complex(nan,0) == complex(nan,0)` is `False`).  The identity
            // short-circuit that makes `z in [z]` / `{z:1}[z]` work lives in
            // `is_identical_nan` and the `PyKey::Complex` arm, not in `==` (#2535).
            (ValueKind::Complex(ar, ai), ValueKind::Complex(br, bi)) => ar == br && ai == bi,
            (ValueKind::Int(n), ValueKind::Complex(br, bi)) => (n as f64) == br && bi == 0.0,
            (ValueKind::Complex(ar, ai), ValueKind::Int(n)) => ar == (n as f64) && ai == 0.0,
            (ValueKind::Float(f), ValueKind::Complex(br, bi)) => f == br && bi == 0.0,
            (ValueKind::Complex(ar, ai), ValueKind::Float(f)) => ar == f && ai == 0.0,
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
