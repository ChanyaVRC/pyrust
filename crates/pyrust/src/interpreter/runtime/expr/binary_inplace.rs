impl Interpreter {
    pub(crate) fn try_inplace_op(
        &mut self,
        left: &Value,
        op: BinaryOp,
        right: &Value,
        is_augmented_assign: bool,
    ) -> Result<Option<Value>> {
        // Fast paths for built-in mutable containers: mutate in-place and
        // return the *same* Value (same Rc pointer) so that aliases see the
        // update.  This implements the Python guarantee that `a += b` on a
        // list or set does not rebind aliases.
        //
        // Quick scalar-exit: primitive scalars (Int, Float, Bool, Str, Bytes,
        // BigInt, Complex, None, Ellipsis, Range) cannot have in-place mutation
        // semantics, so return None immediately without dispatching a dunder.
        // This keeps BinOpConst cost near-zero for the common int/float case.
        if matches!(
            left.kind(),
            ValueKind::Int(_)
                | ValueKind::Float(_)
                | ValueKind::Bool(_)
                | ValueKind::Str(_)
                | ValueKind::Bytes(_)
                | ValueKind::BigInt(_)
                | ValueKind::Complex(_, _)
                | ValueKind::None
                | ValueKind::Ellipsis
                | ValueKind::Tuple(_)
                | ValueKind::Range { .. }
                | ValueKind::NotImplemented
        ) {
            return Ok(None);
        }
        // In-place mutation / `__iadd__`-style semantics only apply to a genuine
        // augmented assignment (`a += b`).  A plain binary `+`/`*`/… that the
        // optimizer fused into a const/imm opcode arrives here with
        // `is_augmented_assign == false` (dst != lhs); it must NOT mutate the LHS
        // or extend it.  Bail out so the caller falls through to eval_binary, which
        // applies the correct non-mutating `__add__` semantics (e.g.
        // `list + non-list` raises TypeError instead of extending).  See issue #1874.
        if !is_augmented_assign {
            return Ok(None);
        }
        // mappingproxy is read-only: `mp |= x` is rejected (CPython 3.12), even
        // though `mp | x` produces a merged dict (PEP 584).
        if op == BinaryOp::BitOr && is_mapping_proxy(left) {
            return Err(pyrust_core::type_err!(
                "'|=' is not supported by mappingproxy; use '|' instead"
            ));
        }
        let is_list = matches!(left.kind(), ValueKind::List(_));
        let is_set = matches!(left.kind(), ValueKind::Set(_));
        if is_list {
            match op {
                BinaryOp::Add => {
                    // list += iterable  =>  list.extend(iterable)
                    let items = self.collect_iterable(right)?;
                    left.list_extend(items)?;
                    return Ok(Some(left.clone()));
                }
                BinaryOp::Mul => {
                    // list *= n  =>  repeat in-place
                    let Some(n) = int_repeat_count(right) else {
                        return Ok(None); // fall through to TypeError
                    };
                    list_repeat_in_place(left, n);
                    return Ok(Some(left.clone()));
                }
                _ => {}
            }
        } else if is_set {
            match op {
                BinaryOp::BitOr | BinaryOp::BitAnd | BinaryOp::Sub | BinaryOp::BitXor => {
                    // set |= / &= / -= / ^= require RHS to be a set or frozenset.
                    // If RHS is neither, raise the CPython-format TypeError directly
                    // (the op symbol must include `=` for in-place operators).
                    let rhs_items = match set_items_from_value(right) {
                        Some((items, _)) => items,
                        None => {
                            let op_sym = match op {
                                BinaryOp::BitOr => "|=",
                                BinaryOp::BitAnd => "&=",
                                BinaryOp::Sub => "-=",
                                BinaryOp::BitXor => "^=",
                                _ => unreachable!(),
                            };
                            let lt = value_type_name_str(left);
                            let rt = value_type_name_str(right);
                            return Err(pyrust_core::type_err!(
                                "unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"
                            ));
                        }
                    };
                    if op == BinaryOp::Sub {
                        set_subtract_in_place(self, left, &rhs_items)?;
                        return Ok(Some(left.clone()));
                    }
                    // Fast path: when no user-instance key is involved, raw
                    // `IndexSet` identity comparison is exact (issue #2244).
                    // Most sets are primitive, so keep this allocation-cheap
                    // path with no `__eq__` dispatch, and — critically — without
                    // adding a second full LHS scan that would regress the
                    // primitive `s |= t` hot loop (issue #2244 perf-neutrality).
                    //
                    // Object-detection rules per op, mirroring the eq-aware
                    // membership the existing dict/set machinery already
                    // implements (a *primitive* key never dispatches `__eq__`
                    // against object keys — `set_lookup_in` only scans object
                    // buckets for object/None/nested-tuple probe keys):
                    //
                    //  - `|=` inserts RHS keys into the LHS. Raw insertion of a
                    //    primitive RHS key is exact regardless of the LHS, so we
                    //    only need to dispatch `__eq__` when the *RHS* holds an
                    //    object key. The LHS is never scanned, and the per-key
                    //    object check is folded into the single insert pass (no
                    //    separate full RHS pre-scan) — this is what keeps the
                    //    `s |= t` hot loop scan-neutral vs the pre-#2244 path.
                    //  - `&=` / `^=` test LHS keys against the RHS (and, for
                    //    `^=`, vice versa), so an object key on *either* side
                    //    means raw `contains` would compare by identity and miss
                    //    an `__eq__`-equal element. These ops already iterate the
                    //    LHS during mutation; the object-check is folded into the
                    //    same single `set_with_mut` borrow. `-=` has already
                    //    delegated to the collection-ops owner above.
                    let needs_eq = left
                        .set_with_mut(|lhs| {
                            match op {
                                BinaryOp::BitOr => {
                                    // Fold the object-key check into the single
                                    // insert pass (no separate full RHS pre-scan):
                                    // bail on the first object key.  Primitive
                                    // keys inserted before the bail are exact and
                                    // are re-inserted idempotently by the slow
                                    // path, so a partial in-place insert is safe.
                                    for k in &rhs_items {
                                        if key_contains_object(k) {
                                            return true;
                                        }
                                        lhs.insert(k.clone());
                                    }
                                }
                                BinaryOp::BitAnd => {
                                    if set_has_object_key(lhs) || set_has_object_key(&rhs_items) {
                                        return true;
                                    }
                                    lhs.retain(|k| rhs_items.contains(k));
                                }
                                BinaryOp::Sub => {
                                    unreachable!("set subtraction is owned by collection_ops")
                                }
                                BinaryOp::BitXor => {
                                    if set_has_object_key(lhs) || set_has_object_key(&rhs_items) {
                                        return true;
                                    }
                                    let mut to_add: Vec<PyKey> = Vec::new();
                                    for k in &rhs_items {
                                        if !lhs.contains(k) {
                                            to_add.push(k.clone());
                                        }
                                    }
                                    lhs.retain(|k| !rhs_items.contains(k));
                                    for k in to_add {
                                        lhs.insert(k);
                                    }
                                }
                                _ => unreachable!(),
                            }
                            false
                        })
                        .unwrap_or(false);
                    if !needs_eq {
                        return Ok(Some(left.clone()));
                    }
                    // Slow path (issue #2244): a user instance is present on
                    // either side, so membership and dedup must dispatch user
                    // `__hash__`/`__eq__`.  Running user code can re-enter and
                    // mutate the receiver, so the backing borrow must not be
                    // held across it: snapshot the LHS, compute the result with
                    // the eq-aware helpers (`set_lookup_in`/`set_insert`), then
                    // replace the receiver's contents in place so aliases see
                    // the update.
                    let lhs = left
                        .set_with(|s| s.clone())
                        .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                    let mut out: PySet = PySet::default();
                    match op {
                        BinaryOp::BitOr => {
                            for k in lhs.iter().chain(rhs_items.iter()) {
                                self.set_insert(&mut out, k.clone())?;
                            }
                        }
                        BinaryOp::BitAnd => {
                            for k in lhs.iter() {
                                if self.set_lookup_in(&rhs_items, k)?.is_some() {
                                    self.set_insert(&mut out, k.clone())?;
                                }
                            }
                        }
                        BinaryOp::Sub => {
                            unreachable!("set subtraction is owned by collection_ops")
                        }
                        BinaryOp::BitXor => {
                            for k in lhs.iter() {
                                if self.set_lookup_in(&rhs_items, k)?.is_none() {
                                    self.set_insert(&mut out, k.clone())?;
                                }
                            }
                            for k in rhs_items.iter() {
                                if self.set_lookup_in(&lhs, k)?.is_none() {
                                    self.set_insert(&mut out, k.clone())?;
                                }
                            }
                        }
                        _ => unreachable!(),
                    }
                    left.set_with_mut(|s| *s = out)
                        .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                    return Ok(Some(left.clone()));
                }
                _ => {}
            }
        } else if let Some(data_rc) = pyrust_builtins::bytearray::as_bytearray_rc(left) {
            // bytearray += / bytearray *= — mutate backing Vec in place so
            // that aliases (other variables referencing the same bytearray)
            // also see the change.
            match op {
                BinaryOp::Add => {
                    // The RHS may itself be a bytes/bytearray subclass instance,
                    // so unwrap its backing before extracting the byte slice.
                    let rhs_val = coerce_operand_backing(right);
                    let rhs = if let Some(rhs_data) =
                        pyrust_builtins::bytearray::as_bytearray_snapshot(&rhs_val)
                    {
                        rhs_data
                    } else if let ValueKind::Bytes(rc) = rhs_val.kind() {
                        rc.as_slice().to_vec()
                    } else {
                        let type_name = value_type_name_str(right);
                        return Err(pyrust_core::type_err!(
                            "can't concat {type_name} to bytearray"
                        ));
                    };
                    data_rc.borrow_mut().extend_from_slice(&rhs);
                    return Ok(Some(left.clone()));
                }
                BinaryOp::Mul => {
                    let n = match right.kind() {
                        ValueKind::Int(n) => n,
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::BigInt(_) => {
                            return Err(pyrust_core::overflow_err!(
                                "cannot fit 'int' into an index-sized integer"
                            ));
                        }
                        _ => {
                            let type_name = value_type_name_str(right);
                            return Err(pyrust_core::type_err!(
                                "can't multiply sequence by non-int of type '{type_name}'"
                            ));
                        }
                    };
                    let mut data = data_rc.borrow_mut();
                    if n <= 0 {
                        data.clear();
                    } else {
                        let orig = data.clone();
                        for _ in 1..n {
                            data.extend_from_slice(&orig);
                        }
                    }
                    return Ok(Some(left.clone()));
                }
                _ => {}
            }
        } else if matches!(left.kind(), ValueKind::Dict(_)) && op == BinaryOp::BitOr {
            // PEP 584: dict |= other → in-place update.
            // Plain dict: skip dunder path, go directly to update().
            // For binary | (not augmented assign), only dict-compatible RHS is
            // valid; fall through to eval_binary for the TypeError with correct
            // operand names.  For |= the full dict.update() semantics apply
            // (accepts dicts and iterables of pairs).
            if is_augmented_assign || dict_entries_from_value(right).is_some() {
                // #1914: `|=` must dedup `PyKey::Object` keys via user `__eq__`.
                // `dict_entries_from_value` handles plain dicts and dict
                // subclasses; iterables-of-pairs fall through to update() below.
                if let Some(entries) = dict_entries_from_value(right) {
                    self.dict_extend_value_dedup(left, entries)?;
                    return Ok(Some(left.clone()));
                }
                let empty_kw = PyDict::default();
                let update_result =
                    self.call_dict_method("update", left.clone(), vec![right.clone()], &empty_kw);
                update_result?;
                return Ok(Some(left.clone()));
            }
        }

        let dunder = match op {
            BinaryOp::Add => "__iadd__",
            BinaryOp::Sub => "__isub__",
            BinaryOp::Mul => "__imul__",
            BinaryOp::MatMul => "__imatmul__",
            BinaryOp::Div => "__itruediv__",
            BinaryOp::FloorDiv => "__ifloordiv__",
            BinaryOp::Mod => "__imod__",
            BinaryOp::Pow => "__ipow__",
            BinaryOp::BitAnd => "__iand__",
            BinaryOp::BitOr => "__ior__",
            BinaryOp::BitXor => "__ixor__",
            BinaryOp::LShift => "__ilshift__",
            BinaryOp::RShift => "__irshift__",
            _ => return Ok(None),
        };
        let result = self.try_call_binary_method(left, dunder, right.clone())?;
        if let Some(ref v) = result
            && !is_not_implemented(v)
        {
            return Ok(result);
        }
        // PEP 584 fallback: PyInstance dict subclass |= other when no `__ior__`
        // was found.  Call update() on the backing dict (so dict_with_mut works)
        // and return `left` to preserve object identity.
        // For binary | (not augmented assign), only dict-compatible RHS is valid;
        // fall through to eval_binary which uses the subclass type name correctly
        // (e.g. 'D' rather than 'dict') in the unsupported-operand TypeError.
        //
        // `result.is_none()` gates this to the no-override case only: if a
        // user-defined `__ior__` *exists* and returned `NotImplemented`, CPython
        // falls back to plain binary `|` (yielding a plain `dict`, dropping the
        // subclass type), so we must let it fall through to `eval_binary` rather
        // than mutate the backing dict in place and return the subclass here (#2639).
        if result.is_none()
            && op == BinaryOp::BitOr
            && let Some(backing) = builtin_data_backing(left)
            && matches!(backing.kind(), ValueKind::Dict(_))
            && (is_augmented_assign || dict_entries_from_value(right).is_some())
        {
            // #1914: dedup `PyKey::Object` keys via user `__eq__`.
            if let Some(entries) = dict_entries_from_value(right) {
                self.dict_extend_value_dedup(&backing, entries)?;
                return Ok(Some(left.clone()));
            }
            let empty_kw = PyDict::default();
            pyrust_builtins::dict::call("update", &backing, vec![right.clone()], &empty_kw)?;
            return Ok(Some(left.clone()));
        }
        // Issue #1006 + #1007: PyInstance set subclass |= / &= / -= / ^= — when
        // no user-defined __ior__ / __iand__ / __isub__ / __ixor__ was found,
        // fall back to mutating the backing set in-place and returning `left`
        // so the subclass type is preserved (matching CPython's set.__ior__ etc.
        // which mutate self and return self).
        //
        // Also covers frozenset (plain BuiltinObject) and set subclass TypeError:
        // when LHS is set-like but RHS is not, raise the CPython-format TypeError
        // with the `|=:` / `&=:` / etc. symbol directly (returning None would
        // fall through to eval_binary which uses the non-`=` symbol).
        if matches!(
            op,
            BinaryOp::BitOr | BinaryOp::BitAnd | BinaryOp::Sub | BinaryOp::BitXor
        ) {
            // `result.is_none()` gates the subclass-preserving in-place arm to the
            // no-override case only: if a user-defined `__ior__` / `__iand__` /
            // `__isub__` / `__ixor__` *exists* and returned `NotImplemented`,
            // CPython falls back to plain binary `|` / `&` / `-` / `^` (yielding a
            // plain `set`, dropping the subclass type), so we let it fall through
            // to `eval_binary` rather than mutate the backing set in place (#2639).
            // The frozenset `else` branch below always runs (its `result` is always
            // `None` since `try_call_binary_method` no-ops on non-PyInstance left).
            if left.as_py_instance_rc().is_some() {
                if result.is_none()
                    && let Some(backing) = builtin_data_backing(left)
                    && matches!(backing.kind(), ValueKind::Set(_))
                {
                    let op_sym = match op {
                        BinaryOp::BitOr => "|=",
                        BinaryOp::BitAnd => "&=",
                        BinaryOp::Sub => "-=",
                        BinaryOp::BitXor => "^=",
                        _ => unreachable!(),
                    };
                    let rhs_items = match set_items_from_value(right) {
                        Some((items, _)) => items,
                        None => {
                            let lt = value_type_name_str(left);
                            let rt = value_type_name_str(right);
                            return Err(pyrust_core::type_err!(
                                "unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"
                            ));
                        }
                    };
                    if op == BinaryOp::Sub {
                        set_subtract_in_place(self, &backing, &rhs_items)?;
                        return Ok(Some(left.clone()));
                    }
                    backing.set_with_mut(|lhs| match op {
                        BinaryOp::BitOr => {
                            for k in &rhs_items {
                                lhs.insert(k.clone());
                            }
                        }
                        BinaryOp::BitAnd => {
                            lhs.retain(|k| rhs_items.contains(k));
                        }
                        BinaryOp::Sub => {
                            unreachable!("set subtraction is owned by collection_ops")
                        }
                        BinaryOp::BitXor => {
                            let mut to_add: Vec<PyKey> = Vec::new();
                            for k in &rhs_items {
                                if !lhs.contains(k) {
                                    to_add.push(k.clone());
                                }
                            }
                            lhs.retain(|k| !rhs_items.contains(k));
                            for k in to_add {
                                lhs.insert(k);
                            }
                        }
                        _ => unreachable!(),
                    });
                    return Ok(Some(left.clone()));
                }
            } else {
                // Plain frozenset (BuiltinObject) — not caught by the is_set
                // branch above (which only matches ValueKind::Set).
                if set_items_from_value(left).is_some() && set_items_from_value(right).is_none() {
                    let op_sym = match op {
                        BinaryOp::BitOr => "|=",
                        BinaryOp::BitAnd => "&=",
                        BinaryOp::Sub => "-=",
                        BinaryOp::BitXor => "^=",
                        _ => unreachable!(),
                    };
                    let lt = value_type_name_str(left);
                    let rt = value_type_name_str(right);
                    return Err(pyrust_core::type_err!(
                        "unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"
                    ));
                }
            }
        }
        // Issue #2986: PyInstance list subclass `+=` / `*=` — when no
        // user-defined `__iadd__` / `__imul__` was found (the inherited
        // `list.__i*__` sentinels are skipped in `try_call_binary_method`),
        // mutate the backing list in place and return `left`, exactly as the
        // plain-list arm above does.  Without this the operator fell through to
        // `__add__` / `__mul__`, which build a *new* plain list: `p = LSub([1]);
        // q = p; p += [9]` left `q == [1]` and `p is q` False, silently breaking
        // every alias of the receiver.
        //
        // `result.is_none()` gates this to the no-override case only, for the
        // same reason as the set / dict / bytearray arms: a user `__iadd__` that
        // returned `NotImplemented` must fall back to plain binary `+` (yielding
        // a plain `list` and dropping the subclass type), not mutate self (#2639).
        if result.is_none()
            && matches!(op, BinaryOp::Add | BinaryOp::Mul)
            && let Some(backing) = builtin_data_backing(left)
            && matches!(backing.kind(), ValueKind::List(_))
            && self.list_backing_inplace_op(&backing, op, right)?
        {
            return Ok(Some(left.clone()));
        }
        // Issue #2386: PyInstance bytearray subclass `+=` / `*=` — when no
        // user-defined `__iadd__` / `__imul__` was found (the inherited
        // `bytearray.__i*__` sentinels are skipped in `try_call_binary_method`),
        // mutate the backing bytearray in place and return `left` so the subclass
        // type and object identity are preserved (matching CPython's
        // `bytearray.__iadd__` / `__imul__`, which mutate self and return self).
        //
        // `result.is_none()` gates this to the no-override case only: if a
        // user-defined `__iadd__` / `__imul__` *exists* and returned
        // `NotImplemented`, CPython falls back to plain binary `+` / `*` (yielding
        // a plain `bytearray`, dropping the subclass type), so we must let it fall
        // through to `eval_binary_aug` rather than mutate self in place here.
        if result.is_none()
            && matches!(op, BinaryOp::Add | BinaryOp::Mul)
            && let Some(backing) = builtin_data_backing(left)
            && let Some(data_rc) = pyrust_builtins::bytearray::as_bytearray_rc(&backing)
        {
            match op {
                BinaryOp::Add => {
                    // The RHS may itself be a bytes/bytearray subclass instance,
                    // so unwrap its backing before extracting the byte slice.
                    let rhs_val = coerce_operand_backing(right);
                    let rhs = if let Some(rhs_data) =
                        pyrust_builtins::bytearray::as_bytearray_snapshot(&rhs_val)
                    {
                        rhs_data
                    } else if let ValueKind::Bytes(rc) = rhs_val.kind() {
                        rc.as_slice().to_vec()
                    } else {
                        // CPython names the LHS by its actual (subclass) type.
                        let lhs_name = value_type_name_str(left);
                        let type_name = value_type_name_str(right);
                        return Err(pyrust_core::type_err!(
                            "can't concat {type_name} to {lhs_name}"
                        ));
                    };
                    data_rc.borrow_mut().extend_from_slice(&rhs);
                    return Ok(Some(left.clone()));
                }
                BinaryOp::Mul => {
                    let n = match right.kind() {
                        ValueKind::Int(n) => n,
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::BigInt(_) => {
                            return Err(pyrust_core::overflow_err!(
                                "cannot fit 'int' into an index-sized integer"
                            ));
                        }
                        _ => {
                            let type_name = value_type_name_str(right);
                            return Err(pyrust_core::type_err!(
                                "can't multiply sequence by non-int of type '{type_name}'"
                            ));
                        }
                    };
                    let mut data = data_rc.borrow_mut();
                    if n <= 0 {
                        data.clear();
                    } else {
                        let orig = data.clone();
                        for _ in 1..n {
                            data.extend_from_slice(&orig);
                        }
                    }
                    return Ok(Some(left.clone()));
                }
                _ => unreachable!(),
            }
        }
        Ok(None)
    }

    /// Apply `+=` / `*=` to the backing list of a `list` subclass instance,
    /// mutating that storage so every alias of the instance observes the update
    /// and the subclass type survives (issue #2986).
    ///
    /// This is the inherited `list.__iadd__` / `list.__imul__`, so it does the
    /// same storage work as the plain-list arm of [`Self::try_inplace_op`] —
    /// including reading the right operand to completion *before* the receiver
    /// is touched, which is what makes `p += p` terminate.
    ///
    /// Returns `Ok(true)` when the operator was applied, `Ok(false)` when it
    /// does not apply and the caller must fall through to the binary path (a
    /// `*=` count with no `__index__`, which the right operand's `__rmul__` may
    /// still handle, or one too large for an index-sized integer).
    ///
    /// `#[inline(never)]`: `try_inplace_op` is long and its codegen is layout-
    /// sensitive (the plain `list` / `set` / `dict` / `bytearray` in-place loops
    /// all run through it), so this subclass-only tail stays out of line.
    #[inline(never)]
    fn list_backing_inplace_op(
        &mut self,
        backing: &Value,
        op: BinaryOp,
        right: &Value,
    ) -> Result<bool> {
        match op {
            BinaryOp::Add => {
                let items = self.collect_iterable(right)?;
                backing.list_extend(items)?;
                Ok(true)
            }
            BinaryOp::Mul => {
                // Off the hot path, so the count may take the full `__index__`
                // protocol — CPython's sequence repetition does. A count with
                // no `__index__` at all falls through rather than raising, so
                // the right operand's `__rmul__` still gets its turn.
                let n = match int_repeat_count(right) {
                    Some(n) => n,
                    None => {
                        let Some(count) = self.try_value_to_index(right)? else {
                            return Ok(false);
                        };
                        // A `BigInt` count cannot fit an index-sized integer;
                        // `eval_binary` raises that `OverflowError` with the
                        // operand names CPython uses, so leave it to do so.
                        let Some(n) = int_repeat_count(&count) else {
                            return Ok(false);
                        };
                        n
                    }
                };
                list_repeat_in_place(backing, n);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// The already-integral repetition count in `v`, if it has one.
///
/// A `BigInt` is deliberately excluded: it cannot fit an index-sized integer,
/// so sequence repetition rejects it rather than truncating.
#[inline]
fn int_repeat_count(v: &Value) -> Option<i64> {
    match v.kind() {
        ValueKind::Int(n) => Some(n),
        ValueKind::Bool(b) => Some(b as i64),
        _ => None,
    }
}

/// Repeat a `ValueKind::List`'s elements in place, the storage half of
/// `list.__imul__`.  A count of zero or less empties the list.
#[inline]
fn list_repeat_in_place(target: &Value, n: i64) {
    target.list_with_mut(|items| {
        if n <= 0 {
            items.clear();
        } else {
            let orig = items.clone();
            for _ in 1..n {
                items.extend_from_slice(&orig);
            }
        }
    });
}
