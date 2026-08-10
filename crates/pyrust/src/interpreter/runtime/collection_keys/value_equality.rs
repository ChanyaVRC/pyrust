/// CPython 3.12's platform C-recursion ceiling for native comparisons.
#[cfg(target_os = "windows")]
const CPYTHON_C_RECURSION_LIMIT: usize = 3000;
#[cfg(all(not(target_os = "windows"), target_arch = "s390x"))]
const CPYTHON_C_RECURSION_LIMIT: usize = 800;
#[cfg(all(
    not(target_os = "windows"),
    not(target_arch = "s390x"),
    target_os = "wasi"
))]
const CPYTHON_C_RECURSION_LIMIT: usize = 500;
#[cfg(all(
    not(target_os = "windows"),
    not(target_arch = "s390x"),
    not(target_os = "wasi")
))]
const CPYTHON_C_RECURSION_LIMIT: usize = 10_000;

const MAX_ACTIVE_COMPARISON_COST: usize = CPYTHON_C_RECURSION_LIMIT - 2;
const MAX_COMPARISON_COST_BEFORE_USER_EQ: usize = CPYTHON_C_RECURSION_LIMIT - 4;
const COMPARISON_STACK_GROW_START_COST: usize = 64;
const COMPARISON_STACK_RED_ZONE: usize = 2 * 1024 * 1024;
const COMPARISON_STACK_SEGMENT: usize = 32 * 1024 * 1024;

fn comparison_recursion_error() -> PyError {
    PyError::named(
        "RecursionError",
        "maximum recursion depth exceeded in comparison".to_string(),
    )
}

fn sequence_item_at(value: &Value, index: usize) -> Option<Value> {
    match value.kind() {
        ValueKind::List(items) => items.get(index).cloned(),
        ValueKind::Tuple(items) => items.get(index).cloned(),
        _ => None,
    }
}

impl Interpreter {
    fn dict_items_backing_value(view: &Value) -> Option<Value> {
        if !matches!(
            pyrust_builtins::dict_views::view_kind(view),
            Some(pyrust_builtins::dict_views::DictViewKind::Items)
        ) {
            return None;
        }
        pyrust_builtins::dict_views::as_dict_rc(view).map(Value::dict_shared)
    }

    /// Capture the provider-owned mutation watch only for an ordered mapping
    /// view. Plain dictionary views continue to use `LiveKeyCursor` alone.
    fn ordered_dict_view_comparison_watch(view: &Value) -> Option<OrderedIterationWatch> {
        pyrust_builtins::ordered_mapping::view_policy(view)
            .is_some()
            .then(|| ordered_iteration_watch(view))
    }

    /// Advance a lazy set-like comparison source after applying the stronger
    /// OrderedDict iterator policy. The guard runs on the terminal advance as
    /// well, so a callback from the last yielded entry cannot be hidden by
    /// ordinary cursor exhaustion.
    fn advance_setlike_comparison_cursor(
        source: &Value,
        cursor: &mut LiveKeyCursor,
        recorded_len: usize,
        ordered_watch: Option<&OrderedIterationWatch>,
    ) -> Result<Option<LiveDictViewItem>> {
        if let Some(watch) = ordered_watch {
            let relinked = watch.relinked();
            let size_changed = set_comparison_len(source) != Some(recorded_len);
            if relinked || size_changed {
                let outcome = ordered_mapping_guard_outcome(source, recorded_len, watch);
                return Err(PyError::Runtime(outcome.message.to_string()));
            }
        }
        advance_live_key_cursor(source, cursor)
    }

    /// Compare dictionary-item membership without materialising `(key, value)`
    /// tuples as set keys. The right mapping is the containment target, so its
    /// stored key and value own the first equality call.
    fn dict_mapping_items_are_subset(
        &mut self,
        subset_view: &Value,
        subset: &Value,
        superset: &Value,
    ) -> Result<bool> {
        let expected_len = subset
            .dict_len()
            .ok_or_else(|| PyError::Runtime("internal: expected dict backing".to_string()))?;
        let superset_len = superset
            .dict_len()
            .ok_or_else(|| PyError::Runtime("internal: expected dict backing".to_string()))?;
        if expected_len > superset_len {
            return Ok(false);
        }
        // Values may recursively contain their own item views. Record the
        // backing pair in stored-value/probe-value order so recursion reaches
        // the ordinary equality guard instead of growing without bound.
        if self.eq_cycle_enter(superset, subset) {
            return Ok(true);
        }

        let result = (|| -> Result<bool> {
            let ordered_watch = Self::ordered_dict_view_comparison_watch(subset_view);
            let mut cursor = LiveKeyCursor::dict(
                subset_view,
                pyrust_builtins::dict_views::DictViewKind::Items.live_cursor_code(),
                expected_len,
            );
            while let Some(item) = Self::advance_setlike_comparison_cursor(
                subset_view,
                &mut cursor,
                expected_len,
                ordered_watch.as_ref(),
            )? {
                let LiveDictViewItem::Pair(probe_value, lhs_value) = item else {
                    unreachable!("dict-items cursor must yield key/value pairs")
                };
                // Iterating an item view produces a fresh tuple and the target
                // mapping hashes that tuple's key again. Converting the live
                // key value preserves user `__hash__` calls and exceptions.
                let probe_key = self.value_to_pykey(&probe_value)?;
                let Some((_, rhs_value)) = self.dict_lookup(superset, &probe_key)? else {
                    return Ok(false);
                };
                if !self.values_richcompare_eq(&rhs_value, &lhs_value)? {
                    return Ok(false);
                }
            }
            Ok(true)
        })();
        self.eq_cycle_exit(superset, subset);
        result
    }

    pub(crate) fn dict_items_views_are_equal(
        &mut self,
        left: &Value,
        right: &Value,
    ) -> Option<Result<bool>> {
        let left_backing = Self::dict_items_backing_value(left)?;
        let right_backing = Self::dict_items_backing_value(right)?;
        if left_backing.dict_len() != right_backing.dict_len() {
            return Some(Ok(false));
        }
        Some(self.dict_mapping_items_are_subset(left, &left_backing, &right_backing))
    }

    pub(crate) fn dict_items_view_is_subset(
        &mut self,
        subset: &Value,
        superset: &Value,
    ) -> Option<Result<bool>> {
        let subset_backing = Self::dict_items_backing_value(subset)?;
        let superset_backing = Self::dict_items_backing_value(superset)?;
        Some(self.dict_mapping_items_are_subset(subset, &subset_backing, &superset_backing))
    }

    fn dict_mapping_items_are_subset_of_setlike(
        &mut self,
        subset_view: &Value,
        subset: &Value,
        target: &Value,
        live_set: Option<&Value>,
        live_dict: Option<&Value>,
        set_snapshot: Option<&PySet>,
    ) -> Result<bool> {
        let expected_len = subset
            .dict_len()
            .ok_or_else(|| PyError::Runtime("internal: expected dict backing".to_string()))?;
        if self.eq_cycle_enter(target, subset) {
            return Ok(true);
        }
        let result = (|| -> Result<bool> {
            let ordered_watch = Self::ordered_dict_view_comparison_watch(subset_view);
            let mut cursor = LiveKeyCursor::dict(
                subset_view,
                pyrust_builtins::dict_views::DictViewKind::Items.live_cursor_code(),
                expected_len,
            );
            while let Some(item) = Self::advance_setlike_comparison_cursor(
                subset_view,
                &mut cursor,
                expected_len,
                ordered_watch.as_ref(),
            )? {
                let LiveDictViewItem::Pair(key, value) = item else {
                    unreachable!("dict-items cursor must yield key/value pairs")
                };
                let probe = self.value_to_pykey(&Value::tuple(vec![key, value]))?;
                let contains = if let Some(set) = live_set {
                    self.set_lookup(set, &probe)?.is_some()
                } else if let Some(dict) = live_dict {
                    self.dict_lookup(dict, &probe)?.is_some()
                } else {
                    self.set_lookup_in(
                        set_snapshot.expect("set-like target must have a lookup backing"),
                        &probe,
                    )?
                    .is_some()
                };
                if !contains {
                    return Ok(false);
                }
            }
            Ok(true)
        })();
        self.eq_cycle_exit(target, subset);
        result
    }

    /// Lazily iterate a `dict_items` source against another set-like operand.
    /// Each live item becomes a fresh tuple key only when reached, preserving
    /// key/value `__hash__` calls and first-miss short-circuiting.
    pub(crate) fn dict_items_view_is_subset_of_setlike(
        &mut self,
        subset: &Value,
        target: &Value,
    ) -> Option<Result<bool>> {
        let subset_backing = Self::dict_items_backing_value(subset)?;
        if matches!(
            pyrust_builtins::dict_views::view_kind(target),
            Some(pyrust_builtins::dict_views::DictViewKind::Items)
        ) {
            return None;
        }

        let live_set = if target.is_set() {
            Some(target.clone())
        } else {
            builtin_data_backing(target).filter(Value::is_set)
        };
        let live_dict = matches!(
            pyrust_builtins::dict_views::view_kind(target),
            Some(pyrust_builtins::dict_views::DictViewKind::Keys)
        )
        .then(|| pyrust_builtins::dict_views::as_dict_rc(target).map(Value::dict_shared))
        .flatten();
        let set_snapshot = if live_set.is_none() && live_dict.is_none() {
            let (items, _) = match self.coerce_set_operand(target)? {
                Ok(items) => items,
                Err(error) => return Some(Err(error)),
            };
            Some(items)
        } else {
            None
        };

        Some(self.dict_mapping_items_are_subset_of_setlike(
            subset,
            &subset_backing,
            target,
            live_set.as_ref(),
            live_dict.as_ref(),
            set_snapshot.as_ref(),
        ))
    }

    /// Lazily scan a mutable set or keys view when a `dict_items` view is the
    /// containment target. A callback from item-value equality must remain
    /// able to invalidate the live source iterator.
    pub(crate) fn setlike_is_subset_of_dict_items(
        &mut self,
        subset: &Value,
        superset: &Value,
    ) -> Option<Result<bool>> {
        if !matches!(
            pyrust_builtins::dict_views::view_kind(superset),
            Some(pyrust_builtins::dict_views::DictViewKind::Items)
        ) || matches!(
            pyrust_builtins::dict_views::view_kind(subset),
            Some(pyrust_builtins::dict_views::DictViewKind::Items)
        ) {
            return None;
        }

        let live_set =
            subset.is_set() || builtin_data_backing(subset).is_some_and(|backing| backing.is_set());
        let keys_view = matches!(
            pyrust_builtins::dict_views::view_kind(subset),
            Some(pyrust_builtins::dict_views::DictViewKind::Keys)
        );
        let live_source_len = (live_set || keys_view)
            .then(|| set_comparison_len(subset))
            .flatten();
        let ordered_watch = Self::ordered_dict_view_comparison_watch(subset);
        let mut live_cursor = if live_set {
            Some(LiveKeyCursor::set(subset))
        } else if keys_view {
            Some(LiveKeyCursor::dict(
                subset,
                pyrust_builtins::dict_views::DictViewKind::Keys.live_cursor_code(),
                live_source_len?,
            ))
        } else {
            None
        };
        let snapshot = if live_cursor.is_none() {
            let (items, _) = match self.coerce_set_operand(subset)? {
                Ok(items) => items,
                Err(error) => return Some(Err(error)),
            };
            Some(items)
        } else {
            None
        };

        if self.eq_cycle_enter(superset, subset) {
            return Some(Ok(true));
        }
        let result = (|| -> Result<bool> {
            if let Some(cursor) = &mut live_cursor {
                while let Some(item) = Self::advance_setlike_comparison_cursor(
                    subset,
                    cursor,
                    live_source_len.expect("live set-like source must have a length"),
                    ordered_watch.as_ref(),
                )? {
                    let LiveDictViewItem::Item(candidate) = item else {
                        unreachable!("set-like cursor must yield individual values")
                    };
                    if !self
                        .dict_items_view_contains(superset, &candidate)
                        .expect("dict_items superset must have a backing mapping")?
                    {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }

            for key in snapshot.expect("immutable set-like source must have a snapshot") {
                let candidate = crate::interpreter::key_to_value(key);
                if !self
                    .dict_items_view_contains(superset, &candidate)
                    .expect("dict_items superset must have a backing mapping")?
                {
                    return Ok(false);
                }
            }
            Ok(true)
        })();
        self.eq_cycle_exit(superset, subset);
        Some(result)
    }

    /// Interpreter-owned `candidate in dict_items_view` for mixed set-like
    /// ordering. This preserves dynamic key and stored-value equality.
    pub(crate) fn dict_items_view_contains(
        &mut self,
        view: &Value,
        candidate: &Value,
    ) -> Option<Result<bool>> {
        let backing = Self::dict_items_backing_value(view)?;
        let tuple_candidate = if candidate.as_tuple().is_some() {
            candidate.clone()
        } else {
            crate::interpreter::builtin_data_backing(candidate).unwrap_or_else(|| candidate.clone())
        };
        let pair = match tuple_candidate.as_tuple() {
            Some(items) if items.len() == 2 => [items[0].clone(), items[1].clone()],
            _ => return Some(Ok(false)),
        };
        Some((|| -> Result<bool> {
            let key = self.value_to_pykey(&pair[0])?;
            let Some((_, stored_value)) = self.dict_lookup(&backing, &key)? else {
                return Ok(false);
            };
            self.values_richcompare_eq(&stored_value, &pair[1])
        })())
    }

    /// Equality membership for frozensets containing a dynamic key. Keep the
    /// hash-index allocation and collision walk out of `values_user_eq`'s hot
    /// primitive/container dispatcher; ordinary primitive frozensets return
    /// before this cold path.
    #[cold]
    #[inline(never)]
    fn frozenset_keys_user_eq(&mut self, lhs: &PySet, rhs: &PySet) -> Result<bool> {
        let lhs_probe_table = lhs.python_hash_snapshot();
        let rhs_probe_table = rhs.python_hash_snapshot();
        let mut candidates = Vec::new();
        for key in lhs_probe_table.active_keys(lhs) {
            let hash = pyrust_core::py_hash_pykey(key) as u64;
            rhs_probe_table.collect_candidate_keys(rhs, hash, &mut candidates);
            if !self.set_lookup_candidates_in_python_eq(key, &candidates)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

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
    ///    nested container), clone only the current live pair and recurse.
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
    /// List/dict recursion uses an exception-capable ordered-pair context:
    /// a pure structural re-entry raises comparison `RecursionError`, while a
    /// user callback advances the dispatch epoch and may replace a later edge
    /// before the platform comparison budget is exhausted. Legacy set/view
    /// paths retain `eq_in_progress` below.
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
            // Slow path: clone only the current pair before invoking user
            // code. Reading subsequent positions live lets a finite callback
            // replace a later recursive edge and terminate the comparison.
            let list_pair = a.is_list() && b.is_list();
            let expected_len = match (a.kind(), b.kind()) {
                (ValueKind::List(left), ValueKind::List(_)) => left.len(),
                (ValueKind::Tuple(left), ValueKind::Tuple(_)) => left.len(),
                _ => unreachable!("needs_seq_dispatch implies a sequence pair"),
            };
            let structural_bias = usize::from(self.comparison_recursion_cost == 0);
            return self.with_comparison_pair_cost(a, b, 1, structural_bias, |interp| {
                for index in 0..expected_len {
                    let Some(x) = sequence_item_at(a, index) else {
                        return Ok(false);
                    };
                    let Some(y) = sequence_item_at(b, index) else {
                        return Ok(false);
                    };
                    if !interp.values_richcompare_eq(&x, &y)? {
                        return Ok(false);
                    }
                }
                if list_pair
                    && (a.list_len() != Some(expected_len) || b.list_len() != Some(expected_len))
                {
                    return Ok(false);
                }
                Ok(true)
            });
        }

        // Primitive / identity fast path. Core dictionary equality has a
        // bool-only cycle guard, so only exact scalar dictionaries may use it;
        // recursive or callback-capable values must reach the guarded path.
        match (a.kind(), b.kind()) {
            (ValueKind::Dict(left), ValueKind::Dict(right)) => {
                if values_are_identical(a, b) {
                    return Ok(true);
                }
                if let Some(equal) = try_dict_fast_eq(&left, &right) {
                    return Ok(equal);
                }
            }
            _ if a == b => return Ok(true),
            _ => {}
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
            ContainerSnapshot::Dict { expected_len } => {
                let structural_bias = usize::from(self.comparison_recursion_cost == 0);
                let result =
                    self.with_comparison_pair_cost(a, b, 1, structural_bias, |interp| {
                        for index in 0..expected_len {
                            // Clone only the current key. This keeps recursive
                            // callback frames O(depth), while values and later
                            // positions remain live for finite edge replacement.
                            let Some(pk) = a
                                .dict_with(|dict| dict.get_index(index).map(|(key, _)| key.clone()))
                                .flatten()
                            else {
                                return Ok(false);
                            };
                            let Some(v_lhs) = a.dict_with(|dict| dict.get(&pk).cloned()).flatten()
                            else {
                                return Ok(false);
                            };
                            let Some((_, v_rhs)) = interp.dict_lookup(b, &pk)? else {
                                return Ok(false);
                            };
                            if !interp.values_richcompare_eq(&v_lhs, &v_rhs)? {
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    })?;
                return Ok(result
                    && a.dict_len() == Some(expected_len)
                    && b.dict_len() == Some(expected_len));
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
        // live inside a BuiltinObject.  `set_lookup_in_python_eq` handles
        // cross-representation elements by dispatching user `__eq__`, so
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
            let lhs_has_dynamic_key = lhs_rc.iter().any(key_contains_object);
            let rhs_has_dynamic_key = rhs_rc.iter().any(key_contains_object);
            if !lhs_has_dynamic_key && !rhs_has_dynamic_key {
                self.eq_cycle_exit(a, b);
                return Ok(false);
            }
            let result = self.frozenset_keys_user_eq(&lhs_rc, &rhs_rc);
            self.eq_cycle_exit(a, b);
            return result;
        }

        // Item-view equality is directional containment: the right mapping's
        // stored value owns equality against the left probe value. It must not
        // hash values, because dictionary items can contain lists/dicts.
        if let Some(result) = self.dict_items_views_are_equal(a, b) {
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
            // Set equality rejects unequal cardinality before materialising or
            // probing either operand. Thus unhashable item values stay inert
            // for `items == empty_set` and other size mismatches.
            if let (Some(a_len), Some(b_len)) = (set_comparison_len(a), set_comparison_len(b))
                && a_len != b_len
            {
                return Ok(false);
            }
            // Mixed view equality keeps the receiver as the lazy source.
            // `items == keys` hashes each reached item tuple, while
            // `keys == items` probes each key as-is and can return false
            // without touching an unhashable item value.
            if let Some(result) = self.dict_items_view_is_subset_of_setlike(a, b) {
                return result;
            }
            if is_setlike_view(a) {
                if let Some(result) = self.setlike_is_subset_of_dict_items(a, b) {
                    return result;
                }
            } else if let Some(result) = self.dict_items_view_is_subset_of_setlike(b, a) {
                // Exact set/frozenset equality returns NotImplemented for a
                // view, so native comparison reaches the reflected item-view
                // slot and uses that item view as the source.
                return result;
            }
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
        if let Some(r) = self.try_comparison_dunder_binary(a, b, "__eq__", "__eq__") {
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

    /// Reject a Python equality callback once the retained native comparison
    /// frames have consumed CPython's remaining callback-entry headroom.
    pub(crate) fn guard_comparison_user_call(&self, additional_cost: usize) -> Result<()> {
        if self
            .comparison_recursion_cost
            .saturating_add(additional_cost)
            >= MAX_COMPARISON_COST_BEFORE_USER_EQ
        {
            return Err(self.recursion_limit_error()?);
        }
        Ok(())
    }

    /// Record observable progress made by a user comparison slot. A callback
    /// may return to the same Python depth after replacing a later recursive
    /// edge, so the marker is monotonic for the lifetime of the interpreter.
    pub(crate) fn note_comparison_dispatch(&mut self) {
        if !self.comparison_pair_stack.is_empty() {
            self.comparison_dispatch_epoch = self.comparison_dispatch_epoch.wrapping_add(1);
        }
    }

    /// Run one recursive container comparison under an ordered-pair guard.
    /// Re-entering the pair at the same call depth and dispatch epoch is a pure
    /// structural cycle; callback progress receives another bounded pass.
    pub(crate) fn with_comparison_pair<T>(
        &mut self,
        a: &Value,
        b: &Value,
        compare: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.with_comparison_pair_cost(a, b, 1, 0, compare)
    }

    /// Sequence ordering retains one native ordering frame in addition to its
    /// equality-prefix walk. The structural bias accounts for the innermost
    /// scalar sequence, which resolves without installing another pair frame.
    pub(crate) fn with_comparison_order_pair<T>(
        &mut self,
        a: &Value,
        b: &Value,
        compare: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.with_comparison_pair_cost(a, b, 1, 1, compare)
    }

    pub(crate) fn has_active_comparison(&self) -> bool {
        self.comparison_recursion_cost != 0
    }

    fn with_comparison_pair_cost<T>(
        &mut self,
        a: &Value,
        b: &Value,
        cost: usize,
        structural_bias: usize,
        compare: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let active_structural_cost = self
            .comparison_recursion_cost
            .saturating_add(self.comparison_structural_cost_bias);
        if cost.saturating_add(structural_bias)
            > MAX_ACTIVE_COMPARISON_COST.saturating_sub(active_structural_cost)
        {
            return Err(comparison_recursion_error());
        }

        let pair = match (a.value_id(), b.value_id()) {
            (Some(a_id), Some(b_id)) => Some((a_id, b_id)),
            _ => None,
        };
        let call_depth = super::get_call_depth();
        let context = (call_depth, self.comparison_dispatch_epoch);
        if pair.is_some_and(|pair| {
            self.comparison_active_contexts
                .get(&pair)
                .is_some_and(|contexts| contexts.contains(&context))
        }) {
            return Err(comparison_recursion_error());
        }

        if let Some(pair) = pair {
            self.comparison_pair_stack
                .push((pair.0, pair.1, context.0, context.1));
            self.comparison_active_contexts
                .entry(pair)
                .or_default()
                .push(context);
        }
        let grow_stack = active_structural_cost >= COMPARISON_STACK_GROW_START_COST;
        self.comparison_recursion_cost += cost;
        self.comparison_structural_cost_bias += structural_bias;
        let result = if grow_stack {
            stacker::maybe_grow(COMPARISON_STACK_RED_ZONE, COMPARISON_STACK_SEGMENT, || {
                compare(self)
            })
        } else {
            compare(self)
        };
        self.comparison_recursion_cost -= cost;
        self.comparison_structural_cost_bias -= structural_bias;

        if let Some(pair) = pair {
            let popped = self
                .comparison_pair_stack
                .pop()
                .expect("comparison pair stack must contain the active pair");
            debug_assert_eq!(popped, (pair.0, pair.1, context.0, context.1));
            let remove_pair = {
                let contexts = self
                    .comparison_active_contexts
                    .get_mut(&pair)
                    .expect("comparison pair index must contain the active pair");
                let popped = contexts
                    .pop()
                    .expect("comparison pair index must contain the active context");
                debug_assert_eq!(popped, context);
                contexts.is_empty()
            };
            if remove_pair {
                self.comparison_active_contexts.remove(&pair);
            }
        }
        result
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
        if let Some(equal) = scalar_pair_fast_eq(a, b) {
            return Ok(equal);
        }
        if values_are_identical(a, b) {
            return Ok(true);
        }
        self.values_user_eq(a, b)
    }

    /// Borrow-only scalar equality used by native sequence containers before
    /// they install the callback-capable comparison guard.
    pub(crate) fn try_scalar_richcompare_eq(a: &Value, b: &Value) -> Option<bool> {
        scalar_pair_fast_eq(a, b)
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
