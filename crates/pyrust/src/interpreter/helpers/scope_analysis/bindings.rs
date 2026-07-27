pub(crate) fn collect_local_names(
    params: &[crate::ast::FunctionParam],
    body: &[Stmt],
    global_names: &HashSet<String>,
    nonlocal_names: &HashSet<String>,
) -> indexmap::IndexSet<String> {
    let mut names: indexmap::IndexSet<String> =
        params.iter().map(|param| param.name.clone()).collect();
    collect_local_names_from_block(body, &mut names, global_names, nonlocal_names);
    names
}

fn collect_local_names_from_block(
    body: &[Stmt],
    names: &mut indexmap::IndexSet<String>,
    global_names: &HashSet<String>,
    nonlocal_names: &HashSet<String>,
) {
    for stmt in body {
        match stmt {
            Stmt::Assign(target, rhs) => {
                collect_assign_target_names(target, names, global_names, nonlocal_names);
                target.visit_evaluated_exprs(&mut |expr| {
                    collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names)
                });
                // Walrus targets inside comprehensions on the RHS escape to this
                // function's scope (PEP 572).
                collect_walrus_targets_in_expr(rhs, names, global_names, nonlocal_names);
            }
            Stmt::AttrAssign { target, expr, .. } => {
                collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
                collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names);
            }
            Stmt::Def { name, .. } => {
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
            }
            Stmt::Class { name, .. } => {
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
            }
            Stmt::Global(_) | Stmt::Nonlocal(_) => {}
            Stmt::Import {
                names: import_names,
            } => {
                for (module, alias) in import_names {
                    let bound = alias
                        .clone()
                        .unwrap_or_else(|| module.split('.').next().unwrap_or(module).to_string());
                    if !global_names.contains(&bound) && !nonlocal_names.contains(&bound) {
                        names.insert(bound);
                    }
                }
            }
            Stmt::ImportFrom {
                names: import_names,
                ..
            } => {
                for (attr_name, alias) in import_names {
                    if attr_name == "*" {
                        continue;
                    }
                    let bound = alias.clone().unwrap_or_else(|| attr_name.clone());
                    if !global_names.contains(&bound) && !nonlocal_names.contains(&bound) {
                        names.insert(bound);
                    }
                }
            }
            Stmt::AnnAssign { name, value, .. } => {
                // Both `x: T = v` (value = Some) and `x: T` (value = None) declare
                // a local slot.  At function scope the bare form causes UnboundLocalError
                // on read (matching CPython); at class scope the slot is allocated but
                // never stored via RecordClassStore so it does not appear in vars(C).
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
                // Walrus targets inside a comprehension on the RHS escape to this
                // function's scope (PEP 572).
                if let Some(v) = value {
                    collect_walrus_targets_in_expr(v, names, global_names, nonlocal_names);
                }
            }
            Stmt::AugAssign { target, expr, .. } => {
                target.visit_evaluated_exprs(&mut |expr| {
                    collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names)
                });
                // Walrus targets inside a comprehension on the RHS escape to this
                // function's scope (PEP 572).
                collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names);
            }
            Stmt::IndexAssign {
                target,
                index,
                expr,
            } => {
                collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
                collect_walrus_targets_in_expr(index, names, global_names, nonlocal_names);
                // Walrus targets inside a comprehension on the RHS escape to this
                // function's scope (PEP 572).
                collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names);
            }
            Stmt::SliceAssign {
                target,
                lower,
                upper,
                step,
                expr,
            } => {
                collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
                for bound in [lower, upper, step]
                    .iter()
                    .flat_map(|bound| bound.as_deref())
                {
                    collect_walrus_targets_in_expr(bound, names, global_names, nonlocal_names);
                }
                collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names);
            }
            Stmt::Raise { expr, cause, .. } => {
                // Walrus targets inside a comprehension in the raise expression or
                // cause escape to this function's scope (PEP 572).
                if let Some(e) = expr {
                    collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
                }
                if let Some(c) = cause {
                    collect_walrus_targets_in_expr(c, names, global_names, nonlocal_names);
                }
            }
            Stmt::Delete(exprs) => {
                for expr in exprs {
                    collect_delete_target_names(expr, names, global_names, nonlocal_names);
                }
            }
            Stmt::Break | Stmt::Continue | Stmt::Pass => {}
            // Walk expressions for walrus operator targets.
            Stmt::Expr(e) => {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
            Stmt::Return(Some(e)) => {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
            Stmt::Return(None) => {}
            Stmt::Assert { test, msg } => {
                collect_walrus_targets_in_expr(test, names, global_names, nonlocal_names);
                if let Some(m) = msg {
                    collect_walrus_targets_in_expr(m, names, global_names, nonlocal_names);
                }
            }
            Stmt::With { items, body, .. } => {
                for (expr, alias) in items {
                    collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names);
                    if let Some(target) = alias {
                        collect_assign_target_names(target, names, global_names, nonlocal_names);
                        target.visit_evaluated_exprs(&mut |expr| {
                            collect_walrus_targets_in_expr(
                                expr,
                                names,
                                global_names,
                                nonlocal_names,
                            )
                        });
                    }
                }
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
            }
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (cond, branch) in branches {
                    collect_walrus_targets_in_expr(cond, names, global_names, nonlocal_names);
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::While {
                cond,
                body,
                else_branch,
                ..
            } => {
                collect_walrus_targets_in_expr(cond, names, global_names, nonlocal_names);
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                for handler in handlers {
                    if let Some(name) = &handler.name
                        && !global_names.contains(name)
                        && !nonlocal_names.contains(name)
                    {
                        names.insert(name.clone());
                    }
                    collect_local_names_from_block(
                        &handler.body,
                        names,
                        global_names,
                        nonlocal_names,
                    );
                }
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
                if let Some(branch) = finally_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::For {
                target,
                iter,
                body,
                else_branch,
                ..
            } => {
                collect_walrus_targets_in_expr(iter, names, global_names, nonlocal_names);
                collect_assign_target_names(target, names, global_names, nonlocal_names);
                target.visit_evaluated_exprs(&mut |expr| {
                    collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names)
                });
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::Match { subject, arms } => {
                collect_walrus_targets_in_expr(subject, names, global_names, nonlocal_names);
                for arm in arms {
                    // Collect capture names introduced by patterns.
                    collect_pattern_names(&arm.pattern, names, global_names, nonlocal_names);
                    arm.pattern.visit_evaluated_exprs(&mut |expr| {
                        collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names)
                    });
                    if let Some(guard) = &arm.guard {
                        collect_walrus_targets_in_expr(guard, names, global_names, nonlocal_names);
                    }
                    collect_local_names_from_block(&arm.body, names, global_names, nonlocal_names);
                }
            }
            // `type X[T] = expr` binds `X` as a local name (PEP 695).
            // The type params (T) are NOT local names — they are temporaries
            // visible only during RHS evaluation and do not escape to the scope.
            Stmt::TypeAlias { name, value, .. } => {
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
                collect_walrus_targets_in_expr(value, names, global_names, nonlocal_names);
            }
        }
    }
}

/// Collect names that a pattern binds (capture patterns, star captures in sequences,
/// and `**rest` in mappings).
fn collect_pattern_names(
    pattern: &crate::ast::Pattern,
    names: &mut indexmap::IndexSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    use crate::ast::Pattern;
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Value(_) => {}
        Pattern::Capture(name) => {
            if !global_names.contains(name) && !nonlocal_names.contains(name) {
                names.insert(name.clone());
            }
        }
        Pattern::Or(alts) => {
            for alt in alts {
                collect_pattern_names(alt, names, global_names, nonlocal_names);
            }
        }
        Pattern::Sequence(elems) => {
            for (elem_pat, _) in elems {
                collect_pattern_names(elem_pat, names, global_names, nonlocal_names);
            }
        }
        Pattern::Mapping(pairs, rest) => {
            for (_, val_pat) in pairs {
                collect_pattern_names(val_pat, names, global_names, nonlocal_names);
            }
            if let Some(rest_name) = rest
                && !global_names.contains(rest_name)
                && !nonlocal_names.contains(rest_name)
            {
                names.insert(rest_name.clone());
            }
        }
        Pattern::Class {
            positional, kwargs, ..
        } => {
            for pat in positional {
                collect_pattern_names(pat, names, global_names, nonlocal_names);
            }
            for (_, attr_pat) in kwargs {
                collect_pattern_names(attr_pat, names, global_names, nonlocal_names);
            }
        }
        Pattern::As { pattern, name } => {
            collect_pattern_names(pattern, names, global_names, nonlocal_names);
            if !global_names.contains(name) && !nonlocal_names.contains(name) {
                names.insert(name.clone());
            }
        }
    }
}

/// Walk an expression tree and collect names bound by walrus operators (`:=`).
fn collect_walrus_targets_in_expr(
    expr: &Expr,
    names: &mut indexmap::IndexSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    match expr {
        Expr::Named { target, value } => {
            if !global_names.contains(target) && !nonlocal_names.contains(target) {
                names.insert(target.clone());
            }
            collect_walrus_targets_in_expr(value, names, global_names, nonlocal_names);
        }
        Expr::Binary { left, right, .. } => {
            collect_walrus_targets_in_expr(left, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(right, names, global_names, nonlocal_names);
        }
        Expr::Unary { expr: e, .. } => {
            collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
        }
        Expr::Compare { left, ops } => {
            collect_walrus_targets_in_expr(left, names, global_names, nonlocal_names);
            for (_, e) in ops {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
        }
        Expr::Call { func, args, .. } => {
            collect_walrus_targets_in_expr(func, names, global_names, nonlocal_names);
            for a in args {
                collect_walrus_targets_in_expr(&a.value, names, global_names, nonlocal_names);
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_walrus_targets_in_expr(cond, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(then, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(else_, names, global_names, nonlocal_names);
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
        }
        Expr::Starred(inner) => {
            collect_walrus_targets_in_expr(inner, names, global_names, nonlocal_names);
        }
        Expr::Dict(items) => {
            for item in items {
                match item {
                    crate::ast::DictItem::Pair(k, v) => {
                        collect_walrus_targets_in_expr(k, names, global_names, nonlocal_names);
                        collect_walrus_targets_in_expr(v, names, global_names, nonlocal_names);
                    }
                    crate::ast::DictItem::DoubleSplat(e) => {
                        collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
                    }
                }
            }
        }
        Expr::Index { target, index, .. } => {
            collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(index, names, global_names, nonlocal_names);
        }
        Expr::Attr { target, .. } => {
            collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
        }
        Expr::FString(parts) => {
            // Walrus inside an f-string interpolation (or inside a nested
            // `{expr}` in the format spec) still binds in the enclosing
            // scope; recurse so the target name is recorded.
            use crate::ast::FStringPart;
            fn walk(
                parts: &[FStringPart],
                names: &mut indexmap::IndexSet<String>,
                global_names: &std::collections::HashSet<String>,
                nonlocal_names: &std::collections::HashSet<String>,
            ) {
                for part in parts {
                    if let FStringPart::Expr {
                        expr, format_spec, ..
                    } = part
                    {
                        collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names);
                        if let Some(spec_parts) = format_spec {
                            walk(spec_parts, names, global_names, nonlocal_names);
                        }
                    }
                }
            }
            walk(parts, names, global_names, nonlocal_names);
        }
        // Walrus targets inside a comprehension escape to the nearest enclosing
        // non-comprehension scope (PEP 572). Descend into the element/value
        // expressions and filter conditions so their walrus targets are recorded
        // as locals of the enclosing function.  Do NOT descend into Lambda nodes
        // (they create a true new scope).
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            collect_walrus_targets_in_expr(elt, names, global_names, nonlocal_names);
            for clause in clauses {
                if let Some(c) = &clause.cond {
                    collect_walrus_targets_in_expr(c, names, global_names, nonlocal_names);
                }
                // Inner clause iters also run in the comprehension scope and can
                // contain walrus expressions that escape outward.
                collect_walrus_targets_in_expr(&clause.iter, names, global_names, nonlocal_names);
            }
        }
        Expr::DictComp { key, val, clauses } => {
            collect_walrus_targets_in_expr(key, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(val, names, global_names, nonlocal_names);
            for clause in clauses {
                if let Some(c) = &clause.cond {
                    collect_walrus_targets_in_expr(c, names, global_names, nonlocal_names);
                }
                collect_walrus_targets_in_expr(&clause.iter, names, global_names, nonlocal_names);
            }
        }
        _ => {}
    }
}

fn collect_assign_target_names(
    target: &AssignTarget,
    names: &mut indexmap::IndexSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    match target {
        AssignTarget::Name(n) => {
            if !global_names.contains(n) && !nonlocal_names.contains(n) {
                names.insert(n.clone());
            }
        }
        AssignTarget::Tuple(targets) => {
            for t in targets {
                collect_assign_target_names(t, names, global_names, nonlocal_names);
            }
        }
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => {}
        AssignTarget::Starred(inner) => {
            collect_assign_target_names(inner, names, global_names, nonlocal_names);
        }
    }
}

/// A deleted plain name is a binding in Python's symbol table just like an
/// assigned name (`def f(): del x` makes `x` local).  Attribute and item
/// deletion do not bind their base expressions.
fn collect_delete_target_names(
    target: &Expr,
    names: &mut indexmap::IndexSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    match target {
        Expr::Var(name, _) => {
            if !global_names.contains(name) && !nonlocal_names.contains(name) {
                names.insert(name.clone());
            }
        }
        Expr::Tuple(items) | Expr::List(items) => {
            for item in items {
                collect_delete_target_names(item, names, global_names, nonlocal_names);
            }
        }
        Expr::Starred(inner) => {
            collect_delete_target_names(inner, names, global_names, nonlocal_names);
        }
        _ => {}
    }
}
