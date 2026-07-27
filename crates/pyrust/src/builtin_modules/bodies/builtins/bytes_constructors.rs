use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: bytes() — bytes constructor.
    /// <https://docs.python.org/3/library/functions.html#func-bytes>
    /// The iterable fallback can run user code by dispatching
    /// `__iter__` and `__next__` when consuming a general iterable (e.g. range,
    /// generator expressions, user-defined iterables).
    fn bytes(args) -> Result<Value> {
        // CPython 3.12: bytes(source, encoding, errors) — source/encoding/
        // errors are keyword-or-positional.
        let bound = bind_bytes_like_args(FN_NAME, args)?;
        let args = &bound[..];
        match args.len() {
            0 => Ok(Value::bytes(Vec::new())),
            1 => match args[0].value.kind() {
                ValueKind::Int(n) => {
                    if n < 0 {
                        return Err(PyError::named("ValueError", "negative count".to_string()));
                    }
                    Ok(Value::bytes(vec![0u8; n as usize]))
                }
                ValueKind::Bool(b) => {
                    // bool is a subclass of int; True == 1, False == 0
                    Ok(Value::bytes(vec![0u8; b as usize]))
                }
                ValueKind::Bytes(rc) => Ok(Value::bytes((**rc).clone())),
                ValueKind::Str(_) => Err(PyError::named(
                    "TypeError",
                    "string argument without an encoding".to_string(),
                )),
                ValueKind::List(items) => {
                    // Warm path: every element is a plain int/bool — no clone,
                    // no per-element dispatch.  Only on hitting a PyInstance do
                    // we clone the remaining elements (releasing the cell borrow
                    // so __index__ can't alias the list) and resolve them.
                    let fast = try_fast_bytes_elems(&items)?;
                    match fast {
                        Ok(out) => Ok(Value::bytes(out)),
                        Err((mut out, from)) => {
                            let rest: Vec<Value> = items[from..].to_vec();
                            drop(items);
                            for v in &rest {
                                out.push(bytes_element_to_u8(_interp, v)?);
                            }
                            Ok(Value::bytes(out))
                        }
                    }
                }
                ValueKind::Tuple(items) => {
                    let fast = try_fast_bytes_elems(items)?;
                    match fast {
                        Ok(out) => Ok(Value::bytes(out)),
                        Err((mut out, from)) => {
                            let rest: Vec<Value> = items[from..].to_vec();
                            for v in &rest {
                                out.push(bytes_element_to_u8(_interp, v)?);
                            }
                            Ok(Value::bytes(out))
                        }
                    }
                }
                ValueKind::BigInt(_) => Err(PyError::named(
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer".to_string(),
                )),
                ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
                    // CPython 3.12: check __bytes__ before falling through to the
                    // iterable path. __bytes__ takes priority over __iter__.
                    let self_val = args[0].value.clone();
                    // Issue #1204: if the instance is a bytes subclass extract the
                    // backing value first. `bytes(MyBytes(b"x"))` must return b'x'.
                    if let ValueKind::PyInstance(instance) = self_val.kind()
                        && let Some(backing) = instance_builtin_data(instance)
                        && matches!(backing.kind(), ValueKind::Bytes(_))
                    {
                        return Ok(backing);
                    }
                    if let Some(method) = lookup_value_special_method(&self_val, "__bytes__") {
                        let result =
                            invoke_class_method(_interp, method, self_val.clone(), &[])?;
                        return if matches!(result.kind(), ValueKind::Bytes(_)) {
                            Ok(result)
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__bytes__ returned non-bytes (type {})",
                                    value_type_name_str(&result)
                                ),
                            ))
                        };
                    }
                    // No __bytes__: CPython next honors __index__ as the count
                    // form (`bytes(obj)` -> N zero bytes) before __iter__ (#1908).
                    if let Some(count) = bytes_count_via_index(_interp, &args[0].value)? {
                        return Ok(Value::bytes(vec![0u8; count]));
                    }
                    // Otherwise fall through to the iterable path.
                    let type_name = value_type_name_str(&args[0].value).to_string();
                    let items =
                        _interp.collect_iterable(&args[0].value).map_err(|e| {
                            if e.class_name_is("TypeError") {
                                PyError::named(
                                    "TypeError",
                                    format!("cannot convert '{type_name}' object to bytes"),
                                )
                            } else {
                                e
                            }
                        })?;
                    Ok(Value::bytes(bytes_from_items(_interp, items)?))
                }
                _ => {
                    // General iterable fallback: any object supporting __iter__ /
                    // __next__ (range, generators, user-defined iterables, etc.).
                    // Non-iterable arguments produce CPython-compatible
                    // "cannot convert 'X' object to bytes".
                    let type_name = pyrust_core::builtin_type_name(&args[0].value).into_owned();
                    let items =
                        _interp.collect_iterable(&args[0].value).map_err(|e| {
                            if e.class_name_is("TypeError") {
                                PyError::named(
                                    "TypeError",
                                    format!("cannot convert '{type_name}' object to bytes"),
                                )
                            } else {
                                e
                            }
                        })?;
                    Ok(Value::bytes(bytes_from_items(_interp, items)?))
                }
                #[allow(unreachable_patterns)]
                _ => Err(PyError::named(
                    "TypeError",
                    "cannot convert to bytes".to_string(),
                )),
            },
            // bytes(source, encoding[, errors]) — encode `source` using
            // `encoding`.  CPython accepts a wide spectrum of codecs; we
            // support the common ASCII-compatible ones (utf-8, ascii,
            // latin-1) and reject the rest with `LookupError` for parity
            // with `LookupError: unknown encoding: <name>`. (#391)
            2 | 3 => {
                // CPython checks the encoding argument before the source
                // argument: if encoding is not a str, report the type; only
                // once encoding is confirmed to be a str do we check whether
                // source is also a str (and give "encoding without a string
                // argument" if not).
                let encoding: String = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    // CPython 3.12 formats the type name of the encoding
                    // argument as "None" (not "NoneType") for the None
                    // singleton — matching the singleton's display name rather
                    // than its class name.  All other types use the class name.
                    ValueKind::None => return Err(PyError::named(
                        "TypeError",
                        "bytes() argument 'encoding' must be str, not None".to_string(),
                    )),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "bytes() argument 'encoding' must be str, not {}",
                            value_type_name_str(&args[1].value),
                        ),
                    )),
                };
                let source: String = match args[0].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::named(
                        "TypeError",
                        "encoding without a string argument".to_string(),
                    )),
                };
                let errors: String = if args.len() == 3 {
                    match args[2].value.kind() {
                        ValueKind::Str(s) => s.to_string(),
                        // Same None special-case as encoding above.
                        ValueKind::None => return Err(PyError::named(
                            "TypeError",
                            "bytes() argument 'errors' must be str, not None".to_string(),
                        )),
                        _ => return Err(PyError::named(
                            "TypeError",
                            format!(
                                "bytes() argument 'errors' must be str, not {}",
                                value_type_name_str(&args[2].value),
                            ),
                        )),
                    }
                } else {
                    "strict".to_string()
                };
                encode_str_to_bytes(&source, &encoding, &errors)
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "bytes() takes at most 3 arguments ({} given)",
                    args.len()
                ),
            )),
        }
    }

    /// CPython: bytearray(...) — mutable bytes constructor.
    /// <https://docs.python.org/3/library/functions.html#func-bytearray>
    /// Mirrors `bytes()` but returns a mutable `bytearray` value.
    fn bytearray(args) -> Result<Value> {
        // CPython 3.12: bytearray(source, encoding, errors) — all three are
        // keyword-or-positional.
        let bound = bind_bytes_like_args(FN_NAME, args)?;
        let args = &bound[..];
        match args.len() {
            0 => Ok(pyrust_builtins::bytearray::bytearray(Vec::new())),
            1 => match args[0].value.kind() {
                ValueKind::Int(n) => {
                    if n < 0 {
                        return Err(PyError::named("ValueError", "negative count".to_string()));
                    }
                    Ok(pyrust_builtins::bytearray::bytearray(vec![0u8; n as usize]))
                }
                ValueKind::Bool(b) => Ok(pyrust_builtins::bytearray::bytearray(vec![0u8; b as usize])),
                ValueKind::Bytes(rc) => Ok(pyrust_builtins::bytearray::bytearray((**rc).clone())),
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
                {
                    // bytearray(bytearray) — copy
                    let snap = pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value)
                        .unwrap_or_default();
                    Ok(pyrust_builtins::bytearray::bytearray(snap))
                }
                ValueKind::Str(_) => Err(PyError::named(
                    "TypeError",
                    "string argument without an encoding".to_string(),
                )),
                ValueKind::BigInt(_) => Err(PyError::named(
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer".to_string(),
                )),
                _ => {
                    // General iterable or PyInstance path. CPython honors the
                    // __index__ count form (`bytearray(obj)` -> N zero bytes)
                    // before falling back to __iter__ (#1908).
                    if let Some(count) = bytes_count_via_index(_interp, &args[0].value)? {
                        return Ok(pyrust_builtins::bytearray::bytearray(vec![0u8; count]));
                    }
                    let type_name = pyrust_core::builtin_type_name(&args[0].value).into_owned();
                    let items = _interp.collect_iterable(&args[0].value).map_err(|e| {
                        if e.class_name_is("TypeError") {
                            PyError::named(
                                "TypeError",
                                format!("cannot convert '{type_name}' object to bytearray"),
                            )
                        } else {
                            e
                        }
                    })?;
                    Ok(pyrust_builtins::bytearray::bytearray(bytes_from_items(
                        _interp, items,
                    )?))
                }
            },
            2 | 3 => {
                // bytearray(source, encoding[, errors])
                let encoding: String = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    ValueKind::None => return Err(PyError::named(
                        "TypeError",
                        "bytearray() argument 'encoding' must be str, not None".to_string(),
                    )),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "bytearray() argument 'encoding' must be str, not {}",
                            value_type_name_str(&args[1].value),
                        ),
                    )),
                };
                let source: String = match args[0].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::named(
                        "TypeError",
                        "encoding without a string argument".to_string(),
                    )),
                };
                let errors: String = if args.len() == 3 {
                    match args[2].value.kind() {
                        ValueKind::Str(s) => s.to_string(),
                        ValueKind::None => return Err(PyError::named(
                            "TypeError",
                            "bytearray() argument 'errors' must be str, not None".to_string(),
                        )),
                        _ => return Err(PyError::named(
                            "TypeError",
                            format!(
                                "bytearray() argument 'errors' must be str, not {}",
                                value_type_name_str(&args[2].value),
                            ),
                        )),
                    }
                } else {
                    "strict".to_string()
                };
                // Reuse the string encoding logic from bytes, then wrap as bytearray.
                let bytes_val = encode_str_to_bytes(&source, &encoding, &errors)?;
                let data = match bytes_val.kind() {
                    ValueKind::Bytes(rc) => (**rc).clone(),
                    _ => unreachable!("encode_str_to_bytes returns bytes"),
                };
                Ok(pyrust_builtins::bytearray::bytearray(data))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("bytearray() takes at most 3 arguments ({} given)", args.len()),
            )),
        }
    }

}
