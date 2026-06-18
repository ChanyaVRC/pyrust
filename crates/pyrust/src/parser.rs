use crate::ast::{
    AssignTarget, BinaryOp, CallArg, CmpOp, CompClause, DictItem, ExceptHandler, Expr, FStringPart,
    FunctionParam, MatchArm, Pattern, Stmt, TypeParam, TypeParamBound, UnaryOp,
};
use crate::error::{PyError, Result};
use crate::token::{FStringPart as LexFStringPart, Token};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Optional 1-based line numbers, one per token.  Empty when the parser was
    /// constructed without line tracking (via `Parser::new`).
    line_nos: Vec<u32>,
    /// Optional 0-based start columns (char offset within the token's physical
    /// line), one per token, for PEP 657 caret anchors (issue #2426).  Empty
    /// when the parser was constructed without column tracking; `Var` anchors
    /// are then recorded as `None`.
    cols: Vec<u32>,
    /// Optional 0-based end columns (char offset one past the token's last char,
    /// within its physical line), one per token, for multi-token PEP 657 caret
    /// anchors (issue #2411).  Empty when constructed without column tracking; a
    /// recorded 0 means "no reliable end col" (e.g. a token whose lexing crossed
    /// a physical newline) and suppresses the caret rather than emit a wrong one.
    cols_end: Vec<u32>,
    /// Current expression-nesting depth.  Incremented on every `parse_expr`
    /// entry; since each bracketed construct (`(`, `[`, `{`, call args,
    /// subscripts) re-enters `parse_expr` for its contents, this tracks the
    /// true nesting depth.  Bounded by [`MAX_EXPR_DEPTH`] so that pathological
    /// input (issue #2009: `eval("[" * 5000 + "1" + "]" * 5000)`) raises a
    /// catchable `SyntaxError` instead of overflowing the native stack
    /// (SIGABRT).  CPython rejects the same input with a `SyntaxError`.
    expr_depth: usize,
}

/// Maximum expression-nesting depth the parser accepts before raising
/// `SyntaxError: too many nested parentheses`.  CPython's recursive-descent
/// parser rejects bracket/paren nesting beyond depth 200; we match that so
/// programs CPython accepts continue to parse while deeper input fails with a
/// catchable error well below pyrust's native stack-overflow point.
///
/// The value is 201 rather than 200 because the outermost expression (an
/// assignment RHS or a bare `eval` expression) consumes one `parse_expr` level
/// before any bracket is opened, so a CPython-accepted bracket depth of 200
/// corresponds to `parse_expr` depth 201.
const MAX_EXPR_DEPTH: usize = 201;

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
                        }))
                    }
                    Some(Token::BigInt(s)) => {
                        self.bump();
                        Ok(Pattern::Literal(Expr::Unary {
                            op: crate::ast::UnaryOp::Neg,
                            expr: Box::new(Expr::BigInt(s)),
                        }))
                    }
                    Some(Token::Float(f)) => {
                        self.bump();
                        Ok(Pattern::Literal(Expr::Unary {
                            op: crate::ast::UnaryOp::Neg,
                            expr: Box::new(Expr::Float(f)),
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
                        "starred expression is not valid in this context".to_string(),
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
                "starred expression is not valid in this context".to_string(),
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

    fn parse_if(&mut self) -> Result<Stmt> {
        self.expect(&Token::If)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::Colon)?;
        let (body, body_lns) = self.parse_suite_with_linenos()?;
        let mut branches = vec![(cond, body)];
        let mut branch_linenos = vec![body_lns];
        let mut else_branch = None;
        let mut else_linenos = Vec::new();
        while self.is(&Token::Elif) {
            self.bump();
            let c = self.parse_expr()?;
            self.expect(&Token::Colon)?;
            let (b, b_lns) = self.parse_suite_with_linenos()?;
            branches.push((c, b));
            branch_linenos.push(b_lns);
        }
        if self.is(&Token::Else) {
            self.bump();
            self.expect(&Token::Colon)?;
            let (else_stmts, else_lns) = self.parse_suite_with_linenos()?;
            else_branch = Some(else_stmts);
            else_linenos = else_lns;
        }
        Ok(Stmt::If {
            branches,
            else_branch,
            branch_linenos,
            else_linenos,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt> {
        self.expect(&Token::While)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::Colon)?;
        let (body, body_linenos) = self.parse_suite_with_linenos()?;
        let (else_branch, else_linenos) = self.parse_optional_else_with_linenos()?;
        Ok(Stmt::While {
            cond,
            body,
            else_branch,
            body_linenos,
            else_linenos,
        })
    }

    fn parse_for(&mut self, is_async: bool) -> Result<Stmt> {
        self.expect(&Token::For)?;
        // Parse possibly-tuple target, supporting starred items like `for a, *b in ...`
        let target = self.parse_for_target()?;
        self.expect(&Token::In)?;
        let iter = self.parse_expr()?;
        self.expect(&Token::Colon)?;
        let (body, body_linenos) = self.parse_suite_with_linenos()?;
        let (else_branch, else_linenos) = self.parse_optional_else_with_linenos()?;
        Ok(Stmt::For {
            target,
            iter,
            body,
            else_branch,
            body_linenos,
            else_linenos,
            is_async,
        })
    }

    /// Parse a for-loop target, which may be a single name, a tuple of names,
    /// a parenthesized tuple target like `(x, y)`, or a starred-name target
    /// like `a, *b, c`.
    fn parse_for_target(&mut self) -> Result<AssignTarget> {
        // Parse the first item (may be starred, a bare name, or a parenthesized tuple)
        let (first, first_starred) = self.parse_for_target_item()?;

        if !self.is(&Token::Comma) {
            if first_starred {
                return Err(PyError::Parse(
                    "starred assignment target must be in a list or tuple".to_string(),
                ));
            }
            return Ok(first);
        }

        // Multiple items: build a tuple target
        let first_target = if first_starred {
            AssignTarget::Starred(Box::new(first))
        } else {
            first
        };
        let mut items: Vec<AssignTarget> = vec![first_target];
        let mut star_count = if first_starred { 1 } else { 0 };

        while self.is(&Token::Comma) {
            self.bump();
            if self.is(&Token::In) || self.is(&Token::RParen) {
                break;
            }
            let (item, is_starred) = self.parse_for_target_item()?;
            if is_starred {
                star_count += 1;
                if star_count > 1 {
                    return Err(PyError::Parse(
                        "multiple starred expressions in assignment".to_string(),
                    ));
                }
                items.push(AssignTarget::Starred(Box::new(item)));
            } else {
                items.push(item);
            }
        }
        Ok(AssignTarget::Tuple(items))
    }

    /// Parse a single for-loop target item: a starred name (`*x`), a
    /// parenthesized sub-target (`(x, y)`), or a bare name.
    /// Returns `(target, is_starred)`.
    fn parse_for_target_item(&mut self) -> Result<(AssignTarget, bool)> {
        if self.is(&Token::Star) {
            self.bump();
            let name = self.expect_ident("for loop starred variable")?;
            return Ok((AssignTarget::Name(name), true));
        }
        if self.is(&Token::LParen) {
            // Parenthesised target like `(x, y)`, `(x, (y, z))`, or `()` (empty tuple).
            // We parse the contents the same way as `parse_for_target` to avoid
            // consuming `in` as a comparison operator (which `parse_expr` would do).
            self.bump(); // consume `(`
            if self.is(&Token::RParen) {
                // Empty tuple target: `for () in ...`
                self.bump(); // consume `)`
                return Ok((AssignTarget::Tuple(vec![]), false));
            }
            let target = self.parse_for_target()?;
            self.expect(&Token::RParen)?;
            return Ok((target, false));
        }
        let name = self.expect_ident("for loop variable")?;
        Ok((AssignTarget::Name(name), false))
    }

    fn parse_optional_else_with_linenos(&mut self) -> Result<(Option<Vec<Stmt>>, Vec<u32>)> {
        if self.is(&Token::Else) {
            self.bump();
            self.expect(&Token::Colon)?;
            let (stmts, linenos) = self.parse_suite_with_linenos()?;
            Ok((Some(stmts), linenos))
        } else {
            Ok((None, Vec::new()))
        }
    }

    fn parse_dotted_name(&mut self) -> Result<String> {
        let first = self.expect_ident("module name")?;
        let mut parts = vec![first];
        while self.is(&Token::Dot) {
            self.bump();
            parts.push(self.expect_ident("module name after '.'")?)
        }
        Ok(parts.join("."))
    }

    fn parse_import(&mut self) -> Result<Stmt> {
        self.expect(&Token::Import)?;
        let mut names = Vec::new();
        loop {
            let module = self.parse_dotted_name()?;
            let alias = if self.is(&Token::As) {
                self.bump();
                Some(self.expect_ident("alias after 'as'")?)
            } else {
                None
            };
            names.push((module, alias));
            if !self.is(&Token::Comma) {
                break;
            }
            self.bump();
        }
        Ok(Stmt::Import { names })
    }

    fn parse_import_from(&mut self) -> Result<Stmt> {
        self.expect(&Token::From)?;
        // Count leading dots for relative imports: `from . import x`,
        // `from .. import x`, `from .pkg import x`, etc.
        // Token::Ellipsis (`...`) counts as 3 dots.
        let mut dots = String::new();
        loop {
            if self.is(&Token::Dot) {
                self.bump();
                dots.push('.');
            } else if self.is(&Token::Ellipsis) {
                self.bump();
                dots.push_str("...");
            } else {
                break;
            }
        }
        // The module name is optional when dots are present (`from . import x`).
        let base = if !dots.is_empty() && self.is(&Token::Import) {
            String::new()
        } else {
            self.parse_dotted_name()?
        };
        let module = format!("{}{}", dots, base);
        self.expect(&Token::Import)?;
        let mut names = Vec::new();
        if self.is(&Token::Star) {
            self.bump();
            names.push(("*".to_string(), None));
        } else {
            let paren = self.is(&Token::LParen);
            if paren {
                self.bump();
            }
            loop {
                let name = self.expect_ident("name in import list")?;
                let alias = if self.is(&Token::As) {
                    self.bump();
                    Some(self.expect_ident("alias after 'as'")?)
                } else {
                    None
                };
                names.push((name, alias));
                if !self.is(&Token::Comma) {
                    break;
                }
                self.bump();
                if paren && self.is(&Token::RParen) {
                    break;
                }
            }
            if paren {
                self.expect(&Token::RParen)?;
            }
        }
        Ok(Stmt::ImportFrom { module, names })
    }

    fn parse_try(&mut self) -> Result<Stmt> {
        self.expect(&Token::Try)?;
        self.expect(&Token::Colon)?;
        let (body, body_linenos) = self.parse_suite_with_linenos()?;
        let mut handlers = Vec::new();
        let mut else_branch = None;
        let mut else_linenos = Vec::new();
        let mut finally_branch = None;
        let mut finally_linenos = Vec::new();
        let mut saw_bare_except = false;
        let mut try_is_star: Option<bool> = None;

        while self.is(&Token::Except) {
            self.bump();
            // PEP 654: `except*` — star immediately follows `except`
            let is_star = if self.is(&Token::Star) {
                self.bump();
                true
            } else {
                false
            };
            // CPython rejects mixing `except` and `except*` on one `try`.
            match try_is_star {
                None => try_is_star = Some(is_star),
                Some(first) if first != is_star => {
                    return Err(PyError::Parse(
                        "cannot have both 'except' and 'except*' on the same 'try'".to_string(),
                    ));
                }
                _ => {}
            }
            let kind = if self.is(&Token::Colon) {
                if is_star {
                    return Err(PyError::Parse(
                        "except* requires an exception type".to_string(),
                    ));
                }
                saw_bare_except = true;
                None
            } else {
                if saw_bare_except {
                    return Err(PyError::Parse("bare except must be last".to_string()));
                }
                Some(self.parse_expr()?)
            };
            let name = if self.is(&Token::As) {
                self.bump();
                Some(self.expect_ident("exception variable")?)
            } else {
                None
            };
            self.expect(&Token::Colon)?;
            let (handler_body, handler_linenos) = self.parse_suite_with_linenos()?;
            if is_star {
                check_except_star_body(&handler_body, false)?;
            }
            handlers.push(ExceptHandler {
                kind,
                name,
                body: handler_body,
                is_star,
                body_linenos: handler_linenos,
            });
        }

        if self.is(&Token::Else) {
            self.bump();
            self.expect(&Token::Colon)?;
            let (stmts, lns) = self.parse_suite_with_linenos()?;
            else_branch = Some(stmts);
            else_linenos = lns;
        }
        if self.is(&Token::Finally) {
            self.bump();
            self.expect(&Token::Colon)?;
            let (stmts, lns) = self.parse_suite_with_linenos()?;
            finally_branch = Some(stmts);
            finally_linenos = lns;
        }
        if handlers.is_empty() && finally_branch.is_none() {
            return Err(PyError::Parse(
                "try statement must have at least one except or finally clause".to_string(),
            ));
        }
        Ok(Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
            body_linenos,
            else_linenos,
            finally_linenos,
        })
    }

    fn parse_raise(&mut self) -> Result<Stmt> {
        // Start col of the `raise` keyword for the #2411 whole-statement anchor.
        let raise_start = self.current_col();
        self.expect(&Token::Raise)?;
        if self.at_stmt_end() {
            Ok(Stmt::Raise {
                expr: None,
                cause: None,
                span: None,
            })
        } else {
            let expr = Some(self.parse_expr()?);
            let cause = if self.is(&Token::From) {
                self.bump();
                Some(self.parse_expr()?)
            } else {
                None
            };
            // The statement spans from `raise` through the end of the last
            // parsed expression (the cause, when present, else the exception).
            // Whole-`^` span (full == prim), matching CPython's raise underline.
            let span = match (raise_start, self.prev_end_col()) {
                (Some(s), Some(e)) if s < e => Some((s, s, e, e)),
                _ => None,
            };
            Ok(Stmt::Raise { expr, cause, span })
        }
    }

    fn parse_del(&mut self) -> Result<Stmt> {
        self.expect(&Token::Del)?;
        let mut targets = vec![self.parse_expr()?];
        while self.is(&Token::Comma) {
            self.bump();
            if self.at_stmt_end() {
                break;
            }
            targets.push(self.parse_expr()?);
        }
        for t in &targets {
            validate_del_target(t)?;
        }
        Ok(Stmt::Delete(targets))
    }

    fn parse_assert(&mut self) -> Result<Stmt> {
        self.expect(&Token::Assert)?;
        let test = self.parse_expr()?;
        let msg = if self.is(&Token::Comma) {
            self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Stmt::Assert { test, msg })
    }

    fn parse_with(&mut self, is_async: bool) -> Result<Stmt> {
        self.expect(&Token::With)?;
        let mut items = Vec::new();

        // PEP 617 (Python 3.10+): detect parenthesized with-items form.
        // `with (expr as x, expr2 as y):` or `with (expr,):` etc.
        // Heuristic: if the next token is `(` and the paren group contains an
        // `as` keyword or a trailing comma at depth-0-within-the-parens, treat
        // the entire parenthesized block as a with-items list rather than a
        // single parenthesized expression.
        if self.is(&Token::LParen) && self.is_parenthesized_with_items() {
            self.bump(); // consume `(`
            loop {
                if self.is(&Token::RParen) {
                    break;
                }
                let expr = self.parse_expr()?;
                let var = if self.is(&Token::As) {
                    self.bump();
                    Some(expr_to_assign_target(&self.parse_expr()?)?)
                } else {
                    None
                };
                items.push((expr, var));
                if !self.is(&Token::Comma) {
                    break;
                }
                self.bump(); // consume `,`
                // allow trailing comma before `)`
            }
            self.expect(&Token::RParen)?;
        } else {
            loop {
                let expr = self.parse_expr()?;
                let var = if self.is(&Token::As) {
                    self.bump();
                    Some(expr_to_assign_target(&self.parse_expr()?)?)
                } else {
                    None
                };
                items.push((expr, var));
                if !self.is(&Token::Comma) {
                    break;
                }
                self.bump();
            }
        }

        self.expect(&Token::Colon)?;
        let (body, body_linenos) = self.parse_suite_with_linenos()?;
        Ok(Stmt::With {
            items,
            body,
            body_linenos,
            is_async,
        })
    }

    /// Return `true` when the `(` at `self.pos` opens a PEP 617
    /// parenthesized-with-items list rather than a plain parenthesized
    /// expression.
    ///
    /// Disambiguation rules (matching CPython 3.10+):
    ///
    /// 1. If the token immediately after the matching `)` is `as`, the parens
    ///    enclose a single expression (possibly a tuple) bound to that name —
    ///    not a with-items list.  E.g. `with (CM(1), CM(2)) as pair:`.
    ///
    /// 2. Otherwise, if the interior of the parens contains an `as` keyword at
    ///    depth 0 (direct child of the outer `(…)`) → PEP 617.
    ///    E.g. `with (CM(1) as a, CM(2) as b):`.
    ///
    /// 3. Otherwise, if the interior contains a comma at depth 0 → PEP 617.
    ///    E.g. `with (CM(1), CM(2)):` or `with (CM(1),):`.
    ///
    /// 4. Otherwise → plain parenthesized expression.
    ///    E.g. `with (CM(1)):`.
    fn is_parenthesized_with_items(&self) -> bool {
        debug_assert!(self.tokens.get(self.pos) == Some(&Token::LParen));

        // First, find the matching `)` and check what follows it.
        let mut i = self.pos + 1;
        let mut depth: usize = 0;
        let close_paren_pos;
        loop {
            match self.tokens.get(i) {
                None | Some(Token::Eof) => return false,
                Some(Token::LParen) | Some(Token::LBracket) | Some(Token::LBrace) => {
                    depth += 1;
                }
                Some(Token::RBracket) | Some(Token::RBrace) => {
                    if depth == 0 {
                        return false; // malformed
                    }
                    depth -= 1;
                }
                Some(Token::RParen) => {
                    if depth == 0 {
                        close_paren_pos = i;
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }

        // Rule 1: if the token after `)` is `as`, this is a plain parenthesized
        // expression (the `as` binds the whole paren group as one context manager).
        if self.tokens.get(close_paren_pos + 1) == Some(&Token::As) {
            return false;
        }

        // Rules 2 & 3: scan inside the parens for `as` or comma at depth 0.
        let mut i = self.pos + 1;
        let mut depth: usize = 0;
        while i < close_paren_pos {
            match self.tokens.get(i) {
                Some(Token::LParen) | Some(Token::LBracket) | Some(Token::LBrace) => {
                    depth += 1;
                }
                Some(Token::RParen) | Some(Token::RBracket) | Some(Token::RBrace) => {
                    depth -= 1;
                }
                Some(Token::As) if depth == 0 => return true,
                Some(Token::Comma) if depth == 0 => return true,
                _ => {}
            }
            i += 1;
        }

        false
    }

    fn parse_suite(&mut self) -> Result<Vec<Stmt>> {
        Ok(self.parse_suite_with_linenos()?.0)
    }

    /// Like `parse_suite` but also returns a parallel `Vec<u32>` of 1-based
    /// source line numbers for each statement in the suite.  When no line
    /// information is available (parser built without `new_with_lines`), all
    /// entries will be 0.
    fn parse_suite_with_linenos(&mut self) -> Result<(Vec<Stmt>, Vec<u32>)> {
        if self.is(&Token::Newline) {
            self.bump();
            // Skip blank / comment-only lines that the lexer folds into bare
            // Newline tokens.  CPython accepts a leading comment or blank line
            // as the first line of any suite-introducing block (try/except/
            // for/while/def/class/with/if/else/match …).
            self.skip_newlines();
            self.expect(&Token::Indent)?;
            let mut out = Vec::new();
            let mut linenos: Vec<u32> = Vec::new();
            self.skip_newlines();
            while !self.is(&Token::Dedent) && !self.is(&Token::Eof) {
                let stmt_lineno = self.current_lineno();
                let new_stmts = self.parse_stmt_sequence()?;
                for _ in &new_stmts {
                    linenos.push(stmt_lineno);
                }
                out.extend(new_stmts);
                self.skip_newlines();
            }
            self.expect(&Token::Dedent)?;
            Ok((out, linenos))
        } else {
            let stmt_lineno = self.current_lineno();
            let stmts = self.parse_stmt_sequence()?;
            // Consume the trailing newline (and any blank lines that follow) so
            // that callers can immediately check for continuation keywords such
            // as `except`, `elif`, `else`, and `finally` without needing their
            // own skip_newlines() calls after every parse_suite() invocation.
            self.skip_newlines();
            let linenos = vec![stmt_lineno; stmts.len()];
            Ok((stmts, linenos))
        }
    }

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
    /// it was parsed); `op_col` / `op_end` bracket the operator token; the right
    /// operand's end col is read from the just-consumed token (`prev_end_col`).
    /// Returns `None` (caret suppressed) if any piece is missing or the columns
    /// are inconsistent — never a wrong caret.
    fn make_binary_span(
        &self,
        left_start: Option<u32>,
        op_col: Option<u32>,
        op_end: Option<u32>,
    ) -> Option<crate::ast::CaretSpan> {
        let full_start = left_start?;
        let prim_start = op_col?;
        let prim_end = op_end?;
        let full_end = self.prev_end_col()?;
        if full_start <= prim_start && prim_start < prim_end && prim_end <= full_end {
            Some((full_start, prim_start, prim_end, full_end))
        } else {
            None
        }
    }

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
            self.bump();
            let expr = self.parse_not()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
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
        let mut expr = self.parse_bitxor()?;
        while self.is(&Token::Pipe) {
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let right = self.parse_bitxor()?;
            let span = self.make_binary_span(left_start, op_col, op_end);
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
        let mut expr = self.parse_bitand()?;
        while self.is(&Token::Caret) {
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let right = self.parse_bitand()?;
            let span = self.make_binary_span(left_start, op_col, op_end);
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
        let mut expr = self.parse_shift()?;
        while self.is(&Token::Ampersand) {
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let right = self.parse_shift()?;
            let span = self.make_binary_span(left_start, op_col, op_end);
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
            let span = self.make_binary_span(left_start, op_col, op_end);
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
            let span = self.make_binary_span(left_start, op_col, op_end);
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
            let span = self.make_binary_span(left_start, op_col, op_end);
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if self.is(&Token::Minus) {
            self.bump();
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.is(&Token::Tilde) {
            self.bump();
            return Ok(Expr::Unary {
                op: UnaryOp::BitNot,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.is(&Token::Plus) {
            self.bump();
            return Ok(Expr::Unary {
                op: UnaryOp::Pos,
                expr: Box::new(self.parse_unary()?),
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
        let base = self.parse_postfix()?;
        if self.is(&Token::StarStar) {
            let (op_col, op_end) = (self.current_col(), self.end_col_at(self.pos));
            self.bump();
            let exp = self.parse_unary()?; // right-associative
            let span = self.make_binary_span(left_start, op_col, op_end);
            return Ok(Expr::Binary {
                left: Box::new(base),
                op: BinaryOp::Pow,
                right: Box::new(exp),
                span,
            });
        }
        Ok(base)
    }

    fn parse_postfix(&mut self) -> Result<Expr> {
        // Start col of the primary (callee / subscript object) for #2411 caret
        // anchors on `callee(...)` and `obj[...]` forms.
        let primary_start = self.current_col();
        let mut expr = self.parse_primary()?;
        loop {
            if self.is(&Token::LParen) {
                self.bump();
                let args = self.parse_call_args()?;
                self.expect(&Token::RParen)?;
                // The whole `callee(...)` span is underlined with `^` (full ==
                // prim); end col is the just-consumed `)` (#2411).
                let span = match (primary_start, self.prev_end_col()) {
                    (Some(s), Some(e)) if s < e => Some((s, s, e, e)),
                    _ => None,
                };
                expr = Expr::Call {
                    func: Box::new(expr),
                    args,
                    span,
                };
                continue;
            }
            if self.is(&Token::LBracket) {
                let bracket_col = self.current_col();
                self.bump();
                expr = self.parse_subscript(expr, primary_start, bracket_col)?;
                continue;
            }
            if self.is(&Token::Dot) {
                self.bump();
                let name = match self.current().cloned() {
                    Some(Token::Ident(name)) => {
                        self.bump();
                        name
                    }
                    other => {
                        return Err(PyError::Parse(format!(
                            "expected attribute name after '.', found {other:?}"
                        )));
                    }
                };
                // PEP 657 caret anchor (#2442): underline the whole `obj.attr`
                // span (full == prim) from the target's start column to the
                // just-consumed attribute name's end column.
                let span = match (primary_start, self.prev_end_col()) {
                    (Some(s), Some(e)) if s < e => Some((s, s, e, e)),
                    _ => None,
                };
                expr = Expr::Attr {
                    target: Box::new(expr),
                    name,
                    span,
                };
                continue;
            }
            break;
        }
        Ok(expr)
    }

    fn parse_subscript(
        &mut self,
        target: Expr,
        // Start col of the subscript object and the `[` bracket, for the #2411
        // caret anchor: object underlined `~`, `[...]` underlined `^`.
        object_start: Option<u32>,
        bracket_col: Option<u32>,
    ) -> Result<Expr> {
        // Inside [ already consumed. Parse optional slice or index.
        // A leading `*` (PEP 646 starred subscript, e.g. `m[*idx]`) forces the
        // index to be a tuple even with a single element.
        let mut saw_star = false;
        let first = if self.is(&Token::Colon) {
            None
        } else if self.is(&Token::Star) {
            self.bump();
            saw_star = true;
            Some(Expr::Starred(Box::new(self.parse_or()?)))
        } else {
            Some(self.parse_expr()?)
        };

        if self.is(&Token::Colon) {
            // Slice
            self.bump();
            let upper = if self.is(&Token::RBracket) || self.is(&Token::Colon) {
                None
            } else {
                Some(Box::new(self.parse_expr()?))
            };
            let step = if self.is(&Token::Colon) {
                self.bump();
                if self.is(&Token::RBracket) {
                    None
                } else {
                    Some(Box::new(self.parse_expr()?))
                }
            } else {
                None
            };
            self.expect(&Token::RBracket)?;
            Ok(Expr::Slice {
                target: Box::new(target),
                lower: first.map(Box::new),
                upper,
                step,
            })
        } else {
            // Plain index. If a comma follows, collect into a tuple key
            // (e.g. `a[b, c]` parses the same as `a[(b, c)]`).
            let first = first.ok_or_else(|| PyError::Parse("empty subscript".to_string()))?;
            let index = if self.is(&Token::Comma) {
                let mut items = vec![first];
                while self.is(&Token::Comma) {
                    self.bump();
                    if self.is(&Token::RBracket) {
                        break;
                    }
                    if self.is(&Token::Star) {
                        self.bump();
                        items.push(Expr::Starred(Box::new(self.parse_or()?)));
                    } else {
                        items.push(self.parse_expr()?);
                    }
                }
                Expr::Tuple(items)
            } else if saw_star {
                // A single starred index still forms a 1-tuple: `m[*xs]` -> `m[(*xs,)]`.
                Expr::Tuple(vec![first])
            } else {
                first
            };
            self.expect(&Token::RBracket)?;
            // Anchor: object `[object_start, bracket_col)` underlined `~`, the
            // `[...]` part `[bracket_col, ]end)` underlined `^` (#2411).
            let span = match (object_start, bracket_col, self.prev_end_col()) {
                (Some(obj), Some(br), Some(end)) if obj <= br && br < end => {
                    Some((obj, br, end, end))
                }
                _ => None,
            };
            Ok(Expr::Index {
                target: Box::new(target),
                index: Box::new(index),
                span,
            })
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<CallArg>> {
        let mut args = Vec::new();
        let mut seen_keyword = false;
        if self.is(&Token::RParen) {
            return Ok(args);
        }
        loop {
            if self.is(&Token::StarStar) {
                self.bump();
                seen_keyword = true;
                args.push(CallArg {
                    name: None,
                    value: self.parse_expr()?,
                    splat: false,
                    double_splat: true,
                });
            } else if self.is(&Token::Star) {
                self.bump();
                if seen_keyword {
                    return Err(PyError::Parse(
                        "positional argument follows keyword argument".to_string(),
                    ));
                }
                args.push(CallArg {
                    name: None,
                    value: self.parse_expr()?,
                    splat: true,
                    double_splat: false,
                });
            } else if let Some(Token::Ident(name)) = self.current().cloned() {
                if self.peek() == Some(&Token::Assign) {
                    self.bump();
                    self.bump();
                    seen_keyword = true;
                    args.push(CallArg {
                        name: Some(name),
                        value: self.parse_expr()?,
                        splat: false,
                        double_splat: false,
                    });
                } else {
                    if seen_keyword {
                        return Err(PyError::Parse(
                            "positional argument follows keyword argument".to_string(),
                        ));
                    }
                    let val = self.parse_expr()?;
                    // Generator expression as sole call argument: f(expr for x in it)
                    if (self.is(&Token::For) || self.is_async_for()) && args.is_empty() {
                        let clauses = self.parse_comp_clauses()?;
                        args.push(CallArg {
                            name: None,
                            value: Expr::GenExp {
                                elt: Box::new(val),
                                clauses,
                            },
                            splat: false,
                            double_splat: false,
                        });
                        break;
                    }
                    args.push(CallArg {
                        name: None,
                        value: val,
                        splat: false,
                        double_splat: false,
                    });
                }
            } else {
                if seen_keyword {
                    return Err(PyError::Parse(
                        "positional argument follows keyword argument".to_string(),
                    ));
                }
                let val = self.parse_expr()?;
                // Generator expression as sole call argument: f(expr for x in it)
                if (self.is(&Token::For) || self.is_async_for()) && args.is_empty() {
                    let clauses = self.parse_comp_clauses()?;
                    args.push(CallArg {
                        name: None,
                        value: Expr::GenExp {
                            elt: Box::new(val),
                            clauses,
                        },
                        splat: false,
                        double_splat: false,
                    });
                    break;
                }
                args.push(CallArg {
                    name: None,
                    value: val,
                    splat: false,
                    double_splat: false,
                });
            }
            if self.is(&Token::RParen) {
                break;
            }
            self.expect(&Token::Comma)?;
            if self.is(&Token::RParen) {
                break;
            }
        }
        // CPython rejects a literal keyword repeated in a call (`f(x=1, x=2)`)
        // at compile time.  `**dict` splats are not checked here.
        let mut seen_kw: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for a in &args {
            if let Some(name) = &a.name
                && !seen_kw.insert(name.as_str())
            {
                return Err(PyError::Parse(format!("keyword argument repeated: {name}")));
            }
        }
        Ok(args)
    }

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
                            parts.extend(self.parse_fstring_parts(lex_parts)?);
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
                self.bump();
                let mut parts = self.parse_fstring_parts(lex_parts)?;
                // Adjacent string/f-string literal concatenation.
                loop {
                    match self.current().cloned() {
                        Some(Token::FString(next_lex)) => {
                            self.bump();
                            parts.extend(self.parse_fstring_parts(next_lex)?);
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
                // column and `len` its char length.  `self.pos` still points at
                // the Ident here (before `bump`).
                let span = self
                    .current_col()
                    .map(|col| (col, col + name.chars().count() as u32));
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

    /// Convert lexer-level `FStringPart`s into AST-level `FStringPart`s by
    /// running a sub-parser on each raw expression source string.
    fn parse_fstring_parts(&self, lex_parts: Vec<LexFStringPart>) -> Result<Vec<FStringPart>> {
        let mut ast_parts = Vec::new();
        for lp in lex_parts {
            match lp {
                LexFStringPart::Literal(s) => {
                    ast_parts.push(FStringPart::Literal(s));
                }
                LexFStringPart::Expr {
                    src,
                    conversion,
                    format_spec,
                    debug_text,
                } => {
                    let expr = parse_expr_str(&src)?;
                    // Recursively parse any nested expressions inside the
                    // format spec — they need to be visible to every AST
                    // recursor (scope-pass, closure-capture analyser, etc.).
                    let format_spec = match format_spec {
                        None => None,
                        Some(parts) => Some(self.parse_fstring_parts(parts)?),
                    };
                    ast_parts.push(FStringPart::Expr {
                        expr: Box::new(expr),
                        conversion,
                        format_spec,
                        debug_text,
                    });
                }
            }
        }
        Ok(ast_parts)
    }
}

/// Parse a single expression from a raw source string (used for f-string sub-expressions).
fn parse_expr_str(src: &str) -> Result<Expr> {
    let lexer = crate::lexer::Lexer::new(src)?;
    let tokens = lexer.into_tokens();
    let mut p = Parser::new(tokens);
    let expr = p.parse_expr()?;
    Ok(expr)
}

/// Build a single assignment statement assigning `rhs` to the LHS `target`.
/// Used by annotation assignment, where exactly one bare target appears.
fn lhs_to_assign_stmt(target: &Expr, rhs: Expr) -> Result<Stmt> {
    match target {
        Expr::Var(name, _) if name == "__debug__" => {
            Err(PyError::Parse("cannot assign to __debug__".to_string()))
        }
        Expr::Var(name, _) => Ok(Stmt::Assign(AssignTarget::Name(name.clone()), rhs)),
        Expr::Attr {
            target, name, span, ..
        } => Ok(Stmt::AttrAssign {
            target: *target.clone(),
            name: name.clone(),
            expr: rhs,
            span: *span,
        }),
        Expr::Index { target, index, .. } => Ok(Stmt::IndexAssign {
            target: target.clone(),
            index: index.clone(),
            expr: rhs,
        }),
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => Ok(Stmt::SliceAssign {
            target: target.clone(),
            lower: lower.clone(),
            upper: upper.clone(),
            step: step.clone(),
            expr: rhs,
        }),
        Expr::Tuple(elems) | Expr::List(elems) => {
            // Items inside a parenthesized or bracketed target may include
            // `*name` (Expr::Starred).  Extract the starred flags the same way
            // expr_to_assign_target does so they aren't silently dropped.
            let mut exprs: Vec<Expr> = Vec::with_capacity(elems.len());
            let mut flags: Vec<bool> = Vec::with_capacity(elems.len());
            for item in elems {
                match item {
                    Expr::Starred(inner) => {
                        exprs.push(*inner.clone());
                        flags.push(true);
                    }
                    other => {
                        exprs.push(other.clone());
                        flags.push(false);
                    }
                }
            }
            let assign_targets = exprs_to_assign_targets(&exprs, &flags)?;
            Ok(Stmt::Assign(AssignTarget::Tuple(assign_targets), rhs))
        }
        _ => Err(PyError::Parse(
            "cannot assign to this expression".to_string(),
        )),
    }
}

/// Build assignment statements for one target group with the given value
/// expression.  A "group" is the parsed left-hand side of one `=`, which is
/// a comma-separated list of expressions (already classified for starred-ness)
/// plus a flag for whether a trailing comma was seen.
fn group_to_assign_stmt(
    items: Vec<Expr>,
    starred_flags: Vec<bool>,
    had_comma: bool,
    value: Expr,
) -> Result<Stmt> {
    if items.len() == 1 && !starred_flags[0] && !had_comma {
        return lhs_to_assign_stmt(&items[0], value);
    }
    // Multi-target or starred tuple unpack: a, b = ...   or   *a, b = ...
    let assign_targets = exprs_to_assign_targets(&items, &starred_flags)?;
    Ok(Stmt::Assign(AssignTarget::Tuple(assign_targets), value))
}

/// Build the lowered statement sequence for one or more target groups
/// assigned from the single RHS `rhs`.  For a single group this is one
/// statement, with `rhs` used directly.  For N > 1 groups (chained
/// assignment), the RHS is evaluated once into a hidden temporary, then
/// each group is assigned from that temporary in left-to-right order so
/// side-effects on the targets (e.g. `obj.x = obj.y = expr`) match
/// CPython's semantics.
fn build_assign_stmts(groups: Vec<(Vec<Expr>, Vec<bool>, bool)>, rhs: Expr) -> Result<Vec<Stmt>> {
    debug_assert!(!groups.is_empty());
    if groups.len() == 1 {
        let (items, flags, had_comma) = groups.into_iter().next().unwrap();
        return Ok(vec![group_to_assign_stmt(items, flags, had_comma, rhs)?]);
    }
    // Chained: evaluate rhs into a unique hidden temporary, then assign
    // from that temporary to each target group left-to-right.
    //
    // The temporary name uses angle brackets and a space so it cannot
    // collide with any user-written Python identifier.  It is local to
    // the enclosing scope; each chained-assignment site uses a distinct
    // name to avoid aliasing across sites.
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_name = format!("<chain_assign {n}>");

    let mut out: Vec<Stmt> = Vec::with_capacity(groups.len() + 1);
    out.push(Stmt::Assign(AssignTarget::Name(tmp_name.clone()), rhs));
    for (items, flags, had_comma) in groups {
        out.push(group_to_assign_stmt(
            items,
            flags,
            had_comma,
            Expr::Var(tmp_name.clone(), None),
        )?);
    }
    Ok(out)
}

fn expr_to_assign_target(expr: &Expr) -> Result<AssignTarget> {
    match expr {
        Expr::Var(name, _) if name == "__debug__" => {
            Err(PyError::Parse("cannot assign to __debug__".to_string()))
        }
        Expr::Var(name, _) => Ok(AssignTarget::Name(name.clone())),
        Expr::Attr {
            target, name, span, ..
        } => Ok(AssignTarget::Attr(target.clone(), name.clone(), *span)),
        Expr::Index { target, index, .. } => Ok(AssignTarget::Index(target.clone(), index.clone())),
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => Ok(AssignTarget::Slice {
            target: target.clone(),
            lower: lower.clone(),
            upper: upper.clone(),
            step: step.clone(),
        }),
        Expr::Tuple(items) | Expr::List(items) => {
            // Items that were parsed as `*expr` inside a parenthesised/bracketed
            // tuple or list literal come in as `Expr::Starred`; lift them into
            // starred flags so the target machinery can handle them correctly.
            let mut exprs: Vec<Expr> = Vec::with_capacity(items.len());
            let mut flags: Vec<bool> = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Expr::Starred(inner) => {
                        exprs.push(*inner.clone());
                        flags.push(true);
                    }
                    other => {
                        exprs.push(other.clone());
                        flags.push(false);
                    }
                }
            }
            let targets = exprs_to_assign_targets(&exprs, &flags)?;
            Ok(AssignTarget::Tuple(targets))
        }
        _ => Err(PyError::Parse(
            "cannot assign to this expression".to_string(),
        )),
    }
}

/// Reject a duplicated parameter name in a `def`/`lambda` parameter list, and
/// a parameter named `__debug__`, matching CPython 3.12.  Duplicate detection
/// runs first (`duplicate argument '<n>' in function definition`); a non-dup
/// `__debug__` parameter then yields `cannot assign to __debug__`.
fn check_duplicate_params(params: &[FunctionParam]) -> Result<()> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for p in params {
        if !seen.insert(p.name.as_str()) {
            return Err(PyError::Parse(format!(
                "duplicate argument '{}' in function definition",
                p.name
            )));
        }
    }
    for p in params {
        if p.name == "__debug__" {
            return Err(PyError::Parse("cannot assign to __debug__".to_string()));
        }
    }
    Ok(())
}

/// Reject `return`/`break`/`continue` inside an `except*` handler body, matching
/// CPython 3.12 (PEP 654).  `return` is always illegal; `break`/`continue` are
/// only illegal when they would target a loop *outside* the handler — a loop
/// nested within the handler body absorbs them, so `in_loop` tracks whether we
/// are currently inside such an enclosed loop.  Nested function/class scopes are
/// not descended (their control flow is independent).
fn check_except_star_body(body: &[Stmt], in_loop: bool) -> Result<()> {
    const MSG: &str = "'break', 'continue' and 'return' cannot appear in an except* block";
    for stmt in body {
        match stmt {
            Stmt::Return(_) => return Err(PyError::Parse(MSG.to_string())),
            Stmt::Break | Stmt::Continue if !in_loop => {
                return Err(PyError::Parse(MSG.to_string()));
            }
            Stmt::Break | Stmt::Continue => {}
            // New scopes: control flow inside them is independent.
            Stmt::Def { .. } | Stmt::Class { .. } | Stmt::TypeAlias { .. } => {}
            Stmt::If {
                branches,
                else_branch,
                ..
            } => {
                for (_, b) in branches {
                    check_except_star_body(b, in_loop)?;
                }
                if let Some(b) = else_branch {
                    check_except_star_body(b, in_loop)?;
                }
            }
            Stmt::While {
                body, else_branch, ..
            }
            | Stmt::For {
                body, else_branch, ..
            } => {
                // The loop body absorbs break/continue, but its else-branch
                // (and the loop's own header) do not introduce a loop for them.
                check_except_star_body(body, true)?;
                if let Some(b) = else_branch {
                    check_except_star_body(b, in_loop)?;
                }
            }
            Stmt::With { body, .. } => check_except_star_body(body, in_loop)?,
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
                ..
            } => {
                check_except_star_body(body, in_loop)?;
                for h in handlers {
                    check_except_star_body(&h.body, in_loop)?;
                }
                if let Some(b) = else_branch {
                    check_except_star_body(b, in_loop)?;
                }
                if let Some(b) = finally_branch {
                    check_except_star_body(b, in_loop)?;
                }
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    check_except_star_body(&arm.body, in_loop)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validate a `match`/`case` pattern at compile time, raising the CPython 3.12
/// `SyntaxError`s pyrust would otherwise accept:
///
/// * a capture name used more than once across the pattern
///   (`multiple assignments to name '<n>' in pattern`) — but each `|`
///   alternative is checked independently, since alternatives are mutually
///   exclusive and (per a separate check) bind the same names;
/// * `_` used as a binding target (`<pat> as _`) → `cannot use '_' as a target`;
/// * a duplicate key in a mapping pattern
///   (`mapping pattern checks duplicate key (<repr>)`); and
/// * a repeated keyword in a class pattern
///   (`attribute name repeated in class pattern: <n>`).
fn validate_pattern(pat: &Pattern) -> Result<()> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    collect_pattern_captures(pat, &mut seen)?;
    Ok(())
}

/// Recursively collect capture names from `pat`, raising on a duplicate.
/// Names bound by `_` are rejected outright (`cannot use '_' as a target`).
fn collect_pattern_captures(
    pat: &Pattern,
    seen: &mut std::collections::HashSet<String>,
) -> Result<()> {
    match pat {
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Value(_) => {}
        Pattern::Capture(name) => bind_capture(name, seen)?,
        Pattern::As { pattern, name } => {
            collect_pattern_captures(pattern, seen)?;
            bind_capture(name, seen)?;
        }
        Pattern::Sequence(elements) => {
            for (elem, _is_star) in elements {
                collect_pattern_captures(elem, seen)?;
            }
        }
        Pattern::Mapping(pairs, rest) => {
            check_duplicate_mapping_keys(pairs)?;
            for (_key, val_pat) in pairs {
                collect_pattern_captures(val_pat, seen)?;
            }
            if let Some(rest) = rest {
                bind_capture(rest, seen)?;
            }
        }
        Pattern::Class {
            positional, kwargs, ..
        } => {
            for p in positional {
                collect_pattern_captures(p, seen)?;
            }
            let mut attrs: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for (attr, p) in kwargs {
                if !attrs.insert(attr.as_str()) {
                    return Err(PyError::Parse(format!(
                        "attribute name repeated in class pattern: {attr}"
                    )));
                }
                collect_pattern_captures(p, seen)?;
            }
        }
        Pattern::Or(alternatives) => {
            // Alternatives are mutually exclusive: each gets a fresh view of the
            // names already bound *outside* the Or, then contributes its bindings
            // back exactly once (all alternatives bind the same names).
            for alt in alternatives {
                let mut branch = seen.clone();
                collect_pattern_captures(alt, &mut branch)?;
            }
            if let Some(first) = alternatives.first() {
                collect_pattern_captures(first, seen)?;
            }
        }
    }
    Ok(())
}

/// Record a capture name, rejecting `_` (illegal target) and duplicates.
fn bind_capture(name: &str, seen: &mut std::collections::HashSet<String>) -> Result<()> {
    if name == "_" {
        return Err(PyError::Parse("cannot use '_' as a target".to_string()));
    }
    if !seen.insert(name.to_string()) {
        return Err(PyError::Parse(format!(
            "multiple assignments to name '{name}' in pattern"
        )));
    }
    Ok(())
}

/// Reject duplicate literal keys in a mapping pattern, comparing by Python value
/// equality (so `1`, `1.0` and `True` collide) and reporting the CPython repr of
/// the offending (second) key.  Non-literal keys (value patterns) are not
/// checked — CPython only flags constant keys.
fn check_duplicate_mapping_keys(pairs: &[(Expr, Pattern)]) -> Result<()> {
    let mut seen: Vec<MapKey> = Vec::new();
    for (key, _) in pairs {
        if let Some(mk) = MapKey::from_expr(key) {
            if seen.iter().any(|s| s.eq_value(&mk)) {
                return Err(PyError::Parse(format!(
                    "mapping pattern checks duplicate key ({})",
                    mk.repr()
                )));
            }
            seen.push(mk);
        }
    }
    Ok(())
}

/// A literal mapping-pattern key, normalised for Python value-equality
/// comparison while retaining its source form for the error repr.
enum MapKey {
    /// Numeric keys compared by `f64` value (`1`, `1.0`, `True` all equal `1.0`).
    Num {
        value: f64,
        repr: String,
    },
    Str(String),
    Bytes(Vec<u8>),
    None,
}

impl MapKey {
    fn from_expr(e: &Expr) -> Option<MapKey> {
        match e {
            Expr::Int(n) => Some(MapKey::Num {
                value: *n as f64,
                repr: n.to_string(),
            }),
            Expr::Float(f) => Some(MapKey::Num {
                value: *f,
                repr: pyrust_core::key_repr(&pyrust_core::PyKey::Float(f.to_bits())),
            }),
            Expr::Bool(b) => Some(MapKey::Num {
                value: if *b { 1.0 } else { 0.0 },
                repr: if *b { "True".into() } else { "False".into() },
            }),
            Expr::None => Some(MapKey::None),
            Expr::Str(s) => Some(MapKey::Str(s.clone())),
            Expr::Bytes(b) => Some(MapKey::Bytes(b.clone())),
            // Negative numeric literal: `-1` parses as Unary(Neg, Int/Float).
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
            } => match MapKey::from_expr(expr) {
                Some(MapKey::Num { value, repr }) => Some(MapKey::Num {
                    value: -value,
                    repr: format!("-{repr}"),
                }),
                _ => None,
            },
            _ => None,
        }
    }

    fn eq_value(&self, other: &MapKey) -> bool {
        match (self, other) {
            (MapKey::Num { value: a, .. }, MapKey::Num { value: b, .. }) => a == b,
            (MapKey::Str(a), MapKey::Str(b)) => a == b,
            (MapKey::Bytes(a), MapKey::Bytes(b)) => a == b,
            (MapKey::None, MapKey::None) => true,
            _ => false,
        }
    }

    fn repr(&self) -> String {
        match self {
            MapKey::Num { repr, .. } => repr.clone(),
            MapKey::Str(s) => pyrust_core::key_repr(&pyrust_core::PyKey::str_from(s)),
            MapKey::Bytes(b) => {
                pyrust_core::key_repr(&pyrust_core::PyKey::Bytes(std::rc::Rc::new(b.clone())))
            }
            MapKey::None => "None".to_string(),
        }
    }
}

/// Validate a single `del` target, raising the CPython 3.12 `SyntaxError`
/// message for non-deletable expressions (constants, literals, `__debug__`,
/// starred items, displays, calls, operators, …). Deletable targets — names,
/// attributes, subscripts, slices, and tuples/lists of these — are accepted.
fn validate_del_target(expr: &Expr) -> Result<()> {
    let reason = match expr {
        Expr::Var(name, _) if name == "__debug__" => {
            return Err(PyError::Parse("cannot delete __debug__".to_string()));
        }
        Expr::Var(_, _) | Expr::Attr { .. } | Expr::Index { .. } | Expr::Slice { .. } => {
            return Ok(());
        }
        Expr::Tuple(items) | Expr::List(items) => {
            for item in items {
                validate_del_target(item)?;
            }
            return Ok(());
        }
        Expr::None => "None",
        Expr::Bool(true) => "True",
        Expr::Bool(false) => "False",
        Expr::Ellipsis => "ellipsis",
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::FString(_) => "literal",
        Expr::Starred(_) => "starred",
        Expr::Set(_) | Expr::SetComp { .. } => "set display",
        Expr::Dict(_) | Expr::DictComp { .. } => "dict literal",
        Expr::ListComp { .. } | Expr::GenExp { .. } => "expression",
        Expr::Call { .. } => "function call",
        Expr::Ternary { .. } => "conditional expression",
        Expr::Compare { .. } => "comparison",
        Expr::Yield(_) | Expr::YieldFrom(_) => "yield expression",
        Expr::Lambda { .. } => "lambda",
        _ => "expression",
    };
    Err(PyError::Parse(format!("cannot delete {reason}")))
}

/// Convert a parallel list of expressions and starred-flags into AssignTargets.
/// `starred[i] == true` means item `i` should be wrapped in `AssignTarget::Starred`.
fn exprs_to_assign_targets(exprs: &[Expr], starred: &[bool]) -> Result<Vec<AssignTarget>> {
    assert_eq!(exprs.len(), starred.len());
    let star_count = starred.iter().filter(|&&s| s).count();
    if star_count > 1 {
        return Err(PyError::Parse(
            "multiple starred expressions in assignment".to_string(),
        ));
    }
    let mut targets = Vec::with_capacity(exprs.len());
    for (expr, &is_starred) in exprs.iter().zip(starred.iter()) {
        let base = expr_to_assign_target(expr)?;
        if is_starred {
            targets.push(AssignTarget::Starred(Box::new(base)));
        } else {
            targets.push(base);
        }
    }
    Ok(targets)
}
