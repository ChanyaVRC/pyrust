impl Compiler {
    /// Emit empty positional list + keyword dict and accumulate all variadic
    /// call arguments in source order. Shared by method and non-method
    /// `compile_variadic_call` paths.
    fn emit_variadic_call_containers(
        &mut self,
        args: &[crate::ast::CallArg],
        make_kw_name: impl Fn() -> crate::bytecode::KwCallName,
    ) -> Option<(Reg, Reg)> {
        let pos_list_reg = self.alloc_temp();
        let empty_list_base = self.reserve_call_frame(1)?;
        self.emit(Insn::BuildList(pos_list_reg, empty_list_base, 0));

        let kw_dict_reg = self.alloc_temp();
        let empty_dict_base = self.reserve_call_frame(1)?;
        self.emit(Insn::BuildDict(
            kw_dict_reg,
            empty_dict_base,
            0,
            DictKeyKindHint::Unicode,
        ));

        // When the call mixes a `**d` splat with other keyword sources, a key
        // present in two of them is a `TypeError` in CPython (DICT_MERGE), not a
        // silent overwrite.  Route kwargs through the duplicate-checking
        // instructions only in that case so the common no-splat call is untouched.
        let has_kw_splat = args.iter().any(|a| a.double_splat);
        for arg in args {
            if arg.splat {
                let val = self.compile_expr(&arg.value);
                let desc_idx = self.push_desc(InsnDesc::ListExtendCall(ListExtendCallDesc {
                    list: pos_list_reg,
                    src: val,
                    name: make_kw_name(),
                }));
                self.emit(Insn::ListExtendCall(desc_idx));
                self.free_temp(val);
            } else if arg.double_splat {
                let val = self.compile_expr(&arg.value);
                let desc_idx = self.push_desc(InsnDesc::DictMergeKwCall(DictMergeKwCallDesc {
                    dict: kw_dict_reg,
                    src: val,
                    name: make_kw_name(),
                }));
                self.emit(Insn::DictMergeKwCall(desc_idx));
                self.free_temp(val);
            } else if let Some(kw_name) = arg.name {
                let val = self.compile_expr(&arg.value);
                let key_idx = self.intern_const(Value::string(kw_name.as_str()));
                let key_reg = self.alloc_temp();
                self.emit(Insn::LoadConst(key_reg, key_idx));
                if has_kw_splat {
                    let desc_idx = self.push_desc(InsnDesc::SetItemKwCall(SetItemKwCallDesc {
                        dict: kw_dict_reg,
                        key: key_reg,
                        val,
                        name: make_kw_name(),
                    }));
                    self.emit(Insn::SetItemKwCall(desc_idx));
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
        Some((pos_list_reg, kw_dict_reg))
    }

    fn compile_variadic_call(&mut self, func: &Expr, args: &[crate::ast::CallArg]) -> Reg {
        // Generic fallback for source-order-sensitive shapes not covered by the
        // compact call opcodes. Evaluate each argument once in source order,
        // accumulating positionals into a private list and keywords into a
        // private dict; the merge instructions preserve duplicate-key errors.
        if let Expr::Attr { target, name, .. } = func {
            // Snapshot a fast-local receiver before evaluating arguments.
            // Argument expressions may rebind that local, while Python still
            // calls the object evaluated before them. Value::clone keeps the
            // shared backing of mutable receivers, so in-place mutations remain
            // visible through the original object.
            let (obj_reg, dst_reg) = if let Expr::Var(tname, _) = target.as_ref()
                && let Some(local) = self.local_reg(tname).filter(|_| !self.is_class_body)
            {
                let obj = self.alloc_temp();
                self.emit(Insn::Move(obj, local));
                (obj, obj)
            } else {
                let o = self.alloc_temp();
                self.compile_expr_into(target, o);
                (o, o)
            };
            let name_idx = self.intern_name(name);

            let Some((pos_list_reg, kw_dict_reg)) =
                self.emit_variadic_call_containers(args, || crate::bytecode::KwCallName::Method {
                    obj: obj_reg,
                    name_idx,
                })
            else {
                return 0;
            };

            let desc_idx = self.push_desc(InsnDesc::CallMethodExpanded(CallMethodExpandedDesc {
                dst: dst_reg,
                obj: obj_reg,
                name_idx,
                pos_list: pos_list_reg,
                kw_dict: kw_dict_reg,
            }));
            self.emit(Insn::CallMethodExpanded(desc_idx));
            self.free_temp(kw_dict_reg);
            self.free_temp(pos_list_reg);
            self.next_temp = dst_reg + 1;
            return dst_reg;
        }

        let func_reg = self.alloc_temp();
        self.compile_expr_into(func, func_reg);

        let Some((pos_list_reg, kw_dict_reg)) = self
            .emit_variadic_call_containers(args, || crate::bytecode::KwCallName::Callee(func_reg))
        else {
            return 0;
        };

        // Reuse CallExArgs as the typed VM transport for the materialized
        // `(pos_list_reg, kw_dict_reg)` pair.
        let desc_idx = self.push_desc(InsnDesc::CallExArgs(CallExArgsDesc {
            func: func_reg,
            npos: 0,
            nkw: 0,
            // Ignored when `nkw == 0`.
            kwnames_idx: 0,
            args_splat: pos_list_reg,
            kwargs: kw_dict_reg,
        }));
        self.emit(Insn::CallExArgs(desc_idx));
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
    /// positional window.
    fn compile_double_splat_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        npos: usize,
    ) -> Reg {
        // Reserve func + npos positional registers + one for the `**d` dict.
        let Some(func_reg) = self.reserve_call_frame(2 + npos as Reg) else {
            return 0;
        };
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        for (i, arg) in (0u32..).zip(args[..npos].iter()) {
            self.compile_call_arg_into(&arg.value, func_reg + 1 + i);
        }
        let kwargs_reg = func_reg + 1 + npos as Reg;
        self.compile_call_arg_into(&args[npos].value, kwargs_reg);
        let desc_idx = self.push_desc(InsnDesc::CallExArgs(CallExArgsDesc {
            func: func_reg,
            npos: npos as u8,
            nkw: 0,
            kwnames_idx: 0,
            args_splat: NO_ARGS_SPLAT,
            kwargs: kwargs_reg,
        }));
        self.emit(Insn::CallExArgs(desc_idx));
        self.next_temp = func_reg + 1;
        func_reg
    }

    /// Compile a positional-splat call `f(<pos…>, *args, kw=v…[, **kw])` (shape
    /// vetted by [`positional_splat_fast_shape`]) into a `CallExArgs` instruction.
    /// Argument sub-expressions are compiled in source (evaluation) order:
    /// positionals, `*args`, literal keyword values, `**kw`.
    fn compile_positional_splat_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        npos: usize,
        nkw: usize,
    ) -> Reg {
        let has_kw = args.last().is_some_and(|a| a.double_splat);
        let slots = (2 + npos as Reg + nkw as Reg) + if has_kw { 1 } else { 0 };
        let Some(func_reg) = self.reserve_call_frame(slots) else {
            return 0;
        };

        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;

        // Leading positionals (source order; all plain positionals per the shape).
        for (i, arg) in (0u32..).zip(args[..npos].iter()) {
            self.compile_call_arg_into(&arg.value, func_reg + 1 + i);
        }
        // `*args` iterable.
        let args_splat_reg = func_reg + 1 + npos as Reg + nkw as Reg;
        self.compile_call_arg_into(&args[npos].value, args_splat_reg);
        // Literal keyword values into `R[func+1+npos .. +nkw]`, names collected for
        // the const-pool tuple.  (Evaluated after `*args`, matching Python order.)
        let litkw_base = func_reg + 1 + npos as Reg;
        let mut kw_names: Vec<Value> = Vec::with_capacity(nkw);
        for (i, arg) in (0u32..).zip(args[npos + 1..npos + 1 + nkw].iter()) {
            let name = arg
                .name
                .expect("literal keyword after splat (shape-checked)");
            kw_names.push(Value::string(name.as_str()));
            self.compile_call_arg_into(&arg.value, litkw_base + i);
        }
        let kwnames_idx = self.intern_const(Value::tuple(kw_names));
        // Optional `**kw` source mapping.
        let kwargs_reg = if has_kw {
            let kwargs_reg = args_splat_reg + 1;
            self.compile_call_arg_into(&args[npos + 1 + nkw].value, kwargs_reg);
            kwargs_reg
        } else {
            crate::bytecode::NO_KWARGS
        };
        let desc_idx = self.push_desc(InsnDesc::CallExArgs(CallExArgsDesc {
            func: func_reg,
            npos: npos as u8,
            nkw: nkw as u8,
            kwnames_idx,
            args_splat: args_splat_reg,
            kwargs: kwargs_reg,
        }));
        self.emit(Insn::CallExArgs(desc_idx));
        self.next_temp = func_reg + 1;
        func_reg
    }
}
