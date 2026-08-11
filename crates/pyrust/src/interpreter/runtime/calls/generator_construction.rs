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
            saved_env: Rc::clone(&saved_env),
            handled_exc_slice: HandledExcBuf::new(),
            active_exception: None,
            exc_saved_active_slice: Vec::new(),
            local_index,
            yield_dst: 0,
            suspended_line: 0,
            last_return_value: None,
            fn_name: std::sync::Arc::clone(&fn_name),
            is_coroutine,
            frame_cache: std::cell::RefCell::new(None),
        };
        // The frame keeps the name as its *compile-time* identity, behind
        // tracebacks and `co_name`; the cell owns the writable `__name__` /
        // `__qualname__` pair, which CPython likewise lets a user reassign
        // without disturbing the code object.
        let value = Value::generator_frame(Box::new(frame), kind, fn_name, qualname);
        if !code.cell_vars.is_empty() {
            // Every binding path allocates a fresh local env when the code owns
            // cells. No-cell generator calls may reuse `function.env`, so they
            // must never install this single-owner backpointer.
            let ValueKind::Generator(owner) = value.kind() else {
                unreachable!("generator frame constructor returned a non-generator value")
            };
            saved_env.borrow_mut().bind_generator_frame_owner(owner);
        }
        value
    }
}
