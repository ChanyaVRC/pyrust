/// Return the canonical primitive owner of an inherited builtin slot.
///
/// A `BuiltinFunction("dict.__getitem__")` value is not sufficient evidence
/// that the slot is the dict implementation: Python code can assign that same
/// descriptor to an unrelated class.  The MRO owner is the provenance that
/// distinguishes inheritance from an explicit assignment.
fn inherited_primitive_builtin_slot_kind(
    class: &Rc<std::cell::RefCell<pyrust_core::PyClass>>,
    slot: &str,
    method: &Value,
) -> Option<pyrust_core::CanonicalClassTag> {
    if !matches!(method.kind(), ValueKind::BuiltinFunction(_)) {
        return None;
    }
    let owner = crate::interpreter::lookup_class_attr_owner(class, slot)?;
    let owner_kind = crate::interpreter::primitive_class_kind(&owner)?;
    let owns_resolved_method = owner
        .borrow()
        .attrs
        .get(slot)
        .is_some_and(|owned| values_are_identical(owned, method));
    owns_resolved_method.then_some(owner_kind)
}

impl Interpreter {
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
        // Slice-object subscript redirect.  Probe the *index* first: for the
        // hot integer-index path the BuiltinObject match misses immediately and
        // no target type_name() probe runs.  Only when the index is actually a
        // slice object do we consult the target type (#1908 adds bytearray to
        // the sequence-like set; bytearray slices are always slice ops, never
        // hashable keys, so the redirect is safe).
        if let ValueKind::BuiltinObject { ops, state } = index.kind()
            && pyrust_builtins::slice::is_slice_ops(ops)
        {
            let target_is_sequence_like = matches!(
                target.kind(),
                ValueKind::List(_)
                    | ValueKind::Tuple(_)
                    | ValueKind::Str(_)
                    | ValueKind::Bytes(_)
                    | ValueKind::PyInstance(_)
                    // Issue #2399: `range(5).__getitem__(slice(1, None))` reaches
                    // `eval_index` with a slice *object* (not a slice expression),
                    // so it must redirect to `eval_slice` — which already handles
                    // `Range`/`BigRange` arithmetically — exactly as `range(5)[1:]`
                    // does.  Without this, the slot-dunder form raised "range
                    // indices must be integers or slices, not slice".
                    | ValueKind::Range { .. }
                    | ValueKind::BigRange { .. }
            ) || matches!(
                target.kind(),
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Bytearray)
            );
            if target_is_sequence_like {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<pyrust_builtins::slice::SliceState>()
                    .expect("SliceOps: bad state");
                let lo = if s.start.is_none() {
                    None
                } else {
                    Some(s.start.clone())
                };
                let hi = if s.stop.is_none() {
                    None
                } else {
                    Some(s.stop.clone())
                };
                let st = if s.step.is_none() {
                    None
                } else {
                    Some(s.step.clone())
                };
                drop(borrow);
                return self.eval_slice(target, lo, hi, st);
            }
        }
        // Handle Dict separately so the temporary `&IndexMap` from
        // `target.kind()` doesn't outlive the call into `dict_lookup`
        // (which may run user `__eq__` that mutates the dict — see the
        // aliasing notes on `Value::as_dict_mut`).
        if target.is_dict() {
            // Fast path for string keys (issue #506): probe via `StrKey` to
            // skip constructing a `PyKey::Str(Value)` (which bumps the RC).
            let lookup = if let Some(s) = index.as_str() {
                self.dict_str_lookup(target, s)?
            } else {
                let key = self.value_to_pykey(&index)?;
                self.dict_lookup(target, &key)?
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
                // ASCII fast path (#2032 / #2116 / #2136): when every byte is
                // ASCII, char index == byte index, so length is `text.len()` and
                // the i-th char is a single byte — O(1) index instead of an
                // O(idx) char scan.  ASCII-ness is cached on the string header
                // (#2124), so the check is O(1) — no per-op rescan, no penalty
                // for non-ASCII strings.  The fast-path body lives in
                // `fast_path.rs::fast_str_ascii_index`.
                if target.str_is_ascii() {
                    return fast_str_ascii_index(text, &index);
                }
                let char_count = target.str_codepoint_len_for_index();
                let idx = normalize_index(&index, char_count, "string")?;
                let (byte_start, byte_end) = target.str_codepoint_byte_range(idx);
                Ok(target.string_slice(byte_start, byte_end))
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
                    ValueKind::Int(v) => i128::from(v),
                    ValueKind::Bool(b) => i128::from(b as i64),
                    // An i64-backed range can still contain 2**64-1 elements,
                    // so a positive or negative BigInt subscript just beyond
                    // i64::MAX may be valid.  i128 covers that whole domain.
                    ValueKind::BigInt(value) => match value.to_i128() {
                        Some(value) => value,
                        None => {
                            return Err(pyrust_core::index_err!("range object index out of range"));
                        }
                    },
                    _ => unreachable!("call_index_protocol guarantees an integer"),
                };
                if i < 0 {
                    i += len;
                }
                if i < 0 || i >= len {
                    return Err(pyrust_core::index_err!("range object index out of range"));
                }
                let value = i128::from(start) + i * i128::from(step);
                Ok(Value::int(
                    i64::try_from(value).expect("an in-range element fits its i64 range bounds"),
                ))
            }
            ValueKind::BigRange { start, stop, step } => {
                // Arbitrary-precision range indexing (#2118).  `call_index_protocol`
                // already resolved any `__index__`, so the subscript is an int-like
                // value; widen it to BigInt for the negative-wrap + bounds check.
                let len = pyrust_core::bigrange_len(start, stop, step);
                let mut i =
                    value_to_bigint(&index).expect("call_index_protocol guarantees an integer");
                if i.sign() == pyrust_core::PyBigIntSign::Minus {
                    i += &len;
                }
                if i.sign() == pyrust_core::PyBigIntSign::Minus || i >= len {
                    return Err(pyrust_core::index_err!("range object index out of range"));
                }
                Ok(value_from_bigint(start + i * step))
            }
            ValueKind::Dict(_) => unreachable!("handled above"),
            ValueKind::BuiltinObject { ops, state } => {
                // Built-in object types opt in to subscripting via
                // `BuiltinTypeOps::get_item`.  The default impl returns a
                // TypeError shaped like the legacy "object is not
                // subscriptable" message, so non-subscriptable types
                // don't need per-type plumbing.  bytearray's __index__ subscript
                // resolution is handled by callers (exec_get_item and the slice
                // redirect above) so the int-index hot path stays untouched.
                ops.get_item(state, &index)
            }
            ValueKind::PyClass(class_rc) => {
                let class = Rc::clone(class_rc);
                // PEP 585: `type[int]` → `types.GenericAlias`.  CPython does NOT
                // expose `__class_getitem__` as an attribute on `type`, so the
                // subscript is special-cased here by pointer-identity rather than
                // via the sentinel-attribute path used by `list`/`dict`/…
                // (`hasattr(type, '__class_getitem__')` stays False and
                // `type.__class_getitem__(int)` raises AttributeError).
                if Rc::ptr_eq(&class, &type_class_singleton()) {
                    let index_is_tuple = matches!(index.kind(), ValueKind::Tuple(_));
                    let type_args = if index_is_tuple {
                        index
                    } else {
                        Value::tuple(vec![index])
                    };
                    return Ok(pyrust_builtins::generic_alias::generic_alias(
                        Value::py_class(class),
                        type_args,
                    ));
                }
                // A metaclass `__getitem__` (e.g. `EnumMeta.__getitem__`, which
                // implements `Color['RED']` name lookup) is a type-level slot
                // that takes precedence over the class's own
                // `__class_getitem__` (#2611).
                if let Some(getitem_fn) =
                    metaclass_dunder_for_call(&class, "__getitem__").transpose()?
                {
                    return invoke_class_method(
                        self,
                        getitem_fn,
                        Value::py_class(Rc::clone(&class)),
                        &[ExpandedCallArg {
                            name: None,
                            value: index,
                        }],
                    );
                }
                // Look up `__class_getitem__` along the MRO (issue #2698).
                // Built-in collection types have a
                // `BuiltinFunction("<type>.__class_getitem__")` sentinel
                // registered by `build_primitive_classes`.  User-defined
                // classes may define it as a classmethod, or *inherit* one —
                // e.g. `class Stack(Generic[T])` inherits
                // `Generic.__class_getitem__`, and `class Sub(Base)` inherits a
                // user-defined `Base.__class_getitem__`.  Walking the MRO (not
                // just the class's own dict) is what makes those subscriptable.
                // Classes without it anywhere in the MRO raise TypeError
                // (matching CPython 3.12).
                let cgitem = lookup_class_attr(&class, "__class_getitem__");
                if let Some(method_val) = cgitem {
                    if is_builtin_class_getitem_sentinel(&method_val) {
                        Ok(make_builtin_generic_alias(class, index))
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
                } else if class.borrow().attrs.get("__type_params__").is_some_and(
                    |tp| matches!(tp.kind(), ValueKind::Tuple(items) if !items.is_empty()),
                ) {
                    // PEP 695 generic class (`class C[T]: ...`): CPython gives it
                    // an implicit `__class_getitem__` that returns a generic
                    // alias, so `C[int]` is subscriptable and `C[int]()`
                    // constructs an instance.  We detect the generic class via a
                    // non-empty `__type_params__` tuple and build the alias
                    // directly, mirroring the built-in-collection path above.
                    let index_is_tuple = matches!(index.kind(), ValueKind::Tuple(_));
                    let type_args = if index_is_tuple {
                        index
                    } else {
                        Value::tuple(vec![index])
                    };
                    Ok(pyrust_builtins::generic_alias::generic_alias(
                        Value::py_class(class),
                        type_args,
                    ))
                } else {
                    Err(pyrust_core::type_err!(
                        "type '{}' is not subscriptable",
                        class.borrow().name
                    ))
                }
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                // PEP 695: a generic `type X[T] = ...` alias is subscriptable —
                // `Pair[int]` returns a `types.GenericAlias` with the alias as
                // origin (CPython 3.12 reprs it `Pair[int]`, not the substituted
                // value).  A non-generic alias raises CPython's specific
                // "Only generic type aliases are subscriptable" (issue #2779).
                if is_type_alias_class(&class) {
                    let has_params =
                        inst_rc.borrow().attrs.get("__type_params__").is_some_and(
                            |p| matches!(p.kind(), ValueKind::Tuple(t) if !t.is_empty()),
                        );
                    if !has_params {
                        return Err(pyrust_core::type_err!(
                            "Only generic type aliases are subscriptable"
                        ));
                    }
                    let type_args = if matches!(index.kind(), ValueKind::Tuple(_)) {
                        index
                    } else {
                        Value::tuple(vec![index])
                    };
                    return Ok(pyrust_builtins::generic_alias::generic_alias(
                        Value::py_instance(inst_rc),
                        type_args,
                    ));
                }
                // Issue #1134: check for a user-defined __getitem__ on the
                // class *before* falling back to the backing primitive fast
                // path.  A dict subclass that overrides __getitem__ must have
                // the override called, not the raw backing-dict lookup.
                // A builtin sentinel is excluded only when its defining MRO
                // owner is the matching canonical primitive class.  Looking at
                // the qualified function name alone is incorrect: user code
                // can explicitly assign `dict.__getitem__` to another class,
                // in which case descriptor dispatch (and its receiver check)
                // must still run.
                let user_getitem = lookup_class_attr(&class, "__getitem__").filter(|v| {
                    inherited_primitive_builtin_slot_kind(&class, "__getitem__", v).is_none()
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
                if let Some(backing) = builtin_data_backing(target) {
                    if backing.is_dict() {
                        let lookup = if let Some(s) = index.as_str() {
                            self.dict_str_lookup(&backing, s)?
                        } else {
                            let key = self.value_to_pykey(&index)?;
                            self.dict_lookup(&backing, &key)?
                        };
                        return match lookup {
                            Some((_, v)) => Ok(v),
                            None => {
                                if let Some(missing_fn) = lookup_class_attr(&class, "__missing__") {
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
                Err(pyrust_core::type_err!(
                    "'{}' object is not subscriptable",
                    pyrust_core::error_type_name(target)
                ))
            }
            _ => Err(pyrust_core::type_err!(
                "'{}' object is not subscriptable",
                pyrust_core::error_type_name(target)
            )),
        }
    }
}
