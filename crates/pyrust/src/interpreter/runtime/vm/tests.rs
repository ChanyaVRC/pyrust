#[cfg(test)]
mod vm_tests {
    use super::{RegSlice, Value};
    use crate::bytecode::{FnCode, Insn};
    use crate::interpreter::Interpreter;

    fn empty_code(insns: Vec<Insn>) -> FnCode {
        use crate::bytecode::{AttrCacheEntry, BinOpCacheEntry, KwCallCacheEntry};
        let n = insns.len();
        FnCode {
            insns,
            filename: std::sync::Arc::from("<unknown>"),
            lineno_table: vec![0u32; n],
            col_table: vec![(0, 0, 0, 0); n],
            first_lineno: 0,
            consts: vec![],
            names: vec![],
            num_regs: 0,
            num_iters: 0,
            num_locals: 0,
            fn_protos: vec![],
            cell_vars: smallvec::smallvec![],
            free_var_candidates: std::cell::OnceCell::new(),
            is_generator: false,
            is_coroutine: false,
            is_class_method: false,
            is_inlined_comp: false,
            comp_enclosing_locals: None,
            attr_cache: std::cell::RefCell::new(vec![AttrCacheEntry::Empty; n]),
            global_cache: std::cell::RefCell::new(Vec::new()),
            global_cache_interest_masks: Vec::new(),
            binop_cache: std::cell::RefCell::new(vec![BinOpCacheEntry::Empty; n]),
            kwcall_cache: std::cell::RefCell::new(vec![KwCallCacheEntry::Empty; n]),
            fmt_spec_cache: std::cell::RefCell::new(vec![
                crate::interpreter::FmtSpecCacheEntry::Empty;
                n
            ]),
            call_builtin_cache: std::cell::RefCell::new(vec![
                crate::interpreter::CallBuiltinCacheEntry::Empty;
                n
            ]),
            // Empty: these hand-built test fixtures run unoptimized, so the VM
            // uses the dynamic SetupExcept/PopExcept handler stack.
            exc_table: Vec::new(),
            has_exc_handlers: false,
        }
    }

    #[test]
    fn matchexcept_with_no_active_exception_returns_error() {
        // MatchExcept must error when no exception is active (compiler bug scenario).
        let mut code = empty_code(vec![]);
        code.num_regs = 1;
        code.insns.push(Insn::LoadNone(0)); // type_reg = None (placeholder)
        code.insns.push(Insn::MatchExcept(0, 1)); // no active_exception → error
        code.insns.push(Insn::ReturnNone);
        code.lineno_table.extend([0u32, 0, 0]);
        code.col_table.extend([(0u32, 0u32, 0u32, 0u32); 3]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![Value::unset(); 1];
        // SAFETY (test): regs is alive for the duration of run_bytecode;
        // no VmFrameView is active, so there is no concurrent access.
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(result.is_err(), "expected Err, got {:?}", result);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no active exception"),
            "error should mention no active exception"
        );
    }

    #[test]
    fn oob_pc_returns_error_not_none() {
        // Jump(100): new_pc = 1 + 100 = 101 > insns.len() (1) → error
        let code = empty_code(vec![Insn::Jump(100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(
            result.is_err(),
            "expected Err for OOB jump, got {:?}",
            result
        );
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn negative_jump_returns_error() {
        // Jump(-100): new_pc = 1 + (-100) = -99 → underflow error
        let code = empty_code(vec![Insn::Jump(-100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(
            result.is_err(),
            "expected Err for negative jump, got {:?}",
            result
        );
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn normal_fallthrough_returns_none() {
        let code = empty_code(vec![Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        assert_eq!(
            interp.run_bytecode(&code, regs_slice).unwrap(),
            Value::none()
        );
    }

    #[test]
    fn setup_except_negative_offset_returns_error() {
        // SetupExcept(-100): handler_pc = 1 + (-100) < 0 → error at push time
        let code = empty_code(vec![Insn::SetupExcept(-100), Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(
            result.is_err(),
            "expected Err for SetupExcept with OOB offset, got {:?}",
            result
        );
    }
}
