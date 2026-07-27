impl Compiler {
    fn compile_raise(
        &mut self,
        expr: Option<&Expr>,
        cause: Option<&Expr>,
        // PEP 657 whole-statement caret anchor for `raise <expr>` (#2411).
        raise_span: Option<crate::ast::CaretSpan>,
    ) {
        // If we are inside an except handler body and that handler's enclosing
        // try/except/finally has a finally clause, the compiler already popped the
        // outer SetupExcept from the VM's exc_handlers stack (to avoid double-running
        // finally on exceptions from the handler body).  A `raise` statement exits the
        // handler body, so we must inline the finally block here before emitting the
        // raise instruction — the VM won't see the outer handler on exc_handlers.
        // True only when we're in an except-handler body that has a finally
        // clause to inline.  The finally clause is the only reason we need
        // `LoadExc` before the cleanup: without it, `RaiseReRaise` can rely
        // on `active_exception` directly.
        let in_except_body_with_finally = self.except_cleanups.iter().any(|c| {
            matches!(
                c,
                EarlyExitCleanup::ExceptBody {
                    finally_stmts: Some(_),
                    ..
                }
            )
        });

        // Compile the raise expressions BEFORE any cleanup, so that references
        // to `except ... as var` bindings resolve (e.g. `raise TypeError() from e`).
        //
        // For bare `raise` when inside an except handler body:
        //   `emit_raise_cleanups` inlines the finally block, which may contain
        //   a try/except that catches an exception.  If that inner exception
        //   matches the outer handler's context entry, `handle_vm_error`'s
        //   de-duplication logic removes it from `handled_exc_stack`, leaving
        //   `active_exception = None` by the time `RaiseReRaise` runs.
        //   Fix: save the current exception via `LoadExc` into a temp before
        //   the cleanup and re-raise it as `RaiseValue` (which doesn't rely on
        //   `active_exception` at the raise site).
        let bare_reraise_tmp: Option<Reg> = if expr.is_none() && in_except_body_with_finally {
            let tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(tmp));
            Some(tmp)
        } else {
            None
        };

        // Compile the raise expressions (for non-bare raise forms).
        //
        // `emit_raise_cleanups` will delete the `as VAR` binding (PEP 3110)
        // before inlining the finally block.  If the cause expression happens
        // to BE the deleted local variable register, we must copy its value
        // into a fresh temp *before* the deletion occurs.  We therefore call
        // `ensure_temp` on both `r` and `c` after evaluating them.
        let compiled = match expr {
            None => None,
            Some(e) => {
                let r = self.compile_expr(e);
                // Copy to temp if r is a fastlocal (ensure_temp = alloc+Move).
                let r = self.ensure_temp(r);
                let c = cause.map(|ce| {
                    let c = self.compile_expr(ce);
                    self.ensure_temp(c)
                });
                Some((r, c))
            }
        };
        if self.failed {
            if let Some(tmp) = bare_reraise_tmp {
                self.free_temp(tmp);
            }
            if let Some((r, c)) = compiled {
                if let Some(c) = c {
                    self.free_temp(c);
                }
                self.free_temp(r);
            }
            return;
        }

        // For a non-bare raise inside an except handler with a finally: pass the
        // register holding the to-be-raised exception so that emit_raise_cleanups
        // can temporarily install it as the active context before inlining the
        // finally block.  Bare raises don't need this because the active exception
        // (which is already on handled_exc_stack) is the one being re-raised.
        let pending_exc_reg = compiled.as_ref().map(|(r, _)| *r);
        self.emit_raise_cleanups(pending_exc_reg);
        if self.failed {
            if let Some(tmp) = bare_reraise_tmp {
                self.free_temp(tmp);
            }
            if let Some((r, c)) = compiled {
                if let Some(c) = c {
                    self.free_temp(c);
                }
                self.free_temp(r);
            }
            return;
        }
        // PEP 657 caret anchor (#2411): `raise <expr>` underlines the whole
        // raise statement (CPython behaviour).  The RaiseValue/RaiseFrom
        // instruction is what raises, so arm the statement span onto it; the
        // formatter omits it when it covers the whole dedented line (a bare
        // `raise name` at statement scope).
        match (compiled, bare_reraise_tmp) {
            // Bare `raise` inside an except handler body with a finally: use the
            // saved exception value so the re-raise is independent of
            // `active_exception`, which may have been cleared by the inlined
            // finally block's own exception handling.
            (None, Some(tmp)) => {
                self.emit(Insn::RaiseValue(tmp));
                self.free_temp(tmp);
            }
            // Bare `raise` outside any except body: rely on `active_exception`.
            (None, None) => {
                self.emit(Insn::RaiseReRaise);
            }
            (Some((r, Some(c))), _) => {
                self.set_col_span_for_next(raise_span);
                self.emit(Insn::RaiseFrom(r, c));
                self.free_temp(c);
                self.free_temp(r);
            }
            (Some((r, None)), _) => {
                self.set_col_span_for_next(raise_span);
                self.emit(Insn::RaiseValue(r));
                self.free_temp(r);
            }
        }
    }

    /// Emit code that fills three *contiguous* registers with the slice bounds
    /// `(lo, hi, step)` — each missing bound becomes `LoadNone`. Returns the base
    /// register; the bounds occupy `[base, base+3)`. Shared by `compile_slice_key`
    /// (which wraps them in a `slice` object via `BuildSlice`) and the rvalue
    /// `GetSlice` fast path (which reads them directly).
    fn compile_slice_bounds(
        &mut self,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Reg {
        // Allocate three *contiguous* slots upfront so that compaction moves cannot
        // alias a later slot with one that was just written. The previous approach
        // compiled each bound into whatever register compile_expr or alloc_temp
        // returned, then slid them into contiguous positions — but the slide could
        // overwrite a "step" register that had been allocated at the same position
        // as the upper slot, causing the step value to hold the upper bound instead
        // of None. (Repro: `a[:x]` where x is a local variable.)
        let lo_slot = self.alloc_temp(); // base
        let hi_slot = self.alloc_temp(); // base + 1
        let st_slot = self.alloc_temp(); // base + 2

        // Fill each slot: compile the expression into whatever register the
        // sub-expression naturally lands in, then Move it into the reserved slot
        // and release the source. If the expression already landed in the right
        // slot (e.g. a nested alloc_temp gave us exactly that register), skip the
        // move to avoid a redundant copy. When the bound is absent, emit LoadNone
        // directly into the slot — no temp needed.
        let fill_slot = |this: &mut Self, slot: Reg, expr: Option<&Expr>| {
            if let Some(e) = expr {
                let src = this.compile_expr(e);
                if src != slot {
                    this.emit(Insn::Move(slot, src));
                    this.free_temp(src);
                }
            } else {
                this.emit(Insn::LoadNone(slot));
            }
        };

        fill_slot(self, lo_slot, lower);
        fill_slot(self, hi_slot, upper);
        fill_slot(self, st_slot, step);
        lo_slot
    }

    /// Build the 3-element slice-key object `(lo, hi, step)` used by GetItem/SetItem/DeleteItem.
    /// Each missing bound is represented as `None`. Returns the register holding the slice.
    fn compile_slice_key(
        &mut self,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Reg {
        let lo_slot = self.compile_slice_bounds(lower, upper, step);
        // The three slots are already contiguous; BuildSlice reads [lo_slot .. lo_slot+3).
        // BuildSlice (not BuildTuple) so the VM can unambiguously distinguish a
        // compiler-generated slice key from a user 3-tuple (issue #931).
        let slice_r = self.alloc_temp();
        self.emit(Insn::BuildSlice(slice_r, lo_slot));
        // Release the three component slots — they are consumed by BuildSlice.
        self.next_temp = slice_r + 1;
        slice_r
    }

    fn compile_delete(&mut self, expr: &Expr) {
        match expr {
            Expr::Var(name, _) => {
                if let Some(reg) = self.local_reg(name) {
                    // Pass the name index so the VM can raise NameError /
                    // UnboundLocalError when the register was never assigned.
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::DeleteLocal(reg, name_idx));
                    // Clear the definitely-bound bit so that any subsequent
                    // read of this name emits CheckLocal and raises the correct
                    // exception (UnboundLocalError at function scope, NameError
                    // at module scope) rather than falling through to vm_read's
                    // generic "local variable referenced before assignment" path.
                    if (reg as usize) < 64 {
                        self.def_set &= !(1u64 << reg);
                    }
                    self.maybe_record_class_del(reg);
                    // Issue #820: at module scope, also remove the name from
                    // env.values and module_globals_dict so that LoadGlobal
                    // from nested functions / after globals() cannot resurrect it.
                    if self.is_module_scope {
                        self.emit(Insn::DeleteModuleGlobal(name_idx));
                    }
                } else {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::DeleteName(name_idx));
                }
            }
            Expr::Attr { target, name, .. } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                self.emit(Insn::DeleteAttr(obj, name_idx));
                self.free_temp(obj);
            }
            Expr::Index { target, index, .. } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                self.emit(Insn::DeleteItem(obj, idx));
                self.writeback_container_if_global(target, obj);
                self.free_temp(idx);
                self.free_temp(obj);
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                let obj = self.compile_expr(target);
                let slice_reg =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                self.emit(Insn::DeleteItem(obj, slice_reg));
                self.writeback_container_if_global(target, obj);
                self.free_temp(slice_reg);
                self.free_temp(obj);
            }
            _ => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("unsupported delete target".to_string());
                }
            }
        }
    }

    fn compile_import(&mut self, names: &[(String, Option<String>)]) {
        for (module_name, alias) in names {
            match alias {
                Some(alias) => {
                    // `import a.b.c as alias` — bind the leaf module under
                    // the alias directly.  No parent walk needed because
                    // the user explicitly renamed.
                    let mod_idx = self.intern_name(module_name);
                    let dst = self.alloc_temp();
                    self.emit(Insn::ImportModule(dst, mod_idx));
                    self.compile_store_name(alias, dst);
                    self.free_temp(dst);
                }
                None => {
                    // `import a.b.c` — CPython binds the *topmost* component
                    // (`a`), and `a.b.c` is reached via attribute chains on
                    // the loaded packages.
                    let top = module_name.split('.').next().unwrap_or(module_name);
                    if top == module_name {
                        // Non-dotted: one import that binds directly under
                        // the name — no parent walk involved.
                        let mod_idx = self.intern_name(module_name);
                        let dst = self.alloc_temp();
                        self.emit(Insn::ImportModule(dst, mod_idx));
                        self.compile_store_name(module_name, dst);
                        self.free_temp(dst);
                    } else {
                        // Dotted: first ensure the leaf is loaded (which
                        // populates the cache and lets the parent-package
                        // identity fix-up in `Interpreter::load_module`
                        // stitch its submodule attrs to the cached
                        // value); then load the topmost component and
                        // bind it.
                        let full_idx = self.intern_name(module_name);
                        let full_reg = self.alloc_temp();
                        self.emit(Insn::ImportModule(full_reg, full_idx));
                        self.free_temp(full_reg);
                        let top_idx = self.intern_name(top);
                        let top_reg = self.alloc_temp();
                        self.emit(Insn::ImportModule(top_reg, top_idx));
                        self.compile_store_name(top, top_reg);
                        self.free_temp(top_reg);
                    }
                }
            }
        }
    }

    fn compile_import_from(&mut self, module: &str, names: &[(String, Option<String>)]) {
        // `from __future__ import X` is a compiler directive in CPython — no
        // runtime import is performed.  Validate the feature name(s) and emit
        // nothing (no-op).  Unrecognised names or star-imports are SyntaxErrors
        // (matching CPython 3.12 behaviour).
        if module == "__future__" {
            // CPython 3.12: `from __future__` is only legal at the top of a
            // module — not inside functions, class bodies, or after any
            // non-__future__ statement (other than the module docstring).
            if !self.is_module_scope || self.past_future_zone {
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(
                        "from __future__ imports must occur at the beginning of the file"
                            .to_string(),
                    );
                }
                return;
            }
            const VALID: &[&str] = &[
                "nested_scopes",
                "generators",
                "division",
                "absolute_import",
                "with_statement",
                "print_function",
                "unicode_literals",
                "barry_as_FLUFL",
                "generator_stop",
                "annotations",
            ];
            for (name, _alias) in names {
                if !VALID.contains(&name.as_str()) {
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some(format!("future feature {} is not defined", name));
                    }
                    return;
                }
            }
            // All names are valid.  Activate compiler flags for directives
            // that affect code generation.
            if names.iter().any(|(n, _)| n == "annotations") {
                self.future_annotations = true;
            }
            // Fall through to the ordinary import-from bytecode path below.
            // CPython 3.12 also emits real import bytecode for
            // `from __future__ import X` (IMPORT_NAME followed by
            // IMPORT_FROM + STORE_NAME), so the feature name is bound in
            // the module namespace and `import __future__; __future__.X` also
            // works.  With a real `__future__` module stub in the registry
            // the emitted ImportModule / ImportFromAttr / StoreGlobal sequence
            // resolves correctly and the binding is visible at runtime.
        }

        let mod_idx = self.intern_name(module);
        let mod_reg = self.alloc_temp();
        self.emit(Insn::ImportModule(mod_reg, mod_idx));
        if names.len() == 1 && names[0].0 == "*" {
            // CPython: `from MOD import *` is only allowed at module level.
            if !self.is_module_scope {
                self.free_temp(mod_reg);
                self.set_syntax_error("import * only allowed at module level");
                return;
            }
            // Star import: emit ImportStar which iterates the module's __all__
            // (or all non-underscore attrs when __all__ is absent) and stores
            // each name into the current scope.
            self.emit(Insn::ImportStar(mod_reg));
        } else {
            for (attr_name, alias) in names {
                let attr_idx = self.intern_name(attr_name);
                let val_reg = self.alloc_temp();
                self.emit(Insn::ImportFromAttr(val_reg, mod_reg, attr_idx));
                let bound = alias.as_deref().unwrap_or(attr_name);
                self.compile_store_name(bound, val_reg);
                self.free_temp(val_reg);
            }
        }
        self.free_temp(mod_reg);
    }
}
