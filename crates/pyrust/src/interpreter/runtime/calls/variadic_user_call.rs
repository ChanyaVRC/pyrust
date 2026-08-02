impl Interpreter {
    /// Variadic argument-binding + execution path for
    /// `call_user_function_expanded` — the `*args` / `**kwargs` case.
    /// Extracted verbatim (size reduction); the fast no-variadic path stays
    /// inline in the caller and returns before reaching this helper.
    fn call_user_function_variadic(
        &mut self,
        function: Rc<UserFunction>,
        args: &[ExpandedCallArg],
        bound_prefix: &[Value],
        has_args_param: bool,
    ) -> Result<Value> {
        // Variadic path: handle *args and **kwargs
        // Gather positional and keyword args
        let mut positional_vals: Vec<Value> = bound_prefix.to_vec();
        let mut keyword_vals: Vec<(String, Value)> = Vec::new();
        for arg in args {
            if let Some(name) = &arg.name {
                keyword_vals.push((name.clone(), arg.value.clone()));
            } else {
                positional_vals.push(arg.value.clone());
            }
        }
        self.call_user_function_variadic_split(
            function,
            positional_vals,
            keyword_vals,
            has_args_param,
        )
    }

    /// Bind pre-split positional / keyword argument vectors into a variadic
    /// callee's frame and run it.  The tail of `call_user_function_variadic`,
    /// factored out (#2841 follow-up) so the `CallExArgs` positional-splat
    /// handler can feed `positional_vals` (leading positionals + `*args` splat
    /// elements) and `keyword_vals` (the `**kw` dict entries) STRAIGHT in —
    /// skipping the `ExpandedCallArg` buffer and the second per-arg clone the
    /// general path pays to split that buffer back into these two vectors.
    ///
    /// `has_args_param` is whether the callee has a `*args` parameter (drives the
    /// excess-positional pre-check ordering, matching CPython).
    pub(super) fn call_user_function_variadic_split(
        &mut self,
        function: Rc<UserFunction>,
        positional_vals: Vec<Value>,
        keyword_vals: Vec<(String, Value)>,
        has_args_param: bool,
    ) -> Result<Value> {
        // Pre-check: reject excess positional arguments before binding when
        // there is no *args to absorb them. This matches CPython's error ordering.
        if !has_args_param {
            let positional_param_count = function
                .params
                .iter()
                .filter(|p| !p.is_keyword_only && !p.is_args && !p.is_kwargs)
                .count();
            // #2395: override-aware required-positional count (see fast path).
            let required_positional_count = function
                .params
                .iter()
                .enumerate()
                .filter(|(i, p)| {
                    !p.is_keyword_only
                        && !p.is_args
                        && !p.is_kwargs
                        && function.positional_default(*i).is_none()
                })
                .count();
            if positional_vals.len() > positional_param_count {
                let keyword_only_given = keyword_vals
                    .iter()
                    .filter(|(name, _)| is_supplied_keyword_only(&function, name))
                    .count();
                return Err(too_many_positional_error(
                    &function,
                    required_positional_count,
                    positional_param_count,
                    positional_vals.len(),
                    keyword_only_given,
                ));
            }
        }

        let has_kwargs = function.params.iter().any(|p| p.is_kwargs);

        // "got multiple values for argument" (CPython 3.12): a keyword that names
        // a param already filled by a positional argument.  Positionals fill the
        // leading non-keyword-only params left-to-right, so the j-th such param is
        // positionally filled iff `j < positional_vals.len()`.  CPython checks the
        // GIVEN keywords in order (not param order) and reports the first colliding
        // one, ahead of the missing-arg and unexpected-keyword diagnostics below.
        for (name, _) in &keyword_vals {
            let mut pos_slot = 0usize;
            for param in &function.params {
                if param.is_args || param.is_kwargs || param.is_keyword_only {
                    continue;
                }
                if &param.name == name {
                    if !param.is_positional_only && pos_slot < positional_vals.len() {
                        return Err(pyrust_core::type_err!(
                            "{}() got multiple values for argument '{}'",
                            function.effective_qualname(),
                            name
                        ));
                    }
                    break;
                }
                pos_slot += 1;
            }
        }

        let mut consumed_keywords = std::collections::HashSet::new();
        let mut pos_idx = 0;
        let mut param_vals: Vec<Value> = Vec::with_capacity(function.params.len());
        // Collect all missing required args before raising, so the error groups
        // them all (CPython 3.12 parity).
        let mut missing_positional: smallvec::SmallVec<[&str; 4]> = smallvec::SmallVec::new();
        let mut missing_kwonly: smallvec::SmallVec<[&str; 4]> = smallvec::SmallVec::new();

        for (param_index, param) in function.params.iter().enumerate() {
            let value = if param.is_args {
                let rest = positional_vals[pos_idx..].to_vec();
                pos_idx = positional_vals.len();
                Value::tuple(rest)
            } else if param.is_kwargs {
                let mut dict: PyDict = PyDict::default();
                for (k, v) in &keyword_vals {
                    if !consumed_keywords.contains(k)
                        && let Some(key) = Value::string(k.clone()).to_key()
                    {
                        dict.insert(key, v.clone());
                    }
                }
                Value::dict(dict)
            } else {
                let kw_pos = if param.is_positional_only {
                    None
                } else {
                    keyword_vals.iter().position(|(k, _)| k == &param.name)
                };
                if let Some(ki) = kw_pos {
                    consumed_keywords.insert(keyword_vals[ki].0.clone());
                    keyword_vals[ki].1.clone()
                } else if !param.is_keyword_only && pos_idx < positional_vals.len() {
                    let v = positional_vals[pos_idx].clone();
                    pos_idx += 1;
                    v
                } else if let Some(d) = if param.is_keyword_only {
                    // #2395: observe a reassigned `f.__kwdefaults__` / `f.__defaults__`.
                    function.kwonly_default(param_index)
                } else {
                    function.positional_default(param_index)
                } {
                    d
                } else if param.is_keyword_only {
                    missing_kwonly.push(&param.name);
                    Value::unset()
                } else {
                    missing_positional.push(&param.name);
                    Value::unset()
                }
            };
            param_vals.push(value);
        }

        // Report positional missing args first; only report kwonly if all
        // positional params were satisfied (matching CPython 3.12 behaviour).
        check_missing_args(
            &function.effective_qualname(),
            &missing_positional,
            &missing_kwonly,
        )?;
        // These hold `&str` borrows of `function.params`; drop them so `function`
        // can be moved into `bind_and_run_variadic_frame` at the tail.
        drop(missing_positional);
        drop(missing_kwonly);

        if !has_kwargs {
            // First pass: collect all positional-only violations so the error
            // lists every offending name, matching CPython 3.12 parity.
            let posonly_violations: smallvec::SmallVec<[&str; 4]> = keyword_vals
                .iter()
                .filter(|(name, _)| {
                    !consumed_keywords.contains(name)
                        && function
                            .params
                            .iter()
                            .any(|p| p.is_positional_only && &p.name == name)
                })
                .map(|(name, _)| name.as_str())
                .collect();
            if !posonly_violations.is_empty() {
                return Err(pyrust_core::type_err!(
                    "{}() got some positional-only arguments passed as keyword arguments: '{}'",
                    function.effective_qualname(),
                    posonly_violations.join(", ")
                ));
            }
            // Second pass: check for entirely unexpected keyword arguments.
            for (name, _) in &keyword_vals {
                if !consumed_keywords.contains(name) {
                    return Err(pyrust_core::type_err!(
                        "{}() got an unexpected keyword argument '{}'",
                        function.effective_qualname(),
                        name
                    ));
                }
            }
        }

        // Bind the resolved per-param values into the callee frame and run it.
        self.bind_and_run_variadic_frame(function, &mut param_vals)
    }

    /// Bind an already-resolved, param-index-aligned `param_vals` slice into a
    /// variadic callee's register file / local env and run the frame.  This is
    /// the shared frame-run tail of `call_user_function_variadic_split` (the
    /// argument-resolution loop precedes it) and of the #2852 pure-forward
    /// direct-bind path (which fills a stack `param_vals` — just the `*A` tuple
    /// and optional `**K` dict — without that loop, so no heap `Vec`).
    /// `param_vals[i]` is the bound value for `function.params[i]`; each is MOVED
    /// into its `Reg`/`Cell` destination (leaving `unset`), so the caller must
    /// not reuse it after.
    pub(super) fn bind_and_run_variadic_frame(
        &mut self,
        function: Rc<UserFunction>,
        param_vals: &mut [Value],
    ) -> Result<Value> {
        // Now run via VM (same as non-variadic Tier-0 path)
        if let Some(code) = self.get_or_compile_bytecode(&function) {
            let num_regs = code.num_regs as usize;
            let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];

            // Bind non-cell params into register file using precomputed slots
            // (#1918).  Cell-var params are inserted into the env below.  Each
            // param binds to EITHER a register OR a cell, so we move the value
            // out of `param_vals` (leaving `unset`) instead of cloning — the
            // second clone (source -> param_vals -> regs) was pure overhead on
            // every variadic call.  The cell loop below moves the remaining
            // (cell) entries.
            for (param_index, bind) in function.param_binds.iter().enumerate() {
                if let pyrust_core::ParamBind::Reg(slot) = *bind {
                    if (slot as usize) >= num_regs {
                        return Err(pyrust_core::py_err!(
                            "SystemError",
                            "parameter '{}' register index {} out of range (num_regs={})",
                            function.params[param_index].name,
                            slot,
                            num_regs
                        ));
                    }
                    regs[slot as usize] =
                        std::mem::replace(&mut param_vals[param_index], Value::unset());
                }
            }
            // Self-reference for recursive calls (only if not a cell var).
            if let Some(slot) = function.self_bind {
                if (slot as usize) >= num_regs {
                    return Err(pyrust_core::py_err!(
                        "SystemError",
                        "self-reference register index {} out of range (num_regs={})",
                        slot,
                        num_regs
                    ));
                }
                regs[slot as usize] = Value::user_function(Rc::clone(&function));
            }

            let _depth_guard = CallDepthGuard::enter();
            if call_depth() > max_call_depth(self) {
                return Err(self.recursion_limit_error()?);
            }

            // Create a local env when the function uses globals, nonlocals, or cell vars.
            let needs_local_env = !function.global_names.is_empty()
                || !function.nonlocal_names.is_empty()
                || !code.cell_vars.is_empty();

            let previous_env = if needs_local_env {
                let local_env = self.alloc_env(Some(Rc::clone(&function.env)));
                {
                    let mut e = local_env.borrow_mut();
                    e.local_names = Rc::clone(&function.local_names);
                    e.global_names = Rc::clone(&function.global_names);
                    e.nonlocal_names = Rc::clone(&function.nonlocal_names);
                    // Store cell var params in the env so inner closures can
                    // capture them.  Move out of `param_vals` (the reg loop above
                    // only consumed the `Reg`-bound entries, so cell entries are
                    // still intact) to avoid the redundant clone.
                    for (param_index, bind) in function.param_binds.iter().enumerate() {
                        if *bind == pyrust_core::ParamBind::Cell {
                            e.values.insert(
                                &function.params[param_index].name,
                                std::mem::replace(&mut param_vals[param_index], Value::unset()),
                            );
                        }
                    }
                }
                std::mem::replace(&mut self.env, local_env)
            } else {
                std::mem::replace(&mut self.env, Rc::clone(&function.env))
            };

            // Issue #488: variadic generator functions (`def g(*args):
            // yield ...` and friends) must also be wrapped in a
            // GeneratorFrame instead of executed synchronously — the
            // simple-path branch already does this above; mirror it here
            // so the body's `yield` isn't observed as a runtime error.
            // Coroutines (`async def`, issue #1039) take the same path.
            if code.is_generator || code.is_coroutine {
                let gen_env = std::mem::replace(&mut self.env, previous_env);
                let gen_qualname = std::sync::Arc::from(function.effective_qualname().as_str());
                return Ok(Self::build_generator_value(
                    &code,
                    regs,
                    gen_env,
                    Rc::clone(&function.local_index),
                    std::sync::Arc::from(&function.name[..]),
                    gen_qualname,
                ));
            }

            // Issue #389: publish a function frame view (see the
            // matching push in the simple-path branch above).
            // Issue #486: nonlocal_names + env for nonlocal resolution.
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
            // when the body actually errored (see the simple-call path above).
            if vm_result.is_err() {
                // Callee's own code-object filename (#2438): an imported module's
                // function reports its module's source file.
                let tb_filename = code.filename.clone();
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
                    // This callee just escaped, so the published anchor (#2411)
                    // is the col span of the instruction that propagated the
                    // error within this frame.
                    col_span: pyrust_core::get_current_vm_col_span(),
                });
            }
            self.vm_frame_views.pop();

            let used_env = std::mem::replace(&mut self.env, previous_env);
            if needs_local_env {
                self.free_env(used_env);
            }
            return vm_result;
        }

        // All user functions must have precompiled bytecode
        Err(PyError::Runtime(format!(
            "no bytecode for '{}'",
            function.name
        )))
    }
}
