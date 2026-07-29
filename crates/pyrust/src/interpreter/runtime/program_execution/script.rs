// Source parsing plus module/eval/exec execution lifecycle.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptErrorMode {
    FormatUncaught,
    PropagateImport,
}

enum ModuleNamespaceStorage {
    /// Ordinary scripts keep their bounded set of module names in registers.
    FastLocals(Rc<HashMap<String, crate::bytecode::Reg>>),
    /// A source-backed module registered before execution must remain
    /// externally mutable while a circular import suspends its body.
    SharedGlobals,
}

impl Interpreter {
    /// Create an interpreter for a named script plus the arguments supplied
    /// after that script on the command line. The resulting context backs
    /// `__file__` and `sys.argv`.
    ///
    /// Stores both the directory (for module-relative imports) and the
    /// filename (for traceback `File "..."` entries).
    pub fn with_script_path_and_args(path: &str, args: &[String]) -> Self {
        let dir = std::path::Path::new(path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let mut script_argv = Vec::with_capacity(args.len() + 1);
        script_argv.push(path.to_string());
        script_argv.extend(args.iter().cloned());
        Self {
            script_dir: Some(dir),
            script_filename: Some(std::sync::Arc::from(path)),
            script_argv,
            ..Default::default()
        }
    }

    pub fn exec_program(&mut self, program: &[Stmt], repl_mode: bool) -> Result<()> {
        self.exec_program_with_linenos(program, &[], "", repl_mode)
    }

    /// Like `exec_program` but with per-statement line numbers and the
    /// verbatim source text.  Used by the file-execution path to enable
    /// traceback source echoing and underlines.
    ///
    /// `linenos` is a parallel slice of 1-based source line numbers for each
    /// statement in `program` (empty = no line info).  `src` is the full
    /// source text of the script file (empty = no source echo in tracebacks).
    pub fn exec_program_with_linenos(
        &mut self,
        program: &[Stmt],
        linenos: &[u32],
        src: &str,
        repl_mode: bool,
    ) -> Result<()> {
        // The core formatting/parsing layer is Interpreter-agnostic, so expose
        // this interpreter's digit policy only for its dynamic execution
        // extent. Nested entries on the same interpreter reuse the scope.
        let _int_max_str_digits_guard = IntMaxStrDigitsExecutionGuard::enter(self);
        let result = if let Some(r) = self.try_exec_vm_script_with_src(
            program,
            linenos,
            src,
            repl_mode,
            ScriptErrorMode::FormatUncaught,
        ) {
            r
        } else {
            Err(PyError::Runtime("compilation failed".to_string()))
        };
        // Intercept SystemExit so finally/with handlers run before the process exits.
        if let Err(PyError::Raised(exc)) = &result
            && let ValueKind::PyInstance(inst) = exc.kind()
        {
            let class = Rc::clone(&inst.borrow().class);
            if pyrust_core::class_chain_contains_builtin_exception(&class, "SystemExit") {
                // Extract exit code from args[0]; default to 0.
                let code = match inst.borrow().attrs.get_slot("args") {
                    Some(args_val) => match args_val.kind() {
                        ValueKind::Tuple(args) if !args.is_empty() => match args[0].kind() {
                            ValueKind::Int(n) => n as i32,
                            ValueKind::Bool(b) => b as i32,
                            ValueKind::None => 0,
                            _ => 1,
                        },
                        _ => 0,
                    },
                    None => 0,
                };
                std::process::exit(code);
            }
        }
        result
    }

    /// Execute a filesystem module body while preserving the typed exception
    /// and captured frame metadata for the importing VM.
    ///
    /// Top-level script execution formats an uncaught error for stderr. An
    /// import is instead nested Python execution: its caller may catch and
    /// inspect the original exception, so formatting here would collapse a
    /// `PyError::Raised` into a new RuntimeError string and discard traceback
    /// identity.
    pub(crate) fn exec_import_program_with_linenos(
        &mut self,
        program: &[Stmt],
        linenos: &[u32],
        src: &str,
    ) -> Result<()> {
        let _int_max_str_digits_guard = IntMaxStrDigitsExecutionGuard::enter(self);
        self.try_exec_vm_script_with_index(
            program,
            linenos,
            src,
            false,
            ScriptErrorMode::PropagateImport,
            ModuleNamespaceStorage::SharedGlobals,
        )
        .unwrap_or_else(|| Err(PyError::Runtime("compilation failed".to_string())))
    }

    fn try_exec_vm_script_with_src(
        &mut self,
        program: &[Stmt],
        linenos: &[u32],
        src: &str,
        repl_mode: bool,
        error_mode: ScriptErrorMode,
    ) -> Option<Result<()>> {
        // Issue #820: restore fastlocal registers for module scope.
        //
        // Prior to PR #810, module-scope names were allocated fastlocal registers
        // (O(1) array access).  PR #810 switched to all-env mode (empty local_index)
        // to fix globals() live-dict semantics (issue #706), but that caused a 3.5x
        // slowdown because every module-scope load/store goes through lookup_name /
        // assign_name (multiple Rc<RefCell<>> borrows + HashSet + HashMap per access).
        //
        // The fix: restore register allocation for module scope and add a new
        // SyncModuleGlobal instruction that keeps the root-owned globals backing
        // live after globals() has exposed it. Before exposure the instruction
        // skips the dict write (the common case).
        //
        // Filesystem modules do not use this path: because their partial module
        // object escapes during circular import, they select SharedGlobals in
        // exec_import_program_with_linenos. Thus the fastlocal cost exception is
        // limited to externally reachable module initialization, not ordinary
        // scripts or exec().
        use std::collections::HashSet;
        let empty: HashSet<String> = HashSet::new();
        let global_names = crate::interpreter::collect_global_names(program);
        let local_names =
            crate::interpreter::collect_local_names(&[], program, &global_names, &empty);
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
        self.try_exec_vm_script_with_index(
            program,
            linenos,
            src,
            repl_mode,
            error_mode,
            ModuleNamespaceStorage::FastLocals(local_index),
        )
    }

    /// Seed the module-level global namespace with the standard dunder keys
    /// that CPython 3.12 always pre-populates for `__main__` (issue #675).
    ///
    /// Inserts only keys that are not already present, so a REPL second-pass
    /// or a future import path that has already seeded the namespace does not
    /// clobber any existing value.  User assignments made during script
    /// execution override the pre-seeded values naturally via `assign_name`,
    /// which keeps both `env.values` and the root globals backing in sync.
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
        // Mirror into this interpreter's `sys.modules` dict (issue #2727) so that
        // `builtins` shows up in the import cache as it does in CPython, even
        // before any explicit `import builtins`.
        let _ = self.insert_import_registry("builtins", builtins_val.clone());

        let me_ref = module_env(&self.env);
        let mut me = me_ref.borrow_mut();
        let module_globals = me.namespace_globals();
        // Insert each dunder only if absent — do not overwrite an existing binding.
        macro_rules! seed_env {
            ($name:literal, $val:expr) => {{
                me.record_namespace_env_binding($name);
                me.values.get_or_insert_with($name, || $val)
            }};
        }
        // Seeding order is Python-visible: `list(globals())` starts with these
        // keys in insertion order (issue #2903).  CPython 3.12 creates
        // `__main__` with `__name__ __doc__ __package__ __loader__ __spec__`,
        // then `__annotations__` and `__builtins__`, and the script runner
        // appends `__file__` / `__cached__` last.
        seed_env!("__name__", intern_string("__main__"));
        seed_env!("__doc__", Value::none());
        seed_env!("__package__", Value::none());
        seed_env!("__loader__", Value::none());
        seed_env!("__spec__", Value::none());
        seed_env!("__annotations__", Value::dict(PyDict::default()));
        seed_env!("__builtins__", builtins_val);
        let script_file = self.script_filename.as_deref().map(Value::string);
        let has_script_file = script_file.is_some();
        if let Some(file) = script_file {
            seed_env!("__file__", file);
        }
        seed_env!("__cached__", Value::none());
        // Mirror env values into the root-owned globals backing (issue #706)
        // in the same order.
        // Only insert keys not already present in the dict (REPL re-run safety).
        let dunders: &[&str] = if has_script_file {
            &[
                "__name__",
                "__doc__",
                "__package__",
                "__loader__",
                "__spec__",
                "__annotations__",
                "__builtins__",
                "__file__",
                "__cached__",
            ]
        } else {
            &[
                "__name__",
                "__doc__",
                "__package__",
                "__loader__",
                "__spec__",
                "__annotations__",
                "__builtins__",
                "__cached__",
            ]
        };
        for name in dunders {
            // StrKey probe (issue #506): read check is zero-alloc; owned key
            // is constructed only when an entry is actually missing and must be
            // inserted (the rare path on first execution of a module).
            let has_key = module_globals
                .dict_with(|d| d.contains_key(&StrKey(name)))
                .unwrap_or(false);
            if !has_key && let Some(v) = me.values.get(name).cloned() {
                let key = PyKey::str_from(*name);
                let _ = module_globals.dict_insert(key, v);
            }
        }
    }

    /// Make the script invocation context observable before the first user
    /// instruction. Loading `sys` here also ensures imported modules share the
    /// top-level script's argument vector, even when they import `sys` first.
    fn initialize_script_argv(&mut self) -> Result<()> {
        if self.script_filename.is_none() {
            return Ok(());
        }
        let sys = self.load_module("sys")?;
        let ValueKind::PyModule(module) = sys.kind() else {
            return Err(PyError::Runtime("sys is not a module".to_string()));
        };
        module.borrow_mut().insert_attr(
            "argv".to_string(),
            Value::list(self.script_argv.iter().map(Value::string).collect()),
        );
        Ok(())
    }

    fn try_exec_vm_script_with_index(
        &mut self,
        program: &[Stmt],
        linenos: &[u32],
        src: &str,
        repl_mode: bool,
        error_mode: ScriptErrorMode,
        namespace_storage: ModuleNamespaceStorage,
    ) -> Option<Result<()>> {
        // Reset any captured frames from a previous run (REPL re-invocation).
        pyrust_core::reset_captured_error_frames();
        pyrust_core::reset_current_vm_line();
        // Tag every code object with this script's path so traceback frames and
        // `__code__.co_filename` report the file the code came from, even when an
        // imported module's function is called from a different script (#2438).
        let script_path = self.script_filename.as_deref().unwrap_or("<unknown>");
        let (local_index, compiled) = match namespace_storage {
            ModuleNamespaceStorage::FastLocals(local_index) => {
                let compiled = crate::compiler::compile_script_with_linenos(
                    program,
                    Rc::clone(&local_index),
                    repl_mode,
                    linenos,
                    script_path,
                );
                (local_index, compiled)
            }
            ModuleNamespaceStorage::SharedGlobals => (
                Rc::new(HashMap::new()),
                crate::compiler::compile_shared_namespace_module_with_linenos(
                    program,
                    linenos,
                    script_path,
                ),
            ),
        };
        let code = match compiled {
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
        if let Err(err) = self.initialize_script_argv() {
            return Some(Err(err));
        }
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
            // Retain the root so globals operations inside an imported
            // function cannot accidentally use the caller's script registers.
            env: Some(Rc::clone(&self.env)),
            is_class_method: false,
            function: None,
            gen_frame: None,
        });
        // SAFETY: regs_ptr is valid for regs_len Values for the lifetime of
        // `regs` (a local RegsBuf that outlives this call).  No &mut [Value]
        // referencing `regs` is held while the dispatch loop runs; RegSlice
        // (raw pointer + len) carries no noalias attribute (issues #547, #648).
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        // Once globals/locals escapes, storage-level mutations (including
        // through another Interpreter or instance.__dict__) refresh this view.
        let namespace_mirror_guard =
            self.register_script_namespace_mirror(regs_ptr, regs_len, &local_index);
        // Issue #712: seed __annotations__ = {} in the module env so that module-level
        // annotated assignments can do LoadGlobal("__annotations__") and SetItem.
        {
            self.env
                .borrow_mut()
                .values
                .get_or_insert_with("__annotations__", || {
                    crate::value::Value::dict(PyDict::default())
                });
        }
        let vm_result = self.run_bytecode(&code, regs_slice);
        if error_mode == ScriptErrorMode::PropagateImport && vm_result.is_err() {
            // Imports are nested execution. Preserve their module frame in the
            // same lazy snapshot that already contains any inner function
            // frames, then return the original typed PyError to the importing
            // opcode. `record_traceback_frame` prepends, yielding
            // module -> outer function -> inner function ordering.
            let lineno = match pyrust_core::get_current_vm_line() {
                0 => None,
                line => Some(line),
            };
            let source_line = if let (Some(line), false) = (lineno, src.is_empty()) {
                src.lines()
                    .nth((line as usize).saturating_sub(1))
                    .map(|line| std::sync::Arc::from(line.trim_end()))
            } else {
                None
            };
            let col_span = source_line
                .as_ref()
                .and_then(|_| pyrust_core::get_current_vm_col_span());
            pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                filename: code.filename.clone(),
                lineno,
                source_line,
                funcname: std::sync::Arc::from("<module>"),
                globals: Some(pyrust_core::FrameGlobals::for_environment(&self.env)),
                col_span,
            });
        }
        // Snapshot the traceback frame stack before the inner function frames
        // are cleared.  The `<module>` frame is prepended by us since it is
        // owned by this level; inner function frames were pushed/popped by
        // `call_user_function_expanded` and are no longer on the stack here
        // (they were popped before returning the error).  The call-site frames
        // were captured into the CAPTURED_ERROR_FRAMES thread-local by calls.rs.
        let maybe_tb: Option<String> = if error_mode == ScriptErrorMode::FormatUncaught {
            if let (Err(e), Some(filename)) = (&vm_result, &self.script_filename) {
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
                    // `filename` borrows `self.script_filename`; clone it to an
                    // owned `Arc` up front so the chained-traceback formatter (which
                    // takes `&mut self`) can run without conflicting with that
                    // immutable borrow.  Cheap refcount bump on the cold print path.
                    let owned_filename = filename.clone();
                    // Build the full frame list: <module> at the bottom (outermost),
                    // then the function frames in innermost-last order.
                    //
                    // The `<module>` frame's line number comes from the VM's
                    // current-line tracker (updated per instruction by the dispatch
                    // loop via `set_current_vm_line`).  When no line info is
                    // available (lineno_table all-zero), `get_current_vm_line()`
                    // returns 0, which we map to `None`.
                    let module_lineno = {
                        let n = pyrust_core::get_current_vm_line();
                        if n != 0 { Some(n) } else { None }
                    };
                    // If we have the source text, look up the line's text.
                    let module_source_line: Option<std::sync::Arc<str>> =
                        if let (Some(n), false) = (module_lineno, src.is_empty()) {
                            src.lines()
                                .nth((n as usize).saturating_sub(1))
                                .map(|l| std::sync::Arc::from(l.trim_end()))
                        } else {
                            None
                        };
                    // PEP 657 caret anchor (#2426): the VM published the raising
                    // instruction's column span on the error path.  Only meaningful
                    // when we have a source line to underline; cleared otherwise so a
                    // stale span never paints a caret onto an unrelated line.
                    let module_col_span = if module_source_line.is_some() {
                        pyrust_core::get_current_vm_col_span()
                    } else {
                        None
                    };
                    let mut frames = vec![pyrust_core::FrameInfo {
                        filename: filename.clone(),
                        lineno: module_lineno,
                        source_line: module_source_line,
                        funcname: std::sync::Arc::from("<module>"),
                        // This frame is consumed only by the stderr formatter;
                        // Python-visible traceback nodes capture globals when
                        // the exception is caught.
                        globals: None,
                        col_span: module_col_span,
                    }];
                    // Issue #2404: a re-raised exception's `__traceback__` chain is
                    // the authoritative, Python-visible frame list — after #2367 the
                    // captured-frame snapshot was reset at the re-raise site and so
                    // diverges (it drops the re-raising/prepended frames).  When the
                    // raised value carries such a chain, derive the inner frames from
                    // it; otherwise fall back to the captured snapshot (raw `PyError`
                    // variants and never-caught exceptions keep the snapshot path).
                    let tb_inner = if let PyError::Raised(exc_val) = e {
                        // `reraise_is_bare` survives to here only when the last
                        // re-raise was bare and no `except` consumed it (i.e. the
                        // exception is genuinely uncaught) — it then tells the
                        // formatter to drop the bare re-raise frame's own node.
                        self.uncaught_inner_frames_from_tb(
                            exc_val,
                            filename,
                            src,
                            &inner_frames,
                            self.reraise_is_bare,
                        )
                    } else {
                        None
                    };
                    match tb_inner {
                        Some(tb_frames) => {
                            // A *bare* re-raise (#2405) — including the PEP 654
                            // `except*` residual re-raise, which re-raises the
                            // leftover group like a bare `raise` (#2755) — adds no
                            // node for the re-raising frame itself.  When that
                            // re-raise sits at module scope the carried frame list
                            // already begins with the `<module>` frame, so the
                            // synthetic `<module>` frame built above (the current VM
                            // line) duplicates it.  Drop the synthetic frame in that
                            // case.  A bare re-raise inside a *function* keeps the
                            // synthetic `<module>` frame: there the carried list
                            // begins with the enclosing function, and `<module>` is a
                            // genuine outer caller.
                            if self.reraise_is_bare
                                && tb_frames
                                    .first()
                                    .is_some_and(|f| f.funcname.as_ref() == "<module>")
                            {
                                frames.clear();
                            }
                            frames.extend(tb_frames);
                        }
                        None => {
                            // Issue #2428: the captured snapshot records each call
                            // frame's `(filename, lineno)` but leaves `source_line`
                            // as `None` — resolving the text at capture time would
                            // put a per-frame line scan on the unwind path.  CPython
                            // echoes the (dedented) source line under *every* frame,
                            // so fill it in here on the cold print path before the
                            // formatter runs.  Same-file frames resolve from the
                            // script `src` we already hold; other files fall back to
                            // a linecache-style disk read (skipped for `<…>` pseudo
                            // filenames such as `<stdin>` / `<unknown>`).
                            for mut fi in inner_frames {
                                if fi.source_line.is_none() {
                                    fi.source_line = resolve_frame_source_line(
                                        &fi.filename,
                                        fi.lineno,
                                        filename.as_ref(),
                                        src,
                                    );
                                }
                                frames.push(fi);
                            }
                        }
                    }
                    let error_line = match e {
                        PyError::Lex(s) => format!("Lex error: {s}"),
                        PyError::Parse(s) => format!("Parse error: {s}"),
                        PyError::Runtime(s) => format!("RuntimeError: {s}"),
                        // CPython omits the `: msg` suffix when the exception's
                        // message is empty (e.g. internal `StopIteration` from an
                        // exhausted iterator displays as `StopIteration`, not
                        // `StopIteration: `).
                        PyError::Named(cls, s) if s.is_empty() => cls.to_string(),
                        PyError::Named(cls, s) => format!("{cls}: {s}"),
                        PyError::Class(cls, s) if s.is_empty() => cls.borrow().name.clone(),
                        PyError::Class(cls, s) => format!("{}: {s}", cls.borrow().name),
                        PyError::KeyError(key) => {
                            // The raw-key fast-path variant: dispatch the key's
                            // __repr__ like CPython's traceback printer (issue
                            // #2390 review); a raising dunder falls back to the
                            // data-only repr.
                            let key_repr = if matches!(key.kind(), ValueKind::PyInstance(_)) {
                                let key_cloned = key.clone();
                                render_instance_repr(self, &key_cloned)
                                    .unwrap_or_else(|_| key.repr_raw())
                            } else {
                                key.repr_raw()
                            };
                            format!("KeyError: {key_repr}")
                        }
                        PyError::NameError {
                            class_name,
                            message,
                            ..
                        } => {
                            format!("{class_name}: {message}")
                        }
                        PyError::AttributeError { message, .. } => {
                            format!("AttributeError: {message}")
                        }
                        PyError::ImportError {
                            class_name,
                            message,
                            ..
                        } => {
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
                                encoding, object, *start, *end, reason,
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
                                encoding, object, *start, *end, reason,
                            );
                            format!("UnicodeEncodeError: {msg}")
                        }
                        PyError::Raised(value) => match value.kind() {
                            ValueKind::PyInstance(inst) => {
                                let class_name = inst.borrow().class.borrow().name.clone();
                                // exception_str_with_dispatch handles KeyError (repr of
                                // single arg) and multi-arg exceptions (tuple notation),
                                // dispatching arg __repr__/__str__ overrides like CPython's
                                // traceback printer; a raising dunder falls back to the
                                // data-only core renderer.
                                let inst_rc = Rc::clone(inst);
                                let cls = Rc::clone(&inst_rc.borrow().class);
                                let msg = exception_str_with_dispatch(self, value, &inst_rc, &cls)
                                    .unwrap_or_else(|_| value.to_py_str());
                                let mut line = if msg.is_empty() {
                                    class_name
                                } else {
                                    format!("{class_name}: {msg}")
                                };
                                // PEP 678: append each note from `__notes__` after the
                                // error message, matching CPython's traceback printer
                                // (`add_note` / `__set_name__` notes). CPython iterates
                                // `str()` of each item only when `__notes__` is a list or
                                // tuple; any other object is printed once as `repr()`.
                                let notes = inst_rc.borrow().attrs.get_cloned("__notes__");
                                if let Some(notes) = notes.as_ref() {
                                    let items = if let Some(tuple) = notes.as_tuple() {
                                        Some(tuple.to_vec())
                                    } else {
                                        notes.as_list().map(|list| list.to_vec())
                                    };
                                    if let Some(items) = items {
                                        for note in &items {
                                            line.push('\n');
                                            line.push_str(&note.to_py_str());
                                        }
                                    } else {
                                        line.push('\n');
                                        line.push_str(&notes.repr_raw());
                                    }
                                }
                                line
                            }
                            _ => format!("Uncaught exception: {}", value.repr_raw()),
                        },
                    };
                    // PEP 3134: walk __cause__ / __context__ and prepend each
                    // chained exception's line with the appropriate connecting
                    // banner ("The above exception was the direct cause of..."
                    // or "During handling of the above exception...").
                    let chain_prefix = if let PyError::Raised(exc_val) = e {
                        format_exc_chain_prefix(self, exc_val, &owned_filename, src)
                    } else {
                        String::new()
                    };
                    let main_tb = pyrust_core::format_traceback(&frames, &error_line);
                    Some(format!("{chain_prefix}{main_tb}"))
                }
            } else {
                None
            }
        } else {
            None
        };
        drop(namespace_mirror_guard);
        self.vm_frame_views.pop();
        // Write fastlocal registers back to the module env so that imported
        // modules and post-run inspection can find all names.
        // StoreGlobal from nested scopes already updated the register via
        // the script-frame view (#520), so the write-back naturally carries
        // the correct value.
        self.write_back_script_locals(&local_index, &mut regs);
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
