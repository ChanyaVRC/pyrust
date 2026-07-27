impl Parser {
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
}
