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
        let kwonlyargcount = function.params.iter().filter(|p| p.is_keyword_only).count() as i64;

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
            names = fncode
                .names
                .iter()
                .map(|n| Value::string(n.clone()))
                .collect();
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
}
