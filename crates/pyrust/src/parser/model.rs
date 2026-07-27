use crate::ast::{
    AssignTarget, BinaryOp, CallArg, CmpOp, CompClause, DictItem, ExceptHandler, Expr, FStringPart,
    FunctionParam, MatchArm, Pattern, Stmt, TypeParam, TypeParamBound, UnaryOp,
};
use crate::error::{PyError, Result};
use crate::token::{FStringPart as LexFStringPart, Token};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Optional 1-based line numbers, one per token.  Empty when the parser was
    /// constructed without line tracking (via `Parser::new`).
    line_nos: Vec<u32>,
    /// Optional 0-based start columns (char offset within the token's physical
    /// line), one per token, for PEP 657 caret anchors (issue #2426).  Empty
    /// when the parser was constructed without column tracking; `Var` anchors
    /// are then recorded as `None`.
    cols: Vec<u32>,
    /// Optional 0-based end columns (char offset one past the token's last char,
    /// within its physical line), one per token, for multi-token PEP 657 caret
    /// anchors (issue #2411).  Empty when constructed without column tracking; a
    /// recorded 0 means "no reliable end col" (e.g. a token whose lexing crossed
    /// a physical newline) and suppresses the caret rather than emit a wrong one.
    cols_end: Vec<u32>,
    /// Current expression-nesting depth.  Incremented on every `parse_expr`
    /// entry; since each bracketed construct (`(`, `[`, `{`, call args,
    /// subscripts) re-enters `parse_expr` for its contents, this tracks the
    /// true nesting depth.  Bounded by [`MAX_EXPR_DEPTH`] so that pathological
    /// input (issue #2009: `eval("[" * 5000 + "1" + "]" * 5000)`) raises a
    /// catchable `SyntaxError` instead of overflowing the native stack
    /// (SIGABRT).  CPython rejects the same input with a `SyntaxError`.
    expr_depth: usize,
}

/// Maximum expression-nesting depth the parser accepts before raising
/// `SyntaxError: too many nested parentheses`.  CPython's recursive-descent
/// parser rejects bracket/paren nesting beyond depth 200; we match that so
/// programs CPython accepts continue to parse while deeper input fails with a
/// catchable error well below pyrust's native stack-overflow point.
///
/// The value is 201 rather than 200 because the outermost expression (an
/// assignment RHS or a bare `eval` expression) consumes one `parse_expr` level
/// before any bracket is opened, so a CPython-accepted bracket depth of 200
/// corresponds to `parse_expr` depth 201.
const MAX_EXPR_DEPTH: usize = 201;
