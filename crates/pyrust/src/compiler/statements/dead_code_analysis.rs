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
                Stmt::Expr(expr) => {
                    self.check_dead_expr(expr);
                }
                Stmt::Nonlocal(_) if !self.is_function_scope && !self.is_class_body => {
                    self.set_syntax_error("nonlocal declaration not allowed at module level");
                }
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
                    let saved_is_class_body = self.is_class_body;
                    self.is_function_scope = true;
                    self.is_async_function = *is_async;
                    self.is_class_body = false;
                    self.check_dead_block(body, false);
                    self.is_function_scope = saved_is_function_scope;
                    self.is_async_function = saved_is_async_function;
                    self.is_class_body = saved_is_class_body;
                }
                Stmt::Class { body, .. } => {
                    let saved_is_function_scope = self.is_function_scope;
                    let saved_is_class_body = self.is_class_body;
                    self.is_function_scope = false;
                    self.is_class_body = true;
                    self.check_dead_block(body, false);
                    self.is_function_scope = saved_is_function_scope;
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
            Expr::Yield(_) | Expr::YieldFrom(_) if !self.is_function_scope => {
                self.set_syntax_error("'yield' outside function");
            }
            Expr::Await(_) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'await' outside function");
                } else if !self.is_async_function {
                    self.set_syntax_error("'await' outside async function");
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
            Expr::Tuple(elts) | Expr::List(elts) | Expr::Set(elts) => {
                for e in elts {
                    if self.failed {
                        return;
                    }
                    self.check_dead_expr(e);
                }
            }
            Expr::Named { value, .. } => {
                self.check_dead_expr(value);
            }
            Expr::Lambda { .. }
            | Expr::ListComp { .. }
            | Expr::SetComp { .. }
            | Expr::DictComp { .. }
            | Expr::GenExp { .. } => {}
            _ => {}
        }
    }
}
