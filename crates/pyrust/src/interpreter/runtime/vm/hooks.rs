// Out-of-line cold VM hook dispatchers for `sys.settrace()`, `sys.setprofile()`,
// and `weakref` callback draining / dead-temporary scrubbing (issue #3190).
//
// Performance contract:
// - The hot VM loop tests a single byte (`self.vm_hooks_flags != 0`) packed
//   into existing `Interpreter` struct padding (0 extra bytes).
// - Every helper in this file is `#[cold] #[inline(never)]` so normal execution
//   with tracing, profiling, and weakrefs inactive pays zero measurable cost.

pub(crate) const VM_HOOK_TRACE: u8 = 1 << 0;
pub(crate) const VM_HOOK_PROFILE: u8 = 1 << 1;
pub(crate) const VM_HOOK_WEAKREF: u8 = 1 << 2;
pub(crate) const VM_HOOK_TRACEMALLOC: u8 = 1 << 3;

/// Classification of how a single instruction interacts with a candidate
/// temporary register `target_reg`.
enum RegAccess {
    /// Instruction reads `target_reg` (so it is live here).
    Reads,
    /// Instruction unconditionally overwrites `target_reg` without reading it
    /// first (kills liveness along this path).
    WritesOnly,
    /// Instruction does not access `target_reg`.
    None,
}

#[inline]
fn reg_in_range(target: u32, base: u32, count: u32) -> bool {
    count > 0 && target >= base && target < base.saturating_add(count)
}

fn kw_call_name_reads(name: &crate::bytecode::KwCallName, target: u32) -> bool {
    match name {
        crate::bytecode::KwCallName::Callee(r) => *r == target,
        crate::bytecode::KwCallName::Method { obj, .. } => *obj == target,
    }
}

/// Conservative register access classification for `target` at `insn`.
/// Returns `RegAccess::Reads` whenever `target` might be read before being
/// overwritten, guaranteeing dead-temp scrubbing is 100% sound.
fn insn_reg_access(
    insn: &crate::bytecode::Insn,
    code: &crate::bytecode::FnCode,
    target: u32,
) -> RegAccess {
    use crate::bytecode::Insn;
    match insn {
        Insn::LoadConst(dst, _)
        | Insn::LoadGlobal(dst, _)
        | Insn::LoadCell(dst, _)
        | Insn::LoadClassDeref(dst, _)
        | Insn::LoadNone(dst)
        | Insn::ImportModule(dst, _)
        | Insn::LoadExc(dst)
        | Insn::MakeTypeVar(dst, _, _) => {
            if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::LoadClassName(dst, local, _) => {
            if *local != crate::bytecode::NO_CLASS_LOCAL && *local == target {
                RegAccess::Reads
            } else if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::StoreGlobal(_, src)
        | Insn::StoreCell(_, src)
        | Insn::DeleteAttr(src, _)
        | Insn::JumpIfFalse(src, _)
        | Insn::JumpIfTrue(src, _)
        | Insn::CmpJumpIfFalseConst(src, _, _, _)
        | Insn::CmpJumpIfTrueConst(src, _, _, _)
        | Insn::JumpIfNotInt(src, _)
        | Insn::JumpIfNotBuiltinLen(src, _)
        | Insn::Return(src)
        | Insn::GetIter(_, src)
        | Insn::CheckLocal(src, _)
        | Insn::RaiseValue(src)
        | Insn::RaiseExceptStarResidual(src)
        | Insn::ImportStar(src)
        | Insn::MatchExcept(src, _)
        | Insn::PushExcContext(src)
        | Insn::PrintExpr(src)
        | Insn::RecordClassStore(src)
        | Insn::SyncModuleGlobal(src, _) => {
            if *src == target {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::RaiseAssert(msg) => {
            if *msg != crate::bytecode::NO_ASSERT_MSG && *msg == target {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::Move(dst, src)
        | Insn::BinOpConst(dst, src, _, _, _)
        | Insn::BinOpImm(dst, src, _, _, _)
        | Insn::UnaryOp(dst, _, src)
        | Insn::MatchSeqExcluded(dst, src)
        | Insn::MatchMapping(dst, src)
        | Insn::GetAttr(dst, src, _)
        | Insn::GetAttrForWith(dst, src, _, _)
        | Insn::LenSeqOrExit(dst, src, _)
        | Insn::BuildListReserve(dst, src)
        | Insn::FormatValue(dst, src)
        | Insn::ImportFromAttr(dst, src, _)
        | Insn::LoadExcTraceback(dst, src)
        | Insn::GetAwaitable(dst, src)
        | Insn::Yield { src, dst } => {
            if *src == target {
                RegAccess::Reads
            } else if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::BinOp(dst, a, _, b)
        | Insn::BinOpInPlace(dst, a, _, b)
        | Insn::GetItem(dst, a, b)
        | Insn::GetItemSeqIntOrExit(dst, a, b, _)
        | Insn::FormatValueSpec(dst, a, b)
        | Insn::MakeTypeAlias(dst, _, a, b) => {
            if *a == target || *b == target {
                RegAccess::Reads
            } else if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::SetAttr(a, _, b)
        | Insn::SetTypeVarAttr(a, _, b)
        | Insn::DeleteItem(a, b)
        | Insn::CmpJumpIfFalse(a, _, b, _)
        | Insn::CmpJumpIfTrue(a, _, b, _)
        | Insn::CountCmpJumpTrue(a, _, b, _, _)
        | Insn::CountCmpJumpFalse(a, _, b, _, _)
        | Insn::RaiseFrom(a, b)
        | Insn::SetAdd(a, b)
        | Insn::ListAppend(a, b)
        | Insn::ListExtend(a, b)
        | Insn::DictUpdate(a, b) => {
            if *a == target || *b == target {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::SetItem(a, b, c) => {
            if *a == target || *b == target || *c == target {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::GetSlice(dst, obj, base) => {
            if *obj == target || reg_in_range(target, *base, 3) {
                RegAccess::Reads
            } else if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::BuildSlice(dst, base) => {
            if reg_in_range(target, *base, 3) {
                RegAccess::Reads
            } else if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::BuildList(dst, base, n) | Insn::BuildTuple(dst, base, n) => {
            if reg_in_range(target, *base, *n) {
                RegAccess::Reads
            } else if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::BuildString(dst, base, n)
        | Insn::Concat {
            dst,
            base,
            count: n,
        } => {
            if reg_in_range(target, *base, u32::from(*n)) {
                RegAccess::Reads
            } else if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::BuildDict(dst, base, n, _) => {
            if reg_in_range(target, *base, n.saturating_mul(2)) {
                RegAccess::Reads
            } else if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::Unpack(dst_base, src, n) => {
            if *src == target {
                RegAccess::Reads
            } else if reg_in_range(target, *dst_base, *n) {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::UnpackEx {
            src,
            before,
            after,
            dst_base,
        } => {
            let total = u32::from(*before).saturating_add(1).saturating_add(*after);
            if *src == target {
                RegAccess::Reads
            } else if reg_in_range(target, *dst_base, total) {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::Call(func, argc) | Insn::CallMemo(func, argc) => {
            if reg_in_range(target, *func, 1 + u32::from(*argc)) {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::CallKw { func, total, .. } => {
            if reg_in_range(target, *func, 1 + u32::from(*total)) {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::CallMethod {
            dst,
            obj,
            args_base,
            nargs,
            ..
        } => {
            if *obj == target || reg_in_range(target, *args_base, u32::from(*nargs)) {
                RegAccess::Reads
            } else if *dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::CallInlineBinOp(desc_idx, _) => {
            let d = code.descs[*desc_idx as usize].as_call_inline_binop();
            if d.callee == target || d.a == target || d.b == target {
                RegAccess::Reads
            } else if d.dst == target {
                // Only conditionally writes (may fall through), so treat as None.
                RegAccess::None
            } else {
                RegAccess::None
            }
        }
        Insn::CallMethodExpanded(desc_idx) => {
            let d = code.descs[*desc_idx as usize].as_call_method_expanded();
            if d.obj == target || d.pos_list == target || d.kw_dict == target {
                RegAccess::Reads
            } else if d.dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::CallExArgs(desc_idx) => {
            let d = code.descs[*desc_idx as usize].as_call_ex_args();
            let pos_read = reg_in_range(target, d.func, 1 + u32::from(d.npos));
            let kw_base = d.func.saturating_add(1).saturating_add(u32::from(d.npos));
            let kw_read = reg_in_range(target, kw_base, u32::from(d.nkw));
            let splat_read =
                d.args_splat != crate::bytecode::NO_ARGS_SPLAT && d.args_splat == target;
            let kwargs_read = d.kwargs != crate::bytecode::NO_KWARGS && d.kwargs == target;
            if pos_read || kw_read || splat_read || kwargs_read {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::CallMethodKw(desc_idx) => {
            let d = code.descs[*desc_idx as usize].as_call_method_kw();
            if d.obj == target || reg_in_range(target, d.args_base, u32::from(d.total)) {
                RegAccess::Reads
            } else if d.dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::MatchClassPositional(desc_idx) => {
            let d = code.descs[*desc_idx as usize].as_match_class_positional();
            if d.subj == target || d.cls == target {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::MakeFunction(desc_idx) => {
            let d = code.descs[*desc_idx as usize].as_make_function();
            if reg_in_range(target, d.defs_base, d.defs_n)
                || reg_in_range(target, d.annots_base, d.annots_n)
            {
                RegAccess::Reads
            } else if d.dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::MatchExceptStar(desc_idx, _) => {
            let d = code.descs[*desc_idx as usize].as_match_except_star();
            if d.type_reg == target || d.src_group == target {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::MakeClass(desc_idx) => {
            let d = code.descs[*desc_idx as usize].as_make_class();
            if reg_in_range(target, d.bases_base, d.bases_n)
                || reg_in_range(target, d.kwarg_base, d.kwarg_n.saturating_mul(2))
                || (d.meta_reg != crate::bytecode::NO_METACLASS && d.meta_reg == target)
            {
                RegAccess::Reads
            } else if d.dst == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::ListExtendCall(desc_idx) => {
            let d = code.descs[*desc_idx as usize].as_list_extend_call();
            if d.list == target || d.src == target || kw_call_name_reads(&d.name, target) {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::DictMergeKwCall(desc_idx) => {
            let d = code.descs[*desc_idx as usize].as_dict_merge_kw_call();
            if d.dict == target || d.src == target || kw_call_name_reads(&d.name, target) {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::SetItemKwCall(desc_idx) => {
            let d = code.descs[*desc_idx as usize].as_set_item_kw_call();
            if d.dict == target
                || d.key == target
                || d.val == target
                || kw_call_name_reads(&d.name, target)
            {
                RegAccess::Reads
            } else {
                RegAccess::None
            }
        }
        Insn::YieldFrom {
            iter_reg,
            sent_reg,
            result_reg,
        } => {
            if *iter_reg == target || *sent_reg == target {
                RegAccess::Reads
            } else if *result_reg == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::ForIter(dst, _, _) => {
            if *dst == target {
                // Only writes on non-exhausted branch; conservative None.
                RegAccess::None
            } else {
                RegAccess::None
            }
        }
        Insn::DeleteLocal(reg, _, _) => {
            if *reg == target {
                RegAccess::WritesOnly
            } else {
                RegAccess::None
            }
        }
        Insn::DeleteCell(..)
        | Insn::DeleteName(_)
        | Insn::PushTypeParamEnv
        | Insn::PopTypeParamEnv
        | Insn::Jump(_)
        | Insn::JumpIfIterNotIntRange(..)
        | Insn::JumpIfIterNotIndexedSeq(..)
        | Insn::JumpIfIterNotIntRangeExact(..)
        | Insn::ReturnNone
        | Insn::RaiseReRaise
        | Insn::SetupExcept(_)
        | Insn::PopExcept
        | Insn::EndExcept
        | Insn::PopExcContext
        | Insn::RecordClassDel(_)
        | Insn::DeleteModuleGlobal(_) => RegAccess::None,
    }
}

/// Conservative forward reachability check: returns `true` if temporary register
/// `target_reg` may be read on any execution path starting from `start_pc`
/// before being overwritten.
#[cold]
#[inline(never)]
fn is_temp_reg_live_at(code: &crate::bytecode::FnCode, start_pc: usize, target_reg: u32) -> bool {
    use crate::bytecode::Insn;
    let len = code.insns.len();
    if start_pc >= len {
        return false;
    }
    let mut visited = vec![false; len];
    let mut queue = std::collections::VecDeque::new();
    visited[start_pc] = true;
    queue.push_back(start_pc);

    while let Some(pc) = queue.pop_front() {
        let insn = &code.insns[pc];
        match insn_reg_access(insn, code, target_reg) {
            RegAccess::Reads => return true,
            RegAccess::WritesOnly => continue,
            RegAccess::None => {}
        }

        let mut push_target = |next_pc: usize| {
            if next_pc < len && !visited[next_pc] {
                visited[next_pc] = true;
                queue.push_back(next_pc);
            }
        };

        // Also follow exception handler edge if this PC has one.
        if let Some(&handler) = code.exc_table.get(pc)
            && handler != crate::bytecode::EXC_NO_HANDLER
        {
            push_target(handler as usize);
        }

        let next = pc + 1;
        let jump_target = |offset: i32| -> Option<usize> {
            let tgt = next as i64 + i64::from(offset);
            (tgt >= 0 && (tgt as usize) < len).then_some(tgt as usize)
        };

        match insn {
            Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseAssert(_)
            | Insn::RaiseValue(_)
            | Insn::RaiseFrom(_, _)
            | Insn::RaiseReRaise
            | Insn::RaiseExceptStarResidual(_) => {}
            Insn::Jump(off) => {
                if let Some(t) = jump_target(*off) {
                    push_target(t);
                }
            }
            Insn::JumpIfFalse(_, off)
            | Insn::JumpIfTrue(_, off)
            | Insn::CmpJumpIfFalse(_, _, _, off)
            | Insn::CmpJumpIfTrue(_, _, _, off)
            | Insn::CmpJumpIfFalseConst(_, _, _, off)
            | Insn::CmpJumpIfTrueConst(_, _, _, off)
            | Insn::JumpIfNotInt(_, off)
            | Insn::CountCmpJumpTrue(_, _, _, _, off)
            | Insn::CountCmpJumpFalse(_, _, _, _, off)
            | Insn::JumpIfIterNotIntRange(_, off)
            | Insn::JumpIfIterNotIndexedSeq(_, off)
            | Insn::JumpIfIterNotIntRangeExact(_, off)
            | Insn::JumpIfNotBuiltinLen(_, off)
            | Insn::LenSeqOrExit(_, _, off)
            | Insn::CallInlineBinOp(_, off)
            | Insn::ForIter(_, _, off)
            | Insn::SetupExcept(off)
            | Insn::MatchExcept(_, off)
            | Insn::MatchExceptStar(_, off) => {
                push_target(next);
                if let Some(t) = jump_target(*off) {
                    push_target(t);
                }
            }
            Insn::GetItemSeqIntOrExit(_, _, _, off) => {
                push_target(next);
                if let Some(t) = jump_target(i32::from(*off)) {
                    push_target(t);
                }
            }
            _ => push_target(next),
        }
    }
    false
}

impl Interpreter {
    /// Recompute the single hot-loop hook bitmask from `sys_state` and the
    /// thread-local weakref counters.
    #[inline]
    pub(crate) fn sync_vm_hooks_flags(&mut self) {
        let mut flags = 0u8;
        {
            let state = self.sys_state.borrow();
            if !state.tracing_in_progress {
                if !state.trace_func.is_none() {
                    flags |= VM_HOOK_TRACE;
                }
                if !state.profile_func.is_none() {
                    flags |= VM_HOOK_PROFILE;
                }
            }
        }
        if pyrust_core::has_live_weakrefs() || pyrust_core::has_pending_weakref_callbacks() {
            flags |= VM_HOOK_WEAKREF;
        }
        if self.cold.tracemalloc_state.borrow().tracing {
            flags |= VM_HOOK_TRACEMALLOC;
        }
        self.vm_hooks_flags = flags;
    }

    /// Clear dead temporary registers (`>= num_locals`) that still hold a
    /// heap value (which may be a weak-referenced object or a wrapper/container
    /// such as a `BoundMethod` retaining one), then drain any resulting weakref
    /// callbacks.
    #[cold]
    #[inline(never)]
    pub(crate) fn vm_scrub_dead_weakref_temps(
        &mut self,
        code: &crate::bytecode::FnCode,
        pc: usize,
        regs_ptr: std::ptr::NonNull<Value>,
        regs_len: usize,
        num_locals: u32,
    ) {
        if pyrust_core::has_live_weakrefs() {
            let start = num_locals as usize;
            // SAFETY: `regs_ptr`/`regs_len` come directly from the active
            // `RegSlice` of the suspended VM loop frame; no aliasing `&mut [Value]`
            // exists concurrently.
            let regs = unsafe { std::slice::from_raw_parts_mut(regs_ptr.as_ptr(), regs_len) };
            for (idx, v) in regs.iter_mut().enumerate().skip(start) {
                if !v.is_unset()
                    && !matches!(
                        v.kind(),
                        ValueKind::None
                            | ValueKind::Bool(_)
                            | ValueKind::Int(_)
                            | ValueKind::Float(_)
                            | ValueKind::Str(_)
                    )
                    && !is_temp_reg_live_at(code, pc, idx as u32)
                {
                    *v = Value::unset();
                }
            }
        }
        if pyrust_core::has_pending_weakref_callbacks() {
            self.drain_pending_weakref_callbacks();
        }
        if !pyrust_core::has_live_weakrefs() && !pyrust_core::has_pending_weakref_callbacks() {
            self.vm_hooks_flags &= !VM_HOOK_WEAKREF;
        }
    }

    /// Drain and invoke all queued weakref death callbacks.
    ///
    /// Exceptions raised inside a weakref callback are not propagated; they are
    /// reported to stderr matching CPython 3.12's unraisable-exception behavior.
    #[cold]
    #[inline(never)]
    pub(crate) fn drain_pending_weakref_callbacks(&mut self) {
        // Loop because invoking one callback may drop additional objects and
        // enqueue further weakref callbacks.
        while pyrust_core::has_pending_weakref_callbacks() {
            let pending = pyrust_core::take_pending_weakref_callbacks();
            for (wrapper, callback) in pending {
                let arg = [ExpandedCallArg {
                    name: None,
                    value: wrapper,
                }];
                if let Err(err) = self.call_function_expanded(callback, &arg) {
                    let msg = match err.kind() {
                        crate::error::PyErrorKind::Raised(v) => format_single_exc_line(self, v),
                        _ => format!("{err}"),
                    };
                    eprintln!("Exception ignored in weakref callback: {msg}");
                }
            }
        }
        self.sync_vm_hooks_flags();
    }

    /// Invoke a trace or profile callback under the CPython re-entrancy guard.
    #[cold]
    #[inline(never)]
    fn invoke_hook_callback(
        &mut self,
        hook: Value,
        frame: Value,
        event: &'static str,
        arg: Value,
        is_trace: bool,
    ) -> Result<Value> {
        let prev_in_progress = {
            let mut state = self.sys_state.borrow_mut();
            let prev = state.tracing_in_progress;
            state.tracing_in_progress = true;
            prev
        };
        self.sync_vm_hooks_flags();

        let call_args = [
            ExpandedCallArg {
                name: None,
                value: frame,
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(event),
            },
            ExpandedCallArg {
                name: None,
                value: arg,
            },
        ];
        let result = self.call_function_expanded(hook, &call_args);

        {
            let mut state = self.sys_state.borrow_mut();
            state.tracing_in_progress = prev_in_progress;
            if result.is_err() {
                // CPython 3.12: an exception escaping a trace or profile
                // function immediately clears that hook.
                if is_trace {
                    state.trace_func = Value::none();
                } else {
                    state.profile_func = Value::none();
                }
            }
        }
        self.sync_vm_hooks_flags();
        result
    }

    /// Out-of-line per-step hook dispatcher called at the top of the VM loop
    /// only when `self.vm_hooks_flags != 0`.
    #[cold]
    #[inline(never)]
    pub(crate) fn vm_dispatch_step_hooks(
        &mut self,
        code: &crate::bytecode::FnCode,
        pc: usize,
        regs_ptr: std::ptr::NonNull<Value>,
        regs_len: usize,
        num_locals: u32,
    ) -> Result<()> {
        if self.vm_hooks_flags & VM_HOOK_WEAKREF != 0 {
            self.vm_scrub_dead_weakref_temps(code, pc, regs_ptr, regs_len, num_locals);
        }

        if self.vm_hooks_flags & VM_HOOK_TRACE != 0 && !code.is_inlined_comp {
            let cur_line = code.line_at(pc);
            if cur_line != 0 {
                self.vm_emit_line_trace(cur_line, pc)?;
            }
        }
        if self.vm_hooks_flags & VM_HOOK_TRACEMALLOC != 0 {
            self.vm_tracemalloc_step(code, pc, regs_ptr, regs_len);
        }
        Ok(())
    }

    /// Cold per-step allocation sampler for `_tracemalloc` / `tracemalloc`.
    /// Runs only while `tracemalloc.is_tracing()` is true; inspects live
    /// registers in the current frame for newly created heap objects and
    /// records their approximate size and Python stack traceback.
    #[cold]
    #[inline(never)]
    fn vm_tracemalloc_step(
        &mut self,
        code: &crate::bytecode::FnCode,
        pc: usize,
        regs_ptr: std::ptr::NonNull<Value>,
        regs_len: usize,
    ) {
        let Ok(mut tm) = self.cold.tracemalloc_state.try_borrow_mut() else {
            return;
        };
        if !tm.tracing {
            return;
        }
        let limit = tm.traceback_limit.max(1);
        // SAFETY: `regs_ptr`/`regs_len` come directly from the active
        // `RegSlice` of the suspended VM loop frame.
        let regs = unsafe { std::slice::from_raw_parts(regs_ptr.as_ptr(), regs_len) };

        let mut cached_tb: Option<(Vec<(String, i64)>, usize)> = None;
        let line_pc = pc.saturating_sub(1);
        let cur_line = {
            let l = code.line_at(line_pc);
            if l != 0 {
                l as i64
            } else {
                let l2 = code.line_at(pc);
                if l2 != 0 {
                    l2 as i64
                } else {
                    code.first_lineno as i64
                }
            }
        };

        for val in regs {
            if val.is_unset() {
                continue;
            }
            let Some(approx_size) = tracemalloc_heap_size(val) else {
                continue;
            };
            let oid = val.object_id().as_int().unwrap_or(0);
            if oid == 0 || tm.ignored_objects.contains(&oid) {
                continue;
            }
            if let Some(existing) = tm.tracked_objects.get_mut(&oid) {
                let old_size = existing.size;
                if approx_size != old_size {
                    existing.size = approx_size;
                    if approx_size > old_size {
                        tm.current_memory =
                            tm.current_memory.saturating_add(approx_size - old_size);
                    } else {
                        tm.current_memory =
                            tm.current_memory.saturating_sub(old_size - approx_size);
                    }
                    if tm.current_memory > tm.peak_memory {
                        tm.peak_memory = tm.current_memory;
                    }
                }
                continue;
            }
            let (frames, total_nframe) = cached_tb
                .get_or_insert_with(|| self.capture_tracemalloc_frames(code, cur_line, limit));
            tm.tracked_objects.insert(
                oid,
                crate::interpreter::TracemallocRecord {
                    domain: 0,
                    size: approx_size,
                    frames: frames.clone(),
                    total_nframe: *total_nframe,
                },
            );
            tm.current_memory = tm.current_memory.saturating_add(approx_size);
            if tm.current_memory > tm.peak_memory {
                tm.peak_memory = tm.current_memory;
            }
        }
    }

    /// Capture `(frames, total_nframe)` for the current VM stack up to `limit`
    /// frames, most recent frame first (matching CPython's `tracemalloc`).
    #[cold]
    #[inline(never)]
    pub(crate) fn capture_tracemalloc_frames(
        &self,
        current_code: &crate::bytecode::FnCode,
        cur_line: i64,
        limit: usize,
    ) -> (Vec<(String, i64)>, usize) {
        let mut all_frames: Vec<(String, i64)> = Vec::new();
        let top_file = if !current_code.filename.is_empty() {
            current_code.filename.to_string()
        } else {
            self.cold
                .script_filename
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "<unknown>".to_string())
        };
        all_frames.push((top_file, cur_line.max(1)));

        // Walk outer visible frames starting at depth 1 (depth 0 is `current_code`).
        let mut depth = 1usize;
        while let Some(view_idx) = self.visible_view_index_for_depth(depth) {
            let view = &self.vm_frame_views[view_idx];
            let (filename, lineno) = self.tracemalloc_view_location(view_idx, view);
            all_frames.push((filename, lineno.max(1)));
            depth += 1;
        }
        let total = all_frames.len();
        all_frames.truncate(limit.max(1));
        (all_frames, total)
    }

    fn tracemalloc_view_location(
        &self,
        view_idx: usize,
        view: &crate::interpreter::VmFrameView,
    ) -> (String, i64) {
        let filename = if let Some(func) = view.function() {
            func.precompiled_code
                .as_ref()
                .and_then(|rc| rc.downcast_ref::<crate::bytecode::FnCode>())
                .map(|c| c.filename.to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    self.cold
                        .script_filename
                        .as_ref()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "<unknown>".to_string())
                })
        } else if let Some(gen_ptr) = view.gen_frame() {
            // SAFETY: active view on `vm_frame_views`.
            let gf = unsafe { gen_ptr.as_ref() };
            if !gf.code.filename.is_empty() {
                gf.code.filename.to_string()
            } else {
                "<unknown>".to_string()
            }
        } else {
            self.cold
                .script_filename
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "<unknown>".to_string())
        };
        let lineno = self.resolve_view_lineno(view_idx, false, 0);
        (filename, lineno)
    }
}

/// Approximate heap allocation size in bytes for `tracemalloc` tracking.
/// Returns `None` for immediates (`None`, `bool`, small `int`, `float`).
fn tracemalloc_heap_size(val: &Value) -> Option<usize> {
    match val.kind() {
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::Float(_)
        | ValueKind::NotImplemented
        | ValueKind::Ellipsis
        | ValueKind::BuiltinFunction(_) => None,
        ValueKind::BigInt(bi) => Some(32 + bi.bits() as usize / 8),
        ValueKind::Str(s) => {
            if s.len() <= 5 {
                None
            } else {
                Some(49 + s.len())
            }
        }
        ValueKind::Bytes(b) => Some(33 + b.len()),
        ValueKind::List(items) => Some(56 + items.len() * 8),
        ValueKind::Tuple(items) => {
            if items.is_empty() {
                None
            } else {
                Some(40 + items.len() * 8)
            }
        }
        ValueKind::Dict(d) => Some(232 + d.len() * 24),
        ValueKind::Set(s) => Some(216 + s.len() * 16),
        ValueKind::Complex(_, _) => Some(32),
        ValueKind::PyInstance(inst) => Some(64 + inst.borrow().attrs.len() * 24),
        ValueKind::BuiltinObject { ops, state } => {
            let base = 56usize;
            let extra = ops.len(state).unwrap_or(0);
            Some(base + extra)
        }
        ValueKind::UserFunction(_) | ValueKind::PyClass(_) | ValueKind::Generator(_) => Some(128),
        _ => None,
    }
}

impl Interpreter {
    /// Emit a `'line'` trace event for the innermost active frame if it has a
    /// non-`None` `f_trace` and `f_trace_lines == true` and the line changed
    /// (or jumped backward in a loop).
    #[cold]
    #[inline(never)]
    fn vm_emit_line_trace(&mut self, cur_line: u32, pc: usize) -> Result<()> {
        let Some(view_index) = self.visible_view_index_for_depth(0) else {
            return Ok(());
        };
        let Some((frame, local_trace)) = self.prepare_line_trace_frame(view_index, cur_line, pc)
        else {
            return Ok(());
        };
        pyrust_core::set_current_vm_line(cur_line);
        let ret =
            self.invoke_hook_callback(local_trace, frame.clone(), "line", Value::none(), true)?;
        // CPython 3.12: a non-None return value from a local trace function
        // replaces `frame.f_trace`; returning None preserves the existing one.
        if !ret.is_none() {
            self.set_frame_trace_for_vm(&frame, None, ret);
        }
        Ok(())
    }

    /// Emit `'call'` trace and/or profile hooks on Python function or generator
    /// frame entry.
    #[cold]
    #[inline(never)]
    pub(crate) fn vm_hook_function_call(&mut self, code: &crate::bytecode::FnCode) -> Result<()> {
        if code.is_inlined_comp {
            return Ok(());
        }
        let Some(view_index) = self.visible_view_index_for_depth(0) else {
            return Ok(());
        };
        let first_line = if code.first_lineno != 0 {
            code.first_lineno as i64
        } else {
            code.line_at(0) as i64
        };
        let frame = self.build_frame_object_at_view_with_code(view_index, first_line, Some(code));

        if self.vm_hooks_flags & VM_HOOK_TRACE != 0 {
            let global_trace = self.sys_state.borrow().trace_func.clone();
            if !global_trace.is_none() {
                let local_trace = self.invoke_hook_callback(
                    global_trace,
                    frame.clone(),
                    "call",
                    Value::none(),
                    true,
                )?;
                self.set_frame_trace_for_vm(&frame, Some(view_index), local_trace);
            }
        }

        if self.vm_hooks_flags & VM_HOOK_PROFILE != 0 {
            let profile_func = self.sys_state.borrow().profile_func.clone();
            if !profile_func.is_none() {
                self.invoke_hook_callback(profile_func, frame, "call", Value::none(), false)?;
            }
        }
        Ok(())
    }

    /// Emit `'return'` trace and/or profile hooks on Python function or
    /// generator frame exit (`ret_val` is `None` when unwinding on exception).
    #[cold]
    #[inline(never)]
    pub(crate) fn vm_hook_function_return(
        &mut self,
        code: &crate::bytecode::FnCode,
        ret_val: Option<&Value>,
    ) -> Result<()> {
        if code.is_inlined_comp {
            return Ok(());
        }
        let Some(view_index) = self.visible_view_index_for_depth(0) else {
            return Ok(());
        };
        let arg = ret_val.cloned().unwrap_or_else(Value::none);

        if self.vm_hooks_flags & VM_HOOK_TRACE != 0
            && let Some((frame, local_trace)) = self.active_frame_local_trace(view_index)
        {
            let ret =
                self.invoke_hook_callback(local_trace, frame.clone(), "return", arg.clone(), true)?;
            if !ret.is_none() {
                self.set_frame_trace_for_vm(&frame, None, ret);
            }
        }

        if self.vm_hooks_flags & VM_HOOK_PROFILE != 0 {
            let profile_func = self.sys_state.borrow().profile_func.clone();
            if !profile_func.is_none() {
                let frame = self.build_frame_object_at_view(view_index, 0);
                self.invoke_hook_callback(profile_func, frame, "return", arg, false)?;
            }
        }
        Ok(())
    }

    /// Emit `'exception'` trace event when an exception is raised or propagates
    /// into a traced frame.
    #[cold]
    #[inline(never)]
    pub(crate) fn vm_hook_exception(&mut self, err: &PyError, raise_line: u32) -> Result<()> {
        let Some(view_index) = self.visible_view_index_for_depth(0) else {
            return Ok(());
        };
        let Some((frame, local_trace, exc_tuple)) =
            self.prepare_exception_trace_event(view_index, err, raise_line)?
        else {
            return Ok(());
        };
        let ret =
            self.invoke_hook_callback(local_trace, frame.clone(), "exception", exc_tuple, true)?;
        if !ret.is_none() {
            self.set_frame_trace_for_vm(&frame, None, ret);
        }
        Ok(())
    }

    /// Execute a compiled script/`exec`/`eval` code object with `'call'` and
    /// `'return'` trace/profile hooks when active (zero overhead when hooks
    /// are inactive).
    pub(crate) fn run_script_bytecode_with_hooks(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs_slice: RegSlice,
        cells_slice: CellSlice,
    ) -> Result<Value> {
        let call_hook_res = if self.vm_hooks_flags & (VM_HOOK_TRACE | VM_HOOK_PROFILE) != 0 {
            self.vm_hook_function_call(code)
        } else {
            Ok(())
        };
        let vm_result = match call_hook_res {
            Ok(()) => self.run_bytecode_with_cells(code, regs_slice, cells_slice),
            Err(e) => Err(e),
        };
        if self.vm_hooks_flags & (VM_HOOK_TRACE | VM_HOOK_PROFILE) != 0 {
            match self.vm_hook_function_return(code, vm_result.as_ref().ok()) {
                Ok(()) => vm_result,
                Err(e) => Err(e),
            }
        } else {
            vm_result
        }
    }

    /// Emit `'c_call'` profile event before invoking a built-in function.
    #[cold]
    #[inline(never)]
    pub(crate) fn vm_hook_c_call(&mut self, builtin_fn: &Value) -> Result<()> {
        if self.vm_hooks_flags & VM_HOOK_PROFILE == 0 {
            return Ok(());
        }
        let profile_func = self.sys_state.borrow().profile_func.clone();
        if profile_func.is_none() {
            return Ok(());
        }
        let frame = self.build_frame_object(0, 0);
        self.invoke_hook_callback(profile_func, frame, "c_call", builtin_fn.clone(), false)?;
        Ok(())
    }

    /// Emit `'c_return'` or `'c_exception'` profile event after invoking a
    /// built-in function.
    #[cold]
    #[inline(never)]
    pub(crate) fn vm_hook_c_return_or_exc(
        &mut self,
        builtin_fn: &Value,
        succeeded: bool,
    ) -> Result<()> {
        if self.vm_hooks_flags & VM_HOOK_PROFILE == 0 {
            return Ok(());
        }
        let profile_func = self.sys_state.borrow().profile_func.clone();
        if profile_func.is_none() {
            return Ok(());
        }
        let frame = self.build_frame_object(0, 0);
        let event = if succeeded { "c_return" } else { "c_exception" };
        self.invoke_hook_callback(profile_func, frame, event, builtin_fn.clone(), false)?;
        Ok(())
    }
}
