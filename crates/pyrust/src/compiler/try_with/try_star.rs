// PEP 654 except-star lowering.
impl Compiler {
    /// PEP 654 `except*` compilation.
    ///
    /// All handlers are tried sequentially against the same exception group.
    /// Each matching handler receives a sub-group of the matched exceptions;
    /// the group register is narrowed after each match so subsequent handlers
    /// only see the remaining (unhandled) exceptions.
    ///
    /// After all handlers, if any exceptions remain un-handled, they are
    /// re-raised as a new group.
    // AST-node compile entry: same syntactic-child arg shape as `compile_try`
    // (body/handlers/else/finally + their lineno tables) for the `except*` form.
    #[allow(clippy::too_many_arguments)]
    fn compile_try_star(
        &mut self,
        body: &[Stmt],
        handlers: &[crate::ast::ExceptHandler],
        else_branch: Option<&[Stmt]>,
        finally_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
        finally_linenos: &[u32],
    ) {
        // Outer finally handler patch (only if finally_branch is Some)
        let outer_finally_patch: Option<usize> = if finally_branch.is_some() {
            Some(self.emit(Insn::SetupExcept(0)))
        } else {
            None
        };

        // Inner handler patch for except* block
        let inner_handler_patch = self.emit(Insn::SetupExcept(0));

        // Register cleanup entries for early exits from the try body.
        if outer_finally_patch.is_some() {
            self.except_cleanups.push(EarlyExitCleanup::TryBody {
                finally_stmts: Some(finally_branch.unwrap().to_vec()),
            });
        }
        self.except_cleanups.push(EarlyExitCleanup::TryBody {
            finally_stmts: None,
        });

        self.compile_block_with_linenos(body, body_linenos);
        // Save the lineno after the try body so that the "no handler matched"
        // RaiseValue instruction is attributed to the try-body, not to some
        // handler body statement that happened to run last during dispatch.
        let try_body_lineno_star = self.current_lineno;

        self.except_cleanups.pop();
        if outer_finally_patch.is_some() {
            self.except_cleanups.pop();
        }

        if self.failed {
            return;
        }

        // Normal exit from try body
        self.emit(Insn::PopExcept);
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        if outer_finally_patch.is_some() {
            self.emit(Insn::PopExcept);
            let finally_stmts = finally_branch.unwrap();
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
        }
        let end_patch = self.emit(Insn::Jump(0));

        // ── Exception path ──
        self.patch_jump(inner_handler_patch);

        // Load the active exception into a group register.
        // This register will be narrowed by each MatchExceptStar.
        let group_reg = self.alloc_temp();
        self.emit(Insn::LoadExc(group_reg));

        for handler in handlers {
            if let Some(kind_expr) = &handler.kind {
                let type_reg = self.compile_expr(kind_expr);
                let subgroup_reg = self.alloc_temp();
                let skip_patch =
                    self.emit(Insn::MatchExceptStar(type_reg, group_reg, subgroup_reg, 0));
                self.free_temp(type_reg);

                // Bind the `as VAR` variable to the sub-group.
                let var_bind_cleanup = if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        self.emit(Insn::Move(reg, subgroup_reg));
                        self.mark_def(reg);
                        if self.is_module_scope {
                            let name_idx = self.intern_name(var_name);
                            self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        }
                        let module_name = if self.is_module_scope {
                            Some(self.intern_name(var_name))
                        } else {
                            None
                        };
                        Some(ExceptAsVarDel::Local {
                            register: reg,
                            module_name,
                        })
                    } else {
                        let name_idx = self.intern_name(var_name);
                        self.emit(Insn::StoreGlobal(name_idx, subgroup_reg));
                        Some(ExceptAsVarDel::Name(name_idx))
                    }
                } else {
                    None
                };

                // Pop outer finally handler before running handler body
                if outer_finally_patch.is_some() {
                    self.emit(Insn::PopExcept);
                }

                // Register except-body cleanup for early exits.
                self.except_cleanups.push(EarlyExitCleanup::ExceptBody {
                    finally_stmts: finally_branch.map(|s| s.to_vec()),
                    as_var_delete: var_bind_cleanup.clone(),
                });

                self.compile_block_with_linenos(&handler.body, &handler.body_linenos);

                self.except_cleanups.pop();

                if self.failed {
                    self.free_temp(subgroup_reg);
                    return;
                }

                // PEP 3110-style cleanup: delete the `as VAR` binding.
                self.emit_except_as_delete(var_bind_cleanup);

                self.free_temp(subgroup_reg);

                // `skip_patch` jumps here (no match → continue to next handler)
                self.patch_jump(skip_patch);
            }
        }

        // After all handlers: check if group_reg has remaining exceptions.
        // If group_reg is None (all matched), call EndExcept + jump to end.
        // If group_reg is a group, re-raise it.
        // We check by using JumpIfFalse on group_reg (None is falsy; a group is truthy).
        let remaining_check = self.emit(Insn::JumpIfFalse(group_reg, 0));
        // Remaining exceptions exist — re-raise the group.
        // If no handler matched: the outer SetupExcept is still active and will
        // catch this re-raise to run the finally block.
        // If a handler matched but left some exceptions (partial match): the outer
        // SetupExcept was already popped, so the finally will NOT run here; this
        // is a known limitation (see follow-up issue for except*+finally+partial-match).
        // Restore the try-body lineno so the re-raise is attributed to the
        // failing statement in the try block, not to handler body code.
        self.set_lineno(try_body_lineno_star);
        // PEP 654 (#2755): re-raise the residual group without spurious
        // implicit-context chaining or an extra epilogue traceback frame.
        self.emit(Insn::RaiseExceptStarResidual(group_reg));
        self.patch_jump(remaining_check);

        // No remaining exceptions — clean up normally.
        self.free_temp(group_reg);
        self.emit(Insn::EndExcept);

        if let Some(finally_stmts) = finally_branch {
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
        }

        // Outer finally handler (exception path for exceptions that escape the
        // outer SetupExcept — i.e. exceptions raised inside the try body before
        // any handler fires, or exceptions raised by the handler bodies themselves
        // that re-activate the outer SetupExcept).
        if let Some(outer_idx) = outer_finally_patch {
            // After EndExcept + inline finally above, we must jump past the outer
            // finally handler block — otherwise execution falls through into it
            // and hits RaiseReRaise / LoadExc with no active exception.
            let exc_path_end = self.emit(Insn::Jump(0));

            self.patch_jump(outer_idx);
            let exc_tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(exc_tmp));
            self.free_temp(exc_tmp);
            let finally_stmts = finally_branch.unwrap();
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
            self.emit(Insn::RaiseReRaise);

            // Both the normal-exit Jump and the exception-path Jump land here
            // (past the outer finally handler).
            self.patch_jump(exc_path_end);
        }

        // Patch the normal-exit Jump to land here (past the outer finally handler).
        self.patch_jump(end_patch);
    }
}
