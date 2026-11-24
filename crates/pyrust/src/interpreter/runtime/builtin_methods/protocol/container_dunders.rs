// Container, sequence, set/dict algebra, mappingproxy, and str/bytes `%`
// protocol dunders.

impl Interpreter {
    fn dispatch_container_protocol_dunder(
        &mut self,
        method: &str,
        type_name: &str,
        receiver: Value,
        mut args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "__len__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch =
                    crate::builtin_registry::lookup("len").expect("len must be in the registry");
                dispatch(self, &[arg])
            }
            "__getitem__" => {
                let index = args.pop().unwrap();
                self.eval_index(&receiver, index)
            }
            "__contains__" => {
                let item = args.pop().unwrap();
                self.eval_in(receiver, item)
            }
            "__add__" => {
                let other = args.pop().unwrap();
                self.add(receiver, other)
            }
            "__mul__" => {
                // CPython's `sq_repeat` slot wrapper (`list.__mul__`,
                // `str.__mul__`, …) requires the repeat count to be int-like
                // and raises `'X' object cannot be interpreted as an integer`
                // for anything else — stricter than the `*` operator, which
                // says "can't multiply sequence by non-int".  Resolve the
                // count through `__index__` so the dunder matches CPython, then
                // delegate to the same repetition machinery as `*`.
                let other = args.pop().unwrap();
                // Resolve the count through the shared index protocol (#2022):
                // int/bool/bigint/int-subclass/`__index__` are accepted; float
                // and `__int__`-only objects raise the canonical TypeError.
                let count = self.value_to_index(&other, |v| {
                    pyrust_core::type_err!(
                        "'{}' object cannot be interpreted as an integer",
                        pyrust_core::builtin_type_name(v)
                    )
                })?;
                if matches!(count.kind(), ValueKind::BigInt(_)) {
                    return Err(pyrust_core::overflow_err!(
                        "cannot fit '{}' into an index-sized integer",
                        crate::interpreter::error_type_name_str(&other)
                    ));
                }
                self.mul(receiver, count)
            }
            "__setitem__" => {
                let value = args.pop().unwrap();
                let index = args.pop().unwrap();
                // Reuse the VM item-assign machinery (slice assignment, dict
                // key dedup, bytearray __index__ resolution) via a scratch
                // register file: obj@0, idx@1, val@2.  The receiver is not the
                // module globals dict, so the globals write-through in
                // `exec_set_item` stays inert.
                let mut scratch = vec![receiver, index, value];
                let mut regs = unsafe { RegSlice::from_raw(scratch.as_mut_ptr(), scratch.len()) };
                self.exec_set_item(&mut regs, 0, 0, 1, 2)?;
                Ok(Value::none())
            }
            "__delitem__" => {
                let index = args.pop().unwrap();
                let mut scratch = vec![receiver, index];
                let mut regs = unsafe { RegSlice::from_raw(scratch.as_mut_ptr(), scratch.len()) };
                self.exec_delete_item(&mut regs, 0, 0, 1)?;
                Ok(Value::none())
            }
            "__or__" | "__and__" | "__sub__" | "__xor__"
                if matches!(
                    pyrust_builtins::dict_views::view_kind(&receiver),
                    Some(
                        pyrust_builtins::dict_views::DictViewKind::Keys
                            | pyrust_builtins::dict_views::DictViewKind::Items
                    )
                ) =>
            {
                let other = args.pop().unwrap();
                let (op, symbol) = match method {
                    "__or__" => (SetOp::Or, "|"),
                    "__and__" => (SetOp::And, "&"),
                    "__sub__" => (SetOp::Sub, "-"),
                    _ => (SetOp::Xor, "^"),
                };
                match set_binary_op(self, &receiver, &other, op, symbol) {
                    Some(result) => result,
                    None => Ok(Value::not_implemented()),
                }
            }
            "__ror__" | "__rand__" | "__rsub__" | "__rxor__"
                if matches!(
                    pyrust_builtins::dict_views::view_kind(&receiver),
                    Some(
                        pyrust_builtins::dict_views::DictViewKind::Keys
                            | pyrust_builtins::dict_views::DictViewKind::Items
                    )
                ) =>
            {
                let other = args.pop().unwrap();
                let (op, symbol) = match method {
                    "__ror__" => (SetOp::Or, "|"),
                    "__rand__" => (SetOp::And, "&"),
                    "__rsub__" => (SetOp::Sub, "-"),
                    _ => (SetOp::Xor, "^"),
                };
                match set_binary_op(self, &other, &receiver, op, symbol) {
                    Some(result) => result,
                    None => Ok(Value::not_implemented()),
                }
            }
            // list/bytearray in-place dunders (#2119, #2989): direct calls to
            // `sq_inplace_concat` / `sq_inplace_repeat` mutate the receiver in
            // place and return it without consulting RHS `__radd__`/`__rmul__`.
            "__iadd__" if matches!(type_name, "list" | "bytearray") => {
                let other = args.pop().unwrap();
                if matches!(receiver.kind(), ValueKind::List(_)) {
                    self.extend_list_from_iterable(&receiver, &other)?;
                    Ok(receiver)
                } else if let Some(data_rc) = pyrust_builtins::bytearray::as_bytearray_rc(&receiver)
                {
                    super::expressions::bytearray_iadd_in_place(&data_rc, &receiver, &other)?;
                    Ok(receiver)
                } else {
                    match self.try_inplace_op(&receiver, crate::ast::BinaryOp::Add, &other, true)? {
                        Some(v) => Ok(v),
                        None => self.eval_binary(receiver, crate::ast::BinaryOp::Add, other),
                    }
                }
            }
            "__imul__" if matches!(type_name, "list" | "bytearray") => {
                // The `sq_inplace_repeat` slot wrapper resolves the count
                // through `__index__` (like `__mul__`/`sq_repeat`), so a float
                // raises `'X' object cannot be interpreted as an integer` —
                // stricter than the `*=` operator's "can't multiply sequence by
                // non-int" message.  Resolve first, then mutate in place.
                let other = args.pop().unwrap();
                let count = self.value_to_index(&other, |v| {
                    pyrust_core::type_err!(
                        "'{}' object cannot be interpreted as an integer",
                        pyrust_core::builtin_type_name(v)
                    )
                })?;
                if matches!(count.kind(), ValueKind::BigInt(_)) {
                    return Err(pyrust_core::overflow_err!(
                        "cannot fit '{}' into an index-sized integer",
                        crate::interpreter::error_type_name_str(&other)
                    ));
                }
                let n = super::expressions::int_repeat_count(&count)
                    .expect("value_to_index non-BigInt is Int/Bool");
                if matches!(receiver.kind(), ValueKind::List(_)) {
                    super::expressions::list_repeat_in_place(&receiver, n)?;
                    Ok(receiver)
                } else if let Some(data_rc) = pyrust_builtins::bytearray::as_bytearray_rc(&receiver)
                {
                    super::expressions::bytearray_repeat_in_place(&data_rc, n)?;
                    Ok(receiver)
                } else {
                    match self.try_inplace_op(&receiver, crate::ast::BinaryOp::Mul, &count, true)? {
                        Some(v) => Ok(v),
                        None => self.eval_binary(receiver, crate::ast::BinaryOp::Mul, count),
                    }
                }
            }
            // set/frozenset/dict forward algebra & merge dunders (#2122).
            // CPython returns `NotImplemented` (not TypeError) when the other
            // operand is not set-/dict-compatible, so guard the operand type
            // before delegating to the operator machinery.
            "__or__" | "__and__" | "__sub__" | "__xor__"
                if matches!(type_name, "set" | "frozenset") =>
            {
                let other = args.pop().unwrap();
                if set_items_from_value(&other).is_none() {
                    return Ok(Value::not_implemented());
                }
                let op = match method {
                    "__or__" => crate::ast::BinaryOp::BitOr,
                    "__and__" => crate::ast::BinaryOp::BitAnd,
                    "__sub__" => crate::ast::BinaryOp::Sub,
                    _ => crate::ast::BinaryOp::BitXor,
                };
                self.eval_binary(receiver, op, other)
            }
            // Reflected set dunders: `a.__rOP__(b)` computes `b OP a`.  Same
            // NotImplemented guard as the forward forms.
            "__ror__" | "__rand__" | "__rsub__" | "__rxor__"
                if matches!(type_name, "set" | "frozenset") =>
            {
                let other = args.pop().unwrap();
                if set_items_from_value(&other).is_none() {
                    return Ok(Value::not_implemented());
                }
                let op = match method {
                    "__ror__" => crate::ast::BinaryOp::BitOr,
                    "__rand__" => crate::ast::BinaryOp::BitAnd,
                    "__rsub__" => crate::ast::BinaryOp::Sub,
                    _ => crate::ast::BinaryOp::BitXor,
                };
                self.eval_binary(other, op, receiver)
            }
            // set in-place algebra dunders (#2122).  Unlike the `|=`/`&=`/…
            // operators (which raise TypeError on a non-set operand), the
            // dunder returns `NotImplemented`, so guard first and only then
            // route through the mutating operator machinery.
            "__ior__" | "__iand__" | "__isub__" | "__ixor__" if type_name == "set" => {
                let other = args.pop().unwrap();
                if set_items_from_value(&other).is_none() {
                    return Ok(Value::not_implemented());
                }
                let op = match method {
                    "__ior__" => crate::ast::BinaryOp::BitOr,
                    "__iand__" => crate::ast::BinaryOp::BitAnd,
                    "__isub__" => crate::ast::BinaryOp::Sub,
                    _ => crate::ast::BinaryOp::BitXor,
                };
                match self.try_inplace_op(&receiver, op, &other, true)? {
                    Some(v) => Ok(v),
                    None => Ok(receiver),
                }
            }
            // dict PEP-584 forward / reflected merge dunders (#2122).  Returns
            // `NotImplemented` when the other operand is not a mapping (matching
            // `dict.__or__`/`__ror__`, which only accept dicts — not arbitrary
            // iterables of pairs, unlike `__ior__`).
            "__or__" | "__ror__" if type_name == "dict" => {
                let other = args.pop().unwrap();
                if dict_entries_from_value(&other).is_none() {
                    return Ok(Value::not_implemented());
                }
                if method == "__or__" {
                    self.eval_binary(receiver, crate::ast::BinaryOp::BitOr, other)
                } else {
                    self.eval_binary(other, crate::ast::BinaryOp::BitOr, receiver)
                }
            }
            // dict `__ior__` (#2122): identical to `|=` — accepts dicts *and*
            // iterables of (key, value) pairs, mutates in place, returns self.
            "__ior__" if type_name == "dict" => {
                let other = args.pop().unwrap();
                match self.try_inplace_op(&receiver, crate::ast::BinaryOp::BitOr, &other, true)? {
                    Some(v) => Ok(v),
                    None => Ok(receiver),
                }
            }
            // Issue #3019: CPython's `mappingproxy_or` calls
            // `PyNumber_Or(pp->mapping, other)` directly (so `p.__or__(1)`
            // raises `TypeError: unsupported operand type(s) for |: 'dict' and 'int'`,
            // not `NotImplemented`); `mappingproxy.__ior__` always rejects `|=`.
            "__or__" if type_name == "mappingproxy" => {
                let other = args.pop().unwrap();
                self.eval_binary(receiver, crate::ast::BinaryOp::BitOr, other)
            }
            "__ror__" if type_name == "mappingproxy" => {
                let other = args.pop().unwrap();
                self.eval_binary(other, crate::ast::BinaryOp::BitOr, receiver)
            }
            "__ior__" if type_name == "mappingproxy" => Err(pyrust_core::type_err!(
                "'|=' is not supported by mappingproxy; use '|' instead"
            )),
            // Issue #2387: str/bytes/bytearray `%` formatting slots, exposed as
            // method-wrappers (`'%s'.__mod__('x')`, `hasattr(bytes, '__mod__')`).
            // `a.__rmod__(b)` computes `b % a` (swapped operands).  Both delegate
            // to the same `eval_binary` Mod path the `%` operator uses, so the
            // format-machinery TypeErrors match CPython byte-for-byte.
            "__mod__" if matches!(type_name, "str" | "bytes" | "bytearray") => {
                let other = args.pop().unwrap();
                self.eval_binary(receiver, crate::ast::BinaryOp::Mod, other)
            }
            "__rmod__" if matches!(type_name, "str" | "bytes" | "bytearray") => {
                let other = args.pop().unwrap();
                self.eval_binary(other, crate::ast::BinaryOp::Mod, receiver)
            }
            other => Err(PyError::Runtime(format!(
                "internal: unhandled builtin protocol dunder '{other}'"
            ))),
        }
    }
}
