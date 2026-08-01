impl Compiler {
    fn compile_stmt(&mut self, stmt: &Stmt) {
        if self.failed {
            return;
        }
        match stmt {
            Stmt::Pass => {}
            Stmt::Break => {
                if self.loops.is_empty() {
                    self.set_syntax_error("'break' outside loop");
                    return;
                }
                let last = self.loops.len() - 1;
                let depth = self.loops[last].cleanup_depth;
                self.emit_early_exit_cleanups(depth);
                if self.failed {
                    return;
                }
                let idx = self.emit(Insn::Jump(0));
                let last = self.loops.len() - 1;
                self.loops[last].break_patches.push(idx);
            }
            Stmt::Continue => {
                if self.loops.is_empty() {
                    self.set_syntax_error("'continue' not properly in loop");
                    return;
                }
                let last = self.loops.len() - 1;
                let depth = self.loops[last].cleanup_depth;
                self.emit_early_exit_cleanups(depth);
                if self.failed {
                    return;
                }
                let last = self.loops.len() - 1;
                let idx = self.emit(Insn::Jump(0));
                if let Some(target) = self.loops[last].continue_target {
                    let from = idx as i32 + 1;
                    let offset = target as i32 - from;
                    if let Insn::Jump(off) = &mut self.insns[idx] {
                        *off = offset;
                    }
                } else {
                    self.loops[last].continue_patches.push(idx);
                }
            }
            Stmt::Return(None) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'return' outside function");
                    return;
                }
                self.emit_early_exit_cleanups(0);
                if self.failed {
                    return;
                }
                self.emit(Insn::ReturnNone);
            }
            Stmt::Return(Some(expr)) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'return' outside function");
                    return;
                }
                // `return <value>` (including a literal `return None`) inside an
                // async generator is a SyntaxError (#2280); only a bare `return`
                // is allowed.  Matches CPython 3.12.
                if self.is_async_generator_fn {
                    self.set_syntax_error("'return' with value in async generator");
                    return;
                }
                let r = self.compile_expr(expr);
                // `emit_early_exit_cleanups` may emit `DeleteLocal` for the
                // `except … as e` variable (PEP 3110).  If the return
                // expression compiled directly to that same fastlocal register
                // (e.g. `return e`), the deletion would clobber the value
                // before `Return` reads it.  Copy to a temp first so the
                // return value survives any cleanup deletions.
                let r = self.ensure_temp(r);
                self.emit_early_exit_cleanups(0);
                if self.failed {
                    self.free_temp(r);
                    return;
                }
                self.emit(Insn::Return(r));
                self.free_temp(r);
            }
            Stmt::Expr(expr) => {
                if self.try_emit_set_comp_add(expr) {
                    return;
                }
                if self.try_emit_list_comp_append(expr) {
                    return;
                }
                let r = self.compile_discarded_expr(expr);
                self.free_temp(r);
            }
            Stmt::Assign(target, expr) => {
                self.compile_assign(target, expr);
                self.mark_target_def(target);
            }
            Stmt::AnnAssign {
                name,
                annotation,
                value,
            } => {
                self.compile_ann_assign(name, annotation, value.as_ref().map(|v| v as &Expr));
            }
            Stmt::AugAssign { target, op, expr } => {
                self.compile_aug_assign(target, *op, expr);
                if let AssignTarget::Name(name) = target
                    && let Some(reg) = self.local_reg(name)
                {
                    self.mark_def(reg);
                }
            }
            Stmt::AttrAssign {
                target,
                name,
                expr,
                span,
            } => {
                let obj = self.compile_expr(target);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(name);
                // PEP 657 caret anchor (#2442): underline the whole `obj.attr`
                // target span when the SetAttr raises AttributeError.  Arm
                // immediately before the SetAttr; `emit` consumes and clears it.
                self.set_col_span_for_next(*span);
                self.emit(Insn::SetAttr(obj, name_idx, val));
                self.free_temp(val);
                self.free_temp(obj);
            }
            Stmt::IndexAssign {
                target,
                index,
                expr,
            } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, idx, val));
                self.free_temp(val);
                self.free_temp(idx);
                self.writeback_container_if_global(target, obj);
                self.free_temp(obj);
            }
            Stmt::SliceAssign {
                target,
                lower,
                upper,
                step,
                expr,
            } => {
                let obj = self.compile_expr(target);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, slice_r, val));
                self.writeback_container_if_global(target, obj);
                self.free_temp(val);
                self.free_temp(slice_r);
                self.free_temp(obj);
            }
            Stmt::Assert { test, msg } => {
                let cond = self.compile_expr(test);
                let skip = self.emit(Insn::JumpIfTrue(cond, 0));
                self.free_temp(cond);
                if let Some(msg_expr) = msg {
                    let msg_reg = self.compile_expr(msg_expr);
                    self.emit(Insn::RaiseAssert(msg_reg));
                    self.free_temp(msg_reg);
                } else {
                    self.emit(Insn::RaiseAssertNoMsg);
                }
                self.patch_jump(skip);
            }
            Stmt::If {
                branches,
                else_branch,
                branch_linenos,
                else_linenos,
            } => {
                self.compile_if(
                    branches,
                    else_branch.as_deref(),
                    branch_linenos,
                    else_linenos,
                );
            }
            Stmt::While {
                cond,
                body,
                else_branch,
                body_linenos,
                else_linenos,
            } => {
                self.compile_while(
                    cond,
                    body,
                    else_branch.as_deref(),
                    body_linenos,
                    else_linenos,
                );
            }
            Stmt::For {
                target,
                iter,
                body,
                else_branch,
                body_linenos,
                else_linenos,
                is_async,
            } => {
                if *is_async {
                    self.compile_async_for(
                        target,
                        iter,
                        body,
                        else_branch.as_deref(),
                        body_linenos,
                        else_linenos,
                    );
                } else {
                    self.compile_for(
                        target,
                        iter,
                        body,
                        else_branch.as_deref(),
                        body_linenos,
                        else_linenos,
                    );
                }
            }
            Stmt::Global(_) => {
                // Purely a compile-time declaration; no runtime effect.
            }
            Stmt::Nonlocal(_) => {
                // Nonlocal is a compile-time declaration in function bodies.
                // At module level (not inside any function or class), it is a
                // SyntaxError — CPython rejects it at compile time.
                if !self.is_function_scope && !self.is_class_body {
                    self.set_syntax_error("nonlocal declaration not allowed at module level");
                }
            }
            Stmt::Raise { expr, cause, span } => {
                self.compile_raise(expr.as_ref(), cause.as_ref(), *span);
            }
            Stmt::Delete(exprs) => {
                for expr in exprs {
                    self.compile_delete(expr);
                    if self.failed {
                        return;
                    }
                }
            }
            Stmt::Import { names } => {
                self.compile_import(names);
            }
            Stmt::ImportFrom { module, names } => {
                self.compile_import_from(module, names);
            }
            Stmt::Def {
                name,
                params,
                body,
                body_linenos,
                def_lineno,
                decorators,
                return_annotation,
                is_async,
                type_params,
            } => {
                self.compile_def(
                    name,
                    params,
                    body,
                    body_linenos,
                    *def_lineno,
                    decorators,
                    return_annotation.as_ref(),
                    *is_async,
                    type_params,
                );
            }
            Stmt::Class {
                name,
                bases,
                metaclass,
                keywords,
                body,
                decorators,
                type_params,
            } => {
                self.compile_class(
                    name,
                    bases,
                    metaclass.as_ref(),
                    keywords,
                    body,
                    decorators,
                    type_params,
                );
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                body_linenos,
                else_linenos,
                finally_linenos,
            } => {
                self.compile_try(
                    body,
                    handlers,
                    else_branch.as_deref(),
                    finally_branch.as_deref(),
                    body_linenos,
                    else_linenos,
                    finally_linenos,
                );
            }
            Stmt::With {
                items,
                body,
                body_linenos,
                is_async,
            } => {
                if *is_async {
                    self.compile_async_with(items, body, body_linenos);
                } else {
                    self.compile_with(items, body, body_linenos);
                }
            }
            Stmt::Match { subject, arms } => {
                self.compile_match(subject, arms);
            }
            Stmt::TypeAlias {
                name,
                type_params,
                value,
            } => {
                self.compile_type_alias(name, type_params, value);
            }
        }
    }

    // ── Type alias (PEP 695) ──────────────────────────────────────────────────
}
