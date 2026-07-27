// Generic free-variable read analysis shared by cell, class, comprehension,
// and transitive-closure analysis.  These walkers stop at nested Def/Class
// boundaries; descendants are handled by free_variables.rs.

/// Collect all `Var` reads in `stmts`, stopping at nested `Def`/`Class`
/// boundaries.  Used to detect free variables that need to become cell vars.
fn collect_free_var_reads_in_stmts(stmts: &[Stmt], uses: &mut HashSet<String>) {
    for stmt in stmts {
        collect_free_var_reads_in_stmt(stmt, uses);
    }
}

/// An augmented assignment reads a bare-name target before writing it.
/// Receiver/key/bound expressions are handled by
/// [`AssignTarget::visit_evaluated_exprs`].
fn collect_aug_assign_name_reads(target: &AssignTarget, uses: &mut HashSet<String>) {
    match target {
        AssignTarget::Name(name) => {
            uses.insert(name.clone());
        }
        AssignTarget::Tuple(targets) => {
            for target in targets {
                collect_aug_assign_name_reads(target, uses);
            }
        }
        AssignTarget::Starred(target) => collect_aug_assign_name_reads(target, uses),
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => {}
    }
}

fn collect_free_var_reads_in_stmt(stmt: &Stmt, uses: &mut HashSet<String>) {
    match stmt {
        Stmt::Def { .. } | Stmt::Class { .. } => {
            stmt.visit_scope_dependency_exprs(&mut |expr| {
                collect_free_var_reads_in_expr(expr, uses)
            });
        }
        Stmt::Assign(target, value) => {
            target.visit_evaluated_exprs(&mut |expr| collect_free_var_reads_in_expr(expr, uses));
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
            collect_aug_assign_name_reads(target, uses);
            target.visit_evaluated_exprs(&mut |expr| collect_free_var_reads_in_expr(expr, uses));
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
            target,
            iter,
            body,
            else_branch,
            ..
        } => {
            target.visit_evaluated_exprs(&mut |expr| collect_free_var_reads_in_expr(expr, uses));
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
            for (e, target) in items {
                collect_free_var_reads_in_expr(e, uses);
                if let Some(target) = target {
                    target.visit_evaluated_exprs(&mut |expr| {
                        collect_free_var_reads_in_expr(expr, uses)
                    });
                }
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
                arm.pattern
                    .visit_evaluated_exprs(&mut |expr| collect_free_var_reads_in_expr(expr, uses));
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
        Stmt::TypeAlias { .. } => {
            stmt.visit_scope_dependency_exprs(&mut |expr| {
                collect_free_var_reads_in_expr(expr, uses)
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
            // Defaults and annotations execute in the enclosing scope, where
            // lambda parameters do not shadow their names.
            for p in params {
                if let Some(d) = &p.default {
                    collect_free_var_reads_in_expr(d, uses);
                }
                if let Some(annotation) = &p.annotation {
                    collect_free_var_reads_in_expr(annotation, uses);
                }
            }

            // The body is a nested function scope. Surface only its free reads;
            // parameters are local to the lambda and must not spuriously turn a
            // same-named variable in an outer function into a cell.
            let mut body_uses = HashSet::new();
            collect_free_var_reads_in_expr(body, &mut body_uses);
            for parameter in params {
                body_uses.remove(&parameter.name);
            }
            uses.extend(body_uses);
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
