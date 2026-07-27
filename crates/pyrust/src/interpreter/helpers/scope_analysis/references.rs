/// Collect all `Var(name)` references from an expression into `used`, and
/// walrus-operator binding targets into `assigned`.
/// Does NOT descend into nested function scopes (Def, Lambda, comprehensions).
fn collect_var_refs_in_expr(
    expr: &Expr,
    used: &mut HashSet<String>,
    assigned: &mut HashSet<String>,
) {
    match expr {
        Expr::Var(name, _) => {
            used.insert(name.clone());
        }
        Expr::Named { target, value } => {
            // Walrus operator — the target is a binding in the outer scope
            // ("assigned to"), not merely a use.
            assigned.insert(target.clone());
            collect_var_refs_in_expr(value, used, assigned);
        }
        // Do NOT descend into nested scopes — they have their own symbol table.
        Expr::Lambda { .. }
        | Expr::ListComp { .. }
        | Expr::SetComp { .. }
        | Expr::DictComp { .. }
        | Expr::GenExp { .. } => {}
        // Recurse into sub-expressions.
        Expr::List(elts) | Expr::Tuple(elts) | Expr::Set(elts) => {
            for e in elts {
                collect_var_refs_in_expr(e, used, assigned);
            }
        }
        Expr::Dict(items) => {
            for item in items {
                match item {
                    crate::ast::DictItem::Pair(k, v) => {
                        collect_var_refs_in_expr(k, used, assigned);
                        collect_var_refs_in_expr(v, used, assigned);
                    }
                    crate::ast::DictItem::DoubleSplat(e) => {
                        collect_var_refs_in_expr(e, used, assigned);
                    }
                }
            }
        }
        Expr::Unary { expr, .. } => collect_var_refs_in_expr(expr, used, assigned),
        Expr::Binary { left, right, .. } => {
            collect_var_refs_in_expr(left, used, assigned);
            collect_var_refs_in_expr(right, used, assigned);
        }
        Expr::Compare { left, ops } => {
            collect_var_refs_in_expr(left, used, assigned);
            for (_, e) in ops {
                collect_var_refs_in_expr(e, used, assigned);
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_var_refs_in_expr(cond, used, assigned);
            collect_var_refs_in_expr(then, used, assigned);
            collect_var_refs_in_expr(else_, used, assigned);
        }
        Expr::Call { func, args, .. } => {
            collect_var_refs_in_expr(func, used, assigned);
            for arg in args {
                collect_var_refs_in_expr(&arg.value, used, assigned);
            }
        }
        Expr::Attr { target, .. } => collect_var_refs_in_expr(target, used, assigned),
        Expr::Index { target, index, .. } => {
            collect_var_refs_in_expr(target, used, assigned);
            collect_var_refs_in_expr(index, used, assigned);
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            collect_var_refs_in_expr(target, used, assigned);
            if let Some(e) = lower {
                collect_var_refs_in_expr(e, used, assigned);
            }
            if let Some(e) = upper {
                collect_var_refs_in_expr(e, used, assigned);
            }
            if let Some(e) = step {
                collect_var_refs_in_expr(e, used, assigned);
            }
        }
        Expr::Starred(e) => collect_var_refs_in_expr(e, used, assigned),
        Expr::FString(parts) => {
            for part in parts {
                if let crate::ast::FStringPart::Expr {
                    expr, format_spec, ..
                } = part
                {
                    collect_var_refs_in_expr(expr, used, assigned);
                    if let Some(spec_parts) = format_spec {
                        for spec_part in spec_parts {
                            if let crate::ast::FStringPart::Expr {
                                expr: spec_expr, ..
                            } = spec_part
                            {
                                collect_var_refs_in_expr(spec_expr, used, assigned);
                            }
                        }
                    }
                }
            }
        }
        Expr::Yield(Some(e)) => collect_var_refs_in_expr(e, used, assigned),
        Expr::YieldFrom(e) => collect_var_refs_in_expr(e, used, assigned),
        Expr::Await(e) => collect_var_refs_in_expr(e, used, assigned),
        // Literals / constants — no names.
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis
        | Expr::Yield(None) => {}
    }
}

/// Collect the names **bound** by an assignment target (left-hand side of `=`
/// or a `for`/`with` binding target).  Only `Name` targets are collected;
/// attribute and index targets do not introduce new local bindings.
fn collect_assign_target_bound_names(target: &AssignTarget, assigned: &mut HashSet<String>) {
    match target {
        AssignTarget::Name(n) => {
            assigned.insert(n.clone());
        }
        AssignTarget::Tuple(targets) => {
            for t in targets {
                collect_assign_target_bound_names(t, assigned);
            }
        }
        AssignTarget::Starred(inner) => {
            collect_assign_target_bound_names(inner, assigned);
        }
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => {}
    }
}

/// Collect names bound by a match pattern (capture patterns bind names).
fn collect_pattern_bound_names(pattern: &crate::ast::Pattern, assigned: &mut HashSet<String>) {
    match pattern {
        crate::ast::Pattern::Capture(name) => {
            assigned.insert(name.clone());
        }
        crate::ast::Pattern::Or(pats) => {
            for p in pats {
                collect_pattern_bound_names(p, assigned);
            }
        }
        crate::ast::Pattern::Sequence(elts) => {
            for (p, _) in elts {
                collect_pattern_bound_names(p, assigned);
            }
        }
        crate::ast::Pattern::Mapping(pairs, rest) => {
            for (_, p) in pairs {
                collect_pattern_bound_names(p, assigned);
            }
            if let Some(name) = rest {
                assigned.insert(name.clone());
            }
        }
        crate::ast::Pattern::Class {
            positional, kwargs, ..
        } => {
            for p in positional {
                collect_pattern_bound_names(p, assigned);
            }
            for (_, p) in kwargs {
                collect_pattern_bound_names(p, assigned);
            }
        }
        crate::ast::Pattern::Wildcard
        | crate::ast::Pattern::Literal(_)
        | crate::ast::Pattern::Value(_) => {}
        crate::ast::Pattern::As { pattern, name } => {
            collect_pattern_bound_names(pattern, assigned);
            assigned.insert(name.clone());
        }
    }
}

pub(crate) fn compute_def_bound_mask(
    params: &[crate::ast::FunctionParam],
    local_index: &HashMap<String, crate::bytecode::Reg>,
) -> u64 {
    let mut mask: u64 = 0;
    // Only parameters are guaranteed bound at function entry — they are set
    // by the call setup code before the body runs.  Body-level assignments
    // are NOT included here because a name can be read (as a local) before
    // it is assigned (e.g. `y = x; x = 9`), which would cause an unsound
    // unwrap.  The parameter-only subset is sufficient to eliminate the
    // None check for the most frequently read locals in hot inner loops.
    for param in params {
        if let Some(&idx) = local_index.get(&param.name)
            && idx < 64
        {
            mask |= 1u64 << idx;
        }
    }
    mask
}
