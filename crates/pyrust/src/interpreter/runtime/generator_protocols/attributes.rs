impl Interpreter {
    pub(super) fn get_generator_attribute(
        &mut self,
        target: &Value,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
        name: &str,
    ) -> Result<Value> {
        // Generator introspection attributes (issue #1270).
        // All six attributes exposed by CPython 3.12's generator type:
        //   __name__, __qualname__, gi_running, gi_yieldfrom, gi_frame, gi_code.
        //
        // Async generators (#2280) expose the *asynchronous* iteration
        // protocol (`__aiter__`/`__anext__`/`asend`/`athrow`/`aclose`)
        // and NOT the synchronous one (`__iter__`/`__next__`/`send`).
        // Detect this before the generic method exposure below so the
        // two surfaces don't overlap (CPython's `async_generator` has no
        // `__next__`/`send`/`__iter__`).
        // Classify the generator subtype up-front so the protocol
        // surfaces don't overlap.  CPython exposes a distinct set of
        // methods per subtype:
        //   - plain generator: __iter__/__next__/send/throw/close
        //   - coroutine:       send/throw/close/__await__   (NO
        //     __iter__/__next__ — coroutines are awaitable, not
        //     iterable; issue #2314)
        //   - async generator: __aiter__/__anext__/asend/athrow/aclose
        let (is_async_gen, is_coroutine_only) = {
            state_rc
                .try_borrow()
                .ok()
                .and_then(|b| {
                    b.downcast_ref::<GeneratorFrame>().map(|f| {
                        (
                            f.is_async_generator(),
                            f.is_coroutine && !f.code.is_generator,
                        )
                    })
                })
                .unwrap_or((false, false))
        };
        if is_async_gen {
            match name {
                "__aiter__" | "__anext__" | "asend" | "athrow" | "aclose" => {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        name.to_string(),
                        target.clone(),
                    ));
                }
                _ => {}
            }
        }
        if is_coroutine_only {
            // A coroutine exposes send/throw/close but NOT
            // __iter__/__next__ (CPython gates those off — you must
            // `await`, not iterate; issue #2314).  CPython also exposes
            // `__await__` (returning a `coroutine_wrapper`); pyrust
            // drives `await` through the native event-loop bridge and
            // does not surface that wrapper object, so `__await__` is
            // intentionally not exposed here (documented limitation).
            match name {
                "send" | "close" | "throw" => {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        name.to_string(),
                        target.clone(),
                    ));
                }
                _ => {}
            }
        }
        // Issue #1413: also expose the iteration protocol methods as
        // bound-method values so that hasattr/getattr see them.
        // These apply to all generator subtypes (GeneratorFrame,
        // NativeIterFrame, CallableIter, …), so they are checked
        // before the downcast.  Skipped for async generators and
        // coroutines, which expose their own protocol above instead.
        if !is_async_gen && !is_coroutine_only {
            match name {
                "__iter__" | "__next__" | "send" | "close" | "throw" => {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        name.to_string(),
                        target.clone(),
                    ));
                }
                // Issue #2920: only the concrete built-in iterators carry a
                // remaining-count slot.  A generator, `map`/`filter`/`zip`,
                // `enumerate`, and `callable_iterator` have none, so this
                // attribute must stay absent for them.
                "__length_hint__" if value_has_length_hint(target) => {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        name.to_string(),
                        target.clone(),
                    ));
                }
                _ => {}
            }
        }
        let state_rc = Rc::clone(state_rc);
        let borrow = state_rc.borrow();
        if let Some(frame) = borrow.downcast_ref::<GeneratorFrame>() {
            // CPython exposes introspection attributes under a
            // type-specific prefix: `gi_*` on a plain generator,
            // `ag_*` on an async generator, `cr_*` on a coroutine
            // (issue #2302).  A given object exposes ONLY its own
            // prefix; the other two raise AttributeError.  The
            // underlying semantics are shared (suspended frame,
            // running flag, code stub, awaited/delegated sub-iterator),
            // so resolve the prefix first, then dispatch on the suffix.
            //   - async generator: ag_running / ag_frame / ag_code /
            //     ag_await  (no ag_yieldfrom in CPython).
            //   - coroutine:       cr_running / cr_frame / cr_code /
            //     cr_await.
            //   - plain generator: gi_running / gi_frame / gi_code /
            //     gi_yieldfrom  (no gi_await in CPython).
            // `__name__` / `__qualname__` apply to all three.
            let is_coroutine_only = frame.is_coroutine && !frame.code.is_generator;
            // Split off a 3-byte introspection prefix. `name.get(..3)`
            // is UTF-8-boundary-safe (returns None when byte index 3
            // falls inside a multibyte char, e.g. `getattr(g, "agé")`),
            // where the old `split_at(3)` panicked.
            let prefix = name.get(..3).unwrap_or("");
            let suffix = name.get(3..).unwrap_or("");
            let prefix_matches = match prefix {
                "ag_" => is_async_gen,
                "cr_" => is_coroutine_only,
                "gi_" => !is_async_gen && !is_coroutine_only,
                _ => false,
            };
            match name {
                "__name__" => return Ok(Value::string(frame.fn_name.as_ref())),
                "__qualname__" => return Ok(Value::string(frame.qualname.as_ref())),
                _ if prefix_matches => match suffix {
                    // running is always False when accessed from outside
                    // the body (True is only observable from within —
                    // pyrust does not expose re-entrant guards, matching
                    // CPython's "False unless currently on the C call
                    // stack" rule).
                    "running" => return Ok(Value::bool_(false)),
                    // frame: the suspended frame object (or None once
                    // exhausted), built from the retained FnCode (#2185).
                    "frame" => return Ok(self.build_generator_frame_object(frame)),
                    // code: pyrust does not expose a standalone code
                    // object here; return None to avoid AttributeError
                    // (issue #1270).
                    "code" => return Ok(Value::none()),
                    // gi_yieldfrom / ag_await / cr_await: the sub-iterator
                    // being delegated to via `yield from`, or awaited via
                    // an inner `await` (both compile to YieldFrom), else
                    // None.  When suspended at a YieldFrom the sub-iterator
                    // sits in iter_reg.  `frame.pc == 0` means the body
                    // hasn't started — don't inspect insns[0] (iter_reg
                    // unloaded).  CPython spells this `yieldfrom` for a
                    // plain generator and `await` for an async gen /
                    // coroutine, but never both on one object.
                    "yieldfrom" if prefix == "gi_" => {
                        if !frame.done
                            && frame.pc != 0
                            && let Some(crate::bytecode::Insn::YieldFrom { iter_reg, .. }) =
                                frame.code.insns.get(frame.pc)
                        {
                            let sub_iter = frame.regs[*iter_reg as usize].clone();
                            return Ok(sub_iter);
                        }
                        return Ok(Value::none());
                    }
                    "await" if prefix == "ag_" || prefix == "cr_" => {
                        if !frame.done
                            && frame.pc != 0
                            && let Some(crate::bytecode::Insn::YieldFrom { iter_reg, .. }) =
                                frame.code.insns.get(frame.pc)
                        {
                            let sub_iter = frame.regs[*iter_reg as usize].clone();
                            return Ok(sub_iter);
                        }
                        return Ok(Value::none());
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        let obj_name = if is_async_gen {
            "async_generator"
        } else if borrow
            .downcast_ref::<GeneratorFrame>()
            .is_some_and(|f| f.is_coroutine)
        {
            "coroutine"
        } else {
            "generator"
        };
        Err(PyError::attribute_error(
            format!("'{obj_name}' object has no attribute '{name}'"),
            Some(name.to_string()),
            Some(target.clone()),
        ))
    }
}
