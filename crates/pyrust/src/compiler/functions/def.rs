impl Compiler {
    /// Compile a function's default-value expressions into a contiguous
    /// register window.  Returns `(base, count)`, or `None` on register
    /// overflow (error already recorded).
    fn emit_def_default_values(&mut self, params: &[FunctionParam]) -> Option<(Reg, u32)> {
        let defaults: Vec<usize> = params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.default.is_some())
            .map(|(i, _)| i)
            .collect();
        let defs_n = match u32::try_from(defaults.len()) {
            Ok(count) => count,
            Err(_) => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many default values".to_string());
                }
                return None;
            }
        };
        let defs_base = self.next_temp;
        if defs_n > 0 {
            if self.next_temp.checked_add(Reg::from(defs_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many default-value registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(defs_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (slot_i, param_i) in (0u32..).zip(defaults.iter()) {
                let def_expr = params[*param_i].default.as_ref().unwrap();
                let saved = self.next_temp;
                let r = self.compile_expr(def_expr);
                if r != defs_base + slot_i {
                    self.emit(Insn::Move(defs_base + slot_i, r));
                }
                self.next_temp = saved;
            }
        }
        Some((defs_base, defs_n))
    }

    /// Compile a function's parameter/return annotation expressions into a
    /// contiguous register window (param annotations in declaration order, then
    /// the return annotation).  Returns `(base, count)`, or `None` on register
    /// overflow (error already recorded).
    fn emit_def_annotation_values(
        &mut self,
        params: &[FunctionParam],
        return_annotation: Option<&Expr>,
    ) -> Option<(Reg, u32)> {
        // Under PEP 563 (`from __future__ import annotations`), emit the
        // annotation source text as a string literal instead of evaluating.
        let annotated_params: Vec<(usize, &Expr)> = params
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.annotation.as_ref().map(|a| (i, a)))
            .collect();
        let annots_n = match u32::try_from(annotated_params.len())
            .ok()
            .and_then(|count| count.checked_add(u32::from(return_annotation.is_some())))
        {
            Some(count) => count,
            None => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many annotations".to_string());
                }
                return None;
            }
        };
        let annots_base = self.next_temp;
        if annots_n > 0 {
            if self.next_temp.checked_add(Reg::from(annots_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many annotation registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(annots_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (slot_i, (_, annot_expr)) in (0u32..).zip(annotated_params.iter()) {
                let saved = self.next_temp;
                if self.future_annotations {
                    let r = self.compile_literal(Value::string(stringify_annotation(annot_expr)));
                    if r != annots_base + slot_i {
                        self.emit(Insn::Move(annots_base + slot_i, r));
                    }
                } else if let Expr::Starred(inner) = annot_expr {
                    // PEP 646 starred `*args: *Ts` annotation: unpack a single
                    // value from the iterable (`(annot,) = inner`), yielding
                    // `typing.Unpack[Ts]` for a `TypeVarTuple` and matching
                    // CPython's `ValueError` for iterables of length != 1.
                    let iter_reg = self.compile_expr(inner);
                    self.emit(Insn::Unpack(annots_base + slot_i, iter_reg, 1));
                } else {
                    let r = self.compile_expr(annot_expr);
                    if r != annots_base + slot_i {
                        self.emit(Insn::Move(annots_base + slot_i, r));
                    }
                }
                self.next_temp = saved;
            }
            if let Some(ret_annot) = return_annotation {
                let slot_i = annots_n - 1;
                let saved = self.next_temp;
                let r = if self.future_annotations {
                    self.compile_literal(Value::string(stringify_annotation(ret_annot)))
                } else {
                    self.compile_expr(ret_annot)
                };
                if r != annots_base + slot_i {
                    self.emit(Insn::Move(annots_base + slot_i, r));
                }
                self.next_temp = saved;
            }
        }
        Some((annots_base, annots_n))
    }

    /// Apply a chain of decorators to the value in `dst`: evaluate each
    /// decorator expression top-to-bottom, then apply innermost-first
    /// (`fn = d1(d2(d3(fn)))`).  Returns the register holding the final
    /// decorated value (`dst` when there are no decorators), or `None` on
    /// register overflow (error already recorded).  Shared by `compile_def`
    /// and `compile_class`.
    fn emit_decorator_application(&mut self, decorators: &[Expr], dst: Reg) -> Option<Reg> {
        let mut val_reg = dst;
        if !decorators.is_empty() {
            let n = decorators.len() as u32;
            let deco_base = self.next_temp;
            // Need n slots for the callables plus 1 extra arg slot for the first call.
            if deco_base.checked_add(n + 1).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("too many registers for decorator application".to_string());
                }
                return None;
            }
            self.next_temp = deco_base + n + 1;
            if deco_base + n > self.max_reg {
                self.max_reg = deco_base + n;
            }
            for (i, deco_expr) in decorators.iter().enumerate() {
                let saved = self.next_temp;
                self.compile_expr_into(deco_expr, deco_base + i as u32);
                self.next_temp = saved;
            }
            // Apply decorators bottom-to-top (innermost first).
            for i in (0..n).rev() {
                let frame = deco_base + i;
                self.emit(Insn::Move(frame + 1, val_reg));
                self.emit(Insn::Call(frame, 1));
                val_reg = frame;
            }
            self.next_temp = deco_base + 1;
        }
        Some(val_reg)
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_def(
        &mut self,
        name: &str,
        params: &[FunctionParam],
        body: &[Stmt],
        body_linenos: &[u32],
        def_lineno: u32,
        decorators: &[Expr],
        return_annotation: Option<&Expr>,
        is_async: bool,
        type_params: &[TypeParam],
    ) {
        if let Some(message) = validate_type_parameter_bounds(type_params) {
            self.set_syntax_error(&message);
            return;
        }

        let type_param_names: SmallVec<[Symbol; 4]> = type_params
            .iter()
            .map(|tp| Symbol::intern(&tp.name))
            .collect();
        let (proto_idx, is_memo_pure, has_kwonly_params) = match self.build_def_proto(
            name,
            params,
            body,
            body_linenos,
            def_lineno,
            return_annotation,
            is_async,
            &type_param_names,
        ) {
            Some(v) => v,
            None => return,
        };

        // PEP 695 default values are evaluated in the *enclosing* scope, not
        // the type-parameter scope.
        let (defs_base, defs_n) = match self.emit_def_default_values(params) {
            Some(v) => v,
            None => return,
        };

        // PEP 695: push a dedicated type-parameter environment and bind the
        // TypeVar objects before annotations are evaluated.
        let shadowed_before = self.shadowed_type_params.len();
        if !type_params.is_empty() {
            self.emit(Insn::PushTypeParamEnv);
            self.shadowed_type_params
                .extend(type_param_names.iter().copied());
        }
        let (tp_base, tp_n) = self.emit_bind_type_params(type_params);

        let (annots_base, annots_n) =
            match self.emit_def_annotation_values(params, return_annotation) {
                Some(v) => v,
                None => {
                    self.shadowed_type_params.truncate(shadowed_before);
                    return;
                }
            };
        self.shadowed_type_params.truncate(shadowed_before);

        let dst = self.alloc_temp();
        let desc_idx = self.push_desc(InsnDesc::MakeFunction(MakeFunctionDesc {
            dst,
            proto_idx,
            defs_base,
            defs_n,
            annots_base,
            annots_n,
        }));
        self.emit(Insn::MakeFunction(desc_idx));
        if defs_n > 0 || annots_n > 0 {
            // Free the temp registers used by defaults and annotations while
            // keeping `dst` alive (issue #1362).
            self.next_temp = dst + 1;
        }

        // PEP 695: set __type_params__ on the raw function before decorators.
        if tp_n > 0 {
            self.emit_type_params_attr_from_regs(dst, tp_base, tp_n);
        }

        // PEP 695: pop the type-parameter environment before decorators run and
        // before the def name is bound.
        if !type_params.is_empty() {
            self.emit(Insn::PopTypeParamEnv);
        }

        let val_reg = match self.emit_decorator_application(decorators, dst) {
            Some(r) => r,
            None => return,
        };

        self.compile_store_name(name, val_reg);
        if let Some(reg) = self.local_reg(name) {
            self.mark_def(reg);
        }
        if decorators.is_empty() && !has_kwonly_params && is_memo_pure {
            self.pure_locals.insert(name.to_string());
        }
        self.free_temp(dst);
    }
}
