impl Interpreter {
    /// Compare two values via `__eq__`, used by the dict/set runtime when
    /// resolving `PyKey::Object` collisions and by `BinaryOp::Eq`/`Ne`'s
    /// container fall-through path (issue #436).
    ///
    /// Dispatch order, structured to keep the flat-primitive hot path
    /// allocation-free:
    ///
    /// 1. Same-kind sequence pair (`List`/`List` or `Tuple`/`Tuple`):
    ///    `try_seq_fast_eq` walks the borrow pairwise and resolves any
    ///    pair that doesn't transitively need user dispatch via
    ///    `Value::eq`.  This avoids the double-walk an upfront
    ///    `a == b` would cause and matches pre-#436 perf for primitive
    ///    sequences.  When a pair could need dispatch (`PyInstance` or
    ///    nested container), snapshot both sides and recurse.
    /// 2. Primitive / identity fast path: `a == b` for the non-sequence
    ///    cases (`Int`/`Float`/`Bool`/`Str`/`Bytes`/`Complex`/`None`
    ///    and identity-equal `Dict`/`Set`).
    /// 3. Same-kind `Dict`/`Set`: snapshot keys and dispatch via
    ///    `dict_lookup`/`set_lookup`, which already route
    ///    `PyKey::Object` through user `__hash__`/`__eq__` (issue #368).
    /// 4. Both sides are `frozenset` (`BuiltinObject`): same membership
    ///    check as Set but via `set_lookup_in`, so `PyKey::Object`
    ///    elements (user-class instances) dispatch `__eq__` correctly.
    /// 5. `PyInstance` on either side: `try_dunder_binary` for
    ///    `__eq__`/reflected `__eq__`.
    ///
    /// Cycle detection mirrors `Value::eq`'s `EqGuard`: a recursive call
    /// for the same `(value_id(a), value_id(b))` pair returns true (the
    /// recursion bottoms out as "we've already proven the prefix equal"),
    /// so `a.append(a); b.append(b); a == b` doesn't blow the stack.
    pub(crate) fn values_user_eq(&mut self, a: &Value, b: &Value) -> Result<bool> {
        // Same-kind sequence containers come first.  For `List`/`Tuple`
        // pairs an upfront `Value::eq` would double-walk: `Vec::eq`
        // already iterates element-wise, and the recursion below would
        // repeat the walk.  Going straight to `try_seq_fast_eq`
        // resolves flat primitive sequences (`[1,2,3] == [1,2,4]`) in
        // a single borrow-only pass with no allocation — matching
        // pre-#436 perf.  Mixed-kind pairs (e.g. list vs tuple) fall
        // through to the primitive/identity fast path below.
        let needs_seq_dispatch = match (a.kind(), b.kind()) {
            (ValueKind::List(la), ValueKind::List(lb)) => {
                if la.len() != lb.len() {
                    return Ok(false);
                }
                match try_seq_fast_eq(&la, &lb) {
                    SeqFast::Resolved(v) => return Ok(v),
                    SeqFast::NeedsDispatch => true,
                }
            }
            (ValueKind::Tuple(la), ValueKind::Tuple(lb)) => {
                if la.len() != lb.len() {
                    return Ok(false);
                }
                match try_seq_fast_eq(la, lb) {
                    SeqFast::Resolved(v) => return Ok(v),
                    SeqFast::NeedsDispatch => true,
                }
            }
            _ => false,
        };
        if needs_seq_dispatch {
            // Slow path: snapshot both sides to drop the borrow before
            // recursing into user code, then walk element-wise through
            // `values_richcompare_eq` so aliases compare equal by identity
            // before iterator/instance elements dispatch `__eq__`.  Element
            // clones are cheap (Rc/NaN-box copy).
            let list_pair = a.is_list() && b.is_list();
            let (av, bv): (Vec<Value>, Vec<Value>) = match (a.kind(), b.kind()) {
                (ValueKind::List(la), ValueKind::List(lb)) => {
                    (la.iter().cloned().collect(), lb.iter().cloned().collect())
                }
                (ValueKind::Tuple(la), ValueKind::Tuple(lb)) => (la.to_vec(), lb.to_vec()),
                _ => unreachable!("needs_seq_dispatch implies a sequence pair"),
            };
            if self.eq_cycle_enter(a, b) {
                // Already comparing this pair further up the stack —
                // treat as equal to terminate the recursion (matching
                // `Value::eq`'s `EqGuard` policy).
                return Ok(true);
            }
            let mut result = (|| -> Result<bool> {
                for (x, y) in av.iter().zip(bv.iter()) {
                    if !self.values_richcompare_eq(x, y)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            })();
            // CPython's list comparison notices a size change caused by an
            // element __eq__ even after the snapshotted prefix compared equal.
            if list_pair
                && result.as_ref().is_ok_and(|equal| *equal)
                && (a.list_len() != Some(av.len()) || b.list_len() != Some(bv.len()))
            {
                result = Ok(false);
            }
            self.eq_cycle_exit(a, b);
            return result;
        }

        // Primitive / identity fast path: `Value::eq` handles
        // Int/Float/Bool/Str/Bytes/Complex/None and identity-equal
        // Dict/Set without dunder dispatch.  (List/Tuple were already
        // handled above to avoid the double-walk.)
        if a == b {
            return Ok(true);
        }

        // A slice is a hashable aggregate on Python 3.12.  `SliceOps::eq`
        // handles primitive components without interpreter access, but its raw
        // `Value` comparison cannot dispatch a user instance nested in
        // `slice(start, stop, step)`.  Compare a cloned field snapshot in tuple
        // order so object-key lookup/dedup propagates `__eq__` and exceptions.
        //
        // Slice fields are immutable after construction, so unlike List/Dict/
        // Set this recursion cannot cycle and needs no `eq_cycle_enter` guard.
        if let (Some(a_fields), Some(b_fields)) = (
            pyrust_builtins::slice::slice_fields(a),
            pyrust_builtins::slice::slice_fields(b),
        ) {
            for (x, y) in [
                (&a_fields.0, &b_fields.0),
                (&a_fields.1, &b_fields.1),
                (&a_fields.2, &b_fields.2),
            ] {
                if !self.values_user_eq(x, y)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }

        enum ContainerSnapshot {
            Dict {
                entries: Vec<(PyKey, Value)>,
                expected_len: usize,
            },
            Set {
                keys: Vec<PyKey>,
                expected_len: usize,
            },
            Other,
        }
        // Build the owned side of a potentially dispatching comparison inside
        // this expression. The ValueKind Ref guards are both dropped before
        // `dict_lookup` / `set_lookup` can invoke user `__eq__`.
        let container_snapshot = match (a.kind(), b.kind()) {
            (ValueKind::Dict(da), ValueKind::Dict(db)) => {
                if da.len() != db.len() {
                    return Ok(false);
                }
                ContainerSnapshot::Dict {
                    entries: da.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    expected_len: da.len(),
                }
            }
            (ValueKind::Set(sa), ValueKind::Set(sb)) => {
                if sa.len() != sb.len() {
                    return Ok(false);
                }
                ContainerSnapshot::Set {
                    keys: sa.iter().cloned().collect(),
                    expected_len: sa.len(),
                }
            }
            _ => ContainerSnapshot::Other,
        };

        match container_snapshot {
            ContainerSnapshot::Dict {
                entries,
                expected_len,
            } => {
                if self.eq_cycle_enter(a, b) {
                    return Ok(true);
                }
                // Snapshot (PyKey, Value) pairs from `a` so user `__eq__`
                // (run while looking up in `b`) can't invalidate the dict
                // borrow.  We pass the snapshotted `PyKey` straight to
                // `dict_lookup` so `__hash__` / `__eq__` dispatch on
                // `PyKey::Object` keys still works (issue #368).
                let mut result = (|| -> Result<bool> {
                    for (pk, v_lhs) in entries {
                        match self.dict_lookup(b, &pk)? {
                            Some((_, v_rhs)) => {
                                if v_lhs == v_rhs || v_lhs.is_identical_nan(&v_rhs) {
                                    // same object or NaN-identity: treat as equal
                                    // (mirrors CPython PyObject_RichCompareBool)
                                    continue;
                                }
                                if !self.values_user_eq(&v_lhs, &v_rhs)? {
                                    return Ok(false);
                                }
                            }
                            None => return Ok(false),
                        }
                    }
                    Ok(true)
                })();
                if result.as_ref().is_ok_and(|equal| *equal)
                    && (a.dict_len() != Some(expected_len) || b.dict_len() != Some(expected_len))
                {
                    result = Ok(false);
                }
                self.eq_cycle_exit(a, b);
                return result;
            }
            ContainerSnapshot::Set { keys, expected_len } => {
                if self.eq_cycle_enter(a, b) {
                    return Ok(true);
                }
                let mut result = (|| -> Result<bool> {
                    for pk in keys {
                        if self.set_lookup(b, &pk)?.is_none() {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                })();
                if result.as_ref().is_ok_and(|equal| *equal)
                    && (a.set_len() != Some(expected_len) || b.set_len() != Some(expected_len))
                {
                    result = Ok(false);
                }
                self.eq_cycle_exit(a, b);
                return result;
            }
            ContainerSnapshot::Other => {}
        }

        // Frozenset — same membership logic as Set above, but the items
        // live inside a BuiltinObject.  `set_lookup_in` handles
        // `PyKey::Object` elements by dispatching user `__eq__`, so
        // `frozenset({a}) == frozenset({b})` works correctly when
        // `a.__eq__(b)` returns True.  Non-frozenset BuiltinObject pairs
        // fall through to `try_dunder_binary` (the PyInstance path); if
        // that also yields nothing, we return false — identical to
        // `Value::eq`'s behaviour for unrecognised BuiltinObject pairs.
        if let (Some(lhs_rc), Some(rhs_rc)) = (
            pyrust_builtins::frozenset::as_items(a),
            pyrust_builtins::frozenset::as_items(b),
        ) {
            if lhs_rc.len() != rhs_rc.len() {
                return Ok(false);
            }
            if self.eq_cycle_enter(a, b) {
                return Ok(true);
            }
            let lhs_keys: Vec<PyKey> = lhs_rc.iter().cloned().collect();
            let rhs_snap: PySet = rhs_rc.iter().cloned().collect();
            let result = (|| -> Result<bool> {
                for pk in lhs_keys {
                    if self.set_lookup_in(&rhs_snap, &pk)?.is_none() {
                        return Ok(false);
                    }
                }
                Ok(true)
            })();
            self.eq_cycle_exit(a, b);
            return result;
        }

        // Issue #1891: the set-like dict views `dict_keys` / `dict_items`
        // compare as sets against any other set-like operand (`set`,
        // `frozenset`, or another set-like view).  CPython's view `__eq__`
        // returns `False` (not TypeError) when the other operand is *not*
        // set-like — including `dict_values`, lists, and dicts.  `dict_items`
        // with an unhashable value raises `TypeError: unhashable type: …`,
        // which `coerce_set_operand` surfaces.
        if is_setlike_view(a) || is_setlike_view(b) {
            let a_set = self.coerce_set_operand(a);
            let b_set = self.coerce_set_operand(b);
            match (a_set, b_set) {
                (Some(a_res), Some(b_res)) => {
                    let (sa, _) = a_res?;
                    let (sb, _) = b_res?;
                    if sa.len() != sb.len() {
                        return Ok(false);
                    }
                    let needs_eq = set_has_object_key(&sa) || set_has_object_key(&sb);
                    if !needs_eq {
                        return Ok(sa.iter().all(|k| sb.contains(k)));
                    }
                    for k in sa.iter() {
                        if self.set_lookup_in(&sb, k)?.is_none() {
                            return Ok(false);
                        }
                    }
                    return Ok(true);
                }
                // A view vs a non-set-like operand: not equal (CPython returns
                // False without building the set, so an unhashable `dict_items`
                // value does *not* raise here — `items == [..]` is just False).
                (Some(_), None) | (None, Some(_)) => return Ok(false),
                (None, None) => unreachable!("is_setlike_view implies a set-like operand"),
            }
        }

        // Issue #1939: a container subclass (list/tuple/dict/set/frozenset
        // subclass) with no user `__eq__` override inherits the base type's
        // equality, so `L([1,2]) == [1,2]`, `D({1:'a'}) == {1:'a'}`, and
        // `St({1,2}) == {1,2}` compare by backing value.  The concrete-
        // container fast paths above have already returned, so this only runs
        // when a `PyInstance` operand reaches the bottom — no cost on the hot
        // `[1,2,3] == [1,2,3]` path.  Coerce the container backing(s) and
        // recurse so the List/Tuple/Dict/Set/Frozenset arms above run; a user
        // `__eq__` override is excluded by `coerce_subclass_backing`.
        let a_cont = coerce_container_backing_for_eq(a);
        let b_cont = coerce_container_backing_for_eq(b);
        if a_cont.is_some() || b_cont.is_some() {
            let a_c = a_cont.unwrap_or_else(|| a.clone());
            let b_c = b_cont.unwrap_or_else(|| b.clone());
            return self.values_user_eq(&a_c, &b_c);
        }

        // PyInstance (either side) — dispatch `__eq__`/reflected
        // `__eq__`.  This is the original `values_user_eq` body.
        if let Some(r) = self.try_dunder_binary(a, b, "__eq__", "__eq__") {
            let result = r?;
            return self.truthy_value(&result);
        }
        // Issue #1204: if a PyInstance has a scalar primitive backing
        // (e.g. MyInt subclass) and no user __eq__ was found, compare the
        // backing values so `MyInt(5) == 5` returns True.
        let a_cmp = coerce_numeric(a);
        let b_cmp = coerce_numeric(b);
        if !matches!(a_cmp.kind(), ValueKind::PyInstance(_))
            || !matches!(b_cmp.kind(), ValueKind::PyInstance(_))
        {
            // At least one side was coerced out of PyInstance.
            return Ok(a_cmp == b_cmp);
        }
        Ok(false)
    }

    /// CPython's `PyObject_RichCompareBool(a, b, Py_EQ)`: identity implies
    /// equality, so an element always finds *itself* during a container search
    /// even when its `__eq__` says otherwise.
    ///
    /// The identity test is `values_are_identical` — the same predicate `is`
    /// uses — not just `Value::is_identical_nan`.  NaN is the observable
    /// *primitive* case (`nan == nan` is False, yet `n in [n]` is True, #2344 /
    /// #2535 / #2911), but it is not the only one: values whose `PartialEq`
    /// reports two aliases of one object as unequal — generators, the native
    /// iterators (`iter(x)`, `enumerate`, `zip`, `map`, `filter`, `reversed`),
    /// `dict_values` — need the same short-circuit, as does any instance whose
    /// `__eq__` answers False (or raises) for itself.
    ///
    /// Use this for every callback-capable element-wise container scan.  The
    /// list/tuple scans keep scalar identity/equality inline for their
    /// borrow-only primitive fast path, then delegate only the slow branch
    /// here.
    pub(crate) fn values_richcompare_eq(&mut self, a: &Value, b: &Value) -> Result<bool> {
        if a.cannot_user_eq() {
            // Scalar NaN-box tag (`Float`/`None`/`Bool`/`Int`/`Str`): NaN is the
            // only value of these whose identity is not already implied by `==`,
            // so the inlined bit test is the whole rule and the general
            // (kind-matching) predicate would be dead weight on int/str scans.
            if a.is_identical_nan(b) {
                return Ok(true);
            }
        } else if values_are_identical(a, b) {
            return Ok(true);
        }
        self.values_user_eq(a, b)
    }

    /// Enter equality recursion for the `(value_id(a), value_id(b))`
    /// pair.  Returns `true` when a cycle is detected (the caller should
    /// short-circuit to "equal" without pushing); returns `false`
    /// otherwise after pushing the pair onto the recursion stack.  Each
    /// `false` return must be matched by an `eq_cycle_exit` call.
    ///
    /// Primitives (no `value_id`) can't form cycles, so we return
    /// `false` without recording anything — the missing push is paired
    /// with a no-op `eq_cycle_exit`.
    fn eq_cycle_enter(&mut self, a: &Value, b: &Value) -> bool {
        let (Some(a_id), Some(b_id)) = (a.value_id(), b.value_id()) else {
            return false;
        };
        let pair = (a_id, b_id);
        if self.eq_in_progress.contains(&pair) {
            return true;
        }
        self.eq_in_progress.push(pair);
        false
    }

    /// Pop the matching pair from the recursion stack.  No-op when the
    /// pair wasn't pushed (one operand was a primitive without
    /// `value_id`).
    fn eq_cycle_exit(&mut self, a: &Value, b: &Value) {
        let (Some(a_id), Some(b_id)) = (a.value_id(), b.value_id()) else {
            return;
        };
        if let Some(pos) = self.eq_in_progress.iter().rposition(|p| *p == (a_id, b_id)) {
            self.eq_in_progress.remove(pos);
        }
    }
}
