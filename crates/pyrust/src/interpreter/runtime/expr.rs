/// Fallible `BigInt -> f64`.  Raises `OverflowError` (CPython parity:
/// `int too large to convert to float`) when the BigInt's magnitude is
/// outside f64's representable range, instead of silently returning
/// `f64::INFINITY` (which loses sign and produces nonsense `inf`
/// arithmetic).  Centralised here for the mixed BigInt±Float arms in
/// add/sub/mul (PR #484 Copilot review).
fn bigint_to_float_or_overflow(b: &PyBigInt) -> Result<f64> {
    b.to_f64()
        .filter(|f| f.is_finite())
        .ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "int too large to convert to float".to_string(),
            )
        })
}

/// Outcome of the borrow-only sequence equality fast path used by
/// `values_user_eq` for `List`/`Tuple` pairs.
///
/// `Resolved(v)` means every element comparison was settled by
/// `Value::eq` without needing user `__eq__` dispatch, so `v` is the
/// final answer.  `NeedsDispatch` means at least one element pair could
/// only be resolved by recursing into `values_user_eq` (e.g. it
/// contains a `PyInstance` or a nested container that itself may hold
/// one); the caller must drop the borrow, snapshot the elements, and
/// take the slow recursion path.
enum SeqFast {
    Resolved(bool),
    NeedsDispatch,
}

/// Returns `true` iff comparing `a` against `b` via `Value::eq` could
/// give a different answer than user `__eq__` dispatch would.  Used to
/// keep `[1,2,3] == [1,2,4]` (flat primitive sequences) on a
/// zero-allocation walk.
///
/// Conservative: any `PyInstance`, container (`List`/`Tuple`/`Dict`/
/// `Set`), or `BuiltinObject` (e.g. `frozenset`) returns `true`,
/// because each may itself contain a `PyInstance` for which `Value::eq`
/// would fall back to `Rc::ptr_eq`.  Leaf primitives
/// (`Int`/`Float`/`Bool`/`Str`/`Bytes`/`None`/`Complex`/`BigInt`/
/// `Range`) return `false`.
fn pair_may_need_dispatch(a: &Value, b: &Value) -> bool {
    matches!(
        a.kind(),
        ValueKind::PyInstance(_)
            | ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::BuiltinObject { .. }
    ) || matches!(
        b.kind(),
        ValueKind::PyInstance(_)
            | ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::BuiltinObject { .. }
    )
}

/// Element-wise equality over two equal-length slices, returning
/// [`SeqFast::Resolved`] as soon as every pair can be decided by
/// `Value::eq` alone.  As soon as a pair that could need user dispatch
/// is encountered (and didn't already compare equal), bail to
/// [`SeqFast::NeedsDispatch`] so the caller can take the snapshot+
/// recurse path.
fn try_seq_fast_eq(av: &[Value], bv: &[Value]) -> SeqFast {
    debug_assert_eq!(av.len(), bv.len());
    for (x, y) in av.iter().zip(bv.iter()) {
        if x == y {
            continue;
        }
        if pair_may_need_dispatch(x, y) {
            return SeqFast::NeedsDispatch;
        }
        return SeqFast::Resolved(false);
    }
    SeqFast::Resolved(true)
}

/// Python-style `(quotient, remainder)` for `a // b` and `a % b` where
/// both operands are BigInts.  Unlike Rust's `/` / `%` (truncate-toward-
/// zero), CPython uses floor division: the quotient is rounded toward
/// negative infinity and the remainder has the same sign as the
/// divisor.  Caller must guarantee `b != 0`.
///
/// Shared with `builtin_modules/bodies/builtins.rs` (the `divmod()`
/// builtin) to avoid divergence in sign-adjustment logic (issue #493).
pub(crate) fn bigint_divmod_floor(a: &PyBigInt, b: &PyBigInt) -> (PyBigInt, PyBigInt) {
    let mut q = a / b;
    let mut r = a % b;
    // Adjust if the truncated remainder's sign disagrees with the
    // divisor: subtract one from the quotient and add `b` back into the
    // remainder so it matches the divisor's sign (CPython semantics).
    if !r.is_zero() && (r.sign() != b.sign()) {
        q -= 1;
        r += b;
    }
    (q, r)
}

/// Coerce `Int` / `BigInt` / `Bool` to `PyBigInt` for cross-type
/// arithmetic.  Returns `None` for anything else so callers can fall
/// through to the float / TypeError path.
///
/// Shared with `builtin_modules/bodies/builtins.rs` (the `divmod()`
/// builtin) to avoid divergence in coercion logic (issue #493).
pub(crate) fn value_to_bigint(v: &Value) -> Option<PyBigInt> {
    match v.kind() {
        ValueKind::Int(n) => Some(PyBigInt::from(n)),
        ValueKind::BigInt(b) => Some(b.clone()),
        ValueKind::Bool(b) => Some(PyBigInt::from(b as i64)),
        _ => None,
    }
}

/// Result of validating a shift count: either a concrete `usize`
/// (small enough to apply directly), or a marker that the count is
/// non-negative but exceeds `usize::MAX`.  Each shift arm decides how
/// to handle the saturating case — `<<` raises `OverflowError` only
/// when the LHS is non-zero (CPython would actually allocate the
/// bits), while `>>` collapses to `0` / `-1` (CPython parity).
enum ShiftCount {
    Fits(usize),
    Saturated,
}

/// Maximum left-shift count we are willing to materialise at runtime.
/// CPython raises `OverflowError` ("too many digits in integer") for
/// results that would exceed `sys.maxsize` digits; we are more
/// conservative and cap at 2^30 ≈ 10^9 bits (~128 MiB worst-case),
/// which is large enough for any realistic computation.
const MAX_SHIFT: usize = 1 << 30;

/// Validate a shift count and convert it to `ShiftCount`.  Returns
/// `Err(ValueError)` for negative shifts and `Err(TypeError)` if the
/// operand isn't an int / bool.  Matches CPython's error messages
/// where possible.
fn shift_count(v: &Value) -> Result<ShiftCount> {
    let big = value_to_bigint(v).ok_or_else(|| {
        PyError::named("TypeError", "bitwise op requires integer".to_string())
    })?;
    match big.sign() {
        PyBigIntSign::Minus => Err(PyError::named(
            "ValueError",
            "negative shift count".to_string(),
        )),
        PyBigIntSign::NoSign => Ok(ShiftCount::Fits(0)),
        PyBigIntSign::Plus => Ok(match big.to_usize() {
            Some(n) => ShiftCount::Fits(n),
            None => ShiftCount::Saturated,
        }),
    }
}

impl Interpreter {
    fn unsupported_binary_operand(op: &str) -> PyError {
        PyError::named("TypeError", format!("unsupported operand type(s) for {op}"))
    }
    fn eval_index(&mut self, target: Value, index: Value) -> Result<Value> {
        // Handle Dict separately so the temporary `&IndexMap` from
        // `target.kind()` doesn't outlive the call into `dict_lookup`
        // (which may run user `__eq__` that mutates the dict — see the
        // aliasing notes on `Value::as_dict_mut`).
        if target.as_dict().is_some() {
            // Fast path for string keys (issue #506): probe via `StrKey` to
            // skip the `PyKey::Str(String)` heap allocation.
            let lookup = if let Some(s) = index.as_str() {
                self.dict_str_lookup(&target, s)?
            } else {
                let key = self.value_to_pykey(&index)?;
                self.dict_lookup(&target, &key)?
            };
            return match lookup {
                Some((_, v)) => Ok(v),
                None => Err(PyError::key_error(index)),
            };
        }
        match target.kind() {
            ValueKind::List(items) => {
                let idx = normalize_index(&index, items.len(), "list")?;
                Ok(items[idx].clone())
            }
            ValueKind::Tuple(items) => {
                let idx = normalize_index(&index, items.len(), "tuple")?;
                Ok(items[idx].clone())
            }
            ValueKind::Str(text) => {
                let chars: Vec<char> = text.chars().collect();
                let idx = normalize_index(&index, chars.len(), "string")?;
                Ok(Value::string(chars[idx].to_string()))
            }
            ValueKind::Bytes(rc) => {
                let idx = normalize_index(&index, rc.len(), "bytes")?;
                Ok(Value::int(rc[idx] as i64))
            }
            ValueKind::Dict(_) => unreachable!("handled above"),
            ValueKind::BuiltinObject { ops, state } => {
                // Built-in object types opt in to subscripting via
                // `BuiltinTypeOps::get_item`.  The default impl returns a
                // TypeError shaped like the legacy "object is not
                // subscriptable" message, so non-subscriptable types
                // don't need per-type plumbing.
                ops.get_item(state, &index)
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__getitem__") {
                    return invoke_class_method(
                        self,
                        method_val,
                        Value::py_instance(inst_rc),
                        &[ExpandedCallArg {
                            name: None,
                            value: index,
                        }],
                    );
                }
                Err(PyError::named(
                    "TypeError",
                    format!("'{}' object is not subscriptable", class.borrow().name),
                ))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object is not subscriptable",
                    pyrust_core::builtin_type_name(&target)
                ),
            )),
        }
    }

    /// Try to call a binary dunder method on `left` (named `method`), then on
    /// `right` (named `rmethod`).  Returns `Some(result)` if a dunder was found
    /// and called, or `None` if neither operand has the method.
    ///
    /// Routes both `UserFunction` (pure-Python class methods) and
    /// `BuiltinFunction` (methods defined via `pyrust_module!`'s
    /// `class { … }` block, e.g. `Counter.__add__`) through
    /// `invoke_class_method` so operator-overloading works for both
    /// kinds of class — issue #331.
    fn try_dunder_binary(
        &mut self,
        left: &Value,
        right: &Value,
        method: &str,
        rmethod: &str,
    ) -> Option<Result<Value>> {
        if let ValueKind::PyInstance(inst) = left.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, method)
                && is_callable_method(&m)
            {
                let self_val = Value::py_instance(Rc::clone(inst));
                let arg = ExpandedCallArg {
                    name: None,
                    value: right.clone(),
                };
                match invoke_class_method(self, m, self_val, &[arg]) {
                    Ok(v) if is_not_implemented(&v) => {}
                    result => return Some(result),
                }
            }
        }
        if let ValueKind::PyInstance(inst) = right.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, rmethod)
                && is_callable_method(&m)
            {
                let self_val = Value::py_instance(Rc::clone(inst));
                let arg = ExpandedCallArg {
                    name: None,
                    value: left.clone(),
                };
                match invoke_class_method(self, m, self_val, &[arg]) {
                    Ok(v) if is_not_implemented(&v) => {}
                    result => return Some(result),
                }
            }
        }
        None
    }

    /// Try to call a unary dunder method on a PyInstance.  Routes both
    /// `UserFunction` and `BuiltinFunction` class methods through
    /// `invoke_class_method` — same parity with `try_dunder_binary`.
    pub(crate) fn try_dunder_unary(&mut self, val: &Value, method: &str) -> Option<Result<Value>> {
        if let ValueKind::PyInstance(inst) = val.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, method)
                && is_callable_method(&m)
            {
                let self_val = Value::py_instance(Rc::clone(inst));
                return Some(invoke_class_method(self, m, self_val, &[]));
            }
        }
        None
    }

    /// Three-way ordering comparison that dispatches `__lt__` / `__gt__` on
    /// user instances before falling back to `compare_values` for primitives.
    ///
    /// Used by `sorted()` and `min()` — always tries `__lt__` first,
    /// matching CPython's `Py_LT`-primary reduction for those builtins.
    /// For `max()` call `richcmp_order_gt` instead, which mirrors CPython's
    /// `Py_GT`-primary reduction and emits `'>' not supported` on error.
    ///
    /// Algorithm:
    /// 1. If neither operand is a `PyInstance`, delegate to `compare_values`
    ///    (fast primitive path — zero overhead for the common int/str case).
    /// 2. Try `a < b` via `__lt__` on `a` / `__gt__` on `b`.  If truthy →
    ///    `Less`.
    /// 3. If step 2 returned falsy, try `b < a` to distinguish `Equal` from
    ///    `Greater`.
    /// 4. If no dunder was found, fall through to `compare_values` (which
    ///    raises `TypeError: '<' not supported …`, matching CPython `min` /
    ///    `sorted` behaviour).
    pub(crate) fn richcmp_order(
        &mut self,
        a: &Value,
        b: &Value,
    ) -> Result<std::cmp::Ordering> {
        use std::cmp::Ordering;

        // Fast path: neither operand is a user instance.
        if !matches!(a.kind(), ValueKind::PyInstance(_))
            && !matches!(b.kind(), ValueKind::PyInstance(_))
        {
            return compare_values(a, b);
        }

        // Try a < b (dispatches __lt__ on a, then __gt__ on b).
        match self.try_dunder_binary(a, b, "__lt__", "__gt__") {
            Some(Ok(v)) => {
                let lt = self.truthy_value(&v)?;
                if lt {
                    return Ok(Ordering::Less);
                }
                // a is not less than b; try b < a to tell Equal from Greater.
                match self.try_dunder_binary(b, a, "__lt__", "__gt__") {
                    Some(Ok(v2)) => {
                        return Ok(if self.truthy_value(&v2)? {
                            Ordering::Greater
                        } else {
                            Ordering::Equal
                        });
                    }
                    Some(Err(e)) => return Err(e),
                    // No reverse dunder found: incomparable pair — raise
                    // TypeError just as CPython does for these builtins.
                    None => return compare_values(a, b),
                }
            }
            Some(Err(e)) => return Err(e),
            // No __lt__/__gt__ on either operand; fall through to primitive
            // comparison, which raises TypeError for incomparable instance
            // pairs — matches CPython's behaviour when no comparison dunder
            // is defined.
            None => compare_values(a, b),
        }
    }

    /// Three-way ordering comparison for `max()` — tries `__gt__` first,
    /// matching CPython's `Py_GT`-primary reduction for `max`.
    ///
    /// Emits `TypeError: '>' not supported …` (not `'<'`) when no comparison
    /// dunder is found, matching CPython 3.12 parity for `max()`.
    ///
    /// Algorithm mirrors `richcmp_order` but with primary/reflected dunders
    /// swapped and the fallback error using `>` instead of `<`.
    pub(crate) fn richcmp_order_gt(
        &mut self,
        a: &Value,
        b: &Value,
    ) -> Result<std::cmp::Ordering> {
        use std::cmp::Ordering;

        // Fast path: neither operand is a user instance.
        if !matches!(a.kind(), ValueKind::PyInstance(_))
            && !matches!(b.kind(), ValueKind::PyInstance(_))
        {
            return compare_values(a, b);
        }

        // Try a > b (dispatches __gt__ on a, then __lt__ on b).
        match self.try_dunder_binary(a, b, "__gt__", "__lt__") {
            Some(Ok(v)) => {
                let gt = self.truthy_value(&v)?;
                if gt {
                    return Ok(Ordering::Greater);
                }
                // a is not greater than b; try b > a to tell Equal from Less.
                match self.try_dunder_binary(b, a, "__gt__", "__lt__") {
                    Some(Ok(v2)) => {
                        return Ok(if self.truthy_value(&v2)? {
                            Ordering::Less
                        } else {
                            Ordering::Equal
                        });
                    }
                    Some(Err(e)) => return Err(e),
                    // No reverse dunder found: incomparable pair — raise
                    // TypeError just as CPython does for max().
                    None => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "'>' not supported between instances of '{}' and '{}'",
                            value_type_name_str(a),
                            value_type_name_str(b),
                        ),
                    )),
                }
            }
            Some(Err(e)) => return Err(e),
            // No __gt__/__lt__ on either operand; emit '>' error matching
            // CPython's max() TypeError wording.
            None => Err(PyError::named(
                "TypeError",
                format!(
                    "'>' not supported between instances of '{}' and '{}'",
                    value_type_name_str(a),
                    value_type_name_str(b),
                ),
            )),
        }
    }

    /// Convert a `Value` to a `PyKey`, dispatching the user's `__hash__`
    /// when the value is a `PyInstance` so user-defined classes can be used
    /// as dict/set keys (issue #368).
    ///
    /// For values that already map cleanly to a hashable `PyKey` variant
    /// via `Value::to_key`, this is a thin wrapper that surfaces the
    /// canonical "unhashable type" error.  For `PyInstance`, it looks up
    /// `__hash__` on the class, invokes it, and packages the `u64` hash
    /// (Mersenne-prime reduction + `-1 → -2` sentinel remap, matching the
    /// `hash()` builtin — issue #503) into a `PyKey::Object` along with the
    /// instance value.
    pub(crate) fn value_to_pykey(&mut self, value: &Value) -> Result<PyKey> {
        // Tuples need special handling: the core `Value::to_key` cannot
        // recurse through `PyInstance` elements (it has no interpreter
        // reference), and on an unhashable inner element it collapses the
        // error to a generic "unhashable type: 'tuple'".  CPython instead
        // surfaces the offending inner type (e.g. `unhashable type: 'list'`
        // for `{([1], 2): 0}`).  Recurse element-wise here so user
        // `__hash__` dispatch and precise error messages both work.
        if let ValueKind::Tuple(items) = value.kind() {
            let mut keys = Vec::with_capacity(items.len());
            for item in items {
                keys.push(self.value_to_pykey(item)?);
            }
            return Ok(PyKey::Tuple(keys));
        }
        if let Some(k) = value.to_key() {
            return Ok(k);
        }
        if let ValueKind::PyInstance(inst) = value.kind() {
            let class = Rc::clone(&inst.borrow().class);
            // CPython treats a class that explicitly sets `__hash__ = None`
            // as unhashable.  In pyrust we treat the absence of `__hash__`
            // the same way for now.
            if let Some(hash_method) = lookup_class_attr(&class, "__hash__") {
                if matches!(hash_method.kind(), ValueKind::None) {
                    let class_name = class.borrow().name.clone();
                    return Err(PyError::named(
                        "TypeError",
                        format!("unhashable type: '{class_name}'"),
                    ));
                }
                if is_callable_method(&hash_method) {
                    let result = invoke_class_method(
                        self,
                        hash_method,
                        Value::py_instance(Rc::clone(inst)),
                        &[],
                    )?;
                    // Mirror CPython's slot_tp_hash semantics (issue #503):
                    //
                    // When `__hash__` returns an integer that fits in ssize_t
                    // (i64), CPython takes it as-is, applying only the
                    // `-1 → -2` sentinel remap (`-1` is the C-level tp_hash
                    // error indicator and must never appear as a hash value).
                    //
                    // When `__hash__` returns a value larger than ssize_t can
                    // hold (BigInt here), CPython calls `long_hash` on the
                    // returned Python int, applying Mersenne-prime reduction
                    // (mod 2^61-1) before the remap.  `py_hash_bigint` does
                    // exactly that.
                    //
                    // The stored `u64` must match what `hash(obj)` returns so
                    // that direct-hash probes into the table find their entry.
                    let raw: i64 = match result.kind() {
                        ValueKind::Int(n) => if n == -1 { -2 } else { n },
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::BigInt(n) => py_hash_bigint(n),
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                "__hash__ method should return an integer".to_string(),
                            ));
                        }
                    };
                    return Ok(PyKey::Object {
                        hash: raw as u64,
                        value: value.clone(),
                    });
                }
            }
            // No usable __hash__: fall back to the default object-identity
            // hash so `class Foo: pass` instances remain hashable just like
            // CPython's default `object.__hash__`.
            let ptr = Rc::as_ptr(inst) as usize as u64;
            return Ok(PyKey::Object {
                hash: ptr,
                value: value.clone(),
            });
        }
        let type_name = value_type_name_str(value);
        Err(PyError::named(
            "TypeError",
            format!("unhashable type: '{type_name}'"),
        ))
    }

    /// Look up a key in a dict where the key may be a `PyKey::Object`.
    ///
    /// IndexMap's `get` will find entries whose `PyKey` matches by
    /// pointer-identity (because `PyKey::Object`'s `PartialEq` defers to
    /// `Value::eq`, which uses `Rc::ptr_eq` for `PyInstance`).  When the
    /// fast path misses and the key is an `Object`, we linearly scan
    /// entries with the same precomputed hash and dispatch user `__eq__`
    /// for full Python semantics.  Returns `Ok(Some((index, value)))` on
    /// a hit (index returned so callers can implement `pop`/`del`).
    ///
    /// Takes the receiver `&Value` (rather than `&IndexMap`) so the dict
    /// borrow can be scoped tightly: the fast path borrows for `get_full`
    /// only, and the `__eq__`-dispatching slow path borrows only long
    /// enough to extract the same-hash candidate list before dropping the
    /// borrow and running user code.  This avoids the O(N) whole-dict
    /// snapshot that callers used to have to make for soundness.
    pub(crate) fn dict_lookup(
        &mut self,
        receiver: &Value,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        // Fast path — dict borrow scoped to this block.
        {
            let dict = receiver
                .as_dict()
                .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
            if let Some((idx, _, v)) = dict.get_full(key) {
                return Ok(Some((idx, v.clone())));
            }
        }
        // Slow path — only `Object` keys.  Extract candidates under a
        // narrow borrow, then drop the borrow before user `__eq__` runs.
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let candidates: Vec<(usize, Value, Value)> = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                dict.iter()
                    .enumerate()
                    .filter_map(|(i, (k, v))| match k {
                        PyKey::Object { hash, value } if hash == target_hash => {
                            Some((i, value.clone(), v.clone()))
                        }
                        _ => None,
                    })
                    .collect()
            };
            for (idx, candidate_key, value) in candidates {
                if self.values_user_eq(&candidate_key, target)? {
                    return Ok(Some((idx, value)));
                }
            }
        }
        Ok(None)
    }

    /// `dict_lookup` variant that takes the `IndexMap` directly.  Used by
    /// callers that already hold a `&IndexMap` (typically because they
    /// own/snapshotted the dict, so aliasing with mutable access is
    /// impossible).  Prefer [`Self::dict_lookup`] for new call sites — it
    /// scopes the dict borrow tightly without a whole-dict clone.
    pub(crate) fn dict_lookup_in(
        &mut self,
        dict: &indexmap::IndexMap<PyKey, Value>,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        if let Some((idx, _, v)) = dict.get_full(key) {
            return Ok(Some((idx, v.clone())));
        }
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let candidates: Vec<(usize, Value, Value)> = dict
                .iter()
                .enumerate()
                .filter_map(|(i, (k, v))| match k {
                    PyKey::Object { hash, value } if hash == target_hash => {
                        Some((i, value.clone(), v.clone()))
                    }
                    _ => None,
                })
                .collect();
            for (idx, candidate_key, value) in candidates {
                if self.values_user_eq(&candidate_key, target)? {
                    return Ok(Some((idx, value)));
                }
            }
        }
        Ok(None)
    }

    /// Zero-allocation string key lookup in a dict receiver (issue #506).
    ///
    /// Probes the `IndexMap<PyKey, Value>` using `StrKey`, which hashes
    /// identically to `PyKey::Str` without allocating a `String`.  Use this
    /// in place of `dict_lookup(&PyKey::Str(s.to_owned()))` whenever the
    /// lookup key is already a `&str`.  The `PyKey::Object` slow path is
    /// omitted: a `&str` can never match an `Object` key.
    pub(crate) fn dict_str_lookup(
        &mut self,
        receiver: &Value,
        key: &str,
    ) -> Result<Option<(usize, Value)>> {
        let dict = receiver
            .as_dict()
            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        Ok(dict
            .get_full(&StrKey(key))
            .map(|(idx, _, v)| (idx, v.clone())))
    }

    /// Check whether a set contains `key`, dispatching user `__eq__` for
    /// `PyKey::Object` keys (issue #368).  Returns the entry index so
    /// callers can implement `discard`/`remove`.
    ///
    /// Takes the receiver `&Value` so the set borrow is scoped tightly —
    /// see [`Self::dict_lookup`] for the rationale.
    pub(crate) fn set_lookup(
        &mut self,
        receiver: &Value,
        key: &PyKey,
    ) -> Result<Option<usize>> {
        {
            let set = receiver
                .as_set()
                .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
            if let Some(idx) = set.get_index_of(key) {
                return Ok(Some(idx));
            }
        }
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let candidates: Vec<(usize, Value)> = {
                let set = receiver
                    .as_set()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                set.iter()
                    .enumerate()
                    .filter_map(|(i, k)| match k {
                        PyKey::Object { hash, value } if hash == target_hash => {
                            Some((i, value.clone()))
                        }
                        _ => None,
                    })
                    .collect()
            };
            for (idx, candidate) in candidates {
                if self.values_user_eq(&candidate, target)? {
                    return Ok(Some(idx));
                }
            }
        }
        Ok(None)
    }

    /// `set_lookup` variant that takes the `IndexSet` directly — for
    /// callers that already hold a `&IndexSet`.  Prefer
    /// [`Self::set_lookup`] for new call sites.
    pub(crate) fn set_lookup_in(
        &mut self,
        set: &indexmap::IndexSet<PyKey>,
        key: &PyKey,
    ) -> Result<Option<usize>> {
        if let Some(idx) = set.get_index_of(key) {
            return Ok(Some(idx));
        }
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let candidates: Vec<(usize, Value)> = set
                .iter()
                .enumerate()
                .filter_map(|(i, k)| match k {
                    PyKey::Object { hash, value } if hash == target_hash => {
                        Some((i, value.clone()))
                    }
                    _ => None,
                })
                .collect();
            for (idx, candidate) in candidates {
                if self.values_user_eq(&candidate, target)? {
                    return Ok(Some(idx));
                }
            }
        }
        Ok(None)
    }

    /// Insert `(key, value)` into a dict that lives at register/local
    /// `dict_value`, dispatching user `__eq__` to deduplicate against an
    /// existing entry when `key` is a `PyKey::Object`.  This is the
    /// write-side counterpart to [`dict_lookup`].
    pub(crate) fn dict_insert(
        &mut self,
        dict: &mut indexmap::IndexMap<PyKey, Value>,
        key: PyKey,
        value: Value,
    ) -> Result<()> {
        if let PyKey::Object { .. } = &key {
            if let Some((idx, _)) = self.dict_lookup_in(dict, &key)? {
                // Replace value in-place via index access to preserve order.
                let existing_key = dict.get_index(idx).map(|(k, _)| k.clone());
                if let Some(k) = existing_key {
                    dict.insert(k, value);
                    return Ok(());
                }
            }
        }
        dict.insert(key, value);
        Ok(())
    }

    /// Insert `key` into a set, dispatching user `__eq__` for dedup.
    pub(crate) fn set_insert(
        &mut self,
        set: &mut indexmap::IndexSet<PyKey>,
        key: PyKey,
    ) -> Result<()> {
        if let PyKey::Object { .. } = &key {
            if self.set_lookup_in(set, &key)?.is_some() {
                return Ok(());
            }
        }
        set.insert(key);
        Ok(())
    }

    /// Compare two values via `__eq__`, used by the dict/set runtime when
    /// resolving `PyKey::Object` collisions and by `BinaryOp::Eq`/`Ne`'s
    /// container fall-through path (issue #436).
    ///
    /// Dispatch order, structured to keep the flat-primitive hot path
    /// allocation-free:
    ///
    /// 1. Same-kind sequence pair (`List`/`List` or `Tuple`/`Tuple`):
    ///    `try_seq_fast_eq` walks the borrow pairwise and resolves any
    ///    pair that doesn't transitively need user dispatch via
    ///    `Value::eq`.  This avoids the double-walk an upfront
    ///    `a == b` would cause and matches pre-#436 perf for primitive
    ///    sequences.  When a pair could need dispatch (`PyInstance` or
    ///    nested container), snapshot both sides and recurse.
    /// 2. Primitive / identity fast path: `a == b` for the non-sequence
    ///    cases (`Int`/`Float`/`Bool`/`Str`/`Bytes`/`Complex`/`None`
    ///    and identity-equal `Dict`/`Set`).
    /// 3. Same-kind `Dict`/`Set`: snapshot keys and dispatch via
    ///    `dict_lookup`/`set_lookup`, which already route
    ///    `PyKey::Object` through user `__hash__`/`__eq__` (issue #368).
    /// 4. Both sides are `frozenset` (`BuiltinObject`): same membership
    ///    check as Set but via `set_lookup_in`, so `PyKey::Object`
    ///    elements (user-class instances) dispatch `__eq__` correctly.
    /// 5. `PyInstance` on either side: `try_dunder_binary` for
    ///    `__eq__`/reflected `__eq__`.
    ///
    /// Cycle detection mirrors `Value::eq`'s `EqGuard`: a recursive call
    /// for the same `(value_id(a), value_id(b))` pair returns true (the
    /// recursion bottoms out as "we've already proven the prefix equal"),
    /// so `a.append(a); b.append(b); a == b` doesn't blow the stack.
    pub(crate) fn values_user_eq(&mut self, a: &Value, b: &Value) -> Result<bool> {
        // Same-kind sequence containers come first.  For `List`/`Tuple`
        // pairs an upfront `Value::eq` would double-walk: `Vec::eq`
        // already iterates element-wise, and the recursion below would
        // repeat the walk.  Going straight to `try_seq_fast_eq`
        // resolves flat primitive sequences (`[1,2,3] == [1,2,4]`) in
        // a single borrow-only pass with no allocation — matching
        // pre-#436 perf.  Mixed-kind pairs (e.g. list vs tuple) fall
        // through to the primitive/identity fast path below.
        let needs_seq_dispatch = match (a.kind(), b.kind()) {
            (ValueKind::List(la), ValueKind::List(lb)) => {
                if la.len() != lb.len() {
                    return Ok(false);
                }
                match try_seq_fast_eq(&la, &lb) {
                    SeqFast::Resolved(v) => return Ok(v),
                    SeqFast::NeedsDispatch => true,
                }
            }
            (ValueKind::Tuple(la), ValueKind::Tuple(lb)) => {
                if la.len() != lb.len() {
                    return Ok(false);
                }
                match try_seq_fast_eq(la, lb) {
                    SeqFast::Resolved(v) => return Ok(v),
                    SeqFast::NeedsDispatch => true,
                }
            }
            _ => false,
        };
        if needs_seq_dispatch {
            // Slow path: snapshot both sides to drop the borrow before
            // recursing into user code, then walk element-wise through
            // `values_user_eq` so `PyInstance` elements dispatch
            // `__eq__`.  Element clones are cheap (Rc/NaN-box copy).
            let (av, bv): (Vec<Value>, Vec<Value>) = match (a.kind(), b.kind()) {
                (ValueKind::List(la), ValueKind::List(lb)) => {
                    (la.iter().cloned().collect(), lb.iter().cloned().collect())
                }
                (ValueKind::Tuple(la), ValueKind::Tuple(lb)) => {
                    (la.iter().cloned().collect(), lb.iter().cloned().collect())
                }
                _ => unreachable!("needs_seq_dispatch implies a sequence pair"),
            };
            if self.eq_cycle_enter(a, b) {
                // Already comparing this pair further up the stack —
                // treat as equal to terminate the recursion (matching
                // `Value::eq`'s `EqGuard` policy).
                return Ok(true);
            }
            let result = (|| -> Result<bool> {
                for (x, y) in av.iter().zip(bv.iter()) {
                    if !self.values_user_eq(x, y)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            })();
            self.eq_cycle_exit(a, b);
            return result;
        }

        // Primitive / identity fast path: `Value::eq` handles
        // Int/Float/Bool/Str/Bytes/Complex/None and identity-equal
        // Dict/Set without dunder dispatch.  (List/Tuple were already
        // handled above to avoid the double-walk.)
        if a == b {
            return Ok(true);
        }

        match (a.kind(), b.kind()) {
            (ValueKind::Dict(da), ValueKind::Dict(db)) => {
                if da.len() != db.len() {
                    return Ok(false);
                }
                if self.eq_cycle_enter(a, b) {
                    return Ok(true);
                }
                // Snapshot (PyKey, Value) pairs from `a` so user `__eq__`
                // (run while looking up in `b`) can't invalidate the dict
                // borrow.  We pass the snapshotted `PyKey` straight to
                // `dict_lookup` so `__hash__` / `__eq__` dispatch on
                // `PyKey::Object` keys still works (issue #368).
                let entries: Vec<(PyKey, Value)> = da
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let result = (|| -> Result<bool> {
                    for (pk, v_lhs) in entries {
                        match self.dict_lookup(b, &pk)? {
                            Some((_, v_rhs)) => {
                                if !self.values_user_eq(&v_lhs, &v_rhs)? {
                                    return Ok(false);
                                }
                            }
                            None => return Ok(false),
                        }
                    }
                    Ok(true)
                })();
                self.eq_cycle_exit(a, b);
                return result;
            }
            (ValueKind::Set(sa), ValueKind::Set(sb)) => {
                if sa.len() != sb.len() {
                    return Ok(false);
                }
                if self.eq_cycle_enter(a, b) {
                    return Ok(true);
                }
                let keys: Vec<PyKey> = sa.iter().cloned().collect();
                let result = (|| -> Result<bool> {
                    for pk in keys {
                        if self.set_lookup(b, &pk)?.is_none() {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                })();
                self.eq_cycle_exit(a, b);
                return result;
            }
            _ => {}
        }

        // Frozenset — same membership logic as Set above, but the items
        // live inside a BuiltinObject.  `set_lookup_in` handles
        // `PyKey::Object` elements by dispatching user `__eq__`, so
        // `frozenset({a}) == frozenset({b})` works correctly when
        // `a.__eq__(b)` returns True.  Non-frozenset BuiltinObject pairs
        // fall through to `try_dunder_binary` (the PyInstance path); if
        // that also yields nothing, we return false — identical to
        // `Value::eq`'s behaviour for unrecognised BuiltinObject pairs.
        if let (Some(lhs_rc), Some(rhs_rc)) = (
            pyrust_builtins::frozenset::as_items(a),
            pyrust_builtins::frozenset::as_items(b),
        ) {
            if lhs_rc.len() != rhs_rc.len() {
                return Ok(false);
            }
            if self.eq_cycle_enter(a, b) {
                return Ok(true);
            }
            let lhs_keys: Vec<PyKey> = lhs_rc.iter().cloned().collect();
            let rhs_snap: indexmap::IndexSet<PyKey> = rhs_rc.iter().cloned().collect();
            let result = (|| -> Result<bool> {
                for pk in lhs_keys {
                    if self.set_lookup_in(&rhs_snap, &pk)?.is_none() {
                        return Ok(false);
                    }
                }
                Ok(true)
            })();
            self.eq_cycle_exit(a, b);
            return result;
        }

        // PyInstance (either side) — dispatch `__eq__`/reflected
        // `__eq__`.  This is the original `values_user_eq` body.
        if let Some(r) = self.try_dunder_binary(a, b, "__eq__", "__eq__") {
            return Ok(r?.truthy());
        }
        Ok(false)
    }

    /// Enter equality recursion for the `(value_id(a), value_id(b))`
    /// pair.  Returns `true` when a cycle is detected (the caller should
    /// short-circuit to "equal" without pushing); returns `false`
    /// otherwise after pushing the pair onto the recursion stack.  Each
    /// `false` return must be matched by an `eq_cycle_exit` call.
    ///
    /// Primitives (no `value_id`) can't form cycles, so we return
    /// `false` without recording anything — the missing push is paired
    /// with a no-op `eq_cycle_exit`.
    fn eq_cycle_enter(&mut self, a: &Value, b: &Value) -> bool {
        let (Some(a_id), Some(b_id)) = (a.value_id(), b.value_id()) else {
            return false;
        };
        let pair = (a_id, b_id);
        if self.eq_in_progress.contains(&pair) {
            return true;
        }
        self.eq_in_progress.push(pair);
        false
    }

    /// Pop the matching pair from the recursion stack.  No-op when the
    /// pair wasn't pushed (one operand was a primitive without
    /// `value_id`).
    fn eq_cycle_exit(&mut self, a: &Value, b: &Value) {
        let (Some(a_id), Some(b_id)) = (a.value_id(), b.value_id()) else {
            return;
        };
        if let Some(pos) = self
            .eq_in_progress
            .iter()
            .rposition(|p| *p == (a_id, b_id))
        {
            self.eq_in_progress.remove(pos);
        }
    }

    /// Dispatch any dict method.  Methods that read or write keys
    /// (`get`/`pop`/`setdefault`/`__contains__`) route through
    /// `dict_lookup`/`dict_insert` so user-defined `__hash__`/`__eq__`
    /// fire (issue #368).  Everything else delegates to the
    /// interpreter-free `pyrust_builtins::dict::call`.
    ///
    /// Callers don't need to know which methods are which — this is the
    /// single entry point for dict method dispatch.
    pub(crate) fn call_dict_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "get" | "__contains__" | "pop" | "setdefault" => {
                let mut iter = args.into_iter();
                let key_val = iter.next().ok_or_else(|| {
                    PyError::Runtime(format!("dict.{method}() requires at least 1 argument"))
                })?;
                let pk = self.value_to_pykey(&key_val)?;
                match method {
                    "get" => {
                        let default = iter.next().unwrap_or_else(Value::none);
                        Ok(self
                            .dict_lookup(&receiver, &pk)?
                            .map(|(_, v)| v)
                            .unwrap_or(default))
                    }
                    "__contains__" => Ok(Value::bool_(
                        self.dict_lookup(&receiver, &pk)?.is_some(),
                    )),
                    "pop" => match self.dict_lookup(&receiver, &pk)? {
                        Some((idx, v)) => {
                            // `dict_lookup` already dropped its borrow before
                            // running user code, so the index is still valid.
                            receiver.dict_with_mut(|dict| dict.shift_remove_index(idx));
                            Ok(v)
                        }
                        None => {
                            if let Some(default) = iter.next() {
                                Ok(default)
                            } else {
                                Err(PyError::key_error(key_val.clone()))
                            }
                        }
                    },
                    "setdefault" => {
                        let default = iter.next().unwrap_or_else(Value::none);
                        if let Some((_, v)) = self.dict_lookup(&receiver, &pk)? {
                            return Ok(v);
                        }
                        receiver
                            .dict_with_mut(|dict| dict.insert(pk, default.clone()))
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected dict".to_string())
                            })?;
                        Ok(default)
                    }
                    _ => unreachable!(),
                }
            }
            _ => pyrust_builtins::dict::call(method, &receiver, args),
        }
    }

    /// Dispatch any set method.  Methods that read or write keys
    /// (`add`/`discard`/`remove`/`__contains__`) route through
    /// `set_lookup`/`set_insert` so user-defined `__hash__`/`__eq__`
    /// fire (issue #368).  Everything else delegates to the
    /// interpreter-free `pyrust_builtins::set::call`.
    pub(crate) fn call_set_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "add" | "__contains__" | "discard" | "remove" => {
                let mut iter = args.into_iter();
                let key_val = iter.next().ok_or_else(|| {
                    PyError::Runtime(format!("set.{method}() requires at least 1 argument"))
                })?;
                let pk = self.value_to_pykey(&key_val)?;
                match method {
                    "add" => {
                        if self.set_lookup(&receiver, &pk)?.is_some() {
                            return Ok(Value::none());
                        }
                        receiver.set_add(pk)?;
                        Ok(Value::none())
                    }
                    "__contains__" => {
                        Ok(Value::bool_(self.set_lookup(&receiver, &pk)?.is_some()))
                    }
                    "discard" => {
                        if let Some(idx) = self.set_lookup(&receiver, &pk)? {
                            receiver
                                .set_with_mut(|set| {
                                    set.shift_remove_index(idx);
                                })
                                .ok_or_else(|| {
                                    PyError::Runtime("internal: expected set".to_string())
                                })?;
                        }
                        Ok(Value::none())
                    }
                    "remove" => match self.set_lookup(&receiver, &pk)? {
                        Some(idx) => {
                            receiver
                                .set_with_mut(|set| {
                                    set.shift_remove_index(idx);
                                })
                                .ok_or_else(|| {
                                    PyError::Runtime("internal: expected set".to_string())
                                })?;
                            Ok(Value::none())
                        }
                        None => Err(PyError::key_error(key_val.clone())),
                    },
                    _ => unreachable!(),
                }
            }
            _ => pyrust_builtins::set::call(method, &receiver, args),
        }
    }

    /// Dispatch any str method.  `join` is handled here to support generators
    /// and any custom iterable via `collect_iterable`; `format` is intercepted
    /// in the bound-method dispatch path in `calls.rs` (which has access to
    /// kwargs) before reaching this function.  Everything else delegates to
    /// the interpreter-free `pyrust_builtins::string::call`.
    pub(crate) fn call_str_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if method == "join" {
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "str.join() takes exactly one argument ({} given)",
                        args.len()
                    ),
                ));
            }
            let iterable = args.into_iter().next().unwrap();
            // Fast paths: types already handled directly by the builtins join fn.
            // Check the tag first (drops the borrow) before deciding whether to
            // call collect_iterable — the borrow from kind() must not overlap
            // with the &mut self borrow that collect_iterable needs.
            let needs_collect = !matches!(
                iterable.kind(),
                ValueKind::List(_)
                    | ValueKind::Tuple(_)
                    | ValueKind::Str(_)
                    | ValueKind::Dict(_)
            );
            let iterable = if needs_collect {
                let items = self.collect_iterable(iterable).map_err(|e| {
                    // Only rewrite "not iterable" TypeErrors as CPython's
                    // "can only join an iterable". TypeErrors raised by user
                    // code inside __iter__/__next__ or a generator body must
                    // propagate unchanged (#576 Copilot review).
                    let is_not_iterable = e.class_name_is("TypeError")
                        && matches!(&e,
                            PyError::Named(_, msg) | PyError::Class(_, msg)
                                if msg.contains("is not iterable"));
                    if is_not_iterable {
                        PyError::named("TypeError", "can only join an iterable".to_string())
                    } else {
                        e
                    }
                })?;
                Value::list(items)
            } else {
                iterable
            };
            return pyrust_builtins::string::call("join", &receiver, vec![iterable]);
        }
        pyrust_builtins::string::call(method, &receiver, args)
    }

    pub(crate) fn eval_binary(&mut self, left: Value, op: BinaryOp, right: Value) -> Result<Value> {
        match op {
            BinaryOp::Add => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__add__", "__radd__") {
                    return r;
                }
                self.add(left, right)
            }
            BinaryOp::Sub => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__sub__", "__rsub__") {
                    return r;
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::Sub) {
                    return r;
                }
                self.sub(left, right)
            }
            BinaryOp::Mul => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mul__", "__rmul__") {
                    return r;
                }
                self.mul(left, right)
            }
            BinaryOp::MatMul => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__matmul__", "__rmatmul__") {
                    return r;
                }
                self.matmul(left, right)
            }
            BinaryOp::Div => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__truediv__", "__rtruediv__") {
                    return r;
                }
                self.div(left, right)
            }
            BinaryOp::FloorDiv => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__floordiv__", "__rfloordiv__") {
                    return r;
                }
                self.floor_div(left, right)
            }
            BinaryOp::Mod => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mod__", "__rmod__") {
                    return r;
                }
                self.modulo(left, right)
            }
            BinaryOp::Eq => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__eq__", "__eq__") {
                    return r;
                }
                // Containers fall through here: `Value::eq` would call
                // `Rc::ptr_eq` on `PyInstance` elements, missing user
                // `__eq__`.  Route through `values_user_eq` so list /
                // tuple / dict / set element comparison dispatches
                // `__eq__` recursively (issue #436).
                Ok(Value::bool_(self.values_user_eq(&left, &right)?))
            }
            BinaryOp::Ne => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__ne__", "__ne__") {
                    return r;
                }
                Ok(Value::bool_(!self.values_user_eq(&left, &right)?))
            }
            BinaryOp::Lt => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__lt__", "__gt__") {
                    return r;
                }
                self.compare(left, right, "<", |o| o.is_lt())
            }
            BinaryOp::Le => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__le__", "__ge__") {
                    return r;
                }
                self.compare(left, right, "<=", |o| o.is_le())
            }
            BinaryOp::Gt => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__gt__", "__lt__") {
                    return r;
                }
                self.compare(left, right, ">", |o| o.is_gt())
            }
            BinaryOp::Ge => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__ge__", "__le__") {
                    return r;
                }
                self.compare(left, right, ">=", |o| o.is_ge())
            }
            BinaryOp::Pow => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__pow__", "__rpow__") {
                    return r;
                }
                // Integer ** non-negative integer stays in the int domain
                // (BigInt promotion when the result overflows i64).  Once
                // overflow promotes (#421), `(2**64) ** 2` arrives here as
                // `BigInt ** Int`; the dedicated arms below handle those
                // cross-type cases.  Negative exponent on an int LHS
                // returns Float (CPython parity).  See PR #484 Copilot
                // review.
                match (left.kind(), right.kind()) {
                    (ValueKind::Int(a), ValueKind::Int(b)) if b >= 0 => {
                        Ok(int_pow_promoting(a, b))
                    }
                    (ValueKind::BigInt(a), ValueKind::Int(b)) if b >= 0 => {
                        Ok(Value::bigint(PyPow::pow(a.clone(), b as u64)))
                    }
                    (ValueKind::Int(a), ValueKind::BigInt(b)) if *b >= PyBigInt::from(0) => {
                        // BigInt exponent: astronomically large for |a| > 1.
                        // Promote a to BigInt and delegate to BigInt::pow
                        // if the exponent fits in u64; otherwise raise
                        // OverflowError (CPython parity at the
                        // "exponentiation cannot produce a result that
                        // fits in memory" boundary).
                        match b.to_u64_digits().1.as_slice() {
                            [exp] => Ok(Value::bigint(PyPow::pow(PyBigInt::from(a), *exp))),
                            [] => Ok(Value::int(1)), // a ** 0
                            _ => Err(PyError::named(
                                "OverflowError",
                                "exponent too large for ** to compute".to_string(),
                            )),
                        }
                    }
                    (ValueKind::BigInt(a), ValueKind::BigInt(b)) if *b >= PyBigInt::from(0) => {
                        match b.to_u64_digits().1.as_slice() {
                            [exp] => Ok(Value::bigint(PyPow::pow(a.clone(), *exp))),
                            [] => Ok(Value::int(1)),
                            _ => Err(PyError::named(
                                "OverflowError",
                                "exponent too large for ** to compute".to_string(),
                            )),
                        }
                    }
                    _ => {
                        // When either operand is complex, use complex
                        // exponentiation: z^w = exp(w * ln(z)).
                        // `both_as_complex` returns Some only when at least
                        // one operand is already a Complex value, so pure
                        // int/float paths continue to use `powf` below.
                        // BigInt is not handled by `as_complex_pair`
                        // (changing that would affect add/sub/mul/div too);
                        // instead we promote BigInt to f64 here when the
                        // other operand is Complex, matching CPython's
                        // `int.__pow__(complex)` coercion behaviour.
                        if let Some(((zr, zi), (wr, wi))) = both_as_complex(&left, &right) {
                            return Ok(complex_pow(zr, zi, wr, wi)?);
                        }
                        let l_is_c = matches!(left.kind(), ValueKind::Complex(_, _));
                        let r_is_c = matches!(right.kind(), ValueKind::Complex(_, _));
                        if l_is_c || r_is_c {
                            // At least one is Complex but `both_as_complex` returned
                            // None — the other operand must be BigInt.
                            let (zr, zi) = match left.kind() {
                                ValueKind::Complex(re, im) => (re, im),
                                ValueKind::BigInt(b) => (bigint_to_float_or_overflow(b)?, 0.0),
                                _ => unreachable!(),
                            };
                            let (wr, wi) = match right.kind() {
                                ValueKind::Complex(re, im) => (re, im),
                                ValueKind::BigInt(b) => (bigint_to_float_or_overflow(b)?, 0.0),
                                _ => unreachable!(),
                            };
                            return Ok(complex_pow(zr, zi, wr, wi)?);
                        }
                        let a = value_to_float(&left, "**")?;
                        let b = value_to_float(&right, "**")?;
                        // CPython `float_pow` (Objects/floatobject.c) checks
                        // `iv == 0.0 && iw < 0.0` before delegating to pow().
                        // IEEE 754 equality treats +0.0 and -0.0 as equal, so
                        // this guard covers both signs with the same message.
                        // The exponent must be finite: 0.0 ** -inf = inf per
                        // IEEE 754 (|0| < 1, so |0|^(-inf) = inf), which
                        // CPython's C pow() returns correctly.
                        if a == 0.0 && b < 0.0 && b.is_finite() {
                            return Err(PyError::named(
                                "ZeroDivisionError",
                                "0.0 cannot be raised to a negative power".to_string(),
                            ));
                        }
                        let result = a.powf(b);
                        // CPython promotes negative_real ** non-integer_float to
                        // complex when the real result would be NaN (principal
                        // log branch: a^b = |a|^b * e^(i*π*b)).
                        if a < 0.0 && result.is_nan() {
                            let abs_val = a.abs().powf(b);
                            let angle = std::f64::consts::PI * b;
                            Ok(Value::complex(abs_val * angle.cos(), abs_val * angle.sin()))
                        } else {
                            Ok(Value::float(result))
                        }
                    }
                }
            }
            BinaryOp::BitAnd => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__and__", "__rand__") {
                    return r;
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::And) {
                    return r;
                }
                // BigInt × int / int × BigInt / BigInt × BigInt all flow
                // through the BigInt path; int × int stays on the fast
                // path inside `bitwise_op`.  See issue #485.
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named("TypeError", "bitwise op requires integer".to_string())
                    })?;
                    let b = value_to_bigint(&right).ok_or_else(|| {
                        PyError::named("TypeError", "bitwise op requires integer".to_string())
                    })?;
                    return Ok(Value::bigint(a & b));
                }
                self.bitwise_op(&left, &right, |a, b| Ok(a & b))
            }
            BinaryOp::BitOr => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__or__", "__ror__") {
                    return r;
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::Or) {
                    return r;
                }
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named("TypeError", "bitwise op requires integer".to_string())
                    })?;
                    let b = value_to_bigint(&right).ok_or_else(|| {
                        PyError::named("TypeError", "bitwise op requires integer".to_string())
                    })?;
                    return Ok(Value::bigint(a | b));
                }
                self.bitwise_op(&left, &right, |a, b| Ok(a | b))
            }
            BinaryOp::BitXor => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__xor__", "__rxor__") {
                    return r;
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::Xor) {
                    return r;
                }
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named("TypeError", "bitwise op requires integer".to_string())
                    })?;
                    let b = value_to_bigint(&right).ok_or_else(|| {
                        PyError::named("TypeError", "bitwise op requires integer".to_string())
                    })?;
                    return Ok(Value::bigint(a ^ b));
                }
                self.bitwise_op(&left, &right, |a, b| Ok(a ^ b))
            }
            BinaryOp::LShift => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__lshift__", "__rlshift__") {
                    return r;
                }
                // BigInt LHS: shift exactly, no `& 63` truncation.
                // Int LHS with a BigInt RHS: the shift count is
                // astronomically large.  See #485.
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named("TypeError", "bitwise op requires integer".to_string())
                    })?;
                    return match shift_count(&right)? {
                        ShiftCount::Fits(n) => {
                            if n > MAX_SHIFT && !a.is_zero() {
                                return Err(PyError::named(
                                    "OverflowError",
                                    "too many digits in integer".to_string(),
                                ));
                            }
                            Ok(Value::bigint(a << n))
                        }
                        // CPython: `0 << huge == 0` (no allocation
                        // needed), otherwise OverflowError because the
                        // result would not fit in memory.
                        ShiftCount::Saturated => {
                            if a.is_zero() {
                                Ok(Value::bigint(a))
                            } else {
                                Err(PyError::named(
                                    "OverflowError",
                                    "too many digits in integer".to_string(),
                                ))
                            }
                        }
                    };
                }
                // Int LHS, Int (or Bool) RHS — must validate and may promote to BigInt.
                let a = match left.kind() {
                    ValueKind::Int(v) => v,
                    ValueKind::Bool(b) => if b { 1 } else { 0 },
                    _ => return Err(PyError::named("TypeError", "bitwise op requires integer".to_string())),
                };
                match shift_count(&right)? {
                    ShiftCount::Fits(n) => {
                        if n > MAX_SHIFT && a != 0 {
                            return Err(PyError::named(
                                "OverflowError",
                                "too many digits in integer".to_string(),
                            ));
                        }
                        // Shift left, promoting to BigInt when bits are lost.
                        let big = PyBigInt::from(a) << n;
                        Ok(match big.to_i64() {
                            Some(r) => Value::int(r),
                            None => Value::bigint(big),
                        })
                    }
                    ShiftCount::Saturated => {
                        if a == 0 {
                            Ok(Value::int(0))
                        } else {
                            Err(PyError::named(
                                "OverflowError",
                                "too many digits in integer".to_string(),
                            ))
                        }
                    }
                }
            }
            BinaryOp::RShift => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__rshift__", "__rrshift__") {
                    return r;
                }
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named("TypeError", "bitwise op requires integer".to_string())
                    })?;
                    return match shift_count(&right)? {
                        ShiftCount::Fits(n) => Ok(Value::bigint(a >> n)),
                        // CPython: `>>` by a count larger than the
                        // value's bit length collapses to the sign
                        // (`0` for non-negative, `-1` for negative) —
                        // never raises.
                        ShiftCount::Saturated => Ok(Value::bigint(match a.sign() {
                            PyBigIntSign::Minus => PyBigInt::from(-1i64),
                            _ => PyBigInt::from(0i64),
                        })),
                    };
                }
                // Int LHS, Int (or Bool) RHS.
                let a = match left.kind() {
                    ValueKind::Int(v) => v,
                    ValueKind::Bool(b) => if b { 1 } else { 0 },
                    _ => return Err(PyError::named("TypeError", "bitwise op requires integer".to_string())),
                };
                match shift_count(&right)? {
                    ShiftCount::Fits(n) => {
                        // Arithmetic right shift: always fits in i64.
                        if n >= 64 {
                            Ok(Value::int(if a < 0 { -1 } else { 0 }))
                        } else {
                            Ok(Value::int(a >> n))
                        }
                    }
                    // Saturate to sign bit — matches CPython, no error.
                    ShiftCount::Saturated => Ok(Value::int(if a < 0 { -1 } else { 0 })),
                }
            }
            BinaryOp::In => self.eval_in(right, left),
            BinaryOp::NotIn => Ok(Value::bool_(!self.eval_in(right, left)?.truthy())),
            BinaryOp::Is    => Ok(Value::bool_(values_are_identical(&left, &right))),
            BinaryOp::IsNot => Ok(Value::bool_(!values_are_identical(&left, &right))),
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuit handled earlier"),
        }
    }

    fn add(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            return Ok(Value::complex(a.0 + b.0, a.1 + b.1));
        }
        let (l, r) = (coerce_numeric(left), coerce_numeric(right));
        match (l.kind(), r.kind()) {
                (ValueKind::Int(a), ValueKind::Int(b)) => Ok(match a.checked_add(b) {
                    Some(r) => Value::int(r),
                    None => Value::bigint(PyBigInt::from(a) + PyBigInt::from(b)),
                }),
                (ValueKind::Int(a), ValueKind::Float(b)) => Ok(Value::float((a as f64) + b)),
                (ValueKind::Float(a), ValueKind::Int(b)) => Ok(Value::float(a + (b as f64))),
                (ValueKind::Float(a), ValueKind::Float(b)) => Ok(Value::float(a + b)),
                (ValueKind::BigInt(a), ValueKind::BigInt(b)) => Ok(Value::bigint(a + b)),
                (ValueKind::BigInt(a), ValueKind::Int(b)) => Ok(Value::bigint(a + PyBigInt::from(b))),
                (ValueKind::Int(a), ValueKind::BigInt(b)) => Ok(Value::bigint(PyBigInt::from(a) + b)),
                (ValueKind::BigInt(a), ValueKind::Float(b)) => {
                    Ok(Value::float(bigint_to_float_or_overflow(&a)? + b))
                }
                (ValueKind::Float(a), ValueKind::BigInt(b)) => {
                    Ok(Value::float(a + bigint_to_float_or_overflow(&b)?))
                }
                (ValueKind::Str(a), ValueKind::Str(b)) => Ok(Value::string(format!("{a}{b}"))),
                (ValueKind::List(a), ValueKind::List(b)) => {
                    let mut out = a.to_vec();
                    out.extend_from_slice(&b[..]);
                    Ok(Value::list(out))
                }
                (ValueKind::Tuple(a), ValueKind::Tuple(b)) => {
                    let mut out = a.to_vec();
                    out.extend_from_slice(b);
                    Ok(Value::tuple(out))
                }
                (ValueKind::Bytes(a), ValueKind::Bytes(b)) => {
                    let mut out = a.as_ref().clone();
                    out.extend_from_slice(b);
                    Ok(Value::bytes(out))
                }
                (ValueKind::Bytes(_), _) => Err(PyError::named(
                    "TypeError",
                    format!(
                        "can't concat {} to bytes",
                        pyrust_core::builtin_type_name(&r)
                    ),
                )),
                _ => Err(Self::unsupported_binary_operand("+")),
        }
    }

    fn sub(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            return Ok(Value::complex(a.0 - b.0, a.1 - b.1));
        }
        let (l, r) = (coerce_numeric(left), coerce_numeric(right));
        match (l.kind(), r.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => Ok(match a.checked_sub(b) {
                Some(r) => Value::int(r),
                None => Value::bigint(PyBigInt::from(a) - PyBigInt::from(b)),
            }),
            (ValueKind::Int(a), ValueKind::Float(b)) => Ok(Value::float((a as f64) - b)),
            (ValueKind::Float(a), ValueKind::Int(b)) => Ok(Value::float(a - (b as f64))),
            (ValueKind::Float(a), ValueKind::Float(b)) => Ok(Value::float(a - b)),
            (ValueKind::BigInt(a), ValueKind::BigInt(b)) => Ok(Value::bigint(a - b)),
            (ValueKind::BigInt(a), ValueKind::Int(b)) => Ok(Value::bigint(a - PyBigInt::from(b))),
            (ValueKind::Int(a), ValueKind::BigInt(b)) => Ok(Value::bigint(PyBigInt::from(a) - b)),
            (ValueKind::BigInt(a), ValueKind::Float(b)) => {
                Ok(Value::float(bigint_to_float_or_overflow(&a)? - b))
            }
            (ValueKind::Float(a), ValueKind::BigInt(b)) => {
                Ok(Value::float(a - bigint_to_float_or_overflow(&b)?))
            }
            _ => Err(Self::unsupported_binary_operand("-")),
        }
    }

    fn mul(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            // (ar+ai*j) * (br+bi*j) = (ar*br - ai*bi) + (ar*bi + ai*br)j
            return Ok(Value::complex(a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0));
        }
        let (l, r) = (coerce_numeric(left), coerce_numeric(right));
        match (l.kind(), r.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => Ok(match a.checked_mul(b) {
                Some(r) => Value::int(r),
                None => Value::bigint(PyBigInt::from(a) * PyBigInt::from(b)),
            }),
            (ValueKind::Int(a), ValueKind::Float(b)) => Ok(Value::float((a as f64) * b)),
            (ValueKind::Float(a), ValueKind::Int(b)) => Ok(Value::float(a * (b as f64))),
            (ValueKind::Float(a), ValueKind::Float(b)) => Ok(Value::float(a * b)),
            (ValueKind::BigInt(a), ValueKind::BigInt(b)) => Ok(Value::bigint(a * b)),
            (ValueKind::BigInt(a), ValueKind::Int(b)) => Ok(Value::bigint(a * PyBigInt::from(b))),
            (ValueKind::Int(a), ValueKind::BigInt(b)) => Ok(Value::bigint(PyBigInt::from(a) * b)),
            (ValueKind::BigInt(a), ValueKind::Float(b)) => {
                Ok(Value::float(bigint_to_float_or_overflow(&a)? * b))
            }
            (ValueKind::Float(a), ValueKind::BigInt(b)) => {
                Ok(Value::float(a * bigint_to_float_or_overflow(&b)?))
            }
            (ValueKind::Str(text), ValueKind::Int(n)) => {
                if n <= 0 { Ok(Value::string(String::new())) }
                else { Ok(Value::string(text.repeat(n as usize))) }
            }
            (ValueKind::Int(n), ValueKind::Str(text)) => {
                if n <= 0 { Ok(Value::string(String::new())) }
                else { Ok(Value::string(text.repeat(n as usize))) }
            }
            (ValueKind::List(items), ValueKind::Int(n)) => {
                if n <= 0 { return Ok(Value::list(Vec::new())); }
                let n = n as usize;
                let mut out = Vec::with_capacity(items.len() * n);
                for _ in 0..n { out.extend_from_slice(&items[..]); }
                Ok(Value::list(out))
            }
            (ValueKind::Int(n), ValueKind::List(items)) => {
                if n <= 0 { return Ok(Value::list(Vec::new())); }
                let n = n as usize;
                let mut out = Vec::with_capacity(items.len() * n);
                for _ in 0..n { out.extend_from_slice(&items[..]); }
                Ok(Value::list(out))
            }
            (ValueKind::Bytes(data), ValueKind::Int(n)) => {
                if n <= 0 { return Ok(Value::bytes(Vec::new())); }
                Ok(Value::bytes(data.repeat(n as usize)))
            }
            (ValueKind::Int(n), ValueKind::Bytes(data)) => {
                if n <= 0 { return Ok(Value::bytes(Vec::new())); }
                Ok(Value::bytes(data.repeat(n as usize)))
            }
            _ => Err(Self::unsupported_binary_operand("*")),
        }
    }

    /// Dispatch a single binary method (e.g. `__iadd__`) on a
    /// PyInstance receiver.  Returns `Some(result)` when the method
    /// exists and was called (possibly returning `NotImplemented`),
    /// `None` when the method isn't defined on the class.  Like
    /// `try_dunder_binary`, this routes both user-defined and
    /// `pyrust_module!`-generated class methods through
    /// `invoke_class_method` so Counter's `__iadd__` (a BuiltinFunction
    /// in the class's attr map) participates in `+=` dispatch.
    fn try_call_binary_method(
        &mut self,
        receiver: &Value,
        method: &str,
        other: Value,
    ) -> Result<Option<Value>> {
        let inst = match receiver.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => return Ok(None),
        };
        let class = Rc::clone(&inst.borrow().class);
        let Some(method_value) = lookup_class_attr(&class, method) else {
            return Ok(None);
        };
        if !is_callable_method(&method_value) {
            return Ok(None);
        }
        let self_val = Value::py_instance(Rc::clone(&inst));
        let arg = ExpandedCallArg {
            name: None,
            value: other,
        };
        let result = invoke_class_method(self, method_value, self_val, &[arg])?;
        Ok(Some(result))
    }

    pub(crate) fn try_inplace_op(
        &mut self,
        left: Value,
        op: BinaryOp,
        right: Value,
    ) -> Result<Option<Value>> {
        let dunder = match op {
            BinaryOp::Add => "__iadd__",
            BinaryOp::Sub => "__isub__",
            BinaryOp::Mul => "__imul__",
            BinaryOp::MatMul => "__imatmul__",
            BinaryOp::Div => "__itruediv__",
            BinaryOp::FloorDiv => "__ifloordiv__",
            BinaryOp::Mod => "__imod__",
            BinaryOp::Pow => "__ipow__",
            BinaryOp::BitAnd => "__iand__",
            BinaryOp::BitOr => "__ior__",
            BinaryOp::BitXor => "__ixor__",
            BinaryOp::LShift => "__ilshift__",
            BinaryOp::RShift => "__irshift__",
            _ => return Ok(None),
        };
        let result = self.try_call_binary_method(&left, dunder, right)?;
        if let Some(ref v) = result
            && is_not_implemented(v) {
                return Ok(None);
            }
        Ok(result)
    }

    fn matmul(&mut self, left: Value, right: Value) -> Result<Value> {
        if let Some(value) = self.try_call_binary_method(&left, "__matmul__", right.clone())? {
            return Ok(value);
        }
        if let Some(value) = self.try_call_binary_method(&right, "__rmatmul__", left.clone())? {
            return Ok(value);
        }
        Err(Self::unsupported_binary_operand("@"))
    }



    fn div(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            // (ar+ai*j) / (br+bi*j) = ((ar*br + ai*bi) + (ai*br - ar*bi)j) / (br^2 + bi^2)
            let denom = b.0 * b.0 + b.1 * b.1;
            if denom == 0.0 {
                return Err(PyError::named(
                    "ZeroDivisionError",
                    "complex division by zero".to_string(),
                ));
            }
            return Ok(Value::complex(
                (a.0 * b.0 + a.1 * b.1) / denom,
                (a.1 * b.0 - a.0 * b.1) / denom,
            ));
        }
        // CPython distinguishes wording by operand types: `int / int` says
        // "division by zero"; anything involving a float says "float
        // division by zero".  Decide *before* `to_pair_number` coerces
        // both operands to f64.
        let both_int = matches!(
            (left.kind(), right.kind()),
            (
                ValueKind::Int(_) | ValueKind::Bool(_),
                ValueKind::Int(_) | ValueKind::Bool(_),
            ),
        );
        let (a, b) = self.to_pair_number(left, right)?;
        if b == 0.0 {
            return Err(PyError::named(
                "ZeroDivisionError",
                if both_int {
                    "division by zero".to_string()
                } else {
                    "float division by zero".to_string()
                },
            ));
        }
        Ok(Value::float(a / b))
    }

    fn floor_div(&self, left: Value, right: Value) -> Result<Value> {
        // Extract the int/int fast-path values in a scoped block so the
        // `kind()` Ref guards drop before we may need to move
        // `left`/`right` into `to_pair_number` (#450).
        let int_pair: Option<(i64, i64)> = match (left.kind(), right.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => Some((a, b)),
            _ => None,
        };
        if let Some((a, b)) = int_pair {
            if b == 0 {
                return Err(PyError::named(
                    "ZeroDivisionError",
                    "integer division or modulo by zero".to_string(),
                ));
            }
            let modulo = py_mod_i64(a, b);
            return Ok(Value::int((a - modulo) / b));
        }
        // BigInt cross-type arms (#485): once #421 promotes overflow to
        // BigInt, `(2**64) // 2` arrives here with a BigInt operand.
        // Bool coerces to int so `big // True` works.  Float operands
        // fall through to the float path below.
        if matches!(left.kind(), ValueKind::BigInt(_))
            || matches!(right.kind(), ValueKind::BigInt(_))
        {
            if let (Some(a), Some(b)) = (value_to_bigint(&left), value_to_bigint(&right)) {
                if b.is_zero() {
                    return Err(PyError::named(
                        "ZeroDivisionError",
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                let (q, _) = bigint_divmod_floor(&a, &b);
                return Ok(Value::bigint(q));
            }
        }
        let (a, b) = self.to_pair_number(left, right)?;
        if b == 0.0 {
            return Err(PyError::named(
                "ZeroDivisionError",
                "float floor division by zero".to_string(),
            ));
        }
        Ok(Value::float((a / b).floor()))
    }

    fn modulo(&self, left: Value, right: Value) -> Result<Value> {
        // Same #450 scoping rationale as `floor_div`.
        let int_pair: Option<(i64, i64)> = match (left.kind(), right.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => Some((a, b)),
            _ => None,
        };
        if let Some((a, b)) = int_pair {
            if b == 0 {
                return Err(PyError::named(
                    "ZeroDivisionError",
                    "integer modulo by zero".to_string(),
                ));
            }
            return Ok(Value::int(py_mod_i64(a, b)));
        }
        // BigInt cross-type arms (#485) — see `floor_div` for rationale.
        if matches!(left.kind(), ValueKind::BigInt(_))
            || matches!(right.kind(), ValueKind::BigInt(_))
        {
            if let (Some(a), Some(b)) = (value_to_bigint(&left), value_to_bigint(&right)) {
                if b.is_zero() {
                    return Err(PyError::named(
                        "ZeroDivisionError",
                        "integer modulo by zero".to_string(),
                    ));
                }
                let (_, r) = bigint_divmod_floor(&a, &b);
                return Ok(Value::bigint(r));
            }
        }
        let (a, b) = self.to_pair_number(left, right)?;
        if b == 0.0 {
            return Err(PyError::named(
                "ZeroDivisionError",
                "float modulo".to_string(),
            ));
        }
        Ok(Value::float(a - b * (a / b).floor()))
    }

    fn compare(
        &self,
        left: Value,
        right: Value,
        op_name: &str,
        cmp: impl Fn(std::cmp::Ordering) -> bool,
    ) -> Result<Value> {
        if matches!(left.kind(), ValueKind::Float(f) if f.is_nan())
            || matches!(right.kind(), ValueKind::Float(f) if f.is_nan())
        {
            return Ok(Value::bool_(false));
        }
        Ok(Value::bool_(cmp(compare_values_with_op(&left, &right, op_name)?)))
    }

    fn to_pair_number(&self, left: Value, right: Value) -> Result<(f64, f64)> {
        Ok((self.to_number(&left)?, self.to_number(&right)?))
    }

    fn to_number(&self, value: &Value) -> Result<f64> {
        match value.kind() {
            ValueKind::Int(v) => Ok(v as f64),
            ValueKind::Float(v) => Ok(v),
            ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
            _ => Err(PyError::named("TypeError", "expected number".to_string())),
        }
    }

    fn eval_slice(&mut self, target: Value, lo: Option<Value>, hi: Option<Value>, st: Option<Value>) -> Result<Value> {
        let len = match target.kind() {
            ValueKind::List(items) => items.len() as i64,
            ValueKind::Str(s) => s.chars().count() as i64,
            _ => return Err(PyError::Runtime("object is not sliceable".to_string())),
        };
        let (start, end, step) = Self::resolve_slice_bounds(len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
        let indices = Self::slice_target_indices(len, start, end, step);

        match target.kind() {
            ValueKind::List(items) => Ok(Value::list(indices.into_iter().map(|ix| items[ix].clone()).collect::<Vec<Value>>())),
            ValueKind::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let mut out = String::new();
                for ix in indices {
                    out.push(chars[ix]);
                }
                Ok(Value::string(out))
            }
            _ => unreachable!(),
        }
    }

    fn bitwise_op(&self, left: &Value, right: &Value, op: impl Fn(i64, i64) -> Result<i64>) -> Result<Value> {
        let a = match left.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => if b { 1 } else { 0 },
            _ => return Err(PyError::named("TypeError", "bitwise op requires integer".to_string())),
        };
        let b = match right.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => if b { 1 } else { 0 },
            _ => return Err(PyError::named("TypeError", "bitwise op requires integer".to_string())),
        };
        Ok(Value::int(op(a, b)?))
    }
    fn eval_in(&mut self, container: Value, item: Value) -> Result<Value> {
        // Handle Dict/Set separately so the temporary `&IndexMap`/`&IndexSet`
        // from `container.kind()` doesn't outlive the call into
        // `dict_lookup`/`set_lookup` (which may run user `__eq__`).
        if container.as_dict().is_some() {
            let found = if let Some(s) = item.as_str() {
                self.dict_str_lookup(&container, s)?.is_some()
            } else {
                let key = self.value_to_pykey(&item)?;
                self.dict_lookup(&container, &key)?.is_some()
            };
            return Ok(Value::bool_(found));
        }
        if container.as_set().is_some() {
            let key = self.value_to_pykey(&item)?;
            return Ok(Value::bool_(
                self.set_lookup(&container, &key)?.is_some(),
            ));
        }
        // Frozenset membership — must intercept before the generic BuiltinObject
        // arm because `FrozenSetOps::contains` calls `item.to_key()` which has
        // no interpreter access and cannot dispatch user `__hash__`.  Mirror the
        // Set path above: get the key via `value_to_pykey` (which runs user
        // `__hash__`) then search the underlying `IndexSet` via `set_lookup_in`
        // (which dispatches user `__eq__` for `PyKey::Object` entries).
        if let Some(rc) = pyrust_builtins::frozenset::as_items(&container) {
            let key = self.value_to_pykey(&item)?;
            return Ok(Value::bool_(self.set_lookup_in(&rc, &key)?.is_some()));
        }
        match container.kind() {
            ValueKind::List(items) => Ok(Value::bool_(items.iter().any(|b| b == &item))),
            ValueKind::Tuple(items) => Ok(Value::bool_(items.contains(&item))),
            ValueKind::Set(_) => unreachable!("handled above"),
            ValueKind::BuiltinObject { ops, state } => {
                ops.contains(state, &item).map(Value::bool_)
            }
            ValueKind::Bytes(rc) => {
                match item.kind() {
                    ValueKind::Int(n) if (0..=255).contains(&n) => Ok(Value::bool_(rc.contains(&(n as u8)))),
                    ValueKind::Bytes(sub) => Ok(Value::bool_(
                        sub.is_empty() || rc.windows(sub.len()).any(|w| w == sub.as_ref().as_slice())
                    )),
                    _ => Err(PyError::named(
                        "TypeError",
                        "a bytes-like object is required as left operand of 'in <bytes>'".to_string(),
                    )),
                }
            }
            ValueKind::Str(s) => {
                match item.kind() {
                    ValueKind::Str(sub) => Ok(Value::bool_(s.contains(sub))),
                    _ => Err(PyError::Runtime("'in <string>' requires string as left operand".to_string())),
                }
            }
            ValueKind::Dict(_) => unreachable!("handled above"),
            ValueKind::Range { start, stop, step } => {
                match item.kind() {
                    ValueKind::Int(v) => {
                        let in_range = if step > 0 {
                            v >= start && v < stop && (v - start) % step == 0
                        } else if step < 0 {
                            v <= start && v > stop && (v - start) % step == 0
                        } else { false };
                        Ok(Value::bool_(in_range))
                    }
                    _ => Ok(Value::bool_(false)),
                }
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__contains__") {
                    let result = invoke_class_method(
                        self,
                        method_val,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[ExpandedCallArg {
                            name: None,
                            value: item.clone(),
                        }],
                    )?;
                    return Ok(Value::bool_(result.truthy()));
                }
                // No __contains__: fall back to __iter__ if available.
                if let Some(iter_method) = lookup_class_attr(&class, "__iter__") {
                    let iter_obj = invoke_class_method(
                        self,
                        iter_method,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[],
                    )?;
                    loop {
                        match self.call_next(iter_obj.clone(), None) {
                            Ok(elem) => {
                                if elem == item {
                                    return Ok(Value::bool_(true));
                                }
                            }
                            Err(ref e) if e.class_name_is("StopIteration") => {
                                return Ok(Value::bool_(false));
                            }
                            // When __next__ raises `StopIteration` as a real
                            // PyInstance exception (e.g. `raise StopIteration()`
                            // from user code), call_next returns
                            // `PyError::Raised(exc)` rather than `Named`.
                            // Catch it here so the `in` operator returns False
                            // instead of propagating the exception.
                            Err(PyError::Raised(exc)) => {
                                let is_stop = matches!(exc.kind(),
                                    ValueKind::PyInstance(i) if i.borrow().class.borrow().name == "StopIteration"
                                );
                                if is_stop {
                                    return Ok(Value::bool_(false));
                                }
                                return Err(PyError::Raised(exc));
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
                // Legacy sequence-iter protocol (#394): if the class
                // defines `__getitem__` but no `__iter__`/`__contains__`,
                // walk indices 0, 1, … until IndexError/StopIteration.
                // **Short-circuits** on first match (#416 Copilot
                // review): the lazy iterator stops calling
                // `__getitem__` past the matching index, so a later
                // index raising `RuntimeError` doesn't surface.
                if lookup_class_attr(&class, "__getitem__").is_some() {
                    let iter_val = self.make_getitem_iter(Rc::clone(&inst_rc))?;
                    loop {
                        match self.call_next(iter_val.clone(), None) {
                            Ok(elem) => {
                                if elem == item {
                                    return Ok(Value::bool_(true));
                                }
                            }
                            Err(ref e) if e.class_name_is("StopIteration") => {
                                return Ok(Value::bool_(false));
                            }
                            Err(PyError::Raised(exc)) => {
                                let is_stop = matches!(exc.kind(),
                                    ValueKind::PyInstance(i) if i.borrow().class.borrow().name == "StopIteration"
                                );
                                if is_stop {
                                    return Ok(Value::bool_(false));
                                }
                                return Err(PyError::Raised(exc));
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
                Err(PyError::named(
                    "TypeError",
                    format!("argument of type '{}' is not iterable", class.borrow().name),
                ))
            }
            _ => Err(PyError::Runtime("argument of type is not iterable".to_string())),
        }
    }

}

fn is_not_implemented(v: &Value) -> bool {
    matches!(v.kind(), ValueKind::NotImplemented)
}

/// Does a class-attribute value look like a callable method?  Accepts
/// both pure-Python user functions and the `BuiltinFunction` entries
/// that `pyrust_module!`'s `class { … }` block produces — anything
/// else (descriptor, raw int set via `Foo.x = 1`, …) should fall
/// through dunder dispatch without being invoked.  Issue #331 added
/// `BuiltinFunction` to the accepted set so Counter's `__add__`
/// participates in the binary-op path.
fn is_callable_method(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
    )
}

fn coerce_numeric(v: Value) -> Value {
    // Extract via kind() in a scope so the borrow is dropped before we
    // return `v` from the fallthrough — #450 made `kind()`'s borrow
    // explicit, so we can't move `v` while a `ValueKind::List(_)` (or
    // any other Ref-bearing variant) might be live.
    if let ValueKind::Bool(b) = v.kind() {
        return Value::int(b as i64);
    }
    v
}

pub(crate) fn iter_values(value: Value) -> Result<Vec<Value>> {
    match value.kind() {
        ValueKind::List(items) => Ok(items.to_vec()),
        ValueKind::Tuple(items) => Ok(items.to_vec()),
        ValueKind::Set(items) => Ok(items.iter().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::BuiltinObject { .. } => {
            // Frozensets materialise through their inner key set; dict views
            // materialise through their backing IndexMap; everything else
            // iterates via `iter_next`.
            if let Some(rc) = pyrust_builtins::frozenset::as_items(&value) {
                return Ok(rc.iter().map(|k| key_to_value(k.clone())).collect());
            }
            if let Some(kind) = pyrust_builtins::dict_views::view_kind(&value) {
                // `view_kind` and `as_dict_rc` both check the same ops, so
                // they should agree — but use a structured error rather than
                // unwrap to avoid panicking if a future BuiltinObject impl
                // shares the dict-view type name without the matching state.
                // Surface as TypeError so Python-level `except` blocks can
                // catch it (the only way to reach this is a misregistered
                // ops table, which is a type-mismatch error).
                let rc = pyrust_builtins::dict_views::as_dict_rc(&value).ok_or_else(|| {
                    PyError::named(
                        "TypeError",
                        "dict-view state type mismatch".to_string(),
                    )
                })?;
                let map = rc.borrow();
                return Ok(match kind {
                    0 => map.keys().map(|k| key_to_value(k.clone())).collect(),
                    1 => map.values().cloned().collect(),
                    _ => map
                        .iter()
                        .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                        .collect(),
                });
            }
            if let Some(class_rc) = pyrust_builtins::mapping_proxy::as_class_rc(&value) {
                let class = class_rc.borrow();
                return Ok(class
                    .attrs
                    .keys()
                    .map(|k| Value::string(k.clone()))
                    .collect());
            }
            let mut out = Vec::new();
            let ValueKind::BuiltinObject { ops, state } = value.kind() else {
                unreachable!();
            };
            if !ops.is_iterable() {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{}' object is not iterable", ops.type_name()),
                ));
            }
            while let Some(v) = ops.iter_next(state)? {
                out.push(v);
            }
            Ok(out)
        }
        ValueKind::Bytes(rc) => Ok(rc.iter().map(|b| Value::int(*b as i64)).collect()),
        ValueKind::Str(text) => Ok(text.chars().map(|c| Value::string(c.to_string())).collect()),
        ValueKind::Dict(items) => Ok(items.keys().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::Range { start, stop, step } => {
            let mut out = Vec::new();
            if step > 0 {
                let mut cur = start;
                while cur < stop {
                    out.push(Value::int(cur));
                    cur += step;
                }
            } else {
                let mut cur = start;
                while cur > stop {
                    out.push(Value::int(cur));
                    cur += step;
                }
            }
            Ok(out)
        }
        ValueKind::Generator(state_rc) => {
            // Drain a NativeIterFrame (created by iter() on builtins) into a Vec.
            let mut borrow = state_rc.borrow_mut();
            if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                let remaining = native.items[native.pos..].to_vec();
                native.pos = native.items.len();
                Ok(remaining)
            } else {
                Err(PyError::named(
                    "TypeError",
                    "object is not iterable".to_string(),
                ))
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("'{}' object is not iterable", value_type_name_str(&value)),
        )),
    }
}

/// Resolve a built-in name to its `Value::builtin_function` variant.
/// Single source of truth for both the tree-walker (`eval_expr`) and the register VM
/// (`LoadGlobal` fallback).  Any new built-in must be added here only.
///
/// The 11 primitive type names (`int`, `str`, `list`, …) resolve to the
/// per-thread `PyClass` singletons from `primitive_class_by_name` — see
/// issue #462.  `bool` resolves to a class whose `base` chains to `int`,
/// so `bool.__bases__ == (int,)` matches CPython.
pub(crate) fn resolve_builtin(name: &str) -> Option<Value> {
    // One combined match so non-primitive names (`range`, `print`, …) skip
    // the `PRIMITIVE_CLASSES` thread_local entirely.  Primitive names
    // (#462) trigger the TLS access lazily and wrap the singleton `Rc` in
    // a `Value::py_class`.
    match name {
        "bool" | "bytes" | "complex" | "dict" | "float" | "frozenset" | "int" | "list" | "set"
        | "str" | "tuple" => primitive_class_by_name(name).map(Value::py_class),
        "print" => Some(Value::builtin_function("print")),
        "len" => Some(Value::builtin_function("len")),
        "range" => Some(Value::builtin_function("range")),
        "enumerate" => Some(Value::builtin_function("enumerate")),
        "zip" => Some(Value::builtin_function("zip")),
        "reversed" => Some(Value::builtin_function("reversed")),
        "sorted" => Some(Value::builtin_function("sorted")),
        "abs" => Some(Value::builtin_function("abs")),
        "min" => Some(Value::builtin_function("min")),
        "max" => Some(Value::builtin_function("max")),
        "sum" => Some(Value::builtin_function("sum")),
        "isinstance" => Some(Value::builtin_function("isinstance")),
        "type" => Some(Value::builtin_function("type")),
        "id" => Some(Value::builtin_function("id")),
        "hasattr" => Some(Value::builtin_function("hasattr")),
        "getattr" => Some(Value::builtin_function("getattr")),
        "setattr" => Some(Value::builtin_function("setattr")),
        "__vcall__" => Some(Value::builtin_function("__vcall__")),
        "repr" => Some(Value::builtin_function("repr")),
        "ascii" => Some(Value::builtin_function("ascii")),
        "format" => Some(Value::builtin_function("format")),
        "any" => Some(Value::builtin_function("any")),
        "all" => Some(Value::builtin_function("all")),
        "map" => Some(Value::builtin_function("map")),
        "filter" => Some(Value::builtin_function("filter")),
        "callable" => Some(Value::builtin_function("callable")),
        "round" => Some(Value::builtin_function("round")),
        "divmod" => Some(Value::builtin_function("divmod")),
        "pow" => Some(Value::builtin_function("pow")),
        "hash" => Some(Value::builtin_function("hash")),
        "chr" => Some(Value::builtin_function("chr")),
        "ord" => Some(Value::builtin_function("ord")),
        "bin" => Some(Value::builtin_function("bin")),
        "oct" => Some(Value::builtin_function("oct")),
        "hex" => Some(Value::builtin_function("hex")),
        "issubclass" => Some(Value::builtin_function("issubclass")),
        "delattr" => Some(Value::builtin_function("delattr")),
        "classmethod" => Some(Value::builtin_function("classmethod")),
        "staticmethod" => Some(Value::builtin_function("staticmethod")),
        "property" => Some(Value::builtin_function("property")),
        "super" => Some(Value::builtin_function("super")),
        "next" => Some(Value::builtin_function("next")),
        "iter" => Some(Value::builtin_function("iter")),
        "vars" => Some(Value::builtin_function("vars")),
        "dir" => Some(Value::builtin_function("dir")),
        "globals" => Some(Value::builtin_function("globals")),
        "locals" => Some(Value::builtin_function("locals")),
        "open" => Some(Value::builtin_function("open")),
        "NotImplemented" => Some(Value::not_implemented()),
        _ => None,
    }
}

/// Operation tag for set/frozenset binary operators.
#[derive(Clone, Copy)]
enum SetOp {
    Or,  // union
    And, // intersection
    Sub, // difference
    Xor, // symmetric difference
}

/// Compute a binary set operation when both operands are set/frozenset.
/// Returns the result wrapped in `Set` if both operands are sets, otherwise
/// `FrozenSet` (matching CPython: any frozenset operand promotes the result).
fn set_binary_op(left: &Value, right: &Value, op: SetOp) -> Option<Result<Value>> {
    let lhs_items = match left.kind() {
        ValueKind::Set(s) => Some((s.clone(), false)),
        _ => pyrust_builtins::frozenset::as_items(left).map(|rc| ((*rc).clone(), true)),
    }?;
    let rhs_items = match right.kind() {
        ValueKind::Set(s) => Some((s.clone(), false)),
        _ => pyrust_builtins::frozenset::as_items(right).map(|rc| ((*rc).clone(), true)),
    }?;
    let (a, l_frozen) = lhs_items;
    let (b, r_frozen) = rhs_items;
    let mut out = indexmap::IndexSet::new();
    match op {
        SetOp::Or => {
            for k in a.iter().chain(b.iter()) {
                out.insert(k.clone());
            }
        }
        SetOp::And => {
            for k in a.iter() {
                if b.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
        SetOp::Sub => {
            for k in a.iter() {
                if !b.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
        SetOp::Xor => {
            for k in a.iter() {
                if !b.contains(k) {
                    out.insert(k.clone());
                }
            }
            for k in b.iter() {
                if !a.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
    }
    Some(Ok(if l_frozen || r_frozen {
        pyrust_builtins::frozenset::frozenset(out)
    } else {
        Value::set(out)
    }))
}

/// Coerce a numeric value to a `(real, imag)` pair if possible.
///
/// Returns `Ok(Some(...))` on success, `Ok(None)` when the value is not a
/// numeric type that participates in complex arithmetic, and `Err(...)` when
/// the value is a `BigInt` that is too large to convert to `f64` (matching
/// CPython 3.12's `OverflowError: int too large to convert to float`).
fn as_complex_pair(v: &Value) -> Result<Option<(f64, f64)>> {
    match v.kind() {
        ValueKind::Complex(re, im) => Ok(Some((re, im))),
        ValueKind::Int(n) => Ok(Some((n as f64, 0.0))),
        ValueKind::Float(f) => Ok(Some((f, 0.0))),
        ValueKind::Bool(b) => Ok(Some((if b { 1.0 } else { 0.0 }, 0.0))),
        ValueKind::BigInt(b) => Ok(Some((bigint_to_float_or_overflow(b)?, 0.0))),
        _ => Ok(None),
    }
}

/// Returns the two operands as complex `(re, im)` pairs only when AT LEAST
/// one of them is already a complex number — that way pure int/float
/// arithmetic continues to use the dedicated fast paths.
///
/// Returns `Ok(None)` when neither operand is complex or when one operand is
/// not a numeric type.  Returns `Err(...)` when a `BigInt` operand overflows
/// `f64` (propagated as `OverflowError`).
fn both_as_complex(
    left: &Value,
    right: &Value,
) -> Result<Option<((f64, f64), (f64, f64))>> {
    let l_is_c = matches!(left.kind(), ValueKind::Complex(_, _));
    let r_is_c = matches!(right.kind(), ValueKind::Complex(_, _));
    if !l_is_c && !r_is_c {
        return Ok(None);
    }
    let Some(a) = as_complex_pair(left)? else {
        return Ok(None);
    };
    let Some(b) = as_complex_pair(right)? else {
        return Ok(None);
    };
    Ok(Some((a, b)))
}

/// Compute complex exponentiation `(zr + zi*j) ** (wr + wi*j)` with
/// CPython 3.12 parity.
///
/// Mirrors CPython's `_Py_c_pow` from `Objects/complexobject.c`:
///   - For small non-negative integer exponents (`wi == 0`, `wr` is an
///     integer in `0..=100`), use repeated squaring so that results like
///     `(1+1j)**2 == 2j` are exact (no floating-point rounding in the
///     imaginary part).
///   - General case uses `r = |z|` (hypot), `ln_r = ln(r)`, `t = arg(z)`:
///     `len = pow(r, wr) * exp(-wi * t)`,  `at = wr*t + wi*ln_r`,
///     `result = len * (cos(at) + i*sin(at))`.
///     Using `pow(r, wr)` rather than `exp(wr*ln_r)` matches CPython's
///     rounding for cases like `(2+0j)**0.5`.
///
/// Special cases (CPython parity):
///   - `w == 0+0j` → `(1+0j)` for any `z` (including `0j ** 0`).
///   - `z == 0+0j`, `wi != 0` or `wr < 0` → `ZeroDivisionError`.
///   - `z == 0+0j`, `wr > 0`, `wi == 0` → `0j`.
fn complex_pow(zr: f64, zi: f64, wr: f64, wi: f64) -> Result<Value> {
    // z^0 = 1 for any z (including 0j ** 0).
    if wr == 0.0 && wi == 0.0 {
        return Ok(Value::complex(1.0, 0.0));
    }

    let abs_r = zr.hypot(zi); // |z| = sqrt(zr² + zi²)
    if abs_r == 0.0 {
        // 0j ** w where w != 0.
        // CPython raises ZeroDivisionError when the exponent has a nonzero
        // imaginary part or a negative real part.
        if wi != 0.0 || wr < 0.0 {
            return Err(PyError::named(
                "ZeroDivisionError",
                "0.0 to a negative or complex power".to_string(),
            ));
        }
        // wr > 0, wi == 0: 0j ** positive_real = 0j.
        return Ok(Value::complex(0.0, 0.0));
    }

    // CPython optimisation: use repeated squaring for small integer
    // exponents (wi==0, |wr| <= 100, wr == floor(wr)).
    // This avoids rounding error in the exp/log path so that, e.g.,
    // `(1+1j)**2` returns exactly `2j` rather than `(1.22e-16+2j)`.
    // Negative exponents use the same squaring on |n| and then invert:
    // `z**(-n) = 1 / z**n`.  CPython's `_Py_c_pow` applies the same
    // |wr| <= 100 bound for both positive and negative integers.
    if wi == 0.0 {
        let n = wr as i64;
        if n as f64 == wr && (-100..=100).contains(&n) {
            let (mut re, mut im) = (1.0_f64, 0.0_f64);
            let (mut br, mut bi) = (zr, zi);
            let mut exp = n.unsigned_abs(); // works for n == i64::MIN too (can't happen: |n|<=100)
            while exp > 0 {
                if exp & 1 == 1 {
                    let new_re = re * br - im * bi;
                    let new_im = re * bi + im * br;
                    re = new_re;
                    im = new_im;
                }
                let new_br = br * br - bi * bi;
                let new_bi = 2.0 * br * bi;
                br = new_br;
                bi = new_bi;
                exp >>= 1;
            }
            if n < 0 {
                // Invert: 1/(re + im*j) using the c_quot form from CPython's
                // complexobject.c so that signed-zero behaviour matches.
                // c_quot(1+0j, re+im*j):
                //   result_re = (1*re + 0*im) / (re²+im²)
                //   result_im = (0*re - 1*im) / (re²+im²)
                // Writing im as `0.0 * old_re - 1.0 * im` rather than `-im`
                // preserves positive zero when im == +0.0 (0.0*old_re yields
                // +0.0, then +0.0 - +0.0 == +0.0; direct negation of +0.0
                // yields -0.0, which diverges from CPython).
                let denom = re * re + im * im;
                let old_re = re;
                re = (1.0 * old_re + 0.0 * im) / denom;
                im = (0.0 * old_re - 1.0 * im) / denom;
            }
            return Ok(Value::complex(re, im));
        }
    }

    // General case: matches CPython's `_Py_c_pow` from complexobject.c.
    // Using pow(r, wr) rather than exp(wr * ln_r) is deliberate:
    // `exp(0.5 * ln(2))` and `pow(2.0, 0.5)` differ by 1 ULP; CPython
    // uses the `pow` path, so we must match it for parity.
    let ln_r = abs_r.ln();
    let t = zi.atan2(zr);
    let len = abs_r.powf(wr) * (-wi * t).exp();
    if len.is_infinite() {
        // CPython's _Py_c_pow sets errno = ERANGE when `len` overflows to
        // infinity and the caller raises OverflowError (e.g.
        // `(1+1j) ** 10**20` → `OverflowError: complex exponentiation`).
        return Err(PyError::named(
            "OverflowError",
            "complex exponentiation".to_string(),
        ));
    }
    let at = wr * t + wi * ln_r;
    Ok(Value::complex(len * at.cos(), len * at.sin()))
}
