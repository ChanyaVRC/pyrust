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
    /// Attribute target `obj.attr`.  The third field is the PEP 657 caret
    /// anchor (issue #2442) for the whole `obj.attr` span — used to paint the
    /// caret when an augmented attribute assignment (`obj.attr += ...`) raises
    /// `AttributeError` on the read or store.  `None` when built without column
    /// info.
    Attr(Box<Expr>, String, Option<CaretSpan>),
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
        /// PEP 657 caret anchor (issue #2442): the whole `obj.attr` span of the
        /// assignment *target*, underlined with `^` (full == prim).  Used to
        /// paint the caret when the `SetAttr` raises `AttributeError` (e.g.
        /// `x.foo = 1` on an object that rejects the attribute), matching
        /// CPython 3.12.  `None` for targets synthesised by the parser or built
        /// without column info.
        span: Option<CaretSpan>,
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
    /// When non-empty, each param becomes a `TypeVar` bound in the alias's
    /// annotation scope; the lazy `value` evaluator captures that scope and the
    /// resulting alias gets a `__type_params__` attribute.
    TypeAlias {
        name: String,
        type_params: Vec<TypeParam>,
        value: Expr,
    },
}

impl Stmt {
    /// Visit expressions evaluated directly in the statement's enclosing
    /// lexical scope.
    ///
    /// Nested statement blocks and function/class bodies are intentionally not
    /// descended into: callers own the scope transition before visiting those.
    ///
    /// PEP 695 introduces an important second boundary: type-parameter bounds,
    /// generic annotations/bases and type-alias values resolve in a dedicated
    /// annotation scope rather than this enclosing scope. Those expressions
    /// are deliberately excluded here and exposed by
    /// [`Stmt::visit_type_parameter_scope_exprs`]. This distinction matters to
    /// declaration ordering (`global x` must not see a lazy `T: x` as a prior
    /// use), closure capture and side-effect timing.
    pub(crate) fn visit_evaluated_exprs(&self, visitor: &mut impl FnMut(&Expr)) {
        match self {
            // Plain assignment evaluates the RHS before any store-target
            // receiver/key. Augmented assignment must read its target first.
            Stmt::Assign(target, value) => {
                visitor(value);
                target.visit_evaluated_exprs(visitor);
            }
            Stmt::AugAssign {
                target,
                expr: value,
                ..
            } => {
                target.visit_evaluated_exprs(visitor);
                visitor(value);
            }
            Stmt::AnnAssign {
                annotation, value, ..
            } => {
                if let Some(value) = value {
                    visitor(value);
                }
                visitor(annotation);
            }
            Stmt::AttrAssign { target, expr, .. } => {
                visitor(expr);
                visitor(target);
            }
            Stmt::IndexAssign {
                target,
                index,
                expr,
            } => {
                visitor(expr);
                visitor(target);
                visitor(index);
            }
            Stmt::SliceAssign {
                target,
                lower,
                upper,
                step,
                expr,
            } => {
                visitor(expr);
                visitor(target);
                for bound in [lower, upper, step].into_iter().flatten() {
                    visitor(bound);
                }
            }
            Stmt::Expr(expr) => visitor(expr),
            Stmt::Def {
                params,
                decorators,
                return_annotation,
                type_params,
                ..
            } => {
                // Decorators are evaluated first, in source order, before the
                // definition's defaults/annotations are materialized.
                for decorator in decorators {
                    visitor(decorator);
                }
                for parameter in params {
                    if let Some(default) = &parameter.default {
                        visitor(default);
                    }
                    if type_params.is_empty()
                        && let Some(annotation) = &parameter.annotation
                    {
                        visitor(annotation);
                    }
                }
                if type_params.is_empty()
                    && let Some(annotation) = return_annotation
                {
                    visitor(annotation);
                }
            }
            Stmt::Class {
                bases,
                metaclass,
                keywords,
                decorators,
                type_params,
                ..
            } => {
                for decorator in decorators {
                    visitor(decorator);
                }
                if type_params.is_empty() {
                    for base in bases {
                        visitor(base);
                    }
                    if let Some(metaclass) = metaclass {
                        visitor(metaclass);
                    }
                    for (_, value) in keywords {
                        visitor(value);
                    }
                }
            }
            Stmt::If { branches, .. } => {
                for (condition, _) in branches {
                    visitor(condition);
                }
            }
            Stmt::While { cond, .. } => visitor(cond),
            Stmt::For { target, iter, .. } => {
                visitor(iter);
                target.visit_evaluated_exprs(visitor);
            }
            Stmt::Try { handlers, .. } => {
                for handler in handlers {
                    if let Some(kind) = &handler.kind {
                        visitor(kind);
                    }
                }
            }
            Stmt::Raise { expr, cause, .. } => {
                if let Some(expr) = expr {
                    visitor(expr);
                }
                if let Some(cause) = cause {
                    visitor(cause);
                }
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    visitor(value);
                }
            }
            Stmt::Delete(targets) => {
                for target in targets {
                    visitor(target);
                }
            }
            Stmt::Assert { test, msg } => {
                visitor(test);
                if let Some(msg) = msg {
                    visitor(msg);
                }
            }
            Stmt::With { items, .. } => {
                for (context, target) in items {
                    visitor(context);
                    if let Some(target) = target {
                        target.visit_evaluated_exprs(visitor);
                    }
                }
            }
            Stmt::Match { subject, arms } => {
                visitor(subject);
                for arm in arms {
                    arm.pattern.visit_evaluated_exprs(visitor);
                    if let Some(guard) = &arm.guard {
                        visitor(guard);
                    }
                }
            }
            // A PEP 695 alias value is evaluated lazily in its annotation
            // scope when `__value__` is first read.
            Stmt::TypeAlias { .. } => {}
            Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Import { .. }
            | Stmt::ImportFrom { .. }
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Pass => {}
        }
    }

    /// Visit expressions owned by a PEP 695 type-parameter annotation scope.
    ///
    /// These names are semantically outside the enclosing statement scope even
    /// when code generation happens at definition time (generic annotations,
    /// bases and keywords). Bounds/constraints and type-alias values are a
    /// further deferred subset exposed by
    /// [`Stmt::visit_deferred_annotation_exprs`].
    pub(crate) fn visit_type_parameter_scope_exprs(&self, visitor: &mut impl FnMut(&Expr)) {
        fn visit_type_params(type_params: &[TypeParam], visitor: &mut impl FnMut(&Expr)) {
            for parameter in type_params {
                match &parameter.bound {
                    Some(TypeParamBound::Bound(bound)) => visitor(bound),
                    Some(TypeParamBound::Constraints(constraints)) => {
                        for constraint in constraints {
                            visitor(constraint);
                        }
                    }
                    None => {}
                }
            }
        }

        match self {
            Stmt::Def {
                params,
                return_annotation,
                type_params,
                ..
            } if !type_params.is_empty() => {
                visit_type_params(type_params, visitor);
                for parameter in params {
                    if let Some(annotation) = &parameter.annotation {
                        visitor(annotation);
                    }
                }
                if let Some(annotation) = return_annotation {
                    visitor(annotation);
                }
            }
            Stmt::Class {
                bases,
                metaclass,
                keywords,
                type_params,
                ..
            } if !type_params.is_empty() => {
                visit_type_params(type_params, visitor);
                for base in bases {
                    visitor(base);
                }
                if let Some(metaclass) = metaclass {
                    visitor(metaclass);
                }
                for (_, value) in keywords {
                    visitor(value);
                }
            }
            Stmt::TypeAlias {
                type_params, value, ..
            } => {
                visit_type_params(type_params, visitor);
                visitor(value);
            }
            _ => {}
        }
    }

    /// Visit PEP 695 expressions compiled as zero-argument lazy thunks.
    ///
    /// Unlike generic annotations/bases, these expressions must capture
    /// enclosing fastlocals and must not run until the corresponding
    /// `__bound__`, `__constraints__` or `__value__` attribute is read.
    pub(crate) fn visit_deferred_annotation_exprs(&self, visitor: &mut impl FnMut(&Expr)) {
        fn visit_type_params(type_params: &[TypeParam], visitor: &mut impl FnMut(&Expr)) {
            for parameter in type_params {
                match &parameter.bound {
                    Some(TypeParamBound::Bound(bound)) => visitor(bound),
                    Some(TypeParamBound::Constraints(constraints)) => {
                        for constraint in constraints {
                            visitor(constraint);
                        }
                    }
                    None => {}
                }
            }
        }

        match self {
            Stmt::Def { type_params, .. } | Stmt::Class { type_params, .. } => {
                visit_type_params(type_params, visitor);
            }
            Stmt::TypeAlias {
                type_params, value, ..
            } => {
                visit_type_params(type_params, visitor);
                visitor(value);
            }
            _ => {}
        }
    }

    /// Visit every expression whose name dependencies are owned by this
    /// statement rather than by a nested function/class body.
    ///
    /// This is for free-variable/cell dependency analysis only. Syntax and
    /// declaration-order checks should select the narrower visitor matching
    /// their evaluation boundary.
    pub(crate) fn visit_scope_dependency_exprs(&self, visitor: &mut impl FnMut(&Expr)) {
        self.visit_evaluated_exprs(visitor);
        self.visit_type_parameter_scope_exprs(visitor);
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
///
/// ## Multi-line sentinel (issue #2571)
///
/// A binary expression whose operands straddle physical lines can't be
/// expressed as a single-line column span (the operator / right-operand columns
/// live on a later line than the displayed source line).  In that case the
/// parser records `prim_end == full_end == [`MULTILINE_FULL_END`]` and the
/// formatter clamps the underline to the end of the displayed line, drawing
/// solid `^` from `full_start` — matching CPython 3.12.
pub type CaretSpan = (u32, u32, u32, u32);

/// Sentinel `full_end` (and `prim_end`) marking a multi-line binary-op caret
/// span (issue #2571): the formatter clamps the underline to the end of the
/// displayed source line and draws solid `^` from `full_start`.  `u32::MAX` is
/// safe — no real source line is this long.
pub const MULTILINE_FULL_END: u32 = u32::MAX;

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
    Var(String, Option<(u32, u32, u32)>),
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
        /// PEP 657 caret anchor (issue #2411): the whole `callee(...)` span,
        /// underlined with `^`.  `None` for calls synthesised by the parser or
        /// built without column info.
        span: Option<CaretSpan>,
    },
    Attr {
        target: Box<Expr>,
        name: String,
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
            Expr::ListComp { elt, clauses }
            | Expr::SetComp { elt, clauses }
            | Expr::GenExp { elt, clauses } => {
                elt.visit_enclosing_walrus_targets(visitor);
                for clause in clauses {
                    clause.iter.visit_enclosing_walrus_targets(visitor);
                    if let Some(condition) = &clause.cond {
                        condition.visit_enclosing_walrus_targets(visitor);
                    }
                }
            }
            Expr::DictComp { key, val, clauses } => {
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
            | Expr::YieldFrom(expr)
            | Expr::Await(expr) => expr.visit_enclosing_walrus_targets(visitor),
            Expr::Yield(expr) => {
                if let Some(expr) = expr {
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
