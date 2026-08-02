// Equality-aware list/tuple search operations.

impl Interpreter {
    /// For `list.index` / `tuple.index`, resolve start (pos[1]) and stop
    /// (pos[2]) through `resolve_index_arg`.  pos[0] is the search target
    /// and is left unchanged.  Returns a new `Vec<Value>` with the resolved
    /// slice-boundary arguments in place.
    fn resolve_seq_index_pos(&mut self, mut pos: Vec<Value>) -> Result<Vec<Value>> {
        if pos.len() >= 2 {
            let start = pos.remove(1);
            let resolved = self.resolve_index_arg(start)?;
            pos.insert(1, resolved);
        }
        if pos.len() >= 3 {
            let stop = pos.remove(2);
            let resolved = self.resolve_index_arg(stop)?;
            pos.insert(2, resolved);
        }
        Ok(pos)
    }

    /// Returns `true` if searching `target` in `items` requires user `__eq__`
    /// dispatch (i.e. `values_user_eq`), rather than the primitive `Value::eq`
    /// (which uses `Rc::ptr_eq` for `PyInstance` and recursion for containers).
    ///
    /// Matches the same gate used by the `in` operator fix in PR #1638.
    pub(super) fn seq_search_needs_dispatch(target: &Value, items: &[Value]) -> bool {
        matches!(
            target.kind(),
            ValueKind::PyInstance(_)
                | ValueKind::Generator(_)
                | ValueKind::List(_)
                | ValueKind::Tuple(_)
                | ValueKind::Dict(_)
                | ValueKind::Set(_)
                | ValueKind::BuiltinObject { .. }
        ) || items.iter().any(|e| {
            matches!(
                e.kind(),
                ValueKind::PyInstance(_)
                    | ValueKind::Generator(_)
                    | ValueKind::List(_)
                    | ValueKind::Tuple(_)
                    | ValueKind::Dict(_)
                    | ValueKind::Set(_)
                    | ValueKind::BuiltinObject { .. }
            )
        })
    }
    /// `list.index(target[, start[, stop]])` / `tuple.index(...)` with correct
    /// `__eq__` dispatch and no whole-sequence dispatch pre-scan.
    ///
    /// Primitive elements are compared as they are encountered, so a match near
    /// the front is O(match position), not O(sequence length).  A list borrow is
    /// released before the first comparison that can call user `__eq__`; that
    /// slow path then reads each element afresh so re-entrant list mutation
    /// follows CPython's live-index walk.  Tuples are immutable and can be walked
    /// directly throughout.
    pub(super) fn call_seq_index(
        &mut self,
        receiver: &Value,
        args: Vec<Value>,
        type_name: &'static str,
    ) -> Result<Value> {
        let args = self.resolve_seq_index_pos(args)?;
        let target = args
            .first()
            .ok_or_else(|| pyrust_core::type_err!("index expected at least 1 argument, got 0"))?;
        let len = if type_name == "list" {
            receiver
                .list_with(|items| items.len())
                .ok_or_else(|| pyrust_core::type_err!("list.index receiver is not a list"))?
        } else {
            match receiver.kind() {
                ValueKind::Tuple(items) => items.len(),
                _ => {
                    return Err(pyrust_core::type_err!(
                        "tuple.index receiver is not a tuple"
                    ));
                }
            }
        };
        let start = match args.get(1).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => pyrust_builtins::sequence::normalise_index(i, len).min(len),
            Some(ValueKind::Bool(b)) => {
                pyrust_builtins::sequence::normalise_index(b as i64, len).min(len)
            }
            Some(ValueKind::BigInt(b)) => {
                pyrust_builtins::sequence::normalise_bigint_index(b, len).min(len)
            }
            None => 0,
            // Other types were already rejected by resolve_seq_index_pos.
            _ => 0,
        };
        let stop = match args.get(2).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => pyrust_builtins::sequence::normalise_index(i, len).min(len),
            Some(ValueKind::Bool(b)) => {
                pyrust_builtins::sequence::normalise_index(b as i64, len).min(len)
            }
            Some(ValueKind::BigInt(b)) => {
                pyrust_builtins::sequence::normalise_bigint_index(b, len).min(len)
            }
            None => len,
            _ => len,
        };
        let stop = stop.max(start);

        if type_name == "list" {
            enum SeqIndexScan {
                Found(usize),
                NotFound,
                NeedsDispatch(usize),
            }

            let target_dispatches = Self::value_search_dispatches(target);
            let slow_start = if target_dispatches {
                start
            } else {
                let outcome = receiver.list_with(|items| {
                    for (offset, item) in items[start..stop].iter().enumerate() {
                        let i = start + offset;
                        if !item.cannot_user_eq() {
                            return SeqIndexScan::NeedsDispatch(i);
                        }
                        if item == target || item.is_identical_nan(target) {
                            return SeqIndexScan::Found(i);
                        }
                    }
                    SeqIndexScan::NotFound
                });
                match outcome {
                    Some(SeqIndexScan::Found(i)) => return Ok(Value::int(i as i64)),
                    Some(SeqIndexScan::NotFound) => {
                        let repr_str = render_instance_repr(self, target)?;
                        return Err(pyrust_core::value_err!("{repr_str} is not in list"));
                    }
                    Some(SeqIndexScan::NeedsDispatch(i)) => i,
                    None => {
                        return Err(pyrust_core::type_err!("list.index receiver is not a list"));
                    }
                }
            };

            for i in slow_start..stop {
                let Some(item) = receiver.list_with(|items| items.get(i).cloned()).flatten() else {
                    break;
                };
                if self.values_richcompare_eq(&item, target)? {
                    return Ok(Value::int(i as i64));
                }
            }
        } else {
            let target_dispatches = Self::value_search_dispatches(target);
            let ValueKind::Tuple(items) = receiver.kind() else {
                return Err(pyrust_core::type_err!(
                    "tuple.index receiver is not a tuple"
                ));
            };
            for (i, item) in items[start..stop].iter().enumerate() {
                let equal = if !target_dispatches && item.cannot_user_eq() {
                    item == target || item.is_identical_nan(target)
                } else {
                    self.values_richcompare_eq(item, target)?
                };
                if equal {
                    return Ok(Value::int((start + i) as i64));
                }
            }
        }
        let msg = if type_name == "tuple" {
            "tuple.index(x): x not in tuple".to_string()
        } else {
            let repr_str = render_instance_repr(self, target)?;
            format!("{repr_str} is not in {type_name}")
        };
        Err(pyrust_core::value_err!(msg))
    }

    /// `list.count(target)` / `tuple.count(target)` with correct `__eq__` dispatch.
    ///
    /// The `items` snapshot is taken by the caller for the same borrow-safety
    /// reason as `call_seq_index`.
    pub(super) fn call_seq_count(
        &mut self,
        items: Vec<Value>,
        args: &[Value],
        type_name: &'static str,
    ) -> Result<Value> {
        let target = args.first().ok_or_else(|| {
            pyrust_core::type_err!("{type_name}.count() takes exactly one argument (0 given)")
        })?;
        if Self::seq_search_needs_dispatch(target, &items) {
            let mut n: i64 = 0;
            for item in &items {
                if self.values_richcompare_eq(item, target)? {
                    n += 1;
                }
            }
            Ok(Value::int(n))
        } else {
            // Identity short-circuit (CPython `PyObject_RichCompareBool`):
            // a NaN counting itself matches even though `==` is False.
            let n = items
                .iter()
                .filter(|v| **v == *target || v.is_identical_nan(target))
                .count();
            Ok(Value::int(n as i64))
        }
    }

    /// `list.remove(target)` with correct `__eq__` dispatch.
    ///
    /// Mirrors CPython's `list_remove`: walk the live list by index and remove
    /// the **first** element equal to `target`, in place, on a no match raise
    /// `ValueError` with CPython's fixed wording.
    ///
    /// The per-element comparison fuses two cases in a single scan so the common
    /// all-primitive case never pays for user-dispatch machinery (no second
    /// pass, no `values_user_eq` call):
    ///
    /// - When neither `target` nor the element can fire user `__eq__`
    ///   (`value_search_dispatches` is false for both), compare with
    ///   `Value::eq` — the same primitive equality the interpreter-free fast
    ///   path uses.
    /// - Otherwise dispatch through `values_richcompare_eq`, which checks
    ///   identity before `__eq__` (matching
    ///   `PyObject_RichCompareBool(item, x, Py_EQ)`) and may re-enter user code.
    ///
    /// The element is read fresh from the receiver each iteration and the length
    /// is rechecked before removal, so a user `__eq__` that mutates the list
    /// mid-search cannot index out of range.
    pub(super) fn call_seq_remove(&mut self, receiver: &Value, args: Vec<Value>) -> Result<Value> {
        if args.len() != 1 {
            return Err(pyrust_core::type_err!(
                "list.remove() takes exactly one argument ({} given)",
                args.len()
            ));
        }
        // Outcome of the single-borrow primitive fast scan below.
        enum SeqRemoveScan {
            Found(usize),
            NotFound,
            NeedsDispatch,
        }

        let target = &args[0];
        // Whether `target` itself can fire user `__eq__` (container / instance).
        // Cheap, kind-only check — does not scan the list.
        let target_dispatches = Self::value_search_dispatches(target);

        // Fast path: when `target` is primitive, attempt a single-borrow scan
        // using `Value::eq` (no re-entry into user code).  We can resolve the
        // whole search this way *only* while every element seen is also
        // primitive — a dispatching element (PyInstance / container) might
        // match `target` through its own `__eq__`, so as soon as we reach one
        // we abandon the fast scan and restart the slow per-index walk from the
        // front (preserving first-match semantics).  When no dispatching
        // element exists, this is one borrow and one pass — matching the
        // interpreter-free `ms::remove` cost.
        if !target_dispatches {
            let outcome = receiver.list_with(|items| {
                for (i, item) in items.iter().enumerate() {
                    // `cannot_user_eq` resolves scalar elements (the common
                    // all-int/str list) from `top16` alone — a single tag
                    // compare, no `ValueKind` build and no `RefCell` borrow —
                    // so the hot scan pays only this plus the `Value::eq`
                    // below, matching the interpreter-free `ms::remove` cost.
                    // A non-scalar element might match `target` through its own
                    // `__eq__`, so we abandon the fast scan and restart the slow
                    // per-index walk from the front (preserving first-match).
                    if !item.cannot_user_eq() {
                        return SeqRemoveScan::NeedsDispatch;
                    }
                    // Identity short-circuit (CPython `PyObject_RichCompareBool`):
                    // a NaN removing itself matches even though `==` is False.
                    if item == target || item.is_identical_nan(target) {
                        return SeqRemoveScan::Found(i);
                    }
                }
                SeqRemoveScan::NotFound
            });
            match outcome {
                Some(SeqRemoveScan::Found(i)) => {
                    receiver.list_pop_at(i)?;
                    return Ok(Value::none());
                }
                Some(SeqRemoveScan::NotFound) => {
                    return Err(pyrust_core::value_err!("list.remove(x): x not in list"));
                }
                // NeedsDispatch, or receiver is no longer a list — fall through
                // to the slow per-index walk below.
                _ => {}
            }
        }

        // Slow path: at least one operand can fire user `__eq__`.  Read each
        // element fresh and recheck length before removal so a user `__eq__`
        // that mutates the list mid-search cannot index out of range.
        let mut i = 0usize;
        loop {
            let item = match receiver.list_with(|items| items.get(i).cloned()) {
                Some(Some(item)) => item,
                _ => break,
            };
            if self.values_richcompare_eq(&item, target)? {
                if receiver.list_with(|items| i < items.len()).unwrap_or(false) {
                    receiver.list_pop_at(i)?;
                }
                return Ok(Value::none());
            }
            i += 1;
        }
        Err(pyrust_core::value_err!("list.remove(x): x not in list"))
    }

    /// `true` if a single value can fire user `__eq__` during a membership
    /// search (a `PyInstance`, or a container that may transitively hold one).
    /// The per-value half of `seq_search_needs_dispatch`.
    pub(super) fn value_search_dispatches(v: &Value) -> bool {
        matches!(
            v.kind(),
            ValueKind::PyInstance(_)
                | ValueKind::Generator(_)
                | ValueKind::List(_)
                | ValueKind::Tuple(_)
                | ValueKind::Dict(_)
                | ValueKind::Set(_)
                | ValueKind::BuiltinObject { .. }
        )
    }
}
