impl Compiler {
    fn compile_assign(&mut self, target: &AssignTarget, expr: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    // List-comprehension accumulator init (`.acc = []`) in a
                    // single-clause, unconditional comp: reserve the result list
                    // to the source length up front (`BuildListReserve`), skipping
                    // the geometric-growth reallocations of repeated `append`.
                    // `.acc` and `.0` are compiler-internal names that cannot
                    // appear in user source, so this never intercepts user code.
                    if self.list_comp_presize
                        && name == ".acc"
                        && matches!(expr, Expr::List(items) if items.is_empty())
                        && let Some(src_reg) = self.local_reg(".0")
                    {
                        self.emit(Insn::BuildListReserve(reg, src_reg));
                        return;
                    }
                    self.compile_expr_into(expr, reg);
                    // Class-body `x = expr` is the common case; record the store
                    // for class-namespace insertion order.  (Outside class
                    // bodies this is a no-op — see `maybe_record_class_store`.)
                    self.maybe_record_class_store(reg);
                    // Issue #820: at module scope, keep module_globals_dict live.
                    if self.is_module_scope {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                    }
                } else {
                    // cell / nonlocal / global
                    let src = self.compile_expr(expr);
                    if let Some(cell_idx) = self.resolve_cell_slot_for_write(name) {
                        self.emit(Insn::StoreCell(cell_idx, src));
                    } else {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::StoreGlobal(name_idx, src));
                    }
                    self.free_temp(src);
                }
            }
            AssignTarget::Tuple(targets) => {
                // Check if any target is starred.
                let star_pos = targets
                    .iter()
                    .position(|t| matches!(t, AssignTarget::Starred(_)));

                if let Some(star_idx) = star_pos {
                    // Extended unpack: a, *b, c = seq
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
                    // Total destination registers: before + 1 (starred list) + after
                    let total = targets.len() as u32;
                    let src = self.compile_expr(expr);
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
                        src,
                        before,
                        after,
                        dst_base: base,
                    });
                    self.free_temp(src);
                    // Store results: targets[i] → R[base + i], where targets[star_idx] is the starred list
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        let inner = match t {
                            AssignTarget::Starred(inner) => inner.as_ref(),
                            other => other,
                        };
                        self.compile_store_unpack_target(inner, base + i);
                    }
                    self.next_temp = base;
                    return;
                }

                // No starred target — fast path: matching tuple literal
                if let Expr::Tuple(exprs) = expr
                    && exprs.len() == targets.len()
                    && !targets.is_empty()
                {
                    let mut target_regs: Vec<Option<Reg>> = Vec::with_capacity(targets.len());
                    let mut all_name_locals = true;
                    for t in targets.iter() {
                        match t {
                            AssignTarget::Name(name) => {
                                target_regs.push(self.local_reg(name));
                                if self.local_reg(name).is_none() {
                                    // cell or global — can still do fast path with temps
                                    all_name_locals = false;
                                }
                            }
                            _ => {
                                all_name_locals = false;
                                target_regs.push(None);
                            }
                        }
                    }
                    // If ALL are simple name→local, use the original fast path
                    if all_name_locals && target_regs.iter().all(|r| r.is_some()) {
                        let saved_next = self.next_temp;
                        let mut temps: Vec<Reg> = Vec::with_capacity(exprs.len());
                        for rhs_expr in exprs.iter() {
                            let r = self.compile_expr(rhs_expr);
                            let tmp = if r < self.base_temp {
                                let t = self.alloc_temp();
                                self.emit(Insn::Move(t, r));
                                t
                            } else {
                                r
                            };
                            temps.push(tmp);
                        }
                        if !self.failed {
                            for i in 0..targets.len() {
                                let dst = target_regs[i].unwrap();
                                let src_tmp = temps[i];
                                if src_tmp != dst {
                                    self.emit(Insn::Move(dst, src_tmp));
                                }
                                self.maybe_record_class_store(dst);
                                // Issue #820: sync into module_globals_dict at module scope.
                                if self.is_module_scope {
                                    // all_name_locals guard guarantees AssignTarget::Name here
                                    if let AssignTarget::Name(name) = &targets[i] {
                                        let name_idx = self.intern_name(name);
                                        self.emit(Insn::SyncModuleGlobal(dst, name_idx));
                                    }
                                }
                            }
                        }
                        self.next_temp = saved_next;
                        return;
                    }
                }

                let src = self.compile_expr(expr);
                let n = targets.len() as u32;
                if n == 0 {
                    self.free_temp(src);
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
                self.emit(Insn::Unpack(base, src, n));
                self.free_temp(src);
                for (i, t) in (0u32..).zip(targets.iter()) {
                    match t {
                        AssignTarget::Name(name) => {
                            if let Some(reg) = self.local_reg(name) {
                                self.emit(Insn::Move(reg, base + i));
                                self.maybe_record_class_store(reg);
                                // Issue #820: sync into module_globals_dict at module scope.
                                if self.is_module_scope {
                                    let name_idx = self.intern_name(name);
                                    self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                                }
                            } else if let Some(cell_idx) = self.resolve_cell_slot_for_write(name) {
                                self.emit(Insn::StoreCell(cell_idx, base + i));
                            } else {
                                let name_idx = self.intern_name(name);
                                self.emit(Insn::StoreGlobal(name_idx, base + i));
                            }
                        }
                        AssignTarget::Attr(obj_expr, attr, span) => {
                            let obj = self.compile_expr(obj_expr);
                            let name_idx = self.intern_name(attr);
                            // PEP 657 caret anchor (#2442): underline `obj.attr`
                            // if this store raises AttributeError.
                            self.set_col_span_for_next(*span);
                            self.emit(Insn::SetAttr(obj, name_idx, base + i));
                            self.free_temp(obj);
                        }
                        AssignTarget::Index(obj_expr, idx_expr) => {
                            let obj = self.compile_expr(obj_expr);
                            let idx = self.compile_expr(idx_expr);
                            self.emit(Insn::SetItem(obj, idx, base + i));
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
                            let slice_r = self.compile_slice_key(
                                lower.as_deref(),
                                upper.as_deref(),
                                step.as_deref(),
                            );
                            self.emit(Insn::SetItem(obj, slice_r, base + i));
                            self.free_temp(slice_r);
                            self.free_temp(obj);
                        }
                        AssignTarget::Tuple(_) => {
                            // Nested tuple unpack — compile recursively from the temp register
                            self.compile_store_unpack_target(t, base + i);
                        }
                        AssignTarget::Starred(_) => {
                            // Should not happen (handled above); treat as error
                            self.failed = true;
                            if self.error_msg.is_none() {
                                self.error_msg = Some(
                                    "unexpected starred target in non-extended unpack".to_string(),
                                );
                            }
                        }
                    }
                }
                self.next_temp = base;
            }
            AssignTarget::Attr(obj_expr, attr, span) => {
                let obj = self.compile_expr(obj_expr);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(attr);
                // PEP 657 caret anchor (#2442): underline `obj.attr` if this
                // store raises AttributeError.
                self.set_col_span_for_next(*span);
                self.emit(Insn::SetAttr(obj, name_idx, val));
                self.free_temp(val);
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, idx, val));
                self.free_temp(val);
                self.free_temp(idx);
                self.free_temp(obj);
            }
            AssignTarget::Slice {
                target: obj_expr,
                lower,
                upper,
                step,
            } => {
                // Plain `l[a:b] = rhs` normally lowers to `Stmt::SliceAssign`;
                // this arm covers an `AssignTarget::Slice` reaching the generic
                // assignment path (e.g. as a single target group), mirroring it.
                let obj = self.compile_expr(obj_expr);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, slice_r, val));
                self.free_temp(val);
                self.free_temp(slice_r);
                self.free_temp(obj);
            }
            AssignTarget::Starred(_) => {
                // Standalone starred target (validated away by parser; should not reach here)
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("starred assignment target must be in a list or tuple".to_string());
                }
            }
        }
    }

    fn compile_ann_assign(&mut self, name: &str, annotation: &Expr, value: Option<&Expr>) {
        // 1. If there's a value, compile it as a regular assignment.
        if let Some(val_expr) = value {
            let sym = Symbol::intern(name);
            self.compile_assign(&AssignTarget::Name(sym), val_expr);
            self.mark_target_def(&AssignTarget::Name(sym));
        }
        // 2. Function scope: annotations are NOT stored in __annotations__ at runtime.
        if self.is_function_scope {
            return;
        }
        // 3. Produce the annotation value: either evaluate the expression (eager,
        //    default) or store its source text as a string (PEP 563 lazy mode).
        let ann_reg = if self.future_annotations {
            self.compile_literal(Value::string(stringify_annotation(annotation)))
        } else {
            let r = self.compile_expr(annotation);
            if self.failed {
                self.free_temp(r);
                return;
            }
            r
        };
        // 4. Load the string key for this annotation.
        let name_str_val = crate::value::Value::string(name);
        let key_idx = self.intern_const(name_str_val);
        let key_reg = self.alloc_temp();
        self.emit(Insn::LoadConst(key_reg, key_idx));
        // 5. Load (or locate) the __annotations__ dict.
        let ann_dict_name = "__annotations__";
        let (dict_reg, is_temp) = if self.is_class_body {
            // Class bodies use LOAD_NAME semantics. Once f_locals has escaped,
            // its __annotations__ entry is authoritative over the seed register.
            let local = self
                .local_reg(ann_dict_name)
                .unwrap_or(crate::bytecode::NO_CLASS_LOCAL);
            let name_idx = self.intern_name(ann_dict_name);
            let r = self.alloc_temp();
            self.emit(Insn::LoadClassName(r, local, name_idx));
            (r, true)
        } else if let Some(reg) = self.local_reg(ann_dict_name) {
            (reg, false)
        } else {
            // Module scope: load via LoadGlobal.
            let ann_dict_idx = self.intern_name(ann_dict_name);
            let r = self.alloc_temp();
            self.emit(Insn::LoadGlobal(r, ann_dict_idx));
            (r, true)
        };
        // 6. __annotations__[name] = annotation_value
        self.emit(Insn::SetItem(dict_reg, key_reg, ann_reg));
        if is_temp {
            self.free_temp(dict_reg);
        }
        self.free_temp(key_reg);
        self.free_temp(ann_reg);
    }
}
