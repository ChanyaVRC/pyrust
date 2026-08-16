// Interpreter-aware adapters for built-in methods that consume iterables.
impl Interpreter {
    /// Interpreter-aware `list.extend(iterable)` (#2522).
    ///
    /// The receiver-only `pyrust_builtins::mutable_sequence::extend` materialises
    /// its argument through `iter_values_via_registry` → the free `iter_values`,
    /// which can only drain a `NativeIterFrame` and rejects every other generator
    /// state (`map`/`filter`/genexpr/user generators) with a `TypeError`. Driving
    /// those requires the interpreter, so route `extend` through `collect_iterable`
    /// — the same path `list(iterable)` uses — before touching the receiver.
    ///
    /// Materialising the snapshot before mutating the receiver preserves the
    /// aliasing fix from #414/#427 (`a.extend(a)` cannot produce a simultaneous
    /// borrow): `collect_iterable` reads the argument to completion first.
    fn call_list_extend(&mut self, receiver: &Value, mut args: Vec<Value>) -> Result<Value> {
        if args.len() != 1 {
            return Err(pyrust_core::type_err!(
                "list.extend() takes exactly one argument ({} given)",
                args.len()
            ));
        }
        let iterable = args.pop().unwrap();
        // Fast path for eager built-in containers (list/tuple/str/range/dict/
        // set/bytes). Iterating these never runs user code, so no exception can
        // be raised mid-iteration and there is no partial progress to preserve —
        // CPython's incremental semantics are indistinguishable from a bulk
        // extend here. `collect_iterable` reads the argument to completion before
        // the receiver is touched, which also keeps the self-extend aliasing
        // guarantee (`a.extend(a)`). Skipping the per-element `iter()`/
        // `call_next` loop avoids a ~6.7x regression on the common
        // `list.extend(list)` path. (frozenset / bytearray are `BuiltinObject`,
        // which also backs lazy map/filter/zip iterators, so they take the
        // incremental path below — still correct, just rarer as an argument.)
        if matches!(
            iterable.kind(),
            ValueKind::List(_)
                | ValueKind::Tuple(_)
                | ValueKind::Str(_)
                | ValueKind::Range { .. }
                | ValueKind::BigRange { .. }
                | ValueKind::Dict(_)
                | ValueKind::Set(_)
                | ValueKind::Bytes(_)
        ) {
            let snapshot = self.collect_iterable(&iterable)?;
            receiver.list_extend(snapshot)?;
            return Ok(Value::none());
        }
        // CPython's list.extend appends incrementally and keeps partial
        // progress when the iterator raises. An object-backed mappingproxy
        // acquires the owner's iterator and probes the proxy length hint before
        // the first item; every other lazy iterable uses the ordinary path.
        let iterator = match self.mapping_proxy_iterator_with_length_hint(&iterable)? {
            Some(iterator) => iterator,
            None => make_iterator(self, &iterable)?,
        };
        loop {
            match self.call_next(&iterator, None) {
                Ok(item) => receiver.list_push(item)?,
                Err(ref e) if is_stop_iteration_error(e) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(Value::none())
    }

    /// Interpreter-aware pre-pass for `bytearray.extend(iterable)` (#2532).
    ///
    /// The receiver-only `bytearray::bytes_from_value` materialises its argument
    /// through `iter_values_via_registry`, which can only drain a
    /// `NativeIterFrame` and rejects every other generator state
    /// (`map`/`filter`/genexpr/user generators) with `can't extend bytearray with
    /// <type>`. Driving those needs the interpreter, so when the single argument
    /// is a lazy `Generator` — or a user-defined iterable `PyInstance`
    /// (`__iter__`/`__next__` or the legacy `__getitem__` protocol, #2534) —
    /// materialise it to completion via `collect_iterable` (the same path
    /// `list(iterable)` uses) and replace it with a `List`, which
    /// `bytes_from_value` already handles, including the per-element
    /// `range(0, 256)` / non-int validation.
    ///
    /// Other arguments are left untouched so the ops table keeps its CPython
    /// wording (`can't extend bytearray with int` for a non-iterable, the
    /// per-character `'str' object cannot be interpreted as an integer` for a str,
    /// etc.). A *non-iterable* `PyInstance` is likewise left alone so the ops
    /// table reports `can't extend bytearray with <type>` rather than the
    /// `'X' object is not iterable` `collect_iterable` would raise. Materialising
    /// the snapshot before the receiver is mutated also preserves the self-extend
    /// aliasing guarantee.
    pub(super) fn prepare_bytearray_extend_args(
        &mut self,
        mut args: Vec<Value>,
    ) -> Result<Vec<Value>> {
        pyrust_builtins::bytearray::validate_method_positional_arity("extend", args.len())?;
        if args.len() != 1 {
            return Err(pyrust_core::type_err!(
                "bytearray.extend() takes exactly one argument ({} given)",
                args.len()
            ));
        }
        let (needs_collect, proxy_length_hint) = match args[0].kind() {
            ValueKind::Generator(_) => (true, false),
            ValueKind::BuiltinObject { ops, .. }
                if pyrust_builtins::mapping_proxy::is_object_proxy_ops(ops) =>
            {
                (true, true)
            }
            // A user-defined PyInstance is collected only when it is actually
            // iterable; a plain object falls through so the ops table picks the
            // canonical `can't extend bytearray with <type>` message (#2534).
            // Gate strictly on the iteration protocols (`__iter__` / the legacy
            // `__getitem__`): a subclass of an *iterable* builtin (list/dict/
            // bytes/set/…) inherits one of these on its class, while a subclass
            // of a non-iterable builtin (int/float/complex/bool) inherits
            // neither and must fall through to the ops-table message — collecting
            // it would surface `'int' object is not iterable` instead of
            // `can't extend bytearray with <type>` (matching CPython).
            ValueKind::PyInstance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                (
                    lookup_class_attr(&class, "__iter__").is_some()
                        || lookup_class_attr(&class, "__getitem__").is_some(),
                    false,
                )
            }
            _ => (false, false),
        };
        if needs_collect {
            let snapshot = if proxy_length_hint {
                self.collect_mapping_proxy_with_length_hint(&args[0])?
                    .expect("object mappingproxy classification")
            } else {
                self.collect_iterable(&args[0])?
            };
            args[0] = Value::list(snapshot);
        }
        Ok(args)
    }

    /// Issue #2538: materialise a lazy `bytearray.join` iterable (map/filter/
    /// genexpr/user `__iter__`) before the receiver-only ops table sees it.
    /// The ops table can only drain a `NativeIterFrame`, so anything that is
    /// not already a `List`/`Tuple` is collected through the interpreter,
    /// mirroring `call_bytes_join`.  Elements are coerced from bytes-subclass /
    /// bytearray to a real `Bytes` value (#1928) after collection.
    pub(super) fn prepare_bytearray_join_args(
        &mut self,
        mut args: Vec<Value>,
    ) -> Result<Vec<Value>> {
        pyrust_builtins::bytearray::validate_method_positional_arity("join", args.len())?;
        if args.len() != 1 {
            return Err(pyrust_core::type_err!(
                "bytearray.join() takes exactly one argument ({} given)",
                args.len()
            ));
        }
        let needs_collect = !matches!(args[0].kind(), ValueKind::List(_) | ValueKind::Tuple(_));
        if needs_collect {
            let items = collect_join_iterable(self, &args[0])?;
            args[0] = Value::list(items.into_iter().map(coerce_bytes_subclass_arg).collect());
        } else {
            // List/Tuple fast path may hold bytes-subclass / bytearray items;
            // CPython joins them by their bytes value (#1928).
            args[0] =
                coerce_bytes_subclass_join_iterable(std::mem::replace(&mut args[0], Value::none()));
        }
        Ok(args)
    }
}
