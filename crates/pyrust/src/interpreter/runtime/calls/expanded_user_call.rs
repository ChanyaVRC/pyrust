fn is_supplied_keyword_only(function: &UserFunction, name: &str) -> bool {
    function
        .params
        .iter()
        .any(|param| param.is_keyword_only && param.name == name)
}

fn too_many_positional_error(
    function: &UserFunction,
    required_positional_count: usize,
    positional_param_count: usize,
    positional_given: usize,
    keyword_only_given: usize,
) -> PyError {
    let (takes_str, arg_word) = if required_positional_count == positional_param_count {
        let arg_word = if positional_param_count == 1 {
            "argument"
        } else {
            "arguments"
        };
        (format!("{positional_param_count}"), arg_word)
    } else {
        (
            format!("from {required_positional_count} to {positional_param_count}"),
            "arguments",
        )
    };
    if keyword_only_given == 0 {
        let given_word = if positional_given == 1 { "was" } else { "were" };
        pyrust_core::type_err!(
            "{}() takes {takes_str} positional {arg_word} but {} {given_word} given",
            function.effective_qualname(),
            positional_given,
        )
    } else {
        let positional_word = if positional_given == 1 {
            "argument"
        } else {
            "arguments"
        };
        let keyword_word = if keyword_only_given == 1 {
            "argument"
        } else {
            "arguments"
        };
        pyrust_core::type_err!(
            "{}() takes {takes_str} positional {arg_word} but {} positional {positional_word} (and {} keyword-only {keyword_word}) were given",
            function.effective_qualname(),
            positional_given,
            keyword_only_given,
        )
    }
}

impl Interpreter {
    pub(crate) fn call_user_function_expanded(
        &mut self,
        function: Rc<UserFunction>,
        args: &[ExpandedCallArg],
        bound_prefix: &[Value],
    ) -> Result<Value> {
        // Single pass over the parameter list (replaces three separate
        // `.iter().any()` scans on the hot call path): `*args` / `**kwargs`
        // divert to the variadic path, and whether any parameter is
        // keyword-only gates the exact-positional fast bind below.
        let mut has_args_param = false;
        let mut has_kwargs_param = false;
        let mut has_kwonly_param = false;
        for p in &function.params {
            has_args_param |= p.is_args;
            has_kwargs_param |= p.is_kwargs;
            has_kwonly_param |= p.is_keyword_only;
        }

        if !has_args_param && !has_kwargs_param {
            // Fast path: no variadic params.
            // Tier-0: register-VM path — fetch compiled bytecode up front so we
            // can bind arguments *directly* into the callee's new frame register
            // file (like CPython's fastlocals), skipping the per-call
            // `Vec<Option<Value>>` allocation + option-wrapping (#2123).
            let Some(code) = self.get_or_compile_bytecode(&function) else {
                // All user functions must have precompiled bytecode.
                return Err(PyError::Runtime(format!(
                    "no bytecode for '{}'",
                    function.name
                )));
            };
            let num_regs = code.num_regs as usize;
            let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];

            // Create a local env when the function uses globals, nonlocals, or
            // cell vars.  Determined here (before arg binding) so cell-var
            // parameters can be written straight into the env rather than
            // staged through an intermediate buffer.
            let needs_local_env = !function.global_names.is_empty()
                || !function.nonlocal_names.is_empty()
                || !code.cell_vars.is_empty();
            let local_env = if needs_local_env {
                let env = self.alloc_env(Some(Rc::clone(&function.env)));
                {
                    let mut e = env.borrow_mut();
                    e.local_names = Rc::clone(&function.local_names);
                    e.global_names = Rc::clone(&function.global_names);
                    e.nonlocal_names = Rc::clone(&function.nonlocal_names);
                }
                Some(env)
            } else {
                None
            };

            let nparams = function.params.len();
            // Exact-arity positional fast bind: the dominant call shape — every
            // argument supplied positionally, exactly filling the parameters,
            // with no keyword-only parameters and no bound prefix (e.g. a plain
            // `fib(n - 1)`).  Binds each argument straight into its precomputed
            // destination register/cell, skipping the per-param bound-flag
            // tracking, keyword matching, defaults resolution, and the
            // posonly/missing-argument diagnostics the general path needs.
            if bound_prefix.is_empty()
                && !has_kwonly_param
                && args.len() == nparams
                && args.iter().all(|a| a.name.is_none())
            {
                for (pi, arg) in args.iter().enumerate() {
                    bind_param_direct(
                        &function,
                        num_regs,
                        &mut regs,
                        &local_env,
                        pi,
                        arg.value.clone(),
                    )?;
                }
            } else {
                // General argument binding (CPython 3.12 parity): keyword
                // matching, positional-only / unexpected-keyword diagnostics,
                // defaults, and missing-argument collection.
                let positional_count = args.iter().filter(|arg| arg.name.is_none()).count();
                // Number of params that can accept positional arguments
                // (excludes keyword-only).
                let positional_param_count = function
                    .params
                    .iter()
                    .filter(|p| !p.is_keyword_only)
                    .count();
                // #2395: count required positionals through the override-aware
                // accessor so a reassigned `f.__defaults__` changes the arity
                // reported in "takes N positional arguments" errors.
                let required_positional_count = function
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(i, p)| {
                        !p.is_keyword_only && function.positional_default(*i).is_none()
                    })
                    .count();
                let total_positional_given = positional_count + bound_prefix.len();
                if total_positional_given > positional_param_count {
                    let keyword_only_given = args
                        .iter()
                        .filter_map(|arg| arg.name.as_deref())
                        .filter(|name| is_supplied_keyword_only(&function, name))
                        .count();
                    return Err(too_many_positional_error(
                        &function,
                        required_positional_count,
                        positional_param_count,
                        total_positional_given,
                        keyword_only_given,
                    ));
                }
                // Per-param "already bound" flags (stack-allocated for typical
                // arity — replaces the heap `Vec<Option<Value>>` whose `Some`
                // discriminant previously doubled as this flag).
                let mut bound: smallvec::SmallVec<[bool; 16]> = smallvec![false; nparams];
                // Routes one bound value to its compile-time destination: a frame
                // register (the common case) or — for cell-var params under a local
                // env — an env entry by name.  Marks the param bound.  The body lives
                // in `fast_path.rs::bind_param` (the frame-binding fast path, #2123),
                // taking the frame-local state by reference so the file boundary stays
                // zero-cost (the helper is `#[inline]`).
                for (index, value) in bound_prefix.iter().enumerate() {
                    bind_param(
                        &mut bound,
                        &function,
                        num_regs,
                        &mut regs,
                        &local_env,
                        index,
                        value.clone(),
                    )?;
                }
                let mut positional_index = bound_prefix.len();
                let mut posonly_violations: smallvec::SmallVec<[&str; 4]> =
                    smallvec::SmallVec::new();
                // Deferred unknown-keyword: CPython raises posonly error before
                // unexpected-keyword error when both are present in the same call.
                let mut first_unknown_keyword: Option<&str> = None;
                for arg in args {
                    let value = arg.value.clone();
                    if let Some(name) = &arg.name {
                        let Some(param_index) =
                            function.params.iter().position(|param| param.name == *name)
                        else {
                            // Don't return immediately — a posonly violation earlier
                            // in the arg list must still take priority (CPython 3.12).
                            if first_unknown_keyword.is_none() {
                                first_unknown_keyword = Some(name.as_str());
                            }
                            continue;
                        };
                        if function.params[param_index].is_positional_only {
                            // The fast path only runs when the function has neither
                            // *args nor **kwargs (see the `if !has_args_param &&
                            // !has_kwargs_param` guard above), so there is no
                            // **kwargs to absorb this name — TypeError is correct.
                            // The variadic path (`compute_kw_pos` below) handles
                            // the "absorb into **kwargs" case separately.
                            // Collect all violations so the error lists all names,
                            // matching CPython 3.12: foo() got some positional-only
                            // arguments passed as keyword arguments: 'a, b'
                            posonly_violations.push(name.as_str());
                            continue;
                        }
                        if bound[param_index] {
                            return Err(pyrust_core::type_err!(
                                "{}() got multiple values for argument '{}'",
                                function.effective_qualname(),
                                name
                            ));
                        }
                        bind_param(
                            &mut bound,
                            &function,
                            num_regs,
                            &mut regs,
                            &local_env,
                            param_index,
                            value,
                        )?;
                    } else {
                        // Skip already-bound slots and keyword-only params.
                        while positional_index < nparams
                            && (bound[positional_index]
                                || function.params[positional_index].is_keyword_only)
                        {
                            positional_index += 1;
                        }
                        if positional_index >= nparams
                            || function.params[positional_index].is_keyword_only
                        {
                            let keyword_only_given = args
                                .iter()
                                .filter_map(|arg| arg.name.as_deref())
                                .filter(|name| is_supplied_keyword_only(&function, name))
                                .count();
                            return Err(too_many_positional_error(
                                &function,
                                required_positional_count,
                                positional_param_count,
                                total_positional_given,
                                keyword_only_given,
                            ));
                        }
                        bind_param(
                            &mut bound,
                            &function,
                            num_regs,
                            &mut regs,
                            &local_env,
                            positional_index,
                            value,
                        )?;
                        positional_index += 1;
                    }
                }
                if !posonly_violations.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "{}() got some positional-only arguments passed as keyword arguments: '{}'",
                        function.effective_qualname(),
                        posonly_violations.join(", ")
                    ));
                }
                if let Some(name) = first_unknown_keyword {
                    return Err(pyrust_core::type_err!(
                        "{}() got an unexpected keyword argument '{}'",
                        function.effective_qualname(),
                        name
                    ));
                }
                // Resolve defaults: bind any still-unbound params straight into their
                // destination register/cell.
                // Collect all missing required positional and keyword-only args before
                // raising, so the error groups them all (CPython 3.12 parity).
                let mut missing_positional: smallvec::SmallVec<[&str; 4]> =
                    smallvec::SmallVec::new();
                let mut missing_kwonly: smallvec::SmallVec<[&str; 4]> = smallvec::SmallVec::new();
                for index in 0..nparams {
                    if !bound[index] {
                        // #2395: resolve through the override-aware accessors so a
                        // reassigned `f.__defaults__` / `f.__kwdefaults__` is observed.
                        let default = if function.params[index].is_keyword_only {
                            function.kwonly_default(index)
                        } else {
                            function.positional_default(index)
                        };
                        if let Some(default) = default {
                            bind_param(
                                &mut bound, &function, num_regs, &mut regs, &local_env, index,
                                default,
                            )?;
                        } else if function.params[index].is_keyword_only {
                            missing_kwonly.push(&function.params[index].name);
                        } else {
                            missing_positional.push(&function.params[index].name);
                        }
                    }
                }
                // Use the qualified name (e.g. "Foo.__new__") so the error message
                // matches CPython 3.12: "Foo.__new__() missing 1 required positional
                // argument: 'x'" rather than the bare "__new__()".  `effective_qualname`
                // honours a user-reassigned `f.__qualname__`, which CPython 3.12 also
                // reflects in these messages.
                check_missing_args(
                    &function.effective_qualname(),
                    &missing_positional,
                    &missing_kwonly,
                )?;
            }

            // Arguments are already bound directly into `regs` / `local_env`
            // above (#2123).  Run the register-VM path.
            //
            // NOTE: this body is kept inline (not routed through
            // `run_bound_user_frame`) — extracting it regressed the positional
            // fast path ~3.5% (the hot user-function call path; see CLAUDE.md's
            // frame-setup landmine and the #2382 PR bench table).  The kw
            // fast-bind path (`call_user_function_kw_cached`) uses the extracted
            // copy instead, where the regression doesn't apply.
            {
                let _depth_guard = CallDepthGuard::enter();
                if call_depth() > max_call_depth(self) {
                    return Err(self.recursion_limit_error()?);
                }

                // Swap in the callee's env (the local env built above, or the
                // function's captured env when no local env is needed).
                let previous_env = match local_env {
                    Some(env) => std::mem::replace(&mut self.env, env),
                    None => std::mem::replace(&mut self.env, Rc::clone(&function.env)),
                };

                // Self-reference for recursive calls (only if not a cell var) —
                // bind slot precomputed at compile time (#1918).
                if let Some(slot) = function.self_bind {
                    if slot as usize >= num_regs {
                        return Err(pyrust_core::py_err!(
                            "SystemError",
                            "self-reference register index {} out of range (num_regs={})",
                            slot,
                            num_regs
                        ));
                    }
                    regs[slot as usize] = Value::user_function(Rc::clone(&function));
                }

                // Generator or coroutine function: create a frame rather than
                // executing.  An `async def` body (issue #1039) is always a
                // suspendable frame — even with no `await` — so it returns a
                // coroutine object instead of running synchronously.
                if code.is_generator || code.is_coroutine {
                    // Restore env before capturing it into the frame.
                    // (When `needs_local_env` is false, `gen_env` ==
                    // `function.env` — the GeneratorFrame keeps it alive.)
                    let gen_env = std::mem::replace(&mut self.env, previous_env);
                    let gen_qualname = std::sync::Arc::from(function.effective_qualname().as_str());
                    return Ok(Self::build_generator_value(
                        &code,
                        regs,
                        gen_env,
                        Rc::clone(&function.local_index),
                        std::sync::Arc::from(&function.name[..]),
                        gen_qualname,
                        function.iterable_coroutine.get(),
                    ));
                }

                // Issue #389: publish a view of this function frame so
                // `locals()` can surface its fastlocal registers
                // mid-call.  Popped immediately after `run_bytecode`
                // returns so the raw pointer never outlives `regs`.
                // Issue #486: also capture nonlocal_names and the
                // current env so `snapshot_current_locals` can resolve
                // nonlocal bindings that live in enclosing envs.
                let nonlocal_names_opt = if function.nonlocal_names.is_empty() {
                    None
                } else {
                    Some(Rc::clone(&function.nonlocal_names))
                };
                // Issue #3024: a frame with cell vars must publish its env too —
                // the cells live in the local env created above, not in the
                // register file, so `locals()` can only reach them through here.
                let env_opt = if function.nonlocal_names.is_empty() && code.cell_vars.is_empty() {
                    None
                } else {
                    Some(Rc::clone(&self.env))
                };
                // Capture the raw pointer and length BEFORE constructing RegSlice
                // so both the VmFrameView and the dispatch loop share the same raw
                // pointer with no &mut [Value] in scope (issue #547 / PR #646).
                let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
                let regs_len = regs.len();
                self.vm_frame_views.push(VmFrameView {
                    kind: FrameKind::Function,
                    // SAFETY: SmallVec / Vec allocation is always non-null.
                    // Popped before `regs` is dropped (see above).
                    regs_ptr,
                    regs_len,
                    local_index: Rc::clone(&function.local_index),
                    nonlocal_names: nonlocal_names_opt,
                    env: env_opt,
                    is_class_method: code.is_class_method,
                    function: Some(Rc::clone(&function)),
                    gen_frame: None,
                });
                // SAFETY: regs_ptr is valid for regs_len Values for the lifetime
                // of `regs` (a local RegsBuf that outlives this call).  No
                // &mut [Value] referencing `regs` is held while the dispatch loop
                // runs; RegSlice (raw pointer + len) is used instead, removing
                // the LLVM noalias constraint (issue #547).
                let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
                let vm_result = self.run_bytecode(&code, regs_slice);
                // Lazy traceback: only build + record this frame's `FrameInfo`
                // when the body actually errored.  The no-exception common path
                // does no allocation and touches no traceback thread-local.
                if vm_result.is_err() {
                    // Callee's own code-object filename (#2438): an imported
                    // module's function reports its module's source file.
                    let tb_filename = code.filename.clone();
                    // Capture the source line in this callee where execution
                    // stopped (the callee published it via `set_current_vm_line`
                    // on the way out).  Surfaced to Python as `tb_lineno` /
                    // `f_lineno`; 0 means "no line table" (kept as `None`).
                    let tb_lineno = match pyrust_core::get_current_vm_line() {
                        0 => None,
                        n => Some(n),
                    };
                    pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                        filename: tb_filename,
                        lineno: tb_lineno,
                        source_line: None,
                        funcname: std::sync::Arc::from(&function.name[..]),
                        globals: Some(pyrust_core::FrameGlobals::for_environment(&function.env)),
                        // This callee just escaped, so the published anchor
                        // (#2411) is the col span of the instruction that
                        // propagated the error within this frame.
                        col_span: pyrust_core::get_current_vm_col_span(),
                    });
                }
                self.pop_vm_frame_view();

                let used_env = std::mem::replace(&mut self.env, previous_env);
                if needs_local_env {
                    self.free_env(used_env);
                }
                let value = vm_result?;
                return Ok(value);
            }
        }

        // Variadic path (*args / **kwargs) lives in a helper to keep this
        // function focused on the common no-variadic fast path above.
        self.call_user_function_variadic(function, args, bound_prefix, has_args_param)
    }
}
