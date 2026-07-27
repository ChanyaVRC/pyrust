/// Return `true` if `stmts` contain a `yield` or `yield from` expression
/// anywhere in the immediate function scope, without crossing into nested
/// `Def` or `Class` bodies (those have their own generator status).
///
/// Used to detect that a function is a generator even when the `yield`
/// appears only in compile-time-dead branches (e.g. `if False: yield`),
/// which are skipped during bytecode emission and therefore produce no
/// `Insn::Yield` for the post-compilation `is_generator` scan to find.
fn stmts_contain_yield(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_contains_yield)
}

fn stmt_contains_yield(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(e) => expr_contains_yield(e),
        Stmt::Return(Some(e)) => expr_contains_yield(e),
        Stmt::Return(None) | Stmt::Pass | Stmt::Break | Stmt::Continue => false,
        Stmt::Global(_) | Stmt::Nonlocal(_) | Stmt::Import { .. } | Stmt::ImportFrom { .. } => {
            false
        }
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches
                .iter()
                .any(|(cond, body)| expr_contains_yield(cond) || stmts_contain_yield(body))
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            expr_contains_yield(cond)
                || stmts_contain_yield(body)
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::For {
            target,
            iter,
            body,
            else_branch,
            ..
        } => {
            let mut target_contains_yield = false;
            target.visit_evaluated_exprs(&mut |expr| {
                target_contains_yield |= expr_contains_yield(expr);
            });
            target_contains_yield
                || expr_contains_yield(iter)
                || stmts_contain_yield(body)
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            stmts_contain_yield(body)
                || handlers.iter().any(|h| {
                    h.kind.as_ref().is_some_and(expr_contains_yield) || stmts_contain_yield(&h.body)
                })
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
                || finally_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::With { items, body, .. } => {
            items.iter().any(|(expr, target)| {
                let mut target_contains_yield = false;
                if let Some(target) = target {
                    target.visit_evaluated_exprs(&mut |expr| {
                        target_contains_yield |= expr_contains_yield(expr);
                    });
                }
                expr_contains_yield(expr) || target_contains_yield
            }) || stmts_contain_yield(body)
        }
        Stmt::Assign(target, value) => {
            let mut target_contains_yield = false;
            target.visit_evaluated_exprs(&mut |expr| {
                target_contains_yield |= expr_contains_yield(expr);
            });
            target_contains_yield || expr_contains_yield(value)
        }
        Stmt::AugAssign { target, expr, .. } => {
            let mut target_contains_yield = false;
            target.visit_evaluated_exprs(&mut |expr| {
                target_contains_yield |= expr_contains_yield(expr);
            });
            target_contains_yield || expr_contains_yield(expr)
        }
        Stmt::AnnAssign { value, .. } => value.as_ref().is_some_and(expr_contains_yield),
        Stmt::AttrAssign { target, expr, .. } => {
            expr_contains_yield(target) || expr_contains_yield(expr)
        }
        Stmt::IndexAssign {
            target,
            index,
            expr,
        } => expr_contains_yield(target) || expr_contains_yield(index) || expr_contains_yield(expr),
        Stmt::SliceAssign {
            target,
            lower,
            upper,
            step,
            expr,
        } => {
            expr_contains_yield(target)
                || lower.as_deref().is_some_and(expr_contains_yield)
                || upper.as_deref().is_some_and(expr_contains_yield)
                || step.as_deref().is_some_and(expr_contains_yield)
                || expr_contains_yield(expr)
        }
        Stmt::Raise { expr, cause, .. } => {
            expr.as_ref().is_some_and(expr_contains_yield)
                || cause.as_ref().is_some_and(expr_contains_yield)
        }
        Stmt::Delete(exprs) => exprs.iter().any(expr_contains_yield),
        Stmt::Assert { test, msg } => {
            expr_contains_yield(test) || msg.as_ref().is_some_and(expr_contains_yield)
        }
        Stmt::Match { subject, arms } => {
            expr_contains_yield(subject)
                || arms.iter().any(|arm| {
                    let mut pattern_contains_yield = false;
                    arm.pattern.visit_evaluated_exprs(&mut |expr| {
                        pattern_contains_yield |= expr_contains_yield(expr);
                    });
                    pattern_contains_yield
                        || arm.guard.as_ref().is_some_and(expr_contains_yield)
                        || stmts_contain_yield(&arm.body)
                })
        }
        // Def and Class bodies are separate scopes — their yields do not make
        // the enclosing function a generator.
        Stmt::Def { .. } | Stmt::Class { .. } => false,
        Stmt::TypeAlias { value, .. } => expr_contains_yield(value),
    }
}

fn expr_contains_yield(expr: &Expr) -> bool {
    match expr {
        Expr::Yield(_) | Expr::YieldFrom(_) => true,
        Expr::Binary { left, right, .. } => expr_contains_yield(left) || expr_contains_yield(right),
        Expr::Unary { expr: e, .. } => expr_contains_yield(e),
        Expr::Compare { left, ops } => {
            expr_contains_yield(left) || ops.iter().any(|(_, e)| expr_contains_yield(e))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_contains_yield(cond) || expr_contains_yield(then) || expr_contains_yield(else_)
        }
        Expr::Call { func, args, .. } => {
            expr_contains_yield(func) || args.iter().any(|a| expr_contains_yield(&a.value))
        }
        Expr::Attr { target, .. } => expr_contains_yield(target),
        Expr::Index { target, index, .. } => {
            expr_contains_yield(target) || expr_contains_yield(index)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_contains_yield(target)
                || lower.as_deref().is_some_and(expr_contains_yield)
                || upper.as_deref().is_some_and(expr_contains_yield)
                || step.as_deref().is_some_and(expr_contains_yield)
        }
        Expr::Tuple(items) | Expr::List(items) | Expr::Set(items) => {
            items.iter().any(expr_contains_yield)
        }
        Expr::Dict(items) => items.iter().any(|item| match item {
            crate::ast::DictItem::Pair(k, v) => expr_contains_yield(k) || expr_contains_yield(v),
            crate::ast::DictItem::DoubleSplat(e) => expr_contains_yield(e),
        }),
        Expr::Starred(e) => expr_contains_yield(e),
        Expr::Named { value, .. } => expr_contains_yield(value),
        Expr::Await(e) => expr_contains_yield(e),
        // An f-string's `{expr}` interpolations (and any `{expr}` inside a
        // nested format spec) are real sub-expressions in the same scope, so a
        // `yield` there counts: `(f"{(yield x)}" for x in xs)` is rejected as
        // `'yield' inside generator expression`. Mirrors the
        // `expr_contains_await` f-string handling (#2308 / #2313).
        Expr::FString(parts) => fstring_parts_contain_yield(parts),
        // Lambda, comprehensions, and generator expressions are separate scopes.
        Expr::Lambda { .. }
        | Expr::ListComp { .. }
        | Expr::SetComp { .. }
        | Expr::DictComp { .. }
        | Expr::GenExp { .. } => false,
        // Leaf nodes — cannot contain yield.
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

/// Whether any `{expr}` interpolation in an f-string (or in a nested format
/// spec) contains a `yield` in the current scope. Helper for
/// `expr_contains_yield`. Mirrors `fstring_parts_contain_await`.
fn fstring_parts_contain_yield(parts: &[crate::ast::FStringPart]) -> bool {
    parts.iter().any(|part| match part {
        crate::ast::FStringPart::Literal(_) => false,
        crate::ast::FStringPart::Expr {
            expr, format_spec, ..
        } => {
            expr_contains_yield(expr)
                || format_spec
                    .as_deref()
                    .is_some_and(fstring_parts_contain_yield)
        }
    })
}
