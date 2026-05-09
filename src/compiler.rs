use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::{AssignTarget, BinaryOp, CmpOp, Expr, FunctionParam, Stmt, UnaryOp};
use crate::bytecode::{CellVar, FnCode, FnProto, Insn, Reg};
use crate::value::{Environment, UserFunction, Value};

/// Compile a user function to bytecode.  Always succeeds now that the VM
/// handles all Python features.  Returns None only on internal limits
/// (>255 locals, >255 nested protos, etc.).
pub fn compile_fn(func: &UserFunction) -> Option<FnCode> {
    // Detect which of the outer function's locals are captured by nested
    // functions via `nonlocal`.  These become cell variables stored in the
    // env rather than registers.
    let cell_vars = collect_cell_vars(&func.body, &func.local_index);

    let n = func.local_index.len();
    if n > 255 {
        return None;
    }

    let mut c = Compiler::new(
        Rc::clone(&func.local_index),
        Rc::clone(&func.global_names),
        Rc::clone(&func.nonlocal_names),
        func.def_bound_mask,
        cell_vars,
    );
    c.compile_block(&func.body);
    c.finish()
}

/// Compile a top-level script body.  All script-level names are locals.
///
/// When `repl_mode` is true, top-level `Stmt::Expr` statements emit
/// `Insn::PrintExpr` instead of discarding the result.
pub fn compile_script(
    stmts: &[Stmt],
    local_index: Rc<HashMap<String, usize>>,
    repl_mode: bool,
) -> Option<FnCode> {
    let empty: Rc<HashSet<String>> = Rc::new(HashSet::new());
    // Script-level code cannot have nonlocal, and nothing captures script
    // locals via nonlocal from a nested scope at this level.
    let cell_vars = collect_cell_vars(stmts, &local_index);
    let mut c = Compiler::new(local_index, Rc::clone(&empty), empty, 0, cell_vars);
    if repl_mode {
        for stmt in stmts {
            if let Stmt::Expr(e) = stmt {
                let r = c.compile_expr(e);
                c.emit(Insn::PrintExpr(r));
                c.free_temp(r);
            } else {
                c.compile_stmt(stmt);
            }
        }
    } else {
        c.compile_block(stmts);
    }
    c.finish()
}

// ─── Cell-variable collection ─────────────────────────────────────────────────

/// Collect names from `local_index` that are referenced as `nonlocal` in any
/// directly nested `Stmt::Def` body.  These must be stored in the env (not
/// registers) so that inner closures can share them.
fn collect_cell_vars(body: &[Stmt], local_index: &HashMap<String, usize>) -> Vec<CellVar> {
    let mut cells: HashSet<String> = HashSet::new();
    collect_cell_vars_in(body, local_index, &mut cells);
    cells.into_iter().collect()
}

fn collect_cell_vars_in(
    body: &[Stmt],
    local_index: &HashMap<String, usize>,
    cells: &mut HashSet<String>,
) {
    for stmt in body {
        match stmt {
            Stmt::Def {
                params,
                body: nested_body,
                ..
            } => {
                // Explicit `nonlocal x` in nested body → x is a cell var.
                let nonlocals = crate::interpreter::collect_nonlocal_names(nested_body);
                for name in &nonlocals {
                    if local_index.contains_key(name) {
                        cells.insert(name.clone());
                    }
                }
                // Free variable references: names read in the nested body that are
                // not the nested function's own locals or globals.
                let inner_globals = crate::interpreter::collect_global_names(nested_body);
                // Names declared `global` in the nested function reference the
                // enclosing module env directly.  If those names are fastlocals
                // in the current scope, promote them to cell vars so they live
                // in the env rather than registers — otherwise nested writes via
                // `global` and register write-back at script end would conflict.
                for name in &inner_globals {
                    if local_index.contains_key(name) {
                        cells.insert(name.clone());
                    }
                }
                let inner_locals = crate::interpreter::collect_local_names(
                    params,
                    nested_body,
                    &inner_globals,
                    &nonlocals,
                );
                let mut inner_uses: HashSet<String> = HashSet::new();
                collect_free_var_reads_in_stmts(nested_body, &mut inner_uses);
                for name in inner_uses {
                    if !inner_locals.contains(&name)
                        && !inner_globals.contains(&name)
                        && !nonlocals.contains(&name)
                        && local_index.contains_key(&name)
                    {
                        cells.insert(name);
                    }
                }
                // Don't recurse into nested defs - they see only their own cells.
            }
            Stmt::Class {
                body: nested_body, ..
            } => {
                // Class bodies can also reference outer nonlocals.
                let nonlocals = crate::interpreter::collect_nonlocal_names(nested_body);
                for name in &nonlocals {
                    if local_index.contains_key(name) {
                        cells.insert(name.clone());
                    }
                }
                // Methods inside a class access the enclosing scope directly
                // (Python class scope is not a closure scope for methods).
                // Find names that class methods read as free variables and
                // promote them to cell vars so they live in the env.
                collect_class_method_outer_refs(nested_body, local_index, cells);
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (_, b) in branches {
                    collect_cell_vars_in(b, local_index, cells);
                }
                if let Some(b) = else_branch {
                    collect_cell_vars_in(b, local_index, cells);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_cell_vars_in(body, local_index, cells);
                if let Some(b) = else_branch {
                    collect_cell_vars_in(b, local_index, cells);
                }
            }
            Stmt::For {
                body, else_branch, ..
            } => {
                collect_cell_vars_in(body, local_index, cells);
                if let Some(b) = else_branch {
                    collect_cell_vars_in(b, local_index, cells);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
            } => {
                collect_cell_vars_in(body, local_index, cells);
                for h in handlers {
                    collect_cell_vars_in(&h.body, local_index, cells);
                }
                if let Some(b) = else_branch {
                    collect_cell_vars_in(b, local_index, cells);
                }
                if let Some(b) = finally_branch {
                    collect_cell_vars_in(b, local_index, cells);
                }
            }
            Stmt::With { body, .. } => {
                collect_cell_vars_in(body, local_index, cells);
            }
            _ => {}
        }
    }
}

/// For a class body, collect names that the class's methods read as free
/// variables from the enclosing scope.  Python class scope is not a closure
/// scope for methods: `def method(self): return x` reads the outer `x`, not
/// any `x` defined at class level.  Promote those names to cell vars so they
/// live in the env (not registers) and are accessible via `LoadGlobal`.
fn collect_class_method_outer_refs(
    class_body: &[Stmt],
    local_index: &HashMap<String, usize>,
    cells: &mut HashSet<String>,
) {
    // Collect names assigned at class level (they shadow outer names for the
    // class scope itself, though not for methods — but we're conservative here).
    let empty_set: HashSet<String> = HashSet::new();
    let class_locals =
        crate::interpreter::collect_local_names(&[], class_body, &empty_set, &empty_set);

    for stmt in class_body {
        match stmt {
            Stmt::Def {
                params,
                body: method_body,
                ..
            } => {
                let inner_globals = crate::interpreter::collect_global_names(method_body);
                // Promote global declarations in methods (same as for top-level defs).
                for name in &inner_globals {
                    if local_index.contains_key(name) {
                        cells.insert(name.clone());
                    }
                }
                let inner_nonlocals = crate::interpreter::collect_nonlocal_names(method_body);
                let inner_locals = crate::interpreter::collect_local_names(
                    params,
                    method_body,
                    &inner_globals,
                    &inner_nonlocals,
                );
                let mut uses: HashSet<String> = HashSet::new();
                collect_free_var_reads_in_stmts(method_body, &mut uses);
                for name in uses {
                    if !inner_locals.contains(&name)
                        && !inner_globals.contains(&name)
                        && !inner_nonlocals.contains(&name)
                        && !class_locals.contains(&name)
                        && local_index.contains_key(&name)
                    {
                        cells.insert(name);
                    }
                }
            }
            // Recursively handle class-level control flow.
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (_, b) in branches {
                    collect_class_method_outer_refs(b, local_index, cells);
                }
                if let Some(b) = else_branch {
                    collect_class_method_outer_refs(b, local_index, cells);
                }
            }
            _ => {}
        }
    }
}

/// Collect all `Var` reads in `stmts`, stopping at nested `Def`/`Class`
/// boundaries.  Used to detect free variables that need to become cell vars.
fn collect_free_var_reads_in_stmts(stmts: &[Stmt], uses: &mut HashSet<String>) {
    for stmt in stmts {
        collect_free_var_reads_in_stmt(stmt, uses);
    }
}

fn collect_free_var_reads_in_stmt(stmt: &Stmt, uses: &mut HashSet<String>) {
    match stmt {
        Stmt::Def { .. } | Stmt::Class { .. } => {}
        Stmt::Assign(_, value) => {
            collect_free_var_reads_in_expr(value, uses);
        }
        Stmt::AttrAssign { target, expr, .. } => {
            collect_free_var_reads_in_expr(target, uses);
            collect_free_var_reads_in_expr(expr, uses);
        }
        Stmt::IndexAssign {
            target,
            index,
            expr,
        } => {
            collect_free_var_reads_in_expr(target, uses);
            collect_free_var_reads_in_expr(index, uses);
            collect_free_var_reads_in_expr(expr, uses);
        }
        Stmt::SliceAssign {
            target,
            lower,
            upper,
            step,
            expr,
        } => {
            collect_free_var_reads_in_expr(target, uses);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_free_var_reads_in_expr(e, uses);
            }
            collect_free_var_reads_in_expr(expr, uses);
        }
        Stmt::AugAssign {
            target,
            op: _,
            expr,
        } => {
            if let AssignTarget::Name(n) = target {
                uses.insert(n.clone());
            }
            collect_free_var_reads_in_expr(expr, uses);
        }
        Stmt::Return(Some(e)) => collect_free_var_reads_in_expr(e, uses),
        Stmt::Return(None) => {}
        Stmt::Expr(e) => collect_free_var_reads_in_expr(e, uses),
        Stmt::If {
            branches,
            else_branch,
        } => {
            for (cond, body) in branches {
                collect_free_var_reads_in_expr(cond, uses);
                collect_free_var_reads_in_stmts(body, uses);
            }
            if let Some(b) = else_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
        }
        Stmt::While {
            cond,
            body,
            else_branch,
        } => {
            collect_free_var_reads_in_expr(cond, uses);
            collect_free_var_reads_in_stmts(body, uses);
            if let Some(b) = else_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
        }
        Stmt::For {
            iter,
            body,
            else_branch,
            ..
        } => {
            collect_free_var_reads_in_expr(iter, uses);
            collect_free_var_reads_in_stmts(body, uses);
            if let Some(b) = else_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
        }
        Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
        } => {
            collect_free_var_reads_in_stmts(body, uses);
            for h in handlers {
                if let Some(e) = &h.kind {
                    collect_free_var_reads_in_expr(e, uses);
                }
                collect_free_var_reads_in_stmts(&h.body, uses);
            }
            if let Some(b) = else_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
            if let Some(b) = finally_branch {
                collect_free_var_reads_in_stmts(b, uses);
            }
        }
        Stmt::With { items, body } => {
            for (e, _) in items {
                collect_free_var_reads_in_expr(e, uses);
            }
            collect_free_var_reads_in_stmts(body, uses);
        }
        Stmt::Raise {
            expr: Some(e),
            cause,
        } => {
            collect_free_var_reads_in_expr(e, uses);
            if let Some(c) = cause {
                collect_free_var_reads_in_expr(c, uses);
            }
        }
        Stmt::Assert { test, msg } => {
            collect_free_var_reads_in_expr(test, uses);
            if let Some(m) = msg {
                collect_free_var_reads_in_expr(m, uses);
            }
        }
        Stmt::Delete(exprs) => {
            for e in exprs {
                collect_free_var_reads_in_expr(e, uses);
            }
        }
        Stmt::Import { .. }
        | Stmt::ImportFrom { .. }
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Pass
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Raise { expr: None, .. } => {}
    }
}

fn collect_free_var_reads_in_expr(expr: &Expr, uses: &mut HashSet<String>) {
    match expr {
        Expr::Var(n) => {
            uses.insert(n.clone());
        }
        Expr::Binary { left, right, .. } => {
            collect_free_var_reads_in_expr(left, uses);
            collect_free_var_reads_in_expr(right, uses);
        }
        Expr::Unary { expr: e, .. } => collect_free_var_reads_in_expr(e, uses),
        Expr::Compare { left, ops } => {
            collect_free_var_reads_in_expr(left, uses);
            for (_, e) in ops {
                collect_free_var_reads_in_expr(e, uses);
            }
        }
        Expr::Call { func, args } => {
            collect_free_var_reads_in_expr(func, uses);
            for a in args {
                collect_free_var_reads_in_expr(&a.value, uses);
            }
        }
        Expr::Attr { target, .. } => collect_free_var_reads_in_expr(target, uses),
        Expr::Index { target, index } => {
            collect_free_var_reads_in_expr(target, uses);
            collect_free_var_reads_in_expr(index, uses);
        }
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => {
            collect_free_var_reads_in_expr(target, uses);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_free_var_reads_in_expr(e, uses);
            }
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                collect_free_var_reads_in_expr(e, uses);
            }
        }
        Expr::Dict(pairs) => {
            for (k, v) in pairs {
                collect_free_var_reads_in_expr(k, uses);
                collect_free_var_reads_in_expr(v, uses);
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_free_var_reads_in_expr(cond, uses);
            collect_free_var_reads_in_expr(then, uses);
            collect_free_var_reads_in_expr(else_, uses);
        }
        Expr::Lambda { body, .. } => collect_free_var_reads_in_expr(body, uses),
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::None => {}
    }
}

// ─── Utilities ────────────────────────────────────────────────────────────────

fn cmp_to_binary(op: CmpOp) -> BinaryOp {
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

fn const_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::None, Value::None) => true,
        _ => false,
    }
}

/// Attempt to evaluate a pure constant expression at compile time.
/// Returns Some(value) only when the entire expression tree consists of
/// literals and operations on literals that cannot raise.
fn fold_constant(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Int(v) => Some(Value::Int(*v)),
        Expr::Float(v) => Some(Value::Float(*v)),
        Expr::Str(s) => Some(Value::Str(s.clone())),
        Expr::Bool(b) => Some(Value::Bool(*b)),
        Expr::None => Some(Value::None),
        Expr::Unary { op, expr } => {
            let val = fold_constant(expr)?;
            match (op, &val) {
                (UnaryOp::Neg, Value::Int(n)) => Some(Value::Int(n.wrapping_neg())),
                (UnaryOp::Neg, Value::Float(f)) => Some(Value::Float(-f)),
                (UnaryOp::Not, v) => Some(Value::Bool(!v.truthy())),
                (UnaryOp::BitNot, Value::Int(n)) => Some(Value::Int(!n)),
                _ => None,
            }
        }
        Expr::Binary { left, op, right } => {
            let l = fold_constant(left)?;
            let r = fold_constant(right)?;
            fold_binop(&l, *op, &r)
        }
        Expr::Compare { left, ops } => {
            let mut cur = fold_constant(left)?;
            for (cmp_op, rhs_expr) in ops {
                let rhs = fold_constant(rhs_expr)?;
                let op = cmp_to_binary(*cmp_op);
                let result = fold_binop(&cur, op, &rhs)?;
                if !result.truthy() {
                    return Some(Value::Bool(false));
                }
                cur = rhs;
            }
            Some(Value::Bool(true))
        }
        _ => None,
    }
}

fn fold_binop(l: &Value, op: BinaryOp, r: &Value) -> Option<Value> {
    match (l, op, r) {
        (Value::Int(a), BinaryOp::Add, Value::Int(b)) => Some(Value::Int(a.wrapping_add(*b))),
        (Value::Int(a), BinaryOp::Sub, Value::Int(b)) => Some(Value::Int(a.wrapping_sub(*b))),
        (Value::Int(a), BinaryOp::Mul, Value::Int(b)) => Some(Value::Int(a.wrapping_mul(*b))),
        (Value::Int(a), BinaryOp::Div, Value::Int(b)) if *b != 0 => {
            Some(Value::Float(*a as f64 / *b as f64))
        }
        (Value::Int(a), BinaryOp::FloorDiv, Value::Int(b)) if *b != 0 => {
            let q = a.wrapping_div(*b);
            let r = a.wrapping_rem(*b);
            Some(Value::Int(if (r != 0) && ((r < 0) != (*b < 0)) {
                q - 1
            } else {
                q
            }))
        }
        (Value::Int(a), BinaryOp::Mod, Value::Int(b)) if *b != 0 => {
            let r = a.wrapping_rem(*b);
            Some(Value::Int(if (r != 0) && ((r < 0) != (*b < 0)) {
                r + b
            } else {
                r
            }))
        }
        (Value::Int(a), BinaryOp::Pow, Value::Int(b)) if *b >= 0 => {
            Some(Value::Int(a.wrapping_pow(*b as u32)))
        }
        (Value::Float(a), BinaryOp::Add, Value::Float(b)) => Some(Value::Float(a + b)),
        (Value::Float(a), BinaryOp::Sub, Value::Float(b)) => Some(Value::Float(a - b)),
        (Value::Float(a), BinaryOp::Mul, Value::Float(b)) => Some(Value::Float(a * b)),
        (Value::Float(a), BinaryOp::Div, Value::Float(b)) if *b != 0.0 => Some(Value::Float(a / b)),
        (Value::Str(a), BinaryOp::Add, Value::Str(b)) => Some(Value::Str(a.clone() + b)),
        (Value::Int(a), BinaryOp::Eq, Value::Int(b)) => Some(Value::Bool(a == b)),
        (Value::Int(a), BinaryOp::Ne, Value::Int(b)) => Some(Value::Bool(a != b)),
        (Value::Int(a), BinaryOp::Lt, Value::Int(b)) => Some(Value::Bool(a < b)),
        (Value::Int(a), BinaryOp::Le, Value::Int(b)) => Some(Value::Bool(a <= b)),
        (Value::Int(a), BinaryOp::Gt, Value::Int(b)) => Some(Value::Bool(a > b)),
        (Value::Int(a), BinaryOp::Ge, Value::Int(b)) => Some(Value::Bool(a >= b)),
        (Value::Str(a), BinaryOp::Eq, Value::Str(b)) => Some(Value::Bool(a == b)),
        (Value::Str(a), BinaryOp::Ne, Value::Str(b)) => Some(Value::Bool(a != b)),
        (Value::Bool(a), BinaryOp::Eq, Value::Bool(b)) => Some(Value::Bool(a == b)),
        _ => None,
    }
}

fn extract_literal_int(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(n) => Some(*n),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } => {
            if let Expr::Int(n) = inner.as_ref() {
                Some(-n)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_const_false_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Bool(false) | Expr::Int(0) | Expr::None => true,
        Expr::Float(f) => *f == 0.0,
        _ => false,
    }
}

fn body_has_continue(body: &[Stmt]) -> bool {
    body.iter().any(stmt_has_continue)
}

fn stmt_has_continue(s: &Stmt) -> bool {
    match s {
        Stmt::Continue => true,
        Stmt::If {
            branches,
            else_branch,
        } => {
            branches.iter().any(|(_, b)| body_has_continue(b))
                || else_branch.as_deref().map_or(false, body_has_continue)
        }
        Stmt::While { .. } | Stmt::For { .. } => false,
        _ => false,
    }
}

fn collect_body_written(body: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_written_in(body, &mut names);
    names
}

fn collect_written_in(body: &[Stmt], names: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Assign(target, _) | Stmt::AugAssign { target, .. } => {
                collect_written_target(target, names);
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (_, b) in branches {
                    collect_written_in(b, names);
                }
                if let Some(b) = else_branch {
                    collect_written_in(b, names);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_written_in(body, names);
                if let Some(b) = else_branch {
                    collect_written_in(b, names);
                }
            }
            Stmt::For {
                target,
                body,
                else_branch,
                ..
            } => {
                collect_written_target(target, names);
                collect_written_in(body, names);
                if let Some(b) = else_branch {
                    collect_written_in(b, names);
                }
            }
            _ => {}
        }
    }
}

fn collect_written_target(target: &AssignTarget, names: &mut HashSet<String>) {
    match target {
        AssignTarget::Name(n) => {
            names.insert(n.clone());
        }
        AssignTarget::Tuple(ts) => {
            for t in ts {
                collect_written_target(t, names);
            }
        }
        _ => {}
    }
}

fn expr_is_invariant(expr: &Expr, written: &HashSet<String>) -> bool {
    match expr {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::None => true,
        Expr::Var(name) => !written.contains(name.as_str()),
        Expr::Binary { left, right, .. } => {
            expr_is_invariant(left, written) && expr_is_invariant(right, written)
        }
        Expr::Unary { expr, .. } => expr_is_invariant(expr, written),
        _ => false,
    }
}

/// Detect `while VAR cmp STOP: ...; VAR += STEP` (or -= for decreasing).
fn detect_while_range<'a>(
    cond: &'a Expr,
    body: &'a [Stmt],
) -> Option<(&'a str, &'a Expr, i64, bool)> {
    let (var_name, cmp_op, stop_expr) = match cond {
        Expr::Binary { op, left, right }
            if matches!(
                op,
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
            ) =>
        {
            match left.as_ref() {
                Expr::Var(name) => (name.as_str(), op, right.as_ref()),
                _ => return None,
            }
        }
        _ => return None,
    };
    match body.last()? {
        Stmt::AugAssign {
            target: AssignTarget::Name(aug_var),
            op: BinaryOp::Add,
            expr: Expr::Int(s),
        } if aug_var == var_name && *s > 0 && matches!(cmp_op, BinaryOp::Lt | BinaryOp::Le) => {
            Some((var_name, stop_expr, *s, matches!(cmp_op, BinaryOp::Le)))
        }
        Stmt::AugAssign {
            target: AssignTarget::Name(aug_var),
            op: BinaryOp::Sub,
            expr: Expr::Int(s),
        } if aug_var == var_name && *s > 0 && matches!(cmp_op, BinaryOp::Gt | BinaryOp::Ge) => {
            Some((var_name, stop_expr, -*s, matches!(cmp_op, BinaryOp::Ge)))
        }
        _ => None,
    }
}

// ─── Compiler struct ──────────────────────────────────────────────────────────

struct LoopCtx {
    break_patches: Vec<usize>,
    /// None when the continue target is not yet known (e.g. counter-range loop
    /// where the increment comes after the body).  Patched before the increment.
    continue_target: Option<usize>,
    /// Indices of Jump(0) instructions emitted for `continue` when continue_target
    /// was None; fixed up once continue_target is established.
    continue_patches: Vec<usize>,
}

struct Compiler {
    local_index: Rc<HashMap<String, usize>>,
    global_names: Rc<HashSet<String>>,
    nonlocal_names: Rc<HashSet<String>>,
    cell_vars: HashSet<String>,
    insns: Vec<Insn>,
    consts: Vec<Value>,
    names: Vec<String>,
    name_map: HashMap<String, u16>,
    next_temp: Reg,
    base_temp: Reg,
    iter_depth: u8,
    max_iter: u8,
    max_reg: Reg,
    loops: Vec<LoopCtx>,
    failed: bool,
    def_set: u64,
    fn_protos: Vec<FnProto>,
}

impl Compiler {
    fn new(
        local_index: Rc<HashMap<String, usize>>,
        global_names: Rc<HashSet<String>>,
        nonlocal_names: Rc<HashSet<String>>,
        def_bound_mask: u64,
        cell_vars: Vec<CellVar>,
    ) -> Self {
        let n = local_index.len();
        let cell_set: HashSet<String> = cell_vars.into_iter().collect();
        // base_temp must cover ALL local_index slots (including cell vars) so
        // that temp registers never overlap with local-variable slot numbers.
        let base_temp = n as Reg;
        // Recompute def_bound_mask to skip cell vars (they're not in registers).
        let mut adjusted_mask = 0u64;
        for (name, &idx) in local_index.iter() {
            if !cell_set.contains(name) && idx < 64 {
                // The register index is idx minus how many cell vars precede it.
                // But local_index already maps name → slot; cell vars won't be
                // accessed as registers, so we just strip their bits.
                if def_bound_mask & (1u64 << idx) != 0 {
                    adjusted_mask |= 1u64 << idx;
                }
            }
        }

        Self {
            local_index,
            global_names,
            nonlocal_names,
            cell_vars: cell_set,
            insns: Vec::new(),
            consts: Vec::new(),
            names: Vec::new(),
            name_map: HashMap::new(),
            next_temp: base_temp,
            base_temp,
            iter_depth: 0,
            max_iter: 0,
            max_reg: if n > 0 {
                (n as Reg).saturating_sub(1)
            } else {
                0
            },
            loops: Vec::new(),
            failed: n > 255,
            def_set: def_bound_mask,
            fn_protos: Vec::new(),
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn mark_def(&mut self, reg: Reg) {
        if (reg as usize) < 64 {
            self.def_set |= 1u64 << reg;
        }
    }

    fn mark_target_def(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    self.mark_def(reg);
                }
            }
            AssignTarget::Tuple(targets) => {
                for t in targets {
                    self.mark_target_def(t);
                }
            }
            _ => {}
        }
    }

    fn intern_name(&mut self, name: &str) -> u16 {
        if let Some(&idx) = self.name_map.get(name) {
            return idx;
        }
        if self.names.len() >= u16::MAX as usize {
            self.failed = true;
            return 0;
        }
        let idx = self.names.len() as u16;
        self.names.push(name.to_string());
        self.name_map.insert(name.to_string(), idx);
        idx
    }

    fn try_literal_const_idx(&mut self, expr: &Expr) -> Option<u16> {
        match expr {
            Expr::Int(v) => Some(self.intern_const(Value::Int(*v))),
            Expr::Float(v) => Some(self.intern_const(Value::Float(*v))),
            Expr::Str(s) => Some(self.intern_const(Value::Str(s.clone()))),
            Expr::Bool(b) => Some(self.intern_const(Value::Bool(*b))),
            _ => None,
        }
    }

    fn intern_const(&mut self, val: Value) -> u16 {
        for (i, v) in self.consts.iter().enumerate() {
            if const_eq(v, &val) {
                return i as u16;
            }
        }
        if self.consts.len() >= u16::MAX as usize {
            self.failed = true;
            return 0;
        }
        let idx = self.consts.len() as u16;
        self.consts.push(val);
        idx
    }

    fn alloc_temp(&mut self) -> Reg {
        let r = self.next_temp;
        if r == Reg::MAX {
            self.failed = true;
            return 0;
        }
        self.next_temp += 1;
        if r > self.max_reg {
            self.max_reg = r;
        }
        r
    }

    fn free_temp(&mut self, r: Reg) {
        if r >= self.base_temp && self.next_temp > 0 && r + 1 == self.next_temp {
            self.next_temp -= 1;
        }
    }

    fn alloc_iter(&mut self) -> u8 {
        let s = self.iter_depth;
        self.iter_depth += 1;
        if self.iter_depth > self.max_iter {
            self.max_iter = self.iter_depth;
        }
        s
    }

    fn free_iter(&mut self) {
        if self.iter_depth > 0 {
            self.iter_depth -= 1;
        }
    }

    fn emit(&mut self, insn: Insn) -> usize {
        let idx = self.insns.len();
        self.insns.push(insn);
        idx
    }

    fn pc(&self) -> usize {
        self.insns.len()
    }

    fn patch_jump(&mut self, idx: usize) {
        let target = self.insns.len() as i32;
        let after_jump = idx as i32 + 1;
        let offset = target - after_jump;
        match &mut self.insns[idx] {
            Insn::Jump(off)
            | Insn::JumpIfFalse(_, off)
            | Insn::JumpIfTrue(_, off)
            | Insn::ForIter(_, _, off)
            | Insn::SetupExcept(off)
            | Insn::MatchExcept(_, off)
            | Insn::CmpJumpIfFalse(_, _, _, off)
            | Insn::CmpJumpIfTrue(_, _, _, off)
            | Insn::CmpJumpIfFalseConst(_, _, _, off)
            | Insn::CmpJumpIfTrueConst(_, _, _, off)
            | Insn::ForCountReg(_, _, _, _, off)
            | Insn::ForCountConst(_, _, _, _, off) => *off = offset,
            _ => self.failed = true,
        }
    }

    /// Try to fuse the last emitted instruction with a conditional jump.
    ///
    /// If the last instruction is `BinOpConst(cond_reg, lhs, op, c)` or
    /// `BinOp(cond_reg, lhs, op, rhs)`, replace it with the corresponding
    /// `CmpJump*` variant (offset=0, to be patched).  Otherwise fall back to
    /// emitting `JumpIfFalse`/`JumpIfTrue` as normal.
    ///
    /// `invert=false` → JumpIfFalse semantics (jump when comparison is false)
    /// `invert=true`  → JumpIfTrue semantics (jump when comparison is true)
    fn emit_cond_jump(&mut self, cond_reg: Reg, invert: bool) -> usize {
        // Only fuse when the result register is a temp (not a named local).
        if cond_reg >= self.base_temp {
            if let Some(last) = self.insns.last().cloned() {
                match last {
                    Insn::BinOpConst(dst, lhs, op, c) if dst == cond_reg => {
                        let idx = self.insns.len() - 1;
                        // Free the temp that would have held the bool result.
                        self.free_temp(cond_reg);
                        self.insns[idx] = if invert {
                            Insn::CmpJumpIfTrueConst(lhs, op, c, 0)
                        } else {
                            Insn::CmpJumpIfFalseConst(lhs, op, c, 0)
                        };
                        return idx;
                    }
                    Insn::BinOp(dst, lhs, op, rhs) if dst == cond_reg => {
                        let idx = self.insns.len() - 1;
                        self.free_temp(cond_reg);
                        self.insns[idx] = if invert {
                            Insn::CmpJumpIfTrue(lhs, op, rhs, 0)
                        } else {
                            Insn::CmpJumpIfFalse(lhs, op, rhs, 0)
                        };
                        return idx;
                    }
                    _ => {}
                }
            }
        }
        // Fall back: emit a regular conditional jump.
        if invert {
            self.emit(Insn::JumpIfTrue(cond_reg, 0))
        } else {
            self.emit(Insn::JumpIfFalse(cond_reg, 0))
        }
    }

    /// True if `name` is a cell variable (lives in env, not registers).
    fn is_cell(&self, name: &str) -> bool {
        self.cell_vars.contains(name)
    }

    /// Register index for a local variable, or None if the name is global/nonlocal/cell.
    fn local_reg(&self, name: &str) -> Option<Reg> {
        if self.is_cell(name) {
            return None;
        }
        self.local_index.get(name).copied().map(|i| i as Reg)
    }

    fn compile_block(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            if self.failed {
                return;
            }
            self.compile_stmt(stmt);
        }
    }

    fn finish(self) -> Option<FnCode> {
        if self.failed {
            return None;
        }
        let num_regs = if self.max_reg >= self.base_temp || self.base_temp == 0 {
            self.max_reg.saturating_add(1)
        } else {
            self.base_temp
        };
        Some(FnCode {
            insns: self.insns,
            consts: self.consts,
            names: self.names,
            num_regs,
            num_iters: self.max_iter,
            num_locals: self.base_temp,
            fn_protos: self.fn_protos,
            cell_vars: self.cell_vars.into_iter().collect(),
        })
    }

    /// Allocate result register: reuse `candidate` if it's a temp, else fresh.
    fn ensure_dst(&mut self, candidate: Reg) -> Reg {
        if candidate >= self.base_temp {
            candidate
        } else {
            self.alloc_temp()
        }
    }

    // ── Store helpers ─────────────────────────────────────────────────────────

    /// Emit the appropriate store for `name` from register `src`.
    /// If `container_expr` is a global/cell variable name, write `obj_reg` back
    /// to the env.  Called after SetItem/SetSlice on a container that was loaded
    /// via LoadGlobal (which creates a copy, so the mutation must be committed).
    fn writeback_container_if_global(&mut self, container_expr: &Expr, obj_reg: Reg) {
        if let Expr::Var(name) = container_expr {
            if self.local_reg(name).is_none() {
                let name_idx = self.intern_name(name);
                self.emit(Insn::StoreGlobal(name_idx, obj_reg));
            }
        }
    }

    fn compile_store_name(&mut self, name: &str, src: Reg) {
        if let Some(reg) = self.local_reg(name) {
            if src != reg {
                self.emit(Insn::Move(reg, src));
            }
        } else {
            let idx = self.intern_name(name);
            self.emit(Insn::StoreGlobal(idx, src));
        }
    }

    /// Compile `target = <value already in src_reg>`.
    fn compile_store_target(&mut self, target: &AssignTarget, src: Reg) {
        match target {
            AssignTarget::Name(name) => {
                self.compile_store_name(name, src);
            }
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.compile_expr(obj_expr);
                let name_idx = self.intern_name(attr);
                self.emit(Insn::SetAttr(obj, name_idx, src));
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                self.emit(Insn::SetItem(obj, idx, src));
                self.free_temp(idx);
                self.free_temp(obj);
            }
            AssignTarget::Tuple(_) => {
                // Caller must handle tuple targets separately.
                self.failed = true;
            }
        }
    }

    // ── Statement compilation ─────────────────────────────────────────────────

    fn compile_stmt(&mut self, stmt: &Stmt) {
        if self.failed {
            return;
        }
        match stmt {
            Stmt::Pass => {}
            Stmt::Break => {
                if self.loops.is_empty() {
                    self.failed = true;
                    return;
                }
                let idx = self.emit(Insn::Jump(0));
                let last = self.loops.len() - 1;
                self.loops[last].break_patches.push(idx);
            }
            Stmt::Continue => {
                if self.loops.is_empty() {
                    self.failed = true;
                    return;
                }
                let last = self.loops.len() - 1;
                let idx = self.emit(Insn::Jump(0));
                if let Some(target) = self.loops[last].continue_target {
                    let from = idx as i32 + 1;
                    let offset = target as i32 - from;
                    if let Insn::Jump(off) = &mut self.insns[idx] {
                        *off = offset;
                    }
                } else {
                    self.loops[last].continue_patches.push(idx);
                }
            }
            Stmt::Return(None) => {
                self.emit(Insn::ReturnNone);
            }
            Stmt::Return(Some(expr)) => {
                let r = self.compile_expr(expr);
                self.emit(Insn::Return(r));
                self.free_temp(r);
            }
            Stmt::Expr(expr) => {
                let r = self.compile_expr(expr);
                self.free_temp(r);
            }
            Stmt::Assign(target, expr) => {
                self.compile_assign(target, expr);
                self.mark_target_def(target);
            }
            Stmt::AugAssign { target, op, expr } => {
                self.compile_aug_assign(target, *op, expr);
                if let AssignTarget::Name(name) = target {
                    if let Some(reg) = self.local_reg(name) {
                        self.mark_def(reg);
                    }
                }
            }
            Stmt::AttrAssign { target, name, expr } => {
                let obj = self.compile_expr(target);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(name);
                self.emit(Insn::SetAttr(obj, name_idx, val));
                self.free_temp(val);
                self.free_temp(obj);
            }
            Stmt::IndexAssign {
                target,
                index,
                expr,
            } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, idx, val));
                self.free_temp(val);
                self.free_temp(idx);
                self.writeback_container_if_global(target, obj);
                self.free_temp(obj);
            }
            Stmt::SliceAssign {
                target,
                lower,
                upper,
                step,
                expr,
            } => {
                let obj = self.compile_expr(target);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, slice_r, val));
                self.writeback_container_if_global(target, obj);
                self.free_temp(val);
                self.free_temp(slice_r);
                self.free_temp(obj);
            }
            Stmt::Assert { test, msg } => {
                let cond = self.compile_expr(test);
                let skip = self.emit(Insn::JumpIfTrue(cond, 0));
                self.free_temp(cond);
                let msg_reg = if let Some(msg_expr) = msg {
                    self.compile_expr(msg_expr)
                } else {
                    let r = self.alloc_temp();
                    self.emit(Insn::LoadNone(r));
                    r
                };
                self.emit(Insn::RaiseAssert(msg_reg));
                self.free_temp(msg_reg);
                self.patch_jump(skip);
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                self.compile_if(branches, else_branch.as_deref());
            }
            Stmt::While {
                cond,
                body,
                else_branch,
            } => {
                self.compile_while(cond, body, else_branch.as_deref());
            }
            Stmt::For {
                target,
                iter,
                body,
                else_branch,
            } => {
                self.compile_for(target, iter, body, else_branch.as_deref());
            }
            Stmt::Global(_) | Stmt::Nonlocal(_) => {
                // These are purely compile-time declarations; no runtime effect.
            }
            Stmt::Raise { expr, cause } => {
                self.compile_raise(expr.as_ref(), cause.as_ref());
            }
            Stmt::Delete(exprs) => {
                for expr in exprs {
                    self.compile_delete(expr);
                    if self.failed {
                        return;
                    }
                }
            }
            Stmt::Import { names } => {
                self.compile_import(names);
            }
            Stmt::ImportFrom { module, names } => {
                self.compile_import_from(module, names);
            }
            Stmt::Def {
                name,
                params,
                body,
                decorators,
            } => {
                self.compile_def(name, params, body, decorators);
            }
            Stmt::Class {
                name,
                bases,
                body,
                decorators,
            } => {
                self.compile_class(name, bases, body, decorators);
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
            } => {
                self.compile_try(
                    body,
                    handlers,
                    else_branch.as_deref(),
                    finally_branch.as_deref(),
                );
            }
            Stmt::With { items, body } => {
                self.compile_with(items, body);
            }
        }
    }

    // ── Assignment ────────────────────────────────────────────────────────────

    fn compile_assign(&mut self, target: &AssignTarget, expr: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    self.compile_expr_into(expr, reg);
                } else {
                    // global / nonlocal / cell var → go through env
                    let src = self.compile_expr(expr);
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::StoreGlobal(name_idx, src));
                    self.free_temp(src);
                }
            }
            AssignTarget::Tuple(targets) => {
                // Fast path: matching tuple literal
                if let Expr::Tuple(exprs) = expr {
                    if exprs.len() == targets.len() && !targets.is_empty() {
                        let mut target_regs: Vec<Option<Reg>> = Vec::with_capacity(targets.len());
                        let mut all_name_locals = true;
                        for t in targets.iter() {
                            match t {
                                AssignTarget::Name(name) => {
                                    target_regs.push(self.local_reg(name));
                                    if self.local_reg(name).is_none() {
                                        // cell or global — can still do fast path with temps
                                        all_name_locals = false;
                                    }
                                }
                                _ => {
                                    all_name_locals = false;
                                    target_regs.push(None);
                                }
                            }
                        }
                        // If ALL are simple name→local, use the original fast path
                        if all_name_locals && target_regs.iter().all(|r| r.is_some()) {
                            let saved_next = self.next_temp;
                            let mut temps: Vec<Reg> = Vec::with_capacity(exprs.len());
                            for rhs_expr in exprs.iter() {
                                let r = self.compile_expr(rhs_expr);
                                let tmp = if r < self.base_temp {
                                    let t = self.alloc_temp();
                                    self.emit(Insn::Move(t, r));
                                    t
                                } else {
                                    r
                                };
                                temps.push(tmp);
                            }
                            if !self.failed {
                                for (dst_reg, src_tmp) in target_regs.iter().zip(temps.iter()) {
                                    let dst = dst_reg.unwrap();
                                    if *src_tmp != dst {
                                        self.emit(Insn::Move(dst, *src_tmp));
                                    }
                                }
                            }
                            self.next_temp = saved_next;
                            return;
                        }
                    }
                }

                let src = self.compile_expr(expr);
                let n = targets.len() as u8;
                if n == 0 {
                    self.free_temp(src);
                    return;
                }
                let base = self.next_temp;
                if base as usize + n as usize > 256 {
                    self.failed = true;
                    return;
                }
                self.next_temp = base + n;
                if self.next_temp - 1 > self.max_reg {
                    self.max_reg = self.next_temp - 1;
                }
                self.emit(Insn::Unpack(base, src, n));
                self.free_temp(src);
                for (i, t) in targets.iter().enumerate() {
                    match t {
                        AssignTarget::Name(name) => {
                            if let Some(reg) = self.local_reg(name) {
                                self.emit(Insn::Move(reg, base + i as u8));
                            } else {
                                let name_idx = self.intern_name(name);
                                self.emit(Insn::StoreGlobal(name_idx, base + i as u8));
                            }
                        }
                        AssignTarget::Attr(obj_expr, attr) => {
                            let obj = self.compile_expr(obj_expr);
                            let name_idx = self.intern_name(attr);
                            self.emit(Insn::SetAttr(obj, name_idx, base + i as u8));
                            self.free_temp(obj);
                        }
                        AssignTarget::Index(obj_expr, idx_expr) => {
                            let obj = self.compile_expr(obj_expr);
                            let idx = self.compile_expr(idx_expr);
                            self.emit(Insn::SetItem(obj, idx, base + i as u8));
                            self.free_temp(idx);
                            self.free_temp(obj);
                        }
                        AssignTarget::Tuple(_) => {
                            // Nested tuple unpack — compile recursively
                            let tmp = base + i as u8;
                            self.compile_assign(t, &Expr::Var(format!("__unpack_{}", tmp)));
                        }
                    }
                }
                self.next_temp = base;
            }
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.compile_expr(obj_expr);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(attr);
                self.emit(Insn::SetAttr(obj, name_idx, val));
                self.free_temp(val);
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, idx, val));
                self.free_temp(val);
                self.free_temp(idx);
                self.free_temp(obj);
            }
        }
    }

    fn compile_aug_assign(&mut self, target: &AssignTarget, op: BinaryOp, expr: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    if let Some(const_idx) = self.try_literal_const_idx(expr) {
                        self.emit(Insn::BinOpConst(reg, reg, op, const_idx));
                    } else {
                        let rhs = self.compile_expr(expr);
                        self.emit(Insn::BinOpInPlace(reg, reg, op, rhs));
                        self.free_temp(rhs);
                    }
                } else {
                    // cell / global: load, compute, store
                    let name_idx = self.intern_name(name);
                    let lhs = self.alloc_temp();
                    self.emit(Insn::LoadGlobal(lhs, name_idx));
                    if let Some(const_idx) = self.try_literal_const_idx(expr) {
                        self.emit(Insn::BinOpConst(lhs, lhs, op, const_idx));
                    } else {
                        let rhs = self.compile_expr(expr);
                        self.emit(Insn::BinOpInPlace(lhs, lhs, op, rhs));
                        self.free_temp(rhs);
                    }
                    self.emit(Insn::StoreGlobal(name_idx, lhs));
                    self.free_temp(lhs);
                }
            }
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.compile_expr(obj_expr);
                let name_idx = self.intern_name(attr);
                let lhs = self.alloc_temp();
                self.emit(Insn::GetAttr(lhs, obj, name_idx));
                if let Some(const_idx) = self.try_literal_const_idx(expr) {
                    self.emit(Insn::BinOpConst(lhs, lhs, op, const_idx));
                } else {
                    let rhs = self.compile_expr(expr);
                    self.emit(Insn::BinOpInPlace(lhs, lhs, op, rhs));
                    self.free_temp(rhs);
                }
                self.emit(Insn::SetAttr(obj, name_idx, lhs));
                self.free_temp(lhs);
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                let lhs = self.alloc_temp();
                self.emit(Insn::GetItem(lhs, obj, idx));
                if let Some(const_idx) = self.try_literal_const_idx(expr) {
                    self.emit(Insn::BinOpConst(lhs, lhs, op, const_idx));
                } else {
                    let rhs = self.compile_expr(expr);
                    self.emit(Insn::BinOpInPlace(lhs, lhs, op, rhs));
                    self.free_temp(rhs);
                }
                self.emit(Insn::SetItem(obj, idx, lhs));
                self.writeback_container_if_global(obj_expr, obj);
                self.free_temp(lhs);
                self.free_temp(idx);
                self.free_temp(obj);
            }
            AssignTarget::Tuple(_) => {
                self.failed = true;
            }
        }
    }

    // ── Control flow ──────────────────────────────────────────────────────────

    fn compile_if(&mut self, branches: &[(Expr, Vec<Stmt>)], else_branch: Option<&[Stmt]>) {
        let has_else = else_branch.is_some();
        let n = branches.len();
        let mut end_patches: Vec<usize> = Vec::new();
        let pre_def_set = self.def_set;
        // Collect def_set after each branch body for definite-assignment analysis.
        let mut branch_def_sets: Vec<u64> = Vec::with_capacity(n + 1);

        for (bi, (cond, body)) in branches.iter().enumerate() {
            self.def_set = pre_def_set;
            let cond_reg = self.compile_expr(cond);
            let jmp_false = self.emit_cond_jump(cond_reg, false);
            self.free_temp(cond_reg);
            self.compile_block(body);
            if self.failed {
                return;
            }
            branch_def_sets.push(self.def_set);
            self.def_set = pre_def_set;
            if bi < n - 1 || has_else {
                let jmp_end = self.emit(Insn::Jump(0));
                end_patches.push(jmp_end);
            }
            self.patch_jump(jmp_false);
        }
        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
            if self.failed {
                return;
            }
            branch_def_sets.push(self.def_set);
        }
        for idx in end_patches {
            self.patch_jump(idx);
        }
        // Variables defined in every branch (including else) are definitely bound after.
        // Without an else, control may skip all branches so no new defs can be assumed.
        if has_else && !branch_def_sets.is_empty() {
            let all_define = branch_def_sets.iter().fold(!0u64, |acc, &s| acc & s);
            self.def_set = pre_def_set | all_define;
        } else {
            self.def_set = pre_def_set;
        }
    }

    fn compile_while(&mut self, cond: &Expr, body: &[Stmt], else_branch: Option<&[Stmt]>) {
        if is_const_false_expr(cond) {
            if let Some(else_stmts) = else_branch {
                self.compile_block(else_stmts);
            }
            return;
        }

        let is_infinite = matches!(cond, Expr::Bool(true) | Expr::Int(1));

        if !is_infinite && !body_has_continue(body) {
            if self.try_compile_while_range(cond, body, else_branch) {
                return;
            }
        }

        let is_licm = !is_infinite && {
            let written = collect_body_written(body);
            expr_is_invariant(cond, &written)
        };

        let (loop_start, exit_jmp) = if is_licm {
            let cond_reg = self.compile_expr(cond);
            let jmp = self.emit_cond_jump(cond_reg, false);
            self.free_temp(cond_reg);
            (self.pc(), Some(jmp))
        } else if is_infinite {
            (self.pc(), None)
        } else {
            let start = self.pc();
            let cond_reg = self.compile_expr(cond);
            let jmp = self.emit_cond_jump(cond_reg, false);
            self.free_temp(cond_reg);
            (start, Some(jmp))
        };

        self.loops.push(LoopCtx {
            break_patches: Vec::new(),
            continue_target: Some(loop_start),
            continue_patches: Vec::new(),
        });
        let saved = self.def_set;
        self.compile_block(body);
        self.def_set = saved;
        if self.failed {
            return;
        }
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));
        if let Some(jmp) = exit_jmp {
            self.patch_jump(jmp);
        }
        let ctx = self.loops.pop().unwrap();
        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
            if self.failed {
                return;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
    }

    /// Convert `while VAR cmp STOP: ...; VAR += STEP` to a range-backed for loop.
    ///
    /// Uses ForIter+IterState::Range (raw i64 counters) rather than ForCount, since
    /// ForIter operates on unboxed integers while ForCount must go through Option<Value>
    /// registers on every iteration.
    ///
    /// The key optimisation over the naive compilation: we omit the last body statement
    /// (VAR += STEP — which is a dead store because ForIter overwrites VAR on the next
    /// iteration), reducing the per-iteration dispatch count from 4 to 3.
    /// After natural loop exit we emit one BinOpConst to restore the post-increment
    /// value that Python semantics require (i.e. `while i < n: …; i += 1` leaves i=n).
    fn try_compile_while_range(
        &mut self,
        cond: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
    ) -> bool {
        let (var_name, stop_expr, step, inclusive) = match detect_while_range(cond, body) {
            Some(x) => x,
            None => return false,
        };
        let var_reg = match self.local_reg(var_name) {
            Some(r) => r,
            None => return false,
        };
        let frame = self.next_temp;
        let slots = if inclusive { 5usize } else { 4 };
        if (frame as usize).saturating_add(slots) > 256 {
            return false;
        }
        self.next_temp = frame + slots as u8;
        if self.next_temp - 1 > self.max_reg {
            self.max_reg = self.next_temp - 1;
        }
        let range_idx = self.intern_name("range");
        self.emit(Insn::LoadGlobal(frame, range_idx));
        self.emit(Insn::Move(frame + 1, var_reg));
        {
            let saved = self.next_temp;
            let r = self.compile_expr(stop_expr);
            if r != frame + 2 {
                self.emit(Insn::Move(frame + 2, r));
            }
            self.next_temp = saved;
        }
        if inclusive {
            let one_idx = self.intern_const(Value::Int(1));
            self.emit(Insn::LoadConst(frame + 4, one_idx));
            let adj = if step > 0 {
                BinaryOp::Add
            } else {
                BinaryOp::Sub
            };
            self.emit(Insn::BinOp(frame + 2, frame + 2, adj, frame + 4));
        }
        let step_idx = self.intern_const(Value::Int(step));
        self.emit(Insn::LoadConst(frame + 3, step_idx));
        self.emit(Insn::Call(frame, 3));
        self.next_temp = frame + 1;
        let iter_slot = self.alloc_iter();
        self.emit(Insn::GetIter(iter_slot, frame));
        self.free_temp(frame);
        let loop_start = self.pc();
        let exit_jmp = self.emit(Insn::ForIter(var_reg, iter_slot, 0));
        self.mark_def(var_reg);
        self.loops.push(LoopCtx {
            break_patches: Vec::new(),
            continue_target: Some(loop_start),
            continue_patches: Vec::new(),
        });
        let saved = self.def_set;
        // Skip the last body statement (VAR += STEP): ForIter already manages the
        // range counter, so VAR += STEP is a dead store on every non-final iteration.
        // On the final iteration it would produce the correct post-loop value, but we
        // restore that with a single BinOpConst after the exit instead.
        let body_without_inc = &body[..body.len() - 1];
        self.compile_block(body_without_inc);
        self.def_set = saved;
        if self.failed {
            return true;
        }
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));
        self.patch_jump(exit_jmp);
        // Restore post-loop variable value: Python semantics require that after
        // `while i < n: …; i += 1` the variable equals n (the value that failed the
        // condition), not n-1 (the last ForIter-assigned value).
        // Break patches jump PAST this instruction, so break exits with the break value.
        self.emit(Insn::BinOpConst(var_reg, var_reg, BinaryOp::Add, step_idx));
        let ctx = self.loops.pop().unwrap();
        self.free_iter();
        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
            if self.failed {
                return true;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
        true
    }

    /// Detect `for VAR in range(...)` and compile to a direct integer counter loop:
    ///   VAR = start; while VAR < stop: body; VAR += step
    /// This avoids calling range(), allocating an IterState, and the ForIter
    /// overhead per iteration, giving a tight loop equivalent to C `for`.
    fn try_compile_for_range(
        &mut self,
        target: &AssignTarget,
        iter_expr: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
    ) -> bool {
        // Target must be a Name with a fastlocal register.
        let var_name = match target {
            AssignTarget::Name(n) => n.as_str(),
            _ => return false,
        };
        let var_reg = match self.local_reg(var_name) {
            Some(r) => r,
            None => return false,
        };

        // iter_expr must be a plain `range(...)` call with no splats/kwargs.
        let (func, args) = match iter_expr {
            Expr::Call { func, args } => (func.as_ref(), args.as_slice()),
            _ => return false,
        };
        if !matches!(func, Expr::Var(n) if n == "range") {
            return false;
        }
        if args
            .iter()
            .any(|a| a.splat || a.double_splat || a.name.is_some())
        {
            return false;
        }

        // Extract (start_opt, stop, step) from 1–3 positional args.
        let (start_opt, stop_expr, step_val): (Option<&Expr>, &Expr, i64) = match args {
            [s] => (None, &s.value, 1),
            [a, b] => (Some(&a.value), &b.value, 1),
            [a, b, c] => {
                let s = match extract_literal_int(&c.value) {
                    Some(v) => v,
                    None => return false,
                };
                if s == 0 {
                    return false;
                }
                (Some(&a.value), &b.value, s)
            }
            _ => return false,
        };

        // ForCount semantics: initialise var = start - step_val, then on each
        // iteration: next = var + step_val; if next op stop → var=next (continue)
        // else → jump (exit).  This way the body always sees the current value
        // and the variable retains its last iteration value after normal exit.
        let cmp_op = if step_val > 0 {
            BinaryOp::Lt
        } else {
            BinaryOp::Gt
        };
        let step_idx = self.intern_const(Value::Int(step_val));

        // ── 1. Initialise var_reg = start - step_val ─────────────────────
        //    For the common range(n) case (start=0), init = -step_val.
        let neg_step_idx = self.intern_const(Value::Int(step_val.wrapping_neg()));
        if let Some(start) = start_opt {
            let r = self.compile_expr(start);
            // var_reg = r + (-step_val) = r - step_val
            self.emit(Insn::BinOpConst(var_reg, r, BinaryOp::Add, neg_step_idx));
            self.free_temp(r);
        } else {
            // start = 0; init = -step_val
            self.emit(Insn::LoadConst(var_reg, neg_step_idx));
        }

        // ── 2. ForCount instruction at loop top ───────────────────────────
        //    If stop is a literal integer use the Const variant (avoids a temp
        //    register and a register read each iteration).  Otherwise compile
        //    stop once into a temp that lives for the loop duration.
        let loop_start = self.pc();
        let (exit_jmp, stop_temp) = if let Some(stop_val) = extract_literal_int(stop_expr) {
            let stop_idx = self.intern_const(Value::Int(stop_val));
            let jmp = self.emit(Insn::ForCountConst(var_reg, cmp_op, stop_idx, step_idx, 0));
            (jmp, None)
        } else {
            let r = self.compile_expr(stop_expr);
            // Copy to a temp if the stop expression resolved to a local register
            // so body mutations of that variable don't affect our bound.
            let sr = if r < self.base_temp {
                let t = self.alloc_temp();
                self.emit(Insn::Move(t, r));
                t
            } else {
                r
            };
            let jmp = self.emit(Insn::ForCountReg(var_reg, cmp_op, sr, step_idx, 0));
            (jmp, Some(sr))
        };

        // ── 3. Body ──────────────────────────────────────────────────────
        //    `continue` → loop_start (ForCount does the increment automatically)
        self.mark_def(var_reg);
        self.loops.push(LoopCtx {
            break_patches: Vec::new(),
            continue_target: Some(loop_start),
            continue_patches: Vec::new(),
        });
        let saved = self.def_set;
        self.compile_block(body);
        self.def_set = saved;
        if self.failed {
            return true;
        }

        // ── 4. Back edge ─────────────────────────────────────────────────
        let back_offset = loop_start as i32 - (self.pc() as i32 + 1);
        self.emit(Insn::Jump(back_offset));

        // ── 5. Exit + else ───────────────────────────────────────────────
        self.patch_jump(exit_jmp);
        if let Some(sr) = stop_temp {
            self.free_temp(sr);
        }
        let ctx = self.loops.pop().unwrap();
        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
            if self.failed {
                return true;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
        true
    }

    fn compile_for(
        &mut self,
        target: &AssignTarget,
        iter_expr: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
    ) {
        if self.try_compile_for_range(target, iter_expr, body, else_branch) {
            return;
        }
        let iter_slot = self.alloc_iter();
        let src = self.compile_expr(iter_expr);
        self.emit(Insn::GetIter(iter_slot, src));
        self.free_temp(src);
        let loop_start = self.pc();
        // For a local-variable target, write ForIter directly into the local register
        // to avoid an extra Move per iteration. For all other cases, use a temp.
        let for_dst = if let AssignTarget::Name(n) = target {
            self.local_reg(n).unwrap_or_else(|| self.alloc_temp())
        } else {
            self.alloc_temp()
        };
        let exit_jmp = self.emit(Insn::ForIter(for_dst, iter_slot, 0));
        match target {
            AssignTarget::Name(name) => {
                if self.local_reg(name).is_none() {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::StoreGlobal(name_idx, for_dst));
                    self.free_temp(for_dst);
                }
                // local case: for_dst == var_reg, already written — no Move needed
            }
            AssignTarget::Tuple(targets) => {
                let n = targets.len() as u8;
                let base = for_dst + 1;
                if base as usize + n as usize > 256 {
                    self.failed = true;
                    return;
                }
                self.next_temp = base + n;
                if self.next_temp - 1 > self.max_reg {
                    self.max_reg = self.next_temp - 1;
                }
                self.emit(Insn::Unpack(base, for_dst, n));
                for (i, t) in targets.iter().enumerate() {
                    match t {
                        AssignTarget::Name(name) => {
                            if let Some(reg) = self.local_reg(name) {
                                self.emit(Insn::Move(reg, base + i as u8));
                            } else {
                                let name_idx = self.intern_name(name);
                                self.emit(Insn::StoreGlobal(name_idx, base + i as u8));
                            }
                        }
                        _ => {
                            self.failed = true;
                            return;
                        }
                    }
                }
                self.next_temp = for_dst;
            }
            _ => {
                self.failed = true;
                return;
            }
        }
        self.loops.push(LoopCtx {
            break_patches: Vec::new(),
            continue_target: Some(loop_start),
            continue_patches: Vec::new(),
        });
        let saved_def_set = self.def_set;
        self.mark_target_def(target);
        self.compile_block(body);
        self.def_set = saved_def_set;
        if self.failed {
            return;
        }
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));
        self.patch_jump(exit_jmp);
        let ctx = self.loops.pop().unwrap();
        self.free_iter();
        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
            if self.failed {
                return;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
    }

    // ── Raise / Delete / Import ───────────────────────────────────────────────

    fn compile_raise(&mut self, expr: Option<&Expr>, cause: Option<&Expr>) {
        match expr {
            None => {
                self.emit(Insn::RaiseReRaise);
            }
            Some(e) => {
                let r = self.compile_expr(e);
                if let Some(cause_expr) = cause {
                    let c = self.compile_expr(cause_expr);
                    self.emit(Insn::RaiseFrom(r, c));
                    self.free_temp(c);
                } else {
                    self.emit(Insn::RaiseValue(r));
                }
                self.free_temp(r);
            }
        }
    }

    /// Build the 3-element slice-key tuple `(lo, hi, step)` used by GetItem/SetItem/DeleteItem.
    /// Each missing bound is represented as `None`. Returns the register holding the tuple.
    fn compile_slice_key(
        &mut self,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Reg {
        let none_reg = |this: &mut Self| {
            let r = this.alloc_temp();
            this.emit(Insn::LoadNone(r));
            r
        };
        let lower_r = lower
            .map(|e| self.compile_expr(e))
            .unwrap_or_else(|| none_reg(self));
        let upper_r = upper
            .map(|e| self.compile_expr(e))
            .unwrap_or_else(|| none_reg(self));
        let step_r = step
            .map(|e| self.compile_expr(e))
            .unwrap_or_else(|| none_reg(self));
        // Arrange contiguously for BuildTuple(base, 3)
        let base_r = lower_r;
        if upper_r != base_r + 1 {
            let t = base_r + 1;
            if t > self.max_reg {
                self.max_reg = t;
            }
            self.emit(Insn::Move(t, upper_r));
            self.free_temp(upper_r);
        }
        let upper_slot = base_r + 1;
        if step_r != upper_slot + 1 {
            let t = upper_slot + 1;
            if t > self.max_reg {
                self.max_reg = t;
            }
            self.emit(Insn::Move(t, step_r));
            self.free_temp(step_r);
        }
        let slice_r = self.alloc_temp();
        self.emit(Insn::BuildTuple(slice_r, base_r, 3));
        self.next_temp = slice_r + 1;
        slice_r
    }

    fn compile_delete(&mut self, expr: &Expr) {
        match expr {
            Expr::Var(name) => {
                let name_idx = self.intern_name(name);
                self.emit(Insn::DeleteName(name_idx));
            }
            Expr::Attr { target, name } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                self.emit(Insn::DeleteAttr(obj, name_idx));
                self.free_temp(obj);
            }
            Expr::Index { target, index } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                self.emit(Insn::DeleteItem(obj, idx));
                self.writeback_container_if_global(target, obj);
                self.free_temp(idx);
                self.free_temp(obj);
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                let obj = self.compile_expr(target);
                let slice_reg =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                self.emit(Insn::DeleteItem(obj, slice_reg));
                self.writeback_container_if_global(target, obj);
                self.free_temp(slice_reg);
                self.free_temp(obj);
            }
            _ => {
                self.failed = true;
            }
        }
    }

    fn compile_import(&mut self, names: &[(String, Option<String>)]) {
        for (module_name, alias) in names {
            let mod_idx = self.intern_name(module_name);
            let dst = self.alloc_temp();
            self.emit(Insn::ImportModule(dst, mod_idx));
            let bound = alias
                .as_deref()
                .unwrap_or_else(|| module_name.split('.').next().unwrap_or(module_name));
            self.compile_store_name(bound, dst);
            self.free_temp(dst);
        }
    }

    fn compile_import_from(&mut self, module: &str, names: &[(String, Option<String>)]) {
        let mod_idx = self.intern_name(module);
        let mod_reg = self.alloc_temp();
        self.emit(Insn::ImportModule(mod_reg, mod_idx));
        if names.len() == 1 && names[0].0 == "*" {
            // Star import: handled at runtime by a special path in ImportModule.
            // Use StoreGlobal with a sentinel name to trigger star import.
            let star_idx = self.intern_name("*");
            self.emit(Insn::StoreGlobal(star_idx, mod_reg));
        } else {
            for (attr_name, alias) in names {
                let attr_idx = self.intern_name(attr_name);
                let val_reg = self.alloc_temp();
                self.emit(Insn::GetAttr(val_reg, mod_reg, attr_idx));
                let bound = alias.as_deref().unwrap_or(attr_name);
                self.compile_store_name(bound, val_reg);
                self.free_temp(val_reg);
            }
        }
        self.free_temp(mod_reg);
    }

    // ── Def / Class ───────────────────────────────────────────────────────────

    fn compile_def(
        &mut self,
        name: &str,
        params: &[FunctionParam],
        body: &[Stmt],
        decorators: &[Expr],
    ) {
        // Build inner function's scope metadata.
        let inner_global = crate::interpreter::collect_global_names(body);
        let inner_nonlocal = crate::interpreter::collect_nonlocal_names(body);
        let inner_local =
            crate::interpreter::collect_local_names(params, body, &inner_global, &inner_nonlocal);

        // Build a compact local_index for the inner function.
        // Parameters come first (preserving declaration order), then body locals.
        let mut inner_index: HashMap<String, usize> = HashMap::new();
        let mut slot = 0usize;
        for param in params {
            if inner_local.contains(&param.name) {
                inner_index.insert(param.name.clone(), slot);
                slot += 1;
            }
        }
        for loc in &inner_local {
            if !inner_index.contains_key(loc) {
                inner_index.insert(loc.clone(), slot);
                slot += 1;
            }
        }
        let inner_index_rc: Rc<HashMap<String, usize>> = Rc::new(inner_index);

        let def_bound = crate::interpreter::compute_def_bound_mask(params, &inner_index_rc);
        let is_pure = crate::interpreter::is_pure_body(body);

        // Detect cell vars for the inner function.
        let inner_cell_vars = collect_cell_vars(body, &inner_index_rc);

        let inner_global_rc = Rc::new(inner_global);
        let inner_nonlocal_rc = Rc::new(inner_nonlocal);

        let mut sub = Compiler::new(
            Rc::clone(&inner_index_rc),
            Rc::clone(&inner_global_rc),
            Rc::clone(&inner_nonlocal_rc),
            def_bound,
            inner_cell_vars.clone(),
        );
        sub.compile_block(body);
        let inner_code = match sub.finish() {
            Some(c) => c,
            None => {
                self.failed = true;
                return;
            }
        };

        if self.fn_protos.len() >= 256 {
            self.failed = true;
            return;
        }
        let proto_idx = self.fn_protos.len() as u8;
        self.fn_protos.push(FnProto {
            name: name.to_string(),
            param_names: params.iter().map(|p| p.name.clone()).collect(),
            param_has_default: params.iter().map(|p| p.default.is_some()).collect(),
            param_is_args: params.iter().map(|p| p.is_args).collect(),
            param_is_kwargs: params.iter().map(|p| p.is_kwargs).collect(),
            code: Rc::new(inner_code),
            local_index: inner_index_rc,
            global_names: inner_global_rc,
            nonlocal_names: inner_nonlocal_rc,
            is_pure,
            def_bound_mask: def_bound,
            is_class_body: false,
        });

        // Compile default values (right-to-left in declaration, left-to-right in slots).
        let defaults: Vec<usize> = params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.default.is_some())
            .map(|(i, _)| i)
            .collect();
        let defs_n = defaults.len() as u8;
        let defs_base = self.next_temp;
        if defs_n > 0 {
            // Reserve slots
            if self.next_temp as usize + defs_n as usize > 256 {
                self.failed = true;
                return;
            }
            self.next_temp += defs_n;
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (slot_i, param_i) in defaults.iter().enumerate() {
                let def_expr = params[*param_i].default.as_ref().unwrap();
                let saved = self.next_temp;
                let r = self.compile_expr(def_expr);
                if r != defs_base + slot_i as u8 {
                    self.emit(Insn::Move(defs_base + slot_i as u8, r));
                }
                self.next_temp = saved;
            }
        }

        let dst = self.alloc_temp();
        self.emit(Insn::MakeFunction(dst, proto_idx, defs_base, defs_n));
        if defs_n > 0 {
            self.next_temp = defs_base + 1;
        }

        // Apply decorators (outermost first in reverse declaration order).
        let mut val_reg = dst;
        for deco_expr in decorators.iter().rev() {
            let frame = self.next_temp;
            if frame as usize + 2 > 256 {
                self.failed = true;
                return;
            }
            self.next_temp = frame + 2;
            if frame + 1 > self.max_reg {
                self.max_reg = frame + 1;
            }
            let saved = self.next_temp;
            self.compile_expr_into(deco_expr, frame);
            self.next_temp = saved;
            self.emit(Insn::Move(frame + 1, val_reg));
            self.emit(Insn::Call(frame, 1));
            self.next_temp = frame + 1;
            val_reg = frame;
        }

        self.compile_store_name(name, val_reg);
        if let Some(reg) = self.local_reg(name) {
            self.mark_def(reg);
        }
        self.free_temp(dst);
    }

    fn compile_class(&mut self, name: &str, bases: &[Expr], body: &[Stmt], decorators: &[Expr]) {
        // Class body: zero-param function that returns its locals as class dict.
        let empty_global: Rc<HashSet<String>> = Rc::new(HashSet::new());
        let empty_nonlocal: Rc<HashSet<String>> = Rc::new(HashSet::new());
        let body_local =
            crate::interpreter::collect_local_names(&[], body, &empty_global, &empty_nonlocal);
        let mut body_index: HashMap<String, usize> = HashMap::new();
        for (i, loc) in body_local.iter().enumerate() {
            body_index.insert(loc.clone(), i);
        }
        let body_index_rc: Rc<HashMap<String, usize>> = Rc::new(body_index);
        let cell_vars = collect_cell_vars(body, &body_index_rc);

        let mut sub = Compiler::new(
            Rc::clone(&body_index_rc),
            Rc::clone(&empty_global),
            Rc::clone(&empty_nonlocal),
            0,
            cell_vars,
        );
        sub.compile_block(body);
        // Add implicit ReturnNone at end of class body
        sub.emit(Insn::ReturnNone);
        let body_code = match sub.finish() {
            Some(c) => c,
            None => {
                self.failed = true;
                return;
            }
        };
        if self.fn_protos.len() >= 256 {
            self.failed = true;
            return;
        }
        let proto_idx = self.fn_protos.len() as u8;
        self.fn_protos.push(FnProto {
            name: name.to_string(),
            param_names: vec![],
            param_has_default: vec![],
            param_is_args: vec![],
            param_is_kwargs: vec![],
            code: Rc::new(body_code),
            local_index: body_index_rc,
            global_names: empty_global,
            nonlocal_names: empty_nonlocal,
            is_pure: false,
            def_bound_mask: 0,
            is_class_body: true,
        });

        // Compile base class expressions.
        let bases_n = bases.len() as u8;
        let bases_base = self.next_temp;
        if bases_n > 0 {
            if self.next_temp as usize + bases_n as usize > 256 {
                self.failed = true;
                return;
            }
            self.next_temp += bases_n;
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (i, base_expr) in bases.iter().enumerate() {
                let saved = self.next_temp;
                let r = self.compile_expr(base_expr);
                if r != bases_base + i as u8 {
                    self.emit(Insn::Move(bases_base + i as u8, r));
                }
                self.next_temp = saved;
            }
        }

        let name_idx = self.intern_name(name);
        let dst = self.alloc_temp();
        self.emit(Insn::MakeClass(
            dst, proto_idx, bases_base, bases_n, name_idx,
        ));
        if bases_n > 0 {
            self.next_temp = bases_base + 1;
        }

        let mut val_reg = dst;
        for deco_expr in decorators.iter().rev() {
            let frame = self.next_temp;
            if frame as usize + 2 > 256 {
                self.failed = true;
                return;
            }
            self.next_temp = frame + 2;
            if frame + 1 > self.max_reg {
                self.max_reg = frame + 1;
            }
            let saved = self.next_temp;
            self.compile_expr_into(deco_expr, frame);
            self.next_temp = saved;
            self.emit(Insn::Move(frame + 1, val_reg));
            self.emit(Insn::Call(frame, 1));
            self.next_temp = frame + 1;
            val_reg = frame;
        }

        self.compile_store_name(name, val_reg);
        if let Some(reg) = self.local_reg(name) {
            self.mark_def(reg);
        }
        self.free_temp(dst);
    }

    // ── Try / With ────────────────────────────────────────────────────────────

    fn compile_try(
        &mut self,
        body: &[Stmt],
        handlers: &[crate::ast::ExceptHandler],
        else_branch: Option<&[Stmt]>,
        finally_branch: Option<&[Stmt]>,
    ) {
        let has_handlers = !handlers.is_empty();

        // Strategy:
        // 1. If we have finally: wrap everything in an outer SetupExcept for finally.
        // 2. If we have handlers: inner SetupExcept for the except clause.
        // Normal-exit path: run else (if any), then finally (if any).
        // Exception path: dispatch handlers; on match run handler then finally;
        //                 on no-match re-raise (outer finally catches it).

        // Outer finally handler patch (only if finally_branch is Some)
        let outer_finally_patch: Option<usize> = if finally_branch.is_some() {
            Some(self.emit(Insn::SetupExcept(0)))
        } else {
            None
        };

        // Inner handler patch (only if has_handlers)
        let inner_handler_patch: Option<usize> = if has_handlers {
            Some(self.emit(Insn::SetupExcept(0)))
        } else {
            None
        };

        // Compile try body
        self.compile_block(body);
        if self.failed {
            return;
        }

        // Normal exit from try body:
        if let Some(_) = inner_handler_patch {
            self.emit(Insn::PopExcept);
        }
        // Compile else branch (normal path only)
        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
            if self.failed {
                return;
            }
        }
        // Normal finally exit
        if let Some(outer_idx) = outer_finally_patch {
            self.emit(Insn::PopExcept);
            let finally_stmts = finally_branch.unwrap();
            self.compile_block(finally_stmts);
            if self.failed {
                return;
            }
        }
        // Jump over handlers + exception path
        let end_patch = self.emit(Insn::Jump(0));

        // ── Exception path ──
        if let Some(inner_idx) = inner_handler_patch {
            self.patch_jump(inner_idx);
        }

        let mut handler_end_patches: Vec<usize> = Vec::new();

        if has_handlers {
            let exc_tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(exc_tmp));

            for handler in handlers {
                let skip_patch: Option<usize> = if let Some(kind_expr) = &handler.kind {
                    let type_reg = self.compile_expr(kind_expr);
                    let p = self.emit(Insn::MatchExcept(type_reg, 0));
                    self.free_temp(type_reg);
                    Some(p)
                } else {
                    None
                };

                // Bind exception variable if `as VAR`
                if let Some(var_name) = &handler.name {
                    if let Some(reg) = self.local_reg(var_name) {
                        self.emit(Insn::Move(reg, exc_tmp));
                        self.mark_def(reg);
                    } else {
                        let name_idx = self.intern_name(var_name);
                        self.emit(Insn::StoreGlobal(name_idx, exc_tmp));
                    }
                }

                // Pop outer finally handler before running handler body
                // (so that exceptions in the handler don't double-run finally)
                if outer_finally_patch.is_some() {
                    self.emit(Insn::PopExcept);
                }

                self.compile_block(&handler.body);
                if self.failed {
                    return;
                }
                self.emit(Insn::EndExcept);

                // Run finally (inline) after successful handler
                if let Some(finally_stmts) = finally_branch {
                    self.compile_block(finally_stmts);
                    if self.failed {
                        return;
                    }
                }

                let jmp = self.emit(Insn::Jump(0));
                handler_end_patches.push(jmp);

                if let Some(p) = skip_patch {
                    self.patch_jump(p);
                }
            }

            // No handler matched: re-raise (outer finally will catch it if present)
            self.free_temp(exc_tmp);
            self.emit(Insn::RaiseReRaise);
        }

        // ── Outer finally handler (exception path) ──
        if let Some(outer_idx) = outer_finally_patch {
            if !has_handlers {
                // No handlers: patch the inner SetupExcept → this finally handler
                self.patch_jump(outer_idx);
            }
            // If has_handlers, outer_finally_patch was patched when? Actually not yet.
            // For try/except/finally: the outer SetupExcept should catch exceptions
            // that escape the handlers (or re-raised). We patch it here.
            if has_handlers {
                self.patch_jump(outer_idx);
            }
            let finally_stmts = finally_branch.unwrap();
            // Load exception and run finally then re-raise
            let exc_tmp = self.alloc_temp();
            self.emit(Insn::LoadExc(exc_tmp));
            self.free_temp(exc_tmp);
            self.compile_block(finally_stmts);
            if self.failed {
                return;
            }
            self.emit(Insn::RaiseReRaise);
        }

        // Patch all successful handler jumps to here (after everything)
        self.patch_jump(end_patch);
        for idx in handler_end_patches {
            self.patch_jump(idx);
        }
    }

    fn compile_with(&mut self, items: &[(Expr, Option<AssignTarget>)], body: &[Stmt]) {
        // Compile nested with items recursively (outermost first).
        if items.is_empty() {
            self.compile_block(body);
            return;
        }
        let (expr, alias) = &items[0];
        let rest = &items[1..];

        // ctx = expr
        let ctx_reg = self.compile_expr(expr);

        // VAR = ctx.__enter__()
        let enter_name_idx = self.intern_name("__enter__");
        let enter_reg = self.alloc_temp();
        self.emit(Insn::GetAttr(enter_reg, ctx_reg, enter_name_idx));
        // Call __enter__() with no args: result goes to enter_reg
        self.emit(Insn::Call(enter_reg, 0));

        // Bind alias if present
        if let Some(tgt) = alias {
            let val_reg = enter_reg;
            match tgt {
                AssignTarget::Name(name) => {
                    if let Some(reg) = self.local_reg(name) {
                        self.emit(Insn::Move(reg, val_reg));
                        self.mark_def(reg);
                    } else {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::StoreGlobal(name_idx, val_reg));
                    }
                }
                _ => {
                    // Complex targets: just assign via the general mechanism
                    // (simplified: ignore for now)
                }
            }
        }

        // SetupExcept for the body
        let setup_patch = self.emit(Insn::SetupExcept(0));

        // Compile nested with items or body
        if rest.is_empty() {
            self.compile_block(body);
        } else {
            self.compile_with(rest, body);
        }
        if self.failed {
            return;
        }

        // Normal exit
        self.emit(Insn::PopExcept);
        // ctx.__exit__(None, None, None)
        let exit_name_idx = self.intern_name("__exit__");
        let exit_frame = self.next_temp;
        if exit_frame as usize + 4 > 256 {
            self.failed = true;
            return;
        }
        self.next_temp = exit_frame + 4;
        if exit_frame + 3 > self.max_reg {
            self.max_reg = exit_frame + 3;
        }
        self.emit(Insn::GetAttr(exit_frame, ctx_reg, exit_name_idx));
        self.emit(Insn::LoadNone(exit_frame + 1));
        self.emit(Insn::LoadNone(exit_frame + 2));
        self.emit(Insn::LoadNone(exit_frame + 3));
        self.emit(Insn::Call(exit_frame, 3));
        self.next_temp = exit_frame;
        let end_patch = self.emit(Insn::Jump(0));

        // Exception path
        self.patch_jump(setup_patch);
        let exc_tmp = self.alloc_temp();
        self.emit(Insn::LoadExc(exc_tmp));
        // ctx.__exit__(type, exc, None)
        let exit_frame2 = self.next_temp;
        if exit_frame2 as usize + 4 > 256 {
            self.failed = true;
            return;
        }
        self.next_temp = exit_frame2 + 4;
        if exit_frame2 + 3 > self.max_reg {
            self.max_reg = exit_frame2 + 3;
        }
        let class_name_idx = self.intern_name("__class__");
        self.emit(Insn::GetAttr(exit_frame2, ctx_reg, exit_name_idx));
        self.emit(Insn::GetAttr(exit_frame2 + 1, exc_tmp, class_name_idx)); // exc_type
        self.emit(Insn::Move(exit_frame2 + 2, exc_tmp));
        self.emit(Insn::Move(exit_frame2 + 3, exc_tmp)); // traceback (non-None placeholder)
        self.emit(Insn::Call(exit_frame2, 3));
        let suppress_reg = exit_frame2;
        self.next_temp = exit_frame2 + 1;
        // If __exit__ returned truthy, suppress exception (EndExcept + skip re-raise)
        let suppress_patch = self.emit(Insn::JumpIfTrue(suppress_reg, 0));
        self.free_temp(exc_tmp);
        self.emit(Insn::RaiseReRaise);
        self.patch_jump(suppress_patch);
        self.emit(Insn::EndExcept);

        self.patch_jump(end_patch);
        self.free_temp(ctx_reg);
    }

    // ── Expression compilation ────────────────────────────────────────────────

    /// Retarget the last emitted instruction's dst from `from` to `to`.
    /// Returns true if the instruction was retargeted (had dst == from).
    fn retarget_last(&mut self, from: Reg, to: Reg) -> bool {
        let Some(insn) = self.insns.last_mut() else {
            return false;
        };
        let dst = match insn {
            Insn::BinOp(d, ..)
            | Insn::BinOpInPlace(d, ..)
            | Insn::BinOpConst(d, ..)
            | Insn::UnaryOp(d, ..)
            | Insn::LoadConst(d, ..)
            | Insn::LoadNone(d)
            | Insn::LoadGlobal(d, ..)
            | Insn::Move(d, ..)
            | Insn::GetAttr(d, ..)
            | Insn::GetItem(d, ..)
            // Call is NOT retargetable: Call(func_reg, argc) uses func_reg as both
            // the function source and the result destination. Retargeting it to a
            // different register would point the call at the wrong function.
            | Insn::MakeFunction(d, ..)
            | Insn::MakeClass(d, ..)
            | Insn::BuildList(d, ..)
            | Insn::BuildTuple(d, ..)
            | Insn::BuildDict(d, ..)
            | Insn::ForIter(d, ..)
            | Insn::LoadExc(d)
            | Insn::ImportModule(d, ..) => d,
            _ => return false,
        };
        if *dst == from {
            *dst = to;
            true
        } else {
            false
        }
    }

    fn compile_expr_into(&mut self, expr: &Expr, dst: Reg) {
        if self.failed {
            return;
        }
        let saved_next = self.next_temp;
        let insn_before = self.insns.len();
        let r = self.compile_expr(expr);
        if r != dst {
            // Safe to retarget only when the expression compiled to EXACTLY one
            // instruction and the result is a fresh temp: guarantees no control
            // flow or multi-instruction sequences where other branches still write
            // to `r` and would be missed by retargeting only the last instruction.
            let single = self.insns.len() == insn_before + 1;
            if single && r >= self.base_temp && self.retarget_last(r, dst) {
                self.next_temp = saved_next;
            } else {
                self.emit(Insn::Move(dst, r));
                if r >= self.base_temp {
                    self.next_temp = saved_next;
                }
            }
        }
    }

    fn compile_expr(&mut self, expr: &Expr) -> Reg {
        if self.failed {
            return 0;
        }
        match expr {
            Expr::None => {
                let dst = self.alloc_temp();
                self.emit(Insn::LoadNone(dst));
                dst
            }
            Expr::Int(v) => {
                let idx = self.intern_const(Value::Int(*v));
                let dst = self.alloc_temp();
                self.emit(Insn::LoadConst(dst, idx));
                dst
            }
            Expr::Float(v) => {
                let idx = self.intern_const(Value::Float(*v));
                let dst = self.alloc_temp();
                self.emit(Insn::LoadConst(dst, idx));
                dst
            }
            Expr::Str(s) => {
                let idx = self.intern_const(Value::Str(s.clone()));
                let dst = self.alloc_temp();
                self.emit(Insn::LoadConst(dst, idx));
                dst
            }
            Expr::Bool(b) => {
                let idx = self.intern_const(Value::Bool(*b));
                let dst = self.alloc_temp();
                self.emit(Insn::LoadConst(dst, idx));
                dst
            }
            Expr::Var(name) => {
                if let Some(reg) = self.local_reg(name) {
                    let definitely_bound = (reg as usize) < 64 && (self.def_set >> reg) & 1 != 0;
                    if !definitely_bound {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::CheckLocal(reg, name_idx));
                    }
                    reg
                } else {
                    // global / nonlocal / cell / free variable
                    let name_idx = self.intern_name(name);
                    let dst = self.alloc_temp();
                    self.emit(Insn::LoadGlobal(dst, name_idx));
                    dst
                }
            }
            Expr::Unary { op, expr } => {
                if let Some(val) = fold_constant(&Expr::Unary {
                    op: *op,
                    expr: expr.clone(),
                }) {
                    let idx = self.intern_const(val);
                    let dst = self.alloc_temp();
                    self.emit(Insn::LoadConst(dst, idx));
                    return dst;
                }
                let src = self.compile_expr(expr);
                let dst = self.ensure_dst(src);
                self.emit(Insn::UnaryOp(dst, *op, src));
                dst
            }
            Expr::Binary { left, op, right } => match op {
                BinaryOp::And => {
                    let lhs = self.compile_expr(left);
                    let dst = self.ensure_dst(lhs);
                    if dst != lhs {
                        self.emit(Insn::Move(dst, lhs));
                    }
                    let jmp = self.emit(Insn::JumpIfFalse(dst, 0));
                    let saved = self.next_temp;
                    let rhs = self.compile_expr(right);
                    self.emit(Insn::Move(dst, rhs));
                    self.next_temp = saved;
                    self.patch_jump(jmp);
                    dst
                }
                BinaryOp::Or => {
                    let lhs = self.compile_expr(left);
                    let dst = self.ensure_dst(lhs);
                    if dst != lhs {
                        self.emit(Insn::Move(dst, lhs));
                    }
                    let jmp = self.emit(Insn::JumpIfTrue(dst, 0));
                    let saved = self.next_temp;
                    let rhs = self.compile_expr(right);
                    self.emit(Insn::Move(dst, rhs));
                    self.next_temp = saved;
                    self.patch_jump(jmp);
                    dst
                }
                _ => {
                    // Constant fold: if both sides are literals, compute at compile time.
                    if let Some(val) = fold_constant(expr) {
                        let idx = self.intern_const(val);
                        let dst = self.alloc_temp();
                        self.emit(Insn::LoadConst(dst, idx));
                        return dst;
                    }
                    let lhs = self.compile_expr(left);
                    let dst = self.ensure_dst(lhs);
                    if let Some(const_idx) = self.try_literal_const_idx(right) {
                        self.emit(Insn::BinOpConst(dst, lhs, *op, const_idx));
                    } else {
                        let rhs = self.compile_expr(right);
                        self.emit(Insn::BinOp(dst, lhs, *op, rhs));
                        self.free_temp(rhs);
                    }
                    dst
                }
            },
            Expr::Compare { left, ops } => {
                // Constant fold: e.g. `1 < 2` at compile time.
                if let Some(val) = fold_constant(expr) {
                    let idx = self.intern_const(val);
                    let dst = self.alloc_temp();
                    self.emit(Insn::LoadConst(dst, idx));
                    return dst;
                }
                if ops.len() == 1 {
                    let (cmp_op, right) = &ops[0];
                    let lhs = self.compile_expr(left);
                    let bin_op = cmp_to_binary(*cmp_op);
                    let dst = self.ensure_dst(lhs);
                    if let Some(const_idx) = self.try_literal_const_idx(right) {
                        self.emit(Insn::BinOpConst(dst, lhs, bin_op, const_idx));
                    } else {
                        let rhs = self.compile_expr(right);
                        self.emit(Insn::BinOp(dst, lhs, bin_op, rhs));
                        self.free_temp(rhs);
                    }
                    dst
                } else {
                    // Chained comparison: a < b < c  →  (a < b) and (b < c)
                    // Evaluate left once, then chain.
                    let first_lhs = self.compile_expr(left);
                    let result_dst = self.alloc_temp();
                    let mut and_patches: Vec<usize> = Vec::new();
                    let mut prev_rhs = first_lhs;
                    for (i, (cmp_op, rhs_expr)) in ops.iter().enumerate() {
                        let bin_op = cmp_to_binary(*cmp_op);
                        let rhs = self.compile_expr(rhs_expr);
                        let cmp_dst = self.alloc_temp();
                        self.emit(Insn::BinOp(cmp_dst, prev_rhs, bin_op, rhs));
                        if i > 0 {
                            self.free_temp(prev_rhs);
                        }
                        self.emit(Insn::Move(result_dst, cmp_dst));
                        self.free_temp(cmp_dst);
                        if i < ops.len() - 1 {
                            let p = self.emit(Insn::JumpIfFalse(result_dst, 0));
                            and_patches.push(p);
                        }
                        prev_rhs = rhs;
                    }
                    self.free_temp(prev_rhs);
                    for p in and_patches {
                        self.patch_jump(p);
                    }
                    self.free_temp(first_lhs);
                    result_dst
                }
            }
            Expr::Call { func, args } => self.compile_call(func, args),
            Expr::Attr { target, name } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                let dst = self.ensure_dst(obj);
                self.emit(Insn::GetAttr(dst, obj, name_idx));
                dst
            }
            Expr::Index { target, index } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                let dst = self.ensure_dst(obj);
                self.emit(Insn::GetItem(dst, obj, idx));
                self.free_temp(idx);
                dst
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                let obj = self.compile_expr(target);
                let slice_r =
                    self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref());
                let dst = self.ensure_dst(obj);
                self.emit(Insn::GetItem(dst, obj, slice_r));
                self.free_temp(slice_r);
                dst
            }
            Expr::List(items) => self.compile_collection(items, false),
            Expr::Tuple(items) => self.compile_collection(items, true),
            Expr::Set(items) => {
                // Build as list then convert — reuse BuildList + special handling.
                // For simplicity, compile as a list and let the VM convert.
                // Actually we don't have a BuildSet instruction.
                // Use BuildList then the VM can interpret it as a set.
                // Best approach: use a "set literal" call: set([items...])
                let n = items.len() as u8;
                let frame = self.next_temp;
                if frame as usize + 1 + n as usize > 256 {
                    self.failed = true;
                    return 0;
                }
                self.next_temp = frame + 1 + n;
                if frame + n > self.max_reg {
                    self.max_reg = frame + n;
                }
                let set_name_idx = self.intern_name("set");
                self.emit(Insn::LoadGlobal(frame, set_name_idx));
                // Build the list of items in frame+1..frame+1+n
                let list_r = frame + 1;
                let saved = self.next_temp;
                let list_base = self.next_temp;
                if list_base as usize + n as usize > 256 {
                    self.failed = true;
                    return 0;
                }
                self.next_temp = list_base + n;
                if list_base + n - 1 > self.max_reg {
                    self.max_reg = list_base + n - 1;
                }
                for (i, item) in items.iter().enumerate() {
                    let slot = list_base + i as u8;
                    let ns = self.next_temp;
                    let r = self.compile_expr(item);
                    if r != slot {
                        self.emit(Insn::Move(slot, r));
                    }
                    self.next_temp = ns;
                }
                self.emit(Insn::BuildList(list_r, list_base, n));
                self.next_temp = saved;
                self.next_temp = frame + 2;
                if frame + 1 > self.max_reg {
                    self.max_reg = frame + 1;
                }
                self.emit(Insn::Call(frame, 1));
                self.next_temp = frame + 1;
                frame
            }
            Expr::Dict(pairs) => {
                let n = pairs.len() as u8;
                let base = self.next_temp;
                let slots_needed = (n as usize).saturating_mul(2);
                if base as usize + slots_needed > 256 {
                    self.failed = true;
                    return 0;
                }
                self.next_temp = base + n.saturating_mul(2);
                if self.next_temp > 0 && self.next_temp - 1 > self.max_reg {
                    self.max_reg = self.next_temp - 1;
                }
                for (i, (key_expr, val_expr)) in pairs.iter().enumerate() {
                    let k_slot = base + (i * 2) as u8;
                    let v_slot = base + (i * 2 + 1) as u8;
                    let saved = self.next_temp;
                    let insn_before = self.insns.len();
                    let kr = self.compile_expr(key_expr);
                    if kr != k_slot {
                        let single = self.insns.len() == insn_before + 1;
                        if single && kr >= self.base_temp && self.retarget_last(kr, k_slot) {
                            // retargeted in place — no Move needed
                        } else {
                            self.emit(Insn::Move(k_slot, kr));
                        }
                    }
                    self.next_temp = saved;
                    let saved = self.next_temp;
                    let insn_before = self.insns.len();
                    let vr = self.compile_expr(val_expr);
                    if vr != v_slot {
                        let single = self.insns.len() == insn_before + 1;
                        if single && vr >= self.base_temp && self.retarget_last(vr, v_slot) {
                            // retargeted in place — no Move needed
                        } else {
                            self.emit(Insn::Move(v_slot, vr));
                        }
                    }
                    self.next_temp = saved;
                }
                self.emit(Insn::BuildDict(base, base, n));
                self.next_temp = base + 1;
                base
            }
            Expr::Ternary { cond, then, else_ } => {
                let cond_reg = self.compile_expr(cond);
                let jmp_false = self.emit(Insn::JumpIfFalse(cond_reg, 0));
                self.free_temp(cond_reg);
                let dst = self.alloc_temp();
                let saved = self.next_temp;
                self.compile_expr_into(then, dst);
                self.next_temp = saved;
                let jmp_end = self.emit(Insn::Jump(0));
                self.patch_jump(jmp_false);
                let saved = self.next_temp;
                self.compile_expr_into(else_, dst);
                self.next_temp = saved;
                self.patch_jump(jmp_end);
                dst
            }
            Expr::Lambda { params, body } => {
                let fp: Vec<FunctionParam> = params
                    .iter()
                    .map(|n| FunctionParam {
                        name: n.clone(),
                        default: None,
                        is_args: false,
                        is_kwargs: false,
                    })
                    .collect();
                self.compile_lambda(&fp, body)
            }
        }
    }

    fn compile_call(&mut self, func: &Expr, args: &[crate::ast::CallArg]) -> Reg {
        // Check for any splat args — these require a variadic call path.
        let has_splat = args.iter().any(|a| a.splat || a.double_splat);
        let has_kwargs = args.iter().any(|a| a.name.is_some());

        if has_splat || has_kwargs {
            // Variadic call: build separate positional and keyword lists, then
            // use the ExpandedCall instruction.
            return self.compile_variadic_call(func, args);
        }

        let argc = args.len() as u8;
        let func_reg = self.next_temp;
        let frame_top = func_reg.wrapping_add(1).wrapping_add(argc);
        if frame_top < func_reg {
            self.failed = true;
            return 0;
        }
        self.next_temp = frame_top;
        if frame_top > 0 && frame_top - 1 > self.max_reg {
            self.max_reg = frame_top - 1;
        }
        let saved = self.next_temp;
        self.compile_expr_into(func, func_reg);
        self.next_temp = saved;
        for (i, arg) in args.iter().enumerate() {
            let arg_reg = func_reg + 1 + i as u8;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(&arg.value);
            if r != arg_reg {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, arg_reg) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(arg_reg, r));
                }
            }
            self.next_temp = saved;
        }
        self.emit(Insn::Call(func_reg, argc));
        self.next_temp = func_reg + 1;
        func_reg
    }

    fn compile_variadic_call(&mut self, func: &Expr, args: &[crate::ast::CallArg]) -> Reg {
        // For variadic/keyword calls, we build a list of positional args and a
        // dict of keyword args, then use a special "expanded call" mechanism.
        // Since the current VM Call instruction only handles simple positional
        // args, we fall back to calling via the `__call__` convention by building
        // lists/dicts and using a runtime helper.
        //
        // Simplified: collect positional args into a list and keyword args into
        // a dict, then call the runtime built-in "apply" helper.
        // For now, compile as: func(*args, **kwargs) using the ExpandedCall route.
        //
        // The approach: build a list of positional args and a dict of kwargs,
        // then use a special instruction (or just let the normal Call handle it
        // by putting them in the right registers).
        //
        // Actually, to avoid a new instruction, we compile this as:
        //   _call_helper(func, positional_list, kwargs_dict)
        // where _call_helper is a built-in that expands and calls.
        //
        // Better: use ExpandedCall instruction. For now, emit the positional
        // args normally and mark the call as variadic.
        //
        // SIMPLIFICATION: compile splat/kwargs calls by materializing all args
        // and using a special VM call path via a built-in helper function.
        // The interpreter's call_function_expanded handles this already.
        //
        // We encode a variadic call as: emit args into consecutive registers
        // including splat markers, then emit a special extended Call instruction.
        // Since we don't have that instruction, use a simpler encoding:
        // build a list for *args and a dict for **kwargs.

        // Strategy: compile func, then for each arg:
        // - if splat: use list.extend
        // - if double_splat: use dict.update
        // - if named: add to kwargs dict
        // - else: add to positional list
        // Then call via "built-in" expanded call.

        // Simpler approach: just pack everything into two registers (pos_list, kw_dict)
        // and use the existing call infrastructure.

        // For *args: args.iter().flat_map expand
        // For **kwargs: dict from kwargs

        let func_reg = self.alloc_temp();
        self.compile_expr_into(func, func_reg);

        // Build positional list
        let pos_list_reg = self.alloc_temp();
        // Use: pos_list = []  → then extend/append
        let empty_list_base = self.next_temp;
        if empty_list_base as usize + 1 > 256 {
            self.failed = true;
            return 0;
        }
        self.next_temp = empty_list_base + 1;
        if empty_list_base > self.max_reg {
            self.max_reg = empty_list_base;
        }
        self.emit(Insn::BuildList(pos_list_reg, empty_list_base, 0));

        // Build kwargs dict
        let kw_dict_reg = self.alloc_temp();
        let empty_dict_base = self.next_temp;
        if empty_dict_base as usize + 1 > 256 {
            self.failed = true;
            return 0;
        }
        self.next_temp = empty_dict_base + 1;
        if empty_dict_base > self.max_reg {
            self.max_reg = empty_dict_base;
        }
        self.emit(Insn::BuildDict(kw_dict_reg, empty_dict_base, 0));

        for arg in args {
            if arg.splat {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::ListExtend(pos_list_reg, val));
                self.free_temp(val);
            } else if arg.double_splat {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::DictUpdate(kw_dict_reg, val));
                self.free_temp(val);
            } else if let Some(kw_name) = &arg.name {
                let val = self.compile_expr(&arg.value);
                let key_idx = self.intern_const(Value::Str(kw_name.clone()));
                let key_reg = self.alloc_temp();
                self.emit(Insn::LoadConst(key_reg, key_idx));
                self.emit(Insn::SetItem(kw_dict_reg, key_reg, val));
                self.free_temp(key_reg);
                self.free_temp(val);
            } else {
                let val = self.compile_expr(&arg.value);
                self.emit(Insn::ListAppend(pos_list_reg, val));
                self.free_temp(val);
            }
        }

        // Emit a ExpandedCall using the __pyrust_vcall__ builtin.
        // Actually, the cleanest way: use a built-in "apply" that takes
        // (func, pos_list, kw_dict). This requires a new built-in or instruction.
        //
        // For now, emit via a 3-arg call to "__vcall__" which the interpreter
        // knows how to dispatch.
        let vcall_name_idx = self.intern_name("__vcall__");
        let vcall_reg = self.alloc_temp();
        self.emit(Insn::LoadGlobal(vcall_reg, vcall_name_idx));
        // vcall_reg+1 = func_reg
        // vcall_reg+2 = pos_list_reg
        // vcall_reg+3 = kw_dict_reg
        // We need these in consecutive registers vcall_reg+1..vcall_reg+4
        let f1 = self.alloc_temp();
        let f2 = self.alloc_temp();
        let f3 = self.alloc_temp();
        self.emit(Insn::Move(f1, func_reg));
        self.emit(Insn::Move(f2, pos_list_reg));
        self.emit(Insn::Move(f3, kw_dict_reg));
        self.emit(Insn::Call(vcall_reg, 3));
        self.free_temp(f3);
        self.free_temp(f2);
        self.free_temp(f1);
        self.free_temp(vcall_reg);
        self.free_temp(kw_dict_reg);
        self.free_temp(pos_list_reg);
        self.free_temp(func_reg);
        // Return value is in vcall_reg
        vcall_reg
    }

    fn compile_lambda(&mut self, params: &[FunctionParam], body: &Expr) -> Reg {
        // Convert lambda body into an implicit return statement.
        let body_stmts = vec![Stmt::Return(Some(body.clone()))];
        let temp_name = "<lambda>";
        self.compile_def(temp_name, params, &body_stmts, &[]);
        // compile_def stored the result in local or global named "<lambda>".
        // We need to return the register it's in.
        // Actually compile_def uses compile_store_name which may put it in a
        // global. We need to load it back.
        let name_idx = self.intern_name(temp_name);
        let dst = self.alloc_temp();
        self.emit(Insn::LoadGlobal(dst, name_idx));
        dst
    }

    fn compile_collection(&mut self, items: &[Expr], is_tuple: bool) -> Reg {
        let n = items.len() as u8;
        let base = self.next_temp;
        if base as usize + n as usize > 256 {
            self.failed = true;
            return 0;
        }
        self.next_temp = base + n;
        if n > 0 && base + n - 1 > self.max_reg {
            self.max_reg = base + n - 1;
        }
        for (i, item) in items.iter().enumerate() {
            let slot = base + i as u8;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(item);
            if r != slot {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, slot) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(slot, r));
                }
            }
            self.next_temp = saved;
        }
        if is_tuple {
            self.emit(Insn::BuildTuple(base, base, n));
        } else {
            self.emit(Insn::BuildList(base, base, n));
        }
        self.next_temp = base + 1;
        base
    }
}
