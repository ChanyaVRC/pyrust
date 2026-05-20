use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::{
    AssignTarget, BinaryOp, CompClause, DictItem, Expr, FStringPart, FunctionParam, MatchArm,
    Pattern, Stmt, UnaryOp,
};
use crate::bytecode::{CellVar, FnCode, FnParamSpec, FnProto, Insn, Reg};
use crate::error::PyError;
use crate::value::{PyBigInt, PyPow, PyToPrimitive, Value, ValueKind};

/// Compile a top-level script body.  All script-level names are locals.
///
/// When `repl_mode` is true, top-level `Stmt::Expr` statements emit
/// `Insn::PrintExpr` instead of discarding the result.
pub fn compile_script(
    stmts: &[Stmt],
    local_index: Rc<HashMap<String, Reg>>,
    repl_mode: bool,
) -> Result<FnCode, PyError> {
    // Script-level code cannot have nonlocal, and nothing captures script
    // locals via nonlocal from a nested scope at this level.
    let cell_vars = collect_cell_vars(stmts, &local_index);
    let mut c = Compiler::new(local_index, 0, cell_vars);
    // Issue #820: module-scope stores emit SyncModuleGlobal to keep
    // module_globals_dict live after globals() has been called.
    c.is_module_scope = true;
    // Issue #711: if the first statement is a bare string literal and we are
    // compiling a script file (not the REPL), it is the module docstring.
    // Emit a StoreGlobal for `__doc__` (CPython parity) before compiling the
    // rest.  In REPL mode every string-expression is just a value expression
    // whose repr is printed; it is NOT a module docstring (CPython's interactive
    // console does not set __doc__ from string literals typed interactively).
    let body = if !repl_mode {
        match stmts {
            [Stmt::Expr(Expr::Str(s)), rest @ ..] => {
                let r = c.compile_literal(Value::string(s.clone()));
                c.compile_store_name("__doc__", r);
                c.free_temp(r);
                rest
            }
            _ => stmts,
        }
    } else {
        stmts
    };
    if repl_mode {
        for stmt in body {
            if let Stmt::Expr(e) = stmt {
                let r = c.compile_expr(e);
                c.emit(Insn::PrintExpr(r));
                c.free_temp(r);
            } else {
                c.compile_stmt(stmt);
            }
        }
    } else {
        c.compile_block(body);
    }
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
        Stmt::With { items, body } => {
            for (e, _) in items {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            }
            collect_lambda_captures(body, local_index, is_class_scope, cells);
        }
        Stmt::Raise {
            expr: Some(e),
            cause,
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
        Stmt::AnnDeclare(_) => {}
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
                for param in params {
                    uses.remove(param);
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
        Expr::Call { func, args } => {
            lambda_captures_in_expr(func, local_index, is_class_scope, cells);
            for a in args {
                lambda_captures_in_expr(&a.value, local_index, is_class_scope, cells);
            }
        }
        Expr::Attr { target, .. } => {
            lambda_captures_in_expr(target, local_index, is_class_scope, cells)
        }
        Expr::Index { target, index } => {
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
        Expr::ListComp { elt, clauses } | Expr::SetComp { elt, clauses } => {
            for clause in clauses {
                lambda_captures_in_expr(&clause.iter, local_index, is_class_scope, cells);
                if let Some(c) = &clause.cond {
                    lambda_captures_in_expr(c, local_index, is_class_scope, cells);
                }
            }
            lambda_captures_in_expr(elt, local_index, is_class_scope, cells);
        }
        Expr::GenExp { elt, clauses } => {
            // The outermost iterable is evaluated in the enclosing scope.
            if let Some(first) = clauses.first() {
                lambda_captures_in_expr(&first.iter, local_index, is_class_scope, cells);
            }
            // Everything inside the genexp body (outermost cond, inner
            // iters/conds, and the element expression) runs in the genexp's
            // own scope.  Collect all free-var reads from that inner body,
            // subtract the loop-target names that are bound by the genexp
            // itself, and promote any remaining names that live in the
            // enclosing local_index to cell vars so they're accessible via
            // the env chain when the generator body resumes.
            if !is_class_scope {
                let mut inner_uses: HashSet<String> = HashSet::new();
                if let Some(first) = clauses.first() {
                    if let Some(c) = &first.cond {
                        collect_free_var_reads_in_expr(c, &mut inner_uses);
                    }
                }
                for clause in clauses.iter().skip(1) {
                    collect_free_var_reads_in_expr(&clause.iter, &mut inner_uses);
                    if let Some(c) = &clause.cond {
                        collect_free_var_reads_in_expr(c, &mut inner_uses);
                    }
                }
                collect_free_var_reads_in_expr(elt, &mut inner_uses);
                // Remove names bound by the genexp's own clause targets.
                let mut bound: HashSet<String> = HashSet::new();
                for clause in clauses {
                    collect_written_target(&clause.target, &mut bound);
                }
                for name in inner_uses {
                    if !bound.contains(&name) && local_index.contains_key(&name) {
                        cells.insert(name);
                    }
                }
            }
        }
        Expr::DictComp { key, val, clauses } => {
            for clause in clauses {
                lambda_captures_in_expr(&clause.iter, local_index, is_class_scope, cells);
                if let Some(c) = &clause.cond {
                    lambda_captures_in_expr(c, local_index, is_class_scope, cells);
                }
            }
            lambda_captures_in_expr(key, local_index, is_class_scope, cells);
            lambda_captures_in_expr(val, local_index, is_class_scope, cells);
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
        Expr::Var(_)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None => {}
        Expr::Yield(Some(e)) => lambda_captures_in_expr(e, local_index, is_class_scope, cells),
        Expr::Yield(None) => {}
        Expr::YieldFrom(e) => lambda_captures_in_expr(e, local_index, is_class_scope, cells),
    }
}

/// Walk a class body and record names bound at the top level in their
/// *textual* order — used only to assign **register slot numbers** for
/// the class-body sub-compiler.  Slot order has **no** influence on
/// class-namespace insertion order any more (`vars(C)` follows runtime
/// stores via `Insn::RecordClassStore`); we keep this textual walk so
/// register assignments remain deterministic across runs (HashSet
/// iteration order is randomised).  Names not in `body_local` are
/// skipped (they're declared `global` / `nonlocal` and don't get a
/// class-body slot).
fn collect_class_body_names_textual(
    body: &[Stmt],
    ordered: &mut Vec<String>,
    seen: &mut HashSet<String>,
    body_local: &HashSet<String>,
) {
    for stmt in body {
        match stmt {
            Stmt::Assign(target, _) => {
                collect_assign_target_textual(target, ordered, seen, body_local);
            }
            Stmt::AnnAssign { name, .. } => {
                if body_local.contains(name) && seen.insert(name.clone()) {
                    ordered.push(name.clone());
                }
            }
            Stmt::Def { name, .. } | Stmt::Class { name, .. } => {
                if body_local.contains(name) && seen.insert(name.clone()) {
                    ordered.push(name.clone());
                }
            }
            Stmt::If {
                branches,
                else_branch,
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
            Stmt::AnnAssign {
                name,
                value: Some(_),
                ..
            } => {
                if body_local.contains(name) && seen.insert(name.clone()) {
                    ordered.push(name.clone());
                }
            }
            Stmt::AnnAssign { value: None, .. } => {}
            _ => {}
        }
    }
}

fn collect_assign_target_textual(
    target: &AssignTarget,
    ordered: &mut Vec<String>,
    seen: &mut HashSet<String>,
    body_local: &HashSet<String>,
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
        AssignTarget::Attr(..) | AssignTarget::Index(..) => {}
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
    let class_locals_opt: Option<&HashSet<String>> = if outer_is_class_scope {
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
                        && class_locals_opt.map_or(true, |cl| !cl.contains(&name))
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
fn collect_class_lambda_outer_refs_in_expr(
    expr: &Expr,
    local_index: &HashMap<String, Reg>,
    class_locals: &HashSet<String>,
    cells: &mut HashSet<String>,
) {
    match expr {
        Expr::Lambda { params, body } => {
            let mut uses: HashSet<String> = HashSet::new();
            collect_free_var_reads_in_expr(body, &mut uses);
            collect_transitive_free_vars_in_expr(body, &mut uses);
            for p in params {
                uses.remove(p);
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
        Expr::Call { func, args } => {
            collect_class_lambda_outer_refs_in_expr(func, local_index, class_locals, cells);
            for a in args {
                collect_class_lambda_outer_refs_in_expr(&a.value, local_index, class_locals, cells);
            }
        }
        Expr::Attr { target, .. } => {
            collect_class_lambda_outer_refs_in_expr(target, local_index, class_locals, cells)
        }
        Expr::Index { target, index } => {
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
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            for clause in clauses {
                collect_class_lambda_outer_refs_in_expr(
                    &clause.iter,
                    local_index,
                    class_locals,
                    cells,
                );
                if let Some(c) = &clause.cond {
                    collect_class_lambda_outer_refs_in_expr(c, local_index, class_locals, cells);
                }
            }
            collect_class_lambda_outer_refs_in_expr(elt, local_index, class_locals, cells);
        }
        Expr::DictComp { key, val, clauses } => {
            for clause in clauses {
                collect_class_lambda_outer_refs_in_expr(
                    &clause.iter,
                    local_index,
                    class_locals,
                    cells,
                );
                if let Some(c) = &clause.cond {
                    collect_class_lambda_outer_refs_in_expr(c, local_index, class_locals, cells);
                }
            }
            collect_class_lambda_outer_refs_in_expr(key, local_index, class_locals, cells);
            collect_class_lambda_outer_refs_in_expr(val, local_index, class_locals, cells);
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
        Expr::Var(_)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None => {}
        Expr::Yield(Some(e)) => {
            collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells)
        }
        Expr::Yield(None) => {}
        Expr::YieldFrom(e) => {
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
        Stmt::With { items, body } => {
            for (e, _) in items {
                collect_free_var_reads_in_expr(e, uses);
            }
            collect_free_var_reads_in_stmts(body, uses);
        }
        Stmt::Raise {
            expr: Some(e),
            cause,
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
        Stmt::AnnDeclare(_) => {}
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
        Expr::Var(n) => {
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
        Expr::Call { func, args } => {
            collect_free_var_reads_in_expr(func, uses);
            for a in args {
                collect_free_var_reads_in_expr(&a.value, uses);
            }
        }
        Expr::Attr { target, .. } => collect_free_var_reads_in_expr(target, uses),
        Expr::Index { target, index } => {
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
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            for clause in clauses {
                collect_free_var_reads_in_expr(&clause.iter, uses);
                if let Some(c) = &clause.cond {
                    collect_free_var_reads_in_expr(c, uses);
                }
            }
            collect_free_var_reads_in_expr(elt, uses);
        }
        Expr::DictComp { key, val, clauses } => {
            for clause in clauses {
                collect_free_var_reads_in_expr(&clause.iter, uses);
                if let Some(c) = &clause.cond {
                    collect_free_var_reads_in_expr(c, uses);
                }
            }
            collect_free_var_reads_in_expr(key, uses);
            collect_free_var_reads_in_expr(val, uses);
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_free_var_reads_in_expr(cond, uses);
            collect_free_var_reads_in_expr(then, uses);
            collect_free_var_reads_in_expr(else_, uses);
        }
        Expr::Lambda { body, .. } => collect_free_var_reads_in_expr(body, uses),
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
        | Expr::None => {}
        Expr::Yield(Some(e)) => collect_free_var_reads_in_expr(e, uses),
        Expr::Yield(None) => {}
        Expr::YieldFrom(e) => collect_free_var_reads_in_expr(e, uses),
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
        Stmt::With { items, body } => {
            for (e, _) in items {
                collect_transitive_free_vars_in_expr(e, uses);
            }
            collect_transitive_free_vars_in_stmts(body, uses);
        }
        Stmt::Raise {
            expr: Some(e),
            cause,
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
        Stmt::AnnDeclare(_) => {}
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
            for p in params {
                inner_uses.remove(p);
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
        Expr::Call { func, args } => {
            collect_transitive_free_vars_in_expr(func, uses);
            for a in args {
                collect_transitive_free_vars_in_expr(&a.value, uses);
            }
        }
        Expr::Attr { target, .. } => collect_transitive_free_vars_in_expr(target, uses),
        Expr::Index { target, index } => {
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
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            for clause in clauses {
                collect_transitive_free_vars_in_expr(&clause.iter, uses);
                if let Some(c) = &clause.cond {
                    collect_transitive_free_vars_in_expr(c, uses);
                }
            }
            collect_transitive_free_vars_in_expr(elt, uses);
        }
        Expr::DictComp { key, val, clauses } => {
            for clause in clauses {
                collect_transitive_free_vars_in_expr(&clause.iter, uses);
                if let Some(c) = &clause.cond {
                    collect_transitive_free_vars_in_expr(c, uses);
                }
            }
            collect_transitive_free_vars_in_expr(key, uses);
            collect_transitive_free_vars_in_expr(val, uses);
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
        Expr::Var(_)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None => {}
        Expr::Yield(Some(e)) => collect_transitive_free_vars_in_expr(e, uses),
        Expr::Yield(None) => {}
        Expr::YieldFrom(e) => collect_transitive_free_vars_in_expr(e, uses),
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
        Expr::Unary { op, expr } => {
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
                UnaryOp::Not => Some(Value::bool_(!val.truthy())),
                UnaryOp::BitNot => match val.kind() {
                    ValueKind::Int(n) => Some(Value::int(!n)),
                    _ => None,
                },
                _ => None,
            }
        }
        Expr::Binary { left, op, right } => {
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
                if !result.truthy() {
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
    match (l.kind(), op, r.kind()) {
        (ValueKind::Int(a), BinaryOp::Add, ValueKind::Int(b)) => Some(match a.checked_add(b) {
            Some(r) => Value::int(r),
            None => Value::bigint(PyBigInt::from(a) + PyBigInt::from(b)),
        }),
        (ValueKind::Int(a), BinaryOp::Sub, ValueKind::Int(b)) => Some(match a.checked_sub(b) {
            Some(r) => Value::int(r),
            None => Value::bigint(PyBigInt::from(a) - PyBigInt::from(b)),
        }),
        (ValueKind::Int(a), BinaryOp::Mul, ValueKind::Int(b)) => Some(match a.checked_mul(b) {
            Some(r) => Value::int(r),
            None => Value::bigint(PyBigInt::from(a) * PyBigInt::from(b)),
        }),
        (ValueKind::Int(a), BinaryOp::Div, ValueKind::Int(b)) if b != 0 => {
            Some(Value::float(a as f64 / b as f64))
        }
        (ValueKind::Int(a), BinaryOp::FloorDiv, ValueKind::Int(b)) if b != 0 => {
            let q = a.wrapping_div(b);
            let r = a.wrapping_rem(b);
            Some(Value::int(if (r != 0) && ((r < 0) != (b < 0)) {
                q - 1
            } else {
                q
            }))
        }
        (ValueKind::Int(a), BinaryOp::Mod, ValueKind::Int(b)) if b != 0 => {
            let r = a.wrapping_rem(b);
            Some(Value::int(if (r != 0) && ((r < 0) != (b < 0)) {
                r + b
            } else {
                r
            }))
        }
        (ValueKind::Int(a), BinaryOp::Pow, ValueKind::Int(b)) if b >= 0 => {
            // Limit folded exponents to u32::MAX; larger ones (extremely rare
            // at compile time) fall through and are computed at runtime.
            let exp = u32::try_from(b).ok()?;
            Some(match a.checked_pow(exp) {
                Some(r) => Value::int(r),
                None => Value::bigint(PyPow::pow(PyBigInt::from(a), exp)),
            })
        }
        (ValueKind::Int(a), BinaryOp::BitAnd, ValueKind::Int(b)) => Some(Value::int(a & b)),
        (ValueKind::Int(a), BinaryOp::BitOr, ValueKind::Int(b)) => Some(Value::int(a | b)),
        (ValueKind::Int(a), BinaryOp::BitXor, ValueKind::Int(b)) => Some(Value::int(a ^ b)),
        (ValueKind::Int(a), BinaryOp::LShift, ValueKind::Int(b)) if b >= 0 => {
            // Promote to BigInt when the shift overflows i64 — identical to
            // the runtime path in eval_binary.
            // Cap at 1_000_000 bits: astronomically large shifts (e.g.
            // `1 << i64::MAX`) would exhaust memory during compilation.
            // Values above the cap are left for the runtime to handle
            // (which will raise OverflowError for non-zero LHS).
            if b > 1_000_000 {
                return None;
            }
            let n = b as usize;
            let big = PyBigInt::from(a) << n;
            Some(match big.to_i64() {
                Some(r) => Value::int(r),
                None => Value::bigint(big),
            })
        }
        (ValueKind::Int(a), BinaryOp::RShift, ValueKind::Int(b)) if b >= 0 => {
            // Right-shift by ≥ 64 saturates to the sign bit (0 or -1).
            if b >= 64 {
                Some(Value::int(if a < 0 { -1 } else { 0 }))
            } else {
                Some(Value::int(a >> b))
            }
        }
        (ValueKind::Float(a), BinaryOp::Add, ValueKind::Float(b)) => Some(Value::float(a + b)),
        (ValueKind::Float(a), BinaryOp::Sub, ValueKind::Float(b)) => Some(Value::float(a - b)),
        (ValueKind::Float(a), BinaryOp::Mul, ValueKind::Float(b)) => Some(Value::float(a * b)),
        (ValueKind::Float(a), BinaryOp::Div, ValueKind::Float(b)) if b != 0.0 => {
            Some(Value::float(a / b))
        }
        (ValueKind::Str(a), BinaryOp::Add, ValueKind::Str(b)) => {
            Some(Value::string(a.to_string() + b))
        }
        (ValueKind::Int(a), BinaryOp::Eq, ValueKind::Int(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Int(a), BinaryOp::Ne, ValueKind::Int(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Int(a), BinaryOp::Lt, ValueKind::Int(b)) => Some(Value::bool_(a < b)),
        (ValueKind::Int(a), BinaryOp::Le, ValueKind::Int(b)) => Some(Value::bool_(a <= b)),
        (ValueKind::Int(a), BinaryOp::Gt, ValueKind::Int(b)) => Some(Value::bool_(a > b)),
        (ValueKind::Int(a), BinaryOp::Ge, ValueKind::Int(b)) => Some(Value::bool_(a >= b)),
        (ValueKind::Str(a), BinaryOp::Eq, ValueKind::Str(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Str(a), BinaryOp::Ne, ValueKind::Str(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Bool(a), BinaryOp::Eq, ValueKind::Bool(b)) => Some(Value::bool_(a == b)),
        _ => None,
    }
}

fn extract_literal_int(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(n) => Some(*n),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
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
            Stmt::If { branches, else_branch: None }
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
                    },
                    b_body,
                )],
                else_branch: None,
            });
        } else if b_body.is_empty() {
            // `if guard: A_pre` (the else is empty so don't emit it).
            out.push(Stmt::If {
                branches: vec![(guard, a_body)],
                else_branch: None,
            });
        } else {
            // `if guard: A_pre else: B_pre`
            out.push(Stmt::If {
                branches: vec![(guard, a_body)],
                else_branch: Some(b_body),
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
        | Expr::Var(_) => true,
        Expr::Unary { expr, .. } => expr_is_side_effect_free(expr),
        Expr::Binary { left, right, op: _ } => {
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
        Some(Stmt::If { branches, else_branch: None })
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
        Expr::Binary { left, op, right } => {
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
                },
                None => Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(Expr::Binary { left, op, right }),
                },
            }
        }
        other => Expr::Unary {
            op: UnaryOp::Not,
            expr: Box::new(other),
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
            Stmt::AnnAssign { value: None, .. } | Stmt::AnnDeclare(_) => {}
            Stmt::If {
                branches,
                else_branch,
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
        | Expr::None => true,
        Expr::Var(name) => !written.contains(name.as_str()),
        Expr::Binary { left, right, .. } => {
            expr_is_invariant(left, written) && expr_is_invariant(right, written)
        }
        Expr::Unary { expr, .. } => expr_is_invariant(expr, written),
        // NamedExpr has a side effect (assignment), never invariant
        Expr::Named { .. } => false,
        _ => false,
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
        } => {
            let i = match left.as_ref() {
                Expr::Var(n) => n.clone(),
                _ => return None,
            };
            let c = match right.as_ref() {
                Expr::Call { func, args } => {
                    if !matches!(func.as_ref(), Expr::Var(f) if f == "len") {
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
                        Expr::Var(n) => n.clone(),
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
            },
        ) => {
            t == &i_name
                && matches!(left.as_ref(), Expr::Var(n) if n == &i_name)
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
        iter: Expr::Var(c_name),
        body: new_body,
        else_branch: None,
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
            if matches!(target.as_ref(), Expr::Var(n) if n == c_name) {
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
        } => expr_safe(target, i_name, c_name) && expr_safe(expr, i_name, c_name),
        Stmt::Expr(e) => expr_safe(e, i_name, c_name),
        Stmt::Return(e) => e.as_ref().is_none_or(|x| expr_safe(x, i_name, c_name)),
        Stmt::If {
            branches,
            else_branch,
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
        Stmt::Pass | Stmt::AnnDeclare(_) | Stmt::Global(_) | Stmt::Nonlocal(_) => true,
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
                if let Expr::Index { target, index } = e
                    && is_c_at_i_expr(target, index, c_name, i_name)
                {
                    return false;
                }
                // `del i` and `del c` would change semantics.
                if let Expr::Var(n) = e
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
        Stmt::Raise { expr, cause } => {
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
        | Expr::None => true,
        Expr::Var(n) => {
            // A bare reference to `i` outside of `c[i]` would still need to
            // see the index, not the value — bail.  Bare `c` reads are fine.
            n != i_name
        }
        Expr::Index { target, index } => {
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
        Expr::Call { func, args } => {
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
        | Expr::YieldFrom(_) => false,
    }
}

fn is_c_at_i_expr(target: &Expr, index: &Expr, c_name: &str, i_name: &str) -> bool {
    matches!(target, Expr::Var(n) if n == c_name) && matches!(index, Expr::Var(n) if n == i_name)
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
        AssignTarget::Attr(t, _) => expr_safe(t, i_name, c_name),
        AssignTarget::Index(t, idx) => {
            // `c[i] = ...` is handled at the Stmt::IndexAssign site; here we
            // only see this via `IndexAssign`/`SliceAssign` containers — never
            // reached in practice, but keep it sound.
            expr_safe(t, i_name, c_name) && expr_safe(idx, i_name, c_name)
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
        Stmt::With { items, body } => {
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
        Stmt::Raise { expr, cause } => {
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
        Stmt::AnnDeclare(_) => false,
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
        Expr::Var(n) => n == name,
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None => false,
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
        Expr::Call { func, args } => {
            expr_reads_var(func, name) || args.iter().any(|a| expr_reads_var(&a.value, name))
        }
        Expr::Attr { target, .. } => expr_reads_var(target, name),
        Expr::Index { target, index } => {
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
        Expr::Lambda { body, .. } => expr_reads_var(body, name),
        Expr::Named { value, .. } => expr_reads_var(value, name),
        Expr::Yield(Some(e)) => expr_reads_var(e, name),
        Expr::Yield(None) => false,
        Expr::YieldFrom(e) => expr_reads_var(e, name),
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
        Stmt::Raise { expr, cause } => {
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
    if let Expr::Index { target, index } = expr
        && is_c_at_i_expr(target, index, c_name, i_name)
    {
        *expr = Expr::Var(i_name.to_string());
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
        | Expr::Var(_) => {}
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
        Expr::Call { func, args } => {
            rewrite_c_at_i_in_expr(func, c_name, i_name);
            for a in args.iter_mut() {
                rewrite_c_at_i_in_expr(&mut a.value, c_name, i_name);
            }
        }
        Expr::Attr { target, .. } => rewrite_c_at_i_in_expr(target, c_name, i_name),
        Expr::Index { target, index } => {
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
        | Expr::YieldFrom(_) => {}
    }
}

/// Detect `while VAR cmp STOP: ...; VAR += STEP` (or -= for decreasing).
fn detect_while_range<'a>(
    cond: &'a Expr,
    body: &'a [Stmt],
) -> Option<(&'a str, &'a Expr, i64, bool)> {
    let (var_name, cmp_op, stop_expr) = match cond {
        Expr::Binary { op, left, right }
            if matches!(
                op,
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
            ) =>
        {
            match left.as_ref() {
                Expr::Var(name) => (name.as_str(), op, right.as_ref()),
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
    break_patches: Vec<usize>,
    /// None when the continue target is not yet known (e.g. counter-range loop
    /// where the increment comes after the body).  Patched before the increment.
    continue_target: Option<usize>,
    /// Indices of Jump(0) instructions emitted for `continue` when continue_target
    /// was None; fixed up once continue_target is established.
    continue_patches: Vec<usize>,
    /// Depth of `Compiler::except_cleanups` at the point this loop was entered.
    /// `break` and `continue` must emit cleanups for entries above this depth.
    cleanup_depth: usize,
}

/// Describes the cleanup that must be emitted before an early exit
/// (`break`, `continue`, or `return`) that crosses a guarded block boundary.
#[derive(Clone)]
enum EarlyExitCleanup {
    /// Inside a try-body that has an active `SetupExcept` on the handler stack.
    /// Early exit must emit `PopExcept` then optionally inline the finally block.
    TryBody { finally_stmts: Option<Vec<Stmt>> },
    /// Inside an except-handler body where `active_exception` is set.
    /// Early exit must emit `EndExcept` then optionally inline the finally block.
    ExceptBody { finally_stmts: Option<Vec<Stmt>> },
}

struct Compiler {
    local_index: Rc<HashMap<String, Reg>>,
    cell_vars: HashSet<String>,
    insns: Vec<Insn>,
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
    except_cleanups: Vec<EarlyExitCleanup>,
    failed: bool,
    error_msg: Option<String>,
    def_set: u64,
    fn_protos: Vec<FnProto>,
    /// Names of pure (side-effect-free) functions defined in this scope.
    pure_locals: HashSet<String>,
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
    outer_locals: Vec<Rc<HashMap<String, Reg>>>,
    /// True when this Compiler is producing the body of a function `def`
    /// (or a comprehension, which implicitly creates a function scope).
    /// False for module-level compilation and class-body compilation.
    /// Used to determine whether `self.local_index` counts as an enclosing
    /// function scope for `nonlocal` validation in child compilers.
    is_function_scope: bool,
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
}

fn class_body_has_annotations(body: &[Stmt]) -> bool {
    body.iter().any(|s| match s {
        Stmt::AnnAssign { .. } => true,
        Stmt::If {
            branches,
            else_branch,
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
            insns: Vec::new(),
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
            except_cleanups: Vec::new(),
            failed: n > Reg::MAX as usize,
            error_msg: if n > Reg::MAX as usize {
                Some(format!("too many local variables (max {})", Reg::MAX))
            } else {
                None
            },
            def_set: def_bound_mask,
            fn_protos: Vec::new(),
            pure_locals: HashSet::new(),
            is_class_body: false,
            is_class_method: false,
            qualname_prefix: String::new(),
            outer_locals: Vec::new(),
            is_function_scope: false,
            is_syntax_error: false,
            is_module_scope: false,
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

    fn try_literal_const_idx(&mut self, expr: &Expr) -> Option<u16> {
        match expr {
            Expr::Int(v) => Some(self.intern_const(Value::int(*v))),
            Expr::BigInt(s) => {
                let n = s.parse::<PyBigInt>().ok()?;
                Some(self.intern_const(Value::bigint(n)))
            }
            Expr::Float(v) => Some(self.intern_const(Value::float(*v))),
            Expr::Str(s) => Some(self.intern_const(Value::string(s.clone()))),
            Expr::Bytes(b) => Some(self.intern_const(Value::bytes(b.clone()))),
            Expr::Complex(re, im) => Some(self.intern_const(Value::complex(*re, *im))),
            Expr::Bool(b) => Some(self.intern_const(Value::bool_(*b))),
            Expr::None => Some(self.intern_const(Value::none())),
            _ => fold_constant(expr).map(|v| self.intern_const(v)),
        }
    }

    fn intern_const(&mut self, val: Value) -> u16 {
        // PyKey treats `Bool(b)` and `Int(b as i64)` as hash/eq-equal (matching
        // CPython's `True == 1`), so they would collide in the constant pool's
        // hash index even though they are type-distinct values.  Likewise,
        // `Float(1.0)` and `Int(1)` are now hash/eq-equal in PyKey so that
        // dict/set keys respect CPython's numeric equality invariant.  In both
        // cases the constant pool must keep the values distinct, so we skip the
        // hash-map fast path for booleans and all floats and fall
        // through to the type-exact linear scan instead.
        let is_bool = matches!(val.kind(), ValueKind::Bool(_));
        let is_float = matches!(val.kind(), ValueKind::Float(_));
        if !is_bool
            && !is_float
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
        idx
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

    /// Register index for a local variable, or None if the name is global/nonlocal/cell.
    fn local_reg(&self, name: &str) -> Option<Reg> {
        if self.is_cell(name) {
            return None;
        }
        self.local_index.get(name).copied()
    }

    fn compile_block(&mut self, stmts: &[Stmt]) {
        for (idx, stmt) in stmts.iter().enumerate() {
            if self.failed {
                return;
            }
            // #289: rewrite `while i < len(c): ...; i += 1` → `for i in c: ...`
            // when `i` is unused after the loop.  Needs the post-loop suffix
            // for the unused-after-loop check, so it lives in compile_block
            // (not compile_while which only sees its own body/else).
            if matches!(stmt, Stmt::While { .. })
                && let Some(rewritten) = try_rewrite_while_index_to_for(stmts, idx)
            {
                self.compile_stmt(&rewritten);
                continue;
            }
            self.compile_stmt(stmt);
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
            let saved_tail: Vec<EarlyExitCleanup> = self.except_cleanups.split_off(i);
            match cleanup {
                EarlyExitCleanup::TryBody { finally_stmts } => {
                    self.emit(Insn::PopExcept);
                    if let Some(stmts) = finally_stmts {
                        self.compile_block(&stmts);
                    }
                }
                EarlyExitCleanup::ExceptBody { finally_stmts } => {
                    self.emit(Insn::EndExcept);
                    if let Some(stmts) = finally_stmts {
                        self.compile_block(&stmts);
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
        let is_generator = self
            .insns
            .iter()
            .any(|i| matches!(i, Insn::Yield { .. } | Insn::YieldFrom { .. }));
        Ok(FnCode {
            insns: self.insns,
            consts: self.consts,
            names: self.names,
            num_regs,
            num_iters: self.max_iter,
            num_locals: self.base_temp,
            fn_protos: self.fn_protos,
            cell_vars: self.cell_vars.into_iter().collect(),
            is_generator,
            is_class_method: self.is_class_method,
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

    // ── Store helpers ─────────────────────────────────────────────────────────

    /// Emit the appropriate store for `name` from register `src`.
    /// If `container_expr` is a global/cell variable name, write `obj_reg` back
    /// to the env.  Called after SetItem/SetSlice on a container that was loaded
    /// via LoadGlobal (which creates a copy, so the mutation must be committed).
    fn writeback_container_if_global(&mut self, container_expr: &Expr, obj_reg: Reg) {
        if let Expr::Var(name) = container_expr
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
            self.emit(Insn::StoreGlobal(idx, src));
        }
    }

    // ── Statement compilation ─────────────────────────────────────────────────

    fn compile_stmt(&mut self, stmt: &Stmt) {
        if self.failed {
            return;
        }
        match stmt {
            Stmt::Pass | Stmt::AnnDeclare(_) => {}
            Stmt::Break => {
                if self.loops.is_empty() {
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("'break' outside loop".to_string());
                    }
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
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("'continue' not properly in loop".to_string());
                    }
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
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("'return' outside function".to_string());
                    }
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
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("'return' outside function".to_string());
                    }
                    return;
                }
                let r = self.compile_expr(expr);
                self.emit_early_exit_cleanups(0);
                if self.failed {
                    self.free_temp(r);
                    return;
                }
                self.emit(Insn::Return(r));
                self.free_temp(r);
            }
            Stmt::Expr(expr) => {
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
            Stmt::AnnDeclare(_) => {
                // Bare annotation with no value: no-op at runtime.
            }
            Stmt::AugAssign { target, op, expr } => {
                self.compile_aug_assign(target, *op, expr);
                if let AssignTarget::Name(name) = target
                    && let Some(reg) = self.local_reg(name)
                {
                    self.mark_def(reg);
                }
            }
            Stmt::AttrAssign { target, name, expr } => {
                let obj = self.compile_expr(target);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(name);
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
                let msg_reg = if let Some(msg_expr) = msg {
                    self.compile_expr(msg_expr)
                } else {
                    let r = self.alloc_temp();
                    self.emit(Insn::LoadNone(r));
                    r
                };
                self.emit(Insn::RaiseAssert(msg_reg));
                self.free_temp(msg_reg);
                self.patch_jump(skip);
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                self.compile_if(branches, else_branch.as_deref());
            }
            Stmt::While {
                cond,
                body,
                else_branch,
            } => {
                self.compile_while(cond, body, else_branch.as_deref());
            }
            Stmt::For {
                target,
                iter,
                body,
                else_branch,
            } => {
                self.compile_for(target, iter, body, else_branch.as_deref());
            }
            Stmt::Global(_) => {
                // Purely a compile-time declaration; no runtime effect.
            }
            Stmt::Nonlocal(_) => {
                // Nonlocal is a compile-time declaration in function bodies.
                // At module level (not inside any function or class), it is a
                // SyntaxError — CPython rejects it at compile time.
                if !self.is_function_scope && !self.is_class_body {
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg =
                            Some("nonlocal declaration not allowed at module level".to_string());
                    }
                }
            }
            Stmt::Raise { expr, cause } => {
                self.compile_raise(expr.as_ref(), cause.as_ref());
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
                decorators,
                return_annotation,
            } => {
                self.compile_def(name, params, body, decorators, return_annotation.as_ref());
            }
            Stmt::Class {
                name,
                bases,
                metaclass,
                body,
                decorators,
            } => {
                self.compile_class(name, bases, metaclass.as_ref(), body, decorators);
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
            } => {
                self.compile_try(
                    body,
                    handlers,
                    else_branch.as_deref(),
                    finally_branch.as_deref(),
                );
            }
            Stmt::With { items, body } => {
                self.compile_with(items, body);
            }
            Stmt::Match { subject, arms } => {
                self.compile_match(subject, arms);
            }
        }
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
                        AssignTarget::Attr(obj_expr, attr) => {
                            let obj = self.compile_expr(obj_expr);
                            let name_idx = self.intern_name(attr);
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
                        AssignTarget::Tuple(_) => {
                            // Nested tuple unpack — compile recursively
                            let tmp = base + i;
                            self.compile_assign(t, &Expr::Var(format!("__unpack_{}", tmp)));
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
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.compile_expr(obj_expr);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(attr);
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
        // 3. Evaluate the annotation expression.
        let ann_reg = self.compile_expr(annotation);
        if self.failed {
            self.free_temp(ann_reg);
            return;
        }
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
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.compile_expr(obj_expr);
                let name_idx = self.intern_name(attr);
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
            AssignTarget::Tuple(_) | AssignTarget::Starred(_) => {
                // Nested unpack — compile recursively using a temp var name trick
                let tmp_name = format!("__unpack_{}", src_reg);
                self.compile_assign(target, &Expr::Var(tmp_name));
            }
        }
    }

    fn compile_aug_assign(&mut self, target: &AssignTarget, op: BinaryOp, expr: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    self.emit_aug_binop(reg, op, expr);
                    self.maybe_record_class_store(reg);
                    // Issue #820: sync the updated value into module_globals_dict
                    // when globals_accessed == true (same as compile_store_name).
                    if self.is_module_scope {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                    }
                } else {
                    // cell / global: load, compute, store
                    let name_idx = self.intern_name(name);
                    let lhs = self.alloc_temp();
                    self.emit(Insn::LoadGlobal(lhs, name_idx));
                    self.emit_aug_binop(lhs, op, expr);
                    self.emit(Insn::StoreGlobal(name_idx, lhs));
                    self.free_temp(lhs);
                }
            }
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.compile_expr(obj_expr);
                let name_idx = self.intern_name(attr);
                let lhs = self.alloc_temp();
                self.emit(Insn::GetAttr(lhs, obj, name_idx));
                self.emit_aug_binop(lhs, op, expr);
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
                Stmt::Break => {
                    if !in_loop {
                        self.failed = true;
                        self.is_syntax_error = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some("'break' outside loop".to_string());
                        }
                    }
                }
                Stmt::Continue => {
                    if !in_loop {
                        self.failed = true;
                        self.is_syntax_error = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some("'continue' not properly in loop".to_string());
                        }
                    }
                }
                Stmt::Return(_) => {
                    if !self.is_function_scope {
                        self.failed = true;
                        self.is_syntax_error = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some("'return' outside function".to_string());
                        }
                    }
                }
                Stmt::Expr(expr) => {
                    self.check_dead_expr(expr);
                }
                Stmt::Nonlocal(_) => {
                    if !self.is_function_scope && !self.is_class_body {
                        self.failed = true;
                        self.is_syntax_error = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(
                                "nonlocal declaration not allowed at module level".to_string(),
                            );
                        }
                    }
                }
                Stmt::If {
                    branches,
                    else_branch,
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
                Stmt::Def { params, body, .. } => {
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
                            self.failed = true;
                            self.is_syntax_error = true;
                            if self.error_msg.is_none() {
                                self.error_msg = Some(format!(
                                    "no binding for nonlocal '{}' found",
                                    nonlocal_name
                                ));
                            }
                            return;
                        }
                    }
                    let saved_is_function_scope = self.is_function_scope;
                    let saved_is_class_body = self.is_class_body;
                    self.is_function_scope = true;
                    self.is_class_body = false;
                    self.check_dead_block(body, false);
                    self.is_function_scope = saved_is_function_scope;
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
            Expr::Yield(_) | Expr::YieldFrom(_) => {
                if !self.is_function_scope {
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("'yield' outside function".to_string());
                    }
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
            Expr::Call { func, args } => {
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

    fn compile_if(&mut self, branches: &[(Expr, Vec<Stmt>)], else_branch: Option<&[Stmt]>) {
        let has_else = else_branch.is_some();
        let n = branches.len();
        let mut end_patches: Vec<usize> = Vec::new();
        let pre_def_set = self.def_set;
        // Collect def_set after each branch body for definite-assignment analysis.
        let mut branch_def_sets: Vec<u64> = Vec::with_capacity(n + 1);

        for (bi, (cond, body)) in branches.iter().enumerate() {
            self.def_set = pre_def_set;
            // Constant-condition optimisation: fold at compile time.
            if let Some(val) = fold_constant(cond) {
                if val.truthy() {
                    // Always-true branch: compile body unconditionally; skip rest.
                    self.compile_block(body);
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
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                        if self.failed {
                            return;
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
                    continue;
                }
            }
            let cond_reg = self.compile_expr(cond);
            let jmp_false = self.emit_cond_jump(cond_reg, false);
            self.free_temp(cond_reg);
            self.compile_block(body);
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
            self.compile_block(else_stmts);
            if self.failed {
                return;
            }
            branch_def_sets.push(self.def_set);
        }
        for idx in end_patches {
            self.patch_jump(idx);
        }
        // Variables defined in every branch (including else) are definitely bound after.
        // Without an else, control may skip all branches so no new defs can be assumed.
        if has_else && !branch_def_sets.is_empty() {
            let all_define = branch_def_sets.iter().fold(!0u64, |acc, &s| acc & s);
            self.def_set = pre_def_set | all_define;
        } else {
            self.def_set = pre_def_set;
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
            self.compile_block(&arm.body);
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
            Pattern::Or(alternatives) => {
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
            Pattern::Sequence(elements) => {
                // Check that subject has exactly `fixed_count` elements
                // (unless there's a star element, then >= fixed_count).
                let has_star = elements.iter().any(|(_, is_star)| *is_star);
                let fixed_count = elements.iter().filter(|(_, s)| !s).count();

                // R_len = len(subj)
                let len_name_idx = self.intern_name("len");
                let len_fn = self.alloc_temp();
                self.emit(Insn::LoadGlobal(len_fn, len_name_idx));
                let len_arg = self.alloc_temp();
                self.emit(Insn::Move(len_arg, subj));
                self.emit(Insn::Call(len_fn, 1));
                let r_len = len_fn; // result in len_fn after call
                self.free_temp(len_arg);

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
                            // Build slice subj[start:stop]
                            let slice_key = self.alloc_temp();
                            // Build a 3-tuple (start, stop, None) to represent the slice
                            let none_r = self.alloc_temp();
                            self.emit(Insn::LoadNone(none_r));
                            // Use BuildTuple to create the slice key
                            // Arrange args in consecutive regs: start_r, stop_r, none_r
                            // They might not be consecutive, so move them.
                            let base = self.alloc_temp();
                            self.emit(Insn::Move(base, start_r));
                            let base1 = self.alloc_temp();
                            self.emit(Insn::Move(base1, stop_r));
                            let base2 = self.alloc_temp();
                            self.emit(Insn::Move(base2, none_r));
                            self.emit(Insn::BuildTuple(slice_key, base, 3));
                            self.free_temp(base2);
                            self.free_temp(base1);
                            self.free_temp(base);
                            self.free_temp(none_r);
                            self.free_temp(stop_r);
                            self.free_temp(start_r);
                            // Get the slice: subj[start:stop] via GetItem with a slice tuple
                            let elem_r = self.alloc_temp();
                            self.emit(Insn::GetItem(elem_r, subj, slice_key));
                            self.free_temp(slice_key);
                            // Store into capture name
                            self.compile_store_name(name, elem_r);
                            if let Some(reg) = self.local_reg(name) {
                                self.mark_def(reg);
                            }
                            self.free_temp(elem_r);
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
                        let after_star =
                            elements[elem_i..].iter().filter(|(_, s)| !s).count() as i64;
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
            Pattern::Mapping(pairs, rest_name) => {
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
            Pattern::Class { cls, kwargs } => {
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
                self.free_temp(cls_r);
                self.emit(Insn::Call(isinstance_fn, 2));
                self.free_temp(arg1);
                self.free_temp(arg0);
                let jmp = self.emit(Insn::JumpIfFalse(isinstance_fn, 0));
                fail_patches.push(jmp);
                self.free_temp(isinstance_fn);
                // Now check each keyword attribute.
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
        }
    }

    fn compile_while(&mut self, cond: &Expr, body: &[Stmt], else_branch: Option<&[Stmt]>) {
        if is_const_false_expr(cond) {
            // The while body is statically unreachable, but CPython still
            // validates context-sensitive syntax inside it.  The body counts
            // as a loop context (break/continue inside it are valid), but
            // return/yield are still gated by is_function_scope.
            self.check_dead_block(body, true);
            if self.failed {
                return;
            }
            if let Some(else_stmts) = else_branch {
                self.compile_block(else_stmts);
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
                Some(Stmt::If { branches, else_branch: None })
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
            && self.try_compile_while_range(cond, body, else_branch)
        {
            return;
        }

        let is_licm = !is_infinite && {
            let written = collect_body_written(body);
            expr_is_invariant(cond, &written)
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
            break_patches: Vec::new(),
            continue_target: Some(loop_start),
            continue_patches: Vec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved = self.def_set;
        self.compile_block(body);
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
        if !is_infinite {
            if let Some(else_stmts) = else_branch {
                self.compile_block(else_stmts);
                if self.failed {
                    return;
                }
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
        let neg_step_idx = self.intern_const(Value::int(step.wrapping_neg()));

        // Initialise var_reg = i_initial - step (so first ForCount yields i_initial).
        self.emit(Insn::BinOpConst(
            var_reg,
            var_reg,
            BinaryOp::Add,
            neg_step_idx,
        ));

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
                let adj_idx = self.intern_const(Value::int(stop_adjust));
                self.emit(Insn::BinOpConst(sr, sr, BinaryOp::Add, adj_idx));
            }
            let jmp = self.emit(Insn::ForCountReg(var_reg, cmp_op, sr, step_idx, 0));
            (jmp, Some(sr))
        };

        self.mark_def(var_reg);
        self.loops.push(LoopCtx {
            break_patches: Vec::new(),
            continue_target: Some(loop_start),
            continue_patches: Vec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved = self.def_set;
        // Skip the last body statement (VAR += STEP): ForCount already manages
        // the counter increment, so VAR += STEP is a dead store.
        let body_without_inc = &body[..body.len() - 1];
        self.compile_block(body_without_inc);
        self.def_set = saved;
        if self.failed {
            return true;
        }
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));
        self.patch_jump(exit_jmp);
        // Restore post-loop value: Python semantics require i == stop after natural exit.
        // Break patches jump PAST this BinOpConst so break leaves the break-iteration value.
        self.emit(Insn::BinOpConst(var_reg, var_reg, BinaryOp::Add, step_idx));
        let ctx = self.loops.pop().unwrap();
        if let Some(t) = stop_temp {
            self.free_temp(t);
        }
        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
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
            Expr::Call { func, args } => (func.as_ref(), args.as_slice()),
            _ => return false,
        };
        if !matches!(func, Expr::Var(n) if n == "range") {
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
        let neg_step_idx = self.intern_const(Value::int(step_val.wrapping_neg()));
        if let Some(start) = start_opt {
            let r = self.compile_expr(start);
            // var_reg = r + (-step_val) = r - step_val
            self.emit(Insn::BinOpConst(var_reg, r, BinaryOp::Add, neg_step_idx));
            self.free_temp(r);
        } else {
            // start = 0; init = -step_val
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
            break_patches: Vec::new(),
            continue_target: Some(loop_start),
            continue_patches: Vec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved = self.def_set;
        self.compile_block(body);
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
            self.compile_block(else_stmts);
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
    ) {
        // Collapse the `if guard: continue; <rest>` trampoline (issue #287)
        // before dispatching: this also lets `try_compile_for_range` see the
        // simpler body shape if the rewrite eliminates all `continue`s.
        let rewritten = rewrite_continue_top(body.to_vec());
        let body: &[Stmt] = &rewritten;

        if self.try_compile_for_range(target, iter_expr, body, else_branch) {
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
                            _ => {
                                self.failed = true;
                                if self.error_msg.is_none() {
                                    self.error_msg =
                                        Some("unsupported for-loop unpack target".to_string());
                                }
                                return;
                            }
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
            break_patches: Vec::new(),
            continue_target: Some(loop_start),
            continue_patches: Vec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved_def_set = self.def_set;
        self.mark_target_def(target);
        self.compile_block(body);
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
            self.compile_block(else_stmts);
            if self.failed {
                return;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
    }

    // ── Raise / Delete / Import ───────────────────────────────────────────────

    fn compile_raise(&mut self, expr: Option<&Expr>, cause: Option<&Expr>) {
        match expr {
            None => {
                self.emit(Insn::RaiseReRaise);
            }
            Some(e) => {
                let r = self.compile_expr(e);
                if let Some(cause_expr) = cause {
                    let c = self.compile_expr(cause_expr);
                    self.emit(Insn::RaiseFrom(r, c));
                    self.free_temp(c);
                } else {
                    self.emit(Insn::RaiseValue(r));
                }
                self.free_temp(r);
            }
        }
    }

    /// Build the 3-element slice-key tuple `(lo, hi, step)` used by GetItem/SetItem/DeleteItem.
    /// Each missing bound is represented as `None`. Returns the register holding the tuple.
    fn compile_slice_key(
        &mut self,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Reg {
        let none_reg = |this: &mut Self| {
            let r = this.alloc_temp();
            this.emit(Insn::LoadNone(r));
            r
        };
        let lower_r = lower
            .map(|e| self.compile_expr(e))
            .unwrap_or_else(|| none_reg(self));
        let upper_r = upper
            .map(|e| self.compile_expr(e))
            .unwrap_or_else(|| none_reg(self));
        let step_r = step
            .map(|e| self.compile_expr(e))
            .unwrap_or_else(|| none_reg(self));
        // Arrange contiguously for BuildTuple(base, 3)
        let base_r = lower_r;
        if upper_r != base_r + 1 {
            let t = base_r + 1;
            if t > self.max_reg {
                self.max_reg = t;
            }
            self.emit(Insn::Move(t, upper_r));
            self.free_temp(upper_r);
        }
        let upper_slot = base_r + 1;
        if step_r != upper_slot + 1 {
            let t = upper_slot + 1;
            if t > self.max_reg {
                self.max_reg = t;
            }
            self.emit(Insn::Move(t, step_r));
            self.free_temp(step_r);
        }
        let slice_r = self.alloc_temp();
        self.emit(Insn::BuildTuple(slice_r, base_r, 3));
        self.next_temp = slice_r + 1;
        slice_r
    }

    fn compile_delete(&mut self, expr: &Expr) {
        match expr {
            Expr::Var(name) => {
                if let Some(reg) = self.local_reg(name) {
                    self.emit(Insn::DeleteLocal(reg));
                    self.maybe_record_class_del(reg);
                    // Issue #820: at module scope, also remove the name from
                    // env.values and module_globals_dict so that LoadGlobal
                    // from nested functions / after globals() cannot resurrect it.
                    if self.is_module_scope {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::DeleteModuleGlobal(name_idx));
                    }
                } else {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::DeleteName(name_idx));
                }
            }
            Expr::Attr { target, name } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                self.emit(Insn::DeleteAttr(obj, name_idx));
                self.free_temp(obj);
            }
            Expr::Index { target, index } => {
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
        let mod_idx = self.intern_name(module);
        let mod_reg = self.alloc_temp();
        self.emit(Insn::ImportModule(mod_reg, mod_idx));
        if names.len() == 1 && names[0].0 == "*" {
            // Star import: handled at runtime by a special path in ImportModule.
            // Use StoreGlobal with a sentinel name to trigger star import.
            let star_idx = self.intern_name("*");
            self.emit(Insn::StoreGlobal(star_idx, mod_reg));
        } else {
            for (attr_name, alias) in names {
                let attr_idx = self.intern_name(attr_name);
                let val_reg = self.alloc_temp();
                self.emit(Insn::GetAttr(val_reg, mod_reg, attr_idx));
                let bound = alias.as_deref().unwrap_or(attr_name);
                self.compile_store_name(bound, val_reg);
                self.free_temp(val_reg);
            }
        }
        self.free_temp(mod_reg);
    }

    // ── Def / Class ───────────────────────────────────────────────────────────

    fn compile_def(
        &mut self,
        name: &str,
        params: &[FunctionParam],
        body: &[Stmt],
        decorators: &[Expr],
        return_annotation: Option<&Expr>,
    ) {
        // Build inner function's scope metadata.
        let inner_global = crate::interpreter::collect_global_names(body);
        let inner_nonlocal = crate::interpreter::collect_nonlocal_names(body);
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
        // Include `name` so self-recursive calls are treated as pure (fixpoint assumption).
        let mut pure_fns_with_self = self.pure_locals.clone();
        pure_fns_with_self.insert(name.to_string());
        let is_pure = crate::interpreter::is_pure_body(body, &pure_fns_with_self);

        // Detect cell vars for the inner function.
        let inner_cell_vars = collect_cell_vars(body, &inner_index_rc);

        // Validate annotation targets against global/nonlocal declarations.
        // CPython 3.12 raises SyntaxError for `def f(): global x; x: int` and
        // `def f(): nonlocal x; x: int` (issue #748 / companion to #770).
        let def_ann_targets = crate::interpreter::collect_annotation_target_names(body);
        for ann_name in &def_ann_targets {
            if inner_global.contains(ann_name) {
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(format!("annotated name '{}' can't be global", ann_name));
                }
                return;
            }
            if inner_nonlocal.contains(ann_name) {
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some(format!("annotated name '{}' can't be nonlocal", ann_name));
                }
                return;
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
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some(format!("no binding for nonlocal '{}' found", nonlocal_name));
                }
                return;
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
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(format!("annotated name '{}' can't be global", ann_name));
                }
                return;
            }
            if inner_nonlocal_rc.contains(ann_name) {
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some(format!("annotated name '{}' can't be nonlocal", ann_name));
                }
                return;
            }
        }

        let mut sub = Compiler::new(
            Rc::clone(&inner_index_rc),
            def_bound,
            inner_cell_vars.clone(),
        );
        // Thread the enclosing function scope chain into the child compiler.
        // Since compile_def always produces a function scope, add self.local_index
        // (if self is a function scope) and mark the child as a function scope.
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
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
        if is_pure {
            // Seed the inner compiler with the function's own name so that
            // direct self-recursive calls are compiled as CallMemo rather than
            // Call.  This lets the VM return from the fn_cache on repeated
            // invocations without re-entering call_function_expanded at all,
            // making recursive pure functions (e.g. fib) substantially faster.
            // This is sound: a pure function calling only itself (and other
            // pure things) is itself pure, satisfying the fixpoint assumption.
            sub.pure_locals.insert(name.to_string());
        }
        sub.compile_block(body);
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
                return;
            }
        };

        if self.fn_protos.len() >= 256 {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many nested functions in one scope (max 256)".to_string());
            }
            return;
        }
        let proto_idx = self.fn_protos.len() as u8;
        let local_names = Rc::new(inner_index_rc.keys().cloned().collect::<HashSet<_>>());
        // Collect annotation keys: annotated param names (in declaration order) then
        // "return" if there is a return annotation.  These are parallel to the
        // annotation register window emitted just before MakeFunction.
        let annotation_keys: Vec<String> = params
            .iter()
            .filter(|p| p.annotation.is_some())
            .map(|p| p.name.clone())
            .chain(return_annotation.map(|_| "return".to_string()))
            .collect();
        self.fn_protos.push(FnProto {
            name: name.to_string(),
            qualname: fn_qualname,
            param_spec: Rc::new(FnParamSpec {
                names: params.iter().map(|p| p.name.clone()).collect(),
                has_default: params.iter().map(|p| p.default.is_some()).collect(),
                is_args: params.iter().map(|p| p.is_args).collect(),
                is_kwargs: params.iter().map(|p| p.is_kwargs).collect(),
                is_keyword_only: params.iter().map(|p| p.is_keyword_only).collect(),
                is_positional_only: params.iter().map(|p| p.is_positional_only).collect(),
            }),
            code: Rc::new(inner_code),
            local_index: inner_index_rc,
            local_names,
            global_names: inner_global_rc,
            nonlocal_names: inner_nonlocal_rc,
            is_pure,
            annotation_keys,
        });

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
                return;
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

        // Compile annotation expressions (evaluated in enclosing scope, like defaults).
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
                return;
            }
            self.next_temp += Reg::from(annots_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (slot_i, (_, annot_expr)) in (0u32..).zip(annotated_params.iter()) {
                let saved = self.next_temp;
                let r = self.compile_expr(annot_expr);
                if r != annots_base + slot_i {
                    self.emit(Insn::Move(annots_base + slot_i, r));
                }
                self.next_temp = saved;
            }
            if let Some(ret_annot) = return_annotation {
                let slot_i = annots_n as u32 - 1;
                let saved = self.next_temp;
                let r = self.compile_expr(ret_annot);
                if r != annots_base + slot_i {
                    self.emit(Insn::Move(annots_base + slot_i, r));
                }
                self.next_temp = saved;
            }
        }

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
            // The base of whichever block came first is the new watermark.
            if defs_n > 0 {
                self.next_temp = defs_base + 1;
            } else {
                self.next_temp = annots_base + 1;
            }
        }

        // Apply decorators (outermost first in reverse declaration order).
        let mut val_reg = dst;
        for deco_expr in decorators.iter().rev() {
            let frame = self.next_temp;
            if frame.checked_add(2).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("too many registers for decorator application".to_string());
                }
                return;
            }
            self.next_temp = frame + 2;
            if frame + 1 > self.max_reg {
                self.max_reg = frame + 1;
            }
            let saved = self.next_temp;
            self.compile_expr_into(deco_expr, frame);
            self.next_temp = saved;
            self.emit(Insn::Move(frame + 1, val_reg));
            self.emit(Insn::Call(frame, 1));
            self.next_temp = frame + 1;
            val_reg = frame;
        }

        self.compile_store_name(name, val_reg);
        if let Some(reg) = self.local_reg(name) {
            self.mark_def(reg);
        }
        if is_pure && decorators.is_empty() {
            self.pure_locals.insert(name.to_string());
        }
        self.free_temp(dst);
    }

    fn compile_class(
        &mut self,
        name: &str,
        bases: &[Expr],
        metaclass: Option<&Expr>,
        body: &[Stmt],
        decorators: &[Expr],
    ) {
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
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg =
                            Some(format!("no binding for nonlocal '{}' found", nonlocal_name));
                    }
                    return;
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
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(format!("annotated name '{}' can't be global", ann_name));
                }
                return;
            }
            if body_nonlocal.contains(ann_name) {
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some(format!("annotated name '{}' can't be nonlocal", ann_name));
                }
                return;
            }
        }

        let body_local =
            crate::interpreter::collect_local_names(&[], body, &body_global, &body_nonlocal_rc);
        // Allocate a register slot for every potential class-body local.
        // Slot order is **not** used to encode class-namespace insertion
        // order any more — the order CPython exposes via `vars(C)` is the
        // order stores actually executed at runtime, not source-walk order.
        // Each store now emits `Insn::RecordClassStore(slot)` and the VM
        // builds the attrs dict from that runtime trace inside `MakeClass`.
        // We still walk the body textually here so register numbers stay
        // stable across runs (HashSet iteration order is randomised, which
        // would otherwise cause spurious bytecode diffs).
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
        sub.qualname_prefix = class_qualname.clone();
        // Thread the enclosing function scope chain into the class body compiler.
        // Class scope is transparent to `nonlocal` (not a function scope), so we
        // pass through outer_locals without adding body_index_rc, and leave
        // is_function_scope = false.
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
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
                return;
            }
        };
        if self.fn_protos.len() >= 256 {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many nested classes/functions in one scope (max 256)".to_string());
            }
            return;
        }
        let proto_idx = self.fn_protos.len() as u8;
        let local_names = Rc::new(body_index_rc.keys().cloned().collect::<HashSet<_>>());
        self.fn_protos.push(FnProto {
            name: name.to_string(),
            qualname: class_qualname,
            param_spec: Rc::new(FnParamSpec {
                names: vec![],
                has_default: vec![],
                is_args: vec![],
                is_kwargs: vec![],
                is_keyword_only: vec![],
                is_positional_only: vec![],
            }),
            code: Rc::new(body_code),
            local_index: body_index_rc,
            local_names,
            global_names: body_global,
            nonlocal_names: body_nonlocal_rc,
            is_pure: false,
            annotation_keys: Vec::new(),
        });

        // Compile base class expressions.
        let bases_n = bases.len() as u8;
        let bases_base = self.next_temp;
        if bases_n > 0 {
            if self.next_temp.checked_add(Reg::from(bases_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many base class registers".to_string());
                }
                return;
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

        let name_idx = self.intern_name(name);
        let dst = self.alloc_temp();
        self.emit(Insn::MakeClass(
            dst, proto_idx, bases_base, bases_n, name_idx,
        ));
        if bases_n > 0 && metaclass.is_none() {
            // Without metaclass, the base registers are dead after MakeClass.
            // (With metaclass, dst sits at bases_base + bases_n and must stay live.)
            self.next_temp = bases_base + 1;
        }

        // If a metaclass is provided, replace `dst` with the result of
        // `metaclass(name_str, bases_tuple, namespace_dict)`.
        if let Some(meta_expr) = metaclass {
            // 1. Build the bases tuple from already-evaluated bases.
            //    Since the bases registers may have been freed above, we recompile
            //    them into a fresh contiguous region.
            let tup_base = self.next_temp;
            if self
                .next_temp
                .checked_add(Reg::from(bases_n.max(1)))
                .is_none()
            {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many registers for metaclass call".to_string());
                }
                return;
            }
            self.next_temp += Reg::from(bases_n);
            if bases_n > 0 && self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (i, base_expr) in (0u32..).zip(bases.iter()) {
                let saved = self.next_temp;
                let r = self.compile_expr(base_expr);
                if r != tup_base + i {
                    self.emit(Insn::Move(tup_base + i, r));
                }
                self.next_temp = saved;
            }
            let bases_tuple_reg = self.alloc_temp();
            self.emit(Insn::BuildTuple(bases_tuple_reg, tup_base, bases_n));
            // Note: we keep the [tup_base..bases_tuple_reg] region allocated so
            // bases_tuple_reg isn't clobbered by subsequent temp allocations.

            // 2. Call vars(dst) to get the class namespace proxy, then
            //    dict(proxy) to convert to a mutable plain dict for the metaclass.
            //    Register layout (3 slots):
            //      vars_frame+0  -- function (vars / dict)
            //      vars_frame+1  -- arg
            //      vars_frame+2  -- stash proxy while loading dict fn
            let vars_name_idx = self.intern_name("vars");
            let dict_name_idx_meta = self.intern_name("dict");
            let vars_frame = self.next_temp;
            if vars_frame.checked_add(3).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many registers for metaclass call".to_string());
                }
                return;
            }
            self.next_temp = vars_frame + 3;
            if vars_frame + 2 > self.max_reg {
                self.max_reg = vars_frame + 2;
            }
            // vars(dst) -> proxy in vars_frame
            self.emit(Insn::LoadGlobal(vars_frame, vars_name_idx));
            self.emit(Insn::Move(vars_frame + 1, dst));
            self.emit(Insn::Call(vars_frame, 1));
            // dict(proxy) -> plain dict in vars_frame
            self.emit(Insn::Move(vars_frame + 2, vars_frame));
            self.emit(Insn::LoadGlobal(vars_frame, dict_name_idx_meta));
            self.emit(Insn::Move(vars_frame + 1, vars_frame + 2));
            self.emit(Insn::Call(vars_frame, 1));
            let ns_reg = vars_frame; // result of dict(vars(dst))

            // 3. Set up call frame for metaclass(name_str, bases_tuple, namespace).
            let frame = self.next_temp;
            if frame.checked_add(4).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many registers for metaclass call".to_string());
                }
                return;
            }
            self.next_temp = frame + 4;
            if frame + 3 > self.max_reg {
                self.max_reg = frame + 3;
            }
            let saved = self.next_temp;
            self.compile_expr_into(meta_expr, frame);
            self.next_temp = saved;
            let name_const = self.intern_const(Value::string(name));
            self.emit(Insn::LoadConst(frame + 1, name_const));
            self.emit(Insn::Move(frame + 2, bases_tuple_reg));
            self.emit(Insn::Move(frame + 3, ns_reg));
            self.emit(Insn::Call(frame, 3));
            self.emit(Insn::Move(dst, frame));
            self.next_temp = dst + 1;
        }

        let mut val_reg = dst;
        for deco_expr in decorators.iter().rev() {
            let frame = self.next_temp;
            if frame.checked_add(2).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("too many registers for decorator application".to_string());
                }
                return;
            }
            self.next_temp = frame + 2;
            if frame + 1 > self.max_reg {
                self.max_reg = frame + 1;
            }
            let saved = self.next_temp;
            self.compile_expr_into(deco_expr, frame);
            self.next_temp = saved;
            self.emit(Insn::Move(frame + 1, val_reg));
            self.emit(Insn::Call(frame, 1));
            self.next_temp = frame + 1;
            val_reg = frame;
        }

        self.compile_store_name(name, val_reg);
        if let Some(reg) = self.local_reg(name) {
            self.mark_def(reg);
        }
        self.free_temp(dst);
    }

    // ── Try / With ────────────────────────────────────────────────────────────

    fn compile_try(
        &mut self,
        body: &[Stmt],
        handlers: &[crate::ast::ExceptHandler],
        else_branch: Option<&[Stmt]>,
        finally_branch: Option<&[Stmt]>,
    ) {
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
        self.compile_block(body);

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
            self.compile_block(else_stmts);
            if self.failed {
                return;
            }
        }
        // Normal finally exit
        if outer_finally_patch.is_some() {
            self.emit(Insn::PopExcept);
            let finally_stmts = finally_branch.unwrap();
            self.compile_block(finally_stmts);
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
                // handler body (break/continue/return) emit EndExcept and inline
                // the finally block before jumping.
                self.except_cleanups.push(EarlyExitCleanup::ExceptBody {
                    finally_stmts: finally_branch.map(|s| s.to_vec()),
                });

                self.compile_block(&handler.body);

                // Remove the except-body cleanup before emitting normal handler exit.
                self.except_cleanups.pop();

                if self.failed {
                    return;
                }
                // PEP 3110: delete the `as VAR` binding when the handler exits
                // (breaks reference cycles and matches CPython behaviour).
                if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        self.emit(Insn::DeleteLocal(reg));
                    } else {
                        let name_idx = self.intern_name(var_name);
                        self.emit(Insn::DeleteName(name_idx));
                    }
                }
                self.emit(Insn::EndExcept);

                // Run finally (inline) after successful handler
                if let Some(finally_stmts) = finally_branch {
                    self.compile_block(finally_stmts);
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

            // No handler matched: re-raise (outer finally will catch it if present)
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
            self.compile_block(finally_stmts);
            if self.failed {
                return;
            }
            self.emit(Insn::RaiseReRaise);
        }

        // Patch all successful handler jumps to here (after everything)
        self.patch_jump(end_patch);
        for idx in handler_end_patches {
            self.patch_jump(idx);
        }
    }

    fn compile_with(&mut self, items: &[(Expr, Option<AssignTarget>)], body: &[Stmt]) {
        // Compile nested with items recursively (outermost first).
        if items.is_empty() {
            self.compile_block(body);
            return;
        }
        let (expr, alias) = &items[0];
        let rest = &items[1..];

        // ctx = expr
        let ctx_reg = self.compile_expr(expr);

        // VAR = ctx.__enter__()
        let enter_name_idx = self.intern_name("__enter__");
        let enter_reg = self.alloc_temp();
        self.emit(Insn::GetAttr(enter_reg, ctx_reg, enter_name_idx));
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

        // Compile nested with items or body
        if rest.is_empty() {
            self.compile_block(body);
        } else {
            self.compile_with(rest, body);
        }
        if self.failed {
            return;
        }

        // Normal exit
        self.emit(Insn::PopExcept);
        // ctx.__exit__(None, None, None)
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
        self.emit(Insn::GetAttr(exit_frame, ctx_reg, exit_name_idx));
        self.emit(Insn::LoadNone(exit_frame + 1));
        self.emit(Insn::LoadNone(exit_frame + 2));
        self.emit(Insn::LoadNone(exit_frame + 3));
        self.emit(Insn::Call(exit_frame, 3));
        self.next_temp = exit_frame;
        let end_patch = self.emit(Insn::Jump(0));

        // Exception path
        self.patch_jump(setup_patch);
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
        self.emit(Insn::GetAttr(exit_frame2, ctx_reg, exit_name_idx));
        self.emit(Insn::GetAttr(exit_frame2 + 1, exc_tmp, class_name_idx)); // exc_type
        self.emit(Insn::Move(exit_frame2 + 2, exc_tmp));
        self.emit(Insn::Move(exit_frame2 + 3, exc_tmp)); // traceback (non-None placeholder)
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
            | Insn::UnaryOp(d, ..)
            | Insn::LoadConst(d, ..)
            | Insn::LoadNone(d)
            | Insn::LoadGlobal(d, ..)
            | Insn::Move(d, ..)
            | Insn::GetAttr(d, ..)
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

    fn emit_aug_binop(&mut self, reg: Reg, op: BinaryOp, expr: &Expr) {
        if let Some(const_idx) = self.try_literal_const_idx(expr) {
            self.emit(Insn::BinOpConst(reg, reg, op, const_idx));
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
            Expr::Var(name) => {
                if let Some(reg) = self.local_reg(name) {
                    let definitely_bound = (reg as usize) < 64 && (self.def_set >> reg) & 1 != 0;
                    if !definitely_bound {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::CheckLocal(reg, name_idx));
                    }
                    reg
                } else {
                    // global / nonlocal / cell / free variable
                    let name_idx = self.intern_name(name);
                    let dst = self.alloc_temp();
                    self.emit(Insn::LoadGlobal(dst, name_idx));
                    dst
                }
            }
            Expr::Unary { op, expr } => {
                let src = self.compile_expr(expr);
                let dst = self.ensure_dst(src);
                self.emit(Insn::UnaryOp(dst, *op, src));
                dst
            }
            Expr::Binary { left, op, right } => match op {
                BinaryOp::And => self.compile_short_circuit(left, right, false),
                BinaryOp::Or => self.compile_short_circuit(left, right, true),
                _ => {
                    let lhs = self.compile_expr(left);
                    let dst = self.ensure_dst(lhs);
                    let rhs = self.compile_expr(right);
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
            Expr::Call { func, args } => self.compile_call(func, args),
            Expr::Attr { target, name } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                let dst = self.ensure_dst(obj);
                self.emit(Insn::GetAttr(dst, obj, name_idx));
                dst
            }
            Expr::Index { target, index } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                let dst = self.ensure_dst(obj);
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
                let obj = self.compile_expr(target);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                let dst = self.ensure_dst(obj);
                self.emit(Insn::GetItem(dst, obj, slice_r));
                self.free_temp(slice_r);
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
            Expr::Lambda { params, body } => {
                let fp: Vec<FunctionParam> = params
                    .iter()
                    .map(|n| FunctionParam {
                        name: n.clone(),
                        default: None,
                        annotation: None,
                        is_args: false,
                        is_kwargs: false,
                        is_keyword_only: false,
                        is_positional_only: false,
                    })
                    .collect();
                self.compile_lambda(&fp, body)
            }
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
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("'yield' outside function".to_string());
                    }
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
                    self.failed = true;
                    self.is_syntax_error = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("'yield' outside function".to_string());
                    }
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
        }
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
                } => {
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
                            self.emit(Insn::Call(frame, 1));
                            self.next_temp = frame + 1;
                            frame
                        }
                        _ => val_r,
                    };
                    // Apply format spec if present.  The spec is itself a
                    // mini f-string (literals plus nested `{expr}` parts), so
                    // we compile it via the same fstring helper to obtain a
                    // single string register, then call `format(val, spec)`.
                    if let Some(spec_parts) = format_spec {
                        let spec_r = self.compile_fstring(spec_parts);
                        let frame = self.next_temp;
                        if frame + 2 > self.max_reg {
                            self.max_reg = frame + 2;
                        }
                        self.next_temp = frame + 3;
                        let fmt_idx = self.intern_name("format");
                        self.emit(Insn::LoadGlobal(frame, fmt_idx));
                        if val_r != frame + 1 {
                            self.emit(Insn::Move(frame + 1, val_r));
                        }
                        self.free_temp(val_r);
                        if spec_r != frame + 2 {
                            self.emit(Insn::Move(frame + 2, spec_r));
                        }
                        self.free_temp(spec_r);
                        self.emit(Insn::Call(frame, 2));
                        self.next_temp = frame + 1;
                        frame
                    } else {
                        // format(val, "") — dispatch __format__("") per Python semantics
                        let frame = self.next_temp;
                        if frame + 2 > self.max_reg {
                            self.max_reg = frame + 2;
                        }
                        self.next_temp = frame + 3;
                        let fmt_idx = self.intern_name("format");
                        self.emit(Insn::LoadGlobal(frame, fmt_idx));
                        if val_r != frame + 1 {
                            self.emit(Insn::Move(frame + 1, val_r));
                        }
                        self.free_temp(val_r);
                        let empty_r = self.compile_literal(Value::string(String::new()));
                        if empty_r != frame + 2 {
                            self.emit(Insn::Move(frame + 2, empty_r));
                        }
                        self.free_temp(empty_r);
                        self.emit(Insn::Call(frame, 2));
                        self.next_temp = frame + 1;
                        frame
                    }
                }
            };
            part_regs.push(r);
        }

        // Concatenate all parts with BinOp(Add).
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

    /// Emit the nested for+if loop structure shared by all comprehension kinds.
    ///
    /// `acc` is the accumulator register (already initialised with an empty
    /// list/dict/set).  For each element that passes all filters, `emit_body` is
    /// called with `(compiler, item_reg, acc_reg)` to append/insert the value.
    fn compile_comp_loops(
        &mut self,
        clauses: &[CompClause],
        acc: Reg,
        emit_body: &mut impl FnMut(&mut Self, Reg),
    ) {
        if clauses.is_empty() {
            return;
        }
        let clause = &clauses[0];

        let iter_slot = self.alloc_iter();
        let src = self.compile_expr(&clause.iter);
        self.emit(Insn::GetIter(iter_slot, src));
        self.free_temp(src);

        let loop_start = self.pc();

        // Choose a destination register for the loop variable.
        let item_reg = if let AssignTarget::Name(n) = &clause.target {
            self.local_reg(n).unwrap_or_else(|| self.alloc_temp())
        } else {
            self.alloc_temp()
        };

        let exit_jmp = self.emit(Insn::ForIter(item_reg, iter_slot, 0));

        // Assign loop variable (same logic as compile_for).
        match &clause.target {
            AssignTarget::Name(name) => {
                if self.local_reg(name).is_none() {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::StoreGlobal(name_idx, item_reg));
                    // item_reg is a temp that was just stored; keep it alive for body use.
                }
            }
            AssignTarget::Tuple(targets) => {
                let n = targets.len() as u32;
                let base = item_reg + 1;
                self.next_temp = base + n;
                if self.next_temp - 1 > self.max_reg {
                    self.max_reg = self.next_temp - 1;
                }
                self.emit(Insn::Unpack(base, item_reg, n));
                for (i, t) in (0u32..).zip(targets.iter()) {
                    match t {
                        AssignTarget::Name(name) => {
                            if let Some(reg) = self.local_reg(name) {
                                self.emit(Insn::Move(reg, base + i));
                            } else {
                                let name_idx = self.intern_name(name);
                                self.emit(Insn::StoreGlobal(name_idx, base + i));
                            }
                        }
                        _ => {
                            self.failed = true;
                            if self.error_msg.is_none() {
                                self.error_msg =
                                    Some("unsupported comprehension unpack target".to_string());
                            }
                            return;
                        }
                    }
                }
                self.next_temp = item_reg;
            }
            _ => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("unsupported comprehension target".to_string());
                }
                return;
            }
        }

        // Optional `if` filter.
        let skip_jmp = if let Some(cond) = &clause.cond {
            let cond_reg = self.compile_expr(cond);
            let j = self.emit(Insn::JumpIfFalse(cond_reg, 0));
            self.free_temp(cond_reg);
            Some(j)
        } else {
            None
        };

        // Recurse for nested clauses, or emit the accumulator body.
        if clauses.len() > 1 {
            self.compile_comp_loops(&clauses[1..], acc, emit_body);
        } else {
            emit_body(self, acc);
        }

        // Patch the `if` skip jump.
        if let Some(j) = skip_jmp {
            self.patch_jump(j);
        }

        // Jump back to loop top.
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));

        self.patch_jump(exit_jmp);
        self.free_iter();
        if let AssignTarget::Name(_) = &clause.target {
            if item_reg >= self.base_temp {
                self.free_temp(item_reg);
            }
        } else {
            self.free_temp(item_reg);
        }
    }

    fn compile_list_comp(&mut self, elt: &Expr, clauses: &[CompClause]) -> Reg {
        // Allocate the result register first (before any loop temps).
        let acc = self.alloc_temp();
        self.emit(Insn::BuildList(acc, acc, 0));

        // Save/restore next_temp around the loop body so temps don't accumulate.
        let saved_temp = self.next_temp;

        self.compile_comp_loops(clauses, acc, &mut |this, acc_reg| {
            let val = this.compile_expr(elt);
            this.emit(Insn::ListAppend(acc_reg, val));
            this.free_temp(val);
            this.next_temp = saved_temp;
        });

        acc
    }

    fn compile_dict_comp(&mut self, key: &Expr, val: &Expr, clauses: &[CompClause]) -> Reg {
        let acc = self.alloc_temp();
        // BuildDict with 0 pairs → empty dict.
        self.emit(Insn::BuildDict(acc, acc, 0));

        let saved_temp = self.next_temp;

        self.compile_comp_loops(clauses, acc, &mut |this, acc_reg| {
            let k = this.compile_expr(key);
            let v = this.compile_expr(val);
            this.emit(Insn::SetItem(acc_reg, k, v));
            this.free_temp(v);
            this.free_temp(k);
            this.next_temp = saved_temp;
        });

        acc
    }

    fn compile_set_comp(&mut self, elt: &Expr, clauses: &[CompClause]) -> Reg {
        // Build an empty set via set() call.
        let acc = self.alloc_temp();
        let set_name_idx = self.intern_name("set");
        self.emit(Insn::LoadGlobal(acc, set_name_idx));
        // Call set() with zero args: Call(acc, 0) — result in acc.
        self.emit(Insn::Call(acc, 0));

        let saved_temp = self.next_temp;

        self.compile_comp_loops(clauses, acc, &mut |this, acc_reg| {
            let val = this.compile_expr(elt);
            this.emit(Insn::SetAdd(acc_reg, val));
            this.free_temp(val);
            this.next_temp = saved_temp;
        });

        acc
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

        // Evaluate the outermost iterable in the enclosing (current) scope
        // before creating the nested function.
        let iter_reg = self.compile_expr(&clauses[0].iter);

        // Use ".0" as the implicit parameter name — matches CPython's internal
        // convention and is not a valid Python identifier (cannot be lexed), so
        // user code inside the genexp body cannot accidentally reference or
        // shadow it.
        const IT_PARAM: &str = ".0";

        // Build the inner body working from the innermost clause outward.
        // Start with: yield elt
        let yield_stmt = Stmt::Expr(Expr::Yield(Some(Box::new(elt.clone()))));

        // Wrap in if-cond guard for the first clause if present, then
        // for each additional clause build a nested for-if structure.
        //
        // clauses[0]: for target in IT_PARAM (if cond0)?
        // clauses[1..]: for target in iter (if cond)?
        //
        // Inner body (innermost clause to outermost inner clause):
        let mut body = vec![yield_stmt];
        for clause in clauses[1..].iter().rev() {
            // Optional if-cond filter.
            if let Some(cond) = &clause.cond {
                body = vec![Stmt::If {
                    branches: vec![(cond.clone(), body)],
                    else_branch: None,
                }];
            }
            body = vec![Stmt::For {
                target: clause.target.clone(),
                iter: clause.iter.clone(),
                body,
                else_branch: None,
            }];
        }
        // Wrap the first clause's optional if-cond around the body.
        if let Some(cond) = &clauses[0].cond {
            body = vec![Stmt::If {
                branches: vec![(cond.clone(), body)],
                else_branch: None,
            }];
        }
        // Outermost loop: iterate over the parameter.
        body = vec![Stmt::For {
            target: clauses[0].target.clone(),
            iter: Expr::Var(IT_PARAM.to_string()),
            body,
            else_branch: None,
        }];

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
        let is_pure = false;
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
                self.failed = true;
                self.is_syntax_error = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some(format!("no binding for nonlocal '{}' found", nonlocal_name));
                }
                return 0;
            }
        }

        let mut sub = Compiler::new(Rc::clone(&inner_index_rc), def_bound, inner_cell_vars);
        // Comprehensions create an implicit function scope; thread outer_locals.
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
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

        if self.fn_protos.len() >= 256 {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many nested functions in one scope (max 256)".to_string());
            }
            return 0;
        }
        let proto_idx = self.fn_protos.len() as u8;
        let local_names = Rc::new(inner_index_rc.keys().cloned().collect::<HashSet<_>>());
        self.fn_protos.push(FnProto {
            name: "<genexp>".to_string(),
            qualname: "<genexp>".to_string(),
            param_spec: Rc::new(FnParamSpec {
                names: params.iter().map(|p| p.name.clone()).collect(),
                has_default: params.iter().map(|p| p.default.is_some()).collect(),
                is_args: params.iter().map(|p| p.is_args).collect(),
                is_kwargs: params.iter().map(|p| p.is_kwargs).collect(),
                is_keyword_only: params.iter().map(|p| p.is_keyword_only).collect(),
                is_positional_only: params.iter().map(|p| p.is_positional_only).collect(),
            }),
            code: Rc::new(inner_code),
            local_index: inner_index_rc,
            local_names,
            global_names: inner_global_rc,
            nonlocal_names: inner_nonlocal_rc,
            is_pure,
            annotation_keys: Vec::new(),
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

    fn compile_call(&mut self, func: &Expr, args: &[crate::ast::CallArg]) -> Reg {
        // Check for any splat args — these require a variadic call path.
        let has_splat = args.iter().any(|a| a.splat || a.double_splat);
        let has_kwargs = args.iter().any(|a| a.name.is_some());

        if has_splat || has_kwargs {
            // Variadic call: build separate positional and keyword lists, then
            // use the ExpandedCall instruction.
            return self.compile_variadic_call(func, args);
        }

        // Detect obj.method(args) — emit CallMethod to allow in-place mutation.
        if let Expr::Attr { target, name } = func {
            return self.compile_method_call(target, name, args);
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
        let is_pure_callee = matches!(func, Expr::Var(n) if self.pure_locals.contains(n.as_str()));
        if is_pure_callee {
            self.emit(Insn::CallMemo(func_reg, argc));
        } else {
            self.emit(Insn::Call(func_reg, argc));
        }
        self.next_temp = func_reg + 1;
        func_reg
    }

    fn compile_method_call(
        &mut self,
        target: &Expr,
        method_name: &str,
        args: &[crate::ast::CallArg],
    ) -> Reg {
        let nargs = args.len() as u8;

        // When the receiver is a plain fast-local variable, use its register directly
        // as `obj` so that in-place mutations (append, pop, …) actually update the
        // variable.  The return value goes into a fresh temp `dst_reg ≠ obj_reg`.
        // For all other receivers we fall back to copying the value into a temp and
        // using the same register for both obj and dst.
        let (obj_reg, dst_reg, args_base, need_copy) = if let Expr::Var(name) = target {
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
        if let Expr::Attr { target, name } = func {
            // Same fast-local optimisation as compile_method_call: use the
            // variable's own register as `obj` so mutations persist.
            let (obj_reg, dst_reg) = if let Expr::Var(tname) = target.as_ref() {
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

            for arg in args {
                if arg.splat {
                    let val = self.compile_expr(&arg.value);
                    self.emit(Insn::ListExtend(pos_list_reg, val));
                    self.free_temp(val);
                } else if arg.double_splat {
                    let val = self.compile_expr(&arg.value);
                    self.emit(Insn::DictUpdate(kw_dict_reg, val));
                    self.free_temp(val);
                } else if let Some(kw_name) = &arg.name {
                    let val = self.compile_expr(&arg.value);
                    let key_idx = self.intern_const(Value::string(kw_name.clone()));
                    let key_reg = self.alloc_temp();
                    self.emit(Insn::LoadConst(key_reg, key_idx));
                    self.emit(Insn::SetItem(kw_dict_reg, key_reg, val));
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

        for arg in args {
            if arg.splat {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::ListExtend(pos_list_reg, val));
                self.free_temp(val);
            } else if arg.double_splat {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::DictUpdate(kw_dict_reg, val));
                self.free_temp(val);
            } else if let Some(kw_name) = &arg.name {
                let val = self.compile_expr(&arg.value);
                let key_idx = self.intern_const(Value::string(kw_name.clone()));
                let key_reg = self.alloc_temp();
                self.emit(Insn::LoadConst(key_reg, key_idx));
                self.emit(Insn::SetItem(kw_dict_reg, key_reg, val));
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
        self.free_temp(f3);
        self.free_temp(f2);
        self.free_temp(f1);
        self.free_temp(vcall_reg);
        self.free_temp(kw_dict_reg);
        self.free_temp(pos_list_reg);
        self.free_temp(func_reg);
        // Return value is in vcall_reg
        vcall_reg
    }

    fn compile_lambda(&mut self, params: &[FunctionParam], body: &Expr) -> Reg {
        // Convert lambda body into an implicit return statement.
        let body_stmts = vec![Stmt::Return(Some(body.clone()))];
        let temp_name = "<lambda>";
        self.compile_def(temp_name, params, &body_stmts, &[], None);
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
        let n = items.len() as u8;
        let base = self.next_temp;
        if base.checked_add(Reg::from(n)).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!(
                    "too many elements in {} literal",
                    if is_tuple { "tuple" } else { "list" }
                ));
            }
            return 0;
        }
        self.next_temp = base + Reg::from(n);
        // Always update max_reg with `base` — BuildList/BuildTuple always writes
        // to `base` regardless of element count (even empty collections).
        let max_used = if n > 0 { base + Reg::from(n) - 1 } else { base };
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
            let n = items.len() as u8;
            let frame = self.next_temp;
            if frame.checked_add(1 + Reg::from(n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many elements in set literal".to_string());
                }
                return 0;
            }
            self.next_temp = frame + 1 + Reg::from(n);
            if frame + Reg::from(n) > self.max_reg {
                self.max_reg = frame + Reg::from(n);
            }
            let set_name_idx = self.intern_name("set");
            self.emit(Insn::LoadGlobal(frame, set_name_idx));
            let list_r = frame + 1;
            let saved = self.next_temp;
            let list_base = self.next_temp;
            if list_base.checked_add(Reg::from(n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many elements in set literal".to_string());
                }
                return 0;
            }
            self.next_temp = list_base + Reg::from(n);
            if list_base + Reg::from(n) - 1 > self.max_reg {
                self.max_reg = list_base + Reg::from(n) - 1;
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
            let n = items.len() as u8;
            let base = self.next_temp;
            let slots_needed = Reg::from(n).saturating_mul(2);
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
            self.next_temp = base + Reg::from(n).saturating_mul(2);
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
