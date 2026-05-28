impl Interpreter {
    /// Create an interpreter for running a named script file.
    ///
    /// Stores both the directory (for module-relative imports) and the
    /// filename (for traceback `File "..."` entries).
    pub fn with_script_path(path: &str) -> Self {
        let dir = std::path::Path::new(path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        Self {
            script_dir: Some(dir),
            script_filename: Some(std::sync::Arc::from(path)),
            ..Default::default()
        }
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
        // Issue #820: restore fastlocal registers for module scope.
        //
        // Prior to PR #810, module-scope names were allocated fastlocal registers
        // (O(1) array access).  PR #810 switched to all-env mode (empty local_index)
        // to fix globals() live-dict semantics (issue #706), but that caused a 3.5x
        // slowdown because every module-scope load/store goes through lookup_name /
        // assign_name (multiple Rc<RefCell<>> borrows + HashSet + HashMap per access).
        //
        // The fix: restore register allocation for module scope and add a new
        // SyncModuleGlobal instruction that keeps module_globals_dict live when
        // globals() has been called.  SyncModuleGlobal is a NOP when globals_accessed
        // == false (the common case — most scripts never call globals()).
        use std::collections::HashSet;
        let empty: HashSet<String> = HashSet::new();
        let local_names =
            crate::interpreter::collect_local_names(&[], program, &empty, &empty);
        // Cap at 200 locals to keep register allocation bounded for giant scripts.
        // Anything above this threshold falls back to all-env mode for that name.
        const MAX_SCRIPT_LOCALS: usize = 200;
        let local_index: Rc<HashMap<String, crate::bytecode::Reg>> =
            if local_names.len() <= MAX_SCRIPT_LOCALS {
                Rc::new(
                    (0u32..)
                        .zip(local_names.iter())
                        .map(|(i, n)| (n.clone(), i))
                        .collect(),
                )
            } else {
                // Fall back to all-env mode when there are too many module-level
                // names.  This is an unlikely edge case (scripts with > 200
                // top-level names) so the old regression is acceptable there.
                Rc::new(HashMap::new())
            };
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
        // Clone from the thread-local cache (O(1)) instead of rebuilding the
        // builtins module's ~136-entry HashMap on every script invocation.
        let builtins_val = cached_builtins_module();
        // Warm module_cache so `import builtins` is a free cache hit.
        self.module_cache
            .borrow_mut()
            .entry("builtins".to_string())
            .or_insert_with(|| builtins_val.clone());

        let me_ref = module_env(&self.env);
        let mut me = me_ref.borrow_mut();
        // Insert each dunder only if absent — do not overwrite an existing binding.
        macro_rules! seed_env {
            ($name:literal, $val:expr) => {
                me.values.entry($name.to_string()).or_insert_with(|| $val)
            };
        }
        seed_env!("__name__", intern_string("__main__"));
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
            // StrKey probe (issue #506): read check is zero-alloc; owned key
            // is constructed only when an entry is actually missing and must be
            // inserted (the rare path on first execution of a module).
            let has_key = self
                .module_globals_dict
                .dict_with(|d| d.contains_key(&StrKey(*name)))
                .unwrap_or(false);
            if !has_key {
                if let Some(v) = me.values.get(*name).cloned() {
                    let key = PyKey::str_from(*name);
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
        // Reset any captured frames from a previous run (REPL re-invocation).
        pyrust_core::reset_captured_error_frames();
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
        // with no &mut [Value] alive.  This eliminates two related aliasing UB
        // classes: the suspended-frame case (issue #547, PR #646) and the
        // active-script-frame case (issue #648): when locals()/globals() reads
        // VmFrameView::regs_ptr while the dispatch loop is executing, both
        // accesses go through raw pointers with no noalias assertion in scope.
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
        // (raw pointer + len) carries no noalias attribute (issues #547, #648).
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
        // Snapshot the traceback frame stack before the inner function frames
        // are cleared.  The `<module>` frame is prepended by us since it is
        // owned by this level; inner function frames were pushed/popped by
        // `call_user_function_expanded` and are no longer on the stack here
        // (they were popped before returning the error).  The call-site frames
        // were captured into the CAPTURED_ERROR_FRAMES thread-local by calls.rs.
        let maybe_tb: Option<String> = if let (Err(e), Some(filename)) =
            (&vm_result, &self.script_filename)
        {
            // CPython does not emit a traceback for SystemExit (the process
            // exits cleanly).  Skip traceback formatting when the error is a
            // SystemExit so that `exec_program`'s SystemExit handler still
            // receives the original `PyError::Raised(SystemExit)` variant
            // (not a pre-formatted `PyError::Runtime` string).
            let is_system_exit = e.class_name_is("SystemExit");
            // Take the captured frames regardless; cleans the thread-local
            // for the next invocation.
            let inner_frames = pyrust_core::take_captured_error_frames().unwrap_or_default();
            if is_system_exit {
                // Suppress traceback; let exec_program handle SystemExit normally.
                None
            } else {
                // Build the full frame list: <module> at the bottom (outermost),
                // then the function frames in innermost-last order.
                let mut frames = vec![pyrust_core::FrameInfo {
                    filename: filename.clone(),
                    lineno: None,
                    funcname: std::sync::Arc::from("<module>"),
                }];
                frames.extend(inner_frames);
                let error_line = match e {
                    PyError::Lex(s) => format!("Lex error: {s}"),
                    PyError::Parse(s) => format!("Parse error: {s}"),
                    PyError::Runtime(s) => format!("RuntimeError: {s}"),
                    PyError::Named(cls, s) => format!("{cls}: {s}"),
                    PyError::Class(cls, s) => format!("{}: {s}", cls.borrow().name),
                    PyError::KeyError(key) => format!("KeyError: {}", key.repr()),
                    PyError::ImportError { class_name, message, .. } => {
                        format!("{class_name}: {message}")
                    }
                    PyError::OsError {
                        class_name,
                        errno,
                        strerror,
                        filename,
                        ..
                    } => {
                        if let Some(fname) = filename {
                            format!("{class_name}: [Errno {errno}] {strerror}: '{fname}'")
                        } else {
                            format!("{class_name}: [Errno {errno}] {strerror}")
                        }
                    }
                    PyError::UnicodeDecodeError {
                        encoding,
                        object,
                        start,
                        end,
                        reason,
                    } => {
                        let msg = pyrust_core::format_unicode_decode_str(
                            &encoding, &object, *start, *end, &reason,
                        );
                        format!("UnicodeDecodeError: {msg}")
                    }
                    PyError::UnicodeEncodeError {
                        encoding,
                        object,
                        start,
                        end,
                        reason,
                    } => {
                        let msg = pyrust_core::format_unicode_encode_str(
                            &encoding, &object, *start, *end, &reason,
                        );
                        format!("UnicodeEncodeError: {msg}")
                    }
                    PyError::Raised(value) => match value.kind() {
                        ValueKind::PyInstance(inst) => {
                            let class_name = inst.borrow().class.borrow().name.clone();
                            // to_py_str() uses exception_to_string() which correctly
                            // handles KeyError (repr of single arg) and multi-arg
                            // exceptions (tuple notation), matching CPython __str__.
                            let msg = value.to_py_str();
                            if msg.is_empty() {
                                class_name
                            } else {
                                format!("{class_name}: {msg}")
                            }
                        }
                        _ => format!("Uncaught exception: {}", value.repr()),
                    },
                };
                // PEP 3134: walk __cause__ / __context__ and prepend each
                // chained exception's line with the appropriate connecting
                // banner ("The above exception was the direct cause of..."
                // or "During handling of the above exception...").
                let chain_prefix = if let PyError::Raised(exc_val) = e {
                    format_exc_chain_prefix(exc_val)
                } else {
                    String::new()
                };
                let main_tb = pyrust_core::format_traceback(&frames, &error_line);
                Some(format!("{chain_prefix}{main_tb}"))
            }
        } else {
            None
        };
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
        // If a traceback was formatted, return it as a pre-formatted Runtime
        // error.  The `run_file` thread boundary extracts the raw message from
        // `PyError::Runtime` (without the "Runtime error: " prefix) so the
        // formatted traceback reaches `main`'s `eprintln!` unchanged.
        if let Some(tb) = maybe_tb {
            return Some(Err(PyError::Runtime(tb)));
        }
        Some(vm_result.map(|_| ()))
    }

}
