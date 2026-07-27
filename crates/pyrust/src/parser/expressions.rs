impl Parser {
    // ── Expressions (precedence: low → high) ───────────────────────────────
    //
    // parse_expr      (ternary: X if C else Y, lambda)
    // parse_or
    // parse_and
    // parse_not
    // parse_comparison  (==, !=, <, <=, >, >=, in, not in, is, is not)
    // parse_bitor     (|)
    // parse_bitxor    (^)
    // parse_bitand    (&)
    // parse_shift     (<<, >>)
    // parse_term      (+, -)
    // parse_factor    (*, /, //, %)
    // parse_unary     (-, +, ~)
    // parse_power     (**  right-assoc)
    // parse_postfix   (call, index, attr)
    // parse_primary

    /// Parse one expression.  If followed by a comma (and we are not at a
    /// statement boundary), collect additional comma-separated expressions and
    /// wrap the whole list in `Expr::Tuple`.  A trailing comma is allowed and
    /// also produces a tuple (single-element when only one item precedes it).
    /// This matches CPython's grammar for `return` and `yield` value positions.
    fn parse_expr_or_tuple(&mut self) -> Result<Expr> {
        let first = self.parse_seq_item()?;
        if !self.is(&Token::Comma) {
            return Ok(first);
        }
        // At least one comma seen — collect items into a tuple.
        let mut items = vec![first];
        while self.is(&Token::Comma) {
            self.bump(); // consume comma
            if self.at_stmt_end() {
                break; // trailing comma — stop here, still produce a tuple
            }
            items.push(self.parse_seq_item()?);
        }
        Ok(Expr::Tuple(items))
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        // Depth guard (#2009): every bracketed construct re-enters `parse_expr`
        // for its contents, so bounding the entry depth bounds the total
        // recursive-descent nesting.  Exceeding the limit raises a catchable
        // `SyntaxError` rather than overflowing the native Rust stack (SIGABRT),
        // matching CPython's behaviour for deeply nested literals/expressions.
        self.expr_depth += 1;
        if self.expr_depth > MAX_EXPR_DEPTH {
            self.expr_depth -= 1;
            return Err(PyError::Parse("too many nested parentheses".to_string()));
        }
        let result = self.parse_expr_inner();
        self.expr_depth -= 1;
        result
    }

    fn parse_expr_inner(&mut self) -> Result<Expr> {
        // Yield / yield from
        if self.is(&Token::Yield) {
            self.bump(); // consume `yield`
            if self.is(&Token::From) {
                self.bump(); // consume `from`
                let iter = self.parse_expr()?;
                return Ok(Expr::YieldFrom(Box::new(iter)));
            }
            // bare `yield` or `yield expr`
            if self.at_stmt_end() {
                return Ok(Expr::Yield(None));
            }
            let val = self.parse_expr_or_tuple()?;
            if matches!(val, Expr::Starred(_)) {
                return Err(PyError::Parse(
                    "can't use starred expression here".to_string(),
                ));
            }
            return Ok(Expr::Yield(Some(Box::new(val))));
        }
        // Lambda
        if self.is(&Token::Lambda) {
            return self.parse_lambda();
        }
        let expr = self.parse_or()?;
        // Walrus operator: NAME := expr
        if self.is(&Token::Walrus) {
            if let Expr::Var(name, _) = expr {
                if name == "__debug__" {
                    return Err(PyError::Parse("cannot assign to __debug__".to_string()));
                }
                self.bump(); // consume :=
                let value = self.parse_expr()?;
                return Ok(Expr::Named {
                    target: name,
                    value: Box::new(value),
                });
            } else {
                return Err(PyError::Parse(
                    "walrus operator ':=' requires a name on the left-hand side".to_string(),
                ));
            }
        }
        // Ternary: expr if cond else other
        if self.is(&Token::If) {
            self.bump();
            let cond = self.parse_or()?;
            self.expect(&Token::Else)?;
            let else_ = self.parse_expr()?;
            return Ok(Expr::Ternary {
                cond: Box::new(cond),
                then: Box::new(expr),
                else_: Box::new(else_),
            });
        }
        Ok(expr)
    }

    /// Parse lambda parameter list, terminated by `:` instead of `)`.
    /// Lambda params cannot have type annotations (CPython 3.12).
    fn parse_lambda_params(&mut self) -> Result<Vec<FunctionParam>> {
        let mut params = Vec::new();
        let mut seen_default = false;
        let mut seen_star = false;
        let mut seen_kwargs = false;
        // Set when a bare `*` separator is consumed; cleared when the first
        // keyword-only parameter is added.  If still set when `:` is reached,
        // CPython raises SyntaxError: named arguments must follow bare *.
        let mut bare_star_needs_kwonly = false;

        loop {
            if seen_kwargs {
                return Err(PyError::Parse("parameter after **kwargs".to_string()));
            }

            if self.is(&Token::StarStar) {
                self.bump();
                let name = self.expect_ident("kwargs parameter name")?;
                params.push(FunctionParam {
                    name,
                    default: None,
                    annotation: None,
                    is_args: false,
                    is_kwargs: true,
                    is_keyword_only: false,
                    is_positional_only: false,
                });
                seen_kwargs = true;
            } else if self.is(&Token::Star) {
                if seen_star {
                    return Err(PyError::Parse(
                        "* argument may appear only once".to_string(),
                    ));
                }
                self.bump();
                seen_star = true;
                if self.is(&Token::Comma) || self.is(&Token::Colon) {
                    // bare * separator: keyword-only params must follow
                    bare_star_needs_kwonly = true;
                } else {
                    let name = self.expect_ident("args parameter name")?;
                    params.push(FunctionParam {
                        name,
                        default: None,
                        annotation: None,
                        is_args: true,
                        is_kwargs: false,
                        is_keyword_only: false,
                        is_positional_only: false,
                    });
                }
            } else {
                match self.current().cloned() {
                    Some(Token::Ident(name)) => {
                        self.bump();
                        let default = if self.is(&Token::Assign) {
                            self.bump();
                            seen_default = true;
                            Some(self.parse_expr()?)
                        } else {
                            if seen_default && !seen_star {
                                return Err(PyError::Parse(
                                    "non-default argument follows default argument".to_string(),
                                ));
                            }
                            None
                        };
                        if seen_star {
                            bare_star_needs_kwonly = false;
                        }
                        params.push(FunctionParam {
                            name,
                            default,
                            annotation: None,
                            is_args: false,
                            is_kwargs: false,
                            is_keyword_only: seen_star,
                            is_positional_only: false,
                        });
                    }
                    other => {
                        return Err(PyError::Parse(format!(
                            "expected lambda parameter, found {other:?}"
                        )));
                    }
                }
            }

            if self.is(&Token::Colon) {
                if bare_star_needs_kwonly {
                    return Err(PyError::Parse(
                        "named arguments must follow bare *".to_string(),
                    ));
                }
                break;
            }
            self.expect(&Token::Comma)?;
            if self.is(&Token::Colon) {
                if bare_star_needs_kwonly {
                    return Err(PyError::Parse(
                        "named arguments must follow bare *".to_string(),
                    ));
                }
                break;
            }
        }

        check_duplicate_params(&params)?;
        Ok(params)
    }

    fn parse_lambda(&mut self) -> Result<Expr> {
        self.expect(&Token::Lambda)?;
        let params = if self.is(&Token::Colon) {
            Vec::new()
        } else {
            self.parse_lambda_params()?
        };
        self.expect(&Token::Colon)?;
        let body = self.parse_expr()?;
        Ok(Expr::Lambda {
            params,
            body: Box::new(body),
        })
    }

    /// Build the PEP 657 caret anchor for a binary expression `left OP right`
    /// (issue #2411): operands underlined with `~`, the operator with `^`.
    ///
    /// `left_start` is the start col of the whole left operand (captured before
    /// it was parsed); `left_lineno` is its 1-based source line; `op_col` /
    /// `op_end` bracket the operator token; the right operand's end col is read
    /// from the just-consumed token (`prev_end_col`).  Returns `None` (caret
    /// suppressed) if any piece is missing or the columns are inconsistent —
    /// never a wrong caret.
    ///
    /// ## Multi-line operands (issue #2571)
    ///
    /// When the expression spans more than one physical line (the left operand
    /// and the right operand's end token sit on different lines), the per-line
    /// columns of `op` / `full_end` belong to a *later* line than the displayed
    /// source line (which is the left operand's line).  Mixing them with the
    /// left's column produces a nonsensical single-line span, which the
    /// formatter then drops (no caret).  CPython 3.12 instead underlines from
    /// the expression start to the end of the *first* displayed line, all `^`.
    /// We signal that case with the `MULTILINE_FULL_END` sentinel so the
    /// formatter clamps to the displayed line and draws solid carets.
    fn make_binary_span(
        &self,
        left_start: Option<u32>,
        left_lineno: u32,
        op_col: Option<u32>,
        op_end: Option<u32>,
    ) -> Option<crate::ast::CaretSpan> {
        let full_start = left_start?;
        let prim_start = op_col?;
        let prim_end = op_end?;
        let full_end = self.prev_end_col()?;
        // Multi-line: the displayed line is the left operand's line, but the
        // operator / right-operand columns are measured on a later line, so the
        // single-line span is meaningless.  Emit the clamp sentinel (#2571).
        let end_lineno = self.prev_lineno();
        if left_lineno != 0 && end_lineno != 0 && left_lineno != end_lineno {
            return Some((
                full_start,
                full_start,
                crate::ast::MULTILINE_FULL_END,
                crate::ast::MULTILINE_FULL_END,
            ));
        }
        if full_start <= prim_start && prim_start < prim_end && prim_end <= full_end {
            Some((full_start, prim_start, prim_end, full_end))
        } else {
            None
        }
    }
}
