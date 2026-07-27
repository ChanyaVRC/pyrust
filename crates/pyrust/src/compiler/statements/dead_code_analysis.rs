impl Compiler {
    /// Walk `stmts` without emitting code and report context-sensitive syntax
    /// errors that Python rejects even in unreachable branches.
    ///
    /// `in_loop` becomes true while recursing into a loop body. Function and
    /// class bodies retain their own scope flags while their interiors are
    /// validated.
    fn check_dead_block(&mut self, stmts: &[Stmt], in_loop: bool) {
        for stmt in stmts {
            if self.failed {
                return;
            }
            // Statement-level context rules take precedence over errors inside
            // their child expressions (for example, `return await x` at module
            // scope is first and foremost a return outside a function).
            match stmt {
                Stmt::Break if !in_loop => {
                    self.set_syntax_error("'break' outside loop");
                }
                Stmt::Continue if !in_loop => {
                    self.set_syntax_error("'continue' not properly in loop");
                }
                Stmt::Return(_) if !self.is_function_scope => {
                    self.set_syntax_error("'return' outside function");
                }
                Stmt::Return(Some(_)) if self.is_async_generator_fn => {
                    self.set_syntax_error("'return' with value in async generator");
                }
                Stmt::Nonlocal(_) if !self.is_function_scope && !self.is_class_body => {
                    self.set_syntax_error("nonlocal declaration not allowed at module level");
                }
                Stmt::For { is_async: true, .. }
                    if !self.is_function_scope || !self.is_async_function =>
                {
                    self.set_syntax_error("'async for' outside async function");
                }
                Stmt::With { is_async: true, .. }
                    if !self.is_function_scope || !self.is_async_function =>
                {
                    self.set_syntax_error("'async with' outside async function");
                }
                _ => {}
            }
            if self.failed {
                return;
            }

            // Every expression evaluated by the statement header belongs here,
            // including assignment-target operands, control-flow conditions,
            // definition defaults/decorators, exception kinds and match
            // patterns/guards. Nested blocks are visited below after their
            // function/class/loop scope transition.
            stmt.visit_evaluated_exprs(&mut |expr| self.check_dead_expr(expr));
            if self.failed {
                return;
            }

            match stmt {
                Stmt::If {
                    branches,
                    else_branch,
                    ..
                } => {
                    for (_, body) in branches {
                        self.check_dead_block(body, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                    }
                }
                Stmt::While {
                    body, else_branch, ..
                } => {
                    self.check_dead_block(body, true);
                    if self.failed {
                        return;
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                    }
                }
                Stmt::For {
                    body, else_branch, ..
                } => {
                    self.check_dead_block(body, true);
                    if self.failed {
                        return;
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                    }
                }
                Stmt::Try {
                    body,
                    handlers,
                    else_branch,
                    finally_branch,
                    ..
                } => {
                    self.check_dead_block(body, in_loop);
                    if self.failed {
                        return;
                    }
                    for handler in handlers {
                        self.check_dead_block(&handler.body, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                    if let Some(finally_stmts) = finally_branch {
                        self.check_dead_block(finally_stmts, in_loop);
                    }
                }
                Stmt::With { body, .. } => {
                    self.check_dead_block(body, in_loop);
                }
                Stmt::Match { arms, .. } => {
                    for arm in arms {
                        self.check_dead_block(&arm.body, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                }
                // Def bodies open a new function scope: break/continue are not
                // valid inside a function (even inside a loop in the enclosing
                // scope), and nonlocal names must be bound in an enclosing
                // function scope.  CPython validates these even for defs that
                // appear in dead-code branches, so we must run the checks here
                // rather than relying on child compilation (which is skipped for
                // dead code).
                Stmt::Def {
                    params,
                    body,
                    is_async,
                    ..
                } => {
                    let inner_nonlocal = crate::interpreter::collect_nonlocal_names(body);
                    let mut sorted_nonlocals: Vec<&String> = inner_nonlocal.iter().collect();
                    sorted_nonlocals.sort();
                    for nonlocal_name in sorted_nonlocals {
                        let in_params = params.iter().any(|p| &p.name == nonlocal_name);
                        let found = in_params
                            || self
                                .outer_locals
                                .iter()
                                .any(|m| m.contains_key(nonlocal_name))
                            || (self.is_function_scope
                                && self.local_index.contains_key(nonlocal_name));
                        if !found {
                            self.set_syntax_error(&format!(
                                "no binding for nonlocal '{}' found",
                                nonlocal_name
                            ));
                            return;
                        }
                    }
                    let saved_is_function_scope = self.is_function_scope;
                    let saved_is_async_function = self.is_async_function;
                    let saved_is_async_generator_fn = self.is_async_generator_fn;
                    let saved_is_class_body = self.is_class_body;
                    self.is_function_scope = true;
                    self.is_async_function = *is_async;
                    self.is_async_generator_fn = *is_async && stmts_contain_yield(body);
                    self.is_class_body = false;
                    self.check_dead_block(body, false);
                    self.is_function_scope = saved_is_function_scope;
                    self.is_async_function = saved_is_async_function;
                    self.is_async_generator_fn = saved_is_async_generator_fn;
                    self.is_class_body = saved_is_class_body;
                }
                Stmt::Class { body, .. } => {
                    let saved_is_function_scope = self.is_function_scope;
                    let saved_is_async_generator_fn = self.is_async_generator_fn;
                    let saved_is_class_body = self.is_class_body;
                    self.is_function_scope = false;
                    self.is_async_generator_fn = false;
                    self.is_class_body = true;
                    self.check_dead_block(body, false);
                    self.is_function_scope = saved_is_function_scope;
                    self.is_async_generator_fn = saved_is_async_generator_fn;
                    self.is_class_body = saved_is_class_body;
                }
                // All other statements have no nested blocks or context rules.
                _ => {}
            }
        }
    }

    fn check_dead_expr(&mut self, expr: &Expr) {
        if self.failed {
            return;
        }
        match expr {
            Expr::Yield(value) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'yield' outside function");
                } else if let Some(value) = value {
                    self.check_dead_expr(value);
                }
            }
            Expr::YieldFrom(value) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'yield' outside function");
                } else if self.is_async_function {
                    self.set_syntax_error("'yield from' inside async function");
                } else {
                    self.check_dead_expr(value);
                }
            }
            Expr::Await(value) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'await' outside function");
                } else if !self.is_async_function {
                    self.set_syntax_error("'await' outside async function");
                } else {
                    self.check_dead_expr(value);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_dead_expr(left);
                if !self.failed {
                    self.check_dead_expr(right);
                }
            }
            Expr::Unary { expr: e, .. } => {
                self.check_dead_expr(e);
            }
            Expr::Compare { left, ops } => {
                self.check_dead_expr(left);
                for (_, operand) in ops {
                    self.check_dead_expr(operand);
                }
            }
            Expr::Ternary { cond, then, else_ } => {
                self.check_dead_expr(cond);
                if !self.failed {
                    self.check_dead_expr(then);
                }
                if !self.failed {
                    self.check_dead_expr(else_);
                }
            }
            Expr::Call { func, args, .. } => {
                self.check_dead_expr(func);
                for a in args {
                    if self.failed {
                        return;
                    }
                    self.check_dead_expr(&a.value);
                }
            }
            Expr::Attr { target, .. }
            | Expr::Starred(target)
            | Expr::Named { value: target, .. } => {
                self.check_dead_expr(target);
            }
            Expr::Index { target, index, .. } => {
                self.check_dead_expr(target);
                self.check_dead_expr(index);
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                self.check_dead_expr(target);
                for bound in [lower, upper, step].into_iter().flatten() {
                    self.check_dead_expr(bound);
                }
            }
            Expr::Tuple(elts) | Expr::List(elts) | Expr::Set(elts) => {
                for e in elts {
                    if self.failed {
                        return;
                    }
                    self.check_dead_expr(e);
                }
            }
            Expr::Dict(items) => {
                for item in items {
                    match item {
                        crate::ast::DictItem::Pair(key, value) => {
                            self.check_dead_expr(key);
                            self.check_dead_expr(value);
                        }
                        crate::ast::DictItem::DoubleSplat(value) => {
                            self.check_dead_expr(value);
                        }
                    }
                }
            }
            Expr::FString(parts) => {
                self.check_dead_fstring_parts(parts);
            }
            Expr::Lambda { params, body } => {
                // Defaults and annotations execute in the enclosing scope.
                for parameter in params {
                    if let Some(default) = &parameter.default {
                        self.check_dead_expr(default);
                    }
                    if let Some(annotation) = &parameter.annotation {
                        self.check_dead_expr(annotation);
                    }
                }
                if self.failed {
                    return;
                }

                // The lambda body is a distinct, always-synchronous function
                // scope. A yield is valid there; an await is not inherited from
                // an enclosing async function.
                let saved_is_function_scope = self.is_function_scope;
                let saved_is_async_function = self.is_async_function;
                let saved_is_class_body = self.is_class_body;
                self.is_function_scope = true;
                self.is_async_function = false;
                self.is_class_body = false;
                self.check_dead_expr(body);
                self.is_function_scope = saved_is_function_scope;
                self.is_async_function = saved_is_async_function;
                self.is_class_body = saved_is_class_body;
            }
            Expr::ListComp { elt, clauses } => {
                self.check_dead_comprehension(&[elt], clauses, "list comprehension", true);
            }
            Expr::SetComp { elt, clauses } => {
                self.check_dead_comprehension(&[elt], clauses, "set comprehension", true);
            }
            Expr::DictComp { key, val, clauses } => {
                self.check_dead_comprehension(&[key, val], clauses, "dict comprehension", true);
            }
            Expr::GenExp { elt, clauses } => {
                self.check_dead_comprehension(&[elt], clauses, "generator expression", false);
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
        }
    }

    fn check_dead_fstring_parts(&mut self, parts: &[crate::ast::FStringPart]) {
        for part in parts {
            let crate::ast::FStringPart::Expr {
                expr, format_spec, ..
            } = part
            else {
                continue;
            };
            self.check_dead_expr(expr);
            if let Some(format_spec) = format_spec {
                self.check_dead_fstring_parts(format_spec);
            }
        }
    }

    fn check_dead_comprehension(
        &mut self,
        result_exprs: &[&Expr],
        clauses: &[crate::ast::CompClause],
        kind: &str,
        collection_requires_async_parent: bool,
    ) {
        let Some((outermost, _)) = clauses.split_first() else {
            self.set_syntax_error(&format!("{kind} requires at least one clause"));
            return;
        };

        // Collection comprehensions execute an async body by awaiting its
        // implicit coroutine, so unlike generator expressions they require an
        // async enclosing function.
        let is_async = clauses.iter().any(|clause| clause.is_async)
            || Self::comp_body_has_await(result_exprs, clauses);
        if let Some(message) = check_comprehension(result_exprs, clauses, kind) {
            self.set_syntax_error(&message);
            return;
        }
        if collection_requires_async_parent && is_async && !self.is_async_function {
            self.set_syntax_error("asynchronous comprehension outside of an asynchronous function");
            return;
        }

        // The outermost iterable alone is evaluated in the enclosing scope;
        // the element, conditions and later iterables belong to the
        // comprehension's implicit function and were validated above.
        self.check_dead_expr(&outermost.iter);
    }
}
