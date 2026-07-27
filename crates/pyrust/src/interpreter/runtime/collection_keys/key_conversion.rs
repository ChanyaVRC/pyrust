impl Interpreter {
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
        // Fast path: the common primitive keys can never be a tuple / slice /
        // Range / PyInstance, so build the `PyKey` directly and skip the four
        // interpreter-dispatch branches below.  Semantically identical to the
        // matching `Value::to_key` arms.
        match value.kind() {
            ValueKind::Str(_) => return Ok(PyKey::Str(value.clone())),
            ValueKind::Int(v) => return Ok(PyKey::Int(v)),
            ValueKind::Bool(v) => return Ok(PyKey::Bool(v)),
            ValueKind::None => return Ok(PyKey::None),
            _ => {}
        }
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
        if let ValueKind::BuiltinObject { ops, state } = value.kind()
            && pyrust_builtins::slice::is_slice_ops(ops)
        {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<pyrust_builtins::slice::SliceState>()
                .expect("SliceOps: bad state");
            let needs_interp = value_needs_interp(&s.start)
                || value_needs_interp(&s.stop)
                || value_needs_interp(&s.step);
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
                return Err(pyrust_core::type_err!(
                    "unhashable type: '{component_name}'"
                ));
            }
            // All slices (instance or primitive components) go through
            // hash_value_with_interp to get the CPython-compatible slice hash
            // and to dispatch user __hash__ on PyInstance components.
            let hash = hash_value_with_interp(self, value)? as u64;
            return Ok(PyKey::Object {
                hash,
                value: value.clone(),
            });
        }
        if let Some(k) = value.to_key() {
            return Ok(k);
        }
        // Range objects are hashable (issue #937).  `Value::to_key` returns
        // `None` for ranges (they have no `PyKey` variant), so we handle them
        // here: compute the hash via `hash_value_with_interp` (which calls the
        // `ValueKind::Range` arm in `hash_value`) and store it in `PyKey::Object`
        // so that `range == range` lookup uses `Value`'s `PartialEq`.
        if matches!(
            value.kind(),
            ValueKind::Range { .. } | ValueKind::BigRange { .. }
        ) {
            let hash = hash_value_with_interp(self, value)? as u64;
            return Ok(PyKey::Object {
                hash,
                value: value.clone(),
            });
        }
        if let ValueKind::PyInstance(inst) = value.kind() {
            // Issue #1936: a builtin-subclass instance (int/str/float/bytes/
            // tuple/frozenset subclass) with no user `__hash__` inherits the
            // base type's `__hash__`, so it must key identically to its backing
            // value (`hash(I(5)) == hash(5)`, `{1: "a"}[I(1)]`, `len({1, I(1)})
            // == 1`).  `coerce_subclass_backing` excludes a user `__hash__`
            // override and the `__hash__ = None` unhashable case (handled
            // below), and skips the inherited `object.__hash__`/`int.__hash__`
            // sentinels.  Only hashable (immutable) backings key by value;
            // list/dict/set backings fall through to the unhashable handling.
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
                    // A user `__eq__` override means equality must NOT be
                    // decided structurally by the backing's PyKey (that path
                    // never dispatches the override on lookup).  Keep the
                    // instance as a `PyKey::Object` so the dict/set runtime
                    // dispatches the user comparison, but reuse the backing's
                    // value-based hash so `hash(E(5)) == hash(5)` still holds and
                    // same-value keys land in the same bucket (CPython parity).
                    // Dict/set membership uses `__eq__` only (not `__ne__`), so a
                    // `__ne__`-only subclass stays backing-keyed/interchangeable.
                    if coerce_subclass_backing(value, &["__eq__"]).is_none() {
                        let hash = hash_value_with_interp(self, &backing)? as u64;
                        return Ok(PyKey::Object {
                            hash,
                            value: value.clone(),
                        });
                    }
                    return self.value_to_pykey(&backing);
                }
            }
            let (class, has_builtin_data) = {
                let b = inst.borrow();
                (
                    Rc::clone(&b.class),
                    b.attrs.contains_key(crate::interpreter::BUILTIN_DATA_ATTR),
                )
            };
            // Issue #2324: an instance of a subclass of an unhashable builtin
            // (`list`/`dict`/`set`/`bytearray`) with no `__hash__`-re-enabling
            // override is unhashable as a dict/set key — exactly like
            // `hash(obj)`, which routes through `class_hash_inherits_builtin_none`.
            // The `__hash__ = None` carried by those builtins is injected at
            // attribute-resolution time (`env.rs::get_attr_class`), not stored
            // in `attrs`, so the `lookup_class_attr` probe below never observes
            // it.  Without this check `{L([1])}`, `d[L([1])] = …` and
            // `{BA(b"a")}` silently succeeded (the direct `hash()` path already
            // rejected them).  A class that re-enables hashing
            // (`__hash__ = object.__hash__`) defines `__hash__` in its own dict,
            // so the helper returns `false` and that case stays hashable.
            //
            // Gate on `__builtin_data__`: only a builtin-subclass instance can
            // inherit the implicit `__hash__ = None`, and such instances always
            // carry the backing-data attr.  A plain user-class instance (the hot
            // dict/set-key case) never does, so it skips the MRO-walking helper
            // entirely (avoids a ~7% regression on user-instance keys).
            if has_builtin_data && crate::interpreter::class_hash_inherits_builtin_none(&class) {
                let class_name = class.borrow().name.clone();
                return Err(pyrust_core::type_err!("unhashable type: '{class_name}'"));
            }
            // CPython treats a class that explicitly sets `__hash__ = None`
            // as unhashable.  In pyrust we treat the absence of `__hash__`
            // the same way for now.
            if let Some(hash_method) = lookup_class_attr(&class, "__hash__") {
                if matches!(hash_method.kind(), ValueKind::None) {
                    let class_name = class.borrow().name.clone();
                    return Err(pyrust_core::type_err!("unhashable type: '{class_name}'"));
                }
                // Issue #2299/#2386: the unhashable builtins (list/dict/set/
                // bytearray) carry `__hash__ = None` implicitly, so a subclass
                // that does not override `__hash__` resolves to the inherited
                // `object.__hash__` sentinel and would otherwise key by
                // identity.  Mirror the `hash()` builtin path
                // (`hash_value_with_interp`) and reject it as unhashable so a
                // `bytearray` subclass cannot be used as a set element / dict
                // key, matching CPython.
                if crate::interpreter::value_is_canonical_slot(
                    &hash_method,
                    crate::interpreter::CanonicalSlot::ObjectHash,
                ) && class_hash_inherits_builtin_none(&class)
                {
                    let class_name = class.borrow().name.clone();
                    return Err(pyrust_core::type_err!("unhashable type: '{class_name}'"));
                }
                // Issue #2055: a non-callable `__hash__` slot (`__hash__ = 5`)
                // raises `TypeError: 'int' object is not callable` when hashed,
                // matching CPython, instead of silently falling back to the
                // identity hash.  A callable instance / bound method is invoked
                // (issue #2054) via `invoke_class_method`.
                if !slot_is_callable(&hash_method) {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object is not callable",
                            value_type_name_str(&hash_method)
                        ),
                    ));
                }
                {
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
                        ValueKind::Int(n) => {
                            if n == -1 {
                                -2
                            } else {
                                n
                            }
                        }
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::BigInt(n) => py_hash_bigint(n),
                        _ => {
                            return Err(pyrust_core::type_err!(
                                "__hash__ method should return an integer"
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
        // by concrete Rc identity (CPython: function.__hash__). Builtin module
        // reloads may produce distinct callable objects with the same registry
        // dispatch name, so the static name pointer is not an object identity.
        if let ValueKind::UserFunction(rc) = value.kind() {
            let ptr = Rc::as_ptr(rc) as usize as u64;
            return Ok(PyKey::Object {
                hash: ptr,
                value: value.clone(),
            });
        }
        if matches!(value.kind(), ValueKind::BuiltinFunction(_)) {
            let rc = value
                .as_function_rc()
                .expect("BuiltinFunction must carry Rc<UserFunction>");
            let ptr = Rc::as_ptr(rc) as usize as u64;
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
        Err(pyrust_core::type_err!("unhashable type: '{type_name}'"))
    }

    /// `__eq__`-aware comparison of two `PyKey`s that may nest user objects.
    /// Converts both keys back to their Python `Value` and dispatches through
    /// [`Self::values_user_eq`], which already recurses element-wise into
    /// tuples / frozensets and fires user `__eq__` for `PyInstance` elements.
    /// Used to confirm a same-hash-bucket candidate matches a tuple/frozenset
    /// lookup key whose nested object compares by `__eq__`, not identity
    /// (issue #2059).
    fn nested_object_keys_eq(&mut self, stored: &PyKey, probe: &PyKey) -> Result<bool> {
        let stored_val = crate::interpreter::key_to_value(stored.clone());
        let probe_val = crate::interpreter::key_to_value(probe.clone());
        self.values_user_eq(&stored_val, &probe_val)
    }
}
