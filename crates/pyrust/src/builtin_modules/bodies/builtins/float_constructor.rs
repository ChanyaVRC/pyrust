use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: float(x=0.0) — float constructor.
    /// <https://docs.python.org/3/library/functions.html#float>
    /// This can run user code by dispatching `__float__` and
    /// `__index__` on user-defined objects.
    fn float(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::float(0.0)),
            1 => match args[0].value.kind() {
                ValueKind::Float(v) => Ok(Value::float(v)),
                ValueKind::Int(v) => Ok(Value::float(v as f64)),
                ValueKind::Bool(b) => Ok(Value::float(if b { 1.0 } else { 0.0 })),
                ValueKind::BigInt(b) => b
                    .to_f64()
                    .filter(|f| f.is_finite())
                    .map(Value::float)
                    .ok_or_else(|| {
                        PyError::named(
                            "OverflowError",
                            "int too large to convert to float".to_string(),
                        )
                    }),
                ValueKind::Str(s) => {
                    let err = || {
                        PyError::named(
                            "ValueError",
                            format!("could not convert string to float: '{s}'"),
                        )
                    };
                    // PEP 515: strip valid underscores, reject invalid placement.
                    let cleaned = pep515_strip_float(s.trim()).ok_or_else(err)?;
                    cleaned.parse::<f64>().map(Value::float).map_err(|_| err())
                }
                // bytes-like: decode as ASCII and parse identically to `str`
                // (#2077).  `bytearray` is a BuiltinObject handled by the
                // `_` guard below.
                ValueKind::Bytes(rc) => {
                    float_parse_bytes_like(rc.as_slice(), &args[0].value.repr_raw())
                }
                ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
                    let self_val = args[0].value.clone();
                    // Issue #1204: if the instance is a scalar-primitive subclass
                    // (MyFloat, MyInt, …) extract the backing value first.
                    // `float(MyFloat(3.14))` must return 3.14, not raise TypeError.
                    // A user-owned `__float__` slot wins; a canonical inherited
                    // slot keeps the backing shortcut.
                    if let Some(backing) =
                        coerce_subclass_backing(&self_val, &["__float__"])
                    {
                        match backing.kind() {
                            ValueKind::Float(v) => return Ok(Value::float(v)),
                            ValueKind::Int(v) => return Ok(Value::float(v as f64)),
                            ValueKind::Bool(b) => {
                                return Ok(Value::float(if b { 1.0 } else { 0.0 }));
                            }
                            ValueKind::BigInt(b) => {
                                return b
                                    .to_f64()
                                    .filter(|f| f.is_finite())
                                    .map(Value::float)
                                    .ok_or_else(|| {
                                        PyError::named(
                                            "OverflowError",
                                            "int too large to convert to float".to_string(),
                                        )
                                    });
                            }
                            _ => {}
                        }
                    }
                    // CPython 3.12 dispatch: __float__ → __index__
                    if let Some(method) = lookup_value_special_method(&self_val, "__float__") {
                        let result =
                            invoke_class_method(_interp, method, self_val.clone(), &[])?;
                        if let Some(normalized) = normalize_float_slot_result(&result) {
                            return Ok(normalized);
                        }
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{}.__float__ returned non-float (type {})",
                                value_type_name_str(&self_val),
                                value_type_name_str(&result),
                            ),
                        ));
                    }
                    if let Some(result) = _interp.try_value_to_index(&self_val)? {
                        return value_to_float(&result, "__SENTINEL__").map(Value::float);
                    }
                    Err(PyError::named(
                        "TypeError",
                        format!(
                            "float() argument must be a string or a real number, not '{}'",
                            value_type_name_str(&self_val)
                        ),
                    ))
                }
                // bytearray (a BuiltinObject) is bytes-like: decode + parse as
                // ASCII, same as the `bytes` arm above (#2077).
                _ if pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value).is_some() => {
                    let (data, repr) = float_bytes_like(&args[0].value).unwrap();
                    float_parse_bytes_like(&data, &repr)
                }
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "float() argument must be a string or a real number, not '{}'",
                        value_type_name_str(&args[0].value),
                    ),
                )),
            },
            _ => Err(PyError::named(
                "TypeError",
                format!("float expected at most 1 argument, got {}", args.len()),
            )),
        }
    }
}
