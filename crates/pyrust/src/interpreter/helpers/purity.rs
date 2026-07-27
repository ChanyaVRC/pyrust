/// Returns true when `expr` is safe for result memoization, given the set of
/// locally-defined functions already confirmed memo-pure (`pure_fns`).
///
/// A `Call` is pure only when the callee is an identity-stable recursive name
/// supplied in `pure_fns`.  A registry name is not a binding guard: bare
/// builtins can be shadowed through globals or a custom `__builtins__`, and
/// module attributes can be reassigned.  Treating either spelling as proof
/// would let `CallMemo` cache the result of arbitrary user code.
fn is_pure_expr(
    expr: &Expr,
    pure_fns: &std::collections::HashSet<String>,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    match expr {
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => true,
        // A variable read is pure only when it refers to a local register in the
        // current function scope.  Reads of free variables (globals, names captured
        // from an enclosing scope) are inherently impure: the caller cannot
        // control whether the value changes between invocations, so memoising the
        // result via `CallMemo` would serve stale data.  Without this guard, a
        // function like `def f(): return counter` would be mis-classified as pure
        // and its first result permanently cached, hiding subsequent mutations of
        // `counter` (issue #346 correctness requirement).
        Expr::Var(n, _) => local_names.contains_key(n.as_str()),
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().all(|e| is_pure_expr(e, pure_fns, local_names))
        }
        Expr::Starred(inner) => is_pure_expr(inner, pure_fns, local_names),
        Expr::Dict(items) => items.iter().all(|item| match item {
            crate::ast::DictItem::Pair(k, v) => {
                is_pure_expr(k, pure_fns, local_names) && is_pure_expr(v, pure_fns, local_names)
            }
            crate::ast::DictItem::DoubleSplat(e) => is_pure_expr(e, pure_fns, local_names),
        }),
        // A unary op dispatches a user dunder (`__neg__` / `__pos__` /
        // `__invert__`), but the VM cache only fires for integer arguments,
        // which carry no user dunders. The result is therefore determined by
        // the operand for every cached invocation.
        Expr::Unary { expr, .. } => is_pure_expr(expr, pure_fns, local_names),
        Expr::Binary {
            op, left, right, ..
        } => {
            !binop_may_raise(*op)
                && is_pure_expr(left, pure_fns, local_names)
                && is_pure_expr(right, pure_fns, local_names)
        }
        // A comparison dispatches a user rich-comparison dunder (`__lt__` /
        // `__eq__` / …), but cached invocations have integer arguments and
        // therefore deterministic builtin comparison behavior.
        Expr::Compare { left, ops } => {
            is_pure_expr(left, pure_fns, local_names)
                && ops
                    .iter()
                    .all(|(_, e)| is_pure_expr(e, pure_fns, local_names))
        }
        Expr::Ternary { cond, then, else_ } => {
            is_pure_expr(cond, pure_fns, local_names)
                && is_pure_expr(then, pure_fns, local_names)
                && is_pure_expr(else_, pure_fns, local_names)
        }
        // A lambda expression allocates a fresh Rc<UserFunction> on every evaluation,
        // so any enclosing function that returns one is not pure in the identity sense.
        Expr::Lambda { .. } => false,
        Expr::Call { func, args, .. } => {
            // Only direct recursive calls whose binding identity is supplied
            // by the compiler qualify.  Builtin/module spellings are mutable
            // Python bindings and therefore provide no memoization proof.
            let callee_is_pure = match func.as_ref() {
                Expr::Var(name, _) => pure_fns.contains(name.as_str()),
                _ => false,
            };
            if !callee_is_pure {
                return false;
            }
            args.iter()
                .all(|a| is_pure_expr(&a.value, pure_fns, local_names))
        }
        // Attribute access, subscription and slicing are not memo-pure.
        // They can raise on otherwise-pure operands and invoke user protocol
        // methods (`__getattr__`/`__getattribute__`, `__getitem__`):
        //   (1).foo   -> AttributeError
        //   {}["x"]   -> KeyError
        //   (1)[0]    -> TypeError ('int' object is not subscriptable)
        //   (1)[0:1]  -> TypeError
        // An attribute / item read can observe mutable external state (e.g.
        // `sys.exc_info()[2].tb_lineno`), so caching the result by argument
        // value would serve stale data. Recursing into the operands is unsound
        // because the operation itself is the external read.
        Expr::Attr { .. } | Expr::Index { .. } | Expr::Slice { .. } => false,
        // Comprehensions involve iteration (GetIter, ForIter) which may call
        // __iter__/__next__ — conservatively treat as impure.
        Expr::ListComp { .. }
        | Expr::DictComp { .. }
        | Expr::SetComp { .. }
        | Expr::GenExp { .. } => false,
        // Walrus has a side effect (assignment).
        Expr::Named { .. } => false,
        Expr::FString(parts) => {
            // An f-string with an interpolated expression invokes the formatting
            // protocol (`__format__`/`__str__`/`__repr__`) and can raise even on
            // a pure operand: a bad format spec on a built-in raises `ValueError`
            // (`f"{(1):foo}"`), and a user `__format__` may have side effects,
            // raise, or read mutable external state. Caching the result could
            // therefore serve stale data. Only a fully-literal f-string (no
            // interpolation) is memo-pure.
            use crate::ast::FStringPart;
            parts.iter().all(|p| matches!(p, FStringPart::Literal(_)))
        }
        // yield/yield from/await always have side effects (suspension).
        Expr::Yield(_) | Expr::YieldFrom(_) | Expr::Await(_) => false,
    }
}

/// True when a binary operator can raise an exception (or invoke a protocol
/// method) even on otherwise-pure operands, so an expression using it must NOT
/// be treated as pure by [`is_pure_expr`].
///
/// - `Div` / `FloorDiv` / `Mod` / `Pow`: `ZeroDivisionError` (`1/0`, `1%0`,
///   `0 ** -1`).
/// - `LShift` / `RShift`: `ValueError("negative shift count")`.
/// - `MatMul`: `TypeError` for operands without `__matmul__`.
/// - `In` / `NotIn`: invoke `__contains__` / iteration.
///
/// The remaining arithmetic, bitwise, boolean, comparison and identity
/// operators do not raise for the literal/local operands that pass
/// `is_pure_expr`, so they stay memoizable.
fn binop_may_raise(op: crate::ast::BinaryOp) -> bool {
    use crate::ast::BinaryOp::*;
    matches!(
        op,
        Div | FloorDiv | Mod | Pow | LShift | RShift | MatMul | In | NotIn
    )
}

/// Return whether storing into an assignment target is confined to local
/// registers.  Attribute/item targets execute Python protocols and can mutate
/// state reachable outside the function, including when nested inside an
/// unpacking target.
fn is_pure_binding_target(
    target: &AssignTarget,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    match target {
        AssignTarget::Name(name) => local_names.contains_key(name),
        AssignTarget::Tuple(targets) => targets
            .iter()
            .all(|target| is_pure_binding_target(target, local_names)),
        AssignTarget::Starred(target) => is_pure_binding_target(target, local_names),
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => false,
    }
}

/// Augmented assignment reads and then writes its target.  Only a local-name
/// target can inherit the integer-only CallMemo safety guarantee; attribute,
/// item and slice targets both dispatch user protocols and mutate externally
/// reachable objects.
fn is_pure_aug_assign_target(
    target: &AssignTarget,
    op: crate::ast::BinaryOp,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    matches!(target, AssignTarget::Name(name) if local_names.contains_key(name))
        && !binop_may_raise(op)
}

/// Pattern forms that are safe under CallMemo's integer-only argument gate.
///
/// Sequence, mapping and class patterns invoke structural-match protocols.
/// Value patterns perform a mutable dotted lookup.  They therefore remain
/// conservative misses even when their child expressions look pure.
fn is_pure_pattern(
    pattern: &crate::ast::Pattern,
    pure_fns: &std::collections::HashSet<String>,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    use crate::ast::Pattern;

    match pattern {
        Pattern::Wildcard | Pattern::Capture(_) => true,
        Pattern::Literal(expr) => is_pure_expr(expr, pure_fns, local_names),
        Pattern::Or(patterns) => patterns
            .iter()
            .all(|pattern| is_pure_pattern(pattern, pure_fns, local_names)),
        Pattern::As { pattern, .. } => is_pure_pattern(pattern, pure_fns, local_names),
        Pattern::Sequence(_) | Pattern::Mapping(..) | Pattern::Class { .. } | Pattern::Value(_) => {
            false
        }
    }
}

/// True if `body` is *memo-pure*: a call to a function with this body may have
/// its result cached and reused for equal arguments. The VM result cache
/// (`vm.rs::Insn::CallMemo`) only activates for all-integer arguments and a
/// scalar result. This keeps self-recursive `fib`/`fact`/`ack` (which use
/// comparisons and unary operations) memoized. Persistent state mutation,
/// mutable external reads, I/O, and fresh-object allocation disqualify a body.
pub(crate) fn is_memo_pure_body(
    body: &[Stmt],
    pure_fns: &std::collections::HashSet<String>,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    is_pure_body(body, pure_fns, local_names)
}

fn is_pure_body(
    body: &[Stmt],
    pure_fns: &std::collections::HashSet<String>,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    body.iter().all(|s| is_pure_stmt(s, pure_fns, local_names))
}

fn is_pure_stmt(
    stmt: &Stmt,
    pure_fns: &std::collections::HashSet<String>,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    match stmt {
        // Explicit side effects on outer state.
        Stmt::Global(_) | Stmt::Nonlocal(_) => false,
        // Object / container mutation.
        Stmt::AttrAssign { .. } | Stmt::IndexAssign { .. } | Stmt::SliceAssign { .. } => false,
        // Deletion and imports can affect shared state.
        Stmt::Delete(_) | Stmt::Import { .. } | Stmt::ImportFrom { .. } => false,
        // `with` typically wraps I/O or resource-management side effects.
        Stmt::With { .. } => false,

        // Assignments are pure only when every target is a local binding.
        Stmt::Assign(target, expr) => {
            is_pure_binding_target(target, local_names) && is_pure_expr(expr, pure_fns, local_names)
        }
        Stmt::Expr(expr) => is_pure_expr(expr, pure_fns, local_names),
        Stmt::AugAssign { target, op, expr } => {
            is_pure_aug_assign_target(target, *op, local_names)
                && is_pure_expr(expr, pure_fns, local_names)
        }
        Stmt::Return(Some(expr)) => is_pure_expr(expr, pure_fns, local_names),
        Stmt::Return(None) => true,
        // `raise` and a failing `assert` are not memo-pure: caching a previous
        // scalar result could skip their control-flow effect.
        Stmt::Assert { .. } => false,
        Stmt::Raise { .. } => false,

        // Control flow: recurse into sub-blocks.
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches.iter().all(|(cond, blk)| {
                is_pure_expr(cond, pure_fns, local_names)
                    && is_pure_body(blk, pure_fns, local_names)
            }) && else_branch
                .as_deref()
                .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
        }
        Stmt::While {
            cond,
            body,
            else_branch,
            ..
        } => {
            is_pure_expr(cond, pure_fns, local_names)
                && is_pure_body(body, pure_fns, local_names)
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
        }
        Stmt::For {
            target,
            iter,
            body,
            else_branch,
            ..
        } => {
            is_pure_binding_target(target, local_names)
                && is_pure_expr(iter, pure_fns, local_names)
                && is_pure_body(body, pure_fns, local_names)
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            ..
        } => {
            is_pure_body(body, pure_fns, local_names)
                && handlers.iter().all(|h| {
                    h.kind
                        .as_ref()
                        .is_none_or(|kind| is_pure_expr(kind, pure_fns, local_names))
                        && is_pure_body(&h.body, pure_fns, local_names)
                })
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
                && finally_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
        }

        // Annotated assignment modifies __annotations__ dict — impure at module/class scope.
        Stmt::AnnAssign { .. } => false,
        // Nested definitions always allocate a fresh heap object (Rc<UserFunction> /
        // PyClass), so any function that defines and returns one is non-pure: successive
        // calls with identical arguments produce values with distinct identities.
        Stmt::Def { .. } | Stmt::Class { .. } => false,
        Stmt::Pass | Stmt::Break | Stmt::Continue => true,
        Stmt::Match { subject, arms } => {
            is_pure_expr(subject, pure_fns, local_names)
                && arms.iter().all(|arm| {
                    is_pure_pattern(&arm.pattern, pure_fns, local_names)
                        && arm
                            .guard
                            .as_ref()
                            .is_none_or(|guard| is_pure_expr(guard, pure_fns, local_names))
                        && is_pure_body(&arm.body, pure_fns, local_names)
                })
        }
        // TypeAlias allocates a new heap object → impure (identity changes).
        Stmt::TypeAlias { .. } => false,
    }
}
