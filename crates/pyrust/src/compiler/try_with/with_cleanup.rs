// Shared normal-exit cleanup emission for sync and async context managers.
impl Compiler {
    /// Emit the no-exception `__exit__(None, None, None)` call for a sync
    /// `with` whose context manager is in `ctx_reg`.  Shared by the normal
    /// fall-through exit and the `break`/`continue`/`return` early-exit walk
    /// (`emit_early_exit_cleanups`), so the cleanup runs in both cases
    /// (issue #2295).  Does *not* emit `PopExcept`; the caller is responsible
    /// for popping the handler before invoking this.
    fn emit_with_normal_exit(&mut self, ctx_reg: Reg) {
        let exit_name_idx = self.intern_name("__exit__");
        let exit_frame = self.next_temp;
        if exit_frame.checked_add(4).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many registers for 'with' statement".to_string());
            }
            return;
        }
        self.next_temp = exit_frame + 4;
        if exit_frame + 3 > self.max_reg {
            self.max_reg = exit_frame + 3;
        }
        self.emit(Insn::GetAttrForWith(
            exit_frame,
            ctx_reg,
            exit_name_idx,
            1, // sync with: __exit__
        ));
        self.emit(Insn::LoadNone(exit_frame + 1));
        self.emit(Insn::LoadNone(exit_frame + 2));
        self.emit(Insn::LoadNone(exit_frame + 3));
        self.emit(Insn::Call(exit_frame, 3));
        self.next_temp = exit_frame;
    }

    /// Emit `await __aexit__(None, None, None)` (result discarded) for an
    /// `async with` whose context manager is in `ctx_reg`.  Shared by the
    /// normal fall-through exit and the early-exit walk (issue #2295).  Does
    /// *not* emit `PopExcept`; the caller pops the handler first.
    fn emit_async_with_normal_exit(&mut self, ctx_reg: Reg) {
        let aexit_name_idx = self.intern_name("__aexit__");
        let exit_frame = self.next_temp;
        if exit_frame.checked_add(4).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many registers for 'async with' statement".to_string());
            }
            return;
        }
        self.next_temp = exit_frame + 4;
        if exit_frame + 3 > self.max_reg {
            self.max_reg = exit_frame + 3;
        }
        self.emit(Insn::GetAttrForWith(
            exit_frame,
            ctx_reg,
            aexit_name_idx,
            4, // async with: __aexit__
        ));
        self.emit(Insn::LoadNone(exit_frame + 1));
        self.emit(Insn::LoadNone(exit_frame + 2));
        self.emit(Insn::LoadNone(exit_frame + 3));
        self.emit(Insn::Call(exit_frame, 3));
        // Drive the awaitable returned by __aexit__; result discarded.  Place
        // the result slot just above the call frame so the temp allocator
        // stays balanced.
        self.next_temp = exit_frame + 1;
        let exit_res = self.next_temp; // == exit_frame + 1
        self.next_temp = exit_frame + 2;
        if self.next_temp - 1 > self.max_reg {
            self.max_reg = self.next_temp - 1;
        }
        self.emit_await_drive_into(exit_frame, exit_res);
        self.next_temp = exit_frame;
    }
}
