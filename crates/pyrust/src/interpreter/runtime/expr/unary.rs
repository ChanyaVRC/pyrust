/// Canonical unary `-`/`+`/`~`/`not` evaluation for built-in operands.
///
/// Runtime dispatch, built-in numeric dunders, and optimizer constant folding
/// share this definition so promotion and error semantics cannot drift.
pub(crate) fn eval_builtin_unary(op: UnaryOp, value: Value) -> Result<Value> {
    match op {
        UnaryOp::Neg => match value.kind() {
            ValueKind::Int(integer) => Ok(match integer.checked_neg() {
                Some(result) => Value::int(result),
                None => Value::bigint(-PyBigInt::from(integer)),
            }),
            ValueKind::Float(float) => Ok(Value::float(-float)),
            ValueKind::Complex(real, imag) => Ok(Value::complex(-real, -imag)),
            ValueKind::BigInt(integer) => Ok(Value::bigint(-integer)),
            ValueKind::Bool(boolean) => Ok(Value::int(if boolean { -1 } else { 0 })),
            _ => Err(pyrust_core::type_err!(
                "bad operand type for unary -: '{}'",
                value_type_name_str(&value)
            )),
        },
        UnaryOp::Not => Ok(Value::bool_(!value.truthy_raw())),
        UnaryOp::BitNot => match value.kind() {
            ValueKind::Int(integer) => Ok(Value::int(!integer)),
            ValueKind::Bool(boolean) => Ok(Value::int(if boolean { -2 } else { -1 })),
            ValueKind::BigInt(integer) => Ok(Value::bigint(!integer)),
            _ => Err(pyrust_core::type_err!(
                "bad operand type for unary ~: '{}'",
                value_type_name_str(&value)
            )),
        },
        UnaryOp::Pos => {
            if matches!(value.kind(), ValueKind::BigInt(_)) {
                return Ok(value);
            }
            match value.kind() {
                ValueKind::Int(integer) => Ok(Value::int(integer)),
                ValueKind::Float(float) => Ok(Value::float(float)),
                ValueKind::Complex(real, imag) => Ok(Value::complex(real, imag)),
                ValueKind::Bool(boolean) => Ok(Value::int(if boolean { 1 } else { 0 })),
                _ => Err(pyrust_core::type_err!(
                    "bad operand type for unary +: '{}'",
                    value_type_name_str(&value)
                )),
            }
        }
    }
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

        let operand = builtin_data_backing(&value).unwrap_or(value);
        eval_builtin_unary(op, operand)
    }
}
