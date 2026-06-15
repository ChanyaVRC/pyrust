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
        let co_name = function.name.to_string();
        // co_argcount: positional-only + positional-or-keyword params (excludes
        // *args/**kwargs and keyword-only), matching CPython.
        let argcount = function
            .params
            .iter()
            .filter(|p| !p.is_args && !p.is_kwargs && !p.is_keyword_only)
            .count() as i64;
        // co_posonlyargcount / co_kwonlyargcount (CPython 3.8+/3.0+).
        let posonlyargcount = function
            .params
            .iter()
            .filter(|p| p.is_positional_only)
            .count() as i64;
        let kwonlyargcount = function
            .params
            .iter()
            .filter(|p| p.is_keyword_only)
            .count() as i64;

        // co_varnames: CPython orders parameters as positional (positional-only
        // then positional-or-keyword), then keyword-only, then *args, then
        // **kwargs — followed by the function-body locals in source order.
        // Cell variables (locals captured by a nested scope) are reported in
        // co_cellvars, NOT co_varnames, so they are excluded here.
        let mut param_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut varnames: Vec<Value> = Vec::with_capacity(function.params.len());
        for p in function
            .params
            .iter()
            .filter(|p| !p.is_args && !p.is_kwargs && !p.is_keyword_only)
        {
            param_names.insert(p.name.as_str());
            varnames.push(Value::string(p.name.clone()));
        }
        for p in function.params.iter().filter(|p| p.is_keyword_only) {
            param_names.insert(p.name.as_str());
            varnames.push(Value::string(p.name.clone()));
        }
        for p in function.params.iter().filter(|p| p.is_args) {
            param_names.insert(p.name.as_str());
            varnames.push(Value::string(p.name.clone()));
        }
        for p in function.params.iter().filter(|p| p.is_kwargs) {
            param_names.insert(p.name.as_str());
            varnames.push(Value::string(p.name.clone()));
        }

        // co_qualname (CPython 3.11+): the compile-time qualified name.  Unlike
        // `__qualname__`, this is fixed at compile time and ignores any later
        // `f.__qualname__ = ...` user override.
        let qualname = function.qualname.to_string();

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

        // The remaining attributes come from the compiled `FnCode` when
        // available: co_firstlineno, co_consts, co_names, co_cellvars,
        // co_stacksize, the function-body locals appended to co_varnames, and
        // the CO_GENERATOR flag.  Downcast once and derive them all together.
        let fncode = function
            .precompiled_code
            .as_ref()
            .and_then(|rc| Rc::clone(rc).downcast::<crate::bytecode::FnCode>().ok());

        // co_filename: the source path the function's code object was compiled
        // from.  Read it from the code object (#2438) so an imported module's
        // function reports its module's file, not the importing script's; fall
        // back to the running script path only when no code object is available
        // (synthetic / builtin-shaped functions).
        let filename = fncode
            .as_ref()
            .map(|c| c.filename.to_string())
            .or_else(|| self.script_filename.as_ref().map(|s| s.to_string()))
            .unwrap_or_else(|| "<unknown>".to_string());

        // co_cellvars: cell variables this function defines (locals captured by
        // a nested scope), in CPython's sorted order.  Also used to exclude
        // those names from co_varnames.
        let cellvar_set: std::collections::HashSet<String> = fncode
            .as_ref()
            .map(|c| c.cell_vars.iter().cloned().collect())
            .unwrap_or_default();
        let cellvars: Vec<Value> = {
            let mut v: Vec<String> = cellvar_set.iter().cloned().collect();
            v.sort();
            v.into_iter().map(Value::string).collect()
        };

        // Body locals appended to co_varnames: every name in the function's
        // `local_index` that is not a parameter and not a cell variable, in
        // register-slot order (the compiler assigns slots in source-encounter
        // order, matching CPython's co_varnames body-local ordering; #2185).
        let mut body_locals: Vec<(u32, &str)> = function
            .local_index
            .iter()
            .filter(|(name, _)| {
                !param_names.contains(name.as_str()) && !cellvar_set.contains(name.as_str())
            })
            .map(|(name, &slot)| (slot, name.as_str()))
            .collect();
        body_locals.sort_by_key(|(slot, _)| *slot);
        for (_, name) in &body_locals {
            varnames.push(Value::string(name));
        }
        // co_nlocals == len(co_varnames).
        let nlocals = varnames.len() as i64;

        let mut firstlineno = 0i64;
        let mut consts: Vec<Value> = Vec::new();
        let mut names: Vec<Value> = Vec::new();
        let mut stacksize = 0i64;
        if let Some(fncode) = fncode.as_ref() {
            if fncode.is_generator {
                flags |= code_obj::CO_GENERATOR;
            }
            // co_firstlineno: the `def`/`lambda` line recorded by the compiler
            // (NOT the first body statement, which may be one or more lines
            // below for a multi-line signature; issue #2185).
            firstlineno = fncode.first_lineno as i64;
            // co_consts: CPython always reserves slot 0 for `None` (the implicit
            // `return None` value), followed by the body literals in order, with
            // `None` deduplicated to that single slot.  pyrust's pool holds only
            // the literals actually referenced (no implicit `None`), so prepend
            // a single `None` and drop any other `None` occurrence to match
            // (issue #2185).  (Docstring-carrying functions, where CPython uses
            // slot 0 for the docstring, are not reproduced — pyrust does not yet
            // extract docstrings into the const pool.)
            consts.push(Value::none());
            consts.extend(fncode.consts.iter().filter(|c| !c.is_none()).cloned());
            names = fncode.names.iter().map(|n| Value::string(n.clone())).collect();
            // co_stacksize: CPython's max operand-stack depth.  pyrust is a
            // register VM with no operand stack, so report the register count —
            // a positive int of the right type.
            stacksize = fncode.num_regs as i64;
        }

        // co_freevars: the names this function reads from an enclosing function
        // scope, in CPython's sorted order (issue #2106).  Reuses the same
        // free-variable analysis that powers `__closure__`.
        let freevars: Vec<Value> = self
            .closure_free_vars(function)
            .into_iter()
            .map(|(name, _)| Value::string(name))
            .collect();

        code_obj::CodeBuild {
            name: co_name,
            qualname,
            argcount,
            posonlyargcount,
            kwonlyargcount,
            nlocals,
            stacksize,
            varnames,
            flags,
            filename,
            firstlineno,
            consts,
            names,
            freevars,
            cellvars,
        }
        .build()
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
            self.module_globals_dict.clone(),
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
        let varnames: Vec<Value> = locals
            .iter()
            .map(|(_, n)| Value::string(n))
            .collect();
        let nlocals = varnames.len() as i64;

        let mut cellvars: Vec<String> = fncode.cell_vars.iter().cloned().collect();
        cellvars.sort();
        let cellvars: Vec<Value> = cellvars.into_iter().map(Value::string).collect();

        let mut consts: Vec<Value> = Vec::new();
        consts.push(Value::none());
        consts.extend(fncode.consts.iter().filter(|c| !c.is_none()).cloned());
        let names: Vec<Value> = fncode.names.iter().map(|n| Value::string(n.clone())).collect();

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

    /// Build the Python-visible traceback object chain for an exception that
    /// is being caught, from the lazily-captured `FrameInfo` snapshot plus the
    /// catching frame.  Returns a `traceback` node (the outermost) whose
    /// `tb_next` chain walks inward to the raise site, matching CPython's
    /// `__traceback__` ordering.
    ///
    /// `catch_lineno` is the source line in the catching frame where the
    /// exception propagated from (the call/raise that is being handled).
    /// Build the Python-visible traceback chain from an already-captured
    /// `FrameInfo` snapshot plus the catching frame's `UserFunction`.
    ///
    /// Invoked by [`Self::materialize_deferred_traceback`] the first time
    /// `e.__traceback__` is read.  Building the chain is the dominant cost of the
    /// raise/catch path — the `build_code_object` for the catch frame plus a
    /// `frame` + two dicts per node — so issue #2351 defers it from catch time to
    /// first read.  `catch_func` is `None` for a module-scope (`<module>`)
    /// catching frame.
    pub(crate) fn build_traceback_from_snapshot(
        &self,
        captured: &[pyrust_core::FrameInfo],
        catch_func: Option<&UserFunction>,
        catch_lineno: i64,
        tail: Value,
    ) -> Value {
        // Determine the catching frame's code object.
        let catch_code = match catch_func {
            Some(func) => self.build_code_object(func),
            None => code_obj::code("<module>".to_string(), 0, Vec::new()),
        };

        // Build the chain from innermost (tb_next == None) outward, so the
        // outermost node is returned last and links to the rest via tb_next.
        //
        // `tail` is the already-materialised chain that this snapshot's frames
        // are *prepended* onto (issue #2367): when an exception that already
        // carries a traceback is re-raised, CPython prepends the re-raising
        // frame(s) and links the new innermost node's `tb_next` to the old
        // chain — which stays as the tail, same objects (identity contract).
        // For a fresh catch the tail is `None`.
        let mut node = tail;

        // Innermost-last: walk the captured frames from the END (innermost)
        // toward the front (outermost callee).
        for fi in captured.iter().rev() {
            let lineno = fi.lineno.map(|n| n as i64).unwrap_or(0);
            // Carry the frame's own source filename into `f_code.co_filename`
            // (#2438): a reconstructed traceback frame for an imported module's
            // function must report that module's file, not `<unknown>`.
            let co = code_obj::code_with_loc(
                fi.funcname.to_string(),
                0,
                Vec::new(),
                fi.filename.to_string(),
                0,
            );
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

    /// Build a *deferred* traceback placeholder for an exception being caught.
    ///
    /// Instead of eagerly materialising the (expensive) `traceback` object
    /// chain — which builds a full `code` object for the catching frame plus
    /// a `frame` and two dicts per node, none of which the overwhelming
    /// majority of `try/except` blocks ever read — this captures only the
    /// cheap snapshot the build needs (the `FrameInfo` list, the catching
    /// frame's `UserFunction` `Rc`, and the catch line) and returns a
    /// lightweight placeholder value.
    ///
    /// The first read of `e.__traceback__` materialises the real chain via
    /// [`Self::materialize_deferred_traceback`] and replaces the placeholder, so
    /// the Python-visible behaviour is identical to the eager build (issue
    /// #2351).
    pub(crate) fn build_deferred_traceback(&self, catch_lineno: i64) -> Value {
        self.build_deferred_traceback_with_tail(catch_lineno, Value::none())
    }

    /// Like [`Self::build_deferred_traceback`] but the materialised chain will be
    /// *prepended* onto `tail` (issue #2367).  `tail` is the existing traceback
    /// carried by a re-raised exception — either a real `traceback` chain or a
    /// still-deferred placeholder; it is materialised at read time so the new
    /// innermost node's `tb_next` is a real node with stable identity, matching
    /// CPython's prepend-and-reuse-tail behaviour.
    pub(crate) fn build_deferred_traceback_with_tail(
        &self,
        catch_lineno: i64,
        tail: Value,
    ) -> Value {
        self.build_deferred_traceback_with_tail_impl(catch_lineno, tail, false)
    }

    /// Like [`Self::build_deferred_traceback_with_tail`] but for a **bare**
    /// `raise` (issue #2405).  A bare re-raise re-raises the active exception
    /// *without* adding a traceback node for the re-raising frame itself —
    /// CPython keeps the carried chain and only prepends the genuinely-outer
    /// frames the exception unwinds through *after* the re-raise.  The
    /// re-raising frame is the innermost of the freshly-captured unwind frames
    /// (`captured.last()`, recorded first as the error propagated out), so it
    /// is dropped here; the remaining outer frames prepend onto `tail`.
    pub(crate) fn build_deferred_traceback_with_tail_drop_innermost(
        &self,
        catch_lineno: i64,
        tail: Value,
    ) -> Value {
        self.build_deferred_traceback_with_tail_impl(catch_lineno, tail, true)
    }

    fn build_deferred_traceback_with_tail_impl(
        &self,
        catch_lineno: i64,
        tail: Value,
        drop_innermost: bool,
    ) -> Value {
        let mut captured = pyrust_core::clone_captured_error_frames();
        // Bare re-raise (#2405): the innermost captured frame is the re-raising
        // frame itself, which CPython does not give its own traceback node.
        if drop_innermost && !captured.is_empty() {
            captured.pop();
        }
        let catch_func = self
            .vm_frame_views
            .last()
            .and_then(|view| view.function.clone());
        let state: Box<dyn std::any::Any> = Box::new(DeferredTracebackState {
            frames: captured,
            catch_func,
            catch_lineno,
            tail,
        });
        Value::builtin_object(DEFERRED_TRACEBACK_OPS, state)
    }

    /// If `value` is a deferred-traceback placeholder, materialise it into a real
    /// traceback object chain; otherwise return `None`.  Used by every read site
    /// that may observe an exception's `__traceback__` slot.
    pub(crate) fn materialize_deferred_traceback(&self, value: &Value) -> Option<Value> {
        let ValueKind::BuiltinObject { ops, state } = value.kind() else {
            return None;
        };
        if ops.type_name() != DEFERRED_TRACEBACK_NAME {
            return None;
        }
        // Clone out the snapshot fields, then drop the borrow before recursing
        // into the tail (which may itself be a deferred placeholder sharing no
        // lock, but keeping the borrow narrow is cleaner).
        let (frames, catch_func, catch_lineno, tail) = {
            let borrow = state.borrow();
            let s = borrow.downcast_ref::<DeferredTracebackState>()?;
            (
                s.frames.clone(),
                s.catch_func.clone(),
                s.catch_lineno,
                s.tail.clone(),
            )
        };
        // Materialise the carried tail first (issue #2367) so the prepended
        // frames link to a real chain with stable identity.  A `None` tail (the
        // common fresh-catch case) materialises to itself.
        let tail = self.materialize_deferred_traceback(&tail).unwrap_or(tail);
        Some(self.build_traceback_from_snapshot(
            &frames,
            catch_func.as_deref(),
            catch_lineno,
            tail,
        ))
    }

    /// When `exc` is being re-raised by an explicit `raise e` /
    /// `raise e.with_traceback(tb)` and already carries a traceback, reset the
    /// captured unwind-frame snapshot (issue #2367).
    ///
    /// CPython keeps the carried traceback chain and *prepends* the frames the
    /// re-raise newly unwinds through.  pyrust models that by treating the
    /// carried chain as the tail at the next catch site and rebuilding the
    /// prefix from frames captured *after* this point — so the stale frames of
    /// the original raise must be cleared here, otherwise they would be counted
    /// twice (once in the carried tail, once in the freshly-rebuilt prefix).
    pub(crate) fn reset_captured_frames_if_reraise(&self, exc: &Value) {
        let ValueKind::PyInstance(inst) = exc.kind() else {
            return;
        };
        let has_tb = inst
            .borrow()
            .attrs
            .get("__traceback__")
            .is_some_and(|tb| !tb.is_none());
        if has_tb {
            pyrust_core::reset_captured_error_frames();
            return;
        }
        // Issue #2407: a *fresh* exception (no carried `__traceback__`) raised
        // while another exception is being handled (`handled_exc_stack` is
        // non-empty — the same condition that drives implicit-context chaining
        // in `attach_implicit_context`) must start from an empty unwind-frame
        // snapshot.  Otherwise the stale frames captured while the *handled*
        // exception unwound to this catch site would prepend onto the new
        // exception's traceback, producing a spurious trailing frame (the
        // `f`-frame of the in-flight exception) in both the Python-visible
        // `__traceback__` chain and the uncaught stderr formatter.  The new
        // exception's own unwind frames are recorded *after* this point as it
        // propagates out, so resetting here loses nothing.
        if !self.handled_exc_stack.is_empty() {
            pyrust_core::reset_captured_error_frames();
        }
    }

    /// True when `value` is a deferred-traceback placeholder (not yet
    /// materialised).
    pub(crate) fn is_deferred_traceback(value: &Value) -> bool {
        matches!(
            value.kind(),
            ValueKind::BuiltinObject { ops, .. } if ops.type_name() == DEFERRED_TRACEBACK_NAME
        )
    }

    /// Compute the `__traceback__` value to store for an exception being caught
    /// at `catch_lineno`, given whatever the exception's `__traceback__` slot
    /// currently holds (`existing`).
    ///
    /// Three cases (issue #2367):
    ///  * `existing` is `None` — a *fresh* exception; build a deferred chain
    ///    from the captured unwind frames (the hot path).
    ///  * `existing` already represents the chain *this same frame* built (the
    ///    `with`/`__exit__` same-frame identity case from issue #2359/#2366):
    ///    return `None` so the caller keeps the existing object unchanged.
    ///  * `existing` holds a carried/re-raised chain from another frame: build a
    ///    new deferred chain whose materialised frames are *prepended* onto the
    ///    existing chain, so CPython's "prepend the re-raising frame, keep the
    ///    old tail" behaviour is reproduced.
    ///
    /// `is_bare_reraise` is set when the in-flight exception reached this catch
    /// via a *bare* `raise` (issue #2405): bare re-raise keeps the carried chain
    /// and prepends only the genuinely-outer frames the exception unwound
    /// through after the re-raise, never adding a node for the re-raising frame
    /// itself (CPython's `raise` semantics).
    ///
    /// Returns `Some(new_value)` to store, or `None` to leave the slot untouched.
    pub(crate) fn caught_traceback_value(
        &self,
        existing: &Value,
        catch_lineno: i64,
        is_bare_reraise: bool,
    ) -> Option<Value> {
        // Fresh exception (slot still the pre-initialised `None`): plain build.
        if existing.is_none() {
            return Some(self.build_deferred_traceback(catch_lineno));
        }
        // Slot already carries a chain (real traceback or deferred placeholder).
        let is_real = pyrust_builtins::traceback::is_traceback(existing);
        let is_deferred = Self::is_deferred_traceback(existing);
        if !is_real && !is_deferred {
            // Some non-traceback value (shouldn't normally happen, but be safe):
            // overwrite with a fresh build, matching the historical behaviour.
            return Some(self.build_deferred_traceback(catch_lineno));
        }
        // Bare `raise` carrying a chain (issue #2405).  The bare re-raise reset
        // the captured-frame snapshot at the raise site (`reset_captured_frames`
        // in `RaiseReRaise`), so the captured frames now hold *only* the frames
        // the exception unwound through *after* the re-raise.  CPython keeps the
        // carried chain unchanged and prepends those outer frames — never adding
        // a node for the re-raising frame itself.
        //  * Caught in the *same* frame (no unwind): nothing was captured — keep
        //    the carried chain exactly (this also preserves the `with`/`__exit__`
        //    same-frame identity contract from #2359/#2366, which re-raises via
        //    the bare form).
        //  * Propagated to an outer frame: prepend the captured frames minus the
        //    innermost (the re-raising frame's own node) onto the carried chain.
        if is_bare_reraise {
            if pyrust_core::captured_error_frames_len() == 0 {
                return None;
            }
            return Some(
                self.build_deferred_traceback_with_tail_drop_innermost(
                    catch_lineno,
                    existing.clone(),
                ),
            );
        }
        // Same-frame identity (issue #2359/#2366): a *materialised* chain whose
        // length matches the frames captured for *this* catch was built in this
        // very frame; keep it so the object an inner `with`/`except` saw is
        // identical to the one this `except` reads.  Re-deferring or prepending
        // would mint a distinct head and break that contract.
        if is_real
            && pyrust_builtins::traceback::chain_len(existing)
                == pyrust_core::captured_error_frames_len() + 1
        {
            return None;
        }
        // Explicit `raise e` / `raise e.with_traceback(...)` carried / re-raised
        // across a frame boundary: prepend the new frames onto the existing
        // chain, keeping the old chain as the tail (issue #2367).
        Some(self.build_deferred_traceback_with_tail(catch_lineno, existing.clone()))
    }

    /// Derive the uncaught-exception traceback's *inner* frame list (everything
    /// below the `<module>` frame) from `exc`'s prepended `__traceback__` chain
    /// (issue #2404).
    ///
    /// The stderr formatter normally rebuilds its frame list from the captured
    /// unwind-frame snapshot, but after #2367/#2403 a re-raised exception's
    /// `__traceback__` chain is the authoritative, Python-visible carried frame
    /// list — the snapshot was reset at the re-raise site and so diverges.  When
    /// `exc` carries a `__traceback__` chain, the printed list is:
    ///
    ///   `snapshot_prefix` (the frames the exception unwound through *after* the
    ///   re-raise, captured fresh) ++ the carried `__traceback__` chain.
    ///
    /// This replays at the top level the prepend a hypothetical module-scope
    /// `except` would have done — the re-raise's finalising catch never runs for
    /// an uncaught exception, so the chain itself lacks the re-raise frame's
    /// node; the snapshot supplies it.  `is_bare` drops the innermost snapshot
    /// frame (the bare re-raise's own line gets no node — #2405), mirroring
    /// `caught_traceback_value`.
    ///
    /// Returns `None` for a raw `PyError` variant or a never-caught exception
    /// (`__traceback__` still `None`), so those keep the snapshot path unchanged.
    ///
    /// `filename` is the script path (the tb nodes carry no per-frame filename —
    /// a single-file program shares one); `src` is the script source for
    /// source-line lookup; `snapshot` is the freshly-captured unwind frames
    /// (outermost-first).  The chain is already ordered outermost-first.
    pub(crate) fn uncaught_inner_frames_from_tb(
        &self,
        exc: &Value,
        filename: &std::sync::Arc<str>,
        src: &str,
        snapshot: &[pyrust_core::FrameInfo],
        is_bare: bool,
    ) -> Option<Vec<pyrust_core::FrameInfo>> {
        let ValueKind::PyInstance(inst) = exc.kind() else {
            return None;
        };
        let stored = inst.borrow().attrs.get("__traceback__").cloned();
        let stored = stored.filter(|tb| !tb.is_none())?;
        // Materialise the deferred placeholder if needed (cold uncaught path).
        let tb = self
            .materialize_deferred_traceback(&stored)
            .unwrap_or(stored);
        let nodes = pyrust_builtins::traceback::walk_frames(&tb);
        if nodes.is_empty() {
            return None;
        }
        // Source-line lookup: CPython echoes the line stripped of leading
        // indentation and re-indented to a fixed four spaces (the prefix is
        // added by `format_traceback`), so store the fully-stripped text.
        let resolve = |lineno: Option<u32>| -> Option<std::sync::Arc<str>> {
            let n = lineno?;
            if src.is_empty() {
                return None;
            }
            src.lines()
                .nth((n as usize).saturating_sub(1))
                .map(|l| std::sync::Arc::from(l.trim()))
        };
        let mut frames: Vec<pyrust_core::FrameInfo> = Vec::with_capacity(snapshot.len() + nodes.len());
        // Prefix: the frames the exception unwound through after the re-raise.
        // For a bare re-raise the innermost (the re-raise's own frame) gets no
        // node, mirroring the catch-site `drop_innermost`.
        let prefix_end = if is_bare && !snapshot.is_empty() {
            snapshot.len() - 1
        } else {
            snapshot.len()
        };
        for fi in &snapshot[..prefix_end] {
            // The snapshot lacks per-frame source text; resolve it from `src`.
            frames.push(pyrust_core::FrameInfo {
                filename: filename.clone(),
                lineno: fi.lineno,
                source_line: resolve(fi.lineno),
                funcname: fi.funcname.clone(),
                col_span: None,
            });
        }
        // The carried chain (outermost-first).
        for (funcname, lineno) in nodes {
            let lineno = if lineno > 0 { Some(lineno as u32) } else { None };
            frames.push(pyrust_core::FrameInfo {
                filename: filename.clone(),
                lineno,
                source_line: resolve(lineno),
                funcname: std::sync::Arc::from(&funcname[..]),
                col_span: None,
            });
        }
        Some(frames)
    }
}

/// Internal type name for the deferred-traceback placeholder.  Not user-visible
/// (every read path materialises the placeholder before it can be inspected),
/// but distinct from the real `"traceback"` type so the materialisation
/// interceptor can recognise it.
pub(crate) const DEFERRED_TRACEBACK_NAME: &str = "<deferred traceback>";
pub(crate) const DEFERRED_TRACEBACK_OPS: &DeferredTracebackOps = &DeferredTracebackOps;

/// Cheap snapshot carried by a deferred-traceback placeholder until the
/// traceback object is first read (issue #2351).
pub(crate) struct DeferredTracebackState {
    frames: Vec<pyrust_core::FrameInfo>,
    catch_func: Option<Rc<UserFunction>>,
    catch_lineno: i64,
    /// Existing traceback chain this snapshot's frames are prepended onto when
    /// materialised (issue #2367).  `None` for a fresh catch; a real or deferred
    /// traceback for a re-raised / carried exception.
    tail: Value,
}

pub(crate) struct DeferredTracebackOps;

impl pyrust_core::BuiltinTypeOps for DeferredTracebackOps {
    fn type_name(&self) -> &'static str {
        DEFERRED_TRACEBACK_NAME
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
        // A free-variable read is `LoadGlobal` for a true global / module-scope
        // capture, or `LoadCell` for a function-scope cell / `nonlocal` (issue
        // #2339).  Both must feed the candidate set so `__closure__` /
        // `co_freevars` still see cell reads now routed through `LoadCell`.
        let name_idx = match insn {
            crate::bytecode::Insn::LoadGlobal(_, idx)
            | crate::bytecode::Insn::LoadCell(_, idx) => *idx,
            _ => continue,
        };
        if let Some(name) = fncode.names.get(name_idx as usize)
            && seen.insert(name.clone())
        {
            out.push(name.clone());
        }
    }
    for proto in &fncode.fn_protos {
        collect_loadglobal_names(&proto.code, seen, out);
    }
}
