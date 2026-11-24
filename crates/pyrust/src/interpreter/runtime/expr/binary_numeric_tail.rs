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
        // Issue #1204/#2544: coerce scalar numeric subclass backings for the
        // numeric path, keeping the originals for subclass-named TypeError.
        let cl = unwrap_numeric_subclass(&left);
        let cr = unwrap_numeric_subclass(&right);
        // Canonical 5-type numeric true division via pyrust_core::NumericOps (#458).
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Div, &cl, &cr) {
            return result;
        }
        Err(unsupported_operand("/", &left, &right))
    }

    fn floor_div(&self, left: Value, right: Value) -> Result<Value> {
        let cl = unwrap_numeric_subclass(&left);
        let cr = unwrap_numeric_subclass(&right);
        // Canonical 5-type numeric floor division via pyrust_core::NumericOps (#458).
        if let Some(result) =
            dispatch_numeric_binop_with_originals(BinaryOp::FloorDiv, &left, &right, &cl, &cr)
        {
            return result;
        }
        Err(unsupported_operand("//", &left, &right))
    }

    fn modulo(&self, left: Value, right: Value) -> Result<Value> {
        let cl = unwrap_numeric_subclass(&left);
        let cr = unwrap_numeric_subclass(&right);
        // Canonical 5-type numeric modulo via pyrust_core::NumericOps (#458).
        if let Some(result) =
            dispatch_numeric_binop_with_originals(BinaryOp::Mod, &left, &right, &cl, &cr)
        {
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
        // Canonical 5-type numeric comparison via pyrust_core::NumericOps (#458):
        // handles exact Int/BigInt vs Float precision, NaN -> False (only for
        // numeric pairs; `nan < 'a'` raises TypeError), and Complex ordering
        // TypeError while preserving original subclass type names.
        let nl = unwrap_numeric_subclass(&left);
        let nr = unwrap_numeric_subclass(&right);
        if let Some(cmp_op) = binary_op_to_numeric_cmp(op)
            && let Some(res) = pyrust_core::dispatch_numeric_compare(cmp_op, &nl, &nr)
        {
            return res.map(Value::bool_).map_err(|err| {
                if matches!(err.kind(), pyrust_core::PyErrorKind::Named(cls, _) if cls.as_ref() == "TypeError")
                    && (matches!(left.kind(), ValueKind::PyInstance(_))
                        || matches!(right.kind(), ValueKind::PyInstance(_)))
                {
                    pyrust_core::type_err!(
                        "'{op_name}' not supported between instances of '{}' and '{}'",
                        crate::interpreter::error_type_name_str(&left),
                        crate::interpreter::error_type_name_str(&right),
                    )
                } else {
                    err
                }
            });
        }
        // Issue #1204/#1939: extract container/str/bytes subclass backings so
        // `L([1]) < L([2])` compares the backing lists. Keep originals if
        // comparing incompatible types so subclass names stay intact.
        let orig_left = left;
        let orig_right = right;
        let left = coerce_operand_backing(&orig_left);
        let right = coerce_operand_backing(&orig_right);
        if let Some(sequence_result) = self.richcmp_sequence_binary(&left, &right, op) {
            return sequence_result;
        }
        match compare_values_with_op(&left, &right, op_name) {
            Ok(ordering) => Ok(Value::bool_(cmp(ordering))),
            Err(error) => {
                if !matches!(error.kind(), pyrust_core::PyErrorKind::Named(class, _) if class.as_ref() == "TypeError")
                {
                    return Err(error);
                }
                self.richcmp_sequence_binary(&left, &right, op)
                    .unwrap_or_else(|| {
                        if matches!(orig_left.kind(), ValueKind::PyInstance(_))
                            || matches!(orig_right.kind(), ValueKind::PyInstance(_))
                            || orig_left.is_bool()
                            || orig_right.is_bool()
                        {
                            Err(pyrust_core::type_err!(
                                "'{op_name}' not supported between instances of '{}' and '{}'",
                                crate::interpreter::error_type_name_str(&orig_left),
                                crate::interpreter::error_type_name_str(&orig_right),
                            ))
                        } else {
                            Err(error)
                        }
                    })
            }
        }
    }

    /// Interpreter-aware list/tuple ordering path (issue #2817 / #3014).
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

        Some(self.with_comparison_order_pair(left, right, |interp| {
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

                if interp.values_richcompare_eq(&left_item, &right_item)? {
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
                        return interp.eval_binary(left_item, op, right_item);
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
        }))
    }
}
