use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use smallvec::SmallVec;

use crate::ast::{
    AssignTarget, BinaryOp, CallArg, CompClause, DictItem, Expr, FStringPart, FunctionParam,
    MatchArm, Pattern, Stmt, TypeParam, TypeParamBound, UnaryOp,
};
use crate::bytecode::{
    AttrCacheEntry, BinOpCacheEntry, CellVar, FnCode, FnParamSpec, FnProto, GLOBAL_CACHE_EMPTY,
    Insn, KwCallCacheEntry, Reg,
};
use crate::error::PyError;
use crate::interpreter::dispatch_numeric_binop;
use crate::value::{PyBigInt, Value, ValueKind};

/// Compile a top-level script / module body.  All script-level names are
/// locals.
///
/// When `repl_mode` is true, top-level `Stmt::Expr` statements emit
/// `Insn::PrintExpr` instead of discarding the result.
///
/// `linenos` is an optional parallel slice of 1-based source line numbers for
/// each top-level statement in `stmts`.  Pass an empty slice when no line
/// information is available (the resulting `FnCode::lineno_table` will be all
/// zeros).  They are threaded into the bytecode `lineno_table` so tracebacks
/// report accurate lines.
pub fn compile_script_with_linenos(
    stmts: &[Stmt],
    local_index: Rc<HashMap<String, Reg>>,
    repl_mode: bool,
    linenos: &[u32],
    filename: &str,
) -> Result<FnCode, PyError> {
    // Validate global/nonlocal ordering at module scope.  CPython 3.12 raises
    // SyntaxError for `x = 1; global x` at module level too.
    if let Some(msg) = crate::interpreter::check_global_nonlocal_order(stmts) {
        return Err(PyError::Named("SyntaxError".into(), msg));
    }
    // Script-level code cannot have nonlocal, and nothing captures script
    // locals via nonlocal from a nested scope at this level.
    let cell_vars = collect_cell_vars(stmts, &local_index);
    let mut c = Compiler::new(local_index, 0, cell_vars);
    // Threaded source file (#2438): the module's code object and every nested
    // function/class it compiles report this path as their `co_filename`.
    c.filename = std::sync::Arc::from(filename);
    // Issue #820: module-scope stores emit SyncModuleGlobal to keep
    // module_globals_dict live after globals() has been called.
    c.is_module_scope = true;
    // Issue #711: if the first statement is a bare string literal and we are
    // compiling a script file (not the REPL), it is the module docstring.
    // Emit a StoreGlobal for `__doc__` (CPython parity) before compiling the
    // rest.  In REPL mode every string-expression is just a value expression
    // whose repr is printed; it is NOT a module docstring (CPython's interactive
    // console does not set __doc__ from string literals typed interactively).
    let (body, body_linenos): (&[Stmt], &[u32]) = if !repl_mode {
        match stmts {
            [Stmt::Expr(Expr::Str(s)), rest @ ..] => {
                // Record lineno of the docstring statement if available.
                if let Some(&ln) = linenos.first()
                    && ln != 0
                {
                    c.set_lineno(ln);
                }
                let r = c.compile_literal(Value::string(s.clone()));
                c.compile_store_name("__doc__", r);
                c.free_temp(r);
                (rest, linenos.get(1..).unwrap_or(&[]))
            }
            _ => (stmts, linenos),
        }
    } else {
        (stmts, linenos)
    };
    if repl_mode {
        for (idx, stmt) in body.iter().enumerate() {
            if let Some(&ln) = body_linenos.get(idx)
                && ln != 0
            {
                c.set_lineno(ln);
            }
            if let Stmt::Expr(e) = stmt {
                let r = c.compile_expr(e);
                c.emit(Insn::PrintExpr(r));
                c.free_temp(r);
            } else {
                c.compile_stmt(stmt);
            }
        }
    } else {
        c.compile_block_with_linenos(body, body_linenos);
    }
    c.finish()
}

/// Compile a source body in eval mode: the body must consist of a single
/// expression statement; its value is returned.  Used by `eval()`.
///
/// Raises `SyntaxError` if the body is empty or contains statements that are
/// not a bare expression.
/// Seeds the expression's source line number into the bytecode line table, so
/// an error raised while evaluating an `eval()`'d expression reports the
/// correct internal line (issue #2245).
pub fn compile_eval_expr_with_linenos(
    stmts: &[Stmt],
    local_index: Rc<HashMap<String, Reg>>,
    linenos: &[u32],
    filename: &str,
) -> Result<FnCode, PyError> {
    let expr = match stmts {
        [Stmt::Expr(e)] => e,
        [] => {
            return Err(PyError::named(
                "SyntaxError",
                "eval() requires a non-empty expression".to_string(),
            ));
        }
        _ => {
            return Err(PyError::named(
                "SyntaxError",
                "eval() argument must be a single expression".to_string(),
            ));
        }
    };
    let cell_vars = collect_cell_vars(stmts, &local_index);
    let mut c = Compiler::new(local_index, 0, cell_vars);
    // Threaded source file (#2438): the eval expression's `co_filename`.
    c.filename = std::sync::Arc::from(filename);
    if let Some(&ln) = linenos.first()
        && ln != 0
    {
        c.set_lineno(ln);
    }
    let r = c.compile_expr(expr);
    let r = c.ensure_temp(r);
    c.emit(Insn::Return(r));
    c.free_temp(r);
    c.finish()
}

// ─── Cell-variable collection ─────────────────────────────────────────────────

/// Collect names from `local_index` that are referenced as `nonlocal` in any
/// directly nested `Stmt::Def` body.  These must be stored in the env (not
/// registers) so that inner closures can share them.
fn collect_cell_vars(body: &[Stmt], local_index: &HashMap<String, Reg>) -> Vec<CellVar> {
    let mut cells: HashSet<String> = HashSet::new();
    collect_cell_vars_in(body, local_index, false, &mut cells);
    collect_lambda_captures(body, local_index, false, &mut cells);
    cells.into_iter().collect()
}

/// Like `collect_cell_vars` but called when `local_index` is a class body's
/// register map.  A `global x` declaration in a method does not promote the
/// class-body name `x` to a cell var: methods access module globals directly
/// and do not close over the class body scope (issue #624).
fn collect_cell_vars_for_class_body(
    body: &[Stmt],
    local_index: &HashMap<String, Reg>,
) -> Vec<CellVar> {
    let mut cells: HashSet<String> = HashSet::new();
    collect_cell_vars_in(body, local_index, true, &mut cells);
    collect_lambda_captures(body, local_index, true, &mut cells);
    cells.into_iter().collect()
}

/// `is_class_scope`: when true, the names in `local_index` belong to a class
/// body.  In that case, a `global x` declaration in a directly-nested method
/// must **not** promote `x` to a cell var here — Python class scope is not a
/// closure scope for methods, so their `global` declarations bypass the class
/// namespace entirely and go straight to the module environment.
fn collect_cell_vars_in(
    body: &[Stmt],
    local_index: &HashMap<String, Reg>,
    is_class_scope: bool,
    cells: &mut HashSet<String>,
) {
    for stmt in body {
        match stmt {
            Stmt::Def {
                params,
                body: nested_body,
                ..
            } => {
                // Explicit `nonlocal x` in nested body → x is a cell var.
                // Use `collect_nonlocal_names_through_classes` so that a
                // `nonlocal x` declared inside a class body inside this nested
                // function is also seen (issue #735: class scope is transparent
                // to `nonlocal`, so `def inner(): class C: nonlocal x` still
                // requires `outer` to promote `x` to a cell var).
                let mut nonlocals = HashSet::new();
                collect_nonlocal_names_through_classes(nested_body, &mut nonlocals);
                for name in &nonlocals {
                    if local_index.contains_key(name) {
                        cells.insert(name.clone());
                    }
                }
                // Free variable references: names read in the nested body that are
                // not the nested function's own locals or globals.
                let inner_globals = crate::interpreter::collect_global_names(nested_body);
                // Names declared `global` in the nested function (or any transitively
                // nested function within it) reference the module env directly.  If
                // those names are fastlocals at the current scope, promote them to cell
                // vars so they live in env.values rather than registers.  Without this,
                // a doubly-nested `global x` would leave `x` as a fastlocal register at
                // module scope: `StoreGlobal` would write to env.values but
                // `LoadGlobal` inside the nested function would find nothing there (#520).
                //
                // Exception: when the current scope is a class body (`is_class_scope`),
                // skip this promotion.  A method's `global x` refers to the module
                // global, not the class-body name `x`.  Python class scope is never a
                // closure scope for methods, so their `global` declarations must not
                // force class-body names into cell vars (issue #624).
                if !is_class_scope {
                    let mut all_nested_globals = inner_globals.clone();
                    collect_transitive_global_names(nested_body, &mut all_nested_globals);
                    for name in &all_nested_globals {
                        if local_index.contains_key(name) {
                            cells.insert(name.clone());
                        }
                    }
                }
                let inner_locals = crate::interpreter::collect_local_names(
                    params,
                    nested_body,
                    &inner_globals,
                    &nonlocals,
                );
                // When compiling a class body (`is_class_scope = true`), do NOT
                // promote method free-variable reads to cell vars here.  Python class
                // scope is not a closure scope for methods: `def method(self): return x`
                // reads the enclosing *function* scope's `x`, not the class-body `x`.
                // Promoting `x` to a class-body cell var would route `x = val` inside
                // the class body through `StoreGlobal` (not `RecordClassStore`), silently
                // stripping `x` from the class attribute dict (issue #695).
                // The enclosing function is responsible for promoting its own locals to
                // cell vars via `collect_class_method_outer_refs` (called from the
                // `Stmt::Class` arm of this function).
                if !is_class_scope {
                    let mut inner_uses: HashSet<String> = HashSet::new();
                    collect_free_var_reads_in_stmts(nested_body, &mut inner_uses);
                    // Also include names referenced freely by ANY function/class/lambda
                    // nested deeper inside this body.  Even if `nested_body` itself never
                    // names `x`, an inner-inner function might read `x`; the current scope
                    // must still promote `x` to a cell var so the env chain carries it.
                    collect_transitive_free_vars_in_stmts(nested_body, &mut inner_uses);
                    for name in inner_uses {
                        if !inner_locals.contains(&name)
                            && !inner_globals.contains(&name)
                            && !nonlocals.contains(&name)
                            && local_index.contains_key(&name)
                        {
                            cells.insert(name);
                        }
                    }
                }
                // Don't recurse into nested defs - they see only their own cells.
            }
            Stmt::Class {
                body: nested_body, ..
            } => {
                // Class bodies (and transitively nested class bodies) can reference
                // outer nonlocals.  Class scope is transparent to `nonlocal`, so a
                // `nonlocal x` declared in any depth of nested class bodies still
                // reaches the enclosing *function* scope.  We use
                // `collect_nonlocal_names_through_classes` to find all such names
                // (issue #708: handles `def f(): x=1; class C: class D: nonlocal x`,
                //  issue #735: handles `def f(): x=1; class C: nonlocal x`).
                let mut nonlocals = HashSet::new();
                collect_nonlocal_names_through_classes(nested_body, &mut nonlocals);
                for name in &nonlocals {
                    if local_index.contains_key(name) {
                        cells.insert(name.clone());
                    }
                }
                // Only promote names to cell vars when the enclosing scope is a
                // function (not another class body).  When is_class_scope == true,
                // local_index belongs to the outer class — promoting its names to
                // cell vars would force `Outer.x = 50` to emit StoreGlobal instead
                // of RecordClassStore, losing the class attribute (issue #679).
                if !is_class_scope {
                    // Names declared `global` in the class body (including in
                    // doubly-nested class bodies via collect_global_names_via_class_chain)
                    // target the module env at runtime.  Promoting them to cell vars
                    // prevents the optimizer from const-folding the enclosing register
                    // using the pre-class value (issue #618, #672).
                    let class_body_globals = collect_global_names_via_class_chain(nested_body);
                    for name in &class_body_globals {
                        if local_index.contains_key(name) {
                            cells.insert(name.clone());
                        }
                    }

                    // Names directly read by top-level class-body statements (not
                    // inside methods) can refer to enclosing function locals.
                    // CPython promotes such names to cells so they are reachable
                    // via the env chain when the class body executes (issue #577).
                    let empty_set: HashSet<String> = HashSet::new();
                    let class_locals = crate::interpreter::collect_local_names(
                        &[],
                        nested_body,
                        &empty_set,
                        &empty_set,
                    );
                    let inner_globals = crate::interpreter::collect_global_names(nested_body);
                    // Collect names that appear as AugAssign targets at class body level.
                    // An AugAssign at class scope (e.g. `n += 1`) requires `n` to already
                    // be defined in the class scope — it does NOT capture from the enclosing
                    // function scope.  `collect_free_var_reads_in_stmts` inserts AugAssign
                    // target names as "uses", so we must subtract them here to avoid
                    // wrongly promoting enclosing-function locals to cell vars.
                    let mut class_aug_targets: HashSet<String> = HashSet::new();
                    for stmt in nested_body.iter() {
                        if let Stmt::AugAssign {
                            target: AssignTarget::Name(n),
                            ..
                        } = stmt
                        {
                            class_aug_targets.insert(n.clone());
                        }
                    }
                    let mut body_uses: HashSet<String> = HashSet::new();
                    collect_free_var_reads_in_stmts(nested_body, &mut body_uses);
                    collect_transitive_free_vars_in_stmts(nested_body, &mut body_uses);
                    for name in body_uses {
                        if !class_locals.contains(&name)
                            && !class_aug_targets.contains(&name)
                            && !inner_globals.contains(&name)
                            && !nonlocals.contains(&name)
                            && local_index.contains_key(&name)
                        {
                            cells.insert(name);
                        }
                    }
                }
                // Methods and lambdas inside a class access the enclosing scope
                // directly (Python class scope is not a closure scope for
                // methods or lambdas).  Find names that class methods/lambdas
                // read as free variables and promote them to cell vars so they
                // live in the env.
                //
                // Guard: only promote when the *current* scope is a function
                // (is_class_scope == false).  When is_class_scope == true, local_index
                // belongs to an outer class body — promoting its names to cell vars
                // would turn `Outer.x = val` into StoreGlobal, stripping the
                // class attribute (issue #690 / #701).  The enclosing function, if
                // any, handles promotion correctly when it encounters the outer
                // class via its own collect_cell_vars_in with is_class_scope=false.
                if !is_class_scope {
                    collect_class_method_outer_refs(nested_body, local_index, false, cells);
                }
            }
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    collect_cell_vars_in(b, local_index, is_class_scope, cells);
                }
                if let Some(b) = else_branch {
                    collect_cell_vars_in(b, local_index, is_class_scope, cells);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_cell_vars_in(body, local_index, is_class_scope, cells);
                if let Some(b) = else_branch {
                    collect_cell_vars_in(b, local_index, is_class_scope, cells);
                }
            }
            Stmt::For {
                body, else_branch, ..
            } => {
                collect_cell_vars_in(body, local_index, is_class_scope, cells);
                if let Some(b) = else_branch {
                    collect_cell_vars_in(b, local_index, is_class_scope, cells);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_cell_vars_in(body, local_index, is_class_scope, cells);
                for h in handlers {
                    collect_cell_vars_in(&h.body, local_index, is_class_scope, cells);
                }
                if let Some(b) = else_branch {
                    collect_cell_vars_in(b, local_index, is_class_scope, cells);
                }
                if let Some(b) = finally_branch {
                    collect_cell_vars_in(b, local_index, is_class_scope, cells);
                }
            }
            Stmt::With { body, .. } => {
                collect_cell_vars_in(body, local_index, is_class_scope, cells);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_cell_vars_in(&arm.body, local_index, is_class_scope, cells);
                }
            }
            _ => {}
        }
    }
}

/// Collect `nonlocal` declarations from `body`, recursing into nested class bodies
/// (since class scope is transparent to `nonlocal`) but NOT into nested `Stmt::Def`
/// bodies (function scope creates a new binding scope that stops the search).
///
/// Used by `collect_cell_vars_in` to find all nonlocal names visible from the
/// enclosing function scope, including those declared in doubly-nested classes
/// (e.g. `def f(): x=1; class C: class D: nonlocal x`).
fn collect_nonlocal_names_through_classes(body: &[Stmt], out: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Nonlocal(names) => {
                out.extend(names.iter().cloned());
            }
            Stmt::Class {
                body: class_body, ..
            } => {
                collect_nonlocal_names_through_classes(class_body, out);
            }
            // Stmt::Def and Stmt::Lambda start a new function scope —
            // `nonlocal` inside them binds to their own enclosing scope, not ours.
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    collect_nonlocal_names_through_classes(b, out);
                }
                if let Some(b) = else_branch {
                    collect_nonlocal_names_through_classes(b, out);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_nonlocal_names_through_classes(body, out);
                if let Some(b) = else_branch {
                    collect_nonlocal_names_through_classes(b, out);
                }
            }
            Stmt::For {
                body, else_branch, ..
            } => {
                collect_nonlocal_names_through_classes(body, out);
                if let Some(b) = else_branch {
                    collect_nonlocal_names_through_classes(b, out);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_nonlocal_names_through_classes(body, out);
                for h in handlers {
                    collect_nonlocal_names_through_classes(&h.body, out);
                }
                if let Some(b) = else_branch {
                    collect_nonlocal_names_through_classes(b, out);
                }
                if let Some(b) = finally_branch {
                    collect_nonlocal_names_through_classes(b, out);
                }
            }
            Stmt::With { body, .. } => {
                collect_nonlocal_names_through_classes(body, out);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_nonlocal_names_through_classes(&arm.body, out);
                }
            }
            _ => {}
        }
    }
}

/// Collect every name declared `global` in `body` or in any transitively-nested
/// `Stmt::Def` body, at any depth.  Used by `collect_cell_vars_in` so that a
/// module-level fastlocal referenced via `global x` inside a doubly-nested
/// function is promoted to a cell var, making both `StoreGlobal` (write) and
/// `LoadGlobal` (read) work correctly via `env.values` rather than a stale
/// register.
fn collect_transitive_global_names(body: &[Stmt], out: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Global(names) => {
                out.extend(names.iter().cloned());
            }
            Stmt::Def {
                body: nested_body, ..
            } => {
                collect_transitive_global_names(nested_body, out);
            }
            Stmt::Class { body, .. } => {
                collect_transitive_global_names(body, out);
            }
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    collect_transitive_global_names(b, out);
                }
                if let Some(b) = else_branch {
                    collect_transitive_global_names(b, out);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_transitive_global_names(body, out);
                if let Some(b) = else_branch {
                    collect_transitive_global_names(b, out);
                }
            }
            Stmt::For {
                body, else_branch, ..
            } => {
                collect_transitive_global_names(body, out);
                if let Some(b) = else_branch {
                    collect_transitive_global_names(b, out);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_transitive_global_names(body, out);
                for h in handlers {
                    collect_transitive_global_names(&h.body, out);
                }
                if let Some(b) = else_branch {
                    collect_transitive_global_names(b, out);
                }
                if let Some(b) = finally_branch {
                    collect_transitive_global_names(b, out);
                }
            }
            Stmt::With { body, .. } => {
                collect_transitive_global_names(body, out);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_transitive_global_names(&arm.body, out);
                }
            }
            _ => {}
        }
    }
}

/// Collect every name declared `global` in `body` or in any transitively-nested
/// `Stmt::Class` body, at any depth.  Does NOT cross `Stmt::Def` boundaries —
/// those declare their own function scope and their `global` declarations are
/// handled separately.  Used by `collect_cell_vars_in`'s `Stmt::Class` arm so
/// that `global x` inside a doubly-nested class body (e.g.
/// `class Outer: class Inner: global x`) is visible when scanning the outer
/// class body, without re-introducing the issue #624 regression where a
/// method's `global x` would incorrectly promote the outer class-body name.
fn collect_global_names_via_class_chain(body: &[Stmt]) -> HashSet<String> {
    let mut out = HashSet::new();
    collect_global_names_via_class_chain_into(body, &mut out);
    out
}

fn collect_global_names_via_class_chain_into(body: &[Stmt], out: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Global(names) => {
                out.extend(names.iter().cloned());
            }
            Stmt::Class { body: nested, .. } => {
                collect_global_names_via_class_chain_into(nested, out);
            }
            // Control-flow compound statements: recurse but remain at class scope.
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    collect_global_names_via_class_chain_into(b, out);
                }
                if let Some(b) = else_branch {
                    collect_global_names_via_class_chain_into(b, out);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_global_names_via_class_chain_into(body, out);
                if let Some(b) = else_branch {
                    collect_global_names_via_class_chain_into(b, out);
                }
            }
            Stmt::For {
                body, else_branch, ..
            } => {
                collect_global_names_via_class_chain_into(body, out);
                if let Some(b) = else_branch {
                    collect_global_names_via_class_chain_into(b, out);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_global_names_via_class_chain_into(body, out);
                for h in handlers {
                    collect_global_names_via_class_chain_into(&h.body, out);
                }
                if let Some(b) = else_branch {
                    collect_global_names_via_class_chain_into(b, out);
                }
                if let Some(b) = finally_branch {
                    collect_global_names_via_class_chain_into(b, out);
                }
            }
            Stmt::With { body, .. } => {
                collect_global_names_via_class_chain_into(body, out);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_global_names_via_class_chain_into(&arm.body, out);
                }
            }
            // Stmt::Def: do NOT recurse — function bodies are their own scope.
            _ => {}
        }
    }
}

/// Walk statements looking for `Expr::Lambda` at the current scope level
/// (not crossing into nested `Def`/`Class` scopes) and promote any outer
/// fastlocals that the lambda captures into cell vars so they live in the env.
///
/// `is_class_scope`: when true, the `local_index` belongs to a class body.
/// A lambda inside a class body does NOT close over the class namespace — it
/// closes over the enclosing function/module scope.  Free-var reads in the
/// lambda that match class-attribute names must not promote those names to
/// cell vars, or the class-body assignment (`x = 10`) emits `StoreGlobal`
/// instead of `RecordClassStore` and strips the attribute (issue #699).
fn collect_lambda_captures(
    stmts: &[Stmt],
    local_index: &HashMap<String, Reg>,
    is_class_scope: bool,
    cells: &mut HashSet<String>,
) {
    for stmt in stmts {
        lambda_captures_in_stmt(stmt, local_index, is_class_scope, cells);
    }
}

fn lambda_captures_in_stmt(
    stmt: &Stmt,
    local_index: &HashMap<String, Reg>,
    is_class_scope: bool,
    cells: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Def { .. } | Stmt::Class { .. } => {}
        Stmt::Assign(_, value) => {
            lambda_captures_in_expr(value, local_index, is_class_scope, cells)
        }
        Stmt::AttrAssign { target, expr, .. } => {
            lambda_captures_in_expr(target, local_index, is_class_scope, cells);
            lambda_captures_in_expr(expr, local_index, is_class_scope, cells);
        }
        Stmt::IndexAssign {
            target,
            index,
            expr,
        } => {
            lambda_captures_in_expr(target, local_index, is_class_scope, cells);
            lambda_captures_in_expr(index, local_index, is_class_scope, cells);
            lambda_captures_in_expr(expr, local_index, is_class_scope, cells);
        }
        Stmt::SliceAssign {
            target,
            lower,
            upper,
            step,
            expr,
        } => {
            lambda_captures_in_expr(target, local_index, is_class_scope, cells);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            }
            lambda_captures_in_expr(expr, local_index, is_class_scope, cells);
        }
        Stmt::AugAssign { expr, .. } => {
            lambda_captures_in_expr(expr, local_index, is_class_scope, cells)
        }
        Stmt::Return(Some(e)) => lambda_captures_in_expr(e, local_index, is_class_scope, cells),
        Stmt::Return(None) => {}
        Stmt::Expr(e) => lambda_captures_in_expr(e, local_index, is_class_scope, cells),
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            for (cond, body) in branches {
                lambda_captures_in_expr(cond, local_index, is_class_scope, cells);
                collect_lambda_captures(body, local_index, is_class_scope, cells);
            }
            if let Some(b) = else_branch {
                collect_lambda_captures(b, local_index, is_class_scope, cells);
            }
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            lambda_captures_in_expr(cond, local_index, is_class_scope, cells);
            collect_lambda_captures(body, local_index, is_class_scope, cells);
            if let Some(b) = else_branch {
                collect_lambda_captures(b, local_index, is_class_scope, cells);
            }
        }
        Stmt::For {
            iter,
            body,
            else_branch,
            ..
        } => {
            lambda_captures_in_expr(iter, local_index, is_class_scope, cells);
            collect_lambda_captures(body, local_index, is_class_scope, cells);
            if let Some(b) = else_branch {
                collect_lambda_captures(b, local_index, is_class_scope, cells);
            }
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            collect_lambda_captures(body, local_index, is_class_scope, cells);
            for h in handlers {
                if let Some(e) = &h.kind {
                    lambda_captures_in_expr(e, local_index, is_class_scope, cells);
                }
                collect_lambda_captures(&h.body, local_index, is_class_scope, cells);
            }
            if let Some(b) = else_branch {
                collect_lambda_captures(b, local_index, is_class_scope, cells);
            }
            if let Some(b) = finally_branch {
                collect_lambda_captures(b, local_index, is_class_scope, cells);
            }
        }
        Stmt::With { items, body, .. } => {
            for (e, _) in items {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            }
            collect_lambda_captures(body, local_index, is_class_scope, cells);
        }
        Stmt::Raise {
            expr: Some(e),
            cause,
            ..
        } => {
            lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            if let Some(c) = cause {
                lambda_captures_in_expr(c, local_index, is_class_scope, cells);
            }
        }
        Stmt::Assert { test, msg } => {
            lambda_captures_in_expr(test, local_index, is_class_scope, cells);
            if let Some(m) = msg {
                lambda_captures_in_expr(m, local_index, is_class_scope, cells);
            }
        }
        Stmt::Delete(exprs) => {
            for e in exprs {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            }
        }
        Stmt::Match { subject, arms } => {
            lambda_captures_in_expr(subject, local_index, is_class_scope, cells);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    lambda_captures_in_expr(guard, local_index, is_class_scope, cells);
                }
                collect_lambda_captures(&arm.body, local_index, is_class_scope, cells);
            }
        }
        Stmt::AnnAssign {
            annotation, value, ..
        } => {
            lambda_captures_in_expr(annotation, local_index, is_class_scope, cells);
            if let Some(v) = value {
                lambda_captures_in_expr(v, local_index, is_class_scope, cells);
            }
        }
        Stmt::TypeAlias { value, .. } => {
            lambda_captures_in_expr(value, local_index, is_class_scope, cells);
        }
        Stmt::Import { .. }
        | Stmt::ImportFrom { .. }
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Pass
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Raise { expr: None, .. } => {}
    }
}

/// Walk every sub-expression embedded in an f-string — both the main expression
/// of each replacement field and any nested expressions inside that field's
/// format spec.  This centralises the recursion so every AST analysis pass
/// (closure-capture, free-var collection, walrus collection, …) sees both.
fn for_each_fstring_expr<F: FnMut(&Expr)>(parts: &[FStringPart], f: &mut F) {
    for part in parts {
        if let FStringPart::Expr {
            expr, format_spec, ..
        } = part
        {
            f(expr);
            if let Some(spec_parts) = format_spec {
                for_each_fstring_expr(spec_parts, f);
            }
        }
    }
}

/// `&mut` variant of `for_each_fstring_expr` for passes that rewrite the AST
/// in place (e.g. the c-at-i indexing rewrite).
fn for_each_fstring_expr_mut<F: FnMut(&mut Expr)>(parts: &mut [FStringPart], f: &mut F) {
    for part in parts.iter_mut() {
        if let FStringPart::Expr {
            expr, format_spec, ..
        } = part
        {
            f(expr);
            if let Some(spec_parts) = format_spec.as_mut() {
                for_each_fstring_expr_mut(spec_parts, f);
            }
        }
    }
}

/// Predicate variant: returns true as soon as `pred` returns true for any
/// sub-expression in the f-string (main expr or nested spec expr).
fn any_fstring_expr<F: FnMut(&Expr) -> bool>(parts: &[FStringPart], pred: &mut F) -> bool {
    parts.iter().any(|part| match part {
        FStringPart::Literal(_) => false,
        FStringPart::Expr {
            expr, format_spec, ..
        } => {
            pred(expr)
                || format_spec
                    .as_ref()
                    .is_some_and(|spec_parts| any_fstring_expr(spec_parts, pred))
        }
    })
}

/// Like `any_fstring_expr`, but requires `pred` to hold for every embedded
/// sub-expression.  Literal parts trivially satisfy the predicate.
fn all_fstring_exprs<F: FnMut(&Expr) -> bool>(parts: &[FStringPart], pred: &mut F) -> bool {
    parts.iter().all(|part| match part {
        FStringPart::Literal(_) => true,
        FStringPart::Expr {
            expr, format_spec, ..
        } => {
            pred(expr)
                && format_spec
                    .as_ref()
                    .is_none_or(|spec_parts| all_fstring_exprs(spec_parts, pred))
        }
    })
}

/// Collect names bound by walrus (`:=`) inside `expr`, without descending
/// into nested comprehensions, lambdas, or generator expressions (they create
/// their own implicit scopes, so their walrus targets don't propagate here).
fn collect_walrus_writes_in_expr(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Named { target, value } => {
            out.insert(target.clone());
            collect_walrus_writes_in_expr(value, out);
        }
        Expr::Binary { left, right, .. } => {
            collect_walrus_writes_in_expr(left, out);
            collect_walrus_writes_in_expr(right, out);
        }
        Expr::Unary { expr: e, .. } => collect_walrus_writes_in_expr(e, out),
        Expr::Compare { left, ops } => {
            collect_walrus_writes_in_expr(left, out);
            for (_, e) in ops {
                collect_walrus_writes_in_expr(e, out);
            }
        }
        Expr::Call { func, args, .. } => {
            collect_walrus_writes_in_expr(func, out);
            for a in args {
                collect_walrus_writes_in_expr(&a.value, out);
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_walrus_writes_in_expr(cond, out);
            collect_walrus_writes_in_expr(then, out);
            collect_walrus_writes_in_expr(else_, out);
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                collect_walrus_writes_in_expr(e, out);
            }
        }
        Expr::Dict(items) => {
            for item in items {
                match item {
                    DictItem::Pair(k, v) => {
                        collect_walrus_writes_in_expr(k, out);
                        collect_walrus_writes_in_expr(v, out);
                    }
                    DictItem::DoubleSplat(e) => collect_walrus_writes_in_expr(e, out),
                }
            }
        }
        Expr::Index { target, index, .. } => {
            collect_walrus_writes_in_expr(target, out);
            collect_walrus_writes_in_expr(index, out);
        }
        Expr::Attr { target, .. } => collect_walrus_writes_in_expr(target, out),
        Expr::Starred(e) => collect_walrus_writes_in_expr(e, out),
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            collect_walrus_writes_in_expr(target, out);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_walrus_writes_in_expr(e, out);
            }
        }
        Expr::FString(parts) => {
            for_each_fstring_expr(parts, &mut |e| collect_walrus_writes_in_expr(e, out));
        }
        // Walrus targets inside comprehensions escape to the nearest enclosing
        // non-comprehension scope (PEP 572), so they may need to be promoted to
        // cell vars of an enclosing function. Descend into elt/key/val/cond.
        // Lambda creates a true new scope; stop there.
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            collect_walrus_writes_in_expr(elt, out);
            for clause in clauses {
                if let Some(c) = &clause.cond {
                    collect_walrus_writes_in_expr(c, out);
                }
                collect_walrus_writes_in_expr(&clause.iter, out);
            }
        }
        Expr::DictComp { key, val, clauses } => {
            collect_walrus_writes_in_expr(key, out);
            collect_walrus_writes_in_expr(val, out);
            for clause in clauses {
                if let Some(c) = &clause.cond {
                    collect_walrus_writes_in_expr(c, out);
                }
                collect_walrus_writes_in_expr(&clause.iter, out);
            }
        }
        Expr::Lambda { .. } => {}
        _ => {}
    }
}

/// Collect the simple names bound by a comprehension `for <target>` clause
/// (descending into tuple/starred targets).  Attribute/subscript targets bind
/// no names.
fn collect_comp_target_names(target: &AssignTarget, out: &mut HashSet<String>) {
    match target {
        AssignTarget::Name(name) => {
            out.insert(name.clone());
        }
        AssignTarget::Tuple(targets) => {
            for t in targets {
                collect_comp_target_names(t, out);
            }
        }
        AssignTarget::Starred(inner) => collect_comp_target_names(inner, out),
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => {}
    }
}

/// Validate a comprehension / generator expression at compile time, raising the
/// CPython 3.12 `SyntaxError`s that pyrust would otherwise accept:
///
/// * a `yield`/`yield from` directly in the element/condition expressions
///   (`'yield' inside <kind>`); and
/// * an assignment expression (`:=`) whose target collides with one of the
///   comprehension's own iteration variables (PEP 572,
///   `assignment expression cannot rebind comprehension iteration variable '<n>'`).
///
/// `result_exprs` are the value-producing expressions (`elt`, or `key`+`val`);
/// `clauses` are the comprehension clauses.  Iterable expressions (`clause.iter`)
/// are evaluated in the enclosing scope and are validated elsewhere, so they are
/// not scanned here.  `kind` is the CPython label (e.g. `"list comprehension"`).
///
/// Returns the `SyntaxError` message on violation (`None` when valid).
fn check_comprehension(
    result_exprs: &[&Expr],
    clauses: &[CompClause],
    kind: &str,
) -> Option<String> {
    // yield directly inside the comprehension body.
    let mut yields = result_exprs.iter().any(|e| expr_contains_yield(e));
    yields = yields
        || clauses
            .iter()
            .any(|c| c.cond.as_ref().is_some_and(expr_contains_yield));
    if yields {
        return Some(format!("'yield' inside {kind}"));
    }

    // Walrus target colliding with a comprehension iteration variable.
    let mut targets: HashSet<String> = HashSet::new();
    for c in clauses {
        collect_comp_target_names(&c.target, &mut targets);
    }
    let mut walrus: HashSet<String> = HashSet::new();
    for e in result_exprs {
        collect_walrus_writes_in_expr(e, &mut walrus);
    }
    for c in clauses {
        if let Some(cond) = &c.cond {
            collect_walrus_writes_in_expr(cond, &mut walrus);
        }
    }
    // Report deterministically (smallest name) when several collide.
    walrus.intersection(&targets).min().map(|name| {
        format!("assignment expression cannot rebind comprehension iteration variable '{name}'")
    })
}

fn lambda_captures_in_expr(
    expr: &Expr,
    local_index: &HashMap<String, Reg>,
    is_class_scope: bool,
    cells: &mut HashSet<String>,
) {
    match expr {
        Expr::Lambda { params, body } => {
            // When the enclosing scope is a class body, a lambda does NOT close
            // over the class namespace.  Free-var reads in the lambda that match
            // class-attribute names in `local_index` must not promote those names
            // to cell vars — doing so would make the class-body assignment emit
            // `StoreGlobal` instead of `RecordClassStore` and strip the attribute
            // (issue #699).  Skip promotion entirely for class scopes; the lambda
            // will resolve these names through the outer function/module env.
            if !is_class_scope {
                let mut uses = HashSet::new();
                collect_free_var_reads_in_expr(body, &mut uses);
                // Default expressions are evaluated in the enclosing scope.
                for param in params {
                    if let Some(d) = &param.default {
                        collect_free_var_reads_in_expr(d, &mut uses);
                    }
                }
                for param in params {
                    uses.remove(&param.name);
                }
                for name in uses {
                    if local_index.contains_key(&name) {
                        cells.insert(name);
                    }
                }
            }
        }
        Expr::Binary { left, right, .. } => {
            lambda_captures_in_expr(left, local_index, is_class_scope, cells);
            lambda_captures_in_expr(right, local_index, is_class_scope, cells);
        }
        Expr::Unary { expr: e, .. } => {
            lambda_captures_in_expr(e, local_index, is_class_scope, cells)
        }
        Expr::Compare { left, ops } => {
            lambda_captures_in_expr(left, local_index, is_class_scope, cells);
            for (_, e) in ops {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            }
        }
        Expr::Call { func, args, .. } => {
            lambda_captures_in_expr(func, local_index, is_class_scope, cells);
            for a in args {
                lambda_captures_in_expr(&a.value, local_index, is_class_scope, cells);
            }
        }
        Expr::Attr { target, .. } => {
            lambda_captures_in_expr(target, local_index, is_class_scope, cells)
        }
        Expr::Index { target, index, .. } => {
            lambda_captures_in_expr(target, local_index, is_class_scope, cells);
            lambda_captures_in_expr(index, local_index, is_class_scope, cells);
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            lambda_captures_in_expr(target, local_index, is_class_scope, cells);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            }
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            }
        }
        Expr::Starred(inner) => {
            lambda_captures_in_expr(inner, local_index, is_class_scope, cells);
        }
        Expr::Dict(items) => {
            for item in items {
                match item {
                    DictItem::Pair(k, v) => {
                        lambda_captures_in_expr(k, local_index, is_class_scope, cells);
                        lambda_captures_in_expr(v, local_index, is_class_scope, cells);
                    }
                    DictItem::DoubleSplat(e) => {
                        lambda_captures_in_expr(e, local_index, is_class_scope, cells);
                    }
                }
            }
        }
        // List/set/dict comprehensions and generator expressions all create an
        // implicit nested function scope (CPython behaviour since Python 3).
        // Only the outermost iterable is evaluated in the enclosing scope; the
        // body (inner iters, conditions, element/key/value expressions) runs
        // inside the nested scope and can close over enclosing locals.
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            // The outermost iterable is evaluated in the enclosing scope.
            if let Some(first) = clauses.first() {
                lambda_captures_in_expr(&first.iter, local_index, is_class_scope, cells);
            }
            // Everything inside the comprehension body runs in its own scope.
            // Collect free-var reads from that inner body, subtract the names
            // bound by the comprehension's own clause targets, and promote any
            // remaining names that live in the enclosing local_index to cell
            // vars so they're accessible via the env chain.
            if !is_class_scope {
                let mut inner_uses: HashSet<String> = HashSet::new();
                if let Some(first) = clauses.first()
                    && let Some(c) = &first.cond
                {
                    collect_free_var_reads_in_expr(c, &mut inner_uses);
                }
                for clause in clauses.iter().skip(1) {
                    collect_free_var_reads_in_expr(&clause.iter, &mut inner_uses);
                    if let Some(c) = &clause.cond {
                        collect_free_var_reads_in_expr(c, &mut inner_uses);
                    }
                }
                collect_free_var_reads_in_expr(elt, &mut inner_uses);
                // Remove names bound by the comprehension's own clause targets.
                let mut bound: HashSet<String> = HashSet::new();
                for clause in clauses {
                    collect_written_target(&clause.target, &mut bound);
                }
                for name in inner_uses {
                    if !bound.contains(&name) && local_index.contains_key(&name) {
                        cells.insert(name);
                    }
                }
                // PEP 572: walrus targets in a comprehension body belong to the
                // enclosing scope. Promote them to cell vars so they're reachable
                // via the env chain from inside the comprehension's implicit function.
                let mut walrus_writes: HashSet<String> = HashSet::new();
                for clause in clauses {
                    if let Some(c) = &clause.cond {
                        collect_walrus_writes_in_expr(c, &mut walrus_writes);
                    }
                }
                collect_walrus_writes_in_expr(elt, &mut walrus_writes);
                for name in walrus_writes {
                    if !bound.contains(&name) && local_index.contains_key(&name) {
                        cells.insert(name);
                    }
                }
            }
        }
        Expr::DictComp { key, val, clauses } => {
            // Same scope-isolation logic as list/set comprehensions above.
            if let Some(first) = clauses.first() {
                lambda_captures_in_expr(&first.iter, local_index, is_class_scope, cells);
            }
            if !is_class_scope {
                let mut inner_uses: HashSet<String> = HashSet::new();
                if let Some(first) = clauses.first()
                    && let Some(c) = &first.cond
                {
                    collect_free_var_reads_in_expr(c, &mut inner_uses);
                }
                for clause in clauses.iter().skip(1) {
                    collect_free_var_reads_in_expr(&clause.iter, &mut inner_uses);
                    if let Some(c) = &clause.cond {
                        collect_free_var_reads_in_expr(c, &mut inner_uses);
                    }
                }
                collect_free_var_reads_in_expr(key, &mut inner_uses);
                collect_free_var_reads_in_expr(val, &mut inner_uses);
                let mut bound: HashSet<String> = HashSet::new();
                for clause in clauses {
                    collect_written_target(&clause.target, &mut bound);
                }
                for name in inner_uses {
                    if !bound.contains(&name) && local_index.contains_key(&name) {
                        cells.insert(name);
                    }
                }
                // PEP 572: promote walrus write targets to cell vars.
                let mut walrus_writes: HashSet<String> = HashSet::new();
                for clause in clauses {
                    if let Some(c) = &clause.cond {
                        collect_walrus_writes_in_expr(c, &mut walrus_writes);
                    }
                }
                collect_walrus_writes_in_expr(key, &mut walrus_writes);
                collect_walrus_writes_in_expr(val, &mut walrus_writes);
                for name in walrus_writes {
                    if !bound.contains(&name) && local_index.contains_key(&name) {
                        cells.insert(name);
                    }
                }
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            lambda_captures_in_expr(cond, local_index, is_class_scope, cells);
            lambda_captures_in_expr(then, local_index, is_class_scope, cells);
            lambda_captures_in_expr(else_, local_index, is_class_scope, cells);
        }
        Expr::Named { value, .. } => {
            lambda_captures_in_expr(value, local_index, is_class_scope, cells)
        }
        Expr::FString(parts) => {
            for_each_fstring_expr(parts, &mut |e| {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            });
        }
        Expr::Var(_, _)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => {}
        Expr::Yield(Some(e)) => lambda_captures_in_expr(e, local_index, is_class_scope, cells),
        Expr::Yield(None) => {}
        Expr::YieldFrom(e) => lambda_captures_in_expr(e, local_index, is_class_scope, cells),
        Expr::Await(e) => lambda_captures_in_expr(e, local_index, is_class_scope, cells),
    }
}

/// Walk a class body and record names bound at the top level in their
/// *textual* order — used only to assign **register slot numbers** for
/// the class-body sub-compiler.  Slot order has **no** influence on
/// class-namespace insertion order any more (`vars(C)` follows runtime
/// stores via `Insn::RecordClassStore`); we keep this textual walk so
/// register assignments match declaration order even for names that only
/// appear inside nested control-flow.  Names not in `body_local` are
/// skipped (they're declared `global` / `nonlocal` and don't get a
/// class-body slot).
fn collect_class_body_names_textual(
    body: &[Stmt],
    ordered: &mut Vec<String>,
    seen: &mut HashSet<String>,
    body_local: &indexmap::IndexSet<String>,
) {
    for stmt in body {
        match stmt {
            Stmt::Assign(target, _) => {
                collect_assign_target_textual(target, ordered, seen, body_local);
            }
            Stmt::AnnAssign { name, .. }
                if body_local.contains(name) && seen.insert(name.clone()) =>
            {
                ordered.push(name.clone());
            }
            Stmt::Def { name, .. } | Stmt::Class { name, .. } | Stmt::TypeAlias { name, .. }
                if body_local.contains(name) && seen.insert(name.clone()) =>
            {
                ordered.push(name.clone());
            }
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    collect_class_body_names_textual(b, ordered, seen, body_local);
                }
                if let Some(b) = else_branch {
                    collect_class_body_names_textual(b, ordered, seen, body_local);
                }
            }
            Stmt::While { body, .. } | Stmt::For { body, .. } => {
                collect_class_body_names_textual(body, ordered, seen, body_local);
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_class_body_names_textual(body, ordered, seen, body_local);
                for h in handlers {
                    collect_class_body_names_textual(&h.body, ordered, seen, body_local);
                }
                if let Some(b) = else_branch {
                    collect_class_body_names_textual(b, ordered, seen, body_local);
                }
                if let Some(b) = finally_branch {
                    collect_class_body_names_textual(b, ordered, seen, body_local);
                }
            }
            Stmt::With { body, .. } => {
                collect_class_body_names_textual(body, ordered, seen, body_local);
            }
            _ => {}
        }
    }
}

fn collect_assign_target_textual(
    target: &AssignTarget,
    ordered: &mut Vec<String>,
    seen: &mut HashSet<String>,
    body_local: &indexmap::IndexSet<String>,
) {
    match target {
        AssignTarget::Name(name) => {
            if body_local.contains(name) && seen.insert(name.clone()) {
                ordered.push(name.clone());
            }
        }
        AssignTarget::Tuple(targets) => {
            for t in targets {
                collect_assign_target_textual(t, ordered, seen, body_local);
            }
        }
        AssignTarget::Starred(inner) => {
            collect_assign_target_textual(inner, ordered, seen, body_local);
        }
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => {}
    }
}

/// For a class body, collect names that the class's methods read as free
/// variables from the enclosing scope.  Python class scope is not a closure
/// scope for methods: `def method(self): return x` reads the outer `x`, not
/// any `x` defined at class level.  Promote those names to cell vars so they
/// live in the env (not registers) and are accessible via `LoadGlobal`.
fn collect_class_method_outer_refs(
    class_body: &[Stmt],
    local_index: &HashMap<String, Reg>,
    outer_is_class_scope: bool,
    cells: &mut HashSet<String>,
) {
    // When `local_index` belongs to a *function* scope (`outer_is_class_scope=false`),
    // methods inside this class can close over function locals even when those names
    // are also assigned in the class body.  Python class scope is not a closure scope
    // for methods — a method's free-variable lookup skips the class body entirely.
    //
    // When `local_index` belongs to a *class* scope (`outer_is_class_scope=true`),
    // we must NOT promote class-body names as cell vars just because a further-nested
    // method reads a name that happens to be a class attribute.  The nested method
    // also skips the outer class scope, so promoting would incorrectly turn class
    // attribute assignments into StoreGlobal, stripping the attribute from the dict.
    // Always compute class_locals for use in lambda handling: lambdas in a
    // class body close over the enclosing function scope, not the class scope,
    // so we need the class-body local names to avoid false promotions (issue #699).
    let empty_set: HashSet<String> = HashSet::new();
    let class_locals =
        crate::interpreter::collect_local_names(&[], class_body, &empty_set, &empty_set);
    // For method Def arms: only filter out class-body locals when the outer
    // scope is itself a class scope (outer_is_class_scope=true).  When the
    // outer scope is a function scope, methods may close over function locals
    // even when the class body also defines a name with the same spelling.
    let class_locals_opt: Option<&indexmap::IndexSet<String>> = if outer_is_class_scope {
        Some(&class_locals)
    } else {
        None
    };

    for stmt in class_body {
        match stmt {
            Stmt::Def {
                params,
                body: method_body,
                ..
            } => {
                let inner_globals = crate::interpreter::collect_global_names(method_body);
                // Do NOT promote `global x` declarations from methods here.
                // A method's `global x` routes directly to the module environment
                // regardless of whether the enclosing scope is a function or a class.
                // Promoting them would incorrectly force the outer scope's `x` into a
                // cell var, which for a class body means `x = ...` emits StoreGlobal
                // instead of RecordClassStore (issue #629; see also issue #624).
                let inner_nonlocals = crate::interpreter::collect_nonlocal_names(method_body);
                // Promote nonlocal declarations in methods to cell vars in the
                // enclosing function scope.  This mirrors what `collect_cell_vars_in`
                // does for plain nested `Def`s: any name declared `nonlocal` inside a
                // method must live in the outer env so the method closure can mutate it.
                for name in &inner_nonlocals {
                    if local_index.contains_key(name) {
                        cells.insert(name.clone());
                    }
                }
                let inner_locals = crate::interpreter::collect_local_names(
                    params,
                    method_body,
                    &inner_globals,
                    &inner_nonlocals,
                );
                let mut uses: HashSet<String> = HashSet::new();
                collect_free_var_reads_in_stmts(method_body, &mut uses);
                // Functions/classes nested deeper inside this method may also
                // reference outer names that the method itself never mentions.
                collect_transitive_free_vars_in_stmts(method_body, &mut uses);
                // Note: we intentionally do NOT filter by class-body locals here.
                // Python class scope is not a closure scope for methods: a method
                // reading `x` skips the class namespace entirely and looks in the
                // enclosing function scope.  Even when the class also defines `x`,
                // the outer function's `x` must be promoted to a cell var so the
                // method can reach it (issue #700).
                for name in uses {
                    if !inner_locals.contains(&name)
                        && !inner_globals.contains(&name)
                        && !inner_nonlocals.contains(&name)
                        && class_locals_opt.is_none_or(|cl| !cl.contains(&name))
                        && local_index.contains_key(&name)
                    {
                        cells.insert(name);
                    }
                }
            }
            // Lambdas assigned at class body level (e.g. `fn = lambda self: x`)
            // also close over the *enclosing function* scope (not the class scope).
            // The class body's own `collect_lambda_captures` correctly skips
            // promoting their reads to class cell vars (issue #699), but we still
            // need to promote those reads to cell vars in the *enclosing function*
            // so the env chain carries them when the generator body resumes.
            Stmt::Assign(_, value) => {
                collect_class_lambda_outer_refs_in_expr(value, local_index, &class_locals, cells);
            }
            Stmt::Expr(e) => {
                collect_class_lambda_outer_refs_in_expr(e, local_index, &class_locals, cells);
            }
            Stmt::AugAssign { expr, .. } => {
                collect_class_lambda_outer_refs_in_expr(expr, local_index, &class_locals, cells);
            }
            // Recurse into nested class bodies.  A lambda or method inside
            // `class B` inside `class A` inside a function can still read the
            // outer function's locals; without this arm those reads are never
            // seen and `x` is never promoted to a cell var (issue #703).
            // Use `outer_is_class_scope` to properly handle nested class scopes.
            Stmt::Class {
                body: nested_class_body,
                ..
            } => {
                collect_class_method_outer_refs(
                    nested_class_body,
                    local_index,
                    outer_is_class_scope,
                    cells,
                );
            }
            // Recursively handle class-level control flow.
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
                if let Some(b) = else_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_class_method_outer_refs(body, local_index, outer_is_class_scope, cells);
                if let Some(b) = else_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
            }
            Stmt::For {
                body, else_branch, ..
            } => {
                collect_class_method_outer_refs(body, local_index, outer_is_class_scope, cells);
                if let Some(b) = else_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_class_method_outer_refs(body, local_index, outer_is_class_scope, cells);
                for h in handlers {
                    collect_class_method_outer_refs(
                        &h.body,
                        local_index,
                        outer_is_class_scope,
                        cells,
                    );
                }
                if let Some(b) = else_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
                if let Some(b) = finally_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
            }
            Stmt::With { body, .. } => {
                collect_class_method_outer_refs(body, local_index, outer_is_class_scope, cells);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_class_method_outer_refs(
                        &arm.body,
                        local_index,
                        outer_is_class_scope,
                        cells,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Walk `expr` in a class body context.  For each `Expr::Lambda` found,
/// collect its free-var reads, subtract the lambda's own params and the
/// class-body local names, then promote any remaining names that live in
/// the enclosing function's `local_index` to cell vars.
///
/// This is the mirror of the `Expr::Lambda` arm in `lambda_captures_in_expr`
/// for the class-body case: `collect_lambda_captures` (called on the class
/// body) correctly *skips* promotion into the class cell-var set (issue #699),
/// but when the class body is nested inside a function the enclosing function
/// still needs those names promoted so the env chain carries them (issue #701).
// `class_locals` is threaded for symmetry with the sibling collectors and to
// document the class-scope context; removing it would churn ~30 recursive call
// sites + 3 external callers for no behavior change.
#[allow(clippy::only_used_in_recursion)]
fn collect_class_lambda_outer_refs_in_expr(
    expr: &Expr,
    local_index: &HashMap<String, Reg>,
    class_locals: &indexmap::IndexSet<String>,
    cells: &mut HashSet<String>,
) {
    match expr {
        Expr::Lambda { params, body } => {
            let mut uses: HashSet<String> = HashSet::new();
            collect_free_var_reads_in_expr(body, &mut uses);
            collect_transitive_free_vars_in_expr(body, &mut uses);
            // Default expressions are evaluated in the enclosing scope.
            for p in params {
                if let Some(d) = &p.default {
                    collect_free_var_reads_in_expr(d, &mut uses);
                    collect_transitive_free_vars_in_expr(d, &mut uses);
                }
            }
            for p in params {
                uses.remove(&p.name);
            }
            // Promote any name that the lambda reads from the enclosing function.
            // Note: we do NOT filter out class-body locals here.  Python class
            // scope is not a closure scope — a lambda in a class body that reads
            // `x` always sees the enclosing function/module value even when the
            // class body also has `x = ...`.  The class body's own emit path
            // (`collect_cell_vars_for_class_body`) independently does not promote
            // class-attribute names to cell vars (issue #699), so the class-body
            // assignment correctly emits `RecordClassStore` regardless of what
            // the enclosing function promotes.
            for name in uses {
                if local_index.contains_key(&name) {
                    cells.insert(name);
                }
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_class_lambda_outer_refs_in_expr(left, local_index, class_locals, cells);
            collect_class_lambda_outer_refs_in_expr(right, local_index, class_locals, cells);
        }
        Expr::Unary { expr: e, .. } => {
            collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells)
        }
        Expr::Compare { left, ops } => {
            collect_class_lambda_outer_refs_in_expr(left, local_index, class_locals, cells);
            for (_, e) in ops {
                collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells);
            }
        }
        Expr::Call { func, args, .. } => {
            collect_class_lambda_outer_refs_in_expr(func, local_index, class_locals, cells);
            for a in args {
                collect_class_lambda_outer_refs_in_expr(&a.value, local_index, class_locals, cells);
            }
        }
        Expr::Attr { target, .. } => {
            collect_class_lambda_outer_refs_in_expr(target, local_index, class_locals, cells)
        }
        Expr::Index { target, index, .. } => {
            collect_class_lambda_outer_refs_in_expr(target, local_index, class_locals, cells);
            collect_class_lambda_outer_refs_in_expr(index, local_index, class_locals, cells);
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            collect_class_lambda_outer_refs_in_expr(target, local_index, class_locals, cells);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells);
            }
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells);
            }
        }
        Expr::Starred(inner) => {
            collect_class_lambda_outer_refs_in_expr(inner, local_index, class_locals, cells)
        }
        Expr::Dict(items) => {
            for item in items {
                match item {
                    DictItem::Pair(k, v) => {
                        collect_class_lambda_outer_refs_in_expr(
                            k,
                            local_index,
                            class_locals,
                            cells,
                        );
                        collect_class_lambda_outer_refs_in_expr(
                            v,
                            local_index,
                            class_locals,
                            cells,
                        );
                    }
                    DictItem::DoubleSplat(e) => {
                        collect_class_lambda_outer_refs_in_expr(
                            e,
                            local_index,
                            class_locals,
                            cells,
                        );
                    }
                }
            }
        }
        // All comprehension forms create an implicit nested function scope.
        // Only the outermost iterable is evaluated in the enclosing (class) scope.
        Expr::ListComp { clauses, .. }
        | Expr::SetComp { clauses, .. }
        | Expr::GenExp { clauses, .. } => {
            if let Some(first) = clauses.first() {
                collect_class_lambda_outer_refs_in_expr(
                    &first.iter,
                    local_index,
                    class_locals,
                    cells,
                );
            }
        }
        Expr::DictComp { clauses, .. } => {
            if let Some(first) = clauses.first() {
                collect_class_lambda_outer_refs_in_expr(
                    &first.iter,
                    local_index,
                    class_locals,
                    cells,
                );
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_class_lambda_outer_refs_in_expr(cond, local_index, class_locals, cells);
            collect_class_lambda_outer_refs_in_expr(then, local_index, class_locals, cells);
            collect_class_lambda_outer_refs_in_expr(else_, local_index, class_locals, cells);
        }
        Expr::Named { value, .. } => {
            collect_class_lambda_outer_refs_in_expr(value, local_index, class_locals, cells)
        }
        Expr::FString(parts) => {
            for_each_fstring_expr(parts, &mut |e| {
                collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells);
            });
        }
        Expr::Var(_, _)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => {}
        Expr::Yield(Some(e)) => {
            collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells)
        }
        Expr::Yield(None) => {}
        Expr::YieldFrom(e) => {
            collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells)
        }
        Expr::Await(e) => {
            collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells)
        }
    }
}

/// Collect all `Var` reads in `stmts`, stopping at nested `Def`/`Class`
/// boundaries.  Used to detect free variables that need to become cell vars.
fn collect_free_var_reads_in_stmts(stmts: &[Stmt], uses: &mut HashSet<String>) {
    for stmt in stmts {
        collect_free_var_reads_in_stmt(stmt, uses);
    }
}

fn collect_free_var_reads_in_stmt(stmt: &Stmt, uses: &mut HashSet<String>) {
    match stmt {
        Stmt::Def { .. } | Stmt::Class { .. } => {}
        Stmt::Assign(_, value) => {
            collect_free_var_reads_in_expr(value, uses);
        }
        Stmt::AttrAssign { target, expr, .. } => {
            collect_free_var_reads_in_expr(target, uses);
            collect_free_var_reads_in_expr(expr, uses);
        }
        Stmt::IndexAssign {
            target,
            index,
            expr,
        } => {
            collect_free_var_reads_in_expr(target, uses);
            collect_free_var_reads_in_expr(index, uses);
            collect_free_var_reads_in_expr(expr, uses);
        }
        Stmt::SliceAssign {
            target,
            lower,
            upper,
            step,
            expr,
        } => {
            collect_free_var_reads_in_expr(target, uses);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_free_var_reads_in_expr(e, uses);
            }
            collect_free_var_reads_in_expr(expr, uses);
        }
        Stmt::AugAssign {
            target,
            op: _,
            expr,
        } => {
            if let AssignTarget::Name(n) = target {
                uses.insert(n.clone());
            }
            collect_free_var_reads_in_expr(expr, uses);
        }
        Stmt::Return(Some(e)) => collect_free_var_reads_in_expr(e, uses),
        Stmt::Return(None) => {}
        Stmt::Expr(e) => collect_free_var_reads_in_expr(e, uses),
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            for (cond, body) in branches {
                collect_free_var_reads_in_expr(cond, uses);
                collect_free_var_reads_in_stmts(body, uses);
            }
            if let Some(b) = else_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            collect_free_var_reads_in_expr(cond, uses);
            collect_free_var_reads_in_stmts(body, uses);
            if let Some(b) = else_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
        }
        Stmt::For {
            iter,
            body,
            else_branch,
            ..
        } => {
            collect_free_var_reads_in_expr(iter, uses);
            collect_free_var_reads_in_stmts(body, uses);
            if let Some(b) = else_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            collect_free_var_reads_in_stmts(body, uses);
            for h in handlers {
                if let Some(e) = &h.kind {
                    collect_free_var_reads_in_expr(e, uses);
                }
                collect_free_var_reads_in_stmts(&h.body, uses);
            }
            if let Some(b) = else_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
            if let Some(b) = finally_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
        }
        Stmt::With { items, body, .. } => {
            for (e, _) in items {
                collect_free_var_reads_in_expr(e, uses);
            }
            collect_free_var_reads_in_stmts(body, uses);
        }
        Stmt::Raise {
            expr: Some(e),
            cause,
            ..
        } => {
            collect_free_var_reads_in_expr(e, uses);
            if let Some(c) = cause {
                collect_free_var_reads_in_expr(c, uses);
            }
        }
        Stmt::Assert { test, msg } => {
            collect_free_var_reads_in_expr(test, uses);
            if let Some(m) = msg {
                collect_free_var_reads_in_expr(m, uses);
            }
        }
        Stmt::Delete(exprs) => {
            for e in exprs {
                collect_free_var_reads_in_expr(e, uses);
            }
        }
        Stmt::Match { subject, arms } => {
            collect_free_var_reads_in_expr(subject, uses);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_free_var_reads_in_expr(guard, uses);
                }
                collect_free_var_reads_in_stmts(&arm.body, uses);
            }
        }
        Stmt::AnnAssign {
            annotation, value, ..
        } => {
            collect_free_var_reads_in_expr(annotation, uses);
            if let Some(v) = value {
                collect_free_var_reads_in_expr(v, uses);
            }
        }
        Stmt::TypeAlias { value, .. } => {
            collect_free_var_reads_in_expr(value, uses);
        }
        Stmt::Import { .. }
        | Stmt::ImportFrom { .. }
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Pass
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Raise { expr: None, .. } => {}
    }
}

fn collect_free_var_reads_in_expr(expr: &Expr, uses: &mut HashSet<String>) {
    match expr {
        Expr::Var(n, _) => {
            uses.insert(n.clone());
        }
        Expr::Binary { left, right, .. } => {
            collect_free_var_reads_in_expr(left, uses);
            collect_free_var_reads_in_expr(right, uses);
        }
        Expr::Unary { expr: e, .. } => collect_free_var_reads_in_expr(e, uses),
        Expr::Compare { left, ops } => {
            collect_free_var_reads_in_expr(left, uses);
            for (_, e) in ops {
                collect_free_var_reads_in_expr(e, uses);
            }
        }
        Expr::Call { func, args, .. } => {
            collect_free_var_reads_in_expr(func, uses);
            for a in args {
                collect_free_var_reads_in_expr(&a.value, uses);
            }
        }
        Expr::Attr { target, .. } => collect_free_var_reads_in_expr(target, uses),
        Expr::Index { target, index, .. } => {
            collect_free_var_reads_in_expr(target, uses);
            collect_free_var_reads_in_expr(index, uses);
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            collect_free_var_reads_in_expr(target, uses);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_free_var_reads_in_expr(e, uses);
            }
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                collect_free_var_reads_in_expr(e, uses);
            }
        }
        Expr::Starred(inner) => collect_free_var_reads_in_expr(inner, uses),
        Expr::Dict(items) => {
            for item in items {
                match item {
                    DictItem::Pair(k, v) => {
                        collect_free_var_reads_in_expr(k, uses);
                        collect_free_var_reads_in_expr(v, uses);
                    }
                    DictItem::DoubleSplat(e) => collect_free_var_reads_in_expr(e, uses),
                }
            }
        }
        // All comprehension forms and generator expressions create an implicit
        // nested function scope.  Only the outermost iterable is evaluated at
        // the current scope level; the body runs inside the nested scope.
        Expr::ListComp { clauses, .. }
        | Expr::SetComp { clauses, .. }
        | Expr::GenExp { clauses, .. } => {
            if let Some(first) = clauses.first() {
                collect_free_var_reads_in_expr(&first.iter, uses);
            }
        }
        Expr::DictComp { clauses, .. } => {
            if let Some(first) = clauses.first() {
                collect_free_var_reads_in_expr(&first.iter, uses);
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_free_var_reads_in_expr(cond, uses);
            collect_free_var_reads_in_expr(then, uses);
            collect_free_var_reads_in_expr(else_, uses);
        }
        Expr::Lambda { params, body } => {
            // Default expressions are evaluated in the enclosing scope.
            for p in params {
                if let Some(d) = &p.default {
                    collect_free_var_reads_in_expr(d, uses);
                }
            }
            collect_free_var_reads_in_expr(body, uses);
        }
        Expr::Named { value, .. } => collect_free_var_reads_in_expr(value, uses),
        Expr::FString(parts) => {
            for_each_fstring_expr(parts, &mut |e| collect_free_var_reads_in_expr(e, uses));
        }
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Complex(_, _)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => {}
        Expr::Yield(Some(e)) => collect_free_var_reads_in_expr(e, uses),
        Expr::Yield(None) => {}
        Expr::YieldFrom(e) => collect_free_var_reads_in_expr(e, uses),
        Expr::Await(e) => collect_free_var_reads_in_expr(e, uses),
    }
}

// ─── Transitive free-variable collection ─────────────────────────────────────
//
// `collect_free_var_reads_in_*` deliberately stops at nested `Def`/`Class`
// boundaries because those scopes have their own locals.  For closure capture
// to work through more than two levels of nesting, the *current* scope must
// also know about names that descendants (functions inside the directly-nested
// function) read from outer scopes — otherwise the intermediate scope never
// promotes those names to cell vars, and the env chain has no entry for them.
//
// `collect_transitive_free_vars_in_stmts` walks INTO every nested `Def`,
// `Class`, and `Lambda` it finds and unions their free-name sets into `uses`,
// subtracting only the locals bound by each nested scope.  Combined with the
// usual `collect_free_var_reads_in_stmts` (which handles names mentioned at
// the current level), this yields the full set of outer-scope names that
// the enclosing function must keep accessible via cell vars.

fn collect_transitive_free_vars_in_stmts(stmts: &[Stmt], uses: &mut HashSet<String>) {
    for stmt in stmts {
        collect_transitive_free_vars_in_stmt(stmt, uses);
    }
}

fn collect_transitive_free_vars_in_stmt(stmt: &Stmt, uses: &mut HashSet<String>) {
    match stmt {
        Stmt::Def {
            params,
            body: nested_body,
            decorators,
            return_annotation,
            ..
        } => {
            // Decorator expressions evaluate in the enclosing scope.
            for d in decorators {
                collect_transitive_free_vars_in_expr(d, uses);
                collect_free_var_reads_in_expr(d, uses);
            }
            // Default values are also evaluated in the enclosing scope.
            for p in params {
                if let Some(d) = &p.default {
                    collect_transitive_free_vars_in_expr(d, uses);
                    collect_free_var_reads_in_expr(d, uses);
                }
                // Annotation expressions also evaluate in the enclosing scope.
                if let Some(a) = &p.annotation {
                    collect_transitive_free_vars_in_expr(a, uses);
                    collect_free_var_reads_in_expr(a, uses);
                }
            }
            if let Some(a) = return_annotation {
                collect_transitive_free_vars_in_expr(a, uses);
                collect_free_var_reads_in_expr(a, uses);
            }
            // Names locally bound inside the nested function — exclude them
            // when contributing to the enclosing scope's free-var set.
            let nested_globals = crate::interpreter::collect_global_names(nested_body);
            // Use `collect_nonlocal_names_through_classes` so that a `nonlocal x`
            // declared inside a class body inside this nested function is treated
            // as an enclosing-scope reference (issue #735).
            let mut nested_nonlocals = HashSet::new();
            collect_nonlocal_names_through_classes(nested_body, &mut nested_nonlocals);
            let nested_locals = crate::interpreter::collect_local_names(
                params,
                nested_body,
                &nested_globals,
                &nested_nonlocals,
            );
            let mut nested_uses: HashSet<String> = HashSet::new();
            collect_free_var_reads_in_stmts(nested_body, &mut nested_uses);
            collect_transitive_free_vars_in_stmts(nested_body, &mut nested_uses);
            // Explicit `nonlocal x` makes `x` an enclosing-scope reference even if
            // the body doesn't read it textually.
            for n in &nested_nonlocals {
                nested_uses.insert(n.clone());
            }
            for name in nested_uses {
                if !nested_locals.contains(&name) && !nested_globals.contains(&name) {
                    uses.insert(name);
                }
            }
        }
        Stmt::Class {
            body: nested_body,
            bases,
            metaclass,
            keywords,
            decorators,
            ..
        } => {
            // Decorator + base expressions evaluate in the enclosing scope.
            for d in decorators {
                collect_transitive_free_vars_in_expr(d, uses);
                collect_free_var_reads_in_expr(d, uses);
            }
            for b in bases {
                collect_transitive_free_vars_in_expr(b, uses);
                collect_free_var_reads_in_expr(b, uses);
            }
            if let Some(m) = metaclass {
                collect_transitive_free_vars_in_expr(m, uses);
                collect_free_var_reads_in_expr(m, uses);
            }
            // PEP 487 keyword arg expressions also evaluate in the enclosing scope.
            for (_, v) in keywords {
                collect_transitive_free_vars_in_expr(v, uses);
                collect_free_var_reads_in_expr(v, uses);
            }
            // Class body itself: methods read enclosing scope (skipping class scope).
            // We approximate the class scope conservatively by collecting class-level
            // assignments as the local set, while excluding any `nonlocal` names so
            // they remain visible as enclosing-scope references (issue #735).
            let empty_set: HashSet<String> = HashSet::new();
            let mut class_nonlocals: HashSet<String> = HashSet::new();
            collect_nonlocal_names_through_classes(nested_body, &mut class_nonlocals);
            let class_locals = crate::interpreter::collect_local_names(
                &[],
                nested_body,
                &empty_set,
                &class_nonlocals,
            );
            let mut class_uses: HashSet<String> = HashSet::new();
            collect_free_var_reads_in_stmts(nested_body, &mut class_uses);
            collect_transitive_free_vars_in_stmts(nested_body, &mut class_uses);
            // `nonlocal x` in the class body is an enclosing-scope reference.
            for n in &class_nonlocals {
                class_uses.insert(n.clone());
            }
            for name in class_uses {
                if !class_locals.contains(&name) {
                    uses.insert(name);
                }
            }
        }
        Stmt::Assign(_, value) => collect_transitive_free_vars_in_expr(value, uses),
        Stmt::AttrAssign { target, expr, .. } => {
            collect_transitive_free_vars_in_expr(target, uses);
            collect_transitive_free_vars_in_expr(expr, uses);
        }
        Stmt::IndexAssign {
            target,
            index,
            expr,
        } => {
            collect_transitive_free_vars_in_expr(target, uses);
            collect_transitive_free_vars_in_expr(index, uses);
            collect_transitive_free_vars_in_expr(expr, uses);
        }
        Stmt::SliceAssign {
            target,
            lower,
            upper,
            step,
            expr,
        } => {
            collect_transitive_free_vars_in_expr(target, uses);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_transitive_free_vars_in_expr(e, uses);
            }
            collect_transitive_free_vars_in_expr(expr, uses);
        }
        Stmt::AugAssign { expr, .. } => collect_transitive_free_vars_in_expr(expr, uses),
        Stmt::Return(Some(e)) => collect_transitive_free_vars_in_expr(e, uses),
        Stmt::Expr(e) => collect_transitive_free_vars_in_expr(e, uses),
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            for (cond, body) in branches {
                collect_transitive_free_vars_in_expr(cond, uses);
                collect_transitive_free_vars_in_stmts(body, uses);
            }
            if let Some(b) = else_branch {
                collect_transitive_free_vars_in_stmts(b, uses);
            }
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            collect_transitive_free_vars_in_expr(cond, uses);
            collect_transitive_free_vars_in_stmts(body, uses);
            if let Some(b) = else_branch {
                collect_transitive_free_vars_in_stmts(b, uses);
            }
        }
        Stmt::For {
            iter,
            body,
            else_branch,
            ..
        } => {
            collect_transitive_free_vars_in_expr(iter, uses);
            collect_transitive_free_vars_in_stmts(body, uses);
            if let Some(b) = else_branch {
                collect_transitive_free_vars_in_stmts(b, uses);
            }
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            collect_transitive_free_vars_in_stmts(body, uses);
            for h in handlers {
                if let Some(e) = &h.kind {
                    collect_transitive_free_vars_in_expr(e, uses);
                }
                collect_transitive_free_vars_in_stmts(&h.body, uses);
            }
            if let Some(b) = else_branch {
                collect_transitive_free_vars_in_stmts(b, uses);
            }
            if let Some(b) = finally_branch {
                collect_transitive_free_vars_in_stmts(b, uses);
            }
        }
        Stmt::With { items, body, .. } => {
            for (e, _) in items {
                collect_transitive_free_vars_in_expr(e, uses);
            }
            collect_transitive_free_vars_in_stmts(body, uses);
        }
        Stmt::Raise {
            expr: Some(e),
            cause,
            ..
        } => {
            collect_transitive_free_vars_in_expr(e, uses);
            if let Some(c) = cause {
                collect_transitive_free_vars_in_expr(c, uses);
            }
        }
        Stmt::Assert { test, msg } => {
            collect_transitive_free_vars_in_expr(test, uses);
            if let Some(m) = msg {
                collect_transitive_free_vars_in_expr(m, uses);
            }
        }
        Stmt::Delete(exprs) => {
            for e in exprs {
                collect_transitive_free_vars_in_expr(e, uses);
            }
        }
        Stmt::Match { subject, arms } => {
            collect_transitive_free_vars_in_expr(subject, uses);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_transitive_free_vars_in_expr(guard, uses);
                }
                collect_transitive_free_vars_in_stmts(&arm.body, uses);
            }
        }
        Stmt::AnnAssign {
            annotation, value, ..
        } => {
            collect_transitive_free_vars_in_expr(annotation, uses);
            if let Some(v) = value {
                collect_transitive_free_vars_in_expr(v, uses);
            }
        }
        Stmt::TypeAlias { value, .. } => {
            collect_transitive_free_vars_in_expr(value, uses);
        }
        Stmt::Return(None)
        | Stmt::Import { .. }
        | Stmt::ImportFrom { .. }
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Pass
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Raise { expr: None, .. } => {}
    }
}

fn collect_transitive_free_vars_in_expr(expr: &Expr, uses: &mut HashSet<String>) {
    match expr {
        Expr::Lambda { params, body } => {
            let mut inner_uses: HashSet<String> = HashSet::new();
            collect_free_var_reads_in_expr(body, &mut inner_uses);
            collect_transitive_free_vars_in_expr(body, &mut inner_uses);
            // Default expressions are evaluated in the enclosing scope.
            for p in params {
                if let Some(d) = &p.default {
                    collect_free_var_reads_in_expr(d, &mut inner_uses);
                    collect_transitive_free_vars_in_expr(d, &mut inner_uses);
                }
            }
            for p in params {
                inner_uses.remove(&p.name);
            }
            for n in inner_uses {
                uses.insert(n);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_transitive_free_vars_in_expr(left, uses);
            collect_transitive_free_vars_in_expr(right, uses);
        }
        Expr::Unary { expr: e, .. } => collect_transitive_free_vars_in_expr(e, uses),
        Expr::Compare { left, ops } => {
            collect_transitive_free_vars_in_expr(left, uses);
            for (_, e) in ops {
                collect_transitive_free_vars_in_expr(e, uses);
            }
        }
        Expr::Call { func, args, .. } => {
            collect_transitive_free_vars_in_expr(func, uses);
            for a in args {
                collect_transitive_free_vars_in_expr(&a.value, uses);
            }
        }
        Expr::Attr { target, .. } => collect_transitive_free_vars_in_expr(target, uses),
        Expr::Index { target, index, .. } => {
            collect_transitive_free_vars_in_expr(target, uses);
            collect_transitive_free_vars_in_expr(index, uses);
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            collect_transitive_free_vars_in_expr(target, uses);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_transitive_free_vars_in_expr(e, uses);
            }
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                collect_transitive_free_vars_in_expr(e, uses);
            }
        }
        Expr::Starred(inner) => collect_transitive_free_vars_in_expr(inner, uses),
        Expr::Dict(items) => {
            for item in items {
                match item {
                    DictItem::Pair(k, v) => {
                        collect_transitive_free_vars_in_expr(k, uses);
                        collect_transitive_free_vars_in_expr(v, uses);
                    }
                    DictItem::DoubleSplat(e) => collect_transitive_free_vars_in_expr(e, uses),
                }
            }
        }
        // All comprehension forms and generator expressions create an implicit
        // nested function scope.  For transitive free-var collection we treat
        // them like lambdas: compute the inner body's free-var reads, subtract
        // the names locally bound by the comprehension, then surface the
        // remainder as uses at the enclosing scope.
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            // Outermost iterable is evaluated at this scope level.
            if let Some(first) = clauses.first() {
                collect_transitive_free_vars_in_expr(&first.iter, uses);
                collect_free_var_reads_in_expr(&first.iter, uses);
            }
            // Inner body free vars (subtract clause-bound names).
            let mut inner_uses: HashSet<String> = HashSet::new();
            if let Some(first) = clauses.first()
                && let Some(c) = &first.cond
            {
                collect_free_var_reads_in_expr(c, &mut inner_uses);
                collect_transitive_free_vars_in_expr(c, &mut inner_uses);
            }
            for clause in clauses.iter().skip(1) {
                collect_free_var_reads_in_expr(&clause.iter, &mut inner_uses);
                collect_transitive_free_vars_in_expr(&clause.iter, &mut inner_uses);
                if let Some(c) = &clause.cond {
                    collect_free_var_reads_in_expr(c, &mut inner_uses);
                    collect_transitive_free_vars_in_expr(c, &mut inner_uses);
                }
            }
            collect_free_var_reads_in_expr(elt, &mut inner_uses);
            collect_transitive_free_vars_in_expr(elt, &mut inner_uses);
            let mut bound: HashSet<String> = HashSet::new();
            for clause in clauses {
                collect_written_target(&clause.target, &mut bound);
            }
            for name in inner_uses {
                if !bound.contains(&name) {
                    uses.insert(name);
                }
            }
        }
        Expr::DictComp { key, val, clauses } => {
            if let Some(first) = clauses.first() {
                collect_transitive_free_vars_in_expr(&first.iter, uses);
                collect_free_var_reads_in_expr(&first.iter, uses);
            }
            let mut inner_uses: HashSet<String> = HashSet::new();
            if let Some(first) = clauses.first()
                && let Some(c) = &first.cond
            {
                collect_free_var_reads_in_expr(c, &mut inner_uses);
                collect_transitive_free_vars_in_expr(c, &mut inner_uses);
            }
            for clause in clauses.iter().skip(1) {
                collect_free_var_reads_in_expr(&clause.iter, &mut inner_uses);
                collect_transitive_free_vars_in_expr(&clause.iter, &mut inner_uses);
                if let Some(c) = &clause.cond {
                    collect_free_var_reads_in_expr(c, &mut inner_uses);
                    collect_transitive_free_vars_in_expr(c, &mut inner_uses);
                }
            }
            collect_free_var_reads_in_expr(key, &mut inner_uses);
            collect_transitive_free_vars_in_expr(key, &mut inner_uses);
            collect_free_var_reads_in_expr(val, &mut inner_uses);
            collect_transitive_free_vars_in_expr(val, &mut inner_uses);
            let mut bound: HashSet<String> = HashSet::new();
            for clause in clauses {
                collect_written_target(&clause.target, &mut bound);
            }
            for name in inner_uses {
                if !bound.contains(&name) {
                    uses.insert(name);
                }
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_transitive_free_vars_in_expr(cond, uses);
            collect_transitive_free_vars_in_expr(then, uses);
            collect_transitive_free_vars_in_expr(else_, uses);
        }
        Expr::Named { value, .. } => collect_transitive_free_vars_in_expr(value, uses),
        Expr::FString(parts) => {
            for_each_fstring_expr(parts, &mut |e| {
                collect_transitive_free_vars_in_expr(e, uses)
            });
        }
        Expr::Var(_, _)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => {}
        Expr::Yield(Some(e)) => collect_transitive_free_vars_in_expr(e, uses),
        Expr::Yield(None) => {}
        Expr::YieldFrom(e) => collect_transitive_free_vars_in_expr(e, uses),
        Expr::Await(e) => collect_transitive_free_vars_in_expr(e, uses),
    }
}

// ─── Utilities ────────────────────────────────────────────────────────────────

fn const_eq(a: &Value, b: &Value) -> bool {
    use ValueKind::*;
    match (a.kind(), b.kind()) {
        (Int(x), Int(y)) => x == y,
        (BigInt(x), BigInt(y)) => x == y,
        (Float(x), Float(y)) => x.to_bits() == y.to_bits(),
        // Use bit-level comparison for complex parts so that NaN-keyed
        // constants are treated as the same pool entry (same as Float above).
        (Complex(ar, ai), Complex(br, bi)) => {
            ar.to_bits() == br.to_bits() && ai.to_bits() == bi.to_bits()
        }
        (Str(x), Str(y)) => x == y,
        (Bytes(x), Bytes(y)) => x.as_ref() == y.as_ref(),
        (Bool(x), Bool(y)) => x == y,
        (None, None) => true,
        _ => false,
    }
}

/// Attempt to evaluate a pure constant expression at compile time.
/// Returns Some(value) only when the entire expression tree consists of
/// literals and operations on literals that cannot raise.
fn fold_constant(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Int(v) => Some(Value::int(*v)),
        Expr::BigInt(s) => s.parse::<PyBigInt>().ok().map(Value::bigint),
        Expr::Float(v) => Some(Value::float(*v)),
        Expr::Str(s) => Some(Value::string(s.clone())),
        Expr::Bytes(b) => Some(Value::bytes(b.clone())),
        Expr::Complex(re, im) => Some(Value::complex(*re, *im)),
        Expr::Bool(b) => Some(Value::bool_(*b)),
        Expr::None => Some(Value::none()),
        Expr::Ellipsis => Some(Value::ellipsis()),
        Expr::Unary { op, expr, .. } => {
            let val = fold_constant(expr)?;
            match op {
                UnaryOp::Neg => match val.kind() {
                    // `-i64::MIN` overflows; promote to BigInt to match
                    // CPython's arbitrary-precision int semantics (#421).
                    ValueKind::Int(n) => Some(match n.checked_neg() {
                        Some(r) => Value::int(r),
                        None => Value::bigint(-PyBigInt::from(n)),
                    }),
                    ValueKind::Float(f) => Some(Value::float(-f)),
                    ValueKind::BigInt(b) => Some(Value::bigint(-b)),
                    _ => None,
                },
                UnaryOp::Not => Some(Value::bool_(!val.truthy_raw())),
                UnaryOp::BitNot => match val.kind() {
                    ValueKind::Int(n) => Some(Value::int(!n)),
                    _ => None,
                },
                _ => None,
            }
        }
        Expr::Binary {
            left, op, right, ..
        } => {
            let l = fold_constant(left)?;
            let r = fold_constant(right)?;
            fold_binop(&l, *op, &r)
        }
        Expr::Compare { left, ops } => {
            let mut cur = fold_constant(left)?;
            for (cmp_op, rhs_expr) in ops {
                let rhs = fold_constant(rhs_expr)?;
                let op = BinaryOp::from(*cmp_op);
                let result = fold_binop(&cur, op, &rhs)?;
                if !result.truthy_raw() {
                    return Some(Value::bool_(false));
                }
                cur = rhs;
            }
            Some(Value::bool_(true))
        }
        Expr::Named { .. } => None,
        _ => None,
    }
}

pub(crate) fn fold_binop(l: &Value, op: BinaryOp, r: &Value) -> Option<Value> {
    use BinaryOp::*;
    // Int/int arithmetic, shifts, and bitwise route through the single
    // canonical numeric implementation shared with `eval_binary` (issue
    // #458): one definition of overflow promotion, CPython floored `//` /
    // `%`, and shift/bitwise semantics.  A runtime error (e.g.
    // ZeroDivisionError on `x / 0`) returns `None` so the BinOp stays in
    // the bytecode and raises at runtime, never at compile time.
    if matches!((l.kind(), r.kind()), (ValueKind::Int(_), ValueKind::Int(_))) {
        match op {
            Add | Sub | Mul | Div | FloorDiv | Mod | BitAnd | BitOr | BitXor => {
                return dispatch_numeric_binop(op, l, r)?.ok();
            }
            // `**` and shifts can produce arbitrarily large constants
            // (`2 ** 1_000_000`, `1 << i64::MAX`).  Cap the magnitude here
            // so a hostile literal can't bloat the constant pool during
            // compilation; oversized cases fall through to the runtime
            // (which shares the same slot).
            Pow => {
                if let ValueKind::Int(b) = r.kind()
                    && (0..=u32::MAX as i64).contains(&b)
                {
                    return dispatch_numeric_binop(op, l, r)?.ok();
                }
                return None;
            }
            LShift | RShift => {
                if let ValueKind::Int(b) = r.kind()
                    && (0..=1_000_000).contains(&b)
                {
                    return dispatch_numeric_binop(op, l, r)?.ok();
                }
                return None;
            }
            _ => {}
        }
    }
    // Non-int-arithmetic folds the optimizer has always done: float
    // arithmetic, string concatenation, and constant comparisons.  These
    // are not numeric-slot arithmetic (comparisons) or involve non-int
    // operands, so they stay as explicit arms.
    match (l.kind(), op, r.kind()) {
        (ValueKind::Float(a), Add, ValueKind::Float(b)) => Some(Value::float(a + b)),
        (ValueKind::Float(a), Sub, ValueKind::Float(b)) => Some(Value::float(a - b)),
        (ValueKind::Float(a), Mul, ValueKind::Float(b)) => Some(Value::float(a * b)),
        (ValueKind::Float(a), Div, ValueKind::Float(b)) if b != 0.0 => Some(Value::float(a / b)),
        (ValueKind::Str(a), Add, ValueKind::Str(b)) => Some(Value::string(a.to_string() + b)),
        (ValueKind::Int(a), Eq, ValueKind::Int(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Int(a), Ne, ValueKind::Int(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Int(a), Lt, ValueKind::Int(b)) => Some(Value::bool_(a < b)),
        (ValueKind::Int(a), Le, ValueKind::Int(b)) => Some(Value::bool_(a <= b)),
        (ValueKind::Int(a), Gt, ValueKind::Int(b)) => Some(Value::bool_(a > b)),
        (ValueKind::Int(a), Ge, ValueKind::Int(b)) => Some(Value::bool_(a >= b)),
        (ValueKind::Str(a), Eq, ValueKind::Str(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Str(a), Ne, ValueKind::Str(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Bool(a), Eq, ValueKind::Bool(b)) => Some(Value::bool_(a == b)),
        _ => None,
    }
}

fn extract_literal_int(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(n) => Some(*n),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            ..
        } => {
            if let Expr::Int(n) = inner.as_ref() {
                Some(-n)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_const_false_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Bool(false) | Expr::Int(0) | Expr::None => true,
        Expr::Float(f) => *f == 0.0,
        _ => false,
    }
}

fn body_has_continue(body: &[Stmt]) -> bool {
    body.iter().any(stmt_has_continue)
}

fn stmt_has_continue(s: &Stmt) -> bool {
    match s {
        Stmt::Continue => true,
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches.iter().any(|(_, b)| body_has_continue(b))
                || else_branch.as_deref().is_some_and(body_has_continue)
        }
        Stmt::While { .. } | Stmt::For { .. } => false,
        _ => false,
    }
}

/// Compare two statements for structural equality.  Used by
/// `rewrite_continue_top` to detect the common suffix between an
/// `if guard: pre; continue` branch and the statements following the `if`
/// in the same loop body, so the suffix can be hoisted out and run once.
///
/// `Debug` is implemented for every AST node (derived), and the AST does not
/// carry mutable state, so the `{:?}` formatting is a safe and conservative
/// structural identity check.  The cost is paid at compile time only.
fn stmts_eq(a: &Stmt, b: &Stmt) -> bool {
    format!("{:?}", a) == format!("{:?}", b)
}

/// Length of the longest common trailing run of statements between `a` and `b`,
/// compared by structural equality (`stmts_eq`).
fn common_suffix_len(a: &[Stmt], b: &[Stmt]) -> usize {
    let mut k = 0usize;
    while k < a.len() && k < b.len() {
        let ai = &a[a.len() - 1 - k];
        let bi = &b[b.len() - 1 - k];
        if !stmts_eq(ai, bi) {
            break;
        }
        k += 1;
    }
    k
}

/// Rewrite a loop body to collapse the `if guard: ... ; continue` trampoline
/// (issue #287, sibling of #282 for `break`).
///
/// When the loop body contains a single-branch `if guard: <pre…>; continue`
/// (no else, the if-body ends with an unconditional `Continue`), the literal
/// lowering emits `JumpIfFalse(guard, +1) + Jump(loop_start)` — a redundant
/// two-instruction trampoline whose taken/fallthrough roles are inverted from
/// the common case (the body runs much more often than the continue fires).
///
/// We rewrite to either:
/// - `if not guard: <rest>` when `<pre>` is empty (lowers to a single
///   `JumpIfTrue(guard, loop_start)` after `pass_not_invert` + `pass_thread_jumps`).
/// - `if guard: A_pre else: B_pre ; <suffix>` when `<pre>` shares a non-empty
///   common trailing run with the rest-of-loop body (hoist the shared tail
///   out so it runs once, regardless of branch).  In the empty-A_pre subcase
///   this collapses to `if not guard: B_pre ; <suffix>`.  This is the
///   "hoist tail out" variant explicitly suggested by the issue body.
///
/// After hoisting, the loop body may end with `i += 1` again, which lets
/// `try_compile_while_range` promote the loop to a `ForCount*` counter
/// (the canonical fast path it would have taken if the `if-continue` were
/// not present).
fn rewrite_continue_top(body: Vec<Stmt>) -> Vec<Stmt> {
    let mut out: Vec<Stmt> = Vec::with_capacity(body.len());
    let mut iter = body.into_iter();
    while let Some(stmt) = iter.next() {
        // Match `if guard: <body>` with single branch, no else, body ending
        // in Continue.
        let matches_pattern = matches!(
            &stmt,
            Stmt::If { branches, else_branch: None, .. }
                if branches.len() == 1
                    && branches[0].1.last().is_some_and(|s| matches!(s, Stmt::Continue))
        );
        if !matches_pattern {
            out.push(stmt);
            continue;
        }

        // Decompose the matched if into (guard, if_body_without_trailing_continue).
        let (guard, mut a_body) = match stmt {
            Stmt::If { mut branches, .. } => {
                let (g, mut b) = branches.swap_remove(0);
                b.pop(); // discard the trailing Continue (it's a no-op at loop tail)
                (g, b)
            }
            _ => unreachable!(),
        };

        // Collect the rest of the loop body (this is the implicit "else" arm).
        let mut b_body: Vec<Stmt> = iter.by_ref().collect();
        // Recurse so that consecutive `if a: continue; if b: continue; rest`
        // chains collapse one trampoline at a time.
        b_body = rewrite_continue_top(b_body);

        // Hoist the longest common trailing run of statements out of both arms.
        let k = common_suffix_len(&a_body, &b_body);
        let suffix: Vec<Stmt> = a_body.split_off(a_body.len() - k);
        b_body.truncate(b_body.len() - k);

        // Build the new if-statement.  Drop the if entirely when both arms are
        // empty AND the guard is a pure name/literal (no side effect to preserve).
        let drop_guard = a_body.is_empty() && b_body.is_empty() && expr_is_side_effect_free(&guard);
        if drop_guard {
            // Branch is pure noise: the guard has no side effects and both arms
            // are empty after suffix hoisting.  Skip the if entirely.
        } else if a_body.is_empty() {
            // `if not guard: B_pre`
            out.push(Stmt::If {
                branches: vec![(
                    Expr::Unary {
                        op: UnaryOp::Not,
                        expr: Box::new(guard),
                        span: None,
                    },
                    b_body,
                )],
                else_branch: None,
                branch_linenos: vec![],
                else_linenos: vec![],
            });
        } else if b_body.is_empty() {
            // `if guard: A_pre` (the else is empty so don't emit it).
            out.push(Stmt::If {
                branches: vec![(guard, a_body)],
                else_branch: None,
                branch_linenos: vec![],
                else_linenos: vec![],
            });
        } else {
            // `if guard: A_pre else: B_pre`
            out.push(Stmt::If {
                branches: vec![(guard, a_body)],
                else_branch: Some(b_body),
                branch_linenos: vec![],
                else_linenos: vec![],
            });
        }
        out.extend(suffix);
        return out;
    }
    out
}

/// Conservatively determine whether evaluating `expr` can have observable
/// side effects.  Only used by `rewrite_continue_top` to decide whether the
/// guard of an emptied if-statement can be discarded.
fn expr_is_side_effect_free(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis
        | Expr::Var(_, _) => true,
        Expr::Unary { expr, .. } => expr_is_side_effect_free(expr),
        Expr::Binary { left, right, .. } => {
            expr_is_side_effect_free(left) && expr_is_side_effect_free(right)
        }
        Expr::Compare { left, ops } => {
            expr_is_side_effect_free(left) && ops.iter().all(|(_, e)| expr_is_side_effect_free(e))
        }
        _ => false,
    }
}

/// Rewrite `while True: if c: break; rest` to `while not c: rest` (issue #282,
/// sibling of #287 for `continue`).
///
/// The literal lowering of `if c: break` at the top of an infinite loop emits a
/// `JumpIfFalse(c, +1) + Jump(loop_exit)` two-instruction trampoline whose
/// taken/fallthrough roles are inverted from the common case (the body runs
/// much more often than the break fires).  Inverting the test into the loop
/// condition collapses the trampoline to a single `JumpIfTrue(c, loop_exit)`
/// AND lets `try_compile_while_range` see the canonical `while cond: body; i +=
/// 1` shape, which promotes to `ForCountConstInline` (issue #256) when the
/// body is an inductive counter.
///
/// Returns `Some((not_c, rest))` when the rewrite fires, otherwise `None`.
/// The caller is responsible for ensuring the original loop was infinite
/// (`while True:`) — the `else` branch must already be confirmed absent /
/// dead, since after the rewrite the `c-becomes-true` exit is "natural" and
/// would otherwise resurrect the else.
///
/// Conservative guards (mirroring `rewrite_continue_top`):
/// - the if has exactly one branch (no else) and that branch is exactly `[Break]`
/// - the guard `c` is evaluated once per iteration in both shapes, so its side
///   effects are preserved — no side-effect-free check is required here
/// - we only fire when the original body has at least one statement after the
///   `if c: break` (otherwise the loop is `while True: if c: break` with empty
///   rest, i.e. a busy spin until c — rewriting still works and is harmless,
///   so we allow that case too).
fn rewrite_break_top(body: Vec<Stmt>) -> Option<(Expr, Vec<Stmt>)> {
    let matches_pattern = matches!(
        body.first(),
        Some(Stmt::If { branches, else_branch: None, .. })
            if branches.len() == 1
                && matches!(branches[0].1.as_slice(), [Stmt::Break])
    );
    if !matches_pattern {
        return None;
    }
    let mut iter = body.into_iter();
    let first = iter.next().unwrap();
    let guard = match first {
        Stmt::If { mut branches, .. } => branches.swap_remove(0).0,
        _ => unreachable!(),
    };
    let rest: Vec<Stmt> = iter.collect();
    Some((negate_expr(guard), rest))
}

/// Negate a boolean expression, folding `not (a CMP b)` into the inverted
/// comparison so `try_compile_while_range` (and other shape detectors that key
/// off `Expr::Binary { op: Lt|Le|Gt|Ge, ... }`) can match the rewritten loop.
/// Falls back to `Expr::Unary { Not, expr }` for non-comparison guards.
fn negate_expr(expr: Expr) -> Expr {
    match expr {
        Expr::Binary {
            left, op, right, ..
        } => {
            let inverted = match op {
                BinaryOp::Lt => Some(BinaryOp::Ge),
                BinaryOp::Le => Some(BinaryOp::Gt),
                BinaryOp::Gt => Some(BinaryOp::Le),
                BinaryOp::Ge => Some(BinaryOp::Lt),
                BinaryOp::Eq => Some(BinaryOp::Ne),
                BinaryOp::Ne => Some(BinaryOp::Eq),
                _ => None,
            };
            match inverted {
                Some(inv) => Expr::Binary {
                    left,
                    op: inv,
                    right,
                    span: None,
                },
                None => Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(Expr::Binary {
                        left,
                        op,
                        right,
                        span: None,
                    }),
                    span: None,
                },
            }
        }
        other => Expr::Unary {
            op: UnaryOp::Not,
            expr: Box::new(other),
            span: None,
        },
    }
}

fn collect_body_written(body: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_written_in(body, &mut names);
    names
}

fn collect_written_in(body: &[Stmt], names: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Assign(target, _) | Stmt::AugAssign { target, .. } => {
                collect_written_target(target, names);
            }
            Stmt::AnnAssign {
                name,
                value: Some(_),
                ..
            } => {
                names.insert(name.clone());
            }
            Stmt::AnnAssign { value: None, .. } => {}
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    collect_written_in(b, names);
                }
                if let Some(b) = else_branch {
                    collect_written_in(b, names);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_written_in(body, names);
                if let Some(b) = else_branch {
                    collect_written_in(b, names);
                }
            }
            Stmt::For {
                target,
                body,
                else_branch,
                ..
            } => {
                collect_written_target(target, names);
                collect_written_in(body, names);
                if let Some(b) = else_branch {
                    collect_written_in(b, names);
                }
            }
            _ => {}
        }
    }
}

fn collect_written_target(target: &AssignTarget, names: &mut HashSet<String>) {
    match target {
        AssignTarget::Name(n) => {
            names.insert(n.clone());
        }
        AssignTarget::Tuple(ts) => {
            for t in ts {
                collect_written_target(t, names);
            }
        }
        AssignTarget::Starred(inner) => {
            collect_written_target(inner, names);
        }
        _ => {}
    }
}

fn expr_is_invariant(expr: &Expr, written: &HashSet<String>) -> bool {
    match expr {
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Complex(_, _)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => true,
        Expr::Var(name, _) => !written.contains(name.as_str()),
        Expr::Binary { left, right, .. } => {
            expr_is_invariant(left, written) && expr_is_invariant(right, written)
        }
        Expr::Unary { expr, .. } => expr_is_invariant(expr, written),
        // NamedExpr has a side effect (assignment), never invariant
        Expr::Named { .. } => false,
        _ => false,
    }
}

/// Return `true` if any statement in `body` (recursively, including nested
/// blocks and sub-expressions) could mutate a Python object **in place**.
///
/// Used to keep `while`-condition LICM sound (issue #2034): in-place mutation
/// (`x.pop()`, `x[i] = v`, `del x[i]`, slice/attr assignment, …) changes a
/// container's truthiness without ever reassigning the loop variable's register,
/// so the hoisted condition check would observe a stale value.  Because the
/// mutated object may alias the condition variable in ways we cannot disprove
/// statically, this is intentionally conservative: any in-place mutation in the
/// body disqualifies the loop from LICM.  Pure-name reassignments (`i += 1`,
/// `x = ...`) are *not* mutations — they are already tracked by
/// `collect_body_written`.
fn stmts_may_mutate_object(body: &[Stmt]) -> bool {
    body.iter().any(stmt_may_mutate_object)
}

fn stmt_may_mutate_object(stmt: &Stmt) -> bool {
    match stmt {
        // Direct in-place mutation of a subscript / attribute / slice target.
        Stmt::IndexAssign { .. }
        | Stmt::AttrAssign { .. }
        | Stmt::SliceAssign { .. }
        | Stmt::Delete(_) => true,
        // Augmented assignment to a subscript/attribute/slice mutates in place
        // (e.g. `x[0] += 1`); to a bare name it is a plain reassignment.
        Stmt::AugAssign { target, expr, .. } => {
            !matches!(target, AssignTarget::Name(_))
                || expr_may_mutate_object(expr)
                || assign_target_may_mutate_object(target)
        }
        Stmt::Assign(target, value) => {
            assign_target_may_mutate_object(target) || expr_may_mutate_object(value)
        }
        Stmt::AnnAssign { value, .. } => value.as_ref().is_some_and(expr_may_mutate_object),
        Stmt::Expr(e) | Stmt::Return(Some(e)) => expr_may_mutate_object(e),
        Stmt::Return(None) | Stmt::Pass | Stmt::Break | Stmt::Continue => false,
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches
                .iter()
                .any(|(c, b)| expr_may_mutate_object(c) || stmts_may_mutate_object(b))
                || else_branch.as_deref().is_some_and(stmts_may_mutate_object)
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            expr_may_mutate_object(cond)
                || stmts_may_mutate_object(body)
                || else_branch.as_deref().is_some_and(stmts_may_mutate_object)
        }
        Stmt::For {
            iter,
            body,
            else_branch,
            ..
        } => {
            expr_may_mutate_object(iter)
                || stmts_may_mutate_object(body)
                || else_branch.as_deref().is_some_and(stmts_may_mutate_object)
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            stmts_may_mutate_object(body)
                || handlers.iter().any(|h| stmts_may_mutate_object(&h.body))
                || else_branch.as_deref().is_some_and(stmts_may_mutate_object)
                || finally_branch
                    .as_deref()
                    .is_some_and(stmts_may_mutate_object)
        }
        Stmt::With { items, body, .. } => {
            items.iter().any(|(e, _)| expr_may_mutate_object(e)) || stmts_may_mutate_object(body)
        }
        Stmt::Match { subject, arms } => {
            expr_may_mutate_object(subject)
                || arms.iter().any(|a| {
                    a.guard.as_ref().is_some_and(expr_may_mutate_object)
                        || stmts_may_mutate_object(&a.body)
                })
        }
        Stmt::Raise { expr, cause, .. } => {
            expr.as_ref().is_some_and(expr_may_mutate_object)
                || cause.as_ref().is_some_and(expr_may_mutate_object)
        }
        Stmt::Assert { test, msg } => {
            expr_may_mutate_object(test) || msg.as_ref().is_some_and(expr_may_mutate_object)
        }
        // A nested def/class body does not execute during the loop; only its
        // defaults/decorators (evaluated at definition time) could mutate, but
        // those run once and reassign the name — treat the definition itself as
        // non-mutating for the purpose of the enclosing loop condition.
        Stmt::Def { .. } | Stmt::Class { .. } => false,
        Stmt::TypeAlias { .. } => false,
        Stmt::Import { .. } | Stmt::ImportFrom { .. } | Stmt::Global(_) | Stmt::Nonlocal(_) => {
            false
        }
    }
}

fn assign_target_may_mutate_object(target: &AssignTarget) -> bool {
    match target {
        // Storing into a subscript / attribute / slice mutates the container.
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => true,
        AssignTarget::Name(_) => false,
        AssignTarget::Tuple(ts) => ts.iter().any(assign_target_may_mutate_object),
        AssignTarget::Starred(inner) => assign_target_may_mutate_object(inner),
    }
}

/// Return `true` if evaluating `expr` could mutate a Python object in place.
///
/// The dominant vector is a method call (`x.append(…)`, `x.pop()`): any call
/// whose callee is an attribute access is treated as a potential mutator, as is
/// any call at all (a plain function could mutate an argument), plus walrus
/// assignments to non-name targets are impossible here, so we focus on calls
/// and recurse structurally.  Pure reads (`Var`, literals, arithmetic on
/// immutables) are not mutations.
fn expr_may_mutate_object(expr: &Expr) -> bool {
    match expr {
        // Any call may mutate its receiver/arguments in place.
        Expr::Call { .. } => true,
        Expr::Named { value, .. } => expr_may_mutate_object(value),
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis
        | Expr::Var(_, _) => false,
        Expr::Unary { expr, .. } => expr_may_mutate_object(expr),
        Expr::Binary { left, right, .. } => {
            expr_may_mutate_object(left) || expr_may_mutate_object(right)
        }
        Expr::Compare { left, ops } => {
            expr_may_mutate_object(left) || ops.iter().any(|(_, e)| expr_may_mutate_object(e))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_may_mutate_object(cond)
                || expr_may_mutate_object(then)
                || expr_may_mutate_object(else_)
        }
        Expr::Attr { target, .. } => expr_may_mutate_object(target),
        Expr::Index { target, index, .. } => {
            expr_may_mutate_object(target) || expr_may_mutate_object(index)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_may_mutate_object(target)
                || [lower, upper, step]
                    .iter()
                    .flat_map(|o| o.as_deref())
                    .any(expr_may_mutate_object)
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().any(expr_may_mutate_object)
        }
        Expr::Starred(inner) => expr_may_mutate_object(inner),
        Expr::Dict(items) => items.iter().any(|item| match item {
            DictItem::Pair(k, v) => expr_may_mutate_object(k) || expr_may_mutate_object(v),
            DictItem::DoubleSplat(e) => expr_may_mutate_object(e),
        }),
        Expr::FString(parts) => any_fstring_expr(parts, &mut expr_may_mutate_object),
        // Comprehensions/generators run in a nested scope; only the outermost
        // iterable is evaluated here.  A comprehension allocates a fresh object
        // and does not mutate an existing one, but its element/condition exprs
        // may contain calls — conservatively treat any comprehension as a
        // potential mutator (it is rare in a loop condition's sibling body and
        // the cost is only a missed LICM).
        Expr::ListComp { .. }
        | Expr::SetComp { .. }
        | Expr::DictComp { .. }
        | Expr::GenExp { .. } => true,
        // Lambda body is not evaluated here.
        Expr::Lambda { .. } => false,
        // Yield/await suspend; treat conservatively as potential mutators.
        Expr::Yield(_) | Expr::YieldFrom(_) | Expr::Await(_) => true,
    }
}

/// Try to rewrite `while i < len(c): ...; c[i]; ...; i += 1` into `for i in c: ...`
/// for the matched-pattern `while` at `stmts[idx]`.  Returns `Some(rewritten_for)`
/// when the rewrite is safe; `None` otherwise.
///
/// This promotes the manual-indexed iteration pattern to a `ForIter` loop
/// (issue #289), avoiding the per-iteration `len(c)` bound check and the
/// subscript-by-register-index that the `ForCountReg` lowering still needs.
///
/// We reuse the existing index variable `i` as the loop variable in the
/// rewritten `for` — Python's `for i in c:` already binds the same name, so no
/// fresh fastlocal is required.  This is safe ONLY when `i` is never read
/// after the loop in the same block (and never read in the body in any form
/// other than `c[i]`), since the for-loop leaves `i` equal to the last element
/// value rather than `len(c)`.
///
/// Bail-out conditions (return `None`):
/// - `cond` is not `Var(i) < len(Var(c))` with a single positional arg.
/// - The trailing increment is missing, has step != 1, or assigns to a
///   different variable.
/// - `i` is assigned anywhere else in the body (other than the trailing
///   increment).
/// - `c` is assigned anywhere in the body (would change iteration).
/// - `c[i] = ...` (index-assign) or `del c[i]` appears in the body.
/// - `i` is read in the body in any form other than `Subscript(Var(c), Var(i))`.
/// - `i` is read in any of the statements after the loop in the same block.
/// - The body contains `break` or `continue` (the original program with a
///   `continue` is buggy — infinite loop — and a `break` in the rewritten
///   for-loop has slightly different `i` semantics; conservatively bail).
/// - `len` or `c` is shadowed as a local (we approximate by requiring `c` to
///   be a `Var(name)`; `len` shadowing inside the loop body would be caught
///   by the "i only appears as c[i]" check since shadowing requires assigning
///   to `len`, which the c-mutation check covers).
fn try_rewrite_while_index_to_for(stmts: &[Stmt], idx: usize) -> Option<Stmt> {
    let stmt = stmts.get(idx)?;
    let (cond, body, else_branch) = match stmt {
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => (cond, body, else_branch),
        _ => return None,
    };
    // Don't rewrite when there's an `else:` clause — Python's `for-else` runs
    // when the loop exits naturally (same semantics as `while-else`), but
    // we'd need to preserve it through the rewrite and it adds complexity
    // for a rare pattern.  Conservatively bail.
    if else_branch.is_some() {
        return None;
    }

    // Match `Var(i) < len(Var(c))` (single positional arg).
    let (i_name, c_name) = match cond {
        Expr::Binary {
            op: BinaryOp::Lt,
            left,
            right,
            ..
        } => {
            let i = match left.as_ref() {
                Expr::Var(n, _) => n.clone(),
                _ => return None,
            };
            let c = match right.as_ref() {
                Expr::Call { func, args, .. } => {
                    if !matches!(func.as_ref(), Expr::Var(f, _) if f == "len") {
                        return None;
                    }
                    if args.len() != 1 {
                        return None;
                    }
                    let a = &args[0];
                    if a.splat || a.double_splat || a.name.is_some() {
                        return None;
                    }
                    match &a.value {
                        Expr::Var(n, _) => n.clone(),
                        _ => return None,
                    }
                }
                _ => return None,
            };
            (i, c)
        }
        _ => return None,
    };
    // Cannot rewrite when the index and collection share the same name.
    if i_name == c_name {
        return None;
    }

    // Body must end with `i += 1` (or `i = i + 1`).
    let inc_idx = body.len().checked_sub(1)?;
    let inc_ok = match &body[inc_idx] {
        Stmt::AugAssign {
            target: AssignTarget::Name(t),
            op: BinaryOp::Add,
            expr: Expr::Int(1),
        } => t == &i_name,
        Stmt::Assign(
            AssignTarget::Name(t),
            Expr::Binary {
                op: BinaryOp::Add,
                left,
                right,
                ..
            },
        ) => {
            t == &i_name
                && matches!(left.as_ref(), Expr::Var(n, _) if n == &i_name)
                && matches!(right.as_ref(), Expr::Int(1))
        }
        _ => false,
    };
    if !inc_ok {
        return None;
    }
    let body_without_inc = &body[..inc_idx];

    // Safety analysis on the body (excluding the trailing increment):
    // - `i` is read ONLY as Subscript(Var(c), Var(i))
    // - `i` is NOT assigned anywhere
    // - `c` is NOT assigned anywhere
    // - No `c[i] = ...` index-assign, no `del c[i]`
    // - No `break` or `continue`
    if !body_index_pattern_is_safe(body_without_inc, &i_name, &c_name) {
        return None;
    }

    // Post-loop usage: `i` must not be read in any statement after the while.
    for s in &stmts[idx + 1..] {
        if stmt_reads_var(s, &i_name) {
            return None;
        }
    }

    // Build the rewritten body: replace each `Subscript(Var(c), Var(i))` with
    // `Var(i)`.  The new for-loop binds `i` to the element value.
    let mut new_body: Vec<Stmt> = Vec::with_capacity(body_without_inc.len());
    for s in body_without_inc {
        let mut s2 = s.clone();
        rewrite_c_at_i_in_stmt(&mut s2, &c_name, &i_name);
        new_body.push(s2);
    }

    Some(Stmt::For {
        target: AssignTarget::Name(i_name),
        iter: Expr::Var(c_name, None),
        body: new_body,
        else_branch: None,
        body_linenos: vec![],
        else_linenos: vec![],
        is_async: false,
    })
}

/// Check the body of a candidate `while i < len(c)` loop for the conservative
/// rewrite to `for i in c`.  Returns `true` only if every read of `i` is
/// `Subscript(Var(c), Var(i))`, `i` and `c` are never assigned/deleted, and
/// the body contains no `break`/`continue` or `c[i] = ...` / `del c[i]`.
fn body_index_pattern_is_safe(body: &[Stmt], i_name: &str, c_name: &str) -> bool {
    for s in body {
        if !stmt_safe_for_index_rewrite(s, i_name, c_name) {
            return false;
        }
    }
    true
}

fn stmt_safe_for_index_rewrite(stmt: &Stmt, i_name: &str, c_name: &str) -> bool {
    match stmt {
        Stmt::Break | Stmt::Continue => false,
        // Assigning to `i` or `c` would break the rewrite invariant.
        Stmt::Assign(target, value) => {
            if target_assigns(target, i_name) || target_assigns(target, c_name) {
                return false;
            }
            target_safe_for_rewrite(target, i_name, c_name) && expr_safe(value, i_name, c_name)
        }
        Stmt::AugAssign { target, expr, .. } => {
            if target_assigns(target, i_name) || target_assigns(target, c_name) {
                return false;
            }
            target_safe_for_rewrite(target, i_name, c_name) && expr_safe(expr, i_name, c_name)
        }
        Stmt::IndexAssign {
            target,
            index,
            expr,
        } => {
            // Disallow `c[i] = ...`: the rewritten for-loop sees the snapshot
            // value, not the live `c[i]` slot, so the mutation would not be
            // observable to subsequent reads inside the iteration.
            if is_c_at_i_expr(target, index, c_name, i_name) {
                return false;
            }
            expr_safe(target, i_name, c_name)
                && expr_safe(index, i_name, c_name)
                && expr_safe(expr, i_name, c_name)
        }
        Stmt::SliceAssign {
            target,
            lower,
            upper,
            step,
            expr,
        } => {
            // Slice-assigning into `c` could change its length, breaking iter.
            if matches!(target.as_ref(), Expr::Var(n, _) if n == c_name) {
                return false;
            }
            expr_safe(target, i_name, c_name)
                && lower
                    .as_deref()
                    .is_none_or(|e| expr_safe(e, i_name, c_name))
                && upper
                    .as_deref()
                    .is_none_or(|e| expr_safe(e, i_name, c_name))
                && step.as_deref().is_none_or(|e| expr_safe(e, i_name, c_name))
                && expr_safe(expr, i_name, c_name)
        }
        Stmt::AttrAssign {
            target,
            name: _,
            expr,
            ..
        } => expr_safe(target, i_name, c_name) && expr_safe(expr, i_name, c_name),
        Stmt::Expr(e) => expr_safe(e, i_name, c_name),
        Stmt::Return(e) => e.as_ref().is_none_or(|x| expr_safe(x, i_name, c_name)),
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            for (cond, b) in branches {
                if !expr_safe(cond, i_name, c_name)
                    || !body_index_pattern_is_safe(b, i_name, c_name)
                {
                    return false;
                }
            }
            else_branch
                .as_deref()
                .is_none_or(|b| body_index_pattern_is_safe(b, i_name, c_name))
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            // Nested loop has its own `break`/`continue`, which don't target
            // the outer loop, so we don't need to bail on them here.
            expr_safe(cond, i_name, c_name)
                && nested_loop_body_safe(body, i_name, c_name)
                && else_branch
                    .as_deref()
                    .is_none_or(|b| body_index_pattern_is_safe(b, i_name, c_name))
        }
        Stmt::For {
            target,
            iter,
            body,
            else_branch,
            ..
        } => {
            // Iterating with target `i` or `c` would rebind them.
            if target_assigns(target, i_name) || target_assigns(target, c_name) {
                return false;
            }
            target_safe_for_rewrite(target, i_name, c_name)
                && expr_safe(iter, i_name, c_name)
                && nested_loop_body_safe(body, i_name, c_name)
                && else_branch
                    .as_deref()
                    .is_none_or(|b| body_index_pattern_is_safe(b, i_name, c_name))
        }
        Stmt::Pass | Stmt::Global(_) | Stmt::Nonlocal(_) => true,
        Stmt::AnnAssign {
            name,
            value: Some(value),
            ..
        } => {
            // Annotated assignment to `i` or `c` breaks the rewrite invariant
            // (same reason as plain Assign).
            if name == i_name || name == c_name {
                return false;
            }
            expr_safe(value, i_name, c_name)
        }
        Stmt::AnnAssign { value: None, .. } => true,
        Stmt::Delete(exprs) => {
            // `del c[i]` is unsafe; any other delete is OK as long as it
            // doesn't reference i in a bare way.
            for e in exprs {
                if let Expr::Index { target, index, .. } = e
                    && is_c_at_i_expr(target, index, c_name, i_name)
                {
                    return false;
                }
                // `del i` and `del c` would change semantics.
                if let Expr::Var(n, _) = e
                    && (n == i_name || n == c_name)
                {
                    return false;
                }
                if !expr_safe(e, i_name, c_name) {
                    return false;
                }
            }
            true
        }
        Stmt::Assert { test, msg } => {
            expr_safe(test, i_name, c_name)
                && msg.as_ref().is_none_or(|m| expr_safe(m, i_name, c_name))
        }
        Stmt::Raise { expr, cause, .. } => {
            expr.as_ref().is_none_or(|e| expr_safe(e, i_name, c_name))
                && cause.as_ref().is_none_or(|e| expr_safe(e, i_name, c_name))
        }
        // Conservatively bail on anything that might re-bind or capture `i`/`c`:
        // try/with/match/import/def/class.  These are uncommon inside the hot
        // sentinel-break pattern; leaving them out keeps the analysis tractable.
        _ => false,
    }
}

/// Like `body_index_pattern_is_safe` but for nested loop bodies, where
/// `break`/`continue` are bound to the nested loop and therefore fine.
fn nested_loop_body_safe(body: &[Stmt], i_name: &str, c_name: &str) -> bool {
    for s in body {
        match s {
            Stmt::Break | Stmt::Continue => continue,
            other => {
                if !stmt_safe_for_index_rewrite(other, i_name, c_name) {
                    return false;
                }
            }
        }
    }
    true
}

/// `true` if `expr_safe`: every read of `i_name` is `Subscript(Var(c), Var(i))`
/// (replaced wholesale by the rewrite), and `c_name` only appears as the
/// container in such subscripts or in other reads that don't escape the loop.
///
/// We allow `c` to be read as a bare name elsewhere only if it's clearly a
/// read (no assignment to it).  The mutation check is in
/// `stmt_safe_for_index_rewrite`.
fn expr_safe(expr: &Expr, i_name: &str, c_name: &str) -> bool {
    match expr {
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => true,
        Expr::Var(n, _) => {
            // A bare reference to `i` outside of `c[i]` would still need to
            // see the index, not the value — bail.  Bare `c` reads are fine.
            n != i_name
        }
        Expr::Index { target, index, .. } => {
            // `c[i]` is the canonical safe shape: rewritten to a bare `i`.
            if is_c_at_i_expr(target, index, c_name, i_name) {
                return true;
            }
            expr_safe(target, i_name, c_name) && expr_safe(index, i_name, c_name)
        }
        Expr::Attr { target, .. } => expr_safe(target, i_name, c_name),
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_safe(target, i_name, c_name)
                && lower
                    .as_deref()
                    .is_none_or(|e| expr_safe(e, i_name, c_name))
                && upper
                    .as_deref()
                    .is_none_or(|e| expr_safe(e, i_name, c_name))
                && step.as_deref().is_none_or(|e| expr_safe(e, i_name, c_name))
        }
        Expr::Unary { expr, .. } => expr_safe(expr, i_name, c_name),
        Expr::Binary { left, right, .. } => {
            expr_safe(left, i_name, c_name) && expr_safe(right, i_name, c_name)
        }
        Expr::Compare { left, ops } => {
            expr_safe(left, i_name, c_name) && ops.iter().all(|(_, e)| expr_safe(e, i_name, c_name))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_safe(cond, i_name, c_name)
                && expr_safe(then, i_name, c_name)
                && expr_safe(else_, i_name, c_name)
        }
        Expr::Call { func, args, .. } => {
            expr_safe(func, i_name, c_name)
                && args.iter().all(|a| expr_safe(&a.value, i_name, c_name))
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().all(|e| expr_safe(e, i_name, c_name))
        }
        Expr::Starred(inner) => expr_safe(inner, i_name, c_name),
        Expr::Dict(items) => items.iter().all(|item| match item {
            DictItem::Pair(k, v) => expr_safe(k, i_name, c_name) && expr_safe(v, i_name, c_name),
            DictItem::DoubleSplat(e) => expr_safe(e, i_name, c_name),
        }),
        Expr::FString(parts) => all_fstring_exprs(parts, &mut |e| expr_safe(e, i_name, c_name)),
        // Conservatively bail on anything that captures or re-evaluates
        // expressions in a non-trivial way: comprehensions (they have their
        // own scope and could shadow), lambdas, walrus, yield.
        Expr::ListComp { .. }
        | Expr::DictComp { .. }
        | Expr::SetComp { .. }
        | Expr::GenExp { .. }
        | Expr::Lambda { .. }
        | Expr::Named { .. }
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Await(_) => false,
    }
}

fn is_c_at_i_expr(target: &Expr, index: &Expr, c_name: &str, i_name: &str) -> bool {
    matches!(target, Expr::Var(n, _) if n == c_name)
        && matches!(index, Expr::Var(n, _) if n == i_name)
}

fn target_assigns(target: &AssignTarget, name: &str) -> bool {
    match target {
        AssignTarget::Name(n) => n == name,
        AssignTarget::Tuple(ts) => ts.iter().any(|t| target_assigns(t, name)),
        AssignTarget::Starred(t) => target_assigns(t, name),
        _ => false,
    }
}

/// Verify a non-Name assignment target doesn't read `i`/`c` in unsafe ways.
fn target_safe_for_rewrite(target: &AssignTarget, i_name: &str, c_name: &str) -> bool {
    match target {
        AssignTarget::Name(_) => true,
        AssignTarget::Attr(t, _, _) => expr_safe(t, i_name, c_name),
        AssignTarget::Index(t, idx) => {
            // `c[i] = ...` is handled at the Stmt::IndexAssign site; here we
            // only see this via `IndexAssign`/`SliceAssign` containers — never
            // reached in practice, but keep it sound.
            expr_safe(t, i_name, c_name) && expr_safe(idx, i_name, c_name)
        }
        AssignTarget::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_safe(target, i_name, c_name)
                && lower
                    .as_deref()
                    .is_none_or(|e| expr_safe(e, i_name, c_name))
                && upper
                    .as_deref()
                    .is_none_or(|e| expr_safe(e, i_name, c_name))
                && step.as_deref().is_none_or(|e| expr_safe(e, i_name, c_name))
        }
        AssignTarget::Tuple(ts) => ts
            .iter()
            .all(|t| target_safe_for_rewrite(t, i_name, c_name)),
        AssignTarget::Starred(t) => target_safe_for_rewrite(t, i_name, c_name),
    }
}

/// `true` if `stmt` (recursively, but not crossing function/class scopes)
/// contains a read of `name`.  Used to verify post-loop non-use of `i`.
fn stmt_reads_var(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::Assign(_, e) => expr_reads_var(e, name),
        Stmt::AttrAssign { target, expr, .. } => {
            expr_reads_var(target, name) || expr_reads_var(expr, name)
        }
        Stmt::IndexAssign {
            target,
            index,
            expr,
        } => {
            expr_reads_var(target, name)
                || expr_reads_var(index, name)
                || expr_reads_var(expr, name)
        }
        Stmt::SliceAssign {
            target,
            lower,
            upper,
            step,
            expr,
        } => {
            expr_reads_var(target, name)
                || lower.as_deref().is_some_and(|e| expr_reads_var(e, name))
                || upper.as_deref().is_some_and(|e| expr_reads_var(e, name))
                || step.as_deref().is_some_and(|e| expr_reads_var(e, name))
                || expr_reads_var(expr, name)
        }
        Stmt::AugAssign { expr, .. } => expr_reads_var(expr, name),
        Stmt::Expr(e) => expr_reads_var(e, name),
        Stmt::Return(Some(e)) => expr_reads_var(e, name),
        Stmt::Return(None) => false,
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches
                .iter()
                .any(|(c, b)| expr_reads_var(c, name) || b.iter().any(|s| stmt_reads_var(s, name)))
                || else_branch
                    .as_deref()
                    .is_some_and(|b| b.iter().any(|s| stmt_reads_var(s, name)))
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            expr_reads_var(cond, name)
                || body.iter().any(|s| stmt_reads_var(s, name))
                || else_branch
                    .as_deref()
                    .is_some_and(|b| b.iter().any(|s| stmt_reads_var(s, name)))
        }
        Stmt::For {
            iter,
            body,
            else_branch,
            ..
        } => {
            expr_reads_var(iter, name)
                || body.iter().any(|s| stmt_reads_var(s, name))
                || else_branch
                    .as_deref()
                    .is_some_and(|b| b.iter().any(|s| stmt_reads_var(s, name)))
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            body.iter().any(|s| stmt_reads_var(s, name))
                || handlers.iter().any(|h| {
                    h.kind.as_ref().is_some_and(|e| expr_reads_var(e, name))
                        || h.body.iter().any(|s| stmt_reads_var(s, name))
                })
                || else_branch
                    .as_deref()
                    .is_some_and(|b| b.iter().any(|s| stmt_reads_var(s, name)))
                || finally_branch
                    .as_deref()
                    .is_some_and(|b| b.iter().any(|s| stmt_reads_var(s, name)))
        }
        Stmt::With { items, body, .. } => {
            items.iter().any(|(e, _)| expr_reads_var(e, name))
                || body.iter().any(|s| stmt_reads_var(s, name))
        }
        Stmt::Match { subject, arms } => {
            expr_reads_var(subject, name)
                || arms.iter().any(|a| {
                    a.guard.as_ref().is_some_and(|e| expr_reads_var(e, name))
                        || a.body.iter().any(|s| stmt_reads_var(s, name))
                })
        }
        Stmt::Delete(exprs) => exprs.iter().any(|e| expr_reads_var(e, name)),
        Stmt::Assert { test, msg } => {
            expr_reads_var(test, name) || msg.as_ref().is_some_and(|e| expr_reads_var(e, name))
        }
        Stmt::Raise { expr, cause, .. } => {
            expr.as_ref().is_some_and(|e| expr_reads_var(e, name))
                || cause.as_ref().is_some_and(|e| expr_reads_var(e, name))
        }
        // A nested `def` or `class` may close over `i`.  Conservatively
        // consider that a read of `i` — bail on the rewrite.
        Stmt::Def { body, .. } | Stmt::Class { body, .. } => {
            body.iter().any(|s| stmt_reads_var(s, name))
        }
        Stmt::AnnAssign {
            annotation, value, ..
        } => {
            expr_reads_var(annotation, name)
                || value.as_ref().is_some_and(|v| expr_reads_var(v, name))
        }
        Stmt::TypeAlias { value, .. } => expr_reads_var(value, name),
        Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Pass
        | Stmt::Import { .. }
        | Stmt::ImportFrom { .. } => false,
    }
}

fn expr_reads_var(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Var(n, _) => n == name,
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => false,
        Expr::FString(parts) => any_fstring_expr(parts, &mut |e| expr_reads_var(e, name)),
        Expr::Unary { expr, .. } => expr_reads_var(expr, name),
        Expr::Binary { left, right, .. } => {
            expr_reads_var(left, name) || expr_reads_var(right, name)
        }
        Expr::Compare { left, ops } => {
            expr_reads_var(left, name) || ops.iter().any(|(_, e)| expr_reads_var(e, name))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_reads_var(cond, name) || expr_reads_var(then, name) || expr_reads_var(else_, name)
        }
        Expr::Call { func, args, .. } => {
            expr_reads_var(func, name) || args.iter().any(|a| expr_reads_var(&a.value, name))
        }
        Expr::Attr { target, .. } => expr_reads_var(target, name),
        Expr::Index { target, index, .. } => {
            expr_reads_var(target, name) || expr_reads_var(index, name)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_reads_var(target, name)
                || lower.as_deref().is_some_and(|e| expr_reads_var(e, name))
                || upper.as_deref().is_some_and(|e| expr_reads_var(e, name))
                || step.as_deref().is_some_and(|e| expr_reads_var(e, name))
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().any(|e| expr_reads_var(e, name))
        }
        Expr::Starred(inner) => expr_reads_var(inner, name),
        Expr::Dict(items) => items.iter().any(|item| match item {
            DictItem::Pair(k, v) => expr_reads_var(k, name) || expr_reads_var(v, name),
            DictItem::DoubleSplat(e) => expr_reads_var(e, name),
        }),
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            expr_reads_var(elt, name)
                || clauses.iter().any(|c| {
                    expr_reads_var(&c.iter, name)
                        || c.cond.as_ref().is_some_and(|x| expr_reads_var(x, name))
                })
        }
        Expr::DictComp { key, val, clauses } => {
            expr_reads_var(key, name)
                || expr_reads_var(val, name)
                || clauses.iter().any(|c| {
                    expr_reads_var(&c.iter, name)
                        || c.cond.as_ref().is_some_and(|x| expr_reads_var(x, name))
                })
        }
        Expr::Lambda { params, body } => {
            params
                .iter()
                .filter_map(|p| p.default.as_ref())
                .any(|d| expr_reads_var(d, name))
                || expr_reads_var(body, name)
        }
        Expr::Named { value, .. } => expr_reads_var(value, name),
        Expr::Yield(Some(e)) => expr_reads_var(e, name),
        Expr::Yield(None) => false,
        Expr::YieldFrom(e) => expr_reads_var(e, name),
        Expr::Await(e) => expr_reads_var(e, name),
    }
}

/// Replace `Subscript(Var(c), Var(i))` with `Var(i)` everywhere in `stmt`.
/// Called after `body_index_pattern_is_safe` has verified that every
/// occurrence of `i` in the body is of this exact shape.
fn rewrite_c_at_i_in_stmt(stmt: &mut Stmt, c_name: &str, i_name: &str) {
    match stmt {
        Stmt::Assign(_, e) | Stmt::AugAssign { expr: e, .. } | Stmt::Expr(e) => {
            rewrite_c_at_i_in_expr(e, c_name, i_name);
        }
        Stmt::AnnAssign {
            value: Some(value), ..
        } => rewrite_c_at_i_in_expr(value, c_name, i_name),
        Stmt::AnnAssign { value: None, .. } => {}
        Stmt::Return(Some(e)) => rewrite_c_at_i_in_expr(e, c_name, i_name),
        Stmt::AttrAssign { target, expr, .. } => {
            rewrite_c_at_i_in_expr(target, c_name, i_name);
            rewrite_c_at_i_in_expr(expr, c_name, i_name);
        }
        Stmt::IndexAssign {
            target,
            index,
            expr,
        } => {
            rewrite_c_at_i_in_expr(target, c_name, i_name);
            rewrite_c_at_i_in_expr(index, c_name, i_name);
            rewrite_c_at_i_in_expr(expr, c_name, i_name);
        }
        Stmt::SliceAssign {
            target,
            lower,
            upper,
            step,
            expr,
        } => {
            rewrite_c_at_i_in_expr(target, c_name, i_name);
            if let Some(e) = lower.as_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
            if let Some(e) = upper.as_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
            if let Some(e) = step.as_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
            rewrite_c_at_i_in_expr(expr, c_name, i_name);
        }
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            for (cond, b) in branches.iter_mut() {
                rewrite_c_at_i_in_expr(cond, c_name, i_name);
                for s in b.iter_mut() {
                    rewrite_c_at_i_in_stmt(s, c_name, i_name);
                }
            }
            if let Some(b) = else_branch.as_mut() {
                for s in b.iter_mut() {
                    rewrite_c_at_i_in_stmt(s, c_name, i_name);
                }
            }
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            rewrite_c_at_i_in_expr(cond, c_name, i_name);
            for s in body.iter_mut() {
                rewrite_c_at_i_in_stmt(s, c_name, i_name);
            }
            if let Some(b) = else_branch.as_mut() {
                for s in b.iter_mut() {
                    rewrite_c_at_i_in_stmt(s, c_name, i_name);
                }
            }
        }
        Stmt::For {
            iter,
            body,
            else_branch,
            ..
        } => {
            rewrite_c_at_i_in_expr(iter, c_name, i_name);
            for s in body.iter_mut() {
                rewrite_c_at_i_in_stmt(s, c_name, i_name);
            }
            if let Some(b) = else_branch.as_mut() {
                for s in b.iter_mut() {
                    rewrite_c_at_i_in_stmt(s, c_name, i_name);
                }
            }
        }
        Stmt::Delete(exprs) => {
            for e in exprs.iter_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
        }
        Stmt::Assert { test, msg } => {
            rewrite_c_at_i_in_expr(test, c_name, i_name);
            if let Some(m) = msg.as_mut() {
                rewrite_c_at_i_in_expr(m, c_name, i_name);
            }
        }
        Stmt::Raise { expr, cause, .. } => {
            if let Some(e) = expr.as_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
            if let Some(e) = cause.as_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
        }
        // Other variants are excluded by stmt_safe_for_index_rewrite or have
        // no inner expressions to walk (Pass, Break, Continue, Global,
        // Nonlocal, Return(None)).
        _ => {}
    }
}

fn rewrite_c_at_i_in_expr(expr: &mut Expr, c_name: &str, i_name: &str) {
    // `c[i]` → `i` (the rewritten loop variable now holds the element value).
    if let Expr::Index { target, index, .. } = expr
        && is_c_at_i_expr(target, index, c_name, i_name)
    {
        *expr = Expr::Var(i_name.to_string(), None);
        return;
    }
    match expr {
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis
        | Expr::Var(_, _) => {}
        Expr::FString(parts) => {
            for_each_fstring_expr_mut(parts, &mut |e| rewrite_c_at_i_in_expr(e, c_name, i_name));
        }
        Expr::Unary { expr, .. } => rewrite_c_at_i_in_expr(expr, c_name, i_name),
        Expr::Binary { left, right, .. } => {
            rewrite_c_at_i_in_expr(left, c_name, i_name);
            rewrite_c_at_i_in_expr(right, c_name, i_name);
        }
        Expr::Compare { left, ops } => {
            rewrite_c_at_i_in_expr(left, c_name, i_name);
            for (_, e) in ops.iter_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            rewrite_c_at_i_in_expr(cond, c_name, i_name);
            rewrite_c_at_i_in_expr(then, c_name, i_name);
            rewrite_c_at_i_in_expr(else_, c_name, i_name);
        }
        Expr::Call { func, args, .. } => {
            rewrite_c_at_i_in_expr(func, c_name, i_name);
            for a in args.iter_mut() {
                rewrite_c_at_i_in_expr(&mut a.value, c_name, i_name);
            }
        }
        Expr::Attr { target, .. } => rewrite_c_at_i_in_expr(target, c_name, i_name),
        Expr::Index { target, index, .. } => {
            rewrite_c_at_i_in_expr(target, c_name, i_name);
            rewrite_c_at_i_in_expr(index, c_name, i_name);
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            rewrite_c_at_i_in_expr(target, c_name, i_name);
            if let Some(e) = lower.as_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
            if let Some(e) = upper.as_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
            if let Some(e) = step.as_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items.iter_mut() {
                rewrite_c_at_i_in_expr(e, c_name, i_name);
            }
        }
        Expr::Starred(inner) => rewrite_c_at_i_in_expr(inner, c_name, i_name),
        Expr::Dict(items) => {
            for item in items.iter_mut() {
                match item {
                    DictItem::Pair(k, v) => {
                        rewrite_c_at_i_in_expr(k, c_name, i_name);
                        rewrite_c_at_i_in_expr(v, c_name, i_name);
                    }
                    DictItem::DoubleSplat(e) => rewrite_c_at_i_in_expr(e, c_name, i_name),
                }
            }
        }
        // Comprehensions/lambdas/walrus/yield are bailed on in expr_safe so
        // we should never see them here, but be defensive.
        Expr::ListComp { .. }
        | Expr::DictComp { .. }
        | Expr::SetComp { .. }
        | Expr::GenExp { .. }
        | Expr::Lambda { .. }
        | Expr::Named { .. }
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Await(_) => {}
    }
}

/// Detect `while VAR cmp STOP: ...; VAR += STEP` (or -= for decreasing).
fn detect_while_range<'a>(
    cond: &'a Expr,
    body: &'a [Stmt],
) -> Option<(&'a str, &'a Expr, i64, bool)> {
    let (var_name, cmp_op, stop_expr) = match cond {
        Expr::Binary {
            op, left, right, ..
        } if matches!(
            op,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
        ) =>
        {
            match left.as_ref() {
                Expr::Var(name, _) => (name.as_str(), op, right.as_ref()),
                _ => return None,
            }
        }
        _ => return None,
    };
    match body.last()? {
        Stmt::AugAssign {
            target: AssignTarget::Name(aug_var),
            op: BinaryOp::Add,
            expr: Expr::Int(s),
        } if aug_var == var_name && *s > 0 && matches!(cmp_op, BinaryOp::Lt | BinaryOp::Le) => {
            Some((var_name, stop_expr, *s, matches!(cmp_op, BinaryOp::Le)))
        }
        Stmt::AugAssign {
            target: AssignTarget::Name(aug_var),
            op: BinaryOp::Sub,
            expr: Expr::Int(s),
        } if aug_var == var_name && *s > 0 && matches!(cmp_op, BinaryOp::Gt | BinaryOp::Ge) => {
            Some((var_name, stop_expr, -*s, matches!(cmp_op, BinaryOp::Ge)))
        }
        _ => None,
    }
}

// ─── Compiler struct ──────────────────────────────────────────────────────────

struct LoopCtx {
    /// Instruction indices of `Jump(0)` placeholders for `break` statements;
    /// patched to jump past the loop once the loop end is known.
    /// `SmallVec<[usize; 2]>` avoids heap allocation for the common case of
    /// zero or one `break` per loop.
    break_patches: SmallVec<[usize; 2]>,
    /// None when the continue target is not yet known (e.g. counter-range loop
    /// where the increment comes after the body).  Patched before the increment.
    continue_target: Option<usize>,
    /// Indices of Jump(0) instructions emitted for `continue` when continue_target
    /// was None; fixed up once continue_target is established.
    /// `SmallVec<[usize; 2]>` avoids heap allocation for the common case of
    /// zero or one `continue` before the target is known.
    continue_patches: SmallVec<[usize; 2]>,
    /// Depth of `Compiler::except_cleanups` at the point this loop was entered.
    /// `break` and `continue` must emit cleanups for entries above this depth.
    cleanup_depth: usize,
}

/// Describes the cleanup that must be emitted before an early exit
/// (`break`, `continue`, or `return`) that crosses a guarded block boundary.
// The shared `Body` postfix (`TryBody`/`ExceptBody`/`WithBody`) names the kind
// of guarded body each entry tracks; keeping it is clearer than the lint's
// suggested rename.
#[allow(clippy::enum_variant_names)]
#[derive(Clone)]
enum EarlyExitCleanup {
    /// Inside a try-body that has an active `SetupExcept` on the handler stack.
    /// Early exit must emit `PopExcept` then optionally inline the finally block.
    TryBody { finally_stmts: Option<Vec<Stmt>> },
    /// Inside an except-handler body where `active_exception` is set.
    /// Early exit must emit the PEP 3110 as-var delete (if any), then
    /// `EndExcept`, then optionally inline the finally block.
    ExceptBody {
        finally_stmts: Option<Vec<Stmt>>,
        /// PEP 3110: how to delete the `except E as var` binding on early exit.
        /// `Local(reg)` \u2192 `DeleteLocal(reg, u16::MAX)` (var lives in a register).
        /// `Name(name_idx)` \u2192 `DeleteName(name_idx)` (var lives in env).
        /// `None` \u2192 no `as VAR` clause.
        as_var_delete: Option<ExceptAsVarDel>,
    },
    /// Inside a `with`/`async with` body whose `SetupExcept` is live on the
    /// handler stack.  A `break`/`continue`/`return` that leaves the body must
    /// emit `PopExcept` then call `__exit__(None, None, None)` (sync) or
    /// `await __aexit__(None, None, None)` (async) before jumping (issue #2295).
    /// The exception path is handled separately by the body's `SetupExcept`,
    /// so — like `TryBody` — a `raise` stops the early-exit walk here.
    WithBody {
        /// Register holding the context-manager object (lives for the body).
        ctx_reg: Reg,
        /// `true` for `async with` (drives `await __aexit__`), `false` for `with`.
        is_async: bool,
    },
}

/// Describes how to emit the PEP 3110 except-as variable deletion on early exit.
#[derive(Clone)]
enum ExceptAsVarDel {
    /// Variable lives in a fastlocal register; emit `DeleteLocal(reg, u16::MAX)`.
    Local(Reg),
    /// Variable lives in env (no local slot); emit `DeleteName(name_idx)`.
    Name(u16),
}

struct Compiler {
    local_index: Rc<HashMap<String, Reg>>,
    cell_vars: HashSet<String>,
    /// Names declared `nonlocal` in this function body (issue #2339).  A
    /// `nonlocal x` read/write resolves to an enclosing function scope's cell,
    /// so — like `cell_vars` — it can use the dedicated `LoadCell`/`StoreCell`
    /// opcodes that skip the global inline-cache / module-dict path.  Empty for
    /// module and class scopes (`nonlocal` is invalid there).
    nonlocal_names: HashSet<String>,
    insns: Vec<Insn>,
    /// Per-instruction 1-based source line numbers, parallel to `insns`.
    /// Filled by `emit()` from `current_lineno`.  0 = unknown.
    lineno_table: Vec<u32>,
    /// Per-instruction PEP 657 caret anchor, parallel to `insns` (issues #2426 /
    /// #2411).  Filled by `emit()` from `current_col_span`.  Each entry is
    /// `(full_start, prim_start, prim_end, full_end)` (see [`crate::ast::CaretSpan`]);
    /// `(0, 0, 0, 0)` = no anchor.
    col_table: Vec<crate::ast::CaretSpan>,
    /// 1-based line number of the statement currently being compiled.
    /// Set by `set_lineno()` before each `compile_stmt` call when line
    /// information is available.  0 when no line info is known.
    current_lineno: u32,
    /// PEP 657 caret anchor stamped onto the next emitted instruction(s)
    /// (issues #2426 / #2411).  Set transiently by `compile_expr` around the
    /// instruction that loads a plumbed sub-expression (bare-name `Var`, call,
    /// binary op, subscript), then cleared back to `(0, 0, 0, 0)`.
    /// `(0, 0, 0, 0)` means "no anchor".
    current_col_span: crate::ast::CaretSpan,
    /// 1-based source line of the `def`/`lambda` this compiler is the body of
    /// — emitted into `FnCode::first_lineno` (the function's `co_firstlineno`).
    /// 0 for the module-level compiler (issue #2185).
    first_lineno: u32,
    /// Source-file path the code being compiled comes from — emitted into
    /// `FnCode::filename` (the code object's `co_filename`).  Threaded into every
    /// nested function/class body so an imported module's functions report their
    /// own file in tracebacks and `__code__.co_filename` (issue #2438), rather
    /// than the running script's path.  `<unknown>` until a compile entry point
    /// sets it.
    filename: std::sync::Arc<str>,
    consts: Vec<Value>,
    const_index: HashMap<crate::value::PyKey, u16>,
    names: Vec<String>,
    name_map: HashMap<String, u16>,
    next_temp: Reg,
    base_temp: Reg,
    iter_depth: u8,
    max_iter: u8,
    max_reg: Reg,
    loops: Vec<LoopCtx>,
    /// Stack of cleanup actions needed by early exits (`break`/`continue`/`return`)
    /// that cross a `try`/`except` boundary.  Entries are pushed when entering a
    /// guarded block and popped when leaving it normally.
    /// `SmallVec<[_; 4]>` avoids heap allocation for the common case of at most
    /// four nested try/except levels.
    except_cleanups: SmallVec<[EarlyExitCleanup; 4]>,
    failed: bool,
    error_msg: Option<String>,
    def_set: u64,
    fn_protos: Vec<FnProto>,
    /// Names of *memo-pure* functions defined in this scope — callees whose
    /// result may be cached/reused (drives `CallMemo` emission).  See issue
    /// #2523 for the memo-vs-DCE purity distinction.
    pure_locals: HashSet<String>,
    /// Names of *DCE-pure* functions defined in this scope — callees a
    /// dead-result call to which may be eliminated entirely (no observable
    /// effect, no raise).  A strict subset of `pure_locals`.  Used as the
    /// transitive-callee allow-list when computing a nested function's own
    /// `is_dce_pure` (a function calling a merely-memo-pure callee can still
    /// raise, so it is *not* DCE-pure — issue #2523).
    dce_pure_locals: HashSet<String>,
    /// True when this Compiler is producing the body of a `class` block.
    /// In that mode, every store into a top-level class-body local is
    /// instrumented with `Insn::RecordClassStore(slot)` so the VM can
    /// recover **runtime** insertion order for `vars(C)` / `C.__dict__`.
    /// CPython guarantees class-namespace order follows the order names
    /// are first bound at runtime — not source-walk / slot-allocation order.
    is_class_body: bool,
    /// True when this Compiler is producing a function that was defined directly
    /// inside a class body.  Set by `compile_def` using `self.is_class_body` of
    /// the enclosing compiler.  Only direct class methods get this flag — nested
    /// functions inside methods do not, which mirrors CPython's `__class__` cell
    /// propagation rule: only the directly-defining function gets the cell.
    is_class_method: bool,
    /// The qualname prefix for classes/functions defined in this scope.
    /// Empty for the top-level scope.  When entering a class `Foo`, the child
    /// compiler's prefix becomes `"Foo"` (or `"Outer.Foo"` if nested).  When
    /// entering a function `fn_name`, the child compiler's prefix becomes
    /// `"fn_name.<locals>"` so that classes inside functions get the CPython
    /// `"fn_name.<locals>.ClassName"` form.
    qualname_prefix: String,
    /// Chain of `local_index` maps for every enclosing **function** scope
    /// (not module scope, not class scope — class scope is transparent to
    /// `nonlocal`).  Innermost enclosing function is at the end of the Vec.
    /// Used at compile time to validate `nonlocal` declarations in nested
    /// function bodies.
    /// `SmallVec<[_; 4]>` avoids heap allocation for typical nesting depths (≤ 4).
    outer_locals: SmallVec<[Rc<HashMap<String, Reg>>; 4]>,
    /// True when this Compiler is producing the body of a function `def`
    /// (or a comprehension, which implicitly creates a function scope).
    /// False for module-level compilation and class-body compilation.
    /// Used to determine whether `self.local_index` counts as an enclosing
    /// function scope for `nonlocal` validation in child compilers.
    is_function_scope: bool,
    /// True when this Compiler is producing the body of an `async def` function.
    /// Used to distinguish `'await' outside async function` (inside a non-async
    /// `def`) from `'await' outside function` (at module or class scope).
    is_async_function: bool,
    /// True when this function is an *async generator* (`async def` whose body
    /// contains a bare `yield`, #2280).  Computed from the body AST when the
    /// sub-compiler is set up.  CPython rejects `return <value>` in an async
    /// generator with `SyntaxError: 'return' with value in async generator`.
    is_async_generator_fn: bool,
    /// True when a compile-time `SyntaxError` has been detected (e.g. a
    /// `nonlocal` declaration with no enclosing binding).  Controls whether
    /// `finish()` emits `PyError::Named("SyntaxError", …)` or `PyError::Runtime`.
    is_syntax_error: bool,
    /// True when this Compiler is producing the top-level module/script body.
    /// In that mode, every local-register store emits a `SyncModuleGlobal`
    /// immediately after the `Move`, so that `module_globals_dict` stays live
    /// after `globals()` has been called.  Child compilers for functions and
    /// class bodies set this to false — they write to fastlocals only.
    is_module_scope: bool,
    /// True once we have compiled a statement that is neither a module docstring
    /// nor a `from __future__ import ...`.  After this point any `from __future__`
    /// import must be rejected with
    /// `SyntaxError: from __future__ imports must occur at the beginning of the file`.
    past_future_zone: bool,
    /// True when `from __future__ import annotations` has been seen (PEP 563).
    /// When set, annotation expressions are NOT evaluated; instead, their source
    /// text is stored as a string literal in `__annotations__`.
    future_annotations: bool,
    /// True when any `yield` / `yield from` expression appears in the function
    /// body inside a compile-time-false branch (i.e. `if False: yield`).
    /// Such expressions are never emitted as `Insn::Yield` / `Insn::YieldFrom`,
    /// so the post-compilation `is_generator` scan misses them.  This flag
    /// ensures the function is still treated as a generator, matching CPython
    /// where the presence of `yield` in the source — even in dead code — makes
    /// the enclosing function a generator function (issue #1758).
    has_dead_yield: bool,
    /// True when this Compiler is producing the implicit function body of a
    /// **set comprehension**.  In that mode the synthesized accumulator add
    /// `.acc.add(elt)` is lowered directly to `Insn::SetAdd(acc, elt)` instead
    /// of a full attribute-lookup + method-call dispatch, mirroring CPython's
    /// dedicated `SET_ADD` opcode (issue #1861).
    is_set_comp: bool,
    /// True when this Compiler is producing the implicit function body of a
    /// **list comprehension**.  In that mode the synthesized accumulator append
    /// `.acc.append(elt)` is lowered directly to `Insn::ListAppend(acc, elt)`
    /// instead of a full attribute-lookup + method-call dispatch, mirroring
    /// CPython's dedicated `LIST_APPEND` opcode (issue #1862).
    is_list_comp: bool,
    /// True when this Compiler is producing the implicit function body of a
    /// list / set / dict comprehension — the forms CPython 3.12 *inlines* into
    /// the enclosing frame (PEP 709).  pyrust still runs them as a separate
    /// frame, but for error parity an unbound read of an enclosing local must
    /// surface as `UnboundLocalError` (as if local to the enclosing frame), not
    /// the free-variable `NameError` a real closure / generator expression gets
    /// (issue #2340).
    is_inlined_comp: bool,
    /// For an inlined comprehension, the local-variable names of the
    /// immediately-enclosing real function (the PEP 709 inlining target).  Used
    /// at runtime to decide whether an unbound read is a local of that frame
    /// (`UnboundLocalError`) or a free variable owned by a grandparent scope
    /// (`NameError`) — see issue #2457.  `None` outside an inlined comp.
    comp_enclosing_locals: Option<Rc<HashSet<String>>>,
}

fn class_body_has_annotations(body: &[Stmt]) -> bool {
    body.iter().any(|s| match s {
        Stmt::AnnAssign { .. } => true,
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches.iter().any(|(_, b)| class_body_has_annotations(b))
                || else_branch
                    .as_deref()
                    .is_some_and(class_body_has_annotations)
        }
        Stmt::While { body, .. } | Stmt::For { body, .. } => class_body_has_annotations(body),
        _ => false,
    })
}

/// Return `true` if `stmts` contain a `yield` or `yield from` expression
/// anywhere in the immediate function scope, without crossing into nested
/// `Def` or `Class` bodies (those have their own generator status).
///
/// Used to detect that a function is a generator even when the `yield`
/// appears only in compile-time-dead branches (e.g. `if False: yield`),
/// which are skipped during bytecode emission and therefore produce no
/// `Insn::Yield` for the post-compilation `is_generator` scan to find.
fn stmts_contain_yield(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_contains_yield)
}

fn stmt_contains_yield(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(e) => expr_contains_yield(e),
        Stmt::Return(Some(e)) => expr_contains_yield(e),
        Stmt::Return(None) | Stmt::Pass | Stmt::Break | Stmt::Continue => false,
        Stmt::Global(_) | Stmt::Nonlocal(_) | Stmt::Import { .. } | Stmt::ImportFrom { .. } => {
            false
        }
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches
                .iter()
                .any(|(cond, body)| expr_contains_yield(cond) || stmts_contain_yield(body))
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            expr_contains_yield(cond)
                || stmts_contain_yield(body)
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::For {
            iter,
            body,
            else_branch,
            ..
        } => {
            expr_contains_yield(iter)
                || stmts_contain_yield(body)
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            stmts_contain_yield(body)
                || handlers.iter().any(|h| {
                    h.kind.as_ref().is_some_and(expr_contains_yield) || stmts_contain_yield(&h.body)
                })
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
                || finally_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::With { items, body, .. } => {
            items.iter().any(|(e, _)| expr_contains_yield(e)) || stmts_contain_yield(body)
        }
        Stmt::Assign(_, value) => expr_contains_yield(value),
        Stmt::AugAssign { expr, .. } => expr_contains_yield(expr),
        Stmt::AnnAssign { value, .. } => value.as_ref().is_some_and(expr_contains_yield),
        Stmt::AttrAssign { target, expr, .. } => {
            expr_contains_yield(target) || expr_contains_yield(expr)
        }
        Stmt::IndexAssign {
            target,
            index,
            expr,
        } => expr_contains_yield(target) || expr_contains_yield(index) || expr_contains_yield(expr),
        Stmt::SliceAssign {
            target,
            lower,
            upper,
            step,
            expr,
        } => {
            expr_contains_yield(target)
                || lower.as_deref().is_some_and(expr_contains_yield)
                || upper.as_deref().is_some_and(expr_contains_yield)
                || step.as_deref().is_some_and(expr_contains_yield)
                || expr_contains_yield(expr)
        }
        Stmt::Raise { expr, cause, .. } => {
            expr.as_ref().is_some_and(expr_contains_yield)
                || cause.as_ref().is_some_and(expr_contains_yield)
        }
        Stmt::Delete(exprs) => exprs.iter().any(expr_contains_yield),
        Stmt::Assert { test, msg } => {
            expr_contains_yield(test) || msg.as_ref().is_some_and(expr_contains_yield)
        }
        Stmt::Match { subject, arms } => {
            expr_contains_yield(subject)
                || arms.iter().any(|arm| {
                    arm.guard.as_ref().is_some_and(expr_contains_yield)
                        || stmts_contain_yield(&arm.body)
                })
        }
        // Def and Class bodies are separate scopes — their yields do not make
        // the enclosing function a generator.
        Stmt::Def { .. } | Stmt::Class { .. } => false,
        Stmt::TypeAlias { value, .. } => expr_contains_yield(value),
    }
}

fn expr_contains_yield(expr: &Expr) -> bool {
    match expr {
        Expr::Yield(_) | Expr::YieldFrom(_) => true,
        Expr::Binary { left, right, .. } => expr_contains_yield(left) || expr_contains_yield(right),
        Expr::Unary { expr: e, .. } => expr_contains_yield(e),
        Expr::Compare { left, ops } => {
            expr_contains_yield(left) || ops.iter().any(|(_, e)| expr_contains_yield(e))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_contains_yield(cond) || expr_contains_yield(then) || expr_contains_yield(else_)
        }
        Expr::Call { func, args, .. } => {
            expr_contains_yield(func) || args.iter().any(|a| expr_contains_yield(&a.value))
        }
        Expr::Attr { target, .. } => expr_contains_yield(target),
        Expr::Index { target, index, .. } => {
            expr_contains_yield(target) || expr_contains_yield(index)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_contains_yield(target)
                || lower.as_deref().is_some_and(expr_contains_yield)
                || upper.as_deref().is_some_and(expr_contains_yield)
                || step.as_deref().is_some_and(expr_contains_yield)
        }
        Expr::Tuple(items) | Expr::List(items) | Expr::Set(items) => {
            items.iter().any(expr_contains_yield)
        }
        Expr::Dict(items) => items.iter().any(|item| match item {
            crate::ast::DictItem::Pair(k, v) => expr_contains_yield(k) || expr_contains_yield(v),
            crate::ast::DictItem::DoubleSplat(e) => expr_contains_yield(e),
        }),
        Expr::Starred(e) => expr_contains_yield(e),
        Expr::Named { value, .. } => expr_contains_yield(value),
        Expr::Await(e) => expr_contains_yield(e),
        // An f-string's `{expr}` interpolations (and any `{expr}` inside a
        // nested format spec) are real sub-expressions in the same scope, so a
        // `yield` there counts: `(f"{(yield x)}" for x in xs)` is rejected as
        // `'yield' inside generator expression`. Mirrors the
        // `expr_contains_await` f-string handling (#2308 / #2313).
        Expr::FString(parts) => fstring_parts_contain_yield(parts),
        // Lambda, comprehensions, and generator expressions are separate scopes.
        Expr::Lambda { .. }
        | Expr::ListComp { .. }
        | Expr::SetComp { .. }
        | Expr::DictComp { .. }
        | Expr::GenExp { .. } => false,
        // Leaf nodes — cannot contain yield.
        Expr::Var(_, _)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => false,
    }
}

/// Whether any `{expr}` interpolation in an f-string (or in a nested format
/// spec) contains a `yield` in the current scope. Helper for
/// `expr_contains_yield`. Mirrors `fstring_parts_contain_await`.
fn fstring_parts_contain_yield(parts: &[crate::ast::FStringPart]) -> bool {
    parts.iter().any(|part| match part {
        crate::ast::FStringPart::Literal(_) => false,
        crate::ast::FStringPart::Expr {
            expr, format_spec, ..
        } => {
            expr_contains_yield(expr)
                || format_spec
                    .as_deref()
                    .is_some_and(fstring_parts_contain_yield)
        }
    })
}

/// Whether an expression contains an `await` in the current scope. Used to
/// decide whether a synthesized comprehension function is a coroutine / async
/// generator (#2304): an `await` in a comprehension's element or condition (or
/// a non-outermost clause iterable) makes the comprehension asynchronous even
/// without an `async for` clause.
///
/// Mirrors `expr_contains_yield`: it does NOT descend into lambdas or nested
/// comprehensions / generator expressions, which have their own scope.
fn expr_contains_await(expr: &Expr) -> bool {
    match expr {
        Expr::Await(_) => true,
        Expr::Yield(e) => e.as_deref().is_some_and(expr_contains_await),
        Expr::YieldFrom(e) => expr_contains_await(e),
        Expr::Binary { left, right, .. } => expr_contains_await(left) || expr_contains_await(right),
        Expr::Unary { expr: e, .. } => expr_contains_await(e),
        Expr::Compare { left, ops } => {
            expr_contains_await(left) || ops.iter().any(|(_, e)| expr_contains_await(e))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_contains_await(cond) || expr_contains_await(then) || expr_contains_await(else_)
        }
        Expr::Call { func, args, .. } => {
            expr_contains_await(func) || args.iter().any(|a| expr_contains_await(&a.value))
        }
        Expr::Attr { target, .. } => expr_contains_await(target),
        Expr::Index { target, index, .. } => {
            expr_contains_await(target) || expr_contains_await(index)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_contains_await(target)
                || lower.as_deref().is_some_and(expr_contains_await)
                || upper.as_deref().is_some_and(expr_contains_await)
                || step.as_deref().is_some_and(expr_contains_await)
        }
        Expr::Tuple(items) | Expr::List(items) | Expr::Set(items) => {
            items.iter().any(expr_contains_await)
        }
        Expr::Dict(items) => items.iter().any(|item| match item {
            crate::ast::DictItem::Pair(k, v) => expr_contains_await(k) || expr_contains_await(v),
            crate::ast::DictItem::DoubleSplat(e) => expr_contains_await(e),
        }),
        Expr::Starred(e) => expr_contains_await(e),
        Expr::Named { value, .. } => expr_contains_await(value),
        // An f-string's `{expr}` interpolations (and any `{expr}` inside a
        // nested format spec) are real sub-expressions in the same scope, so an
        // `await` there counts: `[f"{await f(x)}" for x in xs]` is async.
        Expr::FString(parts) => fstring_parts_contain_await(parts),
        // Lambda, comprehensions, and generator expressions are separate scopes.
        Expr::Lambda { .. }
        | Expr::ListComp { .. }
        | Expr::SetComp { .. }
        | Expr::DictComp { .. }
        | Expr::GenExp { .. } => false,
        // Leaf nodes — cannot contain await.
        Expr::Var(_, _)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => false,
    }
}

/// Whether any `{expr}` interpolation in an f-string (or in a nested format
/// spec) contains an `await` in the current scope. Helper for
/// `expr_contains_await`.
fn fstring_parts_contain_await(parts: &[crate::ast::FStringPart]) -> bool {
    parts.iter().any(|part| match part {
        crate::ast::FStringPart::Literal(_) => false,
        crate::ast::FStringPart::Expr {
            expr, format_spec, ..
        } => {
            expr_contains_await(expr)
                || format_spec
                    .as_deref()
                    .is_some_and(fstring_parts_contain_await)
        }
    })
}

/// Whether a list/set/dict comprehension with the given value expressions
/// (`elts`) and `clauses` is itself *asynchronous* — i.e. CPython would compile
/// it as an async comprehension (issue #2312).  This is true when:
///   - any clause is an `async for`, or
///   - the element / condition / non-outermost iterable contains an `await`, or
///   - the element / condition / non-outermost iterable contains a further
///     nested async COLLECTION comprehension (recursive).
///
/// The outermost iterable (`clauses[0].iter`) is excluded: it is evaluated in
/// the enclosing scope, so an async comp there propagates to the *enclosing*
/// comprehension/function, not this one.
fn collection_comp_is_async(elts: &[&Expr], clauses: &[CompClause]) -> bool {
    if clauses.iter().any(|c| c.is_async) {
        return true;
    }
    elts.iter()
        .any(|e| expr_contains_await(e) || expr_has_async_collection_comp(e))
        || clauses.iter().any(|c| {
            c.cond.as_ref().is_some_and(|cond| {
                expr_contains_await(cond) || expr_has_async_collection_comp(cond)
            })
        })
        || clauses[1..]
            .iter()
            .any(|c| expr_contains_await(&c.iter) || expr_has_async_collection_comp(&c.iter))
}

/// Whether an expression subtree contains a directly-nested *async collection
/// comprehension* (list/set/dict comp) that propagates async-ness outward
/// (issue #2312).
///
/// CPython makes the enclosing comprehension/function async when a nested
/// list/set/dict comprehension in its element/cond/non-outermost iterable is
/// itself async (the enclosing body must `await` the inner comp's coroutine).
/// A nested generator expression does NOT propagate — creating the async-gen
/// object needs no `await` — but we still descend into a genexp's parts because
/// an async collection comp can be nested *inside* a genexp's element (which
/// does propagate). The nested comp keeps its own scope: its `await`s are not
/// counted here except insofar as they make *it* async.
fn expr_has_async_collection_comp(expr: &Expr) -> bool {
    match expr {
        // A nested list/set/dict comprehension: it propagates if it is itself
        // async; otherwise descend into its parts (an async comp may be nested
        // deeper, e.g. inside its element wrapped in another expression).
        Expr::ListComp { elt, clauses } | Expr::SetComp { elt, clauses } => {
            collection_comp_is_async(&[elt], clauses) || comp_parts_have_async_comp(&[elt], clauses)
        }
        Expr::DictComp { key, val, clauses } => {
            collection_comp_is_async(&[key, val], clauses)
                || comp_parts_have_async_comp(&[key, val], clauses)
        }
        // A generator expression itself never propagates async-ness, but an
        // async collection comp nested inside its parts does.
        Expr::GenExp { elt, clauses } => comp_parts_have_async_comp(&[elt], clauses),
        Expr::Binary { left, right, .. } => {
            expr_has_async_collection_comp(left) || expr_has_async_collection_comp(right)
        }
        Expr::Unary { expr: e, .. } => expr_has_async_collection_comp(e),
        Expr::Compare { left, ops } => {
            expr_has_async_collection_comp(left)
                || ops.iter().any(|(_, e)| expr_has_async_collection_comp(e))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_has_async_collection_comp(cond)
                || expr_has_async_collection_comp(then)
                || expr_has_async_collection_comp(else_)
        }
        Expr::Call { func, args, .. } => {
            expr_has_async_collection_comp(func)
                || args
                    .iter()
                    .any(|a| expr_has_async_collection_comp(&a.value))
        }
        Expr::Attr { target, .. } => expr_has_async_collection_comp(target),
        Expr::Index { target, index, .. } => {
            expr_has_async_collection_comp(target) || expr_has_async_collection_comp(index)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_has_async_collection_comp(target)
                || lower.as_deref().is_some_and(expr_has_async_collection_comp)
                || upper.as_deref().is_some_and(expr_has_async_collection_comp)
                || step.as_deref().is_some_and(expr_has_async_collection_comp)
        }
        Expr::Tuple(items) | Expr::List(items) | Expr::Set(items) => {
            items.iter().any(expr_has_async_collection_comp)
        }
        Expr::Dict(items) => items.iter().any(|item| match item {
            crate::ast::DictItem::Pair(k, v) => {
                expr_has_async_collection_comp(k) || expr_has_async_collection_comp(v)
            }
            crate::ast::DictItem::DoubleSplat(e) => expr_has_async_collection_comp(e),
        }),
        Expr::Starred(e) => expr_has_async_collection_comp(e),
        Expr::Named { value, .. } => expr_has_async_collection_comp(value),
        Expr::Await(e) => expr_has_async_collection_comp(e),
        Expr::Yield(e) => e.as_deref().is_some_and(expr_has_async_collection_comp),
        Expr::YieldFrom(e) => expr_has_async_collection_comp(e),
        Expr::FString(parts) => parts.iter().any(|part| match part {
            crate::ast::FStringPart::Literal(_) => false,
            crate::ast::FStringPart::Expr {
                expr, format_spec, ..
            } => {
                expr_has_async_collection_comp(expr)
                    || format_spec.as_deref().is_some_and(|fs| {
                        fs.iter().any(|p| match p {
                            crate::ast::FStringPart::Literal(_) => false,
                            crate::ast::FStringPart::Expr { expr, .. } => {
                                expr_has_async_collection_comp(expr)
                            }
                        })
                    })
            }
        }),
        // Lambda bodies are a separate scope and cannot make an enclosing
        // comprehension async; leaf nodes carry nothing.
        Expr::Lambda { .. }
        | Expr::Var(_, _)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => false,
    }
}

/// Scan a comprehension's element(s), conditions, and ALL iterables (including
/// the outermost, since here the comp is itself nested inside an enclosing
/// expression and its outermost iterable is evaluated in the enclosing comp's
/// scope) for a further nested async collection comprehension. Helper for
/// `expr_has_async_collection_comp`.
fn comp_parts_have_async_comp(elts: &[&Expr], clauses: &[CompClause]) -> bool {
    elts.iter().any(|e| expr_has_async_collection_comp(e))
        || clauses.iter().any(|c| {
            expr_has_async_collection_comp(&c.iter)
                || c.cond.as_ref().is_some_and(expr_has_async_collection_comp)
        })
}

/// Produce Python's `repr()` of a string value, matching CPython's output.
///
/// Rules (same as CPython's `repr()` for `str`):
/// - Prefer single-quote delimiters.
/// - If the string contains a single quote but no double quote, use double-quote
///   delimiters instead (avoids the need to escape `'`).
/// - If both quote types appear, use single-quote delimiters and escape `'` as `\'`.
/// - Escape backslashes, non-printable control characters, and surrogates.
fn py_repr_str(s: &str) -> String {
    let has_single = s.contains('\'');
    let has_double = s.contains('"');
    let quote = if has_single && !has_double { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\x0b' => out.push_str("\\v"),
            '\'' if quote == '\'' => out.push_str("\\'"),
            '"' if quote == '"' => out.push_str("\\\""),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Convert an annotation expression to its source-text string representation,
/// as required by PEP 563 (`from __future__ import annotations`).
///
/// CPython stores the unparsed source text of the annotation as a string in
/// `__annotations__`.  We reconstruct a canonical form from the AST that
/// matches CPython output for the annotation expressions commonly found in
/// real code.  String-literal annotations are preserved with their quotes
/// (e.g. `x: 'Foo'` → `"'Foo'"`), consistent with CPython 3.12 behaviour.
fn stringify_annotation(expr: &Expr) -> String {
    match expr {
        Expr::Var(name, _) => name.clone(),
        Expr::None => "None".to_string(),
        Expr::Ellipsis => "...".to_string(),
        Expr::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Expr::Int(n) => n.to_string(),
        Expr::Float(f) => format!("{f}"),
        Expr::Str(s) => py_repr_str(s),
        Expr::Attr { target, name, .. } => {
            format!("{}.{}", stringify_annotation(target), name)
        }
        Expr::Index { target, index, .. } => {
            // In subscript position, a tuple is rendered without outer parens:
            // `dict[str, int]` not `dict[(str, int)]`.
            let index_str = match index.as_ref() {
                Expr::Tuple(elts) => {
                    let parts: Vec<String> = elts.iter().map(stringify_annotation).collect();
                    parts.join(", ")
                }
                other => stringify_annotation(other),
            };
            format!("{}[{}]", stringify_annotation(target), index_str)
        }
        Expr::List(elts) => {
            let parts: Vec<String> = elts.iter().map(stringify_annotation).collect();
            format!("[{}]", parts.join(", "))
        }
        Expr::Tuple(elts) => {
            let parts: Vec<String> = elts.iter().map(stringify_annotation).collect();
            format!("({})", parts.join(", "))
        }
        Expr::Binary {
            left, op, right, ..
        } => {
            let op_str = match op {
                BinaryOp::BitOr => " | ",
                BinaryOp::Add => " + ",
                BinaryOp::Sub => " - ",
                BinaryOp::Mul => " * ",
                _ => " | ",
            };
            format!(
                "{}{}{}",
                stringify_annotation(left),
                op_str,
                stringify_annotation(right)
            )
        }
        Expr::Unary { op, expr, .. } => {
            let op_str = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Pos => "+",
                UnaryOp::Not => "not ",
                UnaryOp::BitNot => "~",
            };
            format!("{}{}", op_str, stringify_annotation(expr))
        }
        // For anything else (calls, comprehensions, etc.) fall back to a
        // best-effort representation — these are rarely used as annotations.
        _ => format!("{expr:?}"),
    }
}

/// Collect all names that a pattern binds on a successful match.
/// Used by `Pattern::Or` validation to enforce that every alternative binds
/// the same set of names (PEP 634 / CPython 3.12 `SyntaxError`).
fn pattern_bound_names(pat: &Pattern) -> HashSet<String> {
    match pat {
        Pattern::Capture(name) => {
            let mut s = HashSet::new();
            s.insert(name.clone());
            s
        }
        Pattern::As { pattern, name } => {
            let mut s = pattern_bound_names(pattern);
            s.insert(name.clone());
            s
        }
        Pattern::Sequence(elements) => {
            let mut s = HashSet::new();
            for (elem_pat, _is_star) in elements {
                s.extend(pattern_bound_names(elem_pat));
            }
            s
        }
        Pattern::Mapping(pairs, rest_name) => {
            let mut s = HashSet::new();
            for (_key, val_pat) in pairs {
                s.extend(pattern_bound_names(val_pat));
            }
            if let Some(rest) = rest_name {
                s.insert(rest.clone());
            }
            s
        }
        Pattern::Class {
            positional, kwargs, ..
        } => {
            let mut s = HashSet::new();
            for pat in positional {
                s.extend(pattern_bound_names(pat));
            }
            for (_attr, pat) in kwargs {
                s.extend(pattern_bound_names(pat));
            }
            s
        }
        Pattern::Or(alternatives) => {
            // All alternatives must bind the same names; return the first's set.
            alternatives
                .first()
                .map(pattern_bound_names)
                .unwrap_or_default()
        }
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Value(_) => HashSet::new(),
    }
}

/// Walk the leading edge of a pattern — descending into the first alternative
/// of nested `Pattern::Or` nodes — and return the name of the first bare
/// `Pattern::Capture` found, or `None` if the leading edge is not a capture.
///
/// Used by the `Pattern::Or` unreachable-check to detect cases like
/// `case (x | 1) | z:` where the inner OR's first alternative `x` is a
/// capture that makes subsequent outer alternatives unreachable.
fn or_leading_capture(pat: &Pattern) -> Option<&str> {
    match pat {
        Pattern::Capture(name) if name != "_" => Some(name),
        Pattern::Or(alts) => alts.first().and_then(or_leading_capture),
        _ => None,
    }
}

/// Same as `or_leading_capture` but for wildcards.
fn or_leading_is_wildcard(pat: &Pattern) -> bool {
    match pat {
        Pattern::Wildcard => true,
        Pattern::Or(alts) => alts.first().is_some_and(or_leading_is_wildcard),
        _ => false,
    }
}

/// Recognise the `f(<pos…>, **d)` shape eligible for the `CallEx` fast lowering
/// (issue #2393): exactly one `**d` double-splat, which must be the final arg,
/// preceded only by plain positional args (no `*a` splat, no literal `name=`
/// keyword).  Returns the number of leading positionals on a match, else `None`.
fn double_splat_fast_shape(args: &[crate::ast::CallArg]) -> Option<usize> {
    let n = args.len();
    if n == 0 {
        return None;
    }
    let last = &args[n - 1];
    if !last.double_splat {
        return None;
    }
    // Every preceding arg must be a plain positional.
    for a in &args[..n - 1] {
        if a.splat || a.double_splat || a.name.is_some() {
            return None;
        }
    }
    Some(n - 1)
}

impl Compiler {
    fn new(
        local_index: Rc<HashMap<String, Reg>>,
        def_bound_mask: u64,
        cell_vars: Vec<CellVar>,
    ) -> Self {
        let n = local_index.len();
        let cell_set: HashSet<String> = cell_vars.into_iter().collect();
        // base_temp must cover ALL local_index slots (including cell vars) so
        // that temp registers never overlap with local-variable slot numbers.
        let base_temp = Reg::try_from(n).unwrap_or(Reg::MAX);

        Self {
            local_index,
            cell_vars: cell_set,
            nonlocal_names: HashSet::new(),
            insns: Vec::new(),
            lineno_table: Vec::new(),
            col_table: Vec::new(),
            // (0, 0, 0, 0) sentinel = no anchor (#2411).
            current_lineno: 0,
            current_col_span: (0, 0, 0, 0),
            first_lineno: 0,
            filename: std::sync::Arc::from("<unknown>"),
            consts: Vec::new(),
            const_index: HashMap::new(),
            names: Vec::new(),
            name_map: HashMap::new(),
            next_temp: base_temp,
            base_temp,
            iter_depth: 0,
            max_iter: 0,
            max_reg: if n > 0 {
                Reg::try_from(n).unwrap_or(Reg::MAX).saturating_sub(1)
            } else {
                0
            },
            loops: Vec::new(),
            except_cleanups: SmallVec::new(),
            failed: n > Reg::MAX as usize,
            error_msg: if n > Reg::MAX as usize {
                Some(format!("too many local variables (max {})", Reg::MAX))
            } else {
                None
            },
            def_set: def_bound_mask,
            fn_protos: Vec::new(),
            pure_locals: HashSet::new(),
            dce_pure_locals: HashSet::new(),
            is_class_body: false,
            is_class_method: false,
            qualname_prefix: String::new(),
            outer_locals: SmallVec::new(),
            is_function_scope: false,
            is_async_function: false,
            is_async_generator_fn: false,
            is_syntax_error: false,
            is_module_scope: false,
            past_future_zone: false,
            future_annotations: false,
            has_dead_yield: false,
            is_set_comp: false,
            is_list_comp: false,
            is_inlined_comp: false,
            comp_enclosing_locals: None,
        }
    }

    /// If this compiler is producing a class body and `reg` is one of the
    /// class-body's local slots, emit a `RecordClassStore(reg)` insn so the
    /// VM can append the slot to the class-namespace store-order list.
    /// No-op outside class bodies and for temp / cell registers — keeping
    /// regular function compilation untouched.
    fn maybe_record_class_store(&mut self, reg: Reg) {
        if self.is_class_body && reg < self.base_temp {
            self.emit(Insn::RecordClassStore(reg));
        }
    }

    /// Companion to `maybe_record_class_store`: emit a `RecordClassDel`
    /// after a `DeleteLocal` so the slot is removed from the class-namespace
    /// store-order list while preserving the order of remaining entries.
    fn maybe_record_class_del(&mut self, reg: Reg) {
        if self.is_class_body && reg < self.base_temp {
            self.emit(Insn::RecordClassDel(reg));
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn mark_def(&mut self, reg: Reg) {
        if (reg as usize) < 64 {
            self.def_set |= 1u64 << reg;
        }
    }

    fn mark_target_def(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    self.mark_def(reg);
                }
            }
            AssignTarget::Tuple(targets) => {
                for t in targets {
                    self.mark_target_def(t);
                }
            }
            AssignTarget::Starred(inner) => {
                self.mark_target_def(inner);
            }
            _ => {}
        }
    }

    fn intern_name(&mut self, name: &str) -> u16 {
        if let Some(&idx) = self.name_map.get(name) {
            return idx;
        }
        if self.names.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!("too many distinct names (max {})", u16::MAX));
            }
            return 0;
        }
        let idx = self.names.len() as u16;
        self.names.push(name.to_string());
        self.name_map.insert(name.to_string(), idx);
        idx
    }

    /// Emit `R[dst] = R[lhs] op n` using `BinOpImm` when `n` fits in `i16`,
    /// or `BinOpConst` with a pool entry otherwise.
    fn emit_int_binop(&mut self, dst: Reg, lhs: Reg, op: BinaryOp, n: i64) {
        if n >= i16::MIN as i64 && n <= i16::MAX as i64 {
            self.emit(Insn::BinOpImm(dst, lhs, op, n as i16, false));
        } else {
            let idx = self.intern_const(Value::int(n));
            self.emit(Insn::BinOpConst(dst, lhs, op, idx, false));
        }
    }

    fn intern_const(&mut self, val: Value) -> u16 {
        // PyKey treats `Bool(b)` and `Int(b as i64)` as hash/eq-equal (matching
        // CPython's `True == 1`), so they would collide in the constant pool's
        // hash index even though they are type-distinct values.  Likewise,
        // `Float(1.0)` and `Int(1)` are now hash/eq-equal in PyKey so that
        // dict/set keys respect CPython's numeric equality invariant.  Complex
        // values with zero imaginary part map to `PyKey::Float` via `to_key()`,
        // which would collide with integer-valued floats and ints.  In all these
        // cases the constant pool must keep the values distinct, so we skip the
        // hash-map fast path and fall through to the type-exact linear scan.
        let is_bool = matches!(val.kind(), ValueKind::Bool(_));
        let is_float = matches!(val.kind(), ValueKind::Float(_));
        let is_complex = matches!(val.kind(), ValueKind::Complex(_, _));
        if !is_bool
            && !is_float
            && !is_complex
            && let Some(key) = val.to_key()
        {
            if let Some(&idx) = self.const_index.get(&key) {
                return idx;
            }
            if self.consts.len() >= u16::MAX as usize {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(format!("too many constants (max {})", u16::MAX));
                }
                return 0;
            }
            let idx = self.consts.len() as u16;
            self.const_index.insert(key, idx);
            self.consts.push(val);
            return idx;
        }
        // Non-hashable constants, booleans, and floats: type-exact linear scan.
        for (i, v) in self.consts.iter().enumerate() {
            if const_eq(v, &val) {
                return i as u16;
            }
        }
        if self.consts.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!("too many constants (max {})", u16::MAX));
            }
            return 0;
        }
        let idx = self.consts.len() as u16;
        self.consts.push(val);
        idx
    }

    fn alloc_temp(&mut self) -> Reg {
        let r = self.next_temp;
        if r == Reg::MAX {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!("too many temporaries (max {})", Reg::MAX));
            }
            return 0;
        }
        self.next_temp += 1;
        if r > self.max_reg {
            self.max_reg = r;
        }
        r
    }

    fn free_temp(&mut self, r: Reg) {
        if r >= self.base_temp && self.next_temp > 0 && r + 1 == self.next_temp {
            self.next_temp -= 1;
        }
    }

    fn alloc_iter(&mut self) -> u8 {
        let s = self.iter_depth;
        self.iter_depth += 1;
        if self.iter_depth > self.max_iter {
            self.max_iter = self.iter_depth;
        }
        s
    }

    fn free_iter(&mut self) {
        if self.iter_depth > 0 {
            self.iter_depth -= 1;
        }
    }

    fn emit(&mut self, insn: Insn) -> usize {
        let idx = self.insns.len();
        self.insns.push(insn);
        self.lineno_table.push(self.current_lineno);
        // The armed PEP 657 anchor applies to exactly this instruction (#2426);
        // consume and clear it so it never leaks onto the next emit.
        self.col_table.push(self.current_col_span);
        self.current_col_span = (0, 0, 0, 0);
        idx
    }

    /// Arm a PEP 657 caret anchor (issues #2426 / #2411) for the **next**
    /// emitted instruction.  `None` (a span-less form) clears the anchor — the
    /// formatter then omits the caret row.  Consumed and reset by `emit`.
    fn set_col_span_for_next(&mut self, span: Option<crate::ast::CaretSpan>) {
        self.current_col_span = span.unwrap_or((0, 0, 0, 0));
    }

    /// Set the source line number for all subsequently emitted instructions.
    /// Call this at the start of each statement (when line info is available).
    fn set_lineno(&mut self, lineno: u32) {
        self.current_lineno = lineno;
    }

    fn set_syntax_error(&mut self, msg: &str) {
        self.failed = true;
        self.is_syntax_error = true;
        if self.error_msg.is_none() {
            self.error_msg = Some(msg.to_string());
        }
    }

    fn pc(&self) -> usize {
        self.insns.len()
    }

    fn patch_jump(&mut self, idx: usize) {
        let target = self.insns.len() as i32;
        let after_jump = idx as i32 + 1;
        let offset = target - after_jump;
        match &mut self.insns[idx] {
            Insn::Jump(off)
            | Insn::JumpIfFalse(_, off)
            | Insn::JumpIfTrue(_, off)
            | Insn::ForIter(_, _, off)
            | Insn::SetupExcept(off)
            | Insn::MatchExcept(_, off)
            | Insn::MatchExceptStar(_, _, _, off)
            | Insn::CmpJumpIfFalse(_, _, _, off)
            | Insn::CmpJumpIfTrue(_, _, _, off)
            | Insn::CmpJumpIfFalseConst(_, _, _, off)
            | Insn::CmpJumpIfTrueConst(_, _, _, off)
            | Insn::ForCountReg(_, _, _, _, off)
            | Insn::ForCountConst(_, _, _, _, off)
            | Insn::ForCountConstInline(_, _, _, _, off) => *off = offset,
            _ => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(
                        "internal compiler error: patch_jump on non-jump instruction".to_string(),
                    );
                }
            }
        }
    }

    /// Try to fuse the last emitted instruction with a conditional jump.
    ///
    /// Emit `JumpIfFalse` or `JumpIfTrue` for `cond_reg` (offset=0, patched later).
    /// `invert=false` → JumpIfFalse, `invert=true` → JumpIfTrue.
    /// BinOp/BinOpConst + conditional-jump fusion is handled by the optimizer.
    fn emit_cond_jump(&mut self, cond_reg: Reg, invert: bool) -> usize {
        if invert {
            self.emit(Insn::JumpIfTrue(cond_reg, 0))
        } else {
            self.emit(Insn::JumpIfFalse(cond_reg, 0))
        }
    }

    /// True if `name` is a cell variable (lives in env, not registers).
    fn is_cell(&self, name: &str) -> bool {
        self.cell_vars.contains(name)
    }

    /// True when a non-register name is guaranteed to resolve to a
    /// **function-scope cell** — either a cell var owned by this scope or a
    /// `nonlocal x` declared here, which binds to an enclosing function's cell
    /// (issue #2339).  Such a name never resolves to a module global or builtin,
    /// so its read/write can use the dedicated `LoadCell`/`StoreCell` opcodes
    /// that skip the `LoadGlobal` inline cache and the module-globals-dict
    /// fallback.  Restricted to function scope: module scope has no cells worth
    /// special-casing, and class bodies keep the name-keyed namespace path
    /// (which `vars()`/`dir()` expose) and the #384 resolution rules untouched.
    fn is_function_cell(&self, name: &str) -> bool {
        self.is_function_scope && (self.is_cell(name) || self.nonlocal_names.contains(name))
    }

    /// Register index for a local variable, or None if the name is global/nonlocal/cell.
    fn local_reg(&self, name: &str) -> Option<Reg> {
        if self.is_cell(name) {
            return None;
        }
        self.local_index.get(name).copied()
    }

    fn compile_block(&mut self, stmts: &[Stmt]) {
        self.compile_block_with_linenos(stmts, &[]);
    }

    /// Like `compile_block` but with per-statement line numbers.  When
    /// `linenos` is shorter than `stmts` (or empty), the missing entries
    /// default to 0 (= keep current lineno).
    fn compile_block_with_linenos(&mut self, stmts: &[Stmt], linenos: &[u32]) {
        for (idx, stmt) in stmts.iter().enumerate() {
            if self.failed {
                return;
            }
            // Update the current line number when info is available.
            if let Some(&ln) = linenos.get(idx)
                && ln != 0
            {
                self.set_lineno(ln);
            }
            // #289: rewrite `while i < len(c): ...; i += 1` → `for i in c: ...`
            // when `i` is unused after the loop.  Needs the post-loop suffix
            // for the unused-after-loop check, so it lives in compile_block
            // (not compile_while which only sees its own body/else).
            if matches!(stmt, Stmt::While { .. })
                && let Some(rewritten) = try_rewrite_while_index_to_for(stmts, idx)
            {
                self.compile_stmt(&rewritten);
                // A rewritten while-to-for is not a __future__ directive.
                if self.is_module_scope {
                    self.past_future_zone = true;
                }
                continue;
            }
            self.compile_stmt(stmt);
            // Track whether we have moved past the zone where `from __future__`
            // imports are valid (module-level, before any non-__future__ statement
            // other than the module docstring which is peeled off by compile_script).
            if self.is_module_scope
                && !matches!(stmt, Stmt::ImportFrom { module, .. } if module == "__future__")
            {
                self.past_future_zone = true;
            }
        }
    }

    /// Emit cleanup instructions for a `raise` statement that exits an `except`
    /// handler body.  Unlike `emit_early_exit_cleanups` (which is for
    /// `break`/`continue`/`return`), `raise` does NOT emit `EndExcept` because
    /// the raise instruction itself manages `handled_exc_stack`:
    ///
    /// - `RaiseReRaise` explicitly pops `handled_exc_stack` before propagating.
    /// - `RaiseValue`/`RaiseFrom` don't pop, but `handle_vm_error` checks for
    ///   a duplicate top-of-stack entry and removes it automatically.
    ///
    /// So for `raise` we only need to: delete any `as VAR` binding (PEP 3110),
    /// then inline any `finally` stmts.  `TryBody` entries don't need compile-time
    /// cleanup — the VM's `exc_handlers` stack still covers them.
    /// `pending_exc_reg`: when a non-bare `raise X` is in progress, the
    /// register holding the to-be-raised exception value.  If `Some`, it is
    /// pushed onto `handled_exc_stack` before inlining the innermost
    /// ExceptBody's finally block, so that any raise inside the finally sees
    /// the correct implicit context (the to-be-raised exception, not the
    /// currently-handled one).  The push is undone by a `PopExcContext` after
    /// the finally block completes normally.
    fn emit_raise_cleanups(&mut self, pending_exc_reg: Option<Reg>) {
        let total = self.except_cleanups.len();
        // Track whether we have already processed the innermost ExceptBody.
        // The PushExcContext is only needed for that first one.
        let mut innermost_except_body_done = false;
        for i in (0..total).rev() {
            if self.failed {
                return;
            }
            let cleanup = self.except_cleanups[i].clone();
            match cleanup {
                EarlyExitCleanup::TryBody { .. } | EarlyExitCleanup::WithBody { .. } => {
                    // A TryBody/WithBody entry means this `raise` site is inside a
                    // try/with body whose SetupExcept is still live on the VM's
                    // exc_handlers stack.  The VM will dispatch the exception to
                    // that handler at runtime (for `with`, the exception-path
                    // `__exit__` call); no compile-time inlining is needed for
                    // this entry or any outer entries (also covered by their own
                    // SetupExcept).
                    return;
                }
                EarlyExitCleanup::ExceptBody {
                    finally_stmts,
                    as_var_delete,
                } => {
                    // PEP 3110: delete the `as VAR` binding (matches the normal
                    // handler exit path at line ~7427).  Also clear def_set so
                    // that any reference to the variable inside the inlined
                    // finally block correctly emits CheckLocal → UnboundLocalError,
                    // matching CPython's behaviour (the `as` binding is gone before
                    // the finally clause runs).
                    match as_var_delete {
                        Some(ExceptAsVarDel::Local(reg)) => {
                            self.emit(Insn::DeleteLocal(reg, u16::MAX));
                            if (reg as usize) < 64 {
                                self.def_set &= !(1u64 << reg);
                            }
                        }
                        Some(ExceptAsVarDel::Name(name_idx)) => {
                            self.emit(Insn::DeleteName(name_idx));
                        }
                        None => {}
                    }
                    // Inline the finally block (if any) without EndExcept.
                    // The raise instruction propagates the exception; any further
                    // enclosing ExceptBody entries (outer except handlers whose
                    // SetupExcept was also popped) are processed by the remaining
                    // loop iterations.  TryBody entries at outer scopes still have
                    // live SetupExcept handlers and are handled by the VM.
                    if let Some(stmts) = finally_stmts {
                        // For the innermost ExceptBody: if we have a pending
                        // exception (non-bare raise), temporarily install it as
                        // the active context so that any raise inside the finally
                        // sees it as __context__ rather than the currently-handled
                        // exception on the stack.  This matches CPython, where the
                        // finally runs with the new exception as the active one.
                        let push_exc_ctx = !innermost_except_body_done && pending_exc_reg.is_some();
                        if let (true, Some(r)) = (push_exc_ctx, pending_exc_reg) {
                            self.emit(Insn::PushExcContext(r));
                        }
                        let saved_tail: Vec<EarlyExitCleanup> =
                            self.except_cleanups.drain(i..).collect();
                        self.compile_block(&stmts);
                        self.except_cleanups.extend(saved_tail);
                        if push_exc_ctx {
                            self.emit(Insn::PopExcContext);
                        }
                    }
                    innermost_except_body_done = true;
                    // Continue the loop: there may be enclosing ExceptBody entries
                    // (from outer except handlers) whose finallys also need inlining,
                    // because their outer SetupExcept was likewise popped when the
                    // outer handler was entered.
                }
            }
        }
    }

    /// Emit cleanup instructions for all `EarlyExitCleanup` entries in
    /// `self.except_cleanups[from_depth..]`, iterating innermost-first (i.e.
    /// from the top of the stack downward).
    ///
    /// Called before `break`, `continue`, or `return` to unwind any active
    /// `try`/`except` guards that the early exit crosses.
    ///
    /// While the inlined finally/handler-cleanup block for frame `i` is being
    /// compiled, `except_cleanups` is temporarily truncated to `[..i]` so that
    /// an early exit (e.g. `return`) inside that inlined block does not re-walk
    /// the frame we are currently unwinding — which would cause infinite
    /// recursion (see issue #365: `try: return X finally: return Y`).
    fn emit_early_exit_cleanups(&mut self, from_depth: usize) {
        let total = self.except_cleanups.len();
        if total <= from_depth {
            return;
        }
        // Walk from innermost (top) down to `from_depth`.
        for i in (from_depth..total).rev() {
            if self.failed {
                return;
            }
            // Clone the entry so we can mutate `self` while compiling the
            // inlined finally block.
            let cleanup = self.except_cleanups[i].clone();
            // Shadow the cleanup stack: any nested cleanup emission triggered
            // by `compile_block` below must not see frames `[i..]` (we are
            // already in the process of unwinding them).
            let saved_tail: Vec<EarlyExitCleanup> = self.except_cleanups.drain(i..).collect();
            match cleanup {
                EarlyExitCleanup::TryBody { finally_stmts } => {
                    self.emit(Insn::PopExcept);
                    if let Some(stmts) = finally_stmts {
                        self.compile_block(&stmts);
                    }
                }
                EarlyExitCleanup::ExceptBody {
                    finally_stmts,
                    as_var_delete,
                } => {
                    // PEP 3110: delete the `as VAR` binding before EndExcept,
                    // matching the normal (non-early-exit) handler exit path.
                    match as_var_delete {
                        Some(ExceptAsVarDel::Local(reg)) => {
                            self.emit(Insn::DeleteLocal(reg, u16::MAX));
                        }
                        Some(ExceptAsVarDel::Name(name_idx)) => {
                            self.emit(Insn::DeleteName(name_idx));
                        }
                        None => {}
                    }
                    self.emit(Insn::EndExcept);
                    if let Some(stmts) = finally_stmts {
                        self.compile_block(&stmts);
                    }
                }
                EarlyExitCleanup::WithBody { ctx_reg, is_async } => {
                    // `with`/`async with` is `try: BODY finally: __exit__(...)`.
                    // Pop the body's handler, then run the no-exception exit
                    // (`__exit__(None, None, None)` / `await __aexit__(...)`)
                    // before the break/continue/return jump (issue #2295).
                    self.emit(Insn::PopExcept);
                    if is_async {
                        self.emit_async_with_normal_exit(ctx_reg);
                    } else {
                        self.emit_with_normal_exit(ctx_reg);
                    }
                }
            }
            // Restore the cleanup stack so the caller (and any sibling
            // iterations) see the original frames.  Unconditional restore
            // is safe because `compile_block` doesn't return errors — it
            // routes failures through `self.failed`, which the early-return
            // guard above catches on the next loop iteration.
            self.except_cleanups.extend(saved_tail);
        }
    }

    fn finish(self) -> Result<FnCode, PyError> {
        if self.failed {
            let msg = self
                .error_msg
                .unwrap_or_else(|| "compilation failed".to_string());
            if self.is_syntax_error {
                return Err(PyError::named("SyntaxError", msg));
            } else {
                return Err(PyError::Runtime(msg));
            }
        }
        let num_regs = if self.max_reg >= self.base_temp || self.base_temp == 0 {
            self.max_reg.saturating_add(1)
        } else {
            self.base_temp
        };
        // Guard against pathological register counts that would OOM at runtime.
        // Each register slot is one Option<Value>, so 1M slots ~= 8 MB per call frame.
        const MAX_REGS: u32 = 1 << 20;
        if num_regs > MAX_REGS {
            return Err(PyError::Runtime(format!(
                "function uses too many registers ({num_regs}); max is {MAX_REGS}"
            )));
        }
        // A function is a generator if it contains any `Yield` or `YieldFrom`
        // instruction OR if any `yield`/`yield from` appears in a dead branch
        // (compile-time-false `if` arm) that was skipped during emission.
        // CPython determines generator status from the AST — the presence of
        // `yield` anywhere in the source makes the function a generator even
        // if that `yield` is unreachable at runtime (issue #1758).
        //
        // In an `async def` body (#2280) the `is_generator` flag distinguishes
        // an *async generator* (`async def` containing `yield`) from a plain
        // coroutine.  `await` lowers to a `GetAwaitable` + `Insn::YieldFrom`
        // pair, so YieldFrom must NOT count here — otherwise every coroutine
        // that awaits anything would be mis-tagged as an async generator.  A
        // bare `yield` (the only thing that makes an `async def` an async
        // generator; `yield from` inside `async def` is a SyntaxError) emits
        // `Insn::Yield`, so for async functions we scan for `Insn::Yield` only.
        let is_generator = if self.is_async_function {
            self.insns.iter().any(|i| matches!(i, Insn::Yield { .. })) || self.has_dead_yield
        } else {
            self.insns
                .iter()
                .any(|i| matches!(i, Insn::Yield { .. } | Insn::YieldFrom { .. }))
                || self.has_dead_yield
        };
        let insns = self.insns;
        let insns_len = insns.len();
        let names_len = self.names.len();
        Ok(FnCode {
            insns,
            filename: self.filename,
            lineno_table: self.lineno_table,
            col_table: self.col_table,
            first_lineno: self.first_lineno,
            consts: self.consts,
            names: self.names,
            num_regs,
            num_iters: self.max_iter,
            num_locals: self.base_temp,
            fn_protos: self.fn_protos,
            cell_vars: self.cell_vars.into_iter().collect(),
            is_generator,
            is_coroutine: self.is_async_function,
            is_class_method: self.is_class_method,
            is_inlined_comp: self.is_inlined_comp,
            comp_enclosing_locals: self.comp_enclosing_locals.clone(),
            attr_cache: std::cell::RefCell::new(vec![AttrCacheEntry::Empty; insns_len]),
            global_cache: RefCell::new(vec![(GLOBAL_CACHE_EMPTY, Value::none()); names_len]),
            binop_cache: RefCell::new(vec![BinOpCacheEntry::Empty; insns_len]),
            kwcall_cache: RefCell::new(vec![KwCallCacheEntry::Empty; insns_len]),
            fmt_spec_cache: RefCell::new(vec![
                crate::interpreter::FmtSpecCacheEntry::Empty;
                insns_len
            ]),
            // Empty until the optimizer's `build_exc_table` pass runs; while
            // empty the VM uses the dynamic SetupExcept/PopExcept handler stack.
            exc_table: Vec::new(),
            // Conservative: un-optimized bytecode is never trampolined.  The
            // optimizer recomputes this from the real `exc_table` (#2234).
            has_exc_handlers: true,
            // The inliner (`pass_inline`) only runs in the optimizer; freshly
            // compiled bytecode has spliced nothing (#2569).
            inline_frames: None,
        })
    }

    /// Allocate result register: reuse `candidate` if it's a temp, else fresh.
    fn ensure_dst(&mut self, candidate: Reg) -> Reg {
        if candidate >= self.base_temp {
            candidate
        } else {
            self.alloc_temp()
        }
    }

    /// If `src` is a fastlocal register (not a temp), copy its value into a
    /// fresh temp register and return that temp.  Otherwise return `src` as-is.
    /// Used when a value must survive a `DeleteLocal` on the same register.
    fn ensure_temp(&mut self, src: Reg) -> Reg {
        if src >= self.base_temp {
            src
        } else {
            let dst = self.alloc_temp();
            self.emit(Insn::Move(dst, src));
            dst
        }
    }

    // ── Store helpers ─────────────────────────────────────────────────────────

    /// Emit the appropriate store for `name` from register `src`.
    /// If `container_expr` is a global/cell variable name, write `obj_reg` back
    /// to the env.  Called after SetItem/SetSlice on a container that was loaded
    /// via LoadGlobal (which creates a copy, so the mutation must be committed).
    fn writeback_container_if_global(&mut self, container_expr: &Expr, obj_reg: Reg) {
        if let Expr::Var(name, _) = container_expr
            && self.local_reg(name).is_none()
        {
            let name_idx = self.intern_name(name);
            self.emit(Insn::StoreGlobal(name_idx, obj_reg));
        }
    }

    fn compile_store_name(&mut self, name: &str, src: Reg) {
        if let Some(reg) = self.local_reg(name) {
            if src != reg {
                self.emit(Insn::Move(reg, src));
            }
            // Record the runtime store for class-namespace ordering. We emit
            // even when `src == reg` (no Move emitted) because the store
            // still semantically happened — e.g. `for i in range(3):` writes
            // `i` directly via `ForIter(reg, ...)` and then `compile_for`
            // calls back through here for synthetic stores.
            self.maybe_record_class_store(reg);
            // Issue #820: at module scope, keep module_globals_dict live so
            // that globals() always returns an up-to-date view.  SyncModuleGlobal
            // is a NOP when globals_accessed == false (common case), so this
            // adds zero overhead to scripts that never call globals().
            if self.is_module_scope {
                let name_idx = self.intern_name(name);
                self.emit(Insn::SyncModuleGlobal(reg, name_idx));
            }
        } else {
            let idx = self.intern_name(name);
            if self.is_function_cell(name) {
                self.emit(Insn::StoreCell(idx, src));
            } else {
                self.emit(Insn::StoreGlobal(idx, src));
            }
        }
    }

    // ── Statement compilation ─────────────────────────────────────────────────

    fn compile_stmt(&mut self, stmt: &Stmt) {
        if self.failed {
            return;
        }
        match stmt {
            Stmt::Pass => {}
            Stmt::Break => {
                if self.loops.is_empty() {
                    self.set_syntax_error("'break' outside loop");
                    return;
                }
                let last = self.loops.len() - 1;
                let depth = self.loops[last].cleanup_depth;
                self.emit_early_exit_cleanups(depth);
                if self.failed {
                    return;
                }
                let idx = self.emit(Insn::Jump(0));
                let last = self.loops.len() - 1;
                self.loops[last].break_patches.push(idx);
            }
            Stmt::Continue => {
                if self.loops.is_empty() {
                    self.set_syntax_error("'continue' not properly in loop");
                    return;
                }
                let last = self.loops.len() - 1;
                let depth = self.loops[last].cleanup_depth;
                self.emit_early_exit_cleanups(depth);
                if self.failed {
                    return;
                }
                let last = self.loops.len() - 1;
                let idx = self.emit(Insn::Jump(0));
                if let Some(target) = self.loops[last].continue_target {
                    let from = idx as i32 + 1;
                    let offset = target as i32 - from;
                    if let Insn::Jump(off) = &mut self.insns[idx] {
                        *off = offset;
                    }
                } else {
                    self.loops[last].continue_patches.push(idx);
                }
            }
            Stmt::Return(None) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'return' outside function");
                    return;
                }
                self.emit_early_exit_cleanups(0);
                if self.failed {
                    return;
                }
                self.emit(Insn::ReturnNone);
            }
            Stmt::Return(Some(expr)) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'return' outside function");
                    return;
                }
                // `return <value>` (including a literal `return None`) inside an
                // async generator is a SyntaxError (#2280); only a bare `return`
                // is allowed.  Matches CPython 3.12.
                if self.is_async_generator_fn {
                    self.set_syntax_error("'return' with value in async generator");
                    return;
                }
                let r = self.compile_expr(expr);
                // `emit_early_exit_cleanups` may emit `DeleteLocal` for the
                // `except … as e` variable (PEP 3110).  If the return
                // expression compiled directly to that same fastlocal register
                // (e.g. `return e`), the deletion would clobber the value
                // before `Return` reads it.  Copy to a temp first so the
                // return value survives any cleanup deletions.
                let r = self.ensure_temp(r);
                self.emit_early_exit_cleanups(0);
                if self.failed {
                    self.free_temp(r);
                    return;
                }
                self.emit(Insn::Return(r));
                self.free_temp(r);
            }
            Stmt::Expr(expr) => {
                if self.try_emit_set_comp_add(expr) {
                    return;
                }
                if self.try_emit_list_comp_append(expr) {
                    return;
                }
                let r = self.compile_expr(expr);
                self.free_temp(r);
            }
            Stmt::Assign(target, expr) => {
                self.compile_assign(target, expr);
                self.mark_target_def(target);
            }
            Stmt::AnnAssign {
                name,
                annotation,
                value,
            } => {
                self.compile_ann_assign(name, annotation, value.as_ref().map(|v| v as &Expr));
            }
            Stmt::AugAssign { target, op, expr } => {
                self.compile_aug_assign(target, *op, expr);
                if let AssignTarget::Name(name) = target
                    && let Some(reg) = self.local_reg(name)
                {
                    self.mark_def(reg);
                }
            }
            Stmt::AttrAssign {
                target,
                name,
                expr,
                span,
            } => {
                let obj = self.compile_expr(target);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(name);
                // PEP 657 caret anchor (#2442): underline the whole `obj.attr`
                // target span when the SetAttr raises AttributeError.  Arm
                // immediately before the SetAttr; `emit` consumes and clears it.
                self.set_col_span_for_next(*span);
                self.emit(Insn::SetAttr(obj, name_idx, val));
                self.free_temp(val);
                self.free_temp(obj);
            }
            Stmt::IndexAssign {
                target,
                index,
                expr,
            } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, idx, val));
                self.free_temp(val);
                self.free_temp(idx);
                self.writeback_container_if_global(target, obj);
                self.free_temp(obj);
            }
            Stmt::SliceAssign {
                target,
                lower,
                upper,
                step,
                expr,
            } => {
                let obj = self.compile_expr(target);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, slice_r, val));
                self.writeback_container_if_global(target, obj);
                self.free_temp(val);
                self.free_temp(slice_r);
                self.free_temp(obj);
            }
            Stmt::Assert { test, msg } => {
                let cond = self.compile_expr(test);
                let skip = self.emit(Insn::JumpIfTrue(cond, 0));
                self.free_temp(cond);
                if let Some(msg_expr) = msg {
                    let msg_reg = self.compile_expr(msg_expr);
                    self.emit(Insn::RaiseAssert(msg_reg));
                    self.free_temp(msg_reg);
                } else {
                    self.emit(Insn::RaiseAssertNoMsg);
                }
                self.patch_jump(skip);
            }
            Stmt::If {
                branches,
                else_branch,
                branch_linenos,
                else_linenos,
            } => {
                self.compile_if(
                    branches,
                    else_branch.as_deref(),
                    branch_linenos,
                    else_linenos,
                );
            }
            Stmt::While {
                cond,
                body,
                else_branch,
                body_linenos,
                else_linenos,
            } => {
                self.compile_while(
                    cond,
                    body,
                    else_branch.as_deref(),
                    body_linenos,
                    else_linenos,
                );
            }
            Stmt::For {
                target,
                iter,
                body,
                else_branch,
                body_linenos,
                else_linenos,
                is_async,
            } => {
                if *is_async {
                    self.compile_async_for(
                        target,
                        iter,
                        body,
                        else_branch.as_deref(),
                        body_linenos,
                        else_linenos,
                    );
                } else {
                    self.compile_for(
                        target,
                        iter,
                        body,
                        else_branch.as_deref(),
                        body_linenos,
                        else_linenos,
                    );
                }
            }
            Stmt::Global(_) => {
                // Purely a compile-time declaration; no runtime effect.
            }
            Stmt::Nonlocal(_) => {
                // Nonlocal is a compile-time declaration in function bodies.
                // At module level (not inside any function or class), it is a
                // SyntaxError — CPython rejects it at compile time.
                if !self.is_function_scope && !self.is_class_body {
                    self.set_syntax_error("nonlocal declaration not allowed at module level");
                }
            }
            Stmt::Raise { expr, cause, span } => {
                self.compile_raise(expr.as_ref(), cause.as_ref(), *span);
            }
            Stmt::Delete(exprs) => {
                for expr in exprs {
                    self.compile_delete(expr);
                    if self.failed {
                        return;
                    }
                }
            }
            Stmt::Import { names } => {
                self.compile_import(names);
            }
            Stmt::ImportFrom { module, names } => {
                self.compile_import_from(module, names);
            }
            Stmt::Def {
                name,
                params,
                body,
                body_linenos,
                def_lineno,
                decorators,
                return_annotation,
                is_async,
                type_params,
            } => {
                self.compile_def(
                    name,
                    params,
                    body,
                    body_linenos,
                    *def_lineno,
                    decorators,
                    return_annotation.as_ref(),
                    *is_async,
                    type_params,
                );
            }
            Stmt::Class {
                name,
                bases,
                metaclass,
                keywords,
                body,
                decorators,
                type_params,
            } => {
                self.compile_class(
                    name,
                    bases,
                    metaclass.as_ref(),
                    keywords,
                    body,
                    decorators,
                    type_params,
                );
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                body_linenos,
                else_linenos,
                finally_linenos,
            } => {
                self.compile_try(
                    body,
                    handlers,
                    else_branch.as_deref(),
                    finally_branch.as_deref(),
                    body_linenos,
                    else_linenos,
                    finally_linenos,
                );
            }
            Stmt::With {
                items,
                body,
                body_linenos,
                is_async,
            } => {
                if *is_async {
                    self.compile_async_with(items, body, body_linenos);
                } else {
                    self.compile_with(items, body, body_linenos);
                }
            }
            Stmt::Match { subject, arms } => {
                self.compile_match(subject, arms);
            }
            Stmt::TypeAlias {
                name,
                type_params,
                value,
            } => {
                self.compile_type_alias(name, type_params, value);
            }
        }
    }

    // ── Type alias (PEP 695) ──────────────────────────────────────────────────

    fn compile_type_alias(&mut self, name: &str, type_params: &[TypeParam], value: &Expr) {
        // ── Step 1: create TypeVar objects for each type parameter ───────────
        // Each TypeVar is bound (via StoreGlobal) in a dedicated type-parameter
        // env so the RHS expression and any bound/constraint can reference it by
        // name.  Binding into a child env (rather than the enclosing namespace)
        // keeps the parameter names from leaking after the statement, while the
        // env stays alive for the lazy bound/constraint thunks created below —
        // PEP 695 evaluates those on first `__bound__` / `__constraints__`
        // access (#2290), by which time the captured env must still resolve
        // every type parameter (e.g. `type X[T, U: T] = ...`).
        if !type_params.is_empty() {
            self.emit(Insn::PushTypeParamEnv);
        }
        // Phase 1: create every TypeVar (unbounded) and bind its name, so the RHS
        // and any bound/constraint can reference every parameter by name.
        let mut typevar_regs: Vec<Reg> = Vec::with_capacity(type_params.len());
        for param in type_params {
            let tv_reg = self.alloc_temp();
            self.emit_make_typevar(tv_reg, &param.name);
            // Bind the TypeVar to the param name so the RHS expression — and the
            // lazily-evaluated bound/constraint thunks — can load it via
            // LoadGlobal.  StoreGlobal writes into the active (type-param) env's
            // `values`, exactly where a free-variable LoadGlobal looks.
            let param_name_idx = self.intern_name(&param.name);
            self.emit(Insn::StoreGlobal(param_name_idx, tv_reg));
            typevar_regs.push(tv_reg);
        }
        // Phase 2: with every name bound, attach each bound/constraint thunk to
        // the corresponding TypeVar.  Doing this after binding all names lets a
        // self/forward-referential bound (`type X[T, U: T] = ...`) resolve when
        // the thunk runs.
        for (param, &tv_reg) in type_params.iter().zip(typevar_regs.iter()) {
            self.emit_typevar_bound(tv_reg, param);
        }

        // ── Step 2: build the __type_params__ tuple ──────────────────────────
        // BuildTuple(dst, base, n) reads R[base..base+n].  The TypeVar regs are
        // guaranteed to be contiguous from alloc_temp calls above *only* if no
        // other allocation happened in between; compile_store_name may emit
        // SyncModuleGlobal which doesn't allocate regs, so they stay contiguous.
        // However to be safe we copy them to a fresh contiguous block.
        let params_reg = if type_params.is_empty() {
            // Empty tuple: use a literal empty tuple constant.
            let empty_tuple = crate::value::Value::tuple(vec![]);
            let const_idx = self.intern_const(empty_tuple);
            let r = self.alloc_temp();
            self.emit(Insn::LoadConst(r, const_idx));
            r
        } else {
            let base = self.alloc_temp();
            // We already allocated typevar_regs[0] as a temp.  If the first
            // TypeVar reg equals `base` we can reuse the block; otherwise we
            // need to copy.  In practice alloc_temp increments sequentially, so
            // after alloc_temp() for `base` the next regs would conflict.
            // The simplest safe approach: copy all TypeVar values into a fresh
            // contiguous range.
            let n = type_params.len() as Reg;
            // base is the first slot of the contiguous block we'll pass to
            // BuildTuple.  Allocate n-1 more slots after it.
            for _ in 1..n as usize {
                self.alloc_temp();
            }
            // Copy each TypeVar into the contiguous range.
            for (i, &tv_reg) in typevar_regs.iter().enumerate() {
                let slot = base + i as Reg;
                if slot != tv_reg {
                    self.emit(Insn::Move(slot, tv_reg));
                }
            }
            let tuple_dst = self.alloc_temp();
            self.emit(Insn::BuildTuple(tuple_dst, base, n));
            // Free the contiguous block (but not tuple_dst which we return).
            for i in 0..n as usize {
                self.free_temp(base + i as Reg);
            }
            tuple_dst
        };
        // Free the individual TypeVar regs (the tuple holds the values via clone).
        for tv_reg in &typevar_regs {
            self.free_temp(*tv_reg);
        }

        // ── Step 3: evaluate the RHS ─────────────────────────────────────────
        // TypeVar names are bound in the active type-param env, so LoadGlobal
        // for e.g. `T` resolves to the TypeVar object.
        let val_reg = self.compile_expr(value);

        // ── Step 4: leave the type-parameter env ─────────────────────────────
        // Mirrors CPython's hidden annotation scope: type params must NOT be
        // visible in the enclosing scope after the type alias statement.  The
        // popped env stays alive via the Rc captured by each lazy bound thunk
        // (reachable from `__type_params__`), so a later `__bound__` access can
        // still resolve a forward/self reference.
        if !type_params.is_empty() {
            self.emit(Insn::PopTypeParamEnv);
        }

        // ── Step 5: intern the alias name and emit MakeTypeAlias ────────────
        let name_str = crate::value::Value::string(name);
        let name_idx = self.intern_const(name_str);
        let dst = self.alloc_temp();
        self.emit(Insn::MakeTypeAlias(dst, name_idx, val_reg, params_reg));
        self.free_temp(val_reg);
        self.free_temp(params_reg);

        // ── Step 6: store the alias under `name` ─────────────────────────────
        let target = crate::ast::AssignTarget::Name(name.to_string());
        if let Some(reg) = self.local_reg(name) {
            if reg != dst {
                self.emit(Insn::Move(reg, dst));
            }
            self.maybe_record_class_store(reg);
            if self.is_module_scope {
                let name_idx = self.intern_name(name);
                self.emit(Insn::SyncModuleGlobal(reg, name_idx));
            }
        } else {
            let name_idx = self.intern_name(name);
            self.emit(Insn::StoreGlobal(name_idx, dst));
        }
        self.free_temp(dst);
        self.mark_target_def(&target);
    }

    // ── PEP 695 generic type parameters helper ────────────────────────────────

    /// PEP 695: bind each generic type parameter to a fresh `TypeVar` object in
    /// the current scope (via `StoreGlobal`) so that annotations, base-class
    /// expressions, and method/function bodies that reference the parameter name
    /// (e.g. `def f[T](x: T)`) resolve it instead of raising `NameError`.
    ///
    /// Returns the contiguous register block `(base, n)` holding the live
    /// TypeVar objects so the caller can reuse them when building the
    /// `__type_params__` tuple — CPython keeps the *same* TypeVar object in both
    /// `__type_params__` and the annotations (`f.__type_params__[0] is
    /// f.__annotations__['x']`).  The caller must keep `next_temp > base + n`
    /// until it has emitted the `__type_params__` tuple, then is free to reclaim
    /// the slots.
    ///
    /// Returns `(0, 0)` when there are no type parameters (caller must skip the
    /// reuse path in that case).
    ///
    /// Note: the bound names are intentionally *not* deleted afterwards. Unlike
    /// the type-alias path (whose RHS is fully evaluated inline), a generic
    /// function/class body references its type parameters lazily at call time via
    /// `LoadGlobal`, so the binding must outlive the definition statement. This
    /// leaks the parameter name into the enclosing namespace, which CPython hides
    /// behind a dedicated annotation scope; see the deferred note in the PR.
    fn emit_bind_type_params(&mut self, type_params: &[TypeParam]) -> (Reg, Reg) {
        let n = type_params.len() as Reg;
        if n == 0 {
            return (0, 0);
        }
        if self.next_temp.checked_add(n as u32).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many registers for type params".to_string());
            }
            return (0, 0);
        }
        let base = self.next_temp;
        self.next_temp += n as u32;
        if self.next_temp - 1 > self.max_reg {
            self.max_reg = self.next_temp - 1;
        }
        // Phase 1: create every TypeVar (initially unbounded) and bind its name.
        // All type parameters must exist and be in scope before any bound is
        // evaluated, so a self/forward-referential bound (`def f[T: T]`,
        // `def g[T, U: T]`) can resolve every parameter name — PEP 695 evaluates
        // bounds/constraints lazily in a scope where all the type params (and the
        // enclosing name) are visible.
        for (i, param) in type_params.iter().enumerate() {
            let tv_reg = base + i as Reg;
            self.emit_make_typevar(tv_reg, &param.name);
            // Bind via StoreGlobal so the name lands in `env.values`, which is
            // exactly where a body's `LoadGlobal` for a free variable looks.
            let name_idx = self.intern_name(&param.name);
            self.emit(Insn::StoreGlobal(name_idx, tv_reg));
        }
        // Phase 2: with every name now bound, evaluate each bound/constraint and
        // store it onto the already-created TypeVar.
        for (i, param) in type_params.iter().enumerate() {
            let tv_reg = base + i as Reg;
            self.emit_typevar_bound(tv_reg, param);
        }
        (base, n)
    }

    /// Emit a `MakeTypeVar` into `tv_reg` for the type parameter named `name`.
    /// The TypeVar is created unbounded (`__bound__ == None`, `__constraints__
    /// == ()`); any bound/constraint clause is populated later by
    /// `emit_typevar_bound`, once every type parameter is in scope.
    fn emit_make_typevar(&mut self, tv_reg: Reg, name: &str) {
        let name_const = self.intern_const(crate::value::Value::string(name));
        self.emit(Insn::MakeTypeVar(tv_reg, name_const));
    }

    /// Attach a PEP 695 lazy bound/constraint *thunk* to an already-created
    /// TypeVar in `tv_reg`.  CPython evaluates a type parameter's bound or
    /// constraints lazily — not at def/class/alias time, but on first access of
    /// `__bound__` / `__constraints__` — in a deferred annotation scope where
    /// every type parameter (and the enclosing names) is visible.
    ///
    /// We mirror this by compiling the clause expression into a zero-argument
    /// closure (`lambda: <expr>`) that captures the active type-parameter env,
    /// and storing it on the TypeVar's internal `__evaluate_bound__` /
    /// `__evaluate_constraints__` slot.  The thunk is invoked once, on first
    /// read of `__bound__` / `__constraints__`, and its result cached (see
    /// `get_attr_instance_raw`).  Self- and forward-referential bounds
    /// (`T: T`, `U: T`) still resolve because the captured env binds every
    /// parameter name.  A bare parameter (no clause) leaves the eager defaults
    /// (`__bound__ == None`, `__constraints__ == ()`) untouched.
    fn emit_typevar_bound(&mut self, tv_reg: Reg, param: &TypeParam) {
        match &param.bound {
            None => {}
            Some(TypeParamBound::Bound(expr)) => {
                let thunk_reg = self.compile_lambda(&[], expr);
                let attr_idx = self.intern_name("__evaluate_bound__");
                self.emit(Insn::SetTypeVarAttr(tv_reg, attr_idx, thunk_reg));
                self.free_temp(thunk_reg);
            }
            Some(TypeParamBound::Constraints(elems)) => {
                let tuple_expr = Expr::Tuple(elems.to_vec());
                let thunk_reg = self.compile_lambda(&[], &tuple_expr);
                let attr_idx = self.intern_name("__evaluate_constraints__");
                self.emit(Insn::SetTypeVarAttr(tv_reg, attr_idx, thunk_reg));
                self.free_temp(thunk_reg);
            }
        }
    }

    /// Build the `__type_params__` tuple from an already-bound contiguous block
    /// of TypeVar registers (produced by `emit_bind_type_params`) and store it on
    /// `obj_reg`.  Reusing the bound registers preserves TypeVar object identity
    /// between `__type_params__` and the annotations that reference the names.
    fn emit_type_params_attr_from_regs(&mut self, obj_reg: Reg, base: Reg, n: Reg) {
        let saved_next = self.next_temp;
        if self.next_temp <= base + n {
            self.next_temp = base + n;
        }
        let tuple_reg = self.alloc_temp();
        self.emit(Insn::BuildTuple(tuple_reg, base, n));
        let attr_name_idx = self.intern_name("__type_params__");
        self.emit(Insn::SetAttr(obj_reg, attr_name_idx, tuple_reg));
        // The TypeVar block and the tuple slot are dead after SetAttr.
        self.next_temp = saved_next;
    }

    // ── Assignment ────────────────────────────────────────────────────────────

    fn compile_assign(&mut self, target: &AssignTarget, expr: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    self.compile_expr_into(expr, reg);
                    // Class-body `x = expr` is the common case; record the store
                    // for class-namespace insertion order.  (Outside class
                    // bodies this is a no-op — see `maybe_record_class_store`.)
                    self.maybe_record_class_store(reg);
                    // Issue #820: at module scope, keep module_globals_dict live.
                    if self.is_module_scope {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                    }
                } else {
                    // global / nonlocal / cell var → go through env
                    let src = self.compile_expr(expr);
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::StoreGlobal(name_idx, src));
                    self.free_temp(src);
                }
            }
            AssignTarget::Tuple(targets) => {
                // Check if any target is starred.
                let star_pos = targets
                    .iter()
                    .position(|t| matches!(t, AssignTarget::Starred(_)));

                if let Some(star_idx) = star_pos {
                    // Extended unpack: a, *b, c = seq
                    let before = star_idx as u8;
                    let after = (targets.len() - star_idx - 1) as u8;
                    // Total destination registers: before + 1 (starred list) + after
                    let total = targets.len() as u32;
                    let src = self.compile_expr(expr);
                    let base = self.next_temp;
                    if base.checked_add(total).is_none() {
                        self.failed = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(format!("too many unpack targets ({})", total));
                        }
                        return;
                    }
                    self.next_temp = base + total;
                    if self.next_temp - 1 > self.max_reg {
                        self.max_reg = self.next_temp - 1;
                    }
                    self.emit(Insn::UnpackEx {
                        src,
                        before,
                        after,
                        dst_base: base,
                    });
                    self.free_temp(src);
                    // Store results: targets[i] → R[base + i], where targets[star_idx] is the starred list
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        let inner = match t {
                            AssignTarget::Starred(inner) => inner.as_ref(),
                            other => other,
                        };
                        self.compile_store_unpack_target(inner, base + i);
                    }
                    self.next_temp = base;
                    return;
                }

                // No starred target — fast path: matching tuple literal
                if let Expr::Tuple(exprs) = expr
                    && exprs.len() == targets.len()
                    && !targets.is_empty()
                {
                    let mut target_regs: Vec<Option<Reg>> = Vec::with_capacity(targets.len());
                    let mut all_name_locals = true;
                    for t in targets.iter() {
                        match t {
                            AssignTarget::Name(name) => {
                                target_regs.push(self.local_reg(name));
                                if self.local_reg(name).is_none() {
                                    // cell or global — can still do fast path with temps
                                    all_name_locals = false;
                                }
                            }
                            _ => {
                                all_name_locals = false;
                                target_regs.push(None);
                            }
                        }
                    }
                    // If ALL are simple name→local, use the original fast path
                    if all_name_locals && target_regs.iter().all(|r| r.is_some()) {
                        let saved_next = self.next_temp;
                        let mut temps: Vec<Reg> = Vec::with_capacity(exprs.len());
                        for rhs_expr in exprs.iter() {
                            let r = self.compile_expr(rhs_expr);
                            let tmp = if r < self.base_temp {
                                let t = self.alloc_temp();
                                self.emit(Insn::Move(t, r));
                                t
                            } else {
                                r
                            };
                            temps.push(tmp);
                        }
                        if !self.failed {
                            for i in 0..targets.len() {
                                let dst = target_regs[i].unwrap();
                                let src_tmp = temps[i];
                                if src_tmp != dst {
                                    self.emit(Insn::Move(dst, src_tmp));
                                }
                                self.maybe_record_class_store(dst);
                                // Issue #820: sync into module_globals_dict at module scope.
                                if self.is_module_scope {
                                    // all_name_locals guard guarantees AssignTarget::Name here
                                    if let AssignTarget::Name(name) = &targets[i] {
                                        let name_idx = self.intern_name(name);
                                        self.emit(Insn::SyncModuleGlobal(dst, name_idx));
                                    }
                                }
                            }
                        }
                        self.next_temp = saved_next;
                        return;
                    }
                }

                let src = self.compile_expr(expr);
                let n = targets.len() as u32;
                if n == 0 {
                    self.free_temp(src);
                    return;
                }
                let base = self.next_temp;
                if base.checked_add(n).is_none() {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some(format!("too many unpack targets ({})", n));
                    }
                    return;
                }
                self.next_temp = base + n;
                if self.next_temp - 1 > self.max_reg {
                    self.max_reg = self.next_temp - 1;
                }
                self.emit(Insn::Unpack(base, src, n));
                self.free_temp(src);
                for (i, t) in (0u32..).zip(targets.iter()) {
                    match t {
                        AssignTarget::Name(name) => {
                            if let Some(reg) = self.local_reg(name) {
                                self.emit(Insn::Move(reg, base + i));
                                self.maybe_record_class_store(reg);
                                // Issue #820: sync into module_globals_dict at module scope.
                                if self.is_module_scope {
                                    let name_idx = self.intern_name(name);
                                    self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                                }
                            } else {
                                let name_idx = self.intern_name(name);
                                self.emit(Insn::StoreGlobal(name_idx, base + i));
                            }
                        }
                        AssignTarget::Attr(obj_expr, attr, span) => {
                            let obj = self.compile_expr(obj_expr);
                            let name_idx = self.intern_name(attr);
                            // PEP 657 caret anchor (#2442): underline `obj.attr`
                            // if this store raises AttributeError.
                            self.set_col_span_for_next(*span);
                            self.emit(Insn::SetAttr(obj, name_idx, base + i));
                            self.free_temp(obj);
                        }
                        AssignTarget::Index(obj_expr, idx_expr) => {
                            let obj = self.compile_expr(obj_expr);
                            let idx = self.compile_expr(idx_expr);
                            self.emit(Insn::SetItem(obj, idx, base + i));
                            self.free_temp(idx);
                            self.free_temp(obj);
                        }
                        AssignTarget::Slice {
                            target: obj_expr,
                            lower,
                            upper,
                            step,
                        } => {
                            let obj = self.compile_expr(obj_expr);
                            let slice_r = self.compile_slice_key(
                                lower.as_deref(),
                                upper.as_deref(),
                                step.as_deref(),
                            );
                            self.emit(Insn::SetItem(obj, slice_r, base + i));
                            self.free_temp(slice_r);
                            self.free_temp(obj);
                        }
                        AssignTarget::Tuple(_) => {
                            // Nested tuple unpack — compile recursively from the temp register
                            self.compile_store_unpack_target(t, base + i);
                        }
                        AssignTarget::Starred(_) => {
                            // Should not happen (handled above); treat as error
                            self.failed = true;
                            if self.error_msg.is_none() {
                                self.error_msg = Some(
                                    "unexpected starred target in non-extended unpack".to_string(),
                                );
                            }
                        }
                    }
                }
                self.next_temp = base;
            }
            AssignTarget::Attr(obj_expr, attr, span) => {
                let obj = self.compile_expr(obj_expr);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(attr);
                // PEP 657 caret anchor (#2442): underline `obj.attr` if this
                // store raises AttributeError.
                self.set_col_span_for_next(*span);
                self.emit(Insn::SetAttr(obj, name_idx, val));
                self.free_temp(val);
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, idx, val));
                self.free_temp(val);
                self.free_temp(idx);
                self.free_temp(obj);
            }
            AssignTarget::Slice {
                target: obj_expr,
                lower,
                upper,
                step,
            } => {
                // Plain `l[a:b] = rhs` normally lowers to `Stmt::SliceAssign`;
                // this arm covers an `AssignTarget::Slice` reaching the generic
                // assignment path (e.g. as a single target group), mirroring it.
                let obj = self.compile_expr(obj_expr);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, slice_r, val));
                self.free_temp(val);
                self.free_temp(slice_r);
                self.free_temp(obj);
            }
            AssignTarget::Starred(_) => {
                // Standalone starred target (validated away by parser; should not reach here)
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("starred assignment target must be in a list or tuple".to_string());
                }
            }
        }
    }

    fn compile_ann_assign(&mut self, name: &str, annotation: &Expr, value: Option<&Expr>) {
        // 1. If there's a value, compile it as a regular assignment.
        if let Some(val_expr) = value {
            self.compile_assign(&AssignTarget::Name(name.to_string()), val_expr);
            self.mark_target_def(&AssignTarget::Name(name.to_string()));
        }
        // 2. Function scope: annotations are NOT stored in __annotations__ at runtime.
        if self.is_function_scope {
            return;
        }
        // 3. Produce the annotation value: either evaluate the expression (eager,
        //    default) or store its source text as a string (PEP 563 lazy mode).
        let ann_reg = if self.future_annotations {
            self.compile_literal(Value::string(stringify_annotation(annotation)))
        } else {
            let r = self.compile_expr(annotation);
            if self.failed {
                self.free_temp(r);
                return;
            }
            r
        };
        // 4. Load the string key for this annotation.
        let name_str_val = crate::value::Value::string(name);
        let key_idx = self.intern_const(name_str_val);
        let key_reg = self.alloc_temp();
        self.emit(Insn::LoadConst(key_reg, key_idx));
        // 5. Load (or locate) the __annotations__ dict.
        let ann_dict_name = "__annotations__";
        let (dict_reg, is_temp) = if let Some(reg) = self.local_reg(ann_dict_name) {
            // Class body: __annotations__ is a fastlocal register.
            self.maybe_record_class_store(reg);
            (reg, false)
        } else {
            // Module scope: load via LoadGlobal.
            let ann_dict_idx = self.intern_name(ann_dict_name);
            let r = self.alloc_temp();
            self.emit(Insn::LoadGlobal(r, ann_dict_idx));
            (r, true)
        };
        // 6. __annotations__[name] = annotation_value
        self.emit(Insn::SetItem(dict_reg, key_reg, ann_reg));
        if is_temp {
            self.free_temp(dict_reg);
        }
        self.free_temp(key_reg);
        self.free_temp(ann_reg);
    }

    /// Store the value in `src_reg` into `target` (a non-starred inner target).
    fn compile_store_unpack_target(&mut self, target: &AssignTarget, src_reg: Reg) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    if reg != src_reg {
                        self.emit(Insn::Move(reg, src_reg));
                    }
                    self.maybe_record_class_store(reg);
                    // Issue #820: sync into module_globals_dict at module scope.
                    if self.is_module_scope {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                    }
                } else {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::StoreGlobal(name_idx, src_reg));
                }
            }
            AssignTarget::Attr(obj_expr, attr, span) => {
                let obj = self.compile_expr(obj_expr);
                let name_idx = self.intern_name(attr);
                // PEP 657 caret anchor (#2442): underline `obj.attr` if this
                // store raises AttributeError.
                self.set_col_span_for_next(*span);
                self.emit(Insn::SetAttr(obj, name_idx, src_reg));
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                self.emit(Insn::SetItem(obj, idx, src_reg));
                self.free_temp(idx);
                self.free_temp(obj);
            }
            AssignTarget::Slice {
                target: obj_expr,
                lower,
                upper,
                step,
            } => {
                let obj = self.compile_expr(obj_expr);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                self.emit(Insn::SetItem(obj, slice_r, src_reg));
                self.free_temp(slice_r);
                self.free_temp(obj);
            }
            AssignTarget::Tuple(targets) => {
                // Nested unpack — unpack directly from src_reg into sub-targets.
                let star_pos = targets
                    .iter()
                    .position(|t| matches!(t, AssignTarget::Starred(_)));
                if let Some(star_idx) = star_pos {
                    // Extended unpack: (a, *b, c) = src_reg
                    let before = star_idx as u8;
                    let after = (targets.len() - star_idx - 1) as u8;
                    let total = targets.len() as u32;
                    let base = self.next_temp;
                    if base.checked_add(total).is_none() {
                        self.failed = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(format!("too many unpack targets ({})", total));
                        }
                        return;
                    }
                    self.next_temp = base + total;
                    if self.next_temp - 1 > self.max_reg {
                        self.max_reg = self.next_temp - 1;
                    }
                    self.emit(Insn::UnpackEx {
                        src: src_reg,
                        before,
                        after,
                        dst_base: base,
                    });
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        let inner = match t {
                            AssignTarget::Starred(inner) => inner.as_ref(),
                            other => other,
                        };
                        self.compile_store_unpack_target(inner, base + i);
                    }
                    self.next_temp = base;
                } else {
                    // Simple unpack: (a, b, c) = src_reg
                    let n = targets.len() as u32;
                    if n == 0 {
                        return;
                    }
                    let base = self.next_temp;
                    if base.checked_add(n).is_none() {
                        self.failed = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(format!("too many unpack targets ({})", n));
                        }
                        return;
                    }
                    self.next_temp = base + n;
                    if self.next_temp - 1 > self.max_reg {
                        self.max_reg = self.next_temp - 1;
                    }
                    self.emit(Insn::Unpack(base, src_reg, n));
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        self.compile_store_unpack_target(t, base + i);
                    }
                    self.next_temp = base;
                }
            }
            AssignTarget::Starred(_) => {
                // Bare starred outside a tuple — should not reach here
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("starred assignment target must be in a list or tuple".to_string());
                }
            }
        }
    }

    fn compile_aug_assign(&mut self, target: &AssignTarget, op: BinaryOp, expr: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    let definitely_bound = (reg as usize) < 64 && (self.def_set >> reg) & 1 != 0;
                    if self.is_module_scope && !definitely_bound {
                        // Issue #1411: at module scope a name that is not yet
                        // definitely bound must be read through the global →
                        // builtins chain, not from the unset fastlocal register.
                        // The fastlocal reg read would produce the wrong error
                        // ("local variable referenced before assignment" instead
                        // of "name 'x' is not defined").
                        let name_idx = self.intern_name(name);
                        let lhs = self.alloc_temp();
                        self.emit(Insn::LoadGlobal(lhs, name_idx));
                        self.emit_aug_binop(lhs, op, expr);
                        // Store result into the fastlocal register so subsequent
                        // reads in the same scope use the fast path.
                        self.emit(Insn::Move(reg, lhs));
                        self.mark_def(reg);
                        self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        self.free_temp(lhs);
                    } else {
                        // Issue #1644: at function scope, a local that is not yet
                        // definitely bound must be guarded by CheckLocal so that
                        // an unset register produces UnboundLocalError (not the
                        // generic NameError that vm_read emits).
                        if !self.is_module_scope && !definitely_bound {
                            let name_idx = self.intern_name(name);
                            self.emit(Insn::CheckLocal(reg, name_idx));
                        }
                        self.emit_aug_binop(reg, op, expr);
                        self.maybe_record_class_store(reg);
                        // Issue #820: sync the updated value into module_globals_dict
                        // when globals_accessed == true (same as compile_store_name).
                        if self.is_module_scope {
                            let name_idx = self.intern_name(name);
                            self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        }
                    }
                } else {
                    // cell / global: load, compute, store.  A function-scope
                    // cell / nonlocal uses LoadCell/StoreCell (issue #2339);
                    // this is the hot path for `nonlocal c; c += 1`.
                    let name_idx = self.intern_name(name);
                    let lhs = self.alloc_temp();
                    if self.is_function_cell(name) {
                        self.emit(Insn::LoadCell(lhs, name_idx));
                        self.emit_aug_binop(lhs, op, expr);
                        self.emit(Insn::StoreCell(name_idx, lhs));
                    } else {
                        self.emit(Insn::LoadGlobal(lhs, name_idx));
                        self.emit_aug_binop(lhs, op, expr);
                        self.emit(Insn::StoreGlobal(name_idx, lhs));
                    }
                    self.free_temp(lhs);
                }
            }
            AssignTarget::Attr(obj_expr, attr, span) => {
                let obj = self.compile_expr(obj_expr);
                let name_idx = self.intern_name(attr);
                let lhs = self.alloc_temp();
                // PEP 657 caret anchor (#2442): underline `obj.attr` for both the
                // read (`obj.attr` missing) and the write-back; CPython anchors
                // the augmented-assignment target span on either failure.
                self.set_col_span_for_next(*span);
                self.emit(Insn::GetAttr(lhs, obj, name_idx));
                self.emit_aug_binop(lhs, op, expr);
                self.set_col_span_for_next(*span);
                self.emit(Insn::SetAttr(obj, name_idx, lhs));
                self.free_temp(lhs);
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                let lhs = self.alloc_temp();
                self.emit(Insn::GetItem(lhs, obj, idx));
                self.emit_aug_binop(lhs, op, expr);
                self.emit(Insn::SetItem(obj, idx, lhs));
                self.writeback_container_if_global(obj_expr, obj);
                self.free_temp(lhs);
                self.free_temp(idx);
                self.free_temp(obj);
            }
            AssignTarget::Slice {
                target: obj_expr,
                lower,
                upper,
                step,
            } => {
                // `l[a:b] OP= rhs` lowers to: read the slice (a fresh copy),
                // apply the in-place op against rhs, then store the result back
                // into the slice. The container is evaluated exactly once, and
                // a single slice-key register is shared between the GetItem read
                // and the SetItem write so bounds are evaluated once too.
                let obj = self.compile_expr(obj_expr);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                let lhs = self.alloc_temp();
                self.emit(Insn::GetItem(lhs, obj, slice_r));
                self.emit_aug_binop(lhs, op, expr);
                self.emit(Insn::SetItem(obj, slice_r, lhs));
                self.writeback_container_if_global(obj_expr, obj);
                self.free_temp(lhs);
                self.free_temp(slice_r);
                self.free_temp(obj);
            }
            AssignTarget::Tuple(_) | AssignTarget::Starred(_) => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(
                        "'tuple' is an illegal expression for augmented assignment".to_string(),
                    );
                }
            }
        }
    }

    // ── Syntax validation for dead-code bodies ───────────────────────────────

    /// Walk `stmts` without emitting code and report any context-sensitive
    /// syntax errors that CPython catches even in unreachable branches.
    ///
    /// Checks enforced:
    /// * `break` / `continue` when `in_loop` is false
    /// * `return` / `yield` / `yield from` when not in a function scope
    /// * `nonlocal` at module level (not inside a function or class body)
    ///
    /// `in_loop` becomes `true` when we recurse into a loop body.
    /// `Stmt::Def` and `Stmt::Class` bodies are recursed with their own scope
    /// rules (is_function_scope / is_class_body) to validate their interiors.
    fn check_dead_block(&mut self, stmts: &[Stmt], in_loop: bool) {
        for stmt in stmts {
            if self.failed {
                return;
            }
            match stmt {
                Stmt::Break if !in_loop => {
                    self.set_syntax_error("'break' outside loop");
                }
                Stmt::Continue if !in_loop => {
                    self.set_syntax_error("'continue' not properly in loop");
                }
                Stmt::Return(_) if !self.is_function_scope => {
                    self.set_syntax_error("'return' outside function");
                }
                Stmt::Expr(expr) => {
                    self.check_dead_expr(expr);
                }
                Stmt::Nonlocal(_) if !self.is_function_scope && !self.is_class_body => {
                    self.set_syntax_error("nonlocal declaration not allowed at module level");
                }
                Stmt::If {
                    branches,
                    else_branch,
                    ..
                } => {
                    for (_, body) in branches {
                        self.check_dead_block(body, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                    }
                }
                Stmt::While {
                    body, else_branch, ..
                } => {
                    self.check_dead_block(body, true);
                    if self.failed {
                        return;
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                    }
                }
                Stmt::For {
                    body, else_branch, ..
                } => {
                    self.check_dead_block(body, true);
                    if self.failed {
                        return;
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                    }
                }
                Stmt::Try {
                    body,
                    handlers,
                    else_branch,
                    finally_branch,
                    ..
                } => {
                    self.check_dead_block(body, in_loop);
                    if self.failed {
                        return;
                    }
                    for handler in handlers {
                        self.check_dead_block(&handler.body, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                    if let Some(finally_stmts) = finally_branch {
                        self.check_dead_block(finally_stmts, in_loop);
                    }
                }
                Stmt::With { body, .. } => {
                    self.check_dead_block(body, in_loop);
                }
                Stmt::Match { arms, .. } => {
                    for arm in arms {
                        self.check_dead_block(&arm.body, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                }
                // Def bodies open a new function scope: break/continue are not
                // valid inside a function (even inside a loop in the enclosing
                // scope), and nonlocal names must be bound in an enclosing
                // function scope.  CPython validates these even for defs that
                // appear in dead-code branches, so we must run the checks here
                // rather than relying on child compilation (which is skipped for
                // dead code).
                Stmt::Def {
                    params,
                    body,
                    is_async,
                    ..
                } => {
                    let inner_nonlocal = crate::interpreter::collect_nonlocal_names(body);
                    let mut sorted_nonlocals: Vec<&String> = inner_nonlocal.iter().collect();
                    sorted_nonlocals.sort();
                    for nonlocal_name in sorted_nonlocals {
                        let in_params = params.iter().any(|p| &p.name == nonlocal_name);
                        let found = in_params
                            || self
                                .outer_locals
                                .iter()
                                .any(|m| m.contains_key(nonlocal_name))
                            || (self.is_function_scope
                                && self.local_index.contains_key(nonlocal_name));
                        if !found {
                            self.set_syntax_error(&format!(
                                "no binding for nonlocal '{}' found",
                                nonlocal_name
                            ));
                            return;
                        }
                    }
                    let saved_is_function_scope = self.is_function_scope;
                    let saved_is_async_function = self.is_async_function;
                    let saved_is_class_body = self.is_class_body;
                    self.is_function_scope = true;
                    self.is_async_function = *is_async;
                    self.is_class_body = false;
                    self.check_dead_block(body, false);
                    self.is_function_scope = saved_is_function_scope;
                    self.is_async_function = saved_is_async_function;
                    self.is_class_body = saved_is_class_body;
                }
                Stmt::Class { body, .. } => {
                    let saved_is_function_scope = self.is_function_scope;
                    let saved_is_class_body = self.is_class_body;
                    self.is_function_scope = false;
                    self.is_class_body = true;
                    self.check_dead_block(body, false);
                    self.is_function_scope = saved_is_function_scope;
                    self.is_class_body = saved_is_class_body;
                }
                // All other statements have no nested blocks or context rules.
                _ => {}
            }
        }
    }

    fn check_dead_expr(&mut self, expr: &Expr) {
        if self.failed {
            return;
        }
        match expr {
            Expr::Yield(_) | Expr::YieldFrom(_) if !self.is_function_scope => {
                self.set_syntax_error("'yield' outside function");
            }
            Expr::Await(_) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'await' outside function");
                } else if !self.is_async_function {
                    self.set_syntax_error("'await' outside async function");
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_dead_expr(left);
                if !self.failed {
                    self.check_dead_expr(right);
                }
            }
            Expr::Unary { expr: e, .. } => {
                self.check_dead_expr(e);
            }
            Expr::Ternary { cond, then, else_ } => {
                self.check_dead_expr(cond);
                if !self.failed {
                    self.check_dead_expr(then);
                }
                if !self.failed {
                    self.check_dead_expr(else_);
                }
            }
            Expr::Call { func, args, .. } => {
                self.check_dead_expr(func);
                for a in args {
                    if self.failed {
                        return;
                    }
                    self.check_dead_expr(&a.value);
                }
            }
            Expr::Tuple(elts) | Expr::List(elts) | Expr::Set(elts) => {
                for e in elts {
                    if self.failed {
                        return;
                    }
                    self.check_dead_expr(e);
                }
            }
            Expr::Named { value, .. } => {
                self.check_dead_expr(value);
            }
            Expr::Lambda { .. }
            | Expr::ListComp { .. }
            | Expr::SetComp { .. }
            | Expr::DictComp { .. }
            | Expr::GenExp { .. } => {}
            _ => {}
        }
    }

    // ── Control flow ──────────────────────────────────────────────────────────

    fn compile_if(
        &mut self,
        branches: &[(Expr, Vec<Stmt>)],
        else_branch: Option<&[Stmt]>,
        branch_linenos: &[Vec<u32>],
        else_linenos: &[u32],
    ) {
        let has_else = else_branch.is_some();
        let n = branches.len();
        let mut end_patches: Vec<usize> = Vec::new();
        let pre_def_set = self.def_set;
        // Collect def_set after each branch body for definite-assignment analysis.
        let mut branch_def_sets: Vec<u64> = Vec::with_capacity(n + 1);

        for (bi, (cond, body)) in branches.iter().enumerate() {
            self.def_set = pre_def_set;
            let body_lns: &[u32] = branch_linenos.get(bi).map(|v| v.as_slice()).unwrap_or(&[]);
            // Constant-condition optimisation: fold at compile time.
            if let Some(val) = fold_constant(cond) {
                if val.truthy_raw() {
                    // Always-true branch: compile body unconditionally; skip rest.
                    self.compile_block_with_linenos(body, body_lns);
                    if self.failed {
                        return;
                    }
                    branch_def_sets.push(self.def_set);
                    // Treat as if there were an else so intersection analysis kicks in.
                    for _ in bi + 1..n {
                        branch_def_sets.push(pre_def_set);
                    }
                    if has_else {
                        branch_def_sets.push(pre_def_set);
                    }
                    // Skipped elif/else bodies are dead code but CPython still
                    // validates their context-sensitive syntax.
                    let in_loop = !self.loops.is_empty();
                    for (_, skipped_body) in &branches[bi + 1..] {
                        self.check_dead_block(skipped_body, in_loop);
                        if self.failed {
                            return;
                        }
                        // A `yield`/`yield from` in a skipped branch still makes
                        // the enclosing function a generator (CPython parity,
                        // issue #1758).
                        if self.is_function_scope && stmts_contain_yield(skipped_body) {
                            self.has_dead_yield = true;
                        }
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                        if self.failed {
                            return;
                        }
                        if self.is_function_scope && stmts_contain_yield(else_stmts) {
                            self.has_dead_yield = true;
                        }
                    }
                    for idx in end_patches {
                        self.patch_jump(idx);
                    }
                    if has_else && !branch_def_sets.is_empty() {
                        let all_define = branch_def_sets.iter().fold(!0u64, |acc, &s| acc & s);
                        self.def_set = pre_def_set | all_define;
                    } else {
                        self.def_set = pre_def_set | branch_def_sets[0];
                    }
                    // Validate skipped elif/else bodies as dead code so that
                    // context-sensitive syntax errors are not silently swallowed.
                    let in_loop = !self.loops.is_empty();
                    for (_, dead_body) in &branches[bi + 1..] {
                        self.check_dead_block(dead_body, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                    }
                    return;
                } else {
                    // Always-false branch: skip emitting code, but still
                    // validate context-sensitive syntax (CPython does this).
                    self.check_dead_block(body, !self.loops.is_empty());
                    if self.failed {
                        return;
                    }
                    // A `yield` / `yield from` in a dead branch still makes
                    // the enclosing function a generator (CPython parity,
                    // issue #1758).  No `Insn::Yield` is emitted for this
                    // branch, so flag it explicitly for `finish()`.
                    if self.is_function_scope && stmts_contain_yield(body) {
                        self.has_dead_yield = true;
                    }
                    continue;
                }
            }
            let cond_reg = self.compile_expr(cond);
            let jmp_false = self.emit_cond_jump(cond_reg, false);
            self.free_temp(cond_reg);
            self.compile_block_with_linenos(body, body_lns);
            if self.failed {
                return;
            }
            branch_def_sets.push(self.def_set);
            self.def_set = pre_def_set;
            if bi < n - 1 || has_else {
                let jmp_end = self.emit(Insn::Jump(0));
                end_patches.push(jmp_end);
            }
            self.patch_jump(jmp_false);
        }
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
            branch_def_sets.push(self.def_set);
        }
        for idx in end_patches {
            self.patch_jump(idx);
        }
        // A variable is definitely bound after the if/elif/else iff it is bound
        // on every possible exit path.
        // With an else: exactly one branch executes, so intersect all branches.
        // Without an else: control may skip all branches (pre_def_set path) or
        // take one branch (branch_def_sets[i] path).  Intersect everything.
        if has_else && !branch_def_sets.is_empty() {
            self.def_set = branch_def_sets.iter().fold(!0u64, |acc, &s| acc & s);
        } else {
            // Include the "no branch taken" path (pre_def_set) in the intersection.
            self.def_set = branch_def_sets.iter().fold(pre_def_set, |acc, &s| acc & s);
        }
    }

    // ── Match/case ────────────────────────────────────────────────────────────

    fn compile_match(&mut self, subject: &Expr, arms: &[MatchArm]) {
        // Evaluate the subject once into a temp register.
        let subj = self.compile_expr(subject);
        let pre_def_set = self.def_set;
        let mut end_patches: Vec<usize> = Vec::new();
        let mut all_arm_def_sets: Vec<u64> = Vec::new();

        for arm in arms {
            self.def_set = pre_def_set;
            // Emit pattern-matching code; collect jump-to-next-arm patches.
            let mut next_arm_patches: Vec<usize> = Vec::new();
            self.compile_pattern_match(subj, &arm.pattern, &mut next_arm_patches);
            if self.failed {
                return;
            }
            // If there's a guard, test it.
            if let Some(guard_expr) = &arm.guard {
                let g = self.compile_expr(guard_expr);
                let jmp = self.emit(Insn::JumpIfFalse(g, 0));
                self.free_temp(g);
                next_arm_patches.push(jmp);
            }
            // Arm body
            self.compile_block_with_linenos(&arm.body, &arm.body_linenos);
            if self.failed {
                return;
            }
            all_arm_def_sets.push(self.def_set);
            // Jump past remaining arms after successful execution.
            let jmp_end = self.emit(Insn::Jump(0));
            end_patches.push(jmp_end);
            // Patch all "no match" jumps to land here (start of next arm).
            for idx in next_arm_patches {
                self.patch_jump(idx);
            }
        }

        // Patch all end-of-arm jumps to land after the whole match.
        for idx in end_patches {
            self.patch_jump(idx);
        }
        self.free_temp(subj);
        // Variables defined in every arm are definitely bound after the match.
        let all_define = if all_arm_def_sets.is_empty() {
            0
        } else {
            all_arm_def_sets.iter().fold(!0u64, |acc, &s| acc & s)
        };
        self.def_set = pre_def_set | all_define;
    }

    /// Emit code that tests whether register `subj` matches `pattern`.
    /// On mismatch, jumps via newly-pushed entries in `fail_patches`
    /// (caller will patch them all to the next arm).
    /// On match, binds any capture variables and falls through.
    /// Compile an OR pattern (`a | b | c`): validate alternatives bind the same
    /// names, then try each in turn, jumping to success on the first match.
    fn compile_or_pattern(
        &mut self,
        subj: Reg,
        alternatives: &[Pattern],
        fail_patches: &mut Vec<usize>,
    ) {
        // Validate that every alternative binds the same set of names
        // (PEP 634; CPython 3.12 raises SyntaxError if they differ).
        //
        // Check first: a bare name capture or wildcard in a non-last
        // position makes every subsequent alternative unreachable —
        // CPython 3.12 emits a dedicated message for each case,
        // distinct from the generic "bind different names" error.
        let non_last = alternatives.len().saturating_sub(1);
        for alt in alternatives.iter().take(non_last) {
            // Recurse into the leading edge of nested OR patterns so that
            // `case (x | 1) | z:` is caught the same way as `case x | z:`.
            if let Some(name) = or_leading_capture(alt) {
                self.set_syntax_error(&format!(
                    "name capture '{}' makes remaining patterns unreachable",
                    name
                ));
                return;
            }
            if or_leading_is_wildcard(alt) {
                self.set_syntax_error("wildcard makes remaining patterns unreachable");
                return;
            }
        }
        if let Some(first) = alternatives.first() {
            let first_names = pattern_bound_names(first);
            for alt in alternatives.iter().skip(1) {
                if pattern_bound_names(alt) != first_names {
                    self.set_syntax_error("alternative patterns bind different names");
                    return;
                }
            }
        }
        // Try each alternative; if one matches, jump to success.
        // If all fail, fall through to after (which will be patched to next arm).
        let mut success_patches: Vec<usize> = Vec::new();
        let n = alternatives.len();
        for (i, alt) in alternatives.iter().enumerate() {
            let mut alt_fail: Vec<usize> = Vec::new();
            self.compile_pattern_match(subj, alt, &mut alt_fail);
            if self.failed {
                return;
            }
            if i < n - 1 {
                // This alternative matched — jump to success.
                let jmp_ok = self.emit(Insn::Jump(0));
                success_patches.push(jmp_ok);
                // Patch the fail of this alternative to try the next one.
                for idx in alt_fail {
                    self.patch_jump(idx);
                }
            } else {
                // Last alternative: its failures propagate to caller.
                fail_patches.extend(alt_fail);
            }
        }
        for idx in success_patches {
            self.patch_jump(idx);
        }
    }

    /// Compile a sequence pattern (`[a, b, *rest]`): exclude non-sequence types,
    /// length-check the subject, then destructure each element (and the star).
    fn compile_sequence_pattern(
        &mut self,
        subj: Reg,
        elements: &[(Pattern, bool)],
        fail_patches: &mut Vec<usize>,
    ) {
        // PEP 634 §3: str, bytes, dict, set, and frozenset are excluded
        // from sequence pattern matching. str/bytes are text sequences;
        // dict/set/frozenset support len() but not integer indexing.
        // A single `MatchSeqExcluded` instruction computes
        // `isinstance(subj, (str, bytes, dict, set, frozenset))` directly
        // — no per-arm `LoadGlobal`/`BuildTuple`/`Call` to rebuild the
        // exclusion tuple on every match execution (issue #1789).  If
        // subj IS one of the excluded types, jump to the fail label.
        {
            let excluded = self.alloc_temp();
            self.emit(Insn::MatchSeqExcluded(excluded, subj));
            let jmp = self.emit(Insn::JumpIfTrue(excluded, 0));
            fail_patches.push(jmp);
            self.free_temp(excluded);
        }

        // Check that subject has exactly `fixed_count` elements
        // (unless there's a star element, then >= fixed_count).
        let has_star = elements.iter().any(|(_, is_star)| *is_star);
        let fixed_count = elements.iter().filter(|(_, s)| !s).count();

        // R_len = len(subj).  Wrap the call in try/except so that a
        // TypeError (subject has no __len__) is treated as a sequence
        // mismatch rather than a propagated error — matching CPython's
        // behaviour for non-sequence types inside OR patterns.
        let len_name_idx = self.intern_name("len");
        let setup_idx = self.emit(Insn::SetupExcept(0));
        let len_fn = self.alloc_temp();
        self.emit(Insn::LoadGlobal(len_fn, len_name_idx));
        let len_arg = self.alloc_temp();
        self.emit(Insn::Move(len_arg, subj));
        self.emit(Insn::Call(len_fn, 1));
        let r_len = len_fn; // result in len_fn after call
        self.free_temp(len_arg);
        // Success path: remove the exception handler.
        self.emit(Insn::PopExcept);
        let jmp_over_handler = self.emit(Insn::Jump(0));
        // Exception handler: any error from len() means the subject is
        // not a sequence — treat as match failure.
        self.patch_jump(setup_idx);
        self.emit(Insn::EndExcept);
        let len_err_jmp = self.emit(Insn::Jump(0));
        fail_patches.push(len_err_jmp);
        self.patch_jump(jmp_over_handler);

        // Check length
        let count_val = self.intern_const(Value::int(fixed_count as i64));
        let len_jmp = if has_star {
            self.emit(Insn::CmpJumpIfFalseConst(r_len, BinaryOp::Ge, count_val, 0))
        } else {
            self.emit(Insn::CmpJumpIfFalseConst(r_len, BinaryOp::Eq, count_val, 0))
        };
        fail_patches.push(len_jmp);
        self.free_temp(r_len);

        // Destructure each element.
        let mut fixed_idx: i64 = 0;
        let mut star_seen = false;
        let total = elements.len();
        for (elem_i, (elem_pat, is_star)) in elements.iter().enumerate() {
            if *is_star {
                star_seen = true;
                // Star element captures subj[fixed_idx:]
                // i.e., subj[fixed_idx : len - (total - elem_i - 1)]
                let trailing = (total - elem_i - 1) as i64;
                if let Pattern::Capture(name) = elem_pat {
                    // Compute start index
                    let start_c = self.intern_const(Value::int(fixed_idx));
                    let start_r = self.alloc_temp();
                    self.emit(Insn::LoadConst(start_r, start_c));
                    // Compute stop index: re-compute len
                    let len2_fn = self.alloc_temp();
                    self.emit(Insn::LoadGlobal(len2_fn, len_name_idx));
                    let arg2 = self.alloc_temp();
                    self.emit(Insn::Move(arg2, subj));
                    self.emit(Insn::Call(len2_fn, 1));
                    self.free_temp(arg2);
                    let r_len2 = len2_fn;
                    let stop_r = if trailing > 0 {
                        let trail_c = self.intern_const(Value::int(trailing));
                        let trail_r = self.alloc_temp();
                        self.emit(Insn::LoadConst(trail_r, trail_c));
                        let stop = self.alloc_temp();
                        self.emit(Insn::BinOp(stop, r_len2, BinaryOp::Sub, trail_r));
                        self.free_temp(trail_r);
                        self.free_temp(r_len2);
                        stop
                    } else {
                        r_len2
                    };
                    // Build slice subj[start:stop] via BuildSlice (issue #931).
                    // Arrange the three bounds in consecutive registers and emit
                    // BuildSlice so the VM unambiguously identifies it as a slice
                    // (not a user 3-tuple).
                    let base = self.alloc_temp();
                    self.emit(Insn::Move(base, start_r));
                    let base1 = self.alloc_temp();
                    self.emit(Insn::Move(base1, stop_r));
                    let base2 = self.alloc_temp();
                    self.emit(Insn::LoadNone(base2));
                    let slice_key = self.alloc_temp();
                    self.emit(Insn::BuildSlice(slice_key, base));
                    self.free_temp(base2);
                    self.free_temp(base1);
                    self.free_temp(base);
                    self.free_temp(stop_r);
                    self.free_temp(start_r);
                    // Get the slice: subj[start:stop] via GetItem with a slice key.
                    // The slice result preserves the subject's type (e.g. tuple →
                    // tuple). CPython guarantees *rest is always a list regardless
                    // of the subject's type, so convert via BuildList + ListExtend.
                    let saved_next = self.next_temp;
                    let slice_r = self.alloc_temp();
                    self.emit(Insn::GetItem(slice_r, subj, slice_key));
                    self.free_temp(slice_key);
                    let list_r = self.alloc_temp();
                    let empty_base = self.next_temp;
                    self.next_temp = empty_base + 1;
                    if empty_base > self.max_reg {
                        self.max_reg = empty_base;
                    }
                    self.emit(Insn::BuildList(list_r, empty_base, 0));
                    self.emit(Insn::ListExtend(list_r, slice_r));
                    // Store into capture name
                    self.compile_store_name(name, list_r);
                    if let Some(reg) = self.local_reg(name) {
                        self.mark_def(reg);
                    }
                    // slice_r / list_r / empty_base cannot be freed in LIFO
                    // order because the phantom empty_base slot sits above
                    // list_r. All three are dead after the store, so restore
                    // next_temp explicitly; max_reg already reflects peak
                    // usage from the empty_base bump above.
                    self.next_temp = saved_next;
                }
                // Don't increment fixed_idx for the star element itself.
                continue;
            }
            // Compute index: if we haven't seen the star yet, use fixed_idx from left.
            // After the star, index from the right.
            let idx_val = if !star_seen {
                fixed_idx
            } else {
                // Negative index (from end): -(fixed_count after star) + offset
                let after_star = elements[elem_i..].iter().filter(|(_, s)| !s).count() as i64;
                -(after_star)
            };
            if !star_seen {
                fixed_idx += 1;
            }

            let idx_c = self.intern_const(Value::int(idx_val));
            let idx_r = self.alloc_temp();
            self.emit(Insn::LoadConst(idx_r, idx_c));
            let elem_r = self.alloc_temp();
            self.emit(Insn::GetItem(elem_r, subj, idx_r));
            self.free_temp(idx_r);
            self.compile_pattern_match(elem_r, elem_pat, fail_patches);
            self.free_temp(elem_r);
            if self.failed {
                return;
            }
        }
    }

    /// Compile a mapping pattern (`{k: p, **rest}`): for each key check
    /// membership and match the value sub-pattern, then bind any `**rest`.
    fn compile_mapping_pattern(
        &mut self,
        subj: Reg,
        pairs: &[(Expr, Pattern)],
        rest_name: Option<&str>,
        fail_patches: &mut Vec<usize>,
    ) {
        // PEP 634 §3: a mapping pattern matches only if the subject is a
        // mapping (`isinstance(subject, collections.abc.Mapping)`).  Guard on
        // that first so a non-mapping subject (int, str, list, set, None, …)
        // fails the match rather than raising on the per-key `in` test below
        // (issue #1879).  Mirrors the `MatchSeqExcluded` gate in
        // `compile_sequence_pattern`; in pyrust the only built-in mapping is
        // `dict` (and its subclasses).
        {
            let is_map = self.alloc_temp();
            self.emit(Insn::MatchMapping(is_map, subj));
            let jmp = self.emit(Insn::JumpIfFalse(is_map, 0));
            fail_patches.push(jmp);
            self.free_temp(is_map);
        }

        // For each key-pattern pair: check key in subject, then match pattern.
        let in_name_idx = self.intern_name("__contains__");
        let _ = in_name_idx; // used indirectly via BinaryOp::In

        for (key_expr, val_pat) in pairs {
            let key_r = self.compile_expr(key_expr);
            // Check: key in subj
            let check_r = self.alloc_temp();
            self.emit(Insn::BinOp(check_r, key_r, BinaryOp::In, subj));
            let jmp = self.emit(Insn::JumpIfFalse(check_r, 0));
            self.free_temp(check_r);
            fail_patches.push(jmp);
            // Get the value: subj[key]
            let val_r = self.alloc_temp();
            self.emit(Insn::GetItem(val_r, subj, key_r));
            self.free_temp(key_r);
            // Match sub-pattern against the value
            self.compile_pattern_match(val_r, val_pat, fail_patches);
            self.free_temp(val_r);
            if self.failed {
                return;
            }
        }
        // If there's a **rest, bind it to subj minus matched keys.
        if let Some(rest) = rest_name {
            // Build a copy of subj and remove matched keys.
            // Simplest: call dict(subj) then del keys.
            let dict_name_idx = self.intern_name("dict");
            let dict_fn = self.alloc_temp();
            self.emit(Insn::LoadGlobal(dict_fn, dict_name_idx));
            let arg = self.alloc_temp();
            self.emit(Insn::Move(arg, subj));
            self.emit(Insn::Call(dict_fn, 1));
            self.free_temp(arg);
            let rest_r = dict_fn; // result in dict_fn
            for (key_expr, _) in pairs {
                let k = self.compile_expr(key_expr);
                self.emit(Insn::DeleteItem(rest_r, k));
                self.free_temp(k);
            }
            self.compile_store_name(rest, rest_r);
            if let Some(reg) = self.local_reg(rest) {
                self.mark_def(reg);
            }
            self.free_temp(rest_r);
        }
    }

    /// Compile a class pattern (`C(p, ..., attr=p)`): isinstance-check the
    /// subject, then match positional (via `__match_args__`) and keyword attrs.
    fn compile_class_pattern(
        &mut self,
        subj: Reg,
        cls: &Expr,
        positional: &[Pattern],
        kwargs: &[(String, Pattern)],
        fail_patches: &mut Vec<usize>,
    ) {
        // isinstance(subj, cls) check must come FIRST so that attribute
        // access is never attempted on a subject of the wrong type.
        let isinstance_name_idx = self.intern_name("isinstance");
        let isinstance_fn = self.alloc_temp();
        self.emit(Insn::LoadGlobal(isinstance_fn, isinstance_name_idx));
        let arg0 = self.alloc_temp();
        self.emit(Insn::Move(arg0, subj));
        let cls_r = self.compile_expr(cls);
        let arg1 = self.alloc_temp();
        self.emit(Insn::Move(arg1, cls_r));
        // Keep cls_r alive when we have positional sub-patterns: the
        // MatchClassPositional instruction needs the class to load
        // __match_args__ from it.
        let cls_for_pos = if !positional.is_empty() {
            let saved = self.alloc_temp();
            self.emit(Insn::Move(saved, cls_r));
            self.free_temp(cls_r);
            Some(saved)
        } else {
            self.free_temp(cls_r);
            None
        };
        self.emit(Insn::Call(isinstance_fn, 2));
        self.free_temp(arg1);
        self.free_temp(arg0);
        let jmp = self.emit(Insn::JumpIfFalse(isinstance_fn, 0));
        fail_patches.push(jmp);
        self.free_temp(isinstance_fn);
        // Positional sub-patterns: resolved via __match_args__.
        if !positional.is_empty() {
            let cls_reg = cls_for_pos.expect("set above when positional non-empty");
            let n = positional.len() as u8;
            // Allocate a contiguous block of n temporaries for the
            // attribute values loaded by MatchClassPositional.
            let dst_base = self.alloc_temp();
            for _ in 1..positional.len() {
                self.alloc_temp();
            }
            self.emit(Insn::MatchClassPositional {
                dst_base,
                subj,
                cls: cls_reg,
                n,
            });
            self.free_temp(cls_reg);
            // Match each positional attribute value against its sub-pattern.
            for (i, pat) in positional.iter().enumerate() {
                let attr_r = dst_base + i as u32;
                self.compile_pattern_match(attr_r, pat, fail_patches);
                if self.failed {
                    // Free remaining allocated registers before returning.
                    for j in i..positional.len() {
                        self.free_temp(dst_base + j as u32);
                    }
                    return;
                }
                self.free_temp(attr_r);
            }
        } else if let Some(cls_reg) = cls_for_pos {
            self.free_temp(cls_reg);
        }
        // Keyword sub-patterns: matched directly against named attributes.
        for (attr_name, attr_pat) in kwargs {
            let name_idx = self.intern_name(attr_name);
            let attr_r = self.alloc_temp();
            self.emit(Insn::GetAttr(attr_r, subj, name_idx));
            self.compile_pattern_match(attr_r, attr_pat, fail_patches);
            self.free_temp(attr_r);
            if self.failed {
                return;
            }
        }
    }

    fn compile_pattern_match(
        &mut self,
        subj: Reg,
        pattern: &Pattern,
        fail_patches: &mut Vec<usize>,
    ) {
        if self.failed {
            return;
        }
        match pattern {
            Pattern::Wildcard => {
                // Always matches, nothing to do.
            }
            Pattern::Capture(name) => {
                // Bind subj to name, always succeeds.
                self.compile_store_name(name, subj);
                if let Some(reg) = self.local_reg(name) {
                    self.mark_def(reg);
                }
            }
            Pattern::Literal(expr) => {
                // Emit: if subj != literal → fail
                let lit = self.compile_expr(expr);
                let jmp = self.emit(Insn::CmpJumpIfFalse(subj, BinaryOp::Eq, lit, 0));
                self.free_temp(lit);
                fail_patches.push(jmp);
            }
            Pattern::Value(expr) => {
                // Value pattern: evaluate the dotted attribute expression and
                // compare with == (same as a literal match, no binding).
                let val = self.compile_expr(expr);
                let jmp = self.emit(Insn::CmpJumpIfFalse(subj, BinaryOp::Eq, val, 0));
                self.free_temp(val);
                fail_patches.push(jmp);
            }
            Pattern::Or(alternatives) => {
                self.compile_or_pattern(subj, alternatives, fail_patches);
            }
            Pattern::Sequence(elements) => {
                self.compile_sequence_pattern(subj, elements, fail_patches);
            }
            Pattern::Mapping(pairs, rest_name) => {
                self.compile_mapping_pattern(subj, pairs, rest_name.as_deref(), fail_patches);
            }
            Pattern::Class {
                cls,
                positional,
                kwargs,
            } => {
                self.compile_class_pattern(subj, cls, positional, kwargs, fail_patches);
            }
            Pattern::As { pattern, name } => {
                // Compile the inner pattern first (may add to fail_patches).
                self.compile_pattern_match(subj, pattern, fail_patches);
                if self.failed {
                    return;
                }
                // If we reach here the inner pattern matched; bind the entire
                // subject (not just the matched portion) to `name`.
                self.compile_store_name(name, subj);
                if let Some(reg) = self.local_reg(name) {
                    self.mark_def(reg);
                }
            }
        }
    }

    fn compile_while(
        &mut self,
        cond: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
    ) {
        if is_const_false_expr(cond) {
            // The while body is statically unreachable, but CPython still
            // validates context-sensitive syntax inside it.  The body counts
            // as a loop context (break/continue inside it are valid), but
            // return/yield are still gated by is_function_scope.
            self.check_dead_block(body, true);
            if self.failed {
                return;
            }
            // A `yield`/`yield from` in a dead `while False` body still makes
            // the enclosing function a generator (CPython parity, issue #1758).
            if self.is_function_scope && stmts_contain_yield(body) {
                self.has_dead_yield = true;
            }
            if let Some(else_stmts) = else_branch {
                self.compile_block_with_linenos(else_stmts, else_linenos);
            }
            return;
        }

        let is_infinite_initial = matches!(cond, Expr::Bool(true) | Expr::Int(1));

        // Collapse `if guard: continue; <rest>` at any top-level position in the
        // loop body (issue #287). The rewrite removes the redundant
        // `JumpIfFalse(g, +1) + Jump(loop_start)` trampoline.
        let rewritten = rewrite_continue_top(body.to_vec());

        // For `while True: if c: break; rest`, rewrite the loop to
        // `while not c: rest` (issue #282).  The else clause is unreachable on
        // an infinite while (the only natural exit was via break, which skips
        // else), so it is safe to drop it before rewriting — after the rewrite
        // the new "c-becomes-true" exit is a natural exit and would otherwise
        // resurrect the dropped else.  We only fire when there was no else to
        // begin with, which keeps the transform purely local.
        let try_break_rewrite = is_infinite_initial
            && else_branch.is_none()
            && matches!(
                rewritten.first(),
                Some(Stmt::If { branches, else_branch: None, .. })
                    if branches.len() == 1
                        && matches!(branches[0].1.as_slice(), [Stmt::Break])
            );
        let (cond_owned, body_owned, is_infinite) = if try_break_rewrite {
            let (new_cond, new_body) = rewrite_break_top(rewritten).expect("guard checked");
            (Some(new_cond), new_body, false)
        } else {
            (None, rewritten, is_infinite_initial)
        };
        let cond: &Expr = cond_owned.as_ref().unwrap_or(cond);
        let body: &[Stmt] = &body_owned;

        if !is_infinite
            && !body_has_continue(body)
            && self.try_compile_while_range(cond, body, else_branch, body_linenos, else_linenos)
        {
            return;
        }

        // LICM (hoisting the condition check before the loop so the back-edge
        // skips re-evaluation) is only sound when `bool(cond)` is provably
        // constant across iterations.  `expr_is_invariant` checks that no name
        // in `cond` is *reassigned* in the body — but a bare container variable's
        // truthiness also changes when the body mutates the object *in place*
        // (`x.pop()`, `x[i] = v`, `del x[i]`, …) without ever touching the
        // register (issue #2034).  In-place mutation can reach the condition's
        // object through aliasing we cannot disprove statically, so we
        // conservatively disable LICM whenever the body performs any in-place
        // object mutation.  The genuine win-case (a `while not done:` flag loop
        // that exits via `break`, with no mutation in its body) is unaffected.
        let is_licm = !is_infinite && {
            let written = collect_body_written(body);
            expr_is_invariant(cond, &written) && !stmts_may_mutate_object(body)
        };

        let (loop_start, exit_jmp) = if is_licm {
            let cond_reg = self.compile_expr(cond);
            let jmp = self.emit_cond_jump(cond_reg, false);
            self.free_temp(cond_reg);
            (self.pc(), Some(jmp))
        } else if is_infinite {
            (self.pc(), None)
        } else {
            let start = self.pc();
            let cond_reg = self.compile_expr(cond);
            let jmp = self.emit_cond_jump(cond_reg, false);
            self.free_temp(cond_reg);
            (start, Some(jmp))
        };

        self.loops.push(LoopCtx {
            break_patches: SmallVec::new(),
            continue_target: Some(loop_start),
            continue_patches: SmallVec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved = self.def_set;
        self.compile_block_with_linenos(body, body_linenos);
        self.def_set = saved;
        if self.failed {
            return;
        }
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));
        if let Some(jmp) = exit_jmp {
            self.patch_jump(jmp);
        }
        let ctx = self.loops.pop().unwrap();
        // For an infinite while (e.g. `while True:`) the else clause is unreachable:
        // the loop never exits naturally, and `break` deliberately skips the else.
        // Skip the emit entirely — semantics-preserving, avoids dead bytecode.
        if !is_infinite && let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
    }

    /// Convert `while VAR cmp STOP: ...; VAR += STEP` to a ForCount* integer counter.
    ///
    /// Uses ForCountConst/ForCountReg (same as the for-range optimisation) instead of
    /// ForIter+IterState::Range.  This avoids the range() call, the IterState allocation,
    /// and the indirect ForIter dispatch, giving a tight integer-counter loop.
    ///
    /// Semantics mirror try_compile_for_range: initialise VAR = i_initial - step so the
    /// first ForCount yields i_initial.  After natural exit emit BinOpConst to restore
    /// the post-loop value (Python requires i == stop after `while i < stop: …; i += 1`).
    fn try_compile_while_range(
        &mut self,
        cond: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
    ) -> bool {
        let (var_name, stop_expr, step, inclusive) = match detect_while_range(cond, body) {
            Some(x) => x,
            None => return false,
        };
        let var_reg = match self.local_reg(var_name) {
            Some(r) => r,
            None => return false,
        };

        let cmp_op = if step > 0 { BinaryOp::Lt } else { BinaryOp::Gt };
        let step_idx = self.intern_const(Value::int(step));
        let neg_step = step.wrapping_neg();

        // Initialise var_reg = i_initial - step (so first ForCount yields i_initial).
        self.emit_int_binop(var_reg, var_reg, BinaryOp::Add, neg_step);

        // Determine stop value: for inclusive (<=/>= condition) adjust by ±1.
        let loop_start = self.pc();
        let stop_adjust: i64 = if inclusive {
            if step > 0 { 1 } else { -1 }
        } else {
            0
        };

        let (exit_jmp, stop_temp) = if let Some(mut stop_val) = extract_literal_int(stop_expr) {
            stop_val = stop_val.wrapping_add(stop_adjust);
            let stop_idx = self.intern_const(Value::int(stop_val));
            let jmp = self.emit(Insn::ForCountConst(var_reg, cmp_op, stop_idx, step_idx, 0));
            (jmp, None)
        } else {
            let r = self.compile_expr(stop_expr);
            let sr = if r < self.base_temp {
                let t = self.alloc_temp();
                self.emit(Insn::Move(t, r));
                t
            } else {
                r
            };
            if stop_adjust != 0 {
                self.emit_int_binop(sr, sr, BinaryOp::Add, stop_adjust);
            }
            let jmp = self.emit(Insn::ForCountReg(var_reg, cmp_op, sr, step_idx, 0));
            (jmp, Some(sr))
        };

        self.mark_def(var_reg);
        self.loops.push(LoopCtx {
            break_patches: SmallVec::new(),
            continue_target: Some(loop_start),
            continue_patches: SmallVec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved = self.def_set;
        // Skip the last body statement (VAR += STEP): ForCount already manages
        // the counter increment, so VAR += STEP is a dead store.
        let body_without_inc = &body[..body.len() - 1];
        // body_linenos for body_without_inc: same prefix (last stmt is the inc).
        let body_lns_without_inc = if body_linenos.len() > body_without_inc.len() {
            &body_linenos[..body_without_inc.len()]
        } else {
            body_linenos
        };
        self.compile_block_with_linenos(body_without_inc, body_lns_without_inc);
        self.def_set = saved;
        if self.failed {
            return true;
        }
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));
        self.patch_jump(exit_jmp);
        // Restore post-loop value: Python semantics require i == stop after natural exit.
        // Break patches jump PAST this BinOpImm/BinOpConst so break leaves the break-iteration value.
        self.emit_int_binop(var_reg, var_reg, BinaryOp::Add, step);
        let ctx = self.loops.pop().unwrap();
        if let Some(t) = stop_temp {
            self.free_temp(t);
        }
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return true;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
        true
    }

    /// Detect `for VAR in range(...)` and compile to a direct integer counter loop:
    ///   VAR = start; while VAR < stop: body; VAR += step
    /// This avoids calling range(), allocating an IterState, and the ForIter
    /// overhead per iteration, giving a tight loop equivalent to C `for`.
    fn try_compile_for_range(
        &mut self,
        target: &AssignTarget,
        iter_expr: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
    ) -> bool {
        // Target must be a Name with a fastlocal register.
        let var_name = match target {
            AssignTarget::Name(n) => n.as_str(),
            _ => return false,
        };
        let var_reg = match self.local_reg(var_name) {
            Some(r) => r,
            None => return false,
        };

        // iter_expr must be a plain `range(...)` call with no splats/kwargs.
        let (func, args) = match iter_expr {
            Expr::Call { func, args, .. } => (func.as_ref(), args.as_slice()),
            _ => return false,
        };
        if !matches!(func, Expr::Var(n, _) if n == "range") {
            return false;
        }
        if args
            .iter()
            .any(|a| a.splat || a.double_splat || a.name.is_some())
        {
            return false;
        }

        // Extract (start_opt, stop, step) from 1–3 positional args.
        let (start_opt, stop_expr, step_val): (Option<&Expr>, &Expr, i64) = match args {
            [s] => (None, &s.value, 1),
            [a, b] => (Some(&a.value), &b.value, 1),
            [a, b, c] => {
                let s = match extract_literal_int(&c.value) {
                    Some(v) => v,
                    None => return false,
                };
                if s == 0 {
                    return false;
                }
                (Some(&a.value), &b.value, s)
            }
            _ => return false,
        };

        // ForCount semantics: initialise var = start - step_val, then on each
        // iteration: next = var + step_val; if next op stop → var=next (continue)
        // else → jump (exit).  This way the body always sees the current value
        // and the variable retains its last iteration value after normal exit.
        let cmp_op = if step_val > 0 {
            BinaryOp::Lt
        } else {
            BinaryOp::Gt
        };
        let step_idx = self.intern_const(Value::int(step_val));

        // ── 1. Initialise var_reg = start - step_val ─────────────────────
        //    For the common range(n) case (start=0), init = -step_val.
        let neg_step_val = step_val.wrapping_neg();
        if let Some(start) = start_opt {
            let r = self.compile_expr(start);
            // var_reg = r + (-step_val) = r - step_val; use BinOpImm when it fits.
            self.emit_int_binop(var_reg, r, BinaryOp::Add, neg_step_val);
            self.free_temp(r);
        } else {
            // start = 0; init = -step_val.  Intern the constant only on this path
            // so that range(start, stop) with a non-zero start doesn't pollute the
            // const pool with an unreferenced neg_step entry.
            let neg_step_idx = self.intern_const(Value::int(neg_step_val));
            self.emit(Insn::LoadConst(var_reg, neg_step_idx));
        }

        // ── 2. ForCount instruction at loop top ───────────────────────────
        //    If stop is a literal integer use the Const variant (avoids a temp
        //    register and a register read each iteration).  Otherwise compile
        //    stop once into a temp that lives for the loop duration.
        let loop_start = self.pc();
        let (exit_jmp, stop_temp) = if let Some(stop_val) = extract_literal_int(stop_expr) {
            let stop_idx = self.intern_const(Value::int(stop_val));
            let jmp = self.emit(Insn::ForCountConst(var_reg, cmp_op, stop_idx, step_idx, 0));
            (jmp, None)
        } else {
            let r = self.compile_expr(stop_expr);
            // Copy to a temp if the stop expression resolved to a local register
            // so body mutations of that variable don't affect our bound.
            let sr = if r < self.base_temp {
                let t = self.alloc_temp();
                self.emit(Insn::Move(t, r));
                t
            } else {
                r
            };
            let jmp = self.emit(Insn::ForCountReg(var_reg, cmp_op, sr, step_idx, 0));
            (jmp, Some(sr))
        };

        // ── 3. Body ──────────────────────────────────────────────────────
        //    `continue` → loop_start (ForCount does the increment automatically)
        self.mark_def(var_reg);
        // In a class body, the iteration variable becomes a class attr — but only
        // when at least one iteration actually runs.  ForCount* falls through to
        // the body only after the bounds test passes, so emitting RecordClassStore
        // here gives us the correct conditional semantics for free.
        self.maybe_record_class_store(var_reg);
        // Issue #820: sync the iteration variable into module_globals_dict at
        // module scope so that globals() returns the live value.  The ForCount*
        // instruction writes var_reg; SyncModuleGlobal (a NOP when globals_accessed
        // == false) must follow immediately so it fires before the body runs.
        if self.is_module_scope {
            let name_idx = self.intern_name(var_name);
            self.emit(Insn::SyncModuleGlobal(var_reg, name_idx));
        }
        self.loops.push(LoopCtx {
            break_patches: SmallVec::new(),
            continue_target: Some(loop_start),
            continue_patches: SmallVec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved = self.def_set;
        self.compile_block_with_linenos(body, body_linenos);
        self.def_set = saved;
        if self.failed {
            return true;
        }

        // ── 4. Back edge ─────────────────────────────────────────────────
        let back_offset = loop_start as i32 - (self.pc() as i32 + 1);
        self.emit(Insn::Jump(back_offset));

        // ── 5. Exit + else ───────────────────────────────────────────────
        self.patch_jump(exit_jmp);
        if let Some(sr) = stop_temp {
            self.free_temp(sr);
        }
        let ctx = self.loops.pop().unwrap();
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return true;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
        true
    }

    fn compile_for(
        &mut self,
        target: &AssignTarget,
        iter_expr: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
    ) {
        // Collapse the `if guard: continue; <rest>` trampoline (issue #287)
        // before dispatching: this also lets `try_compile_for_range` see the
        // simpler body shape if the rewrite eliminates all `continue`s.
        let rewritten = rewrite_continue_top(body.to_vec());
        let body: &[Stmt] = &rewritten;

        if self.try_compile_for_range(
            target,
            iter_expr,
            body,
            else_branch,
            body_linenos,
            else_linenos,
        ) {
            return;
        }
        let iter_slot = self.alloc_iter();
        let src = self.compile_expr(iter_expr);
        self.emit(Insn::GetIter(iter_slot, src));
        self.free_temp(src);
        let loop_start = self.pc();
        // For a local-variable target, write ForIter directly into the local register
        // to avoid an extra Move per iteration. For all other cases, use a temp.
        let for_dst = if let AssignTarget::Name(n) = target {
            self.local_reg(n).unwrap_or_else(|| self.alloc_temp())
        } else {
            self.alloc_temp()
        };
        let exit_jmp = self.emit(Insn::ForIter(for_dst, iter_slot, 0));
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    // local case: for_dst == reg, already written — no Move needed.
                    // Still record the store so class-body for-loops register the
                    // iteration variable in `vars(C)`.
                    self.maybe_record_class_store(reg);
                    // Issue #820: sync into module_globals_dict at module scope.
                    if self.is_module_scope {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                    }
                } else {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::StoreGlobal(name_idx, for_dst));
                    self.free_temp(for_dst);
                }
            }
            AssignTarget::Tuple(targets) => {
                let star_pos = targets
                    .iter()
                    .position(|t| matches!(t, AssignTarget::Starred(_)));
                let n = targets.len() as u32;
                let base = for_dst + 1;

                if let Some(star_idx) = star_pos {
                    // Extended unpack: for a, *b, c in ...
                    let before = star_idx as u8;
                    let after = (targets.len() - star_idx - 1) as u8;
                    if base.checked_add(n).is_none() {
                        self.failed = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(format!("too many unpack targets ({})", n));
                        }
                        return;
                    }
                    self.next_temp = base + n;
                    if self.next_temp - 1 > self.max_reg {
                        self.max_reg = self.next_temp - 1;
                    }
                    self.emit(Insn::UnpackEx {
                        src: for_dst,
                        before,
                        after,
                        dst_base: base,
                    });
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        let inner = match t {
                            AssignTarget::Starred(inner) => inner.as_ref(),
                            other => other,
                        };
                        self.compile_store_unpack_target(inner, base + i);
                    }
                } else {
                    if base.checked_add(n).is_none() {
                        self.failed = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(format!("too many unpack targets ({})", n));
                        }
                        return;
                    }
                    self.next_temp = base + n;
                    if self.next_temp - 1 > self.max_reg {
                        self.max_reg = self.next_temp - 1;
                    }
                    self.emit(Insn::Unpack(base, for_dst, n));
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        self.compile_store_unpack_target(t, base + i);
                        if self.failed {
                            return;
                        }
                    }
                }
                self.next_temp = for_dst;
            }
            _ => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("unsupported for-loop target".to_string());
                }
                return;
            }
        }
        self.loops.push(LoopCtx {
            break_patches: SmallVec::new(),
            continue_target: Some(loop_start),
            continue_patches: SmallVec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved_def_set = self.def_set;
        self.mark_target_def(target);
        self.compile_block_with_linenos(body, body_linenos);
        self.def_set = saved_def_set;
        if self.failed {
            return;
        }
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));
        self.patch_jump(exit_jmp);
        let ctx = self.loops.pop().unwrap();
        self.free_iter();
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
    }

    // ── Raise / Delete / Import ───────────────────────────────────────────────

    fn compile_raise(
        &mut self,
        expr: Option<&Expr>,
        cause: Option<&Expr>,
        // PEP 657 whole-statement caret anchor for `raise <expr>` (#2411).
        raise_span: Option<crate::ast::CaretSpan>,
    ) {
        // If we are inside an except handler body and that handler's enclosing
        // try/except/finally has a finally clause, the compiler already popped the
        // outer SetupExcept from the VM's exc_handlers stack (to avoid double-running
        // finally on exceptions from the handler body).  A `raise` statement exits the
        // handler body, so we must inline the finally block here before emitting the
        // raise instruction — the VM won't see the outer handler on exc_handlers.
        // True only when we're in an except-handler body that has a finally
        // clause to inline.  The finally clause is the only reason we need
        // `LoadExc` before the cleanup: without it, `RaiseReRaise` can rely
        // on `active_exception` directly.
        let in_except_body_with_finally = self.except_cleanups.iter().any(|c| {
            matches!(
                c,
                EarlyExitCleanup::ExceptBody {
                    finally_stmts: Some(_),
                    ..
                }
            )
        });

        // Compile the raise expressions BEFORE any cleanup, so that references
        // to `except ... as var` bindings resolve (e.g. `raise TypeError() from e`).
        //
        // For bare `raise` when inside an except handler body:
        //   `emit_raise_cleanups` inlines the finally block, which may contain
        //   a try/except that catches an exception.  If that inner exception
        //   matches the outer handler's context entry, `handle_vm_error`'s
        //   de-duplication logic removes it from `handled_exc_stack`, leaving
        //   `active_exception = None` by the time `RaiseReRaise` runs.
        //   Fix: save the current exception via `LoadExc` into a temp before
        //   the cleanup and re-raise it as `RaiseValue` (which doesn't rely on
        //   `active_exception` at the raise site).
        let bare_reraise_tmp: Option<Reg> = if expr.is_none() && in_except_body_with_finally {
            let tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(tmp));
            Some(tmp)
        } else {
            None
        };

        // Compile the raise expressions (for non-bare raise forms).
        //
        // `emit_raise_cleanups` will delete the `as VAR` binding (PEP 3110)
        // before inlining the finally block.  If the cause expression happens
        // to BE the deleted local variable register, we must copy its value
        // into a fresh temp *before* the deletion occurs.  We therefore call
        // `ensure_temp` on both `r` and `c` after evaluating them.
        let compiled = match expr {
            None => None,
            Some(e) => {
                let r = self.compile_expr(e);
                // Copy to temp if r is a fastlocal (ensure_temp = alloc+Move).
                let r = self.ensure_temp(r);
                let c = cause.map(|ce| {
                    let c = self.compile_expr(ce);
                    self.ensure_temp(c)
                });
                Some((r, c))
            }
        };
        if self.failed {
            if let Some(tmp) = bare_reraise_tmp {
                self.free_temp(tmp);
            }
            if let Some((r, c)) = compiled {
                if let Some(c) = c {
                    self.free_temp(c);
                }
                self.free_temp(r);
            }
            return;
        }

        // For a non-bare raise inside an except handler with a finally: pass the
        // register holding the to-be-raised exception so that emit_raise_cleanups
        // can temporarily install it as the active context before inlining the
        // finally block.  Bare raises don't need this because the active exception
        // (which is already on handled_exc_stack) is the one being re-raised.
        let pending_exc_reg = compiled.as_ref().map(|(r, _)| *r);
        self.emit_raise_cleanups(pending_exc_reg);
        if self.failed {
            if let Some(tmp) = bare_reraise_tmp {
                self.free_temp(tmp);
            }
            if let Some((r, c)) = compiled {
                if let Some(c) = c {
                    self.free_temp(c);
                }
                self.free_temp(r);
            }
            return;
        }
        // PEP 657 caret anchor (#2411): `raise <expr>` underlines the whole
        // raise statement (CPython behaviour).  The RaiseValue/RaiseFrom
        // instruction is what raises, so arm the statement span onto it; the
        // formatter omits it when it covers the whole dedented line (a bare
        // `raise name` at statement scope).
        match (compiled, bare_reraise_tmp) {
            // Bare `raise` inside an except handler body with a finally: use the
            // saved exception value so the re-raise is independent of
            // `active_exception`, which may have been cleared by the inlined
            // finally block's own exception handling.
            (None, Some(tmp)) => {
                self.emit(Insn::RaiseValue(tmp));
                self.free_temp(tmp);
            }
            // Bare `raise` outside any except body: rely on `active_exception`.
            (None, None) => {
                self.emit(Insn::RaiseReRaise);
            }
            (Some((r, Some(c))), _) => {
                self.set_col_span_for_next(raise_span);
                self.emit(Insn::RaiseFrom(r, c));
                self.free_temp(c);
                self.free_temp(r);
            }
            (Some((r, None)), _) => {
                self.set_col_span_for_next(raise_span);
                self.emit(Insn::RaiseValue(r));
                self.free_temp(r);
            }
        }
    }

    /// Emit code that fills three *contiguous* registers with the slice bounds
    /// `(lo, hi, step)` — each missing bound becomes `LoadNone`. Returns the base
    /// register; the bounds occupy `[base, base+3)`. Shared by `compile_slice_key`
    /// (which wraps them in a `slice` object via `BuildSlice`) and the rvalue
    /// `GetSlice` fast path (which reads them directly).
    fn compile_slice_bounds(
        &mut self,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Reg {
        // Allocate three *contiguous* slots upfront so that compaction moves cannot
        // alias a later slot with one that was just written. The previous approach
        // compiled each bound into whatever register compile_expr or alloc_temp
        // returned, then slid them into contiguous positions — but the slide could
        // overwrite a "step" register that had been allocated at the same position
        // as the upper slot, causing the step value to hold the upper bound instead
        // of None. (Repro: `a[:x]` where x is a local variable.)
        let lo_slot = self.alloc_temp(); // base
        let hi_slot = self.alloc_temp(); // base + 1
        let st_slot = self.alloc_temp(); // base + 2

        // Fill each slot: compile the expression into whatever register the
        // sub-expression naturally lands in, then Move it into the reserved slot
        // and release the source. If the expression already landed in the right
        // slot (e.g. a nested alloc_temp gave us exactly that register), skip the
        // move to avoid a redundant copy. When the bound is absent, emit LoadNone
        // directly into the slot — no temp needed.
        let fill_slot = |this: &mut Self, slot: Reg, expr: Option<&Expr>| {
            if let Some(e) = expr {
                let src = this.compile_expr(e);
                if src != slot {
                    this.emit(Insn::Move(slot, src));
                    this.free_temp(src);
                }
            } else {
                this.emit(Insn::LoadNone(slot));
            }
        };

        fill_slot(self, lo_slot, lower);
        fill_slot(self, hi_slot, upper);
        fill_slot(self, st_slot, step);
        lo_slot
    }

    /// Build the 3-element slice-key object `(lo, hi, step)` used by GetItem/SetItem/DeleteItem.
    /// Each missing bound is represented as `None`. Returns the register holding the slice.
    fn compile_slice_key(
        &mut self,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Reg {
        let lo_slot = self.compile_slice_bounds(lower, upper, step);
        // The three slots are already contiguous; BuildSlice reads [lo_slot .. lo_slot+3).
        // BuildSlice (not BuildTuple) so the VM can unambiguously distinguish a
        // compiler-generated slice key from a user 3-tuple (issue #931).
        let slice_r = self.alloc_temp();
        self.emit(Insn::BuildSlice(slice_r, lo_slot));
        // Release the three component slots — they are consumed by BuildSlice.
        self.next_temp = slice_r + 1;
        slice_r
    }

    fn compile_delete(&mut self, expr: &Expr) {
        match expr {
            Expr::Var(name, _) => {
                if let Some(reg) = self.local_reg(name) {
                    // Pass the name index so the VM can raise NameError /
                    // UnboundLocalError when the register was never assigned.
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::DeleteLocal(reg, name_idx));
                    // Clear the definitely-bound bit so that any subsequent
                    // read of this name emits CheckLocal and raises the correct
                    // exception (UnboundLocalError at function scope, NameError
                    // at module scope) rather than falling through to vm_read's
                    // generic "local variable referenced before assignment" path.
                    if (reg as usize) < 64 {
                        self.def_set &= !(1u64 << reg);
                    }
                    self.maybe_record_class_del(reg);
                    // Issue #820: at module scope, also remove the name from
                    // env.values and module_globals_dict so that LoadGlobal
                    // from nested functions / after globals() cannot resurrect it.
                    if self.is_module_scope {
                        self.emit(Insn::DeleteModuleGlobal(name_idx));
                    }
                } else {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::DeleteName(name_idx));
                }
            }
            Expr::Attr { target, name, .. } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                self.emit(Insn::DeleteAttr(obj, name_idx));
                self.free_temp(obj);
            }
            Expr::Index { target, index, .. } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                self.emit(Insn::DeleteItem(obj, idx));
                self.writeback_container_if_global(target, obj);
                self.free_temp(idx);
                self.free_temp(obj);
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                let obj = self.compile_expr(target);
                let slice_reg =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                self.emit(Insn::DeleteItem(obj, slice_reg));
                self.writeback_container_if_global(target, obj);
                self.free_temp(slice_reg);
                self.free_temp(obj);
            }
            _ => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("unsupported delete target".to_string());
                }
            }
        }
    }

    fn compile_import(&mut self, names: &[(String, Option<String>)]) {
        for (module_name, alias) in names {
            match alias {
                Some(alias) => {
                    // `import a.b.c as alias` — bind the leaf module under
                    // the alias directly.  No parent walk needed because
                    // the user explicitly renamed.
                    let mod_idx = self.intern_name(module_name);
                    let dst = self.alloc_temp();
                    self.emit(Insn::ImportModule(dst, mod_idx));
                    self.compile_store_name(alias, dst);
                    self.free_temp(dst);
                }
                None => {
                    // `import a.b.c` — CPython binds the *topmost* component
                    // (`a`), and `a.b.c` is reached via attribute chains on
                    // the loaded packages.
                    let top = module_name.split('.').next().unwrap_or(module_name);
                    if top == module_name {
                        // Non-dotted: one import that binds directly under
                        // the name — no parent walk involved.
                        let mod_idx = self.intern_name(module_name);
                        let dst = self.alloc_temp();
                        self.emit(Insn::ImportModule(dst, mod_idx));
                        self.compile_store_name(module_name, dst);
                        self.free_temp(dst);
                    } else {
                        // Dotted: first ensure the leaf is loaded (which
                        // populates the cache and lets the parent-package
                        // identity fix-up in `Interpreter::load_module`
                        // stitch its submodule attrs to the cached
                        // value); then load the topmost component and
                        // bind it.
                        let full_idx = self.intern_name(module_name);
                        let full_reg = self.alloc_temp();
                        self.emit(Insn::ImportModule(full_reg, full_idx));
                        self.free_temp(full_reg);
                        let top_idx = self.intern_name(top);
                        let top_reg = self.alloc_temp();
                        self.emit(Insn::ImportModule(top_reg, top_idx));
                        self.compile_store_name(top, top_reg);
                        self.free_temp(top_reg);
                    }
                }
            }
        }
    }

    fn compile_import_from(&mut self, module: &str, names: &[(String, Option<String>)]) {
        // `from __future__ import X` is a compiler directive in CPython — no
        // runtime import is performed.  Validate the feature name(s) and emit
        // nothing (no-op).  Unrecognised names or star-imports are SyntaxErrors
        // (matching CPython 3.12 behaviour).
        if module == "__future__" {
            // CPython 3.12: `from __future__` is only legal at the top of a
            // module — not inside functions, class bodies, or after any
            // non-__future__ statement (other than the module docstring).
            if !self.is_module_scope || self.past_future_zone {
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(
                        "from __future__ imports must occur at the beginning of the file"
                            .to_string(),
                    );
                }
                return;
            }
            const VALID: &[&str] = &[
                "nested_scopes",
                "generators",
                "division",
                "absolute_import",
                "with_statement",
                "print_function",
                "unicode_literals",
                "barry_as_FLUFL",
                "generator_stop",
                "annotations",
            ];
            for (name, _alias) in names {
                if !VALID.contains(&name.as_str()) {
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some(format!("future feature {} is not defined", name));
                    }
                    return;
                }
            }
            // All names are valid.  Activate compiler flags for directives
            // that affect code generation.
            if names.iter().any(|(n, _)| n == "annotations") {
                self.future_annotations = true;
            }
            // Fall through to the ordinary import-from bytecode path below.
            // CPython 3.12 also emits real import bytecode for
            // `from __future__ import X` (IMPORT_NAME followed by
            // IMPORT_FROM + STORE_NAME), so the feature name is bound in
            // the module namespace and `import __future__; __future__.X` also
            // works.  With a real `__future__` module stub in the registry
            // the emitted ImportModule / ImportFromAttr / StoreGlobal sequence
            // resolves correctly and the binding is visible at runtime.
        }

        let mod_idx = self.intern_name(module);
        let mod_reg = self.alloc_temp();
        self.emit(Insn::ImportModule(mod_reg, mod_idx));
        if names.len() == 1 && names[0].0 == "*" {
            // CPython: `from MOD import *` is only allowed at module level.
            if !self.is_module_scope {
                self.free_temp(mod_reg);
                self.set_syntax_error("import * only allowed at module level");
                return;
            }
            // Star import: emit ImportStar which iterates the module's __all__
            // (or all non-underscore attrs when __all__ is absent) and stores
            // each name into the current scope.
            self.emit(Insn::ImportStar(mod_reg));
        } else {
            for (attr_name, alias) in names {
                let attr_idx = self.intern_name(attr_name);
                let val_reg = self.alloc_temp();
                self.emit(Insn::ImportFromAttr(val_reg, mod_reg, attr_idx));
                let bound = alias.as_deref().unwrap_or(attr_name);
                self.compile_store_name(bound, val_reg);
                self.free_temp(val_reg);
            }
        }
        self.free_temp(mod_reg);
    }

    // ── Def / Class ───────────────────────────────────────────────────────────

    /// Build the inner-function scope metadata, validate global/nonlocal/
    /// annotation rules, compile the body into a child compiler, and push the
    /// resulting `FnProto`.  Returns
    /// `(proto_idx, is_memo_pure, is_dce_pure, has_kwonly_params)`,
    /// or `None` when a (syntax/limit) error was recorded and the caller must
    /// bail out.
    #[allow(clippy::too_many_arguments)]
    fn build_def_proto(
        &mut self,
        name: &str,
        params: &[FunctionParam],
        body: &[Stmt],
        body_linenos: &[u32],
        def_lineno: u32,
        return_annotation: Option<&Expr>,
        is_async: bool,
    ) -> Option<(u16, bool, bool, bool)> {
        // Build inner function's scope metadata.
        let inner_global = crate::interpreter::collect_global_names(body);
        let inner_nonlocal = crate::interpreter::collect_nonlocal_names(body);

        // A parameter may not also be declared `global`/`nonlocal` in the body.
        // CPython 3.12 raises `SyntaxError: name 'x' is parameter and global`
        // (resp. `... and nonlocal`).  This conflict wins over the later
        // ordering / annotation / no-binding diagnostics, so check it first.
        for p in params {
            if inner_global.contains(&p.name) {
                self.set_syntax_error(&format!("name '{}' is parameter and global", p.name));
                return None;
            }
            if inner_nonlocal.contains(&p.name) {
                self.set_syntax_error(&format!("name '{}' is parameter and nonlocal", p.name));
                return None;
            }
        }

        let inner_local =
            crate::interpreter::collect_local_names(params, body, &inner_global, &inner_nonlocal);

        // Build a compact local_index for the inner function.
        // Parameters come first (preserving declaration order), then body locals.
        let mut inner_index: HashMap<String, Reg> = HashMap::new();
        let mut slot: Reg = 0;
        for param in params {
            if inner_local.contains(&param.name) {
                inner_index.insert(param.name.clone(), slot);
                slot += 1;
            }
        }
        for loc in &inner_local {
            if !inner_index.contains_key(loc) {
                inner_index.insert(loc.clone(), slot);
                slot += 1;
            }
        }
        let inner_index_rc: Rc<HashMap<String, Reg>> = Rc::new(inner_index);

        let def_bound = crate::interpreter::compute_def_bound_mask(params, &inner_index_rc);
        // Include `name` so self-recursive calls are treated as pure (fixpoint
        // assumption).  The memo-pure analysis trusts every memo-pure local
        // callee; the DCE-pure analysis trusts only DCE-pure local callees (a
        // function calling a merely-memo-pure callee can still raise, so it is
        // not itself DCE-pure — issue #2523).
        let mut pure_fns_with_self = self.pure_locals.clone();
        pure_fns_with_self.insert(name.to_string());
        let mut dce_pure_fns_with_self = self.dce_pure_locals.clone();
        dce_pure_fns_with_self.insert(name.to_string());
        // A coroutine function (`async def`, issue #1039) is never pure: calling
        // it must build a coroutine object (an observable side effect), so it
        // must not be inlined, memoized, or const-folded by the optimizer.
        //
        // Two purity flags drive two different optimizer decisions (issue #2523):
        //   * `is_memo_pure` — may a call's result be cached/reused?  Permissive;
        //     gates `CallMemo` emission, the VM result cache, and inlining.  Keeps
        //     `<`/`-` self-recursive functions (`fib`) memoized.
        //   * `is_dce_pure` — may a *dead-result* call be eliminated?  Conservative;
        //     gates dead-`CallMemo` removal.  Rejects bodies that can raise or
        //     dispatch a user dunder so the effect is never swallowed.
        // `is_dce_pure ⊆ is_memo_pure` by construction.
        let is_memo_pure = !is_async
            && crate::interpreter::is_memo_pure_body(body, &pure_fns_with_self, &inner_index_rc);
        let is_dce_pure = !is_async
            && crate::interpreter::is_dce_pure_body(body, &dce_pure_fns_with_self, &inner_index_rc);

        // Detect cell vars for the inner function.
        let inner_cell_vars = collect_cell_vars(body, &inner_index_rc);

        // Validate ordering: global/nonlocal declarations must appear before
        // any assignment or use of the same name in the function body.
        // CPython 3.12 raises SyntaxError for `def f(): x = 1; global x`.
        if let Some(msg) = crate::interpreter::check_global_nonlocal_order(body) {
            self.failed = true;
            self.is_syntax_error = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(msg);
            }
            return None;
        }

        // Validate annotation targets against global/nonlocal declarations.
        // CPython 3.12 raises SyntaxError for `def f(): global x; x: int` and
        // `def f(): nonlocal x; x: int` (issue #748 / companion to #770).
        let def_ann_targets = crate::interpreter::collect_annotation_target_names(body);
        for ann_name in &def_ann_targets {
            if inner_global.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be global", ann_name));
                return None;
            }
            if inner_nonlocal.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be nonlocal", ann_name));
                return None;
            }
        }

        let inner_global_rc = Rc::new(inner_global);
        let inner_nonlocal_rc = Rc::new(inner_nonlocal);

        // Compile-time validation: every name in `inner_nonlocal` must have a
        // binding in some enclosing *function* scope.  Module scope and class
        // scope do not count (`nonlocal` is only valid inside a function).
        //
        // `self.outer_locals` is the chain of enclosing function scope
        // `local_index` maps (outermost first).  If `self.is_function_scope`,
        // `self.local_index` is also an enclosing function scope.
        let mut sorted_nonlocals: Vec<&String> = inner_nonlocal_rc.iter().collect();
        sorted_nonlocals.sort();
        for nonlocal_name in sorted_nonlocals {
            let found = self
                .outer_locals
                .iter()
                .any(|m| m.contains_key(nonlocal_name))
                || (self.is_function_scope && self.local_index.contains_key(nonlocal_name));
            if !found {
                self.set_syntax_error(&format!(
                    "no binding for nonlocal '{}' found",
                    nonlocal_name
                ));
                return None;
            }
        }

        // Compile-time validation: an annotated name (`x: T` or `x: T = v`)
        // cannot also be declared `global` or `nonlocal` in the same function
        // scope.  CPython 3.12 raises `SyntaxError: annotated name 'x' can't
        // be global` / `can't be nonlocal`.
        let ann_targets = crate::interpreter::collect_annotation_target_names(body);
        let mut sorted_ann: Vec<&String> = ann_targets.iter().collect();
        sorted_ann.sort();
        for ann_name in sorted_ann {
            if inner_global_rc.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be global", ann_name));
                return None;
            }
            if inner_nonlocal_rc.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be nonlocal", ann_name));
                return None;
            }
        }

        let mut sub = Compiler::new(Rc::clone(&inner_index_rc), def_bound, inner_cell_vars);
        // Threaded source file (#2438): the nested function's code object shares
        // its enclosing scope's `co_filename` so an imported module's functions
        // report their own file in tracebacks.
        sub.filename = self.filename.clone();
        // Thread the enclosing function scope chain into the child compiler.
        // Since compile_def always produces a function scope, add self.local_index
        // (if self is a function scope) and mark the child as a function scope.
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
        // Names declared `nonlocal` in this body resolve to an enclosing cell;
        // record them so reads/writes emit LoadCell/StoreCell (issue #2339).
        sub.nonlocal_names = (*inner_nonlocal_rc).clone();
        sub.is_async_function = is_async;
        // An `async def` whose body contains a bare `yield` is an async
        // generator (#2280); `return <value>` inside it is a SyntaxError.
        // Detect it from the body AST here (CPython derives the analogous
        // `ste_generator && ste_coroutine` flag the same way).
        sub.is_async_generator_fn = is_async && stmts_contain_yield(body);
        // Propagate PEP 563 lazy-annotation flag to the inner compiler.
        sub.future_annotations = self.future_annotations;
        // A function compiled directly inside a class body is a class method and
        // gets access to zero-arg super().  Functions compiled inside other
        // functions (nested) do not — they get is_class_method = false (the
        // default from Compiler::new) because self.is_class_body is false there.
        sub.is_class_method = self.is_class_body;
        // Compute the qualname for this function and its `<locals>` prefix.
        // Classes defined inside this function inherit `"fn_name.<locals>"` as
        // their qualname prefix — matching CPython's `"fn_name.<locals>.ClassName"`.
        let fn_qualname = if self.qualname_prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", self.qualname_prefix, name)
        };
        sub.qualname_prefix = format!("{fn_qualname}.<locals>");
        let has_kwonly_params = params.iter().any(|p| p.is_keyword_only);
        if is_memo_pure && !has_kwonly_params {
            // Seed the inner compiler with the function's own name so that
            // direct self-recursive calls are compiled as CallMemo rather than
            // Call.  This lets the VM return from the fn_cache on repeated
            // invocations without re-entering call_function_expanded at all,
            // making recursive memoizable functions (e.g. fib) substantially
            // faster.  Memo-purity (not the stricter DCE-purity) is the right
            // gate here: a self-recursive `fib` uses `<`/`-` and so is *not*
            // DCE-pure, but its result is still cacheable (issue #2523).
            // Exclude kwonly-param functions: CallMemo keys by raw positional
            // registers and would bypass keyword-only enforcement on self-calls.
            sub.pure_locals.insert(name.to_string());
            // A self-recursive dead call inside a DCE-pure function body may be
            // eliminated only when the function itself is DCE-pure (#2523).
            if is_dce_pure {
                sub.dce_pure_locals.insert(name.to_string());
            }
        }
        // `co_firstlineno`: the `def`/`lambda` line, recorded on the body's
        // FnCode (issue #2185).
        sub.first_lineno = def_lineno;
        sub.compile_block_with_linenos(body, body_linenos);
        let inner_code = match sub.finish() {
            Ok(c) => c,
            Err(e) => {
                self.failed = true;
                if matches!(e, PyError::Named(ref cls, _) if cls.as_ref() == "SyntaxError") {
                    self.is_syntax_error = true;
                }
                if self.error_msg.is_none() {
                    self.error_msg = Some(match e {
                        PyError::Named(_, msg) | PyError::Runtime(msg) => msg,
                        other => other.to_string(),
                    });
                }
                return None;
            }
        };

        if self.fn_protos.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many functions in one scope (max 65535)".to_string());
            }
            return None;
        }
        let proto_idx = self.fn_protos.len() as u16;
        let local_names = Rc::new(inner_index_rc.keys().cloned().collect::<HashSet<_>>());
        // Collect annotation keys: annotated param names (in declaration order) then
        // "return" if there is a return annotation.  These are parallel to the
        // annotation register window emitted just before MakeFunction.
        let annotation_keys: SmallVec<[String; 4]> = params
            .iter()
            .filter(|p| p.annotation.is_some())
            .map(|p| p.name.clone())
            .chain(return_annotation.map(|_| "return".to_string()))
            .collect();
        // Extract docstring: if the first statement in the body is a bare
        // string literal, capture it as the function's __doc__ (CPython parity).
        let fn_docstring = match body {
            [Stmt::Expr(Expr::Str(s)), ..] => Some(s.clone()),
            _ => None,
        };
        let param_spec = Rc::new(FnParamSpec {
            names: params.iter().map(|p| p.name.clone()).collect(),
            has_default: params.iter().map(|p| p.default.is_some()).collect(),
            is_args: params.iter().map(|p| p.is_args).collect(),
            is_kwargs: params.iter().map(|p| p.is_kwargs).collect(),
            is_keyword_only: params.iter().map(|p| p.is_keyword_only).collect(),
            is_positional_only: params.iter().map(|p| p.is_positional_only).collect(),
        });
        let param_binds = Rc::new(crate::bytecode::compute_param_binds(
            &param_spec,
            &inner_index_rc,
            &inner_code.cell_vars,
        ));
        let self_bind =
            crate::bytecode::compute_self_bind(name, &inner_index_rc, &inner_code.cell_vars);
        self.fn_protos.push(FnProto {
            name: Rc::from(name),
            qualname: Rc::from(fn_qualname.as_str()),
            param_spec,
            code: Rc::new(inner_code),
            local_index: inner_index_rc,
            param_binds,
            self_bind,
            local_names,
            global_names: inner_global_rc,
            nonlocal_names: inner_nonlocal_rc,
            is_memo_pure,
            is_dce_pure,
            annotation_keys,
            docstring: fn_docstring,
            class_kwarg_names: SmallVec::new(),
        });

        Some((proto_idx, is_memo_pure, is_dce_pure, has_kwonly_params))
    }

    /// Compile a function's default-value expressions into a contiguous
    /// register window.  Returns `(base, count)`, or `None` on register
    /// overflow (error already recorded).
    fn emit_def_default_values(&mut self, params: &[FunctionParam]) -> Option<(Reg, u8)> {
        // Compile default values (right-to-left in declaration, left-to-right in slots).
        let defaults: Vec<usize> = params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.default.is_some())
            .map(|(i, _)| i)
            .collect();
        let defs_n = defaults.len() as u8;
        let defs_base = self.next_temp;
        if defs_n > 0 {
            // Reserve slots
            if self.next_temp.checked_add(Reg::from(defs_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many default-value registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(defs_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (slot_i, param_i) in (0u32..).zip(defaults.iter()) {
                let def_expr = params[*param_i].default.as_ref().unwrap();
                let saved = self.next_temp;
                let r = self.compile_expr(def_expr);
                if r != defs_base + slot_i {
                    self.emit(Insn::Move(defs_base + slot_i, r));
                }
                self.next_temp = saved;
            }
        }
        Some((defs_base, defs_n))
    }

    /// Compile a function's parameter/return annotation expressions into a
    /// contiguous register window (param annotations in declaration order, then
    /// the return annotation).  Returns `(base, count)`, or `None` on register
    /// overflow (error already recorded).
    fn emit_def_annotation_values(
        &mut self,
        params: &[FunctionParam],
        return_annotation: Option<&Expr>,
    ) -> Option<(Reg, u8)> {
        // Compile annotation expressions (evaluated in enclosing scope, like defaults).
        // Under PEP 563 (`from __future__ import annotations`), emit the annotation
        // source text as a string literal instead of evaluating the expression.
        // Order: annotated params in declaration order, then return annotation.
        let annotated_params: Vec<(usize, &Expr)> = params
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.annotation.as_ref().map(|a| (i, a)))
            .collect();
        let annots_n = annotated_params.len() as u8 + return_annotation.is_some() as u8;
        let annots_base = self.next_temp;
        if annots_n > 0 {
            if self.next_temp.checked_add(Reg::from(annots_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many annotation registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(annots_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (slot_i, (_, annot_expr)) in (0u32..).zip(annotated_params.iter()) {
                let saved = self.next_temp;
                let r = if self.future_annotations {
                    self.compile_literal(Value::string(stringify_annotation(annot_expr)))
                } else {
                    self.compile_expr(annot_expr)
                };
                if r != annots_base + slot_i {
                    self.emit(Insn::Move(annots_base + slot_i, r));
                }
                self.next_temp = saved;
            }
            if let Some(ret_annot) = return_annotation {
                let slot_i = annots_n as u32 - 1;
                let saved = self.next_temp;
                let r = if self.future_annotations {
                    self.compile_literal(Value::string(stringify_annotation(ret_annot)))
                } else {
                    self.compile_expr(ret_annot)
                };
                if r != annots_base + slot_i {
                    self.emit(Insn::Move(annots_base + slot_i, r));
                }
                self.next_temp = saved;
            }
        }
        Some((annots_base, annots_n))
    }

    /// Apply a chain of decorators to the value in `dst`: evaluate each
    /// decorator expression top-to-bottom, then apply innermost-first
    /// (`fn = d1(d2(d3(fn)))`).  Returns the register holding the final
    /// decorated value (`dst` when there are no decorators), or `None` on
    /// register overflow (error already recorded).  Shared by `compile_def`
    /// and `compile_class`.
    fn emit_decorator_application(&mut self, decorators: &[Expr], dst: Reg) -> Option<Reg> {
        // Evaluate decorator expressions top-to-bottom, then apply bottom-to-top.
        // CPython evaluates decorators in declaration order (top first) but applies
        // them innermost-first (bottom first): fn = d1(d2(d3(fn))).
        let mut val_reg = dst;
        if !decorators.is_empty() {
            let n = decorators.len() as u32;
            let deco_base = self.next_temp;
            // Need n slots for the callables plus 1 extra arg slot for the first call.
            if deco_base.checked_add(n + 1).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("too many registers for decorator application".to_string());
                }
                return None;
            }
            // Reserve n + 1 registers (n callables + 1 arg slot for the first application).
            self.next_temp = deco_base + n + 1;
            if deco_base + n > self.max_reg {
                self.max_reg = deco_base + n;
            }
            // Evaluate each decorator expression top-to-bottom into consecutive registers.
            for (i, deco_expr) in decorators.iter().enumerate() {
                let saved = self.next_temp;
                self.compile_expr_into(deco_expr, deco_base + i as u32);
                self.next_temp = saved;
            }
            // Apply decorators bottom-to-top (innermost first).
            for i in (0..n).rev() {
                let frame = deco_base + i;
                // frame+1 is the argument slot; for i == n-1 this is deco_base+n
                // (the extra slot reserved above); for smaller i it reuses the
                // register freed by the previous application result.
                self.emit(Insn::Move(frame + 1, val_reg));
                self.emit(Insn::Call(frame, 1));
                val_reg = frame;
            }
            self.next_temp = deco_base + 1;
        }
        Some(val_reg)
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_def(
        &mut self,
        name: &str,
        params: &[FunctionParam],
        body: &[Stmt],
        body_linenos: &[u32],
        def_lineno: u32,
        decorators: &[Expr],
        return_annotation: Option<&Expr>,
        is_async: bool,
        type_params: &[TypeParam],
    ) {
        let (proto_idx, is_memo_pure, is_dce_pure, has_kwonly_params) = match self.build_def_proto(
            name,
            params,
            body,
            body_linenos,
            def_lineno,
            return_annotation,
            is_async,
        ) {
            Some(v) => v,
            None => return,
        };

        // PEP 695 default values are evaluated in the *enclosing* scope, not the
        // type-parameter scope: a default that references a type parameter
        // (`def g[T](x=T)`) sees the enclosing `T` (or raises NameError if none
        // exists), matching CPython.  Evaluate defaults *before* pushing the
        // type-param environment so they resolve against the enclosing scope.
        let (defs_base, defs_n) = match self.emit_def_default_values(params) {
            Some(v) => v,
            None => return,
        };

        // PEP 695: push a dedicated type-parameter environment, then bind the
        // type parameters (as TypeVar objects) into it *before* the annotations
        // are evaluated, so a parameter or return annotation that references `T`
        // (e.g. `def f[T](x: T) -> T`) resolves.  Binding them in a child env
        // (rather than the enclosing namespace) keeps the parameter names from
        // leaking after the def while the generic function — which captures this
        // env via `MakeFunction` — can still resolve them lazily in its body.
        // The returned register block holds the same TypeVar objects reused for
        // `__type_params__` below to preserve object identity.  The block sits
        // below `dst`, so the `next_temp = dst + 1` watermark reset after
        // `MakeFunction` keeps it live until the tuple is built.
        if !type_params.is_empty() {
            self.emit(Insn::PushTypeParamEnv);
        }
        let (tp_base, tp_n) = self.emit_bind_type_params(type_params);

        let (annots_base, annots_n) =
            match self.emit_def_annotation_values(params, return_annotation) {
                Some(v) => v,
                None => return,
            };

        let dst = self.alloc_temp();
        self.emit(Insn::MakeFunction(
            dst,
            proto_idx,
            defs_base,
            defs_n,
            annots_base,
            annots_n,
        ));
        if defs_n > 0 || annots_n > 0 {
            // Free the temp registers used by defaults and annotations; keep
            // only the function value register (dst) alive from this point.
            // `dst` was allocated after all defaults/annotations, so `dst + 1`
            // is the correct watermark: it preserves the function and releases
            // every slot below it (defaults, annotations).
            //
            // The previous formula (`defs_base + 1` or `annots_base + 1`)
            // was wrong when exactly one default or annotation was present:
            // defs_base + 1 == dst, so the subsequent decorator-base
            // allocation used the same register as dst, overwriting the
            // freshly created function with the decorator value (issue #1362).
            self.next_temp = dst + 1;
        }

        // PEP 695: if this is a generic function, build the __type_params__ tuple
        // and store it on the function object before decorators are applied.
        // CPython sets __type_params__ on the raw function, before wrapping it
        // with decorators (verified: the decorator receives a function that already
        // has __type_params__).  Reuse the TypeVar registers bound above so the
        // objects in __type_params__ are identical to those seen in annotations.
        if tp_n > 0 {
            self.emit_type_params_attr_from_regs(dst, tp_base, tp_n);
        }

        // PEP 695: pop the type-parameter environment before decorators run and
        // before the def name is bound — decorators and the binding belong to the
        // enclosing scope (a decorator referencing `T` must see the enclosing
        // `T`, not the type parameter).
        if !type_params.is_empty() {
            self.emit(Insn::PopTypeParamEnv);
        }

        let val_reg = match self.emit_decorator_application(decorators, dst) {
            Some(r) => r,
            None => return,
        };

        self.compile_store_name(name, val_reg);
        if let Some(reg) = self.local_reg(name) {
            self.mark_def(reg);
        }
        // Exclude kwonly-param functions from CallMemo optimisation.
        // CallMemo keys by raw positional arg values; a kwarg-based call stores
        // a cache entry that an invalid positional-only call could match, bypassing
        // keyword-only enforcement in call_user_function_expanded.
        // Memo-purity (not the stricter DCE-purity) gates `CallMemo` emission so
        // the VM result cache stays active; dead-`CallMemo` DCE is separately
        // gated on the target proto's `is_dce_pure` in the optimizer (#2523).
        // Record DCE-pure names too so that a *later* sibling function calling
        // this one can itself qualify as DCE-pure (transitive callee allow-list).
        if decorators.is_empty() && !has_kwonly_params {
            if is_memo_pure {
                self.pure_locals.insert(name.to_string());
            }
            if is_dce_pure {
                self.dce_pure_locals.insert(name.to_string());
            }
        }
        self.free_temp(dst);
    }

    /// Validate the class body's global/nonlocal/annotation rules, build its
    /// register index, compile the body as a zero-param function in a child
    /// compiler, and push the resulting class `FnProto`.  Returns the proto
    /// index, or `None` when an error was recorded and the caller must bail.
    fn build_class_proto(
        &mut self,
        name: &str,
        keywords: &[(String, Expr)],
        body: &[Stmt],
    ) -> Option<u16> {
        // Class body: zero-param function that returns its locals as class dict.
        // Collect names explicitly declared `global` in the class body so they
        // are excluded from `body_local` and routed to `Insn::StoreGlobal`
        // instead of `Insn::RecordClassStore`.  Without this, `global x; x = 42`
        // inside a class body silently stored into the class attribute dict
        // rather than the module-level global (issue #618).
        let body_global = Rc::new(crate::interpreter::collect_global_names(body));
        // Collect `nonlocal` declarations in the class body (issue #708 / #735).
        // These names must not get a class-body register slot — they are
        // stored/loaded via the enclosing function's env, not the class namespace.
        let body_nonlocal = crate::interpreter::collect_nonlocal_names(body);
        // Validate: every `nonlocal x` in the class body must have a binding in
        // some enclosing *function* scope.  Module scope and class scope do not
        // count — `nonlocal` requires an enclosing function binding (CPython 3.12
        // raises SyntaxError: no binding for nonlocal 'x' found).
        {
            let mut sorted: Vec<&String> = body_nonlocal.iter().collect();
            sorted.sort();
            for nonlocal_name in sorted {
                let found = self
                    .outer_locals
                    .iter()
                    .any(|m| m.contains_key(nonlocal_name))
                    || (self.is_function_scope && self.local_index.contains_key(nonlocal_name));
                if !found {
                    self.set_syntax_error(&format!(
                        "no binding for nonlocal '{}' found",
                        nonlocal_name
                    ));
                    return None;
                }
            }
        }
        let body_nonlocal_rc: Rc<HashSet<String>> = Rc::new(body_nonlocal.clone());

        // Validate annotation targets against global/nonlocal declarations.
        // CPython 3.12 raises SyntaxError for `class C: global x; x: int` and
        // `class C: nonlocal x; x: int`.  Declaration order does not matter —
        // the check is whole-scope (issue #770).
        let ann_targets = crate::interpreter::collect_annotation_target_names(body);
        for ann_name in &ann_targets {
            if body_global.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be global", ann_name));
                return None;
            }
            if body_nonlocal.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be nonlocal", ann_name));
                return None;
            }
        }

        // Validate ordering: global/nonlocal declarations must appear before any
        // assignment or use of the same name in the class body.
        if let Some(msg) = crate::interpreter::check_global_nonlocal_order(body) {
            self.failed = true;
            self.is_syntax_error = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(msg);
            }
            return None;
        }

        let body_local =
            crate::interpreter::collect_local_names(&[], body, &body_global, &body_nonlocal_rc);
        // Allocate a register slot for every potential class-body local.
        // Slot order is **not** used to encode class-namespace insertion
        // order any more — the order CPython exposes via `vars(C)` is the
        // order stores actually executed at runtime, not source-walk order.
        // Each store now emits `Insn::RecordClassStore(slot)` and the VM
        // builds the attrs dict from that runtime trace inside `MakeClass`.
        // We still walk the body textually here so register numbers follow
        // declaration order for names that only appear inside control-flow
        // blocks (where the IndexSet insertion order and textual order agree,
        // but names inside nested blocks need the explicit walk to be seen
        // before the catch-all pass at the end).
        //
        // Issue #546: CPython pre-injects `__qualname__` and `__module__`
        // into the class namespace before the body runs.  Give them fixed
        // register slots (0 and 1) so the VM can pre-populate them and so
        // `locals()` inside the class body always includes them.  If the
        // user explicitly assigns either name in the body, `collect_local_names`
        // will have included it in `body_local` already; we skip it here to
        // avoid a duplicate slot.
        let mut ordered: Vec<String> = Vec::with_capacity(body_local.len() + 2);
        let mut seen: HashSet<String> = HashSet::new();
        // CPython injects __module__ first, __qualname__ second.
        for pre_name in ["__module__", "__qualname__"] {
            if !body_local.contains(pre_name) {
                ordered.push(pre_name.to_string());
                seen.insert(pre_name.to_string());
            }
        }
        // Issue #712: if the class body has any annotations, pre-allocate
        // a register slot for __annotations__ so compile_ann_assign can use a
        // fastlocal (RecordClassStore) rather than a LoadGlobal.
        if class_body_has_annotations(body) && !body_local.contains("__annotations__") {
            ordered.push("__annotations__".to_string());
            seen.insert("__annotations__".to_string());
        }
        collect_class_body_names_textual(body, &mut ordered, &mut seen, &body_local);
        for name in body_local.iter() {
            if seen.insert(name.clone()) {
                ordered.push(name.clone());
            }
        }
        let mut body_index: HashMap<String, Reg> = HashMap::new();
        for (i, loc) in (0u32..).zip(ordered.iter()) {
            body_index.insert(loc.clone(), i);
        }
        let body_index_rc: Rc<HashMap<String, Reg>> = Rc::new(body_index);
        // Use the class-body variant: a method's `global x` must not promote
        // the class-body name `x` to a cell var (issue #624).
        let cell_vars = collect_cell_vars_for_class_body(body, &body_index_rc);

        // Compute the full qualname for this class.
        // For `class Outer: class Inner`, `self.qualname_prefix` is `"Outer"` and
        // `class_qualname` becomes `"Outer.Inner"`.
        // The child compiler's `qualname_prefix` is set to `class_qualname` so
        // that further nested classes or functions inside it get the right prefix.
        let class_qualname = if self.qualname_prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", self.qualname_prefix, name)
        };

        let mut sub = Compiler::new(Rc::clone(&body_index_rc), 0, cell_vars);
        sub.is_class_body = true;
        // Threaded source file (#2438): methods defined in this class body inherit
        // the enclosing scope's `co_filename`.
        sub.filename = self.filename.clone();
        sub.qualname_prefix = class_qualname.clone();
        // Thread the enclosing function scope chain into the class body compiler.
        // Class scope is transparent to `nonlocal` (not a function scope), so we
        // pass through outer_locals without adding body_index_rc, and leave
        // is_function_scope = false.
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        // Propagate PEP 563 lazy-annotation flag to the class body compiler.
        sub.future_annotations = self.future_annotations;
        sub.compile_block(body);
        // Add implicit ReturnNone at end of class body
        sub.emit(Insn::ReturnNone);
        let body_code = match sub.finish() {
            Ok(c) => c,
            Err(e) => {
                self.failed = true;
                if matches!(e, PyError::Named(ref cls, _) if cls.as_ref() == "SyntaxError") {
                    self.is_syntax_error = true;
                }
                if self.error_msg.is_none() {
                    self.error_msg = Some(match e {
                        PyError::Named(_, msg) | PyError::Runtime(msg) => msg,
                        other => other.to_string(),
                    });
                }
                return None;
            }
        };
        if self.fn_protos.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many classes/functions in one scope (max 65535)".to_string());
            }
            return None;
        }
        let proto_idx = self.fn_protos.len() as u16;
        let local_names = Rc::new(body_index_rc.keys().cloned().collect::<HashSet<_>>());
        // Extract docstring: if the first statement in the class body is a bare
        // string literal, capture it as the class's __doc__ (CPython parity).
        let class_docstring = match body {
            [Stmt::Expr(Expr::Str(s)), ..] => Some(s.clone()),
            _ => None,
        };
        self.fn_protos.push(FnProto {
            name: Rc::from(name),
            qualname: Rc::from(class_qualname.as_str()),
            param_spec: Rc::new(FnParamSpec {
                names: SmallVec::new(),
                has_default: SmallVec::new(),
                is_args: SmallVec::new(),
                is_kwargs: SmallVec::new(),
                is_keyword_only: SmallVec::new(),
                is_positional_only: SmallVec::new(),
            }),
            code: Rc::new(body_code),
            local_index: body_index_rc,
            param_binds: Rc::new(Vec::new()),
            self_bind: None,
            local_names,
            global_names: body_global,
            nonlocal_names: body_nonlocal_rc,
            is_memo_pure: false,
            is_dce_pure: false,
            annotation_keys: SmallVec::new(),
            docstring: class_docstring,
            class_kwarg_names: keywords.iter().map(|(k, _)| k.clone()).collect(),
        });

        Some(proto_idx)
    }

    /// Compile the base-class expressions and PEP 487 keyword-argument values
    /// into two contiguous register windows.  Returns
    /// `(bases_base, bases_n, kwarg_base, kwarg_n)`, or `None` on register
    /// overflow (error already recorded).
    fn emit_class_bases_and_keywords(
        &mut self,
        bases: &[Expr],
        keywords: &[(String, Expr)],
    ) -> Option<(Reg, u8, Reg, u8)> {
        // Compile base class expressions.
        let bases_n = bases.len() as u8;
        let bases_base = self.next_temp;
        if bases_n > 0 {
            if self.next_temp.checked_add(Reg::from(bases_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many base class registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(bases_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (i, base_expr) in (0u32..).zip(bases.iter()) {
                let saved = self.next_temp;
                let r = self.compile_expr(base_expr);
                if r != bases_base + i {
                    self.emit(Insn::Move(bases_base + i, r));
                }
                self.next_temp = saved;
            }
        }

        // Compile PEP 487 keyword arg values into consecutive registers.
        // These are forwarded to __init_subclass__; names are stored in FnProto.
        let kwarg_n = keywords.len() as u8;
        let kwarg_base = self.next_temp;
        if kwarg_n > 0 {
            if self.next_temp.checked_add(Reg::from(kwarg_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many class keyword registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(kwarg_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (i, (_, val_expr)) in (0u32..).zip(keywords.iter()) {
                let saved = self.next_temp;
                let r = self.compile_expr(val_expr);
                if r != kwarg_base + i {
                    self.emit(Insn::Move(kwarg_base + i, r));
                }
                self.next_temp = saved;
            }
        }

        Some((bases_base, bases_n, kwarg_base, kwarg_n))
    }

    // AST-node compile entry: each arg is a distinct syntactic child of the
    // `class` statement; bundling them into a struct only relocates the field list.
    #[allow(clippy::too_many_arguments)]
    fn compile_class(
        &mut self,
        name: &str,
        bases: &[Expr],
        metaclass: Option<&Expr>,
        keywords: &[(String, Expr)],
        body: &[Stmt],
        decorators: &[Expr],
        type_params: &[TypeParam],
    ) {
        let proto_idx = match self.build_class_proto(name, keywords, body) {
            Some(idx) => idx,
            None => return,
        };

        // PEP 695: push a dedicated type-parameter environment and bind the type
        // parameters into it before the base-class expressions are evaluated (so
        // `class C[T](Base[T])` resolves `T`) and before the class body runs (so
        // a method annotation `def m(self, x: T)` resolves `T` at class-creation
        // time).  Binding them in a child env keeps the names from leaking into
        // the enclosing scope after the class statement, while the class object —
        // which captures this env — can still resolve them.  The block sits below
        // `dst`; the watermark resets below keep it live until
        // `finish_class_definition` builds the tuple and pops the env.
        if !type_params.is_empty() {
            self.emit(Insn::PushTypeParamEnv);
        }
        let (tp_base, tp_n) = self.emit_bind_type_params(type_params);

        let (bases_base, bases_n, kwarg_base, kwarg_n) =
            match self.emit_class_bases_and_keywords(bases, keywords) {
                Some(v) => v,
                None => return,
            };

        let name_idx = self.intern_name(name);

        // With an explicit `metaclass=`, route the whole creation through
        // `MakeClassMeta`: it calls `metaclass.__prepare__`, runs the body into
        // that namespace, and calls `metaclass(name, bases, ns, **kw)` so the
        // class-creation hooks fire once inside the metaclass (issues
        // #2128/#2130).  The metaclass value must live in a register kept alive
        // across the instruction; allocate it after the bases/kwargs region.
        if let Some(meta_expr) = metaclass {
            let meta_reg = self.alloc_temp();
            let saved = self.next_temp;
            self.compile_expr_into(meta_expr, meta_reg);
            self.next_temp = saved;
            let dst = self.alloc_temp();
            self.emit(Insn::MakeClassMeta(
                dst, proto_idx, bases_base, bases_n, name_idx, kwarg_base, kwarg_n, meta_reg,
            ));
            // bases/kwargs/meta_reg are dead after the instruction; keep only
            // `dst` (the class object) live for decorators / type-params / store.
            self.next_temp = dst + 1;
            return self.finish_class_definition(name, dst, decorators, tp_base, tp_n);
        }

        let dst = self.alloc_temp();
        self.emit(Insn::MakeClass(
            dst, proto_idx, bases_base, bases_n, name_idx, kwarg_base, kwarg_n,
        ));
        if bases_n > 0 {
            // The base registers are dead after MakeClass, but `dst` (the freshly
            // built class object) must stay live for the decorator / type-params
            // / store steps below.  `dst` was allocated immediately after the
            // bases, so `dst == bases_base + bases_n`; the correct watermark is
            // therefore `dst + 1`, which preserves the class object and releases
            // every slot above it.
            //
            // The previous formula (`bases_base + 1`) overwrote `dst` whenever a
            // base was present: with one base, `bases_base + 1 == dst`, so the
            // subsequent decorator base allocated the same register as `dst` and
            // the decorator value clobbered the class object (issue #1889). The
            // class decorator then received the decorator function itself.
            self.next_temp = dst + 1;
        }

        self.finish_class_definition(name, dst, decorators, tp_base, tp_n);
    }

    /// Shared tail of class compilation for both the plain `MakeClass` and the
    /// metaclass `MakeClassMeta` paths: apply PEP 695 `__type_params__`, run the
    /// class decorators, store the result, and free the class register.  On
    /// entry `dst` holds the class object and `next_temp == dst + 1`.
    /// `tp_base`/`tp_n` describe the contiguous block of bound TypeVar registers
    /// produced by `emit_bind_type_params` (`tp_n == 0` for a non-generic class).
    fn finish_class_definition(
        &mut self,
        name: &str,
        dst: Reg,
        decorators: &[Expr],
        tp_base: Reg,
        tp_n: Reg,
    ) {
        // PEP 695: if this is a generic class, build the __type_params__ tuple
        // and store it on the class object before decorators are applied.  The
        // tuple reuses the TypeVar registers bound before the class body ran, so
        // the objects in __type_params__ are identical to those the body saw.
        if tp_n > 0 {
            if self.next_temp <= dst {
                self.next_temp = dst + 1;
            }
            self.emit_type_params_attr_from_regs(dst, tp_base, tp_n);
            // PEP 695: pop the type-parameter environment now that the class
            // object exists and its `__type_params__` is set.  Decorators and the
            // class-name binding belong to the enclosing scope.
            self.emit(Insn::PopTypeParamEnv);
        }

        // Evaluate decorator expressions top-to-bottom, then apply bottom-to-top.
        let mut val_reg = dst;
        if !decorators.is_empty() {
            let n = decorators.len() as u32;
            let deco_base = self.next_temp;
            if deco_base.checked_add(n + 1).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("too many registers for decorator application".to_string());
                }
                return;
            }
            self.next_temp = deco_base + n + 1;
            if deco_base + n > self.max_reg {
                self.max_reg = deco_base + n;
            }
            for (i, deco_expr) in decorators.iter().enumerate() {
                let saved = self.next_temp;
                self.compile_expr_into(deco_expr, deco_base + i as u32);
                self.next_temp = saved;
            }
            for i in (0..n).rev() {
                let frame = deco_base + i;
                self.emit(Insn::Move(frame + 1, val_reg));
                self.emit(Insn::Call(frame, 1));
                val_reg = frame;
            }
            self.next_temp = deco_base + 1;
        }

        self.compile_store_name(name, val_reg);
        if let Some(reg) = self.local_reg(name) {
            self.mark_def(reg);
        }
        self.free_temp(dst);
    }

    // ── Try / With ────────────────────────────────────────────────────────────

    // AST-node compile entry: body/handlers/else/finally plus their parallel
    // lineno tables; each is a distinct syntactic child of the `try` statement.
    #[allow(clippy::too_many_arguments)]
    fn compile_try(
        &mut self,
        body: &[Stmt],
        handlers: &[crate::ast::ExceptHandler],
        else_branch: Option<&[Stmt]>,
        finally_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
        finally_linenos: &[u32],
    ) {
        // PEP 654: if any handler is `except*`, route to the star compilation path.
        // (Mixing `except` and `except*` is a SyntaxError in CPython, so we treat
        // all-star or all-non-star as the two cases.)
        let has_star_handlers = handlers.iter().any(|h| h.is_star);
        if has_star_handlers {
            self.compile_try_star(
                body,
                handlers,
                else_branch,
                finally_branch,
                body_linenos,
                else_linenos,
                finally_linenos,
            );
            return;
        }

        let has_handlers = !handlers.is_empty();

        // Strategy:
        // 1. If we have finally: wrap everything in an outer SetupExcept for finally.
        // 2. If we have handlers: inner SetupExcept for the except clause.
        // Normal-exit path: run else (if any), then finally (if any).
        // Exception path: dispatch handlers; on match run handler then finally;
        //                 on no-match re-raise (outer finally catches it).

        // Outer finally handler patch (only if finally_branch is Some)
        let outer_finally_patch: Option<usize> = if finally_branch.is_some() {
            Some(self.emit(Insn::SetupExcept(0)))
        } else {
            None
        };

        // Inner handler patch (only if has_handlers)
        let inner_handler_patch: Option<usize> = if has_handlers {
            Some(self.emit(Insn::SetupExcept(0)))
        } else {
            None
        };

        // Register cleanup entries so that early exits (break/continue/return)
        // from the try body emit the correct PopExcept + finally sequence.
        // The outermost handler is pushed first (will be cleaned up last).
        if outer_finally_patch.is_some() {
            self.except_cleanups.push(EarlyExitCleanup::TryBody {
                finally_stmts: Some(finally_branch.unwrap().to_vec()),
            });
        }
        if inner_handler_patch.is_some() {
            // Inner except handler: no finally at this level (finally belongs to outer).
            self.except_cleanups.push(EarlyExitCleanup::TryBody {
                finally_stmts: None,
            });
        }

        // Compile try body
        self.compile_block_with_linenos(body, body_linenos);
        // Save the lineno after the try body so that the "no handler matched"
        // RaiseReRaise instruction is attributed to the try-body, not to some
        // handler body statement that happened to run last during dispatch.
        let try_body_lineno = self.current_lineno;

        // Pop the try-body cleanup entries before emitting normal-exit cleanup.
        if inner_handler_patch.is_some() {
            self.except_cleanups.pop();
        }
        if outer_finally_patch.is_some() {
            self.except_cleanups.pop();
        }

        if self.failed {
            return;
        }

        // Normal exit from try body:
        if inner_handler_patch.is_some() {
            self.emit(Insn::PopExcept);
        }
        // Compile else branch (normal path only)
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        // Normal finally exit
        if outer_finally_patch.is_some() {
            self.emit(Insn::PopExcept);
            let finally_stmts = finally_branch.unwrap();
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
        }
        // Jump over handlers + exception path
        let end_patch = self.emit(Insn::Jump(0));

        // ── Exception path ──
        if let Some(inner_idx) = inner_handler_patch {
            self.patch_jump(inner_idx);
        }

        let mut handler_end_patches: Vec<usize> = Vec::new();

        if has_handlers {
            let exc_tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(exc_tmp));

            for handler in handlers {
                let skip_patch: Option<usize> = if let Some(kind_expr) = &handler.kind {
                    let type_reg = self.compile_expr(kind_expr);
                    let p = self.emit(Insn::MatchExcept(type_reg, 0));
                    self.free_temp(type_reg);
                    Some(p)
                } else {
                    None
                };

                // Bind exception variable if `as VAR`
                if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        self.emit(Insn::Move(reg, exc_tmp));
                        self.mark_def(reg);
                        // Issue #820: sync into module_globals_dict at module scope.
                        if self.is_module_scope {
                            let name_idx = self.intern_name(var_name);
                            self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        }
                    } else {
                        let name_idx = self.intern_name(var_name);
                        self.emit(Insn::StoreGlobal(name_idx, exc_tmp));
                    }
                }

                // Pop outer finally handler before running handler body
                // (so that exceptions in the handler don't double-run finally)
                if outer_finally_patch.is_some() {
                    self.emit(Insn::PopExcept);
                }

                // Register an except-body cleanup so that early exits from the
                // handler body (break/continue/return) emit the PEP 3110 as-var
                // deletion, EndExcept, and the inlined finally block before jumping.
                let as_var_delete = if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        Some(ExceptAsVarDel::Local(reg))
                    } else {
                        let name_idx = self.intern_name(var_name);
                        Some(ExceptAsVarDel::Name(name_idx))
                    }
                } else {
                    None
                };
                self.except_cleanups.push(EarlyExitCleanup::ExceptBody {
                    finally_stmts: finally_branch.map(|s| s.to_vec()),
                    as_var_delete,
                });

                self.compile_block_with_linenos(&handler.body, &handler.body_linenos);

                // Remove the except-body cleanup before emitting normal handler exit.
                self.except_cleanups.pop();

                if self.failed {
                    return;
                }
                // PEP 3110: delete the `as VAR` binding when the handler exits
                // (breaks reference cycles and matches CPython behaviour).
                // Use u16::MAX as the name_idx sentinel: the variable is
                // always bound at this point (the except clause only runs
                // when the exception matched), so no NameError check needed.
                if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        self.emit(Insn::DeleteLocal(reg, u16::MAX));
                        // Clear the def_set bit so that subsequent reads of this
                        // variable emit CheckLocal and raise UnboundLocalError
                        // (not NameError) — matching CPython's DELETE_FAST semantics
                        // after `except E as var:` cleanup (issue #1277).
                        if (reg as usize) < 64 {
                            self.def_set &= !(1u64 << reg);
                        }
                    } else {
                        let name_idx = self.intern_name(var_name);
                        self.emit(Insn::DeleteName(name_idx));
                    }
                }
                self.emit(Insn::EndExcept);

                // Run finally (inline) after successful handler
                if let Some(finally_stmts) = finally_branch {
                    self.compile_block_with_linenos(finally_stmts, finally_linenos);
                    if self.failed {
                        return;
                    }
                }

                let jmp = self.emit(Insn::Jump(0));
                handler_end_patches.push(jmp);

                if let Some(p) = skip_patch {
                    self.patch_jump(p);
                }
            }

            // No handler matched: re-raise (outer finally will catch it if present).
            // Restore the try-body lineno so the re-raise is attributed to the
            // failing statement in the try block, not to handler body code.
            self.set_lineno(try_body_lineno);
            self.free_temp(exc_tmp);
            self.emit(Insn::RaiseReRaise);
        }

        // ── Outer finally handler (exception path) ──
        if let Some(outer_idx) = outer_finally_patch {
            if !has_handlers {
                // No handlers: patch the inner SetupExcept → this finally handler
                self.patch_jump(outer_idx);
            }
            // If has_handlers, outer_finally_patch was patched when? Actually not yet.
            // For try/except/finally: the outer SetupExcept should catch exceptions
            // that escape the handlers (or re-raised). We patch it here.
            if has_handlers {
                self.patch_jump(outer_idx);
            }
            let finally_stmts = finally_branch.unwrap();
            // Load exception and run finally then re-raise
            let exc_tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(exc_tmp));
            self.free_temp(exc_tmp);
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
            // Restore the try-body lineno so the re-raise is attributed to the
            // failing statement in the try block, not to the finally body.
            self.set_lineno(try_body_lineno);
            self.emit(Insn::RaiseReRaise);
        }

        // Patch all successful handler jumps to here (after everything)
        self.patch_jump(end_patch);
        for idx in handler_end_patches {
            self.patch_jump(idx);
        }
    }

    /// PEP 654 `except*` compilation.
    ///
    /// All handlers are tried sequentially against the same exception group.
    /// Each matching handler receives a sub-group of the matched exceptions;
    /// the group register is narrowed after each match so subsequent handlers
    /// only see the remaining (unhandled) exceptions.
    ///
    /// After all handlers, if any exceptions remain un-handled, they are
    /// re-raised as a new group.
    // AST-node compile entry: same syntactic-child arg shape as `compile_try`
    // (body/handlers/else/finally + their lineno tables) for the `except*` form.
    #[allow(clippy::too_many_arguments)]
    fn compile_try_star(
        &mut self,
        body: &[Stmt],
        handlers: &[crate::ast::ExceptHandler],
        else_branch: Option<&[Stmt]>,
        finally_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
        finally_linenos: &[u32],
    ) {
        // Outer finally handler patch (only if finally_branch is Some)
        let outer_finally_patch: Option<usize> = if finally_branch.is_some() {
            Some(self.emit(Insn::SetupExcept(0)))
        } else {
            None
        };

        // Inner handler patch for except* block
        let inner_handler_patch = self.emit(Insn::SetupExcept(0));

        // Register cleanup entries for early exits from the try body.
        if outer_finally_patch.is_some() {
            self.except_cleanups.push(EarlyExitCleanup::TryBody {
                finally_stmts: Some(finally_branch.unwrap().to_vec()),
            });
        }
        self.except_cleanups.push(EarlyExitCleanup::TryBody {
            finally_stmts: None,
        });

        self.compile_block_with_linenos(body, body_linenos);
        // Save the lineno after the try body so that the "no handler matched"
        // RaiseValue instruction is attributed to the try-body, not to some
        // handler body statement that happened to run last during dispatch.
        let try_body_lineno_star = self.current_lineno;

        self.except_cleanups.pop();
        if outer_finally_patch.is_some() {
            self.except_cleanups.pop();
        }

        if self.failed {
            return;
        }

        // Normal exit from try body
        self.emit(Insn::PopExcept);
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        if outer_finally_patch.is_some() {
            self.emit(Insn::PopExcept);
            let finally_stmts = finally_branch.unwrap();
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
        }
        let end_patch = self.emit(Insn::Jump(0));

        // ── Exception path ──
        self.patch_jump(inner_handler_patch);

        // Load the active exception into a group register.
        // This register will be narrowed by each MatchExceptStar.
        let group_reg = self.alloc_temp();
        self.emit(Insn::LoadExc(group_reg));

        for handler in handlers {
            if let Some(kind_expr) = &handler.kind {
                let type_reg = self.compile_expr(kind_expr);
                let subgroup_reg = self.alloc_temp();
                let skip_patch =
                    self.emit(Insn::MatchExceptStar(type_reg, group_reg, subgroup_reg, 0));
                self.free_temp(type_reg);

                // Bind the `as VAR` variable to the sub-group.
                let var_bind_cleanup = if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        self.emit(Insn::Move(reg, subgroup_reg));
                        self.mark_def(reg);
                        if self.is_module_scope {
                            let name_idx = self.intern_name(var_name);
                            self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        }
                        Some(ExceptAsVarDel::Local(reg))
                    } else {
                        let name_idx = self.intern_name(var_name);
                        self.emit(Insn::StoreGlobal(name_idx, subgroup_reg));
                        Some(ExceptAsVarDel::Name(name_idx))
                    }
                } else {
                    None
                };

                // Pop outer finally handler before running handler body
                if outer_finally_patch.is_some() {
                    self.emit(Insn::PopExcept);
                }

                // Register except-body cleanup for early exits.
                self.except_cleanups.push(EarlyExitCleanup::ExceptBody {
                    finally_stmts: finally_branch.map(|s| s.to_vec()),
                    as_var_delete: var_bind_cleanup.clone(),
                });

                self.compile_block_with_linenos(&handler.body, &handler.body_linenos);

                self.except_cleanups.pop();

                if self.failed {
                    self.free_temp(subgroup_reg);
                    return;
                }

                // PEP 3110-style cleanup: delete the `as VAR` binding.
                if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        self.emit(Insn::DeleteLocal(reg, u16::MAX));
                        if (reg as usize) < 64 {
                            self.def_set &= !(1u64 << reg);
                        }
                    } else {
                        let name_idx = self.intern_name(var_name);
                        self.emit(Insn::DeleteName(name_idx));
                    }
                }

                self.free_temp(subgroup_reg);

                // `skip_patch` jumps here (no match → continue to next handler)
                self.patch_jump(skip_patch);
            }
        }

        // After all handlers: check if group_reg has remaining exceptions.
        // If group_reg is None (all matched), call EndExcept + jump to end.
        // If group_reg is a group, re-raise it.
        // We check by using JumpIfFalse on group_reg (None is falsy; a group is truthy).
        let remaining_check = self.emit(Insn::JumpIfFalse(group_reg, 0));
        // Remaining exceptions exist — re-raise the group.
        // If no handler matched: the outer SetupExcept is still active and will
        // catch this re-raise to run the finally block.
        // If a handler matched but left some exceptions (partial match): the outer
        // SetupExcept was already popped, so the finally will NOT run here; this
        // is a known limitation (see follow-up issue for except*+finally+partial-match).
        // Restore the try-body lineno so the re-raise is attributed to the
        // failing statement in the try block, not to handler body code.
        self.set_lineno(try_body_lineno_star);
        // PEP 654 (#2755): re-raise the residual group without spurious
        // implicit-context chaining or an extra epilogue traceback frame.
        self.emit(Insn::RaiseExceptStarResidual(group_reg));
        self.patch_jump(remaining_check);

        // No remaining exceptions — clean up normally.
        self.free_temp(group_reg);
        self.emit(Insn::EndExcept);

        if let Some(finally_stmts) = finally_branch {
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
        }

        // Outer finally handler (exception path for exceptions that escape the
        // outer SetupExcept — i.e. exceptions raised inside the try body before
        // any handler fires, or exceptions raised by the handler bodies themselves
        // that re-activate the outer SetupExcept).
        if let Some(outer_idx) = outer_finally_patch {
            // After EndExcept + inline finally above, we must jump past the outer
            // finally handler block — otherwise execution falls through into it
            // and hits RaiseReRaise / LoadExc with no active exception.
            let exc_path_end = self.emit(Insn::Jump(0));

            self.patch_jump(outer_idx);
            let exc_tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(exc_tmp));
            self.free_temp(exc_tmp);
            let finally_stmts = finally_branch.unwrap();
            self.compile_block_with_linenos(finally_stmts, finally_linenos);
            if self.failed {
                return;
            }
            self.emit(Insn::RaiseReRaise);

            // Both the normal-exit Jump and the exception-path Jump land here
            // (past the outer finally handler).
            self.patch_jump(exc_path_end);
        }

        // Patch the normal-exit Jump to land here (past the outer finally handler).
        self.patch_jump(end_patch);
    }

    /// Emit the no-exception `__exit__(None, None, None)` call for a sync
    /// `with` whose context manager is in `ctx_reg`.  Shared by the normal
    /// fall-through exit and the `break`/`continue`/`return` early-exit walk
    /// (`emit_early_exit_cleanups`), so the cleanup runs in both cases
    /// (issue #2295).  Does *not* emit `PopExcept`; the caller is responsible
    /// for popping the handler before invoking this.
    fn emit_with_normal_exit(&mut self, ctx_reg: Reg) {
        let exit_name_idx = self.intern_name("__exit__");
        let exit_frame = self.next_temp;
        if exit_frame.checked_add(4).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many registers for 'with' statement".to_string());
            }
            return;
        }
        self.next_temp = exit_frame + 4;
        if exit_frame + 3 > self.max_reg {
            self.max_reg = exit_frame + 3;
        }
        self.emit(Insn::GetAttrForWith(
            exit_frame,
            ctx_reg,
            exit_name_idx,
            1, // sync with: __exit__
        ));
        self.emit(Insn::LoadNone(exit_frame + 1));
        self.emit(Insn::LoadNone(exit_frame + 2));
        self.emit(Insn::LoadNone(exit_frame + 3));
        self.emit(Insn::Call(exit_frame, 3));
        self.next_temp = exit_frame;
    }

    /// Emit `await __aexit__(None, None, None)` (result discarded) for an
    /// `async with` whose context manager is in `ctx_reg`.  Shared by the
    /// normal fall-through exit and the early-exit walk (issue #2295).  Does
    /// *not* emit `PopExcept`; the caller pops the handler first.
    fn emit_async_with_normal_exit(&mut self, ctx_reg: Reg) {
        let aexit_name_idx = self.intern_name("__aexit__");
        let exit_frame = self.next_temp;
        if exit_frame.checked_add(4).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many registers for 'async with' statement".to_string());
            }
            return;
        }
        self.next_temp = exit_frame + 4;
        if exit_frame + 3 > self.max_reg {
            self.max_reg = exit_frame + 3;
        }
        self.emit(Insn::GetAttrForWith(
            exit_frame,
            ctx_reg,
            aexit_name_idx,
            4, // async with: __aexit__
        ));
        self.emit(Insn::LoadNone(exit_frame + 1));
        self.emit(Insn::LoadNone(exit_frame + 2));
        self.emit(Insn::LoadNone(exit_frame + 3));
        self.emit(Insn::Call(exit_frame, 3));
        // Drive the awaitable returned by __aexit__; result discarded.  Place
        // the result slot just above the call frame so the temp allocator
        // stays balanced.
        self.next_temp = exit_frame + 1;
        let exit_res = self.next_temp; // == exit_frame + 1
        self.next_temp = exit_frame + 2;
        if self.next_temp - 1 > self.max_reg {
            self.max_reg = self.next_temp - 1;
        }
        self.emit_await_drive_into(exit_frame, exit_res);
        self.next_temp = exit_frame;
    }

    fn compile_with(
        &mut self,
        items: &[(Expr, Option<AssignTarget>)],
        body: &[Stmt],
        body_linenos: &[u32],
    ) {
        // Compile nested with items recursively (outermost first).
        if items.is_empty() {
            self.compile_block_with_linenos(body, body_linenos);
            return;
        }
        let (expr, alias) = &items[0];
        let rest = &items[1..];

        // Capture the `with` header line so the exception-unwind path below can
        // attribute the enclosing frame's traceback node to it.  When `__exit__`
        // raises (or re-raises) while an exception is in flight, CPython points
        // the enclosing frame at the `with` statement line, not at whatever line
        // inside the body originally raised (issue #2419).
        let with_header_lineno = self.current_lineno;

        // ctx = expr
        let ctx_reg = self.compile_expr(expr);

        // VAR = ctx.__enter__()
        // Use GetAttrForWith so AttributeError is converted to TypeError (#1656).
        let enter_name_idx = self.intern_name("__enter__");
        let enter_reg = self.alloc_temp();
        self.emit(Insn::GetAttrForWith(
            enter_reg,
            ctx_reg,
            enter_name_idx,
            0, // sync with: __enter__
        ));
        // Call __enter__() with no args: result goes to enter_reg
        self.emit(Insn::Call(enter_reg, 0));

        // Bind alias if present
        if let Some(tgt) = alias {
            let val_reg = enter_reg;
            match tgt {
                AssignTarget::Name(name) => {
                    if let Some(reg) = self.local_reg(name) {
                        self.emit(Insn::Move(reg, val_reg));
                        self.mark_def(reg);
                        // Issue #820: sync into module_globals_dict at module scope.
                        if self.is_module_scope {
                            let name_idx = self.intern_name(name);
                            self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                        }
                    } else {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::StoreGlobal(name_idx, val_reg));
                    }
                }
                _ => {
                    // Complex targets: just assign via the general mechanism
                    // (simplified: ignore for now)
                }
            }
        }

        // SetupExcept for the body
        let setup_patch = self.emit(Insn::SetupExcept(0));

        // Register the with-exit cleanup so a `break`/`continue`/`return` that
        // leaves the body runs `__exit__(None, None, None)` (issue #2295).
        self.except_cleanups.push(EarlyExitCleanup::WithBody {
            ctx_reg,
            is_async: false,
        });

        // Compile nested with items or body
        if rest.is_empty() {
            self.compile_block_with_linenos(body, body_linenos);
        } else {
            self.compile_with(rest, body, body_linenos);
        }
        // Pop our cleanup entry; the normal/exception paths below emit the exit
        // inline, and inner break/continue/return already consumed it.
        self.except_cleanups.pop();
        if self.failed {
            return;
        }

        // Normal exit
        self.emit(Insn::PopExcept);
        // ctx.__exit__(None, None, None)
        let exit_name_idx = self.intern_name("__exit__");
        self.emit_with_normal_exit(ctx_reg);
        if self.failed {
            return;
        }
        let end_patch = self.emit(Insn::Jump(0));

        // Exception path
        self.patch_jump(setup_patch);
        // Attribute the enclosing frame to the `with` header line (not the body
        // line that raised) for the duration of the unwind-path `__exit__` call
        // and any re-raise it triggers (issue #2419).  The body was compiled
        // above, leaving `current_lineno` pointing at its last statement.
        self.set_lineno(with_header_lineno);
        let exc_tmp = self.alloc_temp();
        self.emit(Insn::LoadExc(exc_tmp));
        // ctx.__exit__(type, exc, None)
        let exit_frame2 = self.next_temp;
        if exit_frame2.checked_add(4).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many registers for 'with' exception handler".to_string());
            }
            return;
        }
        self.next_temp = exit_frame2 + 4;
        if exit_frame2 + 3 > self.max_reg {
            self.max_reg = exit_frame2 + 3;
        }
        let class_name_idx = self.intern_name("__class__");
        self.emit(Insn::GetAttrForWith(
            exit_frame2,
            ctx_reg,
            exit_name_idx,
            1, // sync with: __exit__
        ));
        self.emit(Insn::GetAttr(exit_frame2 + 1, exc_tmp, class_name_idx)); // exc_type
        self.emit(Insn::Move(exit_frame2 + 2, exc_tmp));
        // traceback: the real `__traceback__` of the in-flight exception (#2359),
        // materialised from its deferred placeholder.
        self.emit(Insn::LoadExcTraceback(exit_frame2 + 3, exc_tmp));
        self.emit(Insn::Call(exit_frame2, 3));
        let suppress_reg = exit_frame2;
        self.next_temp = exit_frame2 + 1;
        // If __exit__ returned truthy, suppress exception (EndExcept + skip re-raise)
        let suppress_patch = self.emit(Insn::JumpIfTrue(suppress_reg, 0));
        self.free_temp(exc_tmp);
        self.emit(Insn::RaiseReRaise);
        self.patch_jump(suppress_patch);
        self.emit(Insn::EndExcept);

        self.patch_jump(end_patch);
        self.free_temp(ctx_reg);
    }

    /// Compile an `async with` statement (issue #2279).
    ///
    /// Mirrors [`compile_with`] but drives the async context-manager protocol:
    /// `v = await mgr.__aenter__()` on entry and
    /// `await mgr.__aexit__(exc_type, exc, tb)` on exit, awaiting each coroutine
    /// to completion via the shared `GetAwaitable` + `YieldFrom` drive.  The
    /// suppression contract is identical: if `__aexit__` returns truthy while an
    /// exception is in flight, the exception is swallowed.
    ///
    /// `async with` is only legal inside an `async def`; the gate lives here so
    /// it fires even for a manager that never reaches the await (CPython reports
    /// the SyntaxError at compile time regardless).
    fn compile_async_with(
        &mut self,
        items: &[(Expr, Option<AssignTarget>)],
        body: &[Stmt],
        body_linenos: &[u32],
    ) {
        if items.is_empty() {
            self.compile_block_with_linenos(body, body_linenos);
            return;
        }
        if !self.is_async_function {
            self.set_syntax_error("'async with' outside async function");
            return;
        }
        let (expr, alias) = &items[0];
        let rest = &items[1..];

        // Capture the `async with` header line for the exception-unwind path
        // below, mirroring the sync `with` fix (issue #2419).
        let with_header_lineno = self.current_lineno;

        // mgr = expr
        let ctx_reg = self.compile_expr(expr);

        // v = await mgr.__aenter__()
        // GetAttrForWith maps a missing dunder to TypeError (#1656), matching the
        // async-context-manager protocol error CPython raises.
        let aenter_name_idx = self.intern_name("__aenter__");
        let aenter_reg = self.alloc_temp();
        self.emit(Insn::GetAttrForWith(
            aenter_reg,
            ctx_reg,
            aenter_name_idx,
            3, // async with: __aenter__
        ));
        self.emit(Insn::Call(aenter_reg, 0));
        // Drive the returned awaitable; result_reg holds __aenter__'s value.
        let entered_reg = self.alloc_temp();
        self.emit_await_drive_into(aenter_reg, entered_reg);
        self.free_temp(aenter_reg);

        // Bind alias if present.
        if let Some(tgt) = alias {
            self.compile_store_unpack_target(tgt, entered_reg);
        }
        self.free_temp(entered_reg);

        // SetupExcept for the body.
        let setup_patch = self.emit(Insn::SetupExcept(0));

        // Register the with-exit cleanup so a `break`/`continue`/`return` that
        // leaves the body awaits `__aexit__(None, None, None)` (issue #2295).
        self.except_cleanups.push(EarlyExitCleanup::WithBody {
            ctx_reg,
            is_async: true,
        });

        if rest.is_empty() {
            self.compile_block_with_linenos(body, body_linenos);
        } else {
            self.compile_async_with(rest, body, body_linenos);
        }
        self.except_cleanups.pop();
        if self.failed {
            return;
        }

        // Normal exit: await mgr.__aexit__(None, None, None); discard result.
        self.emit(Insn::PopExcept);
        let aexit_name_idx = self.intern_name("__aexit__");
        self.emit_async_with_normal_exit(ctx_reg);
        if self.failed {
            return;
        }
        let end_patch = self.emit(Insn::Jump(0));

        // Exception path: res = await mgr.__aexit__(type, exc, None).
        self.patch_jump(setup_patch);
        // Attribute the enclosing frame to the `async with` header line during
        // the unwind-path `__aexit__` call / re-raise (issue #2419).
        self.set_lineno(with_header_lineno);
        let exc_tmp = self.alloc_temp();
        self.emit(Insn::LoadExc(exc_tmp));
        let exit_frame2 = self.next_temp;
        if exit_frame2.checked_add(4).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many registers for 'async with' exception handler".to_string());
            }
            return;
        }
        self.next_temp = exit_frame2 + 4;
        if exit_frame2 + 3 > self.max_reg {
            self.max_reg = exit_frame2 + 3;
        }
        let class_name_idx = self.intern_name("__class__");
        self.emit(Insn::GetAttrForWith(
            exit_frame2,
            ctx_reg,
            aexit_name_idx,
            4, // async with: __aexit__
        ));
        self.emit(Insn::GetAttr(exit_frame2 + 1, exc_tmp, class_name_idx)); // exc_type
        self.emit(Insn::Move(exit_frame2 + 2, exc_tmp));
        // traceback: the real `__traceback__` of the in-flight exception (#2359),
        // materialised from its deferred placeholder.
        self.emit(Insn::LoadExcTraceback(exit_frame2 + 3, exc_tmp));
        self.emit(Insn::Call(exit_frame2, 3));
        // Drive the awaitable returned by __aexit__; its value decides
        // suppression.  The result goes to `suppress_reg` (the slot just above
        // the call frame); the await-drive scratch temps are allocated above it
        // and reclaimed, leaving `suppress_reg` live for the JumpIfTrue below.
        let suppress_reg = exit_frame2 + 4;
        self.next_temp = suppress_reg + 1;
        if suppress_reg > self.max_reg {
            self.max_reg = suppress_reg;
        }
        self.emit_await_drive_into(exit_frame2, suppress_reg);
        self.next_temp = suppress_reg + 1;
        // If __aexit__ returned truthy, suppress; otherwise re-raise.
        let suppress_patch = self.emit(Insn::JumpIfTrue(suppress_reg, 0));
        self.free_temp(exc_tmp);
        self.emit(Insn::RaiseReRaise);
        self.patch_jump(suppress_patch);
        self.emit(Insn::EndExcept);

        self.patch_jump(end_patch);
        self.free_temp(ctx_reg);
    }

    /// Compile an `async for` statement (issue #2279).
    ///
    /// Lowers to the asynchronous-iterator protocol: `it = aiter.__aiter__()`
    /// then a loop body that does `x = await type(it).__anext__(it)`, exiting on
    /// `StopAsyncIteration` (running the `else` clause on a clean exit).  The
    /// `await` reuses the shared `GetAwaitable` + `YieldFrom` drive.
    ///
    /// `async for` is only legal inside an `async def`.
    fn compile_async_for(
        &mut self,
        target: &AssignTarget,
        aiter_expr: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
    ) {
        if !self.is_async_function {
            self.set_syntax_error("'async for' outside async function");
            return;
        }

        // it = aiter.__aiter__()  (not awaited — __aiter__ returns the iterator
        // synchronously per PEP 492).
        let aiter_src = self.compile_expr(aiter_expr);
        let aiter_name_idx = self.intern_name("__aiter__");
        let it_reg = self.alloc_temp();
        self.emit(Insn::GetAttrForWith(it_reg, aiter_src, aiter_name_idx, 2)); // async for: __aiter__
        self.emit(Insn::Call(it_reg, 0));
        self.free_temp(aiter_src);

        // Pre-load the StopAsyncIteration type once, in a register that lives for
        // the whole loop (used by MatchExcept on every iteration's exit check).
        let stop_async_reg = self.compile_expr(&Expr::Var("StopAsyncIteration".to_string(), None));

        let loop_start = self.pc();
        // Each iteration runs `await it.__anext__()` inside a SetupExcept so a
        // StopAsyncIteration (raised by the coroutine) can break the loop.
        let setup_patch = self.emit(Insn::SetupExcept(0));
        let anext_name_idx = self.intern_name("__anext__");
        let anext_reg = self.alloc_temp();
        self.emit(Insn::GetAttrForWith(
            anext_reg,
            it_reg,
            anext_name_idx,
            5, // async for: __anext__
        ));
        self.emit(Insn::Call(anext_reg, 0));
        let item_reg = self.alloc_temp();
        self.emit_await_drive_into(anext_reg, item_reg);
        self.free_temp(anext_reg);
        // Item obtained successfully: leave the per-iteration handler.
        self.emit(Insn::PopExcept);

        // Assign the item to the loop target, then run the body.
        self.compile_store_unpack_target(target, item_reg);
        self.free_temp(item_reg);
        if self.failed {
            return;
        }

        self.loops.push(LoopCtx {
            break_patches: SmallVec::new(),
            continue_target: Some(loop_start),
            continue_patches: SmallVec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved_def_set = self.def_set;
        self.mark_target_def(target);
        self.compile_block_with_linenos(body, body_linenos);
        self.def_set = saved_def_set;
        if self.failed {
            return;
        }
        // Back-edge to the top of the loop.
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));

        // ── Loop-exit handler: reached when __anext__ raised. ──
        self.patch_jump(setup_patch);
        let exc_tmp = self.alloc_temp();
        self.emit(Insn::LoadExc(exc_tmp));
        // If it's NOT StopAsyncIteration, re-raise; otherwise fall through and
        // exit the loop normally (StopAsyncIteration is swallowed).
        let not_stop_patch = self.emit(Insn::MatchExcept(stop_async_reg, 0));
        self.emit(Insn::EndExcept);
        let exit_to_else = self.emit(Insn::Jump(0));
        self.patch_jump(not_stop_patch);
        self.emit(Insn::RaiseReRaise);
        self.patch_jump(exit_to_else);
        self.free_temp(exc_tmp);

        let ctx = self.loops.pop().unwrap();
        self.free_temp(stop_async_reg);
        self.free_temp(it_reg);

        // `else` runs on normal (StopAsyncIteration) exit, not after `break`.
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
    }

    // ── Expression compilation ────────────────────────────────────────────────

    /// Retarget the last emitted instruction's dst from `from` to `to`.
    /// Returns true if the instruction was retargeted (had dst == from).
    fn retarget_last(&mut self, from: Reg, to: Reg) -> bool {
        let Some(insn) = self.insns.last_mut() else {
            return false;
        };
        let dst = match insn {
            Insn::BinOp(d, ..)
            | Insn::BinOpInPlace(d, ..)
            | Insn::BinOpConst(d, ..)
            | Insn::BinOpImm(d, ..)
            | Insn::UnaryOp(d, ..)
            | Insn::LoadConst(d, ..)
            | Insn::LoadNone(d)
            | Insn::LoadGlobal(d, ..)
            | Insn::Move(d, ..)
            | Insn::GetAttr(d, ..)
            | Insn::GetAttrForWith(d, ..)
            | Insn::ImportFromAttr(d, ..)
            | Insn::GetItem(d, ..)
            // Call is NOT retargetable: Call(func_reg, argc) uses func_reg as both
            // the function source and the result destination. Retargeting it to a
            // different register would point the call at the wrong function.
            | Insn::MakeFunction(d, ..)
            | Insn::MakeClass(d, ..)
            | Insn::BuildList(d, ..)
            | Insn::BuildTuple(d, ..)
            | Insn::BuildDict(d, ..)
            | Insn::ForIter(d, ..)
            | Insn::LoadExc(d)
            | Insn::ImportModule(d, ..) => d,
            _ => return false,
        };
        if *dst == from {
            *dst = to;
            true
        } else {
            false
        }
    }

    fn compile_expr_into(&mut self, expr: &Expr, dst: Reg) {
        if self.failed {
            return;
        }
        let saved_next = self.next_temp;
        let insn_before = self.insns.len();
        let r = self.compile_expr(expr);
        if r != dst {
            // Safe to retarget only when the expression compiled to EXACTLY one
            // instruction and the result is a fresh temp: guarantees no control
            // flow or multi-instruction sequences where other branches still write
            // to `r` and would be missed by retargeting only the last instruction.
            let single = self.insns.len() == insn_before + 1;
            if single && r >= self.base_temp && self.retarget_last(r, dst) {
                self.next_temp = saved_next;
            } else {
                self.emit(Insn::Move(dst, r));
                if r >= self.base_temp {
                    self.next_temp = saved_next;
                }
            }
        }
    }

    /// Try to extract a small i16 integer immediate from an expression.
    /// Returns `Some(imm)` when `expr` is an integer literal in `i16` range.
    fn try_imm_i16(expr: &Expr) -> Option<i16> {
        if let Expr::Int(v) = expr
            && *v >= i16::MIN as i64
            && *v <= i16::MAX as i64
        {
            return Some(*v as i16);
        }
        None
    }

    fn emit_aug_binop(&mut self, reg: Reg, op: BinaryOp, expr: &Expr) {
        if let Some(imm) = Self::try_imm_i16(expr) {
            self.emit(Insn::BinOpImm(reg, reg, op, imm, true));
        } else if let Some(val) = fold_constant(expr) {
            // BinOpConst is safe for augmented assignment: the VM's BinOpConst
            // handler calls try_inplace_op before eval_binary, so mutable
            // containers (list *= / list += / set |= etc.) still get the
            // in-place fast path even when the RHS is a folded constant.  The
            // `is_aug = true` flag tells the VM this fused op carries in-place
            // semantics (issue #1874).
            let idx = self.intern_const(val);
            self.emit(Insn::BinOpConst(reg, reg, op, idx, true));
        } else {
            let rhs = self.compile_expr(expr);
            self.emit(Insn::BinOpInPlace(reg, reg, op, rhs));
            self.free_temp(rhs);
        }
    }

    fn compile_short_circuit(&mut self, left: &Expr, right: &Expr, jump_if_true: bool) -> Reg {
        let lhs = self.compile_expr(left);
        // Always copy to a fresh temp so the JumpIf tests the copy, not `lhs`
        // itself. This prevents the optimizer from fusing BinOp(lhs,lhs,...)+
        // JumpIfFalse(lhs) → CmpJumpIfFalse, which would leave `lhs` holding
        // the original (pre-BinOp) value after the jump instead of False/True.
        let dst = self.alloc_temp();
        self.emit(Insn::Move(dst, lhs));
        self.free_temp(lhs);
        let jmp = if jump_if_true {
            self.emit(Insn::JumpIfTrue(dst, 0))
        } else {
            self.emit(Insn::JumpIfFalse(dst, 0))
        };
        let saved = self.next_temp;
        self.compile_expr_into(right, dst);
        self.next_temp = saved;
        self.patch_jump(jmp);
        dst
    }

    fn compile_literal(&mut self, v: Value) -> Reg {
        let idx = self.intern_const(v);
        let dst = self.alloc_temp();
        self.emit(Insn::LoadConst(dst, idx));
        dst
    }

    fn compile_expr(&mut self, expr: &Expr) -> Reg {
        if self.failed {
            return 0;
        }
        match expr {
            Expr::None => {
                let dst = self.alloc_temp();
                self.emit(Insn::LoadNone(dst));
                dst
            }
            Expr::Ellipsis => self.compile_literal(Value::ellipsis()),
            Expr::Int(v) => self.compile_literal(Value::int(*v)),
            Expr::BigInt(s) => {
                // The decimal string was validated at lex time; parse cannot fail.
                let n = s
                    .parse::<PyBigInt>()
                    .expect("BigInt decimal string is valid");
                self.compile_literal(Value::bigint(n))
            }
            Expr::Float(v) => self.compile_literal(Value::float(*v)),
            Expr::Str(s) => self.compile_literal(Value::string(s.clone())),
            Expr::Bytes(b) => self.compile_literal(Value::bytes(b.clone())),
            Expr::Complex(re, im) => self.compile_literal(Value::complex(*re, *im)),
            Expr::Bool(b) => self.compile_literal(Value::bool_(*b)),
            Expr::Var(name, span) => {
                // PEP 657 caret anchor (#2426): `set_col_span_for_next` arms the
                // name's column span so the very next `emit` stamps it onto the
                // load instruction that may raise NameError; `emit` then clears
                // it.  We arm *immediately before* each load emit (not for the
                // definitely-bound-local path, which emits nothing) so a stale
                // span never leaks onto an unrelated instruction.  A bare name's
                // anchor is whole-span (`^`), so widen the `(start, end)` form to
                // the `(full, prim) = (start, start, end, end)` shape (#2411).
                //
                // Multi-line line stamping (#2632): the parser also records the
                // name's own 1-based line.  When it differs from the statement's
                // `current_lineno` (the name sits on a continuation line of a
                // multi-line expression), stamp the load instruction with the
                // name's line so a NameError it raises reports that line and its
                // source text — matching CPython 3.12, which gives each name node
                // its own lineno.  We restore `current_lineno` afterwards so the
                // override never leaks onto sibling instructions.
                let name_lineno = span.and_then(|(_, _, ln)| (ln != 0).then_some(ln));
                let span: Option<crate::ast::CaretSpan> = span.map(|(s, e, _)| (s, s, e, e));
                let saved_lineno = self.current_lineno;
                if let Some(ln) = name_lineno {
                    self.set_lineno(ln);
                }
                let result = if let Some(reg) = self.local_reg(name) {
                    let definitely_bound = (reg as usize) < 64 && (self.def_set >> reg) & 1 != 0;
                    if !definitely_bound {
                        // Issue #1411: at module scope, a name that is not yet
                        // definitely bound must resolve through the global →
                        // builtins chain rather than raising NameError.  Module
                        // scope is sequential (like a REPL), so a later
                        // assignment does NOT shadow earlier reads.  LoadGlobal
                        // already has a fastlocal-register fallback (via
                        // vm_frame_views) for names that have been assigned,
                        // so already-written names are still found efficiently.
                        if self.is_module_scope {
                            let name_idx = self.intern_name(name);
                            let dst = self.alloc_temp();
                            self.set_col_span_for_next(span);
                            self.emit(Insn::LoadGlobal(dst, name_idx));
                            self.set_lineno(saved_lineno);
                            return dst;
                        }
                        let name_idx = self.intern_name(name);
                        self.set_col_span_for_next(span);
                        self.emit(Insn::CheckLocal(reg, name_idx));
                    }
                    reg
                } else {
                    // global / nonlocal / cell / free variable
                    let name_idx = self.intern_name(name);
                    let dst = self.alloc_temp();
                    // A function-scope cell / nonlocal resolves in the env chain;
                    // emit LoadCell to skip the LoadGlobal inline-cache + module
                    // -dict path (issue #2339).  Everything else (true globals,
                    // builtins, module/class-scope free vars) keeps LoadGlobal.
                    self.set_col_span_for_next(span);
                    if self.is_function_cell(name) {
                        self.emit(Insn::LoadCell(dst, name_idx));
                    } else {
                        self.emit(Insn::LoadGlobal(dst, name_idx));
                    }
                    dst
                };
                self.set_lineno(saved_lineno);
                result
            }
            Expr::Unary { op, expr, span } => {
                let src = self.compile_expr(expr);
                let dst = self.ensure_dst(src);
                // PEP 657 caret anchor (#2582): underline the whole `OP operand`
                // span with `^` for the arithmetic unary forms.  Arm immediately
                // before the UnaryOp that may raise (e.g. TypeError on `-"s"`);
                // `emit` consumes and clears it.  `span` is `None` for `not`.
                self.set_col_span_for_next(*span);
                self.emit(Insn::UnaryOp(dst, *op, src));
                dst
            }
            Expr::Binary {
                left,
                op,
                right,
                span,
            } => match op {
                BinaryOp::And => self.compile_short_circuit(left, right, false),
                BinaryOp::Or => self.compile_short_circuit(left, right, true),
                _ => {
                    let lhs = self.compile_expr(left);
                    let dst = self.ensure_dst(lhs);
                    let rhs = self.compile_expr(right);
                    // PEP 657 caret anchor (#2411): the operator underlines `^`,
                    // operands `~`.  Arm immediately before the BinOp that may
                    // raise (e.g. ZeroDivisionError / TypeError); `emit` clears it.
                    self.set_col_span_for_next(*span);
                    self.emit(Insn::BinOp(dst, lhs, *op, rhs));
                    self.free_temp(rhs);
                    dst
                }
            },
            Expr::Compare { left, ops } => {
                if ops.len() == 1 {
                    let (cmp_op, right) = &ops[0];
                    let lhs = self.compile_expr(left);
                    let bin_op = BinaryOp::from(*cmp_op);
                    let dst = self.ensure_dst(lhs);
                    let rhs = self.compile_expr(right);
                    self.emit(Insn::BinOp(dst, lhs, bin_op, rhs));
                    self.free_temp(rhs);
                    dst
                } else {
                    // Chained comparison: a < b < c  →  (a < b) and (b < c)
                    // Evaluate left once, then chain.
                    let first_lhs = self.compile_expr(left);
                    let result_dst = self.alloc_temp();
                    let mut and_patches: Vec<usize> = Vec::new();
                    let mut prev_rhs = first_lhs;
                    for (i, (cmp_op, rhs_expr)) in ops.iter().enumerate() {
                        let bin_op = BinaryOp::from(*cmp_op);
                        let rhs = self.compile_expr(rhs_expr);
                        let last = i == ops.len() - 1;
                        // For the last comparison write directly into result_dst to
                        // avoid a trailing Move(result_dst, cmp_dst).
                        let cmp_dst = if last { result_dst } else { self.alloc_temp() };
                        self.emit(Insn::BinOp(cmp_dst, prev_rhs, bin_op, rhs));
                        if i > 0 {
                            self.free_temp(prev_rhs);
                        }
                        if !last {
                            self.emit(Insn::Move(result_dst, cmp_dst));
                            self.free_temp(cmp_dst);
                            let p = self.emit(Insn::JumpIfFalse(result_dst, 0));
                            and_patches.push(p);
                        }
                        prev_rhs = rhs;
                    }
                    self.free_temp(prev_rhs);
                    for p in and_patches {
                        self.patch_jump(p);
                    }
                    self.free_temp(first_lhs);
                    result_dst
                }
            }
            Expr::Call { func, args, span } => self.compile_call(func, args, *span),
            Expr::Attr { target, name, span } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                let dst = self.ensure_dst(obj);
                // PEP 657 caret anchor (#2442): underline the whole `obj.attr`
                // span.  Arm immediately before the GetAttr that may raise
                // AttributeError; `emit` clears it.
                self.set_col_span_for_next(*span);
                self.emit(Insn::GetAttr(dst, obj, name_idx));
                dst
            }
            Expr::Index {
                target,
                index,
                span,
            } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                let dst = self.ensure_dst(obj);
                // PEP 657 caret anchor (#2411): object underlined `~`, `[...]`
                // underlined `^`.  Arm before the GetItem that may raise
                // KeyError / IndexError / TypeError; `emit` clears it.
                self.set_col_span_for_next(*span);
                self.emit(Insn::GetItem(dst, obj, idx));
                self.free_temp(idx);
                dst
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                // Rvalue slice read `obj[lo:hi:step]`: emit GetSlice, which reads
                // the three contiguous bound registers directly and slices `obj`
                // without materialising a `slice` object on the built-in-sequence
                // fast path (#1964, CPython BINARY_SLICE analogue).
                let obj = self.compile_expr(target);
                let dst = self.ensure_dst(obj);
                let saved_next = self.next_temp;
                let base =
                    self.compile_slice_bounds(lower.as_deref(), upper.as_deref(), step.as_deref());
                self.emit(Insn::GetSlice(dst, obj, base));
                // The three bound slots [base, base+3) are consumed by GetSlice.
                self.next_temp = saved_next;
                dst
            }
            Expr::List(items) => {
                if items.iter().any(|e| matches!(e, Expr::Starred(_))) {
                    self.compile_unpack_list_or_tuple(items, false)
                } else {
                    self.compile_collection(items, false)
                }
            }
            Expr::Tuple(items) => {
                if items.iter().any(|e| matches!(e, Expr::Starred(_))) {
                    self.compile_unpack_list_or_tuple(items, true)
                } else {
                    self.compile_collection(items, true)
                }
            }
            Expr::Starred(_) => {
                // `*expr` is only valid as a child of a list/tuple/set literal,
                // a call-site argument, or an assign target.  Encountering it
                // here means the parser produced one in an unexpected position.
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("can't use starred expression here".to_string());
                }
                0
            }
            Expr::Set(items) => self.compile_set_literal(items),
            Expr::Dict(items) => self.compile_dict_literal(items),
            Expr::Ternary { cond, then, else_ } => {
                let cond_reg = self.compile_expr(cond);
                let jmp_false = self.emit(Insn::JumpIfFalse(cond_reg, 0));
                self.free_temp(cond_reg);
                let dst = self.alloc_temp();
                let saved = self.next_temp;
                self.compile_expr_into(then, dst);
                self.next_temp = saved;
                let jmp_end = self.emit(Insn::Jump(0));
                self.patch_jump(jmp_false);
                let saved = self.next_temp;
                self.compile_expr_into(else_, dst);
                self.next_temp = saved;
                self.patch_jump(jmp_end);
                dst
            }
            Expr::Lambda { params, body } => self.compile_lambda(params, body),
            Expr::ListComp { elt, clauses } => self.compile_list_comp(elt, clauses),
            Expr::DictComp { key, val, clauses } => self.compile_dict_comp(key, val, clauses),
            Expr::SetComp { elt, clauses } => self.compile_set_comp(elt, clauses),
            Expr::GenExp { elt, clauses } => self.compile_gen_exp(elt, clauses),
            Expr::Named { target, value } => {
                let val_reg = self.compile_expr(value);
                if let Some(reg) = self.local_reg(target) {
                    if val_reg != reg {
                        self.emit(Insn::Move(reg, val_reg));
                    }
                    self.mark_def(reg);
                } else {
                    let name_idx = self.intern_name(target);
                    self.emit(Insn::StoreGlobal(name_idx, val_reg));
                }
                val_reg
            }
            Expr::FString(parts) => self.compile_fstring(parts),

            Expr::Yield(val_expr) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'yield' outside function");
                    return 0;
                }
                // Compile the yielded value (or None if bare `yield`).
                let src = if let Some(e) = val_expr {
                    self.compile_expr(e)
                } else {
                    let r = self.alloc_temp();
                    self.emit(Insn::LoadNone(r));
                    r
                };
                let dst = self.alloc_temp();
                self.emit(Insn::Yield { src, dst });
                self.free_temp(src);
                dst
            }

            Expr::YieldFrom(iter_expr) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'yield' outside function");
                    return 0;
                }
                // `yield from` is not allowed inside an `async def` body
                // (#2280): CPython raises SyntaxError.  (A bare `yield` is fine
                // — it makes the function an async generator.)  `await` lowers
                // to the same `YieldFrom` *instruction* internally, but that
                // path goes through `Expr::Await`, not this user-facing
                // `Expr::YieldFrom` compilation, so it is unaffected.
                if self.is_async_function {
                    self.set_syntax_error("'yield from' inside async function");
                    return 0;
                }
                // PEP 380 `yield from` delegation via the single YieldFrom instruction.
                //
                // The VM handles the send/yield/StopIteration loop internally:
                // - Calls sub_iter.send(sent_reg) on each execution.
                // - Yields the produced value to the outer caller, suspending at this
                //   instruction; on resume, writes the received sent value into sent_reg.
                // - On StopIteration, writes the sub-iterator's return value into
                //   result_reg and falls through to the next instruction.
                //
                // Unlike the old ForIter/Yield/Jump loop, YieldFrom forwards the outer
                // caller's sent value (and throw) into the sub-iterator (PEP 380).

                // Evaluate the iterable and call iter() on it to get the iterator
                // object.  For generators, iter(gen) == gen; for lists, tuples, etc.,
                // iter() returns the appropriate iterator.
                // Call convention: Call(func_reg, argc) reads args from
                // func_reg+1 .. func_reg+argc; alloc_temp() is sequential so
                // iter_arg_reg == iter_fn_reg + 1.
                let iter_src = self.compile_expr(iter_expr);
                let iter_fn_reg = self.alloc_temp();
                let iter_name_idx = self.intern_name("iter");
                self.emit(Insn::LoadGlobal(iter_fn_reg, iter_name_idx));
                let iter_arg_reg = self.alloc_temp(); // == iter_fn_reg + 1
                self.emit(Insn::Move(iter_arg_reg, iter_src));
                self.free_temp(iter_src);
                self.emit(Insn::Call(iter_fn_reg, 1)); // result lands in iter_fn_reg
                self.free_temp(iter_arg_reg);
                let iter_reg = iter_fn_reg; // iter_reg holds the iterator object

                // sent_reg: value to send on each iteration.  Initialized to None
                // (first call is always next()-equivalent); on resumption the VM
                // writes the caller's sent value here (like Yield.dst).
                let sent_reg = self.alloc_temp();
                self.emit(Insn::LoadNone(sent_reg));

                // result_reg: receives StopIteration.value when sub-iterator exhausts.
                // This is the value of the `yield from` expression in the outer generator.
                let result_reg = self.alloc_temp();
                self.emit(Insn::LoadNone(result_reg));

                self.emit(Insn::YieldFrom {
                    iter_reg,
                    sent_reg,
                    result_reg,
                });

                // iter_reg and sent_reg are only live during YieldFrom.
                self.free_temp(sent_reg);
                self.free_temp(iter_reg);

                // result_reg is the value of the `yield from` expression.
                result_reg
            }

            Expr::Await(awaited_expr) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'await' outside function");
                    return 0;
                }
                if !self.is_async_function {
                    self.set_syntax_error("'await' outside async function");
                    return 0;
                }
                // `await expr` lowers to roughly `yield from GET_AWAITABLE(expr)`
                // (issue #1039).  `GetAwaitable` resolves the awaitable to its
                // driving iterator (a coroutine drives itself; an object with
                // `__await__` yields its `__await__()` result); `YieldFrom` then
                // reuses the PEP 380 suspend/resume machinery to drive it to
                // completion, surfacing its return value (StopIteration.value).
                //
                // The result register is allocated FIRST so it sits below the
                // scratch temps (`awaited_src`/`iter_reg`/`sent_reg`).  The temp
                // allocator is strictly LIFO — `free_temp` only reclaims the top
                // of the stack — so the scratch temps are freed in reverse
                // allocation order while `result_reg` (the expression's value,
                // which outlives them) stays below.  An earlier version freed
                // `awaited_src` before the temps allocated above it, making those
                // frees silent no-ops; the leaked slots then corrupted register
                // allocation for a *subsequent* await when this await was nested
                // in a larger expression (e.g. `print(await x)`), surfacing as a
                // spurious "object is not iterable".
                let result_reg = self.alloc_temp();
                let awaited_src = self.compile_expr(awaited_expr);
                self.emit_await_drive_into(awaited_src, result_reg);
                self.free_temp(awaited_src);
                result_reg
            }
        }
    }

    /// Emit the `await` drive for an awaitable already living in `awaited_src`,
    /// placing the awaited result into `result_reg` (which the caller has
    /// allocated *below* `awaited_src` so the LIFO temp allocator can reclaim
    /// the scratch temps without clobbering it).
    ///
    /// This is the same `GetAwaitable` + `YieldFrom` sequence the `Expr::Await`
    /// lowering uses (issue #1039); `async for` / `async with` reuse it to drive
    /// `__anext__` / `__aenter__` / `__aexit__` coroutines to completion
    /// (issue #2279).  Both `awaited_src` and `result_reg` must outlive this
    /// call; only the internal scratch temps are freed here.
    fn emit_await_drive_into(&mut self, awaited_src: Reg, result_reg: Reg) {
        let iter_reg = self.alloc_temp();
        self.emit(Insn::GetAwaitable(iter_reg, awaited_src));

        let sent_reg = self.alloc_temp();
        self.emit(Insn::LoadNone(sent_reg));
        self.emit(Insn::LoadNone(result_reg));

        self.emit(Insn::YieldFrom {
            iter_reg,
            sent_reg,
            result_reg,
        });

        self.free_temp(sent_reg);
        self.free_temp(iter_reg);
    }

    /// Compile an f-string into a series of str-conversions concatenated with `+`.
    fn compile_fstring(&mut self, parts: &[FStringPart]) -> Reg {
        if parts.is_empty() {
            return self.compile_literal(Value::string(String::new()));
        }

        // Compile each part into a string register.
        let mut part_regs: Vec<Reg> = Vec::new();
        for part in parts {
            let r = match part {
                FStringPart::Literal(s) => self.compile_literal(Value::string(s.clone())),
                FStringPart::Expr {
                    expr,
                    conversion,
                    format_spec,
                    debug_text,
                    span,
                    line,
                } => {
                    // Stamp this field's instructions with its own source line
                    // so a field on a continuation line of a multi-line
                    // f-string (or in a later implicitly-joined fragment)
                    // anchors the traceback on the right line (issue #2587).
                    // `line == 0` means line info is unavailable; leave the
                    // statement's current line in place.
                    let saved_lineno = self.current_lineno;
                    if *line != 0 {
                        self.set_lineno(*line);
                    }
                    // Python 3.8 debug form `f"{x=}"`: emit the verbatim
                    // source text (with trailing `=`) as a literal prefix
                    // BEFORE the formatted value.  When no explicit
                    // conversion flag and no format spec are given, the
                    // default conversion becomes `repr` instead of the
                    // implicit `str`/`format(val, "")` path.
                    if let Some(label) = debug_text {
                        let lit_r = self.compile_literal(Value::string(label.clone()));
                        part_regs.push(lit_r);
                    }
                    let val_r = self.compile_expr(expr);
                    // Determine the effective conversion: explicit !r/!s/!a
                    // wins; otherwise, in debug form with no format spec, the
                    // implicit conversion is `repr`.
                    let effective_conversion: Option<char> = match conversion {
                        Some(c) => Some(*c),
                        None if debug_text.is_some() && format_spec.is_none() => Some('r'),
                        None => None,
                    };
                    // Apply conversion flag first.
                    let val_r = match effective_conversion {
                        Some('r') => {
                            // repr(val)
                            let frame = self.next_temp;
                            if frame + 1 > self.max_reg {
                                self.max_reg = frame + 1;
                            }
                            self.next_temp = frame + 2;
                            let repr_idx = self.intern_name("repr");
                            self.emit(Insn::LoadGlobal(frame, repr_idx));
                            if val_r != frame + 1 {
                                self.emit(Insn::Move(frame + 1, val_r));
                            }
                            self.free_temp(val_r);
                            // PEP 657 (#2582): the `{...}` field caret covers a
                            // conversion `__repr__` that raises.
                            self.set_col_span_for_next(*span);
                            self.emit(Insn::Call(frame, 1));
                            self.next_temp = frame + 1;
                            frame
                        }
                        Some('s') => {
                            // str(val) — calls __str__ on user instances
                            let frame = self.next_temp;
                            if frame + 1 > self.max_reg {
                                self.max_reg = frame + 1;
                            }
                            self.next_temp = frame + 2;
                            let str_idx = self.intern_name("str");
                            self.emit(Insn::LoadGlobal(frame, str_idx));
                            if val_r != frame + 1 {
                                self.emit(Insn::Move(frame + 1, val_r));
                            }
                            self.free_temp(val_r);
                            // PEP 657 (#2582): the `{...}` field caret covers a
                            // conversion `__str__` that raises.
                            self.set_col_span_for_next(*span);
                            self.emit(Insn::Call(frame, 1));
                            self.next_temp = frame + 1;
                            frame
                        }
                        Some('a') => {
                            // ascii(val) — repr with non-ASCII chars escaped
                            let frame = self.next_temp;
                            if frame + 1 > self.max_reg {
                                self.max_reg = frame + 1;
                            }
                            self.next_temp = frame + 2;
                            let ascii_idx = self.intern_name("ascii");
                            self.emit(Insn::LoadGlobal(frame, ascii_idx));
                            if val_r != frame + 1 {
                                self.emit(Insn::Move(frame + 1, val_r));
                            }
                            self.free_temp(val_r);
                            // PEP 657 (#2582): the `{...}` field caret covers a
                            // conversion `ascii()`/`__repr__` that raises.
                            self.set_col_span_for_next(*span);
                            self.emit(Insn::Call(frame, 1));
                            self.next_temp = frame + 1;
                            frame
                        }
                        _ => val_r,
                    };
                    // Apply format spec if present.  The spec is itself a
                    // mini f-string (literals plus nested `{expr}` parts), so
                    // we compile it via the same fstring helper to obtain a
                    // single string register, then format the value with it via
                    // the dedicated `FormatValueSpec` opcode.  This mirrors
                    // CPython's `FORMAT_VALUE` (which calls `PyObject_Format`
                    // directly): it skips the `format` global lookup, the
                    // two-register call window, and the call-arg expansion that
                    // the previous `Call(format, 2)` lowering paid on every
                    // interpolation.  User `__format__` dispatch for PyInstance
                    // values is preserved by the VM via `dispatch_dunder_format`.
                    let field_r = if let Some(spec_parts) = format_spec {
                        // Nested spec fields carry their own absolute `line`.
                        let spec_r = self.compile_fstring(spec_parts);
                        let dst = self.alloc_temp();
                        // PEP 657 (#2582): the `{...}` field caret covers a
                        // `__format__` that raises.  Arm immediately before the
                        // op; `emit` consumes and clears it.
                        self.set_col_span_for_next(*span);
                        self.emit(Insn::FormatValueSpec(dst, val_r, spec_r));
                        self.free_temp(val_r);
                        self.free_temp(spec_r);
                        dst
                    } else {
                        // format(val, "") — dispatch __format__("") per Python
                        // semantics, but via the dedicated FormatValue opcode so
                        // we skip the `format` global lookup and the generic call
                        // frame (issue #1926). The VM preserves user
                        // `__format__`/`__str__` dispatch for PyInstance values.
                        let dst = self.alloc_temp();
                        // PEP 657 (#2582): the `{...}` field caret covers a
                        // `__format__`/`__str__` that raises.
                        self.set_col_span_for_next(*span);
                        self.emit(Insn::FormatValue(dst, val_r));
                        self.free_temp(val_r);
                        dst
                    };
                    // Restore the statement's line for the next part / literal.
                    self.set_lineno(saved_lineno);
                    field_r
                }
            };
            part_regs.push(r);
        }

        // Single part: nothing to join.
        if part_regs.len() == 1 {
            return part_regs[0];
        }

        // BuildString consumes `n` CONSECUTIVE str registers and joins them in a
        // single preallocated pass (mirrors CPython's BUILD_STRING). The count is
        // encoded as u8, so for the rare >255-part f-string fall back to the
        // chained `BinOp(Add)` fold below.
        if part_regs.len() <= u8::MAX as usize {
            let n = part_regs.len() as u8;
            // Lay the parts out in a consecutive window starting at `base`, then
            // build into `base` (same shape as BuildList lowering).
            let base = self.next_temp;
            self.next_temp = base + Reg::from(n);
            let max_used = base + Reg::from(n) - 1;
            if max_used > self.max_reg {
                self.max_reg = max_used;
            }
            for (i, &r) in part_regs.iter().enumerate() {
                let slot = base + i as Reg;
                if r != slot {
                    self.emit(Insn::Move(slot, r));
                }
            }
            self.emit(Insn::BuildString(base, base, n));
            // Collapse the temp window: every part register lives below `base`
            // and is dead once the join is done.
            self.next_temp = base + 1;
            return base;
        }

        // Fallback for >255 parts: concatenate with BinOp(Add).
        let mut acc = part_regs[0];
        for &r in &part_regs[1..] {
            let dst = self.ensure_dst(acc);
            self.emit(Insn::BinOp(dst, acc, BinaryOp::Add, r));
            self.free_temp(r);
            acc = dst;
        }
        acc
    }

    // ── Comprehension compilation ──────────────────────────────────────────────

    /// Whether a comprehension is asynchronous due to an `await` in its body
    /// (#2304), independent of any `async for` clause. `elts` are the element
    /// expression(s) — one for list/set/gen, two (key, value) for dict.
    ///
    /// Scoping (matches CPython): the OUTERMOST iterable (`clauses[0].iter`) is
    /// evaluated in the ENCLOSING scope, so an `await` there belongs to the
    /// enclosing frame and does NOT make the comprehension async. We therefore
    /// inspect only the element(s), every clause `cond`, and the
    /// non-outermost clause iterables (`clauses[1..]`).
    fn comp_body_has_await(elts: &[&Expr], clauses: &[CompClause]) -> bool {
        // An `await` directly in the element/cond/non-outermost-iterable makes
        // this comprehension async; a nested async list/set/dict comprehension
        // in the same positions also does (CPython propagates async-ness from a
        // directly-nested COLLECTION comprehension outward — the outer body has
        // to await the inner comp's coroutine; issue #2312).  The outermost
        // iterable (`clauses[0].iter`) is excluded — it is evaluated in the
        // ENCLOSING scope, so an async comp there makes the *enclosing* function
        // async, not this one.
        elts.iter()
            .any(|e| expr_contains_await(e) || expr_has_async_collection_comp(e))
            || clauses.iter().any(|c| {
                c.cond.as_ref().is_some_and(|cond| {
                    expr_contains_await(cond) || expr_has_async_collection_comp(cond)
                })
            })
            || clauses[1..]
                .iter()
                .any(|c| expr_contains_await(&c.iter) || expr_has_async_collection_comp(&c.iter))
    }

    /// Build the loop-nest AST shared by list/set/dict comprehensions.
    ///
    /// Returns a `Vec<Stmt>` representing the `for`/`if` structure that wraps
    /// `innermost`.  The outermost `for` iterates over `IT_PARAM` (the implicit
    /// parameter that receives the outermost iterable from the enclosing scope).
    fn build_comp_loop_body(clauses: &[CompClause], innermost: Stmt) -> Vec<Stmt> {
        const IT_PARAM: &str = ".0";

        let mut body = vec![innermost];
        for clause in clauses[1..].iter().rev() {
            if let Some(cond) = &clause.cond {
                body = vec![Stmt::If {
                    branches: vec![(cond.clone(), body)],
                    else_branch: None,
                    branch_linenos: vec![],
                    else_linenos: vec![],
                }];
            }
            body = vec![Stmt::For {
                target: clause.target.clone(),
                iter: clause.iter.clone(),
                body,
                else_branch: None,
                body_linenos: vec![],
                else_linenos: vec![],
                // `async for` clause (#2283): lowers to compile_async_for.
                is_async: clause.is_async,
            }];
        }
        if let Some(cond) = &clauses[0].cond {
            body = vec![Stmt::If {
                branches: vec![(cond.clone(), body)],
                else_branch: None,
                branch_linenos: vec![],
                else_linenos: vec![],
            }];
        }
        body = vec![Stmt::For {
            target: clauses[0].target.clone(),
            iter: Expr::Var(IT_PARAM.to_string(), None),
            body,
            else_branch: None,
            body_linenos: vec![],
            else_linenos: vec![],
            is_async: clauses[0].is_async,
        }];
        body
    }

    /// Collect all `Expr::Named` (walrus `:=`) target names from an expression,
    /// without descending into nested comprehensions or lambdas (those create
    /// their own implicit scopes, so their walrus targets don't leak here).
    fn collect_walrus_targets_in_expr(expr: &Expr, out: &mut HashSet<String>) {
        match expr {
            Expr::Named { target, value } => {
                out.insert(target.clone());
                Self::collect_walrus_targets_in_expr(value, out);
            }
            Expr::Binary { left, right, .. } => {
                Self::collect_walrus_targets_in_expr(left, out);
                Self::collect_walrus_targets_in_expr(right, out);
            }
            Expr::Unary { expr: e, .. } => Self::collect_walrus_targets_in_expr(e, out),
            Expr::Compare { left, ops } => {
                Self::collect_walrus_targets_in_expr(left, out);
                for (_, e) in ops {
                    Self::collect_walrus_targets_in_expr(e, out);
                }
            }
            Expr::Call { func, args, .. } => {
                Self::collect_walrus_targets_in_expr(func, out);
                for a in args {
                    Self::collect_walrus_targets_in_expr(&a.value, out);
                }
            }
            Expr::Ternary { cond, then, else_ } => {
                Self::collect_walrus_targets_in_expr(cond, out);
                Self::collect_walrus_targets_in_expr(then, out);
                Self::collect_walrus_targets_in_expr(else_, out);
            }
            Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
                for e in items {
                    Self::collect_walrus_targets_in_expr(e, out);
                }
            }
            Expr::Dict(items) => {
                for item in items {
                    match item {
                        DictItem::Pair(k, v) => {
                            Self::collect_walrus_targets_in_expr(k, out);
                            Self::collect_walrus_targets_in_expr(v, out);
                        }
                        DictItem::DoubleSplat(e) => {
                            Self::collect_walrus_targets_in_expr(e, out);
                        }
                    }
                }
            }
            Expr::Index { target, index, .. } => {
                Self::collect_walrus_targets_in_expr(target, out);
                Self::collect_walrus_targets_in_expr(index, out);
            }
            Expr::Attr { target, .. } => Self::collect_walrus_targets_in_expr(target, out),
            Expr::Starred(e) => Self::collect_walrus_targets_in_expr(e, out),
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                Self::collect_walrus_targets_in_expr(target, out);
                for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                    Self::collect_walrus_targets_in_expr(e, out);
                }
            }
            Expr::FString(parts) => {
                for_each_fstring_expr(parts, &mut |e| {
                    Self::collect_walrus_targets_in_expr(e, out);
                });
            }
            // Walrus targets inside nested comprehensions still escape to the
            // nearest non-comprehension scope (PEP 572). Descend so that the
            // outer comprehension's compile_collection_comp_impl can route them
            // as nonlocal/global correctly.  Lambda creates a true new scope.
            Expr::ListComp { elt, clauses }
            | Expr::SetComp { elt, clauses }
            | Expr::GenExp { elt, clauses } => {
                Self::collect_walrus_targets_in_expr(elt, out);
                for clause in clauses {
                    if let Some(c) = &clause.cond {
                        Self::collect_walrus_targets_in_expr(c, out);
                    }
                    Self::collect_walrus_targets_in_expr(&clause.iter, out);
                }
            }
            Expr::DictComp { key, val, clauses } => {
                Self::collect_walrus_targets_in_expr(key, out);
                Self::collect_walrus_targets_in_expr(val, out);
                for clause in clauses {
                    if let Some(c) = &clause.cond {
                        Self::collect_walrus_targets_in_expr(c, out);
                    }
                    Self::collect_walrus_targets_in_expr(&clause.iter, out);
                }
            }
            Expr::Lambda { .. } => {}
            _ => {}
        }
    }

    /// Collect walrus targets from an entire statement list (used to find
    /// which names a comprehension body writes to the enclosing scope).
    fn collect_walrus_targets_in_stmts(stmts: &[Stmt], out: &mut HashSet<String>) {
        for stmt in stmts {
            match stmt {
                Stmt::Expr(e) | Stmt::Return(Some(e)) | Stmt::AnnAssign { value: Some(e), .. } => {
                    Self::collect_walrus_targets_in_expr(e, out);
                }
                Stmt::Assign(_, e) => {
                    Self::collect_walrus_targets_in_expr(e, out);
                }
                Stmt::AugAssign { expr: e, .. } => {
                    Self::collect_walrus_targets_in_expr(e, out);
                }
                Stmt::If {
                    branches,
                    else_branch,
                    ..
                } => {
                    for (cond, body) in branches {
                        Self::collect_walrus_targets_in_expr(cond, out);
                        Self::collect_walrus_targets_in_stmts(body, out);
                    }
                    if let Some(b) = else_branch {
                        Self::collect_walrus_targets_in_stmts(b, out);
                    }
                }
                Stmt::For {
                    iter,
                    body,
                    else_branch,
                    ..
                } => {
                    Self::collect_walrus_targets_in_expr(iter, out);
                    Self::collect_walrus_targets_in_stmts(body, out);
                    if let Some(b) = else_branch {
                        Self::collect_walrus_targets_in_stmts(b, out);
                    }
                }
                Stmt::While {
                    cond,
                    body,
                    else_branch,
                    ..
                } => {
                    Self::collect_walrus_targets_in_expr(cond, out);
                    Self::collect_walrus_targets_in_stmts(body, out);
                    if let Some(b) = else_branch {
                        Self::collect_walrus_targets_in_stmts(b, out);
                    }
                }
                Stmt::IndexAssign { expr: e, .. } | Stmt::SliceAssign { expr: e, .. } => {
                    Self::collect_walrus_targets_in_expr(e, out);
                }
                Stmt::Raise { expr, cause, .. } => {
                    if let Some(e) = expr {
                        Self::collect_walrus_targets_in_expr(e, out);
                    }
                    if let Some(c) = cause {
                        Self::collect_walrus_targets_in_expr(c, out);
                    }
                }
                // Def/Class create their own scopes; don't descend.
                _ => {}
            }
        }
    }

    /// Compile a comprehension (list/set/dict) as a nested implicit function,
    /// mirroring CPython's scope-isolation behaviour.
    ///
    /// `iter_reg`   — register holding the outermost iterable (already compiled
    ///                in the enclosing scope by the caller).
    /// `fn_body`    — complete body of the inner function (init + loops + return).
    /// `comp_name`  — display name used in the `FnProto` ("listcomp", etc.).
    ///
    /// Returns the result register (holds the constructed collection after
    /// the implicit call returns).
    fn compile_collection_comp_impl(
        &mut self,
        iter_reg: Reg,
        fn_body: Vec<Stmt>,
        comp_name: &str,
        is_async: bool,
    ) -> Reg {
        const IT_PARAM: &str = ".0";

        let params = vec![FunctionParam {
            name: IT_PARAM.to_string(),
            default: None,
            annotation: None,
            is_args: false,
            is_kwargs: false,
            is_keyword_only: false,
            is_positional_only: false,
        }];

        // PEP 572: walrus targets inside a comprehension body belong to the
        // *enclosing* scope, not to the comprehension's implicit inner scope.
        // Collect them here so we can route writes to the right place:
        //   - target in enclosing function scope → inject as `nonlocal`
        //   - target in module/global scope      → inject as `global`
        let mut walrus_targets: HashSet<String> = HashSet::new();
        Self::collect_walrus_targets_in_stmts(&fn_body, &mut walrus_targets);

        // Determine which walrus targets live in an enclosing function scope.
        let walrus_nonlocal: HashSet<String> = walrus_targets
            .iter()
            .filter(|name| {
                self.outer_locals.iter().any(|m| m.contains_key(*name))
                    || (self.is_function_scope && self.local_index.contains_key(*name))
            })
            .cloned()
            .collect();

        // Walrus targets NOT in any enclosing function scope go through the
        // global (module) env instead.
        let walrus_global: HashSet<String> = walrus_targets
            .difference(&walrus_nonlocal)
            .cloned()
            .collect();

        let mut inner_global = crate::interpreter::collect_global_names(&fn_body);
        inner_global.extend(walrus_global);
        let mut inner_nonlocal = crate::interpreter::collect_nonlocal_names(&fn_body);
        inner_nonlocal.extend(walrus_nonlocal);

        // Build inner locals excluding any walrus targets (they belong to the
        // enclosing scope, not the comprehension's implicit function).
        let raw_inner_local = crate::interpreter::collect_local_names(
            &params,
            &fn_body,
            &inner_global,
            &inner_nonlocal,
        );
        // `collect_local_names` already excludes names in inner_global /
        // inner_nonlocal, but it does NOT know about walrus targets yet
        // (they were just added above). Filter them out explicitly.
        let inner_local: indexmap::IndexSet<String> = raw_inner_local
            .into_iter()
            .filter(|n| !walrus_targets.contains(n))
            .collect();

        let mut inner_index: HashMap<String, Reg> = HashMap::new();
        let mut slot: Reg = 0;
        for param in &params {
            if inner_local.contains(&param.name) {
                inner_index.insert(param.name.clone(), slot);
                slot += 1;
            }
        }
        for loc in &inner_local {
            if !inner_index.contains_key(loc) {
                inner_index.insert(loc.clone(), slot);
                slot += 1;
            }
        }
        let inner_index_rc: Rc<HashMap<String, Reg>> = Rc::new(inner_index);
        let def_bound = crate::interpreter::compute_def_bound_mask(&params, &inner_index_rc);
        let inner_cell_vars = collect_cell_vars(&fn_body, &inner_index_rc);
        let inner_global_rc = Rc::new(inner_global);
        let inner_nonlocal_rc = Rc::new(inner_nonlocal);

        // Validate nonlocal declarations (same as compile_def / compile_gen_exp).
        let mut sorted_nonlocals: Vec<&String> = inner_nonlocal_rc.iter().collect();
        sorted_nonlocals.sort();
        for nonlocal_name in sorted_nonlocals {
            let found = self
                .outer_locals
                .iter()
                .any(|m| m.contains_key(nonlocal_name))
                || (self.is_function_scope && self.local_index.contains_key(nonlocal_name));
            if !found {
                self.set_syntax_error(&format!(
                    "no binding for nonlocal '{}' found",
                    nonlocal_name
                ));
                return 0;
            }
        }

        let mut sub = Compiler::new(Rc::clone(&inner_index_rc), def_bound, inner_cell_vars);
        // Threaded source file (#2438): a lambda's code object inherits the
        // enclosing scope's `co_filename`.
        sub.filename = self.filename.clone();
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
        sub.future_annotations = self.future_annotations;
        // An async comprehension (`[x async for x in ait]`, #2283) compiles its
        // implicit function as a coroutine: the `async for` clauses lower to the
        // `__aiter__`/`await __anext__()` drive, and the enclosing async frame
        // awaits the returned coroutine (below).  No bare `yield` is emitted, so
        // `is_generator` stays false and this is a plain coroutine, not an async
        // generator (only `compile_gen_exp` produces async generators).
        sub.is_async_function = is_async;
        // Set comprehensions lower `.acc.add(elt)` to `Insn::SetAdd` (issue #1861).
        sub.is_set_comp = comp_name == "setcomp";
        // List comprehensions lower `.acc.append(elt)` to `Insn::ListAppend` (issue #1862).
        sub.is_list_comp = comp_name == "listcomp";
        // list / set / dict comprehensions are CPython-3.12-inlined scopes; an
        // unbound enclosing-local read inside them is `UnboundLocalError`, not the
        // free-variable `NameError` (issue #2340).  Generator expressions
        // (`compile_gen_exp`) are a real separate frame and stay free.
        sub.is_inlined_comp = true;
        // Record the locals of the comp's immediately-enclosing *real* function
        // (the PEP 709 inlining target) so the VM can tell an unbound read of one
        // of that frame's locals (`UnboundLocalError`) apart from a free variable
        // owned by a grandparent scope (`NameError`) — issue #2457.  When this
        // compiler is itself an inlined comp, the inlining target is the real
        // function further up, whose locals we already captured; inherit it.
        sub.comp_enclosing_locals = if self.is_inlined_comp {
            self.comp_enclosing_locals.clone()
        } else if self.is_function_scope {
            Some(Rc::new(self.local_index.keys().cloned().collect()))
        } else {
            None
        };
        sub.compile_block(&fn_body);
        let inner_code = match sub.finish() {
            Ok(c) => c,
            Err(e) => {
                self.failed = true;
                if matches!(e, PyError::Named(ref cls, _) if cls.as_ref() == "SyntaxError") {
                    self.is_syntax_error = true;
                }
                if self.error_msg.is_none() {
                    self.error_msg = Some(match e {
                        PyError::Named(_, msg) | PyError::Runtime(msg) => msg,
                        other => other.to_string(),
                    });
                }
                return 0;
            }
        };

        if self.fn_protos.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many functions in one scope (max 65535)".to_string());
            }
            return 0;
        }
        let proto_idx = self.fn_protos.len() as u16;
        let local_names = Rc::new(inner_index_rc.keys().cloned().collect::<HashSet<_>>());
        let display = format!("<{}>", comp_name);
        let param_spec = Rc::new(FnParamSpec {
            names: params.iter().map(|p| p.name.clone()).collect(),
            has_default: params.iter().map(|p| p.default.is_some()).collect(),
            is_args: params.iter().map(|p| p.is_args).collect(),
            is_kwargs: params.iter().map(|p| p.is_kwargs).collect(),
            is_keyword_only: params.iter().map(|p| p.is_keyword_only).collect(),
            is_positional_only: params.iter().map(|p| p.is_positional_only).collect(),
        });
        let param_binds = Rc::new(crate::bytecode::compute_param_binds(
            &param_spec,
            &inner_index_rc,
            &inner_code.cell_vars,
        ));
        let self_bind =
            crate::bytecode::compute_self_bind(&display, &inner_index_rc, &inner_code.cell_vars);
        self.fn_protos.push(FnProto {
            name: Rc::from(display.as_str()),
            qualname: Rc::from(display.as_str()),
            param_spec,
            code: Rc::new(inner_code),
            local_index: inner_index_rc,
            param_binds,
            self_bind,
            local_names,
            global_names: inner_global_rc,
            nonlocal_names: inner_nonlocal_rc,
            // Lambdas / comprehensions are never pure (fresh-closure identity).
            is_memo_pure: false,
            is_dce_pure: false,
            annotation_keys: SmallVec::new(),
            docstring: None,
            class_kwarg_names: SmallVec::new(),
        });

        // Emit MakeFunction + Call, same layout as compile_gen_exp.
        let fn_reg = self.alloc_temp();
        self.emit(Insn::MakeFunction(fn_reg, proto_idx, 0, 0, 0, 0));

        let arg_reg = fn_reg + 1;
        if arg_reg > self.max_reg {
            self.max_reg = arg_reg;
        }
        if fn_reg + 2 > self.next_temp {
            self.next_temp = fn_reg + 2;
        }
        self.emit(Insn::Move(arg_reg, iter_reg));
        self.emit(Insn::Call(fn_reg, 1));
        self.next_temp = fn_reg + 1;
        self.free_temp(iter_reg);

        if is_async {
            // The call produced a coroutine; the enclosing async frame awaits it
            // to materialise the collection (#2283).  A distinct result register
            // above `fn_reg` (the awaited source) receives the awaited value —
            // same register discipline as `compile_async_with`'s `__aenter__`
            // drive; the `GetAwaitable`/`YieldFrom` scratch temps sit above it.
            let result_reg = self.alloc_temp();
            self.emit_await_drive_into(fn_reg, result_reg);
            self.next_temp = result_reg + 1;
            return result_reg;
        }

        fn_reg
    }

    fn compile_list_comp(&mut self, elt: &Expr, clauses: &[CompClause]) -> Reg {
        if clauses.is_empty() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("list comprehension requires at least one clause".to_string());
            }
            return 0;
        }
        let is_async =
            clauses.iter().any(|c| c.is_async) || Self::comp_body_has_await(&[elt], clauses);
        if is_async && !self.is_async_function {
            self.set_syntax_error("asynchronous comprehension outside of an asynchronous function");
            return 0;
        }
        if let Some(msg) = check_comprehension(&[elt], clauses, "list comprehension") {
            self.set_syntax_error(&msg);
            return 0;
        }

        const ACC_NAME: &str = ".acc";

        // Evaluate the outermost iterable in the enclosing scope.
        let iter_reg = self.compile_expr(&clauses[0].iter);

        // Build the innermost statement: .acc.append(elt)
        let innermost = Stmt::Expr(Expr::Call {
            span: None,
            func: Box::new(Expr::Attr {
                target: Box::new(Expr::Var(ACC_NAME.to_string(), None)),
                name: "append".to_string(),
                span: None,
            }),
            args: vec![CallArg {
                name: None,
                value: elt.clone(),
                splat: false,
                double_splat: false,
            }],
        });

        // Build loop nest around the innermost statement.
        let mut fn_body = vec![Stmt::Assign(
            AssignTarget::Name(ACC_NAME.to_string()),
            Expr::List(vec![]),
        )];
        fn_body.extend(Self::build_comp_loop_body(clauses, innermost));
        fn_body.push(Stmt::Return(Some(Expr::Var(ACC_NAME.to_string(), None))));

        self.compile_collection_comp_impl(iter_reg, fn_body, "listcomp", is_async)
    }

    fn compile_dict_comp(&mut self, key: &Expr, val: &Expr, clauses: &[CompClause]) -> Reg {
        if clauses.is_empty() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("dict comprehension requires at least one clause".to_string());
            }
            return 0;
        }
        let is_async =
            clauses.iter().any(|c| c.is_async) || Self::comp_body_has_await(&[key, val], clauses);
        if is_async && !self.is_async_function {
            self.set_syntax_error("asynchronous comprehension outside of an asynchronous function");
            return 0;
        }

        if let Some(msg) = check_comprehension(&[key, val], clauses, "dict comprehension") {
            self.set_syntax_error(&msg);
            return 0;
        }

        const ACC_NAME: &str = ".acc";

        let iter_reg = self.compile_expr(&clauses[0].iter);

        // Build the innermost statement: .acc[key] = val
        let innermost = Stmt::IndexAssign {
            target: Box::new(Expr::Var(ACC_NAME.to_string(), None)),
            index: Box::new(key.clone()),
            expr: val.clone(),
        };

        let mut fn_body = vec![Stmt::Assign(
            AssignTarget::Name(ACC_NAME.to_string()),
            Expr::Dict(vec![]),
        )];
        fn_body.extend(Self::build_comp_loop_body(clauses, innermost));
        fn_body.push(Stmt::Return(Some(Expr::Var(ACC_NAME.to_string(), None))));

        self.compile_collection_comp_impl(iter_reg, fn_body, "dictcomp", is_async)
    }

    /// When compiling the implicit function body of a set comprehension
    /// (`self.is_set_comp`), recognize the synthesized accumulator add
    /// `.acc.add(elt)` and lower it directly to `Insn::SetAdd(acc, elt)`,
    /// skipping attribute resolution + method-call dispatch (issue #1861).
    ///
    /// Returns `true` when it handled `expr` (matching the exact synthesized
    /// shape: a positional-only one-arg call to `.add` on the reserved `.acc`
    /// accumulator name). The `.acc` name is compiler-internal and cannot
    /// appear in user source, so this never intercepts a user `x.add(y)`.
    fn try_emit_set_comp_add(&mut self, expr: &Expr) -> bool {
        if !self.is_set_comp {
            return false;
        }
        let Expr::Call { func, args, .. } = expr else {
            return false;
        };
        let Expr::Attr { target, name, .. } = func.as_ref() else {
            return false;
        };
        if name != "add" || !matches!(target.as_ref(), Expr::Var(v, _) if v == ".acc") {
            return false;
        }
        if args.len() != 1 {
            return false;
        }
        let arg = &args[0];
        if arg.name.is_some() || arg.splat || arg.double_splat {
            return false;
        }
        let acc_reg = self.compile_expr(target);
        let elt_reg = self.compile_expr(&arg.value);
        self.emit(Insn::SetAdd(acc_reg, elt_reg));
        self.free_temp(elt_reg);
        self.free_temp(acc_reg);
        true
    }

    /// When compiling the implicit function body of a list comprehension
    /// (`self.is_list_comp`), recognize the synthesized accumulator append
    /// `.acc.append(elt)` and lower it directly to `Insn::ListAppend(acc, elt)`,
    /// skipping attribute resolution + method-call dispatch (issue #1862).
    ///
    /// Returns `true` when it handled `expr` (matching the exact synthesized
    /// shape: a positional-only one-arg call to `.append` on the reserved
    /// `.acc` accumulator name). The `.acc` name is compiler-internal and
    /// cannot appear in user source, so this never intercepts a user
    /// `x.append(y)`.
    fn try_emit_list_comp_append(&mut self, expr: &Expr) -> bool {
        if !self.is_list_comp {
            return false;
        }
        let Expr::Call { func, args, .. } = expr else {
            return false;
        };
        let Expr::Attr { target, name, .. } = func.as_ref() else {
            return false;
        };
        if name != "append" || !matches!(target.as_ref(), Expr::Var(v, _) if v == ".acc") {
            return false;
        }
        if args.len() != 1 {
            return false;
        }
        let arg = &args[0];
        if arg.name.is_some() || arg.splat || arg.double_splat {
            return false;
        }
        let acc_reg = self.compile_expr(target);
        let elt_reg = self.compile_expr(&arg.value);
        self.emit(Insn::ListAppend(acc_reg, elt_reg));
        self.free_temp(elt_reg);
        self.free_temp(acc_reg);
        true
    }

    fn compile_set_comp(&mut self, elt: &Expr, clauses: &[CompClause]) -> Reg {
        if clauses.is_empty() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("set comprehension requires at least one clause".to_string());
            }
            return 0;
        }
        let is_async =
            clauses.iter().any(|c| c.is_async) || Self::comp_body_has_await(&[elt], clauses);
        if is_async && !self.is_async_function {
            self.set_syntax_error("asynchronous comprehension outside of an asynchronous function");
            return 0;
        }

        if let Some(msg) = check_comprehension(&[elt], clauses, "set comprehension") {
            self.set_syntax_error(&msg);
            return 0;
        }

        const ACC_NAME: &str = ".acc";

        let iter_reg = self.compile_expr(&clauses[0].iter);

        // Build the innermost statement: .acc.add(elt)
        let innermost = Stmt::Expr(Expr::Call {
            span: None,
            func: Box::new(Expr::Attr {
                target: Box::new(Expr::Var(ACC_NAME.to_string(), None)),
                name: "add".to_string(),
                span: None,
            }),
            args: vec![CallArg {
                name: None,
                value: elt.clone(),
                splat: false,
                double_splat: false,
            }],
        });

        // Build the accumulator: .acc = set()
        let acc_init = Expr::Call {
            span: None,
            func: Box::new(Expr::Var("set".to_string(), None)),
            args: vec![],
        };
        let mut fn_body = vec![Stmt::Assign(
            AssignTarget::Name(ACC_NAME.to_string()),
            acc_init,
        )];
        fn_body.extend(Self::build_comp_loop_body(clauses, innermost));
        fn_body.push(Stmt::Return(Some(Expr::Var(ACC_NAME.to_string(), None))));

        self.compile_collection_comp_impl(iter_reg, fn_body, "setcomp", is_async)
    }

    /// Compile a generator expression `(elt for target in iter ...)`.
    ///
    /// Strategy (mirrors CPython):
    ///   1. Evaluate the outermost iterable in the enclosing scope.
    ///   2. Compile an anonymous generator function whose parameter receives
    ///      that iterable and whose body iterates over it, handling nested
    ///      clauses, and yields `elt` on each matching element.
    ///   3. Emit `MakeFunction` directly into a temp register — no global
    ///      store/reload — then call it immediately with the iterable.
    fn compile_gen_exp(&mut self, elt: &Expr, clauses: &[CompClause]) -> Reg {
        if clauses.is_empty() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("generator expression requires at least one clause".to_string());
            }
            return 0;
        }
        // An `async for` clause makes this an *async generator* expression
        // (#2283): `(x async for x in ait)`.  Unlike a collection comprehension
        // it does NOT require an enclosing async function — it constructs an
        // `async_generator` object anywhere, just like an `async def` with a
        // bare `yield` (#2280).
        // An `await` in the element/cond/non-outermost iter (without an
        // `async for`) also makes the genexp an async generator (#2304):
        // `(await f(x) for x in xs)` — matches CPython, which produces an
        // `async_generator` object. Unlike collection comprehensions this does
        // not require an enclosing async function.
        let is_async =
            clauses.iter().any(|c| c.is_async) || Self::comp_body_has_await(&[elt], clauses);
        if let Some(msg) = check_comprehension(&[elt], clauses, "generator expression") {
            self.set_syntax_error(&msg);
            return 0;
        }

        // Evaluate the outermost iterable in the enclosing (current) scope
        // before creating the nested function.
        let iter_reg = self.compile_expr(&clauses[0].iter);

        // Use ".0" as the implicit parameter name — matches CPython's internal
        // convention and is not a valid Python identifier (cannot be lexed), so
        // user code inside the genexp body cannot accidentally reference or
        // shadow it.
        const IT_PARAM: &str = ".0";

        // Build the inner body working from the innermost clause outward:
        // `yield elt` wrapped in the clause loop nest (shared with the
        // collection comprehensions, so `async for` clauses thread through to
        // `compile_async_for` identically — #2283).
        let yield_stmt = Stmt::Expr(Expr::Yield(Some(Box::new(elt.clone()))));
        let body = Self::build_comp_loop_body(clauses, yield_stmt);

        // Parameter spec for the anonymous generator function.
        let params = vec![FunctionParam {
            name: IT_PARAM.to_string(),
            default: None,
            annotation: None,
            is_args: false,
            is_kwargs: false,
            is_keyword_only: false,
            is_positional_only: false,
        }];

        // Inline the function-object construction (mirrors compile_def) but
        // emit MakeFunction directly into a temp register instead of binding
        // the function into the global/local environment.  This avoids the
        // observable StoreGlobal("<genexp>") / LoadGlobal("<genexp>") pair.
        let inner_global = crate::interpreter::collect_global_names(&body);
        let inner_nonlocal = crate::interpreter::collect_nonlocal_names(&body);
        let inner_local =
            crate::interpreter::collect_local_names(&params, &body, &inner_global, &inner_nonlocal);

        let mut inner_index: HashMap<String, Reg> = HashMap::new();
        let mut slot: Reg = 0;
        for param in &params {
            if inner_local.contains(&param.name) {
                inner_index.insert(param.name.clone(), slot);
                slot += 1;
            }
        }
        for loc in &inner_local {
            if !inner_index.contains_key(loc) {
                inner_index.insert(loc.clone(), slot);
                slot += 1;
            }
        }
        let inner_index_rc: Rc<HashMap<String, Reg>> = Rc::new(inner_index);
        let def_bound = crate::interpreter::compute_def_bound_mask(&params, &inner_index_rc);
        // Genexp bodies are never pure (they produce a generator object with
        // side-effectful iteration).
        let inner_cell_vars = collect_cell_vars(&body, &inner_index_rc);
        let inner_global_rc = Rc::new(inner_global);
        let inner_nonlocal_rc = Rc::new(inner_nonlocal);

        // Validate nonlocal declarations in comprehension bodies (same as compile_def).
        let mut sorted_nonlocals: Vec<&String> = inner_nonlocal_rc.iter().collect();
        sorted_nonlocals.sort();
        for nonlocal_name in sorted_nonlocals {
            let found = self
                .outer_locals
                .iter()
                .any(|m| m.contains_key(nonlocal_name))
                || (self.is_function_scope && self.local_index.contains_key(nonlocal_name));
            if !found {
                self.set_syntax_error(&format!(
                    "no binding for nonlocal '{}' found",
                    nonlocal_name
                ));
                return 0;
            }
        }

        let mut sub = Compiler::new(Rc::clone(&inner_index_rc), def_bound, inner_cell_vars);
        // Threaded source file (#2438): the comprehension's implicit code object
        // inherits the enclosing scope's `co_filename`.
        sub.filename = self.filename.clone();
        // Comprehensions create an implicit function scope; thread outer_locals.
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
        sub.future_annotations = self.future_annotations;
        // An async generator expression (`(x async for x in ait)`): the bare
        // `yield elt` plus `is_async_function` makes the synthesized function an
        // async generator (#2280/#2283).  The result is the async-gen object —
        // NOT awaited here, unlike a collection comprehension.
        sub.is_async_function = is_async;
        sub.compile_block(&body);
        let inner_code = match sub.finish() {
            Ok(c) => c,
            Err(e) => {
                self.failed = true;
                if matches!(e, PyError::Named(ref cls, _) if cls.as_ref() == "SyntaxError") {
                    self.is_syntax_error = true;
                }
                if self.error_msg.is_none() {
                    self.error_msg = Some(match e {
                        PyError::Named(_, msg) | PyError::Runtime(msg) => msg,
                        other => other.to_string(),
                    });
                }
                return 0;
            }
        };

        if self.fn_protos.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many functions in one scope (max 65535)".to_string());
            }
            return 0;
        }
        let proto_idx = self.fn_protos.len() as u16;
        let local_names = Rc::new(inner_index_rc.keys().cloned().collect::<HashSet<_>>());
        let param_spec = Rc::new(FnParamSpec {
            names: params.iter().map(|p| p.name.clone()).collect(),
            has_default: params.iter().map(|p| p.default.is_some()).collect(),
            is_args: params.iter().map(|p| p.is_args).collect(),
            is_kwargs: params.iter().map(|p| p.is_kwargs).collect(),
            is_keyword_only: params.iter().map(|p| p.is_keyword_only).collect(),
            is_positional_only: params.iter().map(|p| p.is_positional_only).collect(),
        });
        let param_binds = Rc::new(crate::bytecode::compute_param_binds(
            &param_spec,
            &inner_index_rc,
            &inner_code.cell_vars,
        ));
        let self_bind =
            crate::bytecode::compute_self_bind("<genexpr>", &inner_index_rc, &inner_code.cell_vars);
        self.fn_protos.push(FnProto {
            name: Rc::from("<genexpr>"),
            qualname: Rc::from("<genexpr>"),
            param_spec,
            code: Rc::new(inner_code),
            local_index: inner_index_rc,
            param_binds,
            self_bind,
            local_names,
            global_names: inner_global_rc,
            nonlocal_names: inner_nonlocal_rc,
            // Lambdas / comprehensions are never pure (fresh-closure identity).
            is_memo_pure: false,
            is_dce_pure: false,
            annotation_keys: SmallVec::new(),
            docstring: None,
            class_kwarg_names: SmallVec::new(),
        });

        // Allocate a temp for the function value, emit MakeFunction (no
        // defaults or annotations — the single parameter has no default and
        // genexp params carry no annotations), then call it.
        // Layout: fn_reg = function, fn_reg+1 = iterable arg.
        let fn_reg = self.alloc_temp();
        self.emit(Insn::MakeFunction(fn_reg, proto_idx, 0, 0, 0, 0));

        let arg_reg = fn_reg + 1;
        if arg_reg > self.max_reg {
            self.max_reg = arg_reg;
        }
        if fn_reg + 2 > self.next_temp {
            self.next_temp = fn_reg + 2;
        }
        self.emit(Insn::Move(arg_reg, iter_reg));
        self.emit(Insn::Call(fn_reg, 1));
        // Release the arg register slot; fn_reg stays live as the result.
        self.next_temp = fn_reg + 1;
        self.free_temp(iter_reg);

        fn_reg
    }

    fn compile_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        // PEP 657 caret anchor (#2411 / #2443) for the whole `callee(...)` span.
        // Armed immediately before the terminal call instruction on the simple
        // positional, keyword, and method paths so an error propagated through
        // any of these draws its caret on the call site (#2443 stage 2 lists
        // `a.b()` / `f(a=5)` explicitly).  Splat paths (`f(*a)` / `f(**d)`) stay
        // caret-free (safe — a missing caret beats a wrong one).
        span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        // Check for any splat args — these require a variadic call path.
        let has_splat = args.iter().any(|a| a.splat || a.double_splat);
        let has_kwargs = args.iter().any(|a| a.name.is_some());

        // Keyword call with no `*args` / `**kwargs` splats and a non-method
        // callee: lay the arguments out contiguously in registers and emit a
        // `CallKw` (issue #2382), skipping the BuildDict + BuildList + __vcall__
        // round-trip the generic variadic path uses.  Method keyword calls
        // (`obj.m(a=1)`) still take the variadic path so in-place mutation of
        // the receiver register is preserved.
        if has_kwargs && !has_splat && !matches!(func, Expr::Attr { .. }) {
            return self.compile_keyword_call(func, args, span);
        }

        // Keyword method call `obj.m(<pos…>, k=v…)` with no splats (issue #2392).
        // Lay the receiver and arguments out in registers and emit a
        // `CallMethodKw`, which reuses the `CallMethod` inline cache + the
        // `CallKw` keyword fast-bind (receiver → param 0) instead of the
        // BuildList + BuildDict + `CallMethodExpanded` round-trip.  In-place
        // mutation of a fast-local receiver register is preserved exactly as in
        // `compile_method_call` (same `obj_reg`/`dst_reg` placement).
        if has_kwargs
            && !has_splat
            && let Expr::Attr { target, name, .. } = func
        {
            return self.compile_keyword_method_call(target, name, args, span);
        }

        // Double-splat expansion `f(<pos…>, **d)` (issue #2393): exactly one
        // trailing `**d`, every preceding arg a plain positional (no `*a` splat,
        // no literal keyword), non-method callee.  Lower to `CallEx`, which binds
        // straight from the splat dict via a monomorphic shape cache instead of
        // copying the dict and round-tripping through `__vcall__`.
        if let Some(npos) = double_splat_fast_shape(args)
            && npos <= u8::MAX as usize
            && !matches!(func, Expr::Attr { .. })
        {
            return self.compile_double_splat_call(func, args, npos);
        }

        if has_splat || has_kwargs {
            // Variadic call: build separate positional and keyword lists, then
            // use the ExpandedCall instruction.
            return self.compile_variadic_call(func, args);
        }

        // Detect obj.method(args) — emit CallMethod to allow in-place mutation.
        if let Expr::Attr { target, name, .. } = func {
            return self.compile_method_call(target, name, args, span);
        }

        let argc = args.len() as u8;
        let func_reg = self.next_temp;
        let frame_top = func_reg.wrapping_add(1).wrapping_add(Reg::from(argc));
        if frame_top < func_reg {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = frame_top;
        if frame_top > 0 && frame_top - 1 > self.max_reg {
            self.max_reg = frame_top - 1;
        }
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        for (i, arg) in (0u32..).zip(args.iter()) {
            let arg_reg = func_reg + 1 + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        let is_pure_callee =
            matches!(func, Expr::Var(n, _) if self.pure_locals.contains(n.as_str()));
        // Arm the `callee(...)` caret anchor on the terminal call instruction
        // (#2411); `emit` consumes and clears it.
        self.set_col_span_for_next(span);
        if is_pure_callee {
            self.emit(Insn::CallMemo(func_reg, argc));
        } else {
            self.emit(Insn::Call(func_reg, argc));
        }
        self.next_temp = func_reg + 1;
        func_reg
    }

    /// Compile a keyword-argument call (no splats, non-method callee) into a
    /// `CallKw` instruction (issue #2382).  Arguments are evaluated left-to-right
    /// (Python order) into contiguous registers `func_reg+1 .. func_reg+1+total`,
    /// positionals first then keyword values; the keyword names form a
    /// constant-pool tuple consumed by the runtime binder.
    ///
    /// Python's grammar guarantees every positional argument precedes every
    /// keyword argument in a call, so source order already lays out positionals
    /// before keyword values — no reordering of evaluation is needed.
    fn compile_keyword_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let total = args.len();
        if total > u8::MAX as usize {
            // Too many args to encode in a u8 — fall back to the generic path.
            return self.compile_variadic_call(func, args);
        }
        let nkw = args.iter().filter(|a| a.name.is_some()).count();

        // Build the keyword-names tuple constant (in source order, matching the
        // order the keyword values occupy in the register window).
        let kw_names: Vec<Value> = args
            .iter()
            .filter_map(|a| a.name.as_ref())
            .map(|n| Value::string(n.clone()))
            .collect();
        let kwnames_idx = self.intern_const(Value::tuple(kw_names));

        let func_reg = self.next_temp;
        let frame_top = func_reg.wrapping_add(1).wrapping_add(total as Reg);
        if frame_top < func_reg {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = frame_top;
        if frame_top > 0 && frame_top - 1 > self.max_reg {
            self.max_reg = frame_top - 1;
        }
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        for (i, arg) in (0u32..).zip(args.iter()) {
            let arg_reg = func_reg + 1 + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        // Arm the `callee(...)` caret anchor on the terminal call (#2443);
        // `emit` consumes and clears it.
        self.set_col_span_for_next(span);
        self.emit(Insn::CallKw {
            func: func_reg,
            total: total as u8,
            nkw: nkw as u8,
            kwnames_idx,
        });
        self.next_temp = func_reg + 1;
        func_reg
    }

    /// Compile a keyword-argument method call `obj.m(<pos…>, k=v…)` (no splats)
    /// into a `CallMethodKw` instruction (issue #2392).  The receiver register
    /// placement matches `compile_method_call` exactly (fast-local receivers use
    /// their own register as `obj` so in-place mutation persists; the result goes
    /// to a distinct `dst`).  Arguments are laid out contiguously in
    /// `R[args_base .. args_base+total]` — positionals first, then keyword values
    /// in source order — exactly as `compile_keyword_call` does; the keyword names
    /// form a constant-pool tuple consumed by the runtime binder.
    ///
    /// Python's grammar guarantees every positional argument precedes every
    /// keyword argument in a call, so source order already lays out positionals
    /// before keyword values — no reordering of evaluation is needed.
    fn compile_keyword_method_call(
        &mut self,
        target: &Expr,
        method_name: &str,
        args: &[crate::ast::CallArg],
        span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let total = args.len();
        if total > u8::MAX as usize {
            // Too many args to encode in a u8 — fall back to the generic path.
            return self.compile_variadic_call(
                &Expr::Attr {
                    target: Box::new(target.clone()),
                    name: method_name.to_string(),
                    span: None,
                },
                args,
            );
        }
        let nkw = args.iter().filter(|a| a.name.is_some()).count();
        let total_reg = total as Reg;

        // Build the keyword-names tuple constant (source order, matching the order
        // the keyword values occupy in the register window).
        let kw_names: Vec<Value> = args
            .iter()
            .filter_map(|a| a.name.as_ref())
            .map(|n| Value::string(n.clone()))
            .collect();
        let kwnames_idx = self.intern_const(Value::tuple(kw_names));

        // Receiver / dst / args_base placement — identical to compile_method_call.
        let (obj_reg, dst_reg, args_base, need_copy) = if let Expr::Var(name, _) = target {
            if let Some(local) = self.local_reg(name) {
                let dst = self.next_temp;
                let abase = dst.wrapping_add(1);
                let frame_top = abase.wrapping_add(total_reg);
                if frame_top < dst {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("call frame register overflow".to_string());
                    }
                    return 0;
                }
                self.next_temp = frame_top;
                if frame_top > 0 && frame_top - 1 > self.max_reg {
                    self.max_reg = frame_top - 1;
                }
                (local, dst, abase, false)
            } else {
                let o = self.next_temp;
                let abase = o.wrapping_add(1);
                let frame_top = abase.wrapping_add(total_reg);
                if frame_top < o {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("call frame register overflow".to_string());
                    }
                    return 0;
                }
                self.next_temp = frame_top;
                if frame_top > 0 && frame_top - 1 > self.max_reg {
                    self.max_reg = frame_top - 1;
                }
                (o, o, abase, true)
            }
        } else {
            let o = self.next_temp;
            let abase = o.wrapping_add(1);
            let frame_top = abase.wrapping_add(total_reg);
            if frame_top < o {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("call frame register overflow".to_string());
                }
                return 0;
            }
            self.next_temp = frame_top;
            if frame_top > 0 && frame_top - 1 > self.max_reg {
                self.max_reg = frame_top - 1;
            }
            (o, o, abase, true)
        };

        if need_copy {
            let saved = self.next_temp;
            self.compile_expr_into(target, obj_reg);
            self.next_temp = saved;
        }

        for (i, arg) in (0u32..).zip(args.iter()) {
            let arg_reg = args_base + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        let name_idx = self.intern_name(method_name);
        // Arm the `obj.m(...)` caret anchor on the terminal call (#2443);
        // `emit` consumes and clears it.
        self.set_col_span_for_next(span);
        self.emit(Insn::CallMethodKw {
            dst: dst_reg,
            obj: obj_reg,
            name_idx,
            args_base,
            total: total as u8,
            nkw: nkw as u8,
            kwnames_idx,
        });
        self.next_temp = dst_reg + 1;
        dst_reg
    }

    fn compile_method_call(
        &mut self,
        target: &Expr,
        method_name: &str,
        args: &[crate::ast::CallArg],
        span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let nargs = args.len() as u8;

        // When the receiver is a plain fast-local variable, use its register directly
        // as `obj` so that in-place mutations (append, pop, …) actually update the
        // variable.  The return value goes into a fresh temp `dst_reg ≠ obj_reg`.
        // For all other receivers we fall back to copying the value into a temp and
        // using the same register for both obj and dst.
        let (obj_reg, dst_reg, args_base, need_copy) = if let Expr::Var(name, _) = target {
            if let Some(local) = self.local_reg(name) {
                let dst = self.next_temp;
                let abase = dst.wrapping_add(1);
                let frame_top = abase.wrapping_add(Reg::from(nargs));
                if frame_top < dst {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("call frame register overflow".to_string());
                    }
                    return 0;
                }
                self.next_temp = frame_top;
                if frame_top > 0 && frame_top - 1 > self.max_reg {
                    self.max_reg = frame_top - 1;
                }
                (local, dst, abase, false)
            } else {
                // cell / nonlocal — must load via env first
                let o = self.next_temp;
                let abase = o.wrapping_add(1);
                let frame_top = abase.wrapping_add(Reg::from(nargs));
                if frame_top < o {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("call frame register overflow".to_string());
                    }
                    return 0;
                }
                self.next_temp = frame_top;
                if frame_top > 0 && frame_top - 1 > self.max_reg {
                    self.max_reg = frame_top - 1;
                }
                (o, o, abase, true)
            }
        } else {
            let o = self.next_temp;
            let abase = o.wrapping_add(1);
            let frame_top = abase.wrapping_add(Reg::from(nargs));
            if frame_top < o {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("call frame register overflow".to_string());
                }
                return 0;
            }
            self.next_temp = frame_top;
            if frame_top > 0 && frame_top - 1 > self.max_reg {
                self.max_reg = frame_top - 1;
            }
            (o, o, abase, true)
        };

        if need_copy {
            let saved = self.next_temp;
            self.compile_expr_into(target, obj_reg);
            self.next_temp = saved;
        }

        for (i, arg) in (0u32..).zip(args.iter()) {
            let arg_reg = args_base + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        let name_idx = self.intern_name(method_name);
        // Arm the `obj.m(...)` caret anchor on the terminal call (#2443);
        // `emit` consumes and clears it.
        self.set_col_span_for_next(span);
        self.emit(Insn::CallMethod {
            dst: dst_reg,
            obj: obj_reg,
            name_idx,
            args_base,
            nargs,
        });
        self.next_temp = dst_reg + 1;
        dst_reg
    }

    fn compile_variadic_call(&mut self, func: &Expr, args: &[crate::ast::CallArg]) -> Reg {
        // For variadic/keyword calls, we build a list of positional args and a
        // dict of keyword args, then use a special "expanded call" mechanism.
        // Since the current VM Call instruction only handles simple positional
        // args, we fall back to calling via the `__call__` convention by building
        // lists/dicts and using a runtime helper.
        //
        // Simplified: collect positional args into a list and keyword args into
        // a dict, then call the runtime built-in "apply" helper.
        // For now, compile as: func(*args, **kwargs) using the ExpandedCall route.
        //
        // The approach: build a list of positional args and a dict of kwargs,
        // then use a special instruction (or just let the normal Call handle it
        // by putting them in the right registers).
        //
        // Actually, to avoid a new instruction, we compile this as:
        //   _call_helper(func, positional_list, kwargs_dict)
        // where _call_helper is a built-in that expands and calls.
        //
        // Better: use ExpandedCall instruction. For now, emit the positional
        // args normally and mark the call as variadic.
        //
        // SIMPLIFICATION: compile splat/kwargs calls by materializing all args
        // and using a special VM call path via a built-in helper function.
        // The interpreter's call_function_expanded handles this already.
        //
        // We encode a variadic call as: emit args into consecutive registers
        // including splat markers, then emit a special extended Call instruction.
        // Since we don't have that instruction, use a simpler encoding:
        // build a list for *args and a dict for **kwargs.

        // Strategy: compile func, then for each arg:
        // - if splat: use list.extend
        // - if double_splat: use dict.update
        // - if named: add to kwargs dict
        // - else: add to positional list
        // Then call via "built-in" expanded call.

        // Simpler approach: just pack everything into two registers (pos_list, kw_dict)
        // and use the existing call infrastructure.

        // For *args: args.iter().flat_map expand
        // For **kwargs: dict from kwargs

        // Detect obj.method(*args, **kwargs) — emit CallMethodExpanded.
        if let Expr::Attr { target, name, .. } = func {
            // Same fast-local optimisation as compile_method_call: use the
            // variable's own register as `obj` so mutations persist.
            let (obj_reg, dst_reg) = if let Expr::Var(tname, _) = target.as_ref() {
                if let Some(local) = self.local_reg(tname) {
                    let dst = self.alloc_temp();
                    (local, dst)
                } else {
                    let o = self.alloc_temp();
                    self.compile_expr_into(target, o);
                    (o, o)
                }
            } else {
                let o = self.alloc_temp();
                self.compile_expr_into(target, o);
                (o, o)
            };
            let name_idx = self.intern_name(name);

            let pos_list_reg = self.alloc_temp();
            let empty_list_base = self.next_temp;
            self.next_temp = empty_list_base + 1;
            if empty_list_base > self.max_reg {
                self.max_reg = empty_list_base;
            }
            self.emit(Insn::BuildList(pos_list_reg, empty_list_base, 0));

            let kw_dict_reg = self.alloc_temp();
            let empty_dict_base = self.next_temp;
            self.next_temp = empty_dict_base + 1;
            if empty_dict_base > self.max_reg {
                self.max_reg = empty_dict_base;
            }
            self.emit(Insn::BuildDict(kw_dict_reg, empty_dict_base, 0));

            let has_kw_splat = args.iter().any(|a| a.double_splat);
            for arg in args {
                if arg.splat {
                    let val = self.compile_expr(&arg.value);
                    self.emit(Insn::ListExtend(pos_list_reg, val));
                    self.free_temp(val);
                } else if arg.double_splat {
                    let val = self.compile_expr(&arg.value);
                    // The callee is `obj.<name>`; its qualname is derived from
                    // the receiver's class on the error path.
                    self.emit(Insn::DictMergeKwCall {
                        dict: kw_dict_reg,
                        src: val,
                        name: crate::bytecode::KwCallName::Method {
                            obj: obj_reg,
                            name_idx,
                        },
                    });
                    self.free_temp(val);
                } else if let Some(kw_name) = &arg.name {
                    let val = self.compile_expr(&arg.value);
                    let key_idx = self.intern_const(Value::string(kw_name.clone()));
                    let key_reg = self.alloc_temp();
                    self.emit(Insn::LoadConst(key_reg, key_idx));
                    if has_kw_splat {
                        self.emit(Insn::SetItemKwCall {
                            dict: kw_dict_reg,
                            key: key_reg,
                            val,
                            name: crate::bytecode::KwCallName::Method {
                                obj: obj_reg,
                                name_idx,
                            },
                        });
                    } else {
                        self.emit(Insn::SetItem(kw_dict_reg, key_reg, val));
                    }
                    self.free_temp(key_reg);
                    self.free_temp(val);
                } else {
                    let val = self.compile_expr(&arg.value);
                    self.emit(Insn::ListAppend(pos_list_reg, val));
                    self.free_temp(val);
                }
            }

            self.emit(Insn::CallMethodExpanded {
                dst: dst_reg,
                obj: obj_reg,
                name_idx,
                pos_list: pos_list_reg,
                kw_dict: kw_dict_reg,
            });
            self.free_temp(kw_dict_reg);
            self.free_temp(pos_list_reg);
            self.next_temp = dst_reg + 1;
            return dst_reg;
        }

        let func_reg = self.alloc_temp();
        self.compile_expr_into(func, func_reg);

        // Build positional list
        let pos_list_reg = self.alloc_temp();
        // Use: pos_list = []  → then extend/append
        let empty_list_base = self.next_temp;
        if empty_list_base.checked_add(1).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = empty_list_base + 1;
        if empty_list_base > self.max_reg {
            self.max_reg = empty_list_base;
        }
        self.emit(Insn::BuildList(pos_list_reg, empty_list_base, 0));

        // Build kwargs dict
        let kw_dict_reg = self.alloc_temp();
        let empty_dict_base = self.next_temp;
        if empty_dict_base.checked_add(1).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = empty_dict_base + 1;
        if empty_dict_base > self.max_reg {
            self.max_reg = empty_dict_base;
        }
        self.emit(Insn::BuildDict(kw_dict_reg, empty_dict_base, 0));

        // When the call mixes a `**d` splat with other keyword sources, a key
        // present in two of them is a `TypeError` in CPython (DICT_MERGE), not a
        // silent overwrite.  Route kwargs through the duplicate-checking
        // instructions only in that case so the common no-splat call is
        // untouched.  `func_reg` carries the callee for the error's qualname.
        let has_kw_splat = args.iter().any(|a| a.double_splat);
        for arg in args {
            if arg.splat {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::ListExtend(pos_list_reg, val));
                self.free_temp(val);
            } else if arg.double_splat {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::DictMergeKwCall {
                    dict: kw_dict_reg,
                    src: val,
                    name: crate::bytecode::KwCallName::Callee(func_reg),
                });
                self.free_temp(val);
            } else if let Some(kw_name) = &arg.name {
                let val = self.compile_expr(&arg.value);
                let key_idx = self.intern_const(Value::string(kw_name.clone()));
                let key_reg = self.alloc_temp();
                self.emit(Insn::LoadConst(key_reg, key_idx));
                if has_kw_splat {
                    self.emit(Insn::SetItemKwCall {
                        dict: kw_dict_reg,
                        key: key_reg,
                        val,
                        name: crate::bytecode::KwCallName::Callee(func_reg),
                    });
                } else {
                    self.emit(Insn::SetItem(kw_dict_reg, key_reg, val));
                }
                self.free_temp(key_reg);
                self.free_temp(val);
            } else {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::ListAppend(pos_list_reg, val));
                self.free_temp(val);
            }
        }

        // Emit a ExpandedCall using the __pyrust_vcall__ builtin.
        // Actually, the cleanest way: use a built-in "apply" that takes
        // (func, pos_list, kw_dict). This requires a new built-in or instruction.
        //
        // For now, emit via a 3-arg call to "__vcall__" which the interpreter
        // knows how to dispatch.
        let vcall_name_idx = self.intern_name("__vcall__");
        let vcall_reg = self.alloc_temp();
        self.emit(Insn::LoadGlobal(vcall_reg, vcall_name_idx));
        // vcall_reg+1 = func_reg
        // vcall_reg+2 = pos_list_reg
        // vcall_reg+3 = kw_dict_reg
        // We need these in consecutive registers vcall_reg+1..vcall_reg+4
        let f1 = self.alloc_temp();
        let f2 = self.alloc_temp();
        let f3 = self.alloc_temp();
        self.emit(Insn::Move(f1, func_reg));
        self.emit(Insn::Move(f2, pos_list_reg));
        self.emit(Insn::Move(f3, kw_dict_reg));
        self.emit(Insn::Call(vcall_reg, 3));
        // Keep next_temp at vcall_reg+1 so the result register stays live.
        // This matches compile_call's convention: the returned register is NOT
        // freed, so callers can safely alloc_temp() without aliasing it.
        self.next_temp = vcall_reg + 1;
        // Return value is in vcall_reg
        vcall_reg
    }

    /// Compile a double-splat call `f(<pos…>, **d)` (issue #2393, shape vetted by
    /// [`double_splat_fast_shape`]) into a `CallEx` instruction.  Positionals fill
    /// `R[func+1 .. func+1+npos]` contiguously (as for `CallKw`); the single `**d`
    /// source dict is evaluated into a separate `kwargs` register above the
    /// positional window.  The runtime binder reads the dict directly — no
    /// BuildDict/DictUpdate copy and no `__vcall__` round-trip.
    fn compile_double_splat_call(
        &mut self,
        func: &Expr,
        args: &[crate::ast::CallArg],
        npos: usize,
    ) -> Reg {
        let func_reg = self.next_temp;
        // Reserve func + npos positional registers contiguously, then one more
        // for the `**d` dict.
        let frame_top = func_reg
            .wrapping_add(1)
            .wrapping_add(npos as Reg)
            .wrapping_add(1);
        if frame_top < func_reg {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("call frame register overflow".to_string());
            }
            return 0;
        }
        self.next_temp = frame_top;
        if frame_top > 0 && frame_top - 1 > self.max_reg {
            self.max_reg = frame_top - 1;
        }
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        // Positionals into the contiguous window (source order; every arg but the
        // trailing `**d` is a plain positional — guaranteed by the shape check).
        for (i, arg) in (0u32..).zip(args[..npos].iter()) {
            let arg_reg = func_reg + 1 + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        // The `**d` source mapping into the dedicated kwargs register.
        let kwargs_reg = func_reg + 1 + npos as Reg;
        let saved = self.next_temp;
        let insn_before = self.insns.len();
        let r = self.compile_expr(&args[npos].value);
        if r != kwargs_reg {
            let single = self.insns.len() == insn_before + 1;
            if single && r >= self.base_temp && self.retarget_last(r, kwargs_reg) {
                // retargeted in place — no Move needed
            } else {
                self.emit(Insn::Move(kwargs_reg, r));
            }
        }
        self.next_temp = saved;
        self.emit(Insn::CallEx {
            func: func_reg,
            npos: npos as u8,
            kwargs: kwargs_reg,
        });
        self.next_temp = func_reg + 1;
        func_reg
    }

    fn compile_lambda(&mut self, params: &[FunctionParam], body: &Expr) -> Reg {
        // Convert lambda body into an implicit return statement.
        let body_stmts = vec![Stmt::Return(Some(body.clone()))];
        let temp_name = "<lambda>";
        // A lambda's `co_firstlineno` is the source line it appears on; use the
        // statement line currently being compiled (issue #2185).
        let lambda_lineno = self.current_lineno;
        self.compile_def(
            temp_name,
            params,
            &body_stmts,
            &[],
            lambda_lineno,
            &[],
            None,
            false,
            &[],
        );
        // compile_def stored the result in local or global named "<lambda>".
        // We need to return the register it's in.
        // Actually compile_def uses compile_store_name which may put it in a
        // global. We need to load it back.
        let name_idx = self.intern_name(temp_name);
        let dst = self.alloc_temp();
        self.emit(Insn::LoadGlobal(dst, name_idx));
        dst
    }

    fn compile_collection(&mut self, items: &[Expr], is_tuple: bool) -> Reg {
        let n = items.len() as Reg;
        let base = self.next_temp;
        if base.checked_add(n).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!(
                    "too many elements in {} literal",
                    if is_tuple { "tuple" } else { "list" }
                ));
            }
            return 0;
        }
        self.next_temp = base + n;
        // Always update max_reg with `base` — BuildList/BuildTuple always writes
        // to `base` regardless of element count (even empty collections).
        let max_used = if n > 0 { base + n - 1 } else { base };
        if max_used > self.max_reg {
            self.max_reg = max_used;
        }
        for (i, item) in (0u32..).zip(items.iter()) {
            let slot = base + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(item);
            if r != slot {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, slot) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(slot, r));
                }
            }
            self.next_temp = saved;
        }
        if is_tuple {
            self.emit(Insn::BuildTuple(base, base, n));
        } else {
            self.emit(Insn::BuildList(base, base, n));
        }
        self.next_temp = base + 1;
        base
    }

    /// Compile `[a, *b, c]` / `(a, *b, c)` — PEP 448 sequence splat.
    /// Strategy: build an empty list, then for each item emit either
    /// `ListAppend` (literal) or `ListExtend` (splat).  Tuples reuse the same
    /// path then convert via the `tuple` builtin at the end.
    fn compile_unpack_list_or_tuple(&mut self, items: &[Expr], is_tuple: bool) -> Reg {
        let dst = self.alloc_temp();
        let empty_base = self.next_temp;
        self.next_temp = empty_base + 1;
        if empty_base > self.max_reg {
            self.max_reg = empty_base;
        }
        self.emit(Insn::BuildList(dst, empty_base, 0));
        for item in items {
            match item {
                Expr::Starred(inner) => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(inner);
                    self.emit(Insn::ListExtend(dst, r));
                    self.next_temp = saved;
                }
                _ => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(item);
                    self.emit(Insn::ListAppend(dst, r));
                    self.next_temp = saved;
                }
            }
        }
        if !is_tuple {
            self.next_temp = dst + 1;
            return dst;
        }
        // Convert the freshly-built list into a tuple via the `tuple` builtin.
        let frame = self.next_temp;
        if frame.checked_add(2).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("register overflow in tuple splat".to_string());
            }
            return 0;
        }
        self.next_temp = frame + 2;
        if frame + 1 > self.max_reg {
            self.max_reg = frame + 1;
        }
        let tuple_name_idx = self.intern_name("tuple");
        self.emit(Insn::LoadGlobal(frame, tuple_name_idx));
        self.emit(Insn::Move(frame + 1, dst));
        self.emit(Insn::Call(frame, 1));
        self.next_temp = frame + 1;
        frame
    }

    /// Compile `{a, *b, c}` — PEP 448 set splat.  Strategy: build an empty
    /// list (uniform path with non-splat sets), then convert via the `set`
    /// builtin.  Splat elements are appended via `ListExtend`, ordinary
    /// elements via `ListAppend`.
    fn compile_set_literal(&mut self, items: &[Expr]) -> Reg {
        let has_splat = items.iter().any(|e| matches!(e, Expr::Starred(_)));
        if !has_splat {
            // Fast path: no splat — same code shape as the original.
            let n = items.len() as Reg;
            let frame = self.next_temp;
            if frame.checked_add(1 + n).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many elements in set literal".to_string());
                }
                return 0;
            }
            self.next_temp = frame + 1 + n;
            if frame + n > self.max_reg {
                self.max_reg = frame + n;
            }
            let set_name_idx = self.intern_name("set");
            self.emit(Insn::LoadGlobal(frame, set_name_idx));
            let list_r = frame + 1;
            let saved = self.next_temp;
            let list_base = self.next_temp;
            if list_base.checked_add(n).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many elements in set literal".to_string());
                }
                return 0;
            }
            self.next_temp = list_base + n;
            if list_base + n - 1 > self.max_reg {
                self.max_reg = list_base + n - 1;
            }
            for (i, item) in (0u32..).zip(items.iter()) {
                let slot = list_base + i;
                let ns = self.next_temp;
                let r = self.compile_expr(item);
                if r != slot {
                    self.emit(Insn::Move(slot, r));
                }
                self.next_temp = ns;
            }
            self.emit(Insn::BuildList(list_r, list_base, n));
            self.next_temp = saved;
            self.next_temp = frame + 2;
            if frame + 1 > self.max_reg {
                self.max_reg = frame + 1;
            }
            self.emit(Insn::Call(frame, 1));
            self.next_temp = frame + 1;
            return frame;
        }

        // Slow path with splats: build list incrementally, then call set(list).
        let frame = self.next_temp;
        if frame.checked_add(2).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("register overflow in set splat".to_string());
            }
            return 0;
        }
        self.next_temp = frame + 2;
        if frame + 1 > self.max_reg {
            self.max_reg = frame + 1;
        }
        let set_name_idx = self.intern_name("set");
        self.emit(Insn::LoadGlobal(frame, set_name_idx));
        let list_r = frame + 1;
        let empty_base = self.next_temp;
        self.next_temp = empty_base + 1;
        if empty_base > self.max_reg {
            self.max_reg = empty_base;
        }
        self.emit(Insn::BuildList(list_r, empty_base, 0));
        for item in items {
            match item {
                Expr::Starred(inner) => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(inner);
                    self.emit(Insn::ListExtend(list_r, r));
                    self.next_temp = saved;
                }
                _ => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(item);
                    self.emit(Insn::ListAppend(list_r, r));
                    self.next_temp = saved;
                }
            }
        }
        self.emit(Insn::Call(frame, 1));
        self.next_temp = frame + 1;
        frame
    }

    /// Compile `{k1: v1, **m, k2: v2}` — supports PEP 448 dict splat.
    /// Fast path (no `**` splats) uses `BuildDict` with pre-staged key/value
    /// slots, identical to the pre-PEP-448 shape.  Slow path builds an empty
    /// dict and emits `SetItem` for pairs / `DictUpdate` for splats.
    fn compile_dict_literal(&mut self, items: &[DictItem]) -> Reg {
        let has_splat = items.iter().any(|i| matches!(i, DictItem::DoubleSplat(_)));
        if !has_splat {
            let n = items.len() as Reg;
            let base = self.next_temp;
            let slots_needed = n.saturating_mul(2);
            if base.checked_add(slots_needed).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many entries in dict literal".to_string());
                }
                return 0;
            }
            if base > self.max_reg {
                self.max_reg = base;
            }
            self.next_temp = base + n.saturating_mul(2);
            if self.next_temp > 0 && self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (i, item) in (0u32..).zip(items.iter()) {
                let (key_expr, val_expr) = match item {
                    DictItem::Pair(k, v) => (k, v),
                    DictItem::DoubleSplat(_) => unreachable!("has_splat is false"),
                };
                let k_slot = base + i * 2;
                let v_slot = base + i * 2 + 1;
                let saved = self.next_temp;
                let insn_before = self.insns.len();
                let kr = self.compile_expr(key_expr);
                if kr != k_slot {
                    let single = self.insns.len() == insn_before + 1;
                    if single && kr >= self.base_temp && self.retarget_last(kr, k_slot) {
                        // retargeted in place — no Move needed
                    } else {
                        self.emit(Insn::Move(k_slot, kr));
                    }
                }
                self.next_temp = saved;
                let insn_before = self.insns.len();
                let vr = self.compile_expr(val_expr);
                if vr != v_slot {
                    let single = self.insns.len() == insn_before + 1;
                    if single && vr >= self.base_temp && self.retarget_last(vr, v_slot) {
                        // retargeted in place — no Move needed
                    } else {
                        self.emit(Insn::Move(v_slot, vr));
                    }
                }
                self.next_temp = saved;
            }
            self.emit(Insn::BuildDict(base, base, n));
            self.next_temp = base + 1;
            return base;
        }

        // Slow path: build empty dict, populate via SetItem / DictUpdate.
        let dst = self.alloc_temp();
        let empty_base = self.next_temp;
        self.next_temp = empty_base + 1;
        if empty_base > self.max_reg {
            self.max_reg = empty_base;
        }
        self.emit(Insn::BuildDict(dst, empty_base, 0));
        for item in items {
            match item {
                DictItem::Pair(k, v) => {
                    let saved = self.next_temp;
                    let kr = self.compile_expr(k);
                    let vr = self.compile_expr(v);
                    self.emit(Insn::SetItem(dst, kr, vr));
                    self.next_temp = saved;
                }
                DictItem::DoubleSplat(e) => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(e);
                    self.emit(Insn::DictUpdate(dst, r));
                    self.next_temp = saved;
                }
            }
        }
        self.next_temp = dst + 1;
        dst
    }
}
