impl Interpreter {
    pub(super) fn get_generator_attribute(
        &mut self,
        target: &Value,
        cell: &Rc<GeneratorCell>,
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
        //
        // The subtype comes from the object's immutable kind tag, so the
        // surface stays correct even when the attribute is read from inside
        // the running body — where the state cell is checked out and the old
        // `try_borrow`-and-default read silently degraded every coroutine and
        // async generator to the plain-generator surface (#2978).
        let kind = cell.kind();
        let is_async_gen = kind == GeneratorKind::AsyncGenerator;
        let is_coroutine_only = kind == GeneratorKind::Coroutine;
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
        // Only the three Python frame kinds carry the introspection surface
        // below; built-in iterators have none.
        if kind != GeneratorKind::Iterator {
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
            //
            // Split off a 3-byte introspection prefix. `name.get(..3)`
            // is UTF-8-boundary-safe (returns None when byte index 3
            // falls inside a multibyte char, e.g. `getattr(g, "agé")`),
            // where the old `split_at(3)` panicked.
            let prefix = name.get(..3).unwrap_or("");
            let suffix = name.get(3..).unwrap_or("");
            let prefix_matches = match prefix {
                "ag_" => is_async_gen,
                "cr_" => is_coroutine_only,
                "gi_" => kind == GeneratorKind::Generator,
                _ => false,
            };
            match name {
                // The writable name pair lives beside the state, not in it,
                // so it reads back while the body runs (#2978).
                "__name__" => {
                    if let Some(value) = cell.name() {
                        return Ok(Value::string(value.as_ref()));
                    }
                }
                "__qualname__" => {
                    if let Some(value) = cell.qualname() {
                        return Ok(Value::string(value.as_ref()));
                    }
                }
                _ if prefix_matches => match suffix {
                    // running: True exactly while the body is on the call
                    // stack — the state is checked out for the whole of a
                    // resume, which is the same fact `next(g)` re-entrancy
                    // reports as "generator already executing" (#2285).
                    "running" => return Ok(Value::bool_(generator_is_running(cell))),
                    // code: pyrust does not expose a standalone code
                    // object here; return None to avoid AttributeError
                    // (issue #1270).
                    "code" => return Ok(Value::none()),
                    // frame: the suspended frame object (or None once
                    // exhausted), built from the retained FnCode (#2185).
                    // A *running* frame is checked out and unreadable, so it
                    // reports None rather than a live frame object — a
                    // documented limitation.
                    "frame" => {
                        let qualname = cell.qualname().unwrap_or_else(|| "?".into());
                        return Ok(cell
                            .try_borrow()
                            .ok()
                            .and_then(|borrow| {
                                borrow.downcast_ref::<GeneratorFrame>().map(|frame| {
                                    self.build_generator_frame_object(frame, &qualname)
                                })
                            })
                            .unwrap_or_else(Value::none));
                    }
                    // gi_yieldfrom / ag_await / cr_await: the sub-iterator
                    // being delegated to via `yield from`, or awaited via
                    // an inner `await` (both compile to YieldFrom), else
                    // None.  When suspended at a YieldFrom the sub-iterator
                    // sits in iter_reg.  `frame.pc == 0` means the body
                    // hasn't started — don't inspect insns[0] (iter_reg
                    // unloaded).  CPython spells this `yieldfrom` for a
                    // plain generator and `await` for an async gen /
                    // coroutine, but never both on one object.  A running
                    // frame is not suspended at a `yield from` at all, which
                    // is exactly the None the unreadable-cell path returns.
                    "yieldfrom" if prefix == "gi_" => {
                        return Ok(Self::generator_delegate(cell));
                    }
                    "await" if prefix == "ag_" || prefix == "cr_" => {
                        return Ok(Self::generator_delegate(cell));
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        let obj_name = kind.frame_type_name().unwrap_or("generator");
        Err(PyError::attribute_error(
            format!("'{obj_name}' object has no attribute '{name}'"),
            Some(name.to_string()),
            Some(target.clone()),
        ))
    }

    /// The sub-iterator a suspended frame is delegating to (`yield from` /
    /// inner `await`), or `None` when it is not suspended at one.
    fn generator_delegate(cell: &Rc<GeneratorCell>) -> Value {
        let Ok(borrow) = cell.try_borrow() else {
            return Value::none();
        };
        if let Some(frame) = borrow.downcast_ref::<GeneratorFrame>()
            && !frame.done
            && frame.pc != 0
            && let Some(crate::bytecode::Insn::YieldFrom { iter_reg, .. }) =
                frame.code.insns.get(frame.pc)
        {
            return frame.regs[*iter_reg as usize].clone();
        }
        Value::none()
    }
}
