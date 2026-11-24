/// Canonical unary `-`/`+`/`~`/`not` evaluation for built-in operands.
///
/// Runtime dispatch, built-in numeric dunders, and optimizer constant folding
/// share this definition through `pyrust_core::dispatch_numeric_unary` so
/// promotion and error semantics cannot drift (issue #458).
pub(crate) fn eval_builtin_unary(op: UnaryOp, value: Value) -> Result<Value> {
    let num_op = match op {
        UnaryOp::Neg => pyrust_core::NumericUnaryOp::Neg,
        UnaryOp::Pos => pyrust_core::NumericUnaryOp::Pos,
        UnaryOp::BitNot => pyrust_core::NumericUnaryOp::Invert,
        UnaryOp::Not => return Ok(Value::bool_(!value.truthy_raw())),
    };
    if let Some(result) = pyrust_core::dispatch_numeric_unary(num_op, &value) {
        return result;
    }
    Err(pyrust_core::type_err!(
        "bad operand type for {}: '{}'",
        num_op.symbol(),
        value_type_name_str(&value)
    ))
}

/// Reuse the fast-path definition when expression evaluation is invoked
/// outside the opcode loop. Keeping this small wrapper gives LLVM the same
/// inlining boundary as the original tagged-integer specialization.
#[inline]
fn try_unary_int(value: &Value, op: UnaryOp) -> Option<Value> {
    try_tagged_int_unary!(value, op)
}

impl Interpreter {
    #[inline(always)]
    pub(crate) fn eval_unary(&mut self, value: Value, op: UnaryOp) -> Result<Value> {
        if let Some(result) = try_unary_int(&value, op) {
            return Ok(result);
        }
        if op == UnaryOp::Not {
            return self
                .truthy_value(&value)
                .map(|truthy| Value::bool_(!truthy));
        }

        let method = match op {
            UnaryOp::Neg => "__neg__",
            UnaryOp::Pos => "__pos__",
            UnaryOp::BitNot => "__invert__",
            UnaryOp::Not => unreachable!("handled above"),
        };
        if let Some(result) = self.try_dunder_unary(&value, method) {
            return result;
        }

        let is_instance = matches!(value.kind(), ValueKind::PyInstance(_));
        let operand = builtin_data_backing(&value).unwrap_or_else(|| value.clone());
        eval_builtin_unary(op, operand).map_err(|err| {
            if is_instance && matches!(&err, PyError::Named(cls, _) if cls.as_ref() == "TypeError")
            {
                let sym = match op {
                    UnaryOp::Neg => "unary -",
                    UnaryOp::Pos => "unary +",
                    UnaryOp::BitNot => "unary ~",
                    UnaryOp::Not => unreachable!("handled above"),
                };
                pyrust_core::type_err!(
                    "bad operand type for {sym}: '{}'",
                    value_type_name_str(&value)
                )
            } else {
                err
            }
        })
    }
}
