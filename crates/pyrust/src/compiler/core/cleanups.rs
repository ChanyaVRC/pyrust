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
    /// So for `raise` we only delete the `as VAR` binding (PEP 3110) of every
    /// except-handler body the exception leaves, innermost first.  The walk
    /// stops at the first `TryBody`/`WithBody` entry: its `SetupExcept` is live,
    /// so the VM dispatches the exception to that handler at runtime — this
    /// includes an enclosing `finally`, whose handler stays live across the
    /// `except` bodies of its own `try` statement (#3357).
    fn emit_raise_cleanups(&mut self) {
        for i in (0..self.except_cleanups.len()).rev() {
            match &self.except_cleanups[i] {
                EarlyExitCleanup::TryBody { .. } | EarlyExitCleanup::WithBody { .. } => return,
                EarlyExitCleanup::ExceptBody { as_var_delete } => {
                    // PEP 3110: delete the `as VAR` binding (matches the normal
                    // handler exit path).  This also clears `def_set`, so a
                    // reference to the variable inside the `finally` block that
                    // the exception reaches next emits `CheckLocal` and raises
                    // UnboundLocalError, as in CPython.
                    let as_var_delete = as_var_delete.clone();
                    self.emit_except_as_delete(as_var_delete);
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
                EarlyExitCleanup::ExceptBody { as_var_delete } => {
                    // PEP 3110: delete the `as VAR` binding before EndExcept,
                    // matching the normal (non-early-exit) handler exit path.
                    // An enclosing `finally` is unwound by the `TryBody` entry
                    // beneath this one (PopExcept, then the inlined block).
                    self.emit_except_as_delete(as_var_delete);
                    self.emit(Insn::EndExcept);
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
