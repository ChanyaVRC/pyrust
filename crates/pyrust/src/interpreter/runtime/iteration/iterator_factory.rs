/// Return CPython's concrete iterator type name for a builtin collection.
pub(crate) fn builtin_iter_type_name(value: &Value) -> &'static str {
    match value.kind() {
        ValueKind::List(_) => "list_iterator",
        ValueKind::Tuple(_) => "tuple_iterator",
        ValueKind::Str(_) => {
            if value.str_is_ascii() {
                "str_ascii_iterator"
            } else {
                "str_iterator"
            }
        }
        ValueKind::Set(_) => "set_iterator",
        ValueKind::Dict(_) => "dict_keyiterator",
        ValueKind::Range { .. } => "range_iterator",
        ValueKind::Bytes(_) => "bytes_iterator",
        ValueKind::BuiltinObject { ops, .. } => {
            if pyrust_builtins::instance_dict::is_instance_dict(value) {
                "dict_keyiterator"
            } else if let Some(policy) = pyrust_builtins::ordered_mapping::view_policy(value) {
                policy.iterator_type_name
            } else {
                match pyrust_builtins::dict_views::view_kind(value) {
                    Some(pyrust_builtins::dict_views::DictViewKind::Keys) => "dict_keyiterator",
                    Some(pyrust_builtins::dict_views::DictViewKind::Values) => "dict_valueiterator",
                    Some(pyrust_builtins::dict_views::DictViewKind::Items) => "dict_itemiterator",
                    _ if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
                    {
                        "set_iterator"
                    }
                    _ => "generator",
                }
            }
        }
        _ => "generator",
    }
}

/// Whether every cursor value, including the one-past-the-end increment, fits
/// in the compact i64 range-iterator state.
///
/// The stop bound fitting in i64 is not enough: a final yielded value may be
/// near one extreme while `step` crosses that extreme. The direct VM loop and
/// concrete iterator object share this construction invariant.
fn i64_range_native_cursor_safe(start: i64, stop: i64, step: i64) -> bool {
    let len = range_len(start, stop, step);
    if len == 0 {
        return true;
    }

    let start_wide = i128::from(start);
    let step_wide = i128::from(step);
    let last = start_wide + (len - 1) * step_wide;
    let after_last = last + step_wide;
    len <= i128::from(i64::MAX)
        && after_last >= i128::from(i64::MIN)
        && after_last <= i128::from(i64::MAX)
        && step != i64::MIN
}

/// Construct the concrete lazy iterator CPython uses for an i64-backed range.
///
/// Most ranges fit [`RangeIter`]. If the cursor's final increment or its
/// length cannot fit the C-long-shaped state, use [`BigRangeIter`] just as
/// CPython falls back to `longrange_iterator`.
fn make_i64_range_iterator(start: i64, stop: i64, step: i64) -> Value {
    if i64_range_native_cursor_safe(start, stop, step) {
        return Value::generator(Box::new(RangeIter {
            cur: start,
            stop,
            step,
        }));
    }

    Value::generator(Box::new(BigRangeIter {
        cur: PyBigInt::from(start),
        stop: PyBigInt::from(stop),
        step: PyBigInt::from(step),
    }))
}

/// Construct a lazy reverse iterator for a range without materialising it.
///
/// Only range values are handled here; the ordinary sequence `reversed`
/// implementation keeps its existing snapshot/live semantics.
pub(crate) fn make_reversed_range_iterator(range: &Value) -> Result<Value> {
    match range.kind() {
        ValueKind::Range { start, stop, step } => {
            let len = range_len(start, stop, step);
            if len == 0 {
                return Ok(Value::generator(Box::new(RangeIter {
                    cur: 0,
                    stop: 0,
                    step: 1,
                })));
            }

            let start = i128::from(start);
            let step = i128::from(step);
            let last = start + (len - 1) * step;
            let reverse_step = -step;
            let reverse_stop = start - step;
            let fits_i64 =
                |value: i128| value >= i128::from(i64::MIN) && value <= i128::from(i64::MAX);
            if fits_i64(last) && fits_i64(reverse_step) && fits_i64(reverse_stop) {
                Ok(Value::generator(Box::new(RangeIter {
                    cur: last as i64,
                    stop: reverse_stop as i64,
                    step: reverse_step as i64,
                })))
            } else {
                Ok(Value::generator(Box::new(BigRangeIter {
                    cur: PyBigInt::from(last),
                    stop: PyBigInt::from(reverse_stop),
                    step: PyBigInt::from(reverse_step),
                })))
            }
        }
        ValueKind::BigRange { start, stop, step } => {
            let len = pyrust_core::bigrange_len(start, stop, step);
            let zero = PyBigInt::from(0);
            if len == zero {
                return Ok(Value::generator(Box::new(BigRangeIter {
                    cur: zero.clone(),
                    stop: zero,
                    step: PyBigInt::from(1),
                })));
            }
            let last = start + (&len - 1) * step;
            Ok(Value::generator(Box::new(BigRangeIter {
                cur: last,
                stop: start - step,
                step: -step,
            })))
        }
        _ => Err(PyError::Runtime(
            "make_reversed_range_iterator called for non-range".to_string(),
        )),
    }
}

/// Construct CPython's fixed-initial-length, live-index reverse iterator for
/// native sequence values.
pub(crate) fn make_reversed_sequence_iterator(sequence: &Value) -> Result<Value> {
    let (len, type_name) = match sequence.kind() {
        ValueKind::List(items) => (items.len(), "list_reverseiterator"),
        ValueKind::Tuple(items) => (items.len(), "reversed"),
        ValueKind::Str(_) => (sequence.str_codepoint_len_for_index(), "reversed"),
        ValueKind::Bytes(bytes) => (bytes.len(), "reversed"),
        ValueKind::BuiltinObject { ops, state }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
        {
            (ops.len(state).unwrap_or(0), "reversed")
        }
        _ => {
            return Err(PyError::Runtime(
                "make_reversed_sequence_iterator called for non-sequence".to_string(),
            ));
        }
    };
    Ok(Value::generator(Box::new(
        NativeIterFrame::reverse_indexed(sequence.clone(), len, type_name),
    )))
}

/// Construct the lazy reverse adapter for a user `__len__` + `__getitem__`
/// sequence. No item is requested until the first `next()`.
pub(crate) fn make_reversed_getitem_iterator(
    object: Value,
    getitem_method: Value,
    length_method: Value,
    len: usize,
) -> Value {
    Value::generator(Box::new(GetItemIter {
        obj: object,
        method: getitem_method,
        length_method: Some(length_method),
        index: i64::try_from(len).unwrap_or(i64::MAX).saturating_sub(1),
        step: -1,
        remaining: Some(len),
        exhausted: false,
    }))
}

/// Convert an arbitrary Python iterable value into an iterator object without
/// consuming any elements.
///
/// Mirrors the single-argument `iter()` builtin logic:
/// - `Generator` values (already-created iterators: map, filter, enumerate,
///   generator objects, etc.) are returned as-is.
/// - `PyInstance` values with `__iter__` have it called; the resulting iterator
///   object is returned.  `PyInstance` values with only `__getitem__` are
///   wrapped in a `GetItemIter`.
/// - Lists and tuples are wrapped as live indexed `NativeIterFrame`s; other
///   builtins use the frame's materialized-source form.
///
/// Used by `map()` and `filter()` to avoid eagerly exhausting generator sources
/// at construction time (issue #1388).
pub(crate) fn make_iterator(interp: &mut crate::Interpreter, v: &Value) -> Result<Value> {
    enum IterKind {
        Generator,
        PyInstance(Rc<RefCell<crate::value::PyInstance>>),
        Metaclass(Value),
        Range(i64, i64, i64),
        BigRange(
            crate::value::PyBigInt,
            crate::value::PyBigInt,
            crate::value::PyBigInt,
        ),
        // A `BuiltinObject` that is itself an iterator (`reversed`, `enumerate`,
        // `zip`, `chain`, file objects).  Its `__iter__` returns `self`, so it
        // is returned unchanged and shares position with the original — never
        // re-wrapped in a fresh `NativeIterFrame` (#2117).
        SelfIterator,
        Other,
    }
    // A coroutine (`async def`, issue #1039) — and an async generator (#2280)
    // — is not iterable.
    if is_coroutine_value(v) {
        let tn = full_type_name_str(v);
        return Err(pyrust_core::type_err!("'{tn}' object is not iterable"));
    }
    if pyrust_builtins::instance_dict::is_instance_dict(v) {
        return Ok(Value::generator(Box::new(NativeIterFrame::instance_dict(
            v.clone(),
        ))));
    }
    let kind = match v.kind() {
        ValueKind::Generator(_) => IterKind::Generator,
        ValueKind::PyInstance(inst) => IterKind::PyInstance(Rc::clone(inst)),
        ValueKind::PyClass(class) => metaclass_dunder(class, "__iter__")
            .map(IterKind::Metaclass)
            .unwrap_or(IterKind::Other),
        // The common i64-backed range needs the same lazy construction as a
        // BigRange. Materialising here makes `iter(range(10**9))`, and every
        // map/zip wrapping it, attempt a billion-element allocation.
        ValueKind::Range { start, stop, step } => IterKind::Range(start, stop, step),
        // Arbitrary-precision range (#2118): return a lazy iterator so callers
        // (iter / enumerate / zip) never materialize a huge sequence.
        ValueKind::BigRange { start, stop, step } => {
            IterKind::BigRange(start.clone(), stop.clone(), step.clone())
        }
        ValueKind::BuiltinObject { ops, .. } if ops.is_iterator() => IterKind::SelfIterator,
        _ => IterKind::Other,
    };
    match kind {
        IterKind::Generator | IterKind::SelfIterator => Ok(v.clone()),
        IterKind::Range(cur, stop, step) => Ok(make_i64_range_iterator(cur, stop, step)),
        IterKind::BigRange(cur, stop, step) => {
            Ok(Value::generator(Box::new(BigRangeIter { cur, stop, step })))
        }
        IterKind::Metaclass(iter_method) => {
            let iterator = invoke_class_method(interp, iter_method, v.clone(), &[])?;
            validate_iterator_result(iterator)
        }
        IterKind::PyInstance(inst_rc) => {
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = effective_user_iter(&class, &inst_rc) {
                let iter_obj = invoke_class_method(
                    interp,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?;
                validate_iterator_result(iter_obj)
            } else if let Some(backing) = instance_builtin_data(&inst_rc) {
                // Builtin subclasses inherit the primitive's iterator slot,
                // but store that primitive in `__builtin_data__`.  Keep the
                // classification here, beside the primitive path below, so
                // every consumer of `make_iterator` gets the same lazy source,
                // type name, and mutation guard as the `iter()` builtin.
                let ordered_policy = backing
                    .is_dict()
                    .then(|| pyrust_builtins::ordered_mapping::class_policy(&class))
                    .flatten();
                let ordered = ordered_policy.is_some();
                let type_name = ordered_policy.map_or_else(
                    || builtin_iter_type_name(&backing),
                    |policy| policy.iterator_type_name,
                );
                let mut frame = if backing.is_dict() && !ordered {
                    NativeIterFrame::live_keys(v.clone(), 0, type_name)
                } else if backing.is_set() {
                    NativeIterFrame::live_keys(v.clone(), 3, type_name)
                } else if matches!(backing.kind(), ValueKind::List(_) | ValueKind::Tuple(_)) {
                    NativeIterFrame::indexed(backing.clone(), type_name)
                } else if matches!(backing.kind(), ValueKind::Bytes(_)) {
                    NativeIterFrame::bytes(backing.clone(), type_name)
                } else if matches!(backing.kind(), ValueKind::Str(_)) {
                    NativeIterFrame::string(backing.clone(), type_name)
                } else {
                    // Use the carrier for error reporting so a non-iterable
                    // builtin subclass names the subclass, not its base.
                    NativeIterFrame::new(iter_values(v)?, type_name)
                };
                if let Some(recorded_len) = live_collection_len(&backing) {
                    let (msg, exhaust_first) = if backing.set_len().is_some() {
                        ("Set changed size during iteration", false)
                    } else {
                        ordered_policy
                            .map(|policy| (policy.mutation_message, policy.exhaust_first))
                            .unwrap_or(("dictionary changed size during iteration", false))
                    };
                    frame.guard = Some(Box::new(NativeIterGuard {
                        // Re-resolve the backing through the instance on each
                        // check.  This detects both in-place mutation and a
                        // replacement of the backing Value.
                        container: v.clone(),
                        version: recorded_len as i64,
                        kind: GuardVersion::Size,
                        msg,
                        exhaust_first,
                        provider_sequence: if ordered {
                            pyrust_builtins::ordered_mapping::clear_sequence()
                        } else {
                            0
                        },
                    }));
                }
                Ok(Value::generator(Box::new(frame)))
            } else if lookup_class_attr(&class, "__getitem__").is_some() {
                interp.make_getitem_iter(inst_rc)
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!("'{}' object is not iterable", class.borrow().name),
                ))
            }
        }
        IterKind::Other => {
            let iter_type_name = builtin_iter_type_name(v);
            let mut frame = if v.dict_len().is_some() {
                NativeIterFrame::live_keys(v.clone(), 0, iter_type_name)
            } else if v.set_len().is_some() {
                NativeIterFrame::live_keys(v.clone(), 3, iter_type_name)
            } else if matches!(v.kind(), ValueKind::List(_) | ValueKind::Tuple(_)) {
                NativeIterFrame::indexed(v.clone(), iter_type_name)
            } else if matches!(v.kind(), ValueKind::Bytes(_)) {
                NativeIterFrame::bytes(v.clone(), iter_type_name)
            } else if matches!(v.kind(), ValueKind::Str(_)) {
                NativeIterFrame::string(v.clone(), iter_type_name)
            } else if let Some(kind) = pyrust_builtins::dict_views::view_kind(v) {
                if pyrust_builtins::ordered_mapping::view_policy(v).is_some() {
                    let dict = pyrust_builtins::dict_views::as_dict_rc(v).ok_or_else(|| {
                        PyError::Runtime("dict view lost its backing dict".to_string())
                    })?;
                    let keys = dict.borrow().keys().cloned().collect();
                    NativeIterFrame::dict_view(dict, keys, kind, iter_type_name)
                } else {
                    NativeIterFrame::live_keys(v.clone(), kind.live_cursor_code(), iter_type_name)
                }
            } else {
                let items = iter_values(v).map_err(|_| {
                    PyError::named(
                        "TypeError",
                        format!("'{}' object is not iterable", value_type_name_str(v)),
                    )
                })?;
                NativeIterFrame::new(items, iter_type_name)
            };
            // dict / set / dict-views: guard the manual `iter()` iterator
            // against size mutation, mirroring the `for`-loop guard (#1988).
            if let Some(recorded_len) = crate::interpreter::live_collection_len(v) {
                let ordered_policy = pyrust_builtins::ordered_mapping::view_policy(v);
                let (msg, exhaust_first) = if v.set_len().is_some() {
                    ("Set changed size during iteration", false)
                } else {
                    ordered_policy
                        .map(|policy| (policy.mutation_message, policy.exhaust_first))
                        .unwrap_or(("dictionary changed size during iteration", false))
                };
                frame.guard = Some(Box::new(NativeIterGuard {
                    container: v.clone(),
                    version: recorded_len as i64,
                    kind: GuardVersion::Size,
                    msg,
                    exhaust_first,
                    provider_sequence: if ordered_policy.is_some() {
                        pyrust_builtins::ordered_mapping::clear_sequence()
                    } else {
                        0
                    },
                }));
            }
            Ok(Value::generator(Box::new(frame)))
        }
    }
}

fn validate_iterator_result(iterator: Value) -> Result<Value> {
    let valid = match iterator.kind() {
        ValueKind::Generator(_) => true,
        ValueKind::PyInstance(instance) => {
            let class = Rc::clone(&instance.borrow().class);
            lookup_class_attr(&class, "__next__").is_some()
        }
        ValueKind::BuiltinObject { ops, .. } => ops.is_iterator(),
        _ => false,
    };
    if valid {
        Ok(iterator)
    } else {
        Err(PyError::named(
            "TypeError",
            format!(
                "iter() returned non-iterator of type '{}'",
                value_type_name_str(&iterator),
            ),
        ))
    }
}

/// Build the `reversed()` iterator for a `dict` or one of its views (#2448).
///
/// `items` must already be in *reverse* order (the caller materialises the
/// forward key/value/pair list and reverses it).  `container` is the live
/// `dict` Value or dict-view whose size is re-read on each step: like CPython's
/// forward dict iterators, mutating the dict's size during the `reversed()`
/// walk raises `RuntimeError` on the next `next()` call.  The wording and
/// check-ordering follow the provider policy shared with the forward path:
/// tagged ordered views test exhaustion before the guard (`exhaust_first`),
/// plain dicts test the guard first.
///
/// `type_name` is the CPython iterator type name for the view kind (#2702):
/// `dict_reversekeyiterator` / `dict_reversevalueiterator` /
/// `dict_reverseitemiterator`.  It drives both `type(...).__name__` and the
/// iterator's repr.
pub(crate) fn make_reversed_dict_iter(
    items: Vec<Value>,
    container: Value,
    type_name: &'static str,
) -> NativeIterFrame {
    let recorded_len = items.len();
    let ordered_policy = pyrust_builtins::ordered_mapping::view_policy(&container);
    // A provider may override the iterator presentation shared by its forward
    // and reverse views. Plain dicts retain the kind-specific name supplied by
    // the caller (#2702).
    let type_name = ordered_policy.map_or(type_name, |policy| policy.iterator_type_name);
    let mut frame = NativeIterFrame::new(items, type_name);
    let (msg, exhaust_first) = ordered_policy
        .map(|policy| (policy.mutation_message, policy.exhaust_first))
        .unwrap_or(("dictionary changed size during iteration", false));
    let provider_sequence = if ordered_policy.is_some() {
        pyrust_builtins::ordered_mapping::clear_sequence()
    } else {
        0
    };
    frame.guard = Some(Box::new(NativeIterGuard {
        container,
        version: recorded_len as i64,
        kind: GuardVersion::Size,
        msg,
        exhaust_first,
        provider_sequence,
    }));
    frame
}
