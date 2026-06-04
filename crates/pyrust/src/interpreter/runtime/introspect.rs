// Frame / traceback / code introspection objects (issues #2170, #2171).
//
// These build the Python-visible `frame`, `traceback`, and (full) `code`
// objects from the data the VM already tracks: the `vm_frame_views` stack
// (issue #389), the lazily-captured traceback `FrameInfo` chain (issue #2165),
// and each function's compiled `FnCode`.  Everything here runs off the hot
// path — only when user code explicitly reaches for `sys._getframe()`,
// `f.__code__`, `e.__traceback__`, or `sys.exc_info()`.

use pyrust_builtins::{code as code_obj, frame as frame_obj, traceback as tb_obj};

impl Interpreter {
    /// Build a full `code` object for `function`, populating the best-effort
    /// `co_*` attributes (`co_flags`, `co_filename`, `co_firstlineno`,
    /// `co_consts`, `co_names`) in addition to `co_name`/`co_argcount`/
    /// `co_varnames`.
    pub(crate) fn build_code_object(&self, function: &UserFunction) -> Value {
        let co_name = function.name.clone();
        let argcount = function
            .params
            .iter()
            .filter(|p| !p.is_args && !p.is_kwargs && !p.is_keyword_only)
            .count() as i64;

        // co_varnames: positional, then keyword-only, then *args, then **kwargs
        // (CPython ordering).
        let mut varnames: Vec<Value> = Vec::with_capacity(function.params.len());
        for p in function
            .params
            .iter()
            .filter(|p| !p.is_args && !p.is_kwargs && !p.is_keyword_only)
        {
            varnames.push(Value::string(p.name.clone()));
        }
        for p in function.params.iter().filter(|p| p.is_keyword_only) {
            varnames.push(Value::string(p.name.clone()));
        }
        for p in function.params.iter().filter(|p| p.is_args) {
            varnames.push(Value::string(p.name.clone()));
        }
        for p in function.params.iter().filter(|p| p.is_kwargs) {
            varnames.push(Value::string(p.name.clone()));
        }

        // co_flags: CPython sets CO_OPTIMIZED | CO_NEWLOCALS for every normal
        // function, plus CO_VARARGS / CO_VARKEYWORDS / CO_GENERATOR as the
        // signature/body warrant.
        let mut flags = code_obj::CO_OPTIMIZED | code_obj::CO_NEWLOCALS;
        if function.params.iter().any(|p| p.is_args) {
            flags |= code_obj::CO_VARARGS;
        }
        if function.params.iter().any(|p| p.is_kwargs) {
            flags |= code_obj::CO_VARKEYWORDS;
        }

        // co_filename: the script path the function was compiled from.
        let filename = self
            .script_filename
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());

        // co_consts / co_names / co_firstlineno / CO_GENERATOR from the
        // compiled FnCode when available.
        let mut firstlineno = 0i64;
        let mut consts: Vec<Value> = Vec::new();
        let mut names: Vec<Value> = Vec::new();
        if let Some(rc) = function.precompiled_code.as_ref() {
            if let Ok(fncode) = Rc::clone(rc).downcast::<crate::bytecode::FnCode>() {
                if fncode.is_generator {
                    flags |= code_obj::CO_GENERATOR;
                }
                // co_firstlineno: the first source line the body maps to (the
                // earliest non-zero entry in the line table; 0 when no line
                // information was recorded).
                firstlineno = fncode
                    .lineno_table
                    .iter()
                    .copied()
                    .find(|&n| n != 0)
                    .unwrap_or(0) as i64;
                consts = fncode.consts.clone();
                names = fncode
                    .names
                    .iter()
                    .map(|n| Value::string(n.clone()))
                    .collect();
            }
        }

        // co_freevars: the names this function reads from an enclosing function
        // scope, in CPython's sorted order (issue #2106).  Reuses the same
        // free-variable analysis that powers `__closure__`.
        let freevars: Vec<Value> = self
            .closure_free_vars(function)
            .into_iter()
            .map(|(name, _)| Value::string(name))
            .collect();

        code_obj::code_full(
            co_name,
            argcount,
            varnames,
            flags,
            filename,
            firstlineno,
            consts,
            names,
            freevars,
        )
    }

    /// Compute the function's free variables (the names it reads from an
    /// enclosing *function* scope, i.e. its `__closure__` cells), paired with
    /// their captured values, in CPython's `co_freevars` order (sorted by name).
    ///
    /// pyrust has no per-function free-var list: free variables are resolved at
    /// runtime through the captured `env` chain.  We recover the set by scanning
    /// the compiled body for `LoadGlobal` name references (free reads compile to
    /// `LoadGlobal`, which walks the env chain) and keeping those that actually
    /// resolve to a binding in a *non-module* enclosing env — exactly CPython's
    /// definition of a free variable (bound in an enclosing function scope, not
    /// a module global).  Names the function declares `global`/`local` are
    /// excluded so an explicit `global x` is never mistaken for a closure cell.
    pub(crate) fn closure_free_vars(&self, function: &UserFunction) -> Vec<(String, Value)> {
        let Some(rc) = function.precompiled_code.as_ref() else {
            return Vec::new();
        };
        let Ok(fncode) = Rc::clone(rc).downcast::<crate::bytecode::FnCode>() else {
            return Vec::new();
        };

        // Collect the distinct names loaded via `LoadGlobal` (free reads and
        // true globals both go through this insn; the env-resolution filter
        // below keeps only the free ones).  A free variable referenced *only*
        // inside a nested code object — a comprehension/genexpr, `lambda`, or
        // nested `def` — compiles its `LoadGlobal` into that nested
        // `FnCode`, not into `fncode.insns`, yet CPython still reports it as a
        // free variable of this function.  So recurse through `fn_protos` and
        // gather candidates from every nested body too.  The env-resolution
        // filter below is the authoritative gate: a nested proto's own local
        // never resolves in `function.env`, so it cannot be falsely promoted.
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut candidates: Vec<String> = Vec::new();
        collect_loadglobal_names(&fncode, &mut seen, &mut candidates);

        let mut found: Vec<(String, Value)> = Vec::new();
        for name in &candidates {
            // An explicit `global`/`nonlocal` mismatch or own local is not a
            // closure free var.  `global` names target the module env;
            // own-local names never reach `LoadGlobal` for a free read.
            if function.global_names.contains(name) || function.local_names.contains(name) {
                continue;
            }
            if let Some(value) = lookup_enclosing_function_value(&function.env, name) {
                found.push((name.clone(), value));
            }
        }
        // CPython reports `co_freevars` (and orders `__closure__` cells) sorted
        // by name.
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    /// Build `function.__closure__`: a tuple of `cell` objects, one per free
    /// variable (in `co_freevars` order), or `None` for a function with no free
    /// variables (issue #2106).
    pub(crate) fn build_closure(&self, function: &UserFunction) -> Value {
        let cells: Vec<Value> = self
            .closure_free_vars(function)
            .into_iter()
            .map(|(_, value)| pyrust_builtins::cell::cell(value))
            .collect();
        if cells.is_empty() {
            Value::none()
        } else {
            Value::tuple(cells)
        }
    }

    /// Build a `frame` object for the VM frame view at stack index `idx`
    /// (0 = innermost / top of `vm_frame_views`), chained to its outer frames
    /// via `f_back`.  Returns `Value::none()` when `idx` is out of range.
    ///
    /// `lineno` is the current source line for the innermost frame (caller
    /// supplies it from the VM line tracker); outer frames report line `0`
    /// (a best-effort value, since pyrust keeps per-frame line state
    /// register-resident rather than in the frame view).
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
                // co_name matches CPython (`<module>` for module scope).
                code_obj::code("<module>".to_string(), 0, Vec::new())
            }
        };

        let back = self.build_frame_object(idx + 1, 0);
        let globals = self.module_globals_dict.clone();
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

    /// Build the Python-visible traceback object chain for an exception that
    /// is being caught, from the lazily-captured `FrameInfo` snapshot plus the
    /// catching frame.  Returns a `traceback` node (the outermost) whose
    /// `tb_next` chain walks inward to the raise site, matching CPython's
    /// `__traceback__` ordering.
    ///
    /// `catch_lineno` is the source line in the catching frame where the
    /// exception propagated from (the call/raise that is being handled).
    pub(crate) fn build_traceback_object(&self, catch_lineno: i64) -> Value {
        // Captured frames are ordered outermost-first / innermost-last and
        // contain only the *callee* frames the error unwound through.  The
        // catching frame (the one whose handler caught the error) is one level
        // more outer than all of them and is not in the list.
        let captured = pyrust_core::clone_captured_error_frames();

        // Determine the catching frame's code from the current innermost frame
        // view (the frame running the handler).
        let catch_code = match self.vm_frame_views.last() {
            Some(view) => match &view.function {
                Some(func) => self.build_code_object(func),
                None => code_obj::code("<module>".to_string(), 0, Vec::new()),
            },
            None => code_obj::code("<module>".to_string(), 0, Vec::new()),
        };

        // Build the chain from innermost (tb_next == None) outward, so the
        // outermost node is returned last and links to the rest via tb_next.
        let mut node = Value::none();

        // Innermost-last: walk the captured frames from the END (innermost)
        // toward the front (outermost callee).
        for fi in captured.iter().rev() {
            let lineno = fi.lineno.map(|n| n as i64).unwrap_or(0);
            let co = code_obj::code(fi.funcname.to_string(), 0, Vec::new());
            let frame = frame_obj::frame(
                co,
                lineno,
                Value::none(),
                self.module_globals_dict.clone(),
                Value::dict(Default::default()),
            );
            node = tb_obj::traceback_node(frame, node, lineno, -1);
        }

        // Finally, the catching frame as the outermost node.
        let catch_frame = frame_obj::frame(
            catch_code,
            catch_lineno,
            Value::none(),
            self.module_globals_dict.clone(),
            Value::dict(Default::default()),
        );
        tb_obj::traceback_node(catch_frame, node, catch_lineno, -1)
    }
}

/// Collect the distinct names referenced via `LoadGlobal` in `fncode` and,
/// recursively, in every nested code object (`fn_protos`).  A free variable
/// read only inside a comprehension/genexpr, `lambda`, or nested `def`
/// compiles its `LoadGlobal` into the nested body, so the enclosing
/// function's free-variable set must include those names too (issue #2106).
/// Names are de-duplicated via `seen`; the caller applies the env-resolution
/// filter that distinguishes a true free variable from a module global or a
/// nested body's own local.
fn collect_loadglobal_names(
    fncode: &crate::bytecode::FnCode,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    for insn in &fncode.insns {
        if let crate::bytecode::Insn::LoadGlobal(_, name_idx) = insn {
            if let Some(name) = fncode.names.get(*name_idx as usize) {
                if seen.insert(name.clone()) {
                    out.push(name.clone());
                }
            }
        }
    }
    for proto in &fncode.fn_protos {
        collect_loadglobal_names(&proto.code, seen, out);
    }
}
