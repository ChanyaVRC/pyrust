// Interpreter-aware adapters for built-in methods that consume iterables.
impl Interpreter {
    /// Extend a `ValueKind::List` (`target_list`) from `iterable`, matching
    /// CPython 3.12's `list_extend` / `list_inplace_concat` semantics:
    ///
    /// - Eager built-in containers (`list`/`tuple`/`str`/`range`/`dict`/`set`/
    ///   `bytes`) cannot run user code mid-iteration and take the bulk snapshot
    ///   fast path (which also keeps plain `a.extend(a)` / `a += a` safe).
    /// - A `list` subclass feeding itself (`sub.extend(sub)` / `sub += sub` —
    ///   detected by matching backing `value_id()`) materializes `iterable`
    ///   into a temporary snapshot first, matching CPython's `self == iterable`
    ///   guard: custom `__iter__` overrides are honoured, plain subclasses
    ///   terminate instead of looping forever, and an exception inside a
    ///   custom `__iter__` leaves `self` untouched.
    /// - Every other iterable streams element-by-element via `make_iterator` +
    ///   `call_next` + `list_push`, preserving partial progress when a lazy
    ///   iterable raises mid-way and allowing live interleaving when the
    ///   iterable mutates the receiver.
    pub(super) fn extend_list_from_iterable(
        &mut self,
        target_list: &Value,
        iterable: &Value,
    ) -> Result<()> {
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
            let snapshot = self.collect_iterable(iterable)?;
            target_list.list_extend(snapshot)?;
            return Ok(());
        }
        // Self-identity guard for list subclasses (`LSub.extend(self)` /
        // `LSub += self`): `target_list` is the backing `ValueKind::List`, so a
        // `PyInstance` whose backing shares `target_list.value_id()` is feeding
        // itself. Materialize first so we never iterate a live native cursor
        // over the same Vec we are pushing to.
        if let Some(target_id) = target_list.value_id()
            && builtin_data_backing(iterable).is_some_and(|b| b.value_id() == Some(target_id))
        {
            let snapshot = self.collect_iterable(iterable)?;
            target_list.list_extend(snapshot)?;
            return Ok(());
        }
        let iterator = match self.mapping_proxy_iterator_with_length_hint(iterable)? {
            Some(iterator) => iterator,
            None => make_iterator(self, iterable)?,
        };
        loop {
            match self.call_next(&iterator, None) {
                Ok(item) => target_list.list_push(item)?,
                Err(ref e) if is_stop_iteration_error(e) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Interpreter-aware `list.extend(iterable)` (#2522, #2989).
    fn call_list_extend(&mut self, receiver: &Value, mut args: Vec<Value>) -> Result<Value> {
        if args.len() != 1 {
            return Err(pyrust_core::type_err!(
                "list.extend() takes exactly one argument ({} given)",
                args.len()
            ));
        }
        let iterable = args.pop().unwrap();
        self.extend_list_from_iterable(receiver, &iterable)?;
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
