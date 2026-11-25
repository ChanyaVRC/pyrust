// Frame / traceback / code introspection objects (issues #2170, #2171).
//
// These build the Python-visible `frame`, `traceback`, and (full) `code`
// objects from the data the VM already tracks: the `vm_frame_views` stack
// (issue #389), the lazily-captured traceback `FrameInfo` chain (issue #2165),
// and each function's compiled `FnCode`.  Everything here runs off the hot
// path — only when user code explicitly reaches for `sys._getframe()`,
// `f.__code__`, `e.__traceback__`, or `sys.exc_info()`.

use pyrust_builtins::{code as code_obj, frame as frame_obj, traceback as tb_obj};

/// Derive CPython-style `(start, end, lineno)` line-table entries and per-
/// instruction `(lineno, end_lineno, col_offset, end_col_offset)` position
/// entries from a compiled [`crate::bytecode::FnCode`].
pub(crate) fn build_code_lines_and_positions(
    fncode: &crate::bytecode::FnCode,
    firstlineno: i64,
) -> (
    Vec<code_obj::CodeLineEntry>,
    Vec<code_obj::CodePositionEntry>,
) {
    let n = fncode.insns.len();
    let mut lines = Vec::new();
    if n > 0 {
        let runs = fncode.lineno_table.runs();
        if runs.is_empty() {
            lines.push((0, (n * 2) as i64, (firstlineno > 0).then_some(firstlineno)));
        } else {
            let first_pc = (runs[0].0 as usize).min(n);
            if first_pc > 0 {
                lines.push((
                    0,
                    (first_pc * 2) as i64,
                    (firstlineno > 0).then_some(firstlineno),
                ));
            }
            for i in 0..runs.len() {
                let start_pc = (runs[i].0 as usize).min(n);
                let end_pc = runs
                    .get(i + 1)
                    .map_or(n, |&(next_pc, _)| (next_pc as usize).min(n));
                if start_pc < end_pc {
                    let ln = runs[i].1;
                    lines.push((
                        (start_pc * 2) as i64,
                        (end_pc * 2) as i64,
                        (ln > 0).then_some(ln as i64),
                    ));
                }
            }
        }
    }

    let mut positions = Vec::with_capacity(n);
    for pc in 0..n {
        let ln = fncode.line_at(pc);
        let ln_opt = if ln != 0 {
            Some(ln as i64)
        } else if firstlineno > 0 {
            Some(firstlineno)
        } else {
            None
        };
        if let Some((full_start, _, _, full_end)) = fncode.col_at(pc) {
            let col = Some(full_start as i64);
            let end_col = if full_end == crate::ast::MULTILINE_FULL_END || full_end == 0 {
                None
            } else {
                Some(full_end as i64)
            };
            positions.push((ln_opt, ln_opt, col, end_col));
        } else {
            positions.push((ln_opt, ln_opt, None, None));
        }
    }
    (lines, positions)
}

impl Interpreter {
    /// Build a full `code` object for `function`, populating the best-effort
    /// `co_*` attributes (`co_flags`, `co_filename`, `co_firstlineno`,
    /// `co_consts`, `co_names`) in addition to `co_name`/`co_argcount`/
    /// `co_varnames`.
    pub(crate) fn build_code_object(&self, function: &UserFunction) -> Value {
        let fncode = function
            .precompiled_code
            .as_ref()
            .and_then(|rc| Rc::clone(rc).downcast::<crate::bytecode::FnCode>().ok());
        // Reuse the weakly cached code object when available so `f.__code__`,
        // `g.gi_code`, and `frame.f_code` share Python identity (issue #2980).
        // Skip the cache for `types.coroutine`-wrapped functions whose
        // `CO_ITERABLE_COROUTINE` flag can diverge per function wrapper.
        let cacheable = !function.iterable_coroutine.get();
        if cacheable
            && let Some(fc) = fncode.as_ref()
            && let Some(cached) = fc
                .cached_code_object
                .borrow()
                .as_ref()
                .and_then(pyrust_core::WeakValueCache::upgrade)
        {
            return cached;
        }
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
            varnames.push(Value::string(p.name.as_str()));
        }
        for p in function.params.iter().filter(|p| p.is_keyword_only) {
            param_names.insert(p.name.as_str());
            varnames.push(Value::string(p.name.as_str()));
        }
        for p in function.params.iter().filter(|p| p.is_args) {
            param_names.insert(p.name.as_str());
            varnames.push(Value::string(p.name.as_str()));
        }
        for p in function.params.iter().filter(|p| p.is_kwargs) {
            param_names.insert(p.name.as_str());
            varnames.push(Value::string(p.name.as_str()));
        }

        // co_qualname (CPython 3.11+): the compile-time qualified name.  Unlike
        // `__qualname__`, this is fixed at compile time and ignores any later
        // `f.__qualname__ = ...` user override (#2981).
        let qualname = fncode
            .as_ref()
            .map(|c| c.qualname.to_string())
            .filter(|q| !q.is_empty() && q != "<module>")
            .unwrap_or_else(|| function.qualname.to_string());

        // co_flags: CPython sets CO_OPTIMIZED | CO_NEWLOCALS for every normal
        // function, plus the signature and generator/coroutine flags the
        // function warrants.
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
        // the generator/coroutine flags.

        // co_filename: the source path the function's code object was compiled
        // from.  Read it from the code object (#2438) so an imported module's
        // function reports its module's file, not the importing script's; fall
        // back to the running script path only when no code object is available
        // (synthetic / builtin-shaped functions).
        let filename = fncode
            .as_ref()
            .map(|c| c.filename.to_string())
            .or_else(|| self.cold.script_filename.as_ref().map(|s| s.to_string()))
            .unwrap_or_else(|| "<unknown>".to_string());

        // co_cellvars: cell variables this function defines (locals captured by
        // a nested scope), already sorted in `FnCode::own_cell_slots()`.
        let cellvar_set: std::collections::HashSet<pyrust_core::Symbol> = fncode
            .as_ref()
            .map(|c| c.own_cell_slots().iter().map(|slot| slot.name).collect())
            .unwrap_or_default();
        let cellvars: Vec<Value> = fncode
            .as_ref()
            .map(|c| {
                c.own_cell_slots()
                    .iter()
                    .map(|slot| Value::string(slot.name.as_str()))
                    .collect()
            })
            .unwrap_or_default();

        // Body locals appended to co_varnames: every name in the function's
        // `local_index` that is not a parameter and not a cell variable, in
        // register-slot order (the compiler assigns slots in source-encounter
        // order, matching CPython's co_varnames body-local ordering; #2185).
        let mut body_locals: Vec<(u32, &str)> = function
            .local_index
            .iter()
            .filter(|(name, _)| {
                !param_names.contains(name.as_str())
                    && !cellvar_set.contains(&pyrust_core::Symbol::intern(name.as_str()))
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
            match (fncode.is_coroutine, fncode.is_generator) {
                (true, true) => flags |= code_obj::CO_ASYNC_GENERATOR,
                (true, false) => flags |= code_obj::CO_COROUTINE,
                (false, true) => flags |= code_obj::CO_GENERATOR,
                (false, false) => {}
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
                .map(|n| Value::string(n.as_str()))
                .collect();
            // co_stacksize: CPython's max operand-stack depth.  pyrust is a
            // register VM with no operand stack, so report the register count —
            // a positive int of the right type.
            stacksize = fncode.num_regs as i64;
        }
        if function.iterable_coroutine.get() {
            flags |= code_obj::CO_ITERABLE_COROUTINE;
        }

        // co_freevars: the names this function captures from an enclosing scope,
        // in CPython's sorted order (issues #2106 / #452).
        let freevars: Vec<Value> = self
            .closure_free_var_cells(function)
            .into_iter()
            .map(|(name, _)| Value::string(name))
            .collect();

        let (lines, positions) = match fncode.as_ref() {
            Some(fc) => build_code_lines_and_positions(fc, firstlineno),
            None => (Vec::new(), Vec::new()),
        };
        let linetable = code_obj::encode_linetable(&lines, firstlineno);

        let code = code_obj::CodeBuild {
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
            code_bytes: Vec::new(),
            linetable,
            exceptiontable: Vec::new(),
            lines,
            positions,
            raw_code: fncode
                .as_ref()
                .map(|c| Rc::clone(c) as Rc<dyn std::any::Any>),
            exec_meta: None,
        }
        .build();
        if cacheable && let Some(fc) = fncode.as_ref() {
            *fc.cached_code_object.borrow_mut() = pyrust_core::WeakValueCache::new(&code);
        }
        code
    }

    /// Return the function's free variables paired with their live [`CellSlot`]s
    /// in CPython's sorted `co_freevars` order (issues #2106 / #452).
    ///
    /// Enclosing-function captures come directly from `fncode.free_var_slots()`
    /// zipped with `function.closure_cells` (live, mutable, including empty
    /// cells). When the function was defined inside a non-root `Namespace`
    /// (class body for `__class__` or PEP 695 `TypeParamEnv` for type params),
    /// any non-cell free candidate resolved from that namespace is merged in.
    pub(crate) fn closure_free_var_cells(
        &self,
        function: &UserFunction,
    ) -> Vec<(String, pyrust_core::CellSlot)> {
        let Some(rc) = function.precompiled_code.as_ref() else {
            return Vec::new();
        };
        let Ok(fncode) = Rc::clone(rc).downcast::<crate::bytecode::FnCode>() else {
            return Vec::new();
        };

        let mut found: Vec<(String, pyrust_core::CellSlot)> = fncode
            .free_var_slots()
            .iter()
            .zip(function.closure_cells.iter())
            .map(|(meta, slot)| (meta.name.to_string(), slot.clone()))
            .collect();

        if function.env.borrow().parent.is_some() {
            let mut added_namespace_var = false;
            for &name in fncode.free_var_candidates() {
                let name_str = name.as_str();
                if function.global_names.contains(name_str)
                    || function.local_names.contains(name_str)
                    || found.iter().any(|(existing, _)| existing == name_str)
                {
                    continue;
                }
                if let Some(value) = lookup_enclosing_function_value(&function.env, name_str) {
                    found.push((name_str.to_owned(), pyrust_core::CellSlot::new(value)));
                    added_namespace_var = true;
                }
            }
            if added_namespace_var {
                found.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
        found
    }

    /// Build `function.__closure__`: a tuple of live `cell` objects, one per
    /// free variable (in `co_freevars` order), or `None` for a function with no
    /// free variables (issues #2106 / #452).
    pub(crate) fn build_closure(&self, function: &UserFunction) -> Value {
        let cells: Vec<Value> = self
            .closure_free_var_cells(function)
            .into_iter()
            .map(|(_, slot)| pyrust_builtins::cell::cell(slot))
            .collect();
        if cells.is_empty() {
            Value::none()
        } else {
            Value::tuple(cells)
        }
    }
}
