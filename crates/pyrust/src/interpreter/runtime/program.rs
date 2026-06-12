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
        let result = if let Some(r) =
            self.try_exec_vm_script_with_src(program, linenos, src, repl_mode)
        {
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

    fn try_exec_vm_script_with_src(
        &mut self,
        program: &[Stmt],
        linenos: &[u32],
        src: &str,
        repl_mode: bool,
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
        self.try_exec_vm_script_with_index(program, linenos, src, local_index, repl_mode)
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
                me.values.get_or_insert_with($name, || $val)
            };
        }
        seed_env!("__name__", intern_string("__main__"));
        seed_env!("__doc__", Value::none());
        seed_env!("__package__", Value::none());
        seed_env!("__spec__", Value::none());
        seed_env!("__loader__", Value::none());
        seed_env!("__file__", Value::none());
        seed_env!("__cached__", Value::none());
        seed_env!("__annotations__", Value::dict(PyDict::default()));
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
                .dict_with(|d| d.contains_key(&StrKey(name)))
                .unwrap_or(false);
            if !has_key
                && let Some(v) = me.values.get(name).cloned() {
                    let key = PyKey::str_from(*name);
                    let _ = self.module_globals_dict.dict_insert(key, v);
                }
        }
    }

    fn try_exec_vm_script_with_index(
        &mut self,
        program: &[Stmt],
        linenos: &[u32],
        src: &str,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        repl_mode: bool,
    ) -> Option<Result<()>> {
        // Reset any captured frames from a previous run (REPL re-invocation).
        pyrust_core::reset_captured_error_frames();
        pyrust_core::reset_current_vm_line();
        let code = match crate::compiler::compile_script_with_linenos(
            program,
            Rc::clone(&local_index),
            repl_mode,
            linenos,
        ) {
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
            function: None,
        });
        // SAFETY: regs_ptr is valid for regs_len Values for the lifetime of
        // `regs` (a local RegsBuf that outlives this call).  No &mut [Value]
        // referencing `regs` is held while the dispatch loop runs; RegSlice
        // (raw pointer + len) carries no noalias attribute (issues #547, #648).
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
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
                let mut frames = vec![pyrust_core::FrameInfo {
                    filename: filename.clone(),
                    lineno: module_lineno,
                    source_line: module_source_line,
                    funcname: std::sync::Arc::from("<module>"),
                }];
                frames.extend(inner_frames);
                let error_line = match e {
                    PyError::Lex(s) => format!("Lex error: {s}"),
                    PyError::Parse(s) => format!("Parse error: {s}"),
                    PyError::Runtime(s) => format!("RuntimeError: {s}"),
                    PyError::Named(cls, s) => format!("{cls}: {s}"),
                    PyError::Class(cls, s) => format!("{}: {s}", cls.borrow().name),
                    PyError::KeyError(key) => {
                        // The raw-key fast-path variant: dispatch the key's
                        // __repr__ like CPython's traceback printer (issue
                        // #2390 review); a raising dunder falls back to the
                        // data-only repr.
                        let key_repr = if matches!(key.kind(), ValueKind::PyInstance(_)) {
                            let key_cloned = key.clone();
                            render_instance_repr(self, &key_cloned)
                                .unwrap_or_else(|_| key.repr())
                        } else {
                            key.repr()
                        };
                        format!("KeyError: {key_repr}")
                    }
                    PyError::NameError { class_name, message, .. } => {
                        format!("{class_name}: {message}")
                    }
                    PyError::AttributeError { message, .. } => {
                        format!("AttributeError: {message}")
                    }
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
                    format_exc_chain_prefix(self, exc_val)
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
                self.assign_name(name, val);
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

    /// Parse a Python source string into a statement list plus the per-statement
    /// 1-based line-number table, so the compiler can thread accurate line
    /// numbers into the bytecode for `exec`/`eval`/`compile`'d source.  Without
    /// this, errors raised *inside* exec'd code report wrong internal line
    /// numbers (issue #2245): the original path discarded the lexer's physical
    /// line table via `into_tokens()`.  Converts lexer/parse errors into
    /// `SyntaxError` (or its `IndentationError` subclass for indentation
    /// failures) exceptions.
    pub(crate) fn parse_source_to_stmts_with_linenos(
        source: &str,
    ) -> Result<(Vec<crate::ast::Stmt>, Vec<u32>)> {
        let (tokens, line_nos) = crate::lexer::Lexer::new(source)
            .map_err(lex_parse_to_exc)?
            .into_tokens_with_linenos();
        let mut parser = crate::parser::Parser::new_with_lines(tokens, line_nos);
        parser.parse_program_with_linenos().map_err(lex_parse_to_exc)
    }

    /// Execute a source string as statements, optionally in an explicit
    /// namespace.
    ///
    /// - `globals_dict`: when `None`, runs in the current interpreter's module
    ///   namespace (assignments become globals).  When `Some(dict)`, the dict
    ///   is used as both the globals and locals namespace; assignments write
    ///   back to the dict.
    /// - `locals_dict`: when `Some(dict)` (and `globals_dict` is also `Some`),
    ///   name lookups check this dict first; assignments go to `locals_dict`.
    ///   Matches CPython's exec(code, globals, locals) semantics.
    pub(crate) fn exec_source(
        &mut self,
        source: &str,
        globals_dict: Option<Value>,
        locals_dict: Option<Value>,
    ) -> Result<()> {
        let (program, linenos) = Self::parse_source_to_stmts_with_linenos(source)?;
        match globals_dict {
            None => {
                // No explicit namespace: compile and run in the current module
                // scope, but do NOT go through try_exec_vm_script_with_index —
                // that path converts any raised exception into PyError::Runtime
                // (the traceback-formatted string) which makes the exception
                // uncatchable by type.  exec() must propagate the raw exception
                // so callers can catch ZeroDivisionError, NameError, etc.
                use std::collections::HashSet;
                let empty: HashSet<String> = HashSet::new();
                let local_names =
                    crate::interpreter::collect_local_names(&[], &program, &empty, &empty);
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
                        Rc::new(HashMap::new())
                    };
                // Thread the lexer line table into the bytecode (issue #2245)
                // so errors inside the exec'd source report correct internal
                // line numbers.
                let code = match crate::compiler::compile_script_with_linenos(
                    &program,
                    Rc::clone(&local_index),
                    false,
                    &linenos,
                ) {
                    Ok(c) => Rc::new(crate::optimizer::optimize(c)),
                    Err(e) => return Err(e),
                };
                let num_regs = code.num_regs as usize;
                let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
                let regs_ptr =
                    unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
                let regs_len = regs.len();
                self.vm_frame_views.push(VmFrameView {
                    kind: FrameKind::Script,
                    regs_ptr,
                    regs_len,
                    local_index: Rc::clone(&local_index),
                    nonlocal_names: None,
                    env: None,
                    is_class_method: false,
                    function: None,
                });
                let regs_slice =
                    unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
                let vm_result = self.run_bytecode(&code, regs_slice);
                self.vm_frame_views.pop();
                record_exec_string_frame(&vm_result);
                // Write fastlocals back to module env so names are visible
                // after exec() returns, matching top-level assignment semantics.
                for (name, &idx) in local_index.iter() {
                    if !regs[idx as usize].is_unset() {
                        let val =
                            std::mem::replace(&mut regs[idx as usize], Value::unset());
                        self.assign_name(name, val);
                    }
                }
                vm_result.map(|_| ())
            }
            Some(gdict) => {
                // Explicit globals dict: seed a fresh env from the dict, run,
                // then write all new/changed names back to the dict.
                self.exec_in_dict_env(&program, &linenos, gdict, locals_dict)
            }
        }
    }

    /// Evaluate a source string as a single expression.
    ///
    /// Same namespace semantics as `exec_source`.  Returns the expression's
    /// value.
    pub(crate) fn eval_source(
        &mut self,
        source: &str,
        globals_dict: Option<Value>,
        locals_dict: Option<Value>,
    ) -> Result<Value> {
        // Strip leading and trailing whitespace: CPython's `eval()` strips both.
        // `eval("  1 + 2  ")` and `eval("1 + 2\n")` both work in CPython.
        let trimmed = source.trim();
        let (program, linenos) = Self::parse_source_to_stmts_with_linenos(trimmed)?;
        let local_index: Rc<HashMap<String, crate::bytecode::Reg>> =
            Rc::new(HashMap::new());
        let code = match crate::compiler::compile_eval_expr_with_linenos(
            &program,
            Rc::clone(&local_index),
            &linenos,
        ) {
            Ok(c) => Rc::new(crate::optimizer::optimize(c)),
            Err(e) => return Err(e),
        };
        match globals_dict {
            None => {
                // Run in current module namespace.
                self.run_eval_code_in_module(&code, local_index)
            }
            Some(gdict) => {
                self.eval_in_dict_env(&code, local_index, gdict, locals_dict)
            }
        }
    }

    /// Run a compiled eval-mode code object in the current module namespace.
    /// Pushes a Script VmFrameView so `globals()`/`locals()` work inside the
    /// evaluated expression.
    fn run_eval_code_in_module(
        &mut self,
        code: &Rc<crate::bytecode::FnCode>,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
    ) -> Result<Value> {
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
            env: None,
            is_class_method: false,
            function: None,
        });
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        let vm_result = self.run_bytecode(code, regs_slice);
        self.vm_frame_views.pop();
        record_exec_string_frame(&vm_result);
        vm_result
    }

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
        let local_names =
            crate::interpreter::collect_local_names(&[], program, &empty, &empty);
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
                Rc::new(HashMap::new())
            };
        // Thread the lexer line table into the bytecode (issue #2245) so errors
        // inside the exec'd source report correct internal line numbers.
        let code = match crate::compiler::compile_script_with_linenos(
            program,
            Rc::clone(&local_index),
            false,
            linenos,
        ) {
            Ok(c) => Rc::new(crate::optimizer::optimize(c)),
            Err(e) => return Err(e),
        };

        // Build a fresh env pre-seeded with values from globals_dict (and
        // locals_dict if provided).  This env has no parent so `assign_name`
        // treats it as module scope.
        let exec_env = self.build_dict_env(globals_dict.clone(), locals_dict.as_ref())?;
        let previous_env = std::mem::replace(&mut self.env, exec_env);
        // Preserve the current module_globals_dict and globals_accessed state
        // so they are restored after exec.
        let prev_mgd = self.module_globals_dict.clone();
        let prev_ga = self.globals_accessed;
        self.module_globals_dict = globals_dict.clone();
        self.globals_accessed = false;

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
            env: None,
            is_class_method: false,
            function: None,
        });
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        let vm_result = self.run_bytecode(&code, regs_slice);
        self.vm_frame_views.pop();
        record_exec_string_frame(&vm_result);

        // Write fastlocal registers back to the dict env so we can flush them.
        for (name, &idx) in local_index.iter() {
            if !regs[idx as usize].is_unset() {
                let val = std::mem::replace(&mut regs[idx as usize], Value::unset());
                self.env.borrow_mut().values.insert(name, val);
            }
        }

        // Flush the exec env values back to the target namespace.
        // CPython semantics: when `locals` is provided, all writes go to
        // `locals`; `globals` is read-only (unchanged by the executed code).
        // When only `globals` is provided (no locals), writes go to `globals`.
        let write_target = locals_dict.as_ref().unwrap_or(&globals_dict);
        let env_vals: Vec<(String, Value)> = self
            .env
            .borrow()
            .values
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        for (k, v) in env_vals {
            // Only write back values that originated in the local (exec) scope:
            // skip keys that came from globals and were not modified.
            if let Some(locals) = &locals_dict {
                // When locals is provided: only write back if key is in locals
                // or was a new assignment (not in globals before exec).
                let was_in_globals = globals_dict
                    .dict_with(|d| d.contains_key(&StrKey(k.as_str())))
                    .unwrap_or(false);
                let was_in_locals = locals
                    .dict_with(|d| d.contains_key(&StrKey(k.as_str())))
                    .unwrap_or(false);
                // Skip keys that came from globals-only (not in locals and not
                // new assignments); new assignments are identified by being
                // absent from both original globals and locals — we can't
                // distinguish cleanly without a snapshot, so we write ALL
                // keys present in the exec env to locals_dict.  This matches
                // CPython which writes all final local-namespace values to the
                // provided locals mapping.
                if was_in_globals && !was_in_locals {
                    // Key came purely from globals; skip (don't copy to locals).
                    continue;
                }
                let _ = locals.dict_insert(PyKey::str_from(&k), v);
            } else {
                let _ = write_target.dict_insert(PyKey::str_from(&k), v);
            }
        }

        // Restore interpreter state.
        self.env = previous_env;
        self.module_globals_dict = prev_mgd;
        self.globals_accessed = prev_ga;

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
        let previous_env = std::mem::replace(&mut self.env, exec_env);
        let prev_mgd = self.module_globals_dict.clone();
        let prev_ga = self.globals_accessed;
        self.module_globals_dict = globals_dict;
        self.globals_accessed = false;

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
            env: None,
            is_class_method: false,
            function: None,
        });
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        let vm_result = self.run_bytecode(code, regs_slice);
        self.vm_frame_views.pop();
        record_exec_string_frame(&vm_result);

        self.env = previous_env;
        self.module_globals_dict = prev_mgd;
        self.globals_accessed = prev_ga;

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
                    env: None,
                    is_class_method: false,
                    function: None,
                });
                let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
                let vm_result = self.run_bytecode(&code, regs_slice);
                self.vm_frame_views.pop();
                record_exec_string_frame(&vm_result);
                // Write fastlocals back to module env.
                for (name, &idx) in local_index.iter() {
                    if !regs[idx as usize].is_unset() {
                        let val = std::mem::replace(&mut regs[idx as usize], Value::unset());
                        self.assign_name(name, val);
                    }
                }
                vm_result.map(|_| ())
            }
            Some(gdict) => {
                // Prepare the program slice: we already have compiled code,
                // but exec_in_dict_env wants &[Stmt].  Use the lower-level
                // dict env path directly.
                let num_regs = code.num_regs as usize;
                let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
                let exec_env = self.build_dict_env(gdict.clone(), locals_dict.as_ref())?;
                let previous_env = std::mem::replace(&mut self.env, exec_env);
                let prev_mgd = self.module_globals_dict.clone();
                let prev_ga = self.globals_accessed;
                self.module_globals_dict = gdict.clone();
                self.globals_accessed = false;

                let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(regs.as_mut_ptr()) };
                let regs_len = regs.len();
                self.vm_frame_views.push(VmFrameView {
                    kind: FrameKind::Script,
                    regs_ptr,
                    regs_len,
                    local_index: Rc::clone(&local_index),
                    nonlocal_names: None,
                    env: None,
                    is_class_method: false,
                    function: None,
                });
                let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
                let vm_result = self.run_bytecode(&code, regs_slice);
                self.vm_frame_views.pop();
                record_exec_string_frame(&vm_result);

                for (name, &idx) in local_index.iter() {
                    if !regs[idx as usize].is_unset() {
                        let val = std::mem::replace(&mut regs[idx as usize], Value::unset());
                        self.env.borrow_mut().values.insert(name, val);
                    }
                }
                let env_vals: Vec<(String, Value)> = self
                    .env
                    .borrow()
                    .values
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect();
                for (k, v) in env_vals {
                    if let Some(ldict) = &locals_dict {
                        let was_in_globals = gdict
                            .dict_with(|d| d.contains_key(&StrKey(k.as_str())))
                            .unwrap_or(false);
                        let was_in_locals = ldict
                            .dict_with(|d| d.contains_key(&StrKey(k.as_str())))
                            .unwrap_or(false);
                        if was_in_globals && !was_in_locals {
                            continue;
                        }
                        let _ = ldict.dict_insert(PyKey::str_from(&k), v);
                    } else {
                        let _ = gdict.dict_insert(PyKey::str_from(&k), v);
                    }
                }
                self.env = previous_env;
                self.module_globals_dict = prev_mgd;
                self.globals_accessed = prev_ga;
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
                    && let Some(s) = sv.as_str() {
                        exec_env.borrow_mut().values.insert(s, v.clone());
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
                        && let Some(s) = sv.as_str() {
                            exec_env.borrow_mut().values.insert(s, v.clone());
                        }
                }
            });
            if seeded_locals.is_none() {
                return Err(pyrust_core::type_err!("exec/eval locals must be a dict"));
            }
        }
        Ok(exec_env)
    }
}

/// Convert a `PyError::Lex` / `PyError::Parse` produced during source parsing
/// into the Python exception CPython raises for it.  The raw message is used
/// directly (the `"Lex error: "` / `"Parse error: "` `Display` prefixes are
/// stripped) so the text matches CPython.  Indentation failures map to the
/// `IndentationError` subclass of `SyntaxError`, matching CPython 3.12's type
/// (e.g. `too many levels of indentation`, issue #2221); everything else is a
/// plain `SyntaxError`.
fn lex_parse_to_exc(e: PyError) -> PyError {
    let msg = match e {
        PyError::Lex(s) | PyError::Parse(s) => s,
        other => other.to_string(),
    };
    if is_indentation_message(&msg) {
        pyrust_core::py_err!("IndentationError", msg)
    } else {
        pyrust_core::py_err!("SyntaxError", msg)
    }
}

/// Whether a lexer/parser error message describes an indentation failure that
/// CPython reports as `IndentationError` rather than a bare `SyntaxError`.
fn is_indentation_message(msg: &str) -> bool {
    msg == "too many levels of indentation"
}

/// Synthesize the `<string>` traceback frame for an exception raised inside
/// `exec`/`eval`'d code (issue #2245).  CPython reports such errors with a
/// `File "<string>", line N, in <module>` frame, where N is the 1-based line
/// inside the exec'd source.  The inner VM dispatch loop has already recorded
/// the current line into `CURRENT_VM_LINE` (now that the exec'd bytecode
/// carries a `lineno_table`), so read it back and push a module-scope frame at
/// the front of the captured chain.  The frame carries no `source_line`: the
/// exec'd string is not a file, so CPython prints no source text for it.
///
/// Only records on error; the no-error path skips it entirely.
fn record_exec_string_frame(vm_result: &Result<Value>) {
    if vm_result.is_err() {
        let lineno = match pyrust_core::get_current_vm_line() {
            0 => None,
            n => Some(n),
        };
        pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
            filename: std::sync::Arc::from("<string>"),
            lineno,
            source_line: None,
            funcname: std::sync::Arc::from("<module>"),
        });
    }
}
