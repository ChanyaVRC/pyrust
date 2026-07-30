impl Interpreter {
    /// `eval_binary` for an augmented assignment (`a op= b`) that fell through
    /// the in-place dunder path (`try_inplace_op` returned `None`).
    ///
    /// CPython formats the operand-type `TypeError` with the *augmented* symbol
    /// (`+=`, `-=`, `**=`, …) rather than the plain binary symbol (`+`, `-`,
    /// `** or pow()`). Only that generic operand error is rewritten; specialized
    /// sequence diagnostics remain untouched.
    pub(crate) fn eval_binary_aug(
        &mut self,
        left: Value,
        op: BinaryOp,
        right: Value,
    ) -> Result<Value> {
        self.eval_binary(left, op, right)
            .map_err(|e| rewrite_aug_operand_error(op, e))
    }

    fn add(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            return Ok(Value::complex(a.0 + b.0, a.1 + b.1));
        }
        // Representation-substitutability boundary (#2386): a bytearray-subclass
        // operand acts as its inherited bytearray for concatenation.  Unwrap
        // ONLY for the bytearray-snapshot probe below — `as_bytearray_snapshot`
        // does not see through a `PyInstance`, so without this `BA(b'a') +
        // BA(b'b')` would fall through to a TypeError.  A user `__add__`/
        // `__radd__` was already dispatched at `BinaryOp::Add`, so no override
        // gate is needed.  The original `left`/`right` are kept for the error
        // arms below so CPython's subclass-named messages (`can only
        // concatenate list (not "StrSub") to list`) are preserved.
        let ba_left = effective_builtin_receiver(&left, &[])
            .filter(|b| pyrust_builtins::bytearray::as_bytearray_snapshot(b).is_some());
        let ba_right = effective_builtin_receiver(&right, &[])
            .filter(|b| pyrust_builtins::bytearray::as_bytearray_snapshot(b).is_some());
        if ba_left.is_some() || ba_right.is_some() {
            let l = ba_left.unwrap_or_else(|| left.clone());
            let r = ba_right.unwrap_or_else(|| right.clone());
            return self.add(l, r);
        }
        // bytearray concatenation: handle before coerce_numeric since bytearray
        // is a BuiltinObject and would fall through the numeric match arms.
        // CPython 3.12: bytearray + bytearray → bytearray, bytearray + bytes →
        // bytearray, bytes + bytearray → bytes.
        let lhs_ba = pyrust_builtins::bytearray::as_bytearray_snapshot(&left);
        let rhs_ba = pyrust_builtins::bytearray::as_bytearray_snapshot(&right);
        if lhs_ba.is_some() || rhs_ba.is_some() {
            match (lhs_ba, rhs_ba) {
                (Some(a), Some(b)) => {
                    // bytearray + bytearray → bytearray
                    let mut out = a;
                    out.extend_from_slice(&b);
                    return Ok(pyrust_builtins::bytearray::bytearray(out));
                }
                (Some(a), None) => {
                    // bytearray + bytes → bytearray
                    if let ValueKind::Bytes(rc) = right.kind() {
                        let mut out = a;
                        out.extend_from_slice(rc);
                        return Ok(pyrust_builtins::bytearray::bytearray(out));
                    }
                    return Err(pyrust_core::type_err!(
                        "can't concat {} to bytearray",
                        pyrust_core::builtin_type_name(&right)
                    ));
                }
                (None, Some(b)) => {
                    // bytes + bytearray → bytes (CPython 3.12 parity)
                    if let ValueKind::Bytes(rc) = left.kind() {
                        let mut out = rc.as_ref().clone();
                        out.extend_from_slice(&b);
                        return Ok(Value::bytes(out));
                    }
                    // Non-bytes LHS with bytearray RHS: mirror CPython's
                    // per-type error messages.
                    let lt = value_type_name_str(&left);
                    let err_msg = match left.kind() {
                        ValueKind::Str(_) | ValueKind::List(_) | ValueKind::Tuple(_) => {
                            format!("can only concatenate {lt} (not \"bytearray\") to {lt}")
                        }
                        _ => format!("unsupported operand type(s) for +: '{lt}' and 'bytearray'"),
                    };
                    return Err(pyrust_core::type_err!(err_msg));
                }
                (None, None) => unreachable!(),
            }
        }
        // Issue #1939: list/tuple subclasses inherit `__add__`, so `L([1]) +
        // [2]` (and `[1] + L([2])`) concatenate via the backing list and yield
        // a plain `list`.  Extract container backing (a user `__add__`/
        // `__radd__` was already dispatched at `BinaryOp::Add` before reaching
        // here, so no override check is needed); scalar backing continues
        // through `coerce_numeric`.
        let l = coerce_operand_backing(&left);
        let r = coerce_operand_backing(&right);
        // Canonical numeric arithmetic via the NumericOps slot table
        // (issue #458): handles every numeric type pair in one site.
        // Non-numeric operands return None and fall through to the
        // container / concatenation arms below.
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Add, &l, &r) {
            return result;
        }
        match (l.kind(), r.kind()) {
            (ValueKind::Str(a), ValueKind::Str(b)) => Ok(Value::string(format!("{a}{b}"))),
            (ValueKind::List(a), ValueKind::List(b)) => {
                let mut out = a.to_vec();
                out.extend_from_slice(&b[..]);
                Ok(Value::list(out))
            }
            (ValueKind::Tuple(a), ValueKind::Tuple(b)) => {
                let mut out = a.to_vec();
                out.extend_from_slice(b);
                Ok(Value::tuple(out))
            }
            (ValueKind::Bytes(a), ValueKind::Bytes(b)) => {
                let mut out = a.as_ref().clone();
                out.extend_from_slice(b);
                Ok(Value::bytes(out))
            }
            (ValueKind::Bytes(_), _) => Err(pyrust_core::type_err!(
                "can't concat {} to bytes",
                pyrust_core::builtin_type_name(&right)
            )),
            // CPython sequences (str / list / tuple) report a dedicated
            // "can only concatenate X (not "Y") to X" message when the RHS is
            // not the same sequence type, rather than the generic
            // "unsupported operand type(s)" used for numeric operands.
            (ValueKind::Str(_), _) | (ValueKind::List(_), _) | (ValueKind::Tuple(_), _) => {
                // LHS name comes from the coerced sequence (`str` / `list`
                // / `tuple`, even for subclasses — CPython names the base
                // sequence type whose concat slot ran); RHS name comes from
                // the original operand so subclass names are preserved
                // (e.g. `not "MyInt"`).
                let lt = value_type_name_str(&l);
                let rt = value_type_name_str(&right);
                Err(pyrust_core::type_err!(
                    "can only concatenate {lt} (not \"{rt}\") to {lt}"
                ))
            }
            _ => Err(unsupported_operand("+", &left, &right)),
        }
    }

    fn sub(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            return Ok(Value::complex(a.0 - b.0, a.1 - b.1));
        }
        let (l, r) = (coerce_numeric(&left), coerce_numeric(&right));
        // Canonical numeric arithmetic via the NumericOps slot table (#458).
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Sub, &l, &r) {
            return result;
        }
        Err(unsupported_operand("-", &left, &right))
    }

    /// Resolve a sequence repetition count through `__index__` when the value
    /// is a PyInstance.  Returns the original value for int/bool/bigint.
    /// Raises `TypeError` if `__index__` returns non-int, or if the instance
    /// has no `__index__` at all, matching CPython 3.12 sequence repetition.
    fn try_index_for_seq_repeat(&mut self, val: Value) -> Result<Value> {
        // CPython's repeat-count message names the *original* object's type
        // (both for the non-index TypeError and the BigInt OverflowError), so
        // capture it before `value_to_index` may resolve through `__index__`.
        let type_name_for_err = value_type_name_str(&val).to_string();
        let resolved = self.value_to_index(&val, |_| {
            pyrust_core::type_err!(
                "can't multiply sequence by non-int of type '{type_name_for_err}'"
            )
        })?;
        // `value_to_index` guarantees Int/Bool/BigInt.  A BigInt count is too
        // large to fit a Py_ssize_t; CPython's PyNumber_AsSsize_t raises
        // OverflowError using the *original* object's type name, not "int".
        if matches!(resolved.kind(), ValueKind::BigInt(_)) {
            return Err(pyrust_core::overflow_err!(
                "cannot fit '{type_name_for_err}' into an index-sized integer"
            ));
        }
        Ok(resolved)
    }

    fn mul(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            // (ar+ai*j) * (br+bi*j) = (ar*br - ai*bi) + (ar*bi + ai*br)j
            return Ok(Value::complex(a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0));
        }
        // bytearray * int / int * bytearray — handled before coerce_numeric
        // because bytearray is a BuiltinObject and won't match any explicit arm.
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&left) {
            let n = match right.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(_) => {
                    return Err(pyrust_core::overflow_err!(
                        "cannot fit 'int' into an index-sized integer"
                    ));
                }
                _ => {
                    let type_name = value_type_name_str(&right);
                    return Err(pyrust_core::type_err!(
                        "can't multiply sequence by non-int of type '{type_name}'"
                    ));
                }
            };
            return seq_repeat_bytearray(&data, n);
        }
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&right) {
            let n = match left.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(_) => {
                    return Err(pyrust_core::overflow_err!(
                        "cannot fit 'int' into an index-sized integer"
                    ));
                }
                _ => {
                    let type_name = value_type_name_str(&left);
                    return Err(pyrust_core::type_err!(
                        "can't multiply sequence by non-int of type '{type_name}'"
                    ));
                }
            };
            return seq_repeat_bytearray(&data, n);
        }
        // Issue #1939: list/tuple subclasses inherit `__mul__`, so `T((1,)) *
        // 2` repeats via the backing tuple and yields a plain `tuple`.  Extract
        // container backing (a user `__mul__`/`__rmul__` was already dispatched
        // at `BinaryOp::Mul`); scalar backing continues through `coerce_numeric`.
        let l = coerce_operand_backing(&left);
        let r = coerce_operand_backing(&right);
        // Canonical numeric arithmetic via the NumericOps slot table
        // (#458).  Sequence repetition (Str/List/Tuple/Bytes × Int) and
        // the TypeError diagnostics stay below: at least one operand is a
        // sequence there, so `dispatch_numeric_binop` returns None.
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Mul, &l, &r) {
            return result;
        }
        match (l.kind(), r.kind()) {
            (ValueKind::Str(text), ValueKind::Int(n)) => seq_repeat_str(text, n),
            (ValueKind::Int(n), ValueKind::Str(text)) => seq_repeat_str(text, n),
            (ValueKind::Str(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::Str(_)) => Err(pyrust_core::overflow_err!(
                "cannot fit 'int' into an index-sized integer"
            )),
            (ValueKind::List(items), ValueKind::Int(n)) => seq_repeat_list(&items, n),
            (ValueKind::Int(n), ValueKind::List(items)) => seq_repeat_list(&items, n),
            (ValueKind::List(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::List(_)) => Err(pyrust_core::overflow_err!(
                "cannot fit 'int' into an index-sized integer"
            )),
            (ValueKind::Bytes(data), ValueKind::Int(n)) => seq_repeat_bytes(data, n),
            (ValueKind::Int(n), ValueKind::Bytes(data)) => seq_repeat_bytes(data, n),
            (ValueKind::Bytes(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::Bytes(_)) => Err(pyrust_core::overflow_err!(
                "cannot fit 'int' into an index-sized integer"
            )),
            // Tuple * Int / Int * Tuple — checked repeat, MemoryError on
            // overflow (matches CPython 3.12 `tuplerepeat` behaviour).
            (ValueKind::Tuple(items), ValueKind::Int(n)) => seq_repeat_tuple(items, n),
            (ValueKind::Int(n), ValueKind::Tuple(items)) => seq_repeat_tuple(items, n),
            // Tuple * BigInt / BigInt * Tuple — any BigInt is too large to
            // fit in a platform index; CPython raises OverflowError for both
            // positive and negative BigInt values.
            (ValueKind::Tuple(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::Tuple(_)) => Err(pyrust_core::overflow_err!(
                "cannot fit 'int' into an index-sized integer"
            )),
            _ => {
                let is_sequence = |v: &Value| {
                    matches!(
                        v.kind(),
                        ValueKind::Str(_)
                            | ValueKind::List(_)
                            | ValueKind::Tuple(_)
                            | ValueKind::Bytes(_)
                    )
                };
                let is_int_like =
                    |v: &Value| matches!(v.kind(), ValueKind::Int(_) | ValueKind::BigInt(_));
                if is_sequence(&l) && !is_int_like(&r) {
                    let type_name = value_type_name_str(&r);
                    return Err(pyrust_core::type_err!(
                        "can't multiply sequence by non-int of type '{type_name}'"
                    ));
                }
                if is_sequence(&r) && !is_int_like(&l) {
                    let type_name = value_type_name_str(&l);
                    return Err(pyrust_core::type_err!(
                        "can't multiply sequence by non-int of type '{type_name}'"
                    ));
                }
                Err(unsupported_operand("*", &left, &right))
            }
        }
    }

    /// Dispatch a single binary method (e.g. `__iadd__`) on a
    /// PyInstance receiver.  Returns `Some(result)` when the method
    /// exists and was called (possibly returning `NotImplemented`),
    /// `None` when the method isn't defined on the class, and `Err` when
    /// the slot exists but is neither callable nor a descriptor.  Like
    /// `try_dunder_binary`, this routes both user-defined and
    /// `pyrust_module!`-generated class methods through
    /// `invoke_class_method` so Counter's `__iadd__` (a BuiltinFunction
    /// in the class's attr map) participates in `+=` dispatch — and so a
    /// descriptor in the slot is bound through `__get__` (issue #2944).
    fn try_call_binary_method(
        &mut self,
        receiver: &Value,
        method: &str,
        other: Value,
    ) -> Result<Option<Value>> {
        let inst = match receiver.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => return Ok(None),
        };
        let class = Rc::clone(&inst.borrow().class);
        let Some(method_value) = lookup_class_attr(&class, method) else {
            return Ok(None);
        };
        // Issue #2944: the in-place slot obeys the same rules as every other
        // special-method slot, so gate it exactly like `dispatch_binary_slot`.
        // The previous `UserFunction | BuiltinFunction` test answered "not
        // defined" for anything else, which silently skipped the slot and fell
        // through to `__add__` / the backing fallback: a descriptor `__iadd__`
        // (`property`, user `__get__`) was never bound and its getter never
        // ran, a callable-instance `__iadd__` was ignored (issue #2054), and
        // `__iadd__ = 5` reported `unsupported operand type(s) for +=` where
        // CPython raises `TypeError: 'int' object is not callable` (issue
        // #2055).  All thirteen in-place operators shared the one gate.
        if !slot_is_dispatchable(&method_value) {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object is not callable",
                    value_type_name_str(&method_value)
                ),
            ));
        }
        // Issue #2122: inherited primitive in-place sentinels are not
        // overrides; they must reach the identity-preserving backing fallback.
        // The defining MRO owner is essential here.  The same builtin
        // descriptor explicitly assigned on another class is a genuine
        // override and must be invoked (normally producing the descriptor's
        // receiver TypeError), even though its qualified name is identical.
        let inherited_primitive =
            inherited_primitive_builtin_slot_kind(&class, method, &method_value);
        let inherited_backing_fallback = match inherited_primitive {
            Some(pyrust_core::CanonicalClassTag::Set) => {
                matches!(method, "__ior__" | "__iand__" | "__isub__" | "__ixor__")
            }
            Some(pyrust_core::CanonicalClassTag::Dict) => method == "__ior__",
            Some(pyrust_core::CanonicalClassTag::List)
            | Some(pyrust_core::CanonicalClassTag::Bytearray) => {
                matches!(method, "__iadd__" | "__imul__")
            }
            _ => false,
        };
        if inherited_backing_fallback {
            return Ok(None);
        }
        let self_val = Value::py_instance(Rc::clone(&inst));
        let arg = ExpandedCallArg {
            name: None,
            value: other,
        };
        let result = invoke_class_method(self, method_value, self_val, &[arg])?;
        Ok(Some(result))
    }
}
