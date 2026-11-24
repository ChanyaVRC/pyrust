impl Parser {
    fn parse_postfix(&mut self) -> Result<Expr> {
        // Start col/line of the primary (callee / subscript object) for #2411 /
        // #2992 caret anchors on `callee(...)` and `obj[...]` forms.
        let primary_start = self.current_col();
        let mut call_start = primary_start;
        let mut call_lineno = self.current_lineno();
        let mut expr = self.parse_primary()?;
        loop {
            if self.is(&Token::LParen) {
                self.bump();
                let args = self.parse_call_args()?;
                self.expect(&Token::RParen)?;
                // The whole `callee(...)` span is underlined with `^` (full ==
                // prim); multi-line calls clamp to the call's start line via
                // `MULTILINE_FULL_END` (#2411 / #2992).
                let span = self.make_whole_span(call_start, call_lineno);
                expr = Expr::Call {
                    func: Box::new(expr),
                    args,
                    span,
                    lineno: call_lineno,
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
                let attr_col = self.current_col();
                let attr_lineno = self.current_lineno();
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
                // Multi-line attribute access (`obj\n.method(...)`): CPython
                // anchors a subsequent call on `.method`'s line starting at the
                // attribute name rather than `obj`'s earlier line (#2992).
                if attr_lineno != 0 && call_lineno != 0 && attr_lineno != call_lineno {
                    call_start = attr_col;
                    call_lineno = attr_lineno;
                }
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
                    let elt_lineno = self.current_lineno();
                    let val = self.parse_expr()?;
                    // Generator expression as sole call argument: f(expr for x in it)
                    if (self.is(&Token::For) || self.is_async_for()) && args.is_empty() {
                        let clauses = self.parse_comp_clauses()?;
                        args.push(CallArg {
                            name: None,
                            value: Expr::GenExp {
                                elt: Box::new(val),
                                elt_lineno,
                                first_lineno: elt_lineno,
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
                let elt_lineno = self.current_lineno();
                let val = self.parse_expr()?;
                // Generator expression as sole call argument: f(expr for x in it)
                if (self.is(&Token::For) || self.is_async_for()) && args.is_empty() {
                    let clauses = self.parse_comp_clauses()?;
                    args.push(CallArg {
                        name: None,
                        value: Expr::GenExp {
                            elt: Box::new(val),
                            elt_lineno,
                            first_lineno: elt_lineno,
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
}
