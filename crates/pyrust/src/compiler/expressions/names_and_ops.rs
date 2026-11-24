impl Compiler {
    /// Retarget the last emitted instruction's dst from `from` to `to`.
    /// Returns true if the instruction was retargeted (had dst == from).
    fn retarget_last(&mut self, from: Reg, to: Reg) -> bool {
        let Some(insn) = self.insns.last_mut() else {
            return false;
        };
        let dst = match insn {
            Insn::BinOp(d, ..)
            | Insn::BinOpInPlace(d, ..)
            | Insn::BinOpConst(d, ..)
            | Insn::BinOpImm(d, ..)
            | Insn::UnaryOp(d, ..)
            | Insn::LoadConst(d, ..)
            | Insn::LoadNone(d)
            | Insn::LoadGlobal(d, ..)
            | Insn::LoadClassName(d, ..)
            | Insn::Move(d, ..)
            | Insn::GetAttr(d, ..)
            | Insn::GetAttrForWith(d, ..)
            | Insn::ImportFromAttr(d, ..)
            | Insn::GetItem(d, ..)
            // Call is NOT retargetable: Call(func_reg, argc) uses func_reg as both
            // the function source and the result destination. Retargeting it to a
            // different register would point the call at the wrong function.
            | Insn::BuildList(d, ..)
            | Insn::BuildTuple(d, ..)
            | Insn::BuildDict(d, ..)
            | Insn::ForIter(d, ..)
            | Insn::LoadExc(d)
            | Insn::ImportModule(d, ..) => d,
            Insn::MakeFunction(desc_idx) => {
                let InsnDesc::MakeFunction(desc) = &mut self.descs[*desc_idx as usize] else {
                    unreachable!("MakeFunction desc mismatch");
                };
                &mut desc.dst
            }
            Insn::MakeClass(desc_idx) => {
                let InsnDesc::MakeClass(desc) = &mut self.descs[*desc_idx as usize] else {
                    unreachable!("MakeClass desc mismatch");
                };
                &mut desc.dst
            }
            _ => return false,
        };
        if *dst == from {
            *dst = to;
            true
        } else {
            false
        }
    }

    fn compile_expr_into(&mut self, expr: &Expr, dst: Reg) {
        if self.failed {
            return;
        }
        let saved_next = self.next_temp;
        let insn_before = self.insns.len();
        let r = self.compile_expr(expr);
        if r != dst {
            // Safe to retarget only when the expression compiled to EXACTLY one
            // instruction and the result is a fresh temp: guarantees no control
            // flow or multi-instruction sequences where other branches still write
            // to `r` and would be missed by retargeting only the last instruction.
            let single = self.insns.len() == insn_before + 1;
            if single && r >= self.base_temp && self.retarget_last(r, dst) {
                self.next_temp = saved_next;
            } else {
                self.emit(Insn::Move(dst, r));
                if r >= self.base_temp {
                    self.next_temp = saved_next;
                }
            }
        }
    }

    /// Try to extract a small i16 integer immediate from an expression.
    /// Returns `Some(imm)` when `expr` is an integer literal in `i16` range.
    fn try_imm_i16(expr: &Expr) -> Option<i16> {
        if let Expr::Int(v) = expr
            && *v >= i16::MIN as i64
            && *v <= i16::MAX as i64
        {
            return Some(*v as i16);
        }
        None
    }

    fn emit_aug_binop(&mut self, reg: Reg, op: BinaryOp, expr: &Expr) {
        if let Some(imm) = Self::try_imm_i16(expr) {
            self.emit(Insn::BinOpImm(reg, reg, op, imm, true));
        } else if let Some(val) = fold_constant(expr) {
            // BinOpConst is safe for augmented assignment: the VM's BinOpConst
            // handler calls try_inplace_op before eval_binary, so mutable
            // containers (list *= / list += / set |= etc.) still get the
            // in-place fast path even when the RHS is a folded constant.  The
            // `is_aug = true` flag tells the VM this fused op carries in-place
            // semantics (issue #1874).
            let idx = self.intern_const(val);
            self.emit(Insn::BinOpConst(reg, reg, op, idx, true));
        } else {
            let rhs = self.compile_expr(expr);
            self.emit(Insn::BinOpInPlace(reg, reg, op, rhs));
            self.free_temp(rhs);
        }
    }

    fn compile_short_circuit(&mut self, left: &Expr, right: &Expr, jump_if_true: bool) -> Reg {
        let lhs = self.compile_expr(left);
        // Always copy to a fresh temp so the JumpIf tests the copy, not `lhs`
        // itself. This prevents the optimizer from fusing BinOp(lhs,lhs,...)+
        // JumpIfFalse(lhs) → CmpJumpIfFalse, which would leave `lhs` holding
        // the original (pre-BinOp) value after the jump instead of False/True.
        let dst = self.alloc_temp();
        self.emit(Insn::Move(dst, lhs));
        self.free_temp(lhs);
        let jmp = if jump_if_true {
            self.emit(Insn::JumpIfTrue(dst, 0))
        } else {
            self.emit(Insn::JumpIfFalse(dst, 0))
        };
        let saved = self.next_temp;
        self.compile_expr_into(right, dst);
        self.next_temp = saved;
        self.patch_jump(jmp);
        dst
    }

    fn compile_literal(&mut self, v: Value) -> Reg {
        let idx = self.intern_const(v);
        let dst = self.alloc_temp();
        self.emit(Insn::LoadConst(dst, idx));
        dst
    }

    /// Compile an expression whose result is discarded by its enclosing
    /// statement.  A definitely-bound local normally compiles to its register
    /// with no instruction. If this compile unit can expose its live module
    /// mapping, however, that binding can be removed without changing the
    /// compiler's `def_set`; force the existing checked `LoadGlobal` path so
    /// the discarded read still performs Python's runtime lookup (#3026).
    fn compile_discarded_expr(&mut self, expr: &Expr) -> Reg {
        let Expr::Var(name, _) = expr else {
            return self.compile_expr(expr);
        };
        let Some(reg) = self.local_reg(name) else {
            return self.compile_expr(expr);
        };
        let definitely_bound = (reg as usize) < 64 && (self.def_set >> reg) & 1 != 0;
        let needs_checked_load =
            self.is_module_scope && self.module_namespace_may_be_exposed && definitely_bound;
        if !needs_checked_load {
            return self.compile_expr(expr);
        }

        let saved_def_set = self.def_set;
        self.def_set &= !(1u64 << reg);
        let result = self.compile_expr(expr);
        self.def_set = saved_def_set;
        result
    }

    fn compile_var_expr(&mut self, name: &Symbol, span: Option<(u32, u32, u32)>) -> Reg {
        // PEP 657 caret anchor (#2426): `set_col_span_for_next` arms the
        // name's column span so the very next `emit` stamps it onto the
        // load instruction that may raise NameError; `emit` then clears
        // it.  A bare name's anchor is whole-span (`^`), so widen the
        // `(start, end)` form to `(start, start, end, end)` (#2411).
        //
        // Multi-line line stamping (#2632): stamp the load instruction with the
        // name's own 1-based line when present, then restore `current_lineno`.
        let name_lineno = span.and_then(|(_, _, ln)| (ln != 0).then_some(ln));
        let span: Option<crate::ast::CaretSpan> = span.map(|(s, e, _)| (s, s, e, e));
        let saved_lineno = self.current_lineno;
        if let Some(ln) = name_lineno {
            self.set_lineno(ln);
        }
        let result =
            if !self.shadowed_type_params.is_empty() && self.shadowed_type_params.contains(name) {
                let name_idx = self.intern_name(name);
                let dst = self.alloc_temp();
                self.set_col_span_for_next(span);
                self.emit(Insn::LoadGlobal(dst, name_idx));
                dst
            } else if let Some(reg) = self.local_reg(name) {
                if self.is_class_body {
                    let name_idx = self.intern_name(name);
                    let dst = self.alloc_temp();
                    self.set_col_span_for_next(span);
                    self.emit(Insn::LoadClassName(dst, reg, name_idx));
                    self.set_lineno(saved_lineno);
                    return dst;
                }
                let definitely_bound = (reg as usize) < 64 && (self.def_set >> reg) & 1 != 0;
                if !definitely_bound {
                    // Issue #1411: at module scope, a name that is not yet
                    // definitely bound must resolve through the global →
                    // builtins chain rather than raising NameError.
                    if self.is_module_scope {
                        let name_idx = self.intern_name(name);
                        let dst = self.alloc_temp();
                        self.set_col_span_for_next(span);
                        self.emit(Insn::LoadGlobal(dst, name_idx));
                        self.set_lineno(saved_lineno);
                        return dst;
                    }
                    let name_idx = self.intern_name(name);
                    self.set_col_span_for_next(span);
                    self.emit(Insn::CheckLocal(reg, name_idx));
                }
                reg
            } else {
                // global / nonlocal / cell / free variable
                let dst = self.alloc_temp();
                self.set_col_span_for_next(span);
                if self.is_class_body && !self.class_direct_env_names.contains(name.as_str()) {
                    if let Some(cell_idx) = self.resolve_class_body_deref(name) {
                        self.emit(Insn::LoadClassDeref(dst, cell_idx));
                    } else {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::LoadClassName(
                            dst,
                            crate::bytecode::NO_CLASS_LOCAL,
                            name_idx,
                        ));
                    }
                } else if self.is_class_body && self.nonlocal_names.contains(name.as_str()) {
                    if let Some(cell_idx) = self.resolve_cell_slot_for_write(name) {
                        self.emit(Insn::LoadCell(dst, cell_idx));
                    } else {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::LoadGlobal(dst, name_idx));
                    }
                } else if let Some(cell_idx) = self.resolve_cell_slot_for_read(name) {
                    self.emit(Insn::LoadCell(dst, cell_idx));
                } else {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::LoadGlobal(dst, name_idx));
                }
                dst
            };
        self.set_lineno(saved_lineno);
        result
    }

    fn compile_compare_expr(&mut self, left: &Expr, ops: &[(crate::ast::CmpOp, Expr)]) -> Reg {
        if ops.len() == 1 {
            let (cmp_op, right) = &ops[0];
            let lhs = self.compile_expr(left);
            let bin_op = BinaryOp::from(*cmp_op);
            let dst = self.ensure_dst(lhs);
            let rhs = self.compile_expr(right);
            self.emit(Insn::BinOp(dst, lhs, bin_op, rhs));
            self.free_temp(rhs);
            return dst;
        }
        // Chained comparison: a < b < c  →  (a < b) and (b < c)
        // Evaluate left once, then chain.
        let first_lhs = self.compile_expr(left);
        let result_dst = self.alloc_temp();
        let mut and_patches: Vec<usize> = Vec::new();
        let mut prev_rhs = first_lhs;
        for (i, (cmp_op, rhs_expr)) in ops.iter().enumerate() {
            let bin_op = BinaryOp::from(*cmp_op);
            let rhs = self.compile_expr(rhs_expr);
            let last = i == ops.len() - 1;
            // For the last comparison write directly into result_dst to
            // avoid a trailing Move(result_dst, cmp_dst).
            let cmp_dst = if last { result_dst } else { self.alloc_temp() };
            self.emit(Insn::BinOp(cmp_dst, prev_rhs, bin_op, rhs));
            if i > 0 {
                self.free_temp(prev_rhs);
            }
            if !last {
                self.emit(Insn::Move(result_dst, cmp_dst));
                self.free_temp(cmp_dst);
                let p = self.emit(Insn::JumpIfFalse(result_dst, 0));
                and_patches.push(p);
            }
            prev_rhs = rhs;
        }
        self.free_temp(prev_rhs);
        for p in and_patches {
            self.patch_jump(p);
        }
        self.free_temp(first_lhs);
        result_dst
    }

    fn compile_named_expr(&mut self, target: &Symbol, value: &Expr) -> Reg {
        let val_reg = self.compile_expr(value);
        if let Some(reg) = self.local_reg(target) {
            if val_reg != reg {
                self.emit(Insn::Move(reg, val_reg));
            }
            self.maybe_record_class_store(reg);
            self.mark_def(reg);
        } else if let Some(cell_idx) = self.resolve_cell_slot_for_write(target) {
            self.emit(Insn::StoreCell(cell_idx, val_reg));
        } else {
            let name_idx = self.intern_name(target);
            self.emit(Insn::StoreGlobal(name_idx, val_reg));
        }
        val_reg
    }
}
