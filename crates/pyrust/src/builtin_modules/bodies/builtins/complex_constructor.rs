use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: complex(real=0, imag=0) — complex constructor.
    /// <https://docs.python.org/3/library/functions.html#complex>
    /// This can run user code by dispatching `__complex__`,
    /// `__float__`, and `__index__` on user-defined objects.
    fn complex(args) -> Result<Value> {
        // CPython 3.12: complex(real=0, imag=0) — both keyword-or-positional.
        let bound = bind_constructor_kwargs(FN_NAME, args, &["real", "imag"], &[true, true], 2)?;
        // Flatten to positional args.  If `imag` is supplied but `real` is
        // omitted (`complex(imag=5)`), CPython treats `real` as its default
        // `0` — fill the interior gap so the two-arg path runs.
        let mut bound_pos: Vec<ExpandedCallArg> = Vec::with_capacity(2);
        if bound[1].is_some() && bound[0].is_none() {
            bound_pos.push(ExpandedCallArg { name: None, value: Value::int(0) });
            bound_pos.push(ExpandedCallArg { name: None, value: bound[1].clone().unwrap() });
        } else {
            for slot in bound.into_iter() {
                match slot {
                    Some(v) => bound_pos.push(ExpandedCallArg { name: None, value: v }),
                    None => break,
                }
            }
        }
        let args = &bound_pos[..];

        // Convert a primitive (non-PyInstance) Value to f64.
        // `type_err_msg` is the full TypeError message to emit for unrecognised kinds.
        let prim_to_f64 = |v: &Value, type_err_msg: &str| -> Result<f64> {
            match v.kind() {
                ValueKind::Int(n) => Ok(n as f64),
                ValueKind::Float(f) => Ok(f),
                ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
                ValueKind::BigInt(b) => {
                    let f = b.to_f64().unwrap_or(f64::INFINITY);
                    if f.is_finite() {
                        Ok(f)
                    } else {
                        Err(PyError::named(
                            "OverflowError",
                            "int too large to convert to float",
                        ))
                    }
                }
                _ => Err(PyError::named(
                    "TypeError",
                    format!("{type_err_msg}, not '{}'", value_type_name_str(v)),
                )),
            }
        };

        match args.len() {
            0 => Ok(Value::complex(0.0, 0.0)),
            1 => match args[0].value.kind() {
                ValueKind::Complex(re, im) => Ok(Value::complex(re, im)),
                ValueKind::Str(s) => {
                    let (re, im) = parse_complex_str(s).ok_or_else(|| {
                        PyError::named("ValueError", "complex() arg is a malformed string")
                    })?;
                    Ok(Value::complex(re, im))
                }
                ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
                    let self_val = args[0].value.clone();
                    // CPython 3.12 dispatch: __complex__ → __float__ → __index__
                    // A complex subclass with no user-owned `__complex__` is
                    // already a complex value. Its backing wins over
                    // `__float__`, matching PyComplex_Check.
                    let complex_backing =
                        coerce_subclass_backing(&self_val, &["__complex__"]);
                    if let Some(backing) = &complex_backing
                        && let ValueKind::Complex(real, imag) = backing.kind()
                    {
                        return Ok(Value::complex(real, imag));
                    }
                    if complex_backing.is_none()
                        && let Some(method) = lookup_value_special_method(
                            &self_val,
                            "__complex__",
                        )
                        .transpose()?
                    {
                        let result =
                            invoke_class_method(_interp, method, self_val.clone(), &[])?;
                        return normalize_complex_slot_result(&result).ok_or_else(|| {
                            PyError::named(
                                "TypeError",
                                format!(
                                    "__complex__ returned non-complex (type {})",
                                    value_type_name_str(&result)
                                ),
                            )
                        });
                    }

                    // For int/float subclasses, a user-owned `__float__` wins;
                    // otherwise convert their primitive backing without
                    // invoking the canonical descriptor.
                    if let Some(backing) =
                        coerce_subclass_backing(&self_val, &["__float__"])
                        && matches!(
                            backing.kind(),
                            ValueKind::Int(_)
                                | ValueKind::Bool(_)
                                | ValueKind::BigInt(_)
                                | ValueKind::Float(_)
                        )
                    {
                        return value_to_float(&backing, "__SENTINEL__")
                            .map(|value| Value::complex(value, 0.0));
                    }
                    if let Some(method) =
                        lookup_value_special_method(&self_val, "__float__").transpose()?
                    {
                        let result =
                            invoke_class_method(_interp, method, self_val.clone(), &[])?;
                        return if let Some(normalized) =
                            normalize_float_slot_result(&result)
                        {
                            let ValueKind::Float(f) = normalized.kind() else {
                                unreachable!("normalize_float_slot_result guarantees float")
                            };
                            Ok(Value::complex(f, 0.0))
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                format!(
                                    "{}.__float__ returned non-float (type {})",
                                    value_type_name_str(&self_val),
                                    value_type_name_str(&result),
                                ),
                            ))
                        };
                    }
                    if let Some(result) = _interp.try_value_to_index(&self_val)? {
                        return value_to_float(&result, "__SENTINEL__")
                            .map(|value| Value::complex(value, 0.0));
                    }
                    Err(PyError::named(
                        "TypeError",
                        format!(
                            "complex() first argument must be a string or a number, not '{}'",
                            value_type_name_str(&self_val)
                        ),
                    ))
                }
                _ => Ok(Value::complex(
                    prim_to_f64(&args[0].value, "complex() first argument must be a string or a number")?,
                    0.0,
                )),
            },
            2 => {
                if matches!(args[0].value.kind(), ValueKind::Str(_)) {
                    return Err(PyError::named(
                        "TypeError",
                        "complex() can't take second arg if first is a string",
                    ));
                }
                if matches!(args[1].value.kind(), ValueKind::Str(_)) {
                    return Err(PyError::named(
                        "TypeError",
                        "complex() second arg can't be a string",
                    ));
                }
                // Resolve each arg to a (re, im) pair.
                // First arg: __complex__ yields (re, im); __float__/__index__ → scalar.
                // Second arg: __float__/__index__ only (CPython ignores __complex__ there).
                let first_val = args[0].value.clone();
                let second_val = args[1].value.clone();

                // Helper: call __float__ then __index__ on a user object and return f64.
                // $no_conv_msg is the prefix of the TypeError when no suitable dunder is found;
                // the runtime type name is appended as ", not '<name>'".
                macro_rules! inst_to_f64 {
                    ($self_val:expr, $no_conv_msg:literal) => {{
                        let self_val: &Value = $self_val;
                        // A real builtin subclass may use its backing directly
                        // only while `__float__` is inherited from a canonical
                        // primitive owner. A user-owned slot (including a
                        // copied builtin descriptor) must still be invoked.
                        if let Some(backing) =
                            coerce_subclass_backing(self_val, &["__float__"])
                            && matches!(
                                backing.kind(),
                                ValueKind::Int(_)
                                    | ValueKind::Bool(_)
                                    | ValueKind::BigInt(_)
                                    | ValueKind::Float(_)
                            )
                        {
                            value_to_float(&backing, "__SENTINEL__")?
                        } else if let Some(method) =
                            lookup_value_special_method(self_val, "__float__").transpose()?
                        {
                            let result =
                                invoke_class_method(_interp, method, self_val.clone(), &[])?;
                            if let Some(normalized) = normalize_float_slot_result(&result) {
                                let ValueKind::Float(f) = normalized.kind() else {
                                    unreachable!(
                                        "normalize_float_slot_result guarantees float"
                                    )
                                };
                                f
                            } else {
                                return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "{}.__float__ returned non-float (type {})",
                                        value_type_name_str(self_val),
                                        value_type_name_str(&result),
                                    ),
                                ));
                            }
                        } else if let Some(result) = _interp.try_value_to_index(self_val)? {
                            value_to_float(&result, "__SENTINEL__")?
                        } else {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "{}, not '{}'",
                                    $no_conv_msg,
                                    value_type_name_str(self_val),
                                ),
                            ));
                        }
                    }};
                }

                // Resolve first arg to (re, im) pair.
                let (cr, ci, first_is_complex) = if let ValueKind::Complex(re, im) = first_val.kind() {
                    (re, im, true)
                } else if matches!(
                    first_val.kind(),
                    ValueKind::PyInstance(_) | ValueKind::PyClass(_)
                ) {
                    let complex_backing =
                        coerce_subclass_backing(&first_val, &["__complex__"]);
                    if let Some(backing) = &complex_backing
                        && let ValueKind::Complex(re, im) = backing.kind()
                    {
                        (re, im, true)
                    } else if complex_backing.is_none()
                        && let Some(method) = lookup_value_special_method(
                            &first_val,
                            "__complex__",
                        )
                        .transpose()?
                    {
                        let result =
                            invoke_class_method(_interp, method, first_val.clone(), &[])?;
                        if let Some(normalized) = normalize_complex_slot_result(&result) {
                            let ValueKind::Complex(re, im) = normalized.kind() else {
                                unreachable!(
                                    "normalize_complex_slot_result guarantees complex"
                                )
                            };
                            (re, im, true)
                        } else {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__complex__ returned non-complex (type {})",
                                    value_type_name_str(&result)
                                ),
                            ));
                        }
                    } else {
                        let f = inst_to_f64!(
                            &first_val,
                            "complex() first argument must be a string or a number"
                        );
                        (f, 0.0, false)
                    }
                } else {
                    let f = prim_to_f64(
                        &first_val,
                        "complex() first argument must be a string or a number",
                    )?;
                    (f, 0.0, false)
                };

                // Resolve second arg to (re, im) pair (no __complex__ for second arg).
                let (dr, di, second_is_complex) = if let ValueKind::Complex(re, im) = second_val.kind() {
                    (re, im, true)
                } else if matches!(
                    second_val.kind(),
                    ValueKind::PyInstance(_) | ValueKind::PyClass(_)
                ) {
                    // The imaginary-position operand ignores `__complex__`
                    // and, for a complex subclass, also ignores `__float__`:
                    // CPython structurally extracts its complex backing.
                    if let Some(backing) = coerce_subclass_backing(&second_val, &[])
                        && let ValueKind::Complex(re, im) = backing.kind()
                    {
                        (re, im, true)
                    } else {
                        let f = inst_to_f64!(
                            &second_val,
                            "complex() second argument must be a number"
                        );
                        (f, 0.0, false)
                    }
                } else {
                    let f = prim_to_f64(&second_val, "complex() second argument must be a number")?;
                    (f, 0.0, false)
                };

                // CPython decomposition formula (Objects/complexobject.c):
                // When at least one arg is complex, apply:
                //   result.real = cr - di
                //   result.imag = ci + dr
                // When neither is complex, assign directly (preserving -0.0 sign).
                if first_is_complex || second_is_complex {
                    Ok(Value::complex(cr - di, ci + dr))
                } else {
                    Ok(Value::complex(cr, dr))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 2 arguments ({} given)", args.len()),
            )),
        }
    }

}
