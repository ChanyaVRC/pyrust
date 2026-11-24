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
                return Err(PyError::runtime(format!(
                    "no bytecode for '{}'",
                    function.name
                )));
            };
            let num_regs = code.num_regs as usize;
            let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
            let mut cells = allocate_frame_cells(&code, &function.closure_cells);

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
                        &cells,
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
                        &cells,
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
                            &cells,
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
                            &cells,
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
                                &mut bound, &function, num_regs, &mut regs, &cells, index, default,
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

            // Arguments are already bound directly into `regs` / `cells`
            // above (#2123).  Routed through `#[inline(always)]`
            // `run_bound_user_frame_inline` to keep positional-fast-path codegen
            // identical while sharing frame setup, cold traceback recording, and
            // teardown (#3163).
            return self.run_bound_user_frame_inline(&function, &code, &mut regs, &mut cells);
        }

        // Variadic path (*args / **kwargs) lives in a helper to keep this
        // function focused on the common no-variadic fast path above.
        self.call_user_function_variadic(function, args, bound_prefix, has_args_param)
    }
}
