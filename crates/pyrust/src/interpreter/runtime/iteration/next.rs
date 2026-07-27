// Canonical iterator protocol advancement shared by the VM and built-in APIs.
impl Interpreter {
    /// Build a lazy iterator wrapping the legacy `__getitem__`
    /// sequence-iter protocol for `inst_rc`. Returns a
    /// `Value::generator(...)` that downcasts to [`GetItemIter`].
    /// Each `next()` call invokes `inst.__getitem__(i)` once, so an early exit
    /// does not evaluate unused indices.
    pub(crate) fn make_getitem_iter(&self, inst_rc: Rc<RefCell<PyInstance>>) -> Result<Value> {
        let class = Rc::clone(&inst_rc.borrow().class);
        let method_val = lookup_class_attr(&class, "__getitem__").ok_or_else(|| {
            pyrust_core::type_err!("'{}' object is not iterable", class.borrow().name)
        })?;
        let obj = Value::py_instance(inst_rc);
        Ok(Value::generator(Box::new(GetItemIter {
            obj,
            method: method_val,
            length_method: None,
            index: 0,
            step: 1,
            remaining: None,
            exhausted: false,
        })))
    }

    /// Resolve one `next()` step: yield the produced value, or fall back to
    /// `default` if the iterator is exhausted, or raise a bare `StopIteration`
    /// if there is no default.  Shared by every built-in iterator arm in
    /// [`call_next`], which previously inlined this same match.
    fn step_or_stop(item: Option<Value>, default: Option<Value>) -> Result<Value> {
        match item {
            Some(v) => Ok(v),
            None => default.ok_or_else(|| pyrust_core::py_err!("StopIteration", String::new())),
        }
    }

    /// Call next() on a generator or any object with __next__.
    pub(crate) fn call_next(&mut self, val: &Value, default: Option<Value>) -> Result<Value> {
        use std::any::TypeId;
        if let ValueKind::Generator(state_rc) = val.kind() {
            let state_rc = Rc::clone(state_rc);

            // Fast path: NativeIterFrame (no VM required) — probed exactly
            // as before so this arm pays no extra cost.
            //
            // `try_borrow_mut`: the cell is mutably checked out while the
            // generator's own body executes (a native `resume_generator` up the
            // stack), so a re-entrant `next(g)` from inside `g` must raise
            // CPython's `ValueError: generator already executing` rather than
            // panicking on the re-borrow (issue #2285).
            let mut borrow = state_rc
                .try_borrow_mut()
                .map_err(|_| pyrust_core::value_err!("generator already executing"))?;
            if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                return match native.advance()? {
                    Some(v) => Ok(v),
                    None => {
                        if let Some(d) = default {
                            Ok(d)
                        } else {
                            Err(pyrust_core::py_err!("StopIteration", String::new()))
                        }
                    }
                };
            }

            // Single-probe dispatch on the concrete iterator-state type for
            // everything else (issue #2315).  The previous probe cascade
            // re-borrowed the RefCell and ran a failed `downcast` per
            // variant, so a real generator — the most common kind in hot
            // consumer loops — paid 8 failed probes per element, which
            // dominated `sum(genexpr)`-style consumers.  `TypeId` equality
            // is exactly the check `downcast` performs internally, so the
            // per-arm downcasts below cannot fail.  The borrow is reused by
            // the GeneratorFrame arm and dropped before the `step_*` arms,
            // which re-borrow internally.
            let tid = {
                let any_ref: &dyn std::any::Any = &**borrow;
                any_ref.type_id()
            };

            // A `GenDriving` placeholder means the frame is checked out by
            // the gen-drive trampoline (#2253) up the stack — the generator
            // is executing, so a re-entrant `next(g)` raises CPython's
            // already-executing error (issue #2285).
            if tid == TypeId::of::<GenDriving>() {
                return Err(pyrust_core::value_err!("generator already executing"));
            }

            // GeneratorFrame path (hottest: every generator / genexpr).
            if tid == TypeId::of::<GeneratorFrame>() {
                let frame = borrow
                    .downcast_mut::<GeneratorFrame>()
                    .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
                // Async generators (#2280) are not synchronous iterators: `next(g)`
                // raises TypeError, matching CPython.  They are consumed via
                // `async for` / `__anext__`.
                if frame.is_async_generator() {
                    return Err(pyrust_core::type_err!(
                        "'async_generator' object is not an iterator"
                    ));
                }
                // Coroutines (#2314) are awaitable, not iterable: `next(coro)` raises
                // TypeError instead of driving the coroutine.  They are consumed via
                // `await` / `.send()`.
                if frame.is_coroutine && !frame.code.is_generator {
                    return Err(pyrust_core::type_err!(
                        "'coroutine' object is not an iterator"
                    ));
                }
                if frame.done {
                    drop(borrow);
                    return if let Some(d) = default {
                        Ok(d)
                    } else {
                        // Exhausted generator: StopIteration() with no args → .value is None.
                        let exc = if let Some(cls) = self.exc_classes.get("StopIteration") {
                            PyError::Raised(instantiate_exception(cls, vec![]))
                        } else {
                            pyrust_core::py_err!("StopIteration", String::new())
                        };
                        Err(exc)
                    };
                }
                return match self.resume_generator(frame) {
                    Ok(yielded) => Ok(yielded),
                    Err(e) if is_stop_iteration_error(&e) => {
                        drop(borrow);
                        if let Some(d) = default {
                            Ok(d)
                        } else {
                            // Propagate the original error so StopIteration.value
                            // is preserved (PEP 380 / issue #600).
                            Err(e)
                        }
                    }
                    Err(e) => Err(e),
                };
            }

            // The step_* helpers below re-borrow the state internally.
            drop(borrow);

            // GetItemIter path: drive one `__getitem__(i)` call lazily.
            // Borrow released by step_getitem_iter before invoking the
            // method (it would otherwise re-entrantly re-borrow).
            if tid == TypeId::of::<GetItemIter>() {
                return Self::step_or_stop(self.step_getitem_iter(&state_rc)?, default);
            }

            // CallableIter path: invoke callable(), stop when result == sentinel.
            if tid == TypeId::of::<CallableIter>() {
                return Self::step_or_stop(self.step_callable_iter(&state_rc)?, default);
            }

            // MapIter path: apply func to one row of columns per step.
            if tid == TypeId::of::<MapIter>() {
                return Self::step_or_stop(self.step_map_iter(&state_rc)?, default);
            }

            // FilterIter path: scan forward for next passing element.
            if tid == TypeId::of::<FilterIter>() {
                return Self::step_or_stop(self.step_filter_iter(&state_rc)?, default);
            }

            // Standard-library iterators expose one typed advancement
            // interface; their concrete cursor and API policy remain with the
            // provider.
            if tid == TypeId::of::<ProviderIterator>() {
                return Self::step_or_stop(self.step_provider_iterator(&state_rc)?, default);
            }

            // RangeIter path: lazy common i64 range iteration.
            if tid == TypeId::of::<RangeIter>() {
                return Self::step_or_stop(self.step_range_iter(&state_rc)?, default);
            }

            // BigRangeIter path: lazy arbitrary-precision range iteration (#2118).
            if tid == TypeId::of::<BigRangeIter>() {
                return Self::step_or_stop(self.step_bigrange_iter(&state_rc)?, default);
            }

            // EnumerateIter path: (counter, element) pair per step.
            if tid == TypeId::of::<EnumerateIter>() {
                return Self::step_or_stop(self.step_enumerate_iter(&state_rc)?, default);
            }

            // ZipIter path: one row tuple per step.
            if tid == TypeId::of::<ZipIter>() {
                return Self::step_or_stop(self.step_zip_iter(&state_rc)?, default);
            }

            Err(PyError::Runtime("invalid generator state".to_string()))
        } else if let ValueKind::PyInstance(inst) = val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__next__") {
                match invoke_class_method(self, method_val, Value::py_instance(inst_rc), &[]) {
                    Ok(v) => Ok(v),
                    Err(PyError::Raised(exc)) => {
                        if is_stop_iteration_error(&PyError::Raised(exc.clone())) {
                            if let Some(d) = default {
                                Ok(d)
                            } else {
                                Err(PyError::Raised(exc))
                            }
                        } else {
                            Err(PyError::Raised(exc))
                        }
                    }
                    Err(e) => Err(e),
                }
            } else {
                Err(pyrust_core::type_err!(
                    "'{}' object is not an iterator",
                    class.borrow().name
                ))
            }
        } else if let ValueKind::BuiltinObject { ops, state } = val.kind()
            && ops.is_iterator()
        {
            Self::step_or_stop(ops.iter_next(state)?, default)
        } else {
            Err(pyrust_core::type_err!(
                "'{}' object is not an iterator",
                value_type_name_str(val)
            ))
        }
    }
}
