//! Unified 5-type numeric protocol slots (`PyNumberMethods`-style) for PyRust.
//!
//! Centralizes arithmetic, bitwise, shift, unary, truthiness, and comparison
//! semantics for all five built-in numeric kinds (`int`, `bool`, `float`,
//! `BigInt`, and `complex`) in one canonical module (issue #458).
//!
//! Dispatch uses a flat `Copy` enum ([`NumericSlot`]) rather than
//! `Box<dyn NumericOps>`, avoiding heap allocation and virtual calls on the
//! numeric path.

mod binop;
mod compare;
mod complex;
mod int_float;
mod unary;

#[cfg(test)]
mod tests;

use std::cmp::Ordering;

use crate::errors::{PyError, Result};
use crate::object_model::{PyBigInt, Value, ValueKind};

pub use compare::{numeric_eq, numeric_partial_cmp};
pub use int_float::{
    bigint_divmod_floor, bigint_float_cmp, bigint_float_eq, bigint_to_float_or_overflow,
    float_divmod, int_float_cmp, int_float_eq, int_pow_promoting, py_mod_i64, value_from_bigint,
    value_to_bigint,
};

/// Binary numeric operator understood by [`NumericOps`].
///
/// Defined in `pyrust-core` so runtime, compiler constant folding, and builtins
/// share one canonical dispatch surface without depending on `pyrust::ast`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericBinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    LShift,
    RShift,
    BitAnd,
    BitOr,
    BitXor,
}

impl NumericBinOp {
    /// CPython operator token used in `"unsupported operand type(s) for OP"` errors.
    #[inline]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::FloorDiv => "//",
            Self::Mod => "%",
            Self::Pow => "** or pow()",
            Self::LShift => "<<",
            Self::RShift => ">>",
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::BitXor => "^",
        }
    }
}

/// Unary numeric operator understood by [`NumericOps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericUnaryOp {
    Pos,
    Neg,
    Invert,
    Abs,
}

impl NumericUnaryOp {
    /// CPython operator token used in `"bad operand type for ..."` errors.
    #[inline]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Pos => "unary +",
            Self::Neg => "unary -",
            Self::Invert => "unary ~",
            Self::Abs => "abs()",
        }
    }
}

/// Rich comparison operator understood by [`NumericOps::nb_compare`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericCmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl NumericCmpOp {
    /// CPython operator token used in `"'OP' not supported between instances of ..."` errors.
    #[inline]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }

    #[inline]
    pub(super) const fn matches_ordering(self, ord: Ordering) -> bool {
        match self {
            Self::Eq => ord.is_eq(),
            Self::Ne => ord.is_ne(),
            Self::Lt => ord.is_lt(),
            Self::Le => ord.is_le(),
            Self::Gt => ord.is_gt(),
            Self::Ge => ord.is_ge(),
        }
    }
}

/// Numeric protocol slots, analogous to CPython's `PyNumberMethods` (issue #458).
///
/// Every built-in numeric kind (`int`, `bool`, `float`, `BigInt`, `complex`)
/// implements this protocol through the flat [`NumericSlot`] enum.
///
/// Binary and comparison slots return `Option<Result<...>>`:
/// - `None` corresponds to CPython's `NotImplemented` (the RHS is not one of the
///   five built-in numeric kinds, so callers fall through to sequence /
///   container / dunder handling).
/// - `Some(Ok(v))` is the computed result.
/// - `Some(Err(e))` is a Python exception (`ZeroDivisionError`, `OverflowError`,
///   `ValueError`, or `TypeError` for an unsupported numeric pair such as
///   `complex // int`, `float & int`, or `complex < int`).
pub trait NumericOps {
    fn nb_add(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_sub(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_mul(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_div(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_floordiv(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_mod(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_pow(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_lshift(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_rshift(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_and(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_or(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_xor(&self, rhs: &Value) -> Option<Result<Value>>;

    fn nb_negative(&self) -> Result<Value>;
    fn nb_positive(&self) -> Result<Value>;
    fn nb_invert(&self) -> Result<Value>;
    fn nb_absolute(&self) -> Result<Value>;
    fn nb_bool(&self) -> bool;

    fn nb_compare(&self, op: NumericCmpOp, rhs: &Value) -> Option<Result<bool>>;
}

/// Flat zero-allocation classification of a built-in numeric operand.
///
/// `BigInt` borrows from the source [`Value`], so classifying any of the five
/// numeric kinds is `Copy` and performs zero heap allocations.
#[derive(Debug, Clone, Copy)]
pub enum NumericSlot<'a> {
    Int(i64),
    Bool(bool),
    Float(f64),
    BigInt(&'a PyBigInt),
    Complex(f64, f64),
}

impl<'a> NumericSlot<'a> {
    /// Classify a [`Value`] into one of the five built-in numeric kinds, or
    /// `None` for non-numeric values.
    #[inline]
    pub fn from_value(v: &'a Value) -> Option<Self> {
        match v.kind() {
            ValueKind::Int(n) => Some(Self::Int(n)),
            ValueKind::Bool(b) => Some(Self::Bool(b)),
            ValueKind::Float(f) => Some(Self::Float(f)),
            ValueKind::BigInt(b) => Some(Self::BigInt(b)),
            ValueKind::Complex(re, im) => Some(Self::Complex(re, im)),
            _ => None,
        }
    }

    /// Canonical Python type name for diagnostics (`"int"`, `"bool"`, `"float"`,
    /// `"complex"`).
    #[inline]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Int(_) | Self::BigInt(_) => "int",
            Self::Bool(_) => "bool",
            Self::Float(_) => "float",
            Self::Complex(_, _) => "complex",
        }
    }

    #[inline]
    pub(super) fn is_complex(&self) -> bool {
        matches!(self, Self::Complex(_, _))
    }

    #[inline]
    pub(super) fn is_float_or_complex(&self) -> bool {
        matches!(self, Self::Float(_) | Self::Complex(_, _))
    }

    /// Coerce `Int` or `Bool` to `i64`; returns `None` for `Float`/`BigInt`/`Complex`.
    #[inline]
    pub(super) fn as_i64_coerced(&self) -> Option<i64> {
        match *self {
            Self::Int(n) => Some(n),
            Self::Bool(b) => Some(b as i64),
            _ => None,
        }
    }

    /// Coerce `Int`, `Bool`, `Float`, or `BigInt` to `f64`, raising
    /// `OverflowError` for out-of-range `BigInt`s.
    pub(super) fn to_float(self) -> Result<f64> {
        match self {
            Self::Int(n) => Ok(n as f64),
            Self::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
            Self::Float(f) => Ok(f),
            Self::BigInt(b) => bigint_to_float_or_overflow(b),
            Self::Complex(_, _) => unreachable!("NumericSlot::to_float called on Complex"),
        }
    }

    /// Coerce any of the five numeric kinds to `(real, imag)` complex components.
    pub(super) fn to_complex(self) -> Result<(f64, f64)> {
        match self {
            Self::Complex(re, im) => Ok((re, im)),
            Self::Int(n) => Ok((n as f64, 0.0)),
            Self::Bool(b) => Ok((if b { 1.0 } else { 0.0 }, 0.0)),
            Self::Float(f) => Ok((f, 0.0)),
            Self::BigInt(b) => Ok((bigint_to_float_or_overflow(b)?, 0.0)),
        }
    }

    /// Coerce an integer slot (`Int`, `Bool`, or `BigInt`) to `PyBigInt`.
    pub(super) fn to_bigint(self) -> PyBigInt {
        match self {
            Self::Int(n) => PyBigInt::from(n),
            Self::Bool(b) => PyBigInt::from(b as i64),
            Self::BigInt(b) => b.clone(),
            Self::Float(_) | Self::Complex(_, _) => {
                unreachable!("NumericSlot::to_bigint called on Float/Complex")
            }
        }
    }

    pub(super) fn unsupported_binop(&self, op: NumericBinOp, rhs: &NumericSlot<'_>) -> PyError {
        crate::type_err!(
            "unsupported operand type(s) for {}: '{}' and '{}'",
            op.symbol(),
            self.type_name(),
            rhs.type_name()
        )
    }
}

impl NumericOps for NumericSlot<'_> {
    fn nb_add(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_add(self, &r))
    }

    fn nb_sub(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_sub(self, &r))
    }

    fn nb_mul(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_mul(self, &r))
    }

    fn nb_div(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_div(self, &r))
    }

    fn nb_floordiv(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_floordiv(self, &r))
    }

    fn nb_mod(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_mod(self, &r))
    }

    fn nb_pow(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_pow(self, &r))
    }

    fn nb_lshift(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_lshift(self, &r))
    }

    fn nb_rshift(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_rshift(self, &r))
    }

    fn nb_and(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_and(self, &r))
    }

    fn nb_or(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_or(self, &r))
    }

    fn nb_xor(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(binop::slot_xor(self, &r))
    }

    fn nb_negative(&self) -> Result<Value> {
        unary::slot_negative(self)
    }

    fn nb_positive(&self) -> Result<Value> {
        unary::slot_positive(self)
    }

    fn nb_invert(&self) -> Result<Value> {
        unary::slot_invert(self)
    }

    fn nb_absolute(&self) -> Result<Value> {
        unary::slot_absolute(self)
    }

    fn nb_bool(&self) -> bool {
        unary::slot_bool(self)
    }

    fn nb_compare(&self, op: NumericCmpOp, rhs: &Value) -> Option<Result<bool>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(compare::slot_compare(self, op, &r))
    }
}

// ── Top-level dispatch entry points ──────────────────────────────────────────

/// Dispatch a binary numeric operation through the LHS's [`NumericOps`] slot.
///
/// Returns `None` (`NotImplemented`) when either operand is not one of the five
/// built-in numeric kinds (`int`, `bool`, `float`, `BigInt`, `complex`).
#[inline]
pub fn dispatch_numeric_binop(op: NumericBinOp, lhs: &Value, rhs: &Value) -> Option<Result<Value>> {
    let l = NumericSlot::from_value(lhs)?;
    match op {
        NumericBinOp::Add => l.nb_add(rhs),
        NumericBinOp::Sub => l.nb_sub(rhs),
        NumericBinOp::Mul => l.nb_mul(rhs),
        NumericBinOp::Div => l.nb_div(rhs),
        NumericBinOp::FloorDiv => l.nb_floordiv(rhs),
        NumericBinOp::Mod => l.nb_mod(rhs),
        NumericBinOp::Pow => l.nb_pow(rhs),
        NumericBinOp::LShift => l.nb_lshift(rhs),
        NumericBinOp::RShift => l.nb_rshift(rhs),
        NumericBinOp::BitAnd => l.nb_and(rhs),
        NumericBinOp::BitOr => l.nb_or(rhs),
        NumericBinOp::BitXor => l.nb_xor(rhs),
    }
}

/// Dispatch a unary numeric operation (`+`, `-`, `~`, `abs`) through
/// [`NumericOps`].
///
/// Returns `None` when `value` is not one of the five built-in numeric kinds.
/// For unary `+` on non-`bool` numerics (`int`, `float`, `BigInt`, `complex`),
/// clones `value` directly so `+x is x` identity holds even for NaN floats
/// (issue #2911).
#[inline]
pub fn dispatch_numeric_unary(op: NumericUnaryOp, value: &Value) -> Option<Result<Value>> {
    let slot = NumericSlot::from_value(value)?;
    Some(match op {
        NumericUnaryOp::Pos => {
            if matches!(slot, NumericSlot::Bool(_)) {
                slot.nb_positive()
            } else {
                Ok(value.clone())
            }
        }
        NumericUnaryOp::Neg => slot.nb_negative(),
        NumericUnaryOp::Invert => slot.nb_invert(),
        NumericUnaryOp::Abs => slot.nb_absolute(),
    })
}

/// Dispatch numeric truthiness (`nb_bool`) for built-in numeric values.
#[inline]
pub fn dispatch_numeric_bool(value: &Value) -> Option<bool> {
    NumericSlot::from_value(value).map(|slot| slot.nb_bool())
}

/// Dispatch a numeric rich comparison (`==`, `!=`, `<`, `<=`, `>`, `>=`)
/// through [`NumericOps::nb_compare`].
///
/// Returns `None` when either operand is not a built-in numeric value.
#[inline]
pub fn dispatch_numeric_compare(
    op: NumericCmpOp,
    lhs: &Value,
    rhs: &Value,
) -> Option<Result<bool>> {
    NumericSlot::from_value(lhs)?.nb_compare(op, rhs)
}
