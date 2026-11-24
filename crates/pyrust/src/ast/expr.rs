use super::{AssignTarget, CaretSpan, Symbol};

#[derive(Debug, Clone)]
pub struct FunctionParam {
    pub name: Symbol,
    pub default: Option<Expr>,
    pub annotation: Option<Expr>,
    pub is_args: bool,            // *args
    pub is_kwargs: bool,          // **kwargs
    pub is_keyword_only: bool,    // declared after * or *args
    pub is_positional_only: bool, // declared before / separator
}

#[derive(Debug, Clone)]
pub struct CallArg {
    pub name: Option<Symbol>,
    pub value: Expr,
    pub splat: bool,        // *expr
    pub double_splat: bool, // **expr
}

/// An entry in a dict literal: either a `key: value` pair or a `**expr` splat
/// per PEP 448.
#[derive(Debug, Clone)]
pub enum DictItem {
    Pair(Expr, Expr),
    /// `**expr` — merge `expr` (a mapping) into the dict.
    DoubleSplat(Expr),
}

/// One part of a parsed f-string.
#[derive(Debug, Clone)]
pub enum FStringPart {
    /// A literal text fragment.
    Literal(String),
    /// An embedded expression with optional conversion flag (`!r`/`!s`/`!a`)
    /// and optional format spec.  When present, the format spec is itself a
    /// list of f-string parts so that nested `{expr}` interpolations inside
    /// the spec (e.g. `f"{x:>{width}}"`) are exposed as real sub-expressions
    /// — this is what allows the scope-pass / closure-capture analyser to see
    /// names referenced inside the spec.
    ///
    /// `debug_text`, when `Some`, marks the Python 3.8 debug form `f"{x=}"`
    /// and carries the verbatim source text of the expression with the
    /// trailing `=` (whitespace preserved).  The compiler emits this as a
    /// literal prefix and defaults the value conversion to `repr` (unless an
    /// explicit conversion flag or format spec is present).
    ///
    /// `line` is the absolute source line of this field's `{` (the f-string
    /// fragment's start line plus the field's newline offset within that
    /// fragment).  The compiler stamps the field's bytecode with it so a field
    /// on a continuation line of a multi-line f-string — or in a later
    /// implicitly-joined fragment — reports its own line in tracebacks
    /// (issue #2587).  `0` when source line info is unavailable.
    Expr {
        expr: Box<Expr>,
        conversion: Option<char>,
        format_spec: Option<Vec<FStringPart>>,
        debug_text: Option<String>,
        /// PEP 657 caret anchor (issue #2582): the whole replacement field
        /// `{...}` (including the braces and any conversion/format spec),
        /// underlined with `^` (full == prim), matching CPython's
        /// `FORMAT_VALUE` anchor.  `None` for nested fields inside a format
        /// spec, fields synthesised without column info, or fields on a
        /// continuation line of a multi-line f-string.
        span: Option<CaretSpan>,
        line: u32,
    },
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64),
    /// Integer literal that does not fit in i64; stored as a decimal string.
    BigInt(String),
    Float(f64),
    /// Complex literal: (real, imag) — produced from imaginary `Nj` tokens.
    Complex(f64, f64),
    Str(String),
    Bytes(Vec<u8>),
    Bool(bool),
    None,
    Ellipsis,
    /// An f-string: `f"Hello, {name}!"`
    FString(Vec<FStringPart>),
    /// A bare name reference.  The optional `(col_offset, end_col_offset,
    /// lineno)` is the PEP 657 caret anchor (issue #2426): `col_offset` /
    /// `end_col_offset` are 0-based char columns within the name's source line,
    /// `lineno` is the name's own 1-based source line.  Recorded by the parser
    /// when token position information is available.  `None` for names
    /// synthesised by the parser (desugaring temporaries) or built without
    /// position info.
    ///
    /// The `lineno` slot lets the compiler stamp the name-load instruction with
    /// the line the *name itself* is on, not the enclosing statement's first
    /// line — so a name on a continuation line of a multi-line expression
    /// reports its own line in tracebacks, matching CPython 3.12 (issue #2632).
    Var(Symbol, Option<(u32, u32, u32)>),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Dict(Vec<DictItem>),
    Set(Vec<Expr>),
    /// PEP 448 splat inside a list / tuple / set literal: `*expr`.
    /// Only valid as a direct child of `Expr::List` / `Expr::Tuple` / `Expr::Set`
    /// (collection-literal contexts).  Splat in function-call argument lists is
    /// represented via `CallArg::splat`, and splat assign-targets via
    /// `AssignTarget::Starred`.
    Starred(Box<Expr>),
    /// `[elt for target in iter (if cond)?  (for target2 in iter2 ...)*]`
    ListComp {
        elt: Box<Expr>,
        clauses: Vec<CompClause>,
        span: Option<CaretSpan>,
    },
    /// `{key: val for target in iter ...}`
    DictComp {
        key: Box<Expr>,
        val: Box<Expr>,
        clauses: Vec<CompClause>,
        span: Option<CaretSpan>,
    },
    /// `{elt for target in iter ...}`
    SetComp {
        elt: Box<Expr>,
        clauses: Vec<CompClause>,
        span: Option<CaretSpan>,
    },
    /// `(elt for target in iter ...)` — generator expression
    GenExp {
        elt: Box<Expr>,
        /// 1-based source line of `elt` (issue #2992).
        elt_lineno: u32,
        /// 1-based source line of the opening `(` (or `elt` when unparenthesized
        /// as a sole call argument) — backs `<genexpr>`'s `co_firstlineno`.
        first_lineno: u32,
        clauses: Vec<CompClause>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        /// PEP 657 caret anchor (issue #2582) for the arithmetic unary forms
        /// `-x` / `+x` / `~x`: underlined with `^` (full == prim) from the
        /// operator token through the end of the operand, matching CPython's
        /// `UNARY_NEGATIVE` / `UNARY_INVERT` anchor.  `None` for `not` (CPython
        /// anchors only the operand there, a different shape), for unary forms
        /// synthesised by the parser/optimizer, or when built without column
        /// info.
        span: Option<CaretSpan>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
        /// PEP 657 caret anchor (issue #2411): operands underlined with `~`,
        /// the operator token with `^`.  `None` for operators synthesised by
        /// the parser/optimizer or built without column info.
        span: Option<CaretSpan>,
    },
    /// Chained: a < b < c  →  Compare { left: a, ops: [(Lt, b), (Lt, c)] }
    Compare {
        left: Box<Expr>,
        ops: Vec<(CmpOp, Expr)>,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    Lambda {
        params: Vec<FunctionParam>,
        body: Box<Expr>,
    },
    Call {
        func: Box<Expr>,
        args: Vec<CallArg>,
        /// PEP 657 caret anchor (issue #2411 / #2992): the whole `callee(...)`
        /// span, underlined with `^` (clamped via `MULTILINE_FULL_END` for
        /// multi-line calls).  `None` for calls synthesised by the parser or
        /// built without column info.
        span: Option<CaretSpan>,
        /// 1-based source line where the call starts (`0` when unavailable).
        /// Stamps the terminal call instruction so multi-line nested calls and
        /// chained `.method(...)` calls report their own line (#2992).
        lineno: u32,
    },
    Attr {
        target: Box<Expr>,
        name: Symbol,
        /// PEP 657 caret anchor (issue #2442): the whole `obj.attr` span,
        /// underlined with `^` (full == prim), from the target's start column to
        /// the attribute name's end column.  `None` for attribute accesses
        /// synthesised by the parser or built without column info.
        span: Option<CaretSpan>,
    },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
        /// PEP 657 caret anchor (issue #2411): object underlined with `~`, the
        /// `[...]` subscript with `^`.  `None` when built without column info.
        span: Option<CaretSpan>,
    },
    Slice {
        target: Box<Expr>,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
    /// Walrus operator: `target := value`
    Named {
        target: Symbol,
        value: Box<Expr>,
    },
    /// `yield` or `yield expr` — only valid inside a generator function
    Yield {
        value: Option<Box<Expr>>,
        span: Option<CaretSpan>,
        lineno: u32,
    },
    /// `yield from expr`
    YieldFrom {
        expr: Box<Expr>,
        span: Option<CaretSpan>,
        lineno: u32,
    },
    /// `await expr` — only valid inside an async function
    Await {
        expr: Box<Expr>,
        span: Option<CaretSpan>,
        lineno: u32,
    },
}

impl Expr {
    /// Visit assignment-expression targets that bind the nearest enclosing
    /// non-comprehension scope.
    ///
    /// Comprehensions do not form a binding boundary for `:=` (PEP 572), so
    /// their value, filters, and iterables are traversed. A lambda body does
    /// form a real function scope and is skipped, while its defaults and
    /// annotations are evaluated in the surrounding scope and remain visible.
    ///
    /// Keeping this boundary rule on the AST prevents the compiler's cell
    /// analysis, comprehension validation, and interpreter symbol-table pass
    /// from drifting apart.
    pub(crate) fn visit_enclosing_walrus_targets(&self, visitor: &mut impl FnMut(&str)) {
        match self {
            Expr::Named { target, value } => {
                visitor(target);
                value.visit_enclosing_walrus_targets(visitor);
            }
            Expr::Lambda { params, .. } => {
                for parameter in params {
                    if let Some(default) = &parameter.default {
                        default.visit_enclosing_walrus_targets(visitor);
                    }
                    if let Some(annotation) = &parameter.annotation {
                        annotation.visit_enclosing_walrus_targets(visitor);
                    }
                }
            }
            Expr::ListComp { elt, clauses, .. }
            | Expr::SetComp { elt, clauses, .. }
            | Expr::GenExp { elt, clauses, .. } => {
                elt.visit_enclosing_walrus_targets(visitor);
                for clause in clauses {
                    clause.iter.visit_enclosing_walrus_targets(visitor);
                    if let Some(condition) = &clause.cond {
                        condition.visit_enclosing_walrus_targets(visitor);
                    }
                }
            }
            Expr::DictComp {
                key, val, clauses, ..
            } => {
                key.visit_enclosing_walrus_targets(visitor);
                val.visit_enclosing_walrus_targets(visitor);
                for clause in clauses {
                    clause.iter.visit_enclosing_walrus_targets(visitor);
                    if let Some(condition) = &clause.cond {
                        condition.visit_enclosing_walrus_targets(visitor);
                    }
                }
            }
            Expr::Binary { left, right, .. } => {
                left.visit_enclosing_walrus_targets(visitor);
                right.visit_enclosing_walrus_targets(visitor);
            }
            Expr::Unary { expr, .. }
            | Expr::Starred(expr)
            | Expr::YieldFrom { expr, .. }
            | Expr::Await { expr, .. } => expr.visit_enclosing_walrus_targets(visitor),
            Expr::Yield { value, .. } => {
                if let Some(expr) = value {
                    expr.visit_enclosing_walrus_targets(visitor);
                }
            }
            Expr::Compare { left, ops } => {
                left.visit_enclosing_walrus_targets(visitor);
                for (_, operand) in ops {
                    operand.visit_enclosing_walrus_targets(visitor);
                }
            }
            Expr::Ternary { cond, then, else_ } => {
                cond.visit_enclosing_walrus_targets(visitor);
                then.visit_enclosing_walrus_targets(visitor);
                else_.visit_enclosing_walrus_targets(visitor);
            }
            Expr::Call { func, args, .. } => {
                func.visit_enclosing_walrus_targets(visitor);
                for argument in args {
                    argument.value.visit_enclosing_walrus_targets(visitor);
                }
            }
            Expr::Attr { target, .. } => target.visit_enclosing_walrus_targets(visitor),
            Expr::Index { target, index, .. } => {
                target.visit_enclosing_walrus_targets(visitor);
                index.visit_enclosing_walrus_targets(visitor);
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                target.visit_enclosing_walrus_targets(visitor);
                for bound in [lower, upper, step]
                    .iter()
                    .flat_map(|bound| bound.as_deref())
                {
                    bound.visit_enclosing_walrus_targets(visitor);
                }
            }
            Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    item.visit_enclosing_walrus_targets(visitor);
                }
            }
            Expr::Dict(items) => {
                for item in items {
                    match item {
                        DictItem::Pair(key, value) => {
                            key.visit_enclosing_walrus_targets(visitor);
                            value.visit_enclosing_walrus_targets(visitor);
                        }
                        DictItem::DoubleSplat(expr) => {
                            expr.visit_enclosing_walrus_targets(visitor);
                        }
                    }
                }
            }
            Expr::FString(parts) => {
                fn visit_parts(parts: &[FStringPart], visitor: &mut impl FnMut(&str)) {
                    for part in parts {
                        if let FStringPart::Expr {
                            expr, format_spec, ..
                        } = part
                        {
                            expr.visit_enclosing_walrus_targets(visitor);
                            if let Some(format_spec) = format_spec {
                                visit_parts(format_spec, visitor);
                            }
                        }
                    }
                }
                visit_parts(parts, visitor);
            }
            Expr::Var(_, _)
            | Expr::Int(_)
            | Expr::BigInt(_)
            | Expr::Float(_)
            | Expr::Complex(_, _)
            | Expr::Str(_)
            | Expr::Bytes(_)
            | Expr::Bool(_)
            | Expr::None
            | Expr::Ellipsis => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    Pos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    MatMul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    BitAnd,
    BitOr,
    BitXor,
    LShift,
    RShift,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    In,
    NotIn,
    Is,
    IsNot,
}

/// A single `for target in iter (if cond)?` clause inside a comprehension.
/// `is_async` is set when the clause is `async for target in iter`.
#[derive(Debug, Clone)]
pub struct CompClause {
    pub target: AssignTarget,
    pub iter: Expr,
    pub cond: Option<Expr>,
    /// Individual `if` filters with their 1-based source lines, in source
    /// order, so multi-line chained `if` clauses report accurate line numbers
    /// (issue #2992).  `cond` retains the `and`-folded form for existing static
    /// analyses.
    pub cond_filters: Vec<(Expr, u32)>,
    /// 1-based source line of this clause's `for` / `async for` keyword (`0`
    /// when unavailable).
    pub for_lineno: u32,
    pub is_async: bool,
}

/// Comparison operators (used in chained comparisons)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
    Is,
    IsNot,
}

impl From<CmpOp> for BinaryOp {
    fn from(op: CmpOp) -> Self {
        match op {
            CmpOp::Eq => BinaryOp::Eq,
            CmpOp::Ne => BinaryOp::Ne,
            CmpOp::Lt => BinaryOp::Lt,
            CmpOp::Le => BinaryOp::Le,
            CmpOp::Gt => BinaryOp::Gt,
            CmpOp::Ge => BinaryOp::Ge,
            CmpOp::In => BinaryOp::In,
            CmpOp::NotIn => BinaryOp::NotIn,
            CmpOp::Is => BinaryOp::Is,
            CmpOp::IsNot => BinaryOp::IsNot,
        }
    }
}
