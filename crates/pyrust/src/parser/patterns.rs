impl Parser {
    fn parse_type_alias(&mut self) -> Result<Stmt> {
        // Consume `type` (soft keyword, tokenised as Ident).
        self.bump();
        let name = self.expect_ident("type alias name")?;
        // Parse optional generic type parameters `[T, U: int, ...]` via the
        // shared helper so bounds/constraints are captured uniformly with the
        // function/class paths.
        let type_params = self.parse_type_params("type alias")?;
        self.expect(&Token::Assign)?;
        let value = self.parse_expr_or_tuple()?;
        Ok(Stmt::TypeAlias {
            name,
            type_params,
            value,
        })
    }

    /// Parse an optional PEP 695 type parameter list `[T, U, ...]`.
    /// Returns the list of type parameter names (bounds are parsed and discarded).
    /// Consumes `[` through the matching `]`; returns an empty `Vec` if no `[`
    /// is present.  Used by `parse_def` and `parse_class`.
    fn parse_type_params(&mut self, ctx: &str) -> Result<Vec<TypeParam>> {
        if !self.is(&Token::LBracket) {
            return Ok(vec![]);
        }
        self.bump(); // consume `[`
        let mut params: Vec<TypeParam> = Vec::new();
        loop {
            match self.current() {
                None | Some(Token::Eof) => {
                    return Err(PyError::Parse(format!(
                        "unterminated type parameter list in {ctx}"
                    )));
                }
                Some(Token::RBracket) => {
                    self.bump(); // consume `]`
                    break;
                }
                Some(Token::Comma) => {
                    self.bump(); // consume `,`
                }
                Some(Token::Ident(_)) => {
                    let param_name = self.expect_ident("type parameter name")?;
                    if params.iter().any(|p| p.name == param_name) {
                        return Err(PyError::Parse(format!(
                            "duplicate type parameter '{param_name}'"
                        )));
                    }
                    // Optional bound/constraints: `: expr`.  `parse_expr` stops at
                    // the top-level `,` / `]`, so it captures exactly the bound
                    // expression.  A parenthesised tuple (`T: (int, str)`) parses
                    // to `Expr::Tuple` and is treated as constraints, matching
                    // CPython; any other expression is a single upper bound.  The
                    // empty tuple `T: ()` is the sole exception: CPython treats it
                    // as a (degenerate) bound value, leaving `__constraints__` ==
                    // `()` and `__bound__` == `()`, so we route it through `Bound`.
                    let bound = if self.is(&Token::Colon) {
                        self.bump(); // consume `:`
                        let expr = self.parse_expr()?;
                        Some(match expr {
                            Expr::Tuple(elems) if !elems.is_empty() => {
                                TypeParamBound::Constraints(elems)
                            }
                            other => TypeParamBound::Bound(other),
                        })
                    } else {
                        None
                    };
                    params.push(TypeParam {
                        name: param_name,
                        bound,
                    });
                }
                _ => {
                    // Skip unexpected token (e.g. `*Ts`, `**P` variance markers).
                    self.bump();
                }
            }
        }
        Ok(params)
    }

    fn parse_match(&mut self) -> Result<Stmt> {
        // Consume `match` (soft keyword, tokenised as Ident)
        self.bump();
        let subject = self.parse_expr()?;
        self.expect(&Token::Colon)?;
        // Parse the indented block of `case` arms.
        self.expect(&Token::Newline)?;
        // Skip blank / comment-only lines between the `match:` header and the
        // block Indent token (same pattern as parse_suite).
        self.skip_newlines();
        self.expect(&Token::Indent)?;
        let mut arms: Vec<MatchArm> = Vec::new();
        self.skip_newlines();
        while !self.is(&Token::Dedent) && !self.is(&Token::Eof) {
            // Each arm starts with `case`
            match self.current() {
                Some(Token::Ident(k)) if k == "case" => {
                    self.bump(); // consume `case`
                }
                other => {
                    return Err(PyError::Parse(format!(
                        "expected 'case' in match block, found {other:?}"
                    )));
                }
            }
            let pattern = self.parse_pattern()?;
            validate_pattern(&pattern)?;
            let guard = if self.is(&Token::If) {
                self.bump();
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(&Token::Colon)?;
            let (body, body_linenos) = self.parse_suite_with_linenos()?;
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                body_linenos,
            });
            self.skip_newlines();
        }
        self.expect(&Token::Dedent)?;
        if arms.is_empty() {
            return Err(PyError::Parse(
                "match statement has no case arms".to_string(),
            ));
        }
        Ok(Stmt::Match { subject, arms })
    }

    /// Parse a pattern for a `case` clause.
    /// Handles Or-patterns and As-patterns at the top level:
    /// `p1 | p2 | p3` and `pattern as name` (PEP 634 §7).
    fn parse_pattern(&mut self) -> Result<Pattern> {
        let first = self.parse_pattern_atom()?;
        let or_pat = if self.is(&Token::Pipe) {
            let mut alternatives = vec![first];
            while self.is(&Token::Pipe) {
                self.bump();
                alternatives.push(self.parse_pattern_atom()?);
            }
            Pattern::Or(alternatives)
        } else {
            first
        };
        // As-pattern: `or_pattern 'as' capture_name`
        if self.is(&Token::As) {
            self.bump();
            let name = self.expect_ident("capture name in 'as' pattern")?;
            Ok(Pattern::As {
                pattern: Box::new(or_pat),
                name,
            })
        } else {
            Ok(or_pat)
        }
    }

    /// Parse a single (non-Or) pattern atom.
    fn parse_pattern_atom(&mut self) -> Result<Pattern> {
        match self.current().cloned() {
            // Wildcard: `_`
            Some(Token::Ident(ref name)) if name == "_" => {
                self.bump();
                Ok(Pattern::Wildcard)
            }
            // Negative numeric literal: `-42`
            Some(Token::Minus) => {
                self.bump();
                match self.current().cloned() {
                    Some(Token::Int(n)) => {
                        self.bump();
                        Ok(Pattern::Literal(Expr::Unary {
                            op: crate::ast::UnaryOp::Neg,
                            expr: Box::new(Expr::Int(n)),
                            span: None,
                        }))
                    }
                    Some(Token::BigInt(s)) => {
                        self.bump();
                        Ok(Pattern::Literal(Expr::Unary {
                            op: crate::ast::UnaryOp::Neg,
                            expr: Box::new(Expr::BigInt(s)),
                            span: None,
                        }))
                    }
                    Some(Token::Float(f)) => {
                        self.bump();
                        Ok(Pattern::Literal(Expr::Unary {
                            op: crate::ast::UnaryOp::Neg,
                            expr: Box::new(Expr::Float(f)),
                            span: None,
                        }))
                    }
                    other => Err(PyError::Parse(format!(
                        "expected number after '-' in pattern, found {other:?}"
                    ))),
                }
            }
            // Literal: integer, float, string, True, False, None
            Some(Token::Int(n)) => {
                self.bump();
                Ok(Pattern::Literal(Expr::Int(n)))
            }
            Some(Token::BigInt(s)) => {
                self.bump();
                Ok(Pattern::Literal(Expr::BigInt(s)))
            }
            Some(Token::Float(f)) => {
                self.bump();
                Ok(Pattern::Literal(Expr::Float(f)))
            }
            Some(Token::Str(s)) => {
                self.bump();
                Ok(Pattern::Literal(Expr::Str(s)))
            }
            Some(Token::True) => {
                self.bump();
                Ok(Pattern::Literal(Expr::Bool(true)))
            }
            Some(Token::False) => {
                self.bump();
                Ok(Pattern::Literal(Expr::Bool(false)))
            }
            Some(Token::None) => {
                self.bump();
                Ok(Pattern::Literal(Expr::None))
            }
            // Name: capture pattern, value pattern (dotted), or class pattern.
            // Per PEP 634: a dotted name like `Color.RED` is a value pattern —
            // it is evaluated and compared with ==, not bound as a capture.
            Some(Token::Ident(name)) => {
                self.bump();
                if self.is(&Token::Dot) {
                    // Dotted name → value pattern.  Consume `.attr` chains to
                    // build an `Expr::Attr` chain, then check for a trailing `(`
                    // to decide class-pattern vs value-pattern.
                    let mut expr = Expr::Var(name, None);
                    while self.is(&Token::Dot) {
                        self.bump();
                        let attr = match self.current().cloned() {
                            Some(Token::Ident(attr)) => {
                                self.bump();
                                attr
                            }
                            other => {
                                return Err(PyError::Parse(format!(
                                    "expected attribute name after '.' in pattern, found {other:?}"
                                )));
                            }
                        };
                        expr = Expr::Attr {
                            target: Box::new(expr),
                            name: attr,
                            span: None,
                        };
                    }
                    if self.is(&Token::LParen) {
                        // Dotted class pattern: `module.Class(pos, ..., kwarg=pat, ...)`
                        self.bump();
                        let mut positional: Vec<Pattern> = Vec::new();
                        let mut kwargs: Vec<(String, Pattern)> = Vec::new();
                        while !self.is(&Token::RParen) && !self.is(&Token::Eof) {
                            // A keyword sub-pattern starts with `name =`; anything
                            // else is a positional sub-pattern.
                            let is_keyword = matches!(self.current(), Some(Token::Ident(_)))
                                && self.peek() == Some(&Token::Assign);
                            if is_keyword {
                                let attr = self.expect_ident("class pattern keyword")?;
                                self.expect(&Token::Assign)?;
                                let pat = self.parse_pattern()?;
                                kwargs.push((attr, pat));
                            } else {
                                if !kwargs.is_empty() {
                                    return Err(PyError::Parse(
                                        "positional patterns follow keyword patterns".into(),
                                    ));
                                }
                                positional.push(self.parse_pattern()?);
                            }
                            if self.is(&Token::Comma) {
                                self.bump();
                            } else {
                                break;
                            }
                        }
                        self.expect(&Token::RParen)?;
                        Ok(Pattern::Class {
                            cls: Box::new(expr),
                            positional,
                            kwargs,
                        })
                    } else {
                        Ok(Pattern::Value(expr))
                    }
                } else if self.is(&Token::LParen) {
                    // Class pattern: Name(pos, ..., kwarg=pat, ...)
                    self.bump();
                    let cls = Expr::Var(name, None);
                    let mut positional: Vec<Pattern> = Vec::new();
                    let mut kwargs: Vec<(String, Pattern)> = Vec::new();
                    while !self.is(&Token::RParen) && !self.is(&Token::Eof) {
                        // A keyword sub-pattern starts with `name =`; anything
                        // else is a positional sub-pattern.
                        let is_keyword = matches!(self.current(), Some(Token::Ident(_)))
                            && self.peek() == Some(&Token::Assign);
                        if is_keyword {
                            let attr = self.expect_ident("class pattern keyword")?;
                            self.expect(&Token::Assign)?;
                            let pat = self.parse_pattern()?;
                            kwargs.push((attr, pat));
                        } else {
                            if !kwargs.is_empty() {
                                return Err(PyError::Parse(
                                    "positional patterns follow keyword patterns".into(),
                                ));
                            }
                            positional.push(self.parse_pattern()?);
                        }
                        if self.is(&Token::Comma) {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Pattern::Class {
                        cls: Box::new(cls),
                        positional,
                        kwargs,
                    })
                } else {
                    Ok(Pattern::Capture(name))
                }
            }
            // Sequence pattern: `[p1, p2, *rest]`
            Some(Token::LBracket) => {
                self.bump();
                let mut elements: Vec<(Pattern, bool)> = Vec::new();
                while !self.is(&Token::RBracket) && !self.is(&Token::Eof) {
                    if self.is(&Token::Star) {
                        self.bump();
                        let name = self.expect_ident("star pattern name")?;
                        // `*_` is a non-binding wildcard star (PEP 634).
                        let pat = if name == "_" {
                            Pattern::Wildcard
                        } else {
                            Pattern::Capture(name)
                        };
                        elements.push((pat, true));
                    } else {
                        elements.push((self.parse_pattern()?, false));
                    }
                    if self.is(&Token::Comma) {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.expect(&Token::RBracket)?;
                Ok(Pattern::Sequence(elements))
            }
            // Mapping pattern: `{"key": pat, **rest}`
            Some(Token::LBrace) => {
                self.bump();
                let mut pairs: Vec<(Expr, Pattern)> = Vec::new();
                let mut rest: Option<String> = None;
                while !self.is(&Token::RBrace) && !self.is(&Token::Eof) {
                    if self.is(&Token::StarStar) {
                        self.bump();
                        let name = self.expect_ident("double-star rest pattern")?;
                        rest = Some(name);
                        if self.is(&Token::Comma) {
                            self.bump();
                        }
                        break;
                    }
                    let key = self.parse_expr()?;
                    self.expect(&Token::Colon)?;
                    let val_pat = self.parse_pattern()?;
                    pairs.push((key, val_pat));
                    if self.is(&Token::Comma) {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.expect(&Token::RBrace)?;
                Ok(Pattern::Mapping(pairs, rest))
            }
            // Parenthesised pattern: grouping `(p)` or sequence `(p1, p2)` / `(p,)` / `()`.
            // Per PEP 634: a trailing comma or multiple elements makes it a sequence pattern.
            Some(Token::LParen) => {
                self.bump();
                // Empty parens → empty sequence pattern (matches empty tuple/list).
                if self.is(&Token::RParen) {
                    self.bump();
                    return Ok(Pattern::Sequence(vec![]));
                }
                // Parse the first sub-pattern.
                let first = if self.is(&Token::Star) {
                    self.bump();
                    let name = self.expect_ident("star pattern name")?;
                    // `*_` is a non-binding wildcard star (PEP 634).
                    let pat = if name == "_" {
                        Pattern::Wildcard
                    } else {
                        Pattern::Capture(name)
                    };
                    (pat, true)
                } else {
                    (self.parse_pattern()?, false)
                };
                // If a comma follows, this is a sequence pattern.
                if self.is(&Token::Comma) {
                    let mut elements = vec![first];
                    while self.is(&Token::Comma) {
                        self.bump();
                        if self.is(&Token::RParen) || self.is(&Token::Eof) {
                            break;
                        }
                        if self.is(&Token::Star) {
                            self.bump();
                            let name = self.expect_ident("star pattern name")?;
                            // `*_` is a non-binding wildcard star (PEP 634).
                            let pat = if name == "_" {
                                Pattern::Wildcard
                            } else {
                                Pattern::Capture(name)
                            };
                            elements.push((pat, true));
                        } else {
                            elements.push((self.parse_pattern()?, false));
                        }
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Pattern::Sequence(elements))
                } else {
                    // No comma → plain grouping; return the inner pattern.
                    self.expect(&Token::RParen)?;
                    Ok(first.0)
                }
            }
            other => Err(PyError::Parse(format!(
                "unexpected token in pattern: {other:?}"
            ))),
        }
    }
}
