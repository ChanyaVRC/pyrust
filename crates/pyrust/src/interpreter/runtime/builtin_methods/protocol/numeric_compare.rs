// Rich-comparison and scalar-numeric protocol dunders (`__eq__`..`__ge__`,
// `__round__`, unary/binary/reflected numeric slots).

impl Interpreter {
    /// Rich-comparison dunders (issue #2070): exposed on every primitive type,
    /// mappingproxy, and dict view.  The forward slot returns `NotImplemented`
    /// for operand types it does not accept (`(5).__eq__('x')`, `(1,).__lt__([1])`),
    /// leaving the implicit `==`/`<` operator hot paths untouched.
    fn dispatch_richcmp_protocol_dunder(
        &mut self,
        method: &str,
        receiver: Value,
        other: Value,
    ) -> Result<Value> {
        // Issue #3019: CPython's `mappingproxy_richcompare` calls
        // `PyObject_RichCompare(pp->mapping, other, op)` directly —
        // so `p.__eq__(1)` returns `False` (not `NotImplemented`) and
        // `p.__lt__(1)` raises `TypeError` (not `NotImplemented`).
        if pyrust_builtins::mapping_proxy::is_mapping_proxy(&receiver) {
            let op = match method {
                "__eq__" => crate::ast::BinaryOp::Eq,
                "__ne__" => crate::ast::BinaryOp::Ne,
                "__lt__" => crate::ast::BinaryOp::Lt,
                "__le__" => crate::ast::BinaryOp::Le,
                "__gt__" => crate::ast::BinaryOp::Gt,
                _ => crate::ast::BinaryOp::Ge,
            };
            return self.eval_binary(receiver, op, other);
        }
        match pyrust_builtins::dict_views::view_kind(&receiver) {
            Some(pyrust_builtins::dict_views::DictViewKind::Values) => {
                return Ok(match method {
                    "__eq__" => {
                        if values_are_identical(&receiver, &other) {
                            Value::bool_(true)
                        } else {
                            Value::not_implemented()
                        }
                    }
                    "__ne__" => {
                        if values_are_identical(&receiver, &other) {
                            Value::bool_(false)
                        } else {
                            Value::not_implemented()
                        }
                    }
                    _ => Value::not_implemented(),
                });
            }
            Some(
                pyrust_builtins::dict_views::DictViewKind::Keys
                | pyrust_builtins::dict_views::DictViewKind::Items,
            ) => {
                if !is_set_comparison_operand(&other) {
                    return Ok(Value::not_implemented());
                }
                return match method {
                    "__eq__" => self.values_user_eq(&receiver, &other).map(Value::bool_),
                    "__ne__" => self
                        .values_user_eq(&receiver, &other)
                        .map(|equal| Value::bool_(!equal)),
                    _ => {
                        let op = match method {
                            "__lt__" => crate::ast::BinaryOp::Lt,
                            "__le__" => crate::ast::BinaryOp::Le,
                            "__gt__" => crate::ast::BinaryOp::Gt,
                            _ => crate::ast::BinaryOp::Ge,
                        };
                        match set_subset_cmp(self, &receiver, &other, op) {
                            Some(result) => result,
                            None => Ok(Value::not_implemented()),
                        }
                    }
                };
            }
            None => {}
        }
        self.primitive_richcmp_dunder(method, &receiver, &other)
    }

    /// Dispatch a rich-comparison dunder (`__eq__`/`__ne__`/`__lt__`/…) accessed
    /// directly on a primitive instance (issue #2070).  Returns the boolean
    /// result of the comparison, or `NotImplemented` when the forward slot does
    /// not accept the operand type — matching CPython's per-type slot semantics
    /// exactly (`(5).__eq__('x')` → `NotImplemented`, `(5).__eq__(5.0)` →
    /// `NotImplemented`, `(5.0).__eq__(5)` → `True`, `(1,).__lt__([1])` →
    /// `NotImplemented`).  The `==`/`<` *operators* never call this — they go
    /// through `eval_binary` — so the operator fast paths are untouched.
    fn primitive_richcmp_dunder(
        &mut self,
        method: &str,
        recv: &Value,
        other: &Value,
    ) -> Result<Value> {
        // Issue #2847: an explicitly selected primitive base slot compares
        // the builtin payload of subclass instances; it must neither reject
        // their visible subclass names nor redispatch their user overrides.
        // Keep genuine user objects intact so incompatible operands still
        // produce the base slot's NotImplemented result below.
        let recv = coerce_subclass_backing(recv, &[]).unwrap_or_else(|| recv.clone());
        let other = coerce_subclass_backing(other, &[]).unwrap_or_else(|| other.clone());
        let is_equality = matches!(method, "__eq__" | "__ne__");
        if !richcmp_operand_accepted(&recv, &other, is_equality) {
            return Ok(Value::not_implemented());
        }
        let op = match method {
            "__eq__" => crate::ast::BinaryOp::Eq,
            "__ne__" => crate::ast::BinaryOp::Ne,
            "__lt__" => crate::ast::BinaryOp::Lt,
            "__le__" => crate::ast::BinaryOp::Le,
            "__gt__" => crate::ast::BinaryOp::Gt,
            _ => crate::ast::BinaryOp::Ge,
        };
        self.eval_binary(recv, op, other)
    }

    /// Issue #2297: `int.__round__([ndigits])`.  CPython's `int.__round__`
    /// always returns an `int` (`(125).__round__(-1)` → `120`, banker's
    /// rounding); omitted `ndigits` returns the value unchanged.  Routes
    /// through the `round` registry builtin, which already implements the int +
    /// `ndigits` semantics (negative `ndigits` rounding, `BigInt` results).
    ///
    /// Unlike the `round()` builtin, the `int.__round__` *slot* index-coerces
    /// `ndigits` with no `None` special-case: `(5).__round__(None)` raises
    /// `TypeError: 'NoneType' object cannot be interpreted as an integer`,
    /// whereas `round(5, None)` returns `5`.  `round` swallows an explicit
    /// `None` (treats it as "omitted"), so reject it here before delegating.
    fn int_round_dunder(&mut self, recv: &Value, ndigits: Option<Value>) -> Result<Value> {
        if let Some(n) = &ndigits
            && matches!(n.kind(), ValueKind::None)
        {
            return Err(pyrust_core::type_err!(
                "'NoneType' object cannot be interpreted as an integer"
            ));
        }
        let dispatch =
            crate::builtin_registry::lookup("round").expect("round must be in the registry");
        let mut call_args = vec![ExpandedCallArg {
            name: None,
            value: recv.clone(),
        }];
        if let Some(n) = ndigits {
            call_args.push(ExpandedCallArg {
                name: None,
                value: n,
            });
        }
        dispatch(self, &call_args)
    }

    /// Dispatch a scalar-numeric forward dunder (int/float/complex/bool) accessed
    /// directly on a primitive instance (issue #2070): the binary arithmetic /
    /// bitwise slots and the unary `__neg__`/`__pos__`/`__abs__`/`__invert__`.
    ///
    /// Returns:
    /// - `Some(Ok(NotImplemented))` when a *binary* slot's operand is outside the
    ///   receiver's accepted numeric set (`(5).__add__(5.0)` → `NotImplemented`);
    /// - `Some(Ok(result))` / `Some(Err(..))` when the slot computed / raised;
    /// - `None` when `method` is not a scalar-numeric dunder (the caller then
    ///   falls through to the container/other arms).
    fn primitive_numeric_dunder(
        &mut self,
        method: &str,
        recv: &Value,
        args: &mut Vec<Value>,
    ) -> Result<Option<Value>> {
        use crate::ast::BinaryOp;
        // Unary slots first — no operand-acceptance check needed.  `__neg__` /
        // `__pos__` / `__invert__` route through canonical built-in unary
        // evaluation; `__abs__` goes through the registry builtin (complex
        // `abs` returns a float, which unary evaluation does not cover).
        match method {
            "__neg__" => {
                return Ok(Some(eval_builtin_unary(
                    crate::ast::UnaryOp::Neg,
                    recv.clone(),
                )?));
            }
            "__pos__" => {
                return Ok(Some(eval_builtin_unary(
                    crate::ast::UnaryOp::Pos,
                    recv.clone(),
                )?));
            }
            "__invert__" => {
                return Ok(Some(eval_builtin_unary(
                    crate::ast::UnaryOp::BitNot,
                    recv.clone(),
                )?));
            }
            "__abs__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: recv.clone(),
                };
                let dispatch =
                    crate::builtin_registry::lookup("abs").expect("abs must be in the registry");
                return Ok(Some(dispatch(self, &[arg])?));
            }
            _ => {}
        }
        // `__divmod__` / `__rdivmod__` route through the `divmod` builtin
        // (which returns a 2-tuple) rather than a single `BinaryOp`.  The
        // reflected form swaps the operand order (`b.__rdivmod__(a)` →
        // `divmod(a, b)`), issue #2215.
        match method {
            "__divmod__" => {
                let other = args.pop().unwrap();
                if !numeric_operand_accepted(recv, &other) {
                    return Ok(Some(Value::not_implemented()));
                }
                let dispatch = crate::builtin_registry::lookup("divmod")
                    .expect("divmod must be in the registry");
                let a = ExpandedCallArg {
                    name: None,
                    value: recv.clone(),
                };
                let b = ExpandedCallArg {
                    name: None,
                    value: other,
                };
                return Ok(Some(dispatch(self, &[a, b])?));
            }
            "__rdivmod__" => {
                let other = args.pop().unwrap();
                if !numeric_operand_accepted(recv, &other) {
                    return Ok(Some(Value::not_implemented()));
                }
                let dispatch = crate::builtin_registry::lookup("divmod")
                    .expect("divmod must be in the registry");
                let a = ExpandedCallArg {
                    name: None,
                    value: other,
                };
                let b = ExpandedCallArg {
                    name: None,
                    value: recv.clone(),
                };
                return Ok(Some(dispatch(self, &[a, b])?));
            }
            _ => {}
        }
        // Reflected binary slots (issue #2215): same op table as forward, but
        // with operands swapped (`eval_binary(other, op, recv)` computes
        // `other OP recv`).  Acceptance is identical to the forward direction
        // (see `numeric_operand_accepted`): `(5).__radd__(2.5)` →
        // `NotImplemented` because float outranks int.
        if let Some(rop) = match method {
            "__radd__" => Some(BinaryOp::Add),
            "__rsub__" => Some(BinaryOp::Sub),
            "__rmul__" => Some(BinaryOp::Mul),
            "__rtruediv__" => Some(BinaryOp::Div),
            "__rfloordiv__" => Some(BinaryOp::FloorDiv),
            "__rmod__" => Some(BinaryOp::Mod),
            "__rpow__" => Some(BinaryOp::Pow),
            "__rand__" => Some(BinaryOp::BitAnd),
            "__ror__" => Some(BinaryOp::BitOr),
            "__rxor__" => Some(BinaryOp::BitXor),
            "__rlshift__" => Some(BinaryOp::LShift),
            "__rrshift__" => Some(BinaryOp::RShift),
            _ => None,
        } {
            let other = args.pop().unwrap();
            if !numeric_operand_accepted(recv, &other) {
                return Ok(Some(Value::not_implemented()));
            }
            return Ok(Some(self.eval_binary(other, rop, recv.clone())?));
        }
        let op = match method {
            "__add__" => BinaryOp::Add,
            "__sub__" => BinaryOp::Sub,
            "__mul__" => BinaryOp::Mul,
            "__truediv__" => BinaryOp::Div,
            "__floordiv__" => BinaryOp::FloorDiv,
            "__mod__" => BinaryOp::Mod,
            "__pow__" => BinaryOp::Pow,
            "__and__" => BinaryOp::BitAnd,
            "__or__" => BinaryOp::BitOr,
            "__xor__" => BinaryOp::BitXor,
            "__lshift__" => BinaryOp::LShift,
            "__rshift__" => BinaryOp::RShift,
            _ => return Ok(None),
        };
        let other = args.pop().unwrap();
        if !numeric_operand_accepted(recv, &other) {
            return Ok(Some(Value::not_implemented()));
        }
        Ok(Some(self.eval_binary(recv.clone(), op, other)?))
    }
}
