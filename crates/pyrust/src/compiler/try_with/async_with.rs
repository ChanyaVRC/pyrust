// Asynchronous context-manager lowering.
impl Compiler {
    /// Compile an `async with` statement (issue #2279).
    ///
    /// Mirrors [`compile_with`] but drives the async context-manager protocol:
    /// `v = await mgr.__aenter__()` on entry and
    /// `await mgr.__aexit__(exc_type, exc, tb)` on exit, awaiting each coroutine
    /// to completion via the shared `GetAwaitable` + `YieldFrom` drive.  The
    /// suppression contract is identical: if `__aexit__` returns truthy while an
    /// exception is in flight, the exception is swallowed.
    ///
    /// `async with` is only legal inside an `async def`; the gate lives here so
    /// it fires even for a manager that never reaches the await (CPython reports
    /// the SyntaxError at compile time regardless).
    fn compile_async_with(
        &mut self,
        items: &[(Expr, Option<AssignTarget>)],
        body: &[Stmt],
        body_linenos: &[u32],
    ) {
        if items.is_empty() {
            self.compile_block_with_linenos(body, body_linenos);
            return;
        }
        if !self.is_async_function {
            self.set_syntax_error("'async with' outside async function");
            return;
        }
        let (expr, alias) = &items[0];
        let rest = &items[1..];

        // Capture the `async with` header line for the exception-unwind path
        // below, mirroring the sync `with` fix (issue #2419).
        let with_header_lineno = self.current_lineno;

        // mgr = expr
        let ctx_reg = self.compile_expr(expr);

        // v = await mgr.__aenter__()
        // GetAttrForWith maps a missing dunder to TypeError (#1656), matching the
        // async-context-manager protocol error CPython raises.
        let aenter_name_idx = self.intern_name("__aenter__");
        let aenter_reg = self.alloc_temp();
        self.emit(Insn::GetAttrForWith(
            aenter_reg,
            ctx_reg,
            aenter_name_idx,
            3, // async with: __aenter__
        ));
        self.emit(Insn::Call(aenter_reg, 0));
        // Drive the returned awaitable; result_reg holds __aenter__'s value.
        let entered_reg = self.alloc_temp();
        self.emit_await_drive_into(aenter_reg, entered_reg);
        self.free_temp(aenter_reg);

        // Bind alias if present.
        if let Some(tgt) = alias {
            self.compile_store_unpack_target(tgt, entered_reg);
        }
        self.free_temp(entered_reg);

        // SetupExcept for the body.
        let setup_patch = self.emit(Insn::SetupExcept(0));

        // Register the with-exit cleanup so a `break`/`continue`/`return` that
        // leaves the body awaits `__aexit__(None, None, None)` (issue #2295).
        self.except_cleanups.push(EarlyExitCleanup::WithBody {
            ctx_reg,
            is_async: true,
        });

        if rest.is_empty() {
            self.compile_block_with_linenos(body, body_linenos);
        } else {
            self.compile_async_with(rest, body, body_linenos);
        }
        self.except_cleanups.pop();
        if self.failed {
            return;
        }

        // Normal exit: await mgr.__aexit__(None, None, None); discard result.
        self.emit(Insn::PopExcept);
        let aexit_name_idx = self.intern_name("__aexit__");
        self.emit_async_with_normal_exit(ctx_reg);
        if self.failed {
            return;
        }
        let end_patch = self.emit(Insn::Jump(0));

        // Exception path: res = await mgr.__aexit__(type, exc, None).
        self.patch_jump(setup_patch);
        // Attribute the enclosing frame to the `async with` header line during
        // the unwind-path `__aexit__` call / re-raise (issue #2419).
        self.set_lineno(with_header_lineno);
        let exc_tmp = self.alloc_temp();
        self.emit(Insn::LoadExc(exc_tmp));
        let exit_frame2 = self.next_temp;
        if exit_frame2.checked_add(4).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many registers for 'async with' exception handler".to_string());
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
            aexit_name_idx,
            4, // async with: __aexit__
        ));
        self.emit(Insn::GetAttr(exit_frame2 + 1, exc_tmp, class_name_idx)); // exc_type
        self.emit(Insn::Move(exit_frame2 + 2, exc_tmp));
        // traceback: the real `__traceback__` of the in-flight exception (#2359),
        // materialised from its deferred placeholder.
        self.emit(Insn::LoadExcTraceback(exit_frame2 + 3, exc_tmp));
        self.emit(Insn::Call(exit_frame2, 3));
        // Drive the awaitable returned by __aexit__; its value decides
        // suppression.  The result goes to `suppress_reg` (the slot just above
        // the call frame); the await-drive scratch temps are allocated above it
        // and reclaimed, leaving `suppress_reg` live for the JumpIfTrue below.
        let suppress_reg = exit_frame2 + 4;
        self.next_temp = suppress_reg + 1;
        if suppress_reg > self.max_reg {
            self.max_reg = suppress_reg;
        }
        self.emit_await_drive_into(exit_frame2, suppress_reg);
        self.next_temp = suppress_reg + 1;
        // If __aexit__ returned truthy, suppress; otherwise re-raise.
        let suppress_patch = self.emit(Insn::JumpIfTrue(suppress_reg, 0));
        self.free_temp(exc_tmp);
        self.emit(Insn::RaiseReRaise);
        self.patch_jump(suppress_patch);
        self.emit(Insn::EndExcept);

        self.patch_jump(end_patch);
        self.free_temp(ctx_reg);
    }
}
