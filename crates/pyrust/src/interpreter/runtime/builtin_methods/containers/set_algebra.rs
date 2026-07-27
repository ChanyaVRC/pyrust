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
                    let direct_items = set_items_from_value(arg);
                    let incoming_is_set = direct_items.is_some();
                    let other = match direct_items {
                        Some((items, _)) => items,
                        None => self.materialize_set_operand(arg)?,
                    };
                    let current = self.set_receiver_items(&receiver, method)?;
                    let mut replacement = PySet::default();
                    // With two concrete sets CPython scans the smaller table
                    // (the incoming table wins ties); arbitrary iterables are
                    // scanned from the incoming side.  Keep the representative
                    // from whichever side was scanned.
                    let scan_current = incoming_is_set && other.len() > current.len();
                    if scan_current {
                        for key in &current {
                            if self.set_lookup_in(&other, key)?.is_some() {
                                self.set_insert(&mut replacement, key.clone())?;
                            }
                        }
                    } else {
                        for key in &other {
                            if self.set_lookup_in(&current, key)?.is_some() {
                                self.set_insert(&mut replacement, key.clone())?;
                            }
                        }
                    }
                    self.replace_set_items(&receiver, replacement)?;
                }
            }
            "difference_update" => {
                for arg in &args {
                    let other = self.materialize_set_operand(arg)?;
                    set_subtract_in_place(self, &receiver, &other)?;
                }
            }
            "symmetric_difference_update" => {
                let arg = args.first().ok_or_else(|| {
                    PyError::Runtime(
                        "set.symmetric_difference_update() requires 1 argument".to_string(),
                    )
                })?;
                let other = self.materialize_set_operand(arg)?;
                // Toggle incoming keys against the live receiver one at a
                // time.  This gives the stored receiver key ownership of
                // `__eq__`, and exposes completed-prefix mutation if a later
                // equality call raises, as CPython does.
                for key in other {
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
                let mut acc = self.set_receiver_items(&receiver, method)?;
                for arg in &args {
                    for key in self.materialize_set_operand(arg)? {
                        self.set_insert(&mut acc, key)?;
                    }
                }
                Ok(Value::set(acc))
            }
            "intersection" => {
                let mut acc = self.set_receiver_items(&receiver, method)?;
                for arg in &args {
                    let direct_items = set_items_from_value(arg);
                    let incoming_is_set = direct_items.is_some();
                    let other = match direct_items {
                        Some((items, _)) => items,
                        None => self.materialize_set_operand(arg)?,
                    };
                    let mut next = PySet::default();

                    // CPython's set-vs-set intersection scans the smaller
                    // table (the right table wins ties); arbitrary iterables
                    // are scanned from the incoming side.  The scanned side's
                    // representative is kept in the result.
                    let scan_acc = incoming_is_set && other.len() > acc.len();
                    if scan_acc {
                        for key in &acc {
                            if self.set_lookup_in(&other, key)?.is_some() {
                                self.set_insert(&mut next, key.clone())?;
                            }
                        }
                    } else {
                        for key in &other {
                            if self.set_lookup_in(&acc, key)?.is_some() {
                                self.set_insert(&mut next, key.clone())?;
                            }
                        }
                    }
                    acc = next;
                }
                Ok(Value::set(acc))
            }
            "difference" => {
                let acc = Value::set(self.set_receiver_items(&receiver, method)?);
                for arg in &args {
                    let direct_items = set_items_from_value(arg);
                    let incoming_is_set = direct_items.is_some();
                    let other = match direct_items {
                        Some((items, _)) => items,
                        None => self.materialize_set_operand(arg)?,
                    };
                    let current_len = acc.set_len().unwrap_or(0);
                    // CPython scans the receiver and probes a concrete RHS set
                    // unless the receiver is more than roughly four times
                    // larger.  Otherwise it copies/removes RHS probes.  The
                    // branch is observable through `__eq__` direction.
                    if incoming_is_set && (current_len >> 2) <= other.len() {
                        let current = self.set_receiver_items(&acc, method)?;
                        let mut replacement = PySet::default();
                        for key in &current {
                            if self.set_lookup_in(&other, key)?.is_none() {
                                replacement.insert(key.clone());
                            }
                        }
                        self.replace_set_items(&acc, replacement)?;
                    } else {
                        set_subtract_in_place(self, &acc, &other)?;
                    }
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
                let result = Value::set(self.materialize_set_operand(arg)?);
                for key in recv_items {
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
