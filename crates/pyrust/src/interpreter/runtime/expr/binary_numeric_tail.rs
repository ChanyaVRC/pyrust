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
        &mut self,
        left: Value,
        right: Value,
        op: BinaryOp,
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
        match compare_values_with_op(&left, &right, op_name) {
            Ok(ordering) => Ok(Value::bool_(cmp(ordering))),
            Err(error) => {
                if !matches!(&error, PyError::Named(class, _) if class.as_ref() == "TypeError") {
                    return Err(error);
                }
                self.richcmp_sequence_binary(&left, &right, op)
                    .unwrap_or(Err(error))
            }
        }
    }

    /// Interpreter-aware fallback for list/tuple ordering whose primitive
    /// comparison reached a user instance (issue #2817).
    ///
    /// CPython scans an equality prefix, then returns the exact requested rich
    /// comparison for the first unequal pair.  Returning that value directly
    /// preserves non-bool dunder results; recursively entering `eval_binary`
    /// also retains reflected/subtype dispatch and exception propagation.
    fn richcmp_sequence_binary(
        &mut self,
        left: &Value,
        right: &Value,
        op: BinaryOp,
    ) -> Option<Result<Value>> {
        if !matches!(
            (left.kind(), right.kind()),
            (ValueKind::List(_), ValueKind::List(_)) | (ValueKind::Tuple(_), ValueKind::Tuple(_))
        ) {
            return None;
        }

        Some((|| {
            let mut index = 0;
            loop {
                // Clone only the current pair, then drop the list borrow before
                // invoking user code.  Re-borrowing on each iteration mirrors
                // CPython when `__eq__` mutates a list: an equal prefix is
                // followed by a comparison of the operands' live lengths.
                let (left_item, right_item) = match (left.kind(), right.kind()) {
                    (ValueKind::List(left), ValueKind::List(right)) => {
                        if index >= left.len().min(right.len()) {
                            return Ok(Value::bool_(match op {
                                BinaryOp::Lt => left.len() < right.len(),
                                BinaryOp::Le => left.len() <= right.len(),
                                BinaryOp::Gt => left.len() > right.len(),
                                BinaryOp::Ge => left.len() >= right.len(),
                                _ => unreachable!("sequence comparison used non-ordering operator"),
                            }));
                        }
                        (left[index].clone(), right[index].clone())
                    }
                    (ValueKind::Tuple(left), ValueKind::Tuple(right)) => {
                        if index >= left.len().min(right.len()) {
                            return Ok(Value::bool_(match op {
                                BinaryOp::Lt => left.len() < right.len(),
                                BinaryOp::Le => left.len() <= right.len(),
                                BinaryOp::Gt => left.len() > right.len(),
                                BinaryOp::Ge => left.len() >= right.len(),
                                _ => unreachable!("sequence comparison used non-ordering operator"),
                            }));
                        }
                        (left[index].clone(), right[index].clone())
                    }
                    _ => unreachable!("sequence kinds changed during comparison"),
                };

                if self.values_user_eq(&left_item, &right_item)? {
                    index += 1;
                } else {
                    // CPython fetches the differing pair again after `__eq__`
                    // returns, so replacements made by that hook participate
                    // in ordering.  If mutation removed the current index,
                    // there is no pair left to order and the live lengths
                    // decide the result instead.
                    let (live_items, left_len, right_len) = match (left.kind(), right.kind()) {
                        (ValueKind::List(left), ValueKind::List(right)) => (
                            left.get(index)
                                .zip(right.get(index))
                                .map(|(left, right)| (left.clone(), right.clone())),
                            left.len(),
                            right.len(),
                        ),
                        (ValueKind::Tuple(left), ValueKind::Tuple(right)) => (
                            left.get(index)
                                .zip(right.get(index))
                                .map(|(left, right)| (left.clone(), right.clone())),
                            left.len(),
                            right.len(),
                        ),
                        _ => unreachable!("sequence kinds changed during comparison"),
                    };
                    if let Some((left_item, right_item)) = live_items {
                        return self.eval_binary(left_item, op, right_item);
                    }
                    return Ok(Value::bool_(match op {
                        BinaryOp::Lt => left_len < right_len,
                        BinaryOp::Le => left_len <= right_len,
                        BinaryOp::Gt => left_len > right_len,
                        BinaryOp::Ge => left_len >= right_len,
                        _ => unreachable!("sequence comparison used non-ordering operator"),
                    }));
                }
            }
        })())
    }
}
