//! Compile-time constant evaluation and purity helpers for SSA MIR.

use crate::ast::{BinaryOp, UnaryOp};
use crate::value::{Value, ValueKind};

use super::MirType;

/// True when `val` is an immutable, primitive compile-time constant whose
/// truthiness and pure operations never invoke user Python code.
#[inline]
pub fn is_mir_pure_const(val: &Value) -> bool {
    matches!(
        val.kind(),
        ValueKind::Int(_)
            | ValueKind::BigInt(_)
            | ValueKind::Float(_)
            | ValueKind::Complex(_, _)
            | ValueKind::Str(_)
            | ValueKind::Bytes(_)
            | ValueKind::Bool(_)
            | ValueKind::None
            | ValueKind::Tuple(_)
    )
}

/// Type-exact equality for compile-time constants (`Int(1) != Bool(true)`,
/// `-0.0 != +0.0`, `NaN == NaN` by bit pattern).
pub fn mir_const_exact_eq(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (ValueKind::Int(x), ValueKind::Int(y)) => x == y,
        (ValueKind::BigInt(x), ValueKind::BigInt(y)) => x == y,
        (ValueKind::Float(x), ValueKind::Float(y)) => x.to_bits() == y.to_bits(),
        (ValueKind::Complex(xr, xi), ValueKind::Complex(yr, yi)) => {
            xr.to_bits() == yr.to_bits() && xi.to_bits() == yi.to_bits()
        }
        (ValueKind::Str(x), ValueKind::Str(y)) => x == y,
        (ValueKind::Bytes(x), ValueKind::Bytes(y)) => x == y,
        (ValueKind::Bool(x), ValueKind::Bool(y)) => x == y,
        (ValueKind::None, ValueKind::None) => true,
        (ValueKind::Tuple(xs), ValueKind::Tuple(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| mir_const_exact_eq(x, y))
        }
        _ => false,
    }
}

/// Compute pure Python truthiness of a primitive constant, returning `None`
/// for non-primitive values.
#[inline]
pub fn mir_const_truthy(val: &Value) -> Option<bool> {
    Some(match val.kind() {
        ValueKind::Bool(b) => b,
        ValueKind::Int(n) => n != 0,
        ValueKind::BigInt(n) => n.sign() != pyrust_core::PyBigIntSign::NoSign,
        ValueKind::Float(f) => f != 0.0,
        ValueKind::Complex(r, i) => r != 0.0 || i != 0.0,
        ValueKind::Str(s) => !s.is_empty(),
        ValueKind::Bytes(b) => !b.is_empty(),
        ValueKind::None => false,
        ValueKind::Tuple(t) => !t.is_empty(),
        _ => return None,
    })
}

/// Evaluate a pure binary operation between two compile-time constant `Value`s.
///
/// Delegates integer/float/string arithmetic and comparisons to the canonical
/// [`crate::compiler::fold_binop`] / `pyrust_core::NumericOps` protocol (issue
/// #458 / #421), returning `None` on any runtime error (e.g. division by zero)
/// so the operation stays in MIR and raises at runtime.
pub fn eval_mir_binop(lv: &Value, op: BinaryOp, rv: &Value) -> Option<Value> {
    if let Some(folded) = crate::compiler::fold_binop(lv, op, rv) {
        return Some(folded);
    }
    use BinaryOp::*;
    // Also support BigInt/Int mixed exact arithmetic via canonical dispatch so
    // chained overflow-promoted constants fold accurately (#421).
    if matches!(
        (lv.kind(), rv.kind()),
        (
            ValueKind::Int(_) | ValueKind::BigInt(_),
            ValueKind::Int(_) | ValueKind::BigInt(_)
        )
    ) {
        match op {
            Add | Sub | Mul | Div | FloorDiv | Mod | BitAnd | BitOr | BitXor => {
                return crate::interpreter::dispatch_numeric_binop(op, lv, rv)?.ok();
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                let cmp = crate::interpreter::binary_op_to_numeric_cmp(op)?;
                return pyrust_core::dispatch_numeric_compare(cmp, lv, rv)?
                    .ok()
                    .map(Value::bool_);
            }
            _ => {}
        }
    }
    match (lv.kind(), op, rv.kind()) {
        (ValueKind::Bool(a), Ne, ValueKind::Bool(b)) => Some(Value::bool_(a != b)),
        (ValueKind::None, Is, ValueKind::None) => Some(Value::bool_(true)),
        (ValueKind::None, IsNot, ValueKind::None) => Some(Value::bool_(false)),
        (ValueKind::None, Is, _) if is_mir_pure_const(rv) => Some(Value::bool_(false)),
        (ValueKind::None, IsNot, _) if is_mir_pure_const(rv) => Some(Value::bool_(true)),
        (_, Is, ValueKind::None) if is_mir_pure_const(lv) => Some(Value::bool_(false)),
        (_, IsNot, ValueKind::None) if is_mir_pure_const(lv) => Some(Value::bool_(true)),
        (ValueKind::None, Eq, ValueKind::None) => Some(Value::bool_(true)),
        (ValueKind::None, Ne, ValueKind::None) => Some(Value::bool_(false)),
        (ValueKind::Bool(a), Is, ValueKind::Bool(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Bool(a), IsNot, ValueKind::Bool(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Bool(_), Is, _) if is_mir_pure_const(rv) => Some(Value::bool_(false)),
        (ValueKind::Bool(_), IsNot, _) if is_mir_pure_const(rv) => Some(Value::bool_(true)),
        (_, Is, ValueKind::Bool(_)) if is_mir_pure_const(lv) => Some(Value::bool_(false)),
        (_, IsNot, ValueKind::Bool(_)) if is_mir_pure_const(lv) => Some(Value::bool_(true)),
        (ValueKind::Float(a), Eq, ValueKind::Float(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Float(a), Ne, ValueKind::Float(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Float(a), Lt, ValueKind::Float(b)) => Some(Value::bool_(a < b)),
        (ValueKind::Float(a), Le, ValueKind::Float(b)) => Some(Value::bool_(a <= b)),
        (ValueKind::Float(a), Gt, ValueKind::Float(b)) => Some(Value::bool_(a > b)),
        (ValueKind::Float(a), Ge, ValueKind::Float(b)) => Some(Value::bool_(a >= b)),
        (ValueKind::Str(a), Lt, ValueKind::Str(b)) => Some(Value::bool_(a < b)),
        (ValueKind::Str(a), Le, ValueKind::Str(b)) => Some(Value::bool_(a <= b)),
        (ValueKind::Str(a), Gt, ValueKind::Str(b)) => Some(Value::bool_(a > b)),
        (ValueKind::Str(a), Ge, ValueKind::Str(b)) => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

/// Evaluate a pure unary operation on a compile-time constant using canonical
/// runtime unary semantics (issue #458).
#[inline]
pub fn eval_mir_unary(op: UnaryOp, v: &Value) -> Option<Value> {
    if !is_mir_pure_const(v) {
        return None;
    }
    crate::interpreter::eval_builtin_unary(op, v.clone()).ok()
}

/// True when `MirOp::BinOp` with operand types `(lt, rt)` is proven pure,
/// non-reentrant, and incapable of raising any Python exception.
pub fn is_proven_non_raising_binop(op: BinaryOp, lt: &MirType, rt: &MirType) -> bool {
    use BinaryOp::*;
    if lt.is_int() && rt.is_int() {
        return matches!(
            op,
            Add | Sub | Mul | BitAnd | BitOr | BitXor | Eq | Ne | Lt | Le | Gt | Ge
        );
    }
    if lt.is_float() && rt.is_float() {
        return matches!(op, Add | Sub | Mul | Eq | Ne | Lt | Le | Gt | Ge);
    }
    if lt.is_str() && rt.is_str() {
        return matches!(op, Add | Eq | Ne | Lt | Le | Gt | Ge);
    }
    false
}

/// True when `MirOp::UnaryOp` with operand type `arg_ty` is proven pure,
/// non-reentrant, and incapable of raising any Python exception.
pub fn is_proven_non_raising_unary(op: UnaryOp, arg_ty: &MirType) -> bool {
    match op {
        UnaryOp::Not => arg_ty.is_pure_truthy_primitive(),
        UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot => arg_ty.is_int(),
    }
}
