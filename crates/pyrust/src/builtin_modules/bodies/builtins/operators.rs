use pyrust_derive::pyrust_module;

fn richcmp_subclass_backing(value: Value) -> Value {
    coerce_subclass_backing(&value, &[]).unwrap_or(value)
}

pyrust_module! {
    /// Issue #1256: `int.__add__(self, value)` — exposes `int.__add__` as a
    /// class-level attribute so that `int.__add__(1, 2)` and
    /// `hasattr(int, '__add__')` work.  Returns `NotImplemented` when the
    /// right-hand operand is not an integer type, matching CPython's C slot
    /// which only handles int/bool/BigInt and delegates float/str/other to the
    /// reflected operator on the right-hand side.
    ///
    /// CPython signature: `int.__add__(self, value, /)`
    #[py_name = "int.__add__"]
    fn int_add_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__add__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Add, b)
    }

    /// Issue #1256: `int.__sub__(self, value)`
    #[py_name = "int.__sub__"]
    fn int_sub_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__sub__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Sub, b)
    }

    /// Issue #1256: `int.__mul__(self, value)`
    #[py_name = "int.__mul__"]
    fn int_mul_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__mul__", "int")),
        };
        // CPython's int.__mul__ only accepts integer types; string repetition
        // (1 * "x" = "x") is dispatched via str.__rmul__, not here.
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Mul, b)
    }

    /// Issue #1256: `int.__truediv__(self, value)`
    #[py_name = "int.__truediv__"]
    fn int_truediv_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__truediv__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Div, b)
    }

    /// Issue #1256: `int.__floordiv__(self, value)`
    #[py_name = "int.__floordiv__"]
    fn int_floordiv_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__floordiv__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::FloorDiv, b)
    }

    /// Issue #1256: `int.__mod__(self, value)`
    #[py_name = "int.__mod__"]
    fn int_mod_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__mod__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Mod, b)
    }

    /// Issue #1256: `int.__pow__(self, value)`
    #[py_name = "int.__pow__"]
    fn int_pow_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__pow__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Pow, b)
    }

    /// Issue #1256: `int.__and__(self, value)`
    #[py_name = "int.__and__"]
    fn int_and_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__and__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::BitAnd, b)
    }

    /// Issue #1256: `int.__or__(self, value)`
    #[py_name = "int.__or__"]
    fn int_or_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__or__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::BitOr, b)
    }

    /// Issue #1256: `int.__xor__(self, value)`
    #[py_name = "int.__xor__"]
    fn int_xor_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__xor__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::BitXor, b)
    }

    /// Issue #1256: `int.__lshift__(self, value)`
    #[py_name = "int.__lshift__"]
    fn int_lshift_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__lshift__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::LShift, b)
    }

    /// Issue #1256: `int.__rshift__(self, value)`
    #[py_name = "int.__rshift__"]
    fn int_rshift_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__rshift__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::RShift, b)
    }

    /// Issue #1256: `int.__lt__(self, value)`
    /// Issue #2847: explicit int rich-comparison slots accept int-subclass
    /// backing on either side without redispatching the subclass override.
    #[py_name = "int.__lt__"]
    fn int_lt_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__lt__", "int")),
        };
        let b = richcmp_subclass_backing(b);
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Lt, b)
    }

    /// Issue #1256: `int.__le__(self, value)`
    #[py_name = "int.__le__"]
    fn int_le_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__le__", "int")),
        };
        let b = richcmp_subclass_backing(b);
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Le, b)
    }

    /// Issue #1256: `int.__gt__(self, value)`
    #[py_name = "int.__gt__"]
    fn int_gt_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__gt__", "int")),
        };
        let b = richcmp_subclass_backing(b);
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Gt, b)
    }

    /// Issue #1256: `int.__ge__(self, value)`
    #[py_name = "int.__ge__"]
    fn int_ge_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ge__", "int")),
        };
        let b = richcmp_subclass_backing(b);
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Ge, b)
    }

    /// Issue #1256: `int.__eq__(self, value)`
    #[py_name = "int.__eq__"]
    fn int_eq_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__eq__", "int")),
        };
        let b = richcmp_subclass_backing(b);
        // CPython's int.__eq__ returns NotImplemented for non-integer types;
        // pyrust's eval_binary(Eq) falls through to values_user_eq which
        // returns False for cross-type comparisons without raising TypeError.
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Eq, b)
    }

    /// Issue #1256: `int.__ne__(self, value)`
    #[py_name = "int.__ne__"]
    fn int_ne_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ne__", "int")),
        };
        let b = richcmp_subclass_backing(b);
        // CPython's int.__ne__ returns NotImplemented for non-integer types.
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Ne, b)
    }

    /// Issue #1452: `float.__trunc__(self)` — registered so that float subclasses
    /// inherit it via MRO and `math.trunc(MyFloat(x))` dispatches correctly.
    ///
    /// CPython: `float.__trunc__` truncates toward zero and returns an `int`.
    /// Raises `OverflowError` for infinity, `ValueError` for NaN.
    #[py_name = "float.__trunc__"]
    fn float_trunc_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__trunc__", "float", method)
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Float(f) => {
                if f.is_nan() {
                    return Err(PyError::named(
                        "ValueError",
                        "cannot convert float NaN to integer".to_string(),
                    ));
                }
                if f.is_infinite() {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot convert float infinity to integer".to_string(),
                    ));
                }
                let t = f.trunc();
                // i64::MAX as f64 rounds up to 2^63, so ">=" is required: any float
                // >= 2^63 cannot fit in an i64 and must go through BigInt.
                if t >= i64::MAX as f64 || t < i64::MIN as f64 {
                    float_to_bigint(t)
                } else {
                    Ok(Value::int(t as i64))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor '__trunc__' for 'float' objects doesn't apply to a '{}' object",
                    value_type_name_str(&self_val)
                ),
            )),
        }
    }

    /// Issue #1452: `float.__floor__(self)` — registered so that float subclasses
    /// inherit it via MRO and `math.floor(MyFloat(x))` dispatches correctly.
    ///
    /// CPython: `float.__floor__` rounds toward negative infinity and returns an `int`.
    /// Raises `OverflowError` for infinity, `ValueError` for NaN.
    #[py_name = "float.__floor__"]
    fn float_floor_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__floor__", "float", method)
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Float(f) => {
                if f.is_nan() {
                    return Err(PyError::named(
                        "ValueError",
                        "cannot convert float NaN to integer".to_string(),
                    ));
                }
                if f.is_infinite() {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot convert float infinity to integer".to_string(),
                    ));
                }
                let floor = f.floor();
                // i64::MAX as f64 rounds up to 2^63, so ">=" is required: any float
                // >= 2^63 cannot fit in an i64 and must go through BigInt.
                if floor >= i64::MAX as f64 || floor < i64::MIN as f64 {
                    float_to_bigint(floor)
                } else {
                    Ok(Value::int(floor as i64))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor '__floor__' for 'float' objects doesn't apply to a '{}' object",
                    value_type_name_str(&self_val)
                ),
            )),
        }
    }

    /// Issue #1452: `float.__ceil__(self)` — registered so that float subclasses
    /// inherit it via MRO and `math.ceil(MyFloat(x))` dispatches correctly.
    ///
    /// CPython: `float.__ceil__` rounds toward positive infinity and returns an `int`.
    /// Raises `OverflowError` for infinity, `ValueError` for NaN.
    #[py_name = "float.__ceil__"]
    fn float_ceil_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__ceil__", "float", method)
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Float(f) => {
                if f.is_nan() {
                    return Err(PyError::named(
                        "ValueError",
                        "cannot convert float NaN to integer".to_string(),
                    ));
                }
                if f.is_infinite() {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot convert float infinity to integer".to_string(),
                    ));
                }
                let ceil = f.ceil();
                // i64::MAX as f64 rounds up to 2^63, so ">=" is required: any float
                // >= 2^63 cannot fit in an i64 and must go through BigInt.
                if ceil >= i64::MAX as f64 || ceil < i64::MIN as f64 {
                    float_to_bigint(ceil)
                } else {
                    Ok(Value::int(ceil as i64))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor '__ceil__' for 'float' objects doesn't apply to a '{}' object",
                    value_type_name_str(&self_val)
                ),
            )),
        }
    }

    /// Issue #1256: `str.__len__(self)` — exposes `str.__len__` as a class-level
    /// attribute so that `str.__len__("hello")` and `hasattr(str, '__len__')`
    /// work.
    ///
    /// CPython signature: `str.__len__(self, /)`
    #[py_name = "str.__len__"]
    fn str_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "str")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Str(_) => Ok(Value::int(self_val.str_codepoint_len() as i64)),
            _ => Err(pyrust_core::descriptor_requires!("__len__", "str", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1256: `str.__add__(self, value)`
    #[py_name = "str.__add__"]
    fn str_add_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__add__", "str")),
        };
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Add, b)
    }

    /// Issue #1256: `str.__mul__(self, value)`
    #[py_name = "str.__mul__"]
    fn str_mul_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__mul__", "str")),
        };
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Mul, b)
    }

    /// Issue #1256: `str.__lt__(self, value)`
    /// Issue #2847: explicit str rich-comparison slots accept str-subclass
    /// backing on either side without redispatching the subclass override.
    #[py_name = "str.__lt__"]
    fn str_lt_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__lt__", "str")),
        };
        let b = richcmp_subclass_backing(b);
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Lt, b)
    }

    /// Issue #1256: `str.__le__(self, value)`
    #[py_name = "str.__le__"]
    fn str_le_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__le__", "str")),
        };
        let b = richcmp_subclass_backing(b);
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Le, b)
    }

    /// Issue #1256: `str.__gt__(self, value)`
    #[py_name = "str.__gt__"]
    fn str_gt_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__gt__", "str")),
        };
        let b = richcmp_subclass_backing(b);
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Gt, b)
    }

    /// Issue #1256: `str.__ge__(self, value)`
    #[py_name = "str.__ge__"]
    fn str_ge_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ge__", "str")),
        };
        let b = richcmp_subclass_backing(b);
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Ge, b)
    }

    /// Issue #1256: `str.__eq__(self, value)`
    #[py_name = "str.__eq__"]
    fn str_eq_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__eq__", "str")),
        };
        let b = richcmp_subclass_backing(b);
        // CPython's str.__eq__ returns NotImplemented for non-str types;
        // eval_binary(Eq) falls through to values_user_eq which returns False.
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Eq, b)
    }

    /// Issue #1256: `str.__ne__(self, value)`
    #[py_name = "str.__ne__"]
    fn str_ne_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ne__", "str")),
        };
        let b = richcmp_subclass_backing(b);
        // CPython's str.__ne__ returns NotImplemented for non-str types.
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Ne, b)
    }

    /// Issue #1256: `list.__len__(self)` — exposes `list.__len__` as a
    /// class-level attribute.
    ///
    /// CPython signature: `list.__len__(self, /)`
    #[py_name = "list.__len__"]
    fn list_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "list")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::List(items) => Ok(Value::int(items.len() as i64)),
            // Issue #1434: list subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::List(items)) => Ok(Value::int(items.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "list", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "list", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1256: `tuple.__len__(self)`
    #[py_name = "tuple.__len__"]
    fn tuple_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "tuple")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Tuple(items) => Ok(Value::int(items.len() as i64)),
            // Issue #1434: tuple subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::Tuple(items)) => Ok(Value::int(items.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "tuple", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "tuple", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1256: `dict.__len__(self)`
    #[py_name = "dict.__len__"]
    fn dict_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "dict")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Dict(items) => Ok(Value::int(items.len() as i64)),
            // Issue #1434: dict subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::Dict(items)) => Ok(Value::int(items.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "dict", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "dict", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1390: `dict.fromkeys(iterable[, value])` — classmethod that creates a
    /// new dict with keys from `iterable` and all values set to `value` (default
    /// `None`).
    ///
    /// CPython rules (3.12):
    ///   - At most two positional arguments; no keyword arguments.
    ///   - Duplicate keys: the first occurrence wins for insertion order.
    ///   - Unhashable keys raise `TypeError`.
    ///
    #[py_name = "dict.fromkeys"]
    fn dict_fromkeys(args) -> Result<Value> {
        // The explicit native-classmethod descriptor prepends its bound class.
        // Consume only that leading typed receiver: a PyClass supplied by the
        // user as the iterable must remain an ordinary argument.
        let (bound_class, user_args) = match args.split_first() {
            Some((first, rest)) => match first.value.kind() {
                ValueKind::PyClass(class) if first.name.is_none() => {
                    (Some(Rc::clone(class)), rest)
                }
                _ => (None, args),
            },
            None => (None, args),
        };
        let has_kw = args.iter().any(|a| a.name.is_some());
        if has_kw {
            let owner = bound_class
                .as_ref()
                .map_or_else(|| "dict".to_string(), |class| class.borrow().name.clone());
            return Err(PyError::named(
                "TypeError",
                format!("{owner}.fromkeys() takes no keyword arguments"),
            ));
        }
        if user_args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "fromkeys expected at most 2 arguments, got {}",
                    user_args.len()
                ),
            ));
        }
        let iterable = match user_args.first() {
            Some(a) => a.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "fromkeys expected at least 1 argument, got 0".to_string(),
                ));
            }
        };
        let default_val = user_args.get(1).map_or_else(Value::none, |a| a.value.clone());

        let keys = _interp.collect_iterable(&iterable)?;
        let mut map: PyDict =
            PyDict::with_capacity_and_hasher(keys.len(), Default::default());
        for key in keys {
            let py_key = _interp.value_to_pykey(&key)?;
            // #1914: `dict_insert` dedups `PyKey::Object` keys via user `__eq__`
            // (raw `IndexMap` identity for primitive keys keeps the fast path).
            // The value is always the same default, so last-wins == first-wins;
            // `IndexMap::insert` preserves the first-occurrence position.
            _interp.dict_insert(&mut map, py_key, default_val.clone())?;
        }
        let backing = Value::dict(map);
        let Some(class) = bound_class else {
            return Ok(backing);
        };
        if class.borrow().canonical_tag == Some(pyrust_core::CanonicalClassTag::Dict) {
            return Ok(backing);
        }

        // `dict.fromkeys` is a classmethod: a subclass call constructs `cls`
        // and installs the populated primitive backing on that instance.
        // Class construction remains the interpreter's responsibility so
        // metaclass `__call__` and user construction hooks are preserved.
        let instance = _interp.call_class_expanded(class, &[])?;
        if let ValueKind::PyInstance(instance) = instance.kind() {
            instance.borrow_mut().attrs.insert(
                crate::interpreter::BUILTIN_DATA_ATTR,
                backing,
            );
        }
        Ok(instance)
    }

    /// Issue #1256: `set.__len__(self)`
    #[py_name = "set.__len__"]
    fn set_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "set")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Set(items) => Ok(Value::int(items.len() as i64)),
            // Issue #1434: set subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::Set(items)) => Ok(Value::int(items.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "set", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "set", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1256: `bytes.__len__(self)`
    #[py_name = "bytes.__len__"]
    fn bytes_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "bytes")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Bytes(b) => Ok(Value::int(b.len() as i64)),
            // Issue #1434: bytes subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::Bytes(b)) => Ok(Value::int(b.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "bytes", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "bytes", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1548: `frozenset.__len__(self)`
    #[py_name = "frozenset.__len__"]
    fn frozenset_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "frozenset")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::BuiltinObject { ops, state }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
            {
                Ok(Value::int(ops.len(state).unwrap_or(0) as i64))
            }
            // Issue #1548: frozenset subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::BuiltinObject { ops, state })
                        if ops.canonical_class_tag()
                            == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
                    {
                        Ok(Value::int(ops.len(state).unwrap_or(0) as i64))
                    }
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "frozenset", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "frozenset", value_type_name_str(&self_val))),
        }
    }

}
