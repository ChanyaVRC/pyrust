// Source parsing and execution lifecycle boundary.

impl Interpreter {
    /// Register one Script register file with its root namespace.
    ///
    /// Every Script-frame constructor routes through this helper so explicit
    /// exec/eval, compiled code, nested source execution, and ordinary files
    /// share the same exposed-dictionary mutation semantics.
    fn register_script_namespace_mirror(
        &self,
        regs_ptr: std::ptr::NonNull<Value>,
        regs_len: usize,
        local_index: &Rc<HashMap<String, crate::bytecode::Reg>>,
    ) -> Option<pyrust_core::NamespaceMirrorGuard> {
        if local_index.is_empty() {
            return None;
        }
        // SAFETY: every caller creates `regs_ptr` from its local RegsBuf,
        // keeps that buffer stationary while RegSlice executes, and drops the
        // returned guard before popping the frame and dropping the buffer.
        Some(unsafe {
            self.env
                .borrow()
                .register_namespace_fastlocals(regs_ptr, regs_len, local_index)
        })
    }

    /// Spill a finished Script frame's fast-local registers into the module
    /// environment in binding order (issue #2903).
    ///
    /// `local_index` is a `HashMap`, so iterating it directly writes the names
    /// into `EnvValues` in a per-process randomised order — and `EnvValues` is
    /// insertion ordered, so that randomness becomes the Python-visible
    /// `list(globals())` order after the frame is gone. Registers are allocated
    /// from `collect_local_names` in source order, so walking the slots
    /// ascending restores the module's binding order, matching the ordered
    /// mirror walk that `namespace_materialization_snapshot` performs for a
    /// frame that is still live.
    fn write_back_script_locals(
        &mut self,
        local_index: &HashMap<String, crate::bytecode::Reg>,
        regs: &mut RegsBuf,
    ) {
        let mut by_slot: Vec<(crate::bytecode::Reg, &String)> = local_index
            .iter()
            .map(|(name, &slot)| (slot, name))
            .collect();
        // The name tie-breaker only matters for a layout that maps two names
        // onto one register; it keeps the result independent of the map's
        // iteration order in every case.
        by_slot.sort_unstable();
        for (idx, name) in by_slot {
            let idx = idx as usize;
            if !regs[idx].is_unset() {
                let val = std::mem::replace(&mut regs[idx], Value::unset());
                self.assign_name(name, val);
            }
        }
    }
}

include!("program_execution/script.rs");
include!("program_execution/source.rs");
include!("program_execution/dict_namespace.rs");
include!("program_execution/source_errors.rs");
