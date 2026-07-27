/// Whether an expression contains an `await` in the current scope. Used to
/// decide whether a synthesized comprehension function is a coroutine / async
/// generator (#2304): an `await` in a comprehension's element or condition (or
/// a non-outermost clause iterable) makes the comprehension asynchronous even
/// without an `async for` clause.
///
/// Mirrors `expr_contains_yield`: it does not enter lambda bodies or nested
/// comprehension bodies, which have their own scopes. Lambda defaults and
/// annotations remain in the current scope and are scanned.
fn expr_contains_await(expr: &Expr) -> bool {
    match expr {
        Expr::Await(_) => true,
        Expr::Yield(e) => e.as_deref().is_some_and(expr_contains_await),
        Expr::YieldFrom(e) => expr_contains_await(e),
        Expr::Binary { left, right, .. } => expr_contains_await(left) || expr_contains_await(right),
        Expr::Unary { expr: e, .. } => expr_contains_await(e),
        Expr::Compare { left, ops } => {
            expr_contains_await(left) || ops.iter().any(|(_, e)| expr_contains_await(e))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_contains_await(cond) || expr_contains_await(then) || expr_contains_await(else_)
        }
        Expr::Call { func, args, .. } => {
            expr_contains_await(func) || args.iter().any(|a| expr_contains_await(&a.value))
        }
        Expr::Attr { target, .. } => expr_contains_await(target),
        Expr::Index { target, index, .. } => {
            expr_contains_await(target) || expr_contains_await(index)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_contains_await(target)
                || lower.as_deref().is_some_and(expr_contains_await)
                || upper.as_deref().is_some_and(expr_contains_await)
                || step.as_deref().is_some_and(expr_contains_await)
        }
        Expr::Tuple(items) | Expr::List(items) | Expr::Set(items) => {
            items.iter().any(expr_contains_await)
        }
        Expr::Dict(items) => items.iter().any(|item| match item {
            crate::ast::DictItem::Pair(k, v) => expr_contains_await(k) || expr_contains_await(v),
            crate::ast::DictItem::DoubleSplat(e) => expr_contains_await(e),
        }),
        Expr::Starred(e) => expr_contains_await(e),
        Expr::Named { value, .. } => expr_contains_await(value),
        // An f-string's `{expr}` interpolations (and any `{expr}` inside a
        // nested format spec) are real sub-expressions in the same scope, so an
        // `await` there counts: `[f"{await f(x)}" for x in xs]` is async.
        Expr::FString(parts) => fstring_parts_contain_await(parts),
        // Lambda defaults and annotations are evaluated in the current scope;
        // only the body belongs to the nested function.
        Expr::Lambda { params, .. } => params.iter().any(|parameter| {
            parameter.default.as_ref().is_some_and(expr_contains_await)
                || parameter
                    .annotation
                    .as_ref()
                    .is_some_and(expr_contains_await)
        }),
        // A comprehension body has its own synthesized scope, but its first
        // iterable is evaluated by the enclosing scope before entry.
        Expr::ListComp { clauses, .. }
        | Expr::SetComp { clauses, .. }
        | Expr::DictComp { clauses, .. }
        | Expr::GenExp { clauses, .. } => clauses
            .first()
            .is_some_and(|clause| expr_contains_await(&clause.iter)),
        // Leaf nodes — cannot contain await.
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
/// spec) contains an `await` in the current scope. Helper for
/// `expr_contains_await`.
fn fstring_parts_contain_await(parts: &[crate::ast::FStringPart]) -> bool {
    parts.iter().any(|part| match part {
        crate::ast::FStringPart::Literal(_) => false,
        crate::ast::FStringPart::Expr {
            expr, format_spec, ..
        } => {
            expr_contains_await(expr)
                || format_spec
                    .as_deref()
                    .is_some_and(fstring_parts_contain_await)
        }
    })
}

/// Whether a list/set/dict comprehension with the given value expressions
/// (`elts`) and `clauses` is itself *asynchronous* — i.e. CPython would compile
/// it as an async comprehension (issue #2312).  This is true when:
///   - any clause is an `async for`, or
///   - the element / condition / non-outermost iterable contains an `await`, or
///   - the element / condition / non-outermost iterable contains a further
///     nested async COLLECTION comprehension (recursive).
///
/// The outermost iterable (`clauses[0].iter`) is excluded: it is evaluated in
/// the enclosing scope, so an async comp there propagates to the *enclosing*
/// comprehension/function, not this one.
fn collection_comp_is_async(elts: &[&Expr], clauses: &[CompClause]) -> bool {
    if clauses.iter().any(|c| c.is_async) {
        return true;
    }
    elts.iter()
        .any(|e| expr_contains_await(e) || expr_has_async_collection_comp(e))
        || clauses.iter().any(|c| {
            c.cond.as_ref().is_some_and(|cond| {
                expr_contains_await(cond) || expr_has_async_collection_comp(cond)
            })
        })
        || clauses[1..]
            .iter()
            .any(|c| expr_contains_await(&c.iter) || expr_has_async_collection_comp(&c.iter))
}

/// Whether an expression subtree contains a directly-nested *async collection
/// comprehension* (list/set/dict comp) that propagates async-ness outward
/// (issue #2312).
///
/// CPython makes the enclosing comprehension/function async when a nested
/// list/set/dict comprehension in its element/cond/non-outermost iterable is
/// itself async (the enclosing body must `await` the inner comp's coroutine).
/// A nested generator expression does NOT propagate — creating the async-gen
/// object needs no `await` — but we still descend into a genexp's parts because
/// an async collection comp can be nested *inside* a genexp's element (which
/// does propagate). The nested comp keeps its own scope: its `await`s are not
/// counted here except insofar as they make *it* async.
fn expr_has_async_collection_comp(expr: &Expr) -> bool {
    match expr {
        // A nested list/set/dict comprehension: it propagates if it is itself
        // async; otherwise descend into its parts (an async comp may be nested
        // deeper, e.g. inside its element wrapped in another expression).
        Expr::ListComp { elt, clauses } | Expr::SetComp { elt, clauses } => {
            collection_comp_is_async(&[elt], clauses) || comp_parts_have_async_comp(&[elt], clauses)
        }
        Expr::DictComp { key, val, clauses } => {
            collection_comp_is_async(&[key, val], clauses)
                || comp_parts_have_async_comp(&[key, val], clauses)
        }
        // A generator expression itself never propagates async-ness, but an
        // async collection comp nested inside its parts does.
        Expr::GenExp { elt, clauses } => comp_parts_have_async_comp(&[elt], clauses),
        Expr::Binary { left, right, .. } => {
            expr_has_async_collection_comp(left) || expr_has_async_collection_comp(right)
        }
        Expr::Unary { expr: e, .. } => expr_has_async_collection_comp(e),
        Expr::Compare { left, ops } => {
            expr_has_async_collection_comp(left)
                || ops.iter().any(|(_, e)| expr_has_async_collection_comp(e))
        }
        Expr::Ternary { cond, then, else_ } => {
            expr_has_async_collection_comp(cond)
                || expr_has_async_collection_comp(then)
                || expr_has_async_collection_comp(else_)
        }
        Expr::Call { func, args, .. } => {
            expr_has_async_collection_comp(func)
                || args
                    .iter()
                    .any(|a| expr_has_async_collection_comp(&a.value))
        }
        Expr::Attr { target, .. } => expr_has_async_collection_comp(target),
        Expr::Index { target, index, .. } => {
            expr_has_async_collection_comp(target) || expr_has_async_collection_comp(index)
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            expr_has_async_collection_comp(target)
                || lower.as_deref().is_some_and(expr_has_async_collection_comp)
                || upper.as_deref().is_some_and(expr_has_async_collection_comp)
                || step.as_deref().is_some_and(expr_has_async_collection_comp)
        }
        Expr::Tuple(items) | Expr::List(items) | Expr::Set(items) => {
            items.iter().any(expr_has_async_collection_comp)
        }
        Expr::Dict(items) => items.iter().any(|item| match item {
            crate::ast::DictItem::Pair(k, v) => {
                expr_has_async_collection_comp(k) || expr_has_async_collection_comp(v)
            }
            crate::ast::DictItem::DoubleSplat(e) => expr_has_async_collection_comp(e),
        }),
        Expr::Starred(e) => expr_has_async_collection_comp(e),
        Expr::Named { value, .. } => expr_has_async_collection_comp(value),
        Expr::Await(e) => expr_has_async_collection_comp(e),
        Expr::Yield(e) => e.as_deref().is_some_and(expr_has_async_collection_comp),
        Expr::YieldFrom(e) => expr_has_async_collection_comp(e),
        Expr::FString(parts) => fstring_parts_have_async_collection_comp(parts),
        // Lambda bodies are separate, but defaults and annotations execute in
        // the enclosing scope and can create an async collection comp there.
        Expr::Lambda { params, .. } => params.iter().any(|parameter| {
            parameter
                .default
                .as_ref()
                .is_some_and(expr_has_async_collection_comp)
                || parameter
                    .annotation
                    .as_ref()
                    .is_some_and(expr_has_async_collection_comp)
        }),
        // Leaf nodes carry nothing.
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

/// Whether an f-string replacement field or a recursively nested format spec
/// contains an async collection comprehension.
fn fstring_parts_have_async_collection_comp(parts: &[crate::ast::FStringPart]) -> bool {
    parts.iter().any(|part| match part {
        crate::ast::FStringPart::Literal(_) => false,
        crate::ast::FStringPart::Expr {
            expr, format_spec, ..
        } => {
            expr_has_async_collection_comp(expr)
                || format_spec
                    .as_deref()
                    .is_some_and(fstring_parts_have_async_collection_comp)
        }
    })
}

/// Scan a comprehension's element(s), conditions, and ALL iterables (including
/// the outermost, since here the comp is itself nested inside an enclosing
/// expression and its outermost iterable is evaluated in the enclosing comp's
/// scope) for a further nested async collection comprehension. Helper for
/// `expr_has_async_collection_comp`.
fn comp_parts_have_async_comp(elts: &[&Expr], clauses: &[CompClause]) -> bool {
    elts.iter().any(|e| expr_has_async_collection_comp(e))
        || clauses.iter().any(|c| {
            expr_has_async_collection_comp(&c.iter)
                || c.cond.as_ref().is_some_and(expr_has_async_collection_comp)
        })
}
