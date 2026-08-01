use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use smallvec::SmallVec;

use crate::ast::{
    AssignTarget, BinaryOp, CallArg, CompClause, DictItem, Expr, FStringPart, FunctionParam,
    MatchArm, Pattern, Stmt, TypeParam, TypeParamBound, UnaryOp,
};
use crate::bytecode::{
    AttrCacheEntry, BinOpCacheEntry, CellVar, FnCode, FnParamSpec, FnProto, GlobalCacheEntry, Insn,
    KwCallCacheEntry, MAX_FRAME_REGS, Reg,
};
use crate::error::PyError;
use crate::interpreter::dispatch_numeric_binop;
use crate::value::{PyBigInt, Value, ValueKind};

/// Compile a top-level script / module body.  All script-level names are
/// locals.
///
/// When `repl_mode` is true, top-level `Stmt::Expr` statements emit
/// `Insn::PrintExpr` instead of discarding the result.
///
/// `linenos` is an optional parallel slice of 1-based source line numbers for
/// each top-level statement in `stmts`.  Pass an empty slice when no line
/// information is available (the resulting `FnCode::lineno_table` will be all
/// zeros).  They are threaded into the bytecode `lineno_table` so tracebacks
/// report accurate lines.
pub fn compile_script_with_linenos(
    stmts: &[Stmt],
    local_index: Rc<HashMap<String, Reg>>,
    repl_mode: bool,
    linenos: &[u32],
    filename: &str,
) -> Result<FnCode, PyError> {
    // Validate global/nonlocal ordering at module scope.  CPython 3.12 raises
    // SyntaxError for `x = 1; global x` at module level too.
    if let Some(msg) = crate::interpreter::check_global_nonlocal_order(stmts) {
        return Err(PyError::Named("SyntaxError".into(), msg));
    }
    // Script-level code cannot have nonlocal, and nothing captures script
    // locals via nonlocal from a nested scope at this level.
    let cell_vars = collect_cell_vars(stmts, &local_index);
    let mut c = Compiler::new(local_index, 0, cell_vars);
    // Threaded source file (#2438): the module's code object and every nested
    // function/class it compiles report this path as their `co_filename`.
    c.filename = std::sync::Arc::from(filename);
    // Issue #820: module-scope stores emit SyncModuleGlobal to keep
    // module_globals_dict live after globals() has been called.
    c.is_module_scope = true;
    c.module_namespace_may_be_exposed = module_namespace_may_be_exposed(stmts);
    // Issue #711: if the first statement is a bare string literal and we are
    // compiling a script file (not the REPL), it is the module docstring.
    // Emit a StoreGlobal for `__doc__` (CPython parity) before compiling the
    // rest.  In REPL mode every string-expression is just a value expression
    // whose repr is printed; it is NOT a module docstring (CPython's interactive
    // console does not set __doc__ from string literals typed interactively).
    let (body, body_linenos): (&[Stmt], &[u32]) = if !repl_mode {
        match stmts {
            [Stmt::Expr(Expr::Str(s)), rest @ ..] => {
                // Record lineno of the docstring statement if available.
                if let Some(&ln) = linenos.first()
                    && ln != 0
                {
                    c.set_lineno(ln);
                }
                let r = c.compile_literal(Value::string(s.clone()));
                c.compile_store_name("__doc__", r);
                c.free_temp(r);
                (rest, linenos.get(1..).unwrap_or(&[]))
            }
            _ => (stmts, linenos),
        }
    } else {
        (stmts, linenos)
    };
    if repl_mode {
        for (idx, stmt) in body.iter().enumerate() {
            if let Some(&ln) = body_linenos.get(idx)
                && ln != 0
            {
                c.set_lineno(ln);
            }
            if let Stmt::Expr(e) = stmt {
                let r = c.compile_expr(e);
                c.emit(Insn::PrintExpr(r));
                c.free_temp(r);
            } else {
                c.compile_stmt(stmt);
            }
        }
    } else {
        c.compile_block_with_linenos(body, body_linenos);
    }
    c.finish()
}

/// Compile a source-backed module whose Python-visible globals dictionary is
/// the sole live namespace while its body executes.
///
/// Unlike [`compile_script_with_linenos`], this deliberately allocates no
/// module fast-local registers: every module name is lowered through
/// `LoadGlobal` / `StoreGlobal`. A circular import can therefore mutate the
/// partially initialized module through `module.attr` and the suspended body
/// observes that same value when it resumes. Keep ordinary scripts and
/// `exec()` on the fast-local constructor; this shared-namespace mode is for
/// module bodies that are externally reachable before execution completes.
pub fn compile_shared_namespace_module_with_linenos(
    stmts: &[Stmt],
    linenos: &[u32],
    filename: &str,
) -> Result<FnCode, PyError> {
    compile_script_with_linenos(stmts, Rc::new(HashMap::new()), false, linenos, filename)
}

/// Compile a source body in eval mode: the body must consist of a single
/// expression statement; its value is returned.  Used by `eval()`.
///
/// Raises `SyntaxError` if the body is empty or contains statements that are
/// not a bare expression.
/// Seeds the expression's source line number into the bytecode line table, so
/// an error raised while evaluating an `eval()`'d expression reports the
/// correct internal line (issue #2245).
pub fn compile_eval_expr_with_linenos(
    stmts: &[Stmt],
    local_index: Rc<HashMap<String, Reg>>,
    linenos: &[u32],
    filename: &str,
) -> Result<FnCode, PyError> {
    let expr = match stmts {
        [Stmt::Expr(e)] => e,
        [] => {
            return Err(PyError::named(
                "SyntaxError",
                "eval() requires a non-empty expression".to_string(),
            ));
        }
        _ => {
            return Err(PyError::named(
                "SyntaxError",
                "eval() argument must be a single expression".to_string(),
            ));
        }
    };
    let cell_vars = collect_cell_vars(stmts, &local_index);
    let mut c = Compiler::new(local_index, 0, cell_vars);
    // Threaded source file (#2438): the eval expression's `co_filename`.
    c.filename = std::sync::Arc::from(filename);
    if let Some(&ln) = linenos.first()
        && ln != 0
    {
        c.set_lineno(ln);
    }
    let r = c.compile_expr(expr);
    let r = c.ensure_temp(r);
    c.emit(Insn::Return(r));
    c.free_temp(r);
    c.finish()
}
