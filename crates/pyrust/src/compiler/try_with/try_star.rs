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

        // Allocate four contiguous temps packed into a 4-tuple for
        // `RaiseExceptStarResidual`:
        //   0: orig_reg         — original exception raised by the `try` body
        //   1: group_reg        — remaining unhandled exceptions (narrowed per match)
        //   2: raised_list_reg  — exceptions raised inside matched `except*` handlers
        //   3: matched_list_reg — matched subgroups handed to each handler that ran
        let orig_reg = self.alloc_temp();
        let group_reg = self.alloc_temp();
        let raised_list_reg = self.alloc_temp();
        let matched_list_reg = self.alloc_temp();
        debug_assert_eq!(group_reg, orig_reg + 1);
        debug_assert_eq!(raised_list_reg, orig_reg + 2);
        debug_assert_eq!(matched_list_reg, orig_reg + 3);
        self.emit(Insn::LoadExc(orig_reg));
        self.emit(Insn::LoadExc(group_reg));
        self.emit(Insn::BuildList(raised_list_reg, 0, 0));
        self.emit(Insn::BuildList(matched_list_reg, 0, 0));

        for handler in handlers {
            if let Some(kind_expr) = &handler.kind {
                let type_reg = self.compile_expr(kind_expr);
                let subgroup_reg = self.alloc_temp();
                let skip_patch =
                    self.emit(Insn::MatchExceptStar(type_reg, group_reg, subgroup_reg, 0));
                self.free_temp(type_reg);

                self.emit(Insn::ListAppend(matched_list_reg, subgroup_reg));

                // Bind the `as VAR` variable to the matched sub-group.
                let var_bind_cleanup = if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        self.emit(Insn::Move(reg, subgroup_reg));
                        self.mark_def(reg);
                        self.maybe_record_class_store(reg);
                        if self.is_module_scope {
                            let name_idx = self.intern_name(var_name);
                            self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        }
                        let name_index = self.intern_name(var_name);
                        Some(ExceptAsVarDel::Local {
                            register: reg,
                            name_index,
                            delete_module_global: self.is_module_scope,
                        })
                    } else if let Some(cell_idx) = self.resolve_cell_slot_for_write(var_name) {
                        self.emit(Insn::StoreCell(cell_idx, subgroup_reg));
                        Some(ExceptAsVarDel::Cell(cell_idx))
                    } else {
                        let name_idx = self.intern_name(var_name);
                        self.emit(Insn::StoreGlobal(name_idx, subgroup_reg));
                        Some(ExceptAsVarDel::Name(name_idx))
                    }
                } else {
                    None
                };

                // PEP 654 (#2993): each `except*` handler executes with its
                // matched `ExceptionGroup` installed as the active exception
                // (so bare `raise` re-raises that group and new raises attach
                // it as `__context__`), inside a handler-local catch that
                // records any raised exception into `raised_list_reg` without
                // aborting subsequent `except*` clauses.
                let handler_catch_patch = self.emit(Insn::SetupExcept(0));
                self.emit(Insn::PushExcContext(subgroup_reg));

                // Register except-body cleanup for early exits (return/break/
                // continue directly inside `except*` is a SyntaxError, but
                // cleanup still guards nested constructs if any).
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

                // Normal completion of this handler body.
                self.emit(Insn::PopExcContext);
                self.emit(Insn::PopExcept);
                self.emit_except_as_delete(var_bind_cleanup.clone());
                let handler_done = self.emit(Insn::Jump(0));

                // Handler raised an exception: collect it and keep running
                // subsequent `except*` handlers against the remaining group.
                self.patch_jump(handler_catch_patch);
                let raised_reg = self.alloc_temp();
                self.emit(Insn::LoadExc(raised_reg));
                self.emit(Insn::ListAppend(raised_list_reg, raised_reg));
                self.free_temp(raised_reg);
                self.emit(Insn::EndExcept);
                self.emit(Insn::PopExcContext);
                self.emit_except_as_delete(var_bind_cleanup);

                self.patch_jump(handler_done);
                self.free_temp(subgroup_reg);

                // `skip_patch` jumps here (no match → continue to next handler)
                self.patch_jump(skip_patch);
            }
        }

        // After all handlers: if any handler raised OR unhandled exceptions
        // remain in `group_reg`, compute and raise the PEP 654 result via
        // `RaiseExceptStarResidual`.
        let no_raised = self.emit(Insn::JumpIfFalse(raised_list_reg, 0));
        let do_reraise_from_raised = self.emit(Insn::Jump(0));
        self.patch_jump(no_raised);
        let all_handled = self.emit(Insn::JumpIfFalse(group_reg, 0));
        self.patch_jump(do_reraise_from_raised);

        let pack_reg = self.alloc_temp();
        self.emit(Insn::BuildTuple(pack_reg, orig_reg, 4));
        // Restore the try-body lineno so a residual re-raise is attributed to
        // the failing statement in the try block, not to handler body code.
        self.set_lineno(try_body_lineno_star);
        // PEP 654 (#2755 / #2993): combine/re-raise residual + raised groups.
        self.emit(Insn::RaiseExceptStarResidual(pack_reg));
        self.free_temp(pack_reg);
        self.patch_jump(all_handled);

        // No remaining exceptions and nothing raised — clean up normally.
        self.free_temp(matched_list_reg);
        self.free_temp(raised_list_reg);
        self.free_temp(group_reg);
        self.free_temp(orig_reg);
        self.emit(Insn::EndExcept);

        if outer_finally_patch.is_some() {
            self.emit(Insn::PopExcept);
        }
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
