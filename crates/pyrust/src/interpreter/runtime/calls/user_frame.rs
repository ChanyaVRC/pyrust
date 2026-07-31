impl Interpreter {
    /// Execute a user function whose arguments are already bound directly into
    /// `regs` / `local_env` (the no-variadic fast path's frame-execution tail,
    /// #2123).  Swaps in the callee env, handles the generator/coroutine branch,
    /// publishes the `vm_frame_view`, runs the bytecode, records a traceback
    /// frame on error, and restores the env.
    ///
    /// A copy of `call_user_function_expanded`'s frame-execution tail, used by
    /// the keyword-call fast bind (#2382) after binding arguments by cached slot.
    /// Kept as a separate method (rather than having the positional path call it)
    /// because extracting the positional path's tail regressed it ~3.5% — the
    /// frame-setup landmine in CLAUDE.md.  Maintains the same `vm_frame_views`
    /// push/pop invariant as the inline copy.
    fn run_bound_user_frame(
        &mut self,
        function: &Rc<UserFunction>,
        code: &Rc<crate::bytecode::FnCode>,
        mut regs: RegsBuf,
        local_env: Option<EnvRef>,
        needs_local_env: bool,
    ) -> Result<Value> {
        let num_regs = code.num_regs as usize;
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
            regs[slot as usize] = Value::user_function(Rc::clone(function));
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
                code,
                regs,
                gen_env,
                Rc::clone(&function.local_index),
                std::sync::Arc::from(&function.name[..]),
                gen_qualname,
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
        // Issue #3024: a frame with cell vars must publish its env too — the
        // cells live in the local env created above, not in the register file,
        // so `locals()` can only reach them through here.
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
            function: Some(Rc::clone(function)),
            gen_frame: None,
        });
        // SAFETY: regs_ptr is valid for regs_len Values for the lifetime
        // of `regs` (a local RegsBuf that outlives this call).  No
        // &mut [Value] referencing `regs` is held while the dispatch loop
        // runs; RegSlice (raw pointer + len) is used instead, removing
        // the LLVM noalias constraint (issue #547).
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        let vm_result = self.run_bytecode(code, regs_slice);
        // Lazy traceback: only build + record this frame's `FrameInfo`
        // when the body actually errored.  The no-exception common path
        // does no allocation and touches no traceback thread-local.
        if vm_result.is_err() {
            // Use the callee's own code-object filename (#2438): an imported
            // module's function reports the module's source file, not the running
            // script's path.  `code.filename` is `<unknown>` only for code with
            // no source path (REPL / synthetic), matching the old fallback.
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
                // This callee just escaped, so the published anchor (#2411) is
                // the col span of the instruction that propagated the error
                // within this frame.
                col_span: pyrust_core::get_current_vm_col_span(),
            });
        }
        self.vm_frame_views.pop();

        let used_env = std::mem::replace(&mut self.env, previous_env);
        if needs_local_env {
            self.free_env(used_env);
        }
        let value = vm_result?;
        Ok(value)
    }

    /// Keyword-call fast bind (#2382).  Binds `npos` positional values
    /// (`pos[0..npos]`) into the leading parameters and each keyword value
    /// (`kw[i]`) into the parameter slot `slots[i]`, fills defaults for any
    /// still-unbound parameter, then runs the frame.  The caller (`Insn::CallKw`)
    /// only takes this path on a *validated cache hit* — the slot mapping was
    /// already proven to bind every parameter exactly once with no missing
    /// required argument (see `KwCallCacheEntry::Simple`), so no per-call
    /// diagnostics are needed here.  `function` has no `*args` / `**kwargs` and
    /// no positional-only/keyword-only conflicts (the cache fill rejects those).
    ///
    /// `slots[i]` is the parameter index for keyword `i`.  `pos` and `kw` are the
    /// argument values in call order; `pos.len() == npos` and `kw.len() ==
    /// slots.len()`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn call_user_function_kw_cached(
        &mut self,
        function: &Rc<UserFunction>,
        code: &Rc<crate::bytecode::FnCode>,
        npos: usize,
        pos: &mut dyn Iterator<Item = Value>,
        slots: &[u32],
        kw: &mut dyn Iterator<Item = Value>,
    ) -> Result<Value> {
        let num_regs = code.num_regs as usize;
        let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];

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

        // Positionals fill params 0..npos in order.
        for (pi, val) in pos.enumerate().take(npos) {
            bind_param_direct(function, num_regs, &mut regs, &local_env, pi, val)?;
        }
        // Keywords bind to their cached slots.
        for (i, val) in kw.enumerate() {
            bind_param_direct(
                function,
                num_regs,
                &mut regs,
                &local_env,
                slots[i] as usize,
                val,
            )?;
        }
        // Defaults for any parameter not filled by a positional or keyword.  Use
        // override-aware accessors so a reassigned `__defaults__`/`__kwdefaults__`
        // is observed even when the kw-call cache was built before the override.
        let nparams = function.params.len();
        let nkw = slots.len();
        for pi in 0..nparams {
            let filled_positionally = pi < npos;
            let filled_by_kw = slots[..nkw].contains(&(pi as u32));
            if !filled_positionally && !filled_by_kw {
                let default = if function.params[pi].is_keyword_only {
                    function.kwonly_default(pi)
                } else {
                    function.positional_default(pi)
                };
                if let Some(d) = default {
                    bind_param_direct(function, num_regs, &mut regs, &local_env, pi, d)?;
                }
            }
        }

        self.run_bound_user_frame(function, code, regs, local_env, needs_local_env)
    }
}
