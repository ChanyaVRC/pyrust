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
            out.insert(name.to_string());
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
        Expr::ListComp { elt, clauses, .. }
        | Expr::SetComp { elt, clauses, .. }
        | Expr::GenExp { elt, clauses, .. } => {
            expr_contains_assignment_expression(elt)
                || clauses.iter().any(|clause| {
                    expr_contains_assignment_expression(&clause.iter)
                        || clause
                            .cond
                            .as_ref()
                            .is_some_and(expr_contains_assignment_expression)
                })
        }
        Expr::DictComp {
            key, val, clauses, ..
        } => {
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
        | Expr::YieldFrom { expr, .. }
        | Expr::Await { expr, .. } => expr_contains_assignment_expression(expr),
        Expr::Yield { value: expr, .. } => expr
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
