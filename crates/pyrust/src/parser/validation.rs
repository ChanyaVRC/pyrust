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
                ..
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
