impl Compiler {
    fn compile_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        // PEP 657 caret anchor (#2411 / #2443) for the whole `callee(...)` span.
        // Armed immediately before the terminal call instruction on the simple
        // positional, keyword, and method paths so an error propagated through
        // any of these draws its caret on the call site (#2443 stage 2 lists
        // `a.b()` / `f(a=5)` explicitly).  Splat paths (`f(*a)` / `f(**d)`) stay
        // caret-free (safe — a missing caret beats a wrong one).
        span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        // Check for any splat args — these require a variadic call path.
        let has_splat = args.iter().any(|a| a.splat || a.double_splat);
        let has_kwargs = args.iter().any(|a| a.name.is_some());

        // Keyword call with no `*args` / `**kwargs` splats and a non-method
        // callee: lay the arguments out contiguously in registers and emit a
        // `CallKw` (issue #2382), skipping the BuildDict + BuildList + hidden-helper
        // round-trip the generic variadic path uses.  Method keyword calls
        // (`obj.m(a=1)`) still take the variadic path so in-place mutation of
        // the receiver register is preserved.
        if has_kwargs && !has_splat && !matches!(func, Expr::Attr { .. }) {
            return self.compile_keyword_call(func, args, span);
        }

        // Keyword method call `obj.m(<pos…>, k=v…)` with no splats (issue #2392).
        // Lay the receiver and arguments out in registers and emit a
        // `CallMethodKw`, which reuses the `CallMethod` inline cache + the
        // `CallKw` keyword fast-bind (receiver → param 0) instead of the
        // BuildList + BuildDict + `CallMethodExpanded` round-trip.  In-place
        // mutation of a fast-local receiver register is preserved exactly as in
        // `compile_method_call` (same `obj_reg`/`dst_reg` placement).
        if has_kwargs
            && !has_splat
            && let Expr::Attr { target, name, .. } = func
        {
            return self.compile_keyword_method_call(target, name, args, span);
        }

        // Double-splat expansion `f(<pos…>, **d)` (issue #2393): exactly one
        // trailing `**d`, every preceding arg a plain positional (no `*a` splat,
        // no literal keyword), non-method callee.  Lower to `CallEx`, which binds
        // straight from the splat dict via a monomorphic shape cache instead of
        // copying the dict and round-tripping through a Python-visible helper.
        if let Some(npos) = double_splat_fast_shape(args)
            && npos <= u8::MAX as usize
            && !matches!(func, Expr::Attr { .. })
        {
            return self.compile_double_splat_call(func, args, npos);
        }

        // Positional-splat expansion `f(<pos…>, *args[, **kw])` (the
        // decorator/wrapper shape): one `*args`, an optional trailing `**kw`,
        // preceded only by plain positionals, non-method callee.  Lower to
        // `CallExArgs`, which reads the splat iterable and the `**kw` mapping
        // directly instead of building a list + dict and round-tripping through
        // a Python-visible helper.
        if let Some((npos, nkw)) = positional_splat_fast_shape(args)
            && !matches!(func, Expr::Attr { .. })
        {
            return self.compile_positional_splat_call(func, args, npos, nkw);
        }

        if has_splat || has_kwargs {
            // Variadic call: build separate positional and keyword lists, then
            // use the ExpandedCall instruction.
            return self.compile_variadic_call(func, args);
        }

        // `Call` / `CallMethod` encode their positional count in a `u8`.
        // Casting a larger direct call would silently wrap the count and make
        // the reserved register window disagree with the arguments compiled
        // below.  The generic expanded-call lowering has no such limit.
        if args.len() > u8::MAX as usize {
            return self.compile_variadic_call(func, args);
        }

        // Detect obj.method(args) — emit CallMethod to allow in-place mutation.
        if let Expr::Attr { target, name, .. } = func {
            return self.compile_method_call(target, name, args, span);
        }

        let argc = args.len() as u8;
        let func_reg = self.next_temp;
        let frame_top = func_reg.wrapping_add(1).wrapping_add(Reg::from(argc));
        if frame_top < func_reg {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = frame_top;
        if frame_top > 0 && frame_top - 1 > self.max_reg {
            self.max_reg = frame_top - 1;
        }
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        for (i, arg) in (0u32..).zip(args.iter()) {
            let arg_reg = func_reg + 1 + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        let is_pure_callee =
            matches!(func, Expr::Var(n, _) if self.pure_locals.contains(n.as_str()));
        // Arm the `callee(...)` caret anchor on the terminal call instruction
        // (#2411); `emit` consumes and clears it.
        self.set_col_span_for_next(span);
        if is_pure_callee {
            self.emit(Insn::CallMemo(func_reg, argc));
        } else {
            self.emit(Insn::Call(func_reg, argc));
        }
        self.next_temp = func_reg + 1;
        func_reg
    }

    /// Compile a keyword-argument call (no splats, non-method callee) into a
    /// `CallKw` instruction (issue #2382).  Arguments are evaluated left-to-right
    /// (Python order) into contiguous registers `func_reg+1 .. func_reg+1+total`,
    /// positionals first then keyword values; the keyword names form a
    /// constant-pool tuple consumed by the runtime binder.
    ///
    /// Python's grammar guarantees every positional argument precedes every
    /// keyword argument in a call, so source order already lays out positionals
    /// before keyword values — no reordering of evaluation is needed.
    fn compile_keyword_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let total = args.len();
        if total > u8::MAX as usize {
            // Too many args to encode in a u8 — fall back to the generic path.
            return self.compile_variadic_call(func, args);
        }
        let nkw = args.iter().filter(|a| a.name.is_some()).count();

        // Build the keyword-names tuple constant (in source order, matching the
        // order the keyword values occupy in the register window).
        let kw_names: Vec<Value> = args
            .iter()
            .filter_map(|a| a.name.as_ref())
            .map(|n| Value::string(n.clone()))
            .collect();
        let kwnames_idx = self.intern_const(Value::tuple(kw_names));

        let func_reg = self.next_temp;
        let frame_top = func_reg.wrapping_add(1).wrapping_add(total as Reg);
        if frame_top < func_reg {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = frame_top;
        if frame_top > 0 && frame_top - 1 > self.max_reg {
            self.max_reg = frame_top - 1;
        }
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        for (i, arg) in (0u32..).zip(args.iter()) {
            let arg_reg = func_reg + 1 + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        // Arm the `callee(...)` caret anchor on the terminal call (#2443);
        // `emit` consumes and clears it.
        self.set_col_span_for_next(span);
        self.emit(Insn::CallKw {
            func: func_reg,
            total: total as u8,
            nkw: nkw as u8,
            kwnames_idx,
        });
        self.next_temp = func_reg + 1;
        func_reg
    }

    /// Compile a keyword-argument method call `obj.m(<pos…>, k=v…)` (no splats)
    /// into a `CallMethodKw` instruction (issue #2392).  The receiver register
    /// placement matches `compile_method_call` exactly (fast-local receivers use
    /// their own register as `obj` so in-place mutation persists; the result goes
    /// to a distinct `dst`).  Arguments are laid out contiguously in
    /// `R[args_base .. args_base+total]` — positionals first, then keyword values
    /// in source order — exactly as `compile_keyword_call` does; the keyword names
    /// form a constant-pool tuple consumed by the runtime binder.
    ///
    /// Python's grammar guarantees every positional argument precedes every
    /// keyword argument in a call, so source order already lays out positionals
    /// before keyword values — no reordering of evaluation is needed.
    fn compile_keyword_method_call(
        &mut self,
        target: &Expr,
        method_name: &str,
        args: &[crate::ast::CallArg],
        span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let total = args.len();
        if total > u8::MAX as usize {
            // Too many args to encode in a u8 — fall back to the generic path.
            return self.compile_variadic_call(
                &Expr::Attr {
                    target: Box::new(target.clone()),
                    name: method_name.to_string(),
                    span: None,
                },
                args,
            );
        }
        let nkw = args.iter().filter(|a| a.name.is_some()).count();
        let total_reg = total as Reg;

        // Build the keyword-names tuple constant (source order, matching the order
        // the keyword values occupy in the register window).
        let kw_names: Vec<Value> = args
            .iter()
            .filter_map(|a| a.name.as_ref())
            .map(|n| Value::string(n.clone()))
            .collect();
        let kwnames_idx = self.intern_const(Value::tuple(kw_names));

        // Receiver / dst / args_base placement — identical to compile_method_call.
        let (obj_reg, dst_reg, args_base, need_copy) = if let Expr::Var(name, _) = target {
            if let Some(local) = self.local_reg(name).filter(|_| !self.is_class_body) {
                let dst = self.next_temp;
                let abase = dst.wrapping_add(1);
                let frame_top = abase.wrapping_add(total_reg);
                if frame_top < dst {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("call frame register overflow".to_string());
                    }
                    return 0;
                }
                self.next_temp = frame_top;
                if frame_top > 0 && frame_top - 1 > self.max_reg {
                    self.max_reg = frame_top - 1;
                }
                (local, dst, abase, false)
            } else {
                let o = self.next_temp;
                let abase = o.wrapping_add(1);
                let frame_top = abase.wrapping_add(total_reg);
                if frame_top < o {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("call frame register overflow".to_string());
                    }
                    return 0;
                }
                self.next_temp = frame_top;
                if frame_top > 0 && frame_top - 1 > self.max_reg {
                    self.max_reg = frame_top - 1;
                }
                (o, o, abase, true)
            }
        } else {
            let o = self.next_temp;
            let abase = o.wrapping_add(1);
            let frame_top = abase.wrapping_add(total_reg);
            if frame_top < o {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("call frame register overflow".to_string());
                }
                return 0;
            }
            self.next_temp = frame_top;
            if frame_top > 0 && frame_top - 1 > self.max_reg {
                self.max_reg = frame_top - 1;
            }
            (o, o, abase, true)
        };

        if need_copy {
            let saved = self.next_temp;
            self.compile_expr_into(target, obj_reg);
            self.next_temp = saved;
        }

        for (i, arg) in (0u32..).zip(args.iter()) {
            let arg_reg = args_base + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        let name_idx = self.intern_name(method_name);
        // Arm the `obj.m(...)` caret anchor on the terminal call (#2443);
        // `emit` consumes and clears it.
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
        self.next_temp = dst_reg + 1;
        dst_reg
    }

    fn compile_method_call(
        &mut self,
        target: &Expr,
        method_name: &str,
        args: &[crate::ast::CallArg],
        span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let nargs = args.len() as u8;

        // When the receiver is a plain fast-local variable, use its register directly
        // as `obj` so that in-place mutations (append, pop, …) actually update the
        // variable.  The return value goes into a fresh temp `dst_reg ≠ obj_reg`.
        // For all other receivers we fall back to copying the value into a temp and
        // using the same register for both obj and dst.
        let (obj_reg, dst_reg, args_base, need_copy) = if let Expr::Var(name, _) = target {
            if let Some(local) = self.local_reg(name).filter(|_| !self.is_class_body) {
                let dst = self.next_temp;
                let abase = dst.wrapping_add(1);
                let frame_top = abase.wrapping_add(Reg::from(nargs));
                if frame_top < dst {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("call frame register overflow".to_string());
                    }
                    return 0;
                }
                self.next_temp = frame_top;
                if frame_top > 0 && frame_top - 1 > self.max_reg {
                    self.max_reg = frame_top - 1;
                }
                (local, dst, abase, false)
            } else {
                // cell / nonlocal — must load via env first
                let o = self.next_temp;
                let abase = o.wrapping_add(1);
                let frame_top = abase.wrapping_add(Reg::from(nargs));
                if frame_top < o {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("call frame register overflow".to_string());
                    }
                    return 0;
                }
                self.next_temp = frame_top;
                if frame_top > 0 && frame_top - 1 > self.max_reg {
                    self.max_reg = frame_top - 1;
                }
                (o, o, abase, true)
            }
        } else {
            let o = self.next_temp;
            let abase = o.wrapping_add(1);
            let frame_top = abase.wrapping_add(Reg::from(nargs));
            if frame_top < o {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("call frame register overflow".to_string());
                }
                return 0;
            }
            self.next_temp = frame_top;
            if frame_top > 0 && frame_top - 1 > self.max_reg {
                self.max_reg = frame_top - 1;
            }
            (o, o, abase, true)
        };

        if need_copy {
            let saved = self.next_temp;
            self.compile_expr_into(target, obj_reg);
            self.next_temp = saved;
        }

        for (i, arg) in (0u32..).zip(args.iter()) {
            let arg_reg = args_base + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        let name_idx = self.intern_name(method_name);
        // Arm the `obj.m(...)` caret anchor on the terminal call (#2443);
        // `emit` consumes and clears it.
        self.set_col_span_for_next(span);
        self.emit(Insn::CallMethod {
            dst: dst_reg,
            obj: obj_reg,
            name_idx,
            args_base,
            nargs,
        });
        self.next_temp = dst_reg + 1;
        dst_reg
    }

    fn compile_variadic_call(&mut self, func: &Expr, args: &[crate::ast::CallArg]) -> Reg {
        // Generic fallback for source-order-sensitive shapes not covered by the
        // compact call opcodes. Evaluate each argument once in source order,
        // accumulating positionals into a private list and keywords into a
        // private dict; the merge instructions preserve duplicate-key errors.
        // Method calls use CallMethodExpanded. Other calls pass the materialized
        // pair through CallExArgs directly, without exposing transport through
        // Python globals.
        if let Expr::Attr { target, name, .. } = func {
            // Snapshot a fast-local receiver before evaluating arguments.
            // Argument expressions may rebind that local, while Python still
            // calls the object evaluated before them. Value::clone keeps the
            // shared backing of mutable receivers, so in-place mutations made
            // by the method remain visible through the original object.
            let (obj_reg, dst_reg) = if let Expr::Var(tname, _) = target.as_ref() {
                if let Some(local) = self.local_reg(tname).filter(|_| !self.is_class_body) {
                    let obj = self.alloc_temp();
                    self.emit(Insn::Move(obj, local));
                    (obj, obj)
                } else {
                    let o = self.alloc_temp();
                    self.compile_expr_into(target, o);
                    (o, o)
                }
            } else {
                let o = self.alloc_temp();
                self.compile_expr_into(target, o);
                (o, o)
            };
            let name_idx = self.intern_name(name);

            let pos_list_reg = self.alloc_temp();
            let empty_list_base = self.next_temp;
            self.next_temp = empty_list_base + 1;
            if empty_list_base > self.max_reg {
                self.max_reg = empty_list_base;
            }
            self.emit(Insn::BuildList(pos_list_reg, empty_list_base, 0));

            let kw_dict_reg = self.alloc_temp();
            let empty_dict_base = self.next_temp;
            self.next_temp = empty_dict_base + 1;
            if empty_dict_base > self.max_reg {
                self.max_reg = empty_dict_base;
            }
            self.emit(Insn::BuildDict(
                kw_dict_reg,
                empty_dict_base,
                0,
                DictKeyKindHint::Unicode,
            ));

            let has_kw_splat = args.iter().any(|a| a.double_splat);
            for arg in args {
                if arg.splat {
                    let val = self.compile_expr(&arg.value);
                    self.emit(Insn::ListExtendCall {
                        list: pos_list_reg,
                        src: val,
                        name: crate::bytecode::KwCallName::Method {
                            obj: obj_reg,
                            name_idx,
                        },
                    });
                    self.free_temp(val);
                } else if arg.double_splat {
                    let val = self.compile_expr(&arg.value);
                    // The callee is `obj.<name>`; its qualname is derived from
                    // the receiver's class on the error path.
                    self.emit(Insn::DictMergeKwCall {
                        dict: kw_dict_reg,
                        src: val,
                        name: crate::bytecode::KwCallName::Method {
                            obj: obj_reg,
                            name_idx,
                        },
                    });
                    self.free_temp(val);
                } else if let Some(kw_name) = &arg.name {
                    let val = self.compile_expr(&arg.value);
                    let key_idx = self.intern_const(Value::string(kw_name.clone()));
                    let key_reg = self.alloc_temp();
                    self.emit(Insn::LoadConst(key_reg, key_idx));
                    if has_kw_splat {
                        self.emit(Insn::SetItemKwCall {
                            dict: kw_dict_reg,
                            key: key_reg,
                            val,
                            name: crate::bytecode::KwCallName::Method {
                                obj: obj_reg,
                                name_idx,
                            },
                        });
                    } else {
                        self.emit(Insn::SetItem(kw_dict_reg, key_reg, val));
                    }
                    self.free_temp(key_reg);
                    self.free_temp(val);
                } else {
                    let val = self.compile_expr(&arg.value);
                    self.emit(Insn::ListAppend(pos_list_reg, val));
                    self.free_temp(val);
                }
            }

            self.emit(Insn::CallMethodExpanded {
                dst: dst_reg,
                obj: obj_reg,
                name_idx,
                pos_list: pos_list_reg,
                kw_dict: kw_dict_reg,
            });
            self.free_temp(kw_dict_reg);
            self.free_temp(pos_list_reg);
            self.next_temp = dst_reg + 1;
            return dst_reg;
        }

        let func_reg = self.alloc_temp();
        self.compile_expr_into(func, func_reg);

        // Build positional list
        let pos_list_reg = self.alloc_temp();
        // Use: pos_list = []  → then extend/append
        let empty_list_base = self.next_temp;
        if empty_list_base.checked_add(1).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = empty_list_base + 1;
        if empty_list_base > self.max_reg {
            self.max_reg = empty_list_base;
        }
        self.emit(Insn::BuildList(pos_list_reg, empty_list_base, 0));

        // Build kwargs dict
        let kw_dict_reg = self.alloc_temp();
        let empty_dict_base = self.next_temp;
        if empty_dict_base.checked_add(1).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = empty_dict_base + 1;
        if empty_dict_base > self.max_reg {
            self.max_reg = empty_dict_base;
        }
        self.emit(Insn::BuildDict(
            kw_dict_reg,
            empty_dict_base,
            0,
            DictKeyKindHint::Unicode,
        ));

        // When the call mixes a `**d` splat with other keyword sources, a key
        // present in two of them is a `TypeError` in CPython (DICT_MERGE), not a
        // silent overwrite.  Route kwargs through the duplicate-checking
        // instructions only in that case so the common no-splat call is
        // untouched.  `func_reg` carries the callee for the error's qualname.
        let has_kw_splat = args.iter().any(|a| a.double_splat);
        for arg in args {
            if arg.splat {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::ListExtendCall {
                    list: pos_list_reg,
                    src: val,
                    name: crate::bytecode::KwCallName::Callee(func_reg),
                });
                self.free_temp(val);
            } else if arg.double_splat {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::DictMergeKwCall {
                    dict: kw_dict_reg,
                    src: val,
                    name: crate::bytecode::KwCallName::Callee(func_reg),
                });
                self.free_temp(val);
            } else if let Some(kw_name) = &arg.name {
                let val = self.compile_expr(&arg.value);
                let key_idx = self.intern_const(Value::string(kw_name.clone()));
                let key_reg = self.alloc_temp();
                self.emit(Insn::LoadConst(key_reg, key_idx));
                if has_kw_splat {
                    self.emit(Insn::SetItemKwCall {
                        dict: kw_dict_reg,
                        key: key_reg,
                        val,
                        name: crate::bytecode::KwCallName::Callee(func_reg),
                    });
                } else {
                    self.emit(Insn::SetItem(kw_dict_reg, key_reg, val));
                }
                self.free_temp(key_reg);
                self.free_temp(val);
            } else {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::ListAppend(pos_list_reg, val));
                self.free_temp(val);
            }
        }

        // The source-order-sensitive generic shape is now fully materialized:
        // `pos_list_reg` is a private plain list and `kw_dict_reg` is a private
        // plain dict. Reuse CallExArgs as the typed VM transport for that pair
        // instead of resolving an implementation-only helper through Python's
        // global namespace. Besides preventing a user binding from changing
        // call syntax, this also removes one global lookup, three register
        // moves, and one nested builtin dispatch.
        self.emit(Insn::CallExArgs {
            func: func_reg,
            npos: 0,
            nkw: 0,
            // Ignored when `nkw == 0`.
            kwnames_idx: 0,
            args_splat: pos_list_reg,
            kwargs: kw_dict_reg,
        });
        self.free_temp(kw_dict_reg);
        self.free_temp(pos_list_reg);
        // CallExArgs writes the result back to `func_reg`. Keep it live for the
        // caller, matching every other compile_call lowering.
        self.next_temp = func_reg + 1;
        func_reg
    }

    /// Compile a double-splat call `f(<pos…>, **d)` (issue #2393, shape vetted by
    /// [`double_splat_fast_shape`]) into a `CallEx` instruction.  Positionals fill
    /// `R[func+1 .. func+1+npos]` contiguously (as for `CallKw`); the single `**d`
    /// source dict is evaluated into a separate `kwargs` register above the
    /// positional window.  The runtime binder reads the dict directly — no
    /// BuildDict/DictUpdate copy and no hidden-helper round-trip.
    fn compile_double_splat_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        npos: usize,
    ) -> Reg {
        let func_reg = self.next_temp;
        // Reserve func + npos positional registers contiguously, then one more
        // for the `**d` dict.
        let frame_top = func_reg
            .wrapping_add(1)
            .wrapping_add(npos as Reg)
            .wrapping_add(1);
        if frame_top < func_reg {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = frame_top;
        if frame_top > 0 && frame_top - 1 > self.max_reg {
            self.max_reg = frame_top - 1;
        }
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        // Positionals into the contiguous window (source order; every arg but the
        // trailing `**d` is a plain positional — guaranteed by the shape check).
        for (i, arg) in (0u32..).zip(args[..npos].iter()) {
            let arg_reg = func_reg + 1 + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        // The `**d` source mapping into the dedicated kwargs register.
        let kwargs_reg = func_reg + 1 + npos as Reg;
        let saved = self.next_temp;
        let insn_before = self.insns.len();
        let r = self.compile_expr(&args[npos].value);
        if r != kwargs_reg {
            let single = self.insns.len() == insn_before + 1;
            if single && r >= self.base_temp && self.retarget_last(r, kwargs_reg) {
                // retargeted in place — no Move needed
            } else {
                self.emit(Insn::Move(kwargs_reg, r));
            }
        }
        self.next_temp = saved;
        self.emit(Insn::CallEx {
            func: func_reg,
            npos: npos as u8,
            kwargs: kwargs_reg,
        });
        self.next_temp = func_reg + 1;
        func_reg
    }

    /// Compile a positional-splat call `f(<pos…>, *args, kw=v…[, **kw])` (shape
    /// vetted by [`positional_splat_fast_shape`]) into a `CallExArgs` instruction.
    /// Register frame: `R[func]` = callee; `R[func+1 .. func+1+npos]` = leading
    /// positionals; `R[func+1+npos .. func+1+npos+nkw]` = the `nkw` literal
    /// keyword VALUES (names in the `kwnames_idx` const tuple); then the `*args`
    /// iterable and the optional `**kw` mapping in their own registers (`**kw`
    /// absent ⇒ `NO_KWARGS`).  Argument sub-expressions are compiled in source
    /// (evaluation) order: positionals, `*args`, literal keyword values, `**kw`.
    fn compile_positional_splat_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        npos: usize,
        nkw: usize,
    ) -> Reg {
        let has_kw = args.last().is_some_and(|a| a.double_splat);
        let func_reg = self.next_temp;
        // Reserve func + npos positionals + nkw literal-keyword values + one for
        // `*args` + one more for `**kw` when present.
        let frame_top = func_reg
            .wrapping_add(1)
            .wrapping_add(npos as Reg)
            .wrapping_add(nkw as Reg)
            .wrapping_add(1)
            .wrapping_add(if has_kw { 1 } else { 0 });
        if frame_top < func_reg {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = frame_top;
        if frame_top > 0 && frame_top - 1 > self.max_reg {
            self.max_reg = frame_top - 1;
        }
        // Compile one argument sub-expression into its reserved `dst` slot,
        // reusing the retarget-last / Move dance shared by the whole frame.
        let compile_into_slot = |this: &mut Self, value: &Expr, dst: Reg| {
            let saved = this.next_temp;
            let insn_before = this.insns.len();
            let r = this.compile_expr(value);
            if r != dst {
                let single = this.insns.len() == insn_before + 1;
                if single && r >= this.base_temp && this.retarget_last(r, dst) {
                    // retargeted in place — no Move needed
                } else {
                    this.emit(Insn::Move(dst, r));
                }
            }
            this.next_temp = saved;
        };

        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;

        // Leading positionals (source order; all plain positionals per the shape).
        for (i, arg) in (0u32..).zip(args[..npos].iter()) {
            compile_into_slot(self, &arg.value, func_reg + 1 + i);
        }
        // `*args` iterable.
        let args_splat_reg = func_reg + 1 + npos as Reg + nkw as Reg;
        compile_into_slot(self, &args[npos].value, args_splat_reg);
        // Literal keyword values into `R[func+1+npos .. +nkw]`, names collected for
        // the const-pool tuple.  (Evaluated after `*args`, matching Python order.)
        let litkw_base = func_reg + 1 + npos as Reg;
        let mut kw_names: Vec<Value> = Vec::with_capacity(nkw);
        for (i, arg) in (0u32..).zip(args[npos + 1..npos + 1 + nkw].iter()) {
            let name = arg
                .name
                .as_ref()
                .expect("literal keyword after splat (shape-checked)");
            kw_names.push(Value::string(name.clone()));
            compile_into_slot(self, &arg.value, litkw_base + i);
        }
        let kwnames_idx = self.intern_const(Value::tuple(kw_names));
        // Optional `**kw` source mapping.
        let kwargs_reg = if has_kw {
            let kwargs_reg = args_splat_reg + 1;
            compile_into_slot(self, &args[npos + 1 + nkw].value, kwargs_reg);
            kwargs_reg
        } else {
            crate::bytecode::NO_KWARGS
        };
        self.emit(Insn::CallExArgs {
            func: func_reg,
            npos: npos as u8,
            nkw: nkw as u8,
            kwnames_idx,
            args_splat: args_splat_reg,
            kwargs: kwargs_reg,
        });
        self.next_temp = func_reg + 1;
        func_reg
    }
}
