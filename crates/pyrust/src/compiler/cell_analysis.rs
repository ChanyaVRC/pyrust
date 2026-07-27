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
