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
}

include!("program_execution/script.rs");
include!("program_execution/source.rs");
include!("program_execution/dict_namespace.rs");
include!("program_execution/source_errors.rs");
