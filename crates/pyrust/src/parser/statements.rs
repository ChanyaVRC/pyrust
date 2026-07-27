impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            line_nos: Vec::new(),
            cols: Vec::new(),
            cols_end: Vec::new(),
            expr_depth: 0,
        }
    }

    /// Construct a parser with per-token line numbers produced by
    /// `Lexer::into_tokens_with_linenos`.
    ///
    /// Superseded for the script path by [`Parser::new_with_pos`] (which also
    /// carries columns for #2426); retained as the line-only constructor.
    #[allow(dead_code)]
    pub fn new_with_lines(tokens: Vec<Token>, line_nos: Vec<u32>) -> Self {
        Self {
            tokens,
            pos: 0,
            line_nos,
            cols: Vec::new(),
            cols_end: Vec::new(),
            expr_depth: 0,
        }
    }

    /// Construct a parser with per-token line numbers **and** start/end columns
    /// produced by `Lexer::into_tokens_with_pos` (issues #2426 / #2411).  Enables
    /// PEP 657 caret anchors on the plumbed expression forms.
    pub fn new_with_pos(
        tokens: Vec<Token>,
        line_nos: Vec<u32>,
        cols: Vec<u32>,
        cols_end: Vec<u32>,
    ) -> Self {
        Self {
            tokens,
            pos: 0,
            line_nos,
            cols,
            cols_end,
            expr_depth: 0,
        }
    }

    /// Return the 1-based source line number of the current token, or 0 when
    /// no line number information is available.
    fn current_lineno(&self) -> u32 {
        self.line_nos.get(self.pos).copied().unwrap_or(0)
    }

    /// Return the 1-based source line number of the token most recently
    /// consumed (`self.pos - 1`), or 0 when unavailable (issue #2571).
    fn prev_lineno(&self) -> u32 {
        match self.pos.checked_sub(1) {
            Some(p) => self.line_nos.get(p).copied().unwrap_or(0),
            None => 0,
        }
    }

    /// Return the 0-based start column of the current token, or `None` when no
    /// column information is available (issue #2426).
    fn current_col(&self) -> Option<u32> {
        self.cols.get(self.pos).copied()
    }

    /// Return the 0-based end column (one past the last char) of the token at
    /// `pos`, or `None` when no end-column information is available or the token
    /// carries the "no reliable end col" sentinel 0 (issue #2411).
    fn end_col_at(&self, pos: usize) -> Option<u32> {
        match self.cols_end.get(pos).copied() {
            Some(0) | None => None,
            Some(c) => Some(c),
        }
    }

    /// Return the end column of the token most recently consumed (`self.pos - 1`),
    /// i.e. the end column of the sub-expression that just finished parsing
    /// (issue #2411).  `None` before any token is consumed or when no reliable
    /// end col is available.
    fn prev_end_col(&self) -> Option<u32> {
        self.end_col_at(self.pos.checked_sub(1)?)
    }

    pub fn parse_program(&mut self) -> Result<Vec<Stmt>> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.is(&Token::Eof) {
            stmts.extend(self.parse_stmt_sequence()?);
            self.skip_newlines();
        }
        Ok(stmts)
    }

    /// Like `parse_program` but also returns a parallel `Vec<u32>` of 1-based
    /// source line numbers (one per top-level statement).  Only meaningful
    /// when the parser was constructed with `new_with_lines`.
    pub fn parse_program_with_linenos(&mut self) -> Result<(Vec<Stmt>, Vec<u32>)> {
        let mut stmts: Vec<Stmt> = Vec::new();
        let mut linenos: Vec<u32> = Vec::new();
        self.skip_newlines();
        while !self.is(&Token::Eof) {
            let stmt_lineno = self.current_lineno();
            let new_stmts = self.parse_stmt_sequence()?;
            // All statements produced from one parse_stmt_sequence share the
            // starting line (they are separated by `;` on the same logical line).
            for _ in &new_stmts {
                linenos.push(stmt_lineno);
            }
            stmts.extend(new_stmts);
            self.skip_newlines();
        }
        Ok((stmts, linenos))
    }

    fn parse_stmt_sequence(&mut self) -> Result<Vec<Stmt>> {
        let mut stmts = Vec::new();
        stmts.extend(self.parse_stmt()?);
        while self.is(&Token::Semicolon) {
            self.bump();
            if self.at_stmt_end() {
                break;
            }
            stmts.extend(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    // ── Statements ─────────────────────────────────────────────────────────

    fn parse_stmt(&mut self) -> Result<Vec<Stmt>> {
        // Collect decorators before def/class.  CPython 3.12 reports the line
        // of the *first decorator* (not the `def`) as `co_firstlineno` for a
        // decorated function, so capture the `@` line before consuming it
        // (issue #2185).  `0` when undecorated → `parse_def` falls back to the
        // `def` line.
        let deco_lineno = if self.is(&Token::At) {
            self.current_lineno()
        } else {
            0
        };
        let mut decorators: Vec<Expr> = Vec::new();
        while self.is(&Token::At) {
            decorators.push(self.parse_decorator()?);
            self.skip_newlines();
        }

        match self.current() {
            Some(Token::Def) => Ok(vec![self.parse_def(decorators, false, deco_lineno)?]),
            Some(Token::Class) => Ok(vec![self.parse_class(decorators)?]),
            // `async def` — soft keyword `async` followed by `def`
            Some(Token::Ident(kw)) if kw == "async" && matches!(self.peek(), Some(Token::Def)) => {
                Ok(vec![self.parse_async_def(decorators, deco_lineno)?])
            }
            // `async for` / `async with` — soft keyword `async` followed by
            // `for`/`with`.  Whether they appear inside an `async def` is checked
            // by the compiler (SyntaxError otherwise), matching CPython.
            Some(Token::Ident(kw)) if kw == "async" && matches!(self.peek(), Some(Token::For)) => {
                self.bump(); // consume `async`
                Ok(vec![self.parse_for(true)?])
            }
            Some(Token::Ident(kw)) if kw == "async" && matches!(self.peek(), Some(Token::With)) => {
                self.bump(); // consume `async`
                Ok(vec![self.parse_with(true)?])
            }
            _ if !decorators.is_empty() => Err(PyError::Parse(
                "decorator must be followed by def or class".to_string(),
            )),
            Some(Token::Global) => Ok(vec![self.parse_global()?]),
            Some(Token::Nonlocal) => Ok(vec![self.parse_nonlocal()?]),
            Some(Token::If) => Ok(vec![self.parse_if()?]),
            Some(Token::While) => Ok(vec![self.parse_while()?]),
            Some(Token::For) => Ok(vec![self.parse_for(false)?]),
            Some(Token::Try) => Ok(vec![self.parse_try()?]),
            Some(Token::Raise) => Ok(vec![self.parse_raise()?]),
            Some(Token::Import) => Ok(vec![self.parse_import()?]),
            Some(Token::From) => Ok(vec![self.parse_import_from()?]),
            Some(Token::Del) => Ok(vec![self.parse_del()?]),
            Some(Token::Assert) => Ok(vec![self.parse_assert()?]),
            Some(Token::With) => Ok(vec![self.parse_with(false)?]),
            // `match` is a soft keyword: `match <expr>:` followed by indented `case` arms.
            Some(Token::Ident(kw)) if kw == "match" && self.is_match_stmt() => {
                Ok(vec![self.parse_match()?])
            }
            // `type` is a soft keyword (PEP 695): `type <name> = <expr>`.
            // Only treat it as a type alias when followed by an identifier and `=`.
            Some(Token::Ident(kw)) if kw == "type" && self.is_type_alias_stmt() => {
                Ok(vec![self.parse_type_alias()?])
            }
            Some(Token::Return) => {
                self.bump();
                if self.at_stmt_end() {
                    Ok(vec![Stmt::Return(None)])
                } else {
                    let value = self.parse_expr_or_tuple()?;
                    if matches!(value, Expr::Starred(_)) {
                        return Err(PyError::Parse(
                            "can't use starred expression here".to_string(),
                        ));
                    }
                    Ok(vec![Stmt::Return(Some(value))])
                }
            }
            Some(Token::Break) => {
                self.bump();
                Ok(vec![Stmt::Break])
            }
            Some(Token::Continue) => {
                self.bump();
                Ok(vec![Stmt::Continue])
            }
            Some(Token::Pass) => {
                self.bump();
                Ok(vec![Stmt::Pass])
            }
            _ => self.parse_expr_stmt(),
        }
    }

    /// Determine if the current position starts a `match` statement.
    /// We require `match <expr> :` where the `:` is followed by `Newline Indent case`.
    /// This is a lookahead heuristic: scan forward past the expression to find `:`.
    fn is_match_stmt(&self) -> bool {
        // self.pos points at Token::Ident("match").
        // Scan forward: skip tokens until we find a Newline (which means we saw a `:` at
        // the end of the expression line, since the lexer emits `Newline` after `:` on the
        // match line), or until we hit Eof/Dedent/Indent.
        // Simpler heuristic: look for `Colon` then `Newline` then `Indent` then `Ident("case")`.
        let mut i = self.pos + 1; // skip `match`
        let mut depth = 0usize;
        loop {
            match self.tokens.get(i) {
                None | Some(Token::Eof) => return false,
                Some(Token::LParen) | Some(Token::LBracket) | Some(Token::LBrace) => {
                    depth += 1;
                    i += 1;
                }
                Some(Token::RParen) | Some(Token::RBracket) | Some(Token::RBrace) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                    i += 1;
                }
                Some(Token::Colon) if depth == 0 => {
                    // Found the colon; now check for Newline* Indent Newline* case.
                    // Comment-only lines between the match header and the first
                    // `case` arm fold into extra Newline tokens in the stream
                    // (the lexer skips Indent/Dedent for blank/comment lines).
                    i += 1;
                    // Skip any blank / comment-only Newlines before the Indent.
                    while matches!(self.tokens.get(i), Some(Token::Newline)) {
                        i += 1;
                    }
                    if matches!(self.tokens.get(i), Some(Token::Indent)) {
                        i += 1;
                    }
                    // Skip blank lines inside the indented block before the first case.
                    while matches!(self.tokens.get(i), Some(Token::Newline)) {
                        i += 1;
                    }
                    return matches!(
                        self.tokens.get(i),
                        Some(Token::Ident(k)) if k == "case"
                    );
                }
                Some(Token::Newline) if depth == 0 => return false,
                _ => {
                    i += 1;
                }
            }
        }
    }

    /// Determine if the current position starts a `type` alias statement (PEP 695).
    /// Returns true when `type` is followed by an identifier and then `=` (or `[`
    /// for the generic form, which we skip for now — just skip past `[...]` to
    /// find `=`).  This disambiguates `type = 1` (ordinary assignment) from
    /// `type Vector = list[float]` (type alias).
    fn is_type_alias_stmt(&self) -> bool {
        // self.pos points at Token::Ident("type").
        // Next token must be an identifier (the alias name).
        let i = self.pos + 1;
        if !matches!(self.tokens.get(i), Some(Token::Ident(_))) {
            return false;
        }
        // After the name, allow an optional `[...]` for generic type params,
        // then require `=`.
        let mut j = i + 1;
        if matches!(self.tokens.get(j), Some(Token::LBracket)) {
            // Skip balanced brackets.
            let mut depth = 0usize;
            loop {
                match self.tokens.get(j) {
                    None | Some(Token::Eof) => return false,
                    Some(Token::LBracket) => {
                        depth += 1;
                        j += 1;
                    }
                    Some(Token::RBracket) => {
                        depth -= 1;
                        j += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {
                        j += 1;
                    }
                }
            }
        }
        matches!(self.tokens.get(j), Some(Token::Assign))
    }
}
