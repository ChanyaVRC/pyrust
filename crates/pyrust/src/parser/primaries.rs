impl Parser {
    fn parse_primary(&mut self) -> Result<Expr> {
        match self.current().cloned() {
            Some(Token::Int(v)) => {
                self.bump();
                Ok(Expr::Int(v))
            }
            Some(Token::BigInt(s)) => {
                self.bump();
                Ok(Expr::BigInt(s))
            }
            Some(Token::Float(v)) => {
                self.bump();
                Ok(Expr::Float(v))
            }
            Some(Token::Imag(v)) => {
                self.bump();
                Ok(Expr::Complex(0.0, v))
            }
            Some(Token::Str(v)) => {
                self.bump();
                // Adjacent string literal concatenation (CPython 3.12 semantics).
                // If any adjacent token is an FString the whole result is an FString;
                // a Bytes literal mixed with str/fstring is a SyntaxError.
                let mut plain = v;
                let mut fstring_parts: Option<Vec<FStringPart>> = None;
                loop {
                    match self.current().cloned() {
                        Some(Token::Str(next)) => {
                            self.bump();
                            match fstring_parts {
                                None => plain.push_str(&next),
                                Some(ref mut parts) => {
                                    // Append to the last Literal part, or push a new one.
                                    match parts.last_mut() {
                                        Some(FStringPart::Literal(s)) => s.push_str(&next),
                                        _ => parts.push(FStringPart::Literal(next)),
                                    }
                                }
                            }
                        }
                        Some(Token::FString(lex_parts)) => {
                            let token_line = self.current_lineno();
                            self.bump();
                            // Promote: flush accumulated plain text as a Literal part.
                            let parts = fstring_parts.get_or_insert_with(Vec::new);
                            if !plain.is_empty() {
                                match parts.last_mut() {
                                    Some(FStringPart::Literal(s)) => s.push_str(&plain),
                                    _ => parts.push(FStringPart::Literal(plain.clone())),
                                }
                                plain.clear();
                            }
                            parts.extend(self.parse_fstring_parts(lex_parts, token_line)?);
                        }
                        Some(Token::Bytes(_)) => {
                            return Err(PyError::Parse(
                                "cannot mix bytes and nonbytes literals".to_string(),
                            ));
                        }
                        _ => break,
                    }
                }
                match fstring_parts {
                    None => Ok(Expr::Str(plain)),
                    Some(mut parts) => {
                        // Any remaining plain text (if the last token was a Str, not FString)
                        // has already been folded into `parts` inside the loop.
                        // But if fstring_parts was just promoted and plain was already flushed,
                        // this is always empty. Guard for safety.
                        if !plain.is_empty() {
                            match parts.last_mut() {
                                Some(FStringPart::Literal(s)) => s.push_str(&plain),
                                _ => parts.push(FStringPart::Literal(plain)),
                            }
                        }
                        Ok(Expr::FString(parts))
                    }
                }
            }
            Some(Token::Bytes(v)) => {
                self.bump();
                let mut bs = v;
                loop {
                    match self.current().cloned() {
                        Some(Token::Bytes(next)) => {
                            self.bump();
                            bs.extend_from_slice(&next);
                        }
                        Some(Token::Str(_)) | Some(Token::FString(_)) => {
                            return Err(PyError::Parse(
                                "cannot mix bytes and nonbytes literals".to_string(),
                            ));
                        }
                        _ => break,
                    }
                }
                Ok(Expr::Bytes(bs))
            }
            Some(Token::FString(lex_parts)) => {
                let token_line = self.current_lineno();
                self.bump();
                let mut parts = self.parse_fstring_parts(lex_parts, token_line)?;
                // Adjacent string/f-string literal concatenation.
                loop {
                    match self.current().cloned() {
                        Some(Token::FString(next_lex)) => {
                            let next_line = self.current_lineno();
                            self.bump();
                            parts.extend(self.parse_fstring_parts(next_lex, next_line)?);
                        }
                        Some(Token::Str(next)) => {
                            self.bump();
                            match parts.last_mut() {
                                Some(FStringPart::Literal(s)) => s.push_str(&next),
                                _ => parts.push(FStringPart::Literal(next)),
                            }
                        }
                        Some(Token::Bytes(_)) => {
                            return Err(PyError::Parse(
                                "cannot mix bytes and nonbytes literals".to_string(),
                            ));
                        }
                        _ => break,
                    }
                }
                Ok(Expr::FString(parts))
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
            Some(Token::Ellipsis) => {
                self.bump();
                Ok(Expr::Ellipsis)
            }
            Some(Token::Ident(name)) => {
                // PEP 657 caret anchor (#2426): the name's column span is
                // `[col, col + len)` where `col` is the Ident token's start
                // column and `len` its char length.  Also record the name's own
                // 1-based line (#2632) so the compiler can stamp the load with
                // the line the name is on — not the enclosing statement's first
                // line, which is wrong for a name on a continuation line of a
                // multi-line expression.  `self.pos` still points at the Ident
                // here (before `bump`).
                let lineno = self.current_lineno();
                let span = self
                    .current_col()
                    .map(|col| (col, col + name.chars().count() as u32, lineno));
                self.bump();
                Ok(Expr::Var(name, span))
            }
            Some(Token::LParen) => {
                self.bump();
                if self.is(&Token::RParen) {
                    self.bump();
                    return Ok(Expr::Tuple(vec![]));
                }
                // PEP 448 tuple splat: `(*a, ...)`.  A leading `*` unambiguously
                // commits to a tuple literal (it cannot be a parenthesised
                // expression because `*expr` is not a valid expression on its
                // own outside of a collection / call / assign-target context).
                // We require at least one comma — `(*a)` without a trailing
                // comma is a SyntaxError in CPython, matching the rule that
                // parenthesised expressions do not become tuples without a
                // comma.
                if self.is(&Token::Star) {
                    let first = self.parse_seq_item()?;
                    if !self.is(&Token::Comma) {
                        return Err(PyError::Parse(
                            "cannot use starred expression here".to_string(),
                        ));
                    }
                    let mut items = vec![first];
                    while self.is(&Token::Comma) {
                        self.bump();
                        if self.is(&Token::RParen) {
                            break;
                        }
                        items.push(self.parse_seq_item()?);
                    }
                    self.expect(&Token::RParen)?;
                    return Ok(Expr::Tuple(items));
                }
                let first = self.parse_expr()?;
                if self.is(&Token::For) || self.is_async_for() {
                    // Generator expression: (elt for target in iter ...) or async variant
                    let clauses = self.parse_comp_clauses()?;
                    self.expect(&Token::RParen)?;
                    return Ok(Expr::GenExp {
                        elt: Box::new(first),
                        clauses,
                    });
                }
                if self.is(&Token::Comma) {
                    // Tuple
                    let mut items = vec![first];
                    while self.is(&Token::Comma) {
                        self.bump();
                        if self.is(&Token::RParen) {
                            break;
                        }
                        items.push(self.parse_seq_item()?);
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Expr::Tuple(items))
                } else {
                    self.expect(&Token::RParen)?;
                    Ok(first)
                }
            }
            Some(Token::LBracket) => self.parse_list_literal(),
            Some(Token::LBrace) => self.parse_dict_or_set_literal(),
            other => Err(PyError::Parse(format!(
                "unexpected token in expression: {other:?}"
            ))),
        }
    }

    fn parse_list_literal(&mut self) -> Result<Expr> {
        self.expect(&Token::LBracket)?;
        if self.is(&Token::RBracket) {
            self.bump();
            return Ok(Expr::List(vec![]));
        }
        let first = self.parse_seq_item()?;
        // Detect list comprehension: [expr for ...] or [expr async for ...]
        // Comprehensions cannot start with `*expr` (PEP 448 syntax restriction).
        if self.is(&Token::For) || self.is_async_for() {
            if let Expr::Starred(_) = &first {
                return Err(PyError::Parse(
                    "iterable unpacking cannot be used in comprehension".to_string(),
                ));
            }
            let clauses = self.parse_comp_clauses()?;
            self.expect(&Token::RBracket)?;
            return Ok(Expr::ListComp {
                elt: Box::new(first),
                clauses,
            });
        }
        let mut items = vec![first];
        while self.is(&Token::Comma) {
            self.bump();
            if self.is(&Token::RBracket) {
                break;
            }
            items.push(self.parse_seq_item()?);
        }
        self.expect(&Token::RBracket)?;
        Ok(Expr::List(items))
    }

    /// Parse one element of a list / set / tuple literal: either an ordinary
    /// expression or a PEP 448 `*expr` splat.  The splatted expression is
    /// parsed at `or` precedence (same as call-site `*expr`) so that
    /// `[*a + b]` parses as `[*(a + b)]` would be ambiguous — Python uses the
    /// tighter binding here as well.
    fn parse_seq_item(&mut self) -> Result<Expr> {
        if self.is(&Token::Star) {
            self.bump();
            let inner = self.parse_or()?;
            Ok(Expr::Starred(Box::new(inner)))
        } else {
            self.parse_expr()
        }
    }

    /// Returns `true` if the current token is the soft keyword `async` and the
    /// next token is `for`.  Used to detect `async for` comprehension clauses.
    fn is_async_for(&self) -> bool {
        matches!(self.current(), Some(Token::Ident(kw)) if kw == "async")
            && matches!(self.peek(), Some(Token::For))
    }

    /// Parse one or more comprehension clauses: `for target in iter (if cond)? ...`
    /// Also handles `async for target in iter ...` (PEP 530).
    fn parse_comp_clauses(&mut self) -> Result<Vec<CompClause>> {
        let mut clauses = Vec::new();
        while self.is(&Token::For) || self.is_async_for() {
            let is_async = if self.is_async_for() {
                self.bump(); // consume `async`
                true
            } else {
                false
            };
            self.bump(); // consume `for`
            // Parse the loop target with the same parser the `for` statement uses,
            // so comprehensions accept parenthesized / nested / starred targets.
            let target = self.parse_for_target()?;
            self.expect(&Token::In)?;
            let iter = self.parse_or()?; // parse iterable (no comma-list at this level)
            // Consume any number of chained `if` filters (`for ... if A if B`).
            // They AND together with short-circuit semantics, so fold them into a
            // left-associated `and` chain matching CPython's `comp_if` grammar.
            let mut cond: Option<Expr> = None;
            while self.is(&Token::If) {
                self.bump();
                let filter = self.parse_or()?;
                cond = Some(match cond {
                    None => filter,
                    Some(prev) => Expr::Binary {
                        left: Box::new(prev),
                        op: BinaryOp::And,
                        right: Box::new(filter),
                        span: None,
                    },
                });
            }
            clauses.push(CompClause {
                target,
                iter,
                cond,
                is_async,
            });
        }
        Ok(clauses)
    }

    fn parse_dict_or_set_literal(&mut self) -> Result<Expr> {
        self.expect(&Token::LBrace)?;
        if self.is(&Token::RBrace) {
            self.bump();
            return Ok(Expr::Dict(vec![]));
        }
        // `**expr` at the very start unambiguously commits to a dict literal
        // (PEP 448 dict splat).
        if self.is(&Token::StarStar) {
            self.bump();
            let first_splat = self.parse_or()?;
            let mut items = vec![DictItem::DoubleSplat(first_splat)];
            while self.is(&Token::Comma) {
                self.bump();
                if self.is(&Token::RBrace) {
                    break;
                }
                items.push(self.parse_dict_item()?);
            }
            self.expect(&Token::RBrace)?;
            return Ok(Expr::Dict(items));
        }
        // `*expr` at the start commits to a set literal (PEP 448 set splat).
        if self.is(&Token::Star) {
            self.bump();
            let first_splat = self.parse_or()?;
            let mut items = vec![Expr::Starred(Box::new(first_splat))];
            while self.is(&Token::Comma) {
                self.bump();
                if self.is(&Token::RBrace) {
                    break;
                }
                items.push(self.parse_seq_item()?);
            }
            self.expect(&Token::RBrace)?;
            return Ok(Expr::Set(items));
        }
        let first = self.parse_expr()?;
        if self.is(&Token::Colon) {
            // Dict or dict comprehension
            self.bump();
            let val = self.parse_expr()?;
            if self.is(&Token::For) || self.is_async_for() {
                // Dict comprehension: {key: val for ...} or {key: val async for ...}
                let clauses = self.parse_comp_clauses()?;
                self.expect(&Token::RBrace)?;
                return Ok(Expr::DictComp {
                    key: Box::new(first),
                    val: Box::new(val),
                    clauses,
                });
            }
            let mut items = vec![DictItem::Pair(first, val)];
            while self.is(&Token::Comma) {
                self.bump();
                if self.is(&Token::RBrace) {
                    break;
                }
                items.push(self.parse_dict_item()?);
            }
            self.expect(&Token::RBrace)?;
            Ok(Expr::Dict(items))
        } else {
            // Set or set comprehension
            if self.is(&Token::For) || self.is_async_for() {
                // Set comprehension: {elt for ...} or {elt async for ...}
                let clauses = self.parse_comp_clauses()?;
                self.expect(&Token::RBrace)?;
                return Ok(Expr::SetComp {
                    elt: Box::new(first),
                    clauses,
                });
            }
            let mut items = vec![first];
            while self.is(&Token::Comma) {
                self.bump();
                if self.is(&Token::RBrace) {
                    break;
                }
                items.push(self.parse_seq_item()?);
            }
            self.expect(&Token::RBrace)?;
            Ok(Expr::Set(items))
        }
    }

    /// Parse one entry inside a dict literal: either `key: value` or `**expr`
    /// (PEP 448 dict splat).
    fn parse_dict_item(&mut self) -> Result<DictItem> {
        if self.is(&Token::StarStar) {
            self.bump();
            let val = self.parse_or()?;
            Ok(DictItem::DoubleSplat(val))
        } else {
            let k = self.parse_expr()?;
            self.expect(&Token::Colon)?;
            let v = self.parse_expr()?;
            Ok(DictItem::Pair(k, v))
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    fn expect_ident(&mut self, ctx: &str) -> Result<String> {
        match self.current().cloned() {
            Some(Token::Ident(name)) => {
                self.bump();
                Ok(name)
            }
            other => Err(PyError::Parse(format!("expected {ctx}, found {other:?}"))),
        }
    }

    fn current(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos + 1)
    }

    fn is(&self, token: &Token) -> bool {
        self.current() == Some(token)
    }

    fn bump(&mut self) {
        self.pos += 1;
    }

    fn expect(&mut self, token: &Token) -> Result<()> {
        if self.is(token) {
            self.bump();
            Ok(())
        } else {
            Err(PyError::Parse(format!(
                "expected {token:?}, found {:?}",
                self.current()
            )))
        }
    }

    fn skip_newlines(&mut self) {
        while self.is(&Token::Newline) {
            self.bump();
        }
    }

    fn at_stmt_end(&self) -> bool {
        matches!(
            self.current(),
            Some(Token::Newline)
                | Some(Token::Semicolon)
                | Some(Token::Dedent)
                | Some(Token::Eof)
                | None
        )
    }
}
