impl Parser {
    fn parse_decorator(&mut self) -> Result<Expr> {
        self.expect(&Token::At)?;
        let expr = self.parse_postfix()?;
        // skip newline after decorator
        if self.is(&Token::Newline) {
            self.bump();
        }
        Ok(expr)
    }

    fn parse_expr_stmt(&mut self) -> Result<Vec<Stmt>> {
        // Detect starred assignment target at the start: *name, ... = rhs
        // We need to check if the first token is Star before parsing as an expression,
        // since *name is not a valid expression.
        let first_is_star = self.is(&Token::Star);

        // Parse a comma-separated list (possible tuple or unpack target).
        // If the first token is Star, parse the inner name as a starred assign target item.
        let (first_expr, first_starred) = if first_is_star {
            // *name, ... = rhs  — parse as starred target
            self.bump(); // consume *
            let inner = self.parse_postfix()?;
            (inner, true)
        } else {
            (self.parse_expr()?, false)
        };

        // Check for more comma-separated items (tuple or unpack target).
        // We track whether any item is starred (via a parallel bool vec).
        let mut had_comma = false;
        let (targets, starred_flags) = if self.is(&Token::Comma) {
            had_comma = true;
            let mut items = vec![first_expr];
            let mut flags = vec![first_starred];
            while self.is(&Token::Comma) {
                self.bump();
                if self.at_stmt_end() {
                    break;
                }
                if self.is(&Token::Star) {
                    self.bump(); // consume *
                    let inner = self.parse_postfix()?;
                    items.push(inner);
                    flags.push(true);
                } else {
                    items.push(self.parse_expr()?);
                    flags.push(false);
                }
            }
            (items, flags)
        } else {
            (vec![first_expr], vec![first_starred])
        };

        // Type annotation:  target: Type [= rhs]
        // Only valid when there's a single non-starred target.  The annotation
        // value is parsed and discarded (we don't track types at runtime).
        //
        // Divergence from CPython: the annotation expression is parsed but not
        // evaluated. CPython evaluates annotations at runtime (unless
        // `from __future__ import annotations` is in effect), so observable
        // side effects inside an annotation are silently dropped here. In
        // practice annotations are almost always pure type expressions, so we
        // accept this trade-off in exchange for a single-line parser hook.
        if self.is(&Token::Colon)
            && targets.len() == 1
            && !starred_flags[0]
            && matches!(
                &targets[0],
                Expr::Var(_, _) | Expr::Attr { .. } | Expr::Index { .. }
            )
        {
            self.bump(); // consume :
            let annotation = self.parse_expr()?;
            if self.is(&Token::Assign) {
                self.bump(); // consume =
                let rhs = self.parse_expr()?;
                // For a simple name target, emit AnnAssign so the compiler can
                // detect conflicts with global/nonlocal (CPython SyntaxError).
                if let Expr::Var(name, _) = &targets[0] {
                    if name == "__debug__" {
                        return Err(PyError::Parse("cannot assign to __debug__".to_string()));
                    }
                    return Ok(vec![Stmt::AnnAssign {
                        name: name.clone(),
                        annotation,
                        value: Some(rhs),
                    }]);
                }
                return Ok(vec![lhs_to_assign_stmt(&targets[0], rhs)?]);
            }
            // Bare annotation declaration without value.
            // For a simple name target, emit AnnAssign { value: None } so the
            // annotation expression is preserved for storage in __annotations__
            // (at module / class scope) and so the compiler can detect conflicts
            // with global/nonlocal declarations (CPython SyntaxError).
            // For attribute/index targets (e.g. `self.x: int`) there is no local
            // slot to declare, so this remains a no-op.
            if let Expr::Var(name, _) = &targets[0] {
                if name == "__debug__" {
                    return Err(PyError::Parse("cannot assign to __debug__".to_string()));
                }
                return Ok(vec![Stmt::AnnAssign {
                    name: name.clone(),
                    annotation,
                    value: None,
                }]);
            }
            return Ok(vec![Stmt::Pass]);
        }

        // Assignment: = rhs   (possibly chained: t1 = t2 = ... = rhs)
        if self.is(&Token::Assign) {
            // Validate starred flags on the first target group.
            let star_count = starred_flags.iter().filter(|&&s| s).count();
            if star_count > 1 {
                return Err(PyError::Parse(
                    "multiple starred expressions in assignment".to_string(),
                ));
            }
            // Standalone `*a = x` is invalid, but `*a, = x` (trailing comma) is valid.
            if star_count == 1 && targets.len() == 1 && !had_comma {
                return Err(PyError::Parse(
                    "starred assignment target must be in a list or tuple".to_string(),
                ));
            }

            // Collect every target group separated by `=`.  Python parses
            // `a = b = c = expr` as multiple target groups followed by a
            // single RHS expression.  Each `target_groups` entry is a
            // `(targets, starred_flags, had_comma)` triple matching the
            // first one we just parsed above.
            let mut target_groups: Vec<(Vec<Expr>, Vec<bool>, bool)> =
                vec![(targets, starred_flags, had_comma)];

            // The final RHS expression (after the last `=`).
            let rhs;
            loop {
                self.bump(); // consume `=`
                // Parse the next group: comma-separated expressions.  Star
                // is only valid for an assignment target — we tentatively
                // parse it as such, and reject below if it turns out to be
                // the RHS.
                let first_is_star_next = self.is(&Token::Star);
                let (next_first_expr, next_first_starred) = if first_is_star_next {
                    self.bump();
                    let inner = self.parse_postfix()?;
                    (inner, true)
                } else {
                    (self.parse_expr()?, false)
                };
                let mut next_had_comma = false;
                let (next_items, next_flags) = if self.is(&Token::Comma) {
                    next_had_comma = true;
                    let mut items = vec![next_first_expr];
                    let mut flags = vec![next_first_starred];
                    while self.is(&Token::Comma) {
                        self.bump();
                        if self.at_stmt_end() {
                            break;
                        }
                        if self.is(&Token::Star) {
                            self.bump();
                            let inner = self.parse_postfix()?;
                            items.push(inner);
                            flags.push(true);
                        } else {
                            items.push(self.parse_expr()?);
                            flags.push(false);
                        }
                    }
                    (items, flags)
                } else {
                    (vec![next_first_expr], vec![next_first_starred])
                };

                if self.is(&Token::Assign) {
                    // Another `=` follows — this group is another target list.
                    let star_count = next_flags.iter().filter(|&&s| s).count();
                    if star_count > 1 {
                        return Err(PyError::Parse(
                            "multiple starred expressions in assignment".to_string(),
                        ));
                    }
                    if star_count == 1 && next_items.len() == 1 && !next_had_comma {
                        return Err(PyError::Parse(
                            "starred assignment target must be in a list or tuple".to_string(),
                        ));
                    }
                    target_groups.push((next_items, next_flags, next_had_comma));
                    continue;
                }

                // No more `=`: this last group is the RHS.  Stars are not
                // permitted in the RHS expression.
                if next_flags.iter().any(|&s| s) {
                    return Err(PyError::Parse(
                        "can't use starred expression here".to_string(),
                    ));
                }
                rhs = if next_items.len() == 1 && !next_had_comma {
                    next_items.into_iter().next().unwrap()
                } else {
                    Expr::Tuple(next_items)
                };
                break;
            }

            // Build a Stmt::Assign / AttrAssign / IndexAssign / SliceAssign for
            // each target group.  For chained assignment we evaluate the RHS
            // exactly once and reuse the value for every target.
            return build_assign_stmts(target_groups, rhs);
        }

        // Starred item outside assignment is invalid
        if starred_flags.iter().any(|&s| s) {
            return Err(PyError::Parse(
                "can't use starred expression here".to_string(),
            ));
        }

        // Augmented assignment: += -= etc.
        if let Some(aug_op) = self.current_aug_op() {
            self.bump();
            let rhs = self.parse_expr()?;
            let target = if targets.len() == 1 {
                expr_to_assign_target(&targets[0])?
            } else {
                return Err(PyError::Parse(
                    "invalid augmented assignment target".to_string(),
                ));
            };
            return Ok(vec![Stmt::AugAssign {
                target,
                op: aug_op,
                expr: rhs,
            }]);
        }

        // Plain expression statement
        if targets.len() == 1 {
            Ok(vec![Stmt::Expr(targets.into_iter().next().unwrap())])
        } else {
            Ok(vec![Stmt::Expr(Expr::Tuple(targets))])
        }
    }

    fn current_aug_op(&self) -> Option<BinaryOp> {
        match self.current() {
            Some(Token::PlusAssign) => Some(BinaryOp::Add),
            Some(Token::MinusAssign) => Some(BinaryOp::Sub),
            Some(Token::StarAssign) => Some(BinaryOp::Mul),
            Some(Token::SlashAssign) => Some(BinaryOp::Div),
            Some(Token::SlashSlashAssign) => Some(BinaryOp::FloorDiv),
            Some(Token::PercentAssign) => Some(BinaryOp::Mod),
            Some(Token::StarStarAssign) => Some(BinaryOp::Pow),
            Some(Token::AmpersandAssign) => Some(BinaryOp::BitAnd),
            Some(Token::PipeAssign) => Some(BinaryOp::BitOr),
            Some(Token::CaretAssign) => Some(BinaryOp::BitXor),
            Some(Token::LShiftAssign) => Some(BinaryOp::LShift),
            Some(Token::RShiftAssign) => Some(BinaryOp::RShift),
            Some(Token::AtAssign) => Some(BinaryOp::MatMul),
            _ => None,
        }
    }

    fn parse_def(
        &mut self,
        decorators: Vec<Expr>,
        is_async: bool,
        deco_lineno: u32,
    ) -> Result<Stmt> {
        // `co_firstlineno` is the line of the first decorator when the function
        // is decorated, otherwise the `def` keyword's line (CPython 3.12;
        // issue #2185).  `deco_lineno` is `0` when undecorated.
        let def_lineno = if deco_lineno != 0 {
            deco_lineno
        } else {
            self.current_lineno()
        };
        self.expect(&Token::Def)?;
        let name = self.expect_ident("function name")?;
        // PEP 695: optional `[T, U, ...]` type parameter list before `(`.
        let type_params = self.parse_type_params("function")?;
        self.expect(&Token::LParen)?;
        let params = self.parse_params()?;
        self.expect(&Token::RParen)?;
        // Optional return annotation
        let return_annotation = if self.is(&Token::Arrow) {
            self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(&Token::Colon)?;
        let (body, body_linenos) = self.parse_suite_with_linenos()?;
        Ok(Stmt::Def {
            name,
            params,
            body,
            body_linenos,
            def_lineno,
            decorators,
            return_annotation,
            is_async,
            type_params,
        })
    }

    /// Parse `async def name(...) -> ...: ...`  (the `async` keyword has already
    /// been identified by the caller via lookahead but not consumed yet).
    fn parse_async_def(&mut self, decorators: Vec<Expr>, deco_lineno: u32) -> Result<Stmt> {
        self.bump(); // consume `async`
        self.parse_def(decorators, true, deco_lineno)
    }

    fn parse_params(&mut self) -> Result<Vec<FunctionParam>> {
        let mut params = Vec::new();
        let mut seen_default = false;
        let mut seen_args = false;
        let mut seen_kwargs = false;
        let mut seen_star = false; // bare * or *args seen — params after are keyword-only
        let mut seen_slash = false;

        if self.is(&Token::RParen) {
            return Ok(params);
        }

        loop {
            if seen_kwargs {
                return Err(PyError::Parse("parameter after **kwargs".to_string()));
            }

            if self.is(&Token::Slash) {
                if seen_slash {
                    return Err(PyError::Parse(
                        "duplicate '/' in function parameter list".to_string(),
                    ));
                }
                if seen_star {
                    return Err(PyError::Parse(
                        "'/' must appear before '*' in parameter list".to_string(),
                    ));
                }
                if params.is_empty() {
                    return Err(PyError::Parse(
                        "at least one parameter must precede '/'".to_string(),
                    ));
                }
                // Mark all prior params as positional-only.
                for p in &mut params {
                    p.is_positional_only = true;
                }
                seen_slash = true;
                self.bump();
            } else if self.is(&Token::StarStar) {
                self.bump();
                let name = self.expect_ident("kwargs parameter name")?;
                // **kwargs can have an annotation in theory but it is very rare
                // and CPython does include it in __annotations__; retain it.
                let annotation = if self.is(&Token::Colon) {
                    self.bump();
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                let default = if self.is(&Token::Assign) {
                    self.bump();
                    seen_default = true;
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                params.push(FunctionParam {
                    name,
                    default,
                    annotation,
                    is_args: false,
                    is_kwargs: true,
                    is_keyword_only: false,
                    is_positional_only: false,
                });
                seen_kwargs = true;
            } else if self.is(&Token::Star) {
                self.bump();
                seen_star = true;
                if self.is(&Token::Comma) || self.is(&Token::RParen) {
                    // bare * separator: keyword-only follows
                } else {
                    let name = self.expect_ident("args parameter name")?;
                    // *args can have an annotation; retain it.
                    let annotation = if self.is(&Token::Colon) {
                        self.bump();
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    params.push(FunctionParam {
                        name,
                        default: None,
                        annotation,
                        is_args: true,
                        is_kwargs: false,
                        is_keyword_only: false,
                        is_positional_only: false,
                    });
                    seen_args = true;
                }
            } else {
                match self.current().cloned() {
                    Some(Token::Ident(name)) => {
                        self.bump();
                        // Optional annotation
                        let annotation = if self.is(&Token::Colon) {
                            self.bump();
                            Some(self.parse_expr()?)
                        } else {
                            None
                        };
                        let default = if self.is(&Token::Assign) {
                            self.bump();
                            seen_default = true;
                            Some(self.parse_expr()?)
                        } else {
                            if seen_default && !seen_args && !seen_star {
                                return Err(PyError::Parse(
                                    "non-default argument follows default argument".to_string(),
                                ));
                            }
                            None
                        };
                        params.push(FunctionParam {
                            name,
                            default,
                            annotation,
                            is_args: false,
                            is_kwargs: false,
                            is_keyword_only: seen_star,
                            is_positional_only: false,
                        });
                    }
                    other => {
                        return Err(PyError::Parse(format!(
                            "expected parameter name, found {other:?}"
                        )));
                    }
                }
            }

            if self.is(&Token::RParen) {
                break;
            }
            self.expect(&Token::Comma)?;
            if self.is(&Token::RParen) {
                break;
            }
        }

        check_duplicate_params(&params)?;
        Ok(params)
    }

    fn parse_class(&mut self, decorators: Vec<Expr>) -> Result<Stmt> {
        self.expect(&Token::Class)?;
        let name = self.expect_ident("class name")?;
        // PEP 695: optional `[T, U, ...]` type parameter list before `(` or `:`.
        let type_params = self.parse_type_params("class")?;

        let mut bases = Vec::new();
        let mut metaclass: Option<Expr> = None;
        let mut keywords: Vec<(String, Expr)> = Vec::new();
        if self.is(&Token::LParen) {
            self.bump();
            if !self.is(&Token::RParen) {
                loop {
                    // Detect keyword argument: `ident = expr`
                    let is_kwarg = matches!(self.current(), Some(Token::Ident(_)))
                        && self.peek() == Some(&Token::Assign);
                    if is_kwarg {
                        let key = self.expect_ident("class keyword")?;
                        self.expect(&Token::Assign)?;
                        let value = self.parse_expr()?;
                        if key == "metaclass" {
                            if metaclass.is_some() {
                                return Err(PyError::Parse(
                                    "duplicate 'metaclass' keyword in class header".to_string(),
                                ));
                            }
                            metaclass = Some(value);
                        } else {
                            // PEP 487: forward other kwargs to __init_subclass__.
                            keywords.push((key, value));
                        }
                    } else {
                        bases.push(self.parse_expr()?);
                    }
                    if self.is(&Token::RParen) {
                        break;
                    }
                    self.expect(&Token::Comma)?;
                    if self.is(&Token::RParen) {
                        break;
                    }
                }
            }
            self.expect(&Token::RParen)?;
        }

        self.expect(&Token::Colon)?;
        let body = self.parse_suite()?;
        Ok(Stmt::Class {
            name,
            bases,
            metaclass,
            keywords,
            body,
            decorators,
            type_params,
        })
    }

    fn parse_global(&mut self) -> Result<Stmt> {
        self.expect(&Token::Global)?;
        let names = self.parse_name_list("global")?;
        Ok(Stmt::Global(names))
    }

    fn parse_nonlocal(&mut self) -> Result<Stmt> {
        self.expect(&Token::Nonlocal)?;
        let names = self.parse_name_list("nonlocal")?;
        Ok(Stmt::Nonlocal(names))
    }

    fn parse_name_list(&mut self, ctx: &str) -> Result<Vec<String>> {
        let mut names = Vec::new();
        loop {
            names.push(self.expect_ident(ctx)?);
            if !self.is(&Token::Comma) {
                break;
            }
            self.bump();
        }
        Ok(names)
    }
}
