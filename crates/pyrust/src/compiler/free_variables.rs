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
        stmt @ Stmt::Def {
            params,
            body: nested_body,
            ..
        } => {
            stmt.visit_scope_dependency_exprs(&mut |expr| {
                collect_transitive_free_vars_in_expr(expr, uses);
                collect_free_var_reads_in_expr(expr, uses);
            });
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
        stmt @ Stmt::Class {
            body: nested_body, ..
        } => {
            stmt.visit_scope_dependency_exprs(&mut |expr| {
                collect_transitive_free_vars_in_expr(expr, uses);
                collect_free_var_reads_in_expr(expr, uses);
            });
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
        Stmt::Assign(target, value) => {
            target.visit_evaluated_exprs(&mut |expr| {
                collect_transitive_free_vars_in_expr(expr, uses)
            });
            collect_transitive_free_vars_in_expr(value, uses);
        }
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
        Stmt::AugAssign { target, expr, .. } => {
            target.visit_evaluated_exprs(&mut |expr| {
                collect_transitive_free_vars_in_expr(expr, uses)
            });
            collect_transitive_free_vars_in_expr(expr, uses);
        }
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
            target,
            iter,
            body,
            else_branch,
            ..
        } => {
            target.visit_evaluated_exprs(&mut |expr| {
                collect_transitive_free_vars_in_expr(expr, uses)
            });
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
            for (e, target) in items {
                collect_transitive_free_vars_in_expr(e, uses);
                if let Some(target) = target {
                    target.visit_evaluated_exprs(&mut |expr| {
                        collect_transitive_free_vars_in_expr(expr, uses)
                    });
                }
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
                arm.pattern.visit_evaluated_exprs(&mut |expr| {
                    collect_transitive_free_vars_in_expr(expr, uses)
                });
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
        Stmt::TypeAlias { .. } => {
            stmt.visit_scope_dependency_exprs(&mut |expr| {
                collect_transitive_free_vars_in_expr(expr, uses);
                collect_free_var_reads_in_expr(expr, uses);
            });
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
            let mut body_uses: HashSet<String> = HashSet::new();
            collect_free_var_reads_in_expr(body, &mut body_uses);
            collect_transitive_free_vars_in_expr(body, &mut body_uses);
            for p in params {
                body_uses.remove(&p.name);
            }
            uses.extend(body_uses);

            // Defaults/annotations execute in the enclosing scope, not the
            // lambda body. Keep their provenance separate so parameter-name
            // subtraction applies only to body reads.
            for p in params {
                if let Some(default) = &p.default {
                    collect_free_var_reads_in_expr(default, uses);
                    collect_transitive_free_vars_in_expr(default, uses);
                }
                if let Some(annotation) = &p.annotation {
                    collect_free_var_reads_in_expr(annotation, uses);
                    collect_transitive_free_vars_in_expr(annotation, uses);
                }
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
