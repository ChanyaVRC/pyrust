// Synchronous context-manager lowering.
impl Compiler {
    fn compile_with(
        &mut self,
        items: &[(Expr, Option<AssignTarget>)],
        body: &[Stmt],
        body_linenos: &[u32],
    ) {
        // Compile nested with items recursively (outermost first).
        if items.is_empty() {
            self.compile_block_with_linenos(body, body_linenos);
            return;
        }
        let (expr, alias) = &items[0];
        let rest = &items[1..];

        // Capture the `with` header line so the exception-unwind path below can
        // attribute the enclosing frame's traceback node to it.  When `__exit__`
        // raises (or re-raises) while an exception is in flight, CPython points
        // the enclosing frame at the `with` statement line, not at whatever line
        // inside the body originally raised (issue #2419).
        let with_header_lineno = self.current_lineno;

        // ctx = expr
        let ctx_reg = self.compile_expr(expr);

        // VAR = ctx.__enter__()
        // Use GetAttrForWith so AttributeError is converted to TypeError (#1656).
        let enter_name_idx = self.intern_name("__enter__");
        let enter_reg = self.alloc_temp();
        self.emit(Insn::GetAttrForWith(
            enter_reg,
            ctx_reg,
            enter_name_idx,
            0, // sync with: __enter__
        ));
        // Call __enter__() with no args: result goes to enter_reg
        self.emit(Insn::Call(enter_reg, 0));

        // Bind alias if present
        if let Some(tgt) = alias {
            let val_reg = enter_reg;
            match tgt {
                AssignTarget::Name(name) => {
                    if let Some(reg) = self.local_reg(name) {
                        self.emit(Insn::Move(reg, val_reg));
                        self.mark_def(reg);
                        self.maybe_record_class_store(reg);
                        // Issue #820: sync into module_globals_dict at module scope.
                        if self.is_module_scope {
                            let name_idx = self.intern_name(name);
                            self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        }
                    } else {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::StoreGlobal(name_idx, val_reg));
                    }
                }
                _ => {
                    // Complex targets: just assign via the general mechanism
                    // (simplified: ignore for now)
                }
            }
        }

        // SetupExcept for the body
        let setup_patch = self.emit(Insn::SetupExcept(0));

        // Register the with-exit cleanup so a `break`/`continue`/`return` that
        // leaves the body runs `__exit__(None, None, None)` (issue #2295).
        self.except_cleanups.push(EarlyExitCleanup::WithBody {
            ctx_reg,
            is_async: false,
        });

        // Compile nested with items or body
        if rest.is_empty() {
            self.compile_block_with_linenos(body, body_linenos);
        } else {
            self.compile_with(rest, body, body_linenos);
        }
        // Pop our cleanup entry; the normal/exception paths below emit the exit
        // inline, and inner break/continue/return already consumed it.
        self.except_cleanups.pop();
        if self.failed {
            return;
        }

        // Normal exit
        self.emit(Insn::PopExcept);
        // ctx.__exit__(None, None, None)
        let exit_name_idx = self.intern_name("__exit__");
        self.emit_with_normal_exit(ctx_reg);
        if self.failed {
            return;
        }
        let end_patch = self.emit(Insn::Jump(0));

        // Exception path
        self.patch_jump(setup_patch);
        // Attribute the enclosing frame to the `with` header line (not the body
        // line that raised) for the duration of the unwind-path `__exit__` call
        // and any re-raise it triggers (issue #2419).  The body was compiled
        // above, leaving `current_lineno` pointing at its last statement.
        self.set_lineno(with_header_lineno);
        let exc_tmp = self.alloc_temp();
        self.emit(Insn::LoadExc(exc_tmp));
        // ctx.__exit__(type, exc, None)
        let exit_frame2 = self.next_temp;
        if exit_frame2.checked_add(4).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many registers for 'with' exception handler".to_string());
            }
            return;
        }
        self.next_temp = exit_frame2 + 4;
        if exit_frame2 + 3 > self.max_reg {
            self.max_reg = exit_frame2 + 3;
        }
        let class_name_idx = self.intern_name("__class__");
        self.emit(Insn::GetAttrForWith(
            exit_frame2,
            ctx_reg,
            exit_name_idx,
            1, // sync with: __exit__
        ));
        self.emit(Insn::GetAttr(exit_frame2 + 1, exc_tmp, class_name_idx)); // exc_type
        self.emit(Insn::Move(exit_frame2 + 2, exc_tmp));
        // traceback: the real `__traceback__` of the in-flight exception (#2359),
        // materialised from its deferred placeholder.
        self.emit(Insn::LoadExcTraceback(exit_frame2 + 3, exc_tmp));
        self.emit(Insn::Call(exit_frame2, 3));
        let suppress_reg = exit_frame2;
        self.next_temp = exit_frame2 + 1;
        // If __exit__ returned truthy, suppress exception (EndExcept + skip re-raise)
        let suppress_patch = self.emit(Insn::JumpIfTrue(suppress_reg, 0));
        self.free_temp(exc_tmp);
        self.emit(Insn::RaiseReRaise);
        self.patch_jump(suppress_patch);
        self.emit(Insn::EndExcept);

        self.patch_jump(end_patch);
        self.free_temp(ctx_reg);
    }
}
