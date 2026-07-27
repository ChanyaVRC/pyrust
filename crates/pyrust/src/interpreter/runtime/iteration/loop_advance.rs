pub(crate) enum LoopIteratorAdvance {
    Item(Result<Value>),
    DriveGenerator(Rc<RefCell<Box<dyn std::any::Any>>>),
    NotIterator,
}

impl Interpreter {
    /// Advance an iterator stored in `IterState::UserDefined`.
    ///
    /// The VM owns the loop slot and generator-frame switch. This method owns
    /// representation classification, native adapter selection, and cached
    /// `__next__` protocol dispatch.
    pub(crate) fn advance_loop_iterator(
        &mut self,
        iterator: &Value,
        cached_next: &mut Option<IterNextCacheEntry>,
    ) -> LoopIteratorAdvance {
        match iterator.kind() {
            ValueKind::Generator(state) => {
                if let Some(decision) = classify_generator_loop_state(state) {
                    return decision;
                }
                LoopIteratorAdvance::Item(self.advance_generator_backed_iterator(state))
            }
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                let class = Rc::clone(&instance.borrow().class);
                let class_version = class.borrow().mutation_version.get();
                let method = match cached_next.as_ref() {
                    Some(entry)
                        if entry
                            .class
                            .upgrade()
                            .is_some_and(|cached| Rc::ptr_eq(&cached, &class))
                            && pyrust_core::class_cache_stamp_matches(
                                class_version,
                                entry.class_version,
                                entry.epoch,
                            ) =>
                    {
                        entry.method.clone()
                    }
                    _ => {
                        let method = lookup_class_attr(&class, "__next__");
                        *cached_next = pyrust_core::class_cache_stamp(class_version).map(
                            |(class_version, epoch)| IterNextCacheEntry {
                                class: Rc::downgrade(&class),
                                class_version,
                                epoch,
                                method: method.clone(),
                            },
                        );
                        method
                    }
                };
                match method {
                    Some(method) => LoopIteratorAdvance::Item(invoke_class_method(
                        self,
                        method,
                        Value::py_instance(instance),
                        &[],
                    )),
                    None => LoopIteratorAdvance::NotIterator,
                }
            }
            ValueKind::BuiltinObject { ops, state } if ops.is_iterator() => {
                LoopIteratorAdvance::Item(ops.iter_next(state).and_then(require_iterator_item))
            }
            _ => LoopIteratorAdvance::NotIterator,
        }
    }

    pub(crate) fn advance_generator_backed_iterator(
        &mut self,
        state: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Value> {
        // A real generator frame is checked first: every other arm below is a
        // built-in adapter, so probing nine `is::<T>()` types before reaching
        // the generator made each generator step (native resume, and any body
        // the #2253 trampoline cannot carry) pay for all of them. The concrete
        // states are mutually exclusive, so the order is semantics-preserving.
        {
            let mut borrowed = state.borrow_mut();
            if let Some(frame) = borrowed.downcast_mut::<GeneratorFrame>() {
                if frame.done {
                    return Err(stop_iteration());
                }
                return self.resume_generator(frame);
            }
        }
        if state.borrow().is::<GetItemIter>() {
            return self
                .step_getitem_iter(state)
                .and_then(require_iterator_item);
        }
        if state.borrow().is::<CallableIter>() {
            return self
                .step_callable_iter(state)
                .and_then(require_iterator_item);
        }
        if state.borrow().is::<MapIter>() {
            return self.step_map_iter(state).and_then(require_iterator_item);
        }
        if state.borrow().is::<FilterIter>() {
            return self.step_filter_iter(state).and_then(require_iterator_item);
        }
        if state.borrow().is::<ProviderIterator>() {
            return self
                .step_provider_iterator(state)
                .and_then(require_iterator_item);
        }
        if state.borrow().is::<EnumerateIter>() {
            return self
                .step_enumerate_iter(state)
                .and_then(require_iterator_item);
        }
        if state.borrow().is::<ZipIter>() {
            return self.step_zip_iter(state).and_then(require_iterator_item);
        }
        if state.borrow().is::<RangeIter>() {
            return self.step_range_iter(state).and_then(require_iterator_item);
        }
        if state.borrow().is::<BigRangeIter>() {
            return self
                .step_bigrange_iter(state)
                .and_then(require_iterator_item);
        }

        let mut borrowed = state.borrow_mut();
        if let Some(native) = borrowed.downcast_mut::<NativeIterFrame>() {
            return native.advance().and_then(require_iterator_item);
        }
        Err(PyError::Runtime("invalid generator state".to_string()))
    }
}

fn classify_generator_loop_state(
    state: &Rc<RefCell<Box<dyn std::any::Any>>>,
) -> Option<LoopIteratorAdvance> {
    match state.try_borrow() {
        Ok(borrowed) => {
            if let Some(frame) = borrowed.downcast_ref::<GeneratorFrame>() {
                let drivable = !frame.done
                    && !frame.code.has_exc_handlers
                    && frame.handled_exc_slice.is_empty()
                    && frame.active_exception.is_none()
                    && frame.exc_saved_active_slice.is_empty()
                    && !matches!(
                        frame.code.insns.get(frame.pc),
                        Some(crate::bytecode::Insn::YieldFrom { .. })
                    );
                if drivable {
                    return Some(LoopIteratorAdvance::DriveGenerator(Rc::clone(state)));
                }
            } else if borrowed.is::<GenDriving>() {
                return Some(generator_already_executing());
            }
            None
        }
        Err(_) => Some(generator_already_executing()),
    }
}

fn require_iterator_item(item: Option<Value>) -> Result<Value> {
    item.ok_or_else(stop_iteration)
}

fn stop_iteration() -> PyError {
    pyrust_core::py_err!("StopIteration", String::new())
}

fn generator_already_executing() -> LoopIteratorAdvance {
    LoopIteratorAdvance::Item(Err(pyrust_core::py_err!(
        "ValueError",
        "generator already executing".to_string()
    )))
}
