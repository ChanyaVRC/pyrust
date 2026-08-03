/// Parse a single expression from a raw source string (used for f-string
/// sub-expressions).  `base_line` is the absolute source line of `src`'s first
/// line: it is folded into the sub-lexer's per-token line numbers so a nested
/// f-string's own fields report their absolute source line in tracebacks
/// (issue #2587).  `base_line == 0` means no line info is available, and the
/// sub-parser carries no line numbers (matching the prior behaviour).
fn parse_expr_str(src: &str, base_line: u32) -> Result<Expr> {
    let lexer = crate::lexer::Lexer::new(src)?;
    let mut p = if base_line == 0 {
        Parser::new(lexer.into_tokens())
    } else {
        let (tokens, line_nos) = lexer.into_tokens_with_linenos();
        // `src` line 1 maps to absolute `base_line`, so shift every token's
        // 1-based line number by `base_line - 1`.
        let line_nos = line_nos
            .into_iter()
            .map(|ln| if ln == 0 { 0 } else { ln + base_line - 1 })
            .collect();
        Parser::new_with_lines(tokens, line_nos)
    };
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
        Expr::Int(_) => Err(PyError::Parse(
            "cannot assign to literal here. Maybe you meant '==' instead of '='?".to_string(),
        )),
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
