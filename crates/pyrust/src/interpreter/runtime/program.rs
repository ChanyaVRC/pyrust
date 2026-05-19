impl Interpreter {
    pub fn with_script_dir(dir: PathBuf) -> Self {
        Self { script_dir: Some(dir), ..Default::default() }
    }

    pub fn exec_program(&mut self, program: &[Stmt], repl_mode: bool) -> Result<()> {
        let result = if let Some(r) = self.try_exec_vm_script(program, repl_mode) {
            r
        } else {
            Err(PyError::Runtime("compilation failed".to_string()))
        };
        // Intercept SystemExit so finally/with handlers run before the process exits.
        if let Err(PyError::Raised(exc)) = &result
            && let ValueKind::PyInstance(inst) = exc.kind() {
                let class_name = inst.borrow().class.borrow().name.clone();
                if class_name == "SystemExit" {
                    // Extract exit code from args[0]; default to 0.
                    let code = match inst.borrow().attrs.get("args") {
                        Some(args_val) => match args_val.kind() {
                            ValueKind::Tuple(args) if !args.is_empty() => {
                                match args[0].kind() {
                                    ValueKind::Int(n) => n as i32,
                                    ValueKind::Bool(b) => b as i32,
                                    ValueKind::None => 0,
                                    _ => 1,
                                }
                            }
                            _ => 0,
                        },
                        None => 0,
                    };
                    std::process::exit(code);
                }
            }
        result
    }

    fn try_exec_vm_script(&mut self, program: &[Stmt], repl_mode: bool) -> Option<Result<()>> {
        // Build fastlocal registers for module-level names that are NOT captured
        // by nested functions (those become cell vars and use StoreGlobal/env).
        // This allows tight loops over plain variables to avoid HashMap overhead.
        let empty: HashSet<String> = HashSet::new();
        let local_names =
            crate::interpreter::collect_local_names(&[], program, &empty, &empty);
        // Cap script-level fastlocals so the register array stays small.
        // Scripts with more names than this fall back to all-env mode where names
        // live in a HashMap rather than a Vec<Option<Value>>.
        const MAX_SCRIPT_LOCALS: usize = 200;
        if local_names.len() > MAX_SCRIPT_LOCALS {
            // Too many locals — fall back to all-env mode.
            let local_index: Rc<HashMap<String, crate::bytecode::Reg>> = Rc::new(HashMap::new());
            return self.try_exec_vm_script_with_index(program, local_index, repl_mode);
        }
        let local_index: Rc<HashMap<String, crate::bytecode::Reg>> = Rc::new(
            (0u32..).zip(local_names.iter())
                .map(|(i, n)| (n.clone(), i))
                .collect(),
        );
        self.try_exec_vm_script_with_index(program, local_index, repl_mode)
    }

    fn try_exec_vm_script_with_index(
        &mut self,
        program: &[Stmt],
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        repl_mode: bool,
    ) -> Option<Result<()>> {
        let code = match crate::compiler::compile_script(program, Rc::clone(&local_index), repl_mode) {
            Ok(c) => Rc::new(crate::optimizer::optimize(c)),
            Err(e) => return Some(Err(e)),
        };
        let num_regs = code.num_regs as usize;
        let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
        let _depth_guard = CallDepthGuard::enter();
        // Issue #389: publish a view of the active script frame so
        // `globals()` / `locals()` can surface module-level fastlocal
        // names mid-execution (otherwise the regs only spill to
        // `env.values` AFTER this fn returns).  `vars()` doesn't yet
        // consult `vm_frame_views` — calling it at module scope only
        // sees `env.values`; unifying it with `snapshot_current_locals`
        // is a separate cleanup.  Push before `run_bytecode`, pop
        // afterwards so the raw pointer never outlives the local `regs`.
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Script,
            // SAFETY: SmallVec's inline storage / Vec allocation is always
            // non-null.  The pointer is valid for the lifetime of `regs` on
            // this stack frame; it is popped before `regs` is dropped.
            regs_ptr: unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) },
            regs_len: regs.len(),
            local_index: Rc::clone(&local_index),
            // Script frames have no enclosing function scope, so there
            // are no nonlocal bindings to resolve.
            nonlocal_names: None,
            env: None,
        });
        let vm_result = self.run_bytecode(&code, &mut regs);
        self.vm_frame_views.pop();
        // Write fastlocal registers back to the module env so that imported
        // modules and post-run inspection can find all names.
        // StoreGlobal from nested scopes already updated the register via
        // the script-frame view (#520), so the write-back naturally carries
        // the correct value.
        for (name, &idx) in local_index.iter() {
            if !regs[idx as usize].is_unset() {
                let val = std::mem::replace(&mut regs[idx as usize], Value::unset());
                self.assign_name(name.clone(), val);
            }
        }
        Some(vm_result.map(|_| ()))
    }

}
