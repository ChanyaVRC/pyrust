/// Built-in container receivers handled by every method-call entry path.
///
/// Keeping this as a type prevents the VM/call boundary from passing magic
/// numeric tags and makes adding a new fast-path receiver an exhaustive-match
/// change.
#[derive(Clone, Copy)]
pub(super) enum BuiltinContainerKind {
    List,
    Dict,
    Tuple,
    Str,
    Bytes,
    Set,
}

impl BuiltinContainerKind {
    #[inline(always)]
    pub(super) fn classify(value: Option<&Value>) -> Option<Self> {
        match value?.kind() {
            ValueKind::List(_) => Some(Self::List),
            ValueKind::Dict(_) => Some(Self::Dict),
            ValueKind::Tuple(_) => Some(Self::Tuple),
            ValueKind::Str(_) => Some(Self::Str),
            ValueKind::Bytes(_) => Some(Self::Bytes),
            ValueKind::Set(_) => Some(Self::Set),
            _ => None,
        }
    }

    pub(super) fn from_type_name(type_name: &str) -> Option<Self> {
        match type_name {
            "list" => Some(Self::List),
            "dict" => Some(Self::Dict),
            "tuple" => Some(Self::Tuple),
            "str" => Some(Self::Str),
            "bytes" => Some(Self::Bytes),
            "set" => Some(Self::Set),
            _ => None,
        }
    }

    /// Whether this receiver kind owns `method` in the direct instance-method
    /// dispatcher.
    ///
    /// `bytes.fromhex` is a classmethod and `bytes.maketrans` is a static
    /// helper; CPython also exposes both through an instance, but their binding
    /// semantics belong to the primitive-class descriptor path.  Keep those
    /// names out of the fused exact-bytes path instead of duplicating their
    /// class/static binding rules here.
    #[inline(always)]
    pub(super) fn supports_direct_method(self, method: &str) -> bool {
        !matches!(self, Self::Bytes) || pyrust_builtins::bytes::has_method(method)
    }
}

impl Interpreter {
    /// Dispatch a builtin method recovered from the generic `CallMethod`
    /// inline cache.
    ///
    /// The cache owns class/version validation only. Exact primitive method
    /// names and backing-storage policy stay in this semantic domain, so the
    /// fast-path module does not decode builtin sentinels or
    /// `__builtin_data__` itself.
    pub(super) fn try_dispatch_cached_builtin_method(
        &mut self,
        unbound: &Value,
        instance: &Rc<RefCell<pyrust_core::PyInstance>>,
        args: &[Value],
    ) -> Option<Result<Value>> {
        let ValueKind::BuiltinFunction(function_name) = unbound.kind() else {
            return None;
        };
        let (primitive_type, primitive_method) = function_name
            .split_once('.')
            .filter(|(type_name, _)| matches!(*type_name, "dict" | "list" | "set"))?;
        let backing = instance_builtin_data(instance)?;
        let ordered = is_ordered_dict_class_or_subclass(&instance.borrow().class);
        Some(self.dispatch_backing_primitive_method(
            primitive_type,
            primitive_method,
            backing,
            ordered,
            args.to_vec(),
        ))
    }

    /// Shared dispatch body for method calls on the six classified builtin
    /// container types (`list` / `dict` / `tuple` / `str` / `bytes` / `set`). Both
    /// `Insn::CallMethod` (no kwargs) and `Insn::CallMethodExpanded`
    /// (kwargs / unpacking) route here once they have materialised the
    /// receiver, positional args, and keyword map — so the dispatch decision
    /// lives in exactly one place (#431).
    ///
    /// The "needs interpreter" vs "needs Rc backing" vs "pure builtin"
    /// decision is route-driven via the `pyrust-builtins::<type>` modules
    /// (`list::interpreter_method`, `dict::view_method`,
    /// `string::interpreter_method`) — the single source of truth documented
    /// in `crates/pyrust-builtins/README.md`.
    pub(super) fn dispatch_builtin_container_method(
        &mut self,
        kind: BuiltinContainerKind,
        receiver: Value,
        method: &str,
        mut pos: Vec<Value>,
        kw: &PyDict,
        ordered_mapping: bool,
    ) -> Result<Value> {
        // All entry paths — bytecode method opcodes, bound-method values, and
        // unbound type descriptors — converge here.  Keeping `__iter__` here
        // avoids a fourth, name-based routing decision in each caller.
        if method == "__iter__" {
            reject_kwargs!(kw, "wrapper __iter__");
            if !pos.is_empty() {
                return Err(pyrust_core::type_err!(
                    "expected 0 arguments, got {}",
                    pos.len()
                ));
            }
            let iter_arg = ExpandedCallArg {
                name: None,
                value: receiver,
            };
            let dispatch =
                crate::builtin_registry::lookup("iter").expect("iter must be in the registry");
            return dispatch(self, &[iter_arg]);
        }

        // Issue #2151: object-protocol methods (`__sizeof__`/`__dir__`/
        // `__reduce__`/`__reduce_ex__`) called directly via the method-call
        // opcode on a container.  Handled here (the per-type `call` below would
        // leak a RuntimeError) so `[1].__dir__()` returns a list.
        if method.starts_with("__") {
            if crate::interpreter::is_object_protocol_method(&receiver, method) {
                if !kw.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "{}() takes no keyword arguments",
                        method
                    ));
                }
                return Ok(self.object_protocol_method_result(method, &receiver));
            }
            // Issue #2191: `__format__` called directly via the method-call
            // opcode on a classified container (`"hi".__format__('>5')`,
            // `[1,2].__format__('')`).  Route through `apply_format_spec` so the
            // result matches `format(receiver, spec)` — otherwise the per-type
            // `call` below leaks a RuntimeError (`'str' object has no attribute
            // '__format__'`).  Gated under the `__` prefix so the common
            // method-name path (`.append`, `.upper`, …) is untouched.
            if method == "__format__" {
                if !kw.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "{}.__format__() takes no keyword arguments",
                        format_dunder_owner(&receiver)
                    ));
                }
                let spec = format_dunder_spec_arg(&receiver, &pos)?;
                return apply_format_spec(&receiver, spec);
            }
        }
        // Issue #1909: container/sequence protocol dunders called directly via
        // the `obj.__getitem__(i)` method-call opcode (not through the
        // bound-method value).  Route through the shared dispatcher so the
        // result matches the operator behaviour.  `__iter__` is handled by the
        // callers before this point.
        if is_container_protocol_dunder_name(method) {
            let type_name = pyrust_core::builtin_type_name(&receiver);
            if is_protocol_dunder(&type_name, method) {
                if !kw.is_empty() {
                    // Issue #2398: CPython's keyword-rejection wording depends on
                    // whether the slot is a named method-wrapper or an anonymous
                    // slot wrapper (issue #2291).
                    return Err(if is_named_protocol_wrapper(method, &type_name) {
                        pyrust_core::type_err!("{type_name}.{method}() takes no keyword arguments")
                    } else {
                        pyrust_core::type_err!("wrapper {method}() takes no keyword arguments")
                    });
                }
                return self.dispatch_builtin_protocol_dunder(method, receiver, pos);
            }
            // Issue #2070: `__hash__` on an unhashable built-in (list/dict/set/
            // bytearray) resolves to `None` in CPython, so `[1].__hash__()`
            // raises `'NoneType' object is not callable` — not `AttributeError`.
            if method == "__hash__" && matches!(&*type_name, "list" | "dict" | "set" | "bytearray")
            {
                return Err(pyrust_core::type_err!("'NoneType' object is not callable"));
            }
            // A protocol-dunder name that is valid for *other* types but not
            // this one (e.g. `set().__getitem__`, `"".__setitem__`) is
            // genuinely absent: raise a proper `AttributeError` rather than
            // letting the per-type `call` leak a `RuntimeError` (issue #1909).
            return Err(PyError::attribute_error(
                format!("'{type_name}' object has no attribute '{method}'"),
                Some(method.to_string()),
                Some(receiver),
            ));
        }
        match kind {
            BuiltinContainerKind::List => {
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
                                    .list_with(|items| {
                                        Self::seq_search_needs_dispatch(target, items)
                                    })
                                    .unwrap_or(true)
                            })
                            .unwrap_or(false);
                        if needs_dispatch {
                            let snapshot =
                                receiver.list_with(|items| items.to_vec()).ok_or_else(|| {
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
            BuiltinContainerKind::Dict => {
                let Some(spec) = pyrust_builtins::dict::method_spec(method) else {
                    return pyrust_builtins::dict::call(method, &receiver, pos, kw);
                };
                spec.validate_keywords(!kw.is_empty())?;
                spec.validate_positional_arity(pos.len())?;
                let resolved = spec.method();
                if spec.view_method().is_some() {
                    // Live-view construction belongs to the interpreter adapter:
                    // it owns both the backing Rc and OrderedDict provenance.
                    return Self::dict_view_for_backing(
                        &receiver,
                        resolved.name(),
                        ordered_mapping,
                    );
                }
                if resolved == pyrust_builtins::dict::Method::Clear
                    && ordered_mapping
                    && let Some(id) = receiver.value_id()
                {
                    let prelen = receiver.dict_with(|d| d.len()).unwrap_or(0);
                    pyrust_builtins::ordered_mapping::note_clear(id, prelen);
                }
                self.call_dict_method_resolved(resolved, receiver, pos, kw)
            }
            BuiltinContainerKind::Tuple => {
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
                    _ => return Err(PyError::Runtime("internal: expected tuple".to_string())),
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
            BuiltinContainerKind::Str => {
                if let Some(template_method) = pyrust_builtins::string::interpreter_method(method) {
                    return self.str_template_method(&receiver, template_method, pos, kw);
                }
                if kw.is_empty() {
                    self.call_str_method(method, receiver, pos)
                } else if let Some(binder) = pyrust_builtins::string::keyword_binder(method) {
                    let mut pos = pos;
                    str_merge_kwargs(binder, &mut pos, kw.clone())?;
                    self.call_str_method(method, receiver, pos)
                } else {
                    pyrust_builtins::string::validate_method_keywords(method, true)?;
                    self.call_str_method(method, receiver, pos)
                }
            }
            BuiltinContainerKind::Bytes => {
                pyrust_builtins::bytes::validate_method_keywords(method, !kw.is_empty())?;
                pyrust_builtins::bytes::validate_method_positional_arity(method, pos.len())?;
                if method == "join" {
                    self.call_bytes_join(receiver, pos)
                } else {
                    self.call_bytes_method_with_protocols_prevalidated(method, &receiver, pos, kw)
                }
            }
            BuiltinContainerKind::Set => {
                let Some(spec) = pyrust_builtins::set::method_spec(method) else {
                    return pyrust_builtins::set::call(method, &receiver, pos);
                };
                spec.validate_keywords(!kw.is_empty())?;
                spec.validate_positional_arity(pos.len())?;
                self.call_set_method_resolved(spec.method(), receiver, pos)
            }
        }
    }

    /// Compute the result of an object-protocol method call (#2151) on a
    /// built-in data value whose receiver is already bound.  Single source of
    /// truth shared by the bound-method dispatch and the container method-call
    /// fast path; mirrors the `object.__sizeof__`/`__dir__`/`__reduce*` and
    /// `NoneType.__bool__` registry handlers.  `method` must satisfy
    /// `is_object_protocol_method(receiver, method)`.
    fn object_protocol_method_result(&self, method: &str, receiver: &Value) -> Value {
        match method {
            // Implementation-specific size; tests assert the int type only.
            "__sizeof__" => Value::int(std::mem::size_of::<Value>() as i64),
            "__dir__" => {
                let mut names = dir_names(receiver);
                names.sort();
                names.dedup();
                Value::list(names.into_iter().map(Value::string).collect())
            }
            // A tuple of the correct shape (`(class, ())`); pyrust does not
            // model copyreg, so the exact pickle reduction is not reproduced.
            "__reduce__" | "__reduce_ex__" => {
                Value::tuple(vec![value_class(receiver), Value::tuple(Vec::new())])
            }
            "__bool__" => Value::bool_(false),
            // `__getstate__()` returns None for objects with no instance state.
            "__getstate__" => Value::none(),
            _ => unreachable!("is_object_protocol_method guard"),
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
            // Compute keys via the interpreter, then delegate sorting to builtins.
            let items_snapshot: Vec<Value> = receiver
                .list_with(|items| items.to_vec())
                .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
            let mut keys: Vec<Value> = Vec::with_capacity(items_snapshot.len());
            for item in &items_snapshot {
                let key_val = {
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    buf.push(ExpandedCallArg {
                        name: None,
                        value: item.clone(),
                    });
                    let r = self.call_function_expanded(key_fn_val.clone(), &buf);
                    self.call_arg_buf = buf;
                    r?
                };
                keys.push(key_val);
            }
            if matches!(
                pyrust_core::classify_sort(keys.iter()),
                pyrust_core::SortKind::AllInt | pyrust_core::SortKind::AllStr
            ) {
                return pyrust_builtins::list::sort_with_precomputed_keys(receiver, keys, reverse);
            }
            let mut keyed: Vec<(Value, Value)> = keys.into_iter().zip(items_snapshot).collect();
            let mut sort_err: Option<PyError> = None;
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
            if let Some(error) = sort_err {
                return Err(error);
            }
            receiver.list_with_mut(|items| {
                *items = keyed.into_iter().map(|(_, value)| value).collect();
            });
            return Ok(Value::none());
        }
        // No key.  Pre-scan: if any element is a user instance, comparisons
        // may dispatch `__lt__` (and the reflected `__gt__`), which the
        // interpreter-free `pyrust_builtins::list::call("sort", …)` cannot
        // reach — it would raise a spurious TypeError (#1925).  Route those
        // through the interpreter-aware `richcmp_order`, exactly as `sorted()`
        // does.  All-primitive lists keep the in-crate fast sort: no perf
        // regression on the common int/str/float case.
        let sort_kind = receiver
            .list_with(|items| pyrust_core::classify_sort(items.iter()))
            .unwrap_or(pyrust_core::SortKind::General);
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
        // Snapshot the items so the comparator (which may re-enter the same
        // list via user `__lt__`) does not straddle the receiver's borrow.
        // Write the sorted result back inside a `list_with_mut` window, mirroring
        // `pyrust_builtins::list::sort_by_cmp`.
        let mut snapshot: Vec<Value> = receiver
            .list_with(|items| items.clone())
            .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
        let mut sort_err: Option<PyError> = None;
        snapshot.sort_by(|a, b| {
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
        if let Some(e) = sort_err {
            return Err(e);
        }
        receiver.list_with_mut(|items| *items = snapshot);
        Ok(Value::none())
    }

    /// `str.format` / `str.format_map` / `str.maketrans` — the templating
    /// methods that must run in the interpreter rather than
    /// `pyrust_builtins::string::call`.  Both opcodes share this (#431).
    fn str_template_method(
        &mut self,
        receiver: &Value,
        method: pyrust_builtins::string::InterpreterMethod,
        pos: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        use pyrust_builtins::string::InterpreterMethod;
        pyrust_builtins::string::validate_interpreter_method_keywords(method, !kw.is_empty())?;
        match method {
            InterpreterMethod::Format => {
                pyrust_builtins::string::validate_interpreter_method_positional_arity(
                    method,
                    pos.len(),
                )?;
                let mut keyword: Vec<(&str, Value)> = Vec::with_capacity(kw.len());
                for (k, v) in kw {
                    if let PyKey::Str(name) = k {
                        keyword.push((name.as_str().unwrap_or(""), v.clone()));
                    }
                }
                let template = receiver
                    .as_str()
                    .ok_or_else(|| PyError::Runtime("internal: expected str".to_string()))?;
                self.format_str_template(template, &pos, &keyword)
            }
            InterpreterMethod::FormatMap => {
                pyrust_builtins::string::validate_interpreter_method_positional_arity(
                    method,
                    pos.len(),
                )?;
                if pos.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "str.format_map() takes exactly one argument ({} given)",
                        pos.len()
                    ));
                }
                let mapping = pos.into_iter().next().unwrap();
                let template = receiver
                    .as_str()
                    .ok_or_else(|| PyError::Runtime("internal: expected str".to_string()))?
                    .to_string();
                self.format_str_template_map(&template, mapping)
            }
            // `maketrans` is a staticmethod on str: the receiver is discarded
            // and the call is forwarded to str_maketrans exactly like
            // `str.maketrans(...)` would be.
            InterpreterMethod::MakeTrans => {
                pyrust_builtins::string::validate_interpreter_method_positional_arity(
                    method,
                    pos.len(),
                )?;
                pyrust_builtins::string::str_maketrans(&pos)
            }
        }
    }
    /// Dispatch a method call on the backing primitive of a `list`/`dict`/`set`
    /// subclass instance (#976).  The cached method is a
    /// `BuiltinFunction("<type>.<method>")` and `backing` is its
    /// `__builtin_data__` value; route to the same per-type path the bare
    /// container would take.  `index`/`count` on a list backing may fire user
    /// `__eq__`, so they go through the interpreter-aware sequence helpers.
    fn dispatch_backing_primitive_method(
        &mut self,
        prim_type: &str,
        prim_method: &str,
        backing: Value,
        ordered: bool,
        args: Vec<Value>,
    ) -> Result<Value> {
        // Issue #1909: container protocol dunders on a `list`/`dict`/`set`
        // *subclass* instance (`MyList().__len__()`) operate on the backing
        // primitive — route through the shared dispatcher so they match the
        // plain-primitive form rather than leaking a `RuntimeError` from the
        // per-type `call`.
        if prim_method.starts_with("__") && is_protocol_dunder(prim_type, prim_method) {
            return self.dispatch_builtin_protocol_dunder(prim_method, backing, args);
        }
        let kind = BuiltinContainerKind::from_type_name(prim_type)
            .unwrap_or_else(|| unreachable!("bad backing primitive type {prim_type}"));
        self.dispatch_builtin_container_method(
            kind,
            backing,
            prim_method,
            args,
            &PyDict::default(),
            ordered,
        )
    }

    /// Build a live, `Rc`-shared `dict_views` view (`keys`/`values`/`items`)
    /// from a dict-subclass instance's backing dict (issue #2436).  Sharing the
    /// backing `IndexMap` `Rc` — instead of the snapshot `list` that
    /// `dict::call` materialises when it cannot see the `Rc` — keeps the view
    /// live and routes its iteration through the size-mutation guard.  `ordered`
    /// tags OrderedDict-backed views so the guard reports the OrderedDict
    /// wording.  ONE decision shared by the slow `BkKind::Dict` dispatch and the
    /// `dispatch_backing_primitive_method` inline-cache fast path (the two were
    /// the #2324 duplicate-decision drift that left cached subclass views
    /// unguarded).
    pub(crate) fn dict_view_for_backing(
        backing: &Value,
        method: &str,
        ordered: bool,
    ) -> Result<Value> {
        let rc = backing
            .get_dict_rc()
            .ok_or_else(|| PyError::Runtime("internal: expected dict backing".to_string()))?
            .clone();
        Ok(match method {
            "keys" => pyrust_builtins::dict_views::dict_keys_tagged(rc, ordered),
            "values" => pyrust_builtins::dict_views::dict_values_tagged(rc, ordered),
            _ => pyrust_builtins::dict_views::dict_items_tagged(rc, ordered),
        })
    }
}
