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

    /// Resolve one `next()` step: yield the produced value, or raise a bare
    /// `StopIteration` if the cursor is exhausted.  Shared by every built-in
    /// iterator arm in [`Self::advance_iterator`], which previously inlined
    /// this same match.
    fn step_or_stop(item: Option<Value>) -> Result<Value> {
        item.ok_or_else(|| pyrust_core::py_err!("StopIteration", String::new()))
    }

    /// Call next() on a generator or any object with __next__.
    ///
    /// `default`, when supplied, replaces a `StopIteration` raised by the
    /// advance.  CPython's `builtin_next` catches exactly `StopIteration`
    /// — subclasses included, since it tests with `PyErr_ExceptionMatches`
    /// — and lets every other exception through, so a mutation-latched
    /// cursor's `RuntimeError` or a `__next__` that raises `ValueError`
    /// still propagates.
    ///
    /// The catch lives here, at the single entry point, rather than in the
    /// per-kind arms: [`Self::advance_iterator`] only has to report
    /// exhaustion as a `StopIteration` error and never has to know about
    /// `default`.  Issue #2966 was exactly the gap that per-arm handling
    /// left — built-in-module iterator classes raise
    /// `PyError::Named("StopIteration", …)` from their `__next__`, which
    /// the `PyInstance` arm's `PyError::Raised`-only match did not
    /// recognise, so `next(itertools.combinations([1], 2), "done")` raised
    /// instead of returning `"done"`.
    pub(crate) fn call_next(&mut self, val: &Value, default: Option<Value>) -> Result<Value> {
        match self.advance_iterator(val) {
            Ok(value) => Ok(value),
            // Single-argument `next(it)` — by far the hotter form, and the
            // one every internal consumer uses — never inspects the error,
            // so exhaustion costs exactly what it did before.
            Err(err) => match default {
                Some(fallback) if is_stop_iteration_error(&err) => Ok(fallback),
                _ => Err(err),
            },
        }
    }

    /// Advance `val` by exactly one element.  Exhaustion is reported as a
    /// `StopIteration` error, which [`Self::call_next`] turns into the
    /// caller's `default` when there is one.
    fn advance_iterator(&mut self, val: &Value) -> Result<Value> {
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
                return Self::step_or_stop(native.advance()?);
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
                    // Exhausted generator: StopIteration() with no args → .value is None.
                    let exc = if let Some(cls) = self.exc_classes.get("StopIteration") {
                        PyError::Raised(instantiate_exception(cls, vec![]))
                    } else {
                        pyrust_core::py_err!("StopIteration", String::new())
                    };
                    return Err(exc);
                }
                // A `StopIteration` from the body is returned verbatim so
                // that `.value` survives for a bare `next(g)` (PEP 380 /
                // issue #600); `call_next` discards it in favour of the
                // caller's default when there is one.
                return self.resume_generator(frame);
            }

            // The step_* helpers below re-borrow the state internally.
            drop(borrow);

            // GetItemIter path: drive one `__getitem__(i)` call lazily.
            // Borrow released by step_getitem_iter before invoking the
            // method (it would otherwise re-entrantly re-borrow).
            if tid == TypeId::of::<GetItemIter>() {
                return Self::step_or_stop(self.step_getitem_iter(&state_rc)?);
            }

            // CallableIter path: invoke callable(), stop when result == sentinel.
            if tid == TypeId::of::<CallableIter>() {
                return Self::step_or_stop(self.step_callable_iter(&state_rc)?);
            }

            // MapIter path: apply func to one row of columns per step.
            if tid == TypeId::of::<MapIter>() {
                return Self::step_or_stop(self.step_map_iter(&state_rc)?);
            }

            // FilterIter path: scan forward for next passing element.
            if tid == TypeId::of::<FilterIter>() {
                return Self::step_or_stop(self.step_filter_iter(&state_rc)?);
            }

            // Standard-library iterators expose one typed advancement
            // interface; their concrete cursor and API policy remain with the
            // provider.
            if tid == TypeId::of::<ProviderIterator>() {
                return Self::step_or_stop(self.step_provider_iterator(&state_rc)?);
            }

            // RangeIter path: lazy common i64 range iteration.
            if tid == TypeId::of::<RangeIter>() {
                return Self::step_or_stop(self.step_range_iter(&state_rc)?);
            }

            // BigRangeIter path: lazy arbitrary-precision range iteration (#2118).
            if tid == TypeId::of::<BigRangeIter>() {
                return Self::step_or_stop(self.step_bigrange_iter(&state_rc)?);
            }

            // EnumerateIter path: (counter, element) pair per step.
            if tid == TypeId::of::<EnumerateIter>() {
                return Self::step_or_stop(self.step_enumerate_iter(&state_rc)?);
            }

            // ZipIter path: one row tuple per step.
            if tid == TypeId::of::<ZipIter>() {
                return Self::step_or_stop(self.step_zip_iter(&state_rc)?);
            }

            Err(PyError::Runtime("invalid generator state".to_string()))
        } else if let ValueKind::PyInstance(inst) = val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__next__") {
                // Covers both user classes (`raise StopIteration` →
                // `PyError::Raised`) and built-in-module iterator classes
                // such as `itertools.combinations`, whose `__next__` reports
                // exhaustion as `PyError::Named("StopIteration", …)`.
                invoke_class_method(self, method_val, Value::py_instance(inst_rc), &[])
            } else {
                Err(pyrust_core::type_err!(
                    "'{}' object is not an iterator",
                    class.borrow().name
                ))
            }
        } else if let ValueKind::BuiltinObject { ops, state } = val.kind()
            && ops.is_iterator()
        {
            Self::step_or_stop(ops.iter_next(state)?)
        } else {
            Err(pyrust_core::type_err!(
                "'{}' object is not an iterator",
                value_type_name_str(val)
            ))
        }
    }
}
