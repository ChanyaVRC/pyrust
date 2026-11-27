/// Messages CPython 3.12 raises from its compiler or symtable pass rather than
/// from the parser.  pyrust detects them while parsing, after the construct is
/// consumed, so the parser position is not where CPython points; they stay
/// location-less instead of reporting a wrong one (#3402).
fn is_compile_stage_message(msg: &str) -> bool {
    msg.starts_with("duplicate argument '")
        || msg.starts_with("keyword argument repeated: ")
        || matches!(
            msg,
            "can't use starred expression here"
                | "multiple starred expressions in assignment"
                | "starred assignment target must be in a list or tuple"
        )
}

/// Locate an unlocated lex/parse error on the 0-based columns `col..end_col`
/// (1-based lines), as CPython's parser does for a known node (#3402).
/// `tokenizer_lineno` is the line the parser stood on when it gave up.
#[cold]
fn located_parse_error(
    error: PyError,
    (line, col): (u32, u32),
    (end_line, end_col): (u32, u32),
    tokenizer_lineno: u32,
) -> PyError {
    match error.into_kind() {
        crate::error::PyErrorKind::Lex(msg) | crate::error::PyErrorKind::Parse(msg) => {
            PyError::syntax(crate::error::SyntaxErrorInfo::at_lines(
                msg,
                (line, i64::from(col) + 1),
                (end_line, i64::from(end_col) + 1),
                crate::error::SyntaxTextRule::ParserLine { tokenizer_lineno },
                false,
            ))
        }
        other => PyError::new(other),
    }
}

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

    /// Return the innermost opening delimiter that has not been closed before
    /// the current token. Used only on the parser's EOF error path.
    fn unclosed_delimiter(&self) -> Option<char> {
        self.unclosed_delimiter_at().map(|(open, _)| open)
    }

    /// Like [`Parser::unclosed_delimiter`], also returning the opening token's
    /// index (for the error location, #3402).
    fn unclosed_delimiter_at(&self) -> Option<(char, usize)> {
        let mut delimiters = Vec::new();
        let end = self.pos.min(self.tokens.len());
        for (index, token) in self.tokens[..end].iter().enumerate() {
            match token {
                Token::LParen => delimiters.push(('(', ')', index)),
                Token::LBracket => delimiters.push(('[', ']', index)),
                Token::LBrace => delimiters.push(('{', '}', index)),
                Token::RParen if delimiters.last().is_some_and(|&(_, close, _)| close == ')') => {
                    delimiters.pop();
                }
                Token::RBracket if delimiters.last().is_some_and(|&(_, close, _)| close == ']') => {
                    delimiters.pop();
                }
                Token::RBrace if delimiters.last().is_some_and(|&(_, close, _)| close == '}') => {
                    delimiters.pop();
                }
                _ => {}
            }
        }
        delimiters.last().map(|&(open, _, index)| (open, index))
    }

    /// Attach CPython's `SyntaxError` location to a parse failure (#3402).
    ///
    /// Errors that already carry a location (lexer errors, or parser sites
    /// that know their exact span) pass through.  Otherwise the error is
    /// anchored the way CPython's PEG parser anchors a failure:
    /// - `'(' was never closed` at the unclosed opener, with `end_offset` 0,
    ///   raised once the tokenizer reached the end of the input;
    /// - anything else on the token the parser stopped at — CPython's "last
    ///   token fetched" — spanning that token.  An INDENT (the column just
    ///   past the indentation) and the end of input (just past the last
    ///   line's newline) report the tokenizer cursor with `end_offset` -1,
    ///   like CPython's `unexpected indent` / `unexpected EOF`.
    ///
    /// Without line information (a `Parser::new` parse), and for
    /// [compile-stage messages](is_compile_stage_message), nothing is
    /// attached.
    #[cold]
    fn locate_error(&self, error: PyError) -> PyError {
        let msg = match error.kind() {
            crate::error::PyErrorKind::Lex(msg) | crate::error::PyErrorKind::Parse(msg) => msg,
            _ => return error,
        };
        if self.line_nos.is_empty() || self.tokens.is_empty() || is_compile_stage_message(msg) {
            return error;
        }
        let token_line = |index: usize| self.line_nos.get(index).copied().unwrap_or(0);
        let token_col = |index: usize| i64::from(self.cols.get(index).copied().unwrap_or(0));
        if msg.ends_with("was never closed")
            && let Some((_, open)) = self.unclosed_delimiter_at()
        {
            let line = token_line(open);
            return PyError::syntax(crate::error::SyntaxErrorInfo::at_lines(
                msg.clone(),
                (line, token_col(open) + 1),
                (line, 0),
                crate::error::SyntaxTextRule::ParserLine {
                    tokenizer_lineno: 0,
                },
                false,
            ));
        }
        let mut index = self.pos.min(self.tokens.len() - 1);
        // The DEDENTs closing the open blocks at the end of the input carry no
        // position of their own: CPython reports them at the end of input.
        if matches!(self.tokens[index], Token::Dedent)
            && self.tokens[index..]
                .iter()
                .all(|t| matches!(t, Token::Dedent | Token::Eof))
        {
            index = self.tokens.len() - 1;
        }
        let line = token_line(index);
        if line == 0 {
            return error;
        }
        let col = token_col(index);
        let ((offset, end_offset), newline_token) = match &self.tokens[index] {
            Token::Newline => ((col + 1, col + 2), true),
            // Both record the tokenizer cursor itself (just past the
            // indentation / past the last line's newline), not a token start.
            Token::Indent | Token::Dedent | Token::Eof => ((col, -1), false),
            _ => {
                let end = self.end_col_at(index).map_or(col + 1, i64::from);
                ((col + 1, end + 1), false)
            }
        };
        PyError::syntax(crate::error::SyntaxErrorInfo::at_lines(
            msg.clone(),
            (line, offset),
            (line, end_offset),
            crate::error::SyntaxTextRule::ParserLine {
                tokenizer_lineno: line,
            },
            newline_token,
        ))
    }

    /// The `IndentationError` CPython raises when the block of the compound
    /// statement whose header ends (just past its `:`) at token `header_end`
    /// has no indented body (#3403): `expected an indented block after
    /// 'if' statement on line 1` and its siblings.  The error is left to
    /// [`Parser::locate_error`], which anchors it on the token the parser
    /// stopped at, as CPython does.
    #[cold]
    fn missing_block_error(&self, header_end: usize) -> PyError {
        // A logical line's tokens follow the NEWLINE / INDENT / DEDENT that
        // precede it, so the header starts right after the nearest of those.
        let mut start = header_end.saturating_sub(1);
        while start > 0
            && !matches!(
                self.tokens[start - 1],
                Token::Newline | Token::Indent | Token::Dedent
            )
        {
            start -= 1;
        }
        // `async def` / `async for` / `async with` name the keyword after it.
        if matches!(&self.tokens.get(start), Some(Token::Ident(kw)) if kw == "async") {
            start += 1;
        }
        let what = match self.tokens.get(start) {
            Some(Token::Def) => "function definition",
            Some(Token::Class) => "class definition",
            Some(Token::If) => "'if' statement",
            Some(Token::Elif) => "'elif' statement",
            Some(Token::Else) => "'else' statement",
            Some(Token::While) => "'while' statement",
            Some(Token::For) => "'for' statement",
            Some(Token::Try) => "'try' statement",
            Some(Token::Except) if matches!(self.tokens.get(start + 1), Some(Token::Star)) => {
                "'except*' statement"
            }
            Some(Token::Except) => "'except' statement",
            Some(Token::Finally) => "'finally' statement",
            Some(Token::With) => "'with' statement",
            Some(Token::Ident(kw)) if kw == "match" => "'match' statement",
            Some(Token::Ident(kw)) if kw == "case" => "'case' statement",
            _ => return PyError::parse("expected an indented block".to_string()),
        };
        self.indented_block_error(what, start)
    }

    #[cold]
    fn indented_block_error(&self, what: &str, header: usize) -> PyError {
        match self.line_nos.get(header).copied().filter(|&line| line != 0) {
            Some(line) => PyError::parse(format!(
                "expected an indented block after {what} on line {line}"
            )),
            None => PyError::parse(format!("expected an indented block after {what}")),
        }
    }

    /// The error for a `match` header [`Parser::match_missing_block`] found,
    /// with the parser left on the token that should have been the INDENT.
    #[cold]
    fn match_missing_block_error(&mut self) -> PyError {
        let error = self.indented_block_error("'match' statement", self.pos);
        if let Some(index) = self.match_missing_block() {
            self.pos = index;
        }
        error
    }

    /// For a `match <subject>:` header that is followed by a NEWLINE but no
    /// indented block, the index of the token that should have been the
    /// INDENT (#3403).  `match` is a soft keyword, so such a header is only
    /// told apart from an ordinary expression statement by this shape.
    fn match_missing_block(&self) -> Option<usize> {
        let mut i = self.pos + 1;
        let mut depth = 0usize;
        loop {
            match self.tokens.get(i)? {
                Token::LParen | Token::LBracket | Token::LBrace => depth += 1,
                Token::RParen | Token::RBracket | Token::RBrace => depth = depth.checked_sub(1)?,
                Token::Colon if depth == 0 => break,
                Token::Newline | Token::Eof => return None,
                _ => {}
            }
            i += 1;
        }
        if i == self.pos + 1 || !matches!(self.tokens.get(i + 1), Some(Token::Newline)) {
            return None;
        }
        i += 1;
        while matches!(self.tokens.get(i), Some(Token::Newline)) {
            i += 1;
        }
        (!matches!(self.tokens.get(i), Some(Token::Indent))).then_some(i)
    }

    /// Locate a parse failure on the tokens `start..end` — a construct the
    /// parser had already consumed when it rejected it, such as an invalid
    /// assignment or `del` target, which CPython reports on the construct
    /// itself (#3402).  Errors that are already located, or that carry no
    /// position information, pass through.
    #[cold]
    fn locate_error_at(&self, error: PyError, start: usize, end: usize) -> PyError {
        if start >= end || end > self.line_nos.len() || end > self.cols.len() {
            return error;
        }
        let (line, end_line) = (self.line_nos[start], self.line_nos[end - 1]);
        if line == 0 || end_line == 0 {
            return error;
        }
        let col = self.cols[start];
        let end_col = self
            .end_col_at(end - 1)
            .unwrap_or_else(|| self.cols[end - 1] + 1);
        located_parse_error(
            error,
            (line, col),
            (end_line, end_col),
            self.current_lineno(),
        )
    }

    /// Token ranges of the top-level comma-separated items in
    /// `tokens[start..end]` (an assignment target group), for
    /// [`Parser::locate_error_at`].
    #[cold]
    fn top_level_item_ranges(&self, start: usize, end: usize) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut depth = 0usize;
        let mut item_start = start;
        let end_index = end.min(self.tokens.len());
        for (index, token) in self.tokens[..end_index].iter().enumerate().skip(start) {
            match token {
                Token::LParen | Token::LBracket | Token::LBrace => depth += 1,
                Token::RParen | Token::RBracket | Token::RBrace => {
                    depth = depth.saturating_sub(1);
                }
                Token::Comma if depth == 0 => {
                    ranges.push((item_start, index));
                    item_start = index + 1;
                }
                _ => {}
            }
        }
        if item_start < end {
            ranges.push((item_start, end));
        }
        ranges
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
    ///
    /// Failures carry CPython's `SyntaxError` location (#3402).
    pub fn parse_program_with_linenos(&mut self) -> Result<(Vec<Stmt>, Vec<u32>)> {
        self.parse_program_with_linenos_unlocated()
            .map_err(|error| self.locate_error(error))
    }

    /// Parse `eval`-mode input: like CPython 3.12's `eval` rule
    /// (`expressions NEWLINE* ENDMARKER`), it must be one expression (list)
    /// followed only by newlines.
    ///
    /// The input is parsed as a program and accepted when that yields exactly
    /// one expression statement without a `;`, so a valid expression gets the
    /// same AST as an `exec`-mode expression statement.  Anything else is
    /// rejected the way CPython's parser rejects it (#3402): see
    /// [`Parser::eval_error`].
    pub fn parse_eval_with_linenos(&mut self) -> Result<(Vec<Stmt>, Vec<u32>)> {
        match self.parse_program_with_linenos_unlocated() {
            Ok((stmts, linenos))
                if matches!(stmts.as_slice(), [Stmt::Expr(_)])
                    && !self.tokens.iter().any(|t| matches!(t, Token::Semicolon)) =>
            {
                Ok((stmts, linenos))
            }
            Ok(_) => Err(self.eval_error(None)),
            Err(error) => {
                let error = self.locate_error(error);
                Err(self.eval_error(Some(error)))
            }
        }
    }

    /// The `SyntaxError` for `eval`-mode input that is not a lone expression
    /// list.  `program_error` is the (located) failure of the program parse,
    /// if it failed.
    ///
    /// CPython parses a comma-separated list of (unstarred, walrus-free)
    /// expressions from the start and fails on the first token that cannot
    /// continue it, so the input is re-parsed that way and the error lands
    /// where CPython's does:
    /// - no tokens at all: CPython's end-of-input location;
    /// - the list ends: `invalid syntax` on the next token after any newlines
    ///   (`x = 1` stops at `=`), or `unexpected indent` on an INDENT;
    /// - an item cannot even start (`pass`, `import x`, `*a`): `invalid
    ///   syntax` on its first token, or `unexpected indent` on indentation;
    /// - an item fails part-way: the same failure the program parse hit
    ///   (`'(' was never closed`, a trailing operator).
    #[cold]
    fn eval_error(&mut self, program_error: Option<PyError>) -> PyError {
        self.pos = 0;
        self.skip_newlines();
        if self.is(&Token::Eof) || self.tokens.is_empty() {
            return PyError::syntax(crate::error::SyntaxErrorInfo::empty_input("invalid syntax"));
        }
        loop {
            if self.is(&Token::Star) || self.is(&Token::Indent) {
                break;
            }
            let item_start = self.pos;
            if let Err(item_error) = self.parse_expr_no_walrus() {
                if self.pos == item_start {
                    break;
                }
                return program_error.unwrap_or_else(|| self.locate_error(item_error));
            }
            if !self.is(&Token::Comma) {
                self.skip_newlines();
                break;
            }
            self.bump();
            if self.at_stmt_end() {
                self.skip_newlines();
                break;
            }
        }
        if self.is(&Token::Eof) {
            // Not reachable for input the program parse accepted as one
            // expression; keep whatever that parse reported.
            return program_error
                .unwrap_or_else(|| self.locate_error(PyError::parse("invalid syntax".into())));
        }
        let msg = if self.is(&Token::Indent) {
            "unexpected indent"
        } else {
            "invalid syntax"
        };
        self.locate_error(PyError::parse(msg.to_string()))
    }

    fn parse_program_with_linenos_unlocated(&mut self) -> Result<(Vec<Stmt>, Vec<u32>)> {
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

    /// Parse a single interactive statement for `compile(..., 'single')`.
    ///
    /// Mirrors CPython 3.12's `interactive` grammar:
    /// - Rejects empty / whitespace-only / comment-only input (`SyntaxError`).
    /// - Allows one logical simple-statement line (including `;`-separated
    ///   simple statements) OR one compound statement.
    /// - Rejects an inline compound statement lacking a trailing newline
    ///   (e.g. `if True: 1` without `\n`).
    /// - Rejects trailing statements on subsequent lines (`SyntaxError`).
    ///
    /// Failures carry CPython's `SyntaxError` location (#3402).
    pub fn parse_single_with_linenos(
        &mut self,
        ends_with_newline: bool,
    ) -> Result<(Vec<Stmt>, Vec<u32>)> {
        self.parse_single_with_linenos_unlocated(ends_with_newline)
            .map_err(|error| self.locate_error(error))
    }

    fn parse_single_with_linenos_unlocated(
        &mut self,
        ends_with_newline: bool,
    ) -> Result<(Vec<Stmt>, Vec<u32>)> {
        self.skip_newlines();
        if self.is(&Token::Eof) {
            // Located on CPython's ENDMARKER once the source is known (#3402).
            return Err(PyError::syntax(crate::error::SyntaxErrorInfo::empty_input(
                "invalid syntax",
            )));
        }
        let stmt_lineno = self.current_lineno();
        let stmts = self.parse_stmt_sequence()?;
        let is_compound = |s: &Stmt| {
            matches!(
                s,
                Stmt::If { .. }
                    | Stmt::While { .. }
                    | Stmt::For { .. }
                    | Stmt::Try { .. }
                    | Stmt::With { .. }
                    | Stmt::Match { .. }
                    | Stmt::Def { .. }
                    | Stmt::Class { .. }
            )
        };
        let has_compound = stmts.iter().any(is_compound);
        if has_compound && stmts.len() > 1 {
            return Err(PyError::parse("invalid syntax".to_string()));
        }
        if has_compound {
            let ended_with_dedent = self
                .pos
                .checked_sub(1)
                .and_then(|i| self.tokens.get(i))
                .is_some_and(|t| matches!(t, Token::Dedent));
            if !ended_with_dedent && !ends_with_newline {
                return Err(PyError::parse("invalid syntax".to_string()));
            }
        }
        let stmt_end = self.pos;
        self.skip_newlines();
        if !self.is(&Token::Eof) {
            if has_compound {
                return Err(PyError::parse("invalid syntax".to_string()));
            }
            // CPython reports this on the NEWLINE that ends the first
            // statement (#3402).
            let error = PyError::parse(
                "multiple statements found while compiling a single statement".to_string(),
            );
            let line = self.line_nos.get(stmt_end).copied().unwrap_or(0);
            return Err(match (self.tokens.get(stmt_end), self.cols.get(stmt_end)) {
                (Some(Token::Newline), Some(&col)) if line != 0 => {
                    located_parse_error(error, (line, col), (line, col + 1), line)
                }
                _ => error,
            });
        }
        let linenos = vec![stmt_lineno; stmts.len()];
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
            // Decorators may only precede `def`, `async def`, or `class`.
            Some(Token::Ident(kw))
                if decorators.is_empty()
                    && kw == "async"
                    && matches!(self.peek(), Some(Token::For)) =>
            {
                self.bump(); // consume `async`
                Ok(vec![self.parse_for(true)?])
            }
            Some(Token::Ident(kw))
                if decorators.is_empty()
                    && kw == "async"
                    && matches!(self.peek(), Some(Token::With)) =>
            {
                self.bump(); // consume `async`
                Ok(vec![self.parse_with(true)?])
            }
            _ if !decorators.is_empty() => Err(PyError::parse(
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
            Some(Token::Ident(kw)) if kw == "match" && self.match_missing_block().is_some() => {
                Err(self.match_missing_block_error())
            }
            Some(Token::Indent) => Err(PyError::parse("unexpected indent".to_string())),
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
                        return Err(PyError::parse(
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
