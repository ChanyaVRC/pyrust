impl Compiler {
    /// Reserve a contiguous call-frame window of `slots` temporaries starting
    /// at `self.next_temp`, updating `max_reg` and recording an overflow error
    /// if the register space would wrap.
    fn reserve_call_frame(&mut self, slots: Reg) -> Option<Reg> {
        let base = self.next_temp;
        let Some(frame_top) = base.checked_add(slots) else {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return None;
        };
        self.next_temp = frame_top;
        if frame_top > 0 && frame_top - 1 > self.max_reg {
            self.max_reg = frame_top - 1;
        }
        Some(base)
    }

    /// Compile one call argument expression into its reserved `dst` slot,
    /// retargeting a single-instruction temp result in place to avoid a Move.
    fn compile_call_arg_into(&mut self, value: &Expr, dst: Reg) {
        let saved = self.next_temp;
        let insn_before = self.insns.len();
        let r = self.compile_expr(value);
        if r != dst {
            let single = self.insns.len() == insn_before + 1;
            if single && r >= self.base_temp && self.retarget_last(r, dst) {
                // retargeted in place — no Move needed
            } else {
                self.emit(Insn::Move(dst, r));
            }
        }
        self.next_temp = saved;
    }

    /// Reserve the register frame for a method call (`CallMethod` /
    /// `CallMethodKw`) and materialize the receiver when needed.
    ///
    /// When the receiver is a plain fast-local variable outside a class body,
    /// its register is used directly as `obj_reg` so in-place mutations
    /// (append, pop, …) persist, and the result goes into a distinct `dst_reg`.
    /// Returns `(obj_reg, dst_reg, args_base)`.
    fn setup_method_receiver_frame(
        &mut self,
        target: &Expr,
        nargs: Reg,
    ) -> Option<(Reg, Reg, Reg)> {
        let slots = 1u32.checked_add(nargs)?;
        let (obj_reg, dst_reg, args_base, need_copy) = if let Expr::Var(name, _) = target
            && let Some(local) = self.local_reg(name).filter(|_| !self.is_class_body)
        {
            let dst = self.reserve_call_frame(slots)?;
            (local, dst, dst + 1, false)
        } else {
            let o = self.reserve_call_frame(slots)?;
            (o, o, o + 1, true)
        };
        if need_copy {
            let saved = self.next_temp;
            self.compile_expr_into(target, obj_reg);
            self.next_temp = saved;
        }
        Some((obj_reg, dst_reg, args_base))
    }

    fn compile_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        // PEP 657 caret anchor (#2411 / #2443) for the whole `callee(...)` span.
        // Armed immediately before the terminal call instruction on the simple
        // positional, keyword, and method paths so an error propagated through
        // any of these draws its caret on the call site.  Splat paths
        // (`f(*a)` / `f(**d)`) stay caret-free.
        span: Option<crate::ast::CaretSpan>,
        lineno: u32,
    ) -> Reg {
        let has_splat = args.iter().any(|a| a.splat || a.double_splat);
        let has_kwargs = args.iter().any(|a| a.name.is_some());

        // Keyword call with no `*args` / `**kwargs` splats and a non-method
        // callee: emit `CallKw` (#2382).
        if has_kwargs && !has_splat && !matches!(func, Expr::Attr { .. }) {
            return self.compile_keyword_call(func, args, span, lineno);
        }

        // Keyword method call `obj.m(<pos…>, k=v…)` with no splats: emit
        // `CallMethodKw` (#2392).
        if has_kwargs
            && !has_splat
            && let Expr::Attr { target, name, .. } = func
        {
            return self.compile_keyword_method_call(target, name, args, span, lineno);
        }

        // Double-splat expansion `f(<pos…>, **d)`: emit `CallEx` (#2393).
        if let Some(npos) = double_splat_fast_shape(args)
            && npos <= u8::MAX as usize
            && !matches!(func, Expr::Attr { .. })
        {
            return self.compile_double_splat_call(func, args, npos);
        }

        // Positional-splat expansion `f(<pos…>, *args[, **kw])`: emit `CallExArgs`.
        if let Some((npos, nkw)) = positional_splat_fast_shape(args)
            && !matches!(func, Expr::Attr { .. })
        {
            return self.compile_positional_splat_call(func, args, npos, nkw);
        }

        if has_splat || has_kwargs {
            return self.compile_variadic_call(func, args);
        }

        // `Call` / `CallMethod` encode their positional count in a `u8`.
        if args.len() > u8::MAX as usize {
            return self.compile_variadic_call(func, args);
        }

        // Detect obj.method(args) — emit CallMethod to allow in-place mutation.
        if let Expr::Attr { target, name, .. } = func {
            return self.compile_method_call(target, name, args, span, lineno);
        }

        let argc = args.len() as u8;
        let Some(func_reg) = self.reserve_call_frame(1 + Reg::from(argc)) else {
            return 0;
        };
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        for (i, arg) in (0u32..).zip(args.iter()) {
            self.compile_call_arg_into(&arg.value, func_reg + 1 + i);
        }
        let is_pure_callee =
            matches!(func, Expr::Var(n, _) if self.pure_locals.contains(n.as_str()));
        // Arm the `callee(...)` caret anchor and call-start line (#2411 / #2992).
        let saved_lineno = self.current_lineno;
        if lineno != 0 {
            self.set_lineno(lineno);
        }
        self.set_col_span_for_next(span);
        if is_pure_callee {
            self.emit(Insn::CallMemo(func_reg, argc));
        } else {
            self.emit(Insn::Call(func_reg, argc));
        }
        self.set_lineno(saved_lineno);
        self.next_temp = func_reg + 1;
        func_reg
    }

    /// Compile a keyword-argument call (no splats, non-method callee) into a
    /// `CallKw` instruction (issue #2382).
    fn compile_keyword_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        span: Option<crate::ast::CaretSpan>,
        lineno: u32,
    ) -> Reg {
        let total = args.len();
        if total > u8::MAX as usize {
            return self.compile_variadic_call(func, args);
        }
        let nkw = args.iter().filter(|a| a.name.is_some()).count();

        let kw_names: Vec<Value> = args
            .iter()
            .filter_map(|a| a.name)
            .map(|n| Value::string(n.as_str()))
            .collect();
        let kwnames_idx = self.intern_const(Value::tuple(kw_names));

        let Some(func_reg) = self.reserve_call_frame(1 + total as Reg) else {
            return 0;
        };
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        for (i, arg) in (0u32..).zip(args.iter()) {
            self.compile_call_arg_into(&arg.value, func_reg + 1 + i);
        }
        let saved_lineno = self.current_lineno;
        if lineno != 0 {
            self.set_lineno(lineno);
        }
        self.set_col_span_for_next(span);
        self.emit(Insn::CallKw {
            func: func_reg,
            total: total as u8,
            nkw: nkw as u8,
            kwnames_idx,
        });
        self.set_lineno(saved_lineno);
        self.next_temp = func_reg + 1;
        func_reg
    }

    /// Compile a keyword-argument method call `obj.m(<pos…>, k=v…)` (no splats)
    /// into a `CallMethodKw` instruction (issue #2392).
    fn compile_keyword_method_call(
        &mut self,
        target: &Expr,
        method_name: &str,
        args: &[crate::ast::CallArg],
        span: Option<crate::ast::CaretSpan>,
        lineno: u32,
    ) -> Reg {
        let total = args.len();
        if total > u8::MAX as usize {
            return self.compile_variadic_call(
                &Expr::Attr {
                    target: Box::new(target.clone()),
                    name: Symbol::intern(method_name),
                    span: None,
                },
                args,
            );
        }
        let nkw = args.iter().filter(|a| a.name.is_some()).count();
        let kw_names: Vec<Value> = args
            .iter()
            .filter_map(|a| a.name)
            .map(|n| Value::string(n.as_str()))
            .collect();
        let kwnames_idx = self.intern_const(Value::tuple(kw_names));

        let Some((obj_reg, dst_reg, args_base)) =
            self.setup_method_receiver_frame(target, total as Reg)
        else {
            return 0;
        };
        for (i, arg) in (0u32..).zip(args.iter()) {
            self.compile_call_arg_into(&arg.value, args_base + i);
        }
        let name_idx = self.intern_name(method_name);
        let saved_lineno = self.current_lineno;
        if lineno != 0 {
            self.set_lineno(lineno);
        }
        self.set_col_span_for_next(span);
        self.emit(Insn::CallMethodKw {
            dst: dst_reg,
            obj: obj_reg,
            name_idx,
            args_base,
            total: total as u8,
            nkw: nkw as u8,
            kwnames_idx,
        });
        self.set_lineno(saved_lineno);
        self.next_temp = dst_reg + 1;
        dst_reg
    }

    fn compile_method_call(
        &mut self,
        target: &Expr,
        method_name: &str,
        args: &[crate::ast::CallArg],
        span: Option<crate::ast::CaretSpan>,
        lineno: u32,
    ) -> Reg {
        let nargs = args.len() as u8;
        let Some((obj_reg, dst_reg, args_base)) =
            self.setup_method_receiver_frame(target, Reg::from(nargs))
        else {
            return 0;
        };
        for (i, arg) in (0u32..).zip(args.iter()) {
            self.compile_call_arg_into(&arg.value, args_base + i);
        }
        let name_idx = self.intern_name(method_name);
        let saved_lineno = self.current_lineno;
        if lineno != 0 {
            self.set_lineno(lineno);
        }
        self.set_col_span_for_next(span);
        self.emit(Insn::CallMethod {
            dst: dst_reg,
            obj: obj_reg,
            name_idx,
            args_base,
            nargs,
        });
        self.set_lineno(saved_lineno);
        self.next_temp = dst_reg + 1;
        dst_reg
    }
}
