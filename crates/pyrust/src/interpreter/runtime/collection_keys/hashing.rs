/// Compute the Python hash of a `Value`. Mirrors
/// CPython's semantics:
/// - numeric types use their integer value (so `hash(True) == hash(1)`
///   and `hash(1.0) == hash(1)`);
/// - strings use an FNV-1a-style byte hash;
/// - tuples use the CPython 3.12 xxHash-based formula (issue #892);
/// - mutable containers (list / dict / set) raise `TypeError`.
fn hash_value(value: &Value) -> Result<i64> {
    match value.kind() {
        // ValueKind::Int arrives here for values in [-2^47, 2^47-1] (inline i48)
        // *and* for Opaque::PyBigInt values that happen to fit in i64 (the `kind()`
        // accessor promotes them).  Both need the full Mersenne reduction so that
        // e.g. hash(2**62) and hash(-1) match CPython.
        ValueKind::Int(v) => Ok(int_hash(v)),
        // bool: True==1, False==0 — both well within (-M, M), so int_hash is a
        // no-op for the reduction, but the -1→-2 remap can never fire here either.
        ValueKind::Bool(b) => Ok(b as i64),
        // BigInt arrives only when the value doesn't fit in i64 (|n| > i64::MAX).
        ValueKind::BigInt(n) => Ok(bigint_hash(n)),
        ValueKind::Float(v) => Ok(py_hash_float(v)),
        ValueKind::Str(s) => {
            let mut h: u64 = 14695981039346656037u64;
            for b in s.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(1099511628211u64);
            }
            Ok(h as i64)
        }
        ValueKind::Bytes(rc) => {
            // FNV-1a over the raw byte content, matching PyKey::Bytes hashing
            // so that py_hash_pykey(v.to_key()) == hash(v) for bytes values.
            let mut h: u64 = 14695981039346656037u64;
            for b in rc.iter() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211u64);
            }
            let result = h as i64;
            Ok(if result == -1 { -2 } else { result })
        }
        ValueKind::None => Ok(pyrust_core::py_hash_none()),
        ValueKind::NotImplemented => Ok(pyrust_core::py_hash_not_implemented()),
        ValueKind::Ellipsis => Ok(pyrust_core::py_hash_ellipsis()),
        ValueKind::Tuple(items) => tuple_hash_cpython(items.iter().map(hash_value)),
        ValueKind::List(_) => Err(PyError::named(
            "TypeError",
            "unhashable type: 'list'".to_string(),
        )),
        ValueKind::Dict(_) => Err(PyError::named(
            "TypeError",
            "unhashable type: 'dict'".to_string(),
        )),
        ValueKind::Set(_) => Err(PyError::named(
            "TypeError",
            "unhashable type: 'set'".to_string(),
        )),
        // Range: mirrors CPython Objects/rangeobject.c range_hash (Ubuntu 3.12.3).
        //
        // CPython always builds a 3-element tuple (length, a, b) and hashes it:
        //   len == 0 -> hash((len, None, None))
        //   len == 1 -> hash((len, start, None))
        //   len  > 1 -> hash((len, start, step))
        //
        // `py_hash_int` applies the Mersenne-prime reduction and -1→-2 remap for
        // integer components; `py_hash_none` mirrors CPython's pointer-based
        // `hash(None)` using a stable per-process static address.
        ValueKind::Range { start, stop, step } => {
            let len = range_len(start, stop, step);
            // Keep the ordinary range hash allocation-free. i64 bounds can
            // still describe up to 2**64-1 elements, though, so only the rare
            // wide length uses the arbitrary-precision integer hash path.
            let h_len = i64::try_from(len).map_or_else(
                |_| py_hash_bigint(&pyrust_core::PyBigInt::from(len)),
                py_hash_int,
            );
            let h_none = pyrust_core::py_hash_none();
            tuple_hash_cpython(
                [
                    Ok(h_len),
                    Ok(if len >= 1 { py_hash_int(start) } else { h_none }),
                    Ok(if len >= 2 { py_hash_int(step) } else { h_none }),
                ]
                .into_iter(),
            )
        }
        // Arbitrary-precision range (#2118): same tuple(len, start, step) hash as
        // the i64 case, computed via the BigInt-aware integer hash helper so the
        // big start/step components reduce correctly.  `len` itself is reduced as
        // a BigInt because it may exceed i64.
        ValueKind::BigRange { start, stop, step } => {
            let len = pyrust_core::bigrange_len(start, stop, step);
            let one = pyrust_core::PyBigInt::from(1);
            let two = pyrust_core::PyBigInt::from(2);
            let h_len = py_hash_bigint(&len);
            let h_none = pyrust_core::py_hash_none();
            tuple_hash_cpython(
                [
                    Ok(h_len),
                    Ok(if len >= one {
                        py_hash_bigint(start)
                    } else {
                        h_none
                    }),
                    Ok(if len >= two {
                        py_hash_bigint(step)
                    } else {
                        h_none
                    }),
                ]
                .into_iter(),
            )
        }
        // BuiltinObject: probe the BuiltinTypeOps hash hook (added in PR #781).
        // Types that override BuiltinTypeOps::hash (e.g. frozenset) return
        // Some(u64); anything that leaves it at the default None is unhashable.
        // Note: slice is intercepted before this match in hash_value_with_interp;
        // reaching this arm for a slice correctly returns None (unhashable) because
        // SliceOps::hash was removed in PR #850.
        ValueKind::BuiltinObject { ops, state } => match ops.hash(state) {
            Some(h) => Ok(h as i64),
            None => {
                let type_name = ops.display_error_name_for(state);
                Err(PyError::named(
                    "TypeError",
                    format!("unhashable type: '{type_name}'"),
                ))
            }
        },
        // PyInstance arriving here means either the caller didn't intercept
        // it for __hash__ dispatch (e.g. a tuple element), or no __hash__
        // method exists.  Use the actual class name rather than the generic
        // "object" returned by builtin_type_name.
        ValueKind::PyInstance(_) => {
            let class_name = pyrust_core::error_type_name(value);
            Err(PyError::named(
                "TypeError",
                format!("unhashable type: '{class_name}'"),
            ))
        }
        // Class objects are hashable by identity in CPython (type.__hash__
        // returns id(cls) >> 4, but pointer identity is what matters for
        // correctness).  Use the Rc pointer as the hash value, applying the
        // -1 → -2 sentinel remap matching CPython's tp_hash sentinel rule.
        ValueKind::PyClass(rc) => {
            let ptr = Rc::as_ptr(rc) as i64;
            Ok(if ptr == -1 { -2 } else { ptr })
        }
        // User-defined functions and lambdas are hashable by identity in CPython
        // (function.__hash__ returns id(f) >> 4, but pointer identity is what
        // matters for correctness).  Use the Rc pointer as the hash value.
        ValueKind::UserFunction(rc) => {
            let ptr = Rc::as_ptr(rc) as i64;
            Ok(if ptr == -1 { -2 } else { ptr })
        }
        // Built-in functions hash by concrete function-object identity.
        // Ordinary module reloads produce fresh Rc<UserFunction> objects while
        // flat builtins remain interned, so equal builtins and only equal
        // builtins share this pointer hash.
        ValueKind::BuiltinFunction(_) => {
            let function = value
                .as_function_rc()
                .expect("BuiltinFunction must carry Rc<UserFunction>");
            let ptr = Rc::as_ptr(function) as i64;
            Ok(if ptr == -1 { -2 } else { ptr })
        }
        // Bound methods: CPython hashes as hash(func) ^ hash(self), where func
        // and self are each hashed by pointer identity.  Mirror that here using
        // the Rc pointer of the underlying UserFunction and the Rc pointer of
        // the bound instance.
        ValueKind::BoundMethod { function, receiver } => {
            let func_ptr = Rc::as_ptr(function) as i64;
            let recv_ptr = Rc::as_ptr(receiver) as i64;
            let h = func_ptr ^ recv_ptr;
            Ok(if h == -1 { -2 } else { h })
        }
        // Class-bound methods (classmethods): same XOR pattern, but the second
        // component is the bound class rather than an instance.
        ValueKind::ClassBoundMethod { function, class } => {
            let func_ptr = Rc::as_ptr(function) as i64;
            let class_ptr = Rc::as_ptr(class) as i64;
            let h = func_ptr ^ class_ptr;
            Ok(if h == -1 { -2 } else { h })
        }
        // Complex: CPython Objects/complexobject.c complex_hash.
        //
        //   hash_real = _Py_HashDouble(re)  (as Py_uhash_t)
        //   hash_imag = _Py_HashDouble(im)  (as Py_uhash_t)
        //   combined  = hash_real + _Py_HASH_IMAG * hash_imag  (wrapping u64)
        //   result    = combined as Py_hash_t (i64); if -1 return -2
        //
        // No additional modulo is applied to the sum: CPython uses wrapping
        // unsigned arithmetic matching Py_uhash_t overflow in C.
        ValueKind::Complex(re, im) => {
            const HASH_IMAG: u64 = 1000003;
            let hash_re = py_hash_float(re) as u64;
            let hash_im = py_hash_float(im) as u64;
            let combined = hash_re.wrapping_add(HASH_IMAG.wrapping_mul(hash_im));
            let result = combined as i64;
            Ok(if result == -1 { -2 } else { result })
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("unhashable type: '{}'", pyrust_core::error_type_name(value)),
        )),
    }
}

/// Interpreter-aware hash that dispatches `__hash__` for `PyInstance` values
/// and handles `Tuple` elements by recursing with interpreter access.
///
/// This is the entry point used by the `hash` builtin.  `hash_value` (above)
/// remains a pure helper for primitive leaf types; this function calls it for
/// those cases to avoid duplicating their logic.
///
/// `Tuple`: uses the CPython 3.12 xxHash-based tuplehash algorithm (issue #892),
/// but each element is hashed via this function rather than `hash_value`, so
/// `PyInstance` elements dispatch `__hash__` correctly (issue #502).
///
/// Returns `true` if `v` (or any value recursively nested inside it) is a
/// `PyInstance` that requires interpreter access for `__hash__` dispatch.
///
/// Recurses into `Tuple` elements and `slice` components so that a
/// `PyInstance` hidden inside `(inst, 1)` or `slice((inst, 1), 2)` is
/// correctly detected and routed through the interpreter hashing path.
pub(crate) fn value_needs_interp(v: &Value) -> bool {
    match v.kind() {
        ValueKind::PyInstance(_) => true,
        ValueKind::Tuple(inner) => tuple_needs_interp(inner),
        ValueKind::BuiltinObject { ops, state } if pyrust_builtins::slice::is_slice_ops(ops) => {
            let borrow = state.borrow();
            if let Some(s) = borrow.downcast_ref::<pyrust_builtins::slice::SliceState>() {
                value_needs_interp(&s.start)
                    || value_needs_interp(&s.stop)
                    || value_needs_interp(&s.step)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn tuple_needs_interp(items: &[Value]) -> bool {
    items.iter().any(value_needs_interp)
}

/// Returns `true` when hashing `v` requires the slow per-element path of
/// `hash_value_with_interp` rather than the pure `hash_value` shortcut.
///
/// Two cases force the slow path:
/// 1. A `PyInstance` anywhere in the tree (needs interpreter `__hash__` dispatch).
/// 2. A `slice` whose components contain an unhashable primitive (list/dict/set/…)
///    at any nesting depth — the pure `hash_value` path would blame `'slice'` via
///    `SliceOps::hash` returning `None`, but the slow path properly names the leaf
///    unhashable type (issue #893).
pub(crate) fn value_needs_slow_hash(v: &Value) -> bool {
    if value_needs_interp(v) {
        return true;
    }
    // All slices need the slow path: SliceOps::hash is not implemented, so
    // hash_value would always produce a misleading "unhashable type: 'slice'"
    // error regardless of whether the components are actually hashable.
    // hash_value_with_interp handles all three cases correctly: unhashable
    // primitive component (names the component type), PyInstance component
    // (dispatches __hash__), and all-hashable components (computes the hash).
    if let ValueKind::BuiltinObject { ops, .. } = v.kind()
        && pyrust_builtins::slice::is_slice_ops(ops)
    {
        return true;
    }
    // Recurse into tuple elements.
    if let ValueKind::Tuple(items) = v.kind() {
        return items.iter().any(value_needs_slow_hash);
    }
    false
}

fn invoke_instance_hash_slot(
    interp: &mut Interpreter,
    hash_method: Value,
    receiver: Value,
    class: &Rc<RefCell<PyClass>>,
) -> Result<Value> {
    let unhashable = || {
        let class_name = pyrust_core::error_type_name(&receiver);
        pyrust_core::type_err!("unhashable type: '{class_name}'")
    };
    match bind_class_level_method_wrapper(&hash_method, class) {
        Ok(Some(bound)) => {
            if bound.is_none() {
                return Err(unhashable());
            }
            return call_slot_value_unbound(interp, bound, &[]);
        }
        Ok(None) => {}
        Err(_) => return Err(unhashable()),
    }
    if !slot_supports_descriptor_get(&hash_method) {
        return invoke_class_method(interp, hash_method, receiver, &[]);
    }

    let owner = Value::py_class(Rc::clone(class));
    let bound = match call_descriptor_get(interp, &hash_method, receiver.clone(), owner, "__hash__")
    {
        Ok(bound) => bound,
        Err(_) => return Err(unhashable()),
    };
    if bound.is_none() {
        return Err(unhashable());
    }
    call_slot_value_unbound(interp, bound, &[])
}

pub(crate) fn hash_value_with_interp(
    interp: &mut crate::Interpreter,
    value: &Value,
) -> Result<i64> {
    match value.kind() {
        ValueKind::Tuple(items) => {
            // Fast path: if no element at any depth requires the slow path
            // (PyInstance needing __hash__ dispatch, or a slice with unhashable
            // primitive components that hash_value would misreport as 'slice'),
            // delegate to the pure hash_value helper — no Vec allocation needed.
            if !items.iter().any(value_needs_slow_hash) {
                return hash_value(value);
            }
            // At least one element needs interpreter access (PyInstance or a
            // nested tuple that may contain one).  Clone the slice to release
            // the borrow of `value` before the mutable `interp` calls.
            let items: Vec<Value> = items.to_vec();
            tuple_hash_cpython(
                items
                    .iter()
                    .map(|item| hash_value_with_interp(interp, item)),
            )
        }
        // Slices: CPython 3.12 makes slice hashable when all components are
        // hashable.  Always recurse into each component via this function so
        // that unhashable components (e.g. a list bound) surface the correct
        // per-component "unhashable type: 'list'" TypeError instead of the
        // misleading "unhashable type: 'slice'" that the pure
        // `SliceOps::hash` (DefaultHasher) path used to produce (issue #850).
        // This also handles `PyInstance` bounds that need interpreter access
        // for `__hash__` dispatch.
        ValueKind::BuiltinObject { ops, state } if pyrust_builtins::slice::is_slice_ops(ops) => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<pyrust_builtins::slice::SliceState>()
                .expect("SliceOps: bad state");
            let needs_interp = value_needs_interp(&s.start)
                || value_needs_interp(&s.stop)
                || value_needs_interp(&s.step);
            // Check for unhashable primitive components while the borrow is live.
            let unhashable = if !needs_interp {
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
            // Clone before dropping the borrow — all accesses to s must happen here.
            let (start, stop, step) = (s.start.clone(), s.stop.clone(), s.step.clone());
            drop(borrow);
            if let Some(bad_type) = unhashable {
                return Err(PyError::named(
                    "TypeError",
                    format!("unhashable type: '{bad_type}'"),
                ));
            }
            let hstart = hash_value_with_interp(interp, &start)?;
            let hstop = hash_value_with_interp(interp, &stop)?;
            let hstep = hash_value_with_interp(interp, &step)?;
            // Hash components using CPython 3.12 slice hash: same xxHash kernel as
            // tuplehash but without the final length-mixing XOR step (issue #892).
            slice_hash_cpython([hstart, hstop, hstep].into_iter().map(Ok))
        }
        ValueKind::PyInstance(inst) => {
            // Issue #1936: a builtin-subclass instance (int/str/float/bytes/
            // tuple/frozenset subclass) with no user `__hash__` override hashes
            // by its backing value (`hash(I(5)) == hash(5)`).  Mirror the
            // `value_to_pykey` path so `hash()` and dict/set keying agree.
            if let Some(backing) = coerce_subclass_backing(value, &["__hash__"]) {
                let hashable = matches!(
                    backing.kind(),
                    ValueKind::Int(_)
                        | ValueKind::BigInt(_)
                        | ValueKind::Bool(_)
                        | ValueKind::Float(_)
                        | ValueKind::Str(_)
                        | ValueKind::Bytes(_)
                        | ValueKind::Tuple(_)
                ) || pyrust_builtins::frozenset::as_items(&backing).is_some();
                if hashable {
                    return hash_value_with_interp(interp, &backing);
                }
            }
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            let class_name = pyrust_core::error_type_name(value);
            if let Some(hash_method) = lookup_class_attr(&class, "__hash__") {
                // __hash__ = None means explicitly unhashable (CPython rule).
                if matches!(hash_method.kind(), ValueKind::None) {
                    return Err(PyError::named(
                        "TypeError",
                        format!("unhashable type: '{class_name}'"),
                    ));
                }
                // Issue #2299: the unhashable built-in types (list/dict/set/
                // bytearray) set `__hash__ = None` on the *type*, so a subclass
                // that does not override `__hash__` inherits unhashability.  The
                // MRO lookup lands on the inherited `object.__hash__` sentinel,
                // OR — when an unhashable builtin and a user `__hash__`-defining
                // base are *both* in the MRO — on the user method if it sits
                // after the builtin (`class C(list, M)`: MRO `[C, list, M, …]`).
                // `class_hash_inherits_builtin_none` walks the MRO and reports
                // whether an unhashable builtin precedes any `__hash__`-defining
                // class, so it covers both shapes regardless of which method the
                // attribute lookup resolved (#2611).  A subclass that re-enables
                // hashing (`__hash__ = object.__hash__` in its own dict) shadows
                // that `None` and the helper returns false.
                if class_hash_inherits_builtin_none(&class) {
                    return Err(PyError::named(
                        "TypeError",
                        format!("unhashable type: '{class_name}'"),
                    ));
                }
                let result = invoke_instance_hash_slot(
                    interp,
                    hash_method,
                    Value::py_instance(inst_rc),
                    &class,
                )?;
                // CPython's slot_tp_hash semantics (issue #503):
                // - Int: apply only the `-1 → -2` sentinel remap.
                // - BigInt: apply Mersenne-prime reduction (long_hash).
                let hash_val: i64 = match result.kind() {
                    ValueKind::Int(n) => {
                        if n == -1 {
                            -2
                        } else {
                            n
                        }
                    }
                    ValueKind::Bool(b) => b as i64,
                    ValueKind::BigInt(n) => bigint_hash(n),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "__hash__ method should return an integer".to_string(),
                        ));
                    }
                };
                return Ok(hash_val);
            }
            // No __hash__ at all: identity hash (CPython's default
            // object.__hash__), with the -1 → -2 sentinel remap.
            let ptr = Rc::as_ptr(&inst_rc) as i64;
            Ok(if ptr == -1 { -2 } else { ptr })
        }
        // All other types are primitives; delegate to the pure hash_value helper.
        _ => hash_value(value),
    }
}

#[inline(always)]
fn int_hash(value: i64) -> i64 {
    py_hash_int(value)
}

#[inline(always)]
fn bigint_hash(value: &crate::value::PyBigInt) -> i64 {
    py_hash_bigint(value)
}

// CPython 3.12 tuple and slice hashes share the same xxHash accumulation
// kernel. Tuple hashing adds the element-count mix; slice hashing does not.
const XX_PRIME1: u64 = 11400714785074694791;
const XX_PRIME2: u64 = 14029467366897019727;
const XX_PRIME5: u64 = 2870177450012600261;

#[inline(always)]
fn xxstep(acc: u64, lane: u64) -> u64 {
    let acc = acc.wrapping_add(lane.wrapping_mul(XX_PRIME2));
    acc.rotate_left(31).wrapping_mul(XX_PRIME1)
}

fn tuple_hash_cpython(items: impl Iterator<Item = Result<i64>>) -> Result<i64> {
    let mut acc = XX_PRIME5;
    let mut len = 0_u64;
    for hash in items {
        acc = xxstep(acc, hash? as u64);
        len += 1;
    }
    acc = acc.wrapping_add(len ^ (XX_PRIME5 ^ 3_527_539));
    if acc == u64::MAX {
        acc = 1_546_275_796;
    }
    Ok(acc as i64)
}

fn slice_hash_cpython(items: impl Iterator<Item = Result<i64>>) -> Result<i64> {
    let mut acc = XX_PRIME5;
    for hash in items {
        acc = xxstep(acc, hash? as u64);
    }
    if acc == u64::MAX {
        acc = 1_546_275_796;
    }
    Ok(acc as i64)
}
