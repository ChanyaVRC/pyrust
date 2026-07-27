impl Interpreter {
    fn call_range_bound_method(
        &mut self,
        receiver: &Value,
        method: &str,
        pos: &mut Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        let ValueKind::Range { start, stop, step } = receiver.kind() else {
            unreachable!("receiver family checked by bound method dispatcher");
        };
        let args_vec: Vec<Value> = std::mem::take(pos);
        match method {
            "__len__" => {
                let extra = args_vec.len() + kw.len();
                if extra != 0 {
                    Err(pyrust_core::type_err!("expected 0 arguments, got {extra}"))
                } else {
                    use crate::value::range_len;
                    match i64::try_from(range_len(start, stop, step)) {
                        Ok(n) => Ok(Value::int(n)),
                        Err(_) => Err(pyrust_core::py_err!(
                            "OverflowError",
                            "Python int too large to convert to C ssize_t".to_string()
                        )),
                    }
                }
            }
            "count" => {
                if args_vec.len() != 1 || !kw.is_empty() {
                    Err(pyrust_core::type_err!(
                        "range.count() takes exactly one argument ({} given)",
                        args_vec.len() + kw.len()
                    ))
                } else {
                    let contained = self.range_contains_value(start, stop, step, &args_vec[0])?;
                    Ok(Value::int(if contained { 1 } else { 0 }))
                }
            }
            "index" => {
                if args_vec.len() != 1 || !kw.is_empty() {
                    Err(pyrust_core::type_err!(
                        "range.index() takes exactly one argument ({} given)",
                        args_vec.len() + kw.len()
                    ))
                } else {
                    use crate::value::PyToPrimitive;
                    let v = &args_vec[0];
                    // Convert v to i64 if possible; non-integer types
                    // can never be in a range (CPython returns the
                    // "x not in sequence" message for those).
                    let vi_opt: Option<i64> = match v.kind() {
                        ValueKind::Int(x) => Some(x),
                        ValueKind::Bool(b) => Some(b as i64),
                        ValueKind::BigInt(n) => n.to_i64(),
                        ValueKind::Float(f) => {
                            const I64_MIN_F: f64 = i64::MIN as f64;
                            const I64_MAX_PLUS1_F: f64 = 9_223_372_036_854_775_808.0_f64;
                            if f.is_finite()
                                && f.fract() == 0.0
                                && (I64_MIN_F..I64_MAX_PLUS1_F).contains(&f)
                            {
                                Some(f as i64)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    match vi_opt {
                        None => Err(pyrust_core::value_err!(
                            "sequence.index(x): x not in sequence"
                        )),
                        Some(vi) => {
                            let contained = self.range_contains_value(start, stop, step, v)?;
                            if contained {
                                let index = (i128::from(vi) - i128::from(start)) / i128::from(step);
                                Ok(match i64::try_from(index) {
                                    Ok(index) => Value::int(index),
                                    Err(_) => value_from_bigint(PyBigInt::from(index)),
                                })
                            } else {
                                Err(pyrust_core::value_err!("{} is not in range", v.repr_raw()))
                            }
                        }
                    }
                }
            }
            _ => Err(pyrust_core::py_err!(
                "AttributeError",
                "'range' object has no attribute '{method}'"
            )),
        }
    }
}

impl Interpreter {
    fn call_big_range_bound_method(
        &mut self,
        receiver: &Value,
        method: &str,
        pos: &mut Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        let ValueKind::BigRange { start, stop, step } = receiver.kind() else {
            unreachable!("receiver family checked by bound method dispatcher");
        };
        let (start, stop, step) = (start.clone(), stop.clone(), step.clone());
        let args_vec: Vec<Value> = std::mem::take(pos);
        match method {
            "__len__" => {
                let extra = args_vec.len() + kw.len();
                if extra != 0 {
                    Err(pyrust_core::type_err!("expected 0 arguments, got {extra}"))
                } else {
                    match pyrust_core::bigrange_len(&start, &stop, &step).to_i64() {
                        Some(n) => Ok(Value::int(n)),
                        None => Err(pyrust_core::py_err!(
                            "OverflowError",
                            "Python int too large to convert to C ssize_t".to_string()
                        )),
                    }
                }
            }
            "count" => {
                if args_vec.len() != 1 || !kw.is_empty() {
                    Err(pyrust_core::type_err!(
                        "range.count() takes exactly one argument ({} given)",
                        args_vec.len() + kw.len()
                    ))
                } else {
                    let contained =
                        Self::bigrange_member(&start, &stop, &step, &args_vec[0]).is_some();
                    Ok(Value::int(if contained { 1 } else { 0 }))
                }
            }
            "index" => {
                if args_vec.len() != 1 || !kw.is_empty() {
                    Err(pyrust_core::type_err!(
                        "range.index() takes exactly one argument ({} given)",
                        args_vec.len() + kw.len()
                    ))
                } else {
                    let v = &args_vec[0];
                    // CPython distinguishes a non-integer subscript (which
                    // never has an index → "x not in sequence") from an
                    // integer that simply isn't a member of this range
                    // ("{} is not in range").
                    let is_int_valued = matches!(
                        v.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    ) || matches!(v.kind(), ValueKind::Float(f) if f.is_finite() && f.fract() == 0.0);
                    match Self::bigrange_member(&start, &stop, &step, v) {
                        Some(x) => Ok(value_from_bigint((x - &start) / &step)),
                        None if is_int_valued => {
                            Err(pyrust_core::value_err!("{} is not in range", v.repr_raw()))
                        }
                        None => Err(pyrust_core::value_err!(
                            "sequence.index(x): x not in sequence"
                        )),
                    }
                }
            }
            _ => Err(pyrust_core::py_err!(
                "AttributeError",
                "'range' object has no attribute '{method}'"
            )),
        }
    }
}
