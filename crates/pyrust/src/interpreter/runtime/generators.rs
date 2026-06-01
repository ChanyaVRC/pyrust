// Python-level generator method surface, extracted from `vm.rs`.
//
// Owns the user-facing generator methods — the `send` / `throw` / `close`
// dispatch (alongside the `__iter__` / `__next__` arms) and their argument
// validation — plus the implementation helpers `generator_close` /
// `generator_throw` / `generator_send`.  `vm.rs` keeps only the low-level
// resume machinery (`resume_generator*`, `GeneratorFrame`,
// `yield_from_advance`), so no non-dunder method-name string literals remain
// there.
//
// All items are `impl Interpreter` methods; the file is `include!`d into the
// same module from `runtime.rs`, so visibility and behaviour are identical to
// when they lived in `vm.rs`.  The split is purely organisational.

impl Interpreter {
    /// Dispatch a method call on a `Generator` value (`g.close()`, `g.throw()`,
    /// `g.__next__()`, `g.__iter__()`).  Other names raise `AttributeError`.
    pub(crate) fn call_generator_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "__iter__" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!("generator.__iter__() takes no arguments"));
                }
                Ok(receiver)
            }
            "__next__" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!("generator.__next__() takes no arguments"));
                }
                self.call_next(&receiver, None)
            }
            "close" => {
                if !args.is_empty() {
                    return Err(pyrust_core::type_err!("generator.close() takes no arguments"));
                }
                self.generator_close(receiver)
            }
            "throw" => {
                if args.is_empty() || args.len() > 3 {
                    return Err(pyrust_core::type_err!("generator.throw() takes 1 to 3 arguments"));
                }
                // CPython's throw(typ, val=None, tb=None) semantics (3.12):
                //   - 1 arg:  pass through to generator_throw (handles both
                //             class and instance via coerce_to_exception).
                //   - 2+ args: typ=args[0], val=args[1]; traceback (args[2])
                //             is ignored (PEP 3109; deprecated since 3.12).
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
                            return Err(pyrust_core::type_err!("instance exception may not have a separate value"));
                        }
                        typ
                    }
                };
                self.generator_throw(receiver, exc)
            }
            "send" => {
                if args.len() != 1 {
                    return Err(pyrust_core::type_err!("generator.send() takes exactly one argument ({} given)",
                            args.len()));
                }
                let sent_value = args.into_iter().next().unwrap();
                self.generator_send(receiver, sent_value)
            }
            other => Err(PyError::attribute_error(
                format!("'generator' object has no attribute '{}'", other),
                Some(other.to_string()),
                None,
            )),
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
                return Err(pyrust_core::type_err!("generator.close() called on non-generator"));
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
        }

        let mut borrow = match state_rc.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => {
                return Err(pyrust_core::value_err!("generator already executing"));
            }
        };
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
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
            // class_name_is walks the hierarchy so StopIteration subclasses are
            // also accepted as a normal termination.
            Err(ref e) if e.class_name_is("StopIteration") => Ok(Value::none()),
            // Generator re-raised GeneratorExit — that's the expected close
            // behaviour, swallow it.  Subclasses are equally valid.
            Err(ref e) if e.class_name_is("GeneratorExit") => Ok(Value::none()),
            Err(e) => Err(e),
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
                return Err(pyrust_core::type_err!("generator.throw() called on non-generator"));
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
        }

        let mut borrow = match state_rc.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => {
                return Err(pyrust_core::value_err!("generator already executing"));
            }
        };
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
        if frame.done {
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
            Err(e) if e.class_name_is("StopIteration") => Err(e),
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
                return Err(pyrust_core::type_err!("generator.send() called on non-generator"));
            }
        };

        let mut borrow = match state_rc.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => {
                return Err(pyrust_core::value_err!("generator already executing"));
            }
        };

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

        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;

        if frame.done {
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
            return Err(pyrust_core::type_err!("can't send non-None value to a just-started generator"));
        }

        match self.resume_generator_with_exc(frame, None, sent_value) {
            Ok(yielded) => Ok(yielded),
            // Propagate the original StopIteration so .value is preserved
            // (PEP 380 / issue #600).  Mirrors the same fix in call_next.
            Err(e) if e.class_name_is("StopIteration") => Err(e),
            Err(e) => Err(e),
        }
    }
}
