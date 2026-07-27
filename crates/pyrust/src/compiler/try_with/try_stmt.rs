impl Compiler {
    // ── Try / With ────────────────────────────────────────────────────────────

    // AST-node compile entry: body/handlers/else/finally plus their parallel
    // lineno tables; each is a distinct syntactic child of the `try` statement.
    #[allow(clippy::too_many_arguments)]
    fn compile_try(
        &mut self,
        body: &[Stmt],
        handlers: &[crate::ast::ExceptHandler],
        else_branch: Option<&[Stmt]>,
        finally_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
        finally_linenos: &[u32],
    ) {
        // PEP 654: if any handler is `except*`, route to the star compilation path.
        // (Mixing `except` and `except*` is a SyntaxError in CPython, so we treat
        // all-star or all-non-star as the two cases.)
        let has_star_handlers = handlers.iter().any(|h| h.is_star);
        if has_star_handlers {
            self.compile_try_star(
                body,
                handlers,
                else_branch,
                finally_branch,
                body_linenos,
                else_linenos,
                finally_linenos,
            );
            return;
        }

        let has_handlers = !handlers.is_empty();

        // Strategy:
        // 1. If we have finally: wrap everything in an outer SetupExcept for finally.
        // 2. If we have handlers: inner SetupExcept for the except clause.
        // Normal-exit path: run else (if any), then finally (if any).
        // Exception path: dispatch handlers; on match run handler then finally;
        //                 on no-match re-raise (outer finally catches it).

        // Outer finally handler patch (only if finally_branch is Some)
        let outer_finally_patch: Option<usize> = if finally_branch.is_some() {
            Some(self.emit(Insn::SetupExcept(0)))
        } else {
            None
        };

        // Inner handler patch (only if has_handlers)
        let inner_handler_patch: Option<usize> = if has_handlers {
            Some(self.emit(Insn::SetupExcept(0)))
        } else {
            None
        };

        // Register cleanup entries so that early exits (break/continue/return)
        // from the try body emit the correct PopExcept + finally sequence.
        // The outermost handler is pushed first (will be cleaned up last).
        if outer_finally_patch.is_some() {
            self.except_cleanups.push(EarlyExitCleanup::TryBody {
                finally_stmts: Some(finally_branch.unwrap().to_vec()),
            });
        }
        if inner_handler_patch.is_some() {
            // Inner except handler: no finally at this level (finally belongs to outer).
            self.except_cleanups.push(EarlyExitCleanup::TryBody {
                finally_stmts: None,
            });
        }

        // Compile try body
        self.compile_block_with_linenos(body, body_linenos);
        // Save the lineno after the try body so that the "no handler matched"
        // RaiseReRaise instruction is attributed to the try-body, not to some
        // handler body statement that happened to run last during dispatch.
        let try_body_lineno = self.current_lineno;

        // Pop the try-body cleanup entries before emitting normal-exit cleanup.
        if inner_handler_patch.is_some() {
            self.except_cleanups.pop();
        }
        if outer_finally_patch.is_some() {
            self.except_cleanups.pop();
        }

        if self.failed {
            return;
        }

        // Normal exit from try body:
        if inner_handler_patch.is_some() {
            self.emit(Insn::PopExcept);
        }
        // Compile else branch (normal path only)
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        // Normal finally exit
        if outer_finally_patch.is_some() {
            self.emit(Insn::PopExcept);
            let finally_stmts = finally_branch.unwrap();
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
        }
        // Jump over handlers + exception path
        let end_patch = self.emit(Insn::Jump(0));

        // ── Exception path ──
        if let Some(inner_idx) = inner_handler_patch {
            self.patch_jump(inner_idx);
        }

        let mut handler_end_patches: Vec<usize> = Vec::new();

        if has_handlers {
            let exc_tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(exc_tmp));

            for handler in handlers {
                let skip_patch: Option<usize> = if let Some(kind_expr) = &handler.kind {
                    let type_reg = self.compile_expr(kind_expr);
                    let p = self.emit(Insn::MatchExcept(type_reg, 0));
                    self.free_temp(type_reg);
                    Some(p)
                } else {
                    None
                };

                // Bind exception variable if `as VAR`
                if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        self.emit(Insn::Move(reg, exc_tmp));
                        self.mark_def(reg);
                        // Issue #820: sync into module_globals_dict at module scope.
                        if self.is_module_scope {
                            let name_idx = self.intern_name(var_name);
                            self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        }
                    } else {
                        let name_idx = self.intern_name(var_name);
                        self.emit(Insn::StoreGlobal(name_idx, exc_tmp));
                    }
                }

                // Pop outer finally handler before running handler body
                // (so that exceptions in the handler don't double-run finally)
                if outer_finally_patch.is_some() {
                    self.emit(Insn::PopExcept);
                }

                // Register an except-body cleanup so that early exits from the
                // handler body (break/continue/return) emit the PEP 3110 as-var
                // deletion, EndExcept, and the inlined finally block before jumping.
                let as_var_delete = if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
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
                        Some(ExceptAsVarDel::Name(name_idx))
                    }
                } else {
                    None
                };
                self.except_cleanups.push(EarlyExitCleanup::ExceptBody {
                    finally_stmts: finally_branch.map(|s| s.to_vec()),
                    as_var_delete: as_var_delete.clone(),
                });

                self.compile_block_with_linenos(&handler.body, &handler.body_linenos);

                // Remove the except-body cleanup before emitting normal handler exit.
                self.except_cleanups.pop();

                if self.failed {
                    return;
                }
                // PEP 3110: delete the `as VAR` binding when the handler exits
                // (breaks reference cycles and matches CPython behaviour).
                // Use u16::MAX as the name_idx sentinel: the variable is
                // always bound at this point (the except clause only runs
                // when the exception matched), so no NameError check needed.
                self.emit_except_as_delete(as_var_delete);
                self.emit(Insn::EndExcept);

                // Run finally (inline) after successful handler
                if let Some(finally_stmts) = finally_branch {
                    self.compile_block_with_linenos(finally_stmts, finally_linenos);
                    if self.failed {
                        return;
                    }
                }

                let jmp = self.emit(Insn::Jump(0));
                handler_end_patches.push(jmp);

                if let Some(p) = skip_patch {
                    self.patch_jump(p);
                }
            }

            // No handler matched: re-raise (outer finally will catch it if present).
            // Restore the try-body lineno so the re-raise is attributed to the
            // failing statement in the try block, not to handler body code.
            self.set_lineno(try_body_lineno);
            self.free_temp(exc_tmp);
            self.emit(Insn::RaiseReRaise);
        }

        // ── Outer finally handler (exception path) ──
        if let Some(outer_idx) = outer_finally_patch {
            if !has_handlers {
                // No handlers: patch the inner SetupExcept → this finally handler
                self.patch_jump(outer_idx);
            }
            // If has_handlers, outer_finally_patch was patched when? Actually not yet.
            // For try/except/finally: the outer SetupExcept should catch exceptions
            // that escape the handlers (or re-raised). We patch it here.
            if has_handlers {
                self.patch_jump(outer_idx);
            }
            let finally_stmts = finally_branch.unwrap();
            // Load exception and run finally then re-raise
            let exc_tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(exc_tmp));
            self.free_temp(exc_tmp);
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
            // Restore the try-body lineno so the re-raise is attributed to the
            // failing statement in the try block, not to the finally body.
            self.set_lineno(try_body_lineno);
            self.emit(Insn::RaiseReRaise);
        }

        // Patch all successful handler jumps to here (after everything)
        self.patch_jump(end_patch);
        for idx in handler_end_patches {
            self.patch_jump(idx);
        }
    }
}
