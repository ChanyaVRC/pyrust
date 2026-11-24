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
            // `types.coroutine` keeps the object a real generator while
            // granting only this activation the legacy await capability.  The
            // immutable tag is outside the borrowable frame so it is readable
            // even while that frame is executing.
            if state_rc.is_iterable_coroutine() {
                return Ok(awaited.clone());
            }
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
                    // (#2280) and the `anext(aiter, default)` wrapper (#2983)
                    // are self-driving awaitables: pass them through for
                    // `YieldFrom` to step.
                    if b.downcast_ref::<AsyncGenASend>().is_some()
                        || b.downcast_ref::<AnextAwaitable>().is_some()
                    {
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
                    return self.step_async_gen_asend(&state_rc, sent_val, true);
                }

                // Two-argument `anext(aiter, default)` awaitable (#2983).
                let is_anext_awaitable = state_rc
                    .try_borrow()
                    .map(|b| b.downcast_ref::<AnextAwaitable>().is_some())
                    .unwrap_or(false);
                if is_anext_awaitable {
                    return self.step_anext_awaitable_send(&state_rc, sent_val);
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
                                return Err(PyError::raised(exc));
                            }
                            return Err(e);
                        }
                        Err(e) => return Err(e),
                    }
                }

                Err(PyError::runtime(
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
    pub(crate) fn step_async_gen_asend(
        &mut self,
        asend_rc: &Rc<GeneratorCell>,
        sent_val: Value,
        enforce_running_owner: bool,
    ) -> Result<Value> {
        fn reject_running(
            asend_rc: &Rc<GeneratorCell>,
            is_close_or_throw: bool,
            is_aclose: bool,
        ) -> PyError {
            // CPython exhausts failed aclose/athrow awaitables, while a failed
            // __anext__/asend remains retryable after the current owner exits.
            if is_close_or_throw
                && let Ok(mut state) = asend_rc.try_borrow_mut()
                && let Some(asend) = state.downcast_mut::<AsyncGenASend>()
            {
                asend.done = true;
            }
            let family = if is_aclose {
                "aclose"
            } else if is_close_or_throw {
                "athrow"
            } else {
                "anext"
            };
            pyrust_core::runtime_err!("{family}(): asynchronous generator is already running")
        }

        fn clear_running_owner(
            frame: &mut GeneratorFrame,
            asend_rc: &Rc<GeneratorCell>,
            clear_any_owner: bool,
        ) {
            if clear_any_owner
                || frame
                    .async_gen_running_owner
                    .as_ref()
                    .is_some_and(|owner| owner.as_ptr() == Rc::as_ptr(asend_rc))
            {
                frame.async_gen_running_owner = None;
            }
        }

        // Inspect the wrapper without consuming its first-step state. A
        // competing awaitable must fail before `send_value` / `throw_exc` or
        // `started` changes, so __anext__/asend can be retried later.
        let (agen_rc, is_close_or_throw, is_aclose) = {
            let b = asend_rc.borrow();
            let asend = b.downcast_ref::<AsyncGenASend>().ok_or_else(|| {
                PyError::runtime("invalid async-generator asend state".to_string())
            })?;
            if asend.done {
                if asend.is_close_or_throw {
                    return Err(pyrust_core::runtime_err!(
                        "cannot reuse already awaited aclose()/athrow()"
                    ));
                }
                return Err(pyrust_core::runtime_err!(
                    "cannot reuse already awaited __anext__()/asend()"
                ));
            }
            (
                Rc::clone(&asend.agen),
                asend.is_close_or_throw,
                asend.is_aclose,
            )
        };

        // The transient borrow/GenDriving guards cover re-entrancy while the
        // body is executing. Persistent ownership below covers an inner-await
        // suspension, when the frame is available but only the same wrapper
        // may resume it.
        let mut borrow = match agen_rc.try_borrow_mut() {
            Ok(borrow) => borrow,
            Err(_) => {
                return Err(reject_running(asend_rc, is_close_or_throw, is_aclose));
            }
        };
        if borrow.is::<GenDriving>() {
            drop(borrow);
            return Err(reject_running(asend_rc, is_close_or_throw, is_aclose));
        }
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::runtime("invalid async-generator state".to_string()))?;

        if enforce_running_owner {
            match frame.async_gen_running_owner.as_ref() {
                Some(owner) if owner.as_ptr() != Rc::as_ptr(asend_rc) => {
                    drop(borrow);
                    return Err(reject_running(asend_rc, is_close_or_throw, is_aclose));
                }
                Some(_) => {}
                None => {
                    frame.async_gen_running_owner = Some(Rc::downgrade(asend_rc));
                }
            }
        }

        // The owner is established before the first-step payload is consumed.
        // Direct awaitable.throw() deliberately bypasses the claim for that
        // one step, matching CPython's generator-throw adapter.
        let (send_value, throw_exc) = {
            let mut b = asend_rc.borrow_mut();
            let asend = b.downcast_mut::<AsyncGenASend>().ok_or_else(|| {
                PyError::runtime("invalid async-generator asend state".to_string())
            })?;
            let send_value = if asend.started {
                Some(sent_val)
            } else {
                match asend.send_value.take() {
                    Some(value) if !value.is_none() => Some(value),
                    _ => Some(sent_val),
                }
            };
            let throw_exc = asend.throw_exc.take();
            asend.started = true;
            (send_value, throw_exc)
        };
        if frame.done {
            // `aclose()` on an already-finished async generator is a silent
            // no-op (the awaitable completes with None); `__anext__`/`asend`
            // raise StopAsyncIteration.
            let result = if is_aclose {
                Err(self.make_stop_iteration())
            } else {
                Err(self.make_stop_async_iteration())
            };
            clear_running_owner(frame, asend_rc, !enforce_running_owner);
            drop(borrow);
            let mut asend_state = asend_rc.borrow_mut();
            asend_state
                .downcast_mut::<AsyncGenASend>()
                .expect("checked async-generator asend state")
                .done = true;
            return result;
        }
        // CPython: sending a non-None value into a *just-started* async
        // generator (one never resumed, `pc == 0`) via `asend(v)` is a
        // TypeError, raised when the awaitable is first driven (#2280).
        // `athrow`/`aclose` (which carry an injected exception) and
        // `__anext__`/`asend(None)` are exempt.
        if frame.pc == 0 && throw_exc.is_none() && send_value.as_ref().is_some_and(|v| !v.is_none())
        {
            clear_running_owner(frame, asend_rc, !enforce_running_owner);
            drop(borrow);
            let mut asend_state = asend_rc.borrow_mut();
            asend_state
                .downcast_mut::<AsyncGenASend>()
                .expect("checked async-generator asend state")
                .done = true;
            return Err(pyrust_core::type_err!(
                "can't send non-None value to a just-started async generator"
            ));
        }
        let prestart_throw = frame.pc == 0 && throw_exc.is_some();
        let resume = self.resume_generator_with_exc(
            frame,
            throw_exc,
            send_value.unwrap_or_else(Value::none),
        );
        let result = match resume {
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
            // A pre-start `athrow(StopIteration(...))` raises the thrown
            // StopIteration verbatim without converting to StopAsyncIteration.
            Err(ref e) if !prestart_throw && is_stop_iteration_error(e) => {
                if is_aclose {
                    // aclose: a clean StopAsyncIteration/return means the close
                    // succeeded — complete the await with None.
                    Err(self.make_stop_iteration())
                } else {
                    Err(self.make_stop_async_iteration())
                }
            }
            // `aclose()`: the body let the injected GeneratorExit propagate
            // (the normal, well-behaved case) — the close succeeded, so the
            // awaitable completes with None rather than re-raising.
            Err(ref e) if is_aclose && e.class_name_is("GeneratorExit") => {
                Err(self.make_stop_iteration())
            }
            // PEP 479 / PEP 525: an explicit `StopAsyncIteration` raised inside
            // an async generator body converts to `RuntimeError("async generator
            // raised StopAsyncIteration")` with the original exception as
            // `__cause__` / `__context__` (owning the async generator's frames).
            // A pre-start `athrow(StopAsyncIteration(...))` raises the thrown
            // exception verbatim without entering the body or PEP 479 wrapping
            // (#2983).
            Err(e) if !prestart_throw && is_stop_async_iteration_error(&e) => {
                Err(self.pep479_wrap_stop_async_iteration(e))
            }
            Err(e) => Err(e),
        };
        if result.is_err() {
            // A direct awaitable.throw() bypasses ownership for its one step.
            // If that step terminates at a bare yield or error, CPython also
            // releases a distinct pre-existing owner.  An inner-await `Ok`
            // remains suspended and deliberately preserves that owner.
            clear_running_owner(frame, asend_rc, !enforce_running_owner);
            drop(borrow);
            let mut asend_state = asend_rc.borrow_mut();
            asend_state
                .downcast_mut::<AsyncGenASend>()
                .expect("checked async-generator asend state")
                .done = true;
        }
        result
    }

    /// Build a `StopAsyncIteration` error (async-generator exhaustion, #2280).
    fn make_stop_async_iteration(&self) -> PyError {
        if let Some(cls) = self.exc_classes.get("StopAsyncIteration") {
            PyError::raised(instantiate_exception(cls, vec![]))
        } else {
            pyrust_core::py_err!("StopAsyncIteration", String::new())
        }
    }

    /// Build the argument-less `StopIteration` used when `aclose()` completes.
    fn make_stop_iteration(&self) -> PyError {
        if let Some(cls) = self.exc_classes.get("StopIteration") {
            PyError::raised(instantiate_exception(cls, vec![]))
        } else {
            pyrust_core::py_err!("StopIteration", String::new())
        }
    }

    /// Build a `StopIteration(value)` error carrying a produced async-gen item
    /// (#2280).  The consumer's `YieldFrom` reads `.value` to obtain the item.
    fn make_stop_iteration_with_value(&self, value: Value) -> PyError {
        if let Some(cls) = self.exc_classes.get("StopIteration") {
            PyError::raised(instantiate_exception(cls, vec![value]))
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

                // Two-argument `anext(aiter, default)` awaitable (#2983):
                // forward the thrown exception through the wrapped awaitable
                // and convert `StopAsyncIteration` to `StopIteration(default)`.
                if borrow.downcast_ref::<AnextAwaitable>().is_some() {
                    drop(borrow);
                    let exc_val = match exc.kind() {
                        pyrust_core::PyErrorKind::Raised(_)
                        | pyrust_core::PyErrorKind::Named(_, _) => match exc.into_kind() {
                            pyrust_core::PyErrorKind::Raised(v) => v,
                            pyrust_core::PyErrorKind::Named(name, msg) => {
                                self.instantiate_named_exception(name.as_ref(), msg)?
                            }
                            _ => unreachable!(),
                        },
                        _ => return Err(exc),
                    };
                    return self.step_anext_awaitable_throw(&state_rc, vec![exc_val]);
                }

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
                                return Err(PyError::raised(exc_with_val));
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
                    let exc_val = match exc.kind() {
                        pyrust_core::PyErrorKind::Raised(_)
                        | pyrust_core::PyErrorKind::Named(_, _) => match exc.into_kind() {
                            pyrust_core::PyErrorKind::Raised(v) => v,
                            pyrust_core::PyErrorKind::Named(name, msg) => {
                                self.instantiate_named_exception(name.as_ref(), msg)?
                            }
                            _ => unreachable!(),
                        },
                        _ => return Err(exc),
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

    /// Check CPython's `PyAIter_Check`: whether `type(val)` defines `__anext__`
    /// (`tp_as_async->am_anext != NULL`).
    fn is_async_iterator(val: &Value) -> bool {
        match val.kind() {
            ValueKind::Generator(cell) => cell.kind() == GeneratorKind::AsyncGenerator,
            ValueKind::PyInstance(inst_rc) => {
                let class = Rc::clone(&inst_rc.borrow().class);
                lookup_class_attr(&class, "__anext__").is_some()
            }
            ValueKind::PyClass(class) => metaclass_dunder(class, "__anext__").is_some(),
            _ => false,
        }
    }

    /// Implementation of the `aiter(async_iterable)` builtin (issue #3140).
    ///
    /// Resolves `__aiter__` on `type(obj)` (mirroring CPython's `PyObject_GetAIter`
    /// — instance-dict `__aiter__` is ignored), invokes it synchronously, and
    /// validates that the returned object satisfies `PyAIter_Check` (its type
    /// defines `__anext__`).
    pub(crate) fn call_aiter(&mut self, obj: &Value) -> Result<Value> {
        let it = match obj.kind() {
            ValueKind::Generator(cell) if cell.kind() == GeneratorKind::AsyncGenerator => {
                return Ok(obj.clone());
            }
            ValueKind::PyInstance(inst_rc) => {
                let class = Rc::clone(&inst_rc.borrow().class);
                let Some(aiter_method) = lookup_class_attr(&class, "__aiter__") else {
                    return Err(pyrust_core::type_err!(
                        "'{}' object is not an async iterable",
                        class.borrow().name
                    ));
                };
                invoke_class_method(self, aiter_method, obj.clone(), &[])?
            }
            ValueKind::PyClass(class) => {
                if let Some(aiter_method) = metaclass_dunder(class, "__aiter__") {
                    invoke_class_method(self, aiter_method, obj.clone(), &[])?
                } else {
                    return Err(pyrust_core::type_err!(
                        "'{}' object is not an async iterable",
                        full_type_name_str(obj)
                    ));
                }
            }
            _ => {
                return Err(pyrust_core::type_err!(
                    "'{}' object is not an async iterable",
                    full_type_name_str(obj)
                ));
            }
        };
        if !Self::is_async_iterator(&it) {
            return Err(pyrust_core::type_err!(
                "aiter() returned not an async iterator of type '{}'",
                full_type_name_str(&it)
            ));
        }
        Ok(it)
    }

    /// Implementation of the `anext(aiter[, default])` builtin (issue #2983).
    ///
    /// Resolves `__anext__` on `type(aiter)` (mirroring CPython's
    /// `tp_as_async->am_anext` lookup — instance-dict `__anext__` is ignored),
    /// calls it synchronously, and either returns the raw result (1-arg form)
    /// or wraps it in an `AnextAwaitable` that maps `StopAsyncIteration` to
    /// `default` without awaiting `default` (2-arg form).
    pub(crate) fn call_anext(&mut self, aiter: &Value, default: Option<Value>) -> Result<Value> {
        let awaitable = match aiter.kind() {
            ValueKind::Generator(cell) if cell.kind() == GeneratorKind::AsyncGenerator => {
                self.make_async_gen_asend(aiter, None, None, false)?
            }
            ValueKind::PyInstance(inst_rc) => {
                let class = Rc::clone(&inst_rc.borrow().class);
                let Some(anext_method) = lookup_class_attr(&class, "__anext__") else {
                    return Err(pyrust_core::type_err!(
                        "'{}' object is not an async iterator",
                        class.borrow().name
                    ));
                };
                invoke_class_method(self, anext_method, aiter.clone(), &[])?
            }
            ValueKind::PyClass(class) => {
                if let Some(anext_method) = metaclass_dunder(class, "__anext__") {
                    invoke_class_method(self, anext_method, aiter.clone(), &[])?
                } else {
                    return Err(pyrust_core::type_err!(
                        "'{}' object is not an async iterator",
                        full_type_name_str(aiter)
                    ));
                }
            }
            _ => {
                return Err(pyrust_core::type_err!(
                    "'{}' object is not an async iterator",
                    full_type_name_str(aiter)
                ));
            }
        };
        match default {
            None => Ok(awaitable),
            Some(default_value) => Ok(Value::generator(Box::new(AnextAwaitable {
                wrapped: awaitable,
                default_value,
            }))),
        }
    }

    /// Advance a two-argument `anext(aiter, default)` awaitable by one step
    /// (`send(val)` / `__next__()` / `await`, issue #2983).
    pub(crate) fn step_anext_awaitable_send(
        &mut self,
        aw_rc: &Rc<GeneratorCell>,
        sent_val: Value,
    ) -> Result<Value> {
        let (wrapped, default_value) = {
            let b = aw_rc.borrow();
            let aw = b
                .downcast_ref::<AnextAwaitable>()
                .ok_or_else(|| PyError::runtime("invalid anext_awaitable state".to_string()))?;
            (aw.wrapped.clone(), aw.default_value.clone())
        };
        let iter_val = self.get_awaitable(&wrapped)?;
        let step_res = match iter_val.kind() {
            ValueKind::Generator(rc) => {
                let is_sendable = rc
                    .try_borrow()
                    .map(|b| {
                        b.is::<GeneratorFrame>()
                            || b.is::<GenDriving>()
                            || b.is::<AsyncGenASend>()
                            || b.is::<AnextAwaitable>()
                    })
                    .unwrap_or(true);
                if is_sendable {
                    self.generator_send(iter_val.clone(), sent_val)
                } else if sent_val.is_none() {
                    self.call_next(&iter_val, None)
                } else {
                    self.call_generator_method(iter_val.clone(), "send", vec![sent_val])
                }
            }
            ValueKind::PyInstance(inst_rc) => {
                if sent_val.is_none() {
                    self.call_next(&iter_val, None)
                } else {
                    let class = Rc::clone(&inst_rc.borrow().class);
                    if let Some(send_method) = lookup_class_attr(&class, "send") {
                        invoke_class_method(
                            self,
                            send_method,
                            iter_val.clone(),
                            &[ExpandedCallArg {
                                name: None,
                                value: sent_val,
                            }],
                        )
                    } else {
                        Err(PyError::attribute_error(
                            format!("'{}' object has no attribute 'send'", class.borrow().name),
                            Some("send".to_string()),
                            Some(iter_val.clone()),
                        ))
                    }
                }
            }
            _ => {
                if sent_val.is_none() {
                    self.call_next(&iter_val, None)
                } else {
                    let send_attr = self.get_attr(&iter_val, "send")?;
                    self.call_function_expanded(
                        send_attr,
                        &[ExpandedCallArg {
                            name: None,
                            value: sent_val,
                        }],
                    )
                }
            }
        };
        match step_res {
            Ok(v) => Ok(v),
            Err(ref e) if is_stop_async_iteration_error(e) => {
                Err(self.make_stop_iteration_with_value(default_value))
            }
            Err(e) => Err(e),
        }
    }

    /// Forward `throw(*args)` through a two-argument `anext(aiter, default)`
    /// awaitable, converting `StopAsyncIteration` to `StopIteration(default)`.
    pub(crate) fn step_anext_awaitable_throw(
        &mut self,
        aw_rc: &Rc<GeneratorCell>,
        args: Vec<Value>,
    ) -> Result<Value> {
        let (wrapped, default_value) = {
            let b = aw_rc.borrow();
            let aw = b
                .downcast_ref::<AnextAwaitable>()
                .ok_or_else(|| PyError::runtime("invalid anext_awaitable state".to_string()))?;
            (aw.wrapped.clone(), aw.default_value.clone())
        };
        let iter_val = self.get_awaitable(&wrapped)?;
        let step_res = match iter_val.kind() {
            ValueKind::Generator(_) => self.call_generator_method(iter_val.clone(), "throw", args),
            ValueKind::PyInstance(inst_rc) => {
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(throw_method) = lookup_class_attr(&class, "throw") {
                    let call_args: Vec<ExpandedCallArg> = args
                        .into_iter()
                        .map(|value| ExpandedCallArg { name: None, value })
                        .collect();
                    invoke_class_method(self, throw_method, iter_val.clone(), &call_args)
                } else {
                    Err(PyError::attribute_error(
                        format!("'{}' object has no attribute 'throw'", class.borrow().name),
                        Some("throw".to_string()),
                        Some(iter_val.clone()),
                    ))
                }
            }
            _ => {
                let throw_attr = self.get_attr(&iter_val, "throw")?;
                let call_args: Vec<ExpandedCallArg> = args
                    .into_iter()
                    .map(|value| ExpandedCallArg { name: None, value })
                    .collect();
                self.call_function_expanded(throw_attr, &call_args)
            }
        };
        match step_res {
            Ok(v) => Ok(v),
            Err(ref e) if is_stop_async_iteration_error(e) => {
                Err(self.make_stop_iteration_with_value(default_value))
            }
            Err(e) => Err(e),
        }
    }

    /// Forward `close()` through a two-argument `anext(aiter, default)` awaitable.
    pub(crate) fn step_anext_awaitable_close(
        &mut self,
        aw_rc: &Rc<GeneratorCell>,
    ) -> Result<Value> {
        let wrapped = {
            let b = aw_rc.borrow();
            let aw = b
                .downcast_ref::<AnextAwaitable>()
                .ok_or_else(|| PyError::runtime("invalid anext_awaitable state".to_string()))?;
            aw.wrapped.clone()
        };
        let iter_val = self.get_awaitable(&wrapped)?;
        match iter_val.kind() {
            ValueKind::Generator(_) => self.generator_close(iter_val.clone()),
            ValueKind::PyInstance(inst_rc) => {
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(close_method) = lookup_class_attr(&class, "close") {
                    invoke_class_method(self, close_method, iter_val.clone(), &[])
                } else {
                    Ok(Value::none())
                }
            }
            _ => {
                if let Ok(close_attr) = self.get_attr(&iter_val, "close") {
                    self.call_function_expanded(close_attr, &[])
                } else {
                    Ok(Value::none())
                }
            }
        }
    }
}
