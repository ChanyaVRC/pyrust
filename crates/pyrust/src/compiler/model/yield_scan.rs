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
    let mut direct = false;
    stmt.visit_evaluated_exprs(&mut |expr| {
        direct |= expr_contains_yield(expr);
    });
    if direct {
        return true;
    }

    // Expression evaluation belongs to the AST visitor above. This match owns
    // only nested statement blocks and their scope transitions.
    match stmt {
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches.iter().any(|(_, body)| stmts_contain_yield(body))
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::While {
            body, else_branch, ..
        } => stmts_contain_yield(body) || else_branch.as_deref().is_some_and(stmts_contain_yield),
        Stmt::For {
            body, else_branch, ..
        } => stmts_contain_yield(body) || else_branch.as_deref().is_some_and(stmts_contain_yield),
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            stmts_contain_yield(body)
                || handlers.iter().any(|h| stmts_contain_yield(&h.body))
                || else_branch.as_deref().is_some_and(stmts_contain_yield)
                || finally_branch.as_deref().is_some_and(stmts_contain_yield)
        }
        Stmt::With { body, .. } => stmts_contain_yield(body),
        Stmt::Match { arms, .. } => arms.iter().any(|arm| stmts_contain_yield(&arm.body)),
        // Def and Class bodies are separate scopes — their yields do not make
        // the enclosing function a generator. Their headers were evaluated by
        // `visit_evaluated_exprs` above and do belong to this scope.
        Stmt::Def { .. }
        | Stmt::Class { .. }
        | Stmt::Assign(..)
        | Stmt::AnnAssign { .. }
        | Stmt::AttrAssign { .. }
        | Stmt::IndexAssign { .. }
        | Stmt::SliceAssign { .. }
        | Stmt::AugAssign { .. }
        | Stmt::Expr(_)
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Import { .. }
        | Stmt::ImportFrom { .. }
        | Stmt::Return(_)
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Pass
        | Stmt::Raise { .. }
        | Stmt::Delete(_)
        | Stmt::Assert { .. }
        | Stmt::TypeAlias { .. } => false,
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
        Expr::Lambda { params, .. } => params.iter().any(|parameter| {
            parameter.default.as_ref().is_some_and(expr_contains_yield)
                || parameter
                    .annotation
                    .as_ref()
                    .is_some_and(expr_contains_yield)
        }),
        // A comprehension body has its own scope, but its first iterable is
        // evaluated by the enclosing scope before that scope is entered.
        Expr::ListComp { clauses, .. }
        | Expr::SetComp { clauses, .. }
        | Expr::DictComp { clauses, .. }
        | Expr::GenExp { clauses, .. } => clauses
            .first()
            .is_some_and(|clause| expr_contains_yield(&clause.iter)),
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
