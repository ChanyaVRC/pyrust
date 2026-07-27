pub(crate) fn iter_values(value: &Value) -> Result<Vec<Value>> {
    // list/dict/set subclass: delegate to the backing primitive value.
    // Keep the `inst_rc` binding (not just `builtin_data_backing`) so the
    // not-iterable error below can name the actual subclass, not the base.
    if let Some(inst_rc) = value.as_py_instance_rc()
        && let Some(backing) = instance_builtin_data(inst_rc)
    {
        // A subclass of a *non-iterable* builtin (e.g. `class C(int): pass`)
        // is itself not iterable.  CPython reports the actual subclass name
        // ("'C' object is not iterable"), not the backing base's name, so
        // re-label the not-iterable error with the carrier's class name
        // rather than letting the int/float/… backing surface "'int' …".
        return iter_values(&backing).map_err(|e| {
            if e.class_name_is("TypeError") {
                pyrust_core::type_err!(
                    "'{}' object is not iterable",
                    inst_rc.borrow().class.borrow().name
                )
            } else {
                e
            }
        });
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
            if let Some(elems) = pyrust_builtins::bytearray::iter_elements(value) {
                return Ok(elems);
            }
            if let Some(keys) = pyrust_builtins::instance_dict::iter_visible_keys(value) {
                return Ok(keys);
            }
            if let Some(rc) = pyrust_builtins::frozenset::as_items(value) {
                return Ok(rc.iter().map(|k| key_to_value(k.clone())).collect());
            }
            if let Some(kind) = pyrust_builtins::dict_views::view_kind(value) {
                // `view_kind` and `as_dict_rc` both check the same concrete
                // ops/state pair, so they should agree — but use a structured
                // error rather than unwrap if a future implementation is
                // misregistered.
                // Surface as TypeError so Python-level `except` blocks can
                // catch it (the only way to reach this is a misregistered
                // ops table, which is a type-mismatch error).
                let rc = pyrust_builtins::dict_views::as_dict_rc(value)
                    .ok_or_else(|| pyrust_core::type_err!("dict-view state type mismatch"))?;
                let map = rc.borrow();
                return Ok(match kind {
                    pyrust_builtins::dict_views::DictViewKind::Keys => {
                        map.keys().map(|k| key_to_value(k.clone())).collect()
                    }
                    pyrust_builtins::dict_views::DictViewKind::Values => {
                        map.values().cloned().collect()
                    }
                    pyrust_builtins::dict_views::DictViewKind::Items => map
                        .iter()
                        .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                        .collect(),
                });
            }
            if let Some(class_rc) = pyrust_builtins::mapping_proxy::as_class_rc(value) {
                let class = class_rc.borrow();
                return Ok(class
                    .attrs
                    .keys()
                    .map(|k| Value::string(k.clone()))
                    .collect());
            }
            // Dict-backed `mappingproxy` (`d.keys().mapping`, issue #2679):
            // iterating yields the parent dict's keys, like iterating a dict.
            if let Some(rc) = pyrust_builtins::mapping_proxy::as_dict_rc(value) {
                return Ok(rc
                    .borrow()
                    .keys()
                    .map(|k| key_to_value(k.clone()))
                    .collect());
            }
            let mut out = Vec::new();
            let ValueKind::BuiltinObject { ops, state } = value.kind() else {
                unreachable!();
            };
            if !ops.is_iterator() {
                return Err(pyrust_core::type_err!(
                    "'{}' object is not iterable",
                    ops.display_type_name()
                ));
            }
            while let Some(v) = ops.iter_next(state)? {
                out.push(v);
            }
            Ok(out)
        }
        ValueKind::Bytes(rc) => Ok(rc.iter().map(|b| Value::int(*b as i64)).collect()),
        ValueKind::Str(text) => Ok(pyrust_core::cesu8_codepoints(text)
            .map(|cp| Value::string(pyrust_core::cesu8_encode_codepoint(cp)))
            .collect()),
        ValueKind::Dict(items) => Ok(items.keys().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::Range { start, stop, step } => {
            let mut out = Vec::new();
            if step > 0 {
                let mut cur = start;
                while cur < stop {
                    out.push(Value::int(cur));
                    let Some(next) = cur.checked_add(step) else {
                        // Crossing i64 here is necessarily beyond the i64 stop;
                        // the just-pushed element was the range's final value.
                        break;
                    };
                    cur = next;
                }
            } else {
                let mut cur = start;
                while cur > stop {
                    out.push(Value::int(cur));
                    let Some(next) = cur.checked_add(step) else {
                        break;
                    };
                    cur = next;
                }
            }
            Ok(out)
        }
        ValueKind::BigRange { start, stop, step } => {
            // Materialize an arbitrary-precision range (#2118).  Only reached for
            // out-of-i64 bounds; the element *count* still fits in memory (a range
            // whose length itself overflows would OOM here, exactly as CPython's
            // `list(range(...))` does).
            let mut out = Vec::new();
            let mut cur = start.clone();
            if step.sign() == pyrust_core::PyBigIntSign::Plus {
                while cur < *stop {
                    out.push(value_from_bigint(cur.clone()));
                    cur += step;
                }
            } else {
                while cur > *stop {
                    out.push(value_from_bigint(cur.clone()));
                    cur += step;
                }
            }
            Ok(out)
        }
        ValueKind::Generator(state_rc) => {
            // Drain a NativeIterFrame (created by iter() on builtins) into a Vec.
            let mut borrow = state_rc.borrow_mut();
            if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                native.drain_remaining()
            } else {
                Err(pyrust_core::type_err!("object is not iterable"))
            }
        }
        _ => Err(pyrust_core::type_err!(
            "'{}' object is not iterable",
            value_type_name_str(value)
        )),
    }
}
