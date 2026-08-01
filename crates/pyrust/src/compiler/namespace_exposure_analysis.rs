/// Return whether syntax anywhere in this compile unit can make its live
/// module namespace reachable. This deliberately crosses nested scope
/// boundaries: a nested function can retain `globals()` and mutate the
/// mapping before a later module-level read.
fn module_namespace_may_be_exposed(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_may_expose_module_namespace)
}

fn stmt_may_expose_module_namespace(stmt: &Stmt) -> bool {
    let mut exposed = false;
    stmt.visit_scope_dependency_exprs(&mut |expr| {
        exposed |= expr_may_expose_module_namespace(expr);
    });
    if exposed {
        return true;
    }

    match stmt {
        Stmt::Def { body, .. } | Stmt::Class { body, .. } | Stmt::With { body, .. } => {
            module_namespace_may_be_exposed(body)
        }
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches
                .iter()
                .any(|(_, body)| module_namespace_may_be_exposed(body))
                || else_branch
                    .as_deref()
                    .is_some_and(module_namespace_may_be_exposed)
        }
        Stmt::While {
            body, else_branch, ..
        }
        | Stmt::For {
            body, else_branch, ..
        } => {
            module_namespace_may_be_exposed(body)
                || else_branch
                    .as_deref()
                    .is_some_and(module_namespace_may_be_exposed)
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            module_namespace_may_be_exposed(body)
                || handlers
                    .iter()
                    .any(|handler| module_namespace_may_be_exposed(&handler.body))
                || else_branch
                    .as_deref()
                    .is_some_and(module_namespace_may_be_exposed)
                || finally_branch
                    .as_deref()
                    .is_some_and(module_namespace_may_be_exposed)
        }
        Stmt::Match { arms, .. } => arms
            .iter()
            .any(|arm| module_namespace_may_be_exposed(&arm.body)),
        _ => false,
    }
}

fn target_may_expose_module_namespace(target: &AssignTarget) -> bool {
    let mut exposed = false;
    target.visit_evaluated_exprs(&mut |expr| {
        exposed |= expr_may_expose_module_namespace(expr);
    });
    exposed
}

fn expr_may_expose_module_namespace(expr: &Expr) -> bool {
    match expr {
        // A bare read is enough: the callable can be retained under an alias
        // and invoked later, so looking only for direct calls is unsound.
        Expr::Var(name, _) => matches!(
            name.as_str(),
            "globals" | "locals" | "vars" | "exec" | "eval" | "_getframe"
        ),
        Expr::Attr { target, name, .. } => {
            matches!(name.as_str(), "_getframe" | "f_locals" | "f_globals")
                || expr_may_expose_module_namespace(target)
        }
        Expr::Binary { left, right, .. } => {
            expr_may_expose_module_namespace(left) || expr_may_expose_module_namespace(right)
        }
        Expr::Unary { expr, .. }
        | Expr::Starred(expr)
        | Expr::YieldFrom(expr)
        | Expr::Await(expr) => expr_may_expose_module_namespace(expr),
        Expr::Compare { left, ops } => {
            expr_may_expose_module_namespace(left)
                || ops
                    .iter()
                    .any(|(_, operand)| expr_may_expose_module_namespace(operand))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_may_expose_module_namespace(cond)
                || expr_may_expose_module_namespace(then)
                || expr_may_expose_module_namespace(else_)
        }
        Expr::Lambda { params, body } => {
            params.iter().any(|parameter| {
                parameter
                    .default
                    .as_ref()
                    .is_some_and(expr_may_expose_module_namespace)
                    || parameter
                        .annotation
                        .as_ref()
                        .is_some_and(expr_may_expose_module_namespace)
            }) || expr_may_expose_module_namespace(body)
        }
        Expr::Call { func, args, .. } => {
            expr_may_expose_module_namespace(func)
                || args
                    .iter()
                    .any(|argument| expr_may_expose_module_namespace(&argument.value))
        }
        Expr::Index { target, index, .. } => {
            let target_exposed = expr_may_expose_module_namespace(target);
            target_exposed || expr_may_expose_module_namespace(index)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_may_expose_module_namespace(target)
                || [lower, upper, step]
                    .iter()
                    .flat_map(|bound| bound.as_deref())
                    .any(expr_may_expose_module_namespace)
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().any(expr_may_expose_module_namespace)
        }
        Expr::Dict(items) => items.iter().any(|item| match item {
            DictItem::Pair(key, value) => {
                let key_exposed = expr_may_expose_module_namespace(key);
                key_exposed || expr_may_expose_module_namespace(value)
            }
            DictItem::DoubleSplat(expr) => expr_may_expose_module_namespace(expr),
        }),
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            expr_may_expose_module_namespace(elt)
                || clauses.iter().any(|clause| {
                    target_may_expose_module_namespace(&clause.target)
                        || expr_may_expose_module_namespace(&clause.iter)
                        || clause
                            .cond
                            .as_ref()
                            .is_some_and(expr_may_expose_module_namespace)
                })
        }
        Expr::DictComp { key, val, clauses } => {
            expr_may_expose_module_namespace(key)
                || expr_may_expose_module_namespace(val)
                || clauses.iter().any(|clause| {
                    target_may_expose_module_namespace(&clause.target)
                        || expr_may_expose_module_namespace(&clause.iter)
                        || clause
                            .cond
                            .as_ref()
                            .is_some_and(expr_may_expose_module_namespace)
                })
        }
        Expr::Named { value, .. } => expr_may_expose_module_namespace(value),
        Expr::FString(parts) => {
            let mut exposed = false;
            for_each_fstring_expr(parts, &mut |expr| {
                exposed |= expr_may_expose_module_namespace(expr);
            });
            exposed
        }
        Expr::Yield(expr) => expr
            .as_deref()
            .is_some_and(expr_may_expose_module_namespace),
        Expr::Int(_)
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
