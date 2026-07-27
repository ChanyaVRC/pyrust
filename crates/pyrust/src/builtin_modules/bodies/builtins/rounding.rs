use pyrust_derive::pyrust_module;

/// Resolve primitive `round(..., ndigits)` arguments through the shared index
/// protocol, then apply round's consumer-specific i32 saturation.
///
/// User-defined `__round__` receives its original `ndigits` unchanged and
/// therefore does not call this helper. Both primitive values and primitive
/// subclasses without an override do, keeping their coercion and diagnostics
/// identical.
fn round_ndigits_i32(interp: &mut Interpreter, ndigits: Option<&PyValue>) -> Result<Option<i32>> {
    let Some(ndigits) = ndigits else {
        return Ok(None);
    };
    if matches!(ndigits.0.kind(), ValueKind::None) {
        return Ok(None);
    }
    let resolved = interp.value_to_index(&ndigits.0, |value| {
        pyrust_core::type_err!(
            "'{}' object cannot be interpreted as an integer",
            value_type_name_str(value),
        )
    })?;
    let value = match resolved.kind() {
        // Clamp instead of casting so large magnitudes cannot wrap, and avoid
        // producing i32::MIN (the rounding paths negate negative ndigits).
        ValueKind::Int(value) => value.clamp(-(i32::MAX as i64), i32::MAX as i64) as i32,
        ValueKind::Bool(value) => value as i32,
        // A ValueKind::BigInt lies outside the tagged i64 range. Round only
        // needs its sign because either magnitude saturates the i32 consumer.
        ValueKind::BigInt(value) => {
            if value.sign() == num_bigint::Sign::Minus {
                -i32::MAX
            } else {
                i32::MAX
            }
        }
        _ => unreachable!("value_to_index guarantees an integer"),
    };
    Ok(Some(value))
}

pyrust_module! {
    /// CPython: round(number, ndigits=None) — banker's rounding.
    /// <https://docs.python.org/3/library/functions.html#round>
    ///
    /// Both `number` and `ndigits` are keyword-or-positional in CPython 3.12
    /// (#2180), so this uses the raw-`args` form + `bind_constructor_kwargs`
    /// rather than the typed-signature dialect (whose unknown-keyword wording
    /// differs from the C-clinic "invalid keyword argument" message round
    /// emits).  `x`/`ndigits` are resolved to `PyValue` so the body can
    /// dispatch on `ValueKind` for full CPython parity: `bool ⊆ int` (both
    /// round unchanged), `float` uses half-even rounding, and everything else
    /// raises `TypeError`.
    fn round(args) -> Result<Value> {
        // CPython 3.12: round(number, ndigits=None) — both `number` and
        // `ndigits` are keyword-or-positional.  Bind kwargs (with the
        // C-clinic "invalid keyword argument" wording) before the existing
        // positional dispatch below.
        //
        // Unlike the int/complex/str constructors, `round` is an argument-clinic
        // function whose *missing required positional* check precedes the
        // unknown-keyword check: `round(x=3.5)` / `round(foo=1)` report
        // "missing required argument 'number'", not "'x'/'foo' is an invalid
        // keyword argument".  The arity-overflow check still comes first, so
        // mirror CPython's exact order: arity, then missing-number, then the
        // generic keyword binding (which surfaces the remaining errors).
        if args.len() > 2 {
            let noun = if args.iter().all(|a| a.name.is_some()) {
                "keyword arguments"
            } else {
                "arguments"
            };
            return Err(PyError::named(
                "TypeError",
                format!("round() takes at most 2 {noun} ({} given)", args.len()),
            ));
        }
        let number_bound = args
            .iter()
            .any(|a| a.name.is_none() || a.name.as_deref() == Some("number"));
        if !number_bound {
            return Err(PyError::named(
                "TypeError",
                "round() missing required argument 'number' (pos 1)".to_string(),
            ));
        }
        let bound =
            bind_constructor_kwargs(FN_NAME, args, &["number", "ndigits"], &[true, true], 2)?;
        // `number` was confirmed bound above; the slot is always populated here.
        let x: PyValue = PyValue(bound[0].clone().expect("number slot bound"));
        let ndigits: Option<PyValue> = bound[1].clone().map(PyValue);
        // Classify x first so we can decide whether to validate ndigits.
        // CPython forwards any ndigits type to user-defined __round__ without
        // pre-validating it (round(obj, 3.5) passes 3.5 to __round__), but
        // raises TypeError for non-int ndigits on all primitive types.
        enum NumKind { Int(i64), Bool(bool), BigInt, Float(f64), Other }
        let num = match x.0.kind() {
            ValueKind::Int(v) => NumKind::Int(v),
            ValueKind::Bool(b) => NumKind::Bool(b),
            ValueKind::BigInt(_) => NumKind::BigInt,
            ValueKind::Float(v) => NumKind::Float(v),
            _ => NumKind::Other,
        };
        // Validate ndigits type for primitive dispatches only.
        let ndigits_i32: Option<i32> = if matches!(num, NumKind::Other) {
            // Deferred: for user objects ndigits is passed as-is to __round__.
            None
        } else {
            round_ndigits_i32(_interp, ndigits.as_ref())?
        };
        match num {
            NumKind::Int(v) => match ndigits_i32 {
                Some(n) if n < 0 => Ok(round_bigint_neg_ndigits(PyBigInt::from(v), (-n) as u32)),
                _ => Ok(Value::int(v)),
            },
            NumKind::Bool(b) => {
                let v: i64 = if b { 1 } else { 0 };
                match ndigits_i32 {
                    Some(n) if n < 0 => Ok(round_bigint_neg_ndigits(PyBigInt::from(v), (-n) as u32)),
                    _ => Ok(Value::int(v)),
                }
            }
            NumKind::BigInt => {
                if let ValueKind::BigInt(b) = x.0.kind() {
                    match ndigits_i32 {
                        Some(n) if n < 0 => {
                            Ok(round_bigint_neg_ndigits(b.clone(), (-n) as u32))
                        }
                        _ => Ok(Value::bigint(b.clone())),
                    }
                } else {
                    unreachable!()
                }
            }
            NumKind::Float(v) => match ndigits_i32 {
                None => Ok(Value::int(py_round_half_even_checked(v)?)),
                Some(n) => round_float_ndigits(v, n),
            },
            NumKind::Other => {
                // Check for user-defined __round__ before raising TypeError.
                // CPython does not validate the ndigits type for user objects;
                // ndigits is forwarded as-is to __round__.
                if let ValueKind::PyInstance(inst) = x.0.kind() {
                    let inst_rc = Rc::clone(inst);
                    let class = Rc::clone(&inst_rc.borrow().class);
                    if let Some(method_val) = lookup_class_attr(&class, "__round__") {
                        // CPython: __round__() is called with no args when ndigits
                        // is absent or None; otherwise the original ndigits value
                        // (which may be a bool) is forwarded as-is.
                        let call_args: Vec<ExpandedCallArg> = match ndigits {
                            None => vec![],
                            Some(ref v) if matches!(v.0.kind(), ValueKind::None) => vec![],
                            Some(ref v) => vec![ExpandedCallArg {
                                name: None,
                                value: v.0.clone(),
                            }],
                        };
                        return invoke_class_method(
                            _interp,
                            method_val,
                            Value::py_instance(inst_rc),
                            &call_args,
                        );
                    }
                    // No user-defined __round__ found: if this is an int/float
                    // subclass, coerce to the backing primitive and re-dispatch
                    // using the same rounding logic as the primitive arms above.
                    // This matches CPython's inherited int.__round__ / float.__round__
                    // behaviour for subclasses that don't override __round__.
                    let coerced = coerce_numeric(&x.0);
                    let ndigits_i32_coerced =
                        round_ndigits_i32(_interp, ndigits.as_ref())?;
                    match coerced.kind() {
                        ValueKind::Int(v) => return match ndigits_i32_coerced {
                            Some(n) if n < 0 => Ok(round_bigint_neg_ndigits(PyBigInt::from(v), (-n) as u32)),
                            _ => Ok(Value::int(v)),
                        },
                        ValueKind::BigInt(b) => return match ndigits_i32_coerced {
                            Some(n) if n < 0 => Ok(round_bigint_neg_ndigits(b.clone(), (-n) as u32)),
                            _ => Ok(Value::bigint(b.clone())),
                        },
                        ValueKind::Float(v) => return match ndigits_i32_coerced {
                            None => Ok(Value::int(py_round_half_even_checked(v)?)),
                            Some(n) => round_float_ndigits(v, n),
                        },
                        _ => {}
                    }
                }
                Err(PyError::named(
                    "TypeError",
                    format!("type {} doesn't define __round__ method", value_type_name_str(&x.0)),
                ))
            }
        }
    }
}
