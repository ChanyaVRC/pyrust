impl Compiler {
    /// Store the value in `src_reg` into `target` (a non-starred inner target).
    fn compile_store_unpack_target(&mut self, target: &AssignTarget, src_reg: Reg) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    if reg != src_reg {
                        self.emit(Insn::Move(reg, src_reg));
                    }
                    self.maybe_record_class_store(reg);
                    // Issue #820: sync into module_globals_dict at module scope.
                    if self.is_module_scope {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                    }
                } else if let Some(cell_idx) = self.resolve_cell_slot_for_write(name) {
                    self.emit(Insn::StoreCell(cell_idx, src_reg));
                } else {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::StoreGlobal(name_idx, src_reg));
                }
            }
            AssignTarget::Attr(obj_expr, attr, span) => {
                let obj = self.compile_expr(obj_expr);
                let name_idx = self.intern_name(attr);
                // PEP 657 caret anchor (#2442): underline `obj.attr` if this
                // store raises AttributeError.
                self.set_col_span_for_next(*span);
                self.emit(Insn::SetAttr(obj, name_idx, src_reg));
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                self.emit(Insn::SetItem(obj, idx, src_reg));
                self.free_temp(idx);
                self.free_temp(obj);
            }
            AssignTarget::Slice {
                target: obj_expr,
                lower,
                upper,
                step,
            } => {
                let obj = self.compile_expr(obj_expr);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                self.emit(Insn::SetItem(obj, slice_r, src_reg));
                self.free_temp(slice_r);
                self.free_temp(obj);
            }
            AssignTarget::Tuple(targets) => {
                // Nested unpack — unpack directly from src_reg into sub-targets.
                let star_pos = targets
                    .iter()
                    .position(|t| matches!(t, AssignTarget::Starred(_)));
                if let Some(star_idx) = star_pos {
                    // Extended unpack: (a, *b, c) = src_reg
                    let before = match u8::try_from(star_idx) {
                        Ok(count) => count,
                        Err(_) => {
                            self.failed = true;
                            self.is_syntax_error = true;
                            if self.error_msg.is_none() {
                                self.error_msg = Some(
                                    "too many expressions in star-unpacking assignment".into(),
                                );
                            }
                            return;
                        }
                    };
                    let after = (targets.len() - star_idx - 1) as u32;
                    let total = targets.len() as u32;
                    let base = self.next_temp;
                    if base.checked_add(total).is_none() {
                        self.failed = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(format!("too many unpack targets ({})", total));
                        }
                        return;
                    }
                    self.next_temp = base + total;
                    if self.next_temp - 1 > self.max_reg {
                        self.max_reg = self.next_temp - 1;
                    }
                    self.emit(Insn::UnpackEx {
                        src: src_reg,
                        before,
                        after,
                        dst_base: base,
                    });
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        let inner = match t {
                            AssignTarget::Starred(inner) => inner.as_ref(),
                            other => other,
                        };
                        self.compile_store_unpack_target(inner, base + i);
                    }
                    self.next_temp = base;
                } else {
                    // Simple unpack: (a, b, c) = src_reg
                    let n = targets.len() as u32;
                    if n == 0 {
                        return;
                    }
                    let base = self.next_temp;
                    if base.checked_add(n).is_none() {
                        self.failed = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(format!("too many unpack targets ({})", n));
                        }
                        return;
                    }
                    self.next_temp = base + n;
                    if self.next_temp - 1 > self.max_reg {
                        self.max_reg = self.next_temp - 1;
                    }
                    self.emit(Insn::Unpack(base, src_reg, n));
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        self.compile_store_unpack_target(t, base + i);
                    }
                    self.next_temp = base;
                }
            }
            AssignTarget::Starred(_) => {
                // Bare starred outside a tuple — should not reach here
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("starred assignment target must be in a list or tuple".to_string());
                }
            }
        }
    }

    fn compile_aug_assign(&mut self, target: &AssignTarget, op: BinaryOp, expr: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    let definitely_bound = (reg as usize) < 64 && (self.def_set >> reg) & 1 != 0;
                    if self.is_class_body {
                        let name_idx = self.intern_name(name);
                        let lhs = self.alloc_temp();
                        self.emit(Insn::LoadClassName(lhs, reg, name_idx));
                        self.emit_aug_binop(lhs, op, expr);
                        self.emit(Insn::Move(reg, lhs));
                        self.maybe_record_class_store(reg);
                        self.free_temp(lhs);
                    } else if self.is_module_scope && !definitely_bound {
                        // Issue #1411: at module scope a name that is not yet
                        // definitely bound must be read through the global →
                        // builtins chain, not from the unset fastlocal register.
                        let name_idx = self.intern_name(name);
                        let lhs = self.alloc_temp();
                        self.emit(Insn::LoadGlobal(lhs, name_idx));
                        self.emit_aug_binop(lhs, op, expr);
                        self.emit(Insn::Move(reg, lhs));
                        self.mark_def(reg);
                        self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        self.free_temp(lhs);
                    } else {
                        // Issue #1644: at function scope, a local that is not yet
                        // definitely bound must be guarded by CheckLocal so that
                        // an unset register produces UnboundLocalError.
                        if !self.is_module_scope && !definitely_bound {
                            let name_idx = self.intern_name(name);
                            self.emit(Insn::CheckLocal(reg, name_idx));
                        }
                        self.emit_aug_binop(reg, op, expr);
                        self.maybe_record_class_store(reg);
                        if self.is_module_scope {
                            let name_idx = self.intern_name(name);
                            self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        }
                    }
                } else {
                    // cell / global: load, compute, store (#452 / #2339 / #2348).
                    let lhs = self.alloc_temp();
                    if let Some(cell_idx) = self.resolve_cell_slot_for_write(name) {
                        self.emit(Insn::LoadCell(lhs, cell_idx));
                        self.emit_aug_binop(lhs, op, expr);
                        self.emit(Insn::StoreCell(cell_idx, lhs));
                    } else {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::LoadGlobal(lhs, name_idx));
                        self.emit_aug_binop(lhs, op, expr);
                        self.emit(Insn::StoreGlobal(name_idx, lhs));
                    }
                    self.free_temp(lhs);
                }
            }
            AssignTarget::Attr(obj_expr, attr, span) => {
                let obj = self.compile_expr(obj_expr);
                let name_idx = self.intern_name(attr);
                let lhs = self.alloc_temp();
                // PEP 657 caret anchor (#2442): underline `obj.attr` for both
                // the read and write-back.
                self.set_col_span_for_next(*span);
                self.emit(Insn::GetAttr(lhs, obj, name_idx));
                self.emit_aug_binop(lhs, op, expr);
                self.set_col_span_for_next(*span);
                self.emit(Insn::SetAttr(obj, name_idx, lhs));
                self.free_temp(lhs);
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                let lhs = self.alloc_temp();
                self.emit(Insn::GetItem(lhs, obj, idx));
                self.emit_aug_binop(lhs, op, expr);
                self.emit(Insn::SetItem(obj, idx, lhs));
                self.writeback_container_if_global(obj_expr, obj);
                self.free_temp(lhs);
                self.free_temp(idx);
                self.free_temp(obj);
            }
            AssignTarget::Slice {
                target: obj_expr,
                lower,
                upper,
                step,
            } => {
                let obj = self.compile_expr(obj_expr);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                let lhs = self.alloc_temp();
                self.emit(Insn::GetItem(lhs, obj, slice_r));
                self.emit_aug_binop(lhs, op, expr);
                self.emit(Insn::SetItem(obj, slice_r, lhs));
                self.writeback_container_if_global(obj_expr, obj);
                self.free_temp(lhs);
                self.free_temp(slice_r);
                self.free_temp(obj);
            }
            AssignTarget::Tuple(_) | AssignTarget::Starred(_) => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(
                        "'tuple' is an illegal expression for augmented assignment".to_string(),
                    );
                }
            }
        }
    }
}
