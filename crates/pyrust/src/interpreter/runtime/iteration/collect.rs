// Interpreter-aware materialisation of arbitrary Python iterables.

impl Interpreter {
    /// Materialise an iterable for the `list()` / `tuple()` constructors.
    ///
    /// CPython asks a reverse sequence iterator for a length hint before
    /// draining it. For a user `__len__` + `__getitem__` sequence that hint
    /// re-checks the live `__len__` slot, but it does not change the reverse
    /// iterator's fixed initial index. Keep that constructor-only callback
    /// here rather than adding it to every consumer of `collect_iterable`
    /// (`sum`, `join`, and other consumers do not all request a hint).
    pub(crate) fn collect_sequence_constructor_iterable(
        &mut self,
        val: &Value,
    ) -> Result<Vec<Value>> {
        self.probe_reverse_getitem_length_hint(val)?;
        self.collect_iterable(val)
    }

    fn probe_reverse_getitem_length_hint(&mut self, val: &Value) -> Result<()> {
        let ValueKind::Generator(state_rc) = val.kind() else {
            return Ok(());
        };
        let snapshot = {
            let state = state_rc
                .try_borrow()
                .map_err(|_| pyrust_core::value_err!("generator already executing"))?;
            let Some(iterator) = state.downcast_ref::<GetItemIter>() else {
                return Ok(());
            };
            if iterator.step >= 0 || iterator.exhausted || iterator.remaining == Some(0) {
                return Ok(());
            }
            iterator
                .length_method
                .as_ref()
                .map(|method| (iterator.obj.clone(), method.clone()))
        };
        let Some((object, length_method)) = snapshot else {
            return Ok(());
        };
        let length = match invoke_class_method(self, length_method, object, &[]) {
            Ok(length) => length,
            // `reversed.__length_hint__` clears a TypeError from the live
            // sequence length and reports zero. Other exceptions propagate.
            Err(error) if error.class_name_is("TypeError") => return Ok(()),
            Err(error) => return Err(error),
        };
        match self.normalize_len_result(&length) {
            Ok(_) => Ok(()),
            // The reverse iterator's C-level length hint treats an invalid
            // `__len__` result like a cleared TypeError and zero. Keep that
            // consumer-specific policy here while sharing protocol dispatch
            // and result validation with `len()` and truth-value testing.
            Err(error) if error.class_name_is("TypeError") => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Collect all values from an iterable (including generators) into a Vec.
    pub(crate) fn collect_iterable(&mut self, val: &Value) -> Result<Vec<Value>> {
        // A coroutine (`async def`, issue #1039) is not iterable — `list(coro)`,
        // `tuple(coro)`, unpacking, etc. all raise TypeError, matching CPython.
        if is_coroutine_value(val) {
            return Err(pyrust_core::type_err!("'coroutine' object is not iterable"));
        }
        if let ValueKind::Generator(state_rc) = val.kind() {
            let state_rc = Rc::clone(state_rc);

            // Fast path: NativeIterFrame — drain remaining items in one shot.
            //
            // `try_borrow_mut`: the cell is mutably checked out while the
            // generator's own body executes, so a re-entrant `list(g)` /
            // `sum(g)` / unpack from inside `g` raises CPython's
            // `ValueError: generator already executing` instead of panicking
            // on the re-borrow (issue #2285).
            let mut probe = state_rc
                .try_borrow_mut()
                .map_err(|_| pyrust_core::value_err!("generator already executing"))?;
            if let Some(native) = probe.downcast_mut::<NativeIterFrame>() {
                return native.drain_remaining();
            }

            // Single-probe dispatch on the concrete iterator-state type for
            // everything else; see `call_next` for the rationale (#2315).
            let tid = {
                let any_ref: &dyn std::any::Any = &**probe;
                any_ref.type_id()
            };
            drop(probe);
            use std::any::TypeId;

            // A `GenDriving` placeholder means the frame is checked out by
            // the gen-drive trampoline (#2253) — the generator is executing,
            // so collecting it re-entrantly raises the already-executing
            // error (issue #2285).
            if tid == TypeId::of::<GenDriving>() {
                return Err(pyrust_core::value_err!("generator already executing"));
            }

            // GeneratorFrame path: drive the generator to exhaustion.
            if tid == TypeId::of::<GeneratorFrame>() {
                let mut items = Vec::new();
                loop {
                    let mut borrow = state_rc
                        .try_borrow_mut()
                        .map_err(|_| pyrust_core::value_err!("generator already executing"))?;
                    if borrow.is::<GenDriving>() {
                        return Err(pyrust_core::value_err!("generator already executing"));
                    }
                    let frame = borrow
                        .downcast_mut::<GeneratorFrame>()
                        .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
                    if frame.done {
                        break;
                    }
                    match self.resume_generator(frame) {
                        Ok(yielded) => {
                            drop(borrow);
                            items.push(yielded);
                        }
                        Err(ref error) if is_stop_iteration_error(error) => break,
                        Err(error) => return Err(error),
                    }
                }
                return Ok(items);
            }

            // Concrete lazy iterators share their one-step implementation with
            // `call_next` and `ForIter`.
            macro_rules! collect_steps {
                ($step:ident) => {{
                    let mut items = Vec::new();
                    while let Some(value) = self.$step(&state_rc)? {
                        items.push(value);
                    }
                    return Ok(items);
                }};
            }

            if tid == TypeId::of::<CallableIter>() {
                let mut items = Vec::new();
                loop {
                    match self.step_callable_iter(&state_rc) {
                        Ok(Some(value)) => items.push(value),
                        Ok(None) => break,
                        // StopIteration raised by the callable stops iteration
                        // exactly like reaching the sentinel.
                        Err(ref error) if is_stop_iteration_error(error) => break,
                        Err(error) => return Err(error),
                    }
                }
                return Ok(items);
            }
            if tid == TypeId::of::<GetItemIter>() {
                collect_steps!(step_getitem_iter);
            }
            if tid == TypeId::of::<MapIter>() {
                collect_steps!(step_map_iter);
            }
            if tid == TypeId::of::<FilterIter>() {
                collect_steps!(step_filter_iter);
            }
            if tid == TypeId::of::<ProviderIterator>() {
                collect_steps!(step_provider_iterator);
            }
            if tid == TypeId::of::<EnumerateIter>() {
                collect_steps!(step_enumerate_iter);
            }
            if tid == TypeId::of::<ZipIter>() {
                collect_steps!(step_zip_iter);
            }
            if tid == TypeId::of::<RangeIter>() {
                collect_steps!(step_range_iter);
            }
            if tid == TypeId::of::<BigRangeIter>() {
                collect_steps!(step_bigrange_iter);
            }

            Err(PyError::Runtime("invalid generator state".to_string()))
        } else if let ValueKind::PyInstance(instance) = val.kind() {
            // CPython fallback order is `__iter__` first, then the legacy
            // sequence-iter protocol via `__getitem__`. Having only `__next__`
            // does not make a class iterable.
            let instance = Rc::clone(instance);
            let class = Rc::clone(&instance.borrow().class);
            let user_iter = lookup_class_attr(&class, "__iter__")
                .filter(|method| !is_inherited_builtin_iter_sentinel(&class, method));

            // A primitive subclass without a user override iterates its backing
            // value. Preserve the carrier class name in not-iterable errors.
            if user_iter.is_none()
                && let Some(backing) = builtin_data_backing(val)
            {
                return self.collect_iterable(&backing).map_err(|error| {
                    if error.class_name_is("TypeError") {
                        pyrust_core::type_err!("'{}' object is not iterable", class.borrow().name)
                    } else {
                        error
                    }
                });
            }

            let iterator = if let Some(method) = user_iter {
                invoke_class_method(self, method, Value::py_instance(Rc::clone(&instance)), &[])?
            } else if lookup_class_attr(&class, "__getitem__").is_some() {
                self.make_getitem_iter(Rc::clone(&instance))?
            } else {
                return Err(pyrust_core::type_err!(
                    "'{}' object is not iterable",
                    class.borrow().name
                ));
            };
            let iterator = validate_iterator_result(iterator)?;

            self.collect_iterator(&iterator)
        } else if let ValueKind::PyClass(class) = val.kind()
            && let Some(iter_method) = metaclass_dunder(class, "__iter__")
        {
            let iterator = invoke_class_method(self, iter_method, val.clone(), &[])?;
            let iterator = validate_iterator_result(iterator)?;
            self.collect_iterator(&iterator)
        } else {
            iter_values(val)
        }
    }

    /// Drain a value that has already passed iterator acquisition/validation.
    ///
    /// Unlike [`Interpreter::collect_iterable`], this never calls `__iter__`.
    /// Consumers such as `str.join` can therefore translate acquisition
    /// TypeErrors while preserving every exception raised later by `__next__`.
    pub(crate) fn collect_iterator(&mut self, iterator: &Value) -> Result<Vec<Value>> {
        match iterator.kind() {
            ValueKind::Generator(_) => self.collect_iterable(iterator),
            ValueKind::BuiltinObject { ops, .. } if ops.is_iterator() => {
                self.collect_iterable(iterator)
            }
            ValueKind::PyInstance(iter_instance) => {
                let iter_class = Rc::clone(&iter_instance.borrow().class);
                let Some(next_method) = lookup_class_attr(&iter_class, "__next__") else {
                    return Err(pyrust_core::type_err!(
                        "'{}' object is not an iterator",
                        iter_class.borrow().name
                    ));
                };
                let mut items = Vec::new();
                loop {
                    match invoke_class_method(self, next_method.clone(), iterator.clone(), &[]) {
                        Ok(value) => items.push(value),
                        Err(ref error) if is_stop_iteration_error(error) => break,
                        Err(error) => return Err(error),
                    }
                }
                Ok(items)
            }
            _ => Err(pyrust_core::type_err!(
                "'{}' object is not an iterator",
                value_type_name_str(iterator)
            )),
        }
    }
}
