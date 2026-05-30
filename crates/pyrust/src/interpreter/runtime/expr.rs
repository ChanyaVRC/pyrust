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

/// Sequence repeat helpers.  Each raises the appropriate Python error when
/// the repeat count or resulting allocation exceeds platform limits, matching
/// CPython 3.12 behaviour:
///
/// - `n <= 0`                                → empty result
/// - `BigInt` (any)                          → `OverflowError: cannot fit 'int' into an index-sized integer`
/// - `Int` and `char_count * n > isize::MAX` → `OverflowError: repeated string is too long`
/// - allocation fails (OOM)                  → `MemoryError`
fn seq_repeat_str(text: &str, n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(Value::string(String::new()));
    }
    let n = n as usize;
    // Fast path: if byte_total fits in isize::MAX then char_count * n ≤ byte_total
    // ≤ isize::MAX, so the CPython char-count overflow check cannot fire.  We only
    // pay for chars().count() when byte_total itself already approaches the limit.
    let byte_total = match text.len().checked_mul(n) {
        Some(b) => b,
        None => return Err(PyError::named("MemoryError", String::new())),
    };
    if byte_total > isize::MAX as usize {
        // Only compute char_count here; CPython raises OverflowError when
        // char_count * n > Py_ssize_t_MAX, MemoryError otherwise.
        let char_count = text.chars().count();
        if char_count.checked_mul(n).map_or(true, |t| t > isize::MAX as usize) {
            return Err(PyError::named(
                "OverflowError",
                "repeated string is too long".to_string(),
            ));
        }
        // char_count * n fits, but byte_total doesn't — OOM.
        return Err(PyError::named("MemoryError", String::new()));
    }
    // Use try_reserve to catch OOM rather than letting the allocator abort,
    // then delegate to str::repeat for its O(log n) doubling strategy.
    let mut probe = String::new();
    if probe.try_reserve(byte_total).is_err() {
        return Err(PyError::named("MemoryError", String::new()));
    }
    Ok(Value::string(text.repeat(n)))
}

fn seq_repeat_list(items: &[Value], n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(Value::list(Vec::new()));
    }
    let n = n as usize;
    let total = match items.len().checked_mul(n) {
        Some(t) => t,
        None => return Err(PyError::named("MemoryError", String::new())),
    };
    let mut out: Vec<Value> = Vec::new();
    if out.try_reserve(total).is_err() {
        return Err(PyError::named("MemoryError", String::new()));
    }
    for _ in 0..n {
        out.extend_from_slice(items);
    }
    Ok(Value::list(out))
}

fn seq_repeat_bytes(data: &[u8], n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(Value::bytes(Vec::new()));
    }
    let n = n as usize;
    let total = match data.len().checked_mul(n) {
        Some(t) => t,
        None => return Err(PyError::named("MemoryError", String::new())),
    };
    let mut out: Vec<u8> = Vec::new();
    if out.try_reserve(total).is_err() {
        return Err(PyError::named("MemoryError", String::new()));
    }
    for _ in 0..n {
        out.extend_from_slice(data);
    }
    Ok(Value::bytes(out))
}

fn seq_repeat_bytearray(data: &[u8], n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(pyrust_builtins::bytearray::bytearray(Vec::new()));
    }
    let n = n as usize;
    let total = match data.len().checked_mul(n) {
        Some(t) => t,
        None => return Err(PyError::named("MemoryError", String::new())),
    };
    let mut out: Vec<u8> = Vec::new();
    if out.try_reserve(total).is_err() {
        return Err(PyError::named("MemoryError", String::new()));
    }
    for _ in 0..n {
        out.extend_from_slice(data);
    }
    Ok(pyrust_builtins::bytearray::bytearray(out))
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
/// operand isn't an int / bool.  Call sites replace the TypeError message
/// with the operand-specific "unsupported operand type(s) for OP: 'X' and 'Y'"
/// format via `map_err`.
fn shift_count(v: &Value) -> Result<ShiftCount> {
    let big = value_to_bigint(v).ok_or_else(|| {
        // Caller replaces this message via map_err; see LShift / RShift arms.
        PyError::named("TypeError", String::new())
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

/// Repeat a tuple slice `n` times, matching CPython 3.12 `tuplerepeat`
/// semantics:
///
/// - `n <= 0` → empty tuple (no allocation).
/// - `items.len() * n > isize::MAX` → `MemoryError` (catches overflow
///   before any allocation attempt, preventing an allocator abort).
/// - allocation failure (Vec::try_reserve) → `MemoryError`.
fn seq_repeat_tuple(items: &[Value], n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(Value::tuple(Vec::new()));
    }
    let n = n as usize;
    let total = items.len().checked_mul(n).filter(|&t| t <= isize::MAX as usize);
    let total = total.ok_or_else(|| PyError::named("MemoryError", String::new()))?;
    let mut out: Vec<Value> = Vec::new();
    out.try_reserve(total)
        .map_err(|_| PyError::named("MemoryError", String::new()))?;
    for _ in 0..n {
        out.extend_from_slice(items);
    }
    Ok(Value::tuple(out))
}

impl Interpreter {
    fn unsupported_binary_operand(op: &str) -> PyError {
        PyError::named("TypeError", format!("unsupported operand type(s) for {op}"))
    }
    pub(crate) fn eval_index(&mut self, target: &Value, index: Value) -> Result<Value> {
        // If the index is a `slice` object (built by `eval_slice` and passed
        // into a `__getitem__` call, which then subscripts a built-in sequence
        // with it), extract the bounds and delegate to `eval_slice` so that
        // `self.data[slice_arg]` inside a `__getitem__` works correctly.
        //
        // Dicts and BuiltinObjects are excluded: they may accept slice objects
        // as legitimate hashable keys (e.g. `d = {}; d[slice(1,3)] = "a"`).
        // Only sequence-like targets (List, Tuple, Str, Bytes, PyInstance) need
        // the redirect.
        let target_is_sequence_like = matches!(
            target.kind(),
            ValueKind::List(_)
                | ValueKind::Tuple(_)
                | ValueKind::Str(_)
                | ValueKind::Bytes(_)
                | ValueKind::PyInstance(_)
        );
        if target_is_sequence_like {
            if let ValueKind::BuiltinObject { ops, state } = index.kind()
                && ops.type_name() == pyrust_builtins::slice::TYPE_NAME
            {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<pyrust_builtins::slice::SliceState>()
                    .expect("SliceOps: bad state");
                let lo = if s.start.is_none() { None } else { Some(s.start.clone()) };
                let hi = if s.stop.is_none() { None } else { Some(s.stop.clone()) };
                let st = if s.step.is_none() { None } else { Some(s.step.clone()) };
                drop(borrow);
                return self.eval_slice(target, lo, hi, st);
            }
        }
        // Handle Dict separately so the temporary `&IndexMap` from
        // `target.kind()` doesn't outlive the call into `dict_lookup`
        // (which may run user `__eq__` that mutates the dict — see the
        // aliasing notes on `Value::as_dict_mut`).
        if target.as_dict().is_some() {
            // Fast path for string keys (issue #506): probe via `StrKey` to
            // skip constructing a `PyKey::Str(Value)` (which bumps the RC).
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
        // Resolve the __index__ protocol for sequence targets before the borrow
        // from target.kind() is held across the match arms (which call &mut self
        // helpers that cannot coexist with an active kind() borrow).
        let seq_label: Option<&'static str> = match target.kind() {
            ValueKind::List(_) => Some("list"),
            ValueKind::Tuple(_) => Some("tuple"),
            ValueKind::Str(_) => Some("string"),
            ValueKind::Bytes(_) => Some("bytes"),
            ValueKind::Range { .. } => Some("range"),
            _ => None,
        };
        let index = if let Some(label) = seq_label {
            self.call_index_protocol(&index, label)?
        } else {
            index
        };
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
                let char_count = text.chars().count();
                let idx = normalize_index(&index, char_count, "string")?;
                // Use nth() to avoid collecting a Vec<char>; normalize_index
                // guarantees idx < char_count so unwrap is safe.
                let ch = text.chars().nth(idx).expect("normalize_index bounds check");
                // Stack-encode to a &str to avoid an intermediate String allocation.
                let mut buf = [0u8; 4];
                Ok(Value::string(ch.encode_utf8(&mut buf) as &str))
            }
            ValueKind::Bytes(rc) => {
                let idx = normalize_index(&index, rc.len(), "bytes")?;
                Ok(Value::int(rc[idx] as i64))
            }
            ValueKind::Range { start, stop, step } => {
                let len = range_len(start, stop, step);
                // call_index_protocol (via seq_label) has already resolved any
                // __index__ on the subscript; the value is now Int/Bool/BigInt.
                // Cannot use normalize_index because its error message is
                // "range index out of range", but CPython says
                // "range object index out of range".
                let mut i = match index.kind() {
                    ValueKind::Int(v) => v,
                    ValueKind::Bool(b) => b as i64,
                    // BigInt is a valid integer but will always be out of range
                    // for any realistic range length.
                    ValueKind::BigInt(_) => {
                        return Err(PyError::named("IndexError", "range object index out of range".to_string()));
                    }
                    _ => unreachable!("call_index_protocol guarantees an integer"),
                };
                if i < 0 {
                    i += len;
                }
                if i < 0 || i >= len {
                    return Err(PyError::named("IndexError", "range object index out of range".to_string()));
                }
                Ok(Value::int(start + i * step))
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
            ValueKind::PyClass(class_rc) => {
                let class = Rc::clone(class_rc);
                // Look up `__class_getitem__` in the class's own attrs (not
                // the MRO).  Built-in collection types have a
                // `BuiltinFunction("<type>.__class_getitem__")` sentinel
                // registered by `build_primitive_classes`.  User-defined
                // classes may define it as a classmethod.  Classes without
                // it raise TypeError (matching CPython 3.12).
                let cgitem = class.borrow().attrs.get("__class_getitem__").cloned();
                if let Some(method_val) = cgitem {
                    // Distinguish between the built-in sentinel and a
                    // user-defined classmethod.
                    let is_builtin_sentinel = matches!(
                        method_val.kind(),
                        ValueKind::BuiltinFunction(name)
                            if name.contains(".__class_getitem__")
                    );
                    if is_builtin_sentinel {
                        // Built-in sentinel: create a `GenericAlias` directly.
                        // Normalise the subscript into a tuple for
                        // `GenericAlias.__args__`:
                        //   `list[int]`       → args = (int,)
                        //   `dict[str, int]`  → args = (str, int) [tuple index]
                        let is_tuple = matches!(index.kind(), ValueKind::Tuple(_));
                        let type_args = if is_tuple {
                            index
                        } else {
                            Value::tuple(vec![index])
                        };
                        Ok(pyrust_builtins::generic_alias::generic_alias(
                            Value::py_class(class),
                            type_args,
                        ))
                    } else {
                        // User-defined `__class_getitem__` (typically a
                        // classmethod): call it with the class as the
                        // implicit receiver and the subscript as the arg.
                        let class_val = Value::py_class(class);
                        invoke_class_method(
                            self,
                            method_val,
                            class_val,
                            &[ExpandedCallArg {
                                name: None,
                                value: index,
                            }],
                        )
                    }
                } else {
                    Err(PyError::named(
                        "TypeError",
                        format!("type '{}' is not subscriptable", class.borrow().name),
                    ))
                }
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                // Issue #1134: check for a user-defined __getitem__ on the
                // class *before* falling back to the backing primitive fast
                // path.  A dict subclass that overrides __getitem__ must have
                // the override called, not the raw backing-dict lookup.
                // The BuiltinFunction sentinel `dict.__getitem__` registered
                // on the dict base class itself is excluded — it is the base
                // implementation, not an override.  Any other __getitem__
                // (UserFunction from user code, or BuiltinFunction from a
                // builtin class like Counter) is treated as an override.
                let user_getitem = lookup_class_attr(&class, "__getitem__").filter(|v| {
                    !matches!(
                        v.kind(),
                        ValueKind::BuiltinFunction(
                            "dict.__getitem__"
                                | "list.__getitem__"
                                | "tuple.__getitem__"
                                | "bytes.__getitem__"
                        )
                    )
                });
                if let Some(method_val) = user_getitem {
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
                // No user __getitem__: delegate to the backing primitive when
                // present.  For dict backing, also honour __missing__ on a
                // missing key (issue #1134).
                if let Some(backing) = instance_builtin_data(&inst_rc) {
                    if backing.as_dict().is_some() {
                        let lookup = if let Some(s) = index.as_str() {
                            self.dict_str_lookup(&backing, s)?
                        } else {
                            let key = self.value_to_pykey(&index)?;
                            self.dict_lookup(&backing, &key)?
                        };
                        return match lookup {
                            Some((_, v)) => Ok(v),
                            None => {
                                if let Some(missing_fn) =
                                    lookup_class_attr(&class, "__missing__")
                                {
                                    invoke_class_method(
                                        self,
                                        missing_fn,
                                        Value::py_instance(inst_rc),
                                        &[ExpandedCallArg {
                                            name: None,
                                            value: index,
                                        }],
                                    )
                                } else {
                                    Err(PyError::key_error(index))
                                }
                            }
                        };
                    }
                    return self.eval_index(&backing, index);
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
                    pyrust_core::builtin_type_name(target)
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
        // Subtype priority (mirrors CPython `binary_op1`): when `rmethod` is a
        // reflected arithmetic slot (e.g. `__radd__`, starts with `__r`) and
        // `right`'s class is a *proper* subtype of `left`'s class AND `right`'s
        // resolved `rmethod` slot (via MRO) differs from `left`'s resolved slot
        // (one is None, or they're different functions), try
        // `right.rmethod(left)` before `left.method(right)`.  This mirrors
        // CPython's `slotw != slotv` check in `binary_op1`: a right type that
        // inherits a different `__radd__` from an intermediate class gets
        // priority, not only types that directly define `rmethod` in their own
        // `__dict__`.  Comparison reflected ops (`__gt__`, `__ge__`, …) do not
        // start with `__r`, so they are unaffected by this check.
        let right_has_subtype_priority = rmethod.starts_with("__r") && {
            if let (ValueKind::PyInstance(li), ValueKind::PyInstance(ri)) =
                (left.kind(), right.kind())
            {
                let lc = Rc::clone(&li.borrow().class);
                let rc_class = Rc::clone(&ri.borrow().class);
                if !Rc::ptr_eq(&lc, &rc_class) && class_is_subclass_of(&rc_class, &lc) {
                    let right_slot = lookup_class_attr(&rc_class, rmethod);
                    let left_slot = lookup_class_attr(&lc, rmethod);
                    right_slot.is_some() && right_slot != left_slot
                } else {
                    false
                }
            } else {
                false
            }
        };

        if right_has_subtype_priority {
            if let ValueKind::PyInstance(inst) = right.kind() {
                let class = Rc::clone(&inst.borrow().class);
                if let Some(m) = lookup_class_attr(&class, rmethod)
                    && is_callable_method(&m)
                {
                    // BuiltinFunction dunders (e.g. `int.__radd__`) operate on
                    // the backing primitive value.  Pass the coerced value so
                    // `eval_binary` inside the dunder doesn't re-dispatch to the
                    // same method on the still-wrapped PyInstance (infinite loop).
                    let self_val = if matches!(m.kind(), ValueKind::BuiltinFunction(_)) {
                        coerce_numeric(right)
                    } else {
                        Value::py_instance(Rc::clone(inst))
                    };
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
        }

        if let ValueKind::PyInstance(inst) = left.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, method)
                && is_callable_method(&m)
            {
                let self_val = if matches!(m.kind(), ValueKind::BuiltinFunction(_)) {
                    coerce_numeric(left)
                } else {
                    Value::py_instance(Rc::clone(inst))
                };
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

        if !right_has_subtype_priority {
            if let ValueKind::PyInstance(inst) = right.kind() {
                let class = Rc::clone(&inst.borrow().class);
                if let Some(m) = lookup_class_attr(&class, rmethod)
                    && is_callable_method(&m)
                {
                    let self_val = if matches!(m.kind(), ValueKind::BuiltinFunction(_)) {
                        coerce_numeric(right)
                    } else {
                        Value::py_instance(Rc::clone(inst))
                    };
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
                // BuiltinFunction dunders operate on the backing primitive value;
                // pass the coerced value so they don't reject the PyInstance wrapper.
                let self_val = if matches!(m.kind(), ValueKind::BuiltinFunction(_)) {
                    coerce_numeric(val)
                } else {
                    Value::py_instance(Rc::clone(inst))
                };
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
        // Slices with PyInstance components need interpreter access to dispatch
        // `__hash__`.  The pure `SliceOps::to_key()` path (via `value.to_key()`)
        // returns `None` for any instance component, producing a misleading
        // "unhashable type: 'slice'" error.  Intercept here when any component
        // is a PyInstance and compute the hash via `hash_value_with_interp`,
        // then store it in a `PyKey::Object` consistent with what `hash()`
        // returns for the same slice (issue #850).
        //
        // When a component is a plain unhashable primitive (list, dict, set),
        // `SliceOps::to_key()` also returns `None` but the fall-through error
        // at the end of this function would blame `'slice'` rather than the
        // actual offending component.  Detect that case here too and surface
        // the correct type name (issue #893).
        if let ValueKind::BuiltinObject { ops, state } = value.kind() {
            if ops.type_name() == pyrust_builtins::slice::TYPE_NAME {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<pyrust_builtins::slice::SliceState>()
                    .expect("SliceOps: bad state");
                let needs_interp =
                    crate::builtin_modules::builtins::value_needs_interp(&s.start)
                        || crate::builtin_modules::builtins::value_needs_interp(&s.stop)
                        || crate::builtin_modules::builtins::value_needs_interp(&s.step);
                // Check whether any component is an unhashable primitive so we
                // can name it precisely in the error rather than blaming 'slice'.
                // Use recursive descent so that a tuple-inside-slice (or
                // further nesting) names the leaf type, matching CPython.
                let unhashable_component: Option<String> = if !needs_interp {
                    [&s.start, &s.stop, &s.step].iter().find_map(|c| {
                        if c.to_key().is_none() {
                            Some(pyrust_builtins::set::leaf_unhashable_type_name(c))
                        } else {
                            None
                        }
                    })
                } else {
                    None
                };
                drop(borrow);
                if let Some(component_name) = unhashable_component {
                    return Err(PyError::named(
                        "TypeError",
                        format!("unhashable type: '{component_name}'"),
                    ));
                }
                // All slices (instance or primitive components) go through
                // hash_value_with_interp to get the CPython-compatible slice hash
                // and to dispatch user __hash__ on PyInstance components.
                let hash =
                    crate::builtin_modules::builtins::hash_value_with_interp(self, value)? as u64;
                return Ok(PyKey::Object {
                    hash,
                    value: value.clone(),
                });
            }
        }
        if let Some(k) = value.to_key() {
            return Ok(k);
        }
        // Range objects are hashable (issue #937).  `Value::to_key` returns
        // `None` for ranges (they have no `PyKey` variant), so we handle them
        // here: compute the hash via `hash_value_with_interp` (which calls the
        // `ValueKind::Range` arm in `hash_value`) and store it in `PyKey::Object`
        // so that `range == range` lookup uses `Value`'s `PartialEq`.
        if matches!(value.kind(), ValueKind::Range { .. }) {
            let hash =
                crate::builtin_modules::builtins::hash_value_with_interp(self, value)? as u64;
            return Ok(PyKey::Object {
                hash,
                value: value.clone(),
            });
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
        // Class objects are hashable by identity (CPython: type.__hash__).
        // Both user-defined classes and built-in primitive classes (`int`,
        // `str`, etc.) are `ValueKind::PyClass`, so this arm covers all of
        // them.  The hash is the Rc pointer, matching the `id()` value and
        // giving stable, unique hashes for distinct class objects.
        if let ValueKind::PyClass(class_rc) = value.kind() {
            let ptr = Rc::as_ptr(class_rc) as usize as u64;
            return Ok(PyKey::Object {
                hash: ptr,
                value: value.clone(),
            });
        }
        // User-defined functions, lambdas, and built-in functions are hashable
        // by identity (CPython: function.__hash__).  Use the Rc pointer for user
        // functions and the static name pointer for built-in functions, matching
        // the hash computed by hash_value for the same values.
        if let ValueKind::UserFunction(rc) = value.kind() {
            let ptr = Rc::as_ptr(rc) as usize as u64;
            return Ok(PyKey::Object {
                hash: ptr,
                value: value.clone(),
            });
        }
        if let ValueKind::BuiltinFunction(name) = value.kind() {
            let ptr = name.as_ptr() as usize as u64;
            return Ok(PyKey::Object {
                hash: ptr,
                value: value.clone(),
            });
        }
        // Bound methods: hash as hash(func) ^ hash(self), using Rc pointer
        // identity for both components, matching CPython method.__hash__.
        if let ValueKind::BoundMethod { function, receiver } = value.kind() {
            let func_ptr = Rc::as_ptr(function) as usize as u64;
            let recv_ptr = Rc::as_ptr(receiver) as usize as u64;
            let h = func_ptr ^ recv_ptr;
            return Ok(PyKey::Object {
                hash: h,
                value: value.clone(),
            });
        }
        // Class-bound methods (classmethods): same XOR pattern using the class
        // Rc pointer instead of an instance pointer.
        if let ValueKind::ClassBoundMethod { function, class } = value.kind() {
            let func_ptr = Rc::as_ptr(function) as usize as u64;
            let class_ptr = Rc::as_ptr(class) as usize as u64;
            let h = func_ptr ^ class_ptr;
            return Ok(PyKey::Object {
                hash: h,
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
        // Slow path — `Object` keys (and cross-variant None/Object matching,
        // issue #906).  Extract candidates under a narrow borrow, then drop
        // the borrow before user `__eq__` runs.
        //
        // Fast pre-check: skip the Vec allocation entirely when no entry
        // with the same hash exists (the common case for non-adversarial inputs).
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let has_candidate = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                dict.keys().any(|k| match k {
                    PyKey::Object { hash, .. } => hash == target_hash,
                    PyKey::None => *target_hash == none_hash,
                    _ => false,
                })
            };
            if has_candidate {
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
                            // PyKey::None has Python-level hash py_hash_none().  When
                            // the Object key hashes to the same value, check whether
                            // __eq__ considers them equal (issue #906).
                            PyKey::None if *target_hash == none_hash => {
                                Some((i, Value::none(), v.clone()))
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
        }
        // Cross-variant slow path: lookup key is PyKey::None but a stored
        // PyKey::Object with hash py_hash_none() may __eq__-match None (issue #906).
        // Fast pre-check (issue #934): skip the full scan if the dict has no
        // Object entries with hash == py_hash_none().  The common case exits here
        // without building a candidates Vec.
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let has_cross_variant = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                dict.keys()
                    .any(|k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash))
            };
            if has_cross_variant {
                let candidates: Vec<(usize, Value, Value)> = {
                    let dict = receiver
                        .as_dict()
                        .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                    dict.iter()
                        .enumerate()
                        .filter_map(|(i, (k, v))| match k {
                            PyKey::Object { hash, value }
                                if *hash == none_hash =>
                            {
                                Some((i, value.clone(), v.clone()))
                            }
                            _ => None,
                        })
                        .collect()
                };
                let none_val = Value::none();
                for (idx, candidate_key, value) in candidates {
                    if self.values_user_eq(&none_val, &candidate_key)? {
                        return Ok(Some((idx, value)));
                    }
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
                    // Cross-variant: PyKey::None has Python-level hash py_hash_none();
                    // include it as a candidate when the Object also hashes to that
                    // value so that __eq__ can confirm the match (issue #906).
                    PyKey::None if *target_hash == (pyrust_core::py_hash_none() as u64) => {
                        Some((i, Value::none(), v.clone()))
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
        // Cross-variant slow path: None key vs Object entries with hash py_hash_none()
        // (issue #906).  Fast pre-check (issue #934): skip the full scan if no
        // Object entry with hash == py_hash_none() exists (common case, no alloc).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            if dict
                .keys()
                .any(|k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash))
            {
                let candidates: Vec<(usize, Value, Value)> = dict
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (k, v))| match k {
                        PyKey::Object { hash, value } if *hash == none_hash => {
                            Some((i, value.clone(), v.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                let none_val = Value::none();
                for (idx, candidate_key, value) in candidates {
                    if self.values_user_eq(&none_val, &candidate_key)? {
                        return Ok(Some((idx, value)));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Zero-allocation string key lookup in a dict receiver (issue #506).
    ///
    /// Probes the `IndexMap<PyKey, Value>` using `StrKey`, which hashes
    /// identically to `PyKey::Str` without constructing a `PyKey` (zero RC
    /// bump, zero allocation).  Use this in place of
    /// `dict_lookup(&PyKey::str_from(s))` whenever the lookup key is already
    /// a `&str`.  The `PyKey::Object` slow path is omitted: a `&str` can
    /// never match an `Object` key.
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
        // Fast pre-check for the Object-key slow path: skip the Vec allocation
        // when no entry with the same hash exists (the common case).
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let has_candidate = {
                let set = receiver
                    .as_set()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                set.iter().any(|k| match k {
                    PyKey::Object { hash, .. } => hash == target_hash,
                    PyKey::None => *target_hash == none_hash,
                    _ => false,
                })
            };
            if has_candidate {
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
                            // PyKey::None has Python-level hash py_hash_none(); include it
                            // as a candidate when the Object key hashes to the same value
                            // so that __eq__ can confirm the match (issue #906).
                            PyKey::None if *target_hash == none_hash => Some((i, Value::none())),
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
        }
        // Cross-variant slow path: None key vs Object entries with hash py_hash_none()
        // (issue #906).  Fast pre-check (issue #934): skip the full scan if no
        // Object entry with hash == py_hash_none() exists (common case, no alloc).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let has_cross_variant = {
                let set = receiver
                    .as_set()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                set.iter()
                    .any(|k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash))
            };
            if has_cross_variant {
                let candidates: Vec<(usize, Value)> = {
                    let set = receiver
                        .as_set()
                        .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                    set.iter()
                        .enumerate()
                        .filter_map(|(i, k)| match k {
                            PyKey::Object { hash, value } if *hash == none_hash => {
                                Some((i, value.clone()))
                            }
                            _ => None,
                        })
                        .collect()
                };
                let none_val = Value::none();
                for (idx, candidate) in candidates {
                    if self.values_user_eq(&none_val, &candidate)? {
                        return Ok(Some(idx));
                    }
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
                    // Cross-variant: PyKey::None has Python-level hash py_hash_none();
                    // include it as a candidate when the Object hashes to the same
                    // value (issue #906).
                    PyKey::None if *target_hash == (pyrust_core::py_hash_none() as u64) => Some((i, Value::none())),
                    _ => None,
                })
                .collect();
            for (idx, candidate) in candidates {
                if self.values_user_eq(&candidate, target)? {
                    return Ok(Some(idx));
                }
            }
        }
        // Cross-variant slow path: None key vs Object entries with hash py_hash_none()
        // (issue #906).  Fast pre-check (issue #934): skip the full scan if no
        // Object entry with hash == py_hash_none() exists (common case, no alloc).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            if set
                .iter()
                .any(|k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash))
            {
                let candidates: Vec<(usize, Value)> = set
                    .iter()
                    .enumerate()
                    .filter_map(|(i, k)| match k {
                        PyKey::Object { hash, value } if *hash == none_hash => {
                            Some((i, value.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                let none_val = Value::none();
                for (idx, candidate) in candidates {
                    if self.values_user_eq(&none_val, &candidate)? {
                        return Ok(Some(idx));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Insert `(key, value)` into a dict that lives at register/local
    /// `dict_value`, dispatching user `__eq__` to deduplicate against an
    /// existing entry when `key` is a `PyKey::Object` or `PyKey::None`.
    /// The None case handles cross-variant dedup (issue #906): inserting
    /// None into a dict that already holds an Object key with hash py_hash_none()
    /// that __eq__-matches None should overwrite the existing entry, not add a
    /// second one.
    pub(crate) fn dict_insert(
        &mut self,
        dict: &mut indexmap::IndexMap<PyKey, Value>,
        key: PyKey,
        value: Value,
    ) -> Result<()> {
        // `PyKey::Object` keys may collide with another Object entry (or with a
        // stored `PyKey::None`) and require `__eq__` dedup via `dict_lookup_in`.
        // `PyKey::None` is the cross-variant case (issue #906): a stored
        // `PyKey::Object{hash == py_hash_none()}` that `__eq__`-matches `None`
        // must be overwritten rather than creating a second entry.
        //
        // Fast path for `PyKey::None` (issue #934): IndexMap already deduplicates
        // `None == None` natively via `Hash`+`PartialEq`.  We only need the slow
        // `dict_lookup_in` path when the dict contains a `PyKey::Object` with hash
        // `py_hash_none()` — an extremely rare cross-variant scenario.  Skip the
        // entire lookup call in the common case.
        let needs_dedup = match &key {
            PyKey::Object { .. } => true,
            PyKey::None => {
                let none_hash = pyrust_core::py_hash_none() as u64;
                dict.keys()
                    .any(|k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash))
            }
            _ => false,
        };
        if needs_dedup {
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
    /// Handles both `Object` keys and `None` keys for cross-variant dedup
    /// (issue #906): inserting None into a set that already holds an Object
    /// with hash py_hash_none() that __eq__-matches None must not create a
    /// duplicate.
    pub(crate) fn set_insert(
        &mut self,
        set: &mut indexmap::IndexSet<PyKey>,
        key: PyKey,
    ) -> Result<()> {
        // Same fast pre-check as `dict_insert` (issue #934): for `PyKey::None`,
        // only call `set_lookup_in` when the set contains a `PyKey::Object` with
        // hash `py_hash_none()` (rare cross-variant case, issue #906).
        let needs_dedup = match &key {
            PyKey::Object { .. } => true,
            PyKey::None => {
                let none_hash = pyrust_core::py_hash_none() as u64;
                set.iter()
                    .any(|k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash))
            }
            _ => false,
        };
        if needs_dedup && self.set_lookup_in(set, &key)?.is_some() {
            return Ok(());
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
        // Issue #1204: if a PyInstance has a scalar primitive backing
        // (e.g. MyInt subclass) and no user __eq__ was found, compare the
        // backing values so `MyInt(5) == 5` returns True.
        let a_cmp = coerce_numeric(a);
        let b_cmp = coerce_numeric(b);
        if !matches!(a_cmp.kind(), ValueKind::PyInstance(_))
            || !matches!(b_cmp.kind(), ValueKind::PyInstance(_))
        {
            // At least one side was coerced out of PyInstance.
            return Ok(a_cmp == b_cmp);
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
        kwargs: &IndexMap<PyKey, Value>,
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
            // `update` with non-primitive iterables (range, generators,
            // user-defined iterables) — the builtins crate has no interpreter
            // access and falls to its `_` arm raising "'X' object is not
            // iterable" for these types.  Intercept here when the positional
            // arg is not one of the five primitive types the builtins crate
            // already handles (Dict/List/Tuple/Str/Bytes).  Delegate for those
            // types to preserve existing behaviour (including the self-alias
            // snapshot logic in snapshot_update_arg).
            "update" => {
                if args.len() > 1 {
                    return Err(PyError::named(
                        "TypeError",
                        format!("update expected at most 1 argument, got {}", args.len()),
                    ));
                }
                // Check whether we need to intercept.  If the single positional
                // arg is a primitive type that pyrust_builtins::dict::call already
                // handles correctly, delegate.
                let needs_interp = match args.first() {
                    None => false,
                    Some(arg) => !matches!(
                        arg.kind(),
                        ValueKind::Dict(_)
                            | ValueKind::List(_)
                            | ValueKind::Tuple(_)
                            | ValueKind::Str(_)
                            | ValueKind::Bytes(_)
                    ),
                };
                if !needs_interp {
                    return pyrust_builtins::dict::call("update", &receiver, args, kwargs);
                }
                // Intercept: the arg is a non-primitive iterable (Range,
                // Generator, BuiltinObject, PyInstance, …).
                let arg = args.into_iter().next().unwrap();
                // Drive the iterable one element at a time and insert each
                // pair into the dict eagerly.  This matches CPython: items
                // yielded before a mid-iteration exception are already in the
                // dict.  Using collect_iterable (materialise-then-process)
                // would silently drop those items when the generator raises.
                let iter = crate::builtin_modules::builtins::make_iterator(self, &arg)?;
                // Each element must be a length-2 sequence; extract the key and
                // value.  Mirror the logic in pyrust_builtins::dict's push_pair,
                // but use value_to_pykey so user-defined __hash__/__eq__ fire
                // correctly for PyInstance keys.
                let mut idx: usize = 0;
                loop {
                    let elem = match self.call_next(&iter, None) {
                        Ok(v) => v,
                        Err(ref e) if e.class_name_is("StopIteration") => break,
                        Err(e) => return Err(e),
                    };
                    let (k_val, v_val): (Value, Value) = match elem.kind() {
                        ValueKind::List(items) => {
                            let len = items.len();
                            if len != 2 {
                                return Err(PyError::named(
                                    "ValueError",
                                    format!(
                                        "dictionary update sequence element #{idx} has length {len}; 2 is required"
                                    ),
                                ));
                            }
                            (items[0].clone(), items[1].clone())
                        }
                        ValueKind::Tuple(items) => {
                            let len = items.len();
                            if len != 2 {
                                return Err(PyError::named(
                                    "ValueError",
                                    format!(
                                        "dictionary update sequence element #{idx} has length {len}; 2 is required"
                                    ),
                                ));
                            }
                            (items[0].clone(), items[1].clone())
                        }
                        ValueKind::Str(s) => {
                            let chars: Vec<char> = s.chars().collect();
                            let len = chars.len();
                            if len != 2 {
                                return Err(PyError::named(
                                    "ValueError",
                                    format!(
                                        "dictionary update sequence element #{idx} has length {len}; 2 is required"
                                    ),
                                ));
                            }
                            (
                                Value::string(chars[0].to_string()),
                                Value::string(chars[1].to_string()),
                            )
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "cannot convert dictionary update sequence element #{idx} to a sequence"
                                ),
                            ));
                        }
                    };
                    let pk = self.value_to_pykey(&k_val)?;
                    receiver
                        .dict_with_mut(|dict| {
                            dict.insert(pk, v_val);
                        })
                        .ok_or_else(|| {
                            PyError::Runtime("internal: expected dict".to_string())
                        })?;
                    idx += 1;
                }
                // Apply keyword arguments after the positional iterable,
                // matching CPython's order.
                for (k, v) in kwargs {
                    receiver
                        .dict_with_mut(|dict| {
                            dict.insert(k.clone(), v.clone());
                        })
                        .ok_or_else(|| {
                            PyError::Runtime("internal: expected dict".to_string())
                        })?;
                }
                Ok(Value::none())
            }
            // `fromkeys` is a classmethod: ignore the dict receiver and call
            // the registry dispatch directly with the user-supplied args.
            "fromkeys" => {
                let dispatch = crate::builtin_registry::lookup("dict.fromkeys")
                    .ok_or_else(|| {
                        PyError::Runtime(
                            "internal: dict.fromkeys not in registry".to_string(),
                        )
                    })?;
                let expanded: Vec<ExpandedCallArg> = args
                    .into_iter()
                    .map(|v| ExpandedCallArg { name: None, value: v })
                    .collect();
                dispatch(self, &expanded)
            }
            _ => pyrust_builtins::dict::call(method, &receiver, args, kwargs),
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
            // set.update uses value_to_pykey so that hashable slices and
            // PyInstance elements (which need __hash__ dispatch) work correctly.
            // The pyrust-builtins path calls Value::to_key() which returns None
            // for slices (SliceOps doesn't implement hash), causing a misleading
            // "unhashable type: 'slice'" error for all slices.
            "update" => {
                for arg in args {
                    // Snapshot if the argument is the receiver itself to avoid
                    // aliased-borrow issues during iteration (matches CPython
                    // semantics: s.update(s) is a no-op).
                    if arg.value_id() == receiver.value_id() && arg.value_id().is_some() {
                        let snapshot: Vec<PyKey> = receiver
                            .set_with(|s| s.iter().cloned().collect())
                            .unwrap_or_default();
                        for pk in snapshot {
                            if self.set_lookup(&receiver, &pk)?.is_none() {
                                receiver.set_add(pk)?;
                            }
                        }
                        continue;
                    }
                    // If the arg is already a set, copy its PyKeys directly.
                    if arg.as_set().is_some() {
                        let keys: Vec<PyKey> =
                            arg.set_with(|s| s.iter().cloned().collect()).unwrap_or_default();
                        for pk in keys {
                            if self.set_lookup(&receiver, &pk)?.is_none() {
                                receiver.set_add(pk)?;
                            }
                        }
                        continue;
                    }
                    // General iterable: iterate and hash each element via
                    // value_to_pykey so slices and PyInstances are handled.
                    let items = self.collect_iterable(&arg)?;
                    for item in items {
                        let pk = self.value_to_pykey(&item)?;
                        if self.set_lookup(&receiver, &pk)?.is_none() {
                            receiver.set_add(pk)?;
                        }
                    }
                }
                Ok(Value::none())
            }
            _ => pyrust_builtins::set::call(method, &receiver, args),
        }
    }

    /// Dispatch any str method.  `join` is handled here to support generators
    /// and any custom iterable via `collect_iterable`; `format_map` is handled
    /// here because it routes through `format_str_template_map`; `format` is
    /// intercepted in the bound-method dispatch path in `calls.rs` (which has
    /// access to kwargs) before reaching this function.  Everything else delegates to
    /// the interpreter-free `pyrust_builtins::string::call`.
    pub(crate) fn call_str_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if method == "format_map" {
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "str.format_map() takes exactly one argument ({} given)",
                        args.len()
                    ),
                ));
            }
            // Borrow template as &str from the receiver to avoid a heap allocation.
            // receiver is held by value for the lifetime of this block.
            let template: &str = match receiver.kind() {
                ValueKind::Str(s) => s,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "descriptor 'format_map' requires a 'str' object".to_string(),
                    ))
                }
            };
            let mapping = args.into_iter().next().unwrap();
            return self.format_str_template_map(template, mapping);
        }
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
                let items = self.collect_iterable(&iterable).map_err(|e| {
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
        if method == "translate" {
            // Dict fast path: delegate to pyrust-builtins which handles the
            // common `str.maketrans`-produced dict without needing the interpreter.
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "str.translate() takes exactly one argument ({} given)",
                        args.len()
                    ),
                ));
            }
            let is_dict = matches!(args[0].kind(), ValueKind::Dict(_));
            if is_dict {
                return pyrust_builtins::string::call("translate", &receiver, args);
            }
            // General mapping protocol: call table[ordinal] per codepoint.
            // KeyError / IndexError / LookupError → keep character;
            // None → delete; int → replace with chr(n); str → replace.
            // Materialise chars and reserve capacity under a narrow borrow so
            // that the &str from receiver.kind() drops before eval_index needs
            // a &mut self borrow (they are separate but keep scopes explicit).
            let (chars, out_capacity) = match receiver.kind() {
                ValueKind::Str(s) => (s.chars().collect::<Vec<char>>(), s.len()),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "descriptor 'translate' requires a 'str' object".to_string(),
                    ))
                }
            };
            let table = args.into_iter().next().unwrap();
            let mut out = String::with_capacity(out_capacity);
            for c in chars {
                let cp = Value::int(c as i64);
                match self.eval_index(&table, cp) {
                    Ok(v) => {
                        // Resolve int/str subclass instances to their backing
                        // primitive before the value match. This covers:
                        //   int subclass  → Int/Bool/BigInt backing
                        //   str subclass  → Str backing
                        // A PyInstance without a relevant backing falls through
                        // to the TypeError arm below. The tag enum breaks the
                        // borrow on v.kind() before we move v or the backing.
                        let inst_backing = match v.kind() {
                            ValueKind::PyInstance(inst) => {
                                Some(instance_builtin_data(&Rc::clone(inst)))
                            }
                            _ => None,
                        };
                        let v = match inst_backing {
                            Some(Some(backing)) => backing,
                            _ => v,
                        };
                        match v.kind() {
                            ValueKind::None => { /* delete */ }
                            ValueKind::Int(n) => {
                                if n < 0 || n > 0x10FFFF {
                                    return Err(PyError::named(
                                        "ValueError",
                                        "character mapping must be in range(0x110000)"
                                            .to_string(),
                                    ));
                                }
                                let replacement = char::from_u32(n as u32).ok_or_else(|| {
                                    PyError::named(
                                        "ValueError",
                                        "character mapping must be in range(0x110000)"
                                            .to_string(),
                                    )
                                })?;
                                out.push(replacement);
                            }
                            ValueKind::Bool(b) => {
                                let replacement = char::from_u32(b as u32)
                                    .expect("0 and 1 are valid codepoints");
                                out.push(replacement);
                            }
                            ValueKind::BigInt(n) => {
                                // Use ToPrimitive::to_u32 then char::from_u32 to
                                // validate the range [0, 0x10FFFF] in one step.
                                // A negative or > u32::MAX BigInt yields None from
                                // to_u32(); char::from_u32 rejects surrogates and
                                // values > 0x10FFFF. Both map to the same ValueError.
                                use crate::value::PyToPrimitive;
                                let replacement =
                                    n.to_u32().and_then(char::from_u32).ok_or_else(|| {
                                        PyError::named(
                                            "ValueError",
                                            "character mapping must be in range(0x110000)"
                                                .to_string(),
                                        )
                                    })?;
                                out.push(replacement);
                            }
                            ValueKind::Str(repl) => {
                                out.push_str(&repl.to_string());
                            }
                            _ => {
                                return Err(PyError::named(
                                    "TypeError",
                                    "character mapping must return integer, None or str"
                                        .to_string(),
                                ));
                            }
                        }
                    }
                    Err(e)
                        if e.class_name_is("KeyError")
                            || e.class_name_is("IndexError")
                            || e.class_name_is("LookupError") =>
                    {
                        out.push(c);
                    }
                    Err(e) => return Err(e),
                }
            }
            return Ok(Value::string(out));
        }
        pyrust_builtins::string::call(method, &receiver, args)
    }

    /// Dispatch `bytes.join()` with support for generators and arbitrary iterables.
    /// All other bytes methods are handled directly by `pyrust_builtins::bytes::call`.
    pub(crate) fn call_bytes_join(
        &mut self,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "bytes.join() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        }
        let iterable = args.into_iter().next().unwrap();
        let needs_collect = !matches!(
            iterable.kind(),
            ValueKind::List(_) | ValueKind::Tuple(_)
        );
        let iterable = if needs_collect {
            let items = self.collect_iterable(&iterable).map_err(|e| {
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
        pyrust_builtins::bytes::call(
            "join",
            &receiver,
            &[iterable],
            &indexmap::IndexMap::new(),
        )
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
                if let Some(r) = set_binary_op(&left, &right, SetOp::Sub, "-") {
                    return r;
                }
                self.sub(left, right)
            }
            BinaryOp::Mul => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mul__", "__rmul__") {
                    return r;
                }
                // Before raising TypeError, try __index__ on the count operand
                // when one side is a built-in sequence.  CPython calls
                // PyNumber_AsSsize_t (which invokes __index__) before failing.
                let is_seq = |v: &Value| {
                    matches!(
                        v.kind(),
                        ValueKind::Str(_)
                            | ValueKind::List(_)
                            | ValueKind::Tuple(_)
                            | ValueKind::Bytes(_)
                    )
                };
                let is_py_instance =
                    |v: &Value| matches!(v.kind(), ValueKind::PyInstance(_));
                if is_seq(&left) && is_py_instance(&right) {
                    let right = self.try_index_for_seq_repeat(right)?;
                    return self.mul(left, right);
                }
                if is_seq(&right) && is_py_instance(&left) {
                    let left = self.try_index_for_seq_repeat(left)?;
                    return self.mul(left, right);
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
                // Issue #1204: extract backing for scalar primitive subclasses.
                self.div(coerce_numeric(&left), coerce_numeric(&right))
            }
            BinaryOp::FloorDiv => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__floordiv__", "__rfloordiv__") {
                    return r;
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                self.floor_div(coerce_numeric(&left), coerce_numeric(&right))
            }
            BinaryOp::Mod => {
                // str % args: printf-style formatting (#1393).
                // Must come BEFORE try_dunder_binary so that rhs.__rmod__ is
                // never consulted when lhs is str — CPython's str.__mod__ is
                // never NotImplemented, so the reflected slot must not run
                // (#1472).
                // Also covers str subclasses (PyInstance with Str backing):
                // CPython's tp_as_sequence->sq_remainder for str subclasses
                // still runs str.__mod__, never returning NotImplemented.
                let str_backing = if matches!(left.kind(), ValueKind::Str(_)) {
                    Some(left.clone())
                } else if let Some(inst_rc) = left.as_py_instance_rc() {
                    if let Some(backing) = instance_builtin_data(&inst_rc) {
                        if matches!(backing.kind(), ValueKind::Str(_)) {
                            Some(backing)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(fmt_val) = str_backing {
                    return self.str_printf_format(fmt_val, right);
                }
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mod__", "__rmod__") {
                    return r;
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                self.modulo(coerce_numeric(&left), coerce_numeric(&right))
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
                if let Some(r) = set_subset_cmp(&left, &right, BinaryOp::Lt) {
                    return r;
                }
                self.compare(left, right, "<", |o| o.is_lt())
            }
            BinaryOp::Le => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__le__", "__ge__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(&left, &right, BinaryOp::Le) {
                    return r;
                }
                self.compare(left, right, "<=", |o| o.is_le())
            }
            BinaryOp::Gt => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__gt__", "__lt__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(&left, &right, BinaryOp::Gt) {
                    return r;
                }
                self.compare(left, right, ">", |o| o.is_gt())
            }
            BinaryOp::Ge => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__ge__", "__le__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(&left, &right, BinaryOp::Ge) {
                    return r;
                }
                self.compare(left, right, ">=", |o| o.is_ge())
            }
            BinaryOp::Pow => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__pow__", "__rpow__") {
                    return r;
                }
                // Issue #1204: extract scalar primitive backing so that
                // `MyInt(42) ** 2` works identically to `42 ** 2`.
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
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
                        // `both_as_complex` returns Ok(Some) only when at
                        // least one operand is already a Complex value, so
                        // pure int/float paths continue to use `powf` below.
                        // The BigInt arm in `as_complex_pair` now handles
                        // BigInt coercion (including OverflowError) uniformly.
                        if let Some(((zr, zi), (wr, wi))) = both_as_complex(&left, &right)? {
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
                if let Some(r) = set_binary_op(&left, &right, SetOp::And, "&") {
                    return r;
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = value_type_name_str(&left);
                let rt = value_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // BigInt × int / int × BigInt / BigInt × BigInt all flow
                // through the BigInt path; int × int stays on the fast
                // path inside `bitwise_op`.  See issue #485.
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for &: '{lt}' and '{rt}'"),
                        )
                    })?;
                    let b = value_to_bigint(&right).ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for &: '{lt}' and '{rt}'"),
                        )
                    })?;
                    return Ok(Value::bigint(a & b));
                }
                self.bitwise_op(&left, &right, |a, b| Ok(a & b), "&", &lt, &rt)
            }
            BinaryOp::BitOr => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__or__", "__ror__") {
                    return r;
                }
                // PEP 584: dict | dict → new merged dict (right wins on key collision).
                // Covers plain `dict` and PyInstance dict subclasses; PyInstance subclasses
                // with a custom `__or__` were already handled by the dunder path above.
                if let Some(lhs_entries) = dict_entries_from_value(&left) {
                    let left_type = value_type_name_str(&left);
                    let right_type = value_type_name_str(&right);
                    let Some(rhs_entries) = dict_entries_from_value(&right) else {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "unsupported operand type(s) for |: '{left_type}' and '{right_type}'"
                            ),
                        ));
                    };
                    let mut merged: IndexMap<PyKey, Value> = lhs_entries.into_iter().collect();
                    for (k, v) in rhs_entries {
                        merged.insert(k, v);
                    }
                    return Ok(Value::dict(merged));
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::Or, "|") {
                    return r;
                }
                // PEP 604: `type | type` (and `None | type`, `type | None`,
                // `UnionType | type`, etc.) creates a `types.UnionType`.
                // `None` is coerced to `NoneType` as the union component.
                // At least one operand must be a strict type (PyClass / BuiltinFunction /
                // UnionType): `None | None` has neither side as a type and must raise
                // TypeError, matching CPython 3.12 (`type.__or__` is what makes it work).
                if is_union_operand(&left)
                    && is_union_operand(&right)
                    && (is_strict_type_union_operand(&left) || is_strict_type_union_operand(&right))
                {
                    let lhs = coerce_none_to_nonetype(left);
                    let rhs = coerce_none_to_nonetype(right);
                    return Ok(pyrust_builtins::union_type::make_union_type(lhs, rhs));
                }
                // `None | None`: both operands looked like union components but neither
                // was a type, so CPython raises TypeError with the operand-type message.
                if is_union_operand(&left) && is_union_operand(&right) {
                    let lt = value_type_name_str(&left);
                    let rt = value_type_name_str(&right);
                    return Err(PyError::named(
                        "TypeError",
                        format!("unsupported operand type(s) for |: '{lt}' and '{rt}'"),
                    ));
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = value_type_name_str(&left);
                let rt = value_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for |: '{lt}' and '{rt}'"),
                        )
                    })?;
                    let b = value_to_bigint(&right).ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for |: '{lt}' and '{rt}'"),
                        )
                    })?;
                    return Ok(Value::bigint(a | b));
                }
                self.bitwise_op(&left, &right, |a, b| Ok(a | b), "|", &lt, &rt)
            }
            BinaryOp::BitXor => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__xor__", "__rxor__") {
                    return r;
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::Xor, "^") {
                    return r;
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = value_type_name_str(&left);
                let rt = value_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for ^: '{lt}' and '{rt}'"),
                        )
                    })?;
                    let b = value_to_bigint(&right).ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for ^: '{lt}' and '{rt}'"),
                        )
                    })?;
                    return Ok(Value::bigint(a ^ b));
                }
                self.bitwise_op(&left, &right, |a, b| Ok(a ^ b), "^", &lt, &rt)
            }
            BinaryOp::LShift => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__lshift__", "__rlshift__") {
                    return r;
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = value_type_name_str(&left);
                let rt = value_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // BigInt LHS: shift exactly, no `& 63` truncation.
                // Int LHS with a BigInt RHS: the shift count is
                // astronomically large.  See #485.
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for <<: '{lt}' and '{rt}'"),
                        )
                    })?;
                    let sc = shift_count(&right).map_err(|e| match e {
                        PyError::Named(ref name, _) if name == "TypeError" => PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for <<: '{lt}' and '{rt}'"),
                        ),
                        other => other,
                    })?;
                    return match sc {
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
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for <<: '{lt}' and '{rt}'"),
                        ))
                    }
                };
                match shift_count(&right).map_err(|e| match e {
                    PyError::Named(ref name, _) if name == "TypeError" => PyError::named(
                        "TypeError",
                        format!("unsupported operand type(s) for <<: '{lt}' and '{rt}'"),
                    ),
                    other => other,
                })? {
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
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = value_type_name_str(&left);
                let rt = value_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                if matches!(left.kind(), ValueKind::BigInt(_)) || matches!(right.kind(), ValueKind::BigInt(_)) {
                    let a = value_to_bigint(&left).ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for >>: '{lt}' and '{rt}'"),
                        )
                    })?;
                    let sc = shift_count(&right).map_err(|e| match e {
                        PyError::Named(ref name, _) if name == "TypeError" => PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for >>: '{lt}' and '{rt}'"),
                        ),
                        other => other,
                    })?;
                    return match sc {
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
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("unsupported operand type(s) for >>: '{lt}' and '{rt}'"),
                        ))
                    }
                };
                match shift_count(&right).map_err(|e| match e {
                    PyError::Named(ref name, _) if name == "TypeError" => PyError::named(
                        "TypeError",
                        format!("unsupported operand type(s) for >>: '{lt}' and '{rt}'"),
                    ),
                    other => other,
                })? {
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
        // bytearray concatenation: handle before coerce_numeric since bytearray
        // is a BuiltinObject and would fall through the numeric match arms.
        // CPython 3.12: bytearray + bytearray → bytearray, bytearray + bytes →
        // bytearray, bytes + bytearray → bytes.
        let lhs_ba = pyrust_builtins::bytearray::as_bytearray_snapshot(&left);
        let rhs_ba = pyrust_builtins::bytearray::as_bytearray_snapshot(&right);
        if lhs_ba.is_some() || rhs_ba.is_some() {
            match (lhs_ba, rhs_ba) {
                (Some(a), Some(b)) => {
                    // bytearray + bytearray → bytearray
                    let mut out = a;
                    out.extend_from_slice(&b);
                    return Ok(pyrust_builtins::bytearray::bytearray(out));
                }
                (Some(a), None) => {
                    // bytearray + bytes → bytearray
                    if let ValueKind::Bytes(rc) = right.kind() {
                        let mut out = a;
                        out.extend_from_slice(&rc);
                        return Ok(pyrust_builtins::bytearray::bytearray(out));
                    }
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "can't concat {} to bytearray",
                            pyrust_core::builtin_type_name(&right)
                        ),
                    ));
                }
                (None, Some(b)) => {
                    // bytes + bytearray → bytes (CPython 3.12 parity)
                    if let ValueKind::Bytes(rc) = left.kind() {
                        let mut out = rc.as_ref().clone();
                        out.extend_from_slice(&b);
                        return Ok(Value::bytes(out));
                    }
                    // Non-bytes LHS with bytearray RHS: mirror CPython's
                    // per-type error messages.
                    let lt = value_type_name_str(&left);
                    let err_msg = match left.kind() {
                        ValueKind::Str(_) | ValueKind::List(_) | ValueKind::Tuple(_) => {
                            format!("can only concatenate {lt} (not \"bytearray\") to {lt}")
                        }
                        _ => format!(
                            "unsupported operand type(s) for +: '{lt}' and 'bytearray'"
                        ),
                    };
                    return Err(PyError::named("TypeError", err_msg));
                }
                (None, None) => unreachable!(),
            }
        }
        let (l, r) = (coerce_numeric(&left), coerce_numeric(&right));
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
        let (l, r) = (coerce_numeric(&left), coerce_numeric(&right));
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

    /// Resolve a sequence repetition count through `__index__` when the value
    /// is a PyInstance.  Returns the original value for int/bool/bigint.
    /// Raises `TypeError` if `__index__` returns non-int, or if the instance
    /// has no `__index__` at all, matching CPython 3.12 sequence repetition.
    fn try_index_for_seq_repeat(&mut self, val: Value) -> Result<Value> {
        // Use a Tag enum so the Ref guard from val.kind() drops before we
        // need to move `val` (same pattern as resolve_index_arg in calls.rs).
        enum Tag {
            Int,
            Instance(Rc<RefCell<PyInstance>>),
            Other,
        }
        let type_name_for_err = value_type_name_str(&val).to_string();
        let tag = match val.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => Tag::Int,
            ValueKind::PyInstance(inst) => Tag::Instance(Rc::clone(inst)),
            _ => Tag::Other,
        };
        match tag {
            Tag::Int => Ok(val),
            Tag::Other => Err(PyError::named(
                "TypeError",
                format!("can't multiply sequence by non-int of type '{type_name_for_err}'"),
            )),
            Tag::Instance(inst_rc) => {
                let class = Rc::clone(&inst_rc.borrow().class);
                let Some(method_val) = lookup_class_attr(&class, "__index__") else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "can't multiply sequence by non-int of type '{type_name_for_err}'"
                        ),
                    ));
                };
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?;
                // Classify the result before consuming it so the borrow on
                // result.kind() ends before we move `result` into Ok(_).
                enum ResultTag {
                    SmallInt,
                    BigInt,
                    Other,
                }
                let result_type_name = value_type_name_str(&result).to_string();
                let result_tag = match result.kind() {
                    ValueKind::Int(_) | ValueKind::Bool(_) => ResultTag::SmallInt,
                    ValueKind::BigInt(_) => ResultTag::BigInt,
                    _ => ResultTag::Other,
                };
                match result_tag {
                    ResultTag::SmallInt => Ok(result),
                    ResultTag::BigInt => {
                        // CPython's PyNumber_AsSsize_t raises OverflowError using
                        // the *original* object's type name, not "int".
                        Err(PyError::named(
                            "OverflowError",
                            format!(
                                "cannot fit '{type_name_for_err}' into an index-sized integer"
                            ),
                        ))
                    }
                    ResultTag::Other => Err(PyError::named(
                        "TypeError",
                        format!("__index__ returned non-int (type {result_type_name})"),
                    )),
                }
            }
        }
    }

    fn mul(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            // (ar+ai*j) * (br+bi*j) = (ar*br - ai*bi) + (ar*bi + ai*br)j
            return Ok(Value::complex(a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0));
        }
        // bytearray * int / int * bytearray — handled before coerce_numeric
        // because bytearray is a BuiltinObject and won't match any explicit arm.
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&left) {
            let n = match right.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(_) => {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot fit 'int' into an index-sized integer".to_string(),
                    ));
                }
                _ => {
                    let type_name = value_type_name_str(&right);
                    return Err(PyError::named(
                        "TypeError",
                        format!("can't multiply sequence by non-int of type '{type_name}'"),
                    ));
                }
            };
            return seq_repeat_bytearray(&data, n);
        }
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&right) {
            let n = match left.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(_) => {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot fit 'int' into an index-sized integer".to_string(),
                    ));
                }
                _ => {
                    let type_name = value_type_name_str(&left);
                    return Err(PyError::named(
                        "TypeError",
                        format!("can't multiply sequence by non-int of type '{type_name}'"),
                    ));
                }
            };
            return seq_repeat_bytearray(&data, n);
        }
        let (l, r) = (coerce_numeric(&left), coerce_numeric(&right));
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
                seq_repeat_str(text, n)
            }
            (ValueKind::Int(n), ValueKind::Str(text)) => {
                seq_repeat_str(text, n)
            }
            (ValueKind::Str(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::Str(_)) => Err(PyError::named(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            )),
            (ValueKind::List(items), ValueKind::Int(n)) => seq_repeat_list(&items, n),
            (ValueKind::Int(n), ValueKind::List(items)) => seq_repeat_list(&items, n),
            (ValueKind::List(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::List(_)) => Err(PyError::named(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            )),
            (ValueKind::Bytes(data), ValueKind::Int(n)) => seq_repeat_bytes(&data, n),
            (ValueKind::Int(n), ValueKind::Bytes(data)) => seq_repeat_bytes(&data, n),
            (ValueKind::Bytes(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::Bytes(_)) => Err(PyError::named(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            )),
            // Tuple * Int / Int * Tuple — checked repeat, MemoryError on
            // overflow (matches CPython 3.12 `tuplerepeat` behaviour).
            (ValueKind::Tuple(items), ValueKind::Int(n)) => {
                seq_repeat_tuple(&items[..], n)
            }
            (ValueKind::Int(n), ValueKind::Tuple(items)) => {
                seq_repeat_tuple(&items[..], n)
            }
            // Tuple * BigInt / BigInt * Tuple — any BigInt is too large to
            // fit in a platform index; CPython raises OverflowError for both
            // positive and negative BigInt values.
            (ValueKind::Tuple(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::Tuple(_)) => Err(PyError::named(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            )),
            _ => {
                let is_sequence = |v: &Value| {
                    matches!(
                        v.kind(),
                        ValueKind::Str(_)
                            | ValueKind::List(_)
                            | ValueKind::Tuple(_)
                            | ValueKind::Bytes(_)
                    )
                };
                let is_int_like = |v: &Value| {
                    matches!(v.kind(), ValueKind::Int(_) | ValueKind::BigInt(_))
                };
                if is_sequence(&l) && !is_int_like(&r) {
                    let type_name = value_type_name_str(&r);
                    return Err(PyError::named(
                        "TypeError",
                        format!("can't multiply sequence by non-int of type '{type_name}'"),
                    ));
                }
                if is_sequence(&r) && !is_int_like(&l) {
                    let type_name = value_type_name_str(&l);
                    return Err(PyError::named(
                        "TypeError",
                        format!("can't multiply sequence by non-int of type '{type_name}'"),
                    ));
                }
                Err(Self::unsupported_binary_operand("*"))
            }
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
        left: &Value,
        op: BinaryOp,
        right: &Value,
        is_augmented_assign: bool,
    ) -> Result<Option<Value>> {
        // Fast paths for built-in mutable containers: mutate in-place and
        // return the *same* Value (same Rc pointer) so that aliases see the
        // update.  This implements the Python guarantee that `a += b` on a
        // list or set does not rebind aliases.
        //
        // Quick scalar-exit: primitive scalars (Int, Float, Bool, Str, Bytes,
        // BigInt, Complex, None, Ellipsis, Range) cannot have in-place mutation
        // semantics, so return None immediately without dispatching a dunder.
        // This keeps BinOpConst cost near-zero for the common int/float case.
        if matches!(
            left.kind(),
            ValueKind::Int(_)
                | ValueKind::Float(_)
                | ValueKind::Bool(_)
                | ValueKind::Str(_)
                | ValueKind::Bytes(_)
                | ValueKind::BigInt(_)
                | ValueKind::Complex(_, _)
                | ValueKind::None
                | ValueKind::Ellipsis
                | ValueKind::Tuple(_)
                | ValueKind::Range { .. }
                | ValueKind::NotImplemented
        ) {
            return Ok(None);
        }
        let is_list = matches!(left.kind(), ValueKind::List(_));
        let is_set = matches!(left.kind(), ValueKind::Set(_));
        if is_list {
            match op {
                BinaryOp::Add => {
                    // list += iterable  =>  list.extend(iterable)
                    let items = self.collect_iterable(right)?;
                    left.list_extend(items)?;
                    return Ok(Some(left.clone()));
                }
                BinaryOp::Mul => {
                    // list *= n  =>  repeat in-place
                    let n = match right.kind() {
                        ValueKind::Int(n) => n,
                        ValueKind::Bool(b) => b as i64,
                        _ => return Ok(None), // fall through to TypeError
                    };
                    left.list_with_mut(|items| {
                        if n <= 0 {
                            items.clear();
                        } else {
                            let orig = items.clone();
                            for _ in 1..n {
                                items.extend_from_slice(&orig);
                            }
                        }
                    });
                    return Ok(Some(left.clone()));
                }
                _ => {}
            }
        } else if is_set {
            match op {
                BinaryOp::BitOr | BinaryOp::BitAnd | BinaryOp::Sub | BinaryOp::BitXor => {
                    // set |= / &= / -= / ^= require RHS to be a set or frozenset.
                    // If RHS is neither, raise the CPython-format TypeError directly
                    // (the op symbol must include `=` for in-place operators).
                    let rhs_items = match set_items_from_value(&right) {
                        Some((items, _)) => items,
                        None => {
                            let op_sym = match op {
                                BinaryOp::BitOr => "|=",
                                BinaryOp::BitAnd => "&=",
                                BinaryOp::Sub => "-=",
                                BinaryOp::BitXor => "^=",
                                _ => unreachable!(),
                            };
                            let lt = value_type_name_str(&left);
                            let rt = value_type_name_str(&right);
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"
                                ),
                            ));
                        }
                    };
                    left.set_with_mut(|lhs| match op {
                        BinaryOp::BitOr => {
                            for k in &rhs_items {
                                lhs.insert(k.clone());
                            }
                        }
                        BinaryOp::BitAnd => {
                            lhs.retain(|k| rhs_items.contains(k));
                        }
                        BinaryOp::Sub => {
                            for k in &rhs_items {
                                lhs.shift_remove(k);
                            }
                        }
                        BinaryOp::BitXor => {
                            let mut to_add: Vec<PyKey> = Vec::new();
                            for k in &rhs_items {
                                if !lhs.contains(k) {
                                    to_add.push(k.clone());
                                }
                            }
                            lhs.retain(|k| !rhs_items.contains(k));
                            for k in to_add {
                                lhs.insert(k);
                            }
                        }
                        _ => unreachable!(),
                    });
                    return Ok(Some(left.clone()));
                }
                _ => {}
            }
        } else if let Some(data_rc) = pyrust_builtins::bytearray::as_bytearray_rc(left) {
            // bytearray += / bytearray *= — mutate backing Vec in place so
            // that aliases (other variables referencing the same bytearray)
            // also see the change.
            match op {
                BinaryOp::Add => {
                    let rhs = if let Some(rhs_data) =
                        pyrust_builtins::bytearray::as_bytearray_snapshot(right)
                    {
                        rhs_data
                    } else if let ValueKind::Bytes(rc) = right.kind() {
                        rc.as_slice().to_vec()
                    } else {
                        let type_name = value_type_name_str(right);
                        return Err(PyError::named(
                            "TypeError",
                            format!("can't concat {type_name} to bytearray"),
                        ));
                    };
                    data_rc.borrow_mut().extend_from_slice(&rhs);
                    return Ok(Some(left.clone()));
                }
                BinaryOp::Mul => {
                    let n = match right.kind() {
                        ValueKind::Int(n) => n,
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::BigInt(_) => {
                            return Err(PyError::named(
                                "OverflowError",
                                "cannot fit 'int' into an index-sized integer".to_string(),
                            ));
                        }
                        _ => {
                            let type_name = value_type_name_str(right);
                            return Err(PyError::named(
                                "TypeError",
                                format!("can't multiply sequence by non-int of type '{type_name}'"),
                            ));
                        }
                    };
                    let mut data = data_rc.borrow_mut();
                    if n <= 0 {
                        data.clear();
                    } else {
                        let orig = data.clone();
                        for _ in 1..n {
                            data.extend_from_slice(&orig);
                        }
                    }
                    return Ok(Some(left.clone()));
                }
                _ => {}
            }
        } else if matches!(left.kind(), ValueKind::Dict(_)) && op == BinaryOp::BitOr {
            // PEP 584: dict |= other → in-place update.
            // Plain dict: skip dunder path, go directly to update().
            // For binary | (not augmented assign), only dict-compatible RHS is
            // valid; fall through to eval_binary for the TypeError with correct
            // operand names.  For |= the full dict.update() semantics apply
            // (accepts dicts and iterables of pairs).
            if is_augmented_assign || dict_entries_from_value(right).is_some() {
                let empty_kw = indexmap::IndexMap::new();
                pyrust_builtins::dict::call("update", left, vec![right.clone()], &empty_kw)?;
                return Ok(Some(left.clone()));
            }
        }

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
        let result = self.try_call_binary_method(&left, dunder, right.clone())?;
        if let Some(ref v) = result {
            if !is_not_implemented(v) {
                return Ok(result);
            }
        }
        // PEP 584 fallback: PyInstance dict subclass |= other when no `__ior__`
        // was found.  Call update() on the backing dict (so dict_with_mut works)
        // and return `left` to preserve object identity.
        // For binary | (not augmented assign), only dict-compatible RHS is valid;
        // fall through to eval_binary which uses the subclass type name correctly
        // (e.g. 'D' rather than 'dict') in the unsupported-operand TypeError.
        if op == BinaryOp::BitOr {
            if let Some(inst_rc) = left.as_py_instance_rc() {
                if let Some(backing) = instance_builtin_data(inst_rc) {
                    if matches!(backing.kind(), ValueKind::Dict(_)) {
                        if is_augmented_assign || dict_entries_from_value(right).is_some() {
                            let empty_kw = indexmap::IndexMap::new();
                            pyrust_builtins::dict::call("update", &backing, vec![right.clone()], &empty_kw)?;
                            return Ok(Some(left.clone()));
                        }
                    }
                }
            }
        }
        // Issue #1006 + #1007: PyInstance set subclass |= / &= / -= / ^= — when
        // no user-defined __ior__ / __iand__ / __isub__ / __ixor__ was found,
        // fall back to mutating the backing set in-place and returning `left`
        // so the subclass type is preserved (matching CPython's set.__ior__ etc.
        // which mutate self and return self).
        //
        // Also covers frozenset (plain BuiltinObject) and set subclass TypeError:
        // when LHS is set-like but RHS is not, raise the CPython-format TypeError
        // with the `|=:` / `&=:` / etc. symbol directly (returning None would
        // fall through to eval_binary which uses the non-`=` symbol).
        if matches!(
            op,
            BinaryOp::BitOr | BinaryOp::BitAnd | BinaryOp::Sub | BinaryOp::BitXor
        ) {
            if let Some(inst_rc) = left.as_py_instance_rc() {
                if let Some(backing) = instance_builtin_data(inst_rc) {
                    if matches!(backing.kind(), ValueKind::Set(_)) {
                        let op_sym = match op {
                            BinaryOp::BitOr => "|=",
                            BinaryOp::BitAnd => "&=",
                            BinaryOp::Sub => "-=",
                            BinaryOp::BitXor => "^=",
                            _ => unreachable!(),
                        };
                        let rhs_items = match set_items_from_value(&right) {
                            Some((items, _)) => items,
                            None => {
                                let lt = value_type_name_str(&left);
                                let rt = value_type_name_str(&right);
                                return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"
                                    ),
                                ));
                            }
                        };
                        backing.set_with_mut(|lhs| match op {
                            BinaryOp::BitOr => {
                                for k in &rhs_items {
                                    lhs.insert(k.clone());
                                }
                            }
                            BinaryOp::BitAnd => {
                                lhs.retain(|k| rhs_items.contains(k));
                            }
                            BinaryOp::Sub => {
                                for k in &rhs_items {
                                    lhs.shift_remove(k);
                                }
                            }
                            BinaryOp::BitXor => {
                                let mut to_add: Vec<PyKey> = Vec::new();
                                for k in &rhs_items {
                                    if !lhs.contains(k) {
                                        to_add.push(k.clone());
                                    }
                                }
                                lhs.retain(|k| !rhs_items.contains(k));
                                for k in to_add {
                                    lhs.insert(k);
                                }
                            }
                            _ => unreachable!(),
                        });
                        return Ok(Some(left.clone()));
                    }
                }
            } else {
                // Plain frozenset (BuiltinObject) — not caught by the is_set
                // branch above (which only matches ValueKind::Set).
                if set_items_from_value(&left).is_some() && set_items_from_value(&right).is_none()
                {
                    let op_sym = match op {
                        BinaryOp::BitOr => "|=",
                        BinaryOp::BitAnd => "&=",
                        BinaryOp::Sub => "-=",
                        BinaryOp::BitXor => "^=",
                        _ => unreachable!(),
                    };
                    let lt = value_type_name_str(&left);
                    let rt = value_type_name_str(&right);
                    return Err(PyError::named(
                        "TypeError",
                        format!("unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"),
                    ));
                }
            }
        }
        Ok(None)
    }

    fn matmul(&mut self, left: Value, right: Value) -> Result<Value> {
        if let Some(value) = self.try_call_binary_method(&left, "__matmul__", right.clone())? {
            return Ok(value);
        }
        if let Some(value) = self.try_call_binary_method(&right, "__rmatmul__", left)? {
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
            // `a - modulo` can overflow i64 when `a` is near `i64::MIN` and
            // `modulo > 0` (e.g. `(-2**63) // 3` → modulo=1, a-modulo wraps).
            // `a_adj / b` can overflow when `a = i64::MIN` and `b = -1`
            // (quotient = 2^63, which exceeds i64::MAX).
            // Promote to BigInt for exact floor division in either case.
            if let Some(q) = a.checked_sub(modulo).and_then(|a_adj| a_adj.checked_div(b)) {
                return Ok(Value::int(q));
            }
            let (q, _) = bigint_divmod_floor(&PyBigInt::from(a), &PyBigInt::from(b));
            return Ok(Value::bigint(q));
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
        let mut r = a % b;
        if r == 0.0 {
            // Match CPython float_rem: zero result copies sign of divisor.
            r = r.copysign(b);
        } else if r.signum() != b.signum() {
            r += b;
        }
        Ok(Value::float(r))
    }

    fn compare(
        &self,
        left: Value,
        right: Value,
        op_name: &str,
        cmp: impl Fn(std::cmp::Ordering) -> bool,
    ) -> Result<Value> {
        // Issue #1204: extract scalar primitive backing for subclasses of
        // int/float/str/bytes so that `MyInt(5) < 10` etc. works.
        let left = coerce_numeric(&left);
        let right = coerce_numeric(&right);
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

    /// Resolve one slice bound through the `__index__` protocol if needed.
    ///
    /// Used for the built-in sequence path (List/Tuple/Str/Bytes) where the
    /// caller needs a concrete integer from each bound.  `PyInstance` and
    /// `BuiltinObject` targets receive the raw (unresolved) bound values so
    /// that user `__getitem__` implementations see the original objects — the
    /// same as CPython: `a[Index(2):]` calls `list.__getitem__` which then
    /// applies `__index__`; `my_obj[Index(2):]` delivers `slice(Index(2),
    /// None, None)` unchanged to `my_obj.__getitem__`.
    ///
    /// `None` (a missing bound, e.g. `a[:]`) and Python `None` are passed
    /// through as-is.  `Int`, `Bool`, and `BigInt` are returned unchanged.
    /// `PyInstance` values that define `__index__` are called and the integer
    /// result is returned.  Anything else is left to `slice_index_from_value`
    /// to reject with a proper TypeError.
    fn resolve_slice_bound_val(&mut self, val: Option<Value>) -> Result<Option<Value>> {
        let v = match val {
            None => return Ok(None),
            Some(v) => v,
        };
        // Fast path: already an integer type or Python None — no protocol call needed.
        if v.is_none()
            || matches!(
                v.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return Ok(Some(v));
        }
        // Slow path: try __index__.
        let resolved = self.resolve_index_arg(v)?;
        Ok(Some(resolved))
    }

    fn eval_slice(&mut self, target: &Value, lo: Option<Value>, hi: Option<Value>, st: Option<Value>) -> Result<Value> {
        // PyInstance: dispatch __getitem__ with a slice object built from the
        // raw (unresolved) bounds.  CPython passes the bound objects as-is so
        // that the user's __getitem__ sees them; resolution via __index__ is
        // the caller's responsibility (e.g. when the user delegates back to a
        // built-in sequence).
        if let ValueKind::PyInstance(inst) = target.kind() {
            let inst_rc = Rc::clone(inst);
            // Issue #994: if the instance has a backing primitive value
            // (tuple/frozenset/dict/list/set subclass), delegate slice to it.
            // eval_index does the same for integer subscripts; without this,
            // `MyTuple([1,2,3])[1:3]` reaches the __getitem__ branch and
            // raises TypeError because tuple subclasses don't register a
            // user-level __getitem__.
            // Issue #1134: check user __getitem__ before backing fast path,
            // matching the same ordering fix in eval_index.  The builtin
            // sentinels for the base types are not overrides.
            let class = Rc::clone(&inst_rc.borrow().class);
            let user_getitem = lookup_class_attr(&class, "__getitem__").filter(|v| {
                !matches!(
                    v.kind(),
                    ValueKind::BuiltinFunction(
                        "dict.__getitem__"
                            | "list.__getitem__"
                            | "tuple.__getitem__"
                            | "bytes.__getitem__"
                    )
                )
            });
            if let Some(method_val) = user_getitem {
                let slice_val = pyrust_builtins::slice::make_slice(lo, hi, st);
                return invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[ExpandedCallArg {
                        name: None,
                        value: slice_val,
                    }],
                );
            }
            if let Some(backing) = instance_builtin_data(&inst_rc) {
                return self.eval_slice(&backing, lo, hi, st);
            }
            return Err(PyError::named(
                "TypeError",
                format!("'{}' object is not subscriptable", class.borrow().name),
            ));
        }

        // BuiltinObject: delegate to ops.get_item with a slice value (issue #847).
        // This mirrors what eval_index does when a runtime slice object is used
        // as a subscript, and lets BuiltinObject types opt into slice subscripting
        // via BuiltinTypeOps::get_item.  Pass raw bounds — ops.get_item receives
        // the constructed slice Value and handles resolution internally.
        if let ValueKind::BuiltinObject { ops, state } = target.kind() {
            let slice_val = pyrust_builtins::slice::make_slice(lo, hi, st);
            return ops.get_item(state, &slice_val);
        }

        // Range slicing: compute the result range arithmetically, matching
        // CPython's range.__getitem__ for slice arguments.  Handled before
        // the general built-in sequence path so we never materialise elements.
        //
        // CPython's algorithm (Objects/rangeobject.c):
        //   (sl_start, sl_stop, sl_step) = slice.indices(len(r))
        //   new_start = r.start + sl_start * r.step
        //   new_stop  = r.start + sl_stop  * r.step  ← note: uses r.start, not new_start
        //   new_step  = r.step  * sl_step
        if let ValueKind::Range { start: r_start, stop: r_stop, step: r_step } = target.kind() {
            let lo = self.resolve_slice_bound_val(lo)?;
            let hi = self.resolve_slice_bound_val(hi)?;
            let st = self.resolve_slice_bound_val(st)?;
            let r_len = range_len(r_start, r_stop, r_step);
            let (sl_start, sl_stop, sl_step) =
                Self::resolve_slice_bounds(r_len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
            let new_start = r_start + sl_start * r_step;
            let new_stop  = r_start + sl_stop  * r_step;
            let new_step  = r_step  * sl_step;
            return Ok(Value::range(new_start, new_stop, new_step));
        }

        // Built-in sequences: resolve bounds through __index__ before applying
        // the integer arithmetic in resolve_slice_bounds (issue #849).
        let lo = self.resolve_slice_bound_val(lo)?;
        let hi = self.resolve_slice_bound_val(hi)?;
        let st = self.resolve_slice_bound_val(st)?;

        let len = match target.kind() {
            ValueKind::List(items) => items.len() as i64,
            ValueKind::Tuple(items) => items.len() as i64,
            ValueKind::Str(s) => s.chars().count() as i64,
            ValueKind::Bytes(rc) => rc.len() as i64,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object is not subscriptable",
                        pyrust_core::builtin_type_name(&target)
                    ),
                ));
            }
        };
        let (start, end, step) = Self::resolve_slice_bounds(len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
        let indices = Self::slice_target_indices(len, start, end, step);

        match target.kind() {
            ValueKind::List(items) => Ok(Value::list(indices.into_iter().map(|ix| items[ix].clone()).collect::<Vec<Value>>())),
            ValueKind::Tuple(items) => Ok(Value::tuple(indices.into_iter().map(|ix| items[ix].clone()).collect::<Vec<Value>>())),
            ValueKind::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let mut out = String::new();
                for ix in indices {
                    out.push(chars[ix]);
                }
                Ok(Value::string(out))
            }
            ValueKind::Bytes(rc) => {
                Ok(Value::bytes(indices.into_iter().map(|ix| rc[ix]).collect()))
            }
            _ => unreachable!(),
        }
    }

    fn bitwise_op(
        &self,
        left: &Value,
        right: &Value,
        op: impl Fn(i64, i64) -> Result<i64>,
        op_sym: &str,
        left_type: &str,
        right_type: &str,
    ) -> Result<Value> {
        let a = match left.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => if b { 1 } else { 0 },
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("unsupported operand type(s) for {op_sym}: '{left_type}' and '{right_type}'"),
                ))
            }
        };
        let b = match right.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => if b { 1 } else { 0 },
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("unsupported operand type(s) for {op_sym}: '{left_type}' and '{right_type}'"),
                ))
            }
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
        // List and Tuple: dispatch user `__eq__` when any element or the
        // item itself is a `PyInstance` (Rust `Value::eq` would fall back to
        // `Rc::ptr_eq` for distinct-but-equal instances).
        //
        // Fast path: if neither `item` nor any element requires user dispatch,
        // iterate the raw slice directly — no allocation, no dunder call.
        //
        // Slow path: snapshot the elements first (so user `__eq__` cannot
        // invalidate the raw backing `Vec` through aliased mutation), then
        // walk with `values_user_eq`.  `values_user_eq` has an identity
        // short-circuit (`if a == b { return Ok(true) }`) so the slow path
        // is still allocation-free for lists that happen to contain primitives
        // alongside one PyInstance.
        if let Some(items) = container.as_list() {
            let needs_dispatch = matches!(
                item.kind(),
                ValueKind::PyInstance(_)
                    | ValueKind::List(_)
                    | ValueKind::Tuple(_)
                    | ValueKind::Dict(_)
                    | ValueKind::Set(_)
                    | ValueKind::BuiltinObject { .. }
            ) || items.iter().any(|e| {
                matches!(
                    e.kind(),
                    ValueKind::PyInstance(_)
                        | ValueKind::List(_)
                        | ValueKind::Tuple(_)
                        | ValueKind::Dict(_)
                        | ValueKind::Set(_)
                        | ValueKind::BuiltinObject { .. }
                )
            });
            if needs_dispatch {
                let items: Vec<Value> = items.to_vec();
                for elem in &items {
                    if self.values_user_eq(elem, &item)? {
                        return Ok(Value::bool_(true));
                    }
                }
            } else {
                for elem in items {
                    if *elem == item {
                        return Ok(Value::bool_(true));
                    }
                }
            }
            return Ok(Value::bool_(false));
        }
        if let Some(items) = container.as_tuple() {
            let needs_dispatch = matches!(
                item.kind(),
                ValueKind::PyInstance(_)
                    | ValueKind::List(_)
                    | ValueKind::Tuple(_)
                    | ValueKind::Dict(_)
                    | ValueKind::Set(_)
                    | ValueKind::BuiltinObject { .. }
            ) || items.iter().any(|e| {
                matches!(
                    e.kind(),
                    ValueKind::PyInstance(_)
                        | ValueKind::List(_)
                        | ValueKind::Tuple(_)
                        | ValueKind::Dict(_)
                        | ValueKind::Set(_)
                        | ValueKind::BuiltinObject { .. }
                )
            });
            if needs_dispatch {
                let items: Vec<Value> = items.to_vec();
                for elem in &items {
                    if self.values_user_eq(elem, &item)? {
                        return Ok(Value::bool_(true));
                    }
                }
            } else {
                for elem in items {
                    if *elem == item {
                        return Ok(Value::bool_(true));
                    }
                }
            }
            return Ok(Value::bool_(false));
        }
        match container.kind() {
            ValueKind::List(_) | ValueKind::Tuple(_) => unreachable!("handled above"),
            ValueKind::Set(_) => unreachable!("handled above"),
            ValueKind::BuiltinObject { ops, state } => {
                ops.contains(state, &item).map(Value::bool_)
            }
            ValueKind::Bytes(rc) => {
                match item.kind() {
                    ValueKind::Int(n) if (0..=255).contains(&n) => Ok(Value::bool_(rc.contains(&(n as u8)))),
                    // bool is a subclass of int in Python; True==1 and False==0 are
                    // valid byte values, so treat them as their integer equivalents.
                    ValueKind::Bool(b) => Ok(Value::bool_(rc.contains(&(if b { 1u8 } else { 0u8 })))),
                    ValueKind::Int(_) | ValueKind::BigInt(_) => Err(PyError::named(
                        "ValueError",
                        "byte must be in range(0, 256)".to_string(),
                    )),
                    ValueKind::Bytes(sub) => Ok(Value::bool_(
                        sub.is_empty() || rc.windows(sub.len()).any(|w| w == sub.as_ref().as_slice())
                    )),
                    _ => Err(PyError::named(
                        "TypeError",
                        format!(
                            "a bytes-like object is required, not '{}'",
                            value_type_name_str(&item)
                        ),
                    )),
                }
            }
            ValueKind::Str(s) => {
                match item.kind() {
                    ValueKind::Str(sub) => Ok(Value::bool_(s.contains(sub))),
                    _ => Err(PyError::named(
                        "TypeError",
                        format!(
                            "'in <string>' requires string as left operand, not {}",
                            value_type_name_str(&item)
                        ),
                    )),
                }
            }
            ValueKind::Dict(_) => unreachable!("handled above"),
            ValueKind::Range { start, stop, step } => {
                let range_contains_i64 = |v: i64| -> bool {
                    if step > 0 {
                        v >= start && v < stop && (v - start) % step == 0
                    } else if step < 0 {
                        v <= start && v > stop && (v - start) % step == 0
                    } else {
                        false
                    }
                };
                match item.kind() {
                    ValueKind::Int(v) => Ok(Value::bool_(range_contains_i64(v))),
                    // bool is a subclass of int; True==1, False==0.
                    ValueKind::Bool(b) => Ok(Value::bool_(range_contains_i64(b as i64))),
                    // BigInt: if it fits in i64 apply the check; if it overflows
                    // it cannot be in any range whose bounds are i64.
                    ValueKind::BigInt(n) => {
                        Ok(Value::bool_(n.to_i64().is_some_and(range_contains_i64)))
                    }
                    // Float: if the value is an integer-valued finite float,
                    // convert to i64 and do the fast O(1) range check.
                    // Non-integer or non-finite floats cannot equal any integer.
                    // This matches CPython 3.12's range.__contains__ behaviour.
                    //
                    // Bounds are checked before casting to avoid Rust's saturating
                    // f64-to-i64 cast.  float(2**63) and float(2**63-1) are the same
                    // f64 value (both round to 9.223372036854776e18), so the round-trip
                    // check `(f as i64) as f64 == f` does NOT detect saturation at the
                    // positive boundary.  Use strict half-open bounds instead:
                    // i64 range is [-2**63, 2**63), both endpoints are exact f64 values.
                    ValueKind::Float(f) => {
                        // 9223372036854775808.0 == 2**63 as f64 (exactly representable)
                        const I64_MIN_F: f64 = i64::MIN as f64;
                        const I64_MAX_PLUS1_F: f64 = 9_223_372_036_854_775_808.0_f64;
                        let in_range = f.is_finite()
                            && f.fract() == 0.0
                            && f >= I64_MIN_F
                            && f < I64_MAX_PLUS1_F
                            && range_contains_i64(f as i64);
                        Ok(Value::bool_(in_range))
                    }
                    // Complex: if imaginary part is zero and real part is an
                    // integer-valued finite float, same fast O(1) check.
                    ValueKind::Complex(re, im) => {
                        const I64_MIN_F: f64 = i64::MIN as f64;
                        const I64_MAX_PLUS1_F: f64 = 9_223_372_036_854_775_808.0_f64;
                        let in_range = im == 0.0
                            && re.is_finite()
                            && re.fract() == 0.0
                            && re >= I64_MIN_F
                            && re < I64_MAX_PLUS1_F
                            && range_contains_i64(re as i64);
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
                // list/dict/set subclass with no user-defined __contains__:
                // delegate to the backing primitive, matching CPython's
                // inherited tp_sq_contains / sq_contains slot behaviour.
                if let Some(backing) = instance_builtin_data(&inst_rc) {
                    return self.eval_in(backing, item);
                }
                // No __contains__ or __builtin_data__: fall back to __iter__ if available.
                if let Some(iter_method) = lookup_class_attr(&class, "__iter__") {
                    let iter_obj = invoke_class_method(
                        self,
                        iter_method,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[],
                    )?;
                    loop {
                        match self.call_next(&iter_obj, None) {
                            Ok(elem) => {
                                if self.values_user_eq(&elem, &item)? {
                                    return Ok(Value::bool_(true));
                                }
                            }
                            Err(ref e) if e.class_name_is("StopIteration") => {
                                return Ok(Value::bool_(false));
                            }
                            // class_name_is walks the hierarchy for Raised variants;
                            // subclasses of StopIteration are caught by the arm above.
                            // Any other Raised exception propagates.
                            Err(PyError::Raised(exc)) => return Err(PyError::Raised(exc)),
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
                        match self.call_next(&iter_val, None) {
                            Ok(elem) => {
                                if self.values_user_eq(&elem, &item)? {
                                    return Ok(Value::bool_(true));
                                }
                            }
                            Err(ref e) if e.class_name_is("StopIteration") => {
                                return Ok(Value::bool_(false));
                            }
                            // class_name_is walks the hierarchy; any remaining
                            // Raised is not StopIteration or a subclass.
                            Err(PyError::Raised(exc)) => return Err(PyError::Raised(exc)),
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

    /// Coerce a `PyInstance` argument to its int backing (for int subclasses) or
    /// call `__index__` (for objects that define it), ready for integer printf
    /// format codes (`%d`, `%i`, `%u`, `%o`, `%x`, `%X`).
    ///
    /// Non-`PyInstance` values are returned unchanged; `str_printf_to_int` will
    /// handle them (or raise `TypeError`) as before.  This mirrors CPython's
    /// `PyNumber_Index` pre-coercion that happens before `formatlong`.
    fn coerce_printf_int_arg(&mut self, val: Value) -> Result<Value> {
        // Use a tag enum so the borrow from val.kind() ends before we move val.
        enum Tag {
            Instance(Rc<RefCell<PyInstance>>),
            Other,
        }
        let tag = match val.kind() {
            ValueKind::PyInstance(inst) => Tag::Instance(Rc::clone(inst)),
            _ => Tag::Other,
        };
        let inst_rc = match tag {
            Tag::Other => return Ok(val),
            Tag::Instance(rc) => rc,
        };
        // Int subclass: extract the backing primitive (Int or BigInt).
        if let Some(backing) = instance_builtin_data(&inst_rc) {
            if matches!(backing.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
                return Ok(backing);
            }
        }
        // Non-int-subclass: look for __index__.
        let class = Rc::clone(&inst_rc.borrow().class);
        let Some(method_val) = lookup_class_attr(&class, "__index__") else {
            // No backing and no __index__: return original; str_printf_to_int
            // will produce the correct TypeError.
            return Ok(val);
        };
        let result = invoke_class_method(
            self,
            method_val,
            Value::py_instance(Rc::clone(&inst_rc)),
            &[],
        )?;
        // CPython: if __index__ returns non-int, the printf format code falls
        // back to its standard error ("a real number is required, not Foo").
        // Return val unchanged so str_printf_to_int produces the right message.
        let is_int = matches!(
            result.kind(),
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
        );
        if is_int { Ok(result) } else { Ok(val) }
    }

    /// Coerce a `PyInstance` argument to `f64` for float printf format codes
    /// (`%e`, `%E`, `%f`, `%F`, `%g`, `%G`).
    ///
    /// Tries `__float__` first (float subclasses carry a float backing value),
    /// then `__index__` (int-like objects acceptable as float arguments).
    /// Non-`PyInstance` values are returned unchanged.
    fn coerce_printf_float_arg(&mut self, val: Value) -> Result<Value> {
        // Use a tag enum so the borrow from val.kind() ends before we move val.
        enum Tag {
            Instance(Rc<RefCell<PyInstance>>),
            Other,
        }
        let tag = match val.kind() {
            ValueKind::PyInstance(inst) => Tag::Instance(Rc::clone(inst)),
            _ => Tag::Other,
        };
        let inst_rc = match tag {
            Tag::Other => return Ok(val),
            Tag::Instance(rc) => rc,
        };
        // Float or int subclass: extract the backing primitive directly.
        if let Some(backing) = instance_builtin_data(&inst_rc) {
            if matches!(
                backing.kind(),
                ValueKind::Float(_) | ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            ) {
                return Ok(backing);
            }
        }
        let class = Rc::clone(&inst_rc.borrow().class);
        // Try __float__ first.
        if let Some(method_val) = lookup_class_attr(&class, "__float__") {
            let result = invoke_class_method(
                self,
                method_val,
                Value::py_instance(Rc::clone(&inst_rc)),
                &[],
            )?;
            enum FloatTag { Ok, Err { class_name: String, type_name: String } }
            let ftag = match result.kind() {
                ValueKind::Float(_) => FloatTag::Ok,
                _ => FloatTag::Err {
                    class_name: inst_rc.borrow().class.borrow().name.clone(),
                    type_name: value_type_name_str(&result).to_string(),
                },
            };
            return match ftag {
                FloatTag::Ok => Ok(result),
                FloatTag::Err { class_name, type_name } => Err(PyError::named(
                    "TypeError",
                    format!("{class_name}.__float__ returned non-float (type {type_name})"),
                )),
            };
        }
        // Try __index__ as fallback (CPython accepts integer-like objects for %f).
        if let Some(method_val) = lookup_class_attr(&class, "__index__") {
            let result = invoke_class_method(
                self,
                method_val,
                Value::py_instance(Rc::clone(&inst_rc)),
                &[],
            )?;
            enum IdxTag { Ok, Err(String) }
            let itag = match result.kind() {
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => IdxTag::Ok,
                _ => IdxTag::Err(value_type_name_str(&result).to_string()),
            };
            return match itag {
                IdxTag::Ok => Ok(result),
                IdxTag::Err(type_name) => Err(PyError::named(
                    "TypeError",
                    format!("__index__ returned non-int (type {type_name})"),
                )),
            };
        }
        // No __float__, no __index__: return original; str_printf_to_float
        // will produce the correct TypeError.
        Ok(val)
    }

    /// `str % args` — CPython-compatible printf-style string formatting (#1393).
    ///
    /// Handles positional (`%s`, `%d`, …) and named (`%(key)s`) format codes.
    /// The right-hand side may be a single value (implicitly a one-element
    /// positional tuple), a tuple (positional), or a dict (named lookup).
    fn str_printf_format(&mut self, fmt_val: Value, args: Value) -> Result<Value> {
        // Borrow the format string directly from the Value to avoid a heap allocation.
        // fmt_val is held by value for the duration of this function, so the &str is valid.
        let fmt: &str = match fmt_val.kind() {
            ValueKind::Str(s) => s,
            _ => unreachable!("str_printf_format called with non-str left"),
        };

        // CPython mapping mode is triggered by the format string, not by the RHS type.
        // A dict RHS is only used as a mapping when the format string contains %(key) codes;
        // if the format has only positional codes, the dict is treated as a single positional arg.
        let has_named_key = {
            let b = fmt.as_bytes();
            let mut found = false;
            let mut j = 0;
            while j + 1 < b.len() {
                if b[j] == b'%' && b[j + 1] == b'(' {
                    found = true;
                    break;
                }
                j += 1;
            }
            found
        };
        let is_mapping = has_named_key && matches!(args.kind(), ValueKind::Dict(_));
        // Wrap a non-tuple, non-mapping rhs in a virtual single-element tuple.
        // Use &[Value] to avoid cloning the tuple's items upfront; borrow from
        // args directly for the single-value case to avoid an extra clone.
        let positional: Option<&[Value]> = if is_mapping {
            None
        } else {
            match args.kind() {
                ValueKind::Tuple(items) => Some(items),
                _ => Some(std::slice::from_ref(&args)),
            }
        };
        let mut pos_idx: usize = 0;

        let mut out = String::with_capacity(fmt.len());
        let bytes = fmt.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if bytes[i] != b'%' {
                let ch = fmt[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
            i += 1; // consume '%'
            if i >= len {
                return Err(PyError::named(
                    "ValueError",
                    "incomplete format".to_string(),
                ));
            }

            // Named key: %(key)s — borrow a slice of fmt directly to avoid allocating.
            let named_key: Option<&str> = if bytes[i] == b'(' {
                i += 1;
                let start = i;
                while i < len && bytes[i] != b')' {
                    i += 1;
                }
                if i >= len {
                    return Err(PyError::named(
                        "ValueError",
                        "incomplete format key".to_string(),
                    ));
                }
                let key = &fmt[start..i];
                i += 1; // consume ')'
                Some(key)
            } else {
                None
            };

            // Flags: -, +, space, #, 0
            let mut flag_minus = false;
            let mut flag_plus = false;
            let mut flag_space = false;
            let mut flag_zero = false;
            let mut flag_hash = false;
            while i < len {
                match bytes[i] {
                    b'-' => flag_minus = true,
                    b'+' => flag_plus = true,
                    b' ' => flag_space = true,
                    b'0' => flag_zero = true,
                    b'#' => flag_hash = true,
                    _ => break,
                }
                i += 1;
            }

            // Width: integer or '*'
            let width: Option<usize> = if i < len && bytes[i] == b'*' {
                i += 1;
                let w = str_printf_take_positional(&positional, &mut pos_idx)?;
                match w.kind() {
                    ValueKind::Int(n) if n >= 0 => Some(n as usize),
                    ValueKind::Int(n) => {
                        flag_minus = true;
                        Some((-n) as usize)
                    }
                    _ => {
                        return Err(PyError::named("TypeError", "* wants int".to_string()));
                    }
                }
            } else if i < len && bytes[i].is_ascii_digit() {
                let start = i;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                Some(fmt[start..i].parse::<usize>().unwrap())
            } else {
                None
            };

            // Precision: .integer or .*
            let precision: Option<usize> = if i < len && bytes[i] == b'.' {
                i += 1;
                if i < len && bytes[i] == b'*' {
                    i += 1;
                    let p = str_printf_take_positional(&positional, &mut pos_idx)?;
                    match p.kind() {
                        ValueKind::Int(n) if n >= 0 => Some(n as usize),
                        ValueKind::Int(_) => Some(0),
                        _ => {
                            return Err(PyError::named("TypeError", "* wants int".to_string()));
                        }
                    }
                } else {
                    let start = i;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == start {
                        Some(0)
                    } else {
                        Some(fmt[start..i].parse::<usize>().unwrap())
                    }
                }
            } else {
                None
            };

            // Length modifier: h, l, L — ignored (CPython ignores them too).
            if i < len && matches!(bytes[i], b'h' | b'l' | b'L') {
                i += 1;
            }

            if i >= len {
                return Err(PyError::named(
                    "ValueError",
                    "incomplete format".to_string(),
                ));
            }
            let conv = bytes[i] as char;
            i += 1;

            // %% — literal percent, no argument consumed.
            if conv == '%' {
                out.push('%');
                continue;
            }

            // Get the argument value.
            let arg: Value = if let Some(key) = named_key {
                if is_mapping {
                    match args.kind() {
                        ValueKind::Dict(d) => {
                            let k = PyKey::Str(intern_string(key));
                            match d.get(&k) {
                                Some(v) => v.clone(),
                                None => {
                                    return Err(PyError::key_error(Value::string(key)));
                                }
                            }
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                "format requires a mapping".to_string(),
                            ));
                        }
                    }
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        "format requires a mapping".to_string(),
                    ));
                }
            } else {
                str_printf_take_positional(&positional, &mut pos_idx)?
            };

            // Format the argument according to the conversion code.
            let formatted: String = match conv {
                's' => apply_str_precision(self.render_value_as_str(&arg)?, precision),
                'r' => apply_str_precision(render_instance_repr(self, &arg)?, precision),
                'd' | 'i' | 'u' => {
                    let coerced_int = self.coerce_printf_int_arg(arg)?;
                    match str_printf_to_int(&coerced_int, conv)? {
                        PrintfInt::Small(n) => {
                            if n < 0 {
                                format!("{}", n)
                            } else if flag_plus {
                                format!("+{}", n)
                            } else if flag_space {
                                format!(" {}", n)
                            } else {
                                format!("{}", n)
                            }
                        }
                        PrintfInt::Big(b) => {
                            // to_str_radix(10) includes the '-' sign for negatives.
                            let mut s = b.to_str_radix(10);
                            if !s.starts_with('-') && flag_plus {
                                s.insert(0, '+');
                            } else if !s.starts_with('-') && flag_space {
                                s.insert(0, ' ');
                            }
                            s
                        }
                    }
                }
                'o' => {
                    let coerced_int = self.coerce_printf_int_arg(arg)?;
                    match str_printf_to_int(&coerced_int, conv)? {
                        PrintfInt::Small(n) => {
                            if n < 0 {
                                // CPython uses sign-magnitude (not two's complement) for negative octal.
                                let u = (n as u64).wrapping_neg();
                                if flag_hash {
                                    format!("-0o{:o}", u)
                                } else {
                                    format!("-{:o}", u)
                                }
                            } else if flag_hash {
                                // CPython applies 0o prefix for all values (including 0) when # is set.
                                if flag_plus {
                                    format!("+0o{:o}", n)
                                } else if flag_space {
                                    format!(" 0o{:o}", n)
                                } else {
                                    format!("0o{:o}", n)
                                }
                            } else if flag_plus {
                                format!("+{:o}", n)
                            } else if flag_space {
                                format!(" {:o}", n)
                            } else {
                                format!("{:o}", n)
                            }
                        }
                        PrintfInt::Big(b) => format_printf_bigint_radix(
                            &b, 8, "0o", false, flag_hash, flag_plus, flag_space,
                        ),
                    }
                }
                'x' => {
                    let coerced_int = self.coerce_printf_int_arg(arg)?;
                    match str_printf_to_int(&coerced_int, conv)? {
                        PrintfInt::Small(n) => {
                            if n < 0 {
                                let u = (n as u64).wrapping_neg();
                                if flag_hash {
                                    format!("-0x{:x}", u)
                                } else {
                                    format!("-{:x}", u)
                                }
                            } else if flag_hash {
                                // CPython applies 0x prefix for all values (including 0) when # is set.
                                if flag_plus {
                                    format!("+0x{:x}", n)
                                } else if flag_space {
                                    format!(" 0x{:x}", n)
                                } else {
                                    format!("0x{:x}", n)
                                }
                            } else if flag_plus {
                                format!("+{:x}", n)
                            } else if flag_space {
                                format!(" {:x}", n)
                            } else {
                                format!("{:x}", n)
                            }
                        }
                        PrintfInt::Big(b) => format_printf_bigint_radix(
                            &b, 16, "0x", false, flag_hash, flag_plus, flag_space,
                        ),
                    }
                }
                'X' => {
                    let coerced_int = self.coerce_printf_int_arg(arg)?;
                    match str_printf_to_int(&coerced_int, conv)? {
                        PrintfInt::Small(n) => {
                            if n < 0 {
                                let u = (n as u64).wrapping_neg();
                                if flag_hash {
                                    format!("-0X{:X}", u)
                                } else {
                                    format!("-{:X}", u)
                                }
                            } else if flag_hash {
                                // CPython applies 0X prefix for all values (including 0) when # is set.
                                if flag_plus {
                                    format!("+0X{:X}", n)
                                } else if flag_space {
                                    format!(" 0X{:X}", n)
                                } else {
                                    format!("0X{:X}", n)
                                }
                            } else if flag_plus {
                                format!("+{:X}", n)
                            } else if flag_space {
                                format!(" {:X}", n)
                            } else {
                                format!("{:X}", n)
                            }
                        }
                        PrintfInt::Big(b) => format_printf_bigint_radix(
                            &b, 16, "0X", true, flag_hash, flag_plus, flag_space,
                        ),
                    }
                }
                'e' | 'E' => {
                    let coerced_float = self.coerce_printf_float_arg(arg)?;
                    let f = str_printf_to_float(&coerced_float, conv)?;
                    let prec = precision.unwrap_or(6);
                    let mut s = format_scientific(f, prec, conv == 'E');
                    if f.is_sign_positive() && flag_plus {
                        s.insert(0, '+');
                    } else if f.is_sign_positive() && flag_space {
                        s.insert(0, ' ');
                    }
                    s
                }
                'f' | 'F' => {
                    let coerced_float = self.coerce_printf_float_arg(arg)?;
                    let f = str_printf_to_float(&coerced_float, conv)?;
                    let upper = conv == 'F';
                    // Special-case NaN and Inf before calling format!(), which
                    // produces Rust-style 'NaN'/'inf' rather than CPython-style
                    // 'nan'/'inf'/'NAN'/'INF'.
                    let mut s = if f.is_nan() {
                        if upper { "NAN".to_string() } else { "nan".to_string() }
                    } else if f.is_infinite() {
                        if f > 0.0 {
                            if upper { "INF".to_string() } else { "inf".to_string() }
                        } else if upper {
                            "-INF".to_string()
                        } else {
                            "-inf".to_string()
                        }
                    } else {
                        let prec = precision.unwrap_or(6);
                        format!("{:.prec$}", f, prec = prec)
                    };
                    if f.is_sign_positive() && flag_plus {
                        s.insert(0, '+');
                    } else if f.is_sign_positive() && flag_space {
                        s.insert(0, ' ');
                    }
                    s
                }
                'g' | 'G' => {
                    let coerced_float = self.coerce_printf_float_arg(arg)?;
                    let f = str_printf_to_float(&coerced_float, conv)?;
                    let prec = precision.unwrap_or(6).max(1);
                    let mut s = format_general_float(f, prec, conv == 'G');
                    if f.is_sign_positive() && flag_plus {
                        s.insert(0, '+');
                    } else if f.is_sign_positive() && flag_space {
                        s.insert(0, ' ');
                    }
                    s
                }
                'c' => {
                    // Coerce int subclasses and __index__ objects the same way
                    // as %d/%x etc.  If __index__ returns non-int, we fall back
                    // to the original value so the match below emits the correct
                    // "%c requires int or char" TypeError.
                    let coerced_char = self.coerce_printf_int_arg(arg)?;
                    match coerced_char.kind() {
                        ValueKind::Str(s) => {
                            let mut cs = s.chars();
                            let c = cs.next().ok_or_else(|| {
                                PyError::named("TypeError", "%c requires int or char".to_string())
                            })?;
                            if cs.next().is_some() {
                                return Err(PyError::named(
                                    "TypeError",
                                    "%c requires a single character".to_string(),
                                ));
                            }
                            c.to_string()
                        }
                        ValueKind::Int(n) => char::from_u32(n as u32)
                            .ok_or_else(|| {
                                PyError::named(
                                    "OverflowError",
                                    "%c arg not in range(0x110000)".to_string(),
                                )
                            })?
                            .to_string(),
                        ValueKind::Bool(b) => char::from_u32(b as u32)
                            .ok_or_else(|| {
                                PyError::named(
                                    "OverflowError",
                                    "%c arg not in range(0x110000)".to_string(),
                                )
                            })?
                            .to_string(),
                        ValueKind::BigInt(b) => {
                            // A BigInt may be in range [0, 0x10ffff] or not.
                            use crate::value::PyToPrimitive;
                            let n = b.to_u32();
                            let c = n
                                .and_then(char::from_u32)
                                .ok_or_else(|| {
                                    PyError::named(
                                        "OverflowError",
                                        "%c arg not in range(0x110000)".to_string(),
                                    )
                                })?;
                            c.to_string()
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                "%c requires int or char".to_string(),
                            ));
                        }
                    }
                }
                _ => {
                    return Err(PyError::named(
                        "ValueError",
                        format!(
                            "unsupported format character '{}' (0x{:02x}) at index {}",
                            conv,
                            conv as u32,
                            i - 1
                        ),
                    ));
                }
            };

            // Apply width and alignment.
            let padded = apply_printf_width(formatted, width, flag_minus, flag_zero, conv);
            out.push_str(&padded);
        }

        // Unconsumed positional arguments: raise TypeError.
        if let Some(pos) = positional {
            if pos_idx < pos.len() {
                return Err(PyError::named(
                    "TypeError",
                    "not all arguments converted during string formatting".to_string(),
                ));
            }
        }

        Ok(Value::string(out))
    }

}

/// Take the next positional argument for printf-style formatting.
fn str_printf_take_positional(positional: &Option<&[Value]>, idx: &mut usize) -> Result<Value> {
    match positional {
        None => Err(PyError::named(
            "TypeError",
            "not enough arguments for format string".to_string(),
        )),
        Some(items) => {
            if *idx >= items.len() {
                Err(PyError::named(
                    "TypeError",
                    "not enough arguments for format string".to_string(),
                ))
            } else {
                let v = items[*idx].clone();
                *idx += 1;
                Ok(v)
            }
        }
    }
}

/// Result of coercing a printf argument to an integer value.
///
/// `Small` covers values that fit in `i64` (the common case: `int`, `bool`,
/// truncated `float`).  `Big` is used only for `BigInt` values that are
/// outside the `i64` range — the caller formats them with BigInt-native
/// methods (`to_str_radix`, etc.) instead of Rust integer formatting.
enum PrintfInt {
    Small(i64),
    Big(PyBigInt),
}

/// Convert a `Value` to a `PrintfInt` for integer printf format codes.
///
/// Unlike the old `i64`-returning version, the `BigInt` arm no longer raises
/// `OverflowError`; it returns `PrintfInt::Big` so that the caller can format
/// arbitrarily large integers using BigInt-native methods.
///
/// For `%d`/`%i`/`%u`, float arguments are truncated toward zero following
/// CPython's `int(float)` semantics: NaN raises `ValueError`, infinity raises
/// `OverflowError`, and finite floats larger than `i64::MAX` are promoted to
/// `PrintfInt::Big` rather than being silently clamped.
fn str_printf_to_int(v: &Value, conv: char) -> Result<PrintfInt> {
    match v.kind() {
        ValueKind::Int(n) => Ok(PrintfInt::Small(n)),
        ValueKind::Bool(b) => Ok(PrintfInt::Small(b as i64)),
        ValueKind::Float(_) if matches!(conv, 'o' | 'x' | 'X') => {
            // CPython 3.12: %o/%x/%X reject float with "an integer is required".
            // %d/%i/%u accept float (truncating toward zero) for historical reasons.
            Err(PyError::named(
                "TypeError",
                format!(
                    "%{conv} format: an integer is required, not float"
                ),
            ))
        }
        ValueKind::Float(f) => {
            // CPython converts via PyLong_FromDouble: NaN → ValueError,
            // infinity → OverflowError, finite → truncate toward zero.
            // Rust's `f as i64` silently saturates at i64::MAX/MIN for
            // out-of-range finite floats, losing significant digits.
            let int_val = float_to_bigint(f)?;
            match int_val.kind() {
                ValueKind::Int(n) => Ok(PrintfInt::Small(n)),
                ValueKind::BigInt(b) => Ok(PrintfInt::Big(b.clone())),
                _ => unreachable!("float_to_bigint returns Int or BigInt"),
            }
        }
        ValueKind::BigInt(b) => Ok(PrintfInt::Big(b.clone())),
        _ => {
            // CPython uses "a real number is required" for %d/%i/%u,
            // and "an integer is required" for %o/%x/%X.
            let msg = if matches!(conv, 'o' | 'x' | 'X') {
                format!(
                    "%{conv} format: an integer is required, not {}",
                    pyrust_core::builtin_type_name(v)
                )
            } else {
                format!(
                    "%{conv} format: a real number is required, not {}",
                    pyrust_core::builtin_type_name(v)
                )
            };
            Err(PyError::named("TypeError", msg))
        }
    }
}

/// Format a `BigInt` value for `%o`/`%x`/`%X` printf codes.
///
/// `to_str_radix` produces sign-magnitude notation (e.g., `-ff` for `-255`),
/// which matches CPython's behaviour.  This helper inserts the optional base
/// prefix (`0o`/`0x`/`0X`) and sign prefix (`+` / ` `) in the positions that
/// `apply_printf_width` expects for correct zero-fill later.
fn format_printf_bigint_radix(
    b: &PyBigInt,
    radix: u32,
    base_prefix: &str,
    upper: bool,
    flag_hash: bool,
    flag_plus: bool,
    flag_space: bool,
) -> String {
    // num_bigint::BigInt::to_str_radix uses sign-magnitude: negative values
    // get a leading '-'; the remaining digits are the absolute magnitude.
    let raw = b.to_str_radix(radix);
    let is_neg = raw.starts_with('-');
    let digits: std::borrow::Cow<str> = if upper {
        let d = if is_neg { &raw[1..] } else { &raw[..] };
        std::borrow::Cow::Owned(d.to_uppercase())
    } else if is_neg {
        std::borrow::Cow::Borrowed(&raw[1..])
    } else {
        std::borrow::Cow::Borrowed(&raw[..])
    };
    if is_neg {
        if flag_hash {
            format!("-{}{}", base_prefix, digits)
        } else {
            format!("-{}", digits)
        }
    } else if flag_hash {
        if flag_plus {
            format!("+{}{}", base_prefix, digits)
        } else if flag_space {
            format!(" {}{}", base_prefix, digits)
        } else {
            format!("{}{}", base_prefix, digits)
        }
    } else if flag_plus {
        format!("+{}", digits)
    } else if flag_space {
        format!(" {}", digits)
    } else {
        digits.into_owned()
    }
}

/// Convert a `Value` to `f64` for float printf format codes.
fn str_printf_to_float(v: &Value, _conv: char) -> Result<f64> {
    match v.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(n) => Ok(n as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        ValueKind::BigInt(b) => bigint_to_float_or_overflow(b),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "must be real number, not {}",
                pyrust_core::builtin_type_name(v)
            ),
        )),
    }
}

/// Truncate a string to `precision` Unicode chars (for `%s` and `%r`).
fn apply_str_precision(s: String, precision: Option<usize>) -> String {
    match precision {
        None => s,
        Some(max_chars) => {
            if s.chars().count() <= max_chars {
                s
            } else {
                s.chars().take(max_chars).collect()
            }
        }
    }
}

/// Apply width padding to a formatted value string.
fn apply_printf_width(
    s: String,
    width: Option<usize>,
    left_align: bool,
    zero_fill: bool,
    conv: char,
) -> String {
    let w = match width {
        None | Some(0) => return s,
        Some(w) => w,
    };
    let char_len = s.chars().count();
    if char_len >= w {
        return s;
    }
    let pad = w - char_len;
    // Zero-fill only for numeric codes, not %s/%r/%c, and not with left-align.
    if zero_fill
        && !left_align
        && matches!(
            conv,
            'd' | 'i' | 'u' | 'o' | 'x' | 'X' | 'f' | 'e' | 'E' | 'g' | 'G'
        )
    {
        // Determine the non-digit prefix: optional sign (+/-/space), then
        // optional base prefix (0x, 0X, 0o).  Zeros are inserted after the
        // full prefix so that "%#010x" % 255 → "0x000000ff" not "0000000xff".
        let prefix_len = {
            let mut cs = s.chars();
            let mut n = 0usize;
            // sign
            if let Some('+' | '-' | ' ') = cs.next() {
                n += 1;
                // base prefix after sign: 0x, 0X, 0o
                let mut peek = s[n..].chars();
                if peek.next() == Some('0') {
                    if matches!(peek.next(), Some('x' | 'X' | 'o')) {
                        n += 2;
                    }
                }
            } else if s.starts_with("0x") || s.starts_with("0X") || s.starts_with("0o") {
                n = 2;
            }
            n
        };
        let mut out = String::with_capacity(w);
        out.push_str(&s[..prefix_len]);
        for _ in 0..pad {
            out.push('0');
        }
        out.push_str(&s[prefix_len..]);
        return out;
    }
    if left_align {
        let mut out = s;
        for _ in 0..pad {
            out.push(' ');
        }
        out
    } else {
        let mut out = String::with_capacity(w);
        for _ in 0..pad {
            out.push(' ');
        }
        out.push_str(&s);
        out
    }
}

/// Format a float in scientific notation matching CPython's `%e`/`%E`.
///
/// CPython always uses a sign and at least two exponent digits: e+03, e-03.
/// Rust's default format may omit the sign for positive exponents; this
/// function normalises the output to match CPython.
fn format_scientific(f: f64, prec: usize, upper: bool) -> String {
    if f.is_nan() {
        return if upper { "NAN".to_string() } else { "nan".to_string() };
    }
    if f.is_infinite() {
        return if f > 0.0 {
            if upper { "INF".to_string() } else { "inf".to_string() }
        } else if upper {
            "-INF".to_string()
        } else {
            "-inf".to_string()
        };
    }
    let raw = format!("{:.prec$e}", f, prec = prec);
    let e_char = if upper { 'E' } else { 'e' };
    if let Some(pos) = raw.find('e') {
        let mantissa = &raw[..pos];
        let exp_str = &raw[pos + 1..];
        let digits = if exp_str.starts_with(['-', '+']) {
            &exp_str[1..]
        } else {
            exp_str
        };
        let sign: i32 = if exp_str.starts_with('-') { -1 } else { 1 };
        let exp_n: i32 = digits.parse::<i32>().unwrap_or(0) * sign;
        // {:+03} produces "+03", "-03" — sign always included, at least 2 digits.
        format!("{}{}{:+03}", mantissa, e_char, exp_n)
    } else {
        raw
    }
}

/// Format a float in "general" notation matching CPython's `%g`/`%G`.
///
/// Uses scientific notation when exp < -4 or exp >= prec; fixed otherwise.
/// Trailing zeros are stripped in both cases.
fn format_general_float(f: f64, prec: usize, upper: bool) -> String {
    if f.is_nan() {
        return if upper { "NAN".to_string() } else { "nan".to_string() };
    }
    if f.is_infinite() {
        return if f > 0.0 {
            if upper { "INF".to_string() } else { "inf".to_string() }
        } else if upper {
            "-INF".to_string()
        } else {
            "-inf".to_string()
        };
    }
    let abs_f = f.abs();
    let use_exp = abs_f != 0.0 && {
        let exp = abs_f.log10().floor() as i64;
        exp < -4 || exp >= prec as i64
    };
    if use_exp {
        let raw = format_scientific(f, prec.saturating_sub(1), upper);
        strip_trailing_zeros_exp(&raw)
    } else {
        let exp = if abs_f == 0.0 {
            0i64
        } else {
            abs_f.log10().floor() as i64
        };
        let decimal_places = (prec as i64 - 1 - exp).max(0) as usize;
        let raw = format!("{:.prec$}", f, prec = decimal_places);
        strip_trailing_zeros_fixed(&raw)
    }
}

fn strip_trailing_zeros_fixed(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let s = s.trim_end_matches('0');
    s.trim_end_matches('.').to_string()
}

fn strip_trailing_zeros_exp(s: &str) -> String {
    let e_pos = s.find('e').or_else(|| s.find('E'));
    match e_pos {
        None => strip_trailing_zeros_fixed(s),
        Some(pos) => {
            let mantissa = &s[..pos];
            let exp_part = &s[pos..];
            format!("{}{}", strip_trailing_zeros_fixed(mantissa), exp_part)
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

pub(crate) fn coerce_numeric(v: &Value) -> Value {
    // Extract via kind() in a scope so the borrow is dropped before we
    // clone `v` in the fallthrough — #450 made `kind()`'s borrow
    // explicit, so we can't hold a borrow while returning an owned Value.
    if let ValueKind::Bool(b) = v.kind() {
        return Value::int(b as i64);
    }
    // Issue #1204: PyInstance subclasses of int/float/str/bytes carry their
    // underlying primitive value as `__builtin_data__`.  Extract it here so
    // that arithmetic and concatenation operations on bare subclass instances
    // (e.g. `MyInt(42) + 1`) fall through to the primitive fast paths below.
    // This mirrors CPython's slot delegation for `tp_as_number` / `tp_as_sequence`.
    if let Some(inst_rc) = v.as_py_instance_rc() {
        if let Some(backing) = instance_builtin_data(inst_rc) {
            let is_scalar = matches!(
                backing.kind(),
                ValueKind::Int(_)
                    | ValueKind::BigInt(_)
                    | ValueKind::Float(_)
                    | ValueKind::Str(_)
                    | ValueKind::Bytes(_)
            );
            if is_scalar {
                return backing;
            }
        }
    }
    v.clone()
}

pub(crate) fn iter_values(value: &Value) -> Result<Vec<Value>> {
    // list/dict/set subclass: delegate to the backing primitive value.
    if let Some(inst_rc) = value.as_py_instance_rc() {
        if let Some(backing) = instance_builtin_data(inst_rc) {
            return iter_values(&backing);
        }
    }
    match value.kind() {
        ValueKind::List(items) => Ok(items.to_vec()),
        ValueKind::Tuple(items) => Ok(items.to_vec()),
        ValueKind::Set(items) => Ok(items.iter().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::BuiltinObject { .. } => {
            // Frozensets materialise through their inner key set; dict views
            // materialise through their backing IndexMap; everything else
            // iterates via `iter_next`.
            // Bytearray: materialise as integers (same shape as bytes iteration).
            if let Some(elems) = pyrust_builtins::bytearray::iter_elements(&value) {
                return Ok(elems);
            }
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

/// Resolve a built-in name to its `Value` for use as a `LoadGlobal` fallback.
///
/// The 11 primitive type names (`int`, `str`, `list`, …) resolve to the
/// per-thread `PyClass` singletons from `primitive_class_by_name` — see
/// issue #462.  `bool` resolves to a class whose `base` chains to `int`,
/// so `bool.__bases__ == (int,)` matches CPython.  These cannot go through
/// the generic registry path because `isinstance`/`issubclass` require a
/// `ValueKind::PyClass`, not a `ValueKind::BuiltinFunction`.
///
/// `NotImplemented` is a singleton constant, not a callable; it is
/// returned directly without a registry lookup.
///
/// All other names are resolved through `builtin_registry::lookup_name`,
/// which returns the interned `&'static str` stored in the registry entry
/// so `Value::builtin_function` never needs to heap-allocate a new name.
/// Adding a `fn foo(…)` to a `pyrust_module!` body automatically makes
/// `foo()` reachable via bare-name `LoadGlobal` with no edits here.
pub(crate) fn resolve_builtin(name: &str) -> Option<Value> {
    // Primitive types: must remain `Value::py_class` so that
    // `isinstance(x, int)` and `type(x) is int` work correctly (#462).
    if matches!(
        name,
        "bool" | "bytearray" | "bytes" | "complex" | "dict" | "float" | "frozenset"
            | "int"
            | "list"
            | "range"
            | "set"
            | "str"
            | "tuple"
    ) {
        return primitive_class_by_name(name).map(Value::py_class);
    }
    if name == "object" {
        return Some(Value::py_class(object_class_singleton()));
    }
    // `type` is the metaclass — must resolve to a PyClass singleton so that
    // `type is type`, `builtins.type is type`, and `repr(type)` all behave
    // as CPython 3.12 (issue #1312).
    if name == "type" {
        return Some(Value::py_class(type_class_singleton()));
    }
    // Singleton constants that are not callable.
    if name == "NotImplemented" {
        return Some(Value::not_implemented());
    }
    if name == "Ellipsis" {
        return Some(Value::ellipsis());
    }
    // Built-in exception classes — resolved lazily via `EXC_CLASS_CACHE`
    // (built once per thread on first access).  Exception classes are no
    // longer pre-inserted into the module env at startup; scripts that
    // never reference an exception class name pay zero class-build cost.
    if matches!(
        name,
        "ArithmeticError"
            | "AssertionError"
            | "AttributeError"
            | "BaseException"
            | "BaseExceptionGroup"
            | "BlockingIOError"
            | "BrokenPipeError"
            | "BufferError"
            | "BytesWarning"
            | "ChildProcessError"
            | "ConnectionAbortedError"
            | "ConnectionError"
            | "ConnectionRefusedError"
            | "ConnectionResetError"
            | "DeprecationWarning"
            | "EOFError"
            | "EncodingWarning"
            | "Exception"
            | "ExceptionGroup"
            | "FileExistsError"
            | "FileNotFoundError"
            | "FloatingPointError"
            | "FutureWarning"
            | "GeneratorExit"
            | "ImportError"
            | "ImportWarning"
            | "IndentationError"
            | "IndexError"
            | "InterruptedError"
            | "IsADirectoryError"
            | "KeyError"
            | "KeyboardInterrupt"
            | "LookupError"
            | "MemoryError"
            | "ModuleNotFoundError"
            | "NameError"
            | "NotADirectoryError"
            | "NotImplementedError"
            | "EnvironmentError"
            | "IOError"
            | "OSError"
            | "OverflowError"
            | "PendingDeprecationWarning"
            | "PermissionError"
            | "ProcessLookupError"
            | "RecursionError"
            | "ReferenceError"
            | "ResourceWarning"
            | "RuntimeError"
            | "RuntimeWarning"
            | "StopAsyncIteration"
            | "StopIteration"
            | "SyntaxError"
            | "SyntaxWarning"
            | "SystemError"
            | "SystemExit"
            | "TabError"
            | "TimeoutError"
            | "TypeError"
            | "UnboundLocalError"
            | "UnicodeDecodeError"
            | "UnicodeEncodeError"
            | "UnicodeError"
            | "UnicodeTranslateError"
            | "UnicodeWarning"
            | "UserWarning"
            | "ValueError"
            | "Warning"
            | "ZeroDivisionError"
    ) {
        return lookup_exc_class(name)
            .map(pyrust_core::Value::py_class);
    }
    // All registered flat-namespace builtins (`print`, `len`, `abs`, …).
    // lookup_name returns the interned &'static str already stored in the
    // registry entry, so Value::builtin_function needs no extra allocation.
    crate::builtin_registry::lookup_name(name).map(Value::builtin_function)
}

/// Operation tag for set/frozenset binary operators.
#[derive(Clone, Copy)]
enum SetOp {
    Or,  // union
    And, // intersection
    Sub, // difference
    Xor, // symmetric difference
}

/// Extract key-value pairs from a plain `dict` or a `PyInstance` dict
/// subclass backed by a dict.  Returns `None` for any other type.
/// Used by the PEP 584 `dict | dict` merge path in `eval_binary`.
fn dict_entries_from_value(v: &Value) -> Option<Vec<(PyKey, Value)>> {
    if let Some(entries) = v.dict_with(|d| {
        d.iter().map(|(k, val)| (k.clone(), val.clone())).collect::<Vec<_>>()
    }) {
        return Some(entries);
    }
    if let Some(inst_rc) = v.as_py_instance_rc() {
        if let Some(backing) = instance_builtin_data(inst_rc) {
            return dict_entries_from_value(&backing);
        }
    }
    None
}


/// Extract a set's items and frozen flag from a value that is a `set`,
/// `frozenset`, or a `PyInstance` subclass backed by either.  Returns
/// `None` when the value is none of those.
fn set_items_from_value(v: &Value) -> Option<(indexmap::IndexSet<PyKey>, bool)> {
    if let ValueKind::Set(s) = v.kind() {
        return Some((s.clone(), false));
    }
    if let Some(rc) = pyrust_builtins::frozenset::as_items(v) {
        return Some(((*rc).clone(), true));
    }
    if let Some(inst_rc) = v.as_py_instance_rc() {
        if let Some(backing) = instance_builtin_data(inst_rc) {
            return set_items_from_value(&backing);
        }
    }
    None
}

/// Compute a binary set operation when both operands are set/frozenset (or
/// `PyInstance` subclasses thereof).  Returns `Set` if both backing stores are
/// mutable sets, otherwise `FrozenSet` (any frozenset operand promotes the
/// result, matching CPython).
///
/// Returns `None` when the left operand is not a set/frozenset (caller should
/// fall through to the next handler).  Returns `Some(Err(...))` when the left
/// operand is a set/frozenset but the right operand is not — CPython raises
/// `TypeError: unsupported operand type(s) for OP: 'X' and 'Y'` in that case.
fn set_binary_op(left: &Value, right: &Value, op: SetOp, op_sym: &str) -> Option<Result<Value>> {
    let lhs_items = set_items_from_value(left)?;
    // LHS is a set/frozenset; if RHS is not, emit the CPython-format TypeError.
    let Some(rhs_items) = set_items_from_value(right) else {
        let lt = value_type_name_str(left);
        let rt = value_type_name_str(right);
        return Some(Err(PyError::named(
            "TypeError",
            format!("unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"),
        )));
    };
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

/// Set/frozenset subset-relation comparison.
///
/// Returns `Some(Ok(bool))` when both `left` and `right` are set/frozenset
/// (or subclasses thereof), `None` otherwise (caller should fall through to
/// a TypeError).
///
/// Semantics match CPython 3.12:
/// - `a < b`  — proper subset: every element of `a` is in `b` and `a != b`
/// - `a <= b` — subset: every element of `a` is in `b`
/// - `a > b`  — proper superset: every element of `b` is in `a` and `a != b`
/// - `a >= b` — superset: every element of `b` is in `a`
///
/// Mixed `set`/`frozenset` comparisons are supported, as in CPython.
fn set_subset_cmp(left: &Value, right: &Value, op: BinaryOp) -> Option<Result<Value>> {
    let (a, _) = set_items_from_value(left)?;
    let (b, _) = set_items_from_value(right)?;
    let is_subset = a.iter().all(|k| b.contains(k));
    let is_superset = b.iter().all(|k| a.contains(k));
    let result = match op {
        BinaryOp::Lt => is_subset && !is_superset,
        BinaryOp::Le => is_subset,
        BinaryOp::Gt => is_superset && !is_subset,
        BinaryOp::Ge => is_superset,
        _ => unreachable!("set_subset_cmp called with non-comparison op"),
    };
    Some(Ok(Value::bool_(result)))
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

/// True if `v` can serve as an operand in `X | Y` (PEP 604).
/// Valid operands: `PyClass`, `BuiltinFunction` type tokens (like `range`, `generator`),
/// `None` itself (coerced to the `NoneType` PyClass singleton), and existing `UnionType` values.
fn is_union_operand(v: &Value) -> bool {
    match v.kind() {
        ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_) | ValueKind::None => true,
        ValueKind::BuiltinObject { ops, .. } => {
            ops.type_name() == pyrust_builtins::union_type::TYPE_NAME
        }
        _ => false,
    }
}

/// True if `v` is a "type-like" PEP 604 operand — a `PyClass`, a `BuiltinFunction`
/// acting as a type token, or an existing `UnionType`.  `None` is excluded: it
/// can appear in a union *only* when the other operand is a type, so at least
/// one side must satisfy this stricter predicate.  This matches CPython's
/// behaviour where `None | None` raises TypeError but `int | None` succeeds
/// (dispatched through `type.__or__`).
fn is_strict_type_union_operand(v: &Value) -> bool {
    match v.kind() {
        ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_) => true,
        ValueKind::BuiltinObject { ops, .. } => {
            ops.type_name() == pyrust_builtins::union_type::TYPE_NAME
        }
        _ => false,
    }
}

/// Convert `None` to the `NoneType` PyClass singleton, leaving all other
/// values unchanged.  Used when assembling union components so that
/// `int | None` stores `NoneType` as the component (matching CPython).
fn coerce_none_to_nonetype(v: Value) -> Value {
    if v.is_none() {
        Value::py_class(crate::interpreter::primitive_class_by_name("NoneType").expect("NoneType singleton"))
    } else {
        v
    }
}
