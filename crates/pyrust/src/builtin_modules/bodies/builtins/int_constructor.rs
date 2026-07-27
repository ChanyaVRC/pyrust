use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: int(x=0, base=10) — integer constructor.
    /// <https://docs.python.org/3/library/functions.html#int>
    /// This can run user code by dispatching `__int__`, `__index__`,
    /// and `__trunc__` on user-defined objects.
    fn int(args) -> Result<Value> {
        // CPython 3.12: int(x, /, base=10) — `x` positional-only, `base`
        // keyword-or-positional.  `int(x='5')` → invalid-keyword error;
        // `int('10', base=2)` is accepted.
        let bound = bind_constructor_kwargs(FN_NAME, args, &["x", "base"], &[false, true], 2)?;
        // `int(base=2)` (base supplied, value omitted): CPython raises
        // `int() missing string argument`, not the default-0 path.
        if bound[0].is_none() && bound[1].is_some() {
            return Err(PyError::named(
                "TypeError",
                "int() missing string argument".to_string(),
            ));
        }
        // Flatten to positional args (stop at the first unfilled slot — `int`
        // has no interior optional gaps once the missing-value case above is
        // handled).
        let mut bound_pos: Vec<ExpandedCallArg> = Vec::with_capacity(2);
        for slot in bound.into_iter() {
            match slot {
                Some(v) => bound_pos.push(ExpandedCallArg { name: None, value: v }),
                None => break,
            }
        }
        let args = &bound_pos[..];
        match args.len() {
            0 => Ok(Value::int(0)),
            1 => match args[0].value.kind() {
                ValueKind::Int(v) => Ok(Value::int(v)),
                ValueKind::BigInt(b) => Ok(Value::bigint((*b).clone())),
                ValueKind::Float(v) => {
                    if v.is_nan() {
                        return Err(PyError::named(
                            "ValueError",
                            "cannot convert float NaN to integer".to_string(),
                        ));
                    }
                    if v.is_infinite() {
                        return Err(PyError::named(
                            "OverflowError",
                            "cannot convert float infinity to integer".to_string(),
                        ));
                    }
                    let t = v.trunc();
                    if t >= i64::MAX as f64 || t < i64::MIN as f64 {
                        float_to_bigint(t)
                    } else {
                        Ok(Value::int(t as i64))
                    }
                }
                ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                ValueKind::Str(s) => {
                    let trimmed = s.trim();
                    let cleaned = int_strip_explicit_base(trimmed, 10).ok_or_else(|| {
                        PyError::named(
                            "ValueError",
                            format!("invalid literal for int() with base 10: '{s}'"),
                        )
                    })?;
                    pyrust_core::check_int_parse_digits(&cleaned, 10)?;
                    match cleaned.parse::<i64>() {
                        Ok(v) => Ok(Value::int(v)),
                        Err(_) => {
                            // Overflow: try BigInt before giving up.
                            use num_traits::Num as _;
                            crate::value::PyBigInt::from_str_radix(&cleaned, 10)
                                .map(Value::bigint)
                                .map_err(|_| PyError::named(
                                    "ValueError",
                                    format!("invalid literal for int() with base 10: '{s}'"),
                                ))
                        }
                    }
                }
                ValueKind::Bytes(rc) => {
                    int_parse_bytes_like(rc.as_slice(), &args[0].value.repr_raw(), 10)
                }
                ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
                    let self_val = args[0].value.clone();
                    // Issue #1204: if the instance is a scalar-primitive subclass
                    // (MyInt, MyFloat, …) extract the backing value first.
                    // `int(MyInt(42))` must return 42, not raise TypeError.
                    // A user-owned `__int__` slot blocks this shortcut; only a
                    // slot inherited from its canonical primitive owner may
                    // reuse the backing directly.
                    if let Some(backing) =
                        coerce_subclass_backing(&self_val, &["__int__"])
                    {
                        let result: Option<Result<Value>> = match backing.kind() {
                            ValueKind::Int(v) => Some(Ok(Value::int(v))),
                            ValueKind::BigInt(_) => Some(Ok(backing.clone())),
                            ValueKind::Bool(b) => Some(Ok(Value::int(if b { 1 } else { 0 }))),
                            ValueKind::Float(v) => {
                                if v.is_nan() {
                                    Some(Err(PyError::named(
                                        "ValueError",
                                        "cannot convert float NaN to integer".to_string(),
                                    )))
                                } else if v.is_infinite() {
                                    Some(Err(PyError::named(
                                        "OverflowError",
                                        "cannot convert float infinity to integer".to_string(),
                                    )))
                                } else {
                                    let t = v.trunc();
                                    if t >= i64::MAX as f64 || t < i64::MIN as f64 {
                                        Some(float_to_bigint(t))
                                    } else {
                                        Some(Ok(Value::int(t as i64)))
                                    }
                                }
                            }
                            _ => None,
                        };
                        if let Some(v) = result {
                            return v;
                        }
                    }
                    // CPython 3.12 dispatch: __int__ → __index__ → __trunc__
                    if let Some(method) = lookup_value_special_method(&self_val, "__int__") {
                        let result =
                            invoke_class_method(_interp, method, self_val.clone(), &[])?;
                        if let Some(normalized) = normalize_int_slot_result(&result) {
                            return Ok(normalized);
                        }
                        return Err(PyError::named(
                            "TypeError",
                            format!("__int__ returned non-int (type {})", value_type_name_str(&result)),
                        ));
                    }
                    if let Some(result) = _interp.try_value_to_index(&self_val)? {
                        return Ok(normalize_int_slot_result(&result)
                            .expect("try_value_to_index guarantees an integer"));
                    }
                    if let Some(method) = lookup_value_special_method(&self_val, "__trunc__") {
                        // Deprecated since 3.11 but still works in 3.12; call int() on the result.
                        let trunc_result =
                            invoke_class_method(_interp, method, self_val.clone(), &[])?;
                        let Some(normalized) = _interp.try_value_to_index(&trunc_result)? else {
                            // CPython 3.12: float is not an Integral type — any float returned
                            // from __trunc__ (including inf/nan) raises TypeError, not
                            // OverflowError/ValueError.  The inf/nan guards belong only in the
                            // direct float-to-int conversion paths, not here.
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__trunc__ returned non-Integral (type {})",
                                    value_type_name_str(&trunc_result)
                                ),
                            ));
                        };
                        return Ok(normalize_int_slot_result(&normalized)
                            .expect("try_value_to_index guarantees an integer"));
                    }
                    Err(PyError::named(
                        "TypeError",
                        format!(
                            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                            value_type_name_str(&self_val)
                        ),
                    ))
                }
                // bytearray (a BuiltinObject) is bytes-like: decode + parse as
                // base-10 ASCII, same as the `bytes` arm above (#2077).  Note
                // CPython's `int()` error uses the *bytes* repr (`b'…'`) even
                // for a bytearray operand, so render from the byte data.
                _ if pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value).is_some() => {
                    let data =
                        pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value).unwrap();
                    let repr = Value::bytes(data.clone()).repr_raw();
                    int_parse_bytes_like(&data, &repr, 10)
                }
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                        value_type_name_str(&args[0].value),
                    ),
                )),
            },
            2 => {
                let resolved_base = _interp.value_to_index(&args[1].value, |value| {
                    PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object cannot be interpreted as an integer",
                            value_type_name_str(value),
                        ),
                    )
                })?;
                let base_arg = match resolved_base.kind() {
                    ValueKind::Int(base) => base,
                    ValueKind::Bool(base) => base as i64,
                    ValueKind::BigInt(_) => {
                        return Err(PyError::named(
                            "ValueError",
                            "int() base must be >= 2 and <= 36, or 0",
                        ));
                    }
                    _ => unreachable!("value_to_index guarantees an integer"),
                };
                if base_arg != 0 && !(2..=36).contains(&base_arg) {
                    return Err(PyError::named(
                        "ValueError",
                        "int() base must be >= 2 and <= 36, or 0",
                    ));
                }
                match args[0].value.kind() {
                    ValueKind::Str(s) => {
                        let trimmed = s.trim();
                        if base_arg == 0 {
                            let (base, digits) = int_parse_base_zero(trimmed).ok_or_else(|| {
                                PyError::named(
                                    "ValueError",
                                    format!("invalid literal for int() with base 0: '{s}'"),
                                )
                            })?;
                            pyrust_core::check_int_parse_digits(&digits, base)?;
                            match i64::from_str_radix(&digits, base) {
                                Ok(v) => Ok(Value::int(v)),
                                Err(_) => {
                                    // Overflow: try BigInt before giving up.
                                    use num_traits::Num as _;
                                    crate::value::PyBigInt::from_str_radix(&digits, base)
                                        .map(Value::bigint)
                                        .map_err(|_| PyError::named(
                                            "ValueError",
                                            format!("invalid literal for int() with base 0: '{s}'"),
                                        ))
                                }
                            }
                        } else {
                            let base = base_arg as u32;
                            let stripped = int_strip_explicit_base(trimmed, base).ok_or_else(|| {
                                PyError::named(
                                    "ValueError",
                                    format!("invalid literal for int() with base {base_arg}: '{s}'"),
                                )
                            })?;
                            pyrust_core::check_int_parse_digits(&stripped, base)?;
                            match i64::from_str_radix(&stripped, base) {
                                Ok(v) => Ok(Value::int(v)),
                                Err(_) => {
                                    // Overflow: try BigInt before giving up.
                                    use num_traits::Num as _;
                                    crate::value::PyBigInt::from_str_radix(&stripped, base)
                                        .map(Value::bigint)
                                        .map_err(|_| PyError::named(
                                            "ValueError",
                                            format!("invalid literal for int() with base {base_arg}: '{s}'"),
                                        ))
                                }
                            }
                        }
                    }
                    ValueKind::Bytes(rc) => {
                        int_parse_bytes_like(rc.as_slice(), &args[0].value.repr_raw(), base_arg)
                    }
                    // bytearray with explicit base — bytes-like, parse as ASCII
                    // (#2077).  As above, CPython's `int()` error uses the
                    // bytes repr for a bytearray operand.
                    _ if pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value)
                        .is_some() =>
                    {
                        let data = pyrust_builtins::bytearray::as_bytearray_snapshot(
                            &args[0].value,
                        )
                        .unwrap();
                        let repr = Value::bytes(data.clone()).repr_raw();
                        int_parse_bytes_like(&data, &repr, base_arg)
                    }
                    _ => Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() can't convert non-string with explicit base"),
                    )),
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("int() takes at most 2 arguments ({} given)", args.len()),
            )),
        }
    }

}
