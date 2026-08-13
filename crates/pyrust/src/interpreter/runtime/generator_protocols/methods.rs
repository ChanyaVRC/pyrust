// Python-visible generator method dispatch and argument validation.

impl Interpreter {
    /// Dispatch a method call on a `Generator` value (`g.close()`, `g.throw()`,
    /// `g.__next__()`, `g.__iter__()`).  Other names raise `AttributeError`.
    pub(crate) fn call_generator_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        let native_class = native_iterator_class(&receiver);
        // Async-generator protocol (#2280): `__aiter__` returns the async
        // generator itself; `__anext__`/`asend`/`athrow`/`aclose` return an
        // awaitable (`AsyncGenASend`) that the `await` machinery drives.
        match method {
            "__aiter__" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "__aiter__() takes no arguments ({} given)",
                        args.len()
                    ));
                }
                return Ok(receiver);
            }
            "__anext__" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "__anext__() takes no arguments ({} given)",
                        args.len()
                    ));
                }
                return self.make_async_gen_asend(&receiver, None, None, false);
            }
            "asend" => {
                if args.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "asend() takes exactly one argument ({} given)",
                        args.len()
                    ));
                }
                let v = args.into_iter().next().unwrap();
                return self.make_async_gen_asend(&receiver, Some(v), None, false);
            }
            "athrow" => {
                if args.is_empty() || args.len() > 3 {
                    return Err(pyrust_core::type_err!("athrow() takes 1 to 3 arguments"));
                }
                let exc_arg = args.into_iter().next().unwrap();
                let exc_val = self.coerce_to_exception(exc_arg)?;
                return self.make_async_gen_asend(
                    &receiver,
                    None,
                    Some(PyError::Raised(exc_val)),
                    false,
                );
            }
            "aclose" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "aclose() takes no arguments ({} given)",
                        args.len()
                    ));
                }
                let inject = pyrust_core::py_err!("GeneratorExit", String::new());
                return self.make_async_gen_asend(&receiver, None, Some(inject), true);
            }
            _ => {}
        }
        match method {
            "__iter__" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "generator.__iter__() takes no arguments"
                    ));
                }
                Ok(receiver)
            }
            "__next__" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "generator.__next__() takes no arguments"
                    ));
                }
                self.call_next(&receiver, None)
            }
            // Issue #2920: the built-in iterators' remaining-element count.
            // The iteration domain owns the per-cursor arithmetic; this arm
            // only presents it under CPython's method name and arity.
            "__length_hint__" => {
                if !value_has_length_hint(&receiver) {
                    return Err(PyError::attribute_error(
                        format!(
                            "'{}' object has no attribute '__length_hint__'",
                            full_type_name_str(&receiver)
                        ),
                        Some(method.to_string()),
                        Some(receiver.clone()),
                    ));
                }
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "{}.__length_hint__() takes no arguments ({} given)",
                        full_type_name_str(&receiver),
                        args.len()
                    ));
                }
                // The check above proved the slot is present; a state that
                // cannot report a count degrades to `NotImplemented`, which is
                // exactly what CPython's `seqiter` returns for a sequence with
                // no length.
                self.builtin_iterator_length_hint_value(&receiver)
                    .map(|hint| hint.unwrap_or_else(Value::not_implemented))
            }
            "__reduce__" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "{}.__reduce__() takes no arguments ({} given)",
                        full_type_name_str(&receiver),
                        args.len()
                    ));
                }
                let kind = native_iterator_class(&receiver).ok_or_else(|| {
                    PyError::attribute_error(
                        format!(
                            "'{}' object has no attribute '__reduce__'",
                            full_type_name_str(&receiver)
                        ),
                        Some("__reduce__".to_string()),
                        Some(receiver.clone()),
                    )
                })?;
                native_iterator_reduce(&receiver, kind)
            }
            "__reduce_ex__" => {
                if args.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "{}.__reduce_ex__() takes exactly one argument ({} given)",
                        full_type_name_str(&receiver),
                        args.len()
                    ));
                }
                let protocol =
                    self.value_to_isize(&args[0], "Python int too large to convert to C int")?;
                i32::try_from(protocol).map_err(|_| {
                    pyrust_core::overflow_err!("Python int too large to convert to C int")
                })?;
                let kind = native_iterator_class(&receiver).ok_or_else(|| {
                    PyError::attribute_error(
                        format!(
                            "'{}' object has no attribute '__reduce_ex__'",
                            full_type_name_str(&receiver)
                        ),
                        Some("__reduce_ex__".to_string()),
                        Some(receiver.clone()),
                    )
                })?;
                native_iterator_reduce(&receiver, kind)
            }
            "__setstate__" if native_class == Some(NativeIteratorClass::Bytearray) => {
                if args.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "bytearray_iterator.__setstate__() takes exactly one argument ({} given)",
                        args.len()
                    ));
                }
                let raw = &args[0];
                let normalized = if matches!(
                    raw.kind(),
                    ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                ) {
                    raw.clone()
                } else {
                    effective_builtin_receiver(raw, &[])
                        .filter(|value| {
                            matches!(
                                value.kind(),
                                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                            )
                        })
                        .ok_or_else(|| pyrust_core::type_err!("an integer is required"))?
                };
                let position = self
                    .value_to_isize(&normalized, "Python int too large to convert to C ssize_t")?
                    .max(0) as usize;
                native_bytearray_iterator_setstate(&receiver, position)
            }
            "__getattribute__" if native_class.is_some() => {
                if args.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "expected 1 argument, got {}",
                        args.len()
                    ));
                }
                let name = args[0].as_str().ok_or_else(|| {
                    pyrust_core::type_err!(
                        "attribute name must be string, not '{}'",
                        full_type_name_str(&args[0])
                    )
                })?;
                self.get_attr(&receiver, name)
            }
            "close" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "generator.close() takes no arguments"
                    ));
                }
                self.generator_close(receiver)
            }
            "throw" => {
                if let Some(err) = Self::async_gen_asend_reuse_error(&receiver) {
                    return Err(err);
                }
                if args.is_empty() || args.len() > 3 {
                    return Err(pyrust_core::type_err!(
                        "generator.throw() takes 1 to 3 arguments"
                    ));
                }
                if args.len() == 3
                    && !args[2].is_none()
                    && !pyrust_builtins::traceback::is_traceback(&args[2])
                {
                    self.cleanup_invalid_throw_traceback(&receiver);
                    return Err(pyrust_core::type_err!(
                        "throw() third argument must be a traceback object"
                    ));
                }
                // CPython's throw(typ, val=None, tb=None) semantics (3.12):
                //   - 1 arg:  pass through to generator_throw (handles both
                //             class and instance via coerce_to_exception).
                //   - 2+ args: typ=args[0], val=args[1]; traceback (args[2])
                //             is validated above, then ignored (PEP 3109;
                //             deprecated since 3.12).
                //     * val is None          → raise typ() with no message.
                //     * val is instance of typ → use val directly.
                //     * otherwise            → raise typ(val).
                let exc = if args.len() == 1 {
                    args.into_iter().next().unwrap()
                } else {
                    let mut arg_iter = args.into_iter();
                    let typ = arg_iter.next().unwrap();
                    let val = arg_iter.next().unwrap();
                    // Extract the class Rc before consuming `typ`, so the
                    // non-class fall-through branch can still return `typ`.
                    let class_opt = match typ.kind() {
                        ValueKind::PyClass(c) => Some(Rc::clone(c)),
                        _ => None,
                    };
                    if let Some(class) = class_opt {
                        // Two-arg construction: val=None, val=instance, or
                        // val=arbitrary value to be passed to the constructor.
                        if val.is_none() {
                            // throw(ExcType, None) — same as throw(ExcType)
                            instantiate_exception(class, Vec::new())
                        } else {
                            // Check if val is already a subclass instance of
                            // typ; extract the Rc before we potentially move val.
                            let val_inst_rc = match val.kind() {
                                ValueKind::PyInstance(inst) => {
                                    let inst_class = Rc::clone(&inst.borrow().class);
                                    if class_is_subclass_of(&inst_class, &class) {
                                        Some(Rc::clone(inst))
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            };
                            if let Some(inst_rc) = val_inst_rc {
                                // val is already a suitable instance — use directly.
                                Value::py_instance(inst_rc)
                            } else {
                                // Construct typ(val).
                                instantiate_exception(class, vec![val])
                            }
                        }
                    } else {
                        // typ is already an instance.  CPython 3.12 requires
                        // val to be None in this case; any other value is a
                        // TypeError ("instance exception may not have a
                        // separate value").
                        if !val.is_none() {
                            return Err(pyrust_core::type_err!(
                                "instance exception may not have a separate value"
                            ));
                        }
                        typ
                    }
                };
                self.generator_throw(receiver, exc)
            }
            "send" => {
                if args.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "generator.send() takes exactly one argument ({} given)",
                        args.len()
                    ));
                }
                let sent_value = args.into_iter().next().unwrap();
                self.generator_send(receiver, sent_value)
            }
            other => {
                if let Some(iterator_class) = native_class {
                    if let Some(expected) = native_iterator_object_method_arity(other)
                        && args.len() != expected
                    {
                        return Err(pyrust_core::type_err!(
                            "expected {expected} arguments, got {}",
                            args.len()
                        ));
                    }
                    if let Some(inherited) = lookup_class_attr(&iterator_class.singleton(), other)
                        && let ValueKind::BuiltinFunction(function_name) = inherited.kind()
                        && function_name.starts_with("object.")
                    {
                        let dispatch = crate::builtin_registry::lookup(function_name)
                            .unwrap_or_else(|| panic!("{function_name} must be in the registry"));
                        let mut call_args = Vec::with_capacity(args.len() + 1);
                        call_args.push(ExpandedCallArg {
                            name: None,
                            value: receiver,
                        });
                        call_args.extend(
                            args.into_iter()
                                .map(|value| ExpandedCallArg { name: None, value }),
                        );
                        return dispatch(self, &call_args);
                    }
                    return Err(PyError::attribute_error(
                        format!(
                            "'{}' object has no attribute '{other}'",
                            iterator_class.full_type_name()
                        ),
                        Some(other.to_string()),
                        Some(receiver),
                    ));
                }
                Err(PyError::attribute_error(
                    format!("'generator' object has no attribute '{}'", other),
                    Some(other.to_string()),
                    None,
                ))
            }
        }
    }

    /// Implementation of `generator.close()`.
    ///
    /// Raises `GeneratorExit` at the current yield point.  Returns silently if
    /// the generator finishes (normally, by re-raising `GeneratorExit`, or by
    /// raising `StopIteration`); raises `RuntimeError("generator ignored
    /// GeneratorExit")` if the generator yields again; propagates any other
    /// exception unchanged.
    fn generator_close(&mut self, receiver: Value) -> Result<Value> {
        let state_rc = match receiver.kind() {
            ValueKind::Generator(rc) => Rc::clone(rc),
            _ => {
                return Err(pyrust_core::type_err!(
                    "generator.close() called on non-generator"
                ));
            }
        };

        // Re-entrancy guard: if the generator is currently executing (its
        // state RefCell is already borrowed by an in-flight `resume_*` call),
        // CPython raises ValueError("generator already executing") rather
        // than panicking.  We detect this via `try_borrow_mut`.
        {
            let mut borrow = match state_rc.try_borrow_mut() {
                Ok(b) => b,
                Err(_) => {
                    return Err(pyrust_core::value_err!("generator already executing"));
                }
            };
            // NativeIterFrame (returned by `iter()` on a built-in): close is a
            // no-op — there's nothing user-visible to clean up.
            if borrow.downcast_mut::<NativeIterFrame>().is_some() {
                return Ok(Value::none());
            }
            // GetItemIter (legacy `__getitem__` protocol, #394): same
            // story — there is no user frame to clean up; mark
            // exhausted so subsequent next() raises StopIteration.
            if let Some(it) = borrow.downcast_mut::<GetItemIter>() {
                it.exhausted = true;
                return Ok(Value::none());
            }
            // `__anext__`/`asend` objects are coroutine-style one-shot
            // awaitables. Closing one only exhausts that wrapper; it does not
            // close the underlying async generator.
            if let Some(asend) = borrow.downcast_mut::<AsyncGenASend>() {
                asend.done = true;
                return Ok(Value::none());
            }
        }

        let mut borrow = match state_rc.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => {
                return Err(pyrust_core::value_err!("generator already executing"));
            }
        };
        // A `GenDriving` placeholder: the frame is checked out by the
        // gen-drive trampoline (#2253) — the generator is executing
        // (issue #2285).
        if borrow.is::<GenDriving>() {
            return Err(pyrust_core::value_err!("generator already executing"));
        }
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
        self.close_generator_frame(frame)
    }

    /// Close one already-borrowed Python generator/coroutine frame by
    /// injecting `GeneratorExit` at its current suspension point.
    fn close_generator_frame(&mut self, frame: &mut GeneratorFrame) -> Result<Value> {
        if frame.done {
            return Ok(Value::none());
        }

        let inject = pyrust_core::py_err!("GeneratorExit", String::new());
        match self.resume_generator_with_exc(frame, Some(inject), Value::none()) {
            // Generator yielded again instead of returning/re-raising — that's
            // an error in CPython.
            Ok(_yielded) => {
                // Mark as done so subsequent calls don't re-execute.
                frame.done = true;
                Err(pyrust_core::runtime_err!("generator ignored GeneratorExit"))
            }
            // Generator returned normally (StopIteration synthesised).
            Err(ref e) if is_stop_iteration_error(e) => Ok(Value::none()),
            // Generator re-raised GeneratorExit — that's the expected close
            // behaviour, swallow it.  Subclasses are equally valid.
            Err(ref e) if e.class_name_is("GeneratorExit") => Ok(Value::none()),
            Err(e) => Err(e),
        }
    }

    /// Return the family-specific reuse error for a completed async-generator
    /// awaitable, before `throw()` validates its argument list or exception.
    fn async_gen_asend_reuse_error(receiver: &Value) -> Option<PyError> {
        let ValueKind::Generator(state_rc) = receiver.kind() else {
            return None;
        };
        let Ok(borrow) = state_rc.try_borrow() else {
            return None;
        };
        let asend = borrow.downcast_ref::<AsyncGenASend>()?;
        if !asend.done {
            return None;
        }
        Some(if asend.is_close_or_throw {
            pyrust_core::runtime_err!("cannot reuse already awaited aclose()/athrow()")
        } else {
            pyrust_core::runtime_err!("cannot reuse already awaited __anext__()/asend()")
        })
    }

    /// Apply CPython's receiver-specific terminal state when legacy
    /// `throw(type, value, traceback)` rejects a non-traceback third argument.
    /// The original TypeError remains the visible result; this cold helper
    /// only performs the cleanup that CPython does before returning it.
    fn cleanup_invalid_throw_traceback(&mut self, receiver: &Value) {
        let ValueKind::Generator(state_rc) = receiver.kind() else {
            return;
        };
        let Ok(mut state) = state_rc.try_borrow_mut() else {
            return;
        };

        if let Some(asend) = state.downcast_mut::<AsyncGenASend>() {
            // Fresh aclose/athrow wrappers retain their pending injected
            // exception after validation fails.  Ordinary __anext__/asend
            // wrappers instead become one-shot exhausted awaitables.
            if asend.is_close_or_throw {
                return;
            }
            let agen = asend.started.then(|| Rc::clone(&asend.agen));
            asend.done = true;
            drop(state);

            // A started wrapper is the coroutine currently driving the async
            // generator.  Closing it injects GeneratorExit through the
            // suspended frame so Python finally blocks run synchronously.
            if let Some(agen) = agen
                && let Ok(mut agen_state) = agen.try_borrow_mut()
                && let Some(frame) = agen_state.downcast_mut::<GeneratorFrame>()
            {
                let _ = self.close_generator_frame(frame);
                frame.async_gen_running_owner = None;
            }
            return;
        }

        // A validation failure closes an already-started coroutine receiver,
        // but leaves a plain generator (and a fresh coroutine) resumable.
        if let Some(frame) = state.downcast_mut::<GeneratorFrame>()
            && frame.is_coroutine
            && !frame.is_async_generator()
            && frame.pc != 0
            && !frame.done
        {
            let _ = self.close_generator_frame(frame);
        }
    }

    /// Implementation of `generator.throw(exc)`.
    ///
    /// Injects `exc` at the current yield point.  If the generator catches it
    /// and yields, returns that value.  If it returns normally, raises
    /// `StopIteration`.  Otherwise propagates the (re-raised or new)
    /// exception.
    fn generator_throw(&mut self, receiver: Value, exc: Value) -> Result<Value> {
        let state_rc = match receiver.kind() {
            ValueKind::Generator(rc) => Rc::clone(rc),
            _ => {
                return Err(pyrust_core::type_err!(
                    "generator.throw() called on non-generator"
                ));
            }
        };

        // Convert `exc` argument into a concrete exception instance so we can
        // hand it to the VM via `PyError::Raised`.  Accepts the same shapes as
        // a `raise` statement: an exception class (auto-instantiates) or an
        // exception instance.  coerce_to_exception already raises TypeError for
        // non-exception values, matching CPython's generator.throw() behaviour.
        let exc_val = self.coerce_to_exception(exc)?;

        // Re-entrancy guard: see generator_close for rationale.
        {
            let mut borrow = match state_rc.try_borrow_mut() {
                Ok(b) => b,
                Err(_) => {
                    return Err(pyrust_core::value_err!("generator already executing"));
                }
            };
            // NativeIterFrame: throw at a built-in iterator simply propagates
            // the exception (matching CPython, where the iterator has no
            // Python frame to inject into).
            if borrow.downcast_mut::<NativeIterFrame>().is_some() {
                return Err(PyError::Raised(exc_val));
            }
            // GetItemIter (#394): same — no Python frame to inject
            // into.  Propagate the thrown exception.
            if borrow.downcast_mut::<GetItemIter>().is_some() {
                return Err(PyError::Raised(exc_val));
            }
            // Throw through the one-shot wrapper into the async generator's
            // current suspension point. The step driver handles either a
            // caught exception or propagation and completes the wrapper.
            if let Some(asend) = borrow.downcast_mut::<AsyncGenASend>() {
                asend.throw_exc = Some(PyError::Raised(exc_val));
                drop(borrow);
                return self.step_async_gen_asend(&state_rc, Value::none(), false);
            }
        }

        let mut borrow = match state_rc.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => {
                return Err(pyrust_core::value_err!("generator already executing"));
            }
        };
        // A `GenDriving` placeholder: the frame is checked out by the
        // gen-drive trampoline (#2253) — the generator is executing
        // (issue #2285).
        if borrow.is::<GenDriving>() {
            return Err(pyrust_core::value_err!("generator already executing"));
        }
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
        if frame.done {
            if frame.is_coroutine && !frame.is_async_generator() {
                return Err(pyrust_core::runtime_err!(
                    "cannot reuse already awaited coroutine"
                ));
            }
            // throw() on an exhausted generator re-raises the exception
            // immediately (CPython behaviour).
            return Err(PyError::Raised(exc_val));
        }

        let inject = PyError::Raised(exc_val);
        match self.resume_generator_with_exc(frame, Some(inject), Value::none()) {
            // Generator caught the injected exception and yielded.
            Ok(v) => Ok(v),
            // Generator returned normally: propagate the original StopIteration so
            // .value (set by resume_generator_with_exc via instantiate_exception)
            // is preserved (PEP 380 / issue #600).
            Err(e) if is_stop_iteration_error(&e) => Err(e),
            // Any other propagating error (including a re-raise of the
            // injected exception) flows through unchanged.
            Err(e) => Err(e),
        }
    }

    /// Implementation of `generator.send(value)`.
    ///
    /// Resumes the generator and delivers `sent_value` as the result of the
    /// suspended `yield` expression inside the body.  Equivalent to `next(g)`
    /// when `sent_value` is `None`.
    ///
    /// CPython raises `TypeError: can't send non-None value to a just-started
    /// generator` when called on a generator that has never been advanced to
    /// its first `yield`.
    fn generator_send(&mut self, receiver: Value, sent_value: Value) -> Result<Value> {
        let state_rc = match receiver.kind() {
            ValueKind::Generator(rc) => Rc::clone(rc),
            _ => {
                return Err(pyrust_core::type_err!(
                    "generator.send() called on non-generator"
                ));
            }
        };

        let mut borrow = match state_rc.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => {
                return Err(pyrust_core::value_err!("generator already executing"));
            }
        };

        // Async-generator `__anext__`/`asend` awaitables expose the same
        // synchronous `send()` entry point that `await` drives. Route it to
        // the existing one-step async-generator driver instead of treating its
        // state cell as a plain GeneratorFrame.
        if borrow.is::<AsyncGenASend>() {
            drop(borrow);
            return self.step_async_gen_asend(&state_rc, sent_value, true);
        }

        // NativeIterFrame and GetItemIter do not support send().
        if borrow.downcast_mut::<NativeIterFrame>().is_some()
            || borrow.downcast_mut::<GetItemIter>().is_some()
        {
            return Err(PyError::attribute_error(
                "'generator' object has no attribute 'send'",
                Some("send".to_string()),
                None,
            ));
        }

        // A `GenDriving` placeholder: the frame is checked out by the
        // gen-drive trampoline (#2253) — the generator is executing
        // (issue #2285).
        if borrow.is::<GenDriving>() {
            return Err(pyrust_core::value_err!("generator already executing"));
        }
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;

        if frame.done {
            if frame.is_coroutine && !frame.is_async_generator() {
                return Err(pyrust_core::runtime_err!(
                    "cannot reuse already awaited coroutine"
                ));
            }
            // Exhausted generator: StopIteration() with no args → .value is None.
            let exc = if let Some(cls) = self.exc_classes.get("StopIteration") {
                PyError::Raised(instantiate_exception(cls, vec![]))
            } else {
                pyrust_core::py_err!("StopIteration", String::new())
            };
            return Err(exc);
        }

        // CPython: sending a non-None value to a just-started generator is an
        // error.  A just-started generator has pc == 0 (never been resumed).
        if frame.pc == 0 && !sent_value.is_none() {
            return Err(pyrust_core::type_err!(
                "can't send non-None value to a just-started generator"
            ));
        }

        match self.resume_generator_with_exc(frame, None, sent_value) {
            Ok(yielded) => Ok(yielded),
            // Propagate the original StopIteration so .value is preserved
            // (PEP 380 / issue #600).  Mirrors the same fix in call_next.
            Err(e) if is_stop_iteration_error(&e) => Err(e),
            Err(e) => Err(e),
        }
    }

    /// Build the awaitable returned by an async generator's
    /// `__anext__`/`asend`/`athrow`/`aclose` (#2280).  `receiver` must be an
    /// async-generator `Value`.  The returned `Value::generator(AsyncGenASend)`
    /// is accepted by `get_awaitable` and stepped by `YieldFrom`.
    fn make_async_gen_asend(
        &self,
        receiver: &Value,
        send_value: Option<Value>,
        throw_exc: Option<PyError>,
        is_aclose: bool,
    ) -> Result<Value> {
        let agen = match receiver.kind() {
            ValueKind::Generator(rc) => Rc::clone(rc),
            _ => {
                return Err(pyrust_core::type_err!(
                    "asynchronous generator method called on non-async-generator"
                ));
            }
        };
        let is_close_or_throw = throw_exc.is_some();
        let asend = AsyncGenASend {
            agen,
            send_value,
            throw_exc,
            started: false,
            done: false,
            is_close_or_throw,
            is_aclose,
        };
        Ok(Value::generator(Box::new(asend)))
    }
}
