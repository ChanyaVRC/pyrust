use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: abs(x) — absolute value.
    /// <https://docs.python.org/3/library/functions.html#abs>
    ///
    /// First builtin migrated to the overload-dispatch dialect (#395):
    /// declare one `fn abs` per concrete arg-type combination, with
    /// `PyValue` as the trailing catch-all for `complex`, `PyInstance`
    /// with `__abs__`, and the not-a-number error path.  The macro
    /// generates a dispatcher that tries each overload in declaration
    /// order.
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `abs() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn abs(#[positional_only] x: PyInt) -> Result<Value> {
        // i64 fast path; fall back to BigInt for both genuine bignums
        // *and* the `i64::MIN` boundary case — `i64::MIN.checked_abs()`
        // returns `None` because `-i64::MIN` doesn't fit in i64 (one
        // more positive value than negative in two's complement).
        // CPython returns `9223372036854775808` for `abs(-i64::MIN)`;
        // matching that requires the BigInt path even though `as_i64`
        // succeeded.  We can't call `BigInt::abs` here without pulling
        // `num_traits` in as a direct dep, so do the negate-if-negative
        // dance manually.
        if let Some(n) = x.as_i64()
            && let Some(abs) = n.checked_abs()
        {
            return Ok(Value::int(abs));
        }
        let big = x.to_bigint();
        let zero: crate::value::PyBigInt = 0i64.into();
        let abs = if big < zero { -big } else { big };
        Ok(Value::bigint(abs))
    }

    #[arity_style(takes_exactly_one)]
    fn abs(#[positional_only] x: PyFloat) -> Result<Value> {
        Ok(Value::float(x.0.abs()))
    }

    #[arity_style(takes_exactly_one)]
    fn abs(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: abs(True) == 1, abs(False) == 0 — promoted to int.
        Ok(Value::int(if x.0 { 1 } else { 0 }))
    }

    #[arity_style(takes_exactly_one)]
    fn abs(#[positional_only] x: PyValue) -> Result<Value> {
        // Catch-all: complex magnitude, user-defined `__abs__`, and the
        // "not a number" error otherwise.  Reached when none of the
        // typed overloads above matched the call's argument.
        let val = x.0;
        if let ValueKind::PyInstance(inst) = val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__abs__") {
                return invoke_class_method(
                    _interp,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[],
                );
            }
            // Issue #1204: fall through to the scalar backing value for
            // primitive subclasses (MyInt, MyFloat, …) that have no user
            // __abs__ defined.
            if let Some(backing) = instance_builtin_data(&inst_rc) {
                match backing.kind() {
                    ValueKind::Int(v) => {
                        // i64::MIN.checked_abs() returns None because
                        // -i64::MIN overflows i64; promote to BigInt to
                        // match CPython (mirrors the PyInt arm above).
                        return Ok(match v.checked_abs() {
                            Some(abs) => Value::int(abs),
                            None => {
                                let big: crate::value::PyBigInt = v.into();
                                Value::bigint(-big)
                            }
                        });
                    }
                    ValueKind::BigInt(v) => {
                        let zero: crate::value::PyBigInt = 0i64.into();
                        let abs = if v < &zero { -v.clone() } else { v.clone() };
                        return Ok(Value::bigint(abs));
                    }
                    ValueKind::Float(v) => return Ok(Value::float(v.abs())),
                    ValueKind::Bool(b) => return Ok(Value::int(if b { 1 } else { 0 })),
                    _ => {}
                }
            }
            return Err(PyError::named(
                "TypeError",
                format!("bad operand type for abs(): '{}'", class.borrow().name),
            ));
        }
        if let ValueKind::Complex(re, im) = val.kind() {
            // Overflow/underflow-safe magnitude (CPython's `_Py_c_abs` uses
            // `hypot`): `(re*re + im*im).sqrt()` spuriously yields inf/0.0 for
            // huge/tiny components.  `f64::hypot` matches CPython byte-for-byte,
            // including inf-beats-nan on infinite components.
            return Ok(Value::float(re.hypot(im)));
        }
        Err(PyError::named(
            "TypeError",
            format!("bad operand type for abs(): '{}'", value_type_name_str(&val)),
        ))
    }

    /// CPython: divmod(a, b) — `(a // b, a % b)`.
    /// <https://docs.python.org/3/library/functions.html#divmod>
    ///
    /// Migrated to the typed-signature dialect (#400/#2331):
    /// `#[arity_style(expected_got)]` reproduces CPython's METH_VARARGS
    /// wording (`divmod expected 2 arguments, got N`) that previously
    /// forced the raw `(args)` dispatch style.  Type dispatch for the
    /// primitive fast paths (int/bool/float combinations) is done inline by
    /// kind-matching; the dunder-dispatch and coerce_numeric fallback paths
    /// are unchanged.
    #[arity_style(expected_got)]
    fn divmod(
        #[positional_only] a: PyValue,
        #[positional_only] b: PyValue,
    ) -> Result<Value> {
        let a = &a.0;
        let b = &b.0;

        // Fast paths: primitive int/bool/float combinations, mirroring the
        // former typed overloads.  bool ⊆ int in CPython: bool arms coerce
        // to i64 before calling the int helper.
        let a_is_bool = a.is_bool();
        let b_is_bool = b.is_bool();
        let a_is_int = matches!(a.kind(), ValueKind::Int(_) | ValueKind::BigInt(_));
        let b_is_int = matches!(b.kind(), ValueKind::Int(_) | ValueKind::BigInt(_));
        let a_is_float = matches!(a.kind(), ValueKind::Float(_));
        let b_is_float = matches!(b.kind(), ValueKind::Float(_));

        if (a_is_int || a_is_bool) && (b_is_int || b_is_bool) {
            // Both are int-family (int or bool); no float involved.
            let ia = if a_is_bool {
                PyInt::from(if let ValueKind::Bool(v) = a.kind() { v as i64 } else { 0 })
            } else {
                PyInt::try_from_value(a, "divmod", "a")?
            };
            let ib = if b_is_bool {
                PyInt::from(if let ValueKind::Bool(v) = b.kind() { v as i64 } else { 0 })
            } else {
                PyInt::try_from_value(b, "divmod", "b")?
            };
            return divmod_int_int(ia, ib);
        }

        if (a_is_float || a_is_int || a_is_bool) && (b_is_float || b_is_int || b_is_bool) {
            // At least one operand is float (since the all-int case was handled
            // above); promote both to f64.
            let af = if a_is_float {
                if let ValueKind::Float(f) = a.kind() { f } else { unreachable!() }
            } else if a_is_bool {
                if let ValueKind::Bool(v) = a.kind() { v as i64 as f64 } else { unreachable!() }
            } else {
                let pi = PyInt::try_from_value(a, "divmod", "a")?;
                pyint_to_f64(&pi)?
            };
            let bf = if b_is_float {
                if let ValueKind::Float(f) = b.kind() { f } else { unreachable!() }
            } else if b_is_bool {
                if let ValueKind::Bool(v) = b.kind() { v as i64 as f64 } else { unreachable!() }
            } else {
                let pi = PyInt::try_from_value(b, "divmod", "b")?;
                pyint_to_f64(&pi)?
            };
            return divmod_float_float(af, bf);
        }

        // Dunder dispatch: consult `__divmod__` / `__rdivmod__` before raising
        // TypeError.  CPython's `PyNumber_Divmod` (Objects/abstract.c) tries
        // `nb_divmod` on the left operand first, then the right operand's slot,
        // and only raises `TypeError` when both return `NotImplemented` or are
        // absent.
        //
        // Subtype rule (mirrors CPython `binary_op1`): when `b`'s type is a
        // *proper* subtype of `a`'s type, try `b.__rdivmod__(a)` first.
        let a_class = if let ValueKind::PyInstance(inst) = a.kind() {
            Some(Rc::clone(&inst.borrow().class))
        } else {
            None
        };
        let b_class = if let ValueKind::PyInstance(inst) = b.kind() {
            Some(Rc::clone(&inst.borrow().class))
        } else {
            None
        };

        let b_is_proper_subtype_of_a = match (&a_class, &b_class) {
            (Some(ac), Some(bc)) => {
                !Rc::ptr_eq(ac, bc)
                    && class_is_subclass_of(bc, ac)
                    && bc.borrow().attrs.contains_key("__rdivmod__")
            }
            _ => false,
        };

        if b_is_proper_subtype_of_a
            && let (Some(bc), ValueKind::PyInstance(inst)) = (&b_class, b.kind())
                && let Some(m) = lookup_class_attr(bc, "__rdivmod__") {
                    let self_val = Value::py_instance(Rc::clone(inst));
                    let arg = ExpandedCallArg { name: None, value: a.clone() };
                    match invoke_class_method(_interp, m, self_val, &[arg]) {
                        Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }

        // Try a.__divmod__(b).
        if let (Some(ac), ValueKind::PyInstance(inst)) = (&a_class, a.kind())
            && let Some(m) = lookup_class_attr(ac, "__divmod__") {
                let self_val = Value::py_instance(Rc::clone(inst));
                let arg = ExpandedCallArg { name: None, value: b.clone() };
                match invoke_class_method(_interp, m, self_val, &[arg]) {
                    Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                    Err(e) => return Err(e),
                    _ => {}
                }
            }

        // Try b.__rdivmod__(a) (skipped above if already tried via subtype rule).
        if !b_is_proper_subtype_of_a
            && let (Some(bc), ValueKind::PyInstance(inst)) = (&b_class, b.kind())
                && let Some(m) = lookup_class_attr(bc, "__rdivmod__") {
                    let self_val = Value::py_instance(Rc::clone(inst));
                    let arg = ExpandedCallArg { name: None, value: a.clone() };
                    match invoke_class_method(_interp, m, self_val, &[arg]) {
                        Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }

        // Issue #1433: if no user-defined dunder was found (or all returned
        // NotImplemented), coerce int/float subclass instances to their primitive
        // backing and try the numeric helpers.  This handles `divmod(MyInt(10), 3)`
        // where `MyInt` does not define its own `__divmod__` — CPython delegates
        // through the `nb_divmod` slot inherited from `int`; pyrust mirrors that
        // with explicit coercion here.
        let ca = coerce_numeric(a);
        let cb = coerce_numeric(b);
        let ca_is_numeric = matches!(
            ca.kind(),
            ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Float(_) | ValueKind::Bool(_)
        );
        let cb_is_numeric = matches!(
            cb.kind(),
            ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Float(_) | ValueKind::Bool(_)
        );
        if ca_is_numeric && cb_is_numeric {
            let ca_is_float = matches!(ca.kind(), ValueKind::Float(_));
            let cb_is_float = matches!(cb.kind(), ValueKind::Float(_));
            if ca_is_float || cb_is_float {
                // At least one is float — promote both to f64.
                let af = match ca.kind() {
                    ValueKind::Float(f) => f,
                    ValueKind::Int(n) => n as f64,
                    ValueKind::Bool(b) => b as i64 as f64,
                    ValueKind::BigInt(_) => {
                        let pi = PyInt::try_from_value(&ca, "divmod", "a")?;
                        pyint_to_f64(&pi)?
                    }
                    _ => unreachable!(),
                };
                let bf = match cb.kind() {
                    ValueKind::Float(f) => f,
                    ValueKind::Int(n) => n as f64,
                    ValueKind::Bool(b) => b as i64 as f64,
                    ValueKind::BigInt(_) => {
                        let pi = PyInt::try_from_value(&cb, "divmod", "b")?;
                        pyint_to_f64(&pi)?
                    }
                    _ => unreachable!(),
                };
                return divmod_float_float(af, bf);
            } else {
                // Promote Bool → Int so PyInt::try_from_value can match
                // (Bool is not accepted by PyInt::matches).  Use nested blocks
                // so the borrow from kind() is dropped before ca/cb are moved.
                let ca = {
                    if let ValueKind::Bool(b) = ca.kind() {
                        Value::int(b as i64)
                    } else {
                        ca
                    }
                };
                let cb = {
                    if let ValueKind::Bool(b) = cb.kind() {
                        Value::int(b as i64)
                    } else {
                        cb
                    }
                };
                let ia = PyInt::try_from_value(&ca, "divmod", "a")?;
                let ib = PyInt::try_from_value(&cb, "divmod", "b")?;
                return divmod_int_int(ia, ib);
            }
        }

        Err(PyError::named(
            "TypeError",
            format!(
                "unsupported operand type(s) for divmod(): '{}' and '{}'",
                value_type_name_str(a),
                value_type_name_str(b),
            ),
        ))
    }

    /// CPython: pow(base, exp[, mod]) — exponentiation, optionally modular.
    /// <https://docs.python.org/3/library/functions.html#pow>
    ///
    /// This can run user code by dispatching `__pow__` / `__rpow__`
    /// for `PyInstance` values, which may invoke arbitrary user code.
    fn pow(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() missing required argument 'base' (pos 1)"),
            ));
        }
        if args.len() == 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() missing required argument 'exp' (pos 2)"),
            ));
        }
        if args.len() > 3 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 3 arguments ({} given)", args.len()),
            ));
        }
        if args.len() == 3 {
            let base_val = &args[0].value;
            let exp_val = &args[1].value;
            let mod_val = &args[2].value;

            // User-defined type as base: dispatch __pow__(exp, mod) first.
            if let ValueKind::PyInstance(inst) = base_val.kind() {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method) = lookup_class_attr(&class, "__pow__") {
                    let self_val = Value::py_instance(inst_rc);
                    let exp_arg = ExpandedCallArg { name: None, value: exp_val.clone() };
                    let mod_arg = ExpandedCallArg { name: None, value: mod_val.clone() };
                    match invoke_class_method(_interp, method, self_val, &[exp_arg, mod_arg]) {
                        Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }
            }

            // Fall through to built-in integer modpow.
            //
            // CPython distinguishes two TypeError messages for 3-arg pow:
            //   - If any argument is a user-defined type (PyInstance): the
            //     "unsupported operand type(s)" format (three type names).
            //   - Otherwise (e.g. float args): "3rd argument not allowed unless
            //     all arguments are integers".
            let any_instance = matches!(base_val.kind(), ValueKind::PyInstance(_))
                || matches!(exp_val.kind(), ValueKind::PyInstance(_))
                || matches!(mod_val.kind(), ValueKind::PyInstance(_));
            if any_instance {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "unsupported operand type(s) for ** or pow(): '{}', '{}', '{}'",
                        value_type_name_str(base_val),
                        value_type_name_str(exp_val),
                        value_type_name_str(mod_val),
                    ),
                ));
            }
            let three_arg_type_error = || PyError::named(
                "TypeError",
                "pow() 3rd argument not allowed unless all arguments are integers".to_string(),
            );
            // Promote to BigInt when any argument is a BigInt so that values
            // outside the i64 range are handled correctly.  The i64 fast path
            // is kept for the common case where all three args fit in i64.
            let any_bigint = matches!(base_val.kind(), ValueKind::BigInt(_))
                || matches!(exp_val.kind(), ValueKind::BigInt(_))
                || matches!(mod_val.kind(), ValueKind::BigInt(_));
            if any_bigint {
                let base_big: PyBigInt = match base_val.kind() {
                    ValueKind::Int(v) => PyBigInt::from(v),
                    ValueKind::Bool(b) => PyBigInt::from(b as i64),
                    ValueKind::BigInt(b) => (*b).clone(),
                    _ => return Err(three_arg_type_error()),
                };
                let exp_big: PyBigInt = match exp_val.kind() {
                    ValueKind::Int(v) => PyBigInt::from(v),
                    ValueKind::Bool(b) => PyBigInt::from(b as i64),
                    ValueKind::BigInt(b) => (*b).clone(),
                    _ => return Err(three_arg_type_error()),
                };
                let mod_big: PyBigInt = match mod_val.kind() {
                    ValueKind::Int(v) => PyBigInt::from(v),
                    ValueKind::Bool(b) => PyBigInt::from(b as i64),
                    ValueKind::BigInt(b) => (*b).clone(),
                    _ => return Err(three_arg_type_error()),
                };
                if mod_big.is_zero() {
                    return Err(PyError::named(
                        "ValueError",
                        "pow() 3rd argument cannot be 0".to_string(),
                    ));
                }
                if exp_big.sign() == crate::value::PyBigIntSign::Minus {
                    // Negative exponent: compute base^|exp| mod |m|, then find modinv.
                    let abs_exp = -&exp_big;
                    let abs_mod = if mod_big.sign() == crate::value::PyBigIntSign::Minus {
                        -&mod_big
                    } else {
                        mod_big.clone()
                    };
                    let powered = modpow_bigint(&base_big, &abs_exp, &abs_mod);
                    let powered_big: PyBigInt = match powered.kind() {
                        ValueKind::Int(v) => PyBigInt::from(v),
                        ValueKind::BigInt(b) => (*b).clone(),
                        _ => unreachable!("modpow_bigint always returns Int or BigInt"),
                    };
                    match modinv_bigint(&powered_big, &abs_mod) {
                        None => return Err(PyError::named(
                            "ValueError",
                            "base is not invertible for the given modulus".to_string(),
                        )),
                        Some(inv) => {
                            use num_traits::ToPrimitive;
                            // inv is in [0, abs_mod).  Adjust for negative modulus:
                            // Python semantics: result has the same sign as modulus.
                            let result = if mod_big.sign() == crate::value::PyBigIntSign::Minus
                                && inv != PyBigInt::from(0i64)
                            {
                                inv - &abs_mod
                            } else {
                                inv
                            };
                            return Ok(match result.to_i64() {
                                Some(v) => Value::int(v),
                                None => Value::bigint(result),
                            });
                        }
                    }
                }
                return Ok(modpow_bigint(&base_big, &exp_big, &mod_big));
            }
            let base = match base_val.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(three_arg_type_error()),
            };
            let exp = match exp_val.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(three_arg_type_error()),
            };
            let modulus = match mod_val.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(three_arg_type_error()),
            };
            if modulus == 0 {
                return Err(PyError::named(
                    "ValueError",
                    "pow() 3rd argument cannot be 0".to_string(),
                ));
            }
            if exp < 0 {
                // Negative exponent: compute base^|exp| mod |m|, then find modinv.
                let powered = modpow_i64(base, exp.unsigned_abs(), modulus);
                match modinv_i64(powered, modulus) {
                    None => return Err(PyError::named(
                        "ValueError",
                        "base is not invertible for the given modulus".to_string(),
                    )),
                    Some(inv) => {
                        // inv is in [0, |modulus|).  Adjust for negative modulus:
                        // Python semantics: result has the same sign as modulus.
                        let result = if modulus < 0 && inv != 0 {
                            inv - modulus.unsigned_abs() as i64
                        } else {
                            inv
                        };
                        return Ok(Value::int(result));
                    }
                }
            }
            let result = modpow_i64(base, exp as u64, modulus);
            Ok(Value::int(result))
        } else {
            let base_val = &args[0].value;
            let exp_val = &args[1].value;

            // Extract classes for the subtype rule (mirrors CPython `binary_op1`).
            let base_class = if let ValueKind::PyInstance(inst) = base_val.kind() {
                Some(Rc::clone(&inst.borrow().class))
            } else {
                None
            };
            let exp_class = if let ValueKind::PyInstance(inst) = exp_val.kind() {
                Some(Rc::clone(&inst.borrow().class))
            } else {
                None
            };

            // Subtype rule: if exp's class is a proper subtype of base's class
            // AND directly defines __rpow__, try exp.__rpow__(base) first.
            let exp_is_proper_subtype_of_base = match (&base_class, &exp_class) {
                (Some(bc), Some(ec)) => {
                    !Rc::ptr_eq(bc, ec)
                        && class_is_subclass_of(ec, bc)
                        && ec.borrow().attrs.contains_key("__rpow__")
                }
                _ => false,
            };

            if exp_is_proper_subtype_of_base
                && let (Some(ec), ValueKind::PyInstance(inst)) = (&exp_class, exp_val.kind())
                    && let Some(m) = lookup_class_attr(ec, "__rpow__") {
                        let self_val = Value::py_instance(Rc::clone(inst));
                        let arg = ExpandedCallArg { name: None, value: base_val.clone() };
                        match invoke_class_method(_interp, m, self_val, &[arg]) {
                            Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                            Err(e) => return Err(e),
                            _ => {}
                        }
                    }

            // Try base.__pow__(exp).
            if let (Some(bc), ValueKind::PyInstance(inst)) = (&base_class, base_val.kind())
                && let Some(m) = lookup_class_attr(bc, "__pow__") {
                    let self_val = Value::py_instance(Rc::clone(inst));
                    let arg = ExpandedCallArg { name: None, value: exp_val.clone() };
                    match invoke_class_method(_interp, m, self_val, &[arg]) {
                        Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }

            // Try exp.__rpow__(base), but only when:
            //   - the subtype rule didn't already try it, AND
            //   - the types differ (when type(exp) is type(base), CPython skips __rpow__).
            let types_differ = match (&base_class, &exp_class) {
                (Some(bc), Some(ec)) => !Rc::ptr_eq(bc, ec),
                // exp is a PyInstance but base is not: types clearly differ.
                (None, Some(_)) => true,
                // base is PyInstance but exp is not, or both non-instance: skip reflected.
                _ => false,
            };
            if !exp_is_proper_subtype_of_base && types_differ
                && let (Some(ec), ValueKind::PyInstance(inst)) = (&exp_class, exp_val.kind())
                    && let Some(m) = lookup_class_attr(ec, "__rpow__") {
                        let self_val = Value::py_instance(Rc::clone(inst));
                        let arg = ExpandedCallArg { name: None, value: base_val.clone() };
                        match invoke_class_method(_interp, m, self_val, &[arg]) {
                            Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                            Err(e) => return Err(e),
                            _ => {}
                        }
                    }

            // Fall through to built-in numeric pow.  Route through the same
            // NumericOps slot dispatch the `**` operator uses (#458) so that
            // bool operands are treated as int (bool ⊆ int): a non-negative
            // bool/int exponent yields an int, while a negative or float
            // exponent yields a float — matching the operator path exactly.
            if let Some(result) = dispatch_numeric_binop(BinaryOp::Pow, base_val, exp_val) {
                return result;
            }
            if base_class.is_some() || exp_class.is_some() {
                // At least one PyInstance — neither __pow__ nor __rpow__ succeeded.
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "unsupported operand type(s) for ** or pow(): '{}' and '{}'",
                        value_type_name_str(base_val),
                        value_type_name_str(exp_val),
                    ),
                ));
            }
            let a = value_to_float(base_val, FN_NAME)?;
            let b = value_to_float(exp_val, FN_NAME)?;
            Ok(Value::float(a.powf(b)))
        }
    }
}
