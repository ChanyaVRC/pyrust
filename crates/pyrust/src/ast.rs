#[derive(Debug, Clone)]
pub struct FunctionParam {
    pub name: String,
    pub default: Option<Expr>,
    pub is_args: bool,            // *args
    pub is_kwargs: bool,          // **kwargs
    pub is_keyword_only: bool,    // declared after * or *args
    pub is_positional_only: bool, // declared before / separator
}

#[derive(Debug, Clone)]
pub struct CallArg {
    pub name: Option<String>,
    pub value: Expr,
    pub splat: bool,        // *expr
    pub double_splat: bool, // **expr
}

#[derive(Debug, Clone)]
pub struct ExceptHandler {
    pub kind: Option<Expr>,
    pub name: Option<String>,
    pub body: Vec<Stmt>,
}

/// Assignment target (left-hand side of = or augmented =)
#[derive(Debug, Clone)]
pub enum AssignTarget {
    Name(String),
    Attr(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// Unpack: a, b, c = ...
    Tuple(Vec<AssignTarget>),
    /// Starred target inside a Tuple: *name or *_ — only valid inside Tuple
    Starred(Box<AssignTarget>),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Assign(AssignTarget, Expr),
    AttrAssign {
        target: Expr,
        name: String,
        expr: Expr,
    },
    IndexAssign {
        target: Box<Expr>,
        index: Box<Expr>,
        expr: Expr,
    },
    SliceAssign {
        target: Box<Expr>,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
        expr: Expr,
    },
    AugAssign {
        target: AssignTarget,
        op: BinaryOp,
        expr: Expr,
    },
    Expr(Expr),
    Def {
        name: String,
        params: Vec<FunctionParam>,
        body: Vec<Stmt>,
        decorators: Vec<Expr>,
    },
    Class {
        name: String,
        bases: Vec<Expr>,
        /// Optional metaclass specified as `metaclass=<expr>` keyword in the
        /// class header.  If present, the class object is produced by calling
        /// `metaclass(name, bases_tuple, namespace_dict)` instead of the
        /// default `type(...)` constructor.
        metaclass: Option<Expr>,
        body: Vec<Stmt>,
        decorators: Vec<Expr>,
    },
    Global(Vec<String>),
    Nonlocal(Vec<String>),
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        else_branch: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
    },
    For {
        target: AssignTarget,
        iter: Expr,
        body: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
    },
    Try {
        body: Vec<Stmt>,
        handlers: Vec<ExceptHandler>,
        else_branch: Option<Vec<Stmt>>,
        finally_branch: Option<Vec<Stmt>>,
    },
    Raise {
        expr: Option<Expr>,
        cause: Option<Expr>,
    },
    Import {
        names: Vec<(String, Option<String>)>,
    },
    ImportFrom {
        module: String,
        names: Vec<(String, Option<String>)>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    Pass,
    Delete(Vec<Expr>),
    Assert {
        test: Expr,
        msg: Option<Expr>,
    },
    With {
        items: Vec<(Expr, Option<AssignTarget>)>,
        body: Vec<Stmt>,
    },
    Match {
        subject: Expr,
        arms: Vec<MatchArm>,
    },
}

/// A single `case <pattern> [if <guard>]: <body>` arm in a `match` statement.
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
}

/// Pattern variants for structural pattern matching (PEP 634).
#[derive(Debug, Clone)]
pub enum Pattern {
    /// `_` — always matches, no binding.
    Wildcard,
    /// `42`, `"str"`, `True`, `None` — equality test.
    Literal(Expr),
    /// `x` — binds the matched value to a name.
    Capture(String),
    /// `p1 | p2` — matches if any sub-pattern matches.
    Or(Vec<Pattern>),
    /// `[a, b, *rest]` — destructures a sequence.
    /// Each element is `(pattern, is_star)`.
    Sequence(Vec<(Pattern, bool)>),
    /// `{"key": pattern, **rest}` — destructures a mapping.
    /// `rest` is the optional `**rest` capture name.
    Mapping(Vec<(Expr, Pattern)>, Option<String>),
    /// `ClassName(attr=pat, ...)` — class pattern.
    Class {
        cls: Box<Expr>,
        kwargs: Vec<(String, Pattern)>,
    },
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
    Expr {
        expr: Box<Expr>,
        conversion: Option<char>,
        format_spec: Option<Vec<FStringPart>>,
        debug_text: Option<String>,
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
    /// An f-string: `f"Hello, {name}!"`
    FString(Vec<FStringPart>),
    Var(String),
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
    },
    /// `{key: val for target in iter ...}`
    DictComp {
        key: Box<Expr>,
        val: Box<Expr>,
        clauses: Vec<CompClause>,
    },
    /// `{elt for target in iter ...}`
    SetComp {
        elt: Box<Expr>,
        clauses: Vec<CompClause>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
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
        params: Vec<String>,
        body: Box<Expr>,
    },
    Call {
        func: Box<Expr>,
        args: Vec<CallArg>,
    },
    Attr {
        target: Box<Expr>,
        name: String,
    },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    Slice {
        target: Box<Expr>,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
    /// Walrus operator: `target := value`
    Named {
        target: String,
        value: Box<Expr>,
    },
    /// `yield` or `yield expr` — only valid inside a generator function
    Yield(Option<Box<Expr>>),
    /// `yield from expr`
    YieldFrom(Box<Expr>),
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
#[derive(Debug, Clone)]
pub struct CompClause {
    pub target: AssignTarget,
    pub iter: Expr,
    pub cond: Option<Expr>,
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
