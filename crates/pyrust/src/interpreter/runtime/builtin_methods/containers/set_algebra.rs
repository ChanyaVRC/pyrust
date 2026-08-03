// Equality-aware set algebra shared by mutable sets, frozen sets, and dict views.
impl Interpreter {
    /// Probe an iterable against a set-like receiver until the first match.
    ///
    /// This is shared by set, frozenset, and dict-view `isdisjoint`.  It keeps
    /// the operand lazy and delegates membership to the receiver, preserving
    /// its exact hashing/equality rules (including unhashable dict-item
    /// values).
    fn iterable_is_disjoint(&mut self, receiver: Value, other: Value) -> Result<Value> {
        let iterator = make_iterator(self, &other)?;
        loop {
            let item = match self.call_next(&iterator, None) {
                Ok(item) => item,
                Err(ref error) if is_stop_iteration_error(error) => {
                    return Ok(Value::bool_(true));
                }
                Err(error) => return Err(error),
            };
            if self.eval_in(receiver.clone(), item)?.truthy_raw() {
                return Ok(Value::bool_(false));
            }
        }
    }

    /// Incremental implementation of `set`/`frozenset.issuperset`.
    fn iterable_is_superset(&mut self, receiver: Value, other: Value) -> Result<Value> {
        let iterator = make_iterator(self, &other)?;
        loop {
            let item = match self.call_next(&iterator, None) {
                Ok(item) => item,
                Err(ref error) if is_stop_iteration_error(error) => {
                    return Ok(Value::bool_(true));
                }
                Err(error) => return Err(error),
            };
            if !self.eval_in(receiver.clone(), item)?.truthy_raw() {
                return Ok(Value::bool_(false));
            }
        }
    }

    /// True when a set-algebra method form (`union`/`intersection`/…) needs
    /// interpreter-owned key conversion or equality: the receiver contains a
    /// `PyKey::Object`, or an operand transitively contains a user object or a
    /// slice key (issue #1907).  Ordinary primitive operands return false and
    /// keep the interpreter-free builtin path.
    fn set_algebra_needs_runtime_keys(&mut self, receiver: &Value, args: &[Value]) -> Result<bool> {
        let recv_has_obj = receiver
            .set_with(set_has_object_key)
            .or_else(|| {
                pyrust_builtins::frozenset::as_items(receiver).map(|rc| set_has_object_key(&rc))
            })
            .unwrap_or(false);
        if recv_has_obj {
            return Ok(true);
        }
        // Cheap operand scan without building any PyKeys.  This detects both
        // user instances and interpreter-owned aggregate conversion (slice)
        // while keeping ordinary primitive method forms on the borrow-only
        // fast path.
        for arg in args {
            if value_iterable_needs_runtime_key_semantics(arg) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Materialise an arbitrary iterable operand into a deduplicated
    /// `PySet`, dispatching user `__hash__`/`__eq__` so that two
    /// `__eq__`-equal instances collapse to a single entry (issue #1907).
    fn materialize_set_operand(&mut self, arg: &Value) -> Result<PySet> {
        // Existing set/frozenset storage is already hash- and equality-
        // deduplicated.  Reuse its PyKeys so method calls between sets do not
        // spuriously call user `__hash__` again.
        if let Some((items, _)) = set_items_from_value(arg) {
            return Ok(items);
        }
        let items = self.collect_iterable(arg)?;
        let mut out: PySet = PySet::default();
        for item in items {
            let pk = self.value_to_pykey(&item)?;
            self.set_insert(&mut out, pk)?;
        }
        Ok(out)
    }

    fn set_receiver_items(&self, receiver: &Value, method: &str) -> Result<PySet> {
        receiver
            .set_with(|items| items.clone())
            .or_else(|| {
                pyrust_builtins::frozenset::as_items(receiver).map(|items| (*items).clone())
            })
            .ok_or_else(|| {
                PyError::named("TypeError", format!("set.{method} receiver is not a set"))
            })
    }

    fn replace_set_items(&self, receiver: &Value, replacement: PySet) -> Result<()> {
        receiver
            .set_with_mut(|items| *items = replacement)
            .ok_or_else(|| PyError::Runtime("internal: expected mutable set".to_string()))
    }

    /// One CPython `set_intersection` step. Exact set operands are scanned in
    /// slot order from the smaller table (the RHS wins ties); arbitrary
    /// iterables are consumed directly and may stop once every receiver item
    /// has been found.
    fn set_intersection_once_eq(&mut self, current: &PySet, arg: &Value) -> Result<PySet> {
        let mut result = PySet::default();
        if let Some((other, _)) = set_items_from_value(arg) {
            let (scan, membership) = if other.len() > current.len() {
                (current, &other)
            } else {
                (&other, current)
            };
            let scan_table = scan.python_hash_snapshot();
            let keys: Vec<PyKey> = scan_table.active_keys(scan).cloned().collect();
            for key in keys {
                if self.set_lookup_in(membership, &key)?.is_some() {
                    self.set_insert(&mut result, key)?;
                }
            }
            return Ok(result);
        }

        let iterator = make_iterator(self, arg)?;
        loop {
            let item = match self.call_next(&iterator, None) {
                Ok(item) => item,
                Err(ref error) if is_stop_iteration_error(error) => break,
                Err(error) => return Err(error),
            };
            let key = self.value_to_pykey(&item)?;
            if self.set_lookup_in(current, &key)?.is_some() {
                self.set_insert(&mut result, key)?;
                if result.len() >= current.len() {
                    break;
                }
            }
        }
        Ok(result)
    }

    /// Construct the temporary set used by CPython's
    /// `set_symmetric_difference`: exact sets merge-copy their slot table,
    /// exact dicts pre-size once, and general iterables insert incrementally.
    fn cpython_set_from_operand_eq(&mut self, arg: &Value) -> Result<PySet> {
        if let Some((source, _)) = set_items_from_value(arg) {
            return Ok(PySet::cpython_merged_copy(&source));
        }
        if let ValueKind::Dict(source) = arg.kind() {
            let keys: Vec<PyKey> = source.keys().cloned().collect();
            let mut result = PySet::with_cpython_dict_capacity(keys.len());
            for key in keys {
                self.set_insert(&mut result, key)?;
            }
            return Ok(result);
        }

        let iterator = make_iterator(self, arg)?;
        let mut result = PySet::default();
        loop {
            let item = match self.call_next(&iterator, None) {
                Ok(item) => item,
                Err(ref error) if is_stop_iteration_error(error) => break,
                Err(error) => return Err(error),
            };
            let key = self.value_to_pykey(&item)?;
            self.set_insert(&mut result, key)?;
        }
        Ok(result)
    }

    fn set_discard_key_eq(&mut self, receiver: &Value, key: &PyKey) -> Result<()> {
        if let Some(index) = self.set_lookup(receiver, key)? {
            receiver
                .set_with_mut(|items| {
                    items.shift_remove_index(index);
                })
                .ok_or_else(|| PyError::Runtime("internal: expected mutable set".to_string()))?;
        }
        Ok(())
    }

    fn finish_set_difference_eq(&self, receiver: &Value) -> Result<()> {
        receiver
            .set_with_mut(PySet::finish_cpython_difference_update)
            .ok_or_else(|| PyError::Runtime("internal: expected mutable set".to_string()))
    }

    fn set_difference_update_arg_eq(&mut self, receiver: &Value, arg: &Value) -> Result<()> {
        if arg.value_id() == receiver.value_id() && receiver.value_id().is_some() {
            receiver.set_clear()?;
            return Ok(());
        }
        if let Some((source, _)) = set_items_from_value(arg) {
            let current = self.set_receiver_items(receiver, "difference_update")?;
            let scan = if (source.len() >> 3) > current.len() {
                self.set_intersection_once_eq(&current, arg)?
            } else {
                source
            };
            let scan_table = scan.python_hash_snapshot();
            let keys: Vec<PyKey> = scan_table.active_keys(&scan).cloned().collect();
            for key in keys {
                self.set_discard_key_eq(receiver, &key)?;
            }
        } else {
            let iterator = make_iterator(self, arg)?;
            loop {
                let item = match self.call_next(&iterator, None) {
                    Ok(item) => item,
                    Err(ref error) if is_stop_iteration_error(error) => break,
                    Err(error) => return Err(error),
                };
                let key = self.value_to_pykey(&item)?;
                self.set_discard_key_eq(receiver, &key)?;
            }
        }
        self.finish_set_difference_eq(receiver)
    }

    /// Interpreter-aware implementations of the mutating algebra methods.
    ///
    /// The primitive-only calls never enter here.  Each positional operand is
    /// committed separately, preserving CPython's partial progress across
    /// multiple operands.  Equality runs without a live receiver borrow.
    fn set_update_method_eq(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "intersection_update" => {
                for arg in &args {
                    let current = self.set_receiver_items(&receiver, method)?;
                    let replacement =
                        if arg.value_id() == receiver.value_id() && receiver.value_id().is_some() {
                            PySet::cpython_merged_copy(&current)
                        } else {
                            self.set_intersection_once_eq(&current, arg)?
                        };
                    self.replace_set_items(&receiver, replacement)?;
                }
            }
            "difference_update" => {
                for arg in &args {
                    self.set_difference_update_arg_eq(&receiver, arg)?;
                }
            }
            "symmetric_difference_update" => {
                let arg = args.first().ok_or_else(|| {
                    PyError::Runtime(
                        "set.symmetric_difference_update() requires 1 argument".to_string(),
                    )
                })?;
                if arg.value_id() == receiver.value_id() && receiver.value_id().is_some() {
                    receiver.set_clear()?;
                    return Ok(Value::none());
                }
                let keys: Vec<PyKey> = if let Some((source, _)) = set_items_from_value(arg) {
                    let source_table = source.python_hash_snapshot();
                    source_table.active_keys(&source).cloned().collect()
                } else if let ValueKind::Dict(source) = arg.kind() {
                    source.keys().cloned().collect()
                } else {
                    let source = self.cpython_set_from_operand_eq(arg)?;
                    let source_table = source.python_hash_snapshot();
                    source_table.active_keys(&source).cloned().collect()
                };
                // Toggle incoming keys against the live receiver one at a
                // time.  This gives the stored receiver key ownership of
                // `__eq__`, and exposes completed-prefix mutation if a later
                // equality call raises, as CPython does.
                for key in keys {
                    if let Some(index) = self.set_lookup(&receiver, &key)? {
                        receiver
                            .set_with_mut(|items| {
                                items.shift_remove_index(index);
                            })
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected mutable set".to_string())
                            })?;
                    } else {
                        receiver.set_add(key)?;
                    }
                }
            }
            _ => unreachable!("set_update_method_eq called for a non-mutating method"),
        }
        Ok(Value::none())
    }

    /// `__eq__`-aware implementation of the set-algebra method forms.
    /// Operands are materialised through the interpreter-aware key converter,
    /// then combined with comparison direction and representative selection
    /// matching CPython (issue #1907).
    fn set_algebra_method_eq(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "union" => {
                // CPython starts a union with set_copy(receiver), whose
                // set_merge path may compact dummies or preserve an exact
                // clean table.  A plain PySet clone would instead retain the
                // receiver's mutation topology in the new result.
                let receiver_items = self.set_receiver_items(&receiver, method)?;
                let mut acc = PySet::cpython_merged_copy(&receiver_items);
                for arg in &args {
                    if arg.value_id() == receiver.value_id() && receiver.value_id().is_some() {
                        continue;
                    }
                    // Exact set/frozenset operands use set_merge: one
                    // pre-resize followed by source-slot-order insertion.
                    // Both choices are observable later when a frozenset key
                    // collides with a dynamic dict probe.
                    if let Some((source, _)) = set_items_from_value(arg) {
                        if acc.is_empty() {
                            acc = PySet::cpython_merged_copy(&source);
                            continue;
                        }
                        let source_table = source.python_hash_snapshot();
                        let keys: Vec<PyKey> = source_table.active_keys(&source).cloned().collect();
                        acc.prepare_cpython_merge(source.len());
                        for key in keys {
                            self.set_insert(&mut acc, key)?;
                        }
                        continue;
                    }

                    // The exact-dict fast path also pre-sizes once, but
                    // traverses keys in dict insertion order rather than a
                    // temporary set's slot order.
                    if let ValueKind::Dict(source) = arg.kind() {
                        let keys: Vec<PyKey> = source.keys().cloned().collect();
                        acc.prepare_cpython_merge(keys.len());
                        for key in keys {
                            self.set_insert(&mut acc, key)?;
                        }
                        continue;
                    }

                    // General iterables are consumed incrementally by
                    // set_update_iterable_lock_held.  Do not materialise a
                    // temporary set here: doing so would deduplicate and then
                    // reorder the source before it reaches the result table.
                    let iterator = make_iterator(self, arg)?;
                    loop {
                        let item = match self.call_next(&iterator, None) {
                            Ok(item) => item,
                            Err(ref error) if is_stop_iteration_error(error) => break,
                            Err(error) => return Err(error),
                        };
                        let key = self.value_to_pykey(&item)?;
                        self.set_insert(&mut acc, key)?;
                    }
                }
                Ok(Value::set(acc))
            }
            "intersection" => {
                let mut acc = self.set_receiver_items(&receiver, method)?;
                for (index, arg) in args.iter().enumerate() {
                    if index == 0
                        && arg.value_id() == receiver.value_id()
                        && receiver.value_id().is_some()
                    {
                        acc = PySet::cpython_merged_copy(&acc);
                    } else {
                        acc = self.set_intersection_once_eq(&acc, arg)?;
                    }
                }
                if args.is_empty() {
                    acc = PySet::cpython_merged_copy(&acc);
                }
                Ok(Value::set(acc))
            }
            "difference" => {
                let receiver_items = self.set_receiver_items(&receiver, method)?;
                if args.is_empty() {
                    return Ok(Value::set(PySet::cpython_merged_copy(&receiver_items)));
                }

                let first = &args[0];
                let exact_set = set_items_from_value(first);
                let exact_dict_size = match first.kind() {
                    ValueKind::Dict(source) => Some(source.len()),
                    _ => None,
                };
                let concrete_size = exact_set
                    .as_ref()
                    .map(|(source, _)| source.len())
                    .or(exact_dict_size);

                let first_result =
                    if concrete_size.is_some_and(|size| (receiver_items.len() >> 2) <= size) {
                        // The compact-result branch scans the receiver's table in
                        // slot order and inserts only survivors into a fresh set.
                        let receiver_table = receiver_items.python_hash_snapshot();
                        let keys: Vec<PyKey> = receiver_table
                            .active_keys(&receiver_items)
                            .cloned()
                            .collect();
                        let mut result = PySet::default();
                        for key in keys {
                            let present = if let Some((source, _)) = exact_set.as_ref() {
                                self.set_lookup_in(source, &key)?.is_some()
                            } else {
                                self.dict_lookup(first, &key)?.is_some()
                            };
                            if !present {
                                self.set_insert(&mut result, key)?;
                            }
                        }
                        result
                    } else {
                        // A small or generic RHS uses set_copy followed by one
                        // difference-update pass, retaining dummies unless the
                        // post-pass cleanup threshold is crossed.
                        let result = Value::set(PySet::cpython_merged_copy(&receiver_items));
                        self.set_difference_update_arg_eq(&result, first)?;
                        self.set_receiver_items(&result, method)?
                    };

                let acc = Value::set(first_result);
                for arg in args.iter().skip(1) {
                    self.set_difference_update_arg_eq(&acc, arg)?;
                }
                Ok(acc)
            }
            "symmetric_difference" => {
                let arg = args.first().ok_or_else(|| {
                    PyError::Runtime("set.symmetric_difference() requires 1 argument".to_string())
                })?;
                let recv_items = self.set_receiver_items(&receiver, method)?;
                // CPython starts from the incoming set and toggles receiver
                // keys into it.  Besides avoiding the old duplicate pair of
                // equality calls, this preserves its observable comparison
                // direction (`incoming.__eq__(receiver)`).
                let result = Value::set(self.cpython_set_from_operand_eq(arg)?);
                let recv_table = recv_items.python_hash_snapshot();
                let keys: Vec<PyKey> = recv_table.active_keys(&recv_items).cloned().collect();
                for key in keys {
                    if let Some(index) = self.set_lookup(&result, &key)? {
                        result
                            .set_with_mut(|items| {
                                items.shift_remove_index(index);
                            })
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected set".to_string())
                            })?;
                    } else {
                        result.set_add(key)?;
                    }
                }
                Ok(result)
            }
            "issubset" | "issuperset" => {
                let arg = args.first().ok_or_else(|| {
                    PyError::Runtime(format!("set.{method}() requires 1 argument"))
                })?;
                let recv_items = self.set_receiver_items(&receiver, method)?;
                let lhs = Value::set(recv_items);
                let rhs = Value::set(self.materialize_set_operand(arg)?);
                let op = if method == "issubset" {
                    BinaryOp::Le
                } else {
                    BinaryOp::Ge
                };
                match set_subset_cmp(self, &lhs, &rhs, op) {
                    Some(r) => r,
                    None => unreachable!("both operands are sets"),
                }
            }
            "isdisjoint" => {
                // `a.isdisjoint(b)` is True when the two sets share no
                // `__eq__`-equal element (issue #1907).  Probe each receiver
                // element against the materialised operand via `set_lookup_in`,
                // which dispatches user `__hash__`-then-`__eq__`.
                let arg = args.first().ok_or_else(|| {
                    PyError::Runtime("set.isdisjoint() requires 1 argument".to_string())
                })?;
                let recv_items = self.set_receiver_items(&receiver, method)?;
                let other = self.materialize_set_operand(arg)?;
                for k in recv_items.iter() {
                    if self.set_lookup_in(&other, k)?.is_some() {
                        return Ok(Value::bool_(false));
                    }
                }
                Ok(Value::bool_(true))
            }
            _ => unreachable!("set_algebra_method_eq called with non-algebra method"),
        }
    }
}
