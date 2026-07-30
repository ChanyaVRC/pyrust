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
        // The Python-visible type of this object, recorded outside the state
        // cell so `type()` / `repr()` / `dir()` can answer while the body is
        // running and the cell is checked out (#2978).  `async def` + `yield`
        // is an async generator; `async def` alone is a coroutine.
        let kind = match (is_coroutine, code.is_generator) {
            (true, true) => GeneratorKind::AsyncGenerator,
            (true, false) => GeneratorKind::Coroutine,
            (false, _) => GeneratorKind::Generator,
        };
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
            fn_name: std::sync::Arc::clone(&fn_name),
            is_coroutine,
        };
        // The frame keeps the name as its *compile-time* identity, behind
        // tracebacks and `co_name`; the cell owns the writable `__name__` /
        // `__qualname__` pair, which CPython likewise lets a user reassign
        // without disturbing the code object.
        Value::generator_frame(Box::new(frame), kind, fn_name, qualname)
    }
}
