impl Interpreter {
    /// Construct the generator frame shared by all user-function binding paths.
    fn build_generator_value(
        code: &Rc<crate::bytecode::FnCode>,
        regs: RegsBuf,
        saved_env: EnvRef,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        fn_name: std::sync::Arc<str>,
        qualname: std::sync::Arc<str>,
    ) -> Value {
        let num_iters = code.num_iters as usize;
        let is_coroutine = code.is_coroutine;
        let frame = GeneratorFrame {
            code: Rc::clone(code),
            regs: regs.into_vec(),
            iters: smallvec![None; num_iters],
            exc_handlers: ExcHandlersBuf::new(),
            pc: 0,
            done: false,
            saved_env,
            handled_exc_slice: HandledExcBuf::new(),
            active_exception: None,
            exc_saved_active_slice: Vec::new(),
            local_index,
            yield_dst: 0,
            suspended_line: 0,
            last_return_value: None,
            fn_name,
            qualname,
            is_coroutine,
        };
        Value::generator(Box::new(frame))
    }
}
