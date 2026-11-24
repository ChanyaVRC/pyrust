use super::{AssignTarget, BinaryOp, CaretSpan, Expr, FunctionParam, MatchArm, Symbol};

/// A single PEP 695 type parameter, e.g. `T`, `T: int`, or `T: (int, str)`.
#[derive(Debug, Clone)]
pub struct TypeParam {
    pub name: Symbol,
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
pub struct ExceptHandler {
    pub kind: Option<Expr>,
    pub name: Option<Symbol>,
    pub body: Vec<Stmt>,
    /// `true` for PEP 654 `except*` handlers; `false` for ordinary `except`.
    pub is_star: bool,
    /// Per-statement 1-based line numbers for the handler body.
    /// Empty when no line info is available.
    pub body_linenos: Vec<u32>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Assign(AssignTarget, Expr),
    AnnAssign {
        name: Symbol,
        annotation: Expr,
        value: Option<Expr>,
    },
    AttrAssign {
        target: Expr,
        name: Symbol,
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
        name: Symbol,
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
        name: Symbol,
        bases: Vec<Expr>,
        /// Optional metaclass specified as `metaclass=<expr>` keyword in the
        /// class header.  If present, the class object is produced by calling
        /// `metaclass(name, bases_tuple, namespace_dict)` instead of the
        /// default `type(...)` constructor.
        metaclass: Option<Expr>,
        /// PEP 487 keyword arguments in the class header other than `metaclass`.
        /// Forwarded to `__init_subclass__` of the base class.
        /// E.g. `class Foo(Base, key=val)` -> `keywords = [("key", val_expr)]`.
        keywords: Vec<(Symbol, Expr)>,
        body: Vec<Stmt>,
        decorators: Vec<Expr>,
        /// PEP 695 type parameters from `class Foo[T, U: int]:`, carrying each
        /// parameter's name and optional bound/constraint clause.
        type_params: Vec<TypeParam>,
    },
    Global(Vec<Symbol>),
    Nonlocal(Vec<Symbol>),
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
        names: Vec<(Symbol, Option<Symbol>)>,
    },
    ImportFrom {
        module: Symbol,
        names: Vec<(Symbol, Option<Symbol>)>,
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
        name: Symbol,
        type_params: Vec<TypeParam>,
        value: Expr,
    },
}

fn visit_type_param_bounds(type_params: &[TypeParam], visitor: &mut impl FnMut(&Expr)) {
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
        match self {
            Stmt::Def {
                params,
                return_annotation,
                type_params,
                ..
            } if !type_params.is_empty() => {
                visit_type_param_bounds(type_params, visitor);
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
                visit_type_param_bounds(type_params, visitor);
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
                visit_type_param_bounds(type_params, visitor);
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
        match self {
            Stmt::Def { type_params, .. } | Stmt::Class { type_params, .. } => {
                visit_type_param_bounds(type_params, visitor);
            }
            Stmt::TypeAlias {
                type_params, value, ..
            } => {
                visit_type_param_bounds(type_params, visitor);
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
