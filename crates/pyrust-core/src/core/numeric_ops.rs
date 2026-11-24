//! Unified 5-type numeric protocol slots (`PyNumberMethods`-style) for PyRust.
//!
//! Centralizes arithmetic, bitwise, shift, unary, truthiness, and comparison
//! semantics for all five built-in numeric kinds (`int`, `bool`, `float`,
//! `BigInt`, and `complex`) in one canonical module (issue #458).
//!
//! Dispatch uses a flat `Copy` enum ([`NumericSlot`]) rather than
//! `Box<dyn NumericOps>`, avoiding heap allocation and virtual calls on the
//! numeric path.

use std::cmp::Ordering;

use num_traits::{FromPrimitive, ToPrimitive, Zero};

use crate::errors::{PyError, Result};
use crate::object_model::{PyBigInt, PyBigIntSign, PyPow, Value, ValueKind};

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
    const fn matches_ordering(self, ord: Ordering) -> bool {
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
    fn is_complex(&self) -> bool {
        matches!(self, Self::Complex(_, _))
    }

    #[inline]
    fn is_float_or_complex(&self) -> bool {
        matches!(self, Self::Float(_) | Self::Complex(_, _))
    }

    /// Coerce `Int` or `Bool` to `i64`; returns `None` for `Float`/`BigInt`/`Complex`.
    #[inline]
    fn as_i64_coerced(&self) -> Option<i64> {
        match *self {
            Self::Int(n) => Some(n),
            Self::Bool(b) => Some(b as i64),
            _ => None,
        }
    }

    /// Coerce `Int`, `Bool`, `Float`, or `BigInt` to `f64`, raising
    /// `OverflowError` for out-of-range `BigInt`s.
    fn to_float(self) -> Result<f64> {
        match self {
            Self::Int(n) => Ok(n as f64),
            Self::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
            Self::Float(f) => Ok(f),
            Self::BigInt(b) => bigint_to_float_or_overflow(b),
            Self::Complex(_, _) => unreachable!("NumericSlot::to_float called on Complex"),
        }
    }

    /// Coerce any of the five numeric kinds to `(real, imag)` complex components.
    fn to_complex(self) -> Result<(f64, f64)> {
        match self {
            Self::Complex(re, im) => Ok((re, im)),
            Self::Int(n) => Ok((n as f64, 0.0)),
            Self::Bool(b) => Ok((if b { 1.0 } else { 0.0 }, 0.0)),
            Self::Float(f) => Ok((f, 0.0)),
            Self::BigInt(b) => Ok((bigint_to_float_or_overflow(b)?, 0.0)),
        }
    }

    /// Coerce an integer slot (`Int`, `Bool`, or `BigInt`) to `PyBigInt`.
    fn to_bigint(self) -> PyBigInt {
        match self {
            Self::Int(n) => PyBigInt::from(n),
            Self::Bool(b) => PyBigInt::from(b as i64),
            Self::BigInt(b) => b.clone(),
            Self::Float(_) | Self::Complex(_, _) => {
                unreachable!("NumericSlot::to_bigint called on Float/Complex")
            }
        }
    }

    fn unsupported_binop(&self, op: NumericBinOp, rhs: &NumericSlot<'_>) -> PyError {
        crate::type_err!(
            "unsupported operand type(s) for {}: '{}' and '{}'",
            op.symbol(),
            self.type_name(),
            rhs.type_name()
        )
    }

    fn eq_slot(&self, rhs: &NumericSlot<'_>) -> bool {
        match (*self, *rhs) {
            (NumericSlot::Int(a), NumericSlot::Int(b)) => a == b,
            (NumericSlot::Bool(a), NumericSlot::Bool(b)) => a == b,
            (NumericSlot::Int(a), NumericSlot::Bool(b)) => a == (b as i64),
            (NumericSlot::Bool(a), NumericSlot::Int(b)) => (a as i64) == b,
            (NumericSlot::Float(a), NumericSlot::Float(b)) => a == b,
            (NumericSlot::Int(a), NumericSlot::Float(b)) => int_float_eq(a, b),
            (NumericSlot::Float(a), NumericSlot::Int(b)) => int_float_eq(b, a),
            (NumericSlot::Bool(a), NumericSlot::Float(b)) => int_float_eq(a as i64, b),
            (NumericSlot::Float(a), NumericSlot::Bool(b)) => int_float_eq(b as i64, a),
            (NumericSlot::BigInt(a), NumericSlot::BigInt(b)) => a == b,
            (NumericSlot::BigInt(a), NumericSlot::Int(b)) => *a == PyBigInt::from(b),
            (NumericSlot::Int(a), NumericSlot::BigInt(b)) => PyBigInt::from(a) == *b,
            (NumericSlot::BigInt(a), NumericSlot::Bool(b)) => *a == PyBigInt::from(b as i64),
            (NumericSlot::Bool(a), NumericSlot::BigInt(b)) => PyBigInt::from(a as i64) == *b,
            (NumericSlot::BigInt(a), NumericSlot::Float(b)) => bigint_float_eq(a, b),
            (NumericSlot::Float(a), NumericSlot::BigInt(b)) => bigint_float_eq(b, a),
            // Plain component equality — NO NaN bit-equality fallback here,
            // matching CPython (`complex(nan, 0) == complex(nan, 0)` is False).
            (NumericSlot::Complex(ar, ai), NumericSlot::Complex(br, bi)) => ar == br && ai == bi,
            (NumericSlot::Complex(ar, ai), NumericSlot::Float(b))
            | (NumericSlot::Float(b), NumericSlot::Complex(ar, ai)) => ai == 0.0 && ar == b,
            (NumericSlot::Complex(ar, ai), NumericSlot::Int(b))
            | (NumericSlot::Int(b), NumericSlot::Complex(ar, ai)) => {
                ai == 0.0 && int_float_eq(b, ar)
            }
            (NumericSlot::Complex(ar, ai), NumericSlot::Bool(b))
            | (NumericSlot::Bool(b), NumericSlot::Complex(ar, ai)) => {
                ai == 0.0 && int_float_eq(b as i64, ar)
            }
            (NumericSlot::Complex(ar, ai), NumericSlot::BigInt(b))
            | (NumericSlot::BigInt(b), NumericSlot::Complex(ar, ai)) => {
                ai == 0.0 && bigint_float_eq(b, ar)
            }
        }
    }

    /// Exact partial ordering for non-complex numeric pairs.
    ///
    /// Returns `Err(TypeError)` when either operand is `Complex`, `Ok(None)`
    /// when a `NaN` float is present, and `Ok(Some(Ordering))` otherwise.
    fn partial_cmp_slot(
        &self,
        op: NumericCmpOp,
        rhs: &NumericSlot<'_>,
    ) -> Result<Option<Ordering>> {
        if self.is_complex() || rhs.is_complex() {
            return Err(crate::type_err!(
                "'{}' not supported between instances of '{}' and '{}'",
                op.symbol(),
                self.type_name(),
                rhs.type_name()
            ));
        }
        Ok(match (*self, *rhs) {
            (NumericSlot::Int(a), NumericSlot::Int(b)) => Some(a.cmp(&b)),
            (NumericSlot::Bool(a), NumericSlot::Bool(b)) => Some(a.cmp(&b)),
            (NumericSlot::Int(a), NumericSlot::Bool(b)) => Some(a.cmp(&(b as i64))),
            (NumericSlot::Bool(a), NumericSlot::Int(b)) => Some((a as i64).cmp(&b)),
            (NumericSlot::Float(a), NumericSlot::Float(b)) => a.partial_cmp(&b),
            (NumericSlot::Int(a), NumericSlot::Float(b)) => int_float_cmp(a, b),
            (NumericSlot::Float(a), NumericSlot::Int(b)) => {
                int_float_cmp(b, a).map(Ordering::reverse)
            }
            (NumericSlot::Bool(a), NumericSlot::Float(b)) => int_float_cmp(a as i64, b),
            (NumericSlot::Float(a), NumericSlot::Bool(b)) => {
                int_float_cmp(b as i64, a).map(Ordering::reverse)
            }
            (NumericSlot::BigInt(a), NumericSlot::BigInt(b)) => Some(a.cmp(b)),
            (NumericSlot::BigInt(a), NumericSlot::Int(b)) => Some(a.cmp(&PyBigInt::from(b))),
            (NumericSlot::Int(a), NumericSlot::BigInt(b)) => Some(PyBigInt::from(a).cmp(b)),
            (NumericSlot::BigInt(a), NumericSlot::Bool(b)) => {
                Some(a.cmp(&PyBigInt::from(b as i64)))
            }
            (NumericSlot::Bool(a), NumericSlot::BigInt(b)) => Some(PyBigInt::from(a as i64).cmp(b)),
            (NumericSlot::BigInt(a), NumericSlot::Float(b)) => bigint_float_cmp(a, b),
            (NumericSlot::Float(a), NumericSlot::BigInt(b)) => {
                bigint_float_cmp(b, a).map(Ordering::reverse)
            }
            (NumericSlot::Complex(_, _), _) | (_, NumericSlot::Complex(_, _)) => {
                unreachable!("complex operands handled above")
            }
        })
    }
}

impl NumericOps for NumericSlot<'_> {
    fn nb_add(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_complex() || r.is_complex() {
            return Some((|| {
                let a = self.to_complex()?;
                let b = r.to_complex()?;
                Ok(Value::complex(a.0 + b.0, a.1 + b.1))
            })());
        }
        Some(match (self.as_i64_coerced(), r.as_i64_coerced()) {
            (Some(a), Some(b)) => Ok(match a.checked_add(b) {
                Some(s) => Value::int(s),
                None => Value::bigint(PyBigInt::from(a) + PyBigInt::from(b)),
            }),
            _ => match (*self, r) {
                (NumericSlot::Float(a), NumericSlot::Float(b)) => Ok(Value::float(a + b)),
                (NumericSlot::Float(a), _) => r.to_float().map(|b| Value::float(a + b)),
                (_, NumericSlot::Float(b)) => self.to_float().map(|a| Value::float(a + b)),
                _ => Ok(value_from_bigint(self.to_bigint() + r.to_bigint())),
            },
        })
    }

    fn nb_sub(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_complex() || r.is_complex() {
            return Some((|| {
                let a = self.to_complex()?;
                let b = r.to_complex()?;
                Ok(Value::complex(a.0 - b.0, a.1 - b.1))
            })());
        }
        Some(match (self.as_i64_coerced(), r.as_i64_coerced()) {
            (Some(a), Some(b)) => Ok(match a.checked_sub(b) {
                Some(s) => Value::int(s),
                None => Value::bigint(PyBigInt::from(a) - PyBigInt::from(b)),
            }),
            _ => match (*self, r) {
                (NumericSlot::Float(a), NumericSlot::Float(b)) => Ok(Value::float(a - b)),
                (NumericSlot::Float(a), _) => r.to_float().map(|b| Value::float(a - b)),
                (_, NumericSlot::Float(b)) => self.to_float().map(|a| Value::float(a - b)),
                _ => Ok(value_from_bigint(self.to_bigint() - r.to_bigint())),
            },
        })
    }

    fn nb_mul(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_complex() || r.is_complex() {
            return Some((|| {
                let a = self.to_complex()?;
                let b = r.to_complex()?;
                // (ar+ai*j) * (br+bi*j) = (ar*br - ai*bi) + (ar*bi + ai*br)j
                Ok(Value::complex(a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0))
            })());
        }
        Some(match (self.as_i64_coerced(), r.as_i64_coerced()) {
            (Some(a), Some(b)) => Ok(match a.checked_mul(b) {
                Some(s) => Value::int(s),
                None => Value::bigint(PyBigInt::from(a) * PyBigInt::from(b)),
            }),
            _ => match (*self, r) {
                (NumericSlot::Float(a), NumericSlot::Float(b)) => Ok(Value::float(a * b)),
                (NumericSlot::Float(a), _) => r.to_float().map(|b| Value::float(a * b)),
                (_, NumericSlot::Float(b)) => self.to_float().map(|a| Value::float(a * b)),
                _ => Ok(value_from_bigint(self.to_bigint() * r.to_bigint())),
            },
        })
    }

    fn nb_div(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_complex() || r.is_complex() {
            return Some((|| {
                let a = self.to_complex()?;
                let b = r.to_complex()?;
                complex_div(a, b)
            })());
        }
        Some(match (self.as_i64_coerced(), r.as_i64_coerced()) {
            // Small int/int fast path: when both operands are exactly
            // representable as f64 (|n| < 2^53), IEEE-754 division is itself
            // correctly rounded and matches CPython byte-for-byte — no BigInt
            // allocation. Larger magnitudes route through the exact divider.
            (Some(a), Some(b)) => {
                if b == 0 {
                    Err(crate::zerodiv_err!("division by zero"))
                } else if a.unsigned_abs() < (1 << 53) && b.unsigned_abs() < (1 << 53) {
                    Ok(Value::float(a as f64 / b as f64))
                } else {
                    bigint_true_divide(&PyBigInt::from(a), &PyBigInt::from(b)).map(Value::float)
                }
            }
            _ if !matches!(*self, NumericSlot::Float(_)) && !matches!(r, NumericSlot::Float(_)) => {
                bigint_true_divide(&self.to_bigint(), &r.to_bigint()).map(Value::float)
            }
            // At least one Float operand: CPython divides as doubles and the
            // ZeroDivisionError wording is "float division by zero".
            _ => (|| {
                let a = self.to_float()?;
                let b = r.to_float()?;
                if b == 0.0 {
                    return Err(crate::zerodiv_err!("float division by zero"));
                }
                Ok(Value::float(a / b))
            })(),
        })
    }

    fn nb_floordiv(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_complex() || r.is_complex() {
            return Some(Err(self.unsupported_binop(NumericBinOp::FloorDiv, &r)));
        }
        Some(match (self.as_i64_coerced(), r.as_i64_coerced()) {
            (Some(a), Some(b)) => {
                if b == 0 {
                    Err(crate::zerodiv_err!("integer division or modulo by zero"))
                } else {
                    let modulo = py_mod_i64(a, b);
                    // `a - modulo` / `a_adj / b` can overflow near i64::MIN;
                    // promote to BigInt for exact floor division (#485).
                    if let Some(q) = a.checked_sub(modulo).and_then(|a_adj| a_adj.checked_div(b)) {
                        Ok(Value::int(q))
                    } else {
                        let (q, _) = bigint_divmod_floor(&PyBigInt::from(a), &PyBigInt::from(b));
                        Ok(Value::bigint(q))
                    }
                }
            }
            _ if !matches!(*self, NumericSlot::Float(_)) && !matches!(r, NumericSlot::Float(_)) => {
                let a = self.to_bigint();
                let b = r.to_bigint();
                if b.is_zero() {
                    Err(crate::zerodiv_err!("integer division or modulo by zero"))
                } else {
                    let (q, _) = bigint_divmod_floor(&a, &b);
                    Ok(value_from_bigint(q))
                }
            }
            // At least one Float operand: CPython fmod-based float_divmod so
            // infinities/signed zeros match and `a // b == divmod(a, b)[0]` (#2025).
            _ => (|| {
                let a = self.to_float()?;
                let b = r.to_float()?;
                if b == 0.0 {
                    return Err(crate::zerodiv_err!("float floor division by zero"));
                }
                let (div, _) = float_divmod(a, b);
                Ok(Value::float(div))
            })(),
        })
    }

    fn nb_mod(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_complex() || r.is_complex() {
            return Some(Err(self.unsupported_binop(NumericBinOp::Mod, &r)));
        }
        Some(match (self.as_i64_coerced(), r.as_i64_coerced()) {
            (Some(a), Some(b)) => {
                if b == 0 {
                    Err(crate::zerodiv_err!("integer modulo by zero"))
                } else {
                    Ok(Value::int(py_mod_i64(a, b)))
                }
            }
            _ if !matches!(*self, NumericSlot::Float(_)) && !matches!(r, NumericSlot::Float(_)) => {
                let a = self.to_bigint();
                let b = r.to_bigint();
                if b.is_zero() {
                    Err(crate::zerodiv_err!("integer modulo by zero"))
                } else {
                    let (_, rem) = bigint_divmod_floor(&a, &b);
                    Ok(value_from_bigint(rem))
                }
            }
            _ => (|| {
                let a = self.to_float()?;
                let b = r.to_float()?;
                if b == 0.0 {
                    return Err(crate::zerodiv_err!("float modulo"));
                }
                let mut rem = a % b;
                if rem == 0.0 {
                    // CPython float_rem: zero result copies sign of divisor.
                    rem = rem.copysign(b);
                } else if rem.signum() != b.signum() {
                    rem += b;
                }
                Ok(Value::float(rem))
            })(),
        })
    }

    fn nb_pow(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_complex() || r.is_complex() {
            return Some((|| {
                let (zr, zi) = self.to_complex()?;
                let (wr, wi) = r.to_complex()?;
                complex_pow(zr, zi, wr, wi)
            })());
        }
        Some(match (self.as_i64_coerced(), r.as_i64_coerced()) {
            // Integer ** non-negative integer stays in the int domain
            // (BigInt promotion on overflow, #421).
            (Some(a), Some(b)) if b >= 0 => Ok(int_pow_promoting(a, b)),
            (None, Some(b)) if matches!(*self, NumericSlot::BigInt(_)) && b >= 0 => {
                Ok(value_from_bigint(PyPow::pow(self.to_bigint(), b as u64)))
            }
            (Some(a), None) if matches!(r, NumericSlot::BigInt(big) if big.sign() != PyBigIntSign::Minus) =>
            {
                let NumericSlot::BigInt(b) = r else {
                    unreachable!()
                };
                match b.to_u64_digits().1.as_slice() {
                    [exp] => Ok(value_from_bigint(PyPow::pow(PyBigInt::from(a), *exp))),
                    [] => Ok(Value::int(1)),
                    _ => Err(crate::overflow_err!("exponent too large for ** to compute")),
                }
            }
            (None, None)
                if matches!(*self, NumericSlot::BigInt(_))
                    && matches!(r, NumericSlot::BigInt(big) if big.sign() != PyBigIntSign::Minus) =>
            {
                let NumericSlot::BigInt(b) = r else {
                    unreachable!()
                };
                match b.to_u64_digits().1.as_slice() {
                    [exp] => Ok(value_from_bigint(PyPow::pow(self.to_bigint(), *exp))),
                    [] => Ok(Value::int(1)),
                    _ => Err(crate::overflow_err!("exponent too large for ** to compute")),
                }
            }
            // Float base/exponent, or integer with a negative exponent:
            // float exponentiation (with CPython's negative-real -> complex
            // promotion and the 0.0 ** negative ZeroDivisionError).
            _ => (|| {
                let a = self.to_float()?;
                let b = r.to_float()?;
                if a == 0.0 && b < 0.0 && b.is_finite() {
                    return Err(crate::zerodiv_err!(
                        "0.0 cannot be raised to a negative power"
                    ));
                }
                let result = a.powf(b);
                if a < 0.0 && b.is_finite() && result.is_nan() {
                    let abs_val = a.abs().powf(b);
                    let angle = std::f64::consts::PI * b;
                    Ok(Value::complex(abs_val * angle.cos(), abs_val * angle.sin()))
                } else {
                    Ok(Value::float(result))
                }
            })(),
        })
    }

    fn nb_lshift(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_float_or_complex() || r.is_float_or_complex() {
            return Some(Err(self.unsupported_binop(NumericBinOp::LShift, &r)));
        }
        Some(slot_lshift(self, &r))
    }

    fn nb_rshift(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_float_or_complex() || r.is_float_or_complex() {
            return Some(Err(self.unsupported_binop(NumericBinOp::RShift, &r)));
        }
        Some(slot_rshift(self, &r))
    }

    fn nb_and(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_float_or_complex() || r.is_float_or_complex() {
            return Some(Err(self.unsupported_binop(NumericBinOp::BitAnd, &r)));
        }
        if let (NumericSlot::Bool(a), NumericSlot::Bool(b)) = (*self, r) {
            return Some(Ok(Value::bool_(a & b)));
        }
        if let (Some(a), Some(b)) = (self.as_i64_coerced(), r.as_i64_coerced()) {
            return Some(Ok(Value::int(a & b)));
        }
        Some(Ok(value_from_bigint(self.to_bigint() & r.to_bigint())))
    }

    fn nb_or(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_float_or_complex() || r.is_float_or_complex() {
            return Some(Err(self.unsupported_binop(NumericBinOp::BitOr, &r)));
        }
        if let (NumericSlot::Bool(a), NumericSlot::Bool(b)) = (*self, r) {
            return Some(Ok(Value::bool_(a | b)));
        }
        if let (Some(a), Some(b)) = (self.as_i64_coerced(), r.as_i64_coerced()) {
            return Some(Ok(Value::int(a | b)));
        }
        Some(Ok(value_from_bigint(self.to_bigint() | r.to_bigint())))
    }

    fn nb_xor(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = NumericSlot::from_value(rhs)?;
        if self.is_float_or_complex() || r.is_float_or_complex() {
            return Some(Err(self.unsupported_binop(NumericBinOp::BitXor, &r)));
        }
        if let (NumericSlot::Bool(a), NumericSlot::Bool(b)) = (*self, r) {
            return Some(Ok(Value::bool_(a ^ b)));
        }
        if let (Some(a), Some(b)) = (self.as_i64_coerced(), r.as_i64_coerced()) {
            return Some(Ok(Value::int(a ^ b)));
        }
        Some(Ok(value_from_bigint(self.to_bigint() ^ r.to_bigint())))
    }

    fn nb_negative(&self) -> Result<Value> {
        Ok(match *self {
            NumericSlot::Int(n) => match n.checked_neg() {
                Some(r) => Value::int(r),
                None => Value::bigint(-PyBigInt::from(n)),
            },
            NumericSlot::Bool(b) => Value::int(if b { -1 } else { 0 }),
            NumericSlot::Float(f) => Value::float(-f),
            NumericSlot::BigInt(b) => value_from_bigint(-b),
            NumericSlot::Complex(re, im) => Value::complex(-re, -im),
        })
    }

    fn nb_positive(&self) -> Result<Value> {
        Ok(match *self {
            NumericSlot::Int(n) => Value::int(n),
            NumericSlot::Bool(b) => Value::int(if b { 1 } else { 0 }),
            NumericSlot::Float(f) => Value::float(f),
            NumericSlot::BigInt(b) => Value::bigint(b.clone()),
            NumericSlot::Complex(re, im) => Value::complex(re, im),
        })
    }

    fn nb_invert(&self) -> Result<Value> {
        match *self {
            NumericSlot::Int(n) => Ok(Value::int(!n)),
            NumericSlot::Bool(b) => Ok(Value::int(if b { -2 } else { -1 })),
            NumericSlot::BigInt(b) => Ok(value_from_bigint(!b)),
            NumericSlot::Float(_) | NumericSlot::Complex(_, _) => Err(crate::type_err!(
                "bad operand type for unary ~: '{}'",
                self.type_name()
            )),
        }
    }

    fn nb_absolute(&self) -> Result<Value> {
        Ok(match *self {
            NumericSlot::Int(n) => match n.checked_abs() {
                Some(r) => Value::int(r),
                None => Value::bigint(-PyBigInt::from(n)),
            },
            NumericSlot::Bool(b) => Value::int(if b { 1 } else { 0 }),
            NumericSlot::Float(f) => Value::float(f.abs()),
            NumericSlot::BigInt(b) => value_from_bigint(magnitude(b)),
            // Overflow/underflow-safe magnitude (CPython `_Py_c_abs` uses `hypot`).
            NumericSlot::Complex(re, im) => Value::float(re.hypot(im)),
        })
    }

    fn nb_bool(&self) -> bool {
        match *self {
            NumericSlot::Int(n) => n != 0,
            NumericSlot::Bool(b) => b,
            NumericSlot::Float(f) => f != 0.0,
            NumericSlot::BigInt(b) => !b.is_zero(),
            NumericSlot::Complex(re, im) => re != 0.0 || im != 0.0,
        }
    }

    fn nb_compare(&self, op: NumericCmpOp, rhs: &Value) -> Option<Result<bool>> {
        let r = NumericSlot::from_value(rhs)?;
        Some(match op {
            NumericCmpOp::Eq => Ok(self.eq_slot(&r)),
            NumericCmpOp::Ne => Ok(!self.eq_slot(&r)),
            NumericCmpOp::Lt | NumericCmpOp::Le | NumericCmpOp::Gt | NumericCmpOp::Ge => {
                // NaN is unordered: all of <, <=, >, >= evaluate to False.
                self.partial_cmp_slot(op, &r)
                    .map(|opt| opt.is_some_and(|ord| op.matches_ordering(ord)))
            }
        })
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

/// Exact numeric equality across all five built-in numeric kinds, or `None`
/// when either operand is non-numeric.
#[inline]
pub fn numeric_eq(lhs: &Value, rhs: &Value) -> Option<bool> {
    let l = NumericSlot::from_value(lhs)?;
    let r = NumericSlot::from_value(rhs)?;
    Some(l.eq_slot(&r))
}

/// Exact partial ordering across built-in numeric kinds for `sorted()` /
/// `min()` / `max()` and comparison dispatch.
///
/// Returns:
/// - `None` when either operand is non-numeric;
/// - `Some(Err(TypeError))` when either operand is `complex`;
/// - `Some(Ok(None))` when either operand is `NaN`;
/// - `Some(Ok(Some(ord)))` for ordered pairs.
#[inline]
pub fn numeric_partial_cmp(
    lhs: &Value,
    rhs: &Value,
    op_name: &str,
) -> Option<Result<Option<Ordering>>> {
    let l = NumericSlot::from_value(lhs)?;
    let r = NumericSlot::from_value(rhs)?;
    if l.is_complex() || r.is_complex() {
        return Some(Err(crate::type_err!(
            "'{op_name}' not supported between instances of '{}' and '{}'",
            l.type_name(),
            r.type_name()
        )));
    }
    Some(l.partial_cmp_slot(NumericCmpOp::Lt, &r))
}

// ── Canonical numeric helper functions ───────────────────────────────────────

/// Fallible `BigInt -> f64`. Raises `OverflowError` (`int too large to convert
/// to float`) when the `BigInt`'s magnitude exceeds `f64`'s finite range.
pub fn bigint_to_float_or_overflow(b: &PyBigInt) -> Result<f64> {
    b.to_f64()
        .filter(|f| f.is_finite())
        .ok_or_else(|| crate::overflow_err!("int too large to convert to float"))
}

/// Python-style modulo for two `i64` values (`b != 0`), where the result has
/// the same sign as the divisor `b`. Handles `i64::MIN % -1 == 0` without panic.
#[inline]
pub fn py_mod_i64(a: i64, b: i64) -> i64 {
    let mut remainder = a.wrapping_rem(b);
    if (remainder > 0 && b < 0) || (remainder < 0 && b > 0) {
        remainder += b;
    }
    remainder
}

/// Port of CPython's `float_divmod` (`Objects/floatobject.c`).
///
/// Returns `(floordiv, mod)` for non-zero divisor `b`, using `fmod` so that
/// infinities and signed zeros propagate identically to CPython. Caller must
/// guard `b == 0.0`.
pub fn float_divmod(a: f64, b: f64) -> (f64, f64) {
    let mut mod_ = a % b;
    let mut div = (a - mod_) / b;
    if mod_ != 0.0 {
        if (b < 0.0) != (mod_ < 0.0) {
            mod_ += b;
            div -= 1.0;
        }
    } else {
        mod_ = 0.0_f64.copysign(b);
    }
    let floordiv = if div != 0.0 {
        let fl = div.floor();
        if div - fl > 0.5 { fl + 1.0 } else { fl }
    } else {
        0.0_f64.copysign(a / b)
    };
    (floordiv, mod_)
}

/// Python-style floor `(quotient, remainder)` for two `PyBigInt`s (`b != 0`).
pub fn bigint_divmod_floor(a: &PyBigInt, b: &PyBigInt) -> (PyBigInt, PyBigInt) {
    let mut q = a / b;
    let mut r = a % b;
    if !r.is_zero() && (r.sign() != b.sign()) {
        q -= 1;
        r += b;
    }
    (q, r)
}

/// Normalize a `PyBigInt` to `Value::int` when it fits in `i64`, otherwise
/// `Value::bigint`.
#[inline]
pub fn value_from_bigint(n: PyBigInt) -> Value {
    match n.to_i64() {
        Some(i) => Value::int(i),
        None => Value::bigint(n),
    }
}

/// Coerce `Int`, `BigInt`, or `Bool` to `PyBigInt`, returning `None` otherwise.
pub fn value_to_bigint(v: &Value) -> Option<PyBigInt> {
    match v.kind() {
        ValueKind::Int(n) => Some(PyBigInt::from(n)),
        ValueKind::BigInt(b) => Some(b.clone()),
        ValueKind::Bool(b) => Some(PyBigInt::from(b as i64)),
        _ => None,
    }
}

/// `a ** b` for non-negative `i64` exponent, promoting to `BigInt` on overflow.
pub fn int_pow_promoting(a: i64, b: i64) -> Value {
    debug_assert!(b >= 0, "int_pow_promoting: caller must guard b < 0");
    let exp = match u32::try_from(b) {
        Ok(e) => e,
        Err(_) => return value_from_bigint(PyPow::pow(PyBigInt::from(a), b as u64)),
    };
    match a.checked_pow(exp) {
        Some(r) => Value::int(r),
        None => Value::bigint(PyPow::pow(PyBigInt::from(a), exp)),
    }
}

/// Exact equality between an `i64` integer and an `f64` float (CPython parity
/// beyond 2^53).
#[inline]
pub fn int_float_eq(i: i64, f: f64) -> bool {
    if !f.is_finite() || f != f.trunc() {
        return false;
    }
    const I64_MAX_PLUS_ONE: f64 = 9_223_372_036_854_775_808.0_f64;
    if f < (i64::MIN as f64) || f >= I64_MAX_PLUS_ONE {
        return false;
    }
    (f as i64) == i
}

/// Exact equality between a `PyBigInt` and an `f64` float.
#[inline]
pub fn bigint_float_eq(big: &PyBigInt, f: f64) -> bool {
    if !f.is_finite() || f != f.trunc() {
        return false;
    }
    match PyBigInt::from_f64(f) {
        Some(f_as_bigint) => f_as_bigint == *big,
        None => false,
    }
}

/// Exact ordering between an `i64` integer and an `f64` float. Returns `None`
/// for `NaN`.
pub fn int_float_cmp(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    const I64_MAX_PLUS_ONE: f64 = 9_223_372_036_854_775_808.0_f64; // 2^63
    if f >= I64_MAX_PLUS_ONE {
        return Some(Ordering::Less);
    }
    if f < (i64::MIN as f64) {
        return Some(Ordering::Greater);
    }
    let trunc = f.trunc();
    let trunc_i = trunc as i64;
    let base = i.cmp(&trunc_i);
    if base != Ordering::Equal || f == trunc {
        Some(base)
    } else if f > 0.0 {
        Some(Ordering::Less)
    } else {
        Some(Ordering::Greater)
    }
}

/// Exact ordering between a `PyBigInt` and an `f64` float. Returns `None` for
/// `NaN`.
pub fn bigint_float_cmp(big: &PyBigInt, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_finite() && f == f.trunc() {
        return PyBigInt::from_f64(f).map(|fi| big.cmp(&fi));
    }
    if f.is_infinite() {
        return if f > 0.0 {
            Some(Ordering::Less)
        } else {
            Some(Ordering::Greater)
        };
    }
    let trunc = f.trunc();
    let base = PyBigInt::from_f64(trunc)
        .map(|ti| big.cmp(&ti))
        .unwrap_or(Ordering::Less);
    if base != Ordering::Equal {
        return Some(base);
    }
    if f > 0.0 {
        Some(Ordering::Less)
    } else {
        Some(Ordering::Greater)
    }
}

// ── Internal numeric algorithms ──────────────────────────────────────────────

fn magnitude(b: &PyBigInt) -> PyBigInt {
    if b.sign() == PyBigIntSign::Minus {
        -b
    } else {
        b.clone()
    }
}

/// Complex division mirroring CPython's `_Py_c_quot` (Smith's scaling formula
/// in `Objects/complexobject.c`) so large/tiny magnitudes don't spuriously
/// overflow/underflow `br² + bi²`.
fn complex_div(a: (f64, f64), b: (f64, f64)) -> Result<Value> {
    let abs_br = b.0.abs();
    let abs_bi = b.1.abs();
    if abs_br >= abs_bi {
        if abs_br == 0.0 {
            return Err(crate::zerodiv_err!("complex division by zero"));
        }
        let ratio = b.1 / b.0;
        let denom = b.0 + b.1 * ratio;
        Ok(Value::complex(
            (a.0 + a.1 * ratio) / denom,
            (a.1 - a.0 * ratio) / denom,
        ))
    } else {
        let ratio = b.0 / b.1;
        let denom = b.0 * ratio + b.1;
        Ok(Value::complex(
            (a.0 * ratio + a.1) / denom,
            (a.1 * ratio - a.0) / denom,
        ))
    }
}

/// Complex exponentiation `(zr + zi*j) ** (wr + wi*j)` mirroring CPython's
/// `_Py_c_pow` (`Objects/complexobject.c`).
fn complex_pow(zr: f64, zi: f64, wr: f64, wi: f64) -> Result<Value> {
    // z^0 = 1 for any z (including 0j ** 0).
    if wr == 0.0 && wi == 0.0 {
        return Ok(Value::complex(1.0, 0.0));
    }

    let abs_r = zr.hypot(zi);
    if abs_r == 0.0 {
        if wi != 0.0 || wr < 0.0 {
            return Err(crate::zerodiv_err!("0.0 to a negative or complex power"));
        }
        return Ok(Value::complex(0.0, 0.0));
    }

    // Small integer exponents (wi == 0, |wr| <= 100): repeated squaring for
    // exact results such as `(1+1j)**2 == 2j`.
    if wi == 0.0 {
        let n = wr as i64;
        if n as f64 == wr && (-100..=100).contains(&n) {
            let (mut re, mut im) = (1.0_f64, 0.0_f64);
            let (mut br, mut bi) = (zr, zi);
            let mut exp = n.unsigned_abs();
            while exp > 0 {
                if exp & 1 == 1 {
                    let new_re = re * br - im * bi;
                    let new_im = re * bi + im * br;
                    re = new_re;
                    im = new_im;
                }
                let new_br = br * br - bi * bi;
                let new_bi = 2.0 * br * bi;
                br = new_br;
                bi = new_bi;
                exp >>= 1;
            }
            if n < 0 {
                let denom = re * re + im * im;
                let old_re = re;
                re = (1.0 * old_re + 0.0 * im) / denom;
                im = (0.0 * old_re - 1.0 * im) / denom;
            }
            return Ok(Value::complex(re, im));
        }
    }

    let t = zi.atan2(zr);
    let mut len = abs_r.powf(wr);
    let mut phase = t * wr;
    if wi != 0.0 {
        len /= (t * wi).exp();
        phase += wi * abs_r.ln();
    }
    if len.is_infinite() {
        return Err(crate::overflow_err!("complex exponentiation"));
    }
    Ok(Value::complex(len * phase.cos(), len * phase.sin()))
}

/// Correctly-rounded `int / int` true division, mirroring CPython's
/// `long_true_divide` (`Objects/longobject.c`, issue #1923).
fn bigint_true_divide(num: &PyBigInt, den: &PyBigInt) -> Result<f64> {
    const MANT_DIG: i64 = 53;
    const MIN_EXP: i64 = -1021;
    const MAX_EXP: i64 = 1024;
    const SUBNORMAL_LSB: i64 = MIN_EXP - MANT_DIG;

    if den.is_zero() {
        return Err(crate::zerodiv_err!("division by zero"));
    }
    let negate = (num.sign() == PyBigIntSign::Minus) != (den.sign() == PyBigIntSign::Minus);
    if num.is_zero() {
        return Ok(if negate { -0.0 } else { 0.0 });
    }
    let a = magnitude(num);
    let b = magnitude(den);
    let diff = a.bits() as i64 - b.bits() as i64;

    if diff > MAX_EXP {
        return Err(crate::overflow_err!(
            "integer division result too large for a float"
        ));
    }
    if diff < SUBNORMAL_LSB - 1 {
        return Ok(if negate { -0.0 } else { 0.0 });
    }

    let l0 = ((diff - 1) - (MANT_DIG - 1)).max(SUBNORMAL_LSB);
    let mut shift = l0 - 2;
    let (mut x, rem) = if shift <= 0 {
        let scaled = &a << ((-shift) as usize);
        (&scaled / &b, &scaled % &b)
    } else {
        let scaled_b = &b << (shift as usize);
        (&a / &scaled_b, &a % &scaled_b)
    };
    let inexact = !rem.is_zero();
    if x.is_zero() {
        return Ok(if negate { -0.0 } else { 0.0 });
    }

    let e = (x.bits() as i64 - 1) + shift;
    let l = (e - (MANT_DIG - 1)).max(SUBNORMAL_LSB);
    let drop = (l - shift) as usize;
    let low = &x & ((PyBigInt::from(1) << drop) - 1);
    x >>= drop;
    let half = PyBigInt::from(1) << (drop - 1);
    let round_up =
        low > half || (low == half && ((&x & PyBigInt::from(1)) == PyBigInt::from(1) || inexact));
    if round_up {
        x += 1;
    }
    shift = l;

    let mantissa = x
        .to_f64()
        .filter(|f| f.is_finite())
        .ok_or_else(|| crate::overflow_err!("integer division result too large for a float"))?;
    let result = ldexp_f64(mantissa, shift);
    if !result.is_finite() {
        return Err(crate::overflow_err!(
            "integer division result too large for a float"
        ));
    }
    Ok(if negate { -result } else { result })
}

fn ldexp_f64(mut m: f64, mut exp: i64) -> f64 {
    if m == 0.0 || !m.is_finite() {
        return m;
    }
    const STEP: i64 = 512;
    const STEP_FACTOR: f64 = 1.340_780_792_994_259_7e154; // 2^512
    while exp > STEP {
        m *= STEP_FACTOR;
        exp -= STEP;
    }
    while exp < -STEP {
        m /= STEP_FACTOR;
        exp += STEP;
    }
    m * 2.0_f64.powi(exp as i32)
}

enum ShiftCount {
    Fits(usize),
    Saturated,
}

const MAX_SHIFT: usize = 1 << 30;

fn shift_count(slot: &NumericSlot<'_>) -> Result<ShiftCount> {
    match *slot {
        NumericSlot::Int(n) => {
            if n < 0 {
                Err(crate::value_err!("negative shift count"))
            } else {
                Ok(usize::try_from(n).map_or(ShiftCount::Saturated, ShiftCount::Fits))
            }
        }
        NumericSlot::Bool(b) => Ok(ShiftCount::Fits(b as usize)),
        NumericSlot::BigInt(big) => match big.sign() {
            PyBigIntSign::Minus => Err(crate::value_err!("negative shift count")),
            PyBigIntSign::NoSign => Ok(ShiftCount::Fits(0)),
            PyBigIntSign::Plus => Ok(big
                .to_usize()
                .map_or(ShiftCount::Saturated, ShiftCount::Fits)),
        },
        NumericSlot::Float(_) | NumericSlot::Complex(_, _) => {
            unreachable!("shift_count called on Float/Complex")
        }
    }
}

fn slot_lshift(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    let a = lhs.to_bigint();
    match shift_count(rhs)? {
        ShiftCount::Fits(n) => {
            if n > MAX_SHIFT && !a.is_zero() {
                return Err(crate::overflow_err!("too many digits in integer"));
            }
            Ok(value_from_bigint(a << n))
        }
        ShiftCount::Saturated => {
            if a.is_zero() {
                Ok(Value::int(0))
            } else {
                Err(crate::overflow_err!("too many digits in integer"))
            }
        }
    }
}

fn slot_rshift(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    let a = lhs.to_bigint();
    match shift_count(rhs)? {
        ShiftCount::Fits(n) => Ok(value_from_bigint(a >> n)),
        ShiftCount::Saturated => Ok(Value::int(if a.sign() == PyBigIntSign::Minus {
            -1
        } else {
            0
        })),
    }
}

#[cfg(test)]
mod tests {
    use crate::errors::PyError;
    use crate::numeric_ops::{
        NumericBinOp, NumericCmpOp, NumericOps, NumericSlot, NumericUnaryOp,
        bigint_to_float_or_overflow, dispatch_numeric_binop, dispatch_numeric_bool,
        dispatch_numeric_compare, dispatch_numeric_unary,
    };
    use crate::object_model::{PyBigInt, Value, ValueKind};

    fn assert_named_err<T: std::fmt::Debug>(res: crate::errors::Result<T>, expected_type: &str) {
        match res {
            Err(PyError::Named(cls, _)) => assert_eq!(cls.as_ref(), expected_type),
            other => panic!("expected {expected_type}, got {other:?}"),
        }
    }

    #[test]
    fn binary_arithmetic_across_all_five_numeric_types() {
        let big = Value::bigint(PyBigInt::from(1i64) << 70);
        let one_j = Value::complex(0.0, 1.0);
        let c = Value::complex(1.0, 2.0);

        // Int overflow promotion to BigInt.
        let overflow_add =
            dispatch_numeric_binop(NumericBinOp::Add, &Value::int(i64::MAX), &Value::int(1))
                .unwrap()
                .unwrap();
        assert_eq!(overflow_add, Value::bigint(PyBigInt::from(i64::MAX) + 1));

        let overflow_sub =
            dispatch_numeric_binop(NumericBinOp::Sub, &Value::int(i64::MIN), &Value::int(1))
                .unwrap()
                .unwrap();
        assert_eq!(overflow_sub, Value::bigint(PyBigInt::from(i64::MIN) - 1));

        let overflow_mul =
            dispatch_numeric_binop(NumericBinOp::Mul, &Value::int(i64::MAX), &Value::int(2))
                .unwrap()
                .unwrap();
        assert_eq!(overflow_mul, Value::bigint(PyBigInt::from(i64::MAX) * 2));

        let overflow_fdiv = dispatch_numeric_binop(
            NumericBinOp::FloorDiv,
            &Value::int(i64::MIN),
            &Value::int(-1),
        )
        .unwrap()
        .unwrap();
        assert_eq!(overflow_fdiv, Value::bigint(-PyBigInt::from(i64::MIN)));

        // Bool arithmetic yields Int / Float / Complex.
        let bt =
            dispatch_numeric_binop(NumericBinOp::Add, &Value::bool_(true), &Value::bool_(true))
                .unwrap()
                .unwrap();
        assert!(matches!(bt.kind(), ValueKind::Int(2)));

        let bf = dispatch_numeric_binop(NumericBinOp::Add, &Value::bool_(true), &Value::float(2.5))
            .unwrap()
            .unwrap();
        assert_eq!(bf, Value::float(3.5));

        let bc = dispatch_numeric_binop(NumericBinOp::Add, &Value::bool_(true), &one_j)
            .unwrap()
            .unwrap();
        assert_eq!(bc, Value::complex(1.0, 1.0));

        // Complex arithmetic (+, -, *, /, **) with all 5 numeric kinds.
        assert_eq!(
            dispatch_numeric_binop(NumericBinOp::Add, &c, &Value::int(3))
                .unwrap()
                .unwrap(),
            Value::complex(4.0, 2.0)
        );
        assert_eq!(
            dispatch_numeric_binop(NumericBinOp::Sub, &Value::float(5.0), &c)
                .unwrap()
                .unwrap(),
            Value::complex(4.0, -2.0)
        );
        assert_eq!(
            dispatch_numeric_binop(NumericBinOp::Mul, &c, &Value::complex(3.0, -4.0))
                .unwrap()
                .unwrap(),
            Value::complex(11.0, 2.0)
        );
        assert_eq!(
            dispatch_numeric_binop(
                NumericBinOp::Div,
                &Value::complex(0.0, 1e200),
                &Value::complex(0.0, 1e200),
            )
            .unwrap()
            .unwrap(),
            Value::complex(1.0, 0.0)
        );
        assert_named_err(
            dispatch_numeric_binop(NumericBinOp::Div, &c, &Value::complex(0.0, 0.0)).unwrap(),
            "ZeroDivisionError",
        );

        // BigInt + Complex and BigInt overflow -> OverflowError.
        let big_c = dispatch_numeric_binop(NumericBinOp::Add, &big, &one_j)
            .unwrap()
            .unwrap();
        assert_eq!(big_c, Value::complex((1u128 << 70) as f64, 1.0));

        let huge = Value::bigint(PyBigInt::from(10i64).pow(400));
        assert_named_err(
            dispatch_numeric_binop(NumericBinOp::Add, &huge, &one_j).unwrap(),
            "OverflowError",
        );
        assert_named_err(
            dispatch_numeric_binop(NumericBinOp::Add, &huge, &Value::float(1.0)).unwrap(),
            "OverflowError",
        );

        // Non-numeric RHS returns None (NotImplemented).
        assert!(
            dispatch_numeric_binop(NumericBinOp::Mul, &Value::int(3), &Value::string("ab"))
                .is_none()
        );
        assert!(dispatch_numeric_binop(NumericBinOp::Add, &one_j, &Value::string("x")).is_none());
    }

    #[test]
    fn pow_and_complex_pow_edge_cases() {
        // 0j ** 0 == 1+0j, 0j ** -1 -> ZeroDivisionError, 0j ** 1j -> ZeroDivisionError.
        assert_eq!(
            dispatch_numeric_binop(NumericBinOp::Pow, &Value::complex(0.0, 0.0), &Value::int(0))
                .unwrap()
                .unwrap(),
            Value::complex(1.0, 0.0)
        );
        assert_named_err(
            dispatch_numeric_binop(
                NumericBinOp::Pow,
                &Value::complex(0.0, 0.0),
                &Value::int(-1),
            )
            .unwrap(),
            "ZeroDivisionError",
        );
        assert_named_err(
            dispatch_numeric_binop(
                NumericBinOp::Pow,
                &Value::complex(0.0, 0.0),
                &Value::complex(0.0, 1.0),
            )
            .unwrap(),
            "ZeroDivisionError",
        );
        // Exact repeated squaring for small integer exponents.
        assert_eq!(
            dispatch_numeric_binop(NumericBinOp::Pow, &Value::complex(1.0, 1.0), &Value::int(2),)
                .unwrap()
                .unwrap(),
            Value::complex(0.0, 2.0)
        );
        // Negative float to fractional power promotes to complex; to NaN stays float NaN.
        let neg_nan = dispatch_numeric_binop(
            NumericBinOp::Pow,
            &Value::float(-2.0),
            &Value::float(f64::NAN),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(neg_nan.kind(), ValueKind::Float(f) if f.is_nan()));

        // Int overflow promotion on **.
        let p64 = dispatch_numeric_binop(NumericBinOp::Pow, &Value::int(2), &Value::int(64))
            .unwrap()
            .unwrap();
        assert_eq!(p64, Value::bigint(PyBigInt::from(1i64) << 64));
    }

    #[test]
    fn bitwise_shifts_and_unsupported_numeric_type_errors() {
        // bool bitwise preserves bool; mixed bool/int returns int.
        let bb = dispatch_numeric_binop(
            NumericBinOp::BitAnd,
            &Value::bool_(true),
            &Value::bool_(false),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(bb.kind(), ValueKind::Bool(false)));
        let bi = dispatch_numeric_binop(NumericBinOp::BitOr, &Value::bool_(true), &Value::int(2))
            .unwrap()
            .unwrap();
        assert!(matches!(bi.kind(), ValueKind::Int(3)));
        let bx = dispatch_numeric_binop(
            NumericBinOp::BitXor,
            &Value::bool_(true),
            &Value::bool_(true),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(bx.kind(), ValueKind::Bool(false)));

        // Shifts with BigInt promotion and negative shift count ValueError.
        let shl = dispatch_numeric_binop(NumericBinOp::LShift, &Value::int(1), &Value::int(80))
            .unwrap()
            .unwrap();
        assert_eq!(shl, Value::bigint(PyBigInt::from(1i64) << 80));
        assert_named_err(
            dispatch_numeric_binop(NumericBinOp::RShift, &Value::int(8), &Value::int(-1)).unwrap(),
            "ValueError",
        );

        // Complex floor-div, mod, bitwise, shift, and Float bitwise/shift -> TypeError.
        let c = Value::complex(1.0, 2.0);
        for op in [
            NumericBinOp::FloorDiv,
            NumericBinOp::Mod,
            NumericBinOp::BitAnd,
            NumericBinOp::BitOr,
            NumericBinOp::BitXor,
            NumericBinOp::LShift,
            NumericBinOp::RShift,
        ] {
            assert_named_err(
                dispatch_numeric_binop(op, &c, &Value::int(2)).unwrap(),
                "TypeError",
            );
            assert_named_err(
                dispatch_numeric_binop(op, &Value::bool_(true), &c).unwrap(),
                "TypeError",
            );
        }
        for op in [
            NumericBinOp::BitAnd,
            NumericBinOp::BitOr,
            NumericBinOp::BitXor,
            NumericBinOp::LShift,
            NumericBinOp::RShift,
        ] {
            assert_named_err(
                dispatch_numeric_binop(op, &Value::float(1.5), &Value::int(1)).unwrap(),
                "TypeError",
            );
        }
    }

    #[test]
    fn unary_and_bool_slots_cover_all_five_types() {
        // Negation incl. i64::MIN promotion.
        assert_eq!(
            dispatch_numeric_unary(NumericUnaryOp::Neg, &Value::int(i64::MIN))
                .unwrap()
                .unwrap(),
            Value::bigint(-PyBigInt::from(i64::MIN))
        );
        assert_eq!(
            dispatch_numeric_unary(NumericUnaryOp::Neg, &Value::bool_(true))
                .unwrap()
                .unwrap(),
            Value::int(-1)
        );
        assert_eq!(
            dispatch_numeric_unary(NumericUnaryOp::Neg, &Value::complex(1.0, -2.0))
                .unwrap()
                .unwrap(),
            Value::complex(-1.0, 2.0)
        );

        // Positive preserves bool -> int coercion and NaN float identity (#2911).
        let pos_true = dispatch_numeric_unary(NumericUnaryOp::Pos, &Value::bool_(true))
            .unwrap()
            .unwrap();
        assert!(matches!(pos_true.kind(), ValueKind::Int(1)));
        let nan = Value::float(f64::NAN);
        let pos_nan = dispatch_numeric_unary(NumericUnaryOp::Pos, &nan)
            .unwrap()
            .unwrap();
        assert!(pos_nan.is_identical_to(&nan));

        // Invert works on int/bool/bigint and raises TypeError on float/complex.
        assert_eq!(
            dispatch_numeric_unary(NumericUnaryOp::Invert, &Value::bool_(true))
                .unwrap()
                .unwrap(),
            Value::int(-2)
        );
        assert_named_err(
            dispatch_numeric_unary(NumericUnaryOp::Invert, &Value::float(1.5)).unwrap(),
            "TypeError",
        );
        assert_named_err(
            dispatch_numeric_unary(NumericUnaryOp::Invert, &Value::complex(1.0, 2.0)).unwrap(),
            "TypeError",
        );

        // Absolute value incl. i64::MIN promotion, bool -> int, complex hypot.
        assert_eq!(
            dispatch_numeric_unary(NumericUnaryOp::Abs, &Value::int(i64::MIN))
                .unwrap()
                .unwrap(),
            Value::bigint(-PyBigInt::from(i64::MIN))
        );
        let abs_true = dispatch_numeric_unary(NumericUnaryOp::Abs, &Value::bool_(true))
            .unwrap()
            .unwrap();
        assert!(matches!(abs_true.kind(), ValueKind::Int(1)));
        assert_eq!(
            dispatch_numeric_unary(NumericUnaryOp::Abs, &Value::complex(3.0, 4.0))
                .unwrap()
                .unwrap(),
            Value::float(5.0)
        );

        // nb_bool across all 5 types.
        assert_eq!(dispatch_numeric_bool(&Value::int(0)), Some(false));
        assert_eq!(dispatch_numeric_bool(&Value::int(7)), Some(true));
        assert_eq!(dispatch_numeric_bool(&Value::bool_(false)), Some(false));
        assert_eq!(dispatch_numeric_bool(&Value::float(-0.0)), Some(false));
        assert_eq!(dispatch_numeric_bool(&Value::float(f64::NAN)), Some(true));
        assert_eq!(
            dispatch_numeric_bool(&Value::bigint(PyBigInt::from(1i64) << 70)),
            Some(true)
        );
        assert_eq!(
            dispatch_numeric_bool(&Value::complex(0.0, -0.0)),
            Some(false)
        );
        assert_eq!(dispatch_numeric_bool(&Value::complex(0.0, 1.0)), Some(true));
        assert_eq!(dispatch_numeric_bool(&Value::string("hi")), None);
    }

    #[test]
    fn compare_slot_precision_nan_signed_zero_and_complex_ordering() {
        // Exact int == float and int == complex precision beyond 2^53.
        let i_big = (1i64 << 53) + 1;
        let f_rounded = i_big as f64;
        assert!(
            !dispatch_numeric_compare(
                NumericCmpOp::Eq,
                &Value::int(i_big),
                &Value::float(f_rounded),
            )
            .unwrap()
            .unwrap()
        );
        assert!(
            dispatch_numeric_compare(
                NumericCmpOp::Gt,
                &Value::int(i_big),
                &Value::float(f_rounded),
            )
            .unwrap()
            .unwrap()
        );
        assert!(
            !dispatch_numeric_compare(
                NumericCmpOp::Eq,
                &Value::int(i_big),
                &Value::complex(f_rounded, 0.0),
            )
            .unwrap()
            .unwrap()
        );

        // BigInt vs complex exact precision.
        let pow2_100 = Value::bigint(PyBigInt::from(1i64) << 100);
        let f_pow2_100 = 2.0_f64.powi(100);
        assert!(
            dispatch_numeric_compare(
                NumericCmpOp::Eq,
                &pow2_100,
                &Value::complex(f_pow2_100, 0.0),
            )
            .unwrap()
            .unwrap()
        );
        let ten_30 = Value::bigint(PyBigInt::from(10i64).pow(30));
        assert!(
            !dispatch_numeric_compare(NumericCmpOp::Eq, &ten_30, &Value::complex(1e30, 0.0))
                .unwrap()
                .unwrap()
        );

        // Bool == Complex and -0.0 == 0.0 == 0j.
        assert!(
            dispatch_numeric_compare(
                NumericCmpOp::Eq,
                &Value::bool_(true),
                &Value::complex(1.0, 0.0),
            )
            .unwrap()
            .unwrap()
        );
        assert!(
            dispatch_numeric_compare(
                NumericCmpOp::Eq,
                &Value::float(-0.0),
                &Value::complex(0.0, -0.0),
            )
            .unwrap()
            .unwrap()
        );

        // NaN comparisons.
        let nan = Value::float(f64::NAN);
        assert!(
            !dispatch_numeric_compare(NumericCmpOp::Eq, &nan, &nan)
                .unwrap()
                .unwrap()
        );
        assert!(
            dispatch_numeric_compare(NumericCmpOp::Ne, &nan, &nan)
                .unwrap()
                .unwrap()
        );
        for op in [
            NumericCmpOp::Lt,
            NumericCmpOp::Le,
            NumericCmpOp::Gt,
            NumericCmpOp::Ge,
        ] {
            assert!(
                !dispatch_numeric_compare(op, &nan, &Value::int(0))
                    .unwrap()
                    .unwrap()
            );
        }

        // Complex ordering raises TypeError across all four order operators.
        let c = Value::complex(1.0, 0.0);
        for op in [
            NumericCmpOp::Lt,
            NumericCmpOp::Le,
            NumericCmpOp::Gt,
            NumericCmpOp::Ge,
        ] {
            assert_named_err(
                NumericSlot::Complex(1.0, 0.0)
                    .nb_compare(op, &Value::int(2))
                    .unwrap(),
                "TypeError",
            );
            assert_named_err(
                dispatch_numeric_compare(op, &Value::int(2), &c).unwrap(),
                "TypeError",
            );
        }
    }

    #[test]
    fn bigint_to_float_or_overflow_bounds() {
        let small = PyBigInt::from(1i64) << 100;
        assert_eq!(
            bigint_to_float_or_overflow(&small).unwrap(),
            2.0_f64.powi(100)
        );

        let huge = PyBigInt::from(10i64).pow(400);
        assert_named_err(bigint_to_float_or_overflow(&huge), "OverflowError");
    }
}
