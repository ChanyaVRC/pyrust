// Asynchronous iterator-statement lowering.
impl Compiler {
    /// Compile an `async for` statement (issue #2279).
    ///
    /// Lowers to the asynchronous-iterator protocol: `it = aiter.__aiter__()`
    /// then a loop body that does `x = await type(it).__anext__(it)`, exiting on
    /// `StopAsyncIteration` (running the `else` clause on a clean exit).  The
    /// `await` reuses the shared `GetAwaitable` + `YieldFrom` drive.
    ///
    /// `async for` is only legal inside an `async def`.
    fn compile_async_for(
        &mut self,
        target: &AssignTarget,
        aiter_expr: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
    ) {
        if !self.is_async_function {
            self.set_syntax_error("'async for' outside async function");
            return;
        }

        // it = aiter.__aiter__()  (not awaited — __aiter__ returns the iterator
        // synchronously per PEP 492).
        let aiter_src = self.compile_expr(aiter_expr);
        let aiter_name_idx = self.intern_name("__aiter__");
        let it_reg = self.alloc_temp();
        self.emit(Insn::GetAttrForWith(it_reg, aiter_src, aiter_name_idx, 2)); // async for: __aiter__
        self.emit(Insn::Call(it_reg, 0));
        self.free_temp(aiter_src);

        // Pre-load the StopAsyncIteration type once, in a register that lives for
        // the whole loop (used by MatchExcept on every iteration's exit check).
        let stop_async_reg = self.compile_expr(&Expr::Var("StopAsyncIteration".to_string(), None));

        let loop_start = self.pc();
        // Each iteration runs `await it.__anext__()` inside a SetupExcept so a
        // StopAsyncIteration (raised by the coroutine) can break the loop.
        let setup_patch = self.emit(Insn::SetupExcept(0));
        let anext_name_idx = self.intern_name("__anext__");
        let anext_reg = self.alloc_temp();
        self.emit(Insn::GetAttrForWith(
            anext_reg,
            it_reg,
            anext_name_idx,
            5, // async for: __anext__
        ));
        self.emit(Insn::Call(anext_reg, 0));
        let item_reg = self.alloc_temp();
        self.emit_await_drive_into(anext_reg, item_reg);
        self.free_temp(anext_reg);
        // Item obtained successfully: leave the per-iteration handler.
        self.emit(Insn::PopExcept);

        // Assign the item to the loop target, then run the body.
        self.compile_store_unpack_target(target, item_reg);
        self.free_temp(item_reg);
        if self.failed {
            return;
        }

        self.loops.push(LoopCtx {
            break_patches: SmallVec::new(),
            continue_target: Some(loop_start),
            continue_patches: SmallVec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved_def_set = self.def_set;
        self.mark_target_def(target);
        self.compile_block_with_linenos(body, body_linenos);
        self.def_set = saved_def_set;
        if self.failed {
            return;
        }
        // Back-edge to the top of the loop.
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));

        // ── Loop-exit handler: reached when __anext__ raised. ──
        self.patch_jump(setup_patch);
        let exc_tmp = self.alloc_temp();
        self.emit(Insn::LoadExc(exc_tmp));
        // If it's NOT StopAsyncIteration, re-raise; otherwise fall through and
        // exit the loop normally (StopAsyncIteration is swallowed).
        let not_stop_patch = self.emit(Insn::MatchExcept(stop_async_reg, 0));
        self.emit(Insn::EndExcept);
        let exit_to_else = self.emit(Insn::Jump(0));
        self.patch_jump(not_stop_patch);
        self.emit(Insn::RaiseReRaise);
        self.patch_jump(exit_to_else);
        self.free_temp(exc_tmp);

        let ctx = self.loops.pop().unwrap();
        self.free_temp(stop_async_reg);
        self.free_temp(it_reg);

        // `else` runs on normal (StopAsyncIteration) exit, not after `break`.
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
    }
}
