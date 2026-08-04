impl Interpreter {
    /// Run compiled statements in an explicit dict-based namespace.
    /// Used by `exec(code, globals_dict, locals_dict)`.
    fn exec_in_dict_env(
        &mut self,
        program: &[crate::ast::Stmt],
        linenos: &[u32],
        globals_dict: Value,
        locals_dict: Option<Value>,
    ) -> Result<()> {
        use std::collections::HashSet;
        let empty: HashSet<String> = HashSet::new();
        let global_names = crate::interpreter::collect_global_names(program);
        let local_names =
            crate::interpreter::collect_local_names(&[], program, &global_names, &empty);
        // Explicit exec(globals, locals) must retain the distinction between a
        // normal module-code assignment (StoreFast -> locals) and a declared
        // global assignment (StoreGlobal -> globals).  The ordinary script
        // runner may switch to all-env mode above 200 names, but doing that
        // here would collapse both writes into StoreGlobal and corrupt the
        // caller's separate namespace dictionaries.
        let local_index: Rc<HashMap<String, crate::bytecode::Reg>> = Rc::new(
            (0u32..)
                .zip(local_names.iter())
                .map(|(i, n)| (n.clone(), i))
                .collect(),
        );
        // Thread the lexer line table into the bytecode (issue #2245) so errors
        // inside the exec'd source report correct internal line numbers.
        let code = {
            let c = crate::compiler::compile_script_with_linenos(
                program,
                Rc::clone(&local_index),
                false,
                linenos,
                "<string>",
            )?;
            Rc::new(crate::optimizer::optimize(c))
        };

        // Build a fresh env pre-seeded with values from globals_dict (and
        // locals_dict if provided).  This env has no parent so `assign_name`
        // treats it as module scope.
        let exec_env = self.build_dict_env(globals_dict.clone(), locals_dict.as_ref())?;
        self.register_explicit_global_namespace(
            &exec_env,
            globals_dict.clone(),
            locals_dict.clone(),
        );
        let previous_env = std::mem::replace(&mut self.env, exec_env);

        let num_regs = code.num_regs as usize;
        let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
        Self::seed_explicit_exec_locals(
            &local_index,
            &mut regs,
            &globals_dict,
            locals_dict.as_ref(),
        );
        let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
        let regs_len = regs.len();
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Script,
            regs_ptr,
            regs_len,
            local_index: Rc::clone(&local_index),
            nonlocal_names: None,
            env: Some(Rc::clone(&self.env)),
            is_class_method: false,
            function: None,
            gen_frame: None,
        });
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        let namespace_mirror_guard =
            self.register_script_namespace_mirror(regs_ptr, regs_len, &local_index);
        let vm_result = self.run_bytecode(&code, regs_slice);
        drop(namespace_mirror_guard);
        self.pop_vm_frame_view();
        record_exec_string_frame(self, &vm_result, &code.filename);

        self.flush_explicit_exec_locals(&local_index, &regs, &globals_dict, locals_dict.as_ref());

        self.env = previous_env;

        vm_result.map(|_| ())
    }

    /// Run a compiled eval-mode code object in an explicit dict-based namespace.
    fn eval_in_dict_env(
        &mut self,
        code: &Rc<crate::bytecode::FnCode>,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        globals_dict: Value,
        locals_dict: Option<Value>,
    ) -> Result<Value> {
        let exec_env = self.build_dict_env(globals_dict.clone(), locals_dict.as_ref())?;
        self.register_explicit_global_namespace(
            &exec_env,
            globals_dict.clone(),
            locals_dict.clone(),
        );
        let previous_env = std::mem::replace(&mut self.env, exec_env);

        let num_regs = code.num_regs as usize;
        let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
        let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
        let regs_len = regs.len();
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Script,
            regs_ptr,
            regs_len,
            local_index: Rc::clone(&local_index),
            nonlocal_names: None,
            env: Some(Rc::clone(&self.env)),
            is_class_method: false,
            function: None,
            gen_frame: None,
        });
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        let namespace_mirror_guard =
            self.register_script_namespace_mirror(regs_ptr, regs_len, &local_index);
        let vm_result = self.run_bytecode(code, regs_slice);
        drop(namespace_mirror_guard);
        self.pop_vm_frame_view();
        record_exec_string_frame(self, &vm_result, &code.filename);

        self.env = previous_env;

        vm_result
    }

    /// Run a pre-compiled exec-mode code object.
    /// Used by `exec(compile(...))`.
    pub(crate) fn run_exec_code(
        &mut self,
        code: Rc<crate::bytecode::FnCode>,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        globals_dict: Option<Value>,
        locals_dict: Option<Value>,
    ) -> Result<()> {
        let _int_max_str_digits_guard = IntMaxStrDigitsExecutionGuard::enter(self);
        match globals_dict {
            None => {
                // Run in current module scope.
                let num_regs = code.num_regs as usize;
                let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
                let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
                let regs_len = regs.len();
                self.vm_frame_views.push(VmFrameView {
                    kind: FrameKind::Script,
                    regs_ptr,
                    regs_len,
                    local_index: Rc::clone(&local_index),
                    nonlocal_names: None,
                    env: Some(Rc::clone(&self.env)),
                    is_class_method: false,
                    function: None,
                    gen_frame: None,
                });
                let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
                let namespace_mirror_guard =
                    self.register_script_namespace_mirror(regs_ptr, regs_len, &local_index);
                let vm_result = self.run_bytecode(&code, regs_slice);
                drop(namespace_mirror_guard);
                self.pop_vm_frame_view();
                record_exec_string_frame(self, &vm_result, &code.filename);
                // Write fastlocals back to module env, in binding order.
                self.write_back_script_locals(&local_index, &mut regs);
                vm_result.map(|_| ())
            }
            Some(gdict) => {
                // Prepare the program slice: we already have compiled code,
                // but exec_in_dict_env wants &[Stmt].  Use the lower-level
                // dict env path directly.
                let num_regs = code.num_regs as usize;
                let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
                Self::seed_explicit_exec_locals(
                    &local_index,
                    &mut regs,
                    &gdict,
                    locals_dict.as_ref(),
                );
                let exec_env = self.build_dict_env(gdict.clone(), locals_dict.as_ref())?;
                self.register_explicit_global_namespace(
                    &exec_env,
                    gdict.clone(),
                    locals_dict.clone(),
                );
                let previous_env = std::mem::replace(&mut self.env, exec_env);

                let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
                let regs_len = regs.len();
                self.vm_frame_views.push(VmFrameView {
                    kind: FrameKind::Script,
                    regs_ptr,
                    regs_len,
                    local_index: Rc::clone(&local_index),
                    nonlocal_names: None,
                    env: Some(Rc::clone(&self.env)),
                    is_class_method: false,
                    function: None,
                    gen_frame: None,
                });
                let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
                let namespace_mirror_guard =
                    self.register_script_namespace_mirror(regs_ptr, regs_len, &local_index);
                let vm_result = self.run_bytecode(&code, regs_slice);
                drop(namespace_mirror_guard);
                self.pop_vm_frame_view();
                record_exec_string_frame(self, &vm_result, &code.filename);

                self.flush_explicit_exec_locals(&local_index, &regs, &gdict, locals_dict.as_ref());
                self.env = previous_env;
                vm_result.map(|_| ())
            }
        }
    }

    /// Run a pre-compiled eval-mode code object and return its value.
    /// Used by `eval(compile(...))`.
    pub(crate) fn run_eval_code_dispatch(
        &mut self,
        code: Rc<crate::bytecode::FnCode>,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        globals_dict: Option<Value>,
        locals_dict: Option<Value>,
    ) -> Result<Value> {
        let _int_max_str_digits_guard = IntMaxStrDigitsExecutionGuard::enter(self);
        match globals_dict {
            None => self.run_eval_code_in_module(&code, local_index),
            Some(gdict) => self.eval_in_dict_env(&code, local_index, gdict, locals_dict),
        }
    }

    /// Build an `EnvRef` pre-seeded with key/value pairs from the given
    /// globals dict (and locals dict, if provided).  The resulting env has
    /// no parent so the compile code treats it as module scope.
    fn build_dict_env(
        &mut self,
        globals_dict: Value,
        locals_dict: Option<&Value>,
    ) -> Result<EnvRef> {
        let exec_env = Environment::new(None);
        // Seed from globals first.
        let seeded = globals_dict.dict_with(|d| {
            for (k, v) in d {
                if let PyKey::Str(sv) = k
                    && let Some(s) = sv.as_str()
                {
                    let mut environment = exec_env.borrow_mut();
                    environment.record_namespace_env_binding(s);
                    environment.values.insert(s, v.clone());
                }
            }
        });
        if seeded.is_none() {
            return Err(pyrust_core::type_err!("exec/eval globals must be a dict"));
        }
        // Then overlay locals on top (locals shadow globals).
        if let Some(ldict) = locals_dict {
            let seeded_locals = ldict.dict_with(|d| {
                for (k, v) in d {
                    if let PyKey::Str(sv) = k
                        && let Some(s) = sv.as_str()
                    {
                        let mut environment = exec_env.borrow_mut();
                        environment.record_namespace_env_binding(s);
                        environment.values.insert(s, v.clone());
                    }
                }
            });
            if seeded_locals.is_none() {
                return Err(pyrust_core::type_err!("exec/eval locals must be a dict"));
            }
        }
        Ok(exec_env)
    }

    /// Commit only ordinary module-code assignments to the active locals
    /// mapping.  The register file itself is the write set: untouched names
    /// remain `unset`, so globals used only as read fallbacks are never copied
    /// into a separate locals dictionary.  Declared-global stores bypass these
    /// registers and are mirrored to `globals_dict` by `assign_name`.
    fn flush_explicit_exec_locals(
        &mut self,
        local_index: &HashMap<String, crate::bytecode::Reg>,
        regs: &[Value],
        globals_dict: &Value,
        locals_dict: Option<&Value>,
    ) {
        let write_target = locals_dict.unwrap_or(globals_dict);
        for (name, &idx) in local_index {
            let value = &regs[idx as usize];
            if value.is_unset() {
                continue;
            }
            {
                let mut environment = self.env.borrow_mut();
                environment.record_namespace_env_binding(name);
                environment.values.insert(name, value.clone());
            }
            let _ = write_target.dict_insert(PyKey::str_from(name), value.clone());
        }
    }

    /// Seed module-code fast locals from the actual locals provider.  This is
    /// required for operations such as `del existing_name`; a separate globals
    /// mapping is intentionally not consulted because deletion targets locals
    /// and must raise NameError when the name exists only in globals.
    fn seed_explicit_exec_locals(
        local_index: &HashMap<String, crate::bytecode::Reg>,
        regs: &mut [Value],
        globals_dict: &Value,
        locals_dict: Option<&Value>,
    ) {
        let read_source = locals_dict.unwrap_or(globals_dict);
        for (name, &idx) in local_index {
            if let Some(value) = read_source
                .dict_with(|dict| dict.get(&StrKey(name)).cloned())
                .flatten()
            {
                regs[idx as usize] = value;
            }
        }
    }
}
