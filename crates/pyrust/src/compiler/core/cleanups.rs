impl Compiler {
    /// Emit the complete PEP 3110 cleanup for one `except E as name` binding.
    ///
    /// `DeleteLocal` clears the fast-local register and, for an exposed class
    /// frame, the live namespace key. `RecordClassDel` updates the class-store
    /// order so a later rebind is appended in CPython order. At module scope
    /// `DeleteModuleGlobal` also removes the mirrored globals entry.
    fn emit_except_as_delete(&mut self, cleanup: Option<ExceptAsVarDel>) {
        match cleanup {
            Some(ExceptAsVarDel::Local {
                register,
                name_index,
                delete_module_global,
            }) => {
                self.emit(Insn::DeleteLocal(register, name_index, false));
                self.maybe_record_class_del(register);
                if delete_module_global {
                    self.emit(Insn::DeleteModuleGlobal(name_index));
                }
                if (register as usize) < 64 {
                    self.def_set &= !(1u64 << register);
                }
            }
            Some(ExceptAsVarDel::Cell(cell_idx)) => {
                self.emit(Insn::DeleteCell(cell_idx, false));
            }
            Some(ExceptAsVarDel::Name(name_index)) => {
                self.emit(Insn::DeleteName(name_index));
            }
            None => {}
        }
    }

    /// Emit cleanup instructions for a `raise` statement that exits an `except`
    /// handler body.  Unlike `emit_early_exit_cleanups` (which is for
    /// `break`/`continue`/`return`), `raise` does NOT emit `EndExcept` because
    /// the raise instruction itself manages `handled_exc_stack`:
    ///
    /// - `RaiseReRaise` explicitly pops `handled_exc_stack` before propagating.
    /// - `RaiseValue`/`RaiseFrom` don't pop, but `handle_vm_error` checks for
    ///   a duplicate top-of-stack entry and removes it automatically.
    ///
    /// So for `raise` we only need to: delete any `as VAR` binding (PEP 3110),
    /// then inline any `finally` stmts.  `TryBody` entries don't need compile-time
    /// cleanup — the VM's `exc_handlers` stack still covers them.
    /// `pending_exc_reg`: when a non-bare `raise X` is in progress, the
    /// register holding the to-be-raised exception value.  If `Some`, it is
    /// pushed onto `handled_exc_stack` before inlining the innermost
    /// ExceptBody's finally block, so that any raise inside the finally sees
    /// the correct implicit context (the to-be-raised exception, not the
    /// currently-handled one).  The push is undone by a `PopExcContext` after
    /// the finally block completes normally.
    fn emit_raise_cleanups(&mut self, pending_exc_reg: Option<Reg>) {
        let total = self.except_cleanups.len();
        // Track whether we have already processed the innermost ExceptBody.
        // The PushExcContext is only needed for that first one.
        let mut innermost_except_body_done = false;
        for i in (0..total).rev() {
            if self.failed {
                return;
            }
            let cleanup = self.except_cleanups[i].clone();
            match cleanup {
                EarlyExitCleanup::TryBody { .. } | EarlyExitCleanup::WithBody { .. } => {
                    // A TryBody/WithBody entry means this `raise` site is inside a
                    // try/with body whose SetupExcept is still live on the VM's
                    // exc_handlers stack.  The VM will dispatch the exception to
                    // that handler at runtime (for `with`, the exception-path
                    // `__exit__` call); no compile-time inlining is needed for
                    // this entry or any outer entries (also covered by their own
                    // SetupExcept).
                    return;
                }
                EarlyExitCleanup::ExceptBody {
                    finally_stmts,
                    as_var_delete,
                } => {
                    // PEP 3110: delete the `as VAR` binding (matches the normal
                    // handler exit path).  Also clear def_set so that any
                    // reference to the variable inside the inlined finally block
                    // correctly emits CheckLocal → UnboundLocalError, matching
                    // CPython's behaviour (the `as` binding is gone before the
                    // finally clause runs).
                    self.emit_except_as_delete(as_var_delete);
                    // Inline the finally block (if any) without EndExcept.
                    // The raise instruction propagates the exception; any further
                    // enclosing ExceptBody entries (outer except handlers whose
                    // SetupExcept was also popped) are processed by the remaining
                    // loop iterations.  TryBody entries at outer scopes still have
                    // live SetupExcept handlers and are handled by the VM.
                    if let Some(stmts) = finally_stmts {
                        // For the innermost ExceptBody: if we have a pending
                        // exception (non-bare raise), temporarily install it as
                        // the active context so that any raise inside the finally
                        // sees it as __context__ rather than the currently-handled
                        // exception on the stack.  This matches CPython, where the
                        // finally runs with the new exception as the active one.
                        let push_exc_ctx = !innermost_except_body_done && pending_exc_reg.is_some();
                        if let (true, Some(r)) = (push_exc_ctx, pending_exc_reg) {
                            self.emit(Insn::PushExcContext(r));
                        }
                        let saved_tail: Vec<EarlyExitCleanup> =
                            self.except_cleanups.drain(i..).collect();
                        self.compile_block(&stmts);
                        self.except_cleanups.extend(saved_tail);
                        if push_exc_ctx {
                            self.emit(Insn::PopExcContext);
                        }
                    }
                    innermost_except_body_done = true;
                    // Continue the loop: there may be enclosing ExceptBody entries
                    // (from outer except handlers) whose finallys also need inlining,
                    // because their outer SetupExcept was likewise popped when the
                    // outer handler was entered.
                }
            }
        }
    }

    /// Emit cleanup instructions for all `EarlyExitCleanup` entries in
    /// `self.except_cleanups[from_depth..]`, iterating innermost-first (i.e.
    /// from the top of the stack downward).
    ///
    /// Called before `break`, `continue`, or `return` to unwind any active
    /// `try`/`except` guards that the early exit crosses.
    ///
    /// While the inlined finally/handler-cleanup block for frame `i` is being
    /// compiled, `except_cleanups` is temporarily truncated to `[..i]` so that
    /// an early exit (e.g. `return`) inside that inlined block does not re-walk
    /// the frame we are currently unwinding — which would cause infinite
    /// recursion (see issue #365: `try: return X finally: return Y`).
    fn emit_early_exit_cleanups(&mut self, from_depth: usize) {
        let total = self.except_cleanups.len();
        if total <= from_depth {
            return;
        }
        // Walk from innermost (top) down to `from_depth`.
        for i in (from_depth..total).rev() {
            if self.failed {
                return;
            }
            // Clone the entry so we can mutate `self` while compiling the
            // inlined finally block.
            let cleanup = self.except_cleanups[i].clone();
            // Shadow the cleanup stack: any nested cleanup emission triggered
            // by `compile_block` below must not see frames `[i..]` (we are
            // already in the process of unwinding them).
            let saved_tail: Vec<EarlyExitCleanup> = self.except_cleanups.drain(i..).collect();
            match cleanup {
                EarlyExitCleanup::TryBody { finally_stmts } => {
                    self.emit(Insn::PopExcept);
                    if let Some(stmts) = finally_stmts {
                        self.compile_block(&stmts);
                    }
                }
                EarlyExitCleanup::ExceptBody {
                    finally_stmts,
                    as_var_delete,
                } => {
                    // PEP 3110: delete the `as VAR` binding before EndExcept,
                    // matching the normal (non-early-exit) handler exit path.
                    self.emit_except_as_delete(as_var_delete);
                    self.emit(Insn::EndExcept);
                    if let Some(stmts) = finally_stmts {
                        self.compile_block(&stmts);
                    }
                }
                EarlyExitCleanup::WithBody { ctx_reg, is_async } => {
                    // `with`/`async with` is `try: BODY finally: __exit__(...)`.
                    // Pop the body's handler, then run the no-exception exit
                    // (`__exit__(None, None, None)` / `await __aexit__(...)`)
                    // before the break/continue/return jump (issue #2295).
                    self.emit(Insn::PopExcept);
                    if is_async {
                        self.emit_async_with_normal_exit(ctx_reg);
                    } else {
                        self.emit_with_normal_exit(ctx_reg);
                    }
                }
            }
            // Restore the cleanup stack so the caller (and any sibling
            // iterations) see the original frames.  Unconditional restore
            // is safe because `compile_block` doesn't return errors — it
            // routes failures through `self.failed`, which the early-return
            // guard above catches on the next loop iteration.
            self.except_cleanups.extend(saved_tail);
        }
    }
}
