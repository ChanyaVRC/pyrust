impl Interpreter {
    /// Resolve an `await` target to the iterator that drives it (issue #1039).
    ///
    /// Mirrors CPython's `GET_AWAITABLE`:
    /// - a coroutine (an `async def` frame) is its own awaitable → returned as-is;
    /// - an object defining `__await__` → returns `obj.__await__()`;
    /// - anything else → `TypeError: object … can't be used in 'await' expression`.
    ///
    /// The resolved value is then driven by the following `YieldFrom`.
    pub(crate) fn get_awaitable(&mut self, awaited: &Value) -> Result<Value> {
        // A coroutine drives itself: `await coro` resolves to the coroutine and
        // the following `YieldFrom` steps it.
        if let ValueKind::Generator(state_rc) = awaited.kind() {
            // Borrow the cell to inspect the coroutine's state.  A busy cell
            // (`try_borrow` → `Err`) means the coroutine is *currently* being
            // awaited / executing — CPython raises `RuntimeError("coroutine is
            // being awaited already")` (the self-await re-entrant case surfaces
            // later as `ValueError("coroutine already executing")` from the
            // resume path; both are coroutine-only).  A done coroutine
            // (`frame.done`) has already run to completion: re-awaiting it must
            // raise `RuntimeError("cannot reuse already awaited coroutine")`
            // rather than silently yielding its stale return value (issue #2282).
            match state_rc.try_borrow() {
                Ok(b) => {
                    // The `asend`/`__anext__` awaitable of an async generator
                    // (#2280) is itself a self-driving awaitable: pass it through
                    // for `YieldFrom` to step.
                    if b.downcast_ref::<AsyncGenASend>().is_some() {
                        return Ok(awaited.clone());
                    }
                    if let Some(frame) = b.downcast_ref::<GeneratorFrame>()
                        && frame.is_coroutine
                    {
                        // An async generator's frame is coroutine-tagged but is
                        // NOT directly awaitable (`await agen()` is a TypeError in
                        // CPython); it is consumed via `async for` / `asend`.
                        if frame.is_async_generator() {
                            return Err(pyrust_core::type_err!(
                                "object async_generator can't be used in 'await' expression"
                            ));
                        }
                        if frame.done {
                            return Err(pyrust_core::runtime_err!(
                                "cannot reuse already awaited coroutine"
                            ));
                        }
                        return Ok(awaited.clone());
                    }
                }
                Err(_) => {
                    // Cell checked out ⇒ the frame is currently executing on
                    // this native stack, so this `await` is re-entrant (a
                    // coroutine awaiting itself, directly or via a helper
                    // chain).  Raise CPython's coroutine wording eagerly
                    // (issue #2285): the generic resume path can only see an
                    // opaque busy cell and would report the *generator*
                    // wording.  The kind is unreadable while the cell is busy,
                    // but the only realistic busy frame reachable from `await`
                    // is a coroutine — a busy sync/async generator here would
                    // require `asyncio.run` nested inside its own body, which
                    // diverges from CPython before this point anyway.
                    return Err(pyrust_core::value_err!("coroutine already executing"));
                }
            }
        }
        // An object with `__await__` (e.g. a future-like awaitable) — call it
        // and drive the returned iterator.
        if let ValueKind::PyInstance(inst_rc) = awaited.kind() {
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(await_method) = lookup_class_attr(&class, "__await__") {
                return invoke_class_method(self, await_method, awaited.clone(), &[]);
            }
        }
        Err(pyrust_core::type_err!(
            "object {} can't be used in 'await' expression",
            value_type_name_str(awaited)
        ))
    }

    /// Step a coroutine exactly once for the real event loop (issue #2281).
    ///
    /// Sends `sent_value` (or injects `inject_exc`) into the coroutine and runs
    /// until its next suspension or completion.  Unlike
    /// `drive_coroutine_to_completion`, this does NOT loop — the yielded value
    /// (the awaitable that bubbled up the `YieldFrom` chain, typically an
    /// `asyncio.Future`) is returned to the caller so the Python-level loop can
    /// suspend the task on it.
    ///
    /// Returns:
    /// - `Ok(CoroStep::Yielded(v))` — coroutine suspended, yielding `v`;
    /// - `Ok(CoroStep::Returned(v))` — coroutine completed, returning `v`;
    /// - `Err(e)` — coroutine raised `e` (or was already done / executing).
    pub(crate) fn coro_step(
        &mut self,
        coro: &Value,
        sent_value: Value,
        inject_exc: Option<PyError>,
    ) -> Result<CoroStep> {
        let state_rc = match coro.kind() {
            ValueKind::Generator(s) => Rc::clone(s),
            _ => {
                return Err(pyrust_core::type_err!(
                    "a coroutine was expected, got {}",
                    value_type_name_str(coro)
                ));
            }
        };
        let mut borrow = state_rc
            .try_borrow_mut()
            .map_err(|_| pyrust_core::value_err!("coroutine already executing"))?;
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| pyrust_core::type_err!("a coroutine was expected"))?;
        match self.resume_generator_with_exc(frame, inject_exc, sent_value) {
            Ok(v) => Ok(CoroStep::Yielded(v)),
            Err(e) if is_stop_iteration_error(&e) => {
                let result = frame.last_return_value.clone().unwrap_or_else(Value::none);
                Ok(CoroStep::Returned(result))
            }
            Err(e) => Err(e),
        }
    }

    /// Advance a `yield from` sub-iterator by one step, forwarding `sent_val`
    /// to the sub-iterator's `send()` method if it is a generator.
    ///
    /// Returns:
    /// - `Ok(v)` — sub-iterator yielded `v`
    /// - `Err(StopIteration)` — sub-iterator exhausted; caller reads `.value`
    ///   from the error to obtain the sub-iterator's return value
    /// - `Err(other)` — exception from the sub-iterator
    pub(super) fn yield_from_advance(
        &mut self,
        iter_val: &Value,
        sent_val: Value,
    ) -> Result<Value> {
        match iter_val.kind() {
            ValueKind::Generator(state_rc) => {
                let state_rc = Rc::clone(state_rc);

                // Async-generator `asend`/`__anext__` awaitable (#2280): a
                // dedicated driver that resumes the underlying async generator
                // and distinguishes a bare `yield v` (→ item delivered) from an
                // inner-`await` suspension (→ propagate the scheduling point).
                let is_asend = state_rc
                    .try_borrow()
                    .map(|b| b.downcast_ref::<AsyncGenASend>().is_some())
                    .unwrap_or(false);
                if is_asend {
                    return self.step_async_gen_asend(&state_rc, sent_val);
                }

                // All non-frame iterator states (range, map, enumerate, native
                // frames, legacy __getitem__, and the other typed adapters)
                // share the canonical one-step service with `for` and
                // `next()`.  Keeping a second downcast table here caused newly
                // added iterator kinds to fail only under `yield from`.
                let is_python_frame_or_driving = state_rc
                    .try_borrow()
                    .map(|state| state.is::<GeneratorFrame>() || state.is::<GenDriving>())
                    .unwrap_or(true);
                if !is_python_frame_or_driving {
                    return self.advance_generator_backed_iterator(&state_rc);
                }

                let mut borrow = state_rc
                    .try_borrow_mut()
                    .map_err(|_| pyrust_core::value_err!("generator already executing"))?;

                // A `GenDriving` placeholder means the generator's frame is
                // checked out by the gen-drive trampoline (#2253) up the stack
                // — it is executing, same as the busy-cell case above
                // (issue #2285).
                if borrow.is::<GenDriving>() {
                    return Err(pyrust_core::value_err!("generator already executing"));
                }

                if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                    // Built-in iterator: no send support, just advance.
                    return match native.advance()? {
                        Some(v) => Ok(v),
                        None => Err(pyrust_core::py_err!("StopIteration", String::new())),
                    };
                }

                if let Some(frame) = borrow.downcast_mut::<GeneratorFrame>() {
                    if frame.done {
                        return Err(pyrust_core::py_err!("StopIteration", String::new()));
                    }
                    // `yield from` bypasses CPython's "can't send non-None to a
                    // just-started generator" check — the compiler initialises
                    // sent_reg to None so the first call is always next()-equivalent.
                    match self.resume_generator_with_exc(frame, None, sent_val) {
                        Ok(v) => return Ok(v),
                        Err(e) if is_stop_iteration_error(&e) => {
                            // Generator exhausted.  If it returned a non-None value
                            // (stashed by resume_generator_with_exc in
                            // frame.last_return_value), materialise a StopIteration
                            // instance with that value as args[0] so that
                            // extract_stop_iteration_value() can retrieve it in the
                            // YieldFrom handler.  self.env is restored at this point
                            // (resume_generator_with_exc swaps it back on return).
                            if let Some(rv) = frame.last_return_value.clone()
                                && !rv.is_none()
                                && let Some(cls) = lookup_exc_class("StopIteration")
                            {
                                let exc = instantiate_exception(cls, vec![rv]);
                                return Err(PyError::Raised(exc));
                            }
                            return Err(e);
                        }
                        Err(e) => return Err(e),
                    }
                }

                Err(PyError::Runtime(
                    "invalid generator state in yield from".to_string(),
                ))
            }
            ValueKind::PyInstance(inst_rc) => {
                let inst_rc = Rc::clone(inst_rc);
                let class = Rc::clone(&inst_rc.borrow().class);
                // Try send() first (PEP 342 compliant generators).
                if !sent_val.is_none()
                    && let Some(send_method) = lookup_class_attr(&class, "send")
                {
                    return invoke_class_method(
                        self,
                        send_method,
                        Value::py_instance(inst_rc),
                        &[ExpandedCallArg {
                            name: None,
                            value: sent_val,
                        }],
                    );
                }
                // Fall back to __next__().
                if let Some(next_method) = lookup_class_attr(&class, "__next__") {
                    invoke_class_method(self, next_method, Value::py_instance(inst_rc), &[])
                } else {
                    Err(pyrust_core::type_err!("object is not an iterator"))
                }
            }
            ValueKind::BuiltinObject { ops, state } => {
                let state = state.clone();
                ops.iter_next(&state).and_then(|opt| {
                    opt.ok_or_else(|| pyrust_core::py_err!("StopIteration", String::new()))
                })
            }
            _ => Err(pyrust_core::type_err!("object is not iterable")),
        }
    }

    /// Advance an async-generator `asend`/`__anext__` awaitable by one step
    /// (#2280).  `asend_rc` is the `AsyncGenASend` state cell; `sent_val` is the
    /// value the surrounding `await` machinery is sending into *this* awaitable
    /// (always `None` for the async-for driver — the value the user `asend`s is
    /// stored in the `AsyncGenASend` itself and delivered into the async
    /// generator on its first step).
    ///
    /// Resumes the underlying async generator once and applies the yield/await
    /// duality:
    /// - bare `yield v` inside the async-gen body → the awaitable *completes*
    ///   with `v`: raise `StopIteration(v)` so the consumer's `YieldFrom`
    ///   captures `v` as the produced item.
    /// - inner `await` suspension (the async-gen body is parked at a `YieldFrom`
    ///   awaiting e.g. `asyncio.sleep(0)`) → `Ok(scheduling_value)`: propagate
    ///   the scheduling point upward so the outer event loop steps it and the
    ///   awaitable is re-driven.
    /// - async-gen returned / exhausted → `StopAsyncIteration` (no value).
    fn step_async_gen_asend(
        &mut self,
        asend_rc: &Rc<GeneratorCell>,
        _sent_val: Value,
    ) -> Result<Value> {
        // Take the per-step injection state out of the AsyncGenASend.  Only the
        // *first* step delivers the original `asend(v)` value / `athrow` exc;
        // subsequent re-drives (after an inner-await suspension) send None.
        let (agen_rc, send_value, throw_exc, is_aclose) = {
            let mut b = asend_rc.borrow_mut();
            let asend = b.downcast_mut::<AsyncGenASend>().ok_or_else(|| {
                PyError::Runtime("invalid async-generator asend state".to_string())
            })?;
            let send_value = if asend.started {
                None
            } else {
                asend.send_value.take()
            };
            let throw_exc = if asend.started {
                None
            } else {
                asend.throw_exc.take()
            };
            asend.started = true;
            (
                Rc::clone(&asend.agen),
                send_value,
                throw_exc,
                asend.is_aclose,
            )
        };

        // Resume the async generator's frame once.  Re-entrant stepping (the
        // agen's own body driving another `__anext__`/`asend` on itself) is a
        // RuntimeError in CPython 3.12 — with the `anext():` prefix even for
        // `asend()` (issue #2285).
        let mut borrow = agen_rc.try_borrow_mut().map_err(|_| {
            pyrust_core::runtime_err!("anext(): asynchronous generator is already running")
        })?;
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid async-generator state".to_string()))?;
        if frame.done {
            // `aclose()` on an already-finished async generator is a silent
            // no-op (the awaitable completes with None); `__anext__`/`asend`
            // raise StopAsyncIteration.
            if is_aclose {
                return Err(self.make_stop_iteration_with_value(Value::none()));
            }
            return Err(self.make_stop_async_iteration());
        }
        // CPython: sending a non-None value into a *just-started* async
        // generator (one never resumed, `pc == 0`) via `asend(v)` is a
        // TypeError, raised when the awaitable is first driven (#2280).
        // `athrow`/`aclose` (which carry an injected exception) and
        // `__anext__`/`asend(None)` are exempt.
        if frame.pc == 0 && throw_exc.is_none() && send_value.as_ref().is_some_and(|v| !v.is_none())
        {
            return Err(pyrust_core::type_err!(
                "can't send non-None value to a just-started async generator"
            ));
        }
        let resume = self.resume_generator_with_exc(
            frame,
            throw_exc,
            send_value.unwrap_or_else(Value::none),
        );
        match resume {
            Ok(value) => {
                // Suspended.  Distinguish a bare `yield v` from an inner-`await`
                // suspension by inspecting the instruction the frame is now
                // parked at: a `YieldFrom` means we suspended inside an `await`
                // (await lowers to GetAwaitable + YieldFrom), so propagate the
                // scheduling value upward.  Anything else means we suspended at
                // a bare `Insn::Yield` whose pc was advanced past it, so `value`
                // is a produced item: complete the await with it.
                let parked_at_yield_from = matches!(
                    frame.code.insns.get(frame.pc),
                    Some(crate::bytecode::Insn::YieldFrom { .. })
                );
                if parked_at_yield_from {
                    // Inner-await scheduling point: propagate upward unchanged.
                    Ok(value)
                } else if is_aclose {
                    // `aclose()` injected GeneratorExit and the body yielded a
                    // value instead of exiting — CPython raises RuntimeError.
                    frame.done = true;
                    Err(pyrust_core::runtime_err!(
                        "async generator ignored GeneratorExit"
                    ))
                } else {
                    // Bare `yield value`: the awaitable completes with `value`.
                    Err(self.make_stop_iteration_with_value(value))
                }
            }
            // Async generator ran to completion (fell off the end or `return`).
            // CPython: `__anext__`/`asend` then raise StopAsyncIteration with no
            // value (an async-gen `return v` with non-None v is a SyntaxError,
            // so the return value is always None and is discarded).
            Err(ref e) if is_stop_iteration_error(e) => {
                if is_aclose {
                    // aclose: a clean StopAsyncIteration/return means the close
                    // succeeded — complete the await with None.
                    Err(self.make_stop_iteration_with_value(Value::none()))
                } else {
                    Err(self.make_stop_async_iteration())
                }
            }
            // `aclose()`: the body let the injected GeneratorExit propagate
            // (the normal, well-behaved case) — the close succeeded, so the
            // awaitable completes with None rather than re-raising.
            Err(ref e) if is_aclose && e.class_name_is("GeneratorExit") => {
                Err(self.make_stop_iteration_with_value(Value::none()))
            }
            Err(e) => Err(e),
        }
    }

    /// Build a `StopAsyncIteration` error (async-generator exhaustion, #2280).
    fn make_stop_async_iteration(&self) -> PyError {
        if let Some(cls) = self.exc_classes.get("StopAsyncIteration") {
            PyError::Raised(instantiate_exception(cls, vec![]))
        } else {
            pyrust_core::py_err!("StopAsyncIteration", String::new())
        }
    }

    /// Build a `StopIteration(value)` error carrying a produced async-gen item
    /// (#2280).  The consumer's `YieldFrom` reads `.value` to obtain the item.
    fn make_stop_iteration_with_value(&self, value: Value) -> PyError {
        if let Some(cls) = self.exc_classes.get("StopIteration") {
            PyError::Raised(instantiate_exception(cls, vec![value]))
        } else {
            pyrust_core::py_err!("StopIteration", String::new())
        }
    }

    /// Forward a thrown exception to a `yield from` sub-iterator (PEP 380 §3).
    ///
    /// Returns:
    /// - `Ok(v)` — sub-iterator caught the exception and yielded `v`
    /// - `Err(StopIteration)` — sub-iterator returned after handling the throw
    /// - `Err(other)` — sub-iterator did not handle it (or raised a new exception)
    pub(super) fn yield_from_throw_forward(
        &mut self,
        iter_val: &Value,
        exc: PyError,
    ) -> Result<Value> {
        match iter_val.kind() {
            ValueKind::Generator(state_rc) => {
                let state_rc = Rc::clone(state_rc);

                // Take the cell once: probing it first with an infallible
                // `borrow()` aborted the process when the sub-iterator was
                // itself mid-execution (#2978).
                let mut borrow = state_rc
                    .try_borrow_mut()
                    .map_err(|_| pyrust_core::value_err!("generator already executing"))?;

                // GetItemIter and NativeIterFrame have no Python frame; propagate.
                if borrow.downcast_mut::<GetItemIter>().is_some()
                    || borrow.downcast_mut::<NativeIterFrame>().is_some()
                {
                    return Err(exc);
                }

                if let Some(frame) = borrow.downcast_mut::<GeneratorFrame>() {
                    if frame.done {
                        return Err(exc);
                    }
                    match self.resume_generator_with_exc(frame, Some(exc), Value::none()) {
                        Ok(v) => return Ok(v),
                        Err(e) if is_stop_iteration_error(&e) => {
                            // Inner generator returned after handling the throw.
                            // Encode the return value in the StopIteration error.
                            if let Some(rv) = frame.last_return_value.clone()
                                && !rv.is_none()
                                && let Some(cls) = lookup_exc_class("StopIteration")
                            {
                                let exc_with_val = instantiate_exception(cls, vec![rv]);
                                return Err(PyError::Raised(exc_with_val));
                            }
                            return Err(e);
                        }
                        Err(e) => return Err(e),
                    }
                }

                Err(exc)
            }
            ValueKind::PyInstance(inst_rc) => {
                let inst_rc = Rc::clone(inst_rc);
                let class = Rc::clone(&inst_rc.borrow().class);
                // Try throw() method first.
                if let Some(throw_method) = lookup_class_attr(&class, "throw") {
                    // Materialise the exception into a Value for the throw() call.
                    let exc_val = match exc {
                        PyError::Raised(v) => v,
                        PyError::Named(name, msg) => {
                            self.instantiate_named_exception(name.as_ref(), msg)?
                        }
                        other => return Err(other),
                    };
                    invoke_class_method(
                        self,
                        throw_method,
                        Value::py_instance(inst_rc),
                        &[ExpandedCallArg {
                            name: None,
                            value: exc_val,
                        }],
                    )
                } else {
                    // No throw() method: propagate the exception.
                    Err(exc)
                }
            }
            // Other iterator types don't have throw() support.
            _ => Err(exc),
        }
    }
}
