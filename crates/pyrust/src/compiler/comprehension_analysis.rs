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

impl Compiler {
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
                Stmt::Assign(target, e) => {
                    target.visit_evaluated_exprs(&mut |expr| {
                        Self::collect_walrus_targets_in_expr(expr, out)
                    });
                    Self::collect_walrus_targets_in_expr(e, out);
                }
                Stmt::AugAssign {
                    target, expr: e, ..
                } => {
                    target.visit_evaluated_exprs(&mut |expr| {
                        Self::collect_walrus_targets_in_expr(expr, out)
                    });
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
                    target,
                    iter,
                    body,
                    else_branch,
                    ..
                } => {
                    target.visit_evaluated_exprs(&mut |expr| {
                        Self::collect_walrus_targets_in_expr(expr, out)
                    });
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
                Stmt::AttrAssign {
                    target, expr: e, ..
                } => {
                    Self::collect_walrus_targets_in_expr(target, out);
                    Self::collect_walrus_targets_in_expr(e, out);
                }
                Stmt::IndexAssign {
                    target,
                    index,
                    expr: e,
                } => {
                    Self::collect_walrus_targets_in_expr(target, out);
                    Self::collect_walrus_targets_in_expr(index, out);
                    Self::collect_walrus_targets_in_expr(e, out);
                }
                Stmt::SliceAssign {
                    target,
                    lower,
                    upper,
                    step,
                    expr: e,
                } => {
                    Self::collect_walrus_targets_in_expr(target, out);
                    for expr in [lower, upper, step].iter().flat_map(|expr| expr.as_deref()) {
                        Self::collect_walrus_targets_in_expr(expr, out);
                    }
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
