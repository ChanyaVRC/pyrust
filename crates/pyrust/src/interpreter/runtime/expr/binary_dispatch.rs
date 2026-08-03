impl Interpreter {
    pub(crate) fn eval_binary(&mut self, left: Value, op: BinaryOp, right: Value) -> Result<Value> {
        match op {
            BinaryOp::Add => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__add__", "__radd__") {
                    return r;
                }
                self.add(left, right)
            }
            BinaryOp::Sub => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__sub__", "__rsub__") {
                    return r;
                }
                if let Some(r) = set_binary_op(self, &left, &right, SetOp::Sub, "-") {
                    return r;
                }
                self.sub(left, right)
            }
            BinaryOp::Mul => {
                // Fast path: tagged `str * int` / `int * str` (the common
                // `"x" * n`).  Tagged `Str`/`Int` are never `PyInstance`, so no
                // user `__mul__`/`__rmul__` can apply — skip the dunder dispatch,
                // subclass-backing coercion, and numeric-slot probing entirely.
                match (left.kind(), right.kind()) {
                    (ValueKind::Str(t), ValueKind::Int(n))
                    | (ValueKind::Int(n), ValueKind::Str(t)) => return seq_repeat_str(t, n),
                    _ => {}
                }
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mul__", "__rmul__") {
                    return r;
                }
                // Before raising TypeError, try __index__ on the count operand
                // when one side is a built-in sequence.  CPython calls
                // PyNumber_AsSsize_t (which invokes __index__) before failing.
                let is_seq = |v: &Value| {
                    matches!(
                        v.kind(),
                        ValueKind::Str(_)
                            | ValueKind::List(_)
                            | ValueKind::Tuple(_)
                            | ValueKind::Bytes(_)
                    )
                };
                let is_py_instance = |v: &Value| matches!(v.kind(), ValueKind::PyInstance(_));
                if is_seq(&left) && is_py_instance(&right) {
                    let right = self.try_index_for_seq_repeat(right)?;
                    return self.mul(left, right);
                }
                if is_seq(&right) && is_py_instance(&left) {
                    let left = self.try_index_for_seq_repeat(left)?;
                    return self.mul(left, right);
                }
                self.mul(left, right)
            }
            BinaryOp::MatMul => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__matmul__", "__rmatmul__")
                {
                    return r;
                }
                self.matmul(left, right)
            }
            BinaryOp::Div => {
                if let Some(r) =
                    self.try_dunder_binary(&left, &right, "__truediv__", "__rtruediv__")
                {
                    return r;
                }
                // Issue #1204: `div` extracts the scalar backing internally and
                // keeps the original operands for the subclass-named TypeError.
                self.div(left, right)
            }
            BinaryOp::FloorDiv => {
                if let Some(r) =
                    self.try_dunder_binary(&left, &right, "__floordiv__", "__rfloordiv__")
                {
                    return r;
                }
                // Issue #1204: `floor_div` extracts the scalar backing
                // internally and keeps the originals for the TypeError arm.
                self.floor_div(left, right)
            }
            BinaryOp::Mod => {
                // str % args: printf-style formatting (#1393).
                // Must come BEFORE try_dunder_binary so that rhs.__rmod__ is
                // never consulted when lhs is str — CPython's str.__mod__ is
                // never NotImplemented, so the reflected slot must not run
                // (#1472).
                // Also covers str subclasses (PyInstance with Str backing):
                // CPython's tp_as_sequence->sq_remainder for str subclasses
                // still runs str.__mod__, never returning NotImplemented.
                let str_backing = if matches!(left.kind(), ValueKind::Str(_)) {
                    Some(left.clone())
                } else {
                    builtin_data_backing(&left)
                        .filter(|backing| matches!(backing.kind(), ValueKind::Str(_)))
                };
                if let Some(fmt_val) = str_backing {
                    return self.str_printf_format(fmt_val, right);
                }
                // bytes % args / bytearray % args: PEP 461 printf-style
                // formatting (#1883).  Like str, bytes.__mod__ is never
                // NotImplemented, so this must precede try_dunder_binary so
                // rhs.__rmod__ is not consulted.  bytearray % args returns a
                // bytearray (result type follows the left operand); bytes and
                // bytes subclasses return plain bytes.
                let bytes_backing: Option<Vec<u8>> = match left.kind() {
                    ValueKind::Bytes(rc) => Some(rc.to_vec()),
                    _ => builtin_data_backing(&left).and_then(|backing| match backing.kind() {
                        ValueKind::Bytes(rc) => Some(rc.to_vec()),
                        _ => None,
                    }),
                };
                if let Some(data) = bytes_backing {
                    return self.bytes_printf_format(&data, right, false);
                }
                if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&left) {
                    return self.bytes_printf_format(&data, right, true);
                }
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mod__", "__rmod__") {
                    return r;
                }
                // Issue #1204: `modulo` extracts the scalar backing internally
                // and keeps the originals for the TypeError arm.
                self.modulo(left, right)
            }
            BinaryOp::Eq => {
                // Issue #2936: CPython's `mappingproxy_richcompare` compares
                // the *proxied object*, so `mappingproxy(od) == od` is
                // `od == od`.  Only proxies built over a separate object (a
                // dict subclass, or another proxy) carry an owner; a proxy over
                // a plain dict or a class `__dict__` compares through its own
                // ops table as before.  `proxied_of` follows a nested chain to
                // its end — the forwarding is recursive in CPython.
                let left = pyrust_builtins::mapping_proxy::proxied_of(&left).unwrap_or(left);
                let right = pyrust_builtins::mapping_proxy::proxied_of(&right).unwrap_or(right);
                if let Some(r) = self.try_dunder_binary(&left, &right, "__eq__", "__eq__") {
                    return r;
                }
                // Containers fall through here: `Value::eq` would call
                // `Rc::ptr_eq` on `PyInstance` elements, missing user
                // `__eq__`.  Route through `values_user_eq` so list /
                // tuple / dict / set element comparison dispatches
                // `__eq__` recursively (issue #436).
                Ok(Value::bool_(self.values_user_eq(&left, &right)?))
            }
            BinaryOp::Ne => {
                // Issue #2936: as for `Eq`, an owner-carrying `mappingproxy`
                // compares as the object it proxies.
                let left = pyrust_builtins::mapping_proxy::proxied_of(&left).unwrap_or(left);
                let right = pyrust_builtins::mapping_proxy::proxied_of(&right).unwrap_or(right);
                // CPython's `slot_tp_richcompare` derives `__ne__` as the logical
                // negation of `__eq__` whenever a class does not define its own
                // `__ne__` (issue #2645).  pyrust resolves the inherited
                // `object.__ne__` instead, which only compares identity and so
                // returns the wrong answer for `a != a` when `a.__eq__` returns
                // `False` (and similar cases).  Only dispatch `__ne__` directly
                // when at least one operand carries a *user-defined* `__ne__`;
                // otherwise fall through to `not (a == b)`, which already runs
                // the full `__eq__` / reflected-`__eq__` / NotImplemented chain.
                if pyinstance_has_user_ne(&left) || pyinstance_has_user_ne(&right) {
                    // At least one operand carries a user-defined `__ne__`, so
                    // mirror CPython's `do_richcompare(Py_NE)` directly rather
                    // than `not __eq__`.  The ordering is: (subtype-priority
                    // reflected,) forward operand's `__ne__`, reflected operand's
                    // `__ne__`, then the identity default `a is not b`.  Each
                    // `__ne__` step (`pyinstance_ne_step`) dispatches a user
                    // `__ne__` outright and treats the inherited default as
                    // `not __eq__` single-sided.  When *both* steps yield
                    // NotImplemented, CPython falls back to identity — NOT to
                    // `not __eq__` (issue #2648): a user `__ne__` returning
                    // NotImplemented must not re-dispatch `__eq__`.
                    let right_first = matches!(
                        (left.kind(), right.kind()),
                        (ValueKind::PyInstance(li), ValueKind::PyInstance(ri))
                            if !Rc::ptr_eq(&li.borrow().class, &ri.borrow().class)
                                && class_is_subclass_of(
                                    &ri.borrow().class,
                                    &li.borrow().class,
                                )
                    );
                    let (first, second) = if right_first {
                        ((&right, &left), (&left, &right))
                    } else {
                        ((&left, &right), (&right, &left))
                    };
                    if let Some(r) = self.pyinstance_ne_step(first.0, first.1) {
                        return r;
                    }
                    if let Some(r) = self.pyinstance_ne_step(second.0, second.1) {
                        return r;
                    }
                    return Ok(Value::bool_(!values_are_identical(&left, &right)));
                }
                // No user `__ne__`: mirror CPython's `slot_tp_richcompare`, which
                // derives `__ne__` as `not __eq__`.  Try the user `__eq__` /
                // reflected-`__eq__` chain first and negate its truthiness, so
                // `b != b` honours a custom `__eq__` returning `False` (the
                // `values_user_eq` identity short-circuit below would wrongly
                // report equal for `a is b`).  `values_user_eq` remains the
                // fallback for container element-wise dispatch (issue #436).
                if let Some(r) = self.try_dunder_binary(&left, &right, "__eq__", "__eq__") {
                    let result = r?;
                    return Ok(Value::bool_(!self.truthy_value(&result)?));
                }
                Ok(Value::bool_(!self.values_user_eq(&left, &right)?))
            }
            BinaryOp::Lt => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__lt__", "__gt__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(self, &left, &right, BinaryOp::Lt) {
                    return r;
                }
                self.compare(left, right, BinaryOp::Lt, "<", |o| o.is_lt())
            }
            BinaryOp::Le => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__le__", "__ge__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(self, &left, &right, BinaryOp::Le) {
                    return r;
                }
                self.compare(left, right, BinaryOp::Le, "<=", |o| o.is_le())
            }
            BinaryOp::Gt => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__gt__", "__lt__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(self, &left, &right, BinaryOp::Gt) {
                    return r;
                }
                self.compare(left, right, BinaryOp::Gt, ">", |o| o.is_gt())
            }
            BinaryOp::Ge => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__ge__", "__le__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(self, &left, &right, BinaryOp::Ge) {
                    return r;
                }
                self.compare(left, right, BinaryOp::Ge, ">=", |o| o.is_ge())
            }
            BinaryOp::Pow => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__pow__", "__rpow__") {
                    return r;
                }
                // Issue #1204: extract scalar primitive backing so that
                // `MyInt(42) ** 2` works identically to `42 ** 2`.  Keep the
                // original `left`/`right` for the TypeError arm so CPython's
                // subclass-named message (`'C' and 'str'`, #2544) is preserved.
                let cl = coerce_numeric(&left);
                let cr = coerce_numeric(&right);
                // When either operand is complex, use complex
                // exponentiation: z^w = exp(w * ln(z)).  `both_as_complex`
                // returns Ok(Some) only when at least one operand is
                // already a Complex value; pure int/float/bigint pairs
                // route through the canonical NumericOps slot below.
                if let Some(((zr, zi), (wr, wi))) = both_as_complex(&cl, &cr)? {
                    return complex_pow(zr, zi, wr, wi);
                }
                // Canonical numeric `**` via the NumericOps slot table
                // (#458): int**int (BigInt promotion on overflow, #421/#484),
                // the BigInt-exponent OverflowError arms, and the float
                // power path (negative-real → complex, 0.0 ** negative
                // ZeroDivisionError).
                if let Some(result) = dispatch_numeric_binop(BinaryOp::Pow, &cl, &cr) {
                    return result;
                }
                Err(unsupported_operand("** or pow()", &left, &right))
            }
            BinaryOp::BitAnd => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__and__", "__rand__") {
                    return r;
                }
                if let Some(r) = set_binary_op(self, &left, &right, SetOp::And, "&") {
                    return r;
                }
                // CPython keeps the `bool` type for `bool & bool` (only `&`,
                // `|`, `^`; arithmetic like `True + True` yields `int`).  Catch
                // this before `coerce_numeric` collapses Bool → Int below.  A
                // single int operand makes the result `int`, so mixed
                // bool/int falls through to the int path.
                if let (ValueKind::Bool(a), ValueKind::Bool(b)) = (left.kind(), right.kind()) {
                    return Ok(Value::bool_(a & b));
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = crate::interpreter::error_type_name_str(&left);
                let rt = crate::interpreter::error_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `&` via the NumericOps slot table (#458):
                // int×int, BigInt cross-type arms (#485), and Bool coercion
                // all in one site.  Float / non-numeric operands return None
                // → operand-type TypeError below.
                if let Some(result) = dispatch_numeric_binop(BinaryOp::BitAnd, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!(
                    "unsupported operand type(s) for &: '{lt}' and '{rt}'"
                ))
            }
            BinaryOp::BitOr => {
                // Issue #2936: CPython's `mappingproxy_or` merges the *proxied
                // object*, so `mappingproxy(od) | other` is `od | other` and
                // keeps `od`'s type.  Owner-less proxies (plain dict / class
                // `__dict__`) keep merging as `dict` through the entries path
                // below.  `proxied_of` follows a nested chain to its end so
                // `mappingproxy(mappingproxy(counter)) | other` still runs
                // `Counter.__or__` rather than the inner proxy's dict merge.
                let left = pyrust_builtins::mapping_proxy::proxied_of(&left).unwrap_or(left);
                let right = pyrust_builtins::mapping_proxy::proxied_of(&right).unwrap_or(right);
                if let Some(r) = self.try_dunder_binary(&left, &right, "__or__", "__ror__") {
                    return r;
                }
                // PEP 584: dict | dict → new merged dict (right wins on key collision).
                // Covers plain `dict` and PyInstance dict subclasses; PyInstance subclasses
                // with a custom `__or__` were already handled by the dunder path above.
                if let Some(lhs_entries) = dict_entries_from_value(&left) {
                    // A mappingproxy's `|` is `dict.__or__`, so a failing merge
                    // reports a mappingproxy operand as `dict` (CPython 3.12).
                    let left_type = bitor_operand_type_name(&left);
                    let right_type = bitor_operand_type_name(&right);
                    let Some(rhs_entries) = dict_entries_from_value(&right) else {
                        return Err(pyrust_core::type_err!(
                            "unsupported operand type(s) for |: '{left_type}' and '{right_type}'"
                        ));
                    };
                    // #1914: dedup via user `__eq__` for `PyKey::Object` keys.
                    // `dict_extend_dedup` keeps the raw fast path for the common
                    // all-primitive case; later values win on duplicate keys.
                    let mut merged = if let Some(dict) = dict_clone_from_value(&left) {
                        dict
                    } else {
                        let mut dict = PyDict::default();
                        self.dict_extend_dedup(&mut dict, lhs_entries)?;
                        dict
                    };
                    self.dict_extend_dedup(&mut merged, rhs_entries)?;
                    return Ok(Value::dict(merged));
                }
                if let Some(r) = set_binary_op(self, &left, &right, SetOp::Or, "|") {
                    return r;
                }
                // PEP 604: `type | type` (and `None | type`, `type | None`,
                // `UnionType | type`, etc.) creates a `types.UnionType`.
                // `None` is coerced to `NoneType` as the union component.
                // At least one operand must be a strict type (PyClass / BuiltinFunction /
                // UnionType): `None | None` has neither side as a type and must raise
                // TypeError, matching CPython 3.12 (`type.__or__` is what makes it work).
                if is_union_operand(&left)
                    && is_union_operand(&right)
                    && (is_strict_type_union_operand(&left) || is_strict_type_union_operand(&right))
                {
                    let lhs = coerce_none_to_nonetype(left);
                    let rhs = coerce_none_to_nonetype(right);
                    return Ok(pyrust_builtins::union_type::make_union_type(lhs, rhs));
                }
                // `None | None`: both operands looked like union components but neither
                // was a type, so CPython raises TypeError with the operand-type message.
                if is_union_operand(&left) && is_union_operand(&right) {
                    let lt = crate::interpreter::error_type_name_str(&left);
                    let rt = crate::interpreter::error_type_name_str(&right);
                    return Err(pyrust_core::type_err!(
                        "unsupported operand type(s) for |: '{lt}' and '{rt}'"
                    ));
                }
                // CPython keeps the `bool` type for `bool | bool`.  Catch this
                // before `coerce_numeric` collapses Bool → Int below; mixed
                // bool/int falls through to the int path.
                if let (ValueKind::Bool(a), ValueKind::Bool(b)) = (left.kind(), right.kind()) {
                    return Ok(Value::bool_(a | b));
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                // A mappingproxy's `|` / `__ror__` is `dict.__or__`, so a failing
                // merge names it `dict` on either side (CPython 3.12).
                let lt = bitor_operand_type_name(&left);
                let rt = bitor_operand_type_name(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `|` via the NumericOps slot table (#458).
                if let Some(result) = dispatch_numeric_binop(BinaryOp::BitOr, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!(
                    "unsupported operand type(s) for |: '{lt}' and '{rt}'"
                ))
            }
            BinaryOp::BitXor => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__xor__", "__rxor__") {
                    return r;
                }
                if let Some(r) = set_binary_op(self, &left, &right, SetOp::Xor, "^") {
                    return r;
                }
                // CPython keeps the `bool` type for `bool ^ bool`.  Catch this
                // before `coerce_numeric` collapses Bool → Int below; mixed
                // bool/int falls through to the int path.
                if let (ValueKind::Bool(a), ValueKind::Bool(b)) = (left.kind(), right.kind()) {
                    return Ok(Value::bool_(a ^ b));
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = crate::interpreter::error_type_name_str(&left);
                let rt = crate::interpreter::error_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `^` via the NumericOps slot table (#458).
                if let Some(result) = dispatch_numeric_binop(BinaryOp::BitXor, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!(
                    "unsupported operand type(s) for ^: '{lt}' and '{rt}'"
                ))
            }
            BinaryOp::LShift => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__lshift__", "__rlshift__")
                {
                    return r;
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = crate::interpreter::error_type_name_str(&left);
                let rt = crate::interpreter::error_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `<<` via the NumericOps slot table (#458):
                // BigInt-exact shift, Int→BigInt promotion, the
                // OverflowError / "0 << huge" saturation, and the
                // ValueError("negative shift count").  A Float / non-int
                // operand returns None → operand-type TypeError below.
                if let Some(result) = dispatch_numeric_binop(BinaryOp::LShift, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!(
                    "unsupported operand type(s) for <<: '{lt}' and '{rt}'"
                ))
            }
            BinaryOp::RShift => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__rshift__", "__rrshift__")
                {
                    return r;
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = crate::interpreter::error_type_name_str(&left);
                let rt = crate::interpreter::error_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `>>` via the NumericOps slot table (#458):
                // BigInt-exact shift, sign-collapse on huge counts, and the
                // ValueError("negative shift count").
                if let Some(result) = dispatch_numeric_binop(BinaryOp::RShift, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!(
                    "unsupported operand type(s) for >>: '{lt}' and '{rt}'"
                ))
            }
            BinaryOp::In => self.eval_in(right, left),
            BinaryOp::NotIn => Ok(Value::bool_(!self.eval_in(right, left)?.truthy_raw())),
            BinaryOp::Is => Ok(Value::bool_(values_are_identical(&left, &right))),
            BinaryOp::IsNot => Ok(Value::bool_(!values_are_identical(&left, &right))),
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuit handled earlier"),
        }
    }
}
