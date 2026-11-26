/// Positional-only fast bind for a `*args`-only callee (#3422).
///
/// The positional argument sequence is `receiver` (a bound method's `self`,
/// when present) followed by `args`; `function` must satisfy
/// [`Interpreter::is_varargs_positional_callee`] and the call supplies no
/// keywords.  Each bound value is cloned once out of `args`, which may be
/// borrowed straight from the caller's argument registers: the borrow ends when
/// this returns, before the callee frame runs.
///
/// That shape has exactly one possible binding error — a missing required
/// positional — so the general variadic binder's `positional_vals` /
/// `keyword_vals` / `param_vals` vectors, its keyword matching and its
/// unexpected-keyword diagnostics are all skipped: the fixed parameters bind
/// straight into their register/cell slots (missing trailing ones from the
/// override-aware defaults), and the surplus is packed into one fresh tuple for
/// `*args` (the shared empty singleton when there is none, inline storage for 2
/// or 3 elements).  CPython 3.12 reports missing positionals before building
/// `*args`, and so does this.
#[inline(always)]
pub(crate) fn bind_varargs_positional(
    function: &Rc<UserFunction>,
    num_regs: usize,
    regs: &mut RegsBuf,
    cells: &CellsBuf,
    receiver: Option<&Value>,
    args: &[Value],
) -> Result<()> {
    debug_assert!(Interpreter::is_varargs_positional_callee(function));
    let nfixed = function.params.len() - 1;
    let offset = usize::from(receiver.is_some());
    let nbound = (offset + args.len()).min(nfixed);
    if let Some(receiver) = receiver
        && nfixed > 0
    {
        bind_param_direct(function, num_regs, regs, cells, 0, receiver.clone())?;
    }
    // Positionals from `args` that land in fixed parameters.
    let args_bound = nbound.saturating_sub(offset);
    for (i, value) in args[..args_bound].iter().enumerate() {
        bind_param_direct(function, num_regs, regs, cells, offset + i, value.clone())?;
    }
    if nbound < nfixed {
        bind_varargs_missing_defaults(function, num_regs, regs, cells, nbound)?;
    }
    // Surplus positionals → `*args`.  Only a receiver bound to a method with
    // no fixed parameter (`def m(*args)`) lands in the tuple itself.
    let star_args = match receiver {
        Some(receiver) if nfixed == 0 => {
            let mut items = Vec::with_capacity(1 + args.len());
            items.push(receiver.clone());
            items.extend_from_slice(args);
            Value::tuple(items)
        }
        _ => Value::tuple_from_slice(&args[args_bound..]),
    };
    bind_param_direct(function, num_regs, regs, cells, nfixed, star_args)
}

/// Fill the fixed parameters `nbound..` of a `*args`-only callee that the call
/// left unsupplied from their (override-aware, #2395) defaults, raising CPython's
/// grouped "missing N required positional argument(s)" `TypeError` otherwise.
/// Out of line: an exact-or-surplus call never reaches it.
#[inline(never)]
fn bind_varargs_missing_defaults(
    function: &Rc<UserFunction>,
    num_regs: usize,
    regs: &mut RegsBuf,
    cells: &CellsBuf,
    nbound: usize,
) -> Result<()> {
    let nfixed = function.params.len() - 1;
    let mut missing: smallvec::SmallVec<[&str; 4]> = smallvec::SmallVec::new();
    for pi in nbound..nfixed {
        match function.positional_default(pi) {
            Some(default) => bind_param_direct(function, num_regs, regs, cells, pi, default)?,
            None => missing.push(&function.params[pi].name),
        }
    }
    check_missing_args(&function.effective_qualname(), &missing, &[])
}

impl Interpreter {
    /// Is `function` a positional-`*args`-only callee (#3422): its LAST
    /// parameter is `*args`?  Python's parameter order (positional-only,
    /// positional-or-keyword, `*args`, keyword-only, `**kwargs`) makes that one
    /// check equivalent to "has `*args`, no keyword-only parameter, no
    /// `**kwargs`" — the shape [`bind_varargs_positional`] binds.
    #[inline(always)]
    pub(crate) fn is_varargs_positional_callee(function: &UserFunction) -> bool {
        function.params.last().is_some_and(|p| p.is_args)
    }

    /// `Insn::Call` into a `*args`-only callee (#3422): a plain function
    /// (`receiver` = `None`) or a bound method such as the `__exit__` a `with`
    /// statement fetched (`receiver` = its `self`), whose `argc` positional
    /// arguments sit in the caller's registers `func_reg + 1 ..`.
    ///
    /// Binds straight from those registers into the callee frame, skipping the
    /// `ExpandedCallArg` buffer, the builtin call-site cache probe and the
    /// generic callable dispatch that `call_positional_cached` would run before
    /// reaching the same binder.  An unset argument register is reported exactly
    /// as that path does (via `vm_read`); the caller's line is published on
    /// `call_site_lines` for the callee's frame introspection, as there.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn call_varargs_positional_from_regs(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        func_reg: crate::bytecode::Reg,
        argc: u8,
        function: &Rc<UserFunction>,
        receiver: Option<&Value>,
        code: &crate::bytecode::FnCode,
        call_site: usize,
    ) -> Result<Value> {
        let base = func_reg as usize + 1;
        let end = base + argc as usize;
        if let Some(offset) = (**regs)[base..end].iter().position(Value::is_unset) {
            vm_read(
                regs,
                func_reg + 1 + offset as crate::bytecode::Reg,
                num_locals,
            )?;
        }
        let Some(callee_code) = self.get_or_compile_bytecode(function) else {
            return Err(PyError::runtime(format!(
                "no bytecode for '{}'",
                function.name
            )));
        };
        let num_regs = callee_code.num_regs as usize;
        let mut callee_regs: RegsBuf = smallvec![Value::unset(); num_regs];
        let mut cells = allocate_frame_cells(&callee_code, &function.closure_cells);
        // The borrow of the caller's argument registers ends here, before the
        // callee runs (it may write caller frames through frame views).
        bind_varargs_positional(
            function,
            num_regs,
            &mut callee_regs,
            &cells,
            receiver,
            &(**regs)[base..end],
        )?;
        let line = code.line_at(call_site);
        if line != 0 {
            self.cold
                .call_site_lines
                .push((regs.non_null_ptr().as_ptr(), line));
        }
        let result =
            self.run_bound_user_frame_inline(function, &callee_code, &mut callee_regs, &mut cells);
        if line != 0 {
            self.cold.call_site_lines.pop();
        }
        result
    }

    /// Run a `*args`-only callee called positionally (see
    /// [`bind_varargs_positional`]) — the #3422 fast path of
    /// `call_user_function_expanded` and of the `CallMethod` inline-cache hit.
    /// `args` must not borrow a live VM register file (it is a parameter of the
    /// frame run), so callers pass an owned argument buffer.
    pub(crate) fn call_varargs_positional(
        &mut self,
        function: &Rc<UserFunction>,
        receiver: Option<&Value>,
        args: &[Value],
    ) -> Result<Value> {
        let Some(code) = self.get_or_compile_bytecode(function) else {
            return Err(PyError::runtime(format!(
                "no bytecode for '{}'",
                function.name
            )));
        };
        let code = &code;
        let num_regs = code.num_regs as usize;
        let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
        let mut cells = allocate_frame_cells(code, &function.closure_cells);
        bind_varargs_positional(function, num_regs, &mut regs, &cells, receiver, args)?;
        self.run_bound_user_frame_inline(function, code, &mut regs, &mut cells)
    }

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
                    if !consumed_keywords.contains(k) {
                        dict.insert(PyKey::str_from(k), v.clone());
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
            let mut cells = allocate_frame_cells(&code, &function.closure_cells);

            // Bind each parameter directly into its precomputed register or
            // indexed cell slot (#1918 / #452), moving out of `param_vals`
            // (leaving `unset`) to avoid a redundant per-arg clone.
            for (param_index, bind) in function.param_binds.iter().enumerate() {
                match *bind {
                    pyrust_core::ParamBind::Reg(slot) => {
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
                    pyrust_core::ParamBind::Cell(cell_idx) => {
                        cells[cell_idx as usize].set(std::mem::replace(
                            &mut param_vals[param_index],
                            Value::unset(),
                        ));
                    }
                    pyrust_core::ParamBind::None => {}
                }
            }

            return self.run_bound_user_frame(&function, &code, &mut regs, &mut cells);
        }

        // All user functions must have precompiled bytecode
        Err(PyError::runtime(format!(
            "no bytecode for '{}'",
            function.name
        )))
    }
}
