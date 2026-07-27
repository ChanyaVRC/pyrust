impl Parser {
    fn parse_or(&mut self) -> Result<Expr> {
        let mut expr = self.parse_and()?;
        while self.is(&Token::Or) {
            self.bump();
            let right = self.parse_and()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::Or,
                right: Box::new(right),
                // Short-circuit ops don't raise from a single operator
                // instruction; leave caret-free (issue #2411).
                span: None,
            };
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut expr = self.parse_not()?;
        while self.is(&Token::And) {
            self.bump();
            let right = self.parse_not()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::And,
                right: Box::new(right),
                span: None,
            };
        }
        Ok(expr)
    }

    fn parse_not(&mut self) -> Result<Expr> {
        if self.is(&Token::Not) {
            let op_start = self.current_col();
            self.bump();
            let expr = self.parse_not()?;
            // CPython 3.12 anchors the whole `not operand` span with `^`
            // (full == prim), the same shape as arithmetic unary, when the
            // operand's `__bool__` raises (#2582).
            let span = self.make_unary_span(op_start);
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
                span,
            });
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr> {
        let left = self.parse_bitor()?;
        let mut ops: Vec<(CmpOp, Expr)> = Vec::new();

        loop {
            let op = match self.current() {
                Some(Token::Eq) => {
                    self.bump();
                    CmpOp::Eq
                }
                Some(Token::Ne) => {
                    self.bump();
                    CmpOp::Ne
                }
                Some(Token::Lt) => {
                    self.bump();
                    CmpOp::Lt
                }
                Some(Token::Le) => {
                    self.bump();
                    CmpOp::Le
                }
                Some(Token::Gt) => {
                    self.bump();
                    CmpOp::Gt
                }
                Some(Token::Ge) => {
                    self.bump();
                    CmpOp::Ge
                }
                Some(Token::In) => {
                    self.bump();
                    CmpOp::In
                }
                Some(Token::Not) if self.peek() == Some(&Token::In) => {
                    self.bump();
                    self.bump();
                    CmpOp::NotIn
                }
                Some(Token::Is) => {
                    self.bump();
                    if self.is(&Token::Not) {
                        self.bump();
                        CmpOp::IsNot
                    } else {
                        CmpOp::Is
                    }
                }
                _ => break,
            };
            ops.push((op, self.parse_bitor()?));
        }

        if ops.is_empty() {
            return Ok(left);
        }
        // Single comparison: desugar to Binary for simplicity
        if ops.len() == 1 {
            let (op, right) = ops.remove(0);
            return Ok(Expr::Binary {
                left: Box::new(left),
                op: op.into(),
                right: Box::new(right),
                // Comparison operators can be multi-token (`not in`, `is not`);
                // leave caret-free rather than risk a wrong span (issue #2411).
                span: None,
            });
        }
        Ok(Expr::Compare {
            left: Box::new(left),
            ops,
        })
    }

    fn parse_bitor(&mut self) -> Result<Expr> {
        let left_start = self.current_col();
        let left_lineno = self.current_lineno();
        let mut expr = self.parse_bitxor()?;
        while self.is(&Token::Pipe) {
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let right = self.parse_bitxor()?;
            let span = self.make_binary_span(left_start, left_lineno, op_col, op_end);
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::BitOr,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_bitxor(&mut self) -> Result<Expr> {
        let left_start = self.current_col();
        let left_lineno = self.current_lineno();
        let mut expr = self.parse_bitand()?;
        while self.is(&Token::Caret) {
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let right = self.parse_bitand()?;
            let span = self.make_binary_span(left_start, left_lineno, op_col, op_end);
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::BitXor,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_bitand(&mut self) -> Result<Expr> {
        let left_start = self.current_col();
        let left_lineno = self.current_lineno();
        let mut expr = self.parse_shift()?;
        while self.is(&Token::Ampersand) {
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let right = self.parse_shift()?;
            let span = self.make_binary_span(left_start, left_lineno, op_col, op_end);
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::BitAnd,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_shift(&mut self) -> Result<Expr> {
        let left_start = self.current_col();
        let left_lineno = self.current_lineno();
        let mut expr = self.parse_term()?;
        loop {
            let op = if self.is(&Token::LShift) {
                BinaryOp::LShift
            } else if self.is(&Token::RShift) {
                BinaryOp::RShift
            } else {
                break;
            };
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let right = self.parse_term()?;
            let span = self.make_binary_span(left_start, left_lineno, op_col, op_end);
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_term(&mut self) -> Result<Expr> {
        let left_start = self.current_col();
        let left_lineno = self.current_lineno();
        let mut expr = self.parse_factor()?;
        loop {
            let op = if self.is(&Token::Plus) {
                BinaryOp::Add
            } else if self.is(&Token::Minus) {
                BinaryOp::Sub
            } else {
                break;
            };
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let right = self.parse_factor()?;
            let span = self.make_binary_span(left_start, left_lineno, op_col, op_end);
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_factor(&mut self) -> Result<Expr> {
        let left_start = self.current_col();
        let left_lineno = self.current_lineno();
        let mut expr = self.parse_unary()?;
        loop {
            let op = if self.is(&Token::Star) {
                BinaryOp::Mul
            } else if self.is(&Token::At) {
                BinaryOp::MatMul
            } else if self.is(&Token::Slash) {
                BinaryOp::Div
            } else if self.is(&Token::SlashSlash) {
                BinaryOp::FloorDiv
            } else if self.is(&Token::Percent) {
                BinaryOp::Mod
            } else {
                break;
            };
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let right = self.parse_unary()?;
            let span = self.make_binary_span(left_start, left_lineno, op_col, op_end);
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    /// Build the PEP 657 caret anchor for an arithmetic unary expression
    /// `OP operand` (issue #2582): the whole `OP operand` span is underlined
    /// with `^` (full == prim), from the operator's start column through the end
    /// of the operand.  `op_start` is captured before the operator is bumped;
    /// the operand's end col is read from the just-consumed token
    /// (`prev_end_col`).  Returns `None` if either column is missing or
    /// inconsistent — never a wrong caret.
    fn make_unary_span(&self, op_start: Option<u32>) -> Option<crate::ast::CaretSpan> {
        let start = op_start?;
        let end = self.prev_end_col()?;
        if start < end {
            Some((start, start, end, end))
        } else {
            None
        }
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if self.is(&Token::Minus) {
            let op_start = self.current_col();
            self.bump();
            let expr = Box::new(self.parse_unary()?);
            let span = self.make_unary_span(op_start);
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr,
                span,
            });
        }
        if self.is(&Token::Tilde) {
            let op_start = self.current_col();
            self.bump();
            let expr = Box::new(self.parse_unary()?);
            let span = self.make_unary_span(op_start);
            return Ok(Expr::Unary {
                op: UnaryOp::BitNot,
                expr,
                span,
            });
        }
        if self.is(&Token::Plus) {
            let op_start = self.current_col();
            self.bump();
            let expr = Box::new(self.parse_unary()?);
            let span = self.make_unary_span(op_start);
            return Ok(Expr::Unary {
                op: UnaryOp::Pos,
                expr,
                span,
            });
        }
        // `await expr` — soft keyword, only meaningful inside `async def`; the
        // compiler will reject it outside async context.
        if matches!(self.current(), Some(Token::Ident(kw)) if kw == "await") {
            self.bump();
            return Ok(Expr::Await(Box::new(self.parse_unary()?)));
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<Expr> {
        let left_start = self.current_col();
        let left_lineno = self.current_lineno();
        let base = self.parse_postfix()?;
        if self.is(&Token::StarStar) {
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let exp = self.parse_unary()?; // right-associative
            let span = self.make_binary_span(left_start, left_lineno, op_col, op_end);
            return Ok(Expr::Binary {
                left: Box::new(base),
                op: BinaryOp::Pow,
                right: Box::new(exp),
                span,
            });
        }
        Ok(base)
    }
}
