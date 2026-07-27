impl Interpreter {
    /// Publish the running source line only for the frame-introspection
    /// built-in. Exact `sys` API recognition belongs to this domain rather
    /// than the opcode loop or generic method-call router.
    #[inline]
    pub(crate) fn publish_frame_line_for_builtin(callable: &Value, line: u32) {
        if matches!(
            callable.kind(),
            ValueKind::BuiltinFunction(name) if name.ends_with("_getframe")
        ) {
            pyrust_core::set_current_vm_line(line);
        }
    }

    /// Build a frame object at `idx` (zero is the innermost frame), including
    /// its `f_back` chain.
    pub(crate) fn build_frame_object(&self, idx: usize, lineno: i64) -> Value {
        let len = self.vm_frame_views.len();
        if idx >= len {
            return Value::none();
        }
        // `idx` counts from the top (innermost).  Translate to the Vec index.
        let view_index = len - 1 - idx;
        let view = &self.vm_frame_views[view_index];

        let code = match &view.function {
            Some(func) => self.build_code_object(func),
            None => {
                // Script / Class frame: synthesise a minimal code object whose
                // co_name matches CPython (`<module>` for module scope).  Carry
                // the running script's path into `co_filename` (#2438) so
                // `sys._getframe().f_code.co_filename` reports the source file
                // rather than `<unknown>`.
                let filename = self
                    .script_filename
                    .as_ref()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                code_obj::code_with_loc("<module>".to_string(), 0, Vec::new(), filename, 0)
            }
        };

        let back = self.build_frame_object(idx + 1, 0);
        let globals = view
            .env
            .as_ref()
            .map(|env| self.globals_for_environment(env))
            .unwrap_or_else(|| self.globals_for_environment(&self.env));
        // f_locals: a snapshot of this frame's namespace.  Reuse the same
        // machinery `locals()` uses for the innermost frame; for outer frames
        // fall back to an empty dict (a stale snapshot would be misleading).
        let locals = if idx == 0 {
            Value::dict(snapshot_current_locals(self))
        } else {
            Value::dict(Default::default())
        };

        frame_obj::frame(code, lineno, back, globals, locals)
    }

    /// Build the `gi_frame` object for a suspended generator, or `Value::none()`
    /// when the generator is exhausted (matching CPython, where `gi_frame`
    /// becomes `None` only after the generator finishes).  The frame's
    /// `f_lineno` is the source line the generator is suspended on (the line of
    /// the `yield` it last paused at); `f_code` is a code object carrying the
    /// generator function's `co_name` / `co_firstlineno` / `co_consts` etc.
    /// (issue #2185).
    pub(crate) fn build_generator_frame_object(&self, frame: &GeneratorFrame) -> Value {
        if frame.done {
            return Value::none();
        }
        // Current line: a suspended generator stores `pc` as the *resume* point
        // (the instruction after the `Yield`), whose line may already be the
        // next statement.  The line CPython reports is the `yield` it paused at,
        // which is at `pc - 1`.  Scan `[..pc]` (i.e. up to and including the
        // Yield) backward for the last entry that starts a new source line (a
        // `0` entry means "same line as the previous instruction").  When the
        // generator has not started yet (`pc == 0`), CPython reports the `def`
        // line (`first_lineno`).
        let lineno = if frame.pc == 0 {
            frame.code.first_lineno as i64
        } else {
            frame
                .code
                .lineno_table
                .iter()
                .take(frame.pc)
                .rev()
                .copied()
                .find(|&n| n != 0)
                .unwrap_or(frame.code.first_lineno) as i64
        };

        let code = self.build_code_from_fncode(
            &frame.code,
            frame.fn_name.as_ref(),
            frame.qualname.as_ref(),
            &frame.local_index,
        );
        frame_obj::frame(
            code,
            lineno,
            Value::none(),
            self.globals_for_environment(&frame.saved_env),
            Value::dict(Default::default()),
        )
    }

    /// Build a `code` object directly from a compiled `FnCode` (plus the name /
    /// qualname / local-name map), for callers that hold a `FnCode` but no
    /// `UserFunction` — currently generator `gi_frame` (issue #2185).  Populates
    /// the FnCode-derived attributes (`co_firstlineno`, `co_consts`, `co_names`,
    /// `co_cellvars`, `co_varnames` body locals, `co_stacksize`); the
    /// signature-derived counts (`co_argcount` etc.) are reported as 0 since the
    /// parameter list is not recoverable from the `FnCode` alone.
    fn build_code_from_fncode(
        &self,
        fncode: &crate::bytecode::FnCode,
        name: &str,
        qualname: &str,
        local_index: &std::collections::HashMap<String, crate::bytecode::Reg>,
    ) -> Value {
        let cellvar_set: std::collections::HashSet<&str> =
            fncode.cell_vars.iter().map(|s| s.as_str()).collect();
        // co_varnames: the body locals in register-slot order, excluding cell
        // variables.  (Parameters are also in `local_index`; without the
        // signature we cannot reorder them into CPython's posonly/kwonly groups,
        // so we report all locals in slot order — a best-effort that still lists
        // every name with the right membership.)
        let mut locals: Vec<(u32, &str)> = local_index
            .iter()
            .filter(|(n, _)| !cellvar_set.contains(n.as_str()))
            .map(|(n, &slot)| (slot, n.as_str()))
            .collect();
        locals.sort_by_key(|(slot, _)| *slot);
        let varnames: Vec<Value> = locals.iter().map(|(_, n)| Value::string(n)).collect();
        let nlocals = varnames.len() as i64;

        let mut cellvars: Vec<String> = fncode.cell_vars.iter().cloned().collect();
        cellvars.sort();
        let cellvars: Vec<Value> = cellvars.into_iter().map(Value::string).collect();

        let mut consts: Vec<Value> = Vec::new();
        consts.push(Value::none());
        consts.extend(fncode.consts.iter().filter(|c| !c.is_none()).cloned());
        let names: Vec<Value> = fncode
            .names
            .iter()
            .map(|n| Value::string(n.clone()))
            .collect();

        let mut flags = code_obj::CO_OPTIMIZED | code_obj::CO_NEWLOCALS;
        if fncode.is_generator {
            flags |= code_obj::CO_GENERATOR;
        }
        let filename = self
            .script_filename
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());

        code_obj::CodeBuild {
            name: name.to_string(),
            qualname: qualname.to_string(),
            argcount: 0,
            posonlyargcount: 0,
            kwonlyargcount: 0,
            nlocals,
            stacksize: fncode.num_regs as i64,
            varnames,
            flags,
            filename,
            firstlineno: fncode.first_lineno as i64,
            consts,
            names,
            freevars: Vec::new(),
            cellvars,
        }
        .build()
    }
}
