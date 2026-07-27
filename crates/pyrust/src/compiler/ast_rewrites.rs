// ─── Utilities ────────────────────────────────────────────────────────────────

fn const_eq(a: &Value, b: &Value) -> bool {
    use ValueKind::*;
    match (a.kind(), b.kind()) {
        (Int(x), Int(y)) => x == y,
        (BigInt(x), BigInt(y)) => x == y,
        (Float(x), Float(y)) => x.to_bits() == y.to_bits(),
        // Use bit-level comparison for complex parts so that NaN-keyed
        // constants are treated as the same pool entry (same as Float above).
        (Complex(ar, ai), Complex(br, bi)) => {
            ar.to_bits() == br.to_bits() && ai.to_bits() == bi.to_bits()
        }
        (Str(x), Str(y)) => x == y,
        (Bytes(x), Bytes(y)) => x.as_ref() == y.as_ref(),
        (Bool(x), Bool(y)) => x == y,
        (None, None) => true,
        _ => false,
    }
}

/// Attempt to evaluate a pure constant expression at compile time.
/// Returns Some(value) only when the entire expression tree consists of
/// literals and operations on literals that cannot raise.
fn fold_constant(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Int(v) => Some(Value::int(*v)),
        Expr::BigInt(s) => s.parse::<PyBigInt>().ok().map(Value::bigint),
        Expr::Float(v) => Some(Value::float(*v)),
        Expr::Str(s) => Some(Value::string(s.clone())),
        Expr::Bytes(b) => Some(Value::bytes(b.clone())),
        Expr::Complex(re, im) => Some(Value::complex(*re, *im)),
        Expr::Bool(b) => Some(Value::bool_(*b)),
        Expr::None => Some(Value::none()),
        Expr::Ellipsis => Some(Value::ellipsis()),
        Expr::Unary { op, expr, .. } => {
            let val = fold_constant(expr)?;
            match op {
                UnaryOp::Neg => match val.kind() {
                    // `-i64::MIN` overflows; promote to BigInt to match
                    // CPython's arbitrary-precision int semantics (#421).
                    ValueKind::Int(n) => Some(match n.checked_neg() {
                        Some(r) => Value::int(r),
                        None => Value::bigint(-PyBigInt::from(n)),
                    }),
                    ValueKind::Float(f) => Some(Value::float(-f)),
                    ValueKind::BigInt(b) => Some(Value::bigint(-b)),
                    _ => None,
                },
                UnaryOp::Not => Some(Value::bool_(!val.truthy_raw())),
                UnaryOp::BitNot => match val.kind() {
                    ValueKind::Int(n) => Some(Value::int(!n)),
                    _ => None,
                },
                _ => None,
            }
        }
        Expr::Binary {
            left, op, right, ..
        } => {
            let l = fold_constant(left)?;
            let r = fold_constant(right)?;
            fold_binop(&l, *op, &r)
        }
        Expr::Compare { left, ops } => {
            let mut cur = fold_constant(left)?;
            for (cmp_op, rhs_expr) in ops {
                let rhs = fold_constant(rhs_expr)?;
                let op = BinaryOp::from(*cmp_op);
                let result = fold_binop(&cur, op, &rhs)?;
                if !result.truthy_raw() {
                    return Some(Value::bool_(false));
                }
                cur = rhs;
            }
            Some(Value::bool_(true))
        }
        Expr::Named { .. } => None,
        _ => None,
    }
}

pub(crate) fn fold_binop(l: &Value, op: BinaryOp, r: &Value) -> Option<Value> {
    use BinaryOp::*;
    // Int/int arithmetic, shifts, and bitwise route through the single
    // canonical numeric implementation shared with `eval_binary` (issue
    // #458): one definition of overflow promotion, CPython floored `//` /
    // `%`, and shift/bitwise semantics.  A runtime error (e.g.
    // ZeroDivisionError on `x / 0`) returns `None` so the BinOp stays in
    // the bytecode and raises at runtime, never at compile time.
    if matches!((l.kind(), r.kind()), (ValueKind::Int(_), ValueKind::Int(_))) {
        match op {
            Add | Sub | Mul | Div | FloorDiv | Mod | BitAnd | BitOr | BitXor => {
                return dispatch_numeric_binop(op, l, r)?.ok();
            }
            // `**` and shifts can produce arbitrarily large constants
            // (`2 ** 1_000_000`, `1 << i64::MAX`).  Cap the magnitude here
            // so a hostile literal can't bloat the constant pool during
            // compilation; oversized cases fall through to the runtime
            // (which shares the same slot).
            Pow => {
                if let ValueKind::Int(b) = r.kind()
                    && (0..=u32::MAX as i64).contains(&b)
                {
                    return dispatch_numeric_binop(op, l, r)?.ok();
                }
                return None;
            }
            LShift | RShift => {
                if let ValueKind::Int(b) = r.kind()
                    && (0..=1_000_000).contains(&b)
                {
                    return dispatch_numeric_binop(op, l, r)?.ok();
                }
                return None;
            }
            _ => {}
        }
    }
    // Non-int-arithmetic folds the optimizer has always done: float
    // arithmetic, string concatenation, and constant comparisons.  These
    // are not numeric-slot arithmetic (comparisons) or involve non-int
    // operands, so they stay as explicit arms.
    match (l.kind(), op, r.kind()) {
        (ValueKind::Float(a), Add, ValueKind::Float(b)) => Some(Value::float(a + b)),
        (ValueKind::Float(a), Sub, ValueKind::Float(b)) => Some(Value::float(a - b)),
        (ValueKind::Float(a), Mul, ValueKind::Float(b)) => Some(Value::float(a * b)),
        (ValueKind::Float(a), Div, ValueKind::Float(b)) if b != 0.0 => Some(Value::float(a / b)),
        (ValueKind::Str(a), Add, ValueKind::Str(b)) => Some(Value::string(a.to_string() + b)),
        (ValueKind::Int(a), Eq, ValueKind::Int(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Int(a), Ne, ValueKind::Int(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Int(a), Lt, ValueKind::Int(b)) => Some(Value::bool_(a < b)),
        (ValueKind::Int(a), Le, ValueKind::Int(b)) => Some(Value::bool_(a <= b)),
        (ValueKind::Int(a), Gt, ValueKind::Int(b)) => Some(Value::bool_(a > b)),
        (ValueKind::Int(a), Ge, ValueKind::Int(b)) => Some(Value::bool_(a >= b)),
        (ValueKind::Str(a), Eq, ValueKind::Str(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Str(a), Ne, ValueKind::Str(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Bool(a), Eq, ValueKind::Bool(b)) => Some(Value::bool_(a == b)),
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
