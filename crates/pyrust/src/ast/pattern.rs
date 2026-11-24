use super::{CaretSpan, Expr, Stmt, Symbol};

/// Assignment target (left-hand side of = or augmented =)
#[derive(Debug, Clone)]
pub enum AssignTarget {
    Name(Symbol),
    /// Attribute target `obj.attr`.  The third field is the PEP 657 caret
    /// anchor (issue #2442) for the whole `obj.attr` span — used to paint the
    /// caret when an augmented attribute assignment (`obj.attr += ...`) raises
    /// `AttributeError` on the read or store.  `None` when built without column
    /// info.
    Attr(Box<Expr>, Symbol, Option<CaretSpan>),
    Index(Box<Expr>, Box<Expr>),
    /// Slice target: a[lower:upper:step] — only valid for augmented assignment
    /// (plain slice assignment goes through `Stmt::SliceAssign`).
    Slice {
        target: Box<Expr>,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
    /// Unpack: a, b, c = ...
    Tuple(Vec<AssignTarget>),
    /// Starred target inside a Tuple: *name or *_ — only valid inside Tuple
    Starred(Box<AssignTarget>),
}

impl AssignTarget {
    /// Visit expressions that Python evaluates while resolving this target.
    ///
    /// A bare name is a binding, not an expression evaluation.  Attribute,
    /// item and slice targets evaluate their receiver/key/bounds even though
    /// they occur on the left-hand side.  Keeping that shape traversal next to
    /// the AST prevents scope, yield and walrus analyses from independently
    /// forgetting augmented-assignment target reads.
    pub(crate) fn visit_evaluated_exprs(&self, visitor: &mut impl FnMut(&Expr)) {
        match self {
            AssignTarget::Name(_) => {}
            AssignTarget::Attr(target, ..) => visitor(target),
            AssignTarget::Index(target, index) => {
                visitor(target);
                visitor(index);
            }
            AssignTarget::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                visitor(target);
                for expr in [lower, upper, step].iter().flat_map(|expr| expr.as_deref()) {
                    visitor(expr);
                }
            }
            AssignTarget::Tuple(targets) => {
                for target in targets {
                    target.visit_evaluated_exprs(visitor);
                }
            }
            AssignTarget::Starred(target) => target.visit_evaluated_exprs(visitor),
        }
    }
}

/// A single `case <pattern> [if <guard>]: <body>` arm in a `match` statement.
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
    /// Per-statement 1-based line numbers for the arm body.
    /// Empty when no line info is available.
    pub body_linenos: Vec<u32>,
}

/// Pattern variants for structural pattern matching (PEP 634).
#[derive(Debug, Clone)]
pub enum Pattern {
    /// `_` — always matches, no binding.
    Wildcard,
    /// `42`, `"str"`, `True`, `None` — equality test.
    Literal(Expr),
    /// `x` — binds the matched value to a name.
    Capture(Symbol),
    /// `p1 | p2` — matches if any sub-pattern matches.
    Or(Vec<Pattern>),
    /// `[a, b, *rest]` — destructures a sequence.
    /// Each element is `(pattern, is_star)`.
    Sequence(Vec<(Pattern, bool)>),
    /// `{"key": pattern, **rest}` — destructures a mapping.
    /// `rest` is the optional `**rest` capture name.
    Mapping(Vec<(Expr, Pattern)>, Option<Symbol>),
    /// `ClassName(pos, ..., attr=pat, ...)` — class pattern.
    ///
    /// `positional` sub-patterns are resolved at runtime via `__match_args__`;
    /// `kwargs` are matched directly against the named attribute.
    Class {
        cls: Box<Expr>,
        positional: Vec<Pattern>,
        kwargs: Vec<(Symbol, Pattern)>,
    },
    /// `a.b.c` — value pattern (dotted attribute lookup, compared with ==).
    /// Per PEP 634: a name followed by at least one `.attr` is a value pattern,
    /// not a capture.  The inner `Expr` is always an `Expr::Attr` chain.
    Value(Expr),
    /// `pattern as name` — matches `pattern` and, if it succeeds, binds the
    /// entire subject (not just the matched portion) to `name`.
    As { pattern: Box<Pattern>, name: Symbol },
}

impl Pattern {
    /// Visit expressions evaluated by a structural-match pattern.
    ///
    /// Capture names are bindings.  Literal/value expressions, mapping keys and
    /// the class expression are runtime reads and must remain visible to every
    /// scope/effect walker.
    pub(crate) fn visit_evaluated_exprs(&self, visitor: &mut impl FnMut(&Expr)) {
        match self {
            Pattern::Wildcard | Pattern::Capture(_) => {}
            Pattern::Literal(expr) | Pattern::Value(expr) => visitor(expr),
            Pattern::Or(patterns) => {
                for pattern in patterns {
                    pattern.visit_evaluated_exprs(visitor);
                }
            }
            Pattern::Sequence(elements) => {
                for (pattern, _) in elements {
                    pattern.visit_evaluated_exprs(visitor);
                }
            }
            Pattern::Mapping(pairs, _) => {
                for (key, pattern) in pairs {
                    visitor(key);
                    pattern.visit_evaluated_exprs(visitor);
                }
            }
            Pattern::Class {
                cls,
                positional,
                kwargs,
            } => {
                visitor(cls);
                for pattern in positional {
                    pattern.visit_evaluated_exprs(visitor);
                }
                for (_, pattern) in kwargs {
                    pattern.visit_evaluated_exprs(visitor);
                }
            }
            Pattern::As { pattern, .. } => pattern.visit_evaluated_exprs(visitor),
        }
    }
}
