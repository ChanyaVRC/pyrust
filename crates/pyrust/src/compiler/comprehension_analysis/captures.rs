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
        // Definition bodies are nested scopes. Eager header expressions and
        // PEP 695 annotation-scope expressions can still contain explicit
        // lambdas/comprehensions that capture this scope.
        Stmt::Def { .. } | Stmt::Class { .. } => {
            stmt.visit_scope_dependency_exprs(&mut |expr| {
                lambda_captures_in_expr(expr, local_index, is_class_scope, cells)
            });
            // Bounds/constraints are compiled as implicit zero-argument
            // thunks, so their direct reads are captures even though there is
            // no `Expr::Lambda` node in the source AST.
            stmt.visit_deferred_annotation_exprs(&mut |expr| {
                deferred_annotation_captures_in_expr(expr, local_index, is_class_scope, cells)
            });
        }
        Stmt::Assign(target, value) => {
            target.visit_evaluated_exprs(&mut |expr| {
                lambda_captures_in_expr(expr, local_index, is_class_scope, cells)
            });
            lambda_captures_in_expr(value, local_index, is_class_scope, cells);
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
        Stmt::AugAssign { target, expr, .. } => {
            target.visit_evaluated_exprs(&mut |expr| {
                lambda_captures_in_expr(expr, local_index, is_class_scope, cells)
            });
            lambda_captures_in_expr(expr, local_index, is_class_scope, cells);
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
            target,
            iter,
            body,
            else_branch,
            ..
        } => {
            target.visit_evaluated_exprs(&mut |expr| {
                lambda_captures_in_expr(expr, local_index, is_class_scope, cells)
            });
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
            for (e, target) in items {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
                if let Some(target) = target {
                    target.visit_evaluated_exprs(&mut |expr| {
                        lambda_captures_in_expr(expr, local_index, is_class_scope, cells)
                    });
                }
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
                arm.pattern.visit_evaluated_exprs(&mut |expr| {
                    lambda_captures_in_expr(expr, local_index, is_class_scope, cells)
                });
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
        Stmt::TypeAlias { .. } => {
            stmt.visit_scope_dependency_exprs(&mut |expr| {
                lambda_captures_in_expr(expr, local_index, is_class_scope, cells)
            });
            stmt.visit_deferred_annotation_exprs(&mut |expr| {
                deferred_annotation_captures_in_expr(expr, local_index, is_class_scope, cells)
            });
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

fn deferred_annotation_captures_in_expr(
    expr: &Expr,
    local_index: &HashMap<String, Reg>,
    is_class_scope: bool,
    cells: &mut HashSet<String>,
) {
    // An annotation thunk created while executing a class body does not close
    // over the class namespace. The enclosing function's class-scope walker
    // is responsible for promoting its own locals instead.
    if is_class_scope {
        return;
    }
    let mut uses = HashSet::new();
    collect_free_var_reads_in_expr(expr, &mut uses);
    collect_transitive_free_vars_in_expr(expr, &mut uses);
    for name in uses {
        if local_index.contains_key(&name) {
            cells.insert(name);
        }
    }
}

/// Promote enclosing locals read or walrus-assigned inside a comprehension or
/// generator-expression body into cell vars.
fn promote_comp_captures(
    result_exprs: &[&Expr],
    clauses: &[CompClause],
    local_index: &HashMap<String, Reg>,
    is_class_scope: bool,
    cells: &mut HashSet<String>,
) {
    // The outermost iterable is evaluated in the enclosing scope.
    if let Some(first) = clauses.first() {
        lambda_captures_in_expr(&first.iter, local_index, is_class_scope, cells);
    }
    if is_class_scope {
        return;
    }
    // Everything inside the comprehension body runs in its own scope.
    // Collect free-var reads from that inner body, subtract the names
    // bound by the comprehension's own clause targets, and promote any
    // remaining names that live in the enclosing local_index to cell vars.
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
    for expr in result_exprs {
        collect_free_var_reads_in_expr(expr, &mut inner_uses);
    }
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
    // PEP 572: walrus targets in a comprehension body belong to the enclosing
    // scope. Promote them to cell vars so they're reachable via the env chain
    // from inside the comprehension's implicit function.
    let mut walrus_writes: HashSet<String> = HashSet::new();
    for clause in clauses {
        if let Some(c) = &clause.cond {
            collect_walrus_writes_in_expr(c, &mut walrus_writes);
        }
    }
    for expr in result_exprs {
        collect_walrus_writes_in_expr(expr, &mut walrus_writes);
    }
    for name in walrus_writes {
        if !bound.contains(&name) && local_index.contains_key(&name) {
            cells.insert(name);
        }
    }
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
            // over the class namespace (issue #699).
            if !is_class_scope {
                let mut body_uses = HashSet::new();
                collect_free_var_reads_in_expr(body, &mut body_uses);
                for param in params {
                    body_uses.remove(param.name.as_str());
                }
                for name in body_uses {
                    if local_index.contains_key(&name) {
                        cells.insert(name);
                    }
                }
            }
            // Defaults/annotations are evaluated in the enclosing scope, where
            // the lambda parameters do not shadow names.
            for param in params {
                if let Some(default) = &param.default {
                    lambda_captures_in_expr(default, local_index, is_class_scope, cells);
                }
                if let Some(annotation) = &param.annotation {
                    lambda_captures_in_expr(annotation, local_index, is_class_scope, cells);
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
        Expr::SliceLit { lower, upper, step } => {
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                lambda_captures_in_expr(e, local_index, is_class_scope, cells);
            }
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
        Expr::ListComp { elt, clauses, .. }
        | Expr::SetComp { elt, clauses, .. }
        | Expr::GenExp { elt, clauses, .. } => {
            promote_comp_captures(&[elt], clauses, local_index, is_class_scope, cells);
        }
        Expr::DictComp {
            key, val, clauses, ..
        } => {
            promote_comp_captures(&[key, val], clauses, local_index, is_class_scope, cells);
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
        Expr::Yield { value: Some(e), .. } => {
            lambda_captures_in_expr(e, local_index, is_class_scope, cells)
        }
        Expr::Yield { value: None, .. } => {}
        Expr::YieldFrom { expr: e, .. } => {
            lambda_captures_in_expr(e, local_index, is_class_scope, cells)
        }
        Expr::Await { expr: e, .. } => {
            lambda_captures_in_expr(e, local_index, is_class_scope, cells)
        }
    }
}
