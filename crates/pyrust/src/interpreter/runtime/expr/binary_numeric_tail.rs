impl Interpreter {
    fn matmul(&mut self, left: Value, right: Value) -> Result<Value> {
        // `@` has no built-in numeric implementation, and the full dunder
        // dispatch — forward `__matmul__`, reflected `__rmatmul__`, the
        // same-type skip and subtype-priority rule — already ran in
        // `eval_binary` via `try_dunder_binary` before this fallback is
        // reached.  Re-dispatching the reflected slot here bypassed that rule
        // and wrongly called `__rmatmul__` on a same-type operand after the
        // forward returned `NotImplemented` (#2092).  Just raise the TypeError.
        Err(unsupported_operand("@", &left, &right))
    }

    fn div(&self, left: Value, right: Value) -> Result<Value> {
        // Issue #1204/#2544: coerce scalar primitive subclass backings for the
        // numeric path, but keep the originals for the subclass-named TypeError.
        let cl = coerce_numeric(&left);
        let cr = coerce_numeric(&right);
        if let Some((a, b)) = both_as_complex(&cl, &cr)? {
            // (ar+ai*j) / (br+bi*j) = ((ar*br + ai*bi) + (ai*br - ar*bi)j) / (br^2 + bi^2)
            let denom = b.0 * b.0 + b.1 * b.1;
            if denom == 0.0 {
                return Err(pyrust_core::zerodiv_err!("complex division by zero"));
            }
            return Ok(Value::complex(
                (a.0 * b.0 + a.1 * b.1) / denom,
                (a.1 * b.0 - a.0 * b.1) / denom,
            ));
        }
        // Canonical numeric true division via the NumericOps slot table
        // (#458).  Non-numeric operands return None → TypeError.
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Div, &cl, &cr) {
            return result;
        }
        Err(unsupported_operand("/", &left, &right))
    }

    fn floor_div(&self, left: Value, right: Value) -> Result<Value> {
        // Issue #1204/#2544: coerce scalar primitive subclass backings for the
        // numeric path, but keep the originals for the subclass-named TypeError.
        let cl = coerce_numeric(&left);
        let cr = coerce_numeric(&right);
        // Canonical numeric floor division via the NumericOps slot table
        // (#458): one site handles int/int (with BigInt promotion on
        // i64::MIN overflow, #485), BigInt cross-type arms, and float
        // floor division, plus the ZeroDivisionError wording.
        if let Some(result) = dispatch_numeric_binop(BinaryOp::FloorDiv, &cl, &cr) {
            return result;
        }
        Err(unsupported_operand("//", &left, &right))
    }

    fn modulo(&self, left: Value, right: Value) -> Result<Value> {
        // Issue #1204/#2544: coerce scalar primitive subclass backings for the
        // numeric path, but keep the originals for the subclass-named TypeError.
        let cl = coerce_numeric(&left);
        let cr = coerce_numeric(&right);
        // Canonical numeric modulo via the NumericOps slot table (#458).
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Mod, &cl, &cr) {
            return result;
        }
        Err(unsupported_operand("%", &left, &right))
    }

    fn compare(
        &self,
        left: Value,
        right: Value,
        op_name: &str,
        cmp: impl Fn(std::cmp::Ordering) -> bool,
    ) -> Result<Value> {
        // Issue #1204: extract scalar primitive backing for subclasses of
        // int/float/str/bytes so that `MyInt(5) < 10` etc. works.
        // Issue #1939: also extract container backing (list/tuple/dict/set
        // subclasses) so `L([1]) < L([2])` compares the backing lists.  A user
        // comparison override was already dispatched at the `BinaryOp::Lt..Ge`
        // sites via `try_dunder_binary` before reaching `compare`, so an empty
        // override list is safe here.
        let left = coerce_operand_backing(&left);
        let right = coerce_operand_backing(&right);
        if matches!(left.kind(), ValueKind::Float(f) if f.is_nan())
            || matches!(right.kind(), ValueKind::Float(f) if f.is_nan())
        {
            return Ok(Value::bool_(false));
        }
        Ok(Value::bool_(cmp(compare_values_with_op(
            &left, &right, op_name,
        )?)))
    }
}
