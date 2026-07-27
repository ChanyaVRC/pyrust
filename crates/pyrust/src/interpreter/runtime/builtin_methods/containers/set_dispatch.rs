// Set/frozenset method dispatch and dict-view adaptation.
impl Interpreter {
    /// Dispatch a resolved set method. Methods that read or write keys
    /// (`add`/`discard`/`remove`/`__contains__`) route through
    /// `set_lookup`/`set_insert` so user-defined `__hash__`/`__eq__`
    /// fire (issue #368).  Everything else delegates to the
    /// interpreter-free `pyrust_builtins::set::call`. The caller owns the
    /// single name/signature resolution and validation step.
    pub(crate) fn call_set_method_resolved(
        &mut self,
        resolved: pyrust_builtins::set::Method,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        use pyrust_builtins::set::Method as SetMethod;
        match resolved {
            SetMethod::Add | SetMethod::Contains | SetMethod::Discard | SetMethod::Remove => {
                let mut iter = args.into_iter();
                let key_val = iter.next().ok_or_else(|| {
                    PyError::Runtime(format!(
                        "set.{}() requires at least 1 argument",
                        resolved.name()
                    ))
                })?;
                let pk = self.value_to_pykey(&key_val)?;
                match resolved {
                    SetMethod::Add => {
                        if self.set_lookup(&receiver, &pk)?.is_some() {
                            return Ok(Value::none());
                        }
                        receiver.set_add(pk)?;
                        Ok(Value::none())
                    }
                    SetMethod::Contains => {
                        Ok(Value::bool_(self.set_lookup(&receiver, &pk)?.is_some()))
                    }
                    SetMethod::Discard => {
                        if let Some(idx) = self.set_lookup(&receiver, &pk)? {
                            receiver
                                .set_with_mut(|set| {
                                    set.shift_remove_index(idx);
                                })
                                .ok_or_else(|| {
                                    PyError::Runtime("internal: expected set".to_string())
                                })?;
                        }
                        Ok(Value::none())
                    }
                    SetMethod::Remove => match self.set_lookup(&receiver, &pk)? {
                        Some(idx) => {
                            receiver
                                .set_with_mut(|set| {
                                    set.shift_remove_index(idx);
                                })
                                .ok_or_else(|| {
                                    PyError::Runtime("internal: expected set".to_string())
                                })?;
                            Ok(Value::none())
                        }
                        None => Err(PyError::key_error(key_val.clone())),
                    },
                    _ => unreachable!(),
                }
            }
            // set.update uses value_to_pykey so that hashable slices and
            // PyInstance elements (which need __hash__ dispatch) work correctly.
            // The pyrust-builtins path calls Value::to_key() which returns None
            // for slices (SliceOps doesn't implement hash), causing a misleading
            // "unhashable type: 'slice'" error for all slices.
            SetMethod::Update => {
                for arg in args {
                    // Snapshot if the argument is the receiver itself to avoid
                    // aliased-borrow issues during iteration (matches CPython
                    // semantics: s.update(s) is a no-op).
                    if arg.value_id() == receiver.value_id() && arg.value_id().is_some() {
                        let snapshot: Vec<PyKey> = receiver
                            .set_with(|s| s.iter().cloned().collect())
                            .unwrap_or_default();
                        for pk in snapshot {
                            if self.set_lookup(&receiver, &pk)?.is_none() {
                                receiver.set_add(pk)?;
                            }
                        }
                        continue;
                    }
                    // If the arg is already a set, copy its PyKeys directly.
                    if arg.is_set() {
                        let keys: Vec<PyKey> = arg
                            .set_with(|s| s.iter().cloned().collect())
                            .unwrap_or_default();
                        for pk in keys {
                            if self.set_lookup(&receiver, &pk)?.is_none() {
                                receiver.set_add(pk)?;
                            }
                        }
                        continue;
                    }
                    // These concrete sources are finite and cannot execute
                    // Python while being traversed.  Snapshotting them keeps
                    // the common list/tuple/dict update path compact without
                    // changing partial insertion semantics: hashing the
                    // snapshotted elements still happens one by one below.
                    if matches!(
                        arg.kind(),
                        ValueKind::List(_)
                            | ValueKind::Tuple(_)
                            | ValueKind::Str(_)
                            | ValueKind::Bytes(_)
                            | ValueKind::Dict(_)
                    ) {
                        for item in self.collect_iterable(&arg)? {
                            let pk = self.value_to_pykey(&item)?;
                            if self.set_lookup(&receiver, &pk)?.is_none() {
                                receiver.set_add(pk)?;
                            }
                        }
                        continue;
                    }
                    // General iterable: consume and insert one element at a
                    // time.  CPython keeps the completed prefix when a later
                    // `__next__`, `__hash__`, or equality check raises; an
                    // eager snapshot made the whole argument atomic and could
                    // not make progress on an unbounded source.
                    let iterator = make_iterator(self, &arg)?;
                    loop {
                        let item = match self.call_next(&iterator, None) {
                            Ok(item) => item,
                            Err(ref error) if is_stop_iteration_error(error) => break,
                            Err(error) => return Err(error),
                        };
                        let pk = self.value_to_pykey(&item)?;
                        if self.set_lookup(&receiver, &pk)?.is_none() {
                            receiver.set_add(pk)?;
                        }
                    }
                }
                Ok(Value::none())
            }
            SetMethod::IntersectionUpdate
            | SetMethod::DifferenceUpdate
            | SetMethod::SymmetricDifferenceUpdate
                if self.set_algebra_needs_runtime_keys(&receiver, &args)? =>
            {
                self.set_update_method_eq(resolved.name(), receiver, args)
            }
            // `isdisjoint` is a short-circuiting predicate, not a set
            // construction.  Drive arbitrary operands incrementally so a
            // match returns before later source side effects or errors.
            SetMethod::IsDisjoint if args.len() == 1 => {
                self.iterable_is_disjoint(receiver, args.into_iter().next().unwrap())
            }
            // Like `isdisjoint`, `issuperset` can decide False at the first
            // missing element and must not consume the rest of the operand.
            SetMethod::IsSuperset if args.len() == 1 => {
                self.iterable_is_superset(receiver, args.into_iter().next().unwrap())
            }
            // Set-algebra method forms (issue #1907).  When the receiver or any
            // operand holds user-instance keys, use the `__eq__`-aware
            // collection-key services; otherwise fall through to the fast
            // interpreter-free builtin path below.
            SetMethod::Union
            | SetMethod::Intersection
            | SetMethod::Difference
            | SetMethod::SymmetricDifference
            | SetMethod::IsSubset
            | SetMethod::IsSuperset
            | SetMethod::IsDisjoint
                if self.set_algebra_needs_runtime_keys(&receiver, &args)? =>
            {
                self.set_algebra_method_eq(resolved.name(), receiver, args)
            }
            _ => pyrust_builtins::set::call_resolved(resolved, &receiver, args),
        }
    }

    /// `isdisjoint` for the set-like dict views `dict_keys` / `dict_items`
    /// (issue #1891).  Accepts any iterable and returns `True` when no element
    /// of the argument is a member of the view.  Iterating the *argument* (and
    /// probing the view's `__contains__`) — rather than building a set from the
    /// view — matches CPython's `dictviews_isdisjoint`: a `dict_items` view with
    /// unhashable values still works because its own values are never hashed.
    pub(crate) fn dict_view_isdisjoint(
        &mut self,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let view_name = value_type_name_str(&receiver);
        if args.len() != 1 {
            let n = args.len();
            return Err(pyrust_core::type_err!(
                "{view_name}.isdisjoint() takes exactly one argument ({n} given)"
            ));
        }
        self.iterable_is_disjoint(receiver, args.into_iter().next().unwrap())
    }

    /// `frozenset` method dispatch — mirror of [`Self::call_set_method`] for the
    /// frozen variant (issue #1907).  Intercepts the set-algebra method forms
    /// when user `__eq__` is required, uses the shared equality-aware key
    /// services, and coerces algebra results back to `frozenset` (CPython:
    /// `frozenset.union(...)` returns a `frozenset`).  All other methods (and
    /// the all-primitive fast path) go straight to the interpreter-free
    /// builtin implementation.
    pub(crate) fn call_frozenset_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        pyrust_builtins::frozenset::validate_method_positional_arity(method, args.len())?;
        match method {
            "isdisjoint" if args.len() == 1 => {
                self.iterable_is_disjoint(receiver, args.into_iter().next().unwrap())
            }
            "issuperset" if args.len() == 1 => {
                self.iterable_is_superset(receiver, args.into_iter().next().unwrap())
            }
            "union" | "intersection" | "difference" | "symmetric_difference"
                if self.set_algebra_needs_runtime_keys(&receiver, &args)? =>
            {
                let result = self.set_algebra_method_eq(method, receiver, args)?;
                // `set_algebra_method_eq` returns a `set`; re-freeze for the
                // frozenset receiver (the algebra result type follows CPython).
                match result.set_with(|s| pyrust_builtins::frozenset::frozenset(s.clone())) {
                    Some(fz) => Ok(fz),
                    None => Ok(result),
                }
            }
            "issubset" | "issuperset" | "isdisjoint"
                if self.set_algebra_needs_runtime_keys(&receiver, &args)? =>
            {
                self.set_algebra_method_eq(method, receiver, args)
            }
            _ => pyrust_builtins::frozenset::call_prevalidated(method, &receiver, args),
        }
    }
}
