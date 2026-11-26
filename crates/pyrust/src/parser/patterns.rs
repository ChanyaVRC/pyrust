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

    /// Parse an optional PEP 695 type parameter list `[T, *Ts, **P, ...]`.
    /// Consumes `[` through the matching `]`; returns an empty `Vec` if no `[`
    /// is present.  Used by `parse_def`, `parse_class`, and `parse_type_alias`.
    fn parse_type_params(&mut self, ctx: &str) -> Result<Vec<TypeParam>> {
        if !self.is(&Token::LBracket) {
            return Ok(vec![]);
        }
        self.bump(); // consume `[`
        if self.is(&Token::RBracket) {
            return Err(PyError::parse(format!(
                "empty type parameter list in {ctx}"
            )));
        }
        let mut params: Vec<TypeParam> = Vec::new();
        loop {
            let (kind, param_name) = match self.current() {
                None | Some(Token::Eof) => {
                    return Err(PyError::parse(format!(
                        "unterminated type parameter list in {ctx}"
                    )));
                }
                Some(Token::StarStar) => {
                    self.bump(); // consume `**`
                    let name = self.expect_ident("ParamSpec name")?;
                    (TypeParamKind::ParamSpec, name)
                }
                Some(Token::Star) => {
                    self.bump(); // consume `*`
                    let name = self.expect_ident("TypeVarTuple name")?;
                    (TypeParamKind::TypeVarTuple, name)
                }
                Some(Token::Ident(_)) => {
                    let name = self.expect_ident("type parameter name")?;
                    (TypeParamKind::TypeVar, name)
                }
                other => {
                    return Err(PyError::parse(format!(
                        "invalid syntax in type parameter list in {ctx}: {other:?}"
                    )));
                }
            };

            if params.iter().any(|p| p.name == param_name) {
                return Err(PyError::parse(format!(
                    "duplicate type parameter '{param_name}'"
                )));
            }

            // Optional bound/constraints: `: expr`.  CPython 3.12 allows a bound
            // or constraints tuple ONLY on `TypeVar`, rejecting both on
            // `ParamSpec` and `TypeVarTuple` with specific SyntaxErrors.
            let bound = if self.is(&Token::Colon) {
                self.bump(); // consume `:`
                let expr = self.parse_expr()?;
                match kind {
                    TypeParamKind::TypeVar => Some(match expr {
                        // A parenthesised non-empty tuple (`T: (int, str)`) is
                        // constraints; the empty tuple `T: ()` is treated as a
                        // degenerate bound value matching CPython 3.12.
                        Expr::Tuple(elems) if !elems.is_empty() => {
                            TypeParamBound::Constraints(elems)
                        }
                        other => TypeParamBound::Bound(other),
                    }),
                    TypeParamKind::ParamSpec => {
                        let msg = if matches!(expr, Expr::Tuple(_)) {
                            "cannot use constraints with ParamSpec"
                        } else {
                            "cannot use bound with ParamSpec"
                        };
                        return Err(PyError::parse(msg.to_string()));
                    }
                    TypeParamKind::TypeVarTuple => {
                        let msg = if matches!(expr, Expr::Tuple(_)) {
                            "cannot use constraints with TypeVarTuple"
                        } else {
                            "cannot use bound with TypeVarTuple"
                        };
                        return Err(PyError::parse(msg.to_string()));
                    }
                }
            } else {
                None
            };

            params.push(TypeParam {
                name: param_name,
                kind,
                bound,
            });

            match self.current() {
                Some(Token::Comma) => {
                    self.bump(); // consume `,`
                    if self.is(&Token::RBracket) {
                        self.bump(); // allow trailing comma
                        break;
                    }
                }
                Some(Token::RBracket) => {
                    self.bump(); // consume `]`
                    break;
                }
                None | Some(Token::Eof) => {
                    return Err(PyError::parse(format!(
                        "unterminated type parameter list in {ctx}"
                    )));
                }
                other => {
                    return Err(PyError::parse(format!(
                        "expected ',' or ']' in type parameter list in {ctx}, found {other:?}"
                    )));
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
                    return Err(PyError::parse(format!(
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
            if !self.is(&Token::Colon) {
                // CPython only reports `expected ':'` when the case header ends
                // at the newline; any other stray token is `invalid syntax`.
                return Err(PyError::parse(
                    if self.is(&Token::Newline) {
                        "expected ':'"
                    } else {
                        "invalid syntax"
                    }
                    .to_string(),
                ));
            }
            self.bump();
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
            return Err(PyError::parse(
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

    /// Like `expect`, but a stray token inside a pattern is CPython's plain
    /// `invalid syntax` rather than an `expected <Token>` message.
    fn expect_in_pattern(&mut self, token: &Token) -> Result<()> {
        if self.is(token) {
            self.bump();
            Ok(())
        } else if self.is(&Token::Eof) {
            self.expect(token)
        } else {
            Err(PyError::parse("invalid syntax".to_string()))
        }
    }

    /// PEP 634 `signed_number` / `complex_number`:
    /// `['-'] NUMBER [('+'|'-') imaginary NUMBER]`.  The left operand of a
    /// complex literal must be real and the right one imaginary, with the same
    /// `SyntaxError`s as CPython's `ensure_real` / `ensure_imaginary`.
    fn parse_number_pattern_expr(&mut self) -> Result<Expr> {
        let negative = self.is(&Token::Minus);
        if negative {
            self.bump();
        }
        let (num, left_imaginary) = match self.current().cloned() {
            Some(Token::Int(n)) => (Expr::Int(n), false),
            Some(Token::BigInt(s)) => (Expr::BigInt(s), false),
            Some(Token::Float(f)) => (Expr::Float(f), false),
            Some(Token::Imag(v)) => (Expr::Complex(0.0, v), true),
            _ => return Err(PyError::parse("invalid syntax".to_string())),
        };
        self.bump();
        let left = if negative {
            Expr::Unary {
                op: crate::ast::UnaryOp::Neg,
                expr: Box::new(num),
                span: None,
            }
        } else {
            num
        };
        let op = match self.current() {
            Some(Token::Plus) => crate::ast::BinaryOp::Add,
            Some(Token::Minus) => crate::ast::BinaryOp::Sub,
            _ => return Ok(left),
        };
        if left_imaginary {
            return Err(PyError::parse(
                "real number required in complex literal".to_string(),
            ));
        }
        self.bump();
        match self.current().cloned() {
            Some(Token::Imag(v)) => {
                self.bump();
                Ok(Expr::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(Expr::Complex(0.0, v)),
                    span: None,
                })
            }
            Some(Token::Int(_)) | Some(Token::BigInt(_)) | Some(Token::Float(_)) => Err(
                PyError::parse("imaginary number required in complex literal".to_string()),
            ),
            _ => Err(PyError::parse("invalid syntax".to_string())),
        }
    }

    /// A run of adjacent string or bytes literals as a pattern value.  Mixing
    /// `str` and `bytes` is rejected by the shared concatenation logic; an
    /// f-string is rejected with `fstring_msg`.
    fn parse_string_pattern_expr(&mut self, fstring_msg: &str) -> Result<Expr> {
        let expr = self.parse_primary()?;
        if matches!(expr, Expr::FString(_)) {
            return Err(PyError::parse(fstring_msg.to_string()));
        }
        Ok(expr)
    }

    /// A mapping-pattern key: a literal or a dotted attribute lookup.
    fn parse_mapping_pattern_key(&mut self) -> Result<Expr> {
        const MSG: &str = "mapping pattern keys may only match literals and attribute lookups";
        match self.current().cloned() {
            Some(Token::Minus)
            | Some(Token::Int(_))
            | Some(Token::BigInt(_))
            | Some(Token::Float(_))
            | Some(Token::Imag(_)) => self.parse_number_pattern_expr(),
            Some(Token::Str(_)) | Some(Token::Bytes(_)) | Some(Token::FString(_)) => {
                self.parse_string_pattern_expr(MSG)
            }
            Some(Token::True) => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            Some(Token::False) => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            Some(Token::None) => {
                self.bump();
                Ok(Expr::None)
            }
            Some(Token::Ident(name)) => {
                self.bump();
                if !self.is(&Token::Dot) {
                    return Err(PyError::parse("invalid syntax".to_string()));
                }
                let mut expr = Expr::Var(name, None);
                while self.is(&Token::Dot) {
                    self.bump();
                    let attr = self.expect_ident("attribute name after '.' in pattern")?;
                    expr = Expr::Attr {
                        target: Box::new(expr),
                        name: attr,
                        span: None,
                    };
                }
                Ok(expr)
            }
            _ => Err(PyError::parse("invalid syntax".to_string())),
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
            // Numeric literals: signed numbers and complex literals
            // (`-42`, `1.5`, `1+2j`, `-1-2j`, `3j`).
            Some(Token::Minus)
            | Some(Token::Int(_))
            | Some(Token::BigInt(_))
            | Some(Token::Float(_))
            | Some(Token::Imag(_)) => Ok(Pattern::Literal(self.parse_number_pattern_expr()?)),
            // Strings / bytes: adjacent literals concatenate (`"a" "b"`,
            // `b"x" b"y"`); f-strings are not valid patterns.
            Some(Token::Str(_)) | Some(Token::Bytes(_)) | Some(Token::FString(_)) => {
                Ok(Pattern::Literal(self.parse_string_pattern_expr(
                    "patterns may only match literals and attribute lookups",
                )?))
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
                                return Err(PyError::parse(format!(
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
                        let mut kwargs: Vec<(pyrust_core::Symbol, Pattern)> = Vec::new();
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
                                    return Err(PyError::parse(
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
                        self.expect_in_pattern(&Token::RParen)?;
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
                    let mut kwargs: Vec<(pyrust_core::Symbol, Pattern)> = Vec::new();
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
                                return Err(PyError::parse(
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
                    self.expect_in_pattern(&Token::RParen)?;
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
                self.expect_in_pattern(&Token::RBracket)?;
                Ok(Pattern::Sequence(elements))
            }
            // Mapping pattern: `{"key": pat, **rest}`
            Some(Token::LBrace) => {
                self.bump();
                let mut pairs: Vec<(Expr, Pattern)> = Vec::new();
                let mut rest: Option<pyrust_core::Symbol> = None;
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
                    let key = self.parse_mapping_pattern_key()?;
                    self.expect_in_pattern(&Token::Colon)?;
                    let val_pat = self.parse_pattern()?;
                    pairs.push((key, val_pat));
                    if self.is(&Token::Comma) {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.expect_in_pattern(&Token::RBrace)?;
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
                    self.expect_in_pattern(&Token::RParen)?;
                    Ok(Pattern::Sequence(elements))
                } else {
                    // No comma → plain grouping; return the inner pattern.
                    self.expect_in_pattern(&Token::RParen)?;
                    Ok(first.0)
                }
            }
            _ => Err(PyError::parse("invalid syntax".to_string())),
        }
    }
}
