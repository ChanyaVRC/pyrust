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

/// Collect walrus targets that bind the nearest non-comprehension scope.
fn collect_walrus_writes_in_expr(expr: &Expr, out: &mut HashSet<String>) {
    expr.visit_enclosing_walrus_targets(&mut |target| {
        out.insert(target.to_owned());
    });
}

impl Compiler {
    /// Collect walrus targets that bind the nearest non-comprehension scope.
    fn collect_walrus_targets_in_expr(expr: &Expr, out: &mut HashSet<String>) {
        expr.visit_enclosing_walrus_targets(&mut |target| {
            out.insert(target.to_owned());
        });
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

/// Return whether an expression lexically contains an assignment expression.
///
/// This deliberately crosses lambda and nested-comprehension scope boundaries:
/// PEP 572 prohibits `:=` anywhere inside a comprehension iterable's syntax,
/// including inside a lambda body/default or a nested comprehension. This is a
/// syntax rule, not a binding rule, so it must remain separate from
/// `visit_enclosing_walrus_targets`, which correctly stops at function scopes.
fn expr_contains_assignment_expression(expr: &Expr) -> bool {
    match expr {
        Expr::Named { .. } => true,
        Expr::Lambda { params, body } => {
            params.iter().any(|parameter| {
                parameter
                    .default
                    .as_ref()
                    .is_some_and(expr_contains_assignment_expression)
                    || parameter
                        .annotation
                        .as_ref()
                        .is_some_and(expr_contains_assignment_expression)
            }) || expr_contains_assignment_expression(body)
        }
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            expr_contains_assignment_expression(elt)
                || clauses.iter().any(|clause| {
                    expr_contains_assignment_expression(&clause.iter)
                        || clause
                            .cond
                            .as_ref()
                            .is_some_and(expr_contains_assignment_expression)
                })
        }
        Expr::DictComp { key, val, clauses } => {
            expr_contains_assignment_expression(key)
                || expr_contains_assignment_expression(val)
                || clauses.iter().any(|clause| {
                    expr_contains_assignment_expression(&clause.iter)
                        || clause
                            .cond
                            .as_ref()
                            .is_some_and(expr_contains_assignment_expression)
                })
        }
        Expr::Binary { left, right, .. } => {
            expr_contains_assignment_expression(left) || expr_contains_assignment_expression(right)
        }
        Expr::Unary { expr, .. }
        | Expr::Starred(expr)
        | Expr::YieldFrom(expr)
        | Expr::Await(expr) => expr_contains_assignment_expression(expr),
        Expr::Yield(expr) => expr
            .as_deref()
            .is_some_and(expr_contains_assignment_expression),
        Expr::Compare { left, ops } => {
            expr_contains_assignment_expression(left)
                || ops
                    .iter()
                    .any(|(_, operand)| expr_contains_assignment_expression(operand))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_contains_assignment_expression(cond)
                || expr_contains_assignment_expression(then)
                || expr_contains_assignment_expression(else_)
        }
        Expr::Call { func, args, .. } => {
            expr_contains_assignment_expression(func)
                || args
                    .iter()
                    .any(|argument| expr_contains_assignment_expression(&argument.value))
        }
        Expr::Attr { target, .. } => expr_contains_assignment_expression(target),
        Expr::Index { target, index, .. } => {
            expr_contains_assignment_expression(target)
                || expr_contains_assignment_expression(index)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_contains_assignment_expression(target)
                || [lower, upper, step]
                    .iter()
                    .flat_map(|bound| bound.as_deref())
                    .any(expr_contains_assignment_expression)
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().any(expr_contains_assignment_expression)
        }
        Expr::Dict(items) => items.iter().any(|item| match item {
            DictItem::Pair(key, value) => {
                expr_contains_assignment_expression(key)
                    || expr_contains_assignment_expression(value)
            }
            DictItem::DoubleSplat(expr) => expr_contains_assignment_expression(expr),
        }),
        Expr::FString(parts) => {
            let mut contains = false;
            for_each_fstring_expr(parts, &mut |expr| {
                contains |= expr_contains_assignment_expression(expr);
            });
            contains
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
        | Expr::Ellipsis => false,
    }
}

/// Validate a comprehension / generator expression at compile time, raising the
/// CPython 3.12 `SyntaxError`s that pyrust would otherwise accept:
///
/// * an assignment expression anywhere inside an iterable expression
///   (`assignment expression cannot be used in a comprehension iterable expression`);
/// * a `yield`/`yield from` directly in the element/condition expressions
///   (`'yield' inside <kind>`); and
/// * an assignment expression (`:=`) whose target collides with one of the
///   comprehension's own iteration variables (PEP 572,
///   `assignment expression cannot rebind comprehension iteration variable '<n>'`).
///
/// `result_exprs` are the value-producing expressions (`elt`, or `key`+`val`);
/// `clauses` are the comprehension clauses. `kind` is the CPython label
/// (e.g. `"list comprehension"`).
///
/// Returns the `SyntaxError` message on violation (`None` when valid).
fn check_comprehension(
    result_exprs: &[&Expr],
    clauses: &[CompClause],
    kind: &str,
) -> Option<String> {
    // This check has precedence over body-level yield/rebind diagnostics in
    // CPython and applies to every iterable, not only the outermost one.
    if clauses
        .iter()
        .any(|clause| expr_contains_assignment_expression(&clause.iter))
    {
        return Some(
            "assignment expression cannot be used in a comprehension iterable expression"
                .to_string(),
        );
    }

    // `yield` directly inside the comprehension body. The first iterable is
    // evaluated in the enclosing scope, where `yield` may be valid; every
    // later iterable belongs to the implicit comprehension function and must
    // therefore be rejected along with the result/filter expressions.
    let mut yields = result_exprs.iter().any(|e| expr_contains_yield(e));
    yields = yields
        || clauses
            .iter()
            .any(|c| c.cond.as_ref().is_some_and(expr_contains_yield));
    yields = yields || clauses.iter().skip(1).any(|c| expr_contains_yield(&c.iter));
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
                let mut body_uses = HashSet::new();
                collect_free_var_reads_in_expr(body, &mut body_uses);
                for param in params {
                    body_uses.remove(&param.name);
                }
                for name in body_uses {
                    if local_index.contains_key(&name) {
                        cells.insert(name);
                    }
                }
            }
            // Defaults/annotations are evaluated in the enclosing scope, where
            // the lambda parameters do not shadow names. Descend separately so
            // a nested closure in `lambda x=(lambda: x): ...` can still promote
            // the enclosing `x`; merging it with body uses and then subtracting
            // parameters loses that dependency.
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
