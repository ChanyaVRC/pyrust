pub(crate) fn collect_global_names(body: &[Stmt]) -> HashSet<String> {
    collect_declared_names(body, |s| {
        if let Stmt::Global(names) = s {
            Some(names)
        } else {
            None
        }
    })
}

pub(crate) fn collect_nonlocal_names(body: &[Stmt]) -> HashSet<String> {
    collect_declared_names(body, |s| {
        if let Stmt::Nonlocal(names) = s {
            Some(names)
        } else {
            None
        }
    })
}

/// Collect the names used as annotation targets (`x: T` or `x: T = v`) in the
/// direct body, without descending into nested `Def` or `Class` scopes.  Used
/// by `compile_def` to detect conflicts between annotated names and
/// `global`/`nonlocal` declarations (CPython raises `SyntaxError` for these).
pub(crate) fn collect_annotation_target_names(body: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_annotation_target_names_from_block(body, &mut names);
    names
}

fn collect_annotation_target_names_from_block(body: &[Stmt], names: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::AnnAssign { name, .. } => {
                names.insert(name.clone());
            }
            // Do not descend into nested function/class scopes.
            Stmt::Def { .. } | Stmt::Class { .. } => {}
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, branch) in branches {
                    collect_annotation_target_names_from_block(branch, names);
                }
                if let Some(branch) = else_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_annotation_target_names_from_block(body, names);
                if let Some(branch) = else_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
            }
            Stmt::For {
                body, else_branch, ..
            } => {
                collect_annotation_target_names_from_block(body, names);
                if let Some(branch) = else_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_annotation_target_names_from_block(body, names);
                for handler in handlers {
                    collect_annotation_target_names_from_block(&handler.body, names);
                }
                if let Some(branch) = else_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
                if let Some(branch) = finally_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
            }
            Stmt::With { body, .. } => {
                collect_annotation_target_names_from_block(body, names);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_annotation_target_names_from_block(&arm.body, names);
                }
            }
            _ => {}
        }
    }
}

fn collect_declared_names(
    body: &[Stmt],
    pick: fn(&Stmt) -> Option<&Vec<String>>,
) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_declared_names_from_block(body, &mut names, pick);
    names
}

fn collect_declared_names_from_block(
    body: &[Stmt],
    names: &mut HashSet<String>,
    pick: fn(&Stmt) -> Option<&Vec<String>>,
) {
    for stmt in body {
        if let Some(declared) = pick(stmt) {
            names.extend(declared.iter().cloned());
            continue;
        }
        match stmt {
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, branch) in branches {
                    collect_declared_names_from_block(branch, names, pick);
                }
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_declared_names_from_block(body, names, pick);
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::For {
                body, else_branch, ..
            } => {
                collect_declared_names_from_block(body, names, pick);
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_declared_names_from_block(body, names, pick);
                for handler in handlers {
                    collect_declared_names_from_block(&handler.body, names, pick);
                }
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
                if let Some(branch) = finally_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::With { body, .. } => {
                collect_declared_names_from_block(body, names, pick);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_declared_names_from_block(&arm.body, names, pick);
                }
            }
            _ => {}
        }
    }
}

/// Check that no `global x` or `nonlocal x` declaration in `body` (at any
/// nesting depth within the same function scope) appears after a prior
/// assignment to or use of `x` in that same scope.
///
/// CPython 3.12 raises two distinct SyntaxError messages:
/// - `"name 'x' is assigned to before global declaration"` — when `x` was
///   bound (assigned, for-target, with-target, def, class, AugAssign, walrus)
///   before the `global x` declaration.
/// - `"name 'x' is used prior to global declaration"` — when `x` was read
///   (appeared as `Expr::Var`) but not bound before the `global x`.
///
/// The same messages apply for `nonlocal`, substituting "nonlocal" for
/// "global".
///
/// Returns `Some(error_message)` on the first violation found, or `None`.
pub(crate) fn check_global_nonlocal_order(body: &[Stmt]) -> Option<String> {
    let mut assigned: HashSet<String> = HashSet::new();
    let mut used: HashSet<String> = HashSet::new();
    check_global_nonlocal_order_block(body, &mut assigned, &mut used)
}

/// Recursive helper: walk `stmts` in order, updating `assigned` and `used`,
/// and returning an error on the first ordering violation.
fn check_global_nonlocal_order_block(
    stmts: &[Stmt],
    assigned: &mut HashSet<String>,
    used: &mut HashSet<String>,
) -> Option<String> {
    for stmt in stmts {
        match stmt {
            Stmt::Global(names) => {
                for name in names {
                    // CPython checks "used" before "assigned": when both sets
                    // contain the name (e.g. `x = 1; print(x); global x`),
                    // CPython always reports "used prior to global declaration".
                    if used.contains(name) {
                        return Some(format!(
                            "name '{}' is used prior to global declaration",
                            name
                        ));
                    }
                    if assigned.contains(name) {
                        return Some(format!(
                            "name '{}' is assigned to before global declaration",
                            name
                        ));
                    }
                }
            }
            Stmt::Nonlocal(names) => {
                for name in names {
                    // Same priority: "used" wins over "assigned" (matches CPython).
                    if used.contains(name) {
                        return Some(format!(
                            "name '{}' is used prior to nonlocal declaration",
                            name
                        ));
                    }
                    if assigned.contains(name) {
                        return Some(format!(
                            "name '{}' is assigned to before nonlocal declaration",
                            name
                        ));
                    }
                }
            }
            // Assignments bind names.
            Stmt::Assign(target, expr) => {
                target.visit_evaluated_exprs(&mut |expr| {
                    collect_var_refs_in_expr(expr, used, assigned)
                });
                collect_var_refs_in_expr(expr, used, assigned);
                collect_assign_target_bound_names(target, assigned);
            }
            Stmt::AugAssign { target, expr, .. } => {
                // AugAssign reads the target first, then writes it.
                // A bare name counts as assigned for CPython's declaration
                // diagnostic; receiver/key/bound expressions are ordinary
                // reads and must be visited separately.
                target.visit_evaluated_exprs(&mut |expr| {
                    collect_var_refs_in_expr(expr, used, assigned)
                });
                collect_var_refs_in_expr(expr, used, assigned);
                collect_assign_target_bound_names(target, assigned);
            }
            Stmt::AnnAssign {
                annotation, value, ..
            } => {
                // AnnAssign (`x: T` or `x: T = v`) is handled by the separate
                // "annotated name can't be global/nonlocal" check, which is
                // order-independent and always produces "annotated name 'x'
                // can't be global" regardless of order.  Skip the target name
                // here so we don't produce a conflicting message.
                collect_var_refs_in_expr(annotation, used, assigned);
                if let Some(v) = value {
                    collect_var_refs_in_expr(v, used, assigned);
                }
            }
            Stmt::Def {
                name, decorators, ..
            } => {
                // Decorators are evaluated in the outer scope.
                for dec in decorators {
                    collect_var_refs_in_expr(dec, used, assigned);
                }
                // The def name is bound in the outer scope (not recursed into).
                assigned.insert(name.clone());
            }
            Stmt::Class {
                name,
                bases,
                metaclass,
                keywords,
                decorators,
                ..
            } => {
                for dec in decorators {
                    collect_var_refs_in_expr(dec, used, assigned);
                }
                for base in bases {
                    collect_var_refs_in_expr(base, used, assigned);
                }
                if let Some(mc) = metaclass {
                    collect_var_refs_in_expr(mc, used, assigned);
                }
                for (_, kw) in keywords {
                    collect_var_refs_in_expr(kw, used, assigned);
                }
                assigned.insert(name.clone());
            }
            Stmt::For {
                target,
                iter,
                body,
                else_branch,
                ..
            } => {
                collect_var_refs_in_expr(iter, used, assigned);
                target.visit_evaluated_exprs(&mut |expr| {
                    collect_var_refs_in_expr(expr, used, assigned)
                });
                collect_assign_target_bound_names(target, assigned);
                if let Some(msg) = check_global_nonlocal_order_block(body, assigned, used) {
                    return Some(msg);
                }
                if let Some(branch) = else_branch
                    && let Some(msg) = check_global_nonlocal_order_block(branch, assigned, used)
                {
                    return Some(msg);
                }
            }
            Stmt::With { items, body, .. } => {
                for (expr, alias) in items {
                    collect_var_refs_in_expr(expr, used, assigned);
                    if let Some(target) = alias {
                        target.visit_evaluated_exprs(&mut |expr| {
                            collect_var_refs_in_expr(expr, used, assigned)
                        });
                        collect_assign_target_bound_names(target, assigned);
                    }
                }
                if let Some(msg) = check_global_nonlocal_order_block(body, assigned, used) {
                    return Some(msg);
                }
            }
            Stmt::Expr(e) => {
                collect_var_refs_in_expr(e, used, assigned);
            }
            Stmt::Return(Some(e)) => {
                collect_var_refs_in_expr(e, used, assigned);
            }
            Stmt::Return(None) => {}
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (cond, branch) in branches {
                    collect_var_refs_in_expr(cond, used, assigned);
                    if let Some(msg) = check_global_nonlocal_order_block(branch, assigned, used) {
                        return Some(msg);
                    }
                }
                if let Some(branch) = else_branch
                    && let Some(msg) = check_global_nonlocal_order_block(branch, assigned, used)
                {
                    return Some(msg);
                }
            }
            Stmt::While {
                cond,
                body,
                else_branch,
                ..
            } => {
                collect_var_refs_in_expr(cond, used, assigned);
                if let Some(msg) = check_global_nonlocal_order_block(body, assigned, used) {
                    return Some(msg);
                }
                if let Some(branch) = else_branch
                    && let Some(msg) = check_global_nonlocal_order_block(branch, assigned, used)
                {
                    return Some(msg);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                if let Some(msg) = check_global_nonlocal_order_block(body, assigned, used) {
                    return Some(msg);
                }
                for handler in handlers {
                    if let Some(bound) = &handler.name {
                        assigned.insert(bound.clone());
                    }
                    if let Some(exc_type) = &handler.kind {
                        collect_var_refs_in_expr(exc_type, used, assigned);
                    }
                    if let Some(msg) =
                        check_global_nonlocal_order_block(&handler.body, assigned, used)
                    {
                        return Some(msg);
                    }
                }
                if let Some(branch) = else_branch
                    && let Some(msg) = check_global_nonlocal_order_block(branch, assigned, used)
                {
                    return Some(msg);
                }
                if let Some(branch) = finally_branch
                    && let Some(msg) = check_global_nonlocal_order_block(branch, assigned, used)
                {
                    return Some(msg);
                }
            }
            Stmt::Match { subject, arms } => {
                collect_var_refs_in_expr(subject, used, assigned);
                for arm in arms {
                    arm.pattern.visit_evaluated_exprs(&mut |expr| {
                        collect_var_refs_in_expr(expr, used, assigned)
                    });
                    collect_pattern_bound_names(&arm.pattern, assigned);
                    if let Some(guard) = &arm.guard {
                        collect_var_refs_in_expr(guard, used, assigned);
                    }
                    if let Some(msg) = check_global_nonlocal_order_block(&arm.body, assigned, used)
                    {
                        return Some(msg);
                    }
                }
            }
            Stmt::Delete(exprs) => {
                for e in exprs {
                    collect_var_refs_in_expr(e, used, assigned);
                }
            }
            Stmt::Assert { test, msg } => {
                collect_var_refs_in_expr(test, used, assigned);
                if let Some(m) = msg {
                    collect_var_refs_in_expr(m, used, assigned);
                }
            }
            Stmt::Raise { expr, cause, .. } => {
                if let Some(e) = expr {
                    collect_var_refs_in_expr(e, used, assigned);
                }
                if let Some(c) = cause {
                    collect_var_refs_in_expr(c, used, assigned);
                }
            }
            // Import/ImportFrom do NOT trigger "used prior to" — CPython does
            // not flag `import x; global x` as a SyntaxError.
            Stmt::Import { .. } | Stmt::ImportFrom { .. } => {}
            Stmt::AttrAssign { target, expr, .. } => {
                collect_var_refs_in_expr(target, used, assigned);
                collect_var_refs_in_expr(expr, used, assigned);
            }
            Stmt::IndexAssign {
                target,
                index,
                expr,
            } => {
                collect_var_refs_in_expr(target, used, assigned);
                collect_var_refs_in_expr(index, used, assigned);
                collect_var_refs_in_expr(expr, used, assigned);
            }
            Stmt::SliceAssign {
                target,
                lower,
                upper,
                step,
                expr,
            } => {
                collect_var_refs_in_expr(target, used, assigned);
                if let Some(e) = lower {
                    collect_var_refs_in_expr(e, used, assigned);
                }
                if let Some(e) = upper {
                    collect_var_refs_in_expr(e, used, assigned);
                }
                if let Some(e) = step {
                    collect_var_refs_in_expr(e, used, assigned);
                }
                collect_var_refs_in_expr(expr, used, assigned);
            }
            Stmt::Break | Stmt::Continue | Stmt::Pass => {}
            // `type X[T] = expr` binds X; references in expr are "used".
            Stmt::TypeAlias { name, value, .. } => {
                collect_var_refs_in_expr(value, used, assigned);
                assigned.insert(name.clone());
            }
        }
    }
    None
}
