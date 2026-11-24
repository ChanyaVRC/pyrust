// Built-in list and tuple container method dispatch + `list.sort`.

impl Interpreter {
    fn dispatch_list_container_method(
        &mut self,
        receiver: Value,
        method: &str,
        mut pos: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        let Some(spec) = pyrust_builtins::list::method_spec(method) else {
            return pyrust_builtins::list::call(method, &receiver, pos, kw);
        };
        spec.validate_keywords(!kw.is_empty())?;
        spec.validate_positional_arity(pos.len())?;
        // `list.insert(i, x)` / `list.pop([i])` accept any `__index__`
        // object as the index (CPython 3.12).  The receiver-only
        // `pyrust_builtins::list::call` cannot dispatch user dunders, so
        // resolve the index through the shared protocol here (#2022)
        // before delegating.
        if matches!(
            spec.method(),
            pyrust_builtins::list::Method::Insert | pyrust_builtins::list::Method::Pop
        ) && !pos.is_empty()
        {
            let idx = self.value_to_index(&pos[0], |v| {
                pyrust_core::type_err!(
                    "'{}' object cannot be interpreted as an integer",
                    pyrust_core::builtin_type_name(v)
                )
            })?;
            pos[0] = idx;
        }
        use pyrust_builtins::list::InterpreterMethod as ListMethod;
        match spec.interpreter_method() {
            Some(ListMethod::Sort) => self.list_sort_with_kwargs(&receiver, pos, kw),
            Some(ListMethod::Remove) => self.call_seq_remove(&receiver, pos),
            Some(ListMethod::Extend) => self.call_list_extend(&receiver, pos),
            Some(ListMethod::Index) => self.call_seq_index(&receiver, pos, "list"),
            Some(ListMethod::Count) => {
                // `count` scans the full sequence. Classifying first does
                // not introduce the early-hit pathology that `index`
                // avoids in `call_seq_index`.
                let needs_dispatch = pos
                    .first()
                    .map(|target| {
                        receiver
                            .list_with(|items| Self::seq_search_needs_dispatch(target, items))
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                if needs_dispatch {
                    let snapshot = receiver.list_with(|items| items.to_vec()).ok_or_else(|| {
                        pyrust_core::type_err!("list.index receiver is not a list")
                    })?;
                    self.call_seq_count(snapshot, &pos, "list")
                } else {
                    pyrust_builtins::list::call_resolved(spec.method(), &receiver, pos, kw)
                }
            }
            None => pyrust_builtins::list::call_resolved(spec.method(), &receiver, pos, kw),
        }
    }

    fn dispatch_tuple_container_method(
        &mut self,
        receiver: Value,
        method: &str,
        pos: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        pyrust_builtins::tuple::validate_method_keywords(method, !kw.is_empty())?;
        pyrust_builtins::tuple::validate_method_positional_arity(method, pos.len())?;
        let interpreter_method = pyrust_builtins::tuple::interpreter_method(method);
        if matches!(
            interpreter_method,
            Some(pyrust_builtins::tuple::InterpreterMethod::Index)
        ) {
            return self.call_seq_index(&receiver, pos, "tuple");
        }
        // Snapshot the tuple's items once so the `&[Value]` borrow does
        // not straddle the `&mut self` calls below.  Tuples are
        // immutable, so the snapshot is exact.
        let items: Vec<Value> = match receiver.kind() {
            ValueKind::Tuple(items) => items.to_vec(),
            _ => return Err(PyError::runtime("internal: expected tuple".to_string())),
        };
        if matches!(
            interpreter_method,
            Some(pyrust_builtins::tuple::InterpreterMethod::Count)
        ) {
            let needs_dispatch = pos
                .first()
                .map(|t| Self::seq_search_needs_dispatch(t, &items))
                .unwrap_or(false);
            if needs_dispatch {
                self.call_seq_count(items, &pos, "tuple")
            } else {
                pyrust_builtins::tuple::call_prevalidated(method, &items, pos)
            }
        } else {
            pyrust_builtins::tuple::call_prevalidated(method, &items, pos)
        }
    }

    /// `list.sort(...)` with possible `key=` / `reverse=` kwargs.  Both opcodes
    /// share this (#431).  The no-kwargs opcode passes an empty `kw`, reducing
    /// this to a plain `pyrust_builtins::list::call("sort", …)`.
    fn list_sort_with_kwargs(
        &mut self,
        receiver: &Value,
        pos: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        // `list.sort` is keyword-only in CPython 3.12 — `sort(*, key=None,
        // reverse=False)`.  Any positional arg is a TypeError, and the list is
        // left unchanged (#1949).  Enforced centrally so every dispatch site
        // (bound-method, subclass, …) inherits the rejection.
        if !pos.is_empty() {
            return Err(pyrust_core::type_err!(
                "sort() takes no positional arguments"
            ));
        }
        pyrust_builtins::list::validate_sort_keyword_count(kw.len())?;
        let mut key_arg: Option<Value> = None;
        let mut reverse_arg: Option<Value> = None;
        for (k, value) in kw {
            let name = match k {
                PyKey::Str(s) => s.as_str().unwrap_or(""),
                _ => "",
            };
            match pyrust_builtins::list::sort_keyword(name)? {
                pyrust_builtins::list::SortKeyword::Key => key_arg = Some(value.clone()),
                pyrust_builtins::list::SortKeyword::Reverse => reverse_arg = Some(value.clone()),
            }
        }
        // An explicit `key=None` means "no key function" (default comparison),
        // mirroring `sorted`/`min`/`max` (#1937).
        let key_fn = key_arg.filter(|v| !v.is_none());
        // CPython applies `bool(reverse)` to *any* object — a non-empty
        // list/str or an arbitrary object is truthy and reverses (issue #2126).
        // Compute it interpreter-side so a user `__bool__`/`__len__` is honoured,
        // matching `sorted()`.  This single computed flag is threaded into every
        // sort path below; the primitive fast path no longer re-parses `reverse`
        // (its receiver-only `extract_reverse` recognised only Bool/Int/Float).
        let reverse = match reverse_arg {
            Some(v) => self.truthy_value(&v.clone())?,
            None => false,
        };
        if let Some(key_fn_val) = key_fn {
            // Steal the list's items into an owned Vec so the receiver appears
            // empty during `key=` evaluation and comparisons, mirroring CPython's
            // `list_sort_impl` (`ob_item = NULL`).  Any mutation during sort
            // allocates into the empty Vec and is caught by `restore_sorted_list`
            // without panicking or losing the original elements.
            let saved: Vec<Value> = receiver
                .list_with_mut(std::mem::take)
                .ok_or_else(|| PyError::runtime("internal: expected list".to_string()))?;
            if saved.is_empty() {
                return Ok(Value::none());
            }
            let mut keys: Vec<Value> = Vec::with_capacity(saved.len());
            for item in &saved {
                let key_res = {
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    buf.push(ExpandedCallArg {
                        name: None,
                        value: item.clone(),
                    });
                    let r = self.call_function_expanded(key_fn_val.clone(), &buf);
                    buf.clear();
                    self.call_arg_buf = buf;
                    r
                };
                match key_res {
                    Ok(key_val) => keys.push(key_val),
                    Err(err) => {
                        receiver.list_with_mut(|items| *items = saved);
                        return Err(err);
                    }
                }
            }
            let mut keyed: Vec<(Value, Value)> = keys.into_iter().zip(saved).collect();
            let mut sort_err: Option<PyError> = None;
            if keyed.len() >= 2 {
                match pyrust_core::classify_sort(keyed.iter().map(|(k, _)| k)) {
                    pyrust_core::SortKind::AllInt => keyed.sort_by(|(ka, _), (kb, _)| {
                        let (lhs, rhs) = if reverse { (kb, ka) } else { (ka, kb) };
                        lhs.as_int().unwrap_or(0).cmp(&rhs.as_int().unwrap_or(0))
                    }),
                    pyrust_core::SortKind::AllStr => keyed.sort_by(|(ka, _), (kb, _)| {
                        let (lhs, rhs) = if reverse { (kb, ka) } else { (ka, kb) };
                        lhs.as_str().unwrap_or("").cmp(rhs.as_str().unwrap_or(""))
                    }),
                    pyrust_core::SortKind::HasInstance | pyrust_core::SortKind::General => {
                        keyed.sort_by(|(left_key, _), (right_key, _)| {
                            if sort_err.is_some() {
                                return std::cmp::Ordering::Equal;
                            }
                            let (lhs, rhs) = if reverse {
                                (right_key, left_key)
                            } else {
                                (left_key, right_key)
                            };
                            match self.richcmp_order(lhs, rhs) {
                                Ok(order) => order,
                                Err(error) => {
                                    sort_err = Some(error);
                                    std::cmp::Ordering::Equal
                                }
                            }
                        });
                    }
                }
            }
            let sorted_items: Vec<Value> = keyed.into_iter().map(|(_, value)| value).collect();
            return pyrust_builtins::list::restore_sorted_list(receiver, sorted_items, sort_err);
        }
        // No key.  Pre-scan: if any element is a user instance, comparisons
        // may dispatch `__lt__` (and the reflected `__gt__`), which the
        // interpreter-free `pyrust_builtins::list::call("sort", …)` cannot
        // reach — it would raise a spurious TypeError (#1925).  Route those
        // through the interpreter-aware `richcmp_order`, exactly as `sorted()`
        // does.  All-primitive lists keep the in-crate fast sort: no perf
        // regression on the common int/str/float case.
        let (sort_kind, len) = receiver
            .list_with(|items| (pyrust_core::classify_sort(items.iter()), items.len()))
            .ok_or_else(|| PyError::runtime("internal: expected list".to_string()))?;
        if len < 2 {
            return Ok(Value::none());
        }
        if matches!(
            sort_kind,
            pyrust_core::SortKind::AllInt | pyrust_core::SortKind::AllStr
        ) {
            // Primitive fast path: delegate to builtins with the already-resolved
            // `reverse` bool (issue #2126).  Going through `call("sort", …)` would
            // re-parse `reverse` via the receiver-only `extract_reverse`, which
            // recognises only Bool/Int/Float and silently drops a truthy
            // list/str/object — diverging from `sorted()` and CPython.
            return pyrust_builtins::list::sort_no_key(receiver, reverse);
        }
        // Steal the items so the list appears empty during comparisons (mirroring
        // CPython's `list_sort_impl` `ob_item = NULL`), detect any mutation during
        // sort, and always restore the elements even if `__lt__` raises.
        let mut saved: Vec<Value> = receiver
            .list_with_mut(std::mem::take)
            .ok_or_else(|| PyError::runtime("internal: expected list".to_string()))?;
        let mut sort_err: Option<PyError> = None;
        saved.sort_by(|a, b| {
            if sort_err.is_some() {
                return std::cmp::Ordering::Equal;
            }
            let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
            match self.richcmp_order(lhs, rhs) {
                Ok(ord) => ord,
                Err(e) => {
                    sort_err = Some(e);
                    std::cmp::Ordering::Equal
                }
            }
        });
        pyrust_builtins::list::restore_sorted_list(receiver, saved, sort_err)
    }
}
