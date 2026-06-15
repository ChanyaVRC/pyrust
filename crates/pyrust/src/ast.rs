#[derive(Debug, Clone)]
pub struct FunctionParam {
    pub name: String,
    pub default: Option<Expr>,
    pub annotation: Option<Expr>,
    pub is_args: bool,            // *args
    pub is_kwargs: bool,          // **kwargs
    pub is_keyword_only: bool,    // declared after * or *args
    pub is_positional_only: bool, // declared before / separator
}

/// A single PEP 695 type parameter, e.g. `T`, `T: int`, or `T: (int, str)`.
#[derive(Debug, Clone)]
pub struct TypeParam {
    pub name: String,
    /// The bound or constraint clause following `:`, if present.
    pub bound: Option<TypeParamBound>,
}

/// The clause following the `:` in a PEP 695 type parameter.
///
/// CPython distinguishes a single *bound* (`T: int`) from a tuple of
/// *constraints* (`T: (int, str)`): the former populates `__bound__`, the
/// latter `__constraints__`.  A parenthesised expression list is treated as
/// constraints; anything else is a bound.
#[derive(Debug, Clone)]
pub enum TypeParamBound {
    /// `T: int` — a single upper bound, stored on `__bound__`.
    Bound(Expr),
    /// `T: (int, str)` — a constraint tuple, stored on `__constraints__`.
    Constraints(Vec<Expr>),
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
    /// `true` for PEP 654 `except*` handlers; `false` for ordinary `except`.
    pub is_star: bool,
    /// Per-statement 1-based line numbers for the handler body.
    /// Empty when no line info is available.
    pub body_linenos: Vec<u32>,
}

/// Assignment target (left-hand side of = or augmented =)
#[derive(Debug, Clone)]
pub enum AssignTarget {
    Name(String),
    Attr(Box<Expr>, String),
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

#[derive(Debug, Clone)]
pub enum Stmt {
    Assign(AssignTarget, Expr),
    AnnAssign {
        name: String,
        annotation: Expr,
        value: Option<Expr>,
    },
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
        /// Parallel 1-based source line numbers for each statement in `body`
        /// (same length as `body`; `0` = unknown).  Populated by the parser so
        /// the compiler can emit a `FnCode::lineno_table` for the function body,
        /// surfacing per-frame line numbers in `tb_lineno` / `f_lineno`
        /// (issues #2170/#2171).  Empty when no line info is available.
        body_linenos: Vec<u32>,
        /// 1-based source line of the `def` keyword — the function's
        /// `co_firstlineno`.  Populated by the parser; `0` when no line info is
        /// available.  Distinct from `body_linenos[0]` (the first body
        /// statement), which is one or more lines below for multi-line
        /// signatures (issue #2185).
        def_lineno: u32,
        decorators: Vec<Expr>,
        return_annotation: Option<Expr>,
        /// Whether this function was declared with `async def`.
        /// Stored for future use when async function execution is implemented;
        /// the compiler currently rejects `await` expressions regardless.
        #[allow(dead_code)]
        is_async: bool,
        /// PEP 695 type parameters from `def foo[T, U: int]():`, carrying each
        /// parameter's name and optional bound/constraint clause.
        type_params: Vec<TypeParam>,
    },
    Class {
        name: String,
        bases: Vec<Expr>,
        /// Optional metaclass specified as `metaclass=<expr>` keyword in the
        /// class header.  If present, the class object is produced by calling
        /// `metaclass(name, bases_tuple, namespace_dict)` instead of the
        /// default `type(...)` constructor.
        metaclass: Option<Expr>,
        /// PEP 487 keyword arguments in the class header other than `metaclass`.
        /// Forwarded to `__init_subclass__` of the base class.
        /// E.g. `class Foo(Base, key=val)` -> `keywords = [("key", val_expr)]`.
        keywords: Vec<(String, Expr)>,
        body: Vec<Stmt>,
        decorators: Vec<Expr>,
        /// PEP 695 type parameters from `class Foo[T, U: int]:`, carrying each
        /// parameter's name and optional bound/constraint clause.
        type_params: Vec<TypeParam>,
    },
    Global(Vec<String>),
    Nonlocal(Vec<String>),
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        else_branch: Option<Vec<Stmt>>,
        /// Per-statement 1-based line numbers for each branch body, parallel to
        /// `branches`.  `branch_linenos[i]` covers `branches[i].1`.
        /// Empty (or shorter than `branches`) when no line info is available.
        branch_linenos: Vec<Vec<u32>>,
        /// Per-statement 1-based line numbers for the else body.
        /// Empty when no line info is available or no else branch exists.
        else_linenos: Vec<u32>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
        /// Per-statement 1-based line numbers for the while body.
        body_linenos: Vec<u32>,
        /// Per-statement 1-based line numbers for the else body.
        else_linenos: Vec<u32>,
    },
    For {
        target: AssignTarget,
        iter: Expr,
        body: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
        /// Per-statement 1-based line numbers for the for body.
        body_linenos: Vec<u32>,
        /// Per-statement 1-based line numbers for the else body.
        else_linenos: Vec<u32>,
        /// Whether this is an `async for` (drives the async-iterator protocol
        /// `__aiter__` / `await __anext__()`).  Only valid inside an `async def`.
        is_async: bool,
    },
    Try {
        body: Vec<Stmt>,
        handlers: Vec<ExceptHandler>,
        else_branch: Option<Vec<Stmt>>,
        finally_branch: Option<Vec<Stmt>>,
        /// Per-statement 1-based line numbers for the try body.
        body_linenos: Vec<u32>,
        /// Per-statement 1-based line numbers for the else body.
        else_linenos: Vec<u32>,
        /// Per-statement 1-based line numbers for the finally body.
        finally_linenos: Vec<u32>,
    },
    Raise {
        expr: Option<Expr>,
        cause: Option<Expr>,
        /// PEP 657 caret anchor (issue #2411) for the whole `raise <expr>`
        /// statement (from the `raise` keyword through the end of the raised
        /// expression).  CPython underlines the entire raise statement, so this
        /// is a whole-`^` span (`full == prim`); the formatter omits it when it
        /// covers the whole dedented line (a bare `raise name` at statement
        /// scope).  `None` for a bare `raise` or when built without column info.
        span: Option<CaretSpan>,
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
        /// Per-statement 1-based line numbers for the with body.
        body_linenos: Vec<u32>,
        /// Whether this is an `async with` (drives `await __aenter__()` /
        /// `await __aexit__(...)`).  Only valid inside an `async def`.
        is_async: bool,
    },
    Match {
        subject: Expr,
        arms: Vec<MatchArm>,
    },
    /// PEP 695 type alias statement: `type <name>[T, U: int] = <value>`.
    /// Creates a `TypeAliasType` object and binds it to `name`.
    /// `type_params` holds the generic type parameters (name + optional bound).
    /// When non-empty, each param becomes a `TypeVar` bound in the scope where `value` is
    /// evaluated; the resulting alias gets a `__type_params__` attribute.
    TypeAlias {
        name: String,
        type_params: Vec<TypeParam>,
        value: Expr,
    },
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
    Capture(String),
    /// `p1 | p2` — matches if any sub-pattern matches.
    Or(Vec<Pattern>),
    /// `[a, b, *rest]` — destructures a sequence.
    /// Each element is `(pattern, is_star)`.
    Sequence(Vec<(Pattern, bool)>),
    /// `{"key": pattern, **rest}` — destructures a mapping.
    /// `rest` is the optional `**rest` capture name.
    Mapping(Vec<(Expr, Pattern)>, Option<String>),
    /// `ClassName(pos, ..., attr=pat, ...)` — class pattern.
    ///
    /// `positional` sub-patterns are resolved at runtime via `__match_args__`;
    /// `kwargs` are matched directly against the named attribute.
    Class {
        cls: Box<Expr>,
        positional: Vec<Pattern>,
        kwargs: Vec<(String, Pattern)>,
    },
    /// `a.b.c` — value pattern (dotted attribute lookup, compared with ==).
    /// Per PEP 634: a name followed by at least one `.attr` is a value pattern,
    /// not a capture.  The inner `Expr` is always an `Expr::Attr` chain.
    Value(Expr),
    /// `pattern as name` — matches `pattern` and, if it succeeds, binds the
    /// entire subject (not just the matched portion) to `name`.
    As { pattern: Box<Pattern>, name: String },
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

/// A PEP 657 fine-grained caret anchor for a multi-token expression form
/// (issue #2411): `(full_start, prim_start, prim_end, full_end)`, all 0-based
/// char columns within the expression's (single) source line, end-exclusive.
///
/// The formatter underlines `[full_start, full_end)`, drawing `^` under the
/// "primary" sub-range `[prim_start, prim_end)` and `~` under the rest:
///
/// * **Call / attribute / bare name** — `full == prim`, so the whole span is
///   `^` (and, per CPython, omitted when it covers the entire stripped line).
/// * **Binary op** — `prim` is the operator token, the operands get `~`
///   (`(10 + 2) * 3 / 0` → `~~~~~~~~~~~~~^~~`).
/// * **Subscript** — `prim` is the `[...]` part, the object gets `~`
///   (`d['a']` → `~^^^^^`).
pub type CaretSpan = (u32, u32, u32, u32);

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
    /// A bare name reference.  The optional `(col_offset, end_col_offset)`
    /// (0-based char columns within the name's source line) is the PEP 657
    /// caret anchor (issue #2426), recorded by the parser when token column
    /// information is available.  `None` for names synthesised by the parser
    /// (desugaring temporaries) or built without column info.
    Var(String, Option<(u32, u32)>),
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
    /// `(elt for target in iter ...)` — generator expression
    GenExp {
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
        /// PEP 657 caret anchor (issue #2411): the whole `callee(...)` span,
        /// underlined with `^`.  `None` for calls synthesised by the parser or
        /// built without column info.
        span: Option<CaretSpan>,
    },
    Attr {
        target: Box<Expr>,
        name: String,
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
        target: String,
        value: Box<Expr>,
    },
    /// `yield` or `yield expr` — only valid inside a generator function
    Yield(Option<Box<Expr>>),
    /// `yield from expr`
    YieldFrom(Box<Expr>),
    /// `await expr` — only valid inside an async function
    Await(Box<Expr>),
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
