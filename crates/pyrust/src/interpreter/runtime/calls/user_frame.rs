impl Interpreter {
    /// Cold error-exit traceback recorder shared by native user-function frames
    /// (`user_frame`, `expanded_user_call`, `variadic_user_call`) and generator
    /// resume frames (`vm/entry.rs`).
    ///
    /// Keeping this out of line prevents every native call-frame tail from
    /// carrying ~35 lines of traceback/frame-capture code in its hot path.
    #[cold]
    #[inline(never)]
    pub(super) fn record_escaped_vm_frame_traceback(
        &mut self,
        code: &crate::bytecode::FnCode,
        function: &UserFunction,
        prebuilt_funcname: Option<&std::sync::Arc<str>>,
    ) {
        if code.is_inlined_comp {
            // PEP 709 (#2993): list/set/dict/async comprehensions are inlined
            // into their enclosing frame — omit the synthetic `<*comp>` frame
            // and forward the inner PEP 657 caret anchor to the caller.
            self.cold.inlined_comp_escaped_span = Some(pyrust_core::get_current_vm_col_span());
            return;
        }
        // Use the callee's own code-object filename (#2438): an imported
        // module's function reports the module's source file, not the running
        // script's path. `code.filename` is `<unknown>` only for code with no
        // source path (REPL / synthetic), matching the old fallback.
        let tb_filename = code.filename.clone();
        // Capture the source line in this callee where execution stopped (the
        // callee published it via `set_current_vm_line` on the way out).
        // Surfaced to Python as `tb_lineno` / `f_lineno`; 0 means "no line
        // table" (kept as `None`).
        let tb_lineno = match pyrust_core::get_current_vm_line() {
            0 => None,
            n => Some(n),
        };
        let (tb_locals, tb_frame) = self.capture_unwinding_traceback_frame(tb_lineno);
        let funcname = prebuilt_funcname
            .cloned()
            .unwrap_or_else(|| std::sync::Arc::from(&function.name[..]));
        pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
            filename: tb_filename,
            lineno: tb_lineno,
            source_line: None,
            funcname,
            globals: Some(pyrust_core::FrameGlobals::for_environment(&function.env)),
            // This callee just escaped, so the published anchor (#2411 / #2904)
            // is the col span of the instruction that propagated the error
            // within this frame.
            col_span: pyrust_core::get_current_vm_col_span(),
            locals: tb_locals,
            frame: tb_frame,
        });
    }

    /// Execute a user function whose arguments are already bound directly into
    /// `regs` / `cells` (the frame-execution tail shared by positional, keyword,
    /// and variadic call paths, #2123 / #3163).  Swaps in the callee env,
    /// handles the generator/coroutine branch, publishes the `vm_frame_view`,
    /// runs the bytecode, records a traceback frame on error via the shared cold
    /// helper, and restores the env.
    ///
    /// Takes `regs` / `cells` by `&mut` rather than by value to avoid moving the
    /// inline `SmallVec` register buffer across call boundaries (eliminating the
    /// ~3.5–9% frame-setup regression noted on the positional fast path), while
    /// clearing both buffers before restoring `self.env` and finishing the frame
    /// pop so `__del__` drop ordering stays identical.
    #[inline(always)]
    fn run_bound_user_frame_inline(
        &mut self,
        function: &Rc<UserFunction>,
        code: &Rc<crate::bytecode::FnCode>,
        regs: &mut RegsBuf,
        cells: &mut CellsBuf,
    ) -> Result<Value> {
        let num_regs = code.num_regs as usize;
        let _depth_guard = CallDepthGuard::enter();
        if call_depth() > max_call_depth(self) {
            return Err(self.recursion_limit_error()?);
        }

        // Swap in the function's defining namespace env (zero per-call Environment
        // allocation — all own cells and captured freevars live in `cells`, #452).
        let previous_env = std::mem::replace(&mut self.env, Rc::clone(&function.env));

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
            self.env = previous_env;
            let regs_owned = std::mem::take(regs);
            let cells_owned = std::mem::take(cells);
            return Ok(Self::build_generator_value(
                code,
                regs_owned,
                cells_owned,
                function,
            ));
        }

        // Capture the raw pointer and length BEFORE constructing RegSlice
        // so both the VmFrameView and the dispatch loop share the same raw
        // pointer with no &mut [Value] in scope (issue #547 / PR #646).
        let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
        let regs_len = regs.len();
        let cells_slice = CellSlice::from_slice(cells);
        // SAFETY: SmallVec / Vec allocation is always non-null.
        // Popped before `regs` / `cells` are dropped.
        self.vm_frame_views.push(VmFrameView::function_frame(
            regs_ptr,
            regs_len,
            cells_slice,
            Rc::clone(function),
            code.is_class_method,
        ));
        // SAFETY: regs_ptr is valid for regs_len Values for the lifetime
        // of `regs` (a local RegsBuf that outlives this call).  No
        // &mut [Value] referencing `regs` is held while the dispatch loop
        // runs; RegSlice (raw pointer + len) is used instead, removing
        // the LLVM noalias constraint (issue #547).
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        let vm_result = self.run_bytecode_with_cells(code, regs_slice, cells_slice);
        // Lazy traceback: only build + record this frame's `FrameInfo`
        // when the body actually errored.  The no-exception common path
        // does no allocation and touches no traceback thread-local.
        if vm_result.is_err() {
            self.record_escaped_vm_frame_traceback(code, function, None);
        }
        self.detach_vm_frame_view();
        regs.clear();
        cells.clear();
        self.env = previous_env;
        self.finish_pop_vm_frame_view();
        let value = vm_result?;
        Ok(value)
    }

    /// Out-of-line entry point to [`Self::run_bound_user_frame_inline`] for the
    /// keyword-cached and variadic call paths.
    fn run_bound_user_frame(
        &mut self,
        function: &Rc<UserFunction>,
        code: &Rc<crate::bytecode::FnCode>,
        regs: &mut RegsBuf,
        cells: &mut CellsBuf,
    ) -> Result<Value> {
        self.run_bound_user_frame_inline(function, code, regs, cells)
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
        let mut cells = allocate_frame_cells(code, &function.closure_cells);

        // Positionals fill params 0..npos in order.
        for (pi, val) in pos.enumerate().take(npos) {
            bind_param_direct(function, num_regs, &mut regs, &cells, pi, val)?;
        }
        // Keywords bind to their cached slots.
        for (i, val) in kw.enumerate() {
            bind_param_direct(
                function,
                num_regs,
                &mut regs,
                &cells,
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
                    bind_param_direct(function, num_regs, &mut regs, &cells, pi, d)?;
                }
            }
        }

        self.run_bound_user_frame(function, code, &mut regs, &mut cells)
    }
}
