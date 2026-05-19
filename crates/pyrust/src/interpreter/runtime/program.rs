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
        // Issue #706: always use all-env mode for script/module execution.
        //
        // Previously, pyrust allocated fastlocal registers for module-level names
        // as an optimization, but that made it impossible for `globals()` to return
        // a live dict: register writes bypassed `assign_name` entirely, so the
        // `module_globals_dict` (and `env.values`) were never updated mid-execution.
        //
        // With an empty `local_index`, the compiler emits `StoreGlobal` for every
        // module-level assignment.  `StoreGlobal` → `assign_name` → writes to both
        // `env.values` and `module_globals_dict`, keeping the globals dict live.
        // `LoadGlobal` checks `module_globals_dict` first, so `globals()["x"] = 99`
        // is visible as the global `x` immediately (CPython parity, issue #706).
        let local_index: Rc<HashMap<String, crate::bytecode::Reg>> = Rc::new(HashMap::new());
        self.try_exec_vm_script_with_index(program, local_index, repl_mode)
    }

    /// Seed the module-level global namespace with the standard dunder keys
    /// that CPython 3.12 always pre-populates for `__main__` (issue #675).
    ///
    /// Inserts only keys that are not already present, so a REPL second-pass
    /// or a future import path that has already seeded the namespace does not
    /// clobber any existing value.  User assignments made during script
    /// execution override the pre-seeded values naturally via `assign_name`,
    /// which keeps both `env.values` and `module_globals_dict` in sync.
    fn seed_module_dunders(&mut self) {
        // `__builtins__` is the builtins module object in `__main__`.
        let builtins_val = crate::builtin_modules::load_builtin_module("builtins")
            .unwrap_or_else(Value::none);

        let me_ref = module_env(&self.env);
        let mut me = me_ref.borrow_mut();
        // Insert each dunder only if absent — do not overwrite an existing binding.
        macro_rules! seed_env {
            ($name:literal, $val:expr) => {
                me.values.entry($name.to_string()).or_insert_with(|| $val)
            };
        }
        seed_env!("__name__", Value::string("__main__"));
        seed_env!("__doc__", Value::none());
        seed_env!("__package__", Value::none());
        seed_env!("__spec__", Value::none());
        seed_env!("__loader__", Value::none());
        seed_env!("__file__", Value::none());
        seed_env!("__cached__", Value::none());
        seed_env!("__annotations__", Value::dict(IndexMap::new()));
        seed_env!("__builtins__", builtins_val);
        // Mirror env values into module_globals_dict (issue #706: live globals dict).
        // Only insert keys not already present in the dict (REPL re-run safety).
        let dunders: &[&str] = &[
            "__name__", "__doc__", "__package__", "__spec__", "__loader__",
            "__file__", "__cached__", "__annotations__", "__builtins__",
        ];
        for name in dunders {
            let key = PyKey::Str((*name).to_string());
            let has_key = self
                .module_globals_dict
                .dict_with(|d| d.contains_key(&key))
                .unwrap_or(false);
            if !has_key {
                if let Some(v) = me.values.get(*name).cloned() {
                    let _ = self.module_globals_dict.dict_insert(key, v);
                }
            }
        }
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
        // Seed standard module-level dunders (issue #675), including __doc__
        // (issue #711).  Must happen after compilation (so `local_index` is
        // finalized) and before the script frame is pushed and `run_bytecode`
        // fires, so that the keys are visible in `module_env.values` from the
        // very first instruction.  User assignments override them naturally
        // (they either update the fastlocal register which shadows the env
        // entry, or call `assign_name` which overwrites the env entry directly).
        self.seed_module_dunders();
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
        // Capture the raw pointer and length BEFORE constructing RegSlice so
        // both the VmFrameView and the dispatch loop share the same raw pointer
        // with no &mut [Value] alive (eliminates noalias UB, issue #547).
        let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
        let regs_len = regs.len();
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Script,
            // SAFETY: SmallVec's inline storage / Vec allocation is always
            // non-null.  The pointer is valid for the lifetime of `regs` on
            // this stack frame; it is popped before `regs` is dropped.
            regs_ptr,
            regs_len,
            local_index: Rc::clone(&local_index),
            // Script frames have no enclosing function scope, so there
            // are no nonlocal bindings to resolve.
            nonlocal_names: None,
            env: None,
            is_class_method: false,
        });
        // SAFETY: regs_ptr is valid for regs_len Values for the lifetime of
        // `regs` (a local RegsBuf that outlives this call).  No &mut [Value]
        // referencing `regs` is held while the dispatch loop runs; RegSlice
        // (raw pointer + len) removes the LLVM noalias constraint (issue #547).
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        // Issue #712: seed __annotations__ = {} in the module env so that module-level
        // annotated assignments can do LoadGlobal("__annotations__") and SetItem.
        {
            use indexmap::IndexMap;
            self.env
                .borrow_mut()
                .values
                .entry("__annotations__".to_string())
                .or_insert_with(|| crate::value::Value::dict(IndexMap::new()));
        }
        let vm_result = self.run_bytecode(&code, regs_slice);
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
