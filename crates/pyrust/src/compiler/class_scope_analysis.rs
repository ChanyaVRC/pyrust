/// Walk a class body and record names bound at the top level in their
/// *textual* order — used only to assign **register slot numbers** for
/// the class-body sub-compiler.  Slot order has **no** influence on
/// class-namespace insertion order any more (`vars(C)` follows runtime
/// stores via `Insn::RecordClassStore`); we keep this textual walk so
/// register assignments match declaration order even for names that only
/// appear inside nested control-flow.  Names not in `body_local` are
/// skipped (they're declared `global` / `nonlocal` and don't get a
/// class-body slot).
fn collect_class_body_names_textual(
    body: &[Stmt],
    ordered: &mut Vec<String>,
    seen: &mut HashSet<String>,
    body_local: &indexmap::IndexSet<String>,
) {
    for stmt in body {
        match stmt {
            Stmt::Assign(target, _) => {
                collect_assign_target_textual(target, ordered, seen, body_local);
            }
            Stmt::AnnAssign { name, .. }
                if body_local.contains(name) && seen.insert(name.clone()) =>
            {
                ordered.push(name.clone());
            }
            Stmt::Def { name, .. } | Stmt::Class { name, .. } | Stmt::TypeAlias { name, .. }
                if body_local.contains(name) && seen.insert(name.clone()) =>
            {
                ordered.push(name.clone());
            }
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    collect_class_body_names_textual(b, ordered, seen, body_local);
                }
                if let Some(b) = else_branch {
                    collect_class_body_names_textual(b, ordered, seen, body_local);
                }
            }
            Stmt::While { body, .. } | Stmt::For { body, .. } => {
                collect_class_body_names_textual(body, ordered, seen, body_local);
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_class_body_names_textual(body, ordered, seen, body_local);
                for h in handlers {
                    collect_class_body_names_textual(&h.body, ordered, seen, body_local);
                }
                if let Some(b) = else_branch {
                    collect_class_body_names_textual(b, ordered, seen, body_local);
                }
                if let Some(b) = finally_branch {
                    collect_class_body_names_textual(b, ordered, seen, body_local);
                }
            }
            Stmt::With { body, .. } => {
                collect_class_body_names_textual(body, ordered, seen, body_local);
            }
            _ => {}
        }
    }
}

fn collect_assign_target_textual(
    target: &AssignTarget,
    ordered: &mut Vec<String>,
    seen: &mut HashSet<String>,
    body_local: &indexmap::IndexSet<String>,
) {
    match target {
        AssignTarget::Name(name) => {
            if body_local.contains(name) && seen.insert(name.clone()) {
                ordered.push(name.clone());
            }
        }
        AssignTarget::Tuple(targets) => {
            for t in targets {
                collect_assign_target_textual(t, ordered, seen, body_local);
            }
        }
        AssignTarget::Starred(inner) => {
            collect_assign_target_textual(inner, ordered, seen, body_local);
        }
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => {}
    }
}

/// For a class body, collect names that the class's methods read as free
/// variables from the enclosing scope.  Python class scope is not a closure
/// scope for methods: `def method(self): return x` reads the outer `x`, not
/// any `x` defined at class level.  Promote those names to cell vars so they
/// live in the env (not registers) and are accessible via `LoadGlobal`.
fn collect_class_method_outer_refs(
    class_body: &[Stmt],
    local_index: &HashMap<String, Reg>,
    outer_is_class_scope: bool,
    cells: &mut HashSet<String>,
) {
    // When `local_index` belongs to a *function* scope (`outer_is_class_scope=false`),
    // methods inside this class can close over function locals even when those names
    // are also assigned in the class body.  Python class scope is not a closure scope
    // for methods — a method's free-variable lookup skips the class body entirely.
    //
    // When `local_index` belongs to a *class* scope (`outer_is_class_scope=true`),
    // we must NOT promote class-body names as cell vars just because a further-nested
    // method reads a name that happens to be a class attribute.  The nested method
    // also skips the outer class scope, so promoting would incorrectly turn class
    // attribute assignments into StoreGlobal, stripping the attribute from the dict.
    // Always compute class_locals for use in lambda handling: lambdas in a
    // class body close over the enclosing function scope, not the class scope,
    // so we need the class-body local names to avoid false promotions (issue #699).
    let empty_set: HashSet<String> = HashSet::new();
    let class_locals =
        crate::interpreter::collect_local_names(&[], class_body, &empty_set, &empty_set);
    // For method Def arms: only filter out class-body locals when the outer
    // scope is itself a class scope (outer_is_class_scope=true).  When the
    // outer scope is a function scope, methods may close over function locals
    // even when the class body also defines a name with the same spelling.
    let class_locals_opt: Option<&indexmap::IndexSet<String>> = if outer_is_class_scope {
        Some(&class_locals)
    } else {
        None
    };

    for stmt in class_body {
        match stmt {
            Stmt::Def {
                params,
                body: method_body,
                ..
            } => {
                let inner_globals = crate::interpreter::collect_global_names(method_body);
                // Do NOT promote `global x` declarations from methods here.
                // A method's `global x` routes directly to the module environment
                // regardless of whether the enclosing scope is a function or a class.
                // Promoting them would incorrectly force the outer scope's `x` into a
                // cell var, which for a class body means `x = ...` emits StoreGlobal
                // instead of RecordClassStore (issue #629; see also issue #624).
                let inner_nonlocals = crate::interpreter::collect_nonlocal_names(method_body);
                // Promote nonlocal declarations in methods to cell vars in the
                // enclosing function scope.  This mirrors what `collect_cell_vars_in`
                // does for plain nested `Def`s: any name declared `nonlocal` inside a
                // method must live in the outer env so the method closure can mutate it.
                for name in &inner_nonlocals {
                    if local_index.contains_key(name) {
                        cells.insert(name.clone());
                    }
                }
                let inner_locals = crate::interpreter::collect_local_names(
                    params,
                    method_body,
                    &inner_globals,
                    &inner_nonlocals,
                );
                let mut uses: HashSet<String> = HashSet::new();
                collect_free_var_reads_in_stmts(method_body, &mut uses);
                // Functions/classes nested deeper inside this method may also
                // reference outer names that the method itself never mentions.
                collect_transitive_free_vars_in_stmts(method_body, &mut uses);
                // Note: we intentionally do NOT filter by class-body locals here.
                // Python class scope is not a closure scope for methods: a method
                // reading `x` skips the class namespace entirely and looks in the
                // enclosing function scope.  Even when the class also defines `x`,
                // the outer function's `x` must be promoted to a cell var so the
                // method can reach it (issue #700).
                for name in uses {
                    if !inner_locals.contains(&name)
                        && !inner_globals.contains(&name)
                        && !inner_nonlocals.contains(&name)
                        && class_locals_opt.is_none_or(|cl| !cl.contains(&name))
                        && local_index.contains_key(&name)
                    {
                        cells.insert(name);
                    }
                }
            }
            // Lambdas assigned at class body level (e.g. `fn = lambda self: x`)
            // also close over the *enclosing function* scope (not the class scope).
            // The class body's own `collect_lambda_captures` correctly skips
            // promoting their reads to class cell vars (issue #699), but we still
            // need to promote those reads to cell vars in the *enclosing function*
            // so the env chain carries them when the generator body resumes.
            Stmt::Assign(target, value) => {
                target.visit_evaluated_exprs(&mut |expr| {
                    collect_class_lambda_outer_refs_in_expr(expr, local_index, &class_locals, cells)
                });
                collect_class_lambda_outer_refs_in_expr(value, local_index, &class_locals, cells);
            }
            Stmt::Expr(e) => {
                collect_class_lambda_outer_refs_in_expr(e, local_index, &class_locals, cells);
            }
            Stmt::AugAssign { target, expr, .. } => {
                target.visit_evaluated_exprs(&mut |expr| {
                    collect_class_lambda_outer_refs_in_expr(expr, local_index, &class_locals, cells)
                });
                collect_class_lambda_outer_refs_in_expr(expr, local_index, &class_locals, cells);
            }
            // Recurse into nested class bodies.  A lambda or method inside
            // `class B` inside `class A` inside a function can still read the
            // outer function's locals; without this arm those reads are never
            // seen and `x` is never promoted to a cell var (issue #703).
            // Use `outer_is_class_scope` to properly handle nested class scopes.
            Stmt::Class {
                body: nested_class_body,
                ..
            } => {
                collect_class_method_outer_refs(
                    nested_class_body,
                    local_index,
                    outer_is_class_scope,
                    cells,
                );
            }
            // Recursively handle class-level control flow.
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
                if let Some(b) = else_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_class_method_outer_refs(body, local_index, outer_is_class_scope, cells);
                if let Some(b) = else_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
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
                    collect_class_lambda_outer_refs_in_expr(expr, local_index, &class_locals, cells)
                });
                collect_class_lambda_outer_refs_in_expr(iter, local_index, &class_locals, cells);
                collect_class_method_outer_refs(body, local_index, outer_is_class_scope, cells);
                if let Some(b) = else_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                collect_class_method_outer_refs(body, local_index, outer_is_class_scope, cells);
                for h in handlers {
                    collect_class_method_outer_refs(
                        &h.body,
                        local_index,
                        outer_is_class_scope,
                        cells,
                    );
                }
                if let Some(b) = else_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
                if let Some(b) = finally_branch {
                    collect_class_method_outer_refs(b, local_index, outer_is_class_scope, cells);
                }
            }
            Stmt::With { items, body, .. } => {
                for (expr, target) in items {
                    collect_class_lambda_outer_refs_in_expr(
                        expr,
                        local_index,
                        &class_locals,
                        cells,
                    );
                    if let Some(target) = target {
                        target.visit_evaluated_exprs(&mut |expr| {
                            collect_class_lambda_outer_refs_in_expr(
                                expr,
                                local_index,
                                &class_locals,
                                cells,
                            )
                        });
                    }
                }
                collect_class_method_outer_refs(body, local_index, outer_is_class_scope, cells);
            }
            Stmt::Match { subject, arms } => {
                collect_class_lambda_outer_refs_in_expr(subject, local_index, &class_locals, cells);
                for arm in arms {
                    arm.pattern.visit_evaluated_exprs(&mut |expr| {
                        collect_class_lambda_outer_refs_in_expr(
                            expr,
                            local_index,
                            &class_locals,
                            cells,
                        )
                    });
                    if let Some(guard) = &arm.guard {
                        collect_class_lambda_outer_refs_in_expr(
                            guard,
                            local_index,
                            &class_locals,
                            cells,
                        );
                    }
                    collect_class_method_outer_refs(
                        &arm.body,
                        local_index,
                        outer_is_class_scope,
                        cells,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Walk `expr` in a class body context.  For each `Expr::Lambda` found,
/// collect its free-var reads, subtract the lambda's own params and the
/// class-body local names, then promote any remaining names that live in
/// the enclosing function's `local_index` to cell vars.
///
/// This is the mirror of the `Expr::Lambda` arm in `lambda_captures_in_expr`
/// for the class-body case: `collect_lambda_captures` (called on the class
/// body) correctly *skips* promotion into the class cell-var set (issue #699),
/// but when the class body is nested inside a function the enclosing function
/// still needs those names promoted so the env chain carries them (issue #701).
// `class_locals` is threaded for symmetry with the sibling collectors and to
// document the class-scope context; removing it would churn ~30 recursive call
// sites + 3 external callers for no behavior change.
#[allow(clippy::only_used_in_recursion)]
fn collect_class_lambda_outer_refs_in_expr(
    expr: &Expr,
    local_index: &HashMap<String, Reg>,
    class_locals: &indexmap::IndexSet<String>,
    cells: &mut HashSet<String>,
) {
    match expr {
        Expr::Lambda { params, body } => {
            let mut body_uses: HashSet<String> = HashSet::new();
            collect_free_var_reads_in_expr(body, &mut body_uses);
            collect_transitive_free_vars_in_expr(body, &mut body_uses);
            for p in params {
                body_uses.remove(&p.name);
            }
            // Promote any name that the lambda reads from the enclosing function.
            // Note: we do NOT filter out class-body locals here.  Python class
            // scope is not a closure scope — a lambda in a class body that reads
            // `x` always sees the enclosing function/module value even when the
            // class body also has `x = ...`.  The class body's own emit path
            // (`collect_cell_vars_for_class_body`) independently does not promote
            // class-attribute names to cell vars (issue #699), so the class-body
            // assignment correctly emits `RecordClassStore` regardless of what
            // the enclosing function promotes.
            for name in body_uses {
                if local_index.contains_key(&name) {
                    cells.insert(name);
                }
            }
            // A lambda default executes in the class body, but a nested lambda
            // created by that default still skips the class namespace and closes
            // over the surrounding function. Keep defaults separate from body
            // parameter shadowing and recurse with the class-aware collector.
            for p in params {
                if let Some(default) = &p.default {
                    collect_class_lambda_outer_refs_in_expr(
                        default,
                        local_index,
                        class_locals,
                        cells,
                    );
                }
                if let Some(annotation) = &p.annotation {
                    collect_class_lambda_outer_refs_in_expr(
                        annotation,
                        local_index,
                        class_locals,
                        cells,
                    );
                }
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_class_lambda_outer_refs_in_expr(left, local_index, class_locals, cells);
            collect_class_lambda_outer_refs_in_expr(right, local_index, class_locals, cells);
        }
        Expr::Unary { expr: e, .. } => {
            collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells)
        }
        Expr::Compare { left, ops } => {
            collect_class_lambda_outer_refs_in_expr(left, local_index, class_locals, cells);
            for (_, e) in ops {
                collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells);
            }
        }
        Expr::Call { func, args, .. } => {
            collect_class_lambda_outer_refs_in_expr(func, local_index, class_locals, cells);
            for a in args {
                collect_class_lambda_outer_refs_in_expr(&a.value, local_index, class_locals, cells);
            }
        }
        Expr::Attr { target, .. } => {
            collect_class_lambda_outer_refs_in_expr(target, local_index, class_locals, cells)
        }
        Expr::Index { target, index, .. } => {
            collect_class_lambda_outer_refs_in_expr(target, local_index, class_locals, cells);
            collect_class_lambda_outer_refs_in_expr(index, local_index, class_locals, cells);
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            collect_class_lambda_outer_refs_in_expr(target, local_index, class_locals, cells);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells);
            }
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells);
            }
        }
        Expr::Starred(inner) => {
            collect_class_lambda_outer_refs_in_expr(inner, local_index, class_locals, cells)
        }
        Expr::Dict(items) => {
            for item in items {
                match item {
                    DictItem::Pair(k, v) => {
                        collect_class_lambda_outer_refs_in_expr(
                            k,
                            local_index,
                            class_locals,
                            cells,
                        );
                        collect_class_lambda_outer_refs_in_expr(
                            v,
                            local_index,
                            class_locals,
                            cells,
                        );
                    }
                    DictItem::DoubleSplat(e) => {
                        collect_class_lambda_outer_refs_in_expr(
                            e,
                            local_index,
                            class_locals,
                            cells,
                        );
                    }
                }
            }
        }
        // All comprehension forms create an implicit nested function scope.
        // Only the outermost iterable is evaluated in the enclosing (class) scope.
        Expr::ListComp { clauses, .. }
        | Expr::SetComp { clauses, .. }
        | Expr::GenExp { clauses, .. } => {
            if let Some(first) = clauses.first() {
                collect_class_lambda_outer_refs_in_expr(
                    &first.iter,
                    local_index,
                    class_locals,
                    cells,
                );
            }
        }
        Expr::DictComp { clauses, .. } => {
            if let Some(first) = clauses.first() {
                collect_class_lambda_outer_refs_in_expr(
                    &first.iter,
                    local_index,
                    class_locals,
                    cells,
                );
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_class_lambda_outer_refs_in_expr(cond, local_index, class_locals, cells);
            collect_class_lambda_outer_refs_in_expr(then, local_index, class_locals, cells);
            collect_class_lambda_outer_refs_in_expr(else_, local_index, class_locals, cells);
        }
        Expr::Named { value, .. } => {
            collect_class_lambda_outer_refs_in_expr(value, local_index, class_locals, cells)
        }
        Expr::FString(parts) => {
            for_each_fstring_expr(parts, &mut |e| {
                collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells);
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
        Expr::Yield(Some(e)) => {
            collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells)
        }
        Expr::Yield(None) => {}
        Expr::YieldFrom(e) => {
            collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells)
        }
        Expr::Await(e) => {
            collect_class_lambda_outer_refs_in_expr(e, local_index, class_locals, cells)
        }
    }
}
